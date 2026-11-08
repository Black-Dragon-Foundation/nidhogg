//! The backend: `NSApplication`, the windows, and the queue of records the glue drains.
//!
//! The queue's wake descriptor is a pipe that holds one byte while records wait.  Calls with no AppKit counterpart
//! (client-side decorations, reconnecting, sessions, xdg-foreign, activation tokens, cursor themes) are accepted and
//! do nothing; the features later work brings (children, selections, drag and drop, text input, the presenter) answer
//! `Unsupported` until then.

use super::defaults;
use super::kq::{self, Kq};
use super::layers;
use super::runloop::{MacEv, SCOPE};
use super::screens;
use super::surface;
use super::window::Win;
use crate::api::{
    Abandon, ChildDesc, NHSHELL_WIN_UNDECORATED, NhShellPresenterStats, PresenterTiming,
    ShellBackend, ShellError, ShellResult, SurfaceFrame, WindowDesc, WindowId,
};
use crate::caps::NhShellCaps;
use crate::event::{ConfigureEv, FocusEv, NHSHELL_STATE_ACTIVATED, Record, ScaleEv, ShellEvent};
use crate::scope::{Scope, ScopeStats};
use crate::wait::{self, Change, Reg, WaitOps, Woke};
use glitnir::geom::Rect;
use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy, NSEventMask, NSScreen};
use objc2_foundation::{NSDate, NSDefaultRunLoopMode, NSString};
use std::collections::{BTreeSet, HashMap, VecDeque};
use std::time::Duration;

/// The test hooks: `nhmac--call` and `nhmac--query` kinds that exist only in a checking build.
#[cfg(feature = "test-hooks")]
#[path = "hooks.rs"]
mod hooks;

pub struct MacShell {
    mtm: MainThreadMarker,
    app: Retained<NSApplication>,
    windows: HashMap<WindowId, Win>,
    next_id: WindowId,
    serial: u32,
    queue: VecDeque<Record>,
    /// The wake pipe: [read, write].
    wake: [i32; 2],
    /// The main wait's kqueue (`None`: kqueue refused, so `nhg_wait` is unsupported) and its registrations.
    kq: Option<Kq>,
    regs: wait::Regs,
    /// `ns-use-srgb-colorspace`: the colour space new surfaces are tagged with.
    srgb: bool,
    /// Windows a Lisp thread's redisplay filled whose commit waits for the main thread's next pump.
    deferred: Vec<WindowId>,
    /// Test hook: the next pump raises an `NSException` inside the catch.
    #[cfg(feature = "test-hooks")]
    raise_in_pump: bool,
    /// Test hook: how many of the next new registrations go to the helper.
    #[cfg(feature = "test-hooks")]
    force_helper: u32,
}

/// The counters `(nhmac-stats)` returns: the scope's, the main wait's (with the lock hand-off's), the surfaces'.
fn stats_text(s: ScopeStats) -> String {
    format!(
        "(:takes {} :overlapping-takes {} :queued {} :applied {} {} {})",
        s.takes,
        s.overlapping_takes,
        s.queued,
        s.applied,
        wait::STATS.text(&wait::PARK),
        surface::STATS.text()
    )
}

/// The scope's counters now.
fn scope_stats() -> ScopeStats {
    SCOPE.with(Scope::stats)
}

/// Every event AppKit has queued, through `sendEvent:`; the wait's markers are dropped.
fn drain_events(app: &NSApplication) {
    // SAFETY: an immutable string constant of AppKit's.
    let mode = unsafe { NSDefaultRunLoopMode };
    let past = NSDate::distantPast();
    while let Some(ev) = app.nextEventMatchingMask_untilDate_inMode_dequeue(
        NSEventMask::Any,
        Some(&past),
        mode,
        true,
    ) {
        if !kq::is_marker(&ev) {
            app.sendEvent(&ev);
        }
    }
}

fn now_ns() -> u64 {
    let mut ts = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: `ts` is a writable timespec.
    unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut ts) };
    ts.tv_sec as u64 * 1_000_000_000 + ts.tv_nsec as u64
}

impl MacShell {
    /// Open on the main thread (`State` anywhere else): a regular application, with its screens queued.
    pub fn open() -> ShellResult<MacShell> {
        let mtm = MainThreadMarker::new().ok_or(ShellError::State)?;
        let app = NSApplication::sharedApplication(mtm);
        app.setActivationPolicy(NSApplicationActivationPolicy::Regular);
        app.finishLaunching();
        let mut wake = [-1; 2];
        // SAFETY: `wake` holds two writable descriptors' room.
        if unsafe { libc::pipe(wake.as_mut_ptr()) } != 0 {
            return Err(ShellError::NoMem);
        }
        for fd in wake {
            // SAFETY: a descriptor we just made.
            unsafe {
                libc::fcntl(fd, libc::F_SETFL, libc::O_NONBLOCK);
                libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC);
            }
        }
        let mut shell = MacShell {
            mtm,
            app,
            windows: HashMap::new(),
            next_id: 1,
            serial: 0,
            queue: VecDeque::new(),
            wake,
            kq: Kq::open(mtm),
            regs: wait::Regs::default(),
            srgb: true,
            deferred: Vec::new(),
            #[cfg(feature = "test-hooks")]
            raise_in_pump: false,
            #[cfg(feature = "test-hooks")]
            force_helper: 0,
        };
        for o in screens::outputs(mtm) {
            shell.push(ShellEvent::OutputAdd(o), 0);
        }
        layers::watch_spins();
        Ok(shell)
    }

    fn push(&mut self, event: ShellEvent, window: WindowId) {
        if self.queue.is_empty() {
            // SAFETY: our pipe's write end; one byte, a full pipe (impossible with one byte) is harmless.
            unsafe { libc::write(self.wake[1], [1u8].as_ptr().cast(), 1) };
        }
        self.queue.push_back(Record::new(event, window, now_ns()));
    }

    fn win(&self, window: WindowId) -> ShellResult<&Win> {
        self.windows.get(&window).ok_or(ShellError::State)
    }

    /// Lend the backend to the scope while `f` makes AppKit calls that may call back; the events queued meanwhile
    /// are applied after.  `f` reaches the backend only through the scope.
    fn lent<R>(&mut self, f: impl FnOnce(&Scope<MacShell, MacEv>) -> R) -> R {
        let _pumping = layers::Pumping::enter();
        SCOPE.with(|s| s.lend(self, || f(s), |shell, e| shell.apply(e)))
    }

    /// A callback's event, on the backend.  A test window's id names no window here: its events change nothing.
    pub(super) fn apply(&mut self, ev: MacEv) {
        match ev {
            MacEv::Resized(id) => self.compare(id),
            MacEv::Key(id, on) => {
                if self.windows.contains_key(&id) {
                    self.push(ShellEvent::Focus(FocusEv { r#in: u8::from(on) }), id);
                }
            }
            MacEv::Input(id, ev) => {
                if self.windows.contains_key(&id) {
                    self.push(ev, id);
                }
            }
        }
    }

    /// Compare window `id`'s size and scale with what the host was told, and tell it what changed.
    fn compare(&mut self, id: WindowId) {
        let Some(w) = self.windows.get_mut(&id) else {
            return;
        };
        let (size, scale) = (w.content_size(), w.scale_now());
        let rescaled = scale != w.scale120;
        let resized = size != w.size;
        w.scale120 = scale;
        w.size = size;
        if rescaled {
            self.push(ShellEvent::Scale(ScaleEv { scale120: scale }), id);
        }
        if resized {
            self.configure(id, size.0, size.1);
        }
    }

    fn configure(&mut self, id: WindowId, w: i32, h: i32) {
        self.serial = self.serial.wrapping_add(1);
        let serial = self.serial;
        let queued = self.queue.len();
        crate::trace::note(|| {
            format!(
                "configure queued: window {id} {w}x{h} serial {serial}, {queued} records before it"
            )
        });
        self.push(
            ShellEvent::Configure(ConfigureEv {
                w,
                h,
                states: NHSHELL_STATE_ACTIVATED,
                serial,
            }),
            id,
        );
    }

    /// One pump: every queued AppKit event goes through `sendEvent:` with the backend lent to the scope, then the
    /// windows' sizes and scales are compared with what the host was told (a change no delegate reported).
    fn pump(&mut self) {
        #[cfg(feature = "test-hooks")]
        self.maybe_raise();
        if !self.deferred.is_empty() {
            let ids = std::mem::take(&mut self.deferred);
            self.commit(&ids);
        }
        let app = self.app.clone();
        self.lent(|_| {
            drain_events(&app);
            app.updateWindows();
        });
        let ids: Vec<WindowId> = self.windows.keys().copied().collect();
        for id in ids {
            self.compare(id);
        }
    }
}

/// Glitnir's damage of layer `g` since its last commit, committed, clipped to its `w` x `h` (device pixels).
fn layer_damage(g: u64, w: i32, h: i32) -> Vec<Rect> {
    let rects = match glitnir::damage::commit_snapshot(g) {
        Some((rects, _)) => rects,
        None => glitnir::damage::with(g, |d| d.commit().to_vec()),
    };
    let all = Rect::new(0, 0, w, h);
    rects
        .iter()
        .map(|r| r.intersect(&all))
        .filter(|r| !r.is_empty())
        .collect()
}

impl MacShell {
    /// Set the newest surfaces of `ids` as their layers' contents in one flushed transaction (main thread).
    fn commit(&mut self, ids: &[WindowId]) {
        let mut items = Vec::with_capacity(ids.len());
        for id in ids {
            let Some(w) = self.windows.get(id) else {
                continue;
            };
            if let Some(s) = w.surfaces.newest() {
                items.push((
                    &*w.layer,
                    s,
                    w.surfaces.was_same(),
                    w.window.backingScaleFactor(),
                ));
            }
        }
        layers::commit(&items);
        layers::publish_end();
    }

    /// A watched variable's new value (`nhmac--variable-changed`): ARG is `(SYMBOL VALUE)` as Lisp prints it.  Only
    /// `ns-use-srgb-colorspace` is read yet: every ring starts over, tagged the new way.  The rest are accepted.
    fn variable(&mut self, arg: &str) {
        let inner = arg.trim().trim_start_matches('(').trim_end_matches(')');
        let mut it = inner.split_whitespace();
        let (name, on) = (it.next(), it.next() != Some("nil"));
        let relaxed = std::sync::atomic::Ordering::Relaxed;
        match name {
            Some("ns-use-srgb-colorspace") => {
                self.srgb = on;
                super::draw::SRGB.store(self.srgb, relaxed);
                for w in self.windows.values_mut() {
                    w.surfaces.reset(self.srgb);
                }
            }
            Some("ns-antialias-text") => super::draw::ANTIALIAS.store(on, relaxed),
            Some("ns-use-thin-smoothing") => super::draw::THIN_SMOOTHING.store(on, relaxed),
            _ => {}
        }
    }
}

impl WaitOps for MacShell {
    fn kq_apply(&mut self, changes: &[Change], errs: &mut Vec<i32>) {
        match self.kq.as_mut() {
            Some(k) => k.apply(changes, errs),
            None => errs.resize(changes.len(), libc::EINVAL),
        }
    }

    fn kq_poll(&mut self, out: &mut Vec<Reg>) {
        if let Some(k) = self.kq.as_mut() {
            k.poll(out);
        }
    }

    /// The run loop with the backend lent, then a pump for whatever the sleep's event left queued.
    fn sleep(&mut self, helper: &[Reg], timeout: Option<Duration>) -> Woke {
        let Some(mut k) = self.kq.take() else {
            return Woke::Timeout;
        };
        let app = self.app.clone();
        let woke = self.lent(|_| k.sleep(&app, helper, timeout));
        self.kq = Some(k);
        if woke == Woke::Event {
            self.pump();
        }
        woke
    }

    fn key_fd(&self) -> i32 {
        self.wake[0]
    }

    #[cfg(feature = "test-hooks")]
    fn take_forced(&mut self) -> bool {
        let f = self.force_helper > 0;
        self.force_helper = self.force_helper.saturating_sub(1);
        f
    }
}

impl Drop for MacShell {
    fn drop(&mut self) {
        self.windows.clear();
        for fd in self.wake {
            // SAFETY: our own descriptors, closed once.
            unsafe { libc::close(fd) };
        }
    }
}

impl ShellBackend for MacShell {
    fn wake_fd(&self) -> i32 {
        self.wake[0]
    }

    fn caps(&self) -> NhShellCaps {
        NhShellCaps {
            ssd: true,
            overhang_panels: true,
            ..NhShellCaps::default()
        }
    }

    fn next_event(&mut self) -> ShellResult<Option<Record>> {
        let r = self.queue.pop_front();
        if self.queue.is_empty() {
            let mut buf = [0u8; 16];
            // SAFETY: our pipe's read end, non-blocking; `buf` is writable.
            while unsafe { libc::read(self.wake[0], buf.as_mut_ptr().cast(), buf.len()) } > 0 {}
        }
        Ok(r)
    }

    fn wait(
        &mut self,
        want: &BTreeSet<Reg>,
        timeout: Option<Duration>,
        out: &mut Vec<Reg>,
    ) -> ShellResult {
        if self.kq.is_none() {
            return Err(ShellError::Unsupported);
        }
        let mut regs = std::mem::take(&mut self.regs);
        wait::run(
            self,
            &mut regs,
            &wait::PARK,
            &wait::STATS,
            want,
            timeout,
            out,
        );
        self.regs = regs;
        Ok(())
    }

    fn dispatch_main(&mut self) -> ShellResult {
        // AppKit's events are the main thread's alone: a Lisp thread's call (its `read_socket`, from `maybe_quit`)
        // hands the pump to the main thread, whose wait wakes and pumps; the records already queued are its own
        if MainThreadMarker::new().is_none() {
            surface::STATS
                .handed_pumps
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            wait::wake();
            return Ok(());
        }
        // records already wait for the host: it drains them first, and the drain's end pumps when it needs more
        if !self.queue.is_empty() {
            return self.flush_main();
        }
        // an exception raised here is turned into a panic by the glue's guard (nhshell::catch_objc)
        surface::STATS
            .pumps
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.pump();
        Ok(())
    }

    fn flush_main(&mut self) -> ShellResult {
        // a Lisp thread's drain hands the pump over, as `dispatch_main` does
        if MainThreadMarker::new().is_none() {
            return self.dispatch_main();
        }
        if !self.deferred.is_empty() {
            let ids = std::mem::take(&mut self.deferred);
            self.commit(&ids);
        }
        Ok(())
    }

    fn window_new_named(
        &mut self,
        desc: &WindowDesc,
        _session_name: Option<&str>,
    ) -> ShellResult<WindowId> {
        let id = self.next_id;
        self.next_id += 1;
        let win = Win::open(
            self.mtm,
            id,
            desc.w,
            desc.h,
            desc.flags & NHSHELL_WIN_UNDECORATED != 0,
            &desc.title,
        );
        let (w, h, scale) = (win.size.0, win.size.1, win.scale120);
        crate::trace::note(|| format!("window made: window {id} {w}x{h} points, scale120 {scale}"));
        let mut win = win;
        win.surfaces.reset(self.srgb);
        layers::changed_outside();
        self.windows.insert(id, win);
        self.push(ShellEvent::Scale(ScaleEv { scale120: scale }), id);
        self.configure(id, w, h);
        Ok(id)
    }

    fn window_destroy(&mut self, window: WindowId) -> ShellResult {
        self.windows
            .remove(&window)
            .map(drop)
            .ok_or(ShellError::State)
    }

    fn scale_start(&mut self, window: WindowId, _scale120: u32) -> ShellResult {
        self.win(window).map(drop)
    }

    fn set_title(&mut self, window: WindowId, title: &str) -> ShellResult {
        self.win(window)?
            .window
            .setTitle(&NSString::from_str(title));
        layers::changed_outside();
        Ok(())
    }

    fn set_app_id(&mut self, window: WindowId, _app_id: &str) -> ShellResult {
        self.win(window).map(drop)
    }

    fn set_min_size(&mut self, window: WindowId, w: i32, h: i32) -> ShellResult {
        let size = objc2_foundation::NSSize::new(f64::from(w.max(1)), f64::from(h.max(1)));
        self.win(window)?.window.setContentMinSize(size);
        Ok(())
    }

    fn move_to(&mut self, window: WindowId, x: i32, y: i32) -> ShellResult {
        // NS's compute_offset: the frame's top-left, y up from the main screen's bottom
        let top = NSScreen::screens(self.mtm)
            .iter()
            .next()
            .map_or(0.0, |s| s.frame().origin.y + s.frame().size.height);
        let point = objc2_foundation::NSPoint::new(f64::from(x), top - f64::from(y));
        self.win(window)?.window.setFrameTopLeftPoint(point);
        layers::changed_outside();
        Ok(())
    }

    fn set_fullscreen(&mut self, window: WindowId, _on: bool) -> ShellResult {
        self.win(window).map(drop).and(Err(ShellError::Unsupported))
    }

    fn set_maximized(&mut self, window: WindowId, _on: bool) -> ShellResult {
        self.win(window).map(drop).and(Err(ShellError::Unsupported))
    }

    fn minimize(&mut self, window: WindowId) -> ShellResult {
        self.win(window)?.window.miniaturize(None);
        Ok(())
    }

    fn move_window(&mut self, window: WindowId) -> ShellResult {
        self.win(window).map(drop).and(Err(ShellError::Unsupported))
    }

    fn resize(&mut self, window: WindowId, _edges: u32) -> ShellResult {
        self.win(window).map(drop).and(Err(ShellError::Unsupported))
    }

    fn show_menu(&mut self, window: WindowId, _x: i32, _y: i32) -> ShellResult {
        self.win(window).map(drop)
    }

    fn set_geometry(
        &mut self,
        window: WindowId,
        _x: i32,
        _y: i32,
        _w: i32,
        _h: i32,
    ) -> ShellResult {
        self.win(window).map(drop)
    }

    fn ack_configure(&mut self, window: WindowId, _serial: u32, _flags: u32) -> ShellResult {
        self.win(window).map(drop)
    }

    fn set_cursor(&mut self, window: WindowId, _shape: u32) -> ShellResult {
        self.win(window).map(drop)
    }

    fn set_cursor_theme(&mut self, _name: &str, _size: u32) -> ShellResult {
        Ok(())
    }

    fn window_activate(&mut self, window: WindowId, _token: Option<&str>) -> ShellResult {
        self.win(window)?.window.makeKeyAndOrderFront(None);
        Ok(())
    }

    fn activation_token(&mut self, _window: WindowId, _timeout_ms: u32) -> ShellResult<String> {
        Err(ShellError::Unsupported)
    }

    fn bell(&mut self, _window: WindowId) -> ShellResult {
        Err(ShellError::Unsupported)
    }

    fn inhibit_idle(&mut self, _window: WindowId, _on: bool) -> ShellResult {
        Err(ShellError::Unsupported)
    }

    fn window_export(&mut self, _window: WindowId, _timeout_ms: u32) -> ShellResult<String> {
        Err(ShellError::Unsupported)
    }

    fn session_open(&mut self, _id: Option<&str>, _reason: u32) -> ShellResult {
        Err(ShellError::Unsupported)
    }

    fn session_id(&self) -> ShellResult<String> {
        Err(ShellError::Unsupported)
    }

    fn window_session_remove(&mut self, _window: WindowId) -> ShellResult {
        Err(ShellError::Unsupported)
    }

    fn window_scale(&self, window: WindowId) -> ShellResult<u32> {
        Ok(self.win(window)?.scale120)
    }

    fn window_ssd(&self, window: WindowId) -> ShellResult<bool> {
        self.win(window).map(|_| true)
    }

    fn set_decorated(&mut self, window: WindowId, _on: bool) -> ShellResult {
        self.win(window).map(drop)
    }

    fn set_alpha(&mut self, window: WindowId, _factor: u32) -> ShellResult {
        self.win(window).map(drop).and(Err(ShellError::Unsupported))
    }

    fn child_new(&mut self, _desc: &ChildDesc) -> ShellResult<WindowId> {
        Err(ShellError::Unsupported)
    }

    fn set_position(&mut self, _window: WindowId, _x: i32, _y: i32) -> ShellResult<bool> {
        Err(ShellError::Unsupported)
    }

    fn stack(&mut self, _window: WindowId, _sibling: WindowId, _above: bool) -> ShellResult {
        Err(ShellError::Unsupported)
    }

    fn set_input_region(&mut self, window: WindowId, _rects: &[Rect]) -> ShellResult {
        self.win(window).map(drop)
    }

    fn set_child_place(
        &mut self,
        _window: WindowId,
        _pos: Option<(i32, i32)>,
        _input: Option<&[Rect]>,
    ) -> ShellResult {
        Err(ShellError::State)
    }

    fn set_mapped(&mut self, window: WindowId, on: bool) -> ShellResult {
        let w = self.windows.get_mut(&window).ok_or(ShellError::State)?;
        if on != w.mapped {
            w.mapped = on;
            if on {
                w.window.makeKeyAndOrderFront(None);
            } else {
                w.window.orderOut(None);
            }
            layers::changed_outside();
        }
        Ok(())
    }

    fn window_grabbed(&self, _window: WindowId) -> ShellResult<bool> {
        Ok(false)
    }

    fn set_size(&mut self, _window: WindowId, _w: i32, _h: i32) -> ShellResult<bool> {
        Err(ShellError::State)
    }

    fn selection_own(&mut self, _kind: u32, _items: &[(&str, &[u8])]) -> ShellResult {
        Err(ShellError::Unsupported)
    }

    fn selection_disown(&mut self, _kind: u32) -> ShellResult {
        Err(ShellError::Unsupported)
    }

    fn selection_owned(&self, _kind: u32) -> ShellResult<bool> {
        Ok(false)
    }

    fn selection_types(&self, _kind: u32) -> ShellResult<Vec<String>> {
        Ok(Vec::new())
    }

    fn selection_read(
        &mut self,
        _kind: u32,
        _mime: &str,
        _timeout_ms: u32,
    ) -> ShellResult<Vec<u8>> {
        Err(ShellError::State)
    }

    fn dnd_status(&mut self, _mime: Option<&str>, _actions: u32, _preferred: u32) -> ShellResult {
        Err(ShellError::State)
    }

    fn dnd_finish(&mut self, _success: bool) -> ShellResult {
        Err(ShellError::State)
    }

    fn drag_start(
        &mut self,
        _window: WindowId,
        _items: &[(&str, &[u8])],
        _actions: u32,
    ) -> ShellResult {
        Err(ShellError::Unsupported)
    }

    fn drag_cancel(&mut self) -> ShellResult {
        Ok(())
    }

    fn text_input_enable(&mut self, window: WindowId, _on: bool) -> ShellResult {
        self.win(window).map(drop)
    }

    fn text_input_cursor(
        &mut self,
        window: WindowId,
        _x: i32,
        _y: i32,
        _w: i32,
        _h: i32,
    ) -> ShellResult {
        self.win(window).map(drop)
    }

    fn text_input_surrounding(
        &mut self,
        window: WindowId,
        _text: &str,
        _cursor: usize,
        _anchor: usize,
    ) -> ShellResult {
        self.win(window).map(drop)
    }

    fn text_len(&self, _id: u32) -> ShellResult<usize> {
        Err(ShellError::State)
    }

    fn take_text(&mut self, _id: u32) -> ShellResult<Vec<u8>> {
        Err(ShellError::State)
    }

    /// The commit (design §5.4): each frame's surface takes what it lacks from the backing, then the tree's layers
    /// show them in one flushed transaction.  A frame of a window the shell does not have (a child, until child
    /// layers) is left out; one with no damage on a surface of its size commits nothing.
    fn publish(
        &mut self,
        root: WindowId,
        frames: &[SurfaceFrame],
        _t_key_ns: u64,
    ) -> ShellResult<bool> {
        self.win(root)?;
        let mut ids: Vec<WindowId> = Vec::new();
        for f in frames {
            let Some(w) = self.windows.get_mut(&f.window) else {
                continue;
            };
            let (lw, lh) = (f.w - f.src_x, f.h - f.src_y);
            let damage = layer_damage(f.glitnir_root, lw, lh);
            if damage.is_empty() && w.surfaces.newest().is_some() && w.surfaces.size() == (lw, lh) {
                continue;
            }
            if w.surfaces.draw(f, &damage).is_some() {
                ids.push(f.window);
            }
        }
        if ids.is_empty() {
            return Ok(false);
        }
        if MainThreadMarker::new().is_none() {
            // a Lisp thread's redisplay (design §5.3): its surfaces are filled, and the layers take them at the main
            // thread's next pump; no layer or transaction is touched off the main thread
            for id in ids {
                if !self.deferred.contains(&id) {
                    self.deferred.push(id);
                }
            }
            surface::STATS
                .deferred_commits
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            wait::wake();
            return Ok(true);
        }
        self.commit(&ids);
        Ok(true)
    }

    fn present_tree(&mut self, root: WindowId) -> ShellResult<bool> {
        self.win(root).map(|_| false)
    }

    fn set_presenter_thread(&mut self, on: bool) -> ShellResult {
        if on {
            Err(ShellError::Unsupported)
        } else {
            Ok(())
        }
    }

    fn presenter_stats(&self) -> ShellResult<NhShellPresenterStats> {
        Ok(NhShellPresenterStats::default())
    }

    fn window_stacking(&self, root: WindowId) -> ShellResult<Vec<WindowId>> {
        self.win(root).map(|_| Vec::new())
    }

    fn draw_count(&self, root: WindowId) -> ShellResult<u64> {
        self.win(root).map(|_| 0)
    }

    fn tree_snapshot(
        &self,
        _root: WindowId,
        _frames: &[SurfaceFrame],
        _composed: bool,
    ) -> ShellResult<Vec<u32>> {
        Err(ShellError::Unsupported)
    }

    fn abandon(&self) -> Abandon {
        Abandon {
            conn_fd: -1,
            threads: Vec::new(),
        }
    }

    fn presenter_timing(&self) -> ShellResult<PresenterTiming> {
        Ok(PresenterTiming::default())
    }

    fn app_call(&mut self, kind: &str, arg: &str) -> ShellResult {
        #[cfg(feature = "test-hooks")]
        if let Some(r) = self.test_call(kind, arg) {
            return r;
        }
        match kind {
            "set-resource" | "unset-resource" => {
                defaults::set(arg, kind == "unset-resource");
                Ok(())
            }
            "variable" => {
                self.variable(arg);
                Ok(())
            }
            _ => Err(ShellError::Arg),
        }
    }

    fn app_query(&mut self, kind: &str, arg: &str) -> ShellResult<String> {
        #[cfg(feature = "test-hooks")]
        if let Some(r) = self.test_query(kind, arg) {
            return r;
        }
        match kind {
            "stats" => Ok(stats_text(scope_stats())),
            "get-resource" => Ok(defaults::get(arg)),
            _ => Err(ShellError::Arg),
        }
    }
}

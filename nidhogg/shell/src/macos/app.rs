//! The backend: `NSApplication`, the windows, and the queue of records the glue drains.
//!
//! The queue's wake descriptor is a pipe that holds one byte while records wait.  Calls with no AppKit counterpart
//! (client-side decorations, reconnecting, sessions, xdg-foreign, activation tokens, cursor themes) are accepted and
//! do nothing; the features later work brings (children, selections, drag and drop, text input, the presenter) answer
//! `Unsupported` until then.

use super::runloop::{MacEv, SCOPE};
use super::screens;
use super::window::Win;
use crate::api::{
    Abandon, ChildDesc, NHSHELL_WIN_UNDECORATED, NhShellPresenterStats, PresenterTiming,
    ShellBackend, ShellError, ShellResult, SurfaceFrame, WindowDesc, WindowId,
};
use crate::caps::NhShellCaps;
use crate::event::{ConfigureEv, FocusEv, NHSHELL_STATE_ACTIVATED, Record, ScaleEv, ShellEvent};
use crate::scope::{Scope, ScopeStats};
use glitnir::geom::Rect;
use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2_app_kit::{
    NSAlert, NSApplication, NSApplicationActivationPolicy, NSEventMask, NSModalResponseContinue,
};
use objc2_foundation::{NSDate, NSDefaultRunLoopMode, NSException, NSRect, NSSize, NSString};
use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

pub struct MacShell {
    mtm: MainThreadMarker,
    app: Retained<NSApplication>,
    windows: HashMap<WindowId, Win>,
    next_id: WindowId,
    serial: u32,
    queue: VecDeque<Record>,
    /// The wake pipe: [read, write].
    wake: [i32; 2],
    /// Test hook: the next pump raises an `NSException` inside the catch.
    raise_in_pump: bool,
}

/// The counters `(nhmac-stats)` returns: the scope's, then the ones that stay zero until the work that counts them is
/// built.
fn stats_text(s: ScopeStats) -> String {
    format!(
        "(:takes {} :overlapping-takes {} :queued {} :applied {} :lock-waiters 0 :parks-with-waiters 0 \
         :ring-acquires 0 :ring-reuses 0 :commits 0 :deferred-commits 0)",
        s.takes, s.overlapping_takes, s.queued, s.applied
    )
}

/// The id the test windows' delegates report as: no host window has it, so their events change nothing.
const TEST_WINDOW: WindowId = WindowId::MAX;

/// The scope's counters now.
fn scope_stats() -> ScopeStats {
    SCOPE.with(Scope::stats)
}

/// `after - before`, as the test queries return it.
fn stats_delta(before: ScopeStats, after: ScopeStats) -> String {
    format!(
        ":takes {} :overlapping-takes {} :queued {} :applied {}",
        after.takes - before.takes,
        after.overlapping_takes - before.overlapping_takes,
        after.queued - before.queued,
        after.applied - before.applied
    )
}

/// Move `window`'s frame `by` points wider, through `setFrame:display:` (AppKit calls `windowDidResize:` inside).
fn grow(window: &objc2_app_kit::NSWindow, by: f64) {
    let f = window.frame();
    window.setFrame_display(
        NSRect::new(f.origin, NSSize::new(f.size.width + by, f.size.height)),
        false,
    );
}

/// Every event AppKit has queued, through `sendEvent:`.
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
        app.sendEvent(&ev);
    }
}

/// The first element of a printed argument list such as `(5)`, as a number (`Arg` otherwise).
fn first_number(arg: &str) -> ShellResult<u64> {
    let inner = arg.trim().trim_start_matches('(').trim_end_matches(')');
    inner
        .split_whitespace()
        .next()
        .and_then(|n| n.parse().ok())
        .ok_or(ShellError::Arg)
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
            raise_in_pump: false,
        };
        for o in screens::outputs(mtm) {
            shell.push(ShellEvent::OutputAdd(o), 0);
        }
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
        if std::mem::take(&mut self.raise_in_pump) {
            let name = NSString::from_str("NhmacTestException");
            let reason = NSString::from_str("a test exception raised inside the pump");
            if let Some(e) = NSException::new(&name, Some(&reason), None) {
                e.raise();
            }
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

    /// A modal alert until `deadline`, run as a modal session with the backend lent and ended by the deadline: the
    /// pumps' test of a call that runs a modal.  With `resize`, a test window is resized from inside the session after
    /// its first round, so its delegate is called inside the modal.  An exception inside it skips the session's end,
    /// on a display the guard then poisons anyway.
    fn modal_alert(&mut self, deadline: Instant, resize: bool) -> String {
        let (mtm, app) = (self.mtm, self.app.clone());
        let before = scope_stats();
        let test =
            resize.then(|| Win::open(mtm, TEST_WINDOW, 200, 100, false, "Nidhogg test window"));
        let rounds = self.lent(|_| {
            let alert = NSAlert::new(mtm);
            alert.setMessageText(&NSString::from_str("Nidhogg test alert"));
            let panel = alert.window();
            let session = app.beginModalSessionForWindow(&panel);
            let mut rounds = 0u64;
            while Instant::now() < deadline {
                // SAFETY: the session is the one begun above, and not ended yet.
                let r = unsafe { app.runModalSession(session) };
                rounds += 1;
                if let (1, Some(t)) = (rounds, &test) {
                    grow(&t.window, 40.0);
                }
                if r != NSModalResponseContinue {
                    break;
                }
                std::thread::sleep(Duration::from_millis(5));
            }
            // SAFETY: the session begun above, ended once.
            unsafe { app.endModalSession(session) };
            panel.orderOut(None);
            rounds
        });
        // counted before the test window goes: its closing events wait for the next pump
        let r = format!("(:rounds {rounds} {})", stats_delta(before, scope_stats()));
        drop(test);
        r
    }

    /// The re-entry test: AppKit calls that call a delegate back, made with the backend lent (the delegate takes it at
    /// once), while a callback holds the borrow (the event is queued, and applied when the lend ends), and as a
    /// borrow's deferred effect (made after the borrow is put back, so taken at once again).  Inside the borrow it
    /// also runs a modal session and a nested `nextEventMatchingMask:` loop.  Returns the scope's counters' change
    /// and how many events had queued after each step of the lend.
    fn reentry(&mut self) -> String {
        let (mtm, app) = (self.mtm, self.app.clone());
        let before = scope_stats();
        let a = Win::open(mtm, TEST_WINDOW, 200, 100, false, "Nidhogg test window A");
        let b = Win::open(mtm, TEST_WINDOW, 200, 100, false, "Nidhogg test window B");
        let (wa, wb) = (a.window.clone(), b.window.clone());
        let mut steps = Vec::new();
        self.lent(|scope| {
            // the windows' opening events, if any, queued before the lend
            let start = scope.stats().queued;
            // with the backend lent and no borrow live: the delegate takes it
            grow(&wa, 10.0);
            steps.push(format!(":sync-queued {}", scope.stats().queued - start));
            let mut t = scope.take().expect("the pump's scope is lent");
            // inside the borrow: each callback finds the scope empty and queues
            grow(&wa, 10.0);
            wb.makeKeyAndOrderFront(None);
            let alert = NSAlert::new(mtm);
            let panel = alert.window();
            let session = app.beginModalSessionForWindow(&panel);
            // SAFETY: the session begun above, run once and ended once.
            unsafe {
                app.runModalSession(session);
                app.endModalSession(session);
            }
            panel.orderOut(None);
            grow(&wb, 10.0);
            drain_events(&app);
            steps.push(format!(":inner-queued {}", scope.stats().queued - start));
            // a deferred effect: made after the borrow is put back
            let wa2 = wa.clone();
            t.defer(move || grow(&wa2, 10.0));
            drop(t);
            steps.push(format!(
                ":after-effect-queued {}",
                scope.stats().queued - start
            ));
        });
        // counted before the test windows go: their closing events wait for the next pump
        let r = format!(
            "({} {})",
            stats_delta(before, scope_stats()),
            steps.join(" ")
        );
        drop((a, b));
        r
    }

    /// The window's layer facts, as a Lisp plist: whether the root view has our layer, wants it, and has no subviews;
    /// the window's visibility and its content size in device pixels.
    fn window_info(&self, window: WindowId) -> ShellResult<String> {
        let w = self.win(window)?;
        let layer_ours = w
            .view
            .layer()
            .is_some_and(|l| std::ptr::eq(Retained::as_ptr(&l), Retained::as_ptr(&w.layer)));
        let scale = w.window.backingScaleFactor();
        let (cw, ch) = w.content_size();
        let screen = screens::outputs(self.mtm).first().map_or(0, |o| o.w);
        Ok(format!(
            "(:visible {} :layer {} :wants-layer {} :subviews {} :device-width {} :device-height {} \
             :screen-device-width {screen})",
            if w.window.isVisible() { "t" } else { "nil" },
            if layer_ours { "t" } else { "nil" },
            if w.view.wantsLayer() { "t" } else { "nil" },
            w.view.subviews().len(),
            (f64::from(cw) * scale).round() as i64,
            (f64::from(ch) * scale).round() as i64,
        ))
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

    fn dispatch_main(&mut self) -> ShellResult {
        // an exception raised here is turned into a panic by the glue's guard (nhshell::catch_objc)
        self.pump();
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

    fn publish(
        &mut self,
        root: WindowId,
        _frames: &[SurfaceFrame],
        _t_key_ns: u64,
    ) -> ShellResult<bool> {
        self.win(root).map(|_| false)
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

    fn app_call(&mut self, kind: &str, _arg: &str) -> ShellResult {
        match kind {
            "test-raise-in-pump" => {
                self.raise_in_pump = true;
                Ok(())
            }
            _ => Err(ShellError::Arg),
        }
    }

    fn app_query(&mut self, kind: &str, arg: &str) -> ShellResult<String> {
        match kind {
            "window-info" => self.window_info(first_number(arg)?),
            "stats" => Ok(stats_text(scope_stats())),
            "modal-alert" | "modal-resize" => {
                let ms = first_number(arg)?;
                let deadline = Instant::now()
                    .checked_add(Duration::from_millis(ms))
                    .ok_or(ShellError::Arg)?;
                Ok(self.modal_alert(deadline, kind == "modal-resize"))
            }
            "test-reentry" => Ok(self.reentry()),
            _ => Err(ShellError::Arg),
        }
    }
}

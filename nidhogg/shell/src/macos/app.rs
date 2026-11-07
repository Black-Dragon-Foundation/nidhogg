//! The backend: `NSApplication`, the windows, and the queue of records the glue drains.
//!
//! The queue's wake descriptor is a pipe that holds one byte while records wait.  Calls with no AppKit counterpart
//! (client-side decorations, reconnecting, sessions, xdg-foreign, activation tokens, cursor themes) are accepted and
//! do nothing; the features later work brings (children, selections, drag and drop, text input, the presenter) answer
//! `Unsupported` until then.

use super::appdelegate::{self, AppDelegate};
use super::defaults;
use super::kq::{self, Kq};
use super::menus;
use super::power;
use super::runloop::{MacEv, SCOPE};
use super::screens;
use super::window::Win;
use super::windowing;
use super::{dialogs, pasteboard};
use crate::api::{
    Abandon, ChildDesc, NHSHELL_SEL_DND, NHSHELL_WIN_UNDECORATED, NhShellPresenterStats,
    PresenterTiming, ShellBackend, ShellError, ShellResult, SurfaceFrame, WindowDesc, WindowId,
};
use crate::caps::NhShellCaps;
use crate::event::{
    ConfigureEv, FocusEv, MenuEv, NHSHELL_STATE_ACTIVATED, OutputEv, Record, ScaleEv, ShellEvent,
};
use crate::scope::{Scope, ScopeStats};
use crate::vars::Settings;
use crate::wait::{self, Change, Reg, WaitOps, Woke};
use glitnir::geom::Rect;
use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy, NSEventMask};
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
    /// The application delegate (AppKit holds it weakly), the screens the host knows and the idle assertion.
    _delegate: Retained<AppDelegate>,
    known_outputs: Vec<OutputEv>,
    idle_assertion: Option<u32>,
    settings: Settings,
    dark: bool,
    regs: wait::Regs,
    /// Test hook: the next pump raises an `NSException` inside the catch.
    #[cfg(feature = "test-hooks")]
    raise_in_pump: bool,
    /// Test hook: how many of the next new registrations go to the helper.
    #[cfg(feature = "test-hooks")]
    force_helper: u32,
}

/// The counters `(nhmac-stats)` returns: the scope's, the main wait's (with the lock hand-off's), then the ones that
/// stay zero until the work that counts them is built.
fn stats_text(s: ScopeStats) -> String {
    format!(
        "(:takes {} :overlapping-takes {} :queued {} :applied {} {} \
         :ring-acquires 0 :ring-reuses 0 :commits 0 :deferred-commits 0)",
        s.takes,
        s.overlapping_takes,
        s.queued,
        s.applied,
        wait::STATS.text(&wait::PARK)
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
        let delegate = AppDelegate::new(mtm);
        app.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
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
            _delegate: delegate,
            known_outputs: screens::outputs(mtm),
            idle_assertion: None,
            settings: Settings::default(),
            dark: false,
            regs: wait::Regs::default(),
            #[cfg(feature = "test-hooks")]
            raise_in_pump: false,
            #[cfg(feature = "test-hooks")]
            force_helper: 0,
        };
        for o in shell.known_outputs.clone() {
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
            MacEv::Input(id, ev) => {
                if self.windows.contains_key(&id) {
                    self.push(ev, id);
                }
            }
            MacEv::Screens => {
                let new = screens::outputs(self.mtm);
                for (change, o) in crate::outputs::diff(&self.known_outputs, &new) {
                    let ev = match change {
                        crate::outputs::Change::Added => ShellEvent::OutputAdd(o),
                        crate::outputs::Change::Removed => ShellEvent::OutputRemove(o),
                        crate::outputs::Change::Changed => ShellEvent::OutputChange(o),
                    };
                    self.push(ev, 0);
                }
                self.known_outputs = new;
            }
            MacEv::Menu(generation, index) => {
                self.push(ShellEvent::Menu(MenuEv { generation, index }), 0);
            }
        }
    }

    /// `frame-look` "(WINDOW APPEARANCE TITLEBAR)": `ns-appearance` (dark, light or nil) and `ns-transparent-titlebar`.
    fn frame_look(&mut self, arg: &str) -> ShellResult {
        let (n, rest) = crate::menu::split_numbers(arg, 1).ok_or(ShellError::Arg)?;
        let window = u64::try_from(n[0]).map_err(|_| ShellError::Arg)?;
        let rest = rest.trim_end_matches(')');
        let (appearance, titlebar) = rest.split_once(' ').unwrap_or((rest, "nil"));
        windowing::look(
            &self.win(window)?.window,
            appearance,
            titlebar.trim() != "nil",
        );
        Ok(())
    }

    /// `ns-perform-service` "(NAME TEXT)": the service's answer as a Lisp string, or `nil`.
    fn service_perform(&mut self, arg: &str) -> String {
        let inner = arg.strip_prefix('(').unwrap_or(arg);
        let inner = inner.strip_suffix(')').unwrap_or(inner);
        let Some((name, text)) = inner.split_once(crate::pasteboard::US) else {
            return "nil".into();
        };
        let (name, text) = (name.to_string(), text.to_string());
        self.lent(|_| crate::catch_objc(|| super::services::perform(&name, &text)))
            .map_or_else(|| "nil".to_string(), |t| crate::pasteboard::lisp_string(&t))
    }

    /// `x-file-dialog`: the panel modally, with the backend lent; the chosen path as a Lisp string, or `nil`.
    fn file_dialog(&mut self, arg: &str) -> ShellResult<String> {
        let req = crate::pasteboard::dialog_req(arg).ok_or(ShellError::Arg)?;
        let mtm = self.mtm;
        let path = self.lent(|_| crate::catch_objc(|| dialogs::run(mtm, &req)));
        Ok(path.map_or_else(|| "nil".to_string(), |p| crate::pasteboard::lisp_string(&p)))
    }

    /// `x-popup-menu`: the menu at (X, Y) device pixels of the frame's window, shown by AppKit's tracking loop with
    /// the backend lent; the chosen item's place as a Lisp form (`nil` for none).  The arguments are
    /// "(WINDOW X Y RECORDS)".
    fn menu_popup(&mut self, arg: &str) -> ShellResult<String> {
        let (n, text) = crate::menu::split_numbers(arg, 3).ok_or(ShellError::Arg)?;
        let nodes = crate::menu::decode(text.trim_end_matches(')')).ok_or(ShellError::Arg)?;
        let window = u64::try_from(n[0]).map_err(|_| ShellError::Arg)?;
        let (mtm, win) = (
            self.mtm,
            self.windows.get(&window).map(|w| w.window.clone()),
        );
        let chosen = self.lent(|_| menus::popup(mtm, win.as_deref(), n[1], n[2], &nodes));
        Ok(chosen.map_or_else(|| "nil".to_string(), |i| i.to_string()))
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
        #[cfg(feature = "test-hooks")]
        self.maybe_raise();
        let app = self.app.clone();
        self.lent(|_| {
            drain_events(&app);
            app.updateWindows();
        });
        let ids: Vec<WindowId> = self.windows.keys().copied().collect();
        for id in ids {
            self.compare(id);
        }
        let dark = windowing::system_dark(&self.app);
        if dark != self.dark {
            self.dark = dark;
            self.apply(MacEv::Menu(0, appdelegate::APP_APPEARANCE));
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

    fn set_fullscreen(&mut self, window: WindowId, on: bool) -> ShellResult {
        let native = self.settings.native_fullscreen;
        let w = self.windows.get_mut(&window).ok_or(ShellError::State)?;
        windowing::fullscreen(w, on, native);
        Ok(())
    }

    fn set_maximized(&mut self, window: WindowId, on: bool) -> ShellResult {
        windowing::maximize(self.win(window)?, on);
        Ok(())
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

    fn inhibit_idle(&mut self, _window: WindowId, on: bool) -> ShellResult {
        match (on, self.idle_assertion) {
            (true, None) => {
                self.idle_assertion =
                    Some(power::hold("Emacs asked to stay awake").ok_or(ShellError::State)?);
            }
            (false, Some(id)) => {
                power::release(id);
                self.idle_assertion = None;
            }
            _ => {}
        }
        Ok(())
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

    fn selection_types(&self, kind: u32) -> ShellResult<Vec<String>> {
        Ok(if kind == NHSHELL_SEL_DND {
            super::dnd::types()
        } else {
            Vec::new()
        })
    }

    fn selection_read(&mut self, kind: u32, mime: &str, _timeout_ms: u32) -> ShellResult<Vec<u8>> {
        if kind == NHSHELL_SEL_DND {
            super::dnd::read(mime)
        } else {
            Err(ShellError::State)
        }
    }

    fn dnd_status(&mut self, mime: Option<&str>, actions: u32, preferred: u32) -> ShellResult {
        super::dnd::status(mime, actions, preferred)
    }

    fn dnd_finish(&mut self, success: bool) -> ShellResult {
        super::dnd::finish(success)
    }

    fn drag_start(
        &mut self,
        window: WindowId,
        items: &[(&str, &[u8])],
        actions: u32,
    ) -> ShellResult {
        let (mtm, view) = (self.mtm, self.win(window)?.view.clone());
        super::dnd::start(mtm, window, &view, items, actions)
    }

    fn drag_cancel(&mut self) -> ShellResult {
        super::dnd::cancel();
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
            "pasteboard-own" => pasteboard::own(arg),
            "pasteboard-disown" => pasteboard::disown(arg),
            "variable" => {
                if crate::vars::set_variable(&mut self.settings, arg) {
                    windowing::presentation(&self.app, &self.settings);
                }
                Ok(())
            }
            "frame-look" => self.frame_look(arg),
            "menu-bar" => menus::bar(self.mtm, &self.app, arg),
            "menu-dock" => menus::dock(self.mtm, arg),
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
            "system-appearance" => Ok(if self.dark { "dark" } else { "light" }.to_string()),
            "take-open-files" => Ok(take_open_files_text()),
            "pasteboard-read" => Ok(pasteboard::read(arg)),
            "pasteboard-exists" => Ok(pasteboard::exists(arg)),
            "pasteboard-owned" => Ok(pasteboard::owned_text(arg)),
            "file-dialog" => self.file_dialog(arg),
            "service-perform" => Ok(self.service_perform(arg)),
            "menu-popup" => self.menu_popup(arg),
            _ => Err(ShellError::Arg),
        }
    }
}

/// The files queued by `application:openFiles:`, as a Lisp list of strings.
fn take_open_files_text() -> String {
    let files = appdelegate::take_open_files();
    format!(
        "({})",
        files
            .iter()
            .map(|f| crate::pasteboard::lisp_string(f))
            .collect::<Vec<_>>()
            .join(" ")
    )
}

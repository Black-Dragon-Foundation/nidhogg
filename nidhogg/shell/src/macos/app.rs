//! The backend: `NSApplication`, the windows, and the queue of records the glue drains.
//!
//! The queue's wake descriptor is a pipe that holds one byte while records wait.  Calls with no AppKit counterpart
//! (client-side decorations, reconnecting, sessions, xdg-foreign, activation tokens, cursor themes) are accepted and
//! do nothing; the features later work brings (children, selections, drag and drop, text input, the presenter) answer
//! `Unsupported` until then.

use super::appdelegate::{self, AppDelegate};
use super::defaults;
use super::kq::{self, Kq};
use super::layers;
use super::menus;
use super::nsapp;
use super::pacer;
use super::power;
use super::runloop::{MacEv, SCOPE};
use super::screens;
use super::surface;
use super::window::Win;
use super::windowing;
use super::{dialogs, pasteboard};
use crate::api::{
    Abandon, ChildDesc, NHSHELL_SEL_DND, NHSHELL_WIN_UNDECORATED, NhShellPresenterStats, Place,
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
use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy, NSEventMask, NSScreen};
use objc2_foundation::{NSDate, NSDefaultRunLoopMode, NSRect, NSString};
use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::time::Duration;

/// The test hooks: `nhmac--call` and `nhmac--query` kinds that exist only in a checking build.
#[cfg(feature = "test-hooks")]
#[path = "hooks.rs"]
mod hooks;

/// Child windows as container layers, and the pointer's hit-test.
#[path = "child.rs"]
mod child;

/// A child's chrome (Glitnir's corners, border and shadow) on its layers.
#[path = "chrome.rs"]
mod chrome;

#[path = "cards.rs"]
mod cards;
/// A child's show and hide transitions, run by Core Animation.
#[path = "motion.rs"]
mod motion;

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
    /// `ns-use-srgb-colorspace`: the colour space new surfaces are tagged with.
    srgb: bool,
    /// Windows a Lisp thread's redisplay filled whose commit waits for the main thread's next pump.
    deferred: Vec<WindowId>,
    /// The commit pacer's timer: armed while a window holds a commit.
    pacer: pacer::Timer,
    /// Child windows, each parent's children bottom to top, the parents whose children changed places, and children
    /// gone whose containers leave at their root's next commit.
    kids: HashMap<WindowId, child::Kid>,
    order: HashMap<WindowId, Vec<WindowId>>,
    order_dirty: BTreeSet<WindowId>,
    retired: Vec<(WindowId, child::Kid)>,
    /// Roots whose children changed since their last commit.
    owed: BTreeSet<WindowId>,
    /// Each root's window cards (`set_cards`), shown by the root tree's commit.
    cards: BTreeMap<WindowId, cards::Cards>,
    /// The window under the pointer in each root's tree, and the press that holds it (root, window).
    hover: HashMap<WindowId, WindowId>,
    grab: Option<(WindowId, WindowId)>,
    /// Test hook: the next pump raises an `NSException` inside the catch.
    #[cfg(feature = "test-hooks")]
    raise_in_pump: bool,
    /// Test hook: how many of the next new registrations go to the helper.
    #[cfg(feature = "test-hooks")]
    force_helper: u32,
    /// Test hook: the next overhang switch's captures, armed with the colour to count, and what they counted.
    #[cfg(feature = "test-hooks")]
    capture: hooks::Capture,
}

/// The counters `(nhmac-stats)` returns: the scope's, the main wait's (with the lock hand-off's), the surfaces', the
/// layers' property writes, the font driver's.
fn stats_text(s: ScopeStats) -> String {
    format!(
        "(:takes {} :overlapping-takes {} :queued {} :applied {} {} {} {} {} :zoom-calls {})",
        s.takes,
        s.overlapping_takes,
        s.queued,
        s.applied,
        wait::STATS.text(&wait::PARK),
        surface::STATS.text(),
        layers::PROPS.text(),
        super::fonts::STATS.text(),
        super::ax::ZOOM_CALLS.load(std::sync::atomic::Ordering::Relaxed)
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
        // SAFETY: the delegate answers `requestService:userData:error:`; the application keeps the provider.
        let _: () = unsafe { objc2::msg_send![&app, setServicesProvider: &*delegate] };
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
            srgb: true,
            deferred: Vec::new(),
            pacer: pacer::Timer::default(),
            kids: HashMap::new(),
            order: HashMap::new(),
            order_dirty: BTreeSet::new(),
            retired: Vec::new(),
            owed: BTreeSet::new(),
            cards: BTreeMap::new(),
            hover: HashMap::new(),
            grab: None,
            #[cfg(feature = "test-hooks")]
            raise_in_pump: false,
            #[cfg(feature = "test-hooks")]
            force_helper: 0,
            #[cfg(feature = "test-hooks")]
            capture: hooks::Capture::default(),
        };
        for o in shell.known_outputs.clone() {
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

    /// AppKit's point for a frame's top-left at (`x`, `y`) points from the main screen's top-left (NS's
    /// compute_offset: y up from the main screen's bottom).
    fn screen_point(&self, x: i32, y: i32) -> objc2_foundation::NSPoint {
        let top = NSScreen::screens(self.mtm)
            .iter()
            .next()
            .map_or(0.0, |s| s.frame().origin.y + s.frame().size.height);
        objc2_foundation::NSPoint::new(f64::from(x), top - f64::from(y))
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
            MacEv::Screen(id) => self.screen_changed(id),
            MacEv::Key(id, on) => {
                if self.windows.contains_key(&id) {
                    self.push(ShellEvent::Focus(FocusEv { r#in: u8::from(on) }), id);
                }
            }
            MacEv::Input(id, ev) => match ev {
                ShellEvent::PointerMotion(_) | ShellEvent::Button(_) | ShellEvent::Scroll(_) => {
                    self.pointer(id, ev)
                }
                ShellEvent::PointerLeave(p) => {
                    self.pointer_left(id, objc2_foundation::NSPoint::new(p.x, p.y))
                }
                // keys go to the root's window (an overhang panel never becomes key)
                _ => {
                    if let Some(root) = self.root_of(id) {
                        self.push(ev, root);
                    }
                }
            },
            MacEv::Pace => self.release_held(),
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
            MacEv::Resize(id, frame) => self.resize_step(id, frame),
        }
    }

    /// The commit pacer's check (main thread): every window, top-level or child, whose held commit is due (a slot
    /// freed, or the hold's bound) has its root ask the host for a publish, which redraws nothing and reads the
    /// backing under the lock; the timer stays armed while a window still holds one, and is disarmed when none does.
    fn release_held(&mut self) {
        let now = now_ns();
        let owing: Vec<(WindowId, bool)> = self
            .windows
            .iter()
            .map(|(id, w)| (*id, &w.surfaces))
            .chain(self.kids.iter().map(|(id, k)| (*id, &k.surfaces)))
            .filter(|(_, s)| s.owes())
            .map(|(id, s)| (id, s.due(now)))
            .collect();
        let holding = !owing.is_empty();
        let roots: BTreeSet<WindowId> = owing
            .into_iter()
            .filter(|&(_, due)| due)
            .filter_map(|(id, _)| self.root_of(id))
            .collect();
        for id in roots {
            // one request at a time: a publish that is not made yet (inside redisplay) is asked for again
            if !self
                .queue
                .iter()
                .any(|r| r.window == id && matches!(r.event, ShellEvent::FrameReady))
            {
                self.push(ShellEvent::FrameReady, id);
            }
        }
        if holding {
            self.pacer.arm();
        } else {
            self.pacer.disarm();
        }
    }

    /// A drag step of our own resize (`resize::NhWindow`): the host is configured for `frame` now, or, while it is
    /// still drawing for the last step, `frame` waits (the newest replaces an older one).  The window takes the frame
    /// in `commit`, once the host has drawn a surface of its size.
    fn resize_step(&mut self, id: WindowId, frame: NSRect) {
        let Some(w) = self.windows.get_mut(&id) else {
            return;
        };
        if w.resize.inflight.is_some() {
            w.resize.waiting.push(frame);
            return;
        }
        let r = w.window.contentRectForFrameRect(frame);
        let size = (r.size.width.round() as i32, r.size.height.round() as i32);
        if size == w.size {
            // the size held (at its minimum): only the origin can move, and there is nothing to draw
            w.window.setFrame_display(frame, false);
            return;
        }
        let s = w.scale();
        let px = (s.px_i(size.0), s.px_i(size.1));
        w.size = size;
        w.resize.inflight = Some((frame, px));
        self.configure(id, size.0, size.1);
    }

    /// `frame-look` "(WINDOW APPEARANCE TITLEBAR)": `ns-appearance` (dark, light or nil) and `ns-transparent-titlebar`.
    fn frame_look(&mut self, arg: &str) -> ShellResult {
        let (n, rest) = crate::menu::split_numbers(arg, 1).ok_or(ShellError::Arg)?;
        let window = u64::try_from(n[0]).map_err(|_| ShellError::Arg)?;
        let rest = rest.trim_end_matches(')');
        let (appearance, titlebar) = rest.split_once(' ').unwrap_or((rest, "nil"));
        // a child frame is a layer in its root's window: it has no window of its own to give a look (every new
        // frame gets this call, from after-make-frame-functions)
        if self.kids.contains_key(&window) {
            return Ok(());
        }
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
        if w.resize.inflight.is_some() {
            // our own resize: the host was told the new size, which the window takes when it is drawn
            return;
        }
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

    /// Tell the host when window `id` is on another screen than it was told: a leave of the old output and an enter
    /// of the new one (the glue then has Lisp run `move-frame-functions`, as for a Wayland output change).
    fn screen_changed(&mut self, id: WindowId) {
        let Some(w) = self.windows.get_mut(&id) else {
            return;
        };
        let Some(o) = screens::output_of(self.mtm, &w.window) else {
            return;
        };
        let old = std::mem::replace(&mut w.output, o.id);
        if old == o.id {
            return;
        }
        if old != 0 {
            let gone = OutputEv {
                id: old,
                ..OutputEv::default()
            };
            self.push(ShellEvent::LeaveOutput(gone), id);
        }
        self.push(ShellEvent::EnterOutput(o), id);
    }

    pub(super) fn configure(&mut self, id: WindowId, w: i32, h: i32) {
        self.serial = self.serial.wrapping_add(1);
        let serial = self.serial;
        if let Some(win) = self.windows.get_mut(&id) {
            win.configuring = Some(serial);
        }
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
        if !self.deferred.is_empty() || !self.owed.is_empty() {
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
        self.check_appearance();
    }

    /// Tell Lisp when the system's appearance changed.  A switch arrives as a notification on the run loop, with no
    /// event: so this runs after every sleep, whatever woke it, as well as in each pump.
    pub(super) fn check_appearance(&mut self) {
        let dark = windowing::system_dark(&self.app);
        if dark != self.dark {
            self.dark = dark;
            self.apply(MacEv::Menu(0, appdelegate::APP_APPEARANCE));
        }
        self.release_held();
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
    /// Commit the trees of `ids` (windows that drew) and of every root that owes one, each in one flushed transaction
    /// with its children's changes (main thread).
    fn commit(&mut self, ids: &[WindowId]) {
        // our own resize: a window whose host has drawn the size asked for takes that frame first, with the backend
        // lent so the `windowDidResize:` it causes is applied in the same drain
        let mut moves = Vec::new();
        for id in ids {
            if let Some(w) = self.windows.get_mut(id)
                && let Some((frame, px)) = w.resize.inflight
                && w.surfaces.newest().is_some()
                && w.surfaces.size() == px
            {
                w.resize.inflight = None;
                moves.push((w.window.clone(), frame));
            }
        }
        if !moves.is_empty() {
            self.lent(|_| {
                for (window, frame) in &moves {
                    window.setFrame_display(*frame, false);
                }
            });
        }
        let mut roots: BTreeSet<WindowId> = std::mem::take(&mut self.owed);
        roots.extend(ids.iter().filter_map(|&id| self.root_of(id)));
        for root in roots {
            self.commit_tree(root, ids);
        }
        layers::publish_end();
        // the steps that waited for these commits
        for id in ids {
            let next = self
                .windows
                .get_mut(id)
                .filter(|w| w.resize.inflight.is_none())
                .and_then(|w| w.resize.waiting.take());
            if let Some(frame) = next {
                self.resize_step(*id, frame);
            }
        }
    }

    /// A watched variable's new value (`nhmac--variable-changed`): ARG is `(SYMBOL VALUE)` as Lisp prints it.  The
    /// window settings (`vars.rs`: full screen, the menu bar's auto-hide) go to the presentation options;
    /// `ns-use-srgb-colorspace` starts every ring over, tagged the new way; the text smoothing ones reach the drawing.
    /// The rest are accepted.
    fn variable(&mut self, arg: &str) {
        if crate::vars::set_variable(&mut self.settings, arg) {
            windowing::presentation(&self.app, &self.settings);
            return;
        }
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
        } else {
            self.check_appearance();
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
        // what the last redisplay published shows before the main thread sleeps
        if MainThreadMarker::new().is_some() && (!self.deferred.is_empty() || !self.owed.is_empty())
        {
            let ids = std::mem::take(&mut self.deferred);
            self.commit(&ids);
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
            let n = self.queue.len();
            crate::trace::pump(|| format!("shell: {n} records queued, no AppKit pump"));
            return self.flush_main();
        }
        crate::trace::pump(|| "shell: AppKit pump".into());
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
        self.release_held();
        Ok(())
    }

    fn window_new_named(
        &mut self,
        desc: &WindowDesc,
        session_name: Option<&str>,
    ) -> ShellResult<WindowId> {
        self.window_new_placed(desc, session_name, Place::Shell)
    }

    fn window_new_placed(
        &mut self,
        desc: &WindowDesc,
        _session_name: Option<&str>,
        place: Place,
    ) -> ShellResult<WindowId> {
        let id = self.next_id;
        self.next_id += 1;
        let mut win = Win::make(
            self.mtm,
            id,
            desc.w,
            desc.h,
            desc.flags & NHSHELL_WIN_UNDECORATED != 0,
            &desc.title,
        );
        // placed before it shows, so its first screen (and scale) is the one its frame was made for
        match place {
            Place::At(x, y) => {
                win.window.setFrameTopLeftPoint(self.screen_point(x, y));
            }
            Place::Near(near) => {
                if let Some(n) = self.windows.get(&near) {
                    let f = n.window.frame();
                    let from =
                        objc2_foundation::NSPoint::new(f.origin.x, f.origin.y + f.size.height);
                    let _ = win.window.cascadeTopLeftFromPoint(from);
                }
            }
            Place::Shell => {}
        }
        win.window.makeKeyAndOrderFront(None);
        win.placed();
        let (w, h, scale) = (win.size.0, win.size.1, win.scale120);
        crate::trace::note(|| format!("window made: window {id} {w}x{h} points, scale120 {scale}"));
        win.surfaces.reset(self.srgb);
        layers::changed_outside();
        self.windows.insert(id, win);
        self.push(ShellEvent::Scale(ScaleEv { scale120: scale }), id);
        // 0 x 0, ours to pick, as a compositor's first configure: the window has the size the host made it with, and
        // the host may have resized its frame since (a tool bar that wraps at the new width) before this is read;
        // the window takes that size from the host's next surface
        self.configure(id, 0, 0);
        self.screen_changed(id);
        Ok(id)
    }

    fn window_destroy(&mut self, window: WindowId) -> ShellResult {
        super::ime::forget(window);
        if self.kids.contains_key(&window) {
            return self.child_destroy(window);
        }
        self.children_gone(window);
        self.cards.remove(&window);
        self.windows
            .remove(&window)
            .map(drop)
            .ok_or(ShellError::State)
    }

    fn scale_start(&mut self, window: WindowId, _scale120: u32) -> ShellResult {
        self.known(window)
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
        let point = self.screen_point(x, y);
        self.win(window)?.window.setFrameTopLeftPoint(point);
        layers::changed_outside();
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

    fn ack_configure(&mut self, window: WindowId, serial: u32, _flags: u32) -> ShellResult {
        if let Some(w) = self.windows.get_mut(&window) {
            // serials wrap: an ack at or past the one sent last ends it
            if w.configuring
                .is_some_and(|s| serial.wrapping_sub(s) < u32::MAX / 2)
            {
                w.configuring = None;
            }
            return Ok(());
        }
        self.known(window)
    }

    fn set_cursor(&mut self, window: WindowId, _shape: u32) -> ShellResult {
        self.known(window)
    }

    fn set_cursor_theme(&mut self, _name: &str, _size: u32) -> ShellResult {
        Ok(())
    }

    fn window_activate(&mut self, window: WindowId, _token: Option<&str>) -> ShellResult {
        // NS's ns_focus_frame: the application first, or the window cannot become key while another app is active
        #[allow(deprecated)]
        self.app.activateIgnoringOtherApps(true);
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
        // a child is at its root's scale
        let root = self.root_of(window).ok_or(ShellError::State)?;
        Ok(self.win(root)?.scale120)
    }

    fn window_ssd(&self, window: WindowId) -> ShellResult<bool> {
        self.win(window).map(|_| true)
    }

    fn set_decorated(&mut self, window: WindowId, _on: bool) -> ShellResult {
        self.win(window).map(drop)
    }

    fn set_alpha(&mut self, window: WindowId, factor: u32) -> ShellResult {
        if self.kids.contains_key(&window) {
            return self.kid_alpha(window, factor);
        }
        self.win(window).map(drop).and(Err(ShellError::Unsupported))
    }

    fn chrome_on_layers(&self) -> bool {
        true
    }

    fn cards_on_layers(&self) -> bool {
        true
    }

    fn set_cards(&mut self, window: WindowId, list: &[crate::api::CardImage]) -> ShellResult {
        // a top-level window's or a child's (its blocks): shown by its root tree's next commit
        let root = self.root_of(window).ok_or(ShellError::State)?;
        self.cards.entry(window).or_default().set(list)?;
        self.owe(root);
        Ok(())
    }

    fn set_chrome(
        &mut self,
        window: WindowId,
        chrome: Option<&glitnir::scene::Chrome>,
    ) -> ShellResult {
        self.kid_chrome(window, chrome.copied())
    }

    fn set_motion(
        &mut self,
        window: WindowId,
        spec: Option<glitnir::motion::MotionSpec>,
    ) -> ShellResult {
        self.kid_motion(window, spec)
    }

    fn set_backdrop(&mut self, window: WindowId, blur_px: f32) -> ShellResult {
        self.kid_backdrop(window, blur_px)
    }

    fn child_new(&mut self, desc: &ChildDesc) -> ShellResult<WindowId> {
        self.child_create(desc)
    }

    fn reparent(&mut self, window: WindowId, desc: &ChildDesc, title: &str) -> ShellResult {
        self.kid_reparent(window, desc, title)
    }

    fn set_position(&mut self, window: WindowId, x: i32, y: i32) -> ShellResult<bool> {
        self.kid_position(window, x, y)
    }

    fn stack(&mut self, window: WindowId, sibling: WindowId, above: bool) -> ShellResult {
        self.kid_stack(window, sibling, above)
    }

    fn set_input_region(&mut self, window: WindowId, rects: &[Rect]) -> ShellResult {
        if self.kids.contains_key(&window) {
            return self.kid_input(window, rects);
        }
        self.win(window).map(drop)
    }

    fn set_child_place(
        &mut self,
        window: WindowId,
        pos: Option<(i32, i32)>,
        input: Option<&[Rect]>,
    ) -> ShellResult {
        self.kid_place(window, pos, input)
    }

    fn set_mapped(&mut self, window: WindowId, on: bool) -> ShellResult {
        if self.kids.contains_key(&window) {
            return self.kid_mapped(window, on);
        }
        let w = self.windows.get_mut(&window).ok_or(ShellError::State)?;
        // an iconified window stays mapped; showing it is the Dock's deminiaturize
        if on && w.window.isMiniaturized() {
            w.window.deminiaturize(None);
            return Ok(());
        }
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
        x: i32,
        y: i32,
        w: i32,
        h: i32,
    ) -> ShellResult {
        let view = self.win(window)?.view.clone();
        super::ime::with(window, |p| p.cursor = [x, y, w, h]);
        super::ax::cursor_pushed(&view, window);
        Ok(())
    }

    fn text_input_surrounding(
        &mut self,
        window: WindowId,
        text: &str,
        _cursor: usize,
        _anchor: usize,
    ) -> ShellResult {
        self.win(window)?;
        super::ime::with(window, |p| {
            if p.text.text != text {
                p.text.text = text.to_string();
            }
        });
        Ok(())
    }

    fn text_input_offsets(
        &mut self,
        window: WindowId,
        line_start: usize,
        point: usize,
        mark: usize,
    ) -> ShellResult {
        self.win(window)?;
        super::ime::with(window, |p| {
            (p.text.line_start, p.text.point, p.text.mark) = (line_start, point, mark);
        });
        Ok(())
    }

    fn text_len(&self, id: u32) -> ShellResult<usize> {
        super::ime::text_len(id)
    }

    fn take_text(&mut self, id: u32) -> ShellResult<Vec<u8>> {
        super::ime::take_text(id)
    }

    /// The commit (design §5.4): each frame's surface takes what it lacks from the backing, then the tree's layers
    /// show them in one flushed transaction.  A frame of a window the shell does not have (a child, until child
    /// layers) is left out; one with no damage on a surface of its size commits nothing.
    fn publish(
        &mut self,
        root: WindowId,
        frames: &[SurfaceFrame],
        t_key_ns: u64,
    ) -> ShellResult<bool> {
        self.win(root)?;
        let mut ids: Vec<WindowId> = Vec::new();
        let mut held = false;
        for f in frames {
            let Some(s) = self.surfaces_mut(f.window) else {
                continue;
            };
            let (lw, lh) = (f.w - f.src_x, f.h - f.src_y);
            let damage = layer_damage(f.glitnir_root, lw, lh);
            crate::trace::pump(|| format!("publish: window {} damage {damage:?}", f.window));
            if damage.is_empty() && !s.owes() && s.newest().is_some() && s.size() == (lw, lh) {
                continue;
            }
            let start = now_ns();
            let fill = surface::Timed::start(&surface::STATS.fill_ns);
            let drew = s.draw(f, &damage, start);
            drop(fill);
            match drew {
                Some(surface::Drew::Drawn) => {
                    // `glitnir-stats`: one entry per drawing a layer shows (Core Animation composes; no compose time)
                    let px = damage
                        .iter()
                        .map(|r| r.w.max(0) as u64 * r.h.max(0) as u64)
                        .sum();
                    glitnir::stats::record(
                        f.glitnir_root,
                        glitnir::stats::StatsEntry {
                            t_update_ns: t_key_ns,
                            t_commit_ns: start,
                            t_draw_start_ns: start,
                            t_draw_end_ns: now_ns(),
                            presented_us: -1,
                            damage_px: px,
                            painted_px: px,
                            backfill: 2,
                            ..Default::default()
                        },
                    );
                    ids.push(f.window)
                }
                Some(surface::Drew::Held) => {
                    crate::trace::pump(|| format!("publish: window {} held", f.window));
                    held = true;
                }
                None => {}
            }
        }
        // a held commit: the pacer's timer looks for a freed slot (a Lisp thread's publish leaves that to the main
        // thread, which its wake makes pump)
        if held {
            if MainThreadMarker::new().is_some() {
                self.pacer.arm();
            } else {
                wait::wake();
            }
        }
        if ids.is_empty() && !self.owed.contains(&root) {
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
        // the commit leaves the redisplay (as NS's, which Core Animation commits when the run loop turns): the next
        // pump, the wait before it sleeps, or the host's deadline (`flush_presents`) commits it.  A window in a resize
        // commits now, so the drag's frame and its picture change together.
        let resizing = ids.iter().any(|id| {
            self.windows
                .get(id)
                .is_some_and(|w| w.resize.inflight.is_some() || w.window.inLiveResize())
        });
        if resizing {
            self.commit(&ids);
            return Ok(true);
        }
        for id in ids {
            if !self.deferred.contains(&id) {
                self.deferred.push(id);
            }
        }
        Ok(true)
    }

    fn flush_presents(&mut self) -> ShellResult {
        if MainThreadMarker::new().is_some() && (!self.deferred.is_empty() || !self.owed.is_empty())
        {
            surface::STATS
                .deadline_flushes
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let ids = std::mem::take(&mut self.deferred);
            self.commit(&ids);
        }
        Ok(())
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
        self.win(root)?;
        Ok(self.stacking(root))
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
            "pasteboard-own" => pasteboard::own(arg),
            "pasteboard-disown" => pasteboard::disown(arg),
            "frame-look" => self.frame_look(arg),
            "menu-bar" => menus::bar(self.mtm, &self.app, arg),
            "menu-dock" => menus::dock(self.mtm, arg),
            // `ns-hide-emacs`: "(HOW)", 0 unhide, 1 hide, 2 unhide and activate
            "hide" => {
                let (n, _) = crate::menu::split_numbers(arg, 1).ok_or(ShellError::Arg)?;
                match n[0] {
                    1 => self.app.hide(None),
                    2 => {
                        self.app.unhide(None);
                        #[allow(deprecated)]
                        self.app.activateIgnoringOtherApps(true);
                    }
                    _ => self.app.unhide(None),
                }
                Ok(())
            }
            "hide-others" => {
                self.app.hideOtherApplications(None);
                Ok(())
            }
            "about-panel" => {
                self.app.orderFrontStandardAboutPanel(None);
                Ok(())
            }
            "color-panel" => {
                // SAFETY: AppKit's standard action, on the main thread, with no sender.
                unsafe { self.app.orderFrontColorPanel(None) };
                Ok(())
            }
            "character-palette" => {
                self.app.orderFrontCharacterPalette(None);
                Ok(())
            }
            "badge" => {
                nsapp::badge(&self.app, nsapp::text_arg(arg));
                Ok(())
            }
            // "(URGENCY)": 0 cancel, 1 informational, 2 critical
            "attention" => {
                let (n, _) = crate::menu::split_numbers(arg, 1).ok_or(ShellError::Arg)?;
                nsapp::attention(&self.app, n[0]);
                Ok(())
            }
            // "(PROGRESS)", a float; outside 0 to 1 the indicator is hidden
            "dock-progress" => {
                let p: f64 = nsapp::text_arg(arg)
                    .trim()
                    .parse()
                    .map_err(|_| ShellError::Arg)?;
                nsapp::progress(self.mtm, &self.app, p);
                Ok(())
            }
            "warp-mouse" => {
                let (n, _) = crate::menu::split_numbers(arg, 2).ok_or(ShellError::Arg)?;
                nsapp::warp(n[0] as f64, n[1] as f64);
                Ok(())
            }
            // "(WINDOW OTHER ABOVE)"
            "restack" => {
                let (n, _) = crate::menu::split_numbers(arg, 3).ok_or(ShellError::Arg)?;
                let other = self.win(n[1] as WindowId)?.window.clone();
                nsapp::restack(&self.win(n[0] as WindowId)?.window, &other, n[2] == 1);
                Ok(())
            }
            // "(WINDOW ITEMS)", the items separated by US
            "send-items" => {
                let (n, rest) = crate::menu::split_numbers(arg, 1).ok_or(ShellError::Arg)?;
                let id = n[0] as WindowId;
                let view = self.win(id)?.view.clone();
                let items: Vec<&str> = nsapp::text_arg(rest).split('\u{1f}').collect();
                nsapp::send_items(&view, id, &items);
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
            "reduced-motion" => Ok(if motion::reduced() { "t" } else { "nil" }.into()),
            "system-appearance" => Ok(if self.dark { "dark" } else { "light" }.to_string()),
            "take-open-files" => Ok(take_open_files_text()),
            "take-service-requests" => Ok(super::services::take_requests_lisp()),
            "dnd-offer" => Ok(super::dnd::offer_lisp()),
            "pasteboard-read" => Ok(pasteboard::read(arg)),
            "pasteboard-exists" => Ok(pasteboard::exists(arg)),
            "pasteboard-owned" => Ok(pasteboard::owned_text(arg)),
            "file-dialog" => self.file_dialog(arg),
            "service-perform" => Ok(self.service_perform(arg)),
            "menu-popup" => self.menu_popup(arg),
            "mouse-position" => Ok(nsapp::mouse_position(self.mtm)),
            "ax-trusted" => Ok(if nsapp::ax_trusted() { "t" } else { "nil" }.to_string()),
            "applescript" => Ok(nsapp::applescript(nsapp::text_arg(arg))),
            // "(ALLOW-DISPLAY-SLEEP WHY)"
            "sleep-block" => {
                let (n, why) = crate::menu::split_numbers(arg, 1).ok_or(ShellError::Arg)?;
                Ok(nsapp::sleep_block(nsapp::text_arg(why), n[0] == 1))
            }
            "sleep-unblock" => {
                let (n, _) = crate::menu::split_numbers(arg, 1).ok_or(ShellError::Arg)?;
                Ok(nsapp::sleep_unblock(u32::try_from(n[0]).unwrap_or(0)))
            }
            // "(WINDOW…)": their places, front to back
            "z-order" => {
                let ids: Vec<WindowId> = nsapp::text_arg(arg)
                    .split_whitespace()
                    .map(|w| w.parse().map_err(|_| ShellError::Arg))
                    .collect::<ShellResult<_>>()?;
                let windows: Vec<Retained<objc2_app_kit::NSWindow>> = ids
                    .iter()
                    .map(|id| self.win(*id).map(|w| w.window.clone()))
                    .collect::<ShellResult<_>>()?;
                let refs: Vec<&objc2_app_kit::NSWindow> = windows.iter().map(|w| &**w).collect();
                Ok(nsapp::z_order(&self.app, &refs))
            }
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

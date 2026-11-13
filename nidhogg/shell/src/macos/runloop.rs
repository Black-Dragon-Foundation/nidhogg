//! The pump half of the take-out scope (`crate::scope`): the main thread's scope, the events AppKit's callbacks
//! bring, and the window delegate that reports them.
//!
//! The backend lends itself to [`SCOPE`] for every call that can make AppKit call back (the pump, a modal); a
//! delegate reaches it only through [`deliver`], which applies the event at once when the scope is free and queues
//! it otherwise (outside a lend, or inside a callback that holds the borrow).  Nothing reaches the backend through the
//! glue's guard from here.

use super::app::MacShell;
use crate::api::WindowId;
use crate::event::ShellEvent;
use crate::scope::Scope;
use objc2::rc::Retained;
use objc2::runtime::NSObject;
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::NSWindowDelegate;
use objc2_foundation::{NSNotification, NSObjectProtocol, NSRect};

/// What a callback reports.
#[derive(Clone, Copy, Debug)]
pub enum MacEv {
    /// The window's frame or backing scale may have changed (`windowDidResize:`, `windowDidChangeBackingProperties:`).
    Resized(WindowId),
    /// The window may be on another screen (`windowDidChangeScreen:`).
    Screen(WindowId),
    /// The window became (`true`) or stopped being key.
    Key(WindowId, bool),
    /// A key, modifier or mouse record from the window's view, for the host as it is.
    Input(WindowId, ShellEvent),
    /// The commit pacer's timer fired: a held commit may be due.
    Pace,
    /// An item of the menu bar was chosen: the tree's generation and the item's place.
    Menu(u32, u32),
    /// The screens changed (`applicationDidChangeScreenParameters:`).
    Screens,
    /// A drag in the window's resize band asks for this frame (`resize::NhWindow`).
    Resize(WindowId, NSRect),
    /// A native full-screen transition started (`true`) or ended, entered, exited or failed.
    FullScreen(WindowId, bool),
    /// The run loop's turn after a transition ended: the state asked for during it goes out now.
    FullScreenWant(WindowId),
}

/// How many of AppKit's full-screen notifications reached the window delegates, in the order will enter, will exit,
/// did enter, did exit, failed to enter, failed to exit, then how many of them the backend applied (test diagnostics).
pub static FS_NOTES: [std::sync::atomic::AtomicU32; 7] =
    [const { std::sync::atomic::AtomicU32::new(0) }; 7];

/// The full-screen log: when it started, and its entries.
type FsLog = (Option<std::time::Instant>, Vec<(usize, u128)>);

thread_local! {
    /// The newest full-screen steps as (STEP, milliseconds since the first), at most 24 (test diagnostics: how long a
    /// transition took, and whether a request was sent).  STEP 0 to 5 are FS_NOTES' notifications, 7 a toggle sent,
    /// 8 a request held for a transition's end, 9 the run loop's turn after it.
    pub static FS_LOG: std::cell::RefCell<FsLog> = const { std::cell::RefCell::new((None, Vec::new())) };
}

/// The window's focus and transition notifications, oldest first: name, milliseconds since the first, whether the
/// window is key then, and its first responder's class (test diagnostics, `test-key-log`: round 11's text input keys
/// reached nothing after the full-screen and Dock rows, while the window was key).
pub static KEY_LOG: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

/// The window of notification `n` takes its content view (the Emacs view) back as first responder when it holds that
/// itself or nothing: AppKit leaves the window its own first responder after a full-screen transition and the Dock
/// (round 12: from the full-screen and miniaturize rows on, `:first-responder` was the window), and then key events
/// reach the window, never the view's `interpretKeyEvents:`, so Option and dead keys, Command-Shift and marked text
/// did nothing until a click in the view gave it back.  Another responder (a field the view made first) stays.
fn own_responder(n: &NSNotification) {
    let Some(w) = n
        .object()
        .and_then(|o| o.downcast::<objc2_app_kit::NSWindow>().ok())
    else {
        return;
    };
    let Some(view) = w.contentView() else {
        return;
    };
    let lost = w.firstResponder().is_none_or(|r| {
        std::ptr::eq(
            objc2::rc::Retained::as_ptr(&r).cast::<()>(),
            objc2::rc::Retained::as_ptr(&w).cast::<()>(),
        )
    });
    if lost {
        w.makeFirstResponder(Some(&view));
    }
}

fn key_note(n: &NSNotification, name: &str) {
    static T0: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    let ms = T0
        .get_or_init(std::time::Instant::now)
        .elapsed()
        .as_millis();
    let (key, responder) = n
        .object()
        .and_then(|o| o.downcast::<objc2_app_kit::NSWindow>().ok())
        .map_or((false, "nil".to_owned()), |w| {
            (
                w.isKeyWindow(),
                w.firstResponder()
                    .map_or_else(|| "nil".to_owned(), |r| format!("{:?}", r.class().name())),
            )
        });
    let mut l = KEY_LOG
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if l.len() >= 32 {
        l.remove(0);
    }
    l.push(format!(
        "({name} {ms} {} {responder})",
        if key { "t" } else { "nil" }
    ));
}

fn fs_note(i: usize) {
    FS_NOTES[i].fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    fs_log(i);
}

/// Add step `i` to [`FS_LOG`].
pub fn fs_log(i: usize) {
    FS_LOG.with(|l| {
        let mut l = l.borrow_mut();
        let t0 = *l.0.get_or_insert_with(std::time::Instant::now);
        if l.1.len() >= 24 {
            l.1.remove(0);
        }
        l.1.push((i, t0.elapsed().as_millis()));
    });
}

thread_local! {
    /// The main thread's scope; only the main thread runs AppKit.
    pub static SCOPE: Scope<MacShell, MacEv> = const { Scope::new() };
}

/// Hand `ev` to the backend now, or queue it for the next lend.
pub fn deliver(ev: MacEv) {
    SCOPE.with(|s| s.deliver(ev, MacShell::apply_taken));
}

define_class!(
    /// A window's delegate: it knows only its window's id, and reports through [`deliver`].
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "NhmacWindowDelegate"]
    #[ivars = WindowId]
    pub struct WinDelegate;

    unsafe impl NSObjectProtocol for WinDelegate {}

    unsafe impl NSWindowDelegate for WinDelegate {
        #[unsafe(method(windowDidResize:))]
        fn did_resize(&self, _n: &NSNotification) {
            deliver(MacEv::Resized(*self.ivars()));
        }

        #[unsafe(method(windowDidChangeBackingProperties:))]
        fn did_change_backing(&self, _n: &NSNotification) {
            deliver(MacEv::Resized(*self.ivars()));
        }

        #[unsafe(method(windowDidChangeScreen:))]
        fn did_change_screen(&self, _n: &NSNotification) {
            deliver(MacEv::Screen(*self.ivars()));
        }

        #[unsafe(method(windowDidBecomeKey:))]
        fn did_become_key(&self, n: &NSNotification) {
            key_note(n, "windowDidBecomeKey");
            own_responder(n);
            deliver(MacEv::Key(*self.ivars(), true));
        }

        #[unsafe(method(windowDidMiniaturize:))]
        fn did_miniaturize(&self, n: &NSNotification) {
            key_note(n, "windowDidMiniaturize");
            deliver(MacEv::Key(*self.ivars(), false));
        }

        #[unsafe(method(windowDidDeminiaturize:))]
        fn did_deminiaturize(&self, n: &NSNotification) {
            key_note(n, "windowDidDeminiaturize");
            own_responder(n);
            deliver(MacEv::Key(*self.ivars(), true));
        }

        #[unsafe(method(windowDidResignKey:))]
        fn did_resign_key(&self, n: &NSNotification) {
            key_note(n, "windowDidResignKey");
            deliver(MacEv::Key(*self.ivars(), false));
        }

        #[unsafe(method(windowWillEnterFullScreen:))]
        fn will_enter_full_screen(&self, _n: &NSNotification) {
            fs_note(0);
            deliver(MacEv::FullScreen(*self.ivars(), true));
        }

        #[unsafe(method(windowWillExitFullScreen:))]
        fn will_exit_full_screen(&self, _n: &NSNotification) {
            fs_note(1);
            deliver(MacEv::FullScreen(*self.ivars(), true));
        }

        #[unsafe(method(windowDidEnterFullScreen:))]
        fn did_enter_full_screen(&self, n: &NSNotification) {
            key_note(n, "windowDidEnterFullScreen");
            own_responder(n);
            fs_note(2);
            deliver(MacEv::FullScreen(*self.ivars(), false));
        }

        #[unsafe(method(windowDidExitFullScreen:))]
        fn did_exit_full_screen(&self, n: &NSNotification) {
            key_note(n, "windowDidExitFullScreen");
            own_responder(n);
            fs_note(3);
            deliver(MacEv::FullScreen(*self.ivars(), false));
        }

        #[unsafe(method(windowDidFailToEnterFullScreen:))]
        fn did_fail_to_enter_full_screen(&self, _w: &objc2_app_kit::NSWindow) {
            fs_note(4);
            deliver(MacEv::FullScreen(*self.ivars(), false));
        }

        #[unsafe(method(windowDidFailToExitFullScreen:))]
        fn did_fail_to_exit_full_screen(&self, _w: &objc2_app_kit::NSWindow) {
            fs_note(5);
            deliver(MacEv::FullScreen(*self.ivars(), false));
        }
    }

    impl WinDelegate {
        /// The run loop's turn after a transition's end (`windowing::fullscreen_transition` asks for it).
        #[unsafe(method(nhmacFullScreenWant:))]
        fn full_screen_want(&self, _o: Option<&objc2::runtime::AnyObject>) {
            deliver(MacEv::FullScreenWant(*self.ivars()));
        }
    }
);

impl WinDelegate {
    pub fn new(mtm: MainThreadMarker, id: WindowId) -> Retained<WinDelegate> {
        let this = Self::alloc(mtm).set_ivars(id);
        // SAFETY: NSObject's designated initialiser, on a fresh allocation with its ivars set.
        unsafe { msg_send![super(this), init] }
    }
}

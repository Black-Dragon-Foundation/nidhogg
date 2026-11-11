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
}

thread_local! {
    /// The main thread's scope; only the main thread runs AppKit.
    pub static SCOPE: Scope<MacShell, MacEv> = const { Scope::new() };
}

/// Hand `ev` to the backend now, or queue it for the next lend.
pub fn deliver(ev: MacEv) {
    SCOPE.with(|s| s.deliver(ev, |shell, e| shell.apply(e)));
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
        fn did_become_key(&self, _n: &NSNotification) {
            deliver(MacEv::Key(*self.ivars(), true));
        }

        #[unsafe(method(windowDidMiniaturize:))]
        fn did_miniaturize(&self, _n: &NSNotification) {
            deliver(MacEv::Key(*self.ivars(), false));
        }

        #[unsafe(method(windowDidDeminiaturize:))]
        fn did_deminiaturize(&self, _n: &NSNotification) {
            deliver(MacEv::Key(*self.ivars(), true));
        }

        #[unsafe(method(windowDidResignKey:))]
        fn did_resign_key(&self, _n: &NSNotification) {
            deliver(MacEv::Key(*self.ivars(), false));
        }

        #[unsafe(method(windowWillEnterFullScreen:))]
        fn will_enter_full_screen(&self, _n: &NSNotification) {
            deliver(MacEv::FullScreen(*self.ivars(), true));
        }

        #[unsafe(method(windowWillExitFullScreen:))]
        fn will_exit_full_screen(&self, _n: &NSNotification) {
            deliver(MacEv::FullScreen(*self.ivars(), true));
        }

        #[unsafe(method(windowDidEnterFullScreen:))]
        fn did_enter_full_screen(&self, _n: &NSNotification) {
            deliver(MacEv::FullScreen(*self.ivars(), false));
        }

        #[unsafe(method(windowDidExitFullScreen:))]
        fn did_exit_full_screen(&self, _n: &NSNotification) {
            deliver(MacEv::FullScreen(*self.ivars(), false));
        }

        #[unsafe(method(windowDidFailToEnterFullScreen:))]
        fn did_fail_to_enter_full_screen(&self, _w: &objc2_app_kit::NSWindow) {
            deliver(MacEv::FullScreen(*self.ivars(), false));
        }

        #[unsafe(method(windowDidFailToExitFullScreen:))]
        fn did_fail_to_exit_full_screen(&self, _w: &objc2_app_kit::NSWindow) {
            deliver(MacEv::FullScreen(*self.ivars(), false));
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

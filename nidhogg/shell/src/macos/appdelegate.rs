//! The application delegate: quitting, files opened from Finder or the Dock, reopening, the Dock's menu and a
//! screen change.  Each is reported as an application event, which the shell posts as `ShellEvent::Menu` with
//! generation 0: C turns it into the menu-bar events of the key path Lisp registered for it (`nhmac-app-paths`), so
//! the commands `nhmac-quit`, `nhmac-open-files` and `nhmac-reopen` run in the command loop.

use super::menus;
use super::runloop::{MacEv, deliver};
use objc2::rc::Retained;
use objc2::runtime::NSObject;
use objc2::{MainThreadMarker, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{
    NSApplication, NSApplicationDelegate, NSApplicationTerminateReply, NSMenu, NSPasteboard,
};
use objc2_foundation::{NSArray, NSNotification, NSObjectProtocol, NSString};
use std::cell::RefCell;

/// The application events (`nhmac-app-paths` in `nhmac-win.el` has the same numbers).
pub const APP_OPEN_FILES: u32 = 1;
pub const APP_QUIT: u32 = 2;
pub const APP_REOPEN: u32 = 3;
pub const APP_APPEARANCE: u32 = 4;
/// 5 is the Dock menu's New Frame, whose number only Lisp's records carry.
pub const APP_SERVICE: u32 = 6;

thread_local! {
    /// The files `application:openFiles:` queued, until Lisp takes them.
    static OPEN_FILES: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
}

/// Queue `files` and tell Lisp.
pub fn open_files(files: Vec<String>) {
    OPEN_FILES.with(|f| f.borrow_mut().extend(files));
    deliver(MacEv::Menu(0, APP_OPEN_FILES));
}

/// The queued files, emptied (`take-open-files`).
pub fn take_open_files() -> Vec<String> {
    OPEN_FILES.with(|f| std::mem::take(&mut *f.borrow_mut()))
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "NhmacAppDelegate"]
    pub struct AppDelegate;

    unsafe impl NSObjectProtocol for AppDelegate {}

    unsafe impl NSApplicationDelegate for AppDelegate {
        /// Lisp decides (it asks when `ns-confirm-quit` is set, then kills Emacs): AppKit is told to wait.
        #[unsafe(method(applicationShouldTerminate:))]
        fn should_terminate(&self, _app: &NSApplication) -> NSApplicationTerminateReply {
            deliver(MacEv::Menu(0, APP_QUIT));
            NSApplicationTerminateReply::TerminateCancel
        }

        #[unsafe(method(application:openFiles:))]
        fn open_files(&self, app: &NSApplication, files: &NSArray<NSString>) {
            open_files(files.iter().map(|f| f.to_string()).collect());
            app.replyToOpenOrPrint(objc2_app_kit::NSApplicationDelegateReply::Success);
        }

        #[unsafe(method(applicationShouldHandleReopen:hasVisibleWindows:))]
        fn should_reopen(&self, _app: &NSApplication, visible: bool) -> bool {
            if !visible {
                deliver(MacEv::Menu(0, APP_REOPEN));
            }
            true
        }

        #[unsafe(method_id(applicationDockMenu:))]
        fn dock_menu(&self, _app: &NSApplication) -> Option<Retained<NSMenu>> {
            menus::dock_menu()
        }

        #[unsafe(method(applicationDidChangeScreenParameters:))]
        fn screens_changed(&self, _n: &NSNotification) {
            deliver(MacEv::Screens);
        }
    }

    impl AppDelegate {
        /// The services provider's one message (the delegate is the provider, as NSApp is on NS).
        #[unsafe(method(requestService:userData:error:))]
        fn request_service(&self, pb: &NSPasteboard, name: &NSString, _error: *mut *mut NSString) {
            super::services::request(pb, &name.to_string());
        }
    }
);

impl AppDelegate {
    pub fn new(mtm: MainThreadMarker) -> Retained<AppDelegate> {
        let this = Self::alloc(mtm).set_ivars(());
        // SAFETY: NSObject's designated initialiser, on a fresh allocation.
        unsafe { msg_send![super(this), init] }
    }
}

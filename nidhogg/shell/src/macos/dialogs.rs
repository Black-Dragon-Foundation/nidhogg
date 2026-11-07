//! `x-file-dialog`: `NSOpenPanel` (the file must exist) and `NSSavePanel`, run modally from `nhmac--query` with the
//! backend lent and AppKit's exceptions caught (the caller wraps the call).

use crate::pasteboard::DialogReq;
use objc2::MainThreadMarker;
use objc2_app_kit::{NSModalResponseOK, NSOpenPanel, NSSavePanel};
use objc2_foundation::{NSString, NSURL};

#[cfg(feature = "test-hooks")]
pub use auto::schedule_ok;

/// Run the panel `req` asks for; the chosen path, or `None` when it was cancelled.
pub fn run(mtm: MainThreadMarker, req: &DialogReq) -> Option<String> {
    let open;
    let save;
    let panel: &NSSavePanel = if req.must_match || req.only_dir {
        open = NSOpenPanel::openPanel(mtm);
        open.setCanChooseDirectories(req.only_dir);
        open.setCanChooseFiles(!req.only_dir);
        open.setAllowsMultipleSelection(false);
        &open
    } else {
        save = NSSavePanel::savePanel(mtm);
        &save
    };
    if !req.prompt.is_empty() {
        panel.setMessage(Some(&NSString::from_str(&req.prompt)));
    }
    if !req.dir.is_empty() {
        let dir = NSURL::fileURLWithPath_isDirectory(&NSString::from_str(&req.dir), true);
        panel.setDirectoryURL(Some(&dir));
    }
    if !req.default.is_empty() {
        panel.setNameFieldStringValue(&NSString::from_str(&req.default));
    }
    #[cfg(feature = "test-hooks")]
    auto::enter(panel);
    let response = panel.runModal();
    #[cfg(feature = "test-hooks")]
    auto::leave();
    if response != NSModalResponseOK {
        return None;
    }
    panel.URL().and_then(|u| u.path()).map(|p| p.to_string())
}

/// The test hook that answers the panel from inside the modal loop: a timer in every run loop mode sets the panel's
/// directory (and name) and presses OK.
#[cfg(feature = "test-hooks")]
mod auto {
    use objc2::Message;
    use objc2::rc::Retained;
    use objc2::runtime::{AnyObject, NSObject};
    use objc2::{MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
    use objc2_app_kit::NSSavePanel;
    use objc2_foundation::{NSArray, NSObjectProtocol, NSString, NSURL};
    use std::cell::RefCell;

    thread_local! {
        static PANEL: RefCell<Option<Retained<NSSavePanel>>> = const { RefCell::new(None) };
        /// What the next panel is answered with: a directory, or a file for a save panel.
        static ANSWER: RefCell<Option<String>> = const { RefCell::new(None) };
    }

    define_class!(
        #[unsafe(super(NSObject))]
        #[thread_kind = MainThreadOnly]
        #[name = "NhmacDialogAnswer"]
        struct Answer;

        unsafe impl NSObjectProtocol for Answer {}

        impl Answer {
            #[unsafe(method(answer:))]
            fn answer(&self, _sender: Option<&AnyObject>) {
                let Some(panel) = PANEL.with(|p| p.borrow().clone()) else {
                    return;
                };
                if let Some(path) = ANSWER.with(|a| a.borrow_mut().take()) {
                    let url = NSURL::fileURLWithPath(&NSString::from_str(&path));
                    let is_file = std::path::Path::new(&path).file_name().is_some()
                        && !std::path::Path::new(&path).is_dir();
                    if is_file {
                        if let Some(dir) = std::path::Path::new(&path).parent() {
                            let d = NSURL::fileURLWithPath_isDirectory(
                                &NSString::from_str(&dir.to_string_lossy()),
                                true,
                            );
                            panel.setDirectoryURL(Some(&d));
                        }
                        let name = std::path::Path::new(&path).file_name().map(|n| n.to_string_lossy().into_owned());
                        panel.setNameFieldStringValue(&NSString::from_str(&name.unwrap_or_default()));
                    } else {
                        panel.setDirectoryURL(Some(&url));
                    }
                }
                // SAFETY: `ok:` on a live panel, from inside its own modal loop.
                unsafe { panel.ok(None) };
            }
        }
    );

    pub fn enter(panel: &NSSavePanel) {
        PANEL.with(|p| *p.borrow_mut() = Some(panel.retain()));
    }

    pub fn leave() {
        PANEL.with(|p| *p.borrow_mut() = None);
    }

    /// Answer the next panel with `path` after `secs`.
    pub fn schedule_ok(mtm: MainThreadMarker, secs: f64, path: &str) {
        ANSWER.with(|a| *a.borrow_mut() = Some(path.to_string()));
        let this: Retained<Answer> = {
            let t = Answer::alloc(mtm).set_ivars(());
            // SAFETY: NSObject's designated initialiser on a fresh allocation.
            unsafe { msg_send![super(t), init] }
        };
        let modes = NSArray::from_retained_slice(&[NSString::from_str("kCFRunLoopCommonModes")]);
        // SAFETY: `answer:` is defined on Answer; the run loop retains the target until it fires.
        unsafe {
            let _: () = msg_send![&this, performSelector: sel!(answer:), withObject: std::ptr::null::<AnyObject>(), afterDelay: secs, inModes: &*modes];
        }
    }
}

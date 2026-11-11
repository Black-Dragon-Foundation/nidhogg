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
    if let Some(answered) = auto::leave() {
        return Some(answered);
    }
    if response != NSModalResponseOK {
        return None;
    }
    panel.URL().and_then(|u| u.path()).map(|p| p.to_string())
}

/// The test hook that answers the panel from inside the modal loop: a timer in every run loop mode ends the modal
/// session with `abortModal`, and the panel's answer is the path the hook was given.  Not the panel's OK button: the
/// panel runs out of process, and `ok:` (as `stopModal`) from a timer ends the session only at the next event, which
/// a lane with no input never sends, so the row hung.  The panel's own reading of its choice is not tested here.
#[cfg(feature = "test-hooks")]
mod auto {
    use objc2::Message;
    use objc2::rc::Retained;
    use objc2::runtime::{AnyObject, NSObject};
    use objc2::{MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
    use objc2_app_kit::{NSApplication, NSSavePanel};
    use objc2_foundation::{NSArray, NSObjectProtocol, NSString};
    use std::cell::RefCell;

    thread_local! {
        static PANEL: RefCell<Option<Retained<NSSavePanel>>> = const { RefCell::new(None) };
        /// What the next panel is answered with (a directory, or a file for a save panel), and after how many seconds.
        static ANSWER: RefCell<Option<(String, f64)>> = const { RefCell::new(None) };
        /// The answer the hook gave the panel that ran: its result.
        static ANSWERED: RefCell<Option<String>> = const { RefCell::new(None) };
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
                if PANEL.with(|p| p.borrow().is_none()) {
                    return;
                }
                let path = ANSWER.with(|a| a.borrow_mut().take()).map(|(p, _)| p);
                ANSWERED.with(|a| *a.borrow_mut() = path);
                let mtm = MainThreadMarker::from(self);
                // ends the modal session now, from a timer, with no event needed
                NSApplication::sharedApplication(mtm).abortModal();
            }
        }
    );

    /// A panel opens: its answer's timer starts now, so it fires inside the panel's modal loop, never before it.
    pub fn enter(panel: &NSSavePanel) {
        PANEL.with(|p| *p.borrow_mut() = Some(panel.retain()));
        let Some(secs) = ANSWER.with(|a| a.borrow().as_ref().map(|(_, s)| *s)) else {
            return;
        };
        let mtm = MainThreadMarker::from(panel);
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

    /// The panel's session ended: the hook's answer, when it gave one.
    pub fn leave() -> Option<String> {
        PANEL.with(|p| *p.borrow_mut() = None);
        ANSWERED.with(|a| a.borrow_mut().take())
    }

    /// Answer the next panel with `path`, `secs` after it opens.
    pub fn schedule_ok(_mtm: MainThreadMarker, secs: f64, path: &str) {
        ANSWER.with(|a| *a.borrow_mut() = Some((path.to_string(), secs)));
    }
}

//! The Services menu's requestor: `ns-perform-service` sends text to a service and returns what it answers.  (The
//! provider side, Emacs answering other applications' requests, comes with the application delegate.)

use objc2_app_kit::{NSPasteboard, NSPerformService};
use objc2_foundation::NSString;

const TEXT: &str = "public.utf8-plain-text";

/// Run service `name` on `text` through a private pasteboard: the text it leaves there, or `None` when the service
/// does not exist, refuses, or leaves nothing.
pub fn perform(name: &str, text: &str) -> Option<String> {
    let pb = NSPasteboard::pasteboardWithUniqueName();
    let ty = NSString::from_str(TEXT);
    pb.clearContents();
    if !pb.setString_forType(&NSString::from_str(text), &ty) {
        return None;
    }
    if !NSPerformService(&NSString::from_str(name), Some(&pb)) {
        return None;
    }
    pb.stringForType(&ty).map(|s| s.to_string())
}

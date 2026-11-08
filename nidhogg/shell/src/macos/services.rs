//! The Services menu: the requestor (`ns-perform-service` sends text to a service and returns what it answers) and
//! the provider (another application's request for one of the services `Info.plist` lists).  The provider queues, as
//! NS does: the request is an application event, and `nhmac-service` takes the queue and runs `ns-spi-service-call`
//! on each.

use super::appdelegate::APP_SERVICE;
use super::runloop::{MacEv, deliver};
use objc2_app_kit::{NSPasteboard, NSPerformService};
use objc2_foundation::NSString;
use std::cell::RefCell;

thread_local! {
    /// The requests `requestService:userData:error:` queued (the service's name, the text), until Lisp takes them.
    static REQUESTS: RefCell<Vec<(String, String)>> = const { RefCell::new(Vec::new()) };
}

/// `requestService:userData:error:`: queue the request for service `name` on `pb`'s text, and tell Lisp.
pub fn request(pb: &NSPasteboard, name: &str) {
    let text = pb
        .stringForType(&NSString::from_str(TEXT))
        .map(|s| s.to_string())
        .unwrap_or_default();
    REQUESTS.with(|r| r.borrow_mut().push((name.to_string(), text)));
    deliver(MacEv::Menu(0, APP_SERVICE));
}

/// `take-service-requests`: the queued requests as ((NAME . TEXT) ...), emptied.
pub fn take_requests_lisp() -> String {
    use crate::pasteboard::lisp_string;
    let all = REQUESTS.with(|r| std::mem::take(&mut *r.borrow_mut()));
    let items: Vec<String> = all
        .iter()
        .map(|(n, t)| format!("({} . {})", lisp_string(n), lisp_string(t)))
        .collect();
    format!("({})", items.join(" "))
}

/// Test hook: another application's request for service `name` on `text`, sent to the services provider as AppKit
/// sends it.
#[cfg(feature = "test-hooks")]
pub fn test_request(app: &objc2_app_kit::NSApplication, name: &str, text: &str) {
    let pb = NSPasteboard::pasteboardWithUniqueName();
    pb.clearContents();
    pb.setString_forType(&NSString::from_str(text), &NSString::from_str(TEXT));
    // SAFETY: the provider is the application delegate, which answers `requestService:userData:error:`.
    unsafe {
        let provider: Option<objc2::rc::Retained<objc2::runtime::AnyObject>> =
            objc2::msg_send![app, servicesProvider];
        if let Some(p) = provider {
            let _: () = objc2::msg_send![&*p, requestService: &*pb, userData: &*NSString::from_str(name), error: std::ptr::null_mut::<*mut NSString>()];
        }
    }
}

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

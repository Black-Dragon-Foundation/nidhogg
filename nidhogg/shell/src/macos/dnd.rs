//! Drag and drop on AppKit: the drop target (`NSDraggingDestination` on the root view) and the drag source.
//!
//! **Target.**  AppKit asks synchronously while Lisp answers later, so the answer is pushed state: `draggingEntered:`
//! and `draggingUpdated:` return the operation `crate::dnd_map::answer` gives from what Lisp last said
//! (`ShellBackend::dnd_status`), optimistic before it says anything.  The enter, motion, leave and drop go to the host
//! as records; the drop is exact (`performDragOperation:` accepts what Lisp accepted).  The offer is the dragging
//! pasteboard, readable as `NHSHELL_SEL_DND` until the leave or the finish.
//!
//! **Source.**  `beginDraggingSession` returns at once; the session runs in the main loop the pumps spin, and its end
//! posts `DragDone`.  The drag starts from the newest mouse press, which the view hands to [`note_press`].

use super::runloop::{MacEv, deliver};
use crate::api::{ShellError, ShellResult, WindowId};
use crate::dnd_map::{self, answer, mime_of, uti_of};
use crate::event::{DndEv, ShellEvent};
use objc2::Message;
use objc2::rc::Retained;
use objc2::runtime::{NSObject, ProtocolObject};
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{
    NSDragOperation, NSDraggingContext, NSDraggingInfo, NSDraggingItem, NSDraggingSession,
    NSDraggingSource, NSEvent, NSPasteboardItem, NSView,
};
use objc2_foundation::{NSArray, NSData, NSObjectProtocol, NSPoint, NSRect, NSSize, NSString};
use std::cell::{Cell, RefCell};

/// The drag over one of our windows.
struct Offer {
    info: Retained<ProtocolObject<dyn NSDraggingInfo>>,
    source_ops: usize,
    own: bool,
}

/// What Lisp last answered: accepted, its actions, and the one it prefers.
#[derive(Clone, Copy, Default)]
struct Reply {
    accepted: bool,
    actions: u32,
    preferred: u32,
}

thread_local! {
    static OFFER: RefCell<Option<Offer>> = const { RefCell::new(None) };
    static REPLY: Cell<Reply> = const { Cell::new(Reply { accepted: false, actions: 0, preferred: 0 }) };
    /// The newest mouse press, the event a drag out starts from.
    static PRESS: RefCell<Option<Retained<NSEvent>>> = const { RefCell::new(None) };
    /// The drag out now running, and the bytes it carries (readable as the offer of our own drops).
    static SOURCE: RefCell<Option<Retained<Source>>> = const { RefCell::new(None) };
    static OUT_ITEMS: RefCell<Vec<(String, Vec<u8>)>> = const { RefCell::new(Vec::new()) };
}

/// The types a root view takes drops of.
pub const DROP_TYPES: [&str; 3] = ["public.file-url", "public.utf8-plain-text", "public.url"];

/// Register `view` for drops.
pub fn register(view: &NSView) {
    let types: Vec<Retained<NSString>> = DROP_TYPES.iter().map(|t| NSString::from_str(t)).collect();
    view.registerForDraggedTypes(&NSArray::from_retained_slice(&types));
}

/// The press the next drag out starts from.
pub fn note_press(e: &NSEvent) {
    PRESS.with(|p| *p.borrow_mut() = Some(e.retain()));
}

fn target_reply() -> usize {
    let (r, ops) = (
        REPLY.with(Cell::get),
        OFFER.with(|o| o.borrow().as_ref().map_or(0, |o| o.source_ops)),
    );
    answer(r.accepted, ops, r.actions, r.preferred)
}

fn record(
    id: WindowId,
    view: &NSView,
    info: &ProtocolObject<dyn NSDraggingInfo>,
    make: fn(DndEv) -> ShellEvent,
) {
    let p = view.convertPoint_fromView(info.draggingLocation(), None);
    let scale = view.window().map_or(1.0, |w| w.backingScaleFactor());
    let (x, y) = crate::mac_input::device_point(p.x, p.y, view.bounds().size.height, scale);
    let ops = info.draggingSourceOperationMask().0;
    let actions = dnd_map::actions_of(ops);
    deliver(MacEv::Input(
        id,
        make(DndEv {
            x,
            y,
            source_actions: actions,
            preferred: dnd_map::first_action(actions),
            own: u8::from(info.draggingSource().is_some()),
            ..DndEv::default()
        }),
    ));
}

/// `draggingEntered:`.
pub fn entered(id: WindowId, view: &NSView, info: &ProtocolObject<dyn NSDraggingInfo>) -> usize {
    OFFER.with(|o| {
        *o.borrow_mut() = Some(Offer {
            info: info.retain(),
            source_ops: info.draggingSourceOperationMask().0,
            own: info.draggingSource().is_some(),
        });
    });
    // optimistic until Lisp answers: the source's first operation
    REPLY.with(|r| {
        r.set(Reply {
            accepted: true,
            ..Reply::default()
        })
    });
    record(id, view, info, ShellEvent::DndEnter);
    target_reply()
}

/// `draggingUpdated:`.
pub fn updated(id: WindowId, view: &NSView, info: &ProtocolObject<dyn NSDraggingInfo>) -> usize {
    record(id, view, info, ShellEvent::DndMotion);
    target_reply()
}

/// `draggingExited:`.
pub fn exited(id: WindowId, view: &NSView, info: Option<&ProtocolObject<dyn NSDraggingInfo>>) {
    if let Some(info) = info {
        record(id, view, info, ShellEvent::DndLeave);
    } else {
        deliver(MacEv::Input(id, ShellEvent::DndLeave(DndEv::default())));
    }
    OFFER.with(|o| *o.borrow_mut() = None);
}

/// `performDragOperation:`: whether the drop is taken.  The offer stays until `dnd_finish`.
pub fn dropped(id: WindowId, view: &NSView, info: &ProtocolObject<dyn NSDraggingInfo>) -> bool {
    record(id, view, info, ShellEvent::DndDrop);
    REPLY.with(Cell::get).accepted
}

/// `ShellBackend::dnd_status`.
pub fn status(mime: Option<&str>, actions: u32, preferred: u32) -> ShellResult {
    if OFFER.with(|o| o.borrow().is_none()) {
        return Err(ShellError::State);
    }
    REPLY.with(|r| {
        r.set(Reply {
            accepted: mime.is_some() && actions != 0,
            actions,
            preferred,
        });
    });
    Ok(())
}

/// `ShellBackend::dnd_finish`: the offer goes, in every case.
pub fn finish(_success: bool) -> ShellResult {
    OFFER
        .with(|o| o.borrow_mut().take())
        .map(drop)
        .ok_or(ShellError::State)
}

/// `selection_types(NHSHELL_SEL_DND)`: the offer's types as MIME types.
pub fn types() -> Vec<String> {
    if OUT_ITEMS.with(|o| !o.borrow().is_empty())
        && OFFER.with(|o| o.borrow().as_ref().is_some_and(|o| o.own))
    {
        return OUT_ITEMS.with(|o| o.borrow().iter().map(|(m, _)| m.clone()).collect());
    }
    OFFER.with(|o| {
        o.borrow()
            .as_ref()
            .and_then(|o| o.info.draggingPasteboard().types())
            .map(|t| t.iter().map(|u| mime_of(&u.to_string())).collect())
            .unwrap_or_default()
    })
}

/// `selection_read(NHSHELL_SEL_DND, mime)`: the offer's bytes as `mime`.
pub fn read(mime: &str) -> ShellResult<Vec<u8>> {
    if let Some(bytes) = OUT_ITEMS.with(|o| {
        o.borrow()
            .iter()
            .find(|(m, _)| m == mime)
            .map(|(_, b)| b.clone())
    }) && OFFER.with(|o| o.borrow().as_ref().is_some_and(|o| o.own))
    {
        return Ok(bytes);
    }
    let pb = OFFER
        .with(|o| o.borrow().as_ref().map(|o| o.info.draggingPasteboard()))
        .ok_or(ShellError::State)?;
    let uti = NSString::from_str(&uti_of(mime));
    if mime == "text/uri-list" {
        let urls: Vec<String> = pb
            .pasteboardItems()
            .map(|items| {
                items
                    .iter()
                    .filter_map(|i| i.stringForType(&uti).map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default();
        return if urls.is_empty() {
            Err(ShellError::State)
        } else {
            Ok(dnd_map::uri_list(&urls).into_bytes())
        };
    }
    pb.dataForType(&uti)
        .map(|d| d.to_vec())
        .ok_or(ShellError::State)
}

define_class!(
    /// The drag source: the operations it allows, and the end of the session.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "NhmacDragSource"]
    #[ivars = (usize, WindowId)]
    struct Source;

    unsafe impl NSObjectProtocol for Source {}

    unsafe impl NSDraggingSource for Source {
        #[unsafe(method(draggingSession:sourceOperationMaskForDraggingContext:))]
        fn mask(&self, _s: &NSDraggingSession, _c: NSDraggingContext) -> NSDragOperation {
            NSDragOperation(self.ivars().0)
        }

        #[unsafe(method(draggingSession:endedAtPoint:operation:))]
        fn ended(&self, _s: &NSDraggingSession, _p: NSPoint, op: NSDragOperation) {
            let action = dnd_map::actions_of(op.0);
            deliver(MacEv::Input(
                self.ivars().1,
                ShellEvent::DragDone(DndEv {
                    action,
                    ok: u8::from(action != 0),
                    ..DndEv::default()
                }),
            ));
            SOURCE.with(|s| *s.borrow_mut() = None);
        }
    }
);

/// `ShellBackend::drag_start`: drag `items` out of `view`, from the newest press.
pub fn start(
    mtm: MainThreadMarker,
    id: WindowId,
    view: &NSView,
    items: &[(&str, &[u8])],
    actions: u32,
) -> ShellResult {
    let press = PRESS
        .with(|p| p.borrow().clone())
        .ok_or(ShellError::State)?;
    let item = NSPasteboardItem::new();
    for (mime, bytes) in items {
        item.setData_forType(
            &NSData::with_bytes(bytes),
            &NSString::from_str(&uti_of(mime)),
        );
    }
    OUT_ITEMS.with(|o| {
        *o.borrow_mut() = items
            .iter()
            .map(|(m, b)| (m.to_string(), b.to_vec()))
            .collect();
    });
    let source: Retained<Source> = {
        let t = Source::alloc(mtm).set_ivars((dnd_map::ops_of(actions), id));
        // SAFETY: NSObject's designated initialiser, on a fresh allocation with its ivars set.
        unsafe { msg_send![super(t), init] }
    };
    let dragging = NSDraggingItem::initWithPasteboardWriter(
        mtm.alloc::<NSDraggingItem>(),
        ProtocolObject::from_ref(&*item),
    );
    let at = view.convertPoint_fromView(press.locationInWindow(), None);
    // SAFETY: a frame and no image contents, which AppKit takes (the drag shows no icon).
    unsafe { dragging.setDraggingFrame_contents(NSRect::new(at, NSSize::new(16.0, 16.0)), None) };
    let _session = view.beginDraggingSessionWithItems_event_source(
        &NSArray::from_retained_slice(&[dragging]),
        &press,
        ProtocolObject::from_ref(&*source),
    );
    SOURCE.with(|s| *s.borrow_mut() = Some(source));
    Ok(())
}

/// `ShellBackend::drag_cancel`.
pub fn cancel() {
    SOURCE.with(|s| *s.borrow_mut() = None);
}

/// `dnd-offer`: the dropped offer as NS's `drag-n-drop` event carries it, for `ns-drag-n-drop`: `(url
/// (ns-drag-operation-copy) URL...)` for files, `(string nil TEXT)` for text, `nil` without an offer.
pub fn offer_lisp() -> String {
    use crate::pasteboard::lisp_string;
    if let Ok(list) = read("text/uri-list") {
        let urls: Vec<String> = String::from_utf8_lossy(&list)
            .split("\r\n")
            .filter(|u| !u.is_empty())
            .map(lisp_string)
            .collect();
        if !urls.is_empty() {
            return format!("(url (ns-drag-operation-copy) {})", urls.join(" "));
        }
    }
    match read("text/plain;charset=utf-8") {
        Ok(text) => format!(
            "(string nil {})",
            lisp_string(&String::from_utf8_lossy(&text))
        ),
        Err(_) => "nil".into(),
    }
}

#[cfg(feature = "test-hooks")]
define_class!(
    /// Test hook: the dragging info AppKit hands a drop target (answering as `NSDraggingInfo` does), for a drag no
    /// window server runs: a private
    /// pasteboard, a place in the view's window, copy or link from a source in another application.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "NhmacTestDraggingInfo"]
    #[ivars = (Retained<objc2_app_kit::NSPasteboard>, NSPoint)]
    struct TestInfo;

    unsafe impl NSObjectProtocol for TestInfo {}

    // Only what the view reads, not the protocol: declaring NSDraggingInfo would make objc2 require all of its
    // methods when the class is registered (and panic without them); the view's methods take any object.
    impl TestInfo {
        #[unsafe(method_id(draggingPasteboard))]
        fn pasteboard(&self) -> Retained<objc2_app_kit::NSPasteboard> {
            self.ivars().0.clone()
        }

        #[unsafe(method(draggingLocation))]
        fn location(&self) -> NSPoint {
            self.ivars().1
        }

        #[unsafe(method(draggingSourceOperationMask))]
        fn mask(&self) -> NSDragOperation {
            NSDragOperation::Copy | NSDragOperation::Link | NSDragOperation::Generic
        }

        #[unsafe(method(draggingSource))]
        fn source(&self) -> *mut objc2::runtime::AnyObject {
            std::ptr::null_mut()
        }

        #[unsafe(method(draggingSequenceNumber))]
        fn sequence(&self) -> isize {
            1
        }
    }
);

/// Test hook: drop `payload` (a file's path when `file`, else text) on the middle of `view` as AppKit does: the
/// view's `draggingEntered:`, `draggingUpdated:` and `performDragOperation:`.  Whether the drop was taken.
#[cfg(feature = "test-hooks")]
pub fn test_drop(
    mtm: MainThreadMarker,
    view: &NSView,
    file: bool,
    payload: &str,
) -> ShellResult<bool> {
    let pb = objc2_app_kit::NSPasteboard::pasteboardWithUniqueName();
    pb.clearContents();
    let (text, ty) = if file {
        let url = objc2_foundation::NSURL::fileURLWithPath(&NSString::from_str(payload));
        (
            url.absoluteString().ok_or(ShellError::State)?,
            dnd_map::FILE_UTI,
        )
    } else {
        (NSString::from_str(payload), dnd_map::TEXT_UTI)
    };
    if !pb.setString_forType(&text, &NSString::from_str(ty)) {
        return Err(ShellError::State);
    }
    let b = view.bounds();
    let mid = NSPoint::new(b.size.width / 2.0, b.size.height / 2.0);
    let at = view.convertPoint_toView(mid, None);
    let info: Retained<TestInfo> = {
        let t = TestInfo::alloc(mtm).set_ivars((pb, at));
        // SAFETY: NSObject's designated initialiser, on a fresh allocation with its ivars set.
        unsafe { msg_send![super(t), init] }
    };
    // SAFETY: the view is the NhView, a dragging destination; the info answers what its methods read.
    unsafe {
        let _: NSDragOperation = msg_send![view, draggingEntered: &*info];
        let _: NSDragOperation = msg_send![view, draggingUpdated: &*info];
        Ok(msg_send![view, performDragOperation: &*info])
    }
}

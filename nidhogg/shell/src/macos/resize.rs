//! Our own edge-drag resize: the window class whose `sendEvent:` takes a left press in the resize band
//! (`crate::resize::hit`) before AppKit sees it, so AppKit's live-resize loop never starts (`inLiveResize` stays NO),
//! and turns the drag that follows into frames to ask for (`MacEv::Resize`).  The shell sends each as a configure and
//! moves the window to it only when the host has committed a surface of that size (`app.rs`'s `commit`), so a
//! surface of the wrong size is never shown.  Presses outside the band, and every other event, go to AppKit
//! unchanged; the green button, tiling and the Window menu's Move & Resize items still resize through `setFrame:`.
//!
//! The suppressed path: for a band press, `[super sendEvent:]` is skipped for the `LeftMouseDown`, and for every
//! `LeftMouseDragged` and the `LeftMouseUp` until the button is up.

use super::runloop::{MacEv, deliver};
use crate::api::WindowId;
use crate::resize::{self, BAND, BOTTOM, Edges, LEFT, Mods, RIGHT, Rect, TOP};
use objc2::rc::Retained;
use objc2::{
    ClassType, DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel,
};
use objc2_app_kit::{
    NSBackingStoreType, NSCursor, NSCursorFrameResizeDirections, NSCursorFrameResizePosition,
    NSEvent, NSEventModifierFlags, NSEventType, NSResponder, NSWindow, NSWindowStyleMask,
};
use objc2_foundation::{NSObjectProtocol, NSPoint, NSRect, NSSize};
use std::cell::Cell;
use std::sync::atomic::{AtomicU64, Ordering};

/// Band presses `sendEvent:` took (`(nhmac-stats)`'s `:band-presses`, for the rows).
pub static BAND_PRESSES: AtomicU64 = AtomicU64::new(0);

/// A drag that started in the band: the frame and the pointer (screen points) at the press, and the edges.
#[derive(Clone, Copy, Debug)]
struct Drag {
    start: Rect,
    at: NSPoint,
    edges: Edges,
}

pub struct Ivars {
    id: WindowId,
    drag: Cell<Option<Drag>>,
}

fn rect(r: NSRect) -> Rect {
    Rect {
        x: r.origin.x,
        y: r.origin.y,
        w: r.size.width,
        h: r.size.height,
    }
}

/// `r` in whole points, as the frames we ask for are.
fn ns_rect(r: Rect) -> NSRect {
    NSRect::new(
        NSPoint::new(r.x.round(), r.y.round()),
        NSSize::new(r.w.round(), r.h.round()),
    )
}

define_class!(
    /// A top-level window of host window `ivars.id`.
    #[unsafe(super(NSWindow, NSResponder, objc2_foundation::NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "NhmacWindow"]
    #[ivars = Ivars]
    pub struct NhWindow;

    unsafe impl NSObjectProtocol for NhWindow {}

    impl NhWindow {
        #[unsafe(method(sendEvent:))]
        fn send_event(&self, ev: &NSEvent) {
            if !self.take(ev) {
                // SAFETY: NSWindow's own `sendEvent:` with the event AppKit gave us.
                unsafe { msg_send![super(self), sendEvent: ev] }
            }
        }
    }
);

impl NhWindow {
    /// A window as `-[NSWindow initWithContentRect:styleMask:backing:defer:]` makes one, for host window `id`.
    pub fn new(
        mtm: MainThreadMarker,
        id: WindowId,
        content: NSRect,
        style: NSWindowStyleMask,
    ) -> Retained<NhWindow> {
        let this = Self::alloc(mtm).set_ivars(Ivars {
            id,
            drag: Cell::new(None),
        });
        // SAFETY: NSWindow's designated initialiser on a fresh allocation with its ivars set; `Buffered` is the only
        // backing AppKit has.
        unsafe {
            msg_send![super(this), initWithContentRect: content, styleMask: style,
                      backing: NSBackingStoreType::Buffered, defer: false]
        }
    }

    /// Whether the band applies: a resizable window, not in full screen.
    fn resizable(&self) -> bool {
        let style = self.styleMask();
        style.contains(NSWindowStyleMask::Resizable)
            && !style.contains(NSWindowStyleMask::FullScreen)
    }

    /// The edges a pointer at `ev`'s location grabs, if the band applies.
    fn band(&self, ev: &NSEvent) -> Option<Edges> {
        if !self.resizable() {
            return None;
        }
        let p = self.convertPointToScreen(ev.locationInWindow());
        resize::hit(rect(self.frame()), (p.x, p.y), BAND)
    }

    /// The smallest frame: the content's minimum (`set_min_size`) with the title bar.
    fn min_frame(&self) -> (f64, f64) {
        let min = self.contentMinSize();
        let f = self.frameRectForContentRect(NSRect::new(NSPoint::new(0.0, 0.0), min));
        (f.size.width.max(1.0), f.size.height.max(1.0))
    }

    /// Handle `ev` here (true) or leave it to AppKit (false).
    fn take(&self, ev: &NSEvent) -> bool {
        let drag = &self.ivars().drag;
        match ev.r#type() {
            NSEventType::LeftMouseDown => match self.band(ev) {
                Some(edges) => {
                    BAND_PRESSES.fetch_add(1, Ordering::Relaxed);
                    drag.set(Some(Drag {
                        start: rect(self.frame()),
                        at: self.convertPointToScreen(ev.locationInWindow()),
                        edges,
                    }));
                    cursor(edges).set();
                    true
                }
                None => false,
            },
            NSEventType::LeftMouseDragged => {
                let Some(d) = drag.get() else {
                    return false;
                };
                let p = self.convertPointToScreen(ev.locationInWindow());
                let flags = ev.modifierFlags();
                let mods = Mods {
                    symmetric: flags.contains(NSEventModifierFlags::Option),
                    aspect: flags.contains(NSEventModifierFlags::Shift),
                };
                let want = resize::drag(
                    d.start,
                    d.edges,
                    p.x - d.at.x,
                    p.y - d.at.y,
                    mods,
                    self.min_frame(),
                );
                deliver(MacEv::Resize(self.ivars().id, ns_rect(want)));
                true
            }
            NSEventType::LeftMouseUp => drag.take().is_some(),
            NSEventType::MouseMoved => {
                if drag.get().is_none()
                    && let Some(edges) = self.band(ev)
                {
                    cursor(edges).set();
                }
                false
            }
            _ => false,
        }
    }
}

/// The system's resize cursor for `edges`: `frameResizeCursorFromPosition:inDirections:` where AppKit has it, the
/// older edge cursors otherwise (they have none for a corner: the corner's horizontal edge's).
pub fn cursor(edges: Edges) -> Retained<NSCursor> {
    let position = match edges {
        e if e == TOP | LEFT => NSCursorFrameResizePosition::TopLeft,
        e if e == TOP | RIGHT => NSCursorFrameResizePosition::TopRight,
        e if e == BOTTOM | LEFT => NSCursorFrameResizePosition::BottomLeft,
        e if e == BOTTOM | RIGHT => NSCursorFrameResizePosition::BottomRight,
        TOP => NSCursorFrameResizePosition::Top,
        BOTTOM => NSCursorFrameResizePosition::Bottom,
        LEFT => NSCursorFrameResizePosition::Left,
        _ => NSCursorFrameResizePosition::Right,
    };
    // SAFETY: a class method of NSCursor's, asked for by its selector.
    let modern: bool = unsafe {
        msg_send![
            NSCursor::class(),
            respondsToSelector: sel!(frameResizeCursorFromPosition:inDirections:)
        ]
    };
    if modern {
        return NSCursor::frameResizeCursorFromPosition_inDirections(
            position,
            NSCursorFrameResizeDirections::All,
        );
    }
    #[allow(deprecated)]
    if edges & (LEFT | RIGHT) != 0 {
        NSCursor::resizeLeftRightCursor()
    } else {
        NSCursor::resizeUpDownCursor()
    }
}

/// A resize in progress, on the shell's side: the frame whose configure the host is answering, and the newest frame
/// asked for since (drag steps coalesce to it while a redraw runs).
#[derive(Debug, Default)]
pub struct Pending {
    /// The frame asked for, and its content size in device pixels.
    pub inflight: Option<(NSRect, (i32, i32))>,
    pub waiting: resize::Coalesce<NSRect>,
}

/// Test hook: whether the current cursor is the resize cursor for `edges`.
#[cfg(feature = "test-hooks")]
pub fn test_cursor_is(edges: Edges) -> bool {
    let want = cursor(edges);
    NSCursor::currentCursor().isEqual(Some(want.as_ref()))
}

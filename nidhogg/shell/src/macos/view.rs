//! A window's root view: layer-hosting, so Core Animation shows our layers as they are and AppKit draws nothing.

use objc2::rc::Retained;
use objc2::{MainThreadMarker, MainThreadOnly};
use objc2_app_kit::NSView;
use objc2_foundation::{NSPoint, NSRect, NSSize};
use objc2_quartz_core::CALayer;

/// A view of `size` points whose layer is ours.  The layer is set before `wantsLayer`: that order makes the view
/// layer-hosting rather than layer-backed.
pub fn layer_hosting(mtm: MainThreadMarker, size: NSSize) -> (Retained<NSView>, Retained<CALayer>) {
    let view = NSView::initWithFrame(
        NSView::alloc(mtm),
        NSRect::new(NSPoint::new(0.0, 0.0), size),
    );
    let layer = CALayer::new();
    view.setLayer(Some(&layer));
    view.setWantsLayer(true);
    (view, layer)
}

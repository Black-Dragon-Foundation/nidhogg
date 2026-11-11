//! Overhang panels (design §5.6): a child window whose content crosses its root's bounds lives in a borderless,
//! non-activating child `NSPanel` of the root's window, as big as the child's surface grown by its shadow's reach,
//! until it fits again (`crate::mac_child::overhangs`).  The child's container layer moves into the panel's root layer
//! with no redraw.
//!
//! **The switch is ordered, not atomic:** two windows cannot commit together in WindowServer, so going out the panel
//! is ordered in already showing the child's surface before the container leaves the root, and going back the
//! container is in the root again before the panel is ordered out.  The panel never becomes key: keys stay with the
//! root's window.  Its view reports as the child, and the backend hit-tests its clicks in the root's tree, so a click
//! on the margin reaches the parent frame.
//!
//! **Blur:** a child with a backdrop blur lives in its panel for as long as the blur is on, with an
//! `NSVisualEffectView` (Popover material, behind-window blending) under its view, at the child's place and masked to
//! its rounded corners: the window server blurs what lies behind the panel, the root's window included.

use super::view::{self, NhView};
use crate::api::WindowId;
use objc2::encode::{Encoding, RefEncode};
use objc2::msg_send;
use objc2::rc::Retained;
use objc2::{AllocAnyThread, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSAutoresizingMaskOptions, NSBackingStoreType, NSColor, NSImage, NSImageResizingMode, NSPanel,
    NSView, NSVisualEffectBlendingMode, NSVisualEffectMaterial, NSVisualEffectState,
    NSVisualEffectView, NSWindow, NSWindowOrderingMode, NSWindowStyleMask,
};
use objc2_foundation::{NSEdgeInsets, NSSize};
use objc2_foundation::{NSPoint, NSRect};
use objc2_quartz_core::CALayer;
use std::ffi::c_void;

pub struct Panel {
    pub panel: Retained<NSPanel>,
    _view: Retained<NhView>,
    /// The panel's root layer: the child's container while it is out.
    pub layer: Retained<CALayer>,
    /// A sublayer of `layer` at the child's surface's place: the surface as contents while it switches.
    pub standin: Retained<CALayer>,
    /// The panel's content view: the blur (when on) under the child's view.
    content: Retained<NSView>,
    /// The blur under the child, and the corner radius (points) its mask was made for.
    blur: Option<(Retained<NSVisualEffectView>, f64)>,
    /// The panel is ordered in, as a child window of the root's.
    pub shown: bool,
}

impl Panel {
    /// A hidden panel at `frame` (screen points) for child `id`.
    pub fn new(mtm: MainThreadMarker, id: WindowId, frame: NSRect) -> Panel {
        let panel = NSPanel::initWithContentRect_styleMask_backing_defer(
            NSPanel::alloc(mtm),
            frame,
            NSWindowStyleMask::Borderless | NSWindowStyleMask::NonactivatingPanel,
            NSBackingStoreType::Buffered,
            false,
        );
        // SAFETY: we hold the panel in a `Retained`; AppKit must not release it again when it closes.
        unsafe { panel.setReleasedWhenClosed(false) };
        panel.setOpaque(false);
        panel.setBackgroundColor(Some(&NSColor::clearColor()));
        panel.setHasShadow(false);
        panel.setBecomesKeyOnlyIfNeeded(true);
        let (view, layer) = view::layer_hosting(mtm, id, frame.size);
        let content = NSView::initWithFrame(
            NSView::alloc(mtm),
            NSRect::new(NSPoint::new(0.0, 0.0), frame.size),
        );
        view.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable
                | NSAutoresizingMaskOptions::ViewHeightSizable,
        );
        content.addSubview(&view);
        panel.setContentView(Some(&content));
        let standin = CALayer::new();
        standin.setAnchorPoint(NSPoint::new(0.0, 0.0));
        layer.addSublayer(&standin);
        Panel {
            panel,
            _view: view,
            layer,
            standin,
            content,
            blur: None,
            shown: false,
        }
    }

    /// Blur what lies behind the panel under the child (`inner`, points from the panel's bottom left, corners of
    /// `radius` points), or stop (`None`).
    pub fn set_blur(&mut self, mtm: MainThreadMarker, blur: Option<(NSRect, f64)>) {
        let Some((inner, radius)) = blur else {
            if let Some((v, _)) = self.blur.take() {
                v.removeFromSuperview();
            }
            return;
        };
        // a new radius needs a new mask: the view is made again
        if self.blur.as_ref().is_some_and(|(_, r)| *r != radius)
            && let Some((v, _)) = self.blur.take()
        {
            v.removeFromSuperview();
        }
        let (v, _) = self.blur.get_or_insert_with(|| {
            let v = NSVisualEffectView::initWithFrame(NSVisualEffectView::alloc(mtm), inner);
            v.setMaterial(NSVisualEffectMaterial::Popover);
            v.setBlendingMode(NSVisualEffectBlendingMode::BehindWindow);
            v.setState(NSVisualEffectState::Active);
            if let Some(m) = corner_mask(radius) {
                v.setMaskImage(Some(&m));
            }
            // under the child's view
            self.content
                .addSubview_positioned_relativeTo(&v, NSWindowOrderingMode::Below, None);
            (v, radius)
        });
        if v.frame() != inner {
            v.setFrame(inner);
        }
    }

    /// Whether the panel blurs what lies behind it.
    #[cfg_attr(not(feature = "test-hooks"), allow(dead_code))]
    pub fn blurs(&self) -> bool {
        self.blur.is_some()
    }

    /// Put the panel at `frame` (screen points), and its stand-in at `inner` (points from the panel's bottom left);
    /// its layer's size follows the view's.
    pub fn place(&self, frame: NSRect, inner: NSRect) {
        let f = self.panel.frame();
        if f.origin != frame.origin || f.size != frame.size {
            self.panel.setFrame_display(frame, false);
        }
        if self.standin.position() != inner.origin || self.standin.bounds().size != inner.size {
            self.standin.setPosition(inner.origin);
            self.standin
                .setBounds(NSRect::new(NSPoint::new(0.0, 0.0), inner.size));
        }
    }

    /// Order the panel in, above the root's window `parent` and moving with it.
    pub fn order_in(&mut self, parent: &NSWindow) {
        if !self.shown {
            // SAFETY: both windows are ours and live; the panel is not a child window of any other.
            unsafe { parent.addChildWindow_ordered(&self.panel, NSWindowOrderingMode::Above) };
            self.panel.orderFront(None);
            self.shown = true;
        }
    }

    /// Order the panel out.
    pub fn order_out(&mut self, parent: &NSWindow) {
        if self.shown {
            parent.removeChildWindow(&self.panel);
            self.panel.orderOut(None);
            self.shown = false;
        }
    }
}

impl Drop for Panel {
    fn drop(&mut self) {
        if let Some(p) = self.panel.parentWindow() {
            p.removeChildWindow(&self.panel);
        }
        self.panel.orderOut(None);
        self.panel.close();
    }
}

/// Core Graphics' image, as `initWithCGImage:size:` takes it.
#[repr(C)]
struct CGImage {
    _p: [u8; 0],
}

// SAFETY: a CGImageRef is a pointer to an opaque `struct CGImage`, encoded as such.
unsafe impl RefEncode for CGImage {
    const ENCODING_REF: Encoding = Encoding::Pointer(&Encoding::Struct("CGImage", &[]));
}

type CFRef = *const c_void;

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGBitmapContextCreate(
        data: *mut c_void,
        w: usize,
        h: usize,
        bits: usize,
        row: usize,
        cs: CFRef,
        info: u32,
    ) -> *mut c_void;
    fn CGBitmapContextCreateImage(c: *mut c_void) -> CFRef;
    fn CGContextRelease(c: *mut c_void);
    fn CGContextSetGrayFillColor(c: *mut c_void, gray: f64, alpha: f64);
    fn CGPathCreateWithRoundedRect(rect: NSRect, cw: f64, ch: f64, t: *const c_void) -> CFRef;
    fn CGContextAddPath(c: *mut c_void, path: CFRef);
    fn CGContextFillPath(c: *mut c_void);
    fn CGPathRelease(path: CFRef);
    fn CGImageRelease(img: CFRef);
}

/// `kCGImageAlphaOnly`: an alpha channel alone.
const ALPHA_ONLY: u32 = 7;
/// The mask's pixels per point: enough for a Retina screen's edge.
const MASK_SCALE: f64 = 2.0;

/// The blur's mask: a rounded rectangle of `radius` points, stretchable between its corners (`capInsets`).
fn corner_mask(radius: f64) -> Option<Retained<NSImage>> {
    let side = 2.0 * radius.max(0.0) + 1.0;
    let px = (side * MASK_SCALE).ceil() as usize;
    let mut buf = vec![0u8; px * px];
    // SAFETY: the context draws into `buf`, alive for the calls; the image copies it; every +1 is released.
    let img = unsafe {
        let c = CGBitmapContextCreate(
            buf.as_mut_ptr().cast(),
            px,
            px,
            8,
            px,
            std::ptr::null(),
            ALPHA_ONLY,
        );
        if c.is_null() {
            return None;
        }
        let r = radius * MASK_SCALE;
        let path = CGPathCreateWithRoundedRect(
            NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(px as f64, px as f64)),
            r,
            r,
            std::ptr::null(),
        );
        CGContextSetGrayFillColor(c, 0.0, 1.0);
        CGContextAddPath(c, path);
        CGContextFillPath(c);
        CGPathRelease(path);
        let i = CGBitmapContextCreateImage(c);
        CGContextRelease(c);
        i
    };
    if img.is_null() {
        return None;
    }
    // SAFETY: a valid CGImage, retained by the NSImage, then released once.
    let image: Retained<NSImage> = unsafe {
        let m: Retained<NSImage> = msg_send![NSImage::alloc(), initWithCGImage: img.cast::<CGImage>(), size: NSSize::new(side, side)];
        CGImageRelease(img);
        m
    };
    image.setCapInsets(NSEdgeInsets {
        top: radius,
        left: radius,
        bottom: radius,
        right: radius,
    });
    image.setResizingMode(NSImageResizingMode::Stretch);
    Some(image)
}

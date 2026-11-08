//! A top-level window: an `NSWindow` whose content view is layer-hosting and its first responder
//! (`view::layer_hosting`).

use super::runloop::WinDelegate;
use super::surface::Surfaces;
use super::view::{self, NhView};
use crate::api::WindowId;
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2::{MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{NSBackingStoreType, NSWindow, NSWindowStyleMask};
use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};
use objc2_quartz_core::CALayer;

pub struct Win {
    pub window: Retained<NSWindow>,
    // read by the test hooks
    #[cfg_attr(not(feature = "test-hooks"), allow(dead_code))]
    pub view: Retained<NhView>,
    /// The root layer: the surfaces' commit sets its contents.
    pub layer: Retained<CALayer>,
    /// The ring of surfaces the layer shows.
    pub surfaces: Surfaces,
    /// The window holds its delegate weakly.
    pub _delegate: Retained<WinDelegate>,
    /// The content size (points) and scale (times 120) last reported to the host.
    pub size: (i32, i32),
    pub scale120: u32,
    /// Whether the host wants it shown.
    pub mapped: bool,
    /// The serial of the configure sent last, until the host acks it: till then the host may still draw at the old
    /// size, and the window does not take its surfaces' size.
    pub configuring: Option<u32>,
}

impl Win {
    /// A `w` x `h` point window titled `title`, decorated unless `undecorated`, shown at once; its delegate reports
    /// as window `id`.
    pub fn open(
        mtm: MainThreadMarker,
        id: WindowId,
        w: i32,
        h: i32,
        undecorated: bool,
        title: &str,
    ) -> Win {
        let size = NSSize::new(f64::from(w.max(1)), f64::from(h.max(1)));
        let style = if undecorated {
            NSWindowStyleMask::Borderless
        } else {
            NSWindowStyleMask::Titled
                | NSWindowStyleMask::Closable
                | NSWindowStyleMask::Miniaturizable
                | NSWindowStyleMask::Resizable
        };
        // SAFETY: a fresh allocation with AppKit's designated initialiser; `Buffered` is the only backing AppKit has.
        let window = unsafe {
            NSWindow::initWithContentRect_styleMask_backing_defer(
                NSWindow::alloc(mtm),
                NSRect::new(NSPoint::new(100.0, 100.0), size),
                style,
                NSBackingStoreType::Buffered,
                false,
            )
        };
        // SAFETY: we hold the window in a `Retained`; AppKit must not release it again when it closes.
        unsafe { window.setReleasedWhenClosed(false) };
        let (view, layer) = view::layer_hosting(mtm, id, size);
        window.setContentView(Some(&view));
        window.makeFirstResponder(Some(&view));
        window.setAcceptsMouseMovedEvents(true);
        window.setTitle(&NSString::from_str(title));
        let delegate = WinDelegate::new(mtm, id);
        window.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
        window.makeKeyAndOrderFront(None);
        let scale120 = (window.backingScaleFactor() * 120.0).round() as u32;
        Win {
            window,
            view,
            layer,
            surfaces: Surfaces::default(),
            _delegate: delegate,
            size: (w, h),
            scale120,
            mapped: true,
            configuring: None,
        }
    }

    /// The content size in points, as AppKit has it now.
    pub fn content_size(&self) -> (i32, i32) {
        let r = self.window.contentLayoutRect();
        (r.size.width.round() as i32, r.size.height.round() as i32)
    }

    /// Take the size of the surface shown last (`sw` x `sh` device pixels), as a Wayland window takes its buffer's:
    /// the host sized its frame itself (`set-frame-size`).  Not while a configure is unacked or the user drags a
    /// resize: then the window's size is the one the host is told, and the surface follows it.  The size taken is
    /// the one the host knows, so no configure goes back.  Whether it changed.
    pub fn follow_surface(&mut self, sw: i32, sh: i32) -> bool {
        let scale = self.window.backingScaleFactor();
        let want = (
            (f64::from(sw) / scale).round() as i32,
            (f64::from(sh) / scale).round() as i32,
        );
        if want == self.size {
            // drawn at the size it was told: whatever configure it had, it applied (the ack may never come for a
            // configure the host dropped)
            self.configuring = None;
            return false;
        }
        if self.configuring.is_some() || self.window.inLiveResize() || want.0 < 1 || want.1 < 1 {
            return false;
        }
        // AppKit keeps the bottom-left; a frame keeps its top-left, as NS's does
        let f = self.window.frame();
        let top_left = NSPoint::new(f.origin.x, f.origin.y + f.size.height);
        self.window
            .setContentSize(NSSize::new(f64::from(want.0), f64::from(want.1)));
        self.window.setFrameTopLeftPoint(top_left);
        self.size = self.content_size();
        true
    }

    pub fn scale_now(&self) -> u32 {
        (self.window.backingScaleFactor() * 120.0).round() as u32
    }
}

impl Drop for Win {
    fn drop(&mut self) {
        self.window.setDelegate(None);
        self.window.orderOut(None);
        self.window.close();
    }
}

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
    /// Non-native full screen: the frame and style to go back to.
    pub saved: Option<(NSRect, NSWindowStyleMask)>,
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
        super::windowing::setup(&window);
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
            saved: None,
        }
    }

    /// The content size in points, as AppKit has it now.
    pub fn content_size(&self) -> (i32, i32) {
        let r = self.window.contentLayoutRect();
        (r.size.width.round() as i32, r.size.height.round() as i32)
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

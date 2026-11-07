//! A top-level window: an `NSWindow` whose content view is layer-hosting (`view::layer_hosting`).

use super::view;
use objc2::rc::Retained;
use objc2::{MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{NSBackingStoreType, NSView, NSWindow, NSWindowStyleMask};
use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};
use objc2_quartz_core::CALayer;

pub struct Win {
    pub window: Retained<NSWindow>,
    pub view: Retained<NSView>,
    pub layer: Retained<CALayer>,
    /// The content size (points) and scale (times 120) last reported to the host.
    pub size: (i32, i32),
    pub scale120: u32,
    /// Whether the host wants it shown.
    pub mapped: bool,
}

impl Win {
    /// A `w` x `h` point window titled `title`, decorated unless `undecorated`, shown at once.
    pub fn open(mtm: MainThreadMarker, w: i32, h: i32, undecorated: bool, title: &str) -> Win {
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
        let (view, layer) = view::layer_hosting(mtm, size);
        window.setContentView(Some(&view));
        window.setTitle(&NSString::from_str(title));
        window.makeKeyAndOrderFront(None);
        let scale120 = (window.backingScaleFactor() * 120.0).round() as u32;
        Win {
            window,
            view,
            layer,
            size: (w, h),
            scale120,
            mapped: true,
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
        self.window.orderOut(None);
        self.window.close();
    }
}

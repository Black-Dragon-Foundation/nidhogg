//! A top-level window: our `NSWindow` subclass (`resize::NhWindow`, which takes the resize band's drags), whose content
//! view is layer-hosting and its first responder (`view::layer_hosting`).

use super::resize::{NhWindow, Pending};
use super::runloop::WinDelegate;
use super::surface::Surfaces;
use super::view::{self, NhView};
use crate::api::WindowId;
use crate::units::Scale;
use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_app_kit::{NSWindow, NSWindowStyleMask};
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
    /// The output (`screens::outputs`' id) the host was last told the window is on; 0: none yet.
    pub output: u64,
    /// The frame's top-left (logical pixels from the main screen's top-left) and title bar height last reported to the
    /// host.
    pub pos: Option<(i32, i32, i32)>,
    /// Whether the host wants it shown.
    pub mapped: bool,
    /// The serial of the configure sent last, until the host acks it: till then the host may still draw at the old
    /// size, and the window does not take its surfaces' size.
    pub configuring: Option<u32>,
    /// Non-native full screen: the frame and style to go back to.
    pub saved: Option<(NSRect, NSWindowStyleMask)>,
    /// Native full screen: AppKit is in a transition (it ignores a toggle until the transition ends), and the state
    /// asked for meanwhile, applied when it ends.
    pub fs_busy: bool,
    pub fs_want: Option<bool>,
    /// Zoom asked for in native full screen or during a transition: applied once the window is out of full screen.
    pub zoom_want: Option<bool>,
    /// Our own edge-drag resize: the frame the host is drawing for, and the newest one asked for since.
    pub resize: Pending,
}

impl Win {
    /// A `w` x `h` point window titled `title`, decorated unless `undecorated`, shown at once and key; its delegate
    /// reports as window `id`.  The test hooks' windows (`window_new_placed` makes and shows its own).
    #[cfg(feature = "test-hooks")]
    pub fn open(
        mtm: MainThreadMarker,
        id: WindowId,
        w: i32,
        h: i32,
        undecorated: bool,
        title: &str,
    ) -> Win {
        let win = Win::make(mtm, id, w, h, undecorated, title);
        win.window.makeKeyAndOrderFront(None);
        win
    }

    /// A `w` x `h` point window titled `title`, decorated unless `undecorated`, not shown yet; its delegate reports as
    /// window `id`.
    pub fn make(
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
        let window = Retained::into_super(NhWindow::new(
            mtm,
            id,
            NSRect::new(NSPoint::new(100.0, 100.0), size),
            style,
        ));
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
        Win {
            window,
            view,
            layer,
            surfaces: Surfaces::default(),
            _delegate: delegate,
            size: (w, h),
            // not placed yet: `placed` reads it once the window is on its screen
            scale120: 0,
            output: 0,
            pos: None,
            mapped: true,
            configuring: None,
            saved: None,
            fs_busy: false,
            fs_want: None,
            zoom_want: None,
            resize: Pending::default(),
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
        let s = self.scale();
        let want = (s.pt_i(sw), s.pt_i(sh));
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

    /// The scale AppKit has the window at now (`MacShell::compare` tells the host when it is new).
    pub fn scale_now(&self) -> u32 {
        super::scale::of_window(&self.window).s120()
    }

    /// The window is on its screen: it takes that screen's scale, which the caller tells the host.
    pub fn placed(&mut self) -> Scale {
        self.scale120 = self.scale_now();
        self.scale()
    }

    /// The scale the host was told: every conversion between the host's device pixels and AppKit's points uses it.
    pub fn scale(&self) -> Scale {
        debug_assert!(
            self.scale120 != 0,
            "a window's scale read before it was placed"
        );
        Scale::of120(self.scale120)
    }
}

impl Drop for Win {
    fn drop(&mut self) {
        self.window.setDelegate(None);
        self.window.orderOut(None);
        self.window.close();
    }
}

//! Overhang panels (design §5.6): a child window whose content crosses its root's bounds lives in a borderless,
//! non-activating child `NSPanel` of the root's window, as big as the child's surface (its shadow margin included),
//! until it fits again (`crate::mac_child::overhangs`).  The child's container layer moves into the panel's root layer
//! with no redraw.
//!
//! **The switch is ordered, not atomic:** two windows cannot commit together in WindowServer, so going out the panel
//! is ordered in already showing the child's surface before the container leaves the root, and going back the
//! container is in the root again before the panel is ordered out.  The panel never becomes key: keys stay with the
//! root's window.  Its view reports as the child, and the backend hit-tests its clicks in the root's tree, so a click
//! on the margin reaches the parent frame.

use super::view::{self, NhView};
use crate::api::WindowId;
use objc2::rc::Retained;
use objc2::{MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSBackingStoreType, NSColor, NSPanel, NSWindow, NSWindowOrderingMode, NSWindowStyleMask,
};
use objc2_foundation::NSRect;
use objc2_quartz_core::CALayer;

pub struct Panel {
    pub panel: Retained<NSPanel>,
    _view: Retained<NhView>,
    /// The panel's root layer: the child's container while it is out, and its surface as contents while it switches.
    pub layer: Retained<CALayer>,
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
        panel.setContentView(Some(&view));
        Panel {
            panel,
            _view: view,
            layer,
            shown: false,
        }
    }

    /// Put the panel at `frame` (screen points); its layer's size follows the view's.
    pub fn place(&self, frame: NSRect) {
        let f = self.panel.frame();
        if f.origin != frame.origin || f.size != frame.size {
            self.panel.setFrame_display(frame, false);
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

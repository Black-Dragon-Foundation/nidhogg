//! A window's root view: layer-hosting, so Core Animation shows our layers as they are and AppKit draws nothing; and
//! the window's first responder, so its keys and mouse arrive here.  Each becomes the shared event record
//! (`crate::mac_input`) and reaches the backend through the scope (`runloop::deliver`).

use super::runloop::{MacEv, deliver};
use crate::api::WindowId;
use crate::event::{NHSHELL_BUTTON_PRESSED, NHSHELL_BUTTON_RELEASED, PointerEv, ShellEvent};
use crate::mac_input;
use objc2::rc::Retained;
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{NSEvent, NSResponder, NSView};
use objc2_foundation::{NSPoint, NSRect, NSSize};
use objc2_quartz_core::CALayer;

define_class!(
    /// The root view of window `ivars`.
    #[unsafe(super(NSView, NSResponder, objc2_foundation::NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "NhmacView"]
    #[ivars = WindowId]
    pub struct NhView;

    impl NhView {
        #[unsafe(method(acceptsFirstResponder))]
        fn accepts_first_responder(&self) -> bool {
            true
        }

        #[unsafe(method(acceptsFirstMouse:))]
        fn accepts_first_mouse(&self, _e: Option<&NSEvent>) -> bool {
            true
        }

        #[unsafe(method(keyDown:))]
        fn key_down(&self, e: &NSEvent) {
            let ignoring = e.charactersIgnoringModifiers().map(|s| s.to_string()).unwrap_or_default();
            let chars = e.characters().map(|s| s.to_string()).unwrap_or_default();
            let flags = e.modifierFlags().0;
            if let Some(k) = mac_input::key(&ignoring, &chars, flags, e.isARepeat()) {
                self.send(ShellEvent::Key(k));
            }
        }

        #[unsafe(method(keyUp:))]
        fn key_up(&self, _e: &NSEvent) {}

        #[unsafe(method(flagsChanged:))]
        fn flags_changed(&self, e: &NSEvent) {
            self.send(ShellEvent::Modifiers {
                mods: mac_input::mods(e.modifierFlags().0),
            });
        }

        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, e: &NSEvent) {
            self.button(e, NHSHELL_BUTTON_PRESSED);
        }

        #[unsafe(method(mouseUp:))]
        fn mouse_up(&self, e: &NSEvent) {
            self.button(e, NHSHELL_BUTTON_RELEASED);
        }

        #[unsafe(method(rightMouseDown:))]
        fn right_mouse_down(&self, e: &NSEvent) {
            self.button(e, NHSHELL_BUTTON_PRESSED);
        }

        #[unsafe(method(rightMouseUp:))]
        fn right_mouse_up(&self, e: &NSEvent) {
            self.button(e, NHSHELL_BUTTON_RELEASED);
        }

        #[unsafe(method(otherMouseDown:))]
        fn other_mouse_down(&self, e: &NSEvent) {
            self.button(e, NHSHELL_BUTTON_PRESSED);
        }

        #[unsafe(method(otherMouseUp:))]
        fn other_mouse_up(&self, e: &NSEvent) {
            self.button(e, NHSHELL_BUTTON_RELEASED);
        }

        #[unsafe(method(mouseMoved:))]
        fn mouse_moved(&self, e: &NSEvent) {
            self.motion(e);
        }

        #[unsafe(method(mouseDragged:))]
        fn mouse_dragged(&self, e: &NSEvent) {
            self.motion(e);
        }

        #[unsafe(method(rightMouseDragged:))]
        fn right_mouse_dragged(&self, e: &NSEvent) {
            self.motion(e);
        }

        #[unsafe(method(otherMouseDragged:))]
        fn other_mouse_dragged(&self, e: &NSEvent) {
            self.motion(e);
        }

        #[unsafe(method(scrollWheel:))]
        fn scroll_wheel(&self, e: &NSEvent) {
            self.motion(e);
            let (dx, dy) = (e.scrollingDeltaX(), e.scrollingDeltaY());
            if dx != 0.0 || dy != 0.0 {
                self.send(ShellEvent::Scroll(mac_input::wheel(dx, dy, e.hasPreciseScrollingDeltas())));
            }
        }
    }
);

impl NhView {
    fn send(&self, ev: ShellEvent) {
        deliver(MacEv::Input(*self.ivars(), ev));
    }

    /// The event's position in this view, in device pixels from the top left.
    fn position(&self, e: &NSEvent) -> (f64, f64) {
        let p = self.convertPoint_fromView(e.locationInWindow(), None);
        let scale = self.window().map_or(1.0, |w| w.backingScaleFactor());
        mac_input::device_point(p.x, p.y, self.bounds().size.height, scale)
    }

    fn motion(&self, e: &NSEvent) {
        let (x, y) = self.position(e);
        self.send(ShellEvent::PointerMotion(PointerEv {
            x,
            y,
            ..PointerEv::default()
        }));
    }

    /// A press or release: the modifiers and the position first, as the Wayland backend's records come.
    fn button(&self, e: &NSEvent, state: u32) {
        self.send(ShellEvent::Modifiers {
            mods: mac_input::mods(e.modifierFlags().0),
        });
        self.motion(e);
        let (x, y) = self.position(e);
        self.send(ShellEvent::Button(PointerEv {
            x,
            y,
            button: mac_input::button(e.buttonNumber()),
            state,
        }));
    }
}

/// Window `id`'s root view, of `size` points, and its layer.  The layer is set before `wantsLayer`: that order makes
/// the view layer-hosting rather than layer-backed.
pub fn layer_hosting(
    mtm: MainThreadMarker,
    id: WindowId,
    size: NSSize,
) -> (Retained<NhView>, Retained<CALayer>) {
    let this = NhView::alloc(mtm).set_ivars(id);
    // SAFETY: NSView's designated initialiser, on a fresh allocation with its ivars set.
    let view: Retained<NhView> =
        unsafe { msg_send![super(this), initWithFrame: NSRect::new(NSPoint::new(0.0, 0.0), size)] };
    let layer = CALayer::new();
    view.setLayer(Some(&layer));
    view.setWantsLayer(true);
    (view, layer)
}

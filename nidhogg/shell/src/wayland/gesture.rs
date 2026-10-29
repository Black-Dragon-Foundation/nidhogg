//! Touchpad pinch (reader queue): `zwp_pointer_gesture_pinch_v1`, one per pointer, posted as `PINCH` records (W2-9).
//!
//! Only pinch is bound: Emacs has an event for it (`pinch`), and none for swipe or hold.  The protocol's scale is
//! already cumulative since `begin`, as GDK's is, and is passed on as it comes; the deltas and the position become
//! device pixels at the scale of the window the gesture began on.

use crate::event::*;
use crate::wayland::reader::ReaderState;
use wayland_client::protocol::wl_pointer::WlPointer;
use wayland_client::{Connection, Dispatch, QueueHandle, delegate_noop};
use wayland_protocols::wp::pointer_gestures::zv1::client::zwp_pointer_gesture_pinch_v1::{
    self, ZwpPointerGesturePinchV1,
};
use wayland_protocols::wp::pointer_gestures::zv1::client::zwp_pointer_gestures_v1::ZwpPointerGesturesV1;

/// The pinch object of the seat's pointer and the gesture in progress.
#[derive(Default)]
pub struct PinchState {
    pinch: Option<ZwpPointerGesturePinchV1>,
    /// The window the gesture began on (0: none in progress), its finger count and the latest scale.
    window: u64,
    fingers: u32,
    scale: f64,
}

/// The pointer `ptr` came: bind its pinch gesture when the compositor has pointer gestures.
pub fn pointer_added(state: &mut ReaderState, ptr: &WlPointer, qh: &QueueHandle<ReaderState>) {
    if let Some(g) = &state.gestures {
        state.pinch.pinch = Some(g.get_pinch_gesture(ptr, qh, ()));
    }
}

/// The pointer went: its pinch object goes first.
pub fn pointer_removed(state: &mut ReaderState) {
    if let Some(p) = state.pinch.pinch.take() {
        p.destroy();
    }
    state.pinch.window = 0;
}

impl ReaderState {
    /// Post a PINCH record of `phase`; `delta` is (dx, dy, rotation) in the protocol's units.
    fn post_pinch(&mut self, phase: u32, time: u32, delta: (f64, f64, f64), cancelled: bool) {
        let (dx, dy, rotation) = delta;
        let window = self.pinch.window;
        let (_, px, py) = self.pointer.position();
        let (x, y) = self.to_device(window, px, py);
        let (dx, dy) = self.to_device(window, dx, dy);
        let p = PinchEv {
            phase,
            fingers: self.pinch.fingers,
            dx,
            dy,
            scale: self.pinch.scale,
            rotation,
            x,
            y,
            cancelled: cancelled as u8,
        };
        let ev = Record::new(ShellEvent::Pinch(p), window, u64::from(time) * 1_000_000);
        self.post(ev);
    }
}

impl Dispatch<ZwpPointerGesturePinchV1, ()> for ReaderState {
    fn event(
        state: &mut Self,
        _: &ZwpPointerGesturePinchV1,
        event: zwp_pointer_gesture_pinch_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            zwp_pointer_gesture_pinch_v1::Event::Begin {
                time,
                surface,
                fingers,
                ..
            } => {
                state.pinch.window = crate::wayland::window_of(&surface);
                state.pinch.fingers = fingers;
                state.pinch.scale = 1.0;
                state.post_pinch(NHSHELL_PINCH_BEGIN, time, (0.0, 0.0, 0.0), false);
            }
            zwp_pointer_gesture_pinch_v1::Event::Update {
                time,
                dx,
                dy,
                scale,
                rotation,
            } if state.pinch.window != 0 => {
                state.pinch.scale = scale;
                state.post_pinch(NHSHELL_PINCH_UPDATE, time, (dx, dy, rotation), false);
            }
            zwp_pointer_gesture_pinch_v1::Event::End {
                time, cancelled, ..
            } if state.pinch.window != 0 => {
                state.post_pinch(NHSHELL_PINCH_END, time, (0.0, 0.0, 0.0), cancelled != 0);
                state.pinch.window = 0;
            }
            _ => {}
        }
    }
}

delegate_noop!(ReaderState: ignore ZwpPointerGesturesV1);

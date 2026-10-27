//! The pointer (reader queue): enter, leave, motion in device pixels, buttons, and one `SCROLL` per `wl_pointer.frame`.
//!
//! Enter serials and press serials go to the shared `SerialTracker` (the cursor shape and the interactive move, resize
//! and menu requests read them).  The scale a surface has lives on the main queue; the main queue mirrors it into
//! `Shared::scale120` for the reader.

use crate::event::*;
use crate::wayland::reader::ReaderState;
use crate::wayland::selection;
use crate::wayland::serial::SerialKind;
use wayland_client::protocol::wl_pointer::{self, Axis, AxisSource, ButtonState, WlPointer};
use wayland_client::protocol::wl_seat::{Capability, WlSeat};
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle, WEnum, delegate_noop};
use wayland_protocols::wp::cursor_shape::v1::client::wp_cursor_shape_device_v1::WpCursorShapeDeviceV1;
use wayland_protocols::wp::cursor_shape::v1::client::wp_cursor_shape_manager_v1::WpCursorShapeManagerV1;

/// What the reader keeps of the pointer.
#[derive(Default)]
pub struct PointerState {
    pointer: Option<WlPointer>,
    /// The window (surface id) the pointer is over; 0 when none.
    surface: u64,
    frame: ScrollFrame,
}

/// The axis events of one `wl_pointer.frame`.
#[derive(Default)]
pub struct ScrollFrame {
    dx: f64,
    dy: f64,
    v120x: i32,
    v120y: i32,
    source: u32,
    stop: bool,
    any: bool,
}

impl ScrollFrame {
    pub fn source(&mut self, source: u32) {
        self.source = source;
    }

    pub fn axis(&mut self, vertical: bool, value: f64) {
        self.any = true;
        *if vertical { &mut self.dy } else { &mut self.dx } += value;
    }

    pub fn value120(&mut self, vertical: bool, v120: i32) {
        self.any = true;
        *if vertical {
            &mut self.v120y
        } else {
            &mut self.v120x
        } += v120;
    }

    pub fn stop(&mut self) {
        self.any = true;
        self.stop = true;
    }

    /// The frame's record, `None` when no axis event came.  Below seat 8 (`has_value120` false) the continuous value
    /// stands in for the wheel steps: one detent is 10 px, so v120 = value x 12.
    pub fn take(&mut self, has_value120: bool) -> Option<ScrollEv> {
        let f = std::mem::take(self);
        if !f.any {
            return None;
        }
        let (v120x, v120y) = if has_value120 {
            (f.v120x, f.v120y)
        } else {
            ((f.dx * 12.0).round() as i32, (f.dy * 12.0).round() as i32)
        };
        Some(ScrollEv {
            dx: f.dx,
            dy: f.dy,
            v120x,
            v120y,
            source: f.source,
            stop: f.stop as u8,
        })
    }
}

/// Follow the seat's pointer capability (called from the seat's handler).
pub fn seat_capabilities(
    state: &mut ReaderState,
    seat: &WlSeat,
    caps: Capability,
    qh: &QueueHandle<ReaderState>,
) {
    let has = caps.contains(Capability::Pointer);
    match (has, state.pointer.pointer.take()) {
        (true, None) => {
            let ptr = seat.get_pointer(qh, ());
            let device = state
                .cursor_manager
                .as_ref()
                .map(|m| m.get_pointer(&ptr, qh, ()));
            *state
                .shared
                .cursor_device
                .lock()
                .unwrap_or_else(|e| e.into_inner()) = device;
            state.pointer.pointer = Some(ptr);
        }
        (false, Some(ptr)) => {
            if let Some(d) = state
                .shared
                .cursor_device
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .take()
            {
                d.destroy();
            }
            state.pointer.surface = 0;
            if ptr.version() >= 3 {
                ptr.release();
            }
        }
        (_, ptr) => state.pointer.pointer = ptr,
    }
}

fn time_ns(ms: u32) -> u64 {
    u64::from(ms) * 1_000_000
}

impl ReaderState {
    /// Surface-local logical -> device pixels, at the scale of window `id`.
    fn to_device(&self, id: u64, x: f64, y: f64) -> (f64, f64) {
        let s = self
            .shared
            .scale120
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&id)
            .copied()
            .unwrap_or(120);
        let k = f64::from(s) / 120.0;
        (x * k, y * k)
    }

    fn post_pointer(&mut self, kind: u32, time: u32, x: f64, y: f64) {
        let id = self.pointer.surface;
        let (x, y) = self.to_device(id, x, y);
        let mut ev = NhShellEvent::new(kind, id, time_ns(time));
        ev.u.pointer = PointerEv {
            x,
            y,
            button: 0,
            state: 0,
        };
        self.post(ev);
    }
}

impl Dispatch<WlPointer, ()> for ReaderState {
    fn event(
        state: &mut Self,
        _: &WlPointer,
        event: wl_pointer::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            wl_pointer::Event::Enter {
                serial,
                surface,
                surface_x,
                surface_y,
            } => {
                state
                    .shared
                    .serials
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .update(SerialKind::MouseEnter, serial);
                state.pointer.surface = u64::from(surface.id().protocol_id());
                state.post_pointer(NHSHELL_EV_POINTER_ENTER, 0, surface_x, surface_y);
            }
            wl_pointer::Event::Leave { .. } => {
                state.post_pointer(NHSHELL_EV_POINTER_LEAVE, 0, 0.0, 0.0);
                state.pointer.surface = 0;
            }
            wl_pointer::Event::Motion {
                time,
                surface_x,
                surface_y,
            } => state.post_pointer(NHSHELL_EV_POINTER_MOTION, time, surface_x, surface_y),
            wl_pointer::Event::Button {
                serial,
                time,
                button,
                state: WEnum::Value(bs),
            } => {
                let pressed = bs == ButtonState::Pressed;
                if pressed {
                    state
                        .shared
                        .serials
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .update(SerialKind::MousePress, serial);
                    selection::lock(&state.shared).input(serial);
                }
                let mut ev =
                    NhShellEvent::new(NHSHELL_EV_BUTTON, state.pointer.surface, time_ns(time));
                ev.u.pointer = PointerEv {
                    x: 0.0,
                    y: 0.0,
                    button,
                    state: if pressed {
                        NHSHELL_BUTTON_PRESSED
                    } else {
                        NHSHELL_BUTTON_RELEASED
                    },
                };
                state.post(ev);
            }
            wl_pointer::Event::AxisSource {
                axis_source: WEnum::Value(s),
            } => state.pointer.frame.source(match s {
                AxisSource::Finger => NHSHELL_SCROLL_FINGER,
                AxisSource::Continuous => NHSHELL_SCROLL_CONTINUOUS,
                _ => NHSHELL_SCROLL_WHEEL,
            }),
            wl_pointer::Event::Axis {
                axis: WEnum::Value(a),
                value,
                ..
            } => state.pointer.frame.axis(a == Axis::VerticalScroll, value),
            wl_pointer::Event::AxisValue120 {
                axis: WEnum::Value(a),
                value120,
            } => state
                .pointer
                .frame
                .value120(a == Axis::VerticalScroll, value120),
            wl_pointer::Event::AxisStop { .. } => state.pointer.frame.stop(),
            wl_pointer::Event::Frame => {
                let seat_v = state.seat.as_ref().map_or(0, |s| s.version());
                if let Some(s) = state.pointer.frame.take(seat_v >= 8) {
                    let mut ev = NhShellEvent::new(NHSHELL_EV_SCROLL, state.pointer.surface, 0);
                    ev.u.scroll = s;
                    state.post(ev);
                }
            }
            _ => {}
        }
    }
}

delegate_noop!(ReaderState: ignore WpCursorShapeManagerV1);
delegate_noop!(ReaderState: ignore WpCursorShapeDeviceV1);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::*;
    use crate::wayland::cursor::shape_request;
    use crate::wayland::serial::SerialTracker;

    #[test]
    fn axis_without_value120_below_seat_8() {
        let mut f = ScrollFrame::default();
        f.axis(true, 10.0);
        f.axis(false, -5.0);
        let s = f.take(false).unwrap();
        assert_eq!((s.v120y, s.v120x), (120, -60));
        assert_eq!((s.dy, s.dx), (10.0, -5.0));
        assert!(f.take(false).is_none(), "the frame is spent");
    }

    #[test]
    fn value120_wins_from_seat_8() {
        let mut f = ScrollFrame::default();
        f.axis(true, 15.0);
        f.value120(true, 120);
        assert_eq!(f.take(true).unwrap().v120y, 120);
    }

    #[test]
    fn shape_request_carries_latest_enter_serial() {
        let mut t = SerialTracker::new();
        t.update(SerialKind::MouseEnter, 7);
        t.update(SerialKind::MousePress, 99);
        t.update(SerialKind::MouseEnter, 12);
        let (serial, _) = shape_request(&t, NHSHELL_CURSOR_TEXT).unwrap();
        assert_eq!(serial, 12);
        assert!(shape_request(&t, NHSHELL_CURSOR_HIDDEN).is_err());
    }
}

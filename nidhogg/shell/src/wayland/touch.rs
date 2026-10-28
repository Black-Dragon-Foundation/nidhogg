//! Touch (reader queue): `wl_touch` points posted as `TOUCH` records at the end of each touch frame (W2-10).
//!
//! A point belongs to the surface it went down on; its motion and lift carry that window and device pixels at its
//! scale.  The records of a frame wait for `frame`, so a host sees the points of one frame together.  A `cancel`
//! drops the frame's records and posts `NHSHELL_TOUCH_CANCEL` for every live point the host has heard of.  Touch-down
//! serials go to the shared `SerialTracker` (popup grabs and drags use them).

use crate::event::*;
use crate::wayland::reader::ReaderState;
use crate::wayland::serial::SerialKind;
use wayland_client::protocol::wl_seat::{Capability, WlSeat};
use wayland_client::protocol::wl_touch::{self, WlTouch};
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle};

/// A point that is down: its window, its last position (device pixels), and whether its DOWN record was posted.
struct Point {
    id: i32,
    window: u64,
    x: f64,
    y: f64,
    posted: bool,
}

/// What the reader keeps of touch.
#[derive(Default)]
pub struct TouchState {
    touch: Option<WlTouch>,
    /// The live points, in the order they went down.
    points: Vec<Point>,
    /// The records of the frame in progress.
    queued: Vec<NhShellEvent>,
}

/// Follow the seat's touch capability (called from the seat's handler).  A touch device that goes away cancels its
/// live points.
pub fn seat_capabilities(
    state: &mut ReaderState,
    seat: &WlSeat,
    caps: Capability,
    qh: &QueueHandle<ReaderState>,
) {
    let has = caps.contains(Capability::Touch);
    match (has, state.touch.touch.take()) {
        (true, None) => state.touch.touch = Some(seat.get_touch(qh, ())),
        (false, Some(t)) => {
            cancel(state);
            if t.version() >= 3 {
                t.release();
            }
        }
        (_, t) => state.touch.touch = t,
    }
}

fn record(window: u64, time: u32, id: i32, op: u32, x: f64, y: f64) -> NhShellEvent {
    let mut ev = NhShellEvent::new(NHSHELL_EV_TOUCH, window, u64::from(time) * 1_000_000);
    ev.u.touch = TouchEv { id, op, x, y };
    ev
}

/// Drop the frame in progress and end every live point the host has heard of as cancelled.
fn cancel(state: &mut ReaderState) {
    state.touch.queued.clear();
    for p in std::mem::take(&mut state.touch.points) {
        if p.posted {
            state.post(record(p.window, 0, p.id, NHSHELL_TOUCH_CANCEL, p.x, p.y));
        }
    }
}

impl Dispatch<WlTouch, ()> for ReaderState {
    fn event(
        state: &mut Self,
        _: &WlTouch,
        event: wl_touch::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            wl_touch::Event::Down {
                serial,
                time,
                surface,
                id,
                x,
                y,
            } => {
                state
                    .shared
                    .serials
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .update(SerialKind::TouchDown, serial);
                let window = u64::from(surface.id().protocol_id());
                let (x, y) = state.to_device(window, x, y);
                state.touch.points.retain(|p| p.id != id);
                state.touch.points.push(Point {
                    id,
                    window,
                    x,
                    y,
                    posted: false,
                });
                let ev = record(window, time, id, NHSHELL_TOUCH_DOWN, x, y);
                state.touch.queued.push(ev);
            }
            wl_touch::Event::Motion { time, id, x, y } => {
                let Some(i) = state.touch.points.iter().position(|p| p.id == id) else {
                    return;
                };
                let (x, y) = state.to_device(state.touch.points[i].window, x, y);
                let p = &mut state.touch.points[i];
                (p.x, p.y) = (x, y);
                let ev = record(p.window, time, id, NHSHELL_TOUCH_MOTION, x, y);
                state.touch.queued.push(ev);
            }
            wl_touch::Event::Up { time, id, .. } => {
                let Some(i) = state.touch.points.iter().position(|p| p.id == id) else {
                    return;
                };
                let p = state.touch.points.remove(i);
                let ev = record(p.window, time, id, NHSHELL_TOUCH_UP, p.x, p.y);
                state.touch.queued.push(ev);
            }
            wl_touch::Event::Frame => {
                for p in &mut state.touch.points {
                    p.posted = true;
                }
                for ev in std::mem::take(&mut state.touch.queued) {
                    state.post(ev);
                }
            }
            wl_touch::Event::Cancel => cancel(state),
            _ => {}
        }
    }
}

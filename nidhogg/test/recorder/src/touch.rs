//! Touch and pinch: `touch` drives wl_touch (points are delivered with a `frame` unless told otherwise), `pinch`
//! drives zwp_pointer_gesture_pinch_v1 on the surface the pointer is over.
use crate::control::err;
use crate::state::State;
use serde_json::{json, Value};
use smithay::input::pointer::{
    GesturePinchBeginEvent, GesturePinchEndEvent, GesturePinchUpdateEvent,
};
use smithay::input::touch::{DownEvent, MotionEvent, UpEvent};
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::SERIAL_COUNTER;

/// The surface the newest touch went down on: where a point that moves or lifts is delivered.
#[derive(Default)]
pub struct Touch {
    surface: Option<WlSurface>,
}

fn num(req: &Value, key: &str) -> f64 {
    req.get(key).and_then(Value::as_f64).unwrap_or(0.0)
}

/// `{"cmd":"touch","op":"down"|"motion"|"up"|"cancel","id":N,"at":[x,y],"surface":ID|"toplevel"|"newest","frame":true}`.
/// Positions are surface-local logical.  A `down` names the surface (default `"toplevel"`); later ops of the same
/// point go to it.  The reply of a `down` carries the serial the client heard.
pub fn touch(state: &mut State, req: &Value) -> Value {
    let Some(touch) = state.seat.get_touch() else {
        return err("this profile's seat has no touch");
    };
    let op = req.get("op").and_then(Value::as_str).unwrap_or_default();
    let slot = req
        .get("id")
        .and_then(Value::as_u64)
        .map(|n| n as u32)
        .into();
    let location = (
        req.get("at")
            .and_then(|a| a.get(0))
            .and_then(Value::as_f64)
            .unwrap_or(0.0),
        req.get("at")
            .and_then(|a| a.get(1))
            .and_then(Value::as_f64)
            .unwrap_or(0.0),
    )
        .into();
    let time = state.time_ms();
    let mut serial = None;
    match op {
        "down" => {
            let toplevel = json!("toplevel");
            let named = req.get("surface").unwrap_or(&toplevel);
            let surface = match crate::dnd::surface_of(state, Some(named)) {
                Ok(s) => s,
                Err(e) => return err(e),
            };
            state.touch.surface = Some(surface.clone());
            let s = SERIAL_COUNTER.next_serial();
            state.serials[2] = Some(u32::from(s));
            serial = Some(u32::from(s));
            touch.down(
                state,
                Some((surface, (0.0, 0.0).into())),
                &DownEvent {
                    slot,
                    location,
                    serial: s,
                    time,
                },
            );
        }
        "motion" => {
            let focus = state.touch.surface.clone().map(|s| (s, (0.0, 0.0).into()));
            touch.motion(
                state,
                focus,
                &MotionEvent {
                    slot,
                    location,
                    time,
                },
            );
        }
        "up" => touch.up(
            state,
            &UpEvent {
                slot,
                serial: SERIAL_COUNTER.next_serial(),
                time,
            },
        ),
        "cancel" => touch.cancel(state),
        _ => return err("\"op\" must be down, motion, up or cancel"),
    }
    if req.get("frame").and_then(Value::as_bool) != Some(false) {
        touch.frame(state);
    }
    json!({"ok": true, "serial": serial})
}

/// `{"cmd":"pinch","op":"begin"|"update"|"end","fingers":2,"dx":0,"dy":0,"scale":1.0,"rotation":0,"cancelled":false}`:
/// a touchpad pinch over the surface the pointer is on (move the pointer there first).
pub fn pinch(state: &mut State, req: &Value) -> Value {
    let Some(ptr) = state.seat.get_pointer() else {
        return err("this profile's seat has no pointer");
    };
    let time = state.time_ms();
    match req.get("op").and_then(Value::as_str).unwrap_or_default() {
        "begin" => ptr.gesture_pinch_begin(
            state,
            &GesturePinchBeginEvent {
                serial: SERIAL_COUNTER.next_serial(),
                time,
                fingers: req.get("fingers").and_then(Value::as_u64).unwrap_or(2) as u32,
            },
        ),
        "update" => ptr.gesture_pinch_update(
            state,
            &GesturePinchUpdateEvent {
                time,
                delta: (num(req, "dx"), num(req, "dy")).into(),
                scale: req.get("scale").and_then(Value::as_f64).unwrap_or(1.0),
                rotation: num(req, "rotation"),
            },
        ),
        "end" => ptr.gesture_pinch_end(
            state,
            &GesturePinchEndEvent {
                serial: SERIAL_COUNTER.next_serial(),
                time,
                cancelled: req.get("cancelled").and_then(Value::as_bool) == Some(true),
            },
        ),
        _ => return err("\"op\" must be begin, update or end"),
    }
    ptr.frame(state);
    json!({"ok": true})
}

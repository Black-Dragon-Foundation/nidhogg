//! Scale: fractional-scale and viewporter globals, and the `scale` command that announces a new scale everywhere.
use crate::control::err;
use crate::state::State;
use serde_json::{json, Value};
use smithay::delegate_fractional_scale;
use smithay::delegate_viewporter;
use smithay::output::{Mode, Output, PhysicalProperties, Scale, Subpixel};
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::reexports::wayland_server::Resource;
use smithay::utils::Transform;
use smithay::wayland::compositor::{self, with_states};
use smithay::wayland::fractional_scale::{with_fractional_scale, FractionalScaleHandler};

impl FractionalScaleHandler for State {
    fn new_fractional_scale(&mut self, surface: WlSurface) {
        let scale = f64::from(self.scale120) / 120.0;
        with_states(&surface, |s| {
            with_fractional_scale(s, |f| f.set_preferred_scale(scale))
        });
    }
}

delegate_fractional_scale!(State);
delegate_viewporter!(State);

/// `{"cmd":"scale","value120":N}`: `preferred_scale(N)` to every fractional-scale object, `preferred_buffer_scale`
/// and `wl_output.scale` (+ `done`) with the ceiling of N/120.
pub fn scale(state: &mut State, req: &Value) -> Value {
    let Some(n) = req
        .get("value120")
        .and_then(Value::as_u64)
        .filter(|&n| n > 0)
    else {
        return err("\"value120\" must be a positive integer");
    };
    state.scale120 = n as u32;
    let ceil = (n as u32).div_ceil(120) as i32;
    state.surfaces.retain(|s| s.is_alive());
    for surface in &state.surfaces {
        with_states(surface, |s| {
            with_fractional_scale(s, |f| f.set_preferred_scale(n as f64 / 120.0));
            compositor::send_surface_state(surface, s, ceil, Transform::Normal);
        });
    }
    state
        .output
        .change_current_state(None, None, Some(Scale::Integer(ceil)), None);
    json!({"ok": true})
}

/// `{"cmd":"output","op":"add"|"remove"|"scale","name":"N","size":[W,H],"scale120":S}`: adds a wl_output global
/// (to the right of the others), removes one, or changes one's scale (the first output also moves the surfaces'
/// scale, as `scale` does).
pub fn output(state: &mut State, req: &Value) -> Value {
    let Some(name) = req.get("name").and_then(Value::as_str) else {
        return err("\"name\" is required");
    };
    let scale120 = req
        .get("scale120")
        .and_then(Value::as_u64)
        .filter(|&n| n > 0);
    let at = state.outputs.iter().position(|(o, _)| o.name() == name);
    match req.get("op").and_then(Value::as_str) {
        Some("add") => {
            if at.is_some() {
                return err("an output of that name exists");
            }
            let Some([w, h]) = req
                .get("size")
                .and_then(Value::as_array)
                .and_then(|a| Some([a.first()?.as_i64()? as i32, a.get(1)?.as_i64()? as i32]))
            else {
                return err("\"size\" must be [W,H]");
            };
            let x: i32 = state
                .outputs
                .iter()
                .map(|(o, _)| o.current_mode().map_or(0, |m| m.size.w) + o.current_location().x)
                .max()
                .unwrap_or(0);
            let out = Output::new(
                name.into(),
                PhysicalProperties {
                    size: (0, 0).into(),
                    subpixel: Subpixel::Unknown,
                    make: "nh".into(),
                    model: "recorder".into(),
                },
            );
            let mode = Mode {
                size: (w, h).into(),
                refresh: 60_000,
            };
            let ceil = scale120.unwrap_or(120).div_ceil(120) as i32;
            out.change_current_state(
                Some(mode),
                Some(Transform::Normal),
                Some(Scale::Integer(ceil)),
                Some((x, 0).into()),
            );
            out.set_preferred(mode);
            let global = out.create_global::<State>(&state.display);
            state.outputs.push((out, global));
            json!({"ok": true})
        }
        Some("remove") => match at {
            Some(i) => {
                let (_, global) = state.outputs.remove(i);
                state.display.remove_global::<State>(global);
                json!({"ok": true})
            }
            None => err("no such output"),
        },
        Some("scale") => {
            let (Some(i), Some(n)) = (at, scale120) else {
                return err("a known \"name\" and a positive \"scale120\" are required");
            };
            if state.outputs[i].0 == state.output {
                return scale(state, &json!({"value120": n}));
            }
            let ceil = n.div_ceil(120) as i32;
            state.outputs[i]
                .0
                .change_current_state(None, None, Some(Scale::Integer(ceil)), None);
            json!({"ok": true})
        }
        _ => err("\"op\" must be \"add\", \"remove\" or \"scale\""),
    }
}

//! Scale: fractional-scale and viewporter globals, and the `scale` command that announces a new scale everywhere.
use crate::control::err;
use crate::state::State;
use serde_json::{json, Value};
use smithay::delegate_fractional_scale;
use smithay::delegate_viewporter;
use smithay::output::Scale;
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

//! Scale: the buffer-size arithmetic and the per-surface scale state (`SurfaceScale`).
//!
//! The frame's pixel is the device pixel: Emacs draws at the output's real resolution.  The protocol sees logical sizes only: the buffer is
//! `round(logical x s)` device pixels, `wp_viewport`'s destination is the logical size and `buffer_scale` stays 1.
//! A surface's scale comes from `wp_fractional_scale_v1` when the compositor has it, else from
//! `wl_surface.preferred_buffer_scale`, else from the largest scale of the outputs the surface entered.  All of it
//! runs on the main queue.

use crate::event::{
    NHSHELL_EV_ENTER_OUTPUT, NHSHELL_EV_LEAVE_OUTPUT, NHSHELL_EV_SCALE, NhShellEvent,
};
use crate::wayland::MainState;
use wayland_client::protocol::wl_surface;
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle, delegate_noop};
use wayland_protocols::wp::fractional_scale::v1::client::wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1;
use wayland_protocols::wp::fractional_scale::v1::client::wp_fractional_scale_v1::{
    self, WpFractionalScaleV1,
};
use wayland_protocols::wp::viewporter::client::wp_viewport::WpViewport;
use wayland_protocols::wp::viewporter::client::wp_viewporter::WpViewporter;

/// The scale of a surface that has heard nothing: 1.0.
pub const SCALE_ONE: u32 = 120;

/// The buffer size in device pixels for a logical size: `round(logical x scale120 / 120)` each, halves away from
/// zero.  Exact (integer arithmetic).
pub fn buffer_size(logical_w: i32, logical_h: i32, scale120: u32) -> (i32, i32) {
    fn one(l: i32, s: u32) -> i32 {
        let n = i64::from(l) * i64::from(s) * 2 + if l < 0 { -120 } else { 120 };
        (n / 240) as i32
    }
    (one(logical_w, scale120), one(logical_h, scale120))
}

/// The scale state of one `wl_surface`.  The shell keeps it in `MainState::scales`, keyed by the surface's protocol
/// id (which is the window id); a window attaches one with `WaylandShell::scale_attach` and detaches it on destroy.
pub struct SurfaceScale {
    fractional: Option<WpFractionalScaleV1>,
    viewport: Option<WpViewport>,
    /// `preferred_scale` from `wp_fractional_scale_v1`.
    frac120: Option<u32>,
    /// `preferred_buffer_scale`.
    buffer_scale: Option<u32>,
    /// The outputs (registry names) the surface is on.
    pub(crate) entered: Vec<u32>,
    current: u32,
}

impl SurfaceScale {
    /// The scale now, times 120.
    pub fn scale120(&self) -> u32 {
        self.current
    }

    /// Set the viewport's destination to the logical size (a no-op without `wp_viewporter`).
    pub fn set_destination(&self, logical_w: i32, logical_h: i32) {
        if let Some(v) = &self.viewport {
            v.set_destination(logical_w, logical_h);
        }
    }

    fn destroy(self) {
        if let Some(f) = self.fractional {
            f.destroy();
        }
        if let Some(v) = self.viewport {
            v.destroy();
        }
    }
}

impl MainState {
    /// Create the scale objects for `surface` on the main queue and register them under its protocol id.
    pub(crate) fn attach_scale(
        &mut self,
        surface: &wl_surface::WlSurface,
        qh: &QueueHandle<Self>,
    ) -> u64 {
        let id = u64::from(surface.id().protocol_id());
        let fractional = self
            .fractional
            .as_ref()
            .map(|m| m.get_fractional_scale(surface, qh, id));
        let viewport = self
            .viewporter
            .as_ref()
            .map(|v| v.get_viewport(surface, qh, ()));
        self.scales.insert(
            id,
            SurfaceScale {
                fractional,
                viewport,
                frac120: None,
                buffer_scale: None,
                entered: Vec::new(),
                current: SCALE_ONE,
            },
        );
        id
    }

    pub(crate) fn detach_scale(&mut self, id: u64) {
        if let Some(shared) = &self.shared {
            shared
                .scale120
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(&id);
        }
        if let Some(s) = self.scales.remove(&id) {
            s.destroy();
        }
    }

    /// Recompute surface `id`'s scale and post `SCALE` when it changed.
    pub(crate) fn recompute_scale(&mut self, id: u64) {
        let Some(s) = self.scales.get(&id) else {
            return;
        };
        let new = s
            .frac120
            .or(s.buffer_scale.map(|b| b * 120))
            .or_else(|| {
                s.entered
                    .iter()
                    .filter_map(|o| self.outputs.get(o))
                    .map(|o| o.scale120())
                    .max()
            })
            .unwrap_or(SCALE_ONE);
        if new == s.current {
            return;
        }
        if let Some(s) = self.scales.get_mut(&id) {
            s.current = new;
        }
        if let Some(shared) = &self.shared {
            shared
                .scale120
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert(id, new);
        }
        let mut ev = NhShellEvent::new(NHSHELL_EV_SCALE, id, 0);
        ev.u.scale.scale120 = new;
        self.post(ev);
    }

    /// Recompute every surface that is on output `name`.
    pub(crate) fn recompute_scales_on(&mut self, name: u32) {
        let ids: Vec<u64> = self
            .scales
            .iter()
            .filter(|(_, s)| s.entered.contains(&name))
            .map(|(&id, _)| id)
            .collect();
        for id in ids {
            self.recompute_scale(id);
        }
    }
}

/// The `wl_surface` events that carry scale: enter, leave and the preferred buffer scale.
pub fn surface_event(
    state: &mut MainState,
    surface: &wl_surface::WlSurface,
    event: wl_surface::Event,
) {
    let id = u64::from(surface.id().protocol_id());
    match event {
        wl_surface::Event::Enter { output } => {
            if let (Some(&name), Some(s)) = (output.data::<u32>(), state.scales.get_mut(&id))
                && !s.entered.contains(&name)
            {
                s.entered.push(name);
                let mut ev = NhShellEvent::new(NHSHELL_EV_ENTER_OUTPUT, id, 0);
                ev.u.output.id = u64::from(name);
                state.post(ev);
            }
        }
        wl_surface::Event::Leave { output } => {
            if let (Some(&name), Some(s)) = (output.data::<u32>(), state.scales.get_mut(&id)) {
                s.entered.retain(|&o| o != name);
                let mut ev = NhShellEvent::new(NHSHELL_EV_LEAVE_OUTPUT, id, 0);
                ev.u.output.id = u64::from(name);
                state.post(ev);
            }
        }
        wl_surface::Event::PreferredBufferScale { factor } => {
            if let Some(s) = state.scales.get_mut(&id) {
                s.buffer_scale = u32::try_from(factor).ok().filter(|&f| f > 0);
            }
        }
        _ => return,
    }
    state.recompute_scale(id);
}

delegate_noop!(MainState: WpFractionalScaleManagerV1);
delegate_noop!(MainState: WpViewporter);
delegate_noop!(MainState: WpViewport);

impl Dispatch<WpFractionalScaleV1, u64> for MainState {
    fn event(
        state: &mut Self,
        _: &WpFractionalScaleV1,
        event: wp_fractional_scale_v1::Event,
        id: &u64,
        conn: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wp_fractional_scale_v1::Event::PreferredScale { scale } = event {
            state.present_rearm(*id, conn);
            if let Some(s) = state.scales.get_mut(id) {
                s.frac120 = Some(scale);
            }
            state.recompute_scale(*id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buffer_size_examples() {
        assert_eq!(buffer_size(1280, 720, 180), (1920, 1080));
        assert_eq!(buffer_size(1229, 691, 180), (1844, 1037));
        assert_eq!(buffer_size(2048, 1152, 150), (2560, 1440));
        assert_eq!(buffer_size(1707, 960, 180), (2561, 1440));
    }
}

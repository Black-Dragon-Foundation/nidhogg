//! Scale: the buffer-size arithmetic and the per-surface scale state (`SurfaceScale`).
//!
//! The frame's pixel is the device pixel: Emacs draws at the output's real resolution.  The protocol sees logical sizes only: the buffer is
//! `round(logical x s)` device pixels, `wp_viewport`'s destination is the logical size the buffer maps onto exactly
//! (`viewport_size`: a buffer no logical size fits is cut to the largest that does) and `buffer_scale` stays 1.
//! A surface's scale comes from `wp_fractional_scale_v1` when the compositor has it (the integer hints are not used
//! even before its first `preferred_scale`), else from `wl_surface.preferred_buffer_scale`, else from the largest
//! scale of the outputs the surface entered.  All of it
//! runs on the main queue.

use crate::event::{OutputEv, Record, ScaleEv, ShellEvent};
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

/// The viewport's logical extent for a buffer extent of `device` pixels at `scale120`, and the device extent it shows:
/// the logical extent whose device extent (the compositor's stable rounding, `glitnir::snap::stable_extent`) is `device`
/// exactly, else the largest whose device extent is smaller.  Then the viewport's source cuts the buffer to that: its
/// last device rows or columns go unshown, where a destination rounded to the nearest logical pixel would have every
/// pixel resampled (an odd buffer at scale 2 drawn 402 wide from 401).
pub fn viewport_size(device: i32, scale120: u32) -> (i32, i32) {
    let s = scale120.max(1);
    let dev = |l: i32| glitnir::snap::stable_extent(0, l, s);
    let mut l = (i64::from(device.max(0)) * 120 / i64::from(s)) as i32;
    while dev(l + 1) <= device {
        l += 1;
    }
    while l > 0 && dev(l) > device {
        l -= 1;
    }
    (l, dev(l))
}

/// The scale state of one `wl_surface`.  The shell keeps it in `MainState::scales`, keyed by the window id (the
/// surface's protocol id until a reconnect makes the surface again, `window_of`); a window attaches one with `WaylandShell::scale_attach` and detaches it on destroy.
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

    /// The surface's viewport (none without `wp_viewporter`), for its presenter state.
    pub(crate) fn viewport(&self) -> Option<WpViewport> {
        self.viewport.clone()
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
    /// Create the scale objects for `surface` on the main queue and register them under its window id.
    pub(crate) fn attach_scale(
        &mut self,
        surface: &wl_surface::WlSurface,
        qh: &QueueHandle<Self>,
    ) -> u64 {
        let id = crate::wayland::window_of(surface);
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

    /// The window's surface was made again (a reconnect): new scale objects on it, no output entered yet, and the
    /// scale kept until the new compositor says otherwise, so a `SCALE` comes only for a real change.
    pub(crate) fn reattach_scale(
        &mut self,
        surface: &wl_surface::WlSurface,
        qh: &QueueHandle<Self>,
    ) {
        let id = crate::wayland::window_of(surface);
        let current = self.scales.remove(&id).map(|s| {
            let c = s.current;
            s.destroy();
            c
        });
        self.attach_scale(surface, qh);
        if let (Some(c), Some(s)) = (current, self.scales.get_mut(&id)) {
            s.current = c;
        }
    }

    /// Child `id` starts at its parent's scale, as C makes the frame: a compositor tells a subsurface its scale once it
    /// shows, and a first buffer at scale 1 would show at twice its size.  The compositor's own scale counts from then on.
    pub(crate) fn inherit_scale(&mut self, id: u64, parent: u64) {
        let Some(scale) = self.scales.get(&parent).map(SurfaceScale::scale120) else {
            return;
        };
        if let Some(s) = self.scales.get_mut(&id) {
            s.current = scale;
        }
        if let Some(shared) = &self.shared {
            shared
                .scale120
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert(id, scale);
        }
    }

    pub(crate) fn detach_scale(&mut self, id: u64) {
        if let Some(s) = self.take_scale(id) {
            s.destroy();
        }
    }

    /// `detach_scale`, but the viewport lives on with whoever commits the surface (a ghost's fade): without it the
    /// surface would change size.
    pub(crate) fn detach_scale_keeping_viewport(&mut self, id: u64) {
        if let Some(mut s) = self.take_scale(id) {
            s.viewport = None;
            s.destroy();
        }
    }

    fn take_scale(&mut self, id: u64) -> Option<SurfaceScale> {
        if let Some(shared) = &self.shared {
            shared
                .scale120
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(&id);
        }
        self.scales.remove(&id)
    }

    /// Recompute surface `id`'s scale and post `SCALE` when it changed.
    pub(crate) fn recompute_scale(&mut self, id: u64) {
        let Some(s) = self.scales.get(&id) else {
            return;
        };
        // with a fractional-scale object the compositor's fractional scale alone counts: the integer hints round it
        // up (2 for 1.25), and taking one until it comes flipped the surface's scale twice (a frame's fonts opened at
        // 2, then at 1.25 again, dropping a size asked for in between)
        if s.fractional.is_some() && s.frac120.is_none() {
            return;
        }
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
        self.present_scale(id, new);
        if let Some(shared) = &self.shared {
            shared
                .scale120
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert(id, new);
        }
        let s = ScaleEv { scale120: new };
        self.post(Record::new(ShellEvent::Scale(s), id, 0));
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
    let id = crate::wayland::window_of(surface);
    match event {
        wl_surface::Event::Enter { output } => {
            if let (Some(&name), Some(s)) = (output.data::<u32>(), state.scales.get_mut(&id))
                && !s.entered.contains(&name)
            {
                s.entered.push(name);
                let o = OutputEv {
                    id: u64::from(name),
                    ..OutputEv::default()
                };
                state.post(Record::new(ShellEvent::EnterOutput(o), id, 0));
            }
        }
        wl_surface::Event::Leave { output } => {
            if let (Some(&name), Some(s)) = (output.data::<u32>(), state.scales.get_mut(&id)) {
                s.entered.retain(|&o| o != name);
                let o = OutputEv {
                    id: u64::from(name),
                    ..OutputEv::default()
                };
                state.post(Record::new(ShellEvent::LeaveOutput(o), id, 0));
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
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wp_fractional_scale_v1::Event::PreferredScale { scale } = event {
            state.present_rearm(*id);
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
    fn viewport_size_maps_exactly_or_cuts() {
        // sizes a logical size maps to: shown whole, as the rounded destination always did
        assert_eq!(viewport_size(400, 240), (200, 400));
        assert_eq!(viewport_size(400, 150), (320, 400));
        assert_eq!(viewport_size(401, 150), (321, 401));
        assert_eq!(viewport_size(1037, 180), (691, 1037));
        // none does: the largest under it, the last device column cut
        assert_eq!(viewport_size(401, 240), (200, 400));
        assert_eq!(viewport_size(402, 150), (321, 401));
        // every size and scale: exact or one short, never past the buffer
        for s in [120, 150, 180, 210, 240, 300] {
            for d in 1..600 {
                let (l, shown) = viewport_size(d, s);
                assert_eq!(glitnir::snap::stable_extent(0, l, s), shown);
                assert!(
                    shown <= d && glitnir::snap::stable_extent(0, l + 1, s) > d,
                    "{d} at {s}"
                );
            }
        }
        assert_eq!(viewport_size(0, 240), (0, 0));
    }

    #[test]
    fn buffer_size_examples() {
        assert_eq!(buffer_size(1280, 720, 180), (1920, 1080));
        assert_eq!(buffer_size(1229, 691, 180), (1844, 1037));
        assert_eq!(buffer_size(2048, 1152, 150), (2560, 1440));
        assert_eq!(buffer_size(1707, 960, 180), (2561, 1440));
    }
}

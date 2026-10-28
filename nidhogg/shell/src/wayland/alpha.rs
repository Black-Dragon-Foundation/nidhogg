//! Window opacity (`wp_alpha_modifier_v1`): the `alpha` frame parameter as a multiplier on the window's surface.
//!
//! The multiplier is double-buffered surface state.  Setting it makes the window owe a commit (a bare one carries
//! it), and a child's ancestors up to the root too, since a sync child's state applies with its parent's commit.
//! Without the protocol the host multiplies the pixels itself.

use crate::api::*;
use crate::wayland::{MainState, WaylandShell};
use wayland_client::delegate_noop;
use wayland_protocols::wp::alpha_modifier::v1::client::wp_alpha_modifier_surface_v1::WpAlphaModifierSurfaceV1;
use wayland_protocols::wp::alpha_modifier::v1::client::wp_alpha_modifier_v1::WpAlphaModifierV1;

delegate_noop!(MainState: WpAlphaModifierV1);
delegate_noop!(MainState: WpAlphaModifierSurfaceV1);

impl WaylandShell {
    /// Window `id`'s multiplier becomes `factor`; one alpha object per surface (a second is a protocol error).
    pub(super) fn window_set_alpha(&mut self, id: WindowId, factor: u32) -> ShellResult {
        let qh = self.main_queue.handle();
        {
            let m = &mut self.main;
            let manager = m.alpha_modifier.as_ref().ok_or(ShellError::Unsupported)?;
            let surface = m.surfaces.get(&id).ok_or(ShellError::State)?;
            m.alphas
                .entry(id)
                .or_insert_with(|| manager.get_surface(surface, &qh, ()))
                .set_multiplier(factor);
        }
        self.main.present_owe(id, true, true);
        self.flush()
    }

    /// Window `id` is going: its alpha object goes before its surface.
    pub(super) fn alpha_forget(&mut self, id: WindowId) {
        if let Some(a) = self.main.alphas.remove(&id) {
            a.destroy();
        }
    }
}

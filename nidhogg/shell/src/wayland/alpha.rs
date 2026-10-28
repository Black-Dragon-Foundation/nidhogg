//! Window opacity (`wp_alpha_modifier_v1`): the `alpha` frame parameter as a multiplier on the window's surface.
//!
//! The multiplier is double-buffered surface state.  Setting it records it on the window's presenter state and makes
//! the window owe a commit (a bare one carries it), and a child's ancestors up to the root too, since a sync child's
//! state applies with its parent's commit.  The alpha object belongs to the window's commit-time proxies and goes
//! with them.  Without the protocol the host multiplies the pixels itself.

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
        let m = &mut self.main;
        let manager = m.alpha_modifier.as_ref().ok_or(ShellError::Unsupported)?;
        if !m.presents.contains_key(&id) {
            return Err(ShellError::State);
        }
        m.present_record(id, |p| {
            let c = &mut p.commit;
            if c.alpha.is_none() {
                c.alpha = Some(manager.get_surface(&c.surface, &qh, ()));
            }
            p.rec.alpha = Some(factor);
        });
        self.main.present_owe(id, true, true);
        self.flush()
    }
}

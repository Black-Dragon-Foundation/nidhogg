//! Decoration negotiation (`zxdg_decoration_manager_v1`).
//!
//! A window asks for server-side decorations when the compositor offers the protocol and neither the window
//! (`NHSHELL_WIN_UNDECORATED`) nor the host (`NIDHOGG_SHELL_FORCE_CSD=1`) opts out.  The compositor's answer arrives
//! on the main queue before the surface's configure; the window keeps it, and `caps().ssd` follows the latest one.

use crate::wayland::MainState;
use wayland_client::{Connection, Dispatch, QueueHandle, WEnum, delegate_noop};
use wayland_protocols::xdg::decoration::zv1::client::zxdg_decoration_manager_v1::ZxdgDecorationManagerV1;
use wayland_protocols::xdg::decoration::zv1::client::zxdg_toplevel_decoration_v1::{
    self, Mode, ZxdgToplevelDecorationV1,
};

/// `NIDHOGG_SHELL_FORCE_CSD`: set and neither empty nor `0` forces client-side decorations (a test switch).
pub fn force_csd_from_env() -> bool {
    std::env::var_os("NIDHOGG_SHELL_FORCE_CSD").is_some_and(|v| !v.is_empty() && v != "0")
}

delegate_noop!(MainState: ZxdgDecorationManagerV1);

impl Dispatch<ZxdgToplevelDecorationV1, u64> for MainState {
    fn event(
        state: &mut Self,
        _: &ZxdgToplevelDecorationV1,
        event: zxdg_toplevel_decoration_v1::Event,
        id: &u64,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let zxdg_toplevel_decoration_v1::Event::Configure { mode } = event {
            let ssd = mode == WEnum::Value(Mode::ServerSide);
            if let Some(w) = state.windows.get_mut(id) {
                w.ssd = ssd;
            }
            state.ssd = ssd;
        }
    }
}

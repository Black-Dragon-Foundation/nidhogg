//! Windows: `wl_surface` + `xdg_surface` + `xdg_toplevel` on the main queue, and their configure sequencing.
//!
//! The window id is the surface's protocol id (the same id the keyboard's `FOCUS` records carry).  The compositor's
//! `xdg_toplevel.configure` only stores the size and states; the `xdg_surface.configure` that closes the group posts
//! one `CONFIGURE` record.  Nothing here acks: the host does, through `ack_configure`, right before the commit that
//! carries a buffer of the new size.  Records are posted without a wake: these handlers run inside the host's own
//! dispatch, and the host reads the records after it.

use crate::api::*;
use crate::event::*;
use crate::wayland::MainState;
use crate::wayland::WaylandShell;
use crate::wayland::serial::SerialKind;
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle};
use wayland_protocols::xdg::decoration::zv1::client::zxdg_toplevel_decoration_v1::{
    Mode, ZxdgToplevelDecorationV1,
};
use wayland_protocols::xdg::shell::client::xdg_surface::{self, XdgSurface};
use wayland_protocols::xdg::shell::client::xdg_toplevel::{self, ResizeEdge, State, XdgToplevel};

/// `xdg_toplevel.wm_capabilities`, as a mask (bit `n - 1` for the protocol's value `n`).
pub const WM_CAP_WINDOW_MENU: u32 = 1 << 0;
pub const WM_CAP_MAXIMIZE: u32 = 1 << 1;
pub const WM_CAP_FULLSCREEN: u32 = 1 << 2;
pub const WM_CAP_MINIMIZE: u32 = 1 << 3;
/// What a compositor that never sends `wm_capabilities` (`xdg_wm_base` < 5) is assumed to do.
const WM_CAP_ALL: u32 = 0b1111;

pub struct Window {
    pub surface: wayland_client::protocol::wl_surface::WlSurface,
    xdg_surface: XdgSurface,
    toplevel: XdgToplevel,
    decoration: Option<ZxdgToplevelDecorationV1>,
    /// Server-side decorations were granted to this window.
    pub ssd: bool,
    /// The newest `xdg_toplevel.wm_capabilities` (`WM_CAP_*`).
    pub wm_capabilities: u32,
    /// The size and states of the configure group in progress.
    pending: (i32, i32, u32),
}

fn state_bit(s: State) -> u32 {
    match s {
        State::Maximized => NHSHELL_STATE_MAXIMIZED,
        State::Fullscreen => NHSHELL_STATE_FULLSCREEN,
        State::Activated => NHSHELL_STATE_ACTIVATED,
        State::Resizing => NHSHELL_STATE_RESIZING,
        State::TiledLeft => NHSHELL_STATE_TILED_L,
        State::TiledRight => NHSHELL_STATE_TILED_R,
        State::TiledTop => NHSHELL_STATE_TILED_T,
        State::TiledBottom => NHSHELL_STATE_TILED_B,
        State::Suspended => NHSHELL_STATE_SUSPENDED,
        _ => 0,
    }
}

/// The protocol's `uint` array (native-endian words) as `u32`s.
fn words(bytes: &[u8]) -> impl Iterator<Item = u32> + '_ {
    bytes
        .as_chunks::<4>()
        .0
        .iter()
        .map(|c| u32::from_ne_bytes(*c))
}

impl WaylandShell {
    pub(super) fn window_create(&mut self, desc: &WindowDesc) -> ShellResult<WindowId> {
        let qh = self.main_queue.handle();
        let m = &mut self.main;
        let compositor = m.compositor.as_ref().ok_or(ShellError::State)?;
        let wm_base = m.wm_base.as_ref().ok_or(ShellError::State)?;
        let surface = compositor.create_surface(&qh, ());
        let id = u64::from(surface.id().protocol_id());
        let xdg_surface = wm_base.get_xdg_surface(&surface, &qh, id);
        let toplevel = xdg_surface.get_toplevel(&qh, id);
        toplevel.set_title(desc.title.clone());
        toplevel.set_app_id(desc.app_id.clone());
        if desc.flags & NHSHELL_WIN_FULLSCREEN != 0 {
            toplevel.set_fullscreen(None);
        }
        if desc.flags & NHSHELL_WIN_MAXIMIZED != 0 {
            toplevel.set_maximized();
        }
        let decoration = if m.force_csd || desc.flags & NHSHELL_WIN_UNDECORATED != 0 {
            None
        } else {
            m.decoration_manager.as_ref().map(|manager| {
                let d = manager.get_toplevel_decoration(&toplevel, &qh, id);
                d.set_mode(Mode::ServerSide);
                d
            })
        };
        // the window owns its surface's scale state (W0 Task 4), made before the first commit
        m.attach_scale(&surface, &qh);
        // the compositor sends the first configure only after this bare commit
        surface.commit();
        m.surfaces.insert(id, surface.clone());
        m.windows.insert(
            id,
            Window {
                surface,
                xdg_surface,
                toplevel,
                decoration,
                ssd: false,
                wm_capabilities: WM_CAP_ALL,
                pending: (0, 0, 0),
            },
        );
        self.flush()?;
        Ok(id)
    }

    fn window(&self, id: WindowId) -> ShellResult<&Window> {
        self.main.windows.get(&id).ok_or(ShellError::State)
    }

    /// The latest input serial the move, resize and menu requests are sent with (0 before any input).
    fn input_serial(&self) -> u32 {
        self.shared
            .serials
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(SerialKind::MousePress)
            .as_raw()
    }

    /// Whether server-side decorations were granted to window `id`.
    pub fn window_ssd(&self, id: WindowId) -> ShellResult<bool> {
        Ok(self.window(id)?.ssd)
    }

    /// The window's `xdg_toplevel.wm_capabilities` (`WM_CAP_*`; all set when the compositor never says).
    pub fn window_wm_capabilities(&self, id: WindowId) -> ShellResult<u32> {
        Ok(self.window(id)?.wm_capabilities)
    }

    pub(super) fn window_destroy_impl(&mut self, id: WindowId) -> ShellResult {
        let w = self.main.windows.remove(&id).ok_or(ShellError::State)?;
        self.main.surfaces.remove(&id);
        self.main.detach_scale(id);
        self.present_drop(id);
        if let Some(d) = &w.decoration {
            d.destroy();
        }
        w.toplevel.destroy();
        w.xdg_surface.destroy();
        w.surface.destroy();
        self.flush()
    }

    pub(super) fn window_set_title(&mut self, id: WindowId, title: &str) -> ShellResult {
        self.window(id)?.toplevel.set_title(title.to_owned());
        Ok(())
    }

    pub(super) fn window_set_app_id(&mut self, id: WindowId, app_id: &str) -> ShellResult {
        self.window(id)?.toplevel.set_app_id(app_id.to_owned());
        Ok(())
    }

    /// Takes effect with the surface's next commit.
    pub(super) fn window_set_min_size(&mut self, id: WindowId, w: i32, h: i32) -> ShellResult {
        self.window(id)?.toplevel.set_min_size(w.max(0), h.max(0));
        Ok(())
    }

    pub(super) fn window_set_fullscreen(&mut self, id: WindowId, on: bool) -> ShellResult {
        let t = &self.window(id)?.toplevel;
        if on {
            t.set_fullscreen(None);
        } else {
            t.unset_fullscreen();
        }
        self.flush()
    }

    pub(super) fn window_set_maximized(&mut self, id: WindowId, on: bool) -> ShellResult {
        let t = &self.window(id)?.toplevel;
        if on {
            t.set_maximized();
        } else {
            t.unset_maximized();
        }
        self.flush()
    }

    pub(super) fn window_minimize(&mut self, id: WindowId) -> ShellResult {
        self.window(id)?.toplevel.set_minimized();
        self.flush()
    }

    pub(super) fn window_move(&mut self, id: WindowId) -> ShellResult {
        let seat = self.main.seat.as_ref().ok_or(ShellError::State)?;
        self.window(id)?.toplevel._move(seat, self.input_serial());
        self.flush()
    }

    pub(super) fn window_resize(&mut self, id: WindowId, edges: u32) -> ShellResult {
        let edges = ResizeEdge::try_from(edges).map_err(|_| ShellError::Arg)?;
        let seat = self.main.seat.as_ref().ok_or(ShellError::State)?;
        self.window(id)?
            .toplevel
            .resize(seat, self.input_serial(), edges);
        self.flush()
    }

    pub(super) fn window_show_menu(&mut self, id: WindowId, x: i32, y: i32) -> ShellResult {
        let seat = self.main.seat.as_ref().ok_or(ShellError::State)?;
        self.window(id)?
            .toplevel
            .show_window_menu(seat, self.input_serial(), x, y);
        self.flush()
    }

    /// Ack configure `serial`; the host calls it right before the commit that carries the new size.
    pub(super) fn window_ack_configure(&mut self, id: WindowId, serial: u32) -> ShellResult {
        self.window(id)?.xdg_surface.ack_configure(serial);
        self.present_force_next(id);
        Ok(())
    }
}

impl Dispatch<XdgSurface, u64> for MainState {
    fn event(
        state: &mut Self,
        _: &XdgSurface,
        event: xdg_surface::Event,
        id: &u64,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_surface::Event::Configure { serial } = event
            && let (Some(w), Some(shared)) = (state.windows.get(id), state.shared.as_ref())
        {
            let (cw, ch, states) = w.pending;
            let mut ev = NhShellEvent::new(NHSHELL_EV_CONFIGURE, *id, 0);
            ev.u.configure = ConfigureEv {
                w: cw,
                h: ch,
                states,
                serial,
            };
            shared.post(ev);
        }
    }
}

impl Dispatch<XdgToplevel, u64> for MainState {
    fn event(
        state: &mut Self,
        _: &XdgToplevel,
        event: xdg_toplevel::Event,
        id: &u64,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            xdg_toplevel::Event::Configure {
                width,
                height,
                states,
            } => {
                if let Some(w) = state.windows.get_mut(id) {
                    let bits = words(&states)
                        .filter_map(|v| State::try_from(v).ok())
                        .fold(0, |acc, s| acc | state_bit(s));
                    w.pending = (width, height, bits);
                }
            }
            xdg_toplevel::Event::Close => {
                if let Some(shared) = state.shared.as_ref() {
                    shared.post(NhShellEvent::new(NHSHELL_EV_CLOSE, *id, 0));
                }
            }
            xdg_toplevel::Event::WmCapabilities { capabilities } => {
                if let Some(w) = state.windows.get_mut(id) {
                    w.wm_capabilities = words(&capabilities)
                        .filter(|v| (1..=32).contains(v))
                        .fold(0, |acc, v| acc | (1 << (v - 1)));
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn states_and_words() {
        let bytes: Vec<u8> = [1u32, 9, 5, 77]
            .iter()
            .flat_map(|v| v.to_ne_bytes())
            .collect();
        let bits = words(&bytes)
            .filter_map(|v| State::try_from(v).ok())
            .fold(0, |a, s| a | state_bit(s));
        assert_eq!(
            bits,
            NHSHELL_STATE_MAXIMIZED | NHSHELL_STATE_SUSPENDED | NHSHELL_STATE_TILED_L
        );
    }
}

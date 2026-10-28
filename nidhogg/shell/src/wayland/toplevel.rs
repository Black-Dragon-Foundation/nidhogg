//! Windows: `wl_surface` + `xdg_surface` + `xdg_toplevel` on the main queue, their configure sequencing, and the
//! shell API's implementation (`ShellBackend`).
//!
//! The window id is the surface's protocol id (the same id the keyboard's `FOCUS` records carry).  The compositor's
//! `xdg_toplevel.configure` only stores the size and states; the `xdg_surface.configure` that closes the group posts
//! one `CONFIGURE` record.  Nothing here acks: the host does, through `ack_configure`, right before the commit that
//! carries a buffer of the new size.  Records are posted without a wake: these handlers run inside the host's own
//! dispatch, and the host reads the records after it.

use crate::api::*;
use crate::caps::NhShellCaps;
use crate::event::*;
use crate::wayland::MainState;
use crate::wayland::WaylandShell;
use crate::wayland::serial::SerialKind;
use glitnir::geom::Rect;
use std::sync::atomic::Ordering;
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
    pub xdg_surface: XdgSurface,
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

    /// The window's `xdg_toplevel.wm_capabilities` (`WM_CAP_*`; all set when the compositor never says).
    pub fn window_wm_capabilities(&self, id: WindowId) -> ShellResult<u32> {
        Ok(self.window(id)?.wm_capabilities)
    }
}

impl ShellBackend for WaylandShell {
    fn wake_fd(&self) -> i32 {
        self.shared.wake.raw()
    }

    fn caps(&self) -> NhShellCaps {
        let mut caps = self.main.globals.caps();
        caps.ssd = self.main.ssd;
        caps.force_csd = self.main.force_csd;
        caps
    }

    /// The next record.  When the records are empty: drain the wake descriptor, dispatch the main queue if it is
    /// pending, and look again, so nothing posted in between is lost.
    fn next_event(&mut self) -> ShellResult<Option<NhShellEvent>> {
        loop {
            if let Some(ev) = self.shared.take()? {
                self.frame_ready_taken(&ev);
                return Ok(Some(ev));
            }
            self.shared.wake.drain();
            if self.shared.main_pending.swap(false, Ordering::AcqRel) {
                // an error here reaches the host as the reader's DISCONNECTED record
                let _ = self.dispatch_main_queue();
                continue;
            }
            let ev = self.shared.take()?;
            if let Some(ev) = &ev {
                self.frame_ready_taken(ev);
            }
            return Ok(ev);
        }
    }

    fn dispatch_main(&mut self) -> ShellResult {
        if self.shared.is_closed() {
            return Err(ShellError::Disconnected);
        }
        self.dispatch_main_queue()?;
        self.present_housekeeping();
        self.flush()
    }

    fn window_new(&mut self, desc: &WindowDesc) -> ShellResult<WindowId> {
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
        // an undecorated window asks for client-side mode (it draws none), so that `set_decorated` can change it
        let decoration = if m.force_csd {
            None
        } else {
            m.decoration_manager.as_ref().map(|manager| {
                let d = manager.get_toplevel_decoration(&toplevel, &qh, id);
                d.set_mode(if desc.flags & NHSHELL_WIN_UNDECORATED != 0 {
                    Mode::ClientSide
                } else {
                    Mode::ServerSide
                });
                d
            })
        };
        // the window owns its surface's scale state, made before the first commit
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

    fn window_destroy(&mut self, id: WindowId) -> ShellResult {
        self.text_input.forget(id);
        self.alpha_forget(id);
        for dep in self.dependents_of(id) {
            let _ = self.window_destroy(dep);
        }
        if self.main.children.contains_key(&id) {
            return self.child_destroy(id);
        }
        if self.main.popups.contains_key(&id) {
            return self.popup_destroy(id);
        }
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

    fn child_new(&mut self, desc: &ChildDesc) -> ShellResult<WindowId> {
        match desc.kind {
            NHSHELL_KIND_CHILD => self.child_create(desc),
            NHSHELL_KIND_POPUP => self.popup_create(desc),
            _ => Err(ShellError::Arg),
        }
    }

    fn set_position(&mut self, id: WindowId, x: i32, y: i32) -> ShellResult<bool> {
        self.window_set_position(id, x, y)
    }

    fn stack(&mut self, id: WindowId, sibling: WindowId, above: bool) -> ShellResult {
        self.window_stack(id, sibling, above)
    }

    fn set_input_region(&mut self, id: WindowId, rects: &[Rect]) -> ShellResult {
        self.window_set_input_region(id, rects)
    }

    fn set_mapped(&mut self, id: WindowId, on: bool) -> ShellResult {
        self.window_set_mapped(id, on)
    }

    fn set_title(&mut self, id: WindowId, title: &str) -> ShellResult {
        self.window(id)?.toplevel.set_title(title.to_owned());
        Ok(())
    }

    fn set_app_id(&mut self, id: WindowId, app_id: &str) -> ShellResult {
        self.window(id)?.toplevel.set_app_id(app_id.to_owned());
        Ok(())
    }

    /// Takes effect with the surface's next commit.
    fn set_min_size(&mut self, id: WindowId, w: i32, h: i32) -> ShellResult {
        self.window(id)?.toplevel.set_min_size(w.max(0), h.max(0));
        Ok(())
    }

    fn set_fullscreen(&mut self, id: WindowId, on: bool) -> ShellResult {
        let t = &self.window(id)?.toplevel;
        if on {
            t.set_fullscreen(None);
        } else {
            t.unset_fullscreen();
        }
        self.flush()
    }

    fn set_maximized(&mut self, id: WindowId, on: bool) -> ShellResult {
        let t = &self.window(id)?.toplevel;
        if on {
            t.set_maximized();
        } else {
            t.unset_maximized();
        }
        self.flush()
    }

    fn minimize(&mut self, id: WindowId) -> ShellResult {
        self.window(id)?.toplevel.set_minimized();
        self.flush()
    }

    fn move_window(&mut self, id: WindowId) -> ShellResult {
        let seat = self.main.seat.as_ref().ok_or(ShellError::State)?;
        self.window(id)?.toplevel._move(seat, self.input_serial());
        self.flush()
    }

    fn resize(&mut self, id: WindowId, edges: u32) -> ShellResult {
        let edges = ResizeEdge::try_from(edges).map_err(|_| ShellError::Arg)?;
        let seat = self.main.seat.as_ref().ok_or(ShellError::State)?;
        self.window(id)?
            .toplevel
            .resize(seat, self.input_serial(), edges);
        self.flush()
    }

    fn show_menu(&mut self, id: WindowId, x: i32, y: i32) -> ShellResult {
        let seat = self.main.seat.as_ref().ok_or(ShellError::State)?;
        self.window(id)?
            .toplevel
            .show_window_menu(seat, self.input_serial(), x, y);
        self.flush()
    }

    /// Takes effect with the surface's next commit.
    fn set_geometry(&mut self, id: WindowId, x: i32, y: i32, w: i32, h: i32) -> ShellResult {
        if w <= 0 || h <= 0 {
            return Err(ShellError::Arg);
        }
        self.window(id)?.xdg_surface.set_window_geometry(x, y, w, h);
        Ok(())
    }

    /// Ack configure `serial`; the host calls it right before the commit that carries the new size.
    fn ack_configure(&mut self, id: WindowId, serial: u32, flags: u32) -> ShellResult {
        self.window(id)?;
        self.main
            .present_ack(id, serial, flags & NHSHELL_ACK_CONTENT != 0);
        Ok(())
    }

    fn window_dirty(&mut self, id: WindowId) -> ShellResult {
        self.window_dirty_impl(id)
    }

    fn window_ready(&mut self, id: WindowId) -> ShellResult<u32> {
        self.window_ready_impl(id)
    }

    fn present_bare(&mut self, id: WindowId) -> ShellResult {
        self.present_bare_impl(id)
    }

    fn set_cursor(&mut self, id: WindowId, shape: u32) -> ShellResult {
        self.window_set_cursor(id, shape)
    }

    fn set_cursor_theme(&mut self, name: &str, size: u32) -> ShellResult {
        self.set_cursor_theme_impl(name, size)
    }

    fn window_activate(&mut self, w: WindowId, token: Option<&str>) -> ShellResult {
        self.window_activate_impl(w, token)
    }

    fn window_scale(&self, id: WindowId) -> ShellResult<u32> {
        Ok(self
            .main
            .scales
            .get(&id)
            .ok_or(ShellError::State)?
            .scale120())
    }

    fn window_ssd(&self, id: WindowId) -> ShellResult<bool> {
        Ok(self.window(id)?.ssd)
    }

    /// Every top-level has its decoration object from the start (an undecorated one asks for client-side mode), so
    /// the mode can change after a buffer: version 1 forbids making the object then.
    fn set_decorated(&mut self, id: WindowId, on: bool) -> ShellResult {
        let force_csd = self.main.force_csd;
        if let Some(d) = &self.window(id)?.decoration {
            d.set_mode(if on && !force_csd {
                Mode::ServerSide
            } else {
                Mode::ClientSide
            });
        }
        self.flush()
    }

    fn set_alpha(&mut self, id: WindowId, factor: u32) -> ShellResult {
        self.window_set_alpha(id, factor)
    }

    fn present_begin(
        &mut self,
        id: WindowId,
        dev_w: i32,
        dev_h: i32,
    ) -> ShellResult<NhShellBuffer> {
        self.present_begin_impl(id, dev_w, dev_h)
    }

    fn present_commit(&mut self, id: WindowId, damage: &[Rect]) -> ShellResult {
        self.present_commit_impl(id, damage)
    }

    fn publish(
        &mut self,
        root: WindowId,
        frames: &[SurfaceFrame],
        t_key_ns: u64,
    ) -> ShellResult<bool> {
        self.publish_impl(root, frames, t_key_ns)
    }

    fn present_tree(&mut self, root: WindowId) -> ShellResult<bool> {
        self.present_tree_impl(root)
    }

    fn set_presenter_thread(&mut self, on: bool) -> ShellResult {
        self.set_presenter_thread_impl(on)
    }

    fn presenter_stats(&self) -> ShellResult<NhShellPresenterStats> {
        Ok(self.presenter_stats_impl())
    }

    fn text_input_enable(&mut self, w: WindowId, on: bool) -> ShellResult {
        self.text_input_flush(self.main.surfaces.contains_key(&w), |t| t.enable(w, on))
    }

    fn text_input_cursor(&mut self, w: WindowId, x: i32, y: i32, cw: i32, ch: i32) -> ShellResult {
        self.text_input_flush(self.main.surfaces.contains_key(&w), |t| {
            t.cursor(w, [x, y, cw, ch])
        })
    }

    fn text_input_surrounding(
        &mut self,
        w: WindowId,
        text: &str,
        cursor: usize,
        anchor: usize,
    ) -> ShellResult {
        self.text_input_flush(self.main.surfaces.contains_key(&w), |t| {
            t.surrounding(w, text, cursor, anchor)
        })
    }

    fn text_len(&self, id: u32) -> ShellResult<usize> {
        self.text_input.text_len(id)
    }

    fn take_text(&mut self, id: u32) -> ShellResult<Vec<u8>> {
        self.text_input.take_text(id)
    }

    fn selection_own(&mut self, kind: u32, items: &[(&str, &[u8])]) -> ShellResult {
        self.sel_own(kind, items)
    }

    fn selection_disown(&mut self, kind: u32) -> ShellResult {
        self.sel_disown(kind)
    }

    fn selection_owned(&self, kind: u32) -> ShellResult<bool> {
        self.sel_owned(kind)
    }

    fn selection_types(&self, kind: u32) -> ShellResult<Vec<String>> {
        self.sel_types(kind)
    }

    fn selection_read(&mut self, kind: u32, mime: &str, timeout_ms: u32) -> ShellResult<Vec<u8>> {
        self.sel_read(kind, mime, timeout_ms)
    }
}

impl WaylandShell {
    /// Run a text-input request for a known window and flush it if it sent anything.
    fn text_input_flush(
        &self,
        known: bool,
        f: impl FnOnce(&crate::wayland::text_input::TextInput) -> ShellResult<bool>,
    ) -> ShellResult {
        if !known {
            return Err(ShellError::State);
        }
        if f(&self.text_input)? {
            self.flush()?;
        }
        Ok(())
    }
}

impl Dispatch<XdgSurface, u64> for MainState {
    fn event(
        state: &mut Self,
        _: &XdgSurface,
        event: xdg_surface::Event,
        id: &u64,
        conn: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_surface::Event::Configure { serial } = event
            && state.popup_ack(*id, serial)
        {
            return;
        }
        if let xdg_surface::Event::Configure { serial } = event
            && let (Some(w), Some(shared)) = (state.windows.get(id), state.shared.as_ref())
        {
            let (cw, ch, states) = w.pending;
            let suspended = states & NHSHELL_STATE_SUSPENDED != 0;
            let mut ev = NhShellEvent::new(NHSHELL_EV_CONFIGURE, *id, 0);
            ev.u.configure = ConfigureEv {
                w: cw,
                h: ch,
                states,
                serial,
            };
            shared.post(ev);
            state.present_configured(*id, suspended, conn);
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

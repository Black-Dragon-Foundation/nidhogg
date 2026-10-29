//! Windows: `wl_surface` + `xdg_surface` + `xdg_toplevel` on the main queue, their configure sequencing, and the
//! shell API's implementation (`ShellBackend`).
//!
//! The window id is the surface's protocol id (the same id the keyboard's `FOCUS` records carry); a window made again
//! after a reconnect keeps its id on its new surface (`window_of`).  The compositor's
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
use wayland_client::protocol::wl_surface::WlSurface;
use wayland_client::{Connection, Dispatch, QueueHandle};
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
    pub surface: WlSurface,
    pub xdg_surface: XdgSurface,
    toplevel: XdgToplevel,
    decoration: Option<ZxdgToplevelDecorationV1>,
    /// Server-side decorations were granted to this window.
    pub ssd: bool,
    /// The newest `xdg_toplevel.wm_capabilities` (`WM_CAP_*`).
    pub wm_capabilities: u32,
    /// The size and states of the configure group in progress.
    pending: (i32, i32, u32),
    /// What the window asked of its `xdg_toplevel`, which a reconnect asks again.
    asked: Asked,
}

/// What a top-level asked for that its role objects carry.
#[derive(Clone)]
struct Asked {
    title: String,
    app_id: String,
    min: (i32, i32),
    /// `NHSHELL_STATE_MAXIMIZED` and `_FULLSCREEN`: asked for, or what the newest configure granted.
    states: u32,
    /// Server-side decorations asked for (`NHSHELL_WIN_UNDECORATED`, `set_decorated`).
    decorated: bool,
}

/// The states a top-level asks for again after a reconnect.
const ASKED_STATES: u32 = NHSHELL_STATE_MAXIMIZED | NHSHELL_STATE_FULLSCREEN;

/// The objects of a top-level's role.
type Role = (XdgSurface, XdgToplevel, Option<ZxdgToplevelDecorationV1>);

impl MainState {
    /// Give `surface` (window `id`) the top-level role with what `asked` says and its session entry (`name`, restored
    /// when the flag says so), then the bare commit after which the compositor configures it.  The window's scale state
    /// is made before (the caller's).
    fn toplevel_role(
        &mut self,
        surface: &WlSurface,
        id: u64,
        asked: &Asked,
        name: Option<(&str, bool)>,
        qh: &QueueHandle<MainState>,
    ) -> ShellResult<Role> {
        let wm_base = self.wm_base.as_ref().ok_or(ShellError::State)?;
        let xdg_surface = wm_base.get_xdg_surface(surface, qh, id);
        let toplevel = xdg_surface.get_toplevel(qh, id);
        toplevel.set_title(asked.title.clone());
        toplevel.set_app_id(asked.app_id.clone());
        if asked.min != (0, 0) {
            toplevel.set_min_size(asked.min.0, asked.min.1);
        }
        if asked.states & NHSHELL_STATE_FULLSCREEN != 0 {
            toplevel.set_fullscreen(None);
        }
        if asked.states & NHSHELL_STATE_MAXIMIZED != 0 {
            toplevel.set_maximized();
        }
        // an undecorated window asks for client-side mode (it draws none), so that `set_decorated` can change it
        let decoration = if self.force_csd {
            None
        } else {
            self.decoration_manager.as_ref().map(|manager| {
                let d = manager.get_toplevel_decoration(&toplevel, qh, id);
                d.set_mode(if asked.decorated {
                    Mode::ServerSide
                } else {
                    Mode::ClientSide
                });
                d
            })
        };
        // the session entry goes before the commit that sets the role up
        if let Some((name, restore)) = name {
            self.session.attach(&toplevel, id, name, restore, qh);
        }
        // the compositor sends the first configure only after this bare commit
        surface.commit();
        Ok((xdg_surface, toplevel, decoration))
    }
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

    fn window_mut(&mut self, id: WindowId) -> ShellResult<&mut Window> {
        self.main.windows.get_mut(&id).ok_or(ShellError::State)
    }

    /// Ask for state `bit` (`ASKED_STATES`) on or off.
    fn ask_state(&mut self, id: WindowId, bit: u32, on: bool) -> ShellResult<&XdgToplevel> {
        let w = self.window_mut(id)?;
        w.asked.states = if on {
            w.asked.states | bit
        } else {
            w.asked.states & !bit
        };
        Ok(&w.toplevel)
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
    fn next_event(&mut self) -> ShellResult<Option<Record>> {
        loop {
            if let Some(ev) = self.take_record()? {
                self.frame_ready_taken(&ev);
                return Ok(Some(ev));
            }
            self.shared.wake.drain();
            if self.shared.main_pending.swap(false, Ordering::AcqRel) {
                // an error here reaches the host as the reader's DISCONNECTED record
                let _ = self.dispatch_main_queue();
                continue;
            }
            let ev = self.take_record()?;
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
        // nothing to do while reconnecting: `reconnect` reads the new connection
        if self.reconnect.is_some() || self.shared.reconnect_pending() {
            return Ok(());
        }
        self.dispatch_main_queue()?;
        self.present_housekeeping();
        self.flush()
    }

    fn last_error(&self) -> String {
        self.last_error_impl()
    }

    fn set_reconnect(&mut self, on: bool) -> ShellResult {
        self.set_reconnect_impl(on)
    }

    fn connection_state(&self) -> ConnectionState {
        self.connection_state_impl()
    }

    fn reconnect(&mut self, timeout_ms: u32) -> ShellResult {
        self.reconnect_impl(timeout_ms)
    }

    fn window_new_named(
        &mut self,
        desc: &WindowDesc,
        session_name: Option<&str>,
    ) -> ShellResult<WindowId> {
        self.live()?;
        let qh = self.main_queue.handle();
        let session_name = self.main.session.name_for(session_name)?;
        let m = &mut self.main;
        if m.wm_base.is_none() {
            return Err(ShellError::State);
        }
        let (surface, id) = m.window_surface(&qh)?;
        let flag = |f: u32, state: u32| if desc.flags & f != 0 { state } else { 0 };
        let asked = Asked {
            title: desc.title.clone(),
            app_id: desc.app_id.clone(),
            min: (0, 0),
            states: flag(NHSHELL_WIN_FULLSCREEN, NHSHELL_STATE_FULLSCREEN)
                | flag(NHSHELL_WIN_MAXIMIZED, NHSHELL_STATE_MAXIMIZED),
            decorated: desc.flags & NHSHELL_WIN_UNDECORATED == 0,
        };
        // the window owns its surface's scale state, made before the first commit
        m.attach_scale(&surface, &qh);
        let restore = desc.flags & NHSHELL_WIN_RESTORE != 0;
        let name = session_name.as_deref().map(|n| (n, restore));
        let (xdg_surface, toplevel, decoration) =
            m.toplevel_role(&surface, id, &asked, name, &qh)?;
        m.surfaces.insert(id, surface.clone());
        m.windows.insert(
            id,
            Window {
                surface,
                xdg_surface: xdg_surface.clone(),
                toplevel,
                decoration,
                ssd: false,
                wm_capabilities: WM_CAP_ALL,
                pending: (0, 0, 0),
                asked,
            },
        );
        m.present_register(id, Some(xdg_surface), None);
        self.flush()?;
        Ok(id)
    }

    /// After the loss the window's local state still goes (its buffers are unmapped), and the call reports the loss.
    /// While reconnecting the record is dropped and the call succeeds.
    fn window_destroy(&mut self, id: WindowId) -> ShellResult {
        let live = self.live();
        let done = self.window_destroy_local(id);
        match (live, done) {
            // in the gap the window goes locally; the dead connection's flush is not the call's failure
            (Err(ShellError::Reconnecting), Err(ShellError::Disconnected)) => Ok(()),
            (Err(ShellError::Reconnecting), done) => done,
            (live, done) => live.and(done),
        }
    }

    fn child_new(&mut self, desc: &ChildDesc) -> ShellResult<WindowId> {
        self.live()?;
        match desc.kind {
            NHSHELL_KIND_CHILD => self.child_create(desc),
            NHSHELL_KIND_POPUP => self.popup_create(desc),
            _ => Err(ShellError::Arg),
        }
    }

    fn set_position(&mut self, id: WindowId, x: i32, y: i32) -> ShellResult<bool> {
        self.live()?;
        self.window_set_position(id, x, y)
    }

    fn stack(&mut self, id: WindowId, sibling: WindowId, above: bool) -> ShellResult {
        self.live()?;
        self.window_stack(id, sibling, above)
    }

    fn set_input_region(&mut self, id: WindowId, rects: &[Rect]) -> ShellResult {
        self.live()?;
        self.window_set_input_region(id, rects)
    }

    fn set_mapped(&mut self, id: WindowId, on: bool) -> ShellResult {
        self.live()?;
        self.window_set_mapped(id, on)
    }

    fn window_grabbed(&self, id: WindowId) -> ShellResult<bool> {
        self.live()?;
        self.popup_grabbed(id)
    }

    fn set_size(&mut self, id: WindowId, w: i32, h: i32) -> ShellResult<bool> {
        self.live()?;
        self.popup_set_size(id, w, h)
    }

    fn set_title(&mut self, id: WindowId, title: &str) -> ShellResult {
        self.live()?;
        let w = self.window_mut(id)?;
        w.asked.title = title.to_owned();
        w.toplevel.set_title(title.to_owned());
        Ok(())
    }

    fn set_app_id(&mut self, id: WindowId, app_id: &str) -> ShellResult {
        self.live()?;
        let w = self.window_mut(id)?;
        w.asked.app_id = app_id.to_owned();
        w.toplevel.set_app_id(app_id.to_owned());
        Ok(())
    }

    /// Takes effect with the surface's next commit.
    fn set_min_size(&mut self, id: WindowId, w: i32, h: i32) -> ShellResult {
        self.live()?;
        let win = self.window_mut(id)?;
        win.asked.min = (w.max(0), h.max(0));
        win.toplevel.set_min_size(w.max(0), h.max(0));
        Ok(())
    }

    fn set_fullscreen(&mut self, id: WindowId, on: bool) -> ShellResult {
        self.live()?;
        let t = self.ask_state(id, NHSHELL_STATE_FULLSCREEN, on)?;
        if on {
            t.set_fullscreen(None);
        } else {
            t.unset_fullscreen();
        }
        self.flush()
    }

    fn set_maximized(&mut self, id: WindowId, on: bool) -> ShellResult {
        self.live()?;
        let t = self.ask_state(id, NHSHELL_STATE_MAXIMIZED, on)?;
        if on {
            t.set_maximized();
        } else {
            t.unset_maximized();
        }
        self.flush()
    }

    fn minimize(&mut self, id: WindowId) -> ShellResult {
        self.live()?;
        self.window(id)?.toplevel.set_minimized();
        self.flush()
    }

    fn move_window(&mut self, id: WindowId) -> ShellResult {
        self.live()?;
        let seat = self.main.seat.as_ref().ok_or(ShellError::State)?;
        self.window(id)?.toplevel._move(seat, self.input_serial());
        self.flush()
    }

    fn resize(&mut self, id: WindowId, edges: u32) -> ShellResult {
        self.live()?;
        let edges = ResizeEdge::try_from(edges).map_err(|_| ShellError::Arg)?;
        let seat = self.main.seat.as_ref().ok_or(ShellError::State)?;
        self.window(id)?
            .toplevel
            .resize(seat, self.input_serial(), edges);
        self.flush()
    }

    fn show_menu(&mut self, id: WindowId, x: i32, y: i32) -> ShellResult {
        self.live()?;
        let seat = self.main.seat.as_ref().ok_or(ShellError::State)?;
        self.window(id)?
            .toplevel
            .show_window_menu(seat, self.input_serial(), x, y);
        self.flush()
    }

    /// Recorded: it goes with the window's next commit that carries a buffer.
    fn set_geometry(&mut self, id: WindowId, x: i32, y: i32, w: i32, h: i32) -> ShellResult {
        self.live()?;
        if w <= 0 || h <= 0 {
            return Err(ShellError::Arg);
        }
        self.window(id)?;
        self.main
            .present_record(id, |p| p.rec.geometry = Some(Rect::new(x, y, w, h)));
        Ok(())
    }

    /// Ack configure `serial`; the host calls it right before the commit that carries the new size.
    fn ack_configure(&mut self, id: WindowId, serial: u32, flags: u32) -> ShellResult {
        self.live()?;
        self.window(id)?;
        self.main
            .present_ack(id, serial, flags & NHSHELL_ACK_CONTENT != 0);
        Ok(())
    }

    fn set_cursor(&mut self, id: WindowId, shape: u32) -> ShellResult {
        self.live()?;
        self.window_set_cursor(id, shape)
    }

    fn set_cursor_theme(&mut self, name: &str, size: u32) -> ShellResult {
        self.live()?;
        self.set_cursor_theme_impl(name, size)
    }

    fn window_activate(&mut self, w: WindowId, token: Option<&str>) -> ShellResult {
        self.live()?;
        self.window_activate_impl(w, token)
    }

    fn activation_token(&mut self, id: WindowId, timeout_ms: u32) -> ShellResult<String> {
        self.live()?;
        let r = self.activation_token_impl(id, timeout_ms);
        self.waited(r)
    }

    fn bell(&mut self, id: WindowId) -> ShellResult {
        self.live()?;
        self.bell_impl(id)
    }

    fn inhibit_idle(&mut self, id: WindowId, on: bool) -> ShellResult {
        self.live()?;
        self.inhibit_idle_impl(id, on)
    }

    fn window_export(&mut self, id: WindowId, timeout_ms: u32) -> ShellResult<String> {
        self.live()?;
        let r = self.export_impl(id, timeout_ms);
        self.waited(r)
    }

    fn session_open(&mut self, id: Option<&str>, reason: u32) -> ShellResult {
        self.live()?;
        self.session_open_impl(id, reason)
    }

    fn session_id(&self) -> ShellResult<String> {
        self.live()?;
        self.session_id_impl()
    }

    fn window_session_remove(&mut self, id: WindowId) -> ShellResult {
        self.live()?;
        self.session_remove_impl(id)
    }

    fn window_scale(&self, id: WindowId) -> ShellResult<u32> {
        self.live()?;
        Ok(self
            .main
            .scales
            .get(&id)
            .ok_or(ShellError::State)?
            .scale120())
    }

    fn window_ssd(&self, id: WindowId) -> ShellResult<bool> {
        self.live()?;
        Ok(self.window(id)?.ssd)
    }

    /// Every top-level has its decoration object from the start (an undecorated one asks for client-side mode), so
    /// the mode can change after a buffer: version 1 forbids making the object then.
    fn set_decorated(&mut self, id: WindowId, on: bool) -> ShellResult {
        self.live()?;
        let force_csd = self.main.force_csd;
        let w = self.window_mut(id)?;
        w.asked.decorated = on;
        if let Some(d) = &w.decoration {
            d.set_mode(if on && !force_csd {
                Mode::ServerSide
            } else {
                Mode::ClientSide
            });
        }
        self.flush()
    }

    fn set_alpha(&mut self, id: WindowId, factor: u32) -> ShellResult {
        self.live()?;
        self.window_set_alpha(id, factor)
    }

    fn publish(
        &mut self,
        root: WindowId,
        frames: &[SurfaceFrame],
        t_key_ns: u64,
    ) -> ShellResult<bool> {
        self.live()?;
        self.publish_impl(root, frames, t_key_ns)
    }

    fn present_tree(&mut self, root: WindowId) -> ShellResult<bool> {
        self.live()?;
        self.present_tree_impl(root)
    }

    fn set_presenter_thread(&mut self, on: bool) -> ShellResult {
        self.live()?;
        self.set_presenter_thread_impl(on)
    }

    fn presenter_stats(&self) -> ShellResult<NhShellPresenterStats> {
        self.live()?;
        Ok(self.presenter_stats_impl())
    }

    /// Works on a lost connection too: it touches nothing.
    fn abandon(&self) -> Abandon {
        self.abandon_impl()
    }

    fn presenter_timing(&self) -> ShellResult<PresenterTiming> {
        self.live()?;
        Ok(self.presenting.stats.timing())
    }

    fn text_input_enable(&mut self, w: WindowId, on: bool) -> ShellResult {
        self.live()?;
        self.text_input_flush(self.main.surfaces.contains_key(&w), |t| t.enable(w, on))
    }

    fn text_input_cursor(&mut self, w: WindowId, x: i32, y: i32, cw: i32, ch: i32) -> ShellResult {
        self.live()?;
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
        self.live()?;
        self.text_input_flush(self.main.surfaces.contains_key(&w), |t| {
            t.surrounding(w, text, cursor, anchor)
        })
    }

    fn text_len(&self, id: u32) -> ShellResult<usize> {
        self.live()?;
        self.text_input.text_len(id)
    }

    fn take_text(&mut self, id: u32) -> ShellResult<Vec<u8>> {
        self.live()?;
        self.text_input.take_text(id)
    }

    fn selection_own(&mut self, kind: u32, items: &[(&str, &[u8])]) -> ShellResult {
        self.live()?;
        self.sel_own(kind, items)
    }

    fn selection_disown(&mut self, kind: u32) -> ShellResult {
        self.live()?;
        self.sel_disown(kind)
    }

    fn selection_owned(&self, kind: u32) -> ShellResult<bool> {
        self.live()?;
        self.sel_owned(kind)
    }

    fn selection_types(&self, kind: u32) -> ShellResult<Vec<String>> {
        self.live()?;
        self.sel_types(kind)
    }

    fn selection_read(&mut self, kind: u32, mime: &str, timeout_ms: u32) -> ShellResult<Vec<u8>> {
        self.live()?;
        let r = self.sel_read(kind, mime, timeout_ms);
        self.waited(r)
    }

    fn dnd_status(&mut self, mime: Option<&str>, actions: u32, preferred: u32) -> ShellResult {
        self.live()?;
        self.dnd_status_impl(mime, actions, preferred)
    }

    fn dnd_finish(&mut self, success: bool) -> ShellResult {
        self.live()?;
        self.dnd_finish_impl(success)
    }

    fn drag_start(&mut self, w: WindowId, items: &[(&str, &[u8])], actions: u32) -> ShellResult {
        self.live()?;
        self.drag_start_impl(w, items, actions)
    }

    fn drag_cancel(&mut self) -> ShellResult {
        self.live()?;
        self.drag_cancel_impl()
    }
}

/// The host-driven presenter, which the host no longer uses (it publishes, and `present_tree` presents): kept outside
/// `ShellBackend` for the T1 tests that drive a window's pacer and buffers directly.
impl WaylandShell {
    /// The host drew something: `FRAME_READY` follows when the window's pacer lets it present.
    pub fn window_dirty(&mut self, id: WindowId) -> ShellResult {
        self.live()?;
        self.window_dirty_impl(id)
    }

    /// The `NHSHELL_READY_*` bits: what the pacer lets the host do now (0: nothing).  Reporting `CONTENT` takes the
    /// draw (the pacer is not ready again until `window_dirty`, unless a content ack is held).
    pub fn window_ready(&mut self, id: WindowId) -> ShellResult<u32> {
        self.live()?;
        self.window_ready_impl(id)
    }

    /// A commit without a buffer, frame callback or feedback: a bare ack, and the surface state the window owes.
    pub fn present_bare(&mut self, id: WindowId) -> ShellResult {
        self.live()?;
        self.present_bare_impl(id)
    }

    /// Begin a frame of `dev_w` x `dev_h` device pixels.
    pub fn present_begin(
        &mut self,
        id: WindowId,
        dev_w: i32,
        dev_h: i32,
    ) -> ShellResult<NhShellBuffer> {
        self.live()?;
        self.present_begin_impl(id, dev_w, dev_h)
    }

    /// Show the begun buffer; `damage` is in device pixels.
    pub fn present_commit(&mut self, id: WindowId, damage: &[Rect]) -> ShellResult {
        self.live()?;
        self.present_commit_impl(id, damage)
    }
}

impl WaylandShell {
    /// `window_destroy` without the connection's check: the window and its dependents go, popups topmost first.
    pub(super) fn window_destroy_local(&mut self, id: WindowId) -> ShellResult {
        self.text_input.forget(id);
        self.extras_forget(id);
        for dep in self.dependents_of(id) {
            let _ = self.window_destroy_local(dep);
        }
        if self.main.children.contains_key(&id) {
            return self.child_destroy(id);
        }
        if self.main.popups.contains_key(&id) {
            return self.popup_destroy(id);
        }
        if !self.main.windows.contains_key(&id) {
            return Err(ShellError::State);
        }
        self.present_retire(id, |m| {
            m.surfaces.remove(&id);
            m.detach_scale(id);
            if let Some(w) = m.windows.remove(&id) {
                if let Some(d) = &w.decoration {
                    d.destroy();
                }
                w.toplevel.destroy();
                w.xdg_surface.destroy();
                w.surface.destroy();
            }
        });
        self.flush()
    }

    /// Make top-level `id` again on the new connection (a reconnect): a surface that keeps the id, its scale state, the
    /// role with everything it asked for, and its session entry restored under `name`, before the bare commit.  Its
    /// old objects went with the old connection.
    pub(super) fn toplevel_again(&mut self, id: WindowId, name: Option<&str>) -> ShellResult {
        let qh = self.main_queue.handle();
        let m = &mut self.main;
        let asked = m.windows.get(&id).ok_or(ShellError::State)?.asked.clone();
        let surface = m.window_surface_again(id, &qh).ok_or(ShellError::State)?;
        m.reattach_scale(&surface, &qh);
        let (xdg_surface, toplevel, decoration) =
            m.toplevel_role(&surface, id, &asked, name.map(|n| (n, true)), &qh)?;
        m.surfaces.insert(id, surface.clone());
        m.windows.insert(
            id,
            Window {
                surface,
                xdg_surface: xdg_surface.clone(),
                toplevel,
                decoration,
                ssd: false,
                wm_capabilities: WM_CAP_ALL,
                pending: (0, 0, 0),
                asked,
            },
        );
        m.present_again(id, Some(xdg_surface), None, &qh);
        Ok(())
    }

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
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_surface::Event::Configure { serial } = event
            && state.popup_ack(*id, serial)
        {
            return;
        }
        if let xdg_surface::Event::Configure { serial } = event
            && let (Some(w), Some(shared)) = (state.windows.get_mut(id), state.shared.as_ref())
        {
            let (cw, ch, states) = w.pending;
            // what the compositor granted is what a reconnect asks for again
            w.asked.states = states & ASKED_STATES;
            let suspended = states & NHSHELL_STATE_SUSPENDED != 0;
            let c = ConfigureEv {
                w: cw,
                h: ch,
                states,
                serial,
            };
            shared.post(Record::new(ShellEvent::Configure(c), *id, 0));
            state.present_configured(*id, suspended);
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
                    shared.post(Record::new(ShellEvent::Close, *id, 0));
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

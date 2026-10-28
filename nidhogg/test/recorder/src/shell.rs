//! xdg-shell scripting: initial and scripted configures with states, the decoration mode, ping/pong.
use crate::control::{err, write_reply};
use crate::state::{role_name, PendingPing, State};
use serde_json::{json, Value};
use smithay::delegate_xdg_decoration;
use smithay::delegate_xdg_shell;
use smithay::reexports::wayland_protocols::xdg::decoration::zv1::server::zxdg_toplevel_decoration_v1::Mode;
use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_popup;
use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::State as TState;
use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_wm_base::{self, XdgWmBase};
use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::ResizeEdge;
use smithay::reexports::wayland_server::protocol::wl_output::WlOutput;
use smithay::reexports::wayland_server::protocol::wl_seat::WlSeat;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::reexports::wayland_server::Resource;
use smithay::backend::renderer::utils::RendererSurfaceStateUserData;
use smithay::utils::{Logical, Point, Serial, SERIAL_COUNTER};
use smithay::wayland::compositor::{self, RectangleKind, SubsurfaceCachedState, SurfaceAttributes};
use smithay::wayland::shell::xdg::SurfaceCachedState as XdgCachedState;
use smithay::wayland::shell::xdg::XdgPopupSurfaceData;
use smithay::wayland::shm::with_buffer_contents;
use smithay::wayland::shell::xdg::decoration::XdgDecorationHandler;
use smithay::wayland::shell::xdg::{
    Configure, PopupSurface, PositionerState, ShellClient, ToplevelSurface, XdgShellHandler,
    XdgShellState,
};
use std::os::unix::net::UnixStream;
use std::rc::Rc;
use std::time::{Duration, Instant};

const PING_TIMEOUT: Duration = Duration::from_millis(5000);

/// Every state the `configure` command can set; a configure carries exactly the ones it names.
const STATES: [(&str, TState); 9] = [
    ("maximized", TState::Maximized),
    ("fullscreen", TState::Fullscreen),
    ("activated", TState::Activated),
    ("resizing", TState::Resizing),
    ("tiled_left", TState::TiledLeft),
    ("tiled_right", TState::TiledRight),
    ("tiled_top", TState::TiledTop),
    ("tiled_bottom", TState::TiledBottom),
    ("suspended", TState::Suspended),
];

impl State {
    fn note_sent(&mut self, surface: &WlSurface, serial: Serial) {
        self.sent_serial.insert(surface.id(), u32::from(serial));
    }

    /// The configure a real compositor sends after a surface's first commit.
    pub fn initial_configure(&mut self, surface: &WlSurface) {
        let toplevel = self
            .toplevels
            .iter()
            .find(|t| t.wl_surface() == surface)
            .cloned();
        if let Some(t) = toplevel {
            if !t.is_initial_configure_sent() {
                self.session_before_configure(&t);
                let serial = t.send_configure();
                self.note_sent(surface, serial);
            }
            return;
        }
        let popup = self
            .popups
            .iter()
            .find(|p| p.wl_surface() == surface)
            .cloned();
        if let Some(p) = popup {
            if !p.is_initial_configure_sent() {
                if let Ok(serial) = p.send_configure() {
                    self.note_sent(surface, serial);
                }
            }
        }
    }

    /// An interactive xdg_toplevel request, for the input log (`serial` is null for the requests that carry none).
    fn log_toplevel(
        &mut self,
        t: &ToplevelSurface,
        req: &str,
        serial: Option<Serial>,
        extra: Value,
    ) {
        let mut entry = json!({
            "req": format!("xdg_toplevel.{req}"),
            "surface": t.wl_surface().id().protocol_id(),
            "serial": serial.map(u32::from),
        });
        if let (Some(e), Some(x)) = (entry.as_object_mut(), extra.as_object()) {
            e.extend(x.clone());
        }
        self.log_input(entry);
    }

    fn send_decoration(&mut self, toplevel: &ToplevelSurface) {
        let mode = self.decoration_mode;
        toplevel.with_pending_state(|s| s.decoration_mode = Some(mode));
        // before the first commit the initial configure (sent after it) carries the mode: a configure now would
        // arrive before the client has committed (a client may send its decoration request before its first commit)
        if toplevel.is_initial_configure_sent() {
            let serial = toplevel.send_configure();
            self.note_sent(toplevel.wl_surface(), serial);
        }
    }

    /// A pong arrived: finish every ping that has no client left to wait for.
    fn finish_pings(&mut self, pong_from: Option<&ShellClient>) {
        let mut done = Vec::new();
        for (i, p) in self.pings.iter_mut().enumerate() {
            if let Some(c) = pong_from {
                p.waiting.retain(|w| **w != *c);
            }
            if p.waiting.is_empty() {
                done.push(i);
            }
        }
        for i in done.into_iter().rev() {
            let p = self.pings.remove(i);
            let ms = p.started.elapsed().as_secs_f64() * 1000.0;
            write_reply(self, &p.reply, &json!({"ok": true, "pong_ms": ms}));
        }
    }

    /// Fail the pings that waited longer than the timeout.
    pub fn expire_pings(&mut self) {
        let mut i = 0;
        while i < self.pings.len() {
            if self.pings[i].started.elapsed() >= PING_TIMEOUT {
                let p = self.pings.remove(i);
                write_reply(self, &p.reply, &err("no pong in 5000 ms"));
            } else {
                i += 1;
            }
        }
    }
}

impl XdgShellHandler for State {
    fn xdg_shell_state(&mut self) -> &mut XdgShellState {
        &mut self.xdg_shell_state
    }

    fn new_client(&mut self, client: ShellClient) {
        self.shell_clients.retain(|c| c.alive());
        self.shell_clients.push(Rc::new(client));
    }

    fn client_pong(&mut self, client: ShellClient) {
        self.finish_pings(Some(&client));
    }

    fn new_toplevel(&mut self, surface: ToplevelSurface) {
        surface.with_pending_state(|s| {
            s.states.set(TState::Activated);
        });
        self.toplevels.retain(|t| t.alive());
        self.toplevels.push(surface);
    }

    fn new_popup(&mut self, surface: PopupSurface, positioner: PositionerState) {
        surface.with_pending_state(|s| s.geometry = positioner.get_geometry());
        self.popups.retain(|p| p.alive());
        self.popups.push(surface);
    }

    fn grab(&mut self, surface: PopupSurface, _seat: WlSeat, serial: Serial) {
        self.grabs.retain(|(p, _)| p.alive());
        let kind = ["button", "key", "touch"]
            .into_iter()
            .zip(self.serials)
            .find(|(_, s)| *s == Some(u32::from(serial)))
            .map(|(k, _)| k);
        // a serial that no press, key or touch down sent is stale: the popup is dismissed at once
        let Some(kind) = kind else {
            surface.send_popup_done();
            return;
        };
        // the parent of a grabbing popup is a toplevel or the topmost grabbing popup
        let parent = surface.get_parent_surface();
        let under_popup = self
            .popups
            .iter()
            .any(|p| Some(p.wl_surface()) == parent.as_ref());
        let on_top = self.grabs.last().map(|(p, _)| p.wl_surface()) == parent.as_ref();
        if under_popup && !on_top {
            surface.xdg_popup().post_error(
                xdg_popup::Error::InvalidGrab,
                "the parent of a grabbing popup is not the topmost grabbing popup",
            );
            return;
        }
        self.grabs.push((surface, kind));
    }

    fn popup_destroyed(&mut self, surface: PopupSurface) {
        let Some(i) = self
            .grabs
            .iter()
            .position(|(p, _)| p.wl_surface() == surface.wl_surface())
        else {
            return;
        };
        if i + 1 != self.grabs.len() {
            for id in crate::state::objects_of(&self.display, surface.wl_surface(), "xdg_wm_base") {
                if let Ok(base) = XdgWmBase::from_id(&self.display, id) {
                    base.post_error(
                        xdg_wm_base::Error::NotTheTopmostPopup,
                        "a popup under a grabbing popup was destroyed first",
                    );
                }
            }
        }
        self.grabs.remove(i);
    }

    fn reposition_request(
        &mut self,
        surface: PopupSurface,
        positioner: PositionerState,
        token: u32,
    ) {
        surface.with_pending_state(|s| s.geometry = positioner.get_geometry());
        surface.send_repositioned(token);
    }

    fn move_request(&mut self, surface: ToplevelSurface, _: WlSeat, serial: Serial) {
        self.log_toplevel(&surface, "move", Some(serial), json!({}));
    }

    fn resize_request(
        &mut self,
        surface: ToplevelSurface,
        _: WlSeat,
        serial: Serial,
        edges: ResizeEdge,
    ) {
        self.log_toplevel(
            &surface,
            "resize",
            Some(serial),
            json!({"edges": snake(&format!("{edges:?}"))}),
        );
    }

    fn show_window_menu(
        &mut self,
        surface: ToplevelSurface,
        _: WlSeat,
        serial: Serial,
        location: Point<i32, Logical>,
    ) {
        self.log_toplevel(
            &surface,
            "show_window_menu",
            Some(serial),
            json!({"location": [location.x, location.y]}),
        );
    }

    // the defaults send a configure for these two; keep that
    fn maximize_request(&mut self, surface: ToplevelSurface) {
        self.log_toplevel(&surface, "set_maximized", None, json!({}));
        let serial = surface.send_configure();
        self.note_sent(surface.wl_surface(), serial);
    }

    fn unmaximize_request(&mut self, surface: ToplevelSurface) {
        self.log_toplevel(&surface, "unset_maximized", None, json!({}));
    }

    fn fullscreen_request(&mut self, surface: ToplevelSurface, _: Option<WlOutput>) {
        self.log_toplevel(&surface, "set_fullscreen", None, json!({}));
        let serial = surface.send_configure();
        self.note_sent(surface.wl_surface(), serial);
    }

    fn unfullscreen_request(&mut self, surface: ToplevelSurface) {
        self.log_toplevel(&surface, "unset_fullscreen", None, json!({}));
    }

    fn minimize_request(&mut self, surface: ToplevelSurface) {
        self.log_toplevel(&surface, "set_minimized", None, json!({}));
    }

    fn ack_configure(&mut self, surface: WlSurface, configure: Configure) {
        let serial = match configure {
            Configure::Toplevel(c) => c.serial,
            Configure::Popup(c) => c.serial,
        };
        self.acked.insert(surface.id(), u32::from(serial));
    }
}

impl XdgDecorationHandler for State {
    fn new_decoration(&mut self, toplevel: ToplevelSurface) {
        self.send_decoration(&toplevel);
    }
    fn request_mode(&mut self, toplevel: ToplevelSurface, _mode: Mode) {
        self.send_decoration(&toplevel);
    }
    fn unset_mode(&mut self, toplevel: ToplevelSurface) {
        self.send_decoration(&toplevel);
    }
}

delegate_xdg_shell!(State);
delegate_xdg_decoration!(State);

/// `TopLeft` to `top_left`.
fn snake(s: &str) -> String {
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if c.is_uppercase() && i > 0 {
            out.push('_');
        }
        out.extend(c.to_lowercase());
    }
    out
}

/// `{"cmd":"configure","surface":ID|"focused","size":[W,H],"states":[...]}`
/// The toplevel a command's `"surface"` names: a surface id, or `"focused"` (the newest).
fn find_toplevel(state: &mut State, req: &Value) -> Result<ToplevelSurface, Value> {
    state.toplevels.retain(|t| t.alive());
    let toplevel = match req.get("surface") {
        Some(Value::Number(n)) => {
            let id = n.as_u64().unwrap_or(0) as u32;
            state
                .toplevels
                .iter()
                .find(|t| t.wl_surface().id().protocol_id() == id)
        }
        Some(Value::String(s)) if s == "focused" => state.toplevels.last(),
        _ => return Err(err("\"surface\" must be an id or \"focused\"")),
    };
    toplevel.cloned().ok_or_else(|| err("no such toplevel"))
}

/// `close`: send `xdg_toplevel.close`, as a compositor does when the user closes the window.
pub fn close(state: &mut State, req: &Value) -> Value {
    match find_toplevel(state, req) {
        Ok(toplevel) => {
            toplevel.send_close();
            json!({"ok": true})
        }
        Err(e) => e,
    }
}

pub fn configure(state: &mut State, req: &Value) -> Value {
    let toplevel = match find_toplevel(state, req) {
        Ok(t) => t,
        Err(e) => return e,
    };
    let size = match req.get("size") {
        None => None,
        Some(v) => match (
            v.get(0).and_then(Value::as_i64),
            v.get(1).and_then(Value::as_i64),
        ) {
            (Some(w), Some(h)) => Some((w as i32, h as i32)),
            _ => return err("\"size\" must be [W,H]"),
        },
    };
    let mut want = Vec::new();
    for v in req
        .get("states")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default()
    {
        let Some(name) = v.as_str() else {
            return err("\"states\" must be strings");
        };
        match STATES.iter().find(|(n, _)| *n == name) {
            Some((_, s)) => want.push(*s),
            None => return err(format!("unknown state {name:?}")),
        }
    }
    toplevel.with_pending_state(|s| {
        s.size = size.map(Into::into);
        for (_, st) in STATES {
            s.states.unset(st);
        }
        for st in &want {
            s.states.set(*st);
        }
    });
    let serial = toplevel.send_configure();
    state.note_sent(toplevel.wl_surface(), serial);
    json!({"ok": true, "serial": u32::from(serial)})
}

/// `{"cmd":"decoration","mode":"server|client"}`: the mode sent from now on.
pub fn decoration(state: &mut State, req: &Value) -> Value {
    match req.get("mode").and_then(Value::as_str) {
        Some("server") => state.decoration_mode = Mode::ServerSide,
        Some("client") => state.decoration_mode = Mode::ClientSide,
        _ => return err("\"mode\" must be \"server\" or \"client\""),
    }
    json!({"ok": true})
}

/// `{"cmd":"ping"}`: pings every xdg_wm_base client; the reply goes out when all ponged (or the timeout passes).
pub fn ping(state: &mut State, reply_to: &UnixStream) -> Option<Value> {
    state.shell_clients.retain(|c| c.alive());
    let serial = SERIAL_COUNTER.next_serial();
    let waiting: Vec<Rc<ShellClient>> = state
        .shell_clients
        .iter()
        .filter(|c| c.send_ping(serial).is_ok())
        .cloned()
        .collect();
    if waiting.is_empty() {
        return Some(err("no xdg_wm_base client to ping"));
    }
    match reply_to.try_clone() {
        Ok(reply) => {
            state.pings.push(PendingPing {
                reply,
                started: Instant::now(),
                waiting,
            });
            None
        }
        Err(e) => Some(err(e.to_string())),
    }
}

/// `popup_done`: `{"cmd":"popup_done","surface":"newest"|ID}` sends `xdg_popup.popup_done`, as a compositor does when
/// it dismisses a popup.
pub fn popup_done(state: &mut State, req: &Value) -> Value {
    state.popups.retain(|p| p.alive());
    let popup = match req.get("surface") {
        Some(Value::String(s)) if s == "newest" => state.popups.last(),
        Some(Value::Number(n)) => {
            let id = n.as_u64().unwrap_or(0) as u32;
            state
                .popups
                .iter()
                .find(|p| p.wl_surface().id().protocol_id() == id)
        }
        _ => return err("\"surface\" must be an id or \"newest\""),
    };
    match popup {
        Some(p) => {
            p.send_popup_done();
            json!({"ok": true})
        }
        None => err("no such popup"),
    }
}

/// `click_outside`: the user clicks away from the grabbing popups; the compositor dismisses them, topmost first.
pub fn click_outside(state: &mut State) -> Value {
    state.grabs.retain(|(p, _)| p.alive());
    if state.grabs.is_empty() {
        return err("no popup holds a grab");
    }
    for (p, _) in state.grabs.iter().rev() {
        p.send_popup_done();
    }
    json!({"ok": true, "dismissed": state.grabs.len()})
}

/// `tree`: every live surface with its role, parent, place and stack.  `position` is the subsurface offset in its
/// parent (a popup's, the one its last acked commit applied), `sync` whether a subsurface is synchronised, `above` the later siblings in stacking order (Smithay does not say where a
/// parent sits among its children, so `children` lists a parent's subsurfaces bottom to top), `input_region` the
/// added rectangles (null when the client set none).
pub fn tree(state: &mut State) -> Value {
    state.surfaces.retain(|s| s.is_alive());
    state.popups.retain(|p| p.alive());
    let nodes: Vec<Value> = state
        .surfaces
        .iter()
        .map(|surface| {
            let id = surface.id().protocol_id();
            let role = role_name(surface);
            let parent = compositor::get_parent(surface);
            let children: Vec<u32> = compositor::get_children(surface)
                .iter()
                .map(|c| c.id().protocol_id())
                .collect();
            let above: Vec<u32> = match &parent {
                Some(p) => {
                    let siblings: Vec<u32> = compositor::get_children(p)
                        .iter()
                        .map(|c| c.id().protocol_id())
                        .collect();
                    siblings
                        .iter()
                        .position(|&s| s == id)
                        .map(|i| siblings[i + 1..].to_vec())
                        .unwrap_or_default()
                }
                None => Vec::new(),
            };
            let mut position = [0, 0];
            let (mapped, buffer, input_region, geometry) =
                compositor::with_states(surface, |states| {
                    if role == "subsurface" {
                        let l = states
                            .cached_state
                            .get::<SubsurfaceCachedState>()
                            .current()
                            .location;
                        position = [l.x, l.y];
                    }
                    let (mapped, buffer) =
                        match states.data_map.get::<RendererSurfaceStateUserData>() {
                            Some(r) => match r.lock().unwrap().buffer() {
                                Some(b) => (
                                    true,
                                    with_buffer_contents(
                                        b,
                                        |_, _, d| json!({"w": d.width, "h": d.height}),
                                    )
                                    .ok(),
                                ),
                                None => (false, None),
                            },
                            None => (false, None),
                        };
                    let input_region = states
                        .cached_state
                        .get::<SurfaceAttributes>()
                        .current()
                        .input_region
                        .as_ref()
                        .map(|r| {
                            r.rects
                                .iter()
                                .filter(|(k, _)| matches!(k, RectangleKind::Add))
                                .map(|(_, r)| [r.loc.x, r.loc.y, r.size.w, r.size.h])
                                .collect::<Vec<_>>()
                        });
                    let geometry = states
                        .cached_state
                        .get::<XdgCachedState>()
                        .current()
                        .geometry
                        .map(|g| [g.loc.x, g.loc.y, g.size.w, g.size.h]);
                    (mapped, buffer, input_region, geometry)
                });
            // a popup's position is the one the client acked and committed, as in a compositor: not the one the
            // latest configure carries
            if role == "popup" {
                compositor::with_states(surface, |states| {
                    if let Some(d) = states.data_map.get::<XdgPopupSurfaceData>() {
                        let l = d.lock().unwrap().current.geometry.loc;
                        position = [l.x, l.y];
                    }
                });
            }
            let grab = state
                .grabs
                .iter()
                .find(|(p, _)| p.wl_surface() == surface)
                .map(|(_, kind)| json!({"serial_kind": kind}));
            json!({
                "id": id,
                "role": role,
                "grab": grab,
                "parent": parent.map(|p| p.id().protocol_id()),
                "position": position,
                "sync": role == "subsurface" && compositor::is_sync_subsurface(surface),
                "above": above,
                "children": children,
                "mapped": mapped,
                "input_region": input_region,
                "geometry": geometry,
                "buffer": buffer,
            })
        })
        .collect();
    json!({"ok": true, "surfaces": nodes})
}

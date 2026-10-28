//! W2's smaller protocols: the system bell, idle inhibit, xdg-foreign export and xdg session management, each with
//! the log a test reads back (`bell_log`, `idle_inhibitors`, `exports`, `session_log`).
//!
//! Sessions persist in a JSON file (`--session-store`), so a second recorder instance restores what the first stored.
//! Smithay 0.7 has no session management; its protocol is generated here from the XML next to the source.
use crate::state::State;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use smithay::delegate_idle_inhibit;
use smithay::delegate_xdg_system_bell;
use smithay::reexports::wayland_protocols::xdg::foreign::zv2::server::zxdg_exported_v2::ZxdgExportedV2;
use smithay::reexports::wayland_protocols::xdg::foreign::zv2::server::zxdg_exporter_v2::{
    self, ZxdgExporterV2,
};
use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::XdgToplevel;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::reexports::wayland_server::{
    Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource, WEnum,
};
use smithay::wayland::compositor;
use smithay::wayland::idle_inhibit::IdleInhibitHandler;
use smithay::wayland::shell::xdg::ToplevelSurface;
use smithay::wayland::xdg_system_bell::XdgSystemBellHandler;
use std::collections::BTreeMap;
use std::path::PathBuf;

#[allow(
    clippy::all,
    dead_code,
    unused_imports,
    unused_unsafe,
    unused_variables,
    non_camel_case_types,
    non_upper_case_globals,
    non_snake_case,
    missing_docs
)]
pub mod proto {
    use smithay::reexports::wayland_protocols::xdg::shell::server::*;
    use smithay::reexports::wayland_server;
    use smithay::reexports::wayland_server::protocol::*;

    pub mod __interfaces {
        use smithay::reexports::wayland_protocols::xdg::shell::server::__interfaces::*;
        use smithay::reexports::wayland_server::protocol::__interfaces::*;
        wayland_scanner::generate_interfaces!("./protocols/xdg-session-management-v1.xml");
    }
    use self::__interfaces::*;

    wayland_scanner::generate_server_code!("./protocols/xdg-session-management-v1.xml");
}
use proto::xdg_session_manager_v1::{self, Reason, XdgSessionManagerV1};
use proto::xdg_session_v1::{self, XdgSessionV1};
use proto::xdg_toplevel_session_v1::{self, XdgToplevelSessionV1};

/// What the sessions remember between recorder instances: per session id, per toplevel name, the last size.
#[derive(Default, Serialize, Deserialize)]
struct Store {
    sessions: BTreeMap<String, BTreeMap<String, Option<[i32; 2]>>>,
}

/// One toplevel a live session manages.
struct Named {
    name: String,
    toplevel: XdgToplevel,
    surface: WlSurface,
    session: XdgToplevelSessionV1,
}

/// A session object that exists now: the session is "in use".
struct Live {
    id: String,
    res: XdgSessionV1,
    names: Vec<Named>,
}

struct IdleRec {
    surface: WlSurface,
    destroyed: bool,
}

#[derive(Default)]
pub struct Extras {
    bells: Vec<Value>,
    idle: Vec<IdleRec>,
    exports: Vec<Value>,
    store: Store,
    store_path: Option<PathBuf>,
    live: Vec<Live>,
    /// Restores waiting for their toplevel's first configure: the toplevel's session object and the stored size.
    restores: Vec<(WlSurface, XdgToplevelSessionV1, Option<[i32; 2]>)>,
    session_log: Vec<Value>,
}

impl Extras {
    /// Use `path` as the session store: what it holds is loaded, and every change is written back.
    pub fn use_store(&mut self, path: PathBuf) -> Result<(), String> {
        if path.exists() {
            let fail = |e: String| format!("{}: {e}", path.display());
            let text = std::fs::read_to_string(&path).map_err(|e| fail(e.to_string()))?;
            self.store = serde_json::from_str(&text).map_err(|e| fail(e.to_string()))?;
        }
        self.store_path = Some(path);
        Ok(())
    }

    /// What a restart keeps: the store and where it is written; the live sessions and pending restores go.
    pub fn restarted(self) -> Extras {
        Extras {
            store: self.store,
            store_path: self.store_path,
            ..Default::default()
        }
    }

    fn save(&self) {
        if let Some(path) = &self.store_path {
            if let Ok(text) = serde_json::to_string(&self.store) {
                let _ = std::fs::write(path, text);
            }
        }
    }

    fn new_session_id(&self) -> String {
        let mut n = self.store.sessions.len() + 1;
        while self
            .store
            .sessions
            .contains_key(&format!("rec-session-{n}"))
        {
            n += 1;
        }
        format!("rec-session-{n}")
    }
}

/// The ids the logs use: protocol ids, per client, as in the other logs.
fn sid(surface: &WlSurface) -> u32 {
    surface.id().protocol_id()
}

/// The replies of the four query commands.
pub fn command(state: &State, cmd: &str) -> Option<Value> {
    let x = &state.extras;
    Some(match cmd {
        "bell_log" => json!({"ok": true, "bells": x.bells}),
        "idle_inhibitors" => {
            let list: Vec<Value> = x
                .idle
                .iter()
                .map(|r| {
                    let alive = !r.destroyed && r.surface.is_alive();
                    json!({"surface": sid(&r.surface), "alive": alive})
                })
                .collect();
            json!({"ok": true, "inhibitors": list})
        }
        "exports" => json!({"ok": true, "exports": x.exports}),
        "session_log" => json!({"ok": true, "session_log": x.session_log}),
        _ => return None,
    })
}

// --- the bell and idle inhibit -------------------------------------------------------------------------------

impl XdgSystemBellHandler for State {
    fn ring(&mut self, surface: Option<WlSurface>) {
        let surface = surface.as_ref().map(sid);
        self.extras.bells.push(json!({"surface": surface}));
    }
}
delegate_xdg_system_bell!(State);

impl IdleInhibitHandler for State {
    fn inhibit(&mut self, surface: WlSurface) {
        self.extras.idle.push(IdleRec {
            surface,
            destroyed: false,
        });
    }

    fn uninhibit(&mut self, surface: WlSurface) {
        if let Some(r) = self
            .extras
            .idle
            .iter_mut()
            .find(|r| !r.destroyed && r.surface == surface)
        {
            r.destroyed = true;
        }
    }
}
delegate_idle_inhibit!(State);

// --- xdg-foreign: the exporter -------------------------------------------------------------------------------

impl GlobalDispatch<ZxdgExporterV2, ()> for State {
    fn bind(
        _: &mut Self,
        _: &DisplayHandle,
        _: &Client,
        resource: New<ZxdgExporterV2>,
        _: &(),
        init: &mut DataInit<'_, Self>,
    ) {
        init.init(resource, ());
    }
}

impl Dispatch<ZxdgExporterV2, ()> for State {
    fn request(
        state: &mut Self,
        _: &Client,
        resource: &ZxdgExporterV2,
        request: zxdg_exporter_v2::Request,
        _: &(),
        _: &DisplayHandle,
        init: &mut DataInit<'_, Self>,
    ) {
        if let zxdg_exporter_v2::Request::ExportToplevel { id, surface } = request {
            let exported = init.init(id, ());
            if compositor::get_role(&surface) != Some("xdg_toplevel") {
                resource.post_error(
                    zxdg_exporter_v2::Error::InvalidSurface,
                    "the surface is not an xdg_toplevel",
                );
                return;
            }
            let handle = format!("rec-{}", state.extras.exports.len() + 1);
            exported.handle(handle.clone());
            state
                .extras
                .exports
                .push(json!({"surface": sid(&surface), "handle": handle}));
        }
    }
}

impl Dispatch<ZxdgExportedV2, ()> for State {
    fn request(
        _: &mut Self,
        _: &Client,
        _: &ZxdgExportedV2,
        _: <ZxdgExportedV2 as Resource>::Request,
        _: &(),
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
    }
}

// --- xdg session management ----------------------------------------------------------------------------------

impl State {
    fn log_session(&mut self, op: &str, id: Option<&str>, more: Value) {
        let mut entry =
            json!({"op": op, "id": id, "reason": null, "name": null, "before_first_commit": null});
        if let (Some(e), Some(m)) = (entry.as_object_mut(), more.as_object()) {
            e.extend(m.clone());
        }
        self.extras.session_log.push(entry);
    }

    /// A toplevel's first configure: a restored one hears `restored` first and gets the stored size.
    pub fn session_before_configure(&mut self, toplevel: &ToplevelSurface) {
        let Some(i) = self
            .extras
            .restores
            .iter()
            .position(|(s, _, _)| s == toplevel.wl_surface())
        else {
            return;
        };
        let (_, session, size) = self.extras.restores.remove(i);
        session.restored();
        if let Some([w, h]) = size {
            toplevel.with_pending_state(|s| s.size = Some((w, h).into()));
        }
    }

    /// A commit of a toplevel with a buffer: its size is what a later instance restores.
    pub fn session_commit(&mut self, surface: &WlSurface, size: Option<[i32; 2]>) {
        let Some(size) = size else {
            return;
        };
        let mut changed = false;
        for l in &self.extras.live {
            for n in l.names.iter().filter(|n| &n.surface == surface) {
                if let Some(entry) = self
                    .extras
                    .store
                    .sessions
                    .get_mut(&l.id)
                    .and_then(|s| s.get_mut(&n.name))
                {
                    if *entry != Some(size) {
                        *entry = Some(size);
                        changed = true;
                    }
                }
            }
        }
        if changed {
            self.extras.save();
        }
    }
}

impl GlobalDispatch<XdgSessionManagerV1, ()> for State {
    fn bind(
        _: &mut Self,
        _: &DisplayHandle,
        _: &Client,
        resource: New<XdgSessionManagerV1>,
        _: &(),
        init: &mut DataInit<'_, Self>,
    ) {
        init.init(resource, ());
    }
}

impl Dispatch<XdgSessionManagerV1, ()> for State {
    fn request(
        state: &mut Self,
        client: &Client,
        resource: &XdgSessionManagerV1,
        request: xdg_session_manager_v1::Request,
        _: &(),
        _: &DisplayHandle,
        init: &mut DataInit<'_, Self>,
    ) {
        let xdg_session_manager_v1::Request::GetSession {
            id,
            reason,
            session_id,
        } = request
        else {
            return;
        };
        let name = match reason {
            WEnum::Value(Reason::Launch) => "launch",
            WEnum::Value(Reason::Recover) => "recover",
            WEnum::Value(Reason::SessionRestore) => "session_restore",
            _ => {
                resource.post_error(
                    xdg_session_manager_v1::Error::InvalidReason,
                    "invalid reason",
                );
                return;
            }
        };
        state.log_session(
            "get_session",
            session_id.as_deref(),
            json!({"reason": name}),
        );
        let known = session_id.filter(|s| state.extras.store.sessions.contains_key(s));
        if let Some(known) = &known {
            // the same client using it twice is an error; another client's session object is replaced
            let held = state
                .extras
                .live
                .iter()
                .position(|l| &l.id == known && l.res.is_alive());
            if let Some(i) = held {
                if state.extras.live[i].res.client().map(|c| c.id()) == Some(client.id()) {
                    resource.post_error(
                        xdg_session_manager_v1::Error::InUse,
                        "the session is in use",
                    );
                    return;
                }
                let old = state.extras.live.remove(i);
                old.res.replaced();
            }
        }
        let session_id = known
            .clone()
            .unwrap_or_else(|| state.extras.new_session_id());
        let res = init.init(id, session_id.clone());
        if known.is_some() {
            res.restored();
        } else {
            state
                .extras
                .store
                .sessions
                .insert(session_id.clone(), BTreeMap::new());
            state.extras.save();
            res.created(session_id.clone());
        }
        state.extras.live.push(Live {
            id: session_id,
            res,
            names: Vec::new(),
        });
    }
}

impl Dispatch<XdgSessionV1, String> for State {
    fn request(
        state: &mut Self,
        _: &Client,
        resource: &XdgSessionV1,
        request: xdg_session_v1::Request,
        id: &String,
        _: &DisplayHandle,
        init: &mut DataInit<'_, Self>,
    ) {
        use xdg_session_v1::Request as R;
        match request {
            R::Remove => {
                state.extras.store.sessions.remove(id);
                state.extras.save();
                state.log_session("remove", Some(id), json!({}));
            }
            R::RemoveToplevel { name } => {
                if let Some(s) = state.extras.store.sessions.get_mut(id) {
                    s.remove(&name);
                }
                if let Some(l) = state.extras.live.iter_mut().find(|l| l.res == *resource) {
                    l.names.retain(|n| n.name != name);
                }
                state.extras.save();
                state.log_session("remove", Some(id), json!({"name": name}));
            }
            R::AddToplevel {
                id: ts,
                toplevel,
                name,
            } => session_toplevel(state, resource, id, init, ts, toplevel, name, false),
            R::RestoreToplevel {
                id: ts,
                toplevel,
                name,
            } => session_toplevel(state, resource, id, init, ts, toplevel, name, true),
            _ => {}
        }
    }

    fn destroyed(
        state: &mut Self,
        _: smithay::reexports::wayland_server::backend::ClientId,
        resource: &XdgSessionV1,
        _: &String,
    ) {
        state.extras.live.retain(|l| l.res != *resource);
    }
}

#[allow(clippy::too_many_arguments)]
fn session_toplevel(
    state: &mut State,
    resource: &XdgSessionV1,
    id: &str,
    init: &mut DataInit<'_, State>,
    ts: New<XdgToplevelSessionV1>,
    toplevel: XdgToplevel,
    name: String,
    restore: bool,
) {
    let ts = init.init(ts, ());
    let Some(top) = state
        .toplevels
        .iter()
        .find(|t| t.xdg_toplevel() == &toplevel)
        .cloned()
    else {
        return;
    };
    let before = !top.is_initial_configure_sent();
    let op = if restore { "restore" } else { "add" };
    state.log_session(
        op,
        Some(id),
        json!({"name": name, "before_first_commit": before}),
    );
    if restore && !before {
        resource.post_error(
            xdg_session_v1::Error::AlreadyMapped,
            "the toplevel has already committed",
        );
        return;
    }
    let live = &state.extras.live;
    let added = live
        .iter()
        .any(|l| l.names.iter().any(|n| n.toplevel == toplevel));
    let held = live.iter().any(|l| {
        l.id == id
            && l.names
                .iter()
                .any(|n| n.name == name && n.toplevel.is_alive())
    });
    let stored = state
        .extras
        .store
        .sessions
        .get(id)
        .and_then(|s| s.get(&name))
        .copied();
    if added {
        resource.post_error(
            xdg_session_v1::Error::AlreadyAdded,
            "the toplevel is already in a session",
        );
        return;
    }
    if held || (!restore && stored.is_some()) {
        resource.post_error(xdg_session_v1::Error::NameInUse, "the name is in use");
        return;
    }
    if restore {
        if let Some(size) = stored {
            state
                .extras
                .restores
                .push((top.wl_surface().clone(), ts.clone(), size));
        }
    }
    let surface = top.wl_surface().clone();
    if let Some(l) = state.extras.live.iter_mut().find(|l| l.res == *resource) {
        l.names.push(Named {
            name: name.clone(),
            toplevel,
            surface,
            session: ts,
        });
    }
    if let Some(s) = state.extras.store.sessions.get_mut(id) {
        s.entry(name).or_insert(None);
    }
    state.extras.save();
}

impl Dispatch<XdgToplevelSessionV1, ()> for State {
    fn request(
        state: &mut Self,
        _: &Client,
        resource: &XdgToplevelSessionV1,
        request: xdg_toplevel_session_v1::Request,
        _: &(),
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
        if let xdg_toplevel_session_v1::Request::Rename { name } = request {
            for l in &mut state.extras.live {
                let Some(n) = l.names.iter_mut().find(|n| n.session == *resource) else {
                    continue;
                };
                if let Some(s) = state.extras.store.sessions.get_mut(&l.id) {
                    let size = s.remove(&n.name).flatten();
                    s.insert(name.clone(), size);
                }
                n.name = name;
                break;
            }
            state.extras.save();
        }
    }
}

//! Session management (`xdg_session_manager_v1`): one session per connection, and a name per top-level window, so a
//! compositor that keeps sessions puts windows back where they were.
//!
//! Neither wayland-protocols nor Smithay carries this protocol, so its client bindings are generated here from the
//! XML next to the crate.  `add_toplevel` and `restore_toplevel` go out before the window's role-setup commit (the
//! protocol's `already_mapped` rule).  Once the session is `replaced`, or when there is no session, names are ignored.

use crate::api::*;
use crate::event::*;
use crate::wayland::{MainState, WaylandShell};
use std::collections::HashMap;
use wayland_client::{Connection, Dispatch, QueueHandle, delegate_noop};
use wayland_protocols::xdg::shell::client::xdg_toplevel::XdgToplevel;

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
mod proto {
    use wayland_client;
    use wayland_client::protocol::*;
    use wayland_protocols::xdg::shell::client::*;

    pub mod __interfaces {
        use wayland_client::protocol::__interfaces::*;
        use wayland_protocols::xdg::shell::client::__interfaces::*;
        wayland_scanner::generate_interfaces!("./protocols/xdg-session-management-v1.xml");
    }
    use self::__interfaces::*;

    wayland_scanner::generate_client_code!("./protocols/xdg-session-management-v1.xml");
}
use proto::xdg_session_manager_v1::{Reason, XdgSessionManagerV1};
use proto::xdg_session_v1::{self, XdgSessionV1};
use proto::xdg_toplevel_session_v1::XdgToplevelSessionV1;

delegate_noop!(MainState: XdgSessionManagerV1);
delegate_noop!(MainState: ignore XdgToplevelSessionV1);

/// The session, and the windows that hold a name in it.
#[derive(Default)]
pub struct Session {
    pub manager: Option<XdgSessionManagerV1>,
    session: Option<XdgSessionV1>,
    id: Option<String>,
    /// Another client took the session over: names are ignored from now on.
    replaced: bool,
    /// Window -> (its name, its session object).
    named: HashMap<u64, (String, XdgToplevelSessionV1)>,
}

impl Session {
    /// Window `id` is going: its session object goes, its entry in the session stays (the host removes it by name).
    pub fn forget(&mut self, id: u64) {
        if let Some((_, ts)) = self.named.remove(&id) {
            ts.destroy();
        }
    }

    /// `name` as a window will use it: `None` for no name, an empty one, no session or a replaced one; `State` when
    /// another live window holds it (the compositor would end the connection).
    pub fn name_for(&self, name: Option<&str>) -> ShellResult<Option<String>> {
        let name = name.filter(|n| !n.is_empty());
        if self.session.is_none() || self.replaced {
            return Ok(None);
        }
        if let Some(n) = name
            && self.named.values().any(|(held, _)| held == n)
        {
            return Err(ShellError::State);
        }
        Ok(name.map(str::to_owned))
    }

    /// A reconnect: a session that was open, and not taken over, is opened again on the new compositor with reason
    /// `recover` and its id.  The names its windows held come back, for each window to be restored under its name
    /// before its role-setup commit.  Without the global, or after a takeover, the session is gone.
    pub fn reconnected(&mut self, qh: &QueueHandle<MainState>) -> HashMap<u64, String> {
        let names: HashMap<u64, String> = self.named.drain().map(|(w, (n, _))| (w, n)).collect();
        let open = self.session.take().is_some() && !std::mem::take(&mut self.replaced);
        let Some(manager) = self.manager.as_ref().filter(|_| open) else {
            return HashMap::new();
        };
        self.session = Some(manager.get_session(Reason::Recover, self.id.clone(), qh, ()));
        names
    }

    /// Put `toplevel` (window `id`) in the session under `name`, as `restore_toplevel` or `add_toplevel`.
    pub fn attach(
        &mut self,
        toplevel: &XdgToplevel,
        id: u64,
        name: &str,
        restore: bool,
        qh: &QueueHandle<MainState>,
    ) {
        let Some(session) = &self.session else {
            return;
        };
        let ts = if restore {
            session.restore_toplevel(toplevel, name.to_owned(), qh, ())
        } else {
            session.add_toplevel(toplevel, name.to_owned(), qh, ())
        };
        self.named.insert(id, (name.to_owned(), ts));
    }
}

impl WaylandShell {
    /// Open the session: `id` None for a new one, with `reason` `NHSHELL_SESSION_*`.  The answer comes as a
    /// `NHSHELL_EV_SESSION` record; an open session is dropped first (its saved windows stay with the compositor).
    pub(super) fn session_open_impl(&mut self, id: Option<&str>, reason: u32) -> ShellResult {
        let reason = match reason {
            NHSHELL_SESSION_LAUNCH => Reason::Launch,
            NHSHELL_SESSION_RECOVER => Reason::Recover,
            NHSHELL_SESSION_RESTORE => Reason::SessionRestore,
            _ => return Err(ShellError::Arg),
        };
        let id = id.filter(|s| !s.is_empty());
        let qh = self.main_queue.handle();
        let s = &mut self.main.session;
        let manager = s.manager.as_ref().ok_or(ShellError::Unsupported)?;
        // the old one first: the same id twice on one connection is an error
        for (_, (_, ts)) in s.named.drain() {
            ts.destroy();
        }
        if let Some(old) = s.session.take() {
            old.destroy();
        }
        s.session = Some(manager.get_session(reason, id.map(str::to_owned), &qh, ()));
        s.id = id.map(str::to_owned);
        s.replaced = false;
        self.flush()
    }

    /// The session's id (`State` until the session exists).
    pub(super) fn session_id_impl(&self) -> ShellResult<String> {
        self.main.session.id.clone().ok_or(ShellError::State)
    }

    /// Window `id` leaves the session: the compositor forgets its name.
    pub(super) fn session_remove_impl(&mut self, id: WindowId) -> ShellResult {
        let s = &mut self.main.session;
        let (name, ts) = s.named.remove(&id).ok_or(ShellError::State)?;
        if let (Some(session), false) = (&s.session, s.replaced) {
            session.remove_toplevel(name);
        }
        ts.destroy();
        self.flush()
    }
}

impl Dispatch<XdgSessionV1, ()> for MainState {
    fn event(
        state: &mut Self,
        _: &XdgSessionV1,
        event: xdg_session_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let session = &mut state.session;
        let which = match event {
            xdg_session_v1::Event::Created { session_id } => {
                session.id = Some(session_id);
                NHSHELL_SESSION_CREATED
            }
            xdg_session_v1::Event::Restored => NHSHELL_SESSION_RESTORED,
            xdg_session_v1::Event::Replaced => {
                session.replaced = true;
                NHSHELL_SESSION_REPLACED
            }
        };
        if let Some(shared) = &state.shared {
            let mut ev = NhShellEvent::new(NHSHELL_EV_SESSION, 0, 0);
            ev.u.session = SessionEv { state: which };
            shared.post(ev);
        }
    }
}

//! Restart in place: every client is dropped and all compositor state is rebuilt from a profile, as when a
//! compositor crashes and its supervisor starts it again.  The control channel and the session store stay.  The
//! listening socket follows the mode: `keep-socket` queues connections in the backlog for a while (as
//! `kwin_wayland_wrapper` does), `rebind` closes and binds the name again, `gone` stays closed until `restart_up`.
use crate::state::{Profile, State};
use calloop::timer::{TimeoutAction, Timer};
use calloop::RegistrationToken;
use serde_json::{json, Value};
use smithay::reexports::wayland_server::backend::{DisconnectReason, GlobalId};
use smithay::wayland::socket::ListeningSocketSource;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

/// What a restart needs to remember, and the connection counters.
#[derive(Default)]
pub struct Restart {
    pub socket: String,
    pub profile_dir: PathBuf,
    pub profile: Option<Profile>,
    pub output: (i32, i32, u32),
    pub refresh_mhz: i32,
    /// The listening socket's source; None while it is closed.
    pub token: Option<RegistrationToken>,
    /// The globals the current state created (a restart removes them before it creates its own).
    pub globals: Vec<GlobalId>,
    pub accepted: u64,
    pub count: u64,
}

impl Restart {
    pub fn configure(
        &mut self,
        socket: &str,
        profile_path: &std::path::Path,
        profile: Profile,
        output: (i32, i32, u32),
        refresh_mhz: i32,
    ) {
        self.socket = socket.to_string();
        self.profile_dir = profile_path.parent().map(PathBuf::from).unwrap_or_default();
        self.profile = Some(profile);
        self.output = output;
        self.refresh_mhz = refresh_mhz;
    }
}

/// Bind the listening socket and serve it from the event loop.
pub fn listen(state: &mut State) -> Result<(), String> {
    let source =
        ListeningSocketSource::with_name(&state.restart.socket).map_err(|e| e.to_string())?;
    let handle = state.handle.clone();
    let token = handle
        .insert_source(source, |stream, _, state| {
            state.restart.accepted += 1;
            let data = state.new_client_data();
            if let Ok(client) = state.display.insert_client(stream, Arc::new(data)) {
                state.note_client_pid(&client);
            }
        })
        .map_err(|e| e.to_string())?;
    state.restart.token = Some(token);
    Ok(())
}

fn after(state: &State, ms: u64, f: impl FnOnce(&mut State) + 'static) {
    let mut f = Some(f);
    let _ = state.handle.insert_source(
        Timer::from_duration(Duration::from_millis(ms)),
        move |_, _, state| {
            if let Some(f) = f.take() {
                f(state);
            }
            TimeoutAction::Drop
        },
    );
}

fn err(msg: impl Into<String>) -> Value {
    crate::control::err(msg)
}

pub fn command(state: &mut State, cmd: &str, req: &Value) -> Value {
    match cmd {
        "connections" => {
            let live = state.clients.lock().unwrap().len();
            json!({"ok": true, "accepted": state.restart.accepted, "live": live})
        }
        "restart_up" => {
            if state.restart.token.is_some() {
                return json!({"ok": true});
            }
            match listen(state) {
                Ok(()) => json!({"ok": true}),
                Err(e) => err(e),
            }
        }
        _ => restart(state, req),
    }
}

fn restart(state: &mut State, req: &Value) -> Value {
    let mode = req
        .get("mode")
        .and_then(Value::as_str)
        .unwrap_or("keep-socket");
    if !matches!(mode, "keep-socket" | "rebind" | "gone") {
        return err(format!("unknown restart mode {mode:?}"));
    }
    let down_ms = req.get("down_ms").and_then(Value::as_u64).unwrap_or(0);
    let profile = match req.get("profile").and_then(Value::as_str) {
        Some(name) => {
            if name.contains('/') {
                return err("profile is a name, not a path");
            }
            match Profile::load(&state.restart.profile_dir.join(format!("{name}.json"))) {
                Ok(p) => p,
                Err(e) => return err(e),
            }
        }
        None => state.restart.profile.clone().expect("configured at start"),
    };

    // every client goes; the objects they held are cleaned up at the next dispatch
    let backend = state.display.backend_handle();
    let mut ids = Vec::new();
    backend.with_all_clients(|id| ids.push(id));
    for id in ids {
        backend.kill_client(id, DisconnectReason::ConnectionClosed);
    }

    match (mode, state.restart.token) {
        (_, None) => {}
        ("keep-socket", Some(token)) if down_ms > 0 => {
            let _ = state.handle.disable(&token);
        }
        ("keep-socket", Some(_)) => {}
        (_, Some(token)) => {
            state.handle.remove(token);
            state.restart.token = None;
        }
    }
    match mode {
        "keep-socket" if down_ms > 0 => after(state, down_ms, |state| {
            if let Some(token) = state.restart.token {
                let _ = state.handle.enable(&token);
            }
        }),
        "rebind" => after(state, down_ms, |state| {
            if state.restart.token.is_none() {
                let _ = listen(state);
            }
        }),
        _ => {}
    }

    state.reset(&profile);
    state.restart.count += 1;
    let k = state.restart.count;
    state.log.mark_restart(k);
    json!({"ok": true, "restart": k})
}

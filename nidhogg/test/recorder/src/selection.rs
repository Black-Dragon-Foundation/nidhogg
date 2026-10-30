//! Selections (clipboard and primary) and xdg-activation.  `offer` makes the recorder stand in for another client that
//! owns a selection; `read` reads what a real client offers.  Neither ever blocks the event loop: reads wait on a
//! non-blocking socket with a 1 s deadline timer, writes happen on a short-lived thread.
use crate::control::{err, write_reply};
use crate::state::State;
use calloop::generic::Generic;
use calloop::timer::{TimeoutAction, Timer};
use calloop::{Interest, Mode, PostAction, RegistrationToken};
use serde_json::{json, Value};
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::reexports::wayland_server::Resource;
use smithay::wayland::selection::data_device::{
    request_data_device_client_selection, set_data_device_selection,
};
use smithay::wayland::selection::primary_selection::{
    request_primary_client_selection, set_primary_selection, PrimarySelectionHandler,
    PrimarySelectionState,
};
use smithay::wayland::selection::SelectionTarget;
use smithay::wayland::xdg_activation::{
    XdgActivationHandler, XdgActivationState, XdgActivationToken, XdgActivationTokenData,
};
use smithay::{delegate_primary_selection, delegate_xdg_activation};
use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::os::fd::OwnedFd;
use std::os::unix::net::UnixStream;
use std::rc::Rc;
use std::time::Duration;

const READ_DEADLINE: Duration = Duration::from_millis(1000);

/// What the recorder offers as another client's selection.
pub struct Offer {
    pub types: BTreeMap<String, String>,
    pub silent: bool,
}

impl PrimarySelectionHandler for State {
    fn primary_selection_state(&self) -> &PrimarySelectionState {
        &self.primary_selection_state
    }
}
delegate_primary_selection!(State);

impl XdgActivationHandler for State {
    fn activation_state(&mut self) -> &mut XdgActivationState {
        &mut self.activation_state
    }

    fn token_created(&mut self, token: XdgActivationToken, _: XdgActivationTokenData) -> bool {
        self.issued_tokens.push(json!({"token": token.as_str()}));
        true
    }

    fn request_activation(
        &mut self,
        token: XdgActivationToken,
        _: XdgActivationTokenData,
        surface: WlSurface,
    ) {
        self.activations.push(json!({
            "token": token.as_str(),
            "surface": surface.id().protocol_id(),
        }));
    }
}
delegate_xdg_activation!(State);

fn slot(kind: SelectionTarget) -> usize {
    match kind {
        SelectionTarget::Clipboard => 0,
        SelectionTarget::Primary => 1,
    }
}

fn kind_of(req: &Value) -> Result<SelectionTarget, Value> {
    match req.get("kind").and_then(Value::as_str) {
        Some("clipboard") => Ok(SelectionTarget::Clipboard),
        Some("primary") => Ok(SelectionTarget::Primary),
        _ => Err(err("\"kind\" must be \"clipboard\" or \"primary\"")),
    }
}

/// A client asked to receive the selection the recorder offers: write the text on a thread (a silent offer keeps the
/// descriptor open and writes nothing, so the reader's wait times out).
pub fn send(state: &mut State, ty: SelectionTarget, mime_type: String, fd: OwnedFd) {
    let Some(offer) = state.offers[slot(ty)].as_ref() else {
        return;
    };
    if offer.silent {
        state.held_fds.push(fd);
        return;
    }
    let Some(text) = offer.types.get(&mime_type).cloned() else {
        return;
    };
    std::thread::spawn(move || {
        let mut file = std::fs::File::from(fd);
        let _ = file.write_all(text.as_bytes());
    });
}

/// `offer`: `{"cmd":"offer","kind":"clipboard"|"primary","types":{"MIME":"TEXT"},"silent":false}`.
pub fn offer(state: &mut State, req: &Value) -> Value {
    let kind = match kind_of(req) {
        Ok(k) => k,
        Err(e) => return e,
    };
    let mut types = BTreeMap::new();
    match req.get("types").and_then(Value::as_object) {
        Some(o) if !o.is_empty() => {
            for (k, v) in o {
                types.insert(k.clone(), v.as_str().unwrap_or_default().to_string());
            }
        }
        _ => return err("\"types\" must be a non-empty object of MIME to text"),
    }
    let silent = req.get("silent").and_then(Value::as_bool).unwrap_or(false);
    let mimes: Vec<String> = types.keys().cloned().collect();
    state.offers[slot(kind)] = Some(Offer { types, silent });
    let dh = state.display.clone();
    let seat = state.seat.clone();
    match kind {
        SelectionTarget::Clipboard => set_data_device_selection::<State>(&dh, &seat, mimes, ()),
        SelectionTarget::Primary => set_primary_selection::<State>(&dh, &seat, mimes, ()),
    }
    json!({"ok": true})
}

struct Job {
    buf: Vec<u8>,
    reply: UnixStream,
    done: bool,
}

fn finish(state: &State, job: &mut Job, ok: bool) {
    job.done = true;
    let reply = if ok {
        json!({"ok": true, "data": String::from_utf8_lossy(&job.buf)})
    } else {
        json!({"ok": false})
    };
    write_reply(state, &job.reply, &reply);
}

/// `read`: `{"cmd":"read","kind":…,"type":"MIME"}` asks the client that owns the selection to write it, and replies
/// `{"ok":true,"data":…}` when it closes its end, or `{"ok":false}` after 1 s.  The reply goes out later (None now).
pub fn read(state: &mut State, req: &Value, reply_to: &UnixStream) -> Option<Value> {
    let kind = match kind_of(req) {
        Ok(k) => k,
        Err(e) => return Some(e),
    };
    let Some(mime) = req.get("type").and_then(Value::as_str) else {
        return Some(err("\"type\" is required"));
    };
    let Ok((ours, theirs)) = UnixStream::pair() else {
        return Some(err("socketpair failed"));
    };
    let Ok(reply) = reply_to.try_clone() else {
        return Some(err("cannot clone the control stream"));
    };
    ours.set_nonblocking(true).ok()?;
    let fd = OwnedFd::from(theirs);
    let asked = match kind {
        SelectionTarget::Clipboard => {
            request_data_device_client_selection(&state.seat, mime.to_string(), fd)
                .map_err(|e| e.to_string())
        }
        SelectionTarget::Primary => {
            request_primary_client_selection(&state.seat, mime.to_string(), fd)
                .map_err(|e| e.to_string())
        }
    };
    if let Err(e) = asked {
        return Some(err(e));
    }
    let job = Rc::new(RefCell::new(Job {
        buf: Vec::new(),
        reply,
        done: false,
    }));
    let token: Rc<Cell<Option<RegistrationToken>>> = Rc::new(Cell::new(None));
    let (j, t) = (job.clone(), token.clone());
    let source = state.handle.insert_source(
        Generic::new(ours, Interest::READ, Mode::Level),
        move |_, stream, state| {
            let mut job = j.borrow_mut();
            if job.done {
                return Ok(PostAction::Remove);
            }
            let mut chunk = [0u8; 4096];
            loop {
                match (&**stream).read(&mut chunk) {
                    Ok(0) => {
                        finish(state, &mut job, true);
                        t.set(None);
                        return Ok(PostAction::Remove);
                    }
                    Ok(n) => job.buf.extend_from_slice(&chunk[..n]),
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        return Ok(PostAction::Continue)
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                    Err(_) => {
                        finish(state, &mut job, false);
                        t.set(None);
                        return Ok(PostAction::Remove);
                    }
                }
            }
        },
    );
    match source {
        Ok(tok) => token.set(Some(tok)),
        Err(e) => return Some(err(e.to_string())),
    }
    let _ = state
        .handle
        .insert_source(Timer::from_duration(READ_DEADLINE), move |_, _, state| {
            let mut job = job.borrow_mut();
            if !job.done {
                finish(state, &mut job, false);
                if let Some(tok) = token.take() {
                    state.handle.remove(tok);
                }
            }
            TimeoutAction::Drop
        });
    None
}

/// `activation_log`: the `xdg_activation_v1.activate` calls seen so far, and the tokens issued.
pub fn activation_log(state: &State) -> Value {
    json!({"ok": true, "activations": state.activations, "issued": state.issued_tokens})
}

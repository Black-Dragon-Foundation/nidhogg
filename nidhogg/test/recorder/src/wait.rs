//! `wait`: a reply that comes once what a row waits on is in the recorder, so a row goes on from what the recorder
//! announces instead of polling it.  A wait that is still unmet at its deadline fails, naming what never came.
//!
//! `{"cmd":"wait","for":"popup","before":[ID...],"timeout_ms":N}`: a popup surface whose id is not in `before`;
//!   the reply is `{"ok":true,"id":ID}`.
//! `{"cmd":"wait","for":"input","req":"xdg_toplevel.move","since":SEQ,"timeout_ms":N}`: an input-log entry with that
//!   `req` and a `seq` of at least `since`; the reply is `{"ok":true,"entry":{…}}`.
//! `timeout_ms` defaults to 10000.  A wait is checked after every dispatch of the event loop and on each frame tick.
use crate::control::{err, write_reply};
use crate::state::State;
use serde_json::{json, Value};
use std::os::unix::net::UnixStream;
use std::time::{Duration, Instant};

pub enum Cond {
    Popup { before: Vec<u64> },
    Input { req: String, since: u64 },
}

impl Cond {
    fn describe(&self) -> String {
        match self {
            Cond::Popup { before } => format!("no popup other than {before:?}"),
            Cond::Input { req, since } => format!("no {req:?} in the input log from seq {since}"),
        }
    }
}

pub struct PendingWait {
    reply: UnixStream,
    cond: Cond,
    deadline: Instant,
    timeout_ms: u64,
}

/// `wait`: the reply now when the condition holds, else None and the reply comes later (`check`).
pub fn wait(state: &mut State, req: &Value, stream: &UnixStream) -> Option<Value> {
    let timeout_ms = req
        .get("timeout_ms")
        .and_then(Value::as_u64)
        .unwrap_or(10_000);
    let cond = match req.get("for").and_then(Value::as_str) {
        Some("popup") => Cond::Popup {
            before: req
                .get("before")
                .and_then(Value::as_array)
                .map(|a| a.iter().filter_map(Value::as_u64).collect())
                .unwrap_or_default(),
        },
        Some("input") => {
            let Some(r) = req.get("req").and_then(Value::as_str) else {
                return Some(err("wait for input needs \"req\""));
            };
            Cond::Input {
                req: r.to_string(),
                since: req.get("since").and_then(Value::as_u64).unwrap_or(0),
            }
        }
        other => {
            return Some(err(format!(
                "wait: unknown \"for\" {other:?} (popup or input)"
            )))
        }
    };
    if let Some(found) = met(state, &cond) {
        return Some(found);
    }
    match stream.try_clone() {
        Ok(reply) => {
            state.waits.push(PendingWait {
                reply,
                cond,
                deadline: Instant::now() + Duration::from_millis(timeout_ms),
                timeout_ms,
            });
            None
        }
        Err(e) => Some(err(e.to_string())),
    }
}

/// The reply for COND when it holds now.
fn met(state: &mut State, cond: &Cond) -> Option<Value> {
    match cond {
        Cond::Popup { before } => {
            let tree = crate::shell::tree(state);
            tree["surfaces"].as_array()?.iter().find_map(|n| {
                let id = n["id"].as_u64()?;
                (n["role"] == "popup" && !before.contains(&id))
                    .then(|| json!({"ok": true, "id": id}))
            })
        }
        Cond::Input { req, since } => state
            .input_log
            .iter()
            .find(|e| e["seq"].as_u64().unwrap_or(0) >= *since && e["req"].as_str() == Some(req))
            .map(|e| json!({"ok": true, "entry": e})),
    }
}

/// Answer the waits that are met, and fail the ones past their deadline.
pub fn check(state: &mut State) {
    if state.waits.is_empty() {
        return;
    }
    let now = Instant::now();
    let mut keep = Vec::new();
    for w in std::mem::take(&mut state.waits) {
        if let Some(reply) = met(state, &w.cond) {
            write_reply(state, &w.reply, &reply);
        } else if now >= w.deadline {
            let msg = format!("wait: {} after {} ms", w.cond.describe(), w.timeout_ms);
            write_reply(state, &w.reply, &err(msg));
        } else {
            keep.push(w);
        }
    }
    // a wait made while these were checked (none can be: `check` runs no command) keeps its place after them
    keep.append(&mut state.waits);
    state.waits = keep;
}

//! The control channel: one JSON object per line in, one per line out.  `handle` is the dispatch; later tasks add
//! commands to its `match`.
use crate::state::State;
use calloop::generic::Generic;
use calloop::{Interest, Mode, PostAction};
use serde_json::{json, Value};
use std::io::{ErrorKind, Write};
use std::os::unix::net::UnixStream;

pub fn err(msg: impl Into<String>) -> Value {
    json!({"ok": false, "error": msg.into()})
}

/// Write one reply line on a (non-blocking) control stream.  What the socket does not take at once is queued on the
/// event loop and written when the socket is writable; the loop never waits on a slow reader.  (A client sends one
/// command and reads its reply before the next, so replies to one stream do not overtake each other.)
pub fn write_reply(state: &State, stream: &UnixStream, reply: &Value) {
    let mut line = reply.to_string();
    line.push('\n');
    let mut w = stream;
    let mut done = 0;
    while done < line.len() {
        match w.write(&line.as_bytes()[done..]) {
            Ok(n) => done += n,
            Err(e) if e.kind() == ErrorKind::Interrupted => {}
            Err(e) if e.kind() == ErrorKind::WouldBlock => {
                let Ok(clone) = stream.try_clone() else {
                    return;
                };
                let bytes = line.into_bytes();
                let _ = state.handle.insert_source(
                    Generic::new(clone, Interest::WRITE, Mode::Level),
                    move |_, s, _| {
                        let mut w: &UnixStream = s;
                        loop {
                            match w.write(&bytes[done..]) {
                                Ok(n) => done += n,
                                Err(e) if e.kind() == ErrorKind::Interrupted => {}
                                Err(e) if e.kind() == ErrorKind::WouldBlock => {
                                    return Ok(PostAction::Continue)
                                }
                                Err(_) => return Ok(PostAction::Remove),
                            }
                            if done == bytes.len() {
                                return Ok(PostAction::Remove);
                            }
                        }
                    },
                );
                return;
            }
            Err(_) => return,
        }
    }
}

/// Run one command line; returns the reply, or None when the reply comes later (`ping`).
pub fn handle(state: &mut State, line: &str, stream: &UnixStream) -> Option<Value> {
    let req: Value = match serde_json::from_str(line) {
        Ok(v) => v,
        Err(e) => return Some(err(format!("bad JSON: {e}"))),
    };
    let Some(cmd) = req.get("cmd").and_then(Value::as_str) else {
        return Some(err("missing \"cmd\""));
    };
    if cmd == "ping" {
        return crate::shell::ping(state, stream);
    }
    if cmd == "read" {
        return crate::selection::read(state, &req, stream);
    }
    if cmd == "drag_drop" {
        return crate::dnd::drag_drop(state, &req, stream);
    }
    if let Some(reply) = crate::input::command(state, cmd, &req) {
        return Some(reply);
    }
    Some(match cmd {
        "snapshot" => crate::render::snapshot(state, &req),
        "configure" => crate::shell::configure(state, &req),
        "close" => crate::shell::close(state, &req),
        "decoration" => crate::shell::decoration(state, &req),
        "scale" => crate::scale::scale(state, &req),
        "offer" => crate::selection::offer(state, &req),
        "activation_log" => crate::selection::activation_log(state),
        "bell_log" | "idle_inhibitors" | "exports" | "session_log" => {
            crate::extras::command(state, cmd).unwrap_or_else(|| err("no such query"))
        }
        "output" => crate::scale::output(state, &req),
        "tree" => crate::shell::tree(state),
        "popup_done" => crate::shell::popup_done(state, &req),
        "click_outside" => crate::shell::click_outside(state),
        "present" => crate::clock::present(state, &req),
        "mode" => crate::clock::mode(state, &req),
        "occlude" => crate::clock::occlude(state, true),
        "unocclude" => crate::clock::occlude(state, false),
        "release" => crate::clock::release(state),
        "outstanding" => crate::clock::outstanding(state),
        "globals" => json!({"ok": true, "globals": state.globals}),
        "clients" => {
            let list: Vec<Value> = state
                .clients
                .lock()
                .unwrap()
                .iter()
                .map(|c| json!({"id": c.id, "pid": c.pid}))
                .collect();
            json!({"ok": true, "clients": list})
        }
        "log" => {
            let since = req.get("since").and_then(Value::as_u64).unwrap_or(0);
            let commits: Vec<&crate::log::Commit> = state.log.since(since).collect();
            json!({"ok": true, "next": state.log.next_seq(), "commits": commits})
        }
        // commits of toplevels, popups and subsurfaces (not cursors) that attach a new buffer and damage nothing
        "empty_buffer_commits" => {
            let since = req.get("since").and_then(Value::as_u64).unwrap_or(0);
            let commits: Vec<&crate::log::Commit> = state
                .log
                .since(since)
                .filter(|c| {
                    matches!(c.role.as_str(), "toplevel" | "popup" | "subsurface")
                        && c.buffer.is_some()
                        && c.damage.is_empty()
                })
                .collect();
            json!({"ok": true, "next": state.log.next_seq(), "commits": commits})
        }
        "quit" => {
            state.quit();
            json!({"ok": true})
        }
        other => err(format!("unknown command {other:?}")),
    })
}

//! The control channel: one JSON object per line in, one per line out.  `handle` is the dispatch; later tasks add
//! commands to its `match`.
use crate::state::State;
use serde_json::{json, Value};

fn err(msg: impl Into<String>) -> Value {
    json!({"ok": false, "error": msg.into()})
}

/// Run one command line; returns the reply.
pub fn handle(state: &mut State, line: &str) -> Value {
    let req: Value = match serde_json::from_str(line) {
        Ok(v) => v,
        Err(e) => return err(format!("bad JSON: {e}")),
    };
    let Some(cmd) = req.get("cmd").and_then(Value::as_str) else {
        return err("missing \"cmd\"");
    };
    match cmd {
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
        "quit" => {
            state.quit();
            json!({"ok": true})
        }
        other => err(format!("unknown command {other:?}")),
    }
}

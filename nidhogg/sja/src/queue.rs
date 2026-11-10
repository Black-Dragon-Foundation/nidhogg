//! The model queue: a hub's `view-model` line arrives whole (`SJA_PUSH`), unparsed by Lisp.  Sjá reads its envelope
//! (`view`, `hub`, `base`, `version`, `reset`) and its model (an object, `{"ops": [...]}`), decides by the envelope, and
//! holds the model until the draw applies it (`SJA_DRAIN`, up to the version the drawn rows name), or until no row patch
//! names it (`SJA_DUE`).
//!
//! The envelope rules (the stream's hub and last version, per view):
//! - a reset of another hub, or with base 0 (a new subscription): a new stream (`restart`: the queue and the stream go,
//!   a new generation, so its version 1 is never old to the model);
//! - a reset not newer than the stream: dropped; a newer one: queued, replacing the queue;
//! - a diff of another hub (or before any reset), or of an older base: dropped; of a base past the stream: a gap,
//!   `resync` (Lisp asks the hub for a reset); of the stream's version: queued.
//!
//! Past `MAX_QUEUED` models or `MAX_QUEUED_BYTES` the queue and the stream are dropped and the answer is `resync`.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::Value;

use crate::wire::{self, Bad, Op};

/// The most models a view holds unapplied.
pub const MAX_QUEUED: usize = 64;
/// The most bytes of lines a view holds unapplied.
pub const MAX_QUEUED_BYTES: usize = 16 << 20;

/// Counts the streams this process began: a view's stream is known to its model as `HUB/GEN`.
static GEN: AtomicU64 = AtomicU64::new(0);

/// What a push decided.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decision {
    /// A diff queued.
    Queue = 0,
    /// A reset queued (a new stream, or a newer reset of this one): any reset Lisp asked for has come.
    Reset = 1,
    Drop = 2,
    /// A gap, or a cap: ask the hub for a reset.
    Resync = 3,
    /// No buffer shows the view.
    None = 4,
}

/// A `view-model` line's envelope.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Envelope {
    pub view: String,
    pub hub: u64,
    pub base: u64,
    pub version: u64,
    pub reset: bool,
}

/// One queued model: its version, its ops (or why they didn't decode), the bytes of its line.
#[derive(Debug)]
pub struct Queued {
    pub version: u64,
    pub ops: Result<Vec<Op>, Bad>,
    pub bytes: usize,
}

/// One view's stream and queue.
#[derive(Debug, Default)]
pub struct Queue {
    /// The stream's hub: none before its first reset, after the hub went or the queue was dropped.
    pub hub: Option<u64>,
    pub generation: u64,
    /// The newest version the stream queued.
    pub version: u64,
    /// The highest version a row patch named.
    pub named: u64,
    pub items: VecDeque<Queued>,
    pub bytes: usize,
}

impl Queue {
    /// The stream and the queue go: nothing queues again before a reset.
    pub fn lose(&mut self) {
        self.items.clear();
        self.bytes = 0;
        self.hub = None;
        self.version = 0;
        self.named = 0;
    }

    /// The model's name for this stream's hub.
    pub fn hub_name(&self) -> String {
        format!("{}/{}", self.hub.unwrap_or(0), self.generation)
    }

    fn decide(&self, e: &Envelope) -> (Decision, bool) {
        let same = self.hub == Some(e.hub);
        if e.reset {
            if !same || e.base == 0 {
                (Decision::Reset, true)
            } else if e.version <= self.version {
                (Decision::Drop, false)
            } else {
                (Decision::Reset, false)
            }
        } else if !same || e.base < self.version {
            (Decision::Drop, false)
        } else if e.base > self.version {
            (Decision::Resync, false)
        } else {
            (Decision::Queue, false)
        }
    }

    /// Decide `e`, and on `Reset`/`Queue` take its stream state (without the model): the order rules alone.
    fn decide_and_mark(&mut self, e: &Envelope) -> Decision {
        let (d, restart) = self.decide(e);
        if matches!(d, Decision::Reset | Decision::Queue) {
            if restart {
                self.lose();
                self.generation = GEN.fetch_add(1, Ordering::Relaxed) + 1;
            }
            if e.reset {
                self.items.clear();
                self.bytes = 0;
                self.hub = Some(e.hub);
            }
            self.version = e.version;
        }
        d
    }

    /// Push one model with envelope `e`, its ops and its line's bytes.
    pub fn push(&mut self, e: &Envelope, ops: Result<Vec<Op>, Bad>, bytes: usize) -> Decision {
        let d = self.decide_and_mark(e);
        if !matches!(d, Decision::Reset | Decision::Queue) {
            return d;
        }
        if self.items.len() >= MAX_QUEUED || self.bytes + bytes > MAX_QUEUED_BYTES {
            self.lose();
            return Decision::Resync;
        }
        self.bytes += bytes;
        self.items.push_back(Queued {
            version: e.version,
            ops,
            bytes,
        });
        d
    }

    /// The oldest queued model of a version up to `upto` (all with `None`).
    pub fn next_upto(&mut self, upto: Option<u64>) -> Option<Queued> {
        let front = self.items.front()?;
        if upto.is_some_and(|u| front.version > u) {
            return None;
        }
        let q = self.items.pop_front()?;
        self.bytes -= q.bytes;
        Some(q)
    }

    /// Whether the oldest queued model is one no row patch named (the timer applies it).
    pub fn due(&self) -> bool {
        self.items.front().is_some_and(|q| q.version > self.named)
    }

    /// The versions queued, oldest first.
    pub fn queued_versions(&self) -> Vec<u64> {
        self.items.iter().map(|q| q.version).collect()
    }
}

fn count(v: Option<&Value>) -> Option<u64> {
    v.and_then(Value::as_u64)
}

/// Read a `view-model` line: its envelope and its model's ops.  `None` when it is not one.  Typed, or, for what the
/// typed read can't read, through `Value`.
pub fn read_line(line: &str) -> Option<(Envelope, Result<Vec<Op>, Bad>)> {
    if let Some((e, ops)) = read_line_typed(line) {
        return Some((e, Ok(ops)));
    }
    read_line_value(line)
}

/// Read a `view-model` line straight into its envelope and ops (`typed.rs`); `None` for anything else.
pub fn read_line_typed(line: &str) -> Option<(Envelope, Vec<Op>)> {
    crate::typed::line(line)
}

/// Read a `view-model` line through `Value`: the reference the typed read matches.
pub fn read_line_value(line: &str) -> Option<(Envelope, Result<Vec<Op>, Bad>)> {
    let v: Value = serde_json::from_str(line).ok()?;
    if v.get("event")?.as_str()? != "view-model" {
        return None;
    }
    let d = v.get("data")?.as_object()?;
    let e = Envelope {
        view: d.get("view")?.as_str()?.to_string(),
        hub: count(d.get("hub"))?,
        base: count(d.get("base"))?,
        version: count(d.get("version"))?,
        reset: d.get("reset").and_then(Value::as_bool).unwrap_or(false),
    };
    let ops = match d.get("model") {
        Some(m) => wire::decode_value(m),
        None => Err(Bad("view-model: no model".into())),
    };
    Some((e, ops))
}

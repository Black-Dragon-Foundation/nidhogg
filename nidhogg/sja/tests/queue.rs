//! The model queue: a hub's `view-model` line pushed whole, decided by its envelope, held until the draw drains it.
//! The table is process-wide, so every test through `sja_call` holds `EMACS` and uses its own view names.

mod common;
use common::*;
use serde_json::{Value, json};
use sja::abi::*;
use sja::queue::{Decision, Envelope, Queue};
use sja::view::Status;
use std::sync::Mutex;

static EMACS: Mutex<()> = Mutex::new(());

fn lock() -> std::sync::MutexGuard<'static, ()> {
    EMACS.lock().unwrap_or_else(|e| e.into_inner())
}

fn ok(op: i32, args: &[SjaArg]) -> Ans {
    call_for_test(op, args).unwrap_or_else(|rc| panic!("op {op}: code {rc}"))
}

/// A `view-model` line as the hub writes it: the event name first, the model an object.
fn line(view: &str, hub: u64, base: u64, version: u64, reset: bool, model: &str) -> String {
    let model: Value = serde_json::from_str(model).unwrap();
    let mut data =
        json!({"view": view, "hub": hub, "base": base, "version": version, "model": model});
    if reset {
        data["reset"] = json!(true);
    }
    json!({"event": "view-model", "data": data}).to_string()
}

fn push(l: &str) -> Ans {
    ok(SJA_PUSH, &[text(l)])
}

fn decision(a: &Ans) -> i64 {
    match a {
        Ans::List(xs) => match xs.as_slice() {
            [Ans::Str(_), Ans::Int(_), Ans::Int(d)] => *d,
            _ => panic!("push answered {a:?}"),
        },
        _ => panic!("push answered {a:?}"),
    }
}

fn open(view: &str) -> i64 {
    match ok(SJA_OPEN, &[text(view), nil()]) {
        Ans::Int(h) => h,
        a => panic!("open answered {a:?}"),
    }
}

/// `(APPLIED LAST VERSION)`.
fn drain(view: &str, upto: Option<i64>) -> (i64, i64) {
    match ok(SJA_DRAIN, &[text(view), upto.map(int).unwrap_or_else(nil)]) {
        Ans::List(xs) => match xs.as_slice() {
            [Ans::Int(n), Ans::Int(last), _] => (*n, *last),
            _ => panic!("drain answered {xs:?}"),
        },
        a => panic!("drain answered {a:?}"),
    }
}

fn get_version(h: i64) -> Ans {
    ok(SJA_GET, &[int(h), text("version")])
}

fn one_node(id: &str) -> String {
    ops(vec![json!({"nodes": [node(id, "agent", json!({}))]})])
}

/// The envelope rules, written as the editor's Lisp queue had them, as the oracle.
#[derive(Default, Clone)]
struct Oracle {
    hub: Option<u64>,
    version: u64,
    queue: Vec<u64>,
}

impl Oracle {
    fn event(&mut self, e: &Envelope) -> Decision {
        let same = self.hub.is_some() && self.hub == Some(e.hub);
        let what = if e.reset {
            if !same || e.base == 0 {
                "restart"
            } else if e.version <= self.version {
                "drop"
            } else {
                "queue"
            }
        } else if !same || e.base < self.version {
            "drop"
        } else if e.base > self.version {
            "resync"
        } else {
            "queue"
        };
        match what {
            "drop" => Decision::Drop,
            "resync" => Decision::Resync,
            _ => {
                if what == "restart" {
                    *self = Oracle::default();
                }
                if e.reset {
                    self.queue.clear();
                    self.hub = Some(e.hub);
                }
                self.version = e.version;
                self.queue.push(e.version);
                if e.reset {
                    Decision::Reset
                } else {
                    Decision::Queue
                }
            }
        }
    }
}

fn env(hub: u64, base: u64, version: u64, reset: bool) -> Envelope {
    Envelope {
        view: "v".into(),
        hub,
        base,
        version,
        reset,
    }
}

fn permutations(xs: &[Envelope]) -> Vec<Vec<Envelope>> {
    if xs.len() <= 1 {
        return vec![xs.to_vec()];
    }
    let mut out = Vec::new();
    for i in 0..xs.len() {
        let mut rest = xs.to_vec();
        let x = rest.remove(i);
        for mut p in permutations(&rest) {
            p.insert(0, x.clone());
            out.push(p);
        }
    }
    out
}

/// Every arrival order of a subscription's reset, its diffs, a late reset, another hub's reset and an old diff decides
/// as the editor's queue did, and leaves the same queue (pattern A).
#[test]
fn every_arrival_order_decides_as_the_editor_did() {
    let events = [
        env(7, 0, 1, true),
        env(7, 1, 2, false),
        env(7, 2, 3, false),
        env(7, 3, 4, true),
        env(9, 0, 1, true),
        env(7, 0, 1, false),
    ];
    for order in permutations(&events) {
        let (mut q, mut o) = (Queue::default(), Oracle::default());
        for e in &order {
            let want = o.event(e);
            let have = q.push(e, Ok(Vec::new()), 0);
            assert_eq!(have, want, "{e:?} in {order:?}");
        }
        assert_eq!(q.queued_versions(), o.queue, "{order:?}");
    }
}

/// A pushed line is held until drained; a drain up to N applies only versions up to N, oldest first (pattern D).
#[test]
fn a_push_waits_for_the_draw() {
    let _g = lock();
    let h = open("q-draw");
    assert_eq!(
        decision(&push(&line("q-draw", 7, 0, 1, true, &small_map()))),
        Decision::Reset as i64
    );
    assert_eq!(
        decision(&push(&line("q-draw", 7, 1, 2, false, &one_node("agent:d")))),
        Decision::Queue as i64
    );
    assert_eq!(
        get_version(h),
        Ans::Int(0),
        "nothing applies before the draw"
    );
    assert_eq!(drain("q-draw", Some(1)), (1, Status::Applied as i64));
    assert_eq!(get_version(h), Ans::Int(1));
    assert_eq!(drain("q-draw", None), (1, Status::Applied as i64));
    assert_eq!(get_version(h), Ans::Int(2));
    ok(SJA_CLOSE, &[int(h)]);
}

/// A gap answers resync and queues nothing; the reset that answers it starts the stream again (pattern D).
#[test]
fn a_gap_asks_for_a_reset() {
    let _g = lock();
    let h = open("q-gap");
    push(&line("q-gap", 7, 0, 1, true, &small_map()));
    assert_eq!(
        decision(&push(&line("q-gap", 7, 3, 4, false, &one_node("agent:d")))),
        Decision::Resync as i64
    );
    assert_eq!(
        decision(&push(&line("q-gap", 7, 5, 6, true, &small_map()))),
        Decision::Reset as i64
    );
    assert_eq!(drain("q-gap", None).0, 1, "the reset replaced the queue");
    ok(SJA_CLOSE, &[int(h)]);
}

/// Past 64 models the queue is dropped and a reset asked for; the stream takes nothing until one comes (pattern D).
#[test]
fn the_count_cap_drops_the_queue() {
    let _g = lock();
    let h = open("q-cap");
    push(&line("q-cap", 7, 0, 1, true, &small_map()));
    for v in 2..=64 {
        assert_eq!(
            decision(&push(&line(
                "q-cap",
                7,
                v - 1,
                v,
                false,
                &one_node("agent:d")
            ))),
            Decision::Queue as i64
        );
    }
    assert_eq!(
        decision(&push(&line(
            "q-cap",
            7,
            64,
            65,
            false,
            &one_node("agent:d")
        ))),
        Decision::Resync as i64
    );
    assert_eq!(drain("q-cap", None).0, 0);
    assert_eq!(
        decision(&push(&line(
            "q-cap",
            7,
            65,
            66,
            false,
            &one_node("agent:d")
        ))),
        Decision::Drop as i64
    );
    ok(SJA_CLOSE, &[int(h)]);
}

/// Past 16 MiB of queued lines the queue is dropped and a reset asked for (pattern D).
#[test]
fn the_byte_cap_drops_the_queue() {
    let _g = lock();
    let h = open("q-bytes");
    // lines of about 1 MB (250 nodes of a 4,000-byte label): the 17th passes 16 MiB, long before the count cap
    let label = "y".repeat(4000);
    let nodes: Vec<Value> = (0..250)
        .map(|i| json!({"id": format!("x{i}"), "kind": "agent", "fields": [["label", label]]}))
        .collect();
    let big = ops(vec![json!({ "nodes": nodes })]);
    push(&line("q-bytes", 7, 0, 1, true, &small_map()));
    let mut v = 1;
    let mut last = Decision::Queue as i64;
    while last == Decision::Queue as i64 && v < 60 {
        last = decision(&push(&line("q-bytes", 7, v, v + 1, false, &big)));
        v += 1;
    }
    assert_eq!(last, Decision::Resync as i64);
    assert!(
        (16..=18).contains(&v),
        "the byte cap answered after {v} models"
    );
    ok(SJA_CLOSE, &[int(h)]);
}

/// A new subscription's version 1 after a stream at version 5 is a new stream, never an old version (pattern D).
#[test]
fn a_new_subscription_restarts_the_stream() {
    let _g = lock();
    let h = open("q-restart");
    push(&line("q-restart", 7, 0, 1, true, &small_map()));
    for v in 2..=5 {
        push(&line("q-restart", 7, v - 1, v, false, &one_node("agent:d")));
    }
    drain("q-restart", None);
    assert_eq!(get_version(h), Ans::Int(5));
    assert_eq!(
        decision(&push(&line("q-restart", 7, 0, 1, true, &small_map()))),
        Decision::Reset as i64
    );
    assert_eq!(drain("q-restart", None), (1, Status::Applied as i64));
    assert_eq!(get_version(h), Ans::Int(1));
    ok(SJA_CLOSE, &[int(h)]);
}

/// A model named by a row patch waits for that patch's draw; `due` applies only models no patch named (pattern D).
#[test]
fn due_applies_only_what_no_patch_named() {
    let _g = lock();
    let h = open("q-due");
    push(&line("q-due", 7, 0, 1, true, &small_map()));
    push(&line("q-due", 7, 1, 2, false, &one_node("agent:d")));
    ok(SJA_NAMED, &[text("q-due"), int(2)]);
    assert_eq!(
        ok(SJA_DUE, &[text("q-due")]),
        Ans::Nil,
        "the oldest was named: it waits for its draw"
    );
    drain("q-due", Some(2));
    push(&line("q-due", 7, 2, 3, false, &one_node("agent:e")));
    match ok(SJA_DUE, &[text("q-due")]) {
        Ans::List(xs) => assert_eq!(xs[0], Ans::Int(1)),
        a => panic!("due answered {a:?}"),
    }
    assert_eq!(get_version(h), Ans::Int(3));
    ok(SJA_CLOSE, &[int(h)]);
}

/// A line for a view no buffer shows, or a line that isn't a `view-model`, is left alone (pattern D).
#[test]
fn a_line_for_no_view_is_left_alone() {
    let _g = lock();
    assert_eq!(
        decision(&push(&line("q-nobody", 7, 0, 1, true, &small_map()))),
        Decision::None as i64
    );
    assert_eq!(push(r#"{"event":"view-patch","data":{}}"#), Ans::Nil);
    assert_eq!(push("{not json"), Ans::Nil);
    // the push made no slot, so a buffer that opens the view later finds no queue
    let h = open("q-nobody");
    assert_eq!(drain("q-nobody", None).0, 0);
    ok(SJA_CLOSE, &[int(h)]);
}

/// The hub lost drops every queue: the next diff is dropped until a reset (pattern D).
#[test]
fn the_hub_lost_drops_the_queues() {
    let _g = lock();
    let h = open("q-lost");
    push(&line("q-lost", 7, 0, 1, true, &small_map()));
    ok(SJA_HUB_LOST, &[]);
    assert_eq!(drain("q-lost", None).0, 0);
    assert_eq!(
        decision(&push(&line("q-lost", 7, 1, 2, false, &one_node("agent:d")))),
        Decision::Drop as i64
    );
    ok(SJA_CLOSE, &[int(h)]);
}

/// A model that can't apply drops the queue and answers resync at the drain (pattern D).
#[test]
fn a_bad_model_resyncs_at_the_drain() {
    let _g = lock();
    let h = open("q-bad");
    push(&line("q-bad", 7, 0, 1, true, &small_map()));
    push(&line("q-bad", 7, 1, 2, false, r#"{"ops":[{"bogus":1}]}"#));
    push(&line("q-bad", 7, 2, 3, false, &one_node("agent:d")));
    assert_eq!(drain("q-bad", None), (1, Status::Resync as i64));
    assert_eq!(
        decision(&push(&line("q-bad", 7, 3, 4, false, &one_node("agent:e")))),
        Decision::Drop as i64
    );
    ok(SJA_CLOSE, &[int(h)]);
}

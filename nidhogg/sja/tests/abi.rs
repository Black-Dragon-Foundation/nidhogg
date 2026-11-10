//! The C entry point, the guard and the breaker, through `sja_call` (feature `test-hooks`).  The table is
//! process-wide, so every test holds `EMACS` (Emacs's global lock stands in) and uses its own view names.

mod common;
use common::*;
use sja::abi::*;
use sja::queue::Decision;
use sja::view::Status;
use std::sync::Mutex;

static EMACS: Mutex<()> = Mutex::new(());

fn lock() -> std::sync::MutexGuard<'static, ()> {
    EMACS.lock().unwrap_or_else(|e| e.into_inner())
}

fn ok(op: i32, args: &[SjaArg]) -> Ans {
    call_for_test(op, args).unwrap_or_else(|rc| panic!("op {op}: code {rc}"))
}

fn open(view: &str, group: Option<&str>) -> i64 {
    let g = group.map(text).unwrap_or_else(nil);
    match ok(SJA_OPEN, &[text(view), g]) {
        Ans::Int(h) => h,
        a => panic!("open answered {a:?}"),
    }
}

/// PATCH for VIEW from VERSION - 1 to VERSION, as the hub's line (a reset when its first op is one), pushed and
/// drained: the drain's status; a line nothing queued answers as one: `Resync` (a gap, a cap, a held lock) or
/// `Dropped` (old, or a broken view).
fn put(view: &str, version: u64, patch: &str) -> Ans {
    let model: serde_json::Value = serde_json::from_str(patch).unwrap();
    let mut data = serde_json::json!({"view": view, "hub": 1, "base": version - 1, "version": version, "model": model});
    if model["ops"][0].get("reset").is_some() {
        data["reset"] = serde_json::json!(true);
    }
    let line = serde_json::json!({"event": "view-model", "data": data}).to_string();
    let Ans::List(xs) = ok(SJA_PUSH, &[text(&line)]) else {
        panic!("push answered no decision")
    };
    let status = match xs[2] {
        Ans::Int(d) if d == Decision::Queue as i64 || d == Decision::Reset as i64 => {
            match ok(SJA_DRAIN, &[text(view), nil()]) {
                Ans::List(r) => return r[1].clone(),
                a => panic!("drain answered {a:?}"),
            }
        }
        Ans::Int(d) if d == Decision::Resync as i64 => Status::Resync,
        Ans::Int(d) if d == Decision::Drop as i64 => Status::Dropped,
        ref a => panic!("push decided {a:?}"),
    };
    Ans::Int(status as i64)
}

fn get(h: i64, what: &str) -> Ans {
    ok(SJA_GET, &[int(h), text(what)])
}

fn st(s: &str) -> Ans {
    Ans::Str(s.into())
}

fn stats() -> Vec<i64> {
    match ok(SJA_STATS, &[]) {
        Ans::List(xs) => xs
            .into_iter()
            .map(|x| if let Ans::Int(i) = x { i } else { -1 })
            .collect(),
        a => panic!("{a:?}"),
    }
}

/// Every kind of answer comes back as it was built (pattern D).
#[test]
fn answer_tokens_round_trip() {
    let _g = lock();
    let h = open("rt", None);
    assert_eq!(put("rt", 1, &small_map()), Ans::Int(Status::Applied as i64));
    assert_eq!(ok(SJA_FOLLOW, &[int(h), text("right")]), st("agent:a"));
    assert_eq!(ok(SJA_WALK, &[int(h), text("team"), int(1)]), st("agent:b"));
    assert_eq!(
        ok(SJA_WALK_TO, &[int(h), text("outline"), int(3)]),
        st("file:x")
    );
    assert_eq!(get(h, "kind"), st("file"));
    assert_eq!(
        ok(SJA_GET, &[int(h), text("field"), text("label")]),
        st("FILE:X")
    );
    assert_eq!(
        get(h, "class-names"),
        Ans::List(
            ["selected", "chain", "dim", "match", "lit"]
                .map(st)
                .to_vec()
        )
    );
    assert_eq!(
        ok(SJA_GET, &[int(h), text("list"), text("needs-you")]),
        Ans::List(vec![st("agent:b")])
    );
    let Ans::Vector(cl) = get(h, "classes") else {
        panic!("classes is a vector")
    };
    assert_eq!(cl.len(), 14, "7 nodes, id and class each");
    assert_eq!((&cl[0], &cl[1]), (&st("agent:a"), &Ans::Int(1)));
    assert_eq!(get(h, "classes"), Ans::Vector(vec![]));
    assert_eq!(
        ok(SJA_SET, &[int(h), text("select"), nil()]),
        Ans::Nil,
        "cleared"
    );
    assert_eq!(
        ok(SJA_SET, &[int(h), text("tick"), text("file:x")]),
        Ans::Int(1)
    );
    assert_eq!(get(h, "health"), Ans::Int(0));
    // a windowed list's edge: a dotted (ID . DIRECTION)
    let desc = serde_json::json!({"scheme": "list", "lists": ["hits"], "classes": ["selected"]});
    let nodes = (0..4)
        .map(|i| node(&format!("m{i}"), "m", serde_json::json!({})))
        .collect();
    let hits = serde_json::json!({"name": "hits", "ids": ["m0", "m1"], "total": 4, "from": 0});
    assert_eq!(
        put("rt", 2, &reset(desc, nodes, vec![], vec![hits])),
        Ans::Int(0)
    );
    ok(SJA_SET, &[int(h), text("select"), text("m1")]);
    assert_eq!(
        ok(SJA_WALK, &[int(h), text("hits"), int(1)]),
        Ans::Dotted(vec![st("m1"), Ans::Int(1)])
    );
    ok(SJA_CLOSE, &[int(h)]);
}

/// Bad arguments are `SJA_E_ARG`, never a panic (pattern D).
#[test]
fn bad_utf8_and_bad_args_are_e_arg() {
    let _g = lock();
    let h = open("args", None);
    let bad = [0xffu8, 0xfe];
    let bad_str = SjaArg {
        tag: SJA_STR,
        int: 0,
        ptr: bad.as_ptr(),
        len: bad.len(),
    };
    let e = |op, args: &[SjaArg]| call_for_test(op, args).err();
    assert_eq!(e(SJA_OPEN, &[bad_str]), Some(SJA_E_ARG));
    assert_eq!(e(12345, &[]), Some(SJA_E_ARG), "an unknown op");
    assert_eq!(
        e(SJA_FOLLOW, &[int(987_654), text("up")]),
        Some(SJA_E_ARG),
        "an unknown handle"
    );
    assert_eq!(
        e(SJA_FOLLOW, &[text("x"), text("up")]),
        Some(SJA_E_ARG),
        "a wrong type"
    );
    assert_eq!(
        e(SJA_SET, &[int(h), text("nonsense"), nil()]),
        Some(SJA_E_ARG)
    );
    assert_eq!(
        e(1, &[text("args"), text("h"), int(1), text("{\"ops\": []}")]),
        Some(SJA_E_ARG),
        "op 1, the string apply, is gone: a model comes only as the hub's line (SJA_PUSH)"
    );
    let tag = SjaArg {
        tag: 42,
        int: 0,
        ptr: std::ptr::null(),
        len: 0,
    };
    assert_eq!(e(SJA_CLOSE, &[tag]), Some(SJA_E_ARG), "an unknown tag");
    // null out pointers
    let mut n = 0usize;
    // SAFETY: no arguments; `out` is null on purpose.
    let rc = unsafe { sja_call(SJA_STATS, std::ptr::null(), 0, std::ptr::null_mut(), &mut n) };
    assert_eq!(rc, SJA_E_ARG);
    ok(SJA_CLOSE, &[int(h)]);
}

/// A panic in one view drops that view's model and nothing else; its handles keep their selection ids, and the next
/// reset puts them back (pattern A: the panic comes between any two calls).
#[test]
fn panic_drops_one_view() {
    let _g = lock();
    sja::guard::set_now_for_test(Some(1_000_000));
    let (a, b) = (open("pa", None), open("pb", None));
    for v in ["pa", "pb"] {
        assert_eq!(put(v, 1, &small_map()), Ans::Int(0));
    }
    ok(SJA_SET, &[int(a), text("select"), text("agent:c")]);
    ok(SJA_SET, &[int(b), text("select"), text("file:y")]);
    let panics = stats()[3];
    assert_eq!(
        ok(SJA_TEST_PANIC, &[text("pa")]),
        Ans::Int(Status::Resync as i64)
    );
    assert_eq!(stats()[3], panics + 1);
    assert_eq!(get(a, "health"), Ans::Int(1), "pa has no model");
    assert_eq!(get(a, "selection"), Ans::Nil);
    assert_eq!(get(b, "selection"), st("file:y"), "pb untouched");
    assert_eq!(get(b, "health"), Ans::Int(0));
    // the panic took the stream with the model: an increment is dropped (the panic's own resync asked for the
    // reset); the reset puts the selection back
    let inc = ops(vec![serde_json::json!({"remove": ["file:x"]})]);
    assert_eq!(put("pa", 2, &inc), Ans::Int(Status::Dropped as i64));
    assert_eq!(put("pa", 3, &small_map()), Ans::Int(0));
    assert_eq!(get(a, "selection"), st("agent:c"));
    sja::guard::set_now_for_test(None);
    ok(SJA_CLOSE, &[int(a)]);
    ok(SJA_CLOSE, &[int(b)]);
}

/// Two panics within 60 s trip the breaker; panics further apart don't.  Closing the view clears it (pattern D: the
/// breaker's clock is the test's).
#[test]
fn breaker_trips_after_two_panics() {
    let _g = lock();
    let h = open("br", None);
    let panic_at = |ms| {
        sja::guard::set_now_for_test(Some(ms));
        ok(SJA_TEST_PANIC, &[text("br")])
    };
    let resync = Ans::Int(Status::Resync as i64);
    let broken = Ans::Int(Status::Broken as i64);
    assert_eq!(panic_at(0), resync);
    assert_eq!(panic_at(70_000), resync, "70 s apart: not tripped");
    let before = stats()[4];
    assert_eq!(panic_at(80_000), broken, "10 s apart: tripped");
    assert_eq!(stats()[4], before + 1);
    assert_eq!(
        put("br", 1, &small_map()),
        Ans::Int(Status::Dropped as i64),
        "a broken view drops its lines"
    );
    assert_eq!(get(h, "health"), Ans::Int(2));
    assert_eq!(ok(SJA_FOLLOW, &[int(h), text("up")]), Ans::Nil);
    ok(SJA_CLOSE, &[int(h)]);
    assert_eq!(stats()[4], before, "the view went with its last instance");
    let h = open("br", None);
    assert_eq!(get(h, "health"), Ans::Int(1), "a fresh view, no model yet");
    assert_eq!(put("br", 1, &small_map()), Ans::Int(0));
    sja::guard::set_now_for_test(None);
    ok(SJA_CLOSE, &[int(h)]);
}

/// A view whose lock is held (a call a longjmp abandoned) answers as a resync and never blocks (pattern C: the test
/// holds the lock across the calls).
#[test]
fn held_view_lock_answers_resync_not_block() {
    let _g = lock();
    let h = open("held", None);
    assert_eq!(put("held", 1, &small_map()), Ans::Int(0));
    let slot = sja::guard::with_table(|t| t.slot("held")).unwrap();
    {
        let _held = slot.view.lock().unwrap();
        assert_eq!(
            put("held", 2, &small_map()),
            Ans::Int(Status::Resync as i64)
        );
        assert_eq!(ok(SJA_FOLLOW, &[int(h), text("up")]), Ans::Nil);
        assert_eq!(get(h, "health"), Ans::Int(1));
    }
    assert_eq!(ok(SJA_FOLLOW, &[int(h), text("right")]), st("agent:a"));
    ok(SJA_CLOSE, &[int(h)]);
}

/// A hub's line whose model is arbitrary bytes, pushed and drained, never panics past the guard and never passes the
/// caps (pattern D, seeded).
#[test]
fn a_line_of_arbitrary_model_bytes_never_panics_past_the_guard() {
    let _g = lock();
    let h = open("fuzz", None);
    let seed = small_map().into_bytes();
    let panics = stats()[3];
    let mut rng = Rng::new(42);
    for i in 0..2000 {
        let mut b = seed.clone();
        for _ in 0..1 + rng.below(8) {
            let at = rng.below(b.len());
            match rng.below(3) {
                0 => b[at] = rng.below(256) as u8,
                1 => {
                    b.remove(at);
                }
                _ => b.insert(at, b"{}[]\",:0"[rng.below(8)]),
            }
        }
        let mut l = format!(
            r#"{{"event":"view-model","data":{{"view":"fuzz","hub":1,"base":{i},"version":{},"reset":true,"model":"#,
            i + 1
        )
        .into_bytes();
        l.extend_from_slice(&b);
        l.extend_from_slice(b"}}");
        let arg = SjaArg {
            tag: SJA_STR,
            int: 0,
            ptr: l.as_ptr(),
            len: l.len(),
        };
        match call_for_test(SJA_PUSH, &[arg]) {
            Ok(Ans::Nil) | Ok(Ans::List(_)) => {}
            Err(rc) => assert_eq!(rc, SJA_E_ARG, "only bad UTF-8 is refused"),
            Ok(a) => panic!("{a:?}"),
        }
        match ok(SJA_DRAIN, &[text("fuzz"), nil()]) {
            Ans::List(r) => match r[1] {
                Ans::Int(s) => assert!((0..=3).contains(&s), "status {s}"),
                ref a => panic!("{a:?}"),
            },
            a => panic!("{a:?}"),
        }
    }
    assert_eq!(stats()[3], panics, "no panic reached the guard");
    let bytes = sja::guard::with_table(|t| t.stats()[2]).unwrap();
    assert!(bytes <= sja::guard::TOTAL_BYTES);
    ok(SJA_CLOSE, &[int(h)]);
}

/// The header declares every export.
#[test]
fn header_declares_every_export() {
    let h = include_str!("../include/sja.h");
    for e in EXPORTS {
        assert!(h.contains(&format!(" {e} (")), "{e} is not in sja.h");
    }
}

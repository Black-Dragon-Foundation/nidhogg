//! The selection history: the reserved links `back` and `forward` (pattern D, pure: no clock, no threads).

mod common;
use common::*;
use serde_json::json;
use sja::{HISTORY, Status};

#[test]
fn back_and_forward_retrace_the_jumps() {
    let mut v = view_with(&small_map());
    v.select(1, Some("agent:a"));
    v.select(1, Some("ask:1"));
    v.select(1, Some("file:y"));
    assert_eq!(s(&v.follow(1, "back")), Some("ask:1"));
    assert_eq!(s(&v.follow(1, "back")), Some("agent:a"));
    assert_eq!(
        s(&v.follow(1, "back")),
        Some("agent:a"),
        "nothing further back: stays"
    );
    assert_eq!(s(&v.follow(1, "forward")), Some("ask:1"));
    assert_eq!(s(&v.follow(1, "forward")), Some("file:y"));
    assert_eq!(
        s(&v.follow(1, "forward")),
        Some("file:y"),
        "nothing further on: stays"
    );
}

#[test]
fn a_run_of_steps_is_one_move() {
    // arrows, a walk and type-to-navigate are steps: a run of them goes back to where it began
    let mut v = view_with(&small_map());
    v.select(1, Some("agent:a"));
    v.follow(1, "right");
    v.follow(1, "right");
    assert_eq!(s(&v.selection(1)), Some("agent:c"));
    assert_eq!(s(&v.follow(1, "back")), Some("agent:a"));
    assert_eq!(s(&v.follow(1, "forward")), Some("agent:c"));
    // a jump ends the run: the next steps are a run of their own
    v.select(1, Some("ask:1"));
    v.follow(1, "down");
    assert_eq!(s(&v.selection(1)), Some("plan:1"));
    assert_eq!(s(&v.follow(1, "back")), Some("ask:1"));
    assert_eq!(s(&v.follow(1, "back")), Some("agent:c"));
}

#[test]
fn a_new_move_drops_what_was_ahead() {
    let mut v = view_with(&small_map());
    v.select(1, Some("agent:a"));
    v.select(1, Some("ask:1"));
    assert_eq!(s(&v.follow(1, "back")), Some("agent:a"));
    v.select(1, Some("file:y"));
    assert_eq!(
        s(&v.follow(1, "forward")),
        Some("file:y"),
        "the old way forward is gone"
    );
    assert_eq!(s(&v.follow(1, "back")), Some("agent:a"));
}

#[test]
fn back_skips_what_the_model_no_longer_holds() {
    let mut v = view_with(&small_map());
    v.select(1, Some("agent:a"));
    v.select(1, Some("file:x"));
    v.select(1, Some("ask:1"));
    assert_eq!(
        v.apply("h1", 2, &ops(vec![json!({"remove": ["file:x"]})]), BIG),
        Status::Applied
    );
    assert_eq!(s(&v.follow(1, "back")), Some("agent:a"));
}

#[test]
fn the_history_is_bounded() {
    let mut v = view_with(&small_map());
    for k in 0..HISTORY + 10 {
        v.select(1, Some(if k % 2 == 0 { "agent:a" } else { "agent:b" }));
    }
    let mut moves = 0;
    loop {
        let before = v.selection(1);
        if v.follow(1, "back") == before {
            break;
        }
        moves += 1;
        assert!(moves <= HISTORY + 10, "back never stops");
    }
    assert_eq!(moves, HISTORY);
}

#[test]
fn each_instance_has_its_own_history() {
    let mut v = view_with(&small_map());
    v.open(2, None);
    v.select(1, Some("agent:a"));
    v.select(1, Some("ask:1"));
    v.select(2, Some("file:y"));
    assert_eq!(
        s(&v.follow(2, "back")),
        Some("file:y"),
        "instance 2 never moved from a selection"
    );
    assert_eq!(s(&v.follow(1, "back")), Some("agent:a"));
}

//! Apply's rules: versions, hubs, resets, caps, malformed patches (pattern D, pure).

mod common;
use common::*;
use serde_json::json;
use sja::{Status, View};

#[test]
fn reset_then_increments_apply() {
    let mut v = view_with(&small_map());
    let p = ops(vec![
        json!({"nodes": [node("file:z", "file", json!({}))]}),
        json!({"edges": {"add": [["agent:c", "file:z", "edits"]], "remove": [["agent:a", "file:x", "edits"]]}}),
        json!({"splice": {"name": "team", "at": 1, "remove": 1, "ids": ["agent:c", "agent:b"]}}),
    ]);
    assert_eq!(v.apply("h1", 2, &p, BIG), Status::Applied);
    let (nodes, edges, lists) = snap(&v);
    assert!(nodes.contains_key("file:z"));
    assert!(edges.contains("agent:c>file:z:edits"));
    assert!(!edges.contains("agent:a>file:x:edits"));
    assert_eq!(
        lists["team"],
        vec!["agent:a", "agent:c", "agent:b", "agent:c"]
    );
    // the removed edge left the chain
    v.select(1, Some("agent:a"));
    assert_eq!(v.class_of(1, "file:x"), Some(2));
    v.select(1, Some("agent:c"));
    assert_eq!(v.class_of(1, "file:z"), Some(1));
}

#[test]
fn versions_and_hubs() {
    let mut v = view_with(&small_map());
    let p = ops(vec![json!({"remove": ["agent:c"]})]);
    assert_eq!(v.apply("h1", 1, &p, BIG), Status::Dropped, "not newer");
    assert_eq!(
        v.apply("h2", 5, &p, BIG),
        Status::Resync,
        "a new hub needs a reset first"
    );
    assert_eq!(
        v.apply("h2", 5, &small_map(), BIG),
        Status::Applied,
        "a new hub's reset"
    );
    assert_eq!(v.version, 5);
    assert_eq!(v.apply("h2", 6, &p, BIG), Status::Applied);
    let mut fresh = View::default();
    assert_eq!(
        fresh.apply("h1", 1, &p, BIG),
        Status::Resync,
        "nothing before the first reset"
    );
}

#[test]
fn malformed_patches_resync_and_change_nothing() {
    let mut v = view_with(&small_map());
    let before = snap(&v);
    for bad in [
        "not json",
        "{}",
        r#"{"ops": [{"nodes": [{"id": 3}]}]}"#,
        r#"{"ops": [{"nodes": [{"kind": "x"}]}]}"#,
        r#"{"ops": [{"edges": {"add": [["a", "b"]]}}]}"#,
        r#"{"ops": [{"nope": 1}]}"#,
        r#"{"ops": [{"a": 1, "b": 2}]}"#,
        r#"{"ops": [{"remove": ["agent:a"]}, {"splice": {"name": "team", "at": 2, "remove": 5, "ids": []}}]}"#,
        r#"{"ops": [{"splice": {"name": "nope", "at": 0, "remove": 0, "ids": []}}]}"#,
    ] {
        assert_eq!(v.apply("h1", 2, bad, BIG), Status::Resync, "{bad}");
        assert_eq!(snap(&v), before, "{bad} changed the model");
    }
}

#[test]
fn caps_answer_too_large_and_change_nothing() {
    let mut v = view_with(&small_map());
    let before = snap(&v);
    let used = v.bytes();
    let big = "x".repeat(3000);
    let nodes: Vec<_> = (0..40)
        .map(|i| json!({"id": format!("big{i}"), "kind": "file", "fields": [["label", big]]}))
        .collect();
    let p = ops(vec![json!({ "nodes": nodes })]);
    assert_eq!(v.apply("h1", 2, &p, used + 50_000), Status::TooLarge);
    assert_eq!(snap(&v), before);
    let long =
        json!({"name": "team", "ids": (0..20_001).map(|i| format!("a{i}")).collect::<Vec<_>>()});
    assert_eq!(
        v.apply("h1", 2, &ops(vec![json!({"list": long})]), BIG),
        Status::TooLarge,
        "unwindowed"
    );
    let lists: Vec<_> = (0..70)
        .map(|i| json!({"list": {"name": format!("l{i}"), "ids": []}}))
        .collect();
    assert_eq!(
        v.apply("h1", 2, &ops(lists), BIG),
        Status::TooLarge,
        "too many lists"
    );
    assert_eq!(snap(&v), before);
    assert_eq!(
        v.apply("h1", 2, &p, BIG),
        Status::Applied,
        "under the budget it applies"
    );
}

#[test]
fn values_are_cut_at_4_kib() {
    let mut v = view_with(&small_map());
    let long = "é".repeat(3000);
    let n = json!({"id": "file:x", "kind": "file", "fields": [["label", long]]});
    assert_eq!(
        v.apply("h1", 2, &ops(vec![json!({"nodes": [n]})]), BIG),
        Status::Applied
    );
    let Some(sja::wire::Val::Str(s)) = v.field(1, "label", Some("file:x")) else {
        panic!("no label")
    };
    assert!(s.len() <= 4096 && s.len() >= 4094, "{}", s.len());
}

#[test]
fn reset_keeps_selection_by_id_or_follows_the_old_then() {
    let mut v = view_with(&small_map());
    v.select(1, Some("agent:b"));
    assert_eq!(v.apply("h2", 1, &small_map(), BIG), Status::Applied);
    assert_eq!(
        s(&v.selection(1)),
        Some("agent:b"),
        "kept by id across a new hub's reset"
    );
    let mut n = node("file:y", "file", json!({}));
    n["then"] = json!("agent:b");
    assert_eq!(
        v.apply("h2", 2, &ops(vec![json!({"nodes": [n]})]), BIG),
        Status::Applied
    );
    v.select(1, Some("file:y"));
    let without = reset(
        map_desc(),
        vec![
            node("agent:b", "agent", json!({})),
            node("agent:a", "agent", json!({})),
        ],
        vec![],
        vec![],
    );
    assert_eq!(v.apply("h2", 3, &without, BIG), Status::Applied);
    assert_eq!(s(&v.selection(1)), Some("agent:b"), "the old model's then");
    assert_eq!(s(&v.take_finished(1)), Some("file:y"));
}

#[test]
fn hub_lost_marks_stale_and_keys_still_work() {
    let mut v = view_with(&small_map());
    v.hub_lost();
    assert!(v.stale);
    v.select(1, Some("agent:a"));
    assert_eq!(s(&v.follow(1, "right")), Some("agent:b"));
    assert_eq!(v.apply("h9", 1, &small_map(), BIG), Status::Applied);
    assert!(!v.stale);
}

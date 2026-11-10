//! The verbs on a small map and small lists (pattern D, pure: no clock, no threads).

mod common;
use common::*;
use serde_json::json;
use sja::{Status, Step, View};

#[test]
fn follow_each_link_at_ends_and_nulls() {
    let mut v = view_with(&small_map());
    // no selection: the first entry of the start list
    assert_eq!(s(&v.follow(1, "right")), Some("agent:a"));
    assert_eq!(
        s(&v.follow(1, "left")),
        Some("agent:a"),
        "no left link: stays"
    );
    assert_eq!(s(&v.follow(1, "right")), Some("agent:b"));
    assert_eq!(s(&v.follow(1, "right")), Some("agent:c"));
    assert_eq!(s(&v.follow(1, "right")), Some("agent:c"), "the layer's end");
    assert_eq!(s(&v.follow(1, "up")), Some("agent:c"), "a null link: stays");
    v.select(1, Some("agent:b"));
    assert_eq!(s(&v.follow(1, "down")), Some("file:y"));
    assert_eq!(s(&v.follow(1, "up")), Some("agent:b"));
    assert_eq!(s(&v.follow(1, "up")), Some("plan:1"));
    assert_eq!(s(&v.follow(1, "up")), Some("ask:1"));
    assert_eq!(
        s(&v.follow(1, "nowhere")),
        Some("ask:1"),
        "an unknown link: stays"
    );
}

#[test]
fn link_to_a_node_outside_the_model_stays() {
    let mut v = view_with(&small_map());
    let p = ops(vec![
        json!({"nodes": [node("agent:a", "agent", json!({"right": "agent:zz"}))]}),
    ]);
    assert_eq!(v.apply("h1", 2, &p, BIG), Status::Applied);
    v.select(1, Some("agent:a"));
    assert_eq!(s(&v.follow(1, "right")), Some("agent:a"));
}

#[test]
fn list_form_walks_its_list() {
    let mut v = view_with(&small_map());
    v.set_form(1, Some("outline"));
    v.select(1, Some("agent:a"));
    assert_eq!(s(&v.follow(1, "down")), Some("file:x"));
    assert_eq!(s(&v.follow(1, "down")), Some("agent:b"));
    assert_eq!(s(&v.follow(1, "up")), Some("file:x"));
    v.select(1, Some("ask:1"));
    assert_eq!(
        s(&v.follow(1, "up")),
        Some("ask:1"),
        "no wrap in a form's list"
    );
    // left/right still follow the links
    v.select(1, Some("agent:a"));
    assert_eq!(s(&v.follow(1, "right")), Some("agent:b"));
}

#[test]
fn form_link_set_wins_over_the_default() {
    let mut n = node("agent:a", "agent", json!({"right": "agent:b"}));
    n["navs"] = json!({"board": {"right": "agent:c"}});
    let mut v = view_with(&small_map());
    assert_eq!(
        v.apply("h1", 2, &ops(vec![json!({"nodes": [n]})]), BIG),
        Status::Applied
    );
    v.select(1, Some("agent:a"));
    v.set_form(1, Some("board"));
    assert_eq!(s(&v.follow(1, "right")), Some("agent:c"));
    v.select(1, Some("agent:b"));
    // agent:b has no board set: the default set
    assert_eq!(s(&v.follow(1, "right")), Some("agent:c"));
}

#[test]
fn walk_wraps_and_walk_to_jumps() {
    let mut v = view_with(&small_map());
    let w = |id: &str| Step {
        id: Some(id.into()),
        more: None,
    };
    assert_eq!(
        v.walk(1, "team", 1),
        w("agent:a"),
        "from nothing: the first"
    );
    assert_eq!(v.walk(1, "team", 1), w("agent:b"));
    assert_eq!(v.walk(1, "team", 1), w("agent:c"));
    assert_eq!(v.walk(1, "team", 1), w("agent:a"), "wraps");
    assert_eq!(v.walk(1, "team", -1), w("agent:c"), "wraps back");
    v.select(1, Some("file:x"));
    assert_eq!(
        v.walk(1, "team", -1),
        w("agent:c"),
        "from outside the list, backward: the last"
    );
    assert_eq!(v.walk_to(1, "outline", 3), w("file:x"));
    assert_eq!(
        v.walk_to(1, "outline", 99),
        Step {
            id: None,
            more: None
        }
    );
    assert_eq!(s(&v.selection(1)), Some("file:x"), "out of range: no move");
    assert_eq!(v.walk(1, "missing", 1), w("file:x"));
}

#[test]
fn windowed_edge_answers_more() {
    let desc = json!({"scheme": "list", "lists": ["hits"], "classes": ["selected"]});
    let nodes = (0..10)
        .map(|i| node(&format!("m{i}"), "match", json!({})))
        .collect();
    let hits = json!({"name": "hits", "ids": (3..6).map(|i| format!("m{i}")).collect::<Vec<_>>(), "total": 10, "from": 3});
    let mut v = view_with(&reset(desc, nodes, vec![], vec![hits]));
    v.select(1, Some("m5"));
    assert_eq!(
        v.walk(1, "hits", 1),
        Step {
            id: Some("m5".into()),
            more: Some(1)
        }
    );
    v.select(1, Some("m3"));
    assert_eq!(
        v.walk(1, "hits", -1),
        Step {
            id: Some("m3".into()),
            more: Some(-1)
        }
    );
    assert_eq!(
        v.walk(1, "hits", 1),
        Step {
            id: Some("m4".into()),
            more: None
        }
    );
    assert_eq!(
        v.walk_to(1, "hits", 8),
        Step {
            id: Some("m4".into()),
            more: Some(1)
        }
    );
    assert_eq!(
        v.walk_to(1, "hits", 1),
        Step {
            id: Some("m4".into()),
            more: Some(-1)
        }
    );
    assert_eq!(
        v.walk_to(1, "hits", 10),
        Step {
            id: None,
            more: None
        }
    );
}

#[test]
fn removal_follows_then() {
    let mut v = view_with(&small_map());
    v.select(1, Some("file:x"));
    let mut x = node("file:z", "file", json!({}));
    x["then"] = json!("agent:a");
    let p = ops(vec![json!({"nodes": [x]})]);
    assert_eq!(v.apply("h1", 2, &p, BIG), Status::Applied);
    let mut fx = node("file:x", "file", json!({}));
    fx["then"] = json!("agent:a");
    assert_eq!(
        v.apply("h1", 3, &ops(vec![json!({"nodes": [fx]})]), BIG),
        Status::Applied
    );
    assert_eq!(
        v.apply("h1", 4, &ops(vec![json!({"remove": ["file:x"]})]), BIG),
        Status::Applied
    );
    assert_eq!(s(&v.selection(1)), Some("agent:a"));
    assert_eq!(s(&v.take_finished(1)), Some("file:x"));
    assert_eq!(v.take_finished(1), None, "read once");
    // a node that goes with no then: nil (a links view has no index fallback)
    v.select(1, Some("agent:c"));
    assert_eq!(
        v.apply("h1", 5, &ops(vec![json!({"remove": ["agent:c"]})]), BIG),
        Status::Applied
    );
    assert_eq!(v.selection(1), None);
    assert_eq!(s(&v.take_finished(1)), Some("agent:c"));
}

#[test]
fn removal_without_then_uses_the_index_in_a_list() {
    let desc = json!({"scheme": "list", "lists": ["all"], "classes": ["selected"]});
    let nodes = (0..5)
        .map(|i| node(&format!("e{i}"), "entry", json!({})))
        .collect();
    let all = |ids: Vec<&str>| json!({"name": "all", "ids": ids});
    let mut v = view_with(&reset(
        desc,
        nodes,
        vec![],
        vec![all(vec!["e0", "e1", "e2", "e3", "e4"])],
    ));
    v.select(1, Some("e2"));
    let p = ops(vec![
        json!({"remove": ["e2"]}),
        json!({"list": all(vec!["e0", "e1", "e3", "e4"])}),
    ]);
    assert_eq!(v.apply("h1", 2, &p, BIG), Status::Applied);
    assert_eq!(s(&v.selection(1)), Some("e3"), "the entry now at its index");
    v.select(1, Some("e4"));
    let p = ops(vec![
        json!({"remove": ["e4"]}),
        json!({"list": all(vec!["e0", "e1", "e3"])}),
    ]);
    assert_eq!(v.apply("h1", 3, &p, BIG), Status::Applied);
    assert_eq!(s(&v.selection(1)), Some("e3"), "clamped to the end");
}

#[test]
fn empty_view_answers_nil() {
    let mut v = View::default();
    v.open(1, None);
    assert_eq!(v.follow(1, "up"), None);
    assert_eq!(
        v.walk(1, "team", 1),
        Step {
            id: None,
            more: None
        }
    );
    assert_eq!(v.select(1, Some("x")), None);
    assert!(v.classes(1).is_empty());
    assert_eq!(v.kind(1, None), None);
    // and an unknown handle
    let mut v = view_with(&small_map());
    assert_eq!(v.follow(9, "up"), None);
}

#[test]
fn chain_class_is_reachability() {
    let mut v = view_with(&small_map());
    v.select(1, Some("agent:a"));
    let c = |v: &mut View, id: &str| v.class_of(1, id);
    // classes: selected 0, chain 1, dim 2
    assert_eq!(c(&mut v, "agent:a"), Some(0));
    for on in ["ask:1", "plan:1", "file:x"] {
        assert_eq!(c(&mut v, on), Some(1), "{on} is on the chain");
    }
    // agent:b is a sibling (plan:1's other child), agent:c only overlaps: not chain kinds
    for off in ["agent:b", "file:y", "agent:c"] {
        assert_eq!(c(&mut v, off), Some(2), "{off} is dimmed");
    }
}

#[test]
fn search_matches_fields_case_folded() {
    let mut v = view_with(&small_map());
    v.select(1, Some("agent:a"));
    v.set_search(1, Some("FILE:"));
    // classes: selected 0, dim 2, match 3
    assert_eq!(v.class_of(1, "file:x"), Some(3));
    assert_eq!(v.class_of(1, "file:y"), Some(3));
    assert_eq!(v.class_of(1, "plan:1"), Some(2));
    assert_eq!(
        v.class_of(1, "agent:a"),
        Some(0),
        "the selection stays selected"
    );
    v.set_search(1, Some(""));
    assert_eq!(v.class_of(1, "plan:1"), Some(1), "an empty search is none");
}

#[test]
fn lit_list_lights_its_nodes_without_a_selection() {
    let mut v = view_with(&small_map());
    v.set_lit(1, Some("needs-you"));
    assert_eq!(v.class_of(1, "agent:b"), Some(4));
    assert_eq!(v.class_of(1, "agent:a"), None);
}

#[test]
fn classes_answer_only_changes() {
    let mut v = view_with(&small_map());
    assert!(v.classes(1).is_empty(), "no selection: nothing has a class");
    v.select(1, Some("agent:a"));
    let first = v.classes(1);
    assert_eq!(first.len(), 7, "every node gets one");
    assert!(v.classes(1).is_empty(), "nothing changed since");
    v.select(1, Some("agent:b"));
    let ch: Vec<(String, Option<u8>)> = v
        .classes(1)
        .into_iter()
        .map(|(i, k)| (i.to_string(), k))
        .collect();
    assert_eq!(
        ch,
        vec![
            ("agent:a".into(), Some(2)),
            ("agent:b".into(), Some(0)),
            ("file:x".into(), Some(2)),
            ("file:y".into(), Some(1)),
        ]
    );
    v.select(1, None);
    assert!(
        v.classes(1).iter().all(|(_, k)| k.is_none()),
        "cleared: every class goes"
    );
    v.reclass(1);
    assert!(v.classes(1).is_empty());
}

#[test]
fn group_shares_selection() {
    let mut v = View::default();
    v.open(1, Some("map"));
    v.open(2, Some("map"));
    v.open(3, None);
    assert_eq!(v.apply("h1", 1, &small_map(), BIG), Status::Applied);
    v.select(1, Some("agent:b"));
    assert_eq!(s(&v.selection(2)), Some("agent:b"));
    assert_eq!(v.selection(3), None);
    assert!(!v.close(1));
    assert_eq!(
        s(&v.selection(2)),
        Some("agent:b"),
        "the group outlives one member"
    );
}

#[test]
fn fields_flags_kind_and_rows_read_the_selection() {
    let mut n = node("agent:a", "agent", json!({}));
    n["fields"] = json!([["label", "Fix the parser"], ["minutes", 12], ["note", null]]);
    n["flags"] = json!(2);
    n["parent"] = json!("plan:1");
    let mut v = view_with(&small_map());
    assert_eq!(
        v.apply("h1", 2, &ops(vec![json!({"nodes": [n]})]), BIG),
        Status::Applied
    );
    v.select(1, Some("agent:a"));
    assert_eq!(v.kind(1, None), Some("agent"));
    assert_eq!(v.field(1, "minutes", None), Some(&sja::wire::Val::Int(12)));
    assert_eq!(v.field(1, "note", None), None);
    assert_eq!(
        v.field(1, "label", Some("plan:1")),
        Some(&sja::wire::Val::Str("PLAN:1".into()))
    );
    assert!(v.flag(1, "needs_you", None));
    assert!(!v.flag(1, "live", None));
    assert_eq!(s(&v.parent(1, None)), Some("plan:1"));
    assert_eq!(v.rows(1, Some("file:x")), vec!["r-file:x".into()]);
}

#[test]
fn fold_and_tick_toggle() {
    let mut v = view_with(&small_map());
    assert!(v.toggle(1, "file:x", false));
    assert!(v.toggle(1, "file:x", true));
    assert!(!v.toggle(1, "file:x", false));
    assert!(v.inst(1).unwrap().ticked.contains("file:x"));
}

#[test]
fn ticked_class_marks_the_ticked_set_when_named() {
    // a descriptor naming `ticked` (the Explorer's marks): selected 0, ticked 1
    let desc = json!({"scheme": "list", "lists": ["all"], "classes": ["selected", "ticked"], "start": "all"});
    let nodes = ["a", "b", "c"]
        .map(|i| json!({"id": i, "kind": "file"}))
        .to_vec();
    let mut v = view_with(&reset(
        desc,
        nodes,
        vec![],
        vec![json!({"name": "all", "ids": ["a", "b", "c"]})],
    ));
    v.toggle(1, "b", true);
    v.toggle(1, "c", true);
    v.select(1, Some("c"));
    assert_eq!(v.class_of(1, "a"), None);
    assert_eq!(v.class_of(1, "b"), Some(1));
    assert_eq!(
        v.class_of(1, "c"),
        Some(0),
        "the selection wins over a mark"
    );
    // a descriptor that doesn't name it (Search's ticks): the set is no class
    let mut w = view_with(&small_map());
    w.toggle(1, "agent:c", true);
    assert_eq!(w.class_of(1, "agent:c"), None);
}

/// A trail-like window: rows s3..s5 of ten, linked up and down (a conclusion after s5), the list `steps` from 3.
fn trail_window() -> String {
    let desc =
        json!({"scheme": "links", "lists": ["steps"], "classes": ["selected"], "start": "steps"});
    let order = ["s3", "s4", "s5", "c1"];
    let nodes = order
        .iter()
        .enumerate()
        .map(|(i, id)| {
            let mut nav = serde_json::Map::new();
            if i > 0 {
                nav.insert("up".into(), json!(order[i - 1]));
            }
            if i + 1 < order.len() {
                nav.insert("down".into(), json!(order[i + 1]));
            }
            node(id, "step", json!(nav))
        })
        .collect();
    let steps = json!({"name": "steps", "ids": ["s3", "s4", "s5"], "total": 10, "from": 3});
    reset(desc, nodes, vec![], vec![steps])
}

#[test]
fn follow_off_a_windows_edge_answers_more() {
    let mut v = view_with(&trail_window());
    v.select(1, Some("s3"));
    assert_eq!(
        v.follow_step(1, "up"),
        Step {
            id: Some("s3".into()),
            more: Some(-1)
        },
        "no row above, and the list goes on above: more"
    );
    assert_eq!(
        v.follow_step(1, "down"),
        Step {
            id: Some("s4".into()),
            more: None
        }
    );
    v.select(1, Some("c1"));
    assert_eq!(
        v.follow_step(1, "down"),
        Step {
            id: Some("c1".into()),
            more: Some(1)
        },
        "the last row (a conclusion, in no list): more below"
    );
    assert_eq!(
        v.follow_step(1, "nowhere"),
        Step {
            id: Some("c1".into()),
            more: None
        },
        "a link with no direction never pages"
    );
    assert_eq!(
        s(&v.follow(1, "down")),
        Some("c1"),
        "follow alone: the selection"
    );
}

#[test]
fn follow_in_a_whole_list_never_answers_more() {
    let mut v = view_with(&small_map());
    v.select(1, Some("ask:1"));
    assert_eq!(v.follow_step(1, "up").more, None);
}

#[test]
fn a_tree_pages_at_its_visible_edge() {
    let desc = json!({"scheme": "tree", "lists": ["order"], "classes": ["selected"], "flags": ["expanded"], "start": "order"});
    let nodes = ["a", "b", "c"]
        .iter()
        .map(|id| json!({"id": id, "kind": "file", "fields": [["label", id]]}))
        .collect();
    let order = json!({"name": "order", "ids": ["a", "b", "c"], "total": 9, "from": 0});
    let mut v = view_with(&reset(desc, nodes, vec![], vec![order]));
    v.select(1, Some("c"));
    assert_eq!(v.follow_step(1, "down").more, Some(1));
    assert_eq!(
        v.follow_step(1, "down:file").more,
        Some(1),
        "no match below: the next window may hold one"
    );
    v.select(1, Some("a"));
    assert_eq!(
        v.follow_step(1, "up").more,
        None,
        "the window starts the list"
    );
    assert_eq!(
        v.follow_step(1, "left").more,
        None,
        "left is the parent, not a page"
    );
}

#[test]
fn window_tells_from_length_and_total() {
    let v = view_with(&trail_window());
    assert_eq!(v.window("steps"), Some((3, 3, 10)));
    assert_eq!(view_with(&small_map()).window("team"), None, "a whole list");
    assert_eq!(v.window("missing"), None);
}

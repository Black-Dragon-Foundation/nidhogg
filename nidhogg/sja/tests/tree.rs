//! The tree scheme on a small tree (pattern D, pure: no clock, no threads): the visible list, expansion, moves and
//! type-to-navigate over the visible siblings.

mod common;
use common::*;
use serde_json::{Value, json};
use sja::View;

fn tree_desc() -> Value {
    json!({
        "scheme": "tree",
        "chain_kinds": [],
        "match_fields": ["label"],
        "lists": ["order"],
        "classes": ["selected", "match", "dim"],
        "flags": ["expanded"],
        "forms": [],
        "start": "order"
    })
}

fn tnode(id: &str, kind: &str, parent: Option<&str>, label: &str, open: bool) -> Value {
    let mut n =
        json!({"id": id, "kind": kind, "flags": u32::from(open), "fields": [["label", label]]});
    if let Some(p) = parent {
        n["parent"] = json!(p);
    }
    n
}

/// a/ (open: x, y), b/ (closed: z), c.
fn small_tree() -> String {
    reset(
        tree_desc(),
        vec![
            tnode("a", "dir", None, "alpha", true),
            tnode("a/x", "file", Some("a"), "xray", false),
            tnode("a/y", "file", Some("a"), "yankee", false),
            tnode("b", "dir", None, "bravo", false),
            tnode("b/z", "file", Some("b"), "zulu", false),
            tnode("c", "file", None, "charlie", false),
        ],
        vec![],
        vec![json!({"name": "order", "ids": ["a", "a/x", "a/y", "b", "b/z", "c"]})],
    )
}

fn visible(v: &View) -> Vec<String> {
    v.visible(1).iter().map(|i| i.to_string()).collect()
}

#[test]
fn tree_visible_follows_expansion() {
    let mut v = view_with(&small_tree());
    assert_eq!(visible(&v), ["a", "a/x", "a/y", "b", "c"]);
    assert!(v.is_open(1, "a"), "the model's flag");
    assert!(!v.is_open(1, "b"));
    v.toggle(1, "b", false);
    assert!(v.is_open(1, "b"));
    assert_eq!(visible(&v), ["a", "a/x", "a/y", "b", "b/z", "c"]);
    v.toggle(1, "a", false);
    assert!(
        !v.is_open(1, "a"),
        "the user's toggle flips the model's flag"
    );
    assert_eq!(visible(&v), ["a", "b", "b/z", "c"]);
    assert_eq!(
        v.list_of(1, "visible").len(),
        4,
        "the list `visible` by name"
    );
    // another instance keeps its own expansion
    v.open(2, None);
    assert_eq!(v.visible(2).len(), 5);
}

#[test]
fn tree_moves_walk_the_visible_list() {
    let mut v = view_with(&small_tree());
    assert_eq!(
        s(&v.follow(1, "down")),
        Some("a"),
        "from none: the first entry"
    );
    assert_eq!(s(&v.follow(1, "down")), Some("a/x"));
    assert_eq!(s(&v.follow(1, "down")), Some("a/y"));
    assert_eq!(
        s(&v.follow(1, "down")),
        Some("b"),
        "b's closed child is skipped"
    );
    assert_eq!(s(&v.follow(1, "down")), Some("c"));
    assert_eq!(s(&v.follow(1, "down")), Some("c"), "no wrap");
    assert_eq!(s(&v.follow(1, "up")), Some("b"));
    assert_eq!(
        s(&v.follow(1, "right")),
        Some("b"),
        "closed: no child to go to"
    );
    v.toggle(1, "b", false);
    assert_eq!(s(&v.follow(1, "right")), Some("b/z"));
    assert_eq!(s(&v.follow(1, "left")), Some("b"), "left: the parent");
    assert_eq!(s(&v.follow(1, "left")), Some("b"), "a root has none");
    v.select(1, Some("a"));
    assert_eq!(s(&v.follow(1, "down:dir")), Some("b"), "the next of a kind");
    assert_eq!(s(&v.follow(1, "down:dir")), Some("b"), "none after: stays");
    v.select(1, Some("c"));
    assert_eq!(s(&v.follow(1, "up:file")), Some("b/z"));
    assert_eq!(s(&v.walk(1, "visible", 1).id), Some("c"));
}

#[test]
fn tree_prefix_matches_visible_siblings() {
    let mut v = view_with(&small_tree());
    assert_eq!(s(&v.prefix(1, "b")), Some("b"), "from none: the top level");
    assert_eq!(s(&v.prefix(1, "C")), Some("c"), "case folded");
    assert_eq!(
        s(&v.prefix(1, "z")),
        Some("c"),
        "zulu is no sibling (and hidden): stays"
    );
    v.select(1, Some("a/x"));
    assert_eq!(s(&v.prefix(1, "y")), Some("a/y"));
    assert_eq!(
        s(&v.prefix(1, "x")),
        Some("a/x"),
        "after the selection, wrapping"
    );
    assert_eq!(
        s(&v.prefix(1, "xr")),
        Some("a/x"),
        "a longer prefix starts at the selection"
    );
    v.select(1, Some("a"));
    assert_eq!(
        s(&v.prefix(1, "a")),
        Some("a"),
        "the only match is the selection"
    );
}

//! The typed decode reads a model straight into ops, with no `Value` between; it must give exactly what the `Value`
//! decode gives.  Whatever it can't read (a malformed or unusual model) falls back to the `Value` decode, so errors
//! and their messages are the `Value` decode's.

use serde_json::{Value, json};
use sja::queue::{read_line, read_line_typed, read_line_value};
use sja::wire::{decode, decode_typed, decode_value};

/// The `Value` decode of `text`, the reference.
fn reference(text: &str) -> Result<Vec<sja::wire::Op>, sja::wire::Bad> {
    match serde_json::from_str::<Value>(text) {
        Ok(v) => decode_value(&v),
        Err(e) => Err(sja::wire::Bad(format!("json: {e}"))),
    }
}

/// `text` reads typed, and the same as the reference.
fn same_typed(text: &str) {
    let want = reference(text).unwrap_or_else(|e| panic!("the reference refused {text}: {e:?}"));
    assert_eq!(decode_typed(text), Some(want.clone()), "typed: {text}");
    assert_eq!(decode(text), Ok(want), "decode: {text}");
}

/// `text` falls back: the typed decode reads nothing, and `decode` answers the reference's answer.
fn falls_back(text: &str) {
    assert_eq!(decode_typed(text), None, "typed read {text}");
    assert_eq!(decode(text), reference(text), "decode: {text}");
}

fn big_cut() -> String {
    // past MAX_VALUE, with a multibyte character over the cut
    format!("{}é{}", "x".repeat(4095), "y".repeat(10))
}

#[test]
fn every_op_reads_typed_as_the_value_decode_reads_it() {
    let reset = json!({"ops": [{"reset": {
        "descriptor": {"scheme": "tree", "chain_kinds": ["a"], "match_fields": ["label"], "lists": ["order"],
                       "classes": ["selected", "dim"], "flags": ["expanded"],
                       "forms": [{"name": "grid"}, {"name": "col", "list": "order"}, {"name": "x", "list": null}],
                       "start": "order", "later": [1, {"x": 2}]},
        "nodes": [
            {"id": "a", "kind": "file", "parent": null, "flags": 3,
             "nav": {"up": "b", "down": null, "left": "c"}, "navs": {"grid": {"z": "a", "y": null}, "col": {}},
             "fields": [["label", "A\n\"q\" \u{e9}"], ["n", 12], ["neg", -4], ["t", true], ["f", false], ["skip", null],
                        ["big", big_cut()]],
             "rows": ["r1", 2], "then": "b", "unknown": {"deep": [null]}},
            {"id": 7, "kind": "match", "flags": null, "nav": null, "navs": null, "fields": null, "rows": null,
             "then": null},
        ],
        "edges": [["a", "b", "calls"], [1, 2, "k"]],
        "lists": [{"name": "order", "ids": ["a", 7], "total": 10, "from": 2}, {"name": "x", "ids": null, "total": null}],
        "extra": true}}],
        "seq": 4});
    same_typed(&reset.to_string());
    same_typed(&json!({"ops": [{"reset": {"descriptor": {"scheme": "links"}}}]}).to_string());
    same_typed(
        &json!({"ops": [{"reset": {"descriptor": {"scheme": "list"}, "nodes": null, "edges": null,
                                          "lists": null}}]})
        .to_string(),
    );
    let diffs = json!({"ops": [
        {"nodes": [{"id": "a", "kind": "k"}]},
        {"remove": ["a", 3]},
        {"remove": null},
        {"edges": {"add": [["a", "b", "k"]], "remove": [["b", "c", "k"]]}},
        {"edges": {}},
        {"list": {"name": "order", "ids": ["x"]}},
        {"splice": {"name": "order", "at": 3, "remove": 1, "ids": ["y", "z"]}},
        {"splice": {"name": "order"}},
    ]});
    same_typed(&diffs.to_string());
    same_typed(r#"{"ops": []}"#);
    // whitespace, and escapes in keys and strings
    same_typed("{ \"ops\" : [ { \"remove\" : [ \"a\\u0062\" , \"\\ud83d\\ude00\" ] } ] }");
    same_typed(r#"{"ops":[{"nodes":[{"id":"a","kind":"k"}]}]}"#);
}

#[test]
fn the_hub_fixture_reads_typed() {
    let fixture: Value = serde_json::from_str(include_str!("fixtures/grep-append.json")).unwrap();
    for step in fixture["steps"].as_array().unwrap() {
        same_typed(&step["model"].to_string());
    }
}

#[test]
fn a_malformed_or_unusual_model_falls_back_to_the_value_decode() {
    for text in [
        // not JSON, or not a patch
        "",
        "{",
        "[]",
        r#"{"ops": {}}"#,
        r#"{"op": []}"#,
        r#"{"ops": [1]}"#,
        r#"{"ops": []} trailing"#,
        // an op of two keys, or none, or unknown
        r#"{"ops": [{"remove": [], "nodes": []}]}"#,
        r#"{"ops": [{}]}"#,
        r#"{"ops": [{"rename": []}]}"#,
        // a duplicate key: the Value decode keeps the last
        r#"{"ops": [], "ops": [{"remove": ["a"]}]}"#,
        r#"{"ops": [{"nodes": [{"id": "a", "id": "b", "kind": "k"}]}]}"#,
        r#"{"ops": [{"nodes": [{"id": "a", "kind": "k", "nav": {"up": "x", "up": "y"}}]}]}"#,
        // numbers that are not counts, ids or integers
        r#"{"ops": [{"splice": {"name": "o", "at": -1}}]}"#,
        r#"{"ops": [{"splice": {"name": "o", "at": 1.0}}]}"#,
        r#"{"ops": [{"remove": [1.5]}]}"#,
        r#"{"ops": [{"nodes": [{"id": "a", "kind": "k", "flags": 4294967296}]}]}"#,
        r#"{"ops": [{"nodes": [{"id": "a", "kind": "k", "fields": [["n", 9223372036854775808]]}]}]}"#,
        r#"{"ops": [{"nodes": [{"id": "a", "kind": "k", "fields": [["n", 0.5]]}]}]}"#,
        // shapes
        r#"{"ops": [{"nodes": [{"kind": "k"}]}]}"#,
        r#"{"ops": [{"nodes": [{"id": "a", "kind": "k", "fields": [["n"]]}]}]}"#,
        r#"{"ops": [{"nodes": [{"id": "a", "kind": "k", "fields": [["n", [1]]]}]}]}"#,
        r#"{"ops": [{"edges": {"add": [["a", "b"]]}}]}"#,
        r#"{"ops": [{"nodes": null}]}"#,
        r#"{"ops": [{"list": {"ids": []}}]}"#,
        r#"{"ops": [{"reset": {"descriptor": {"scheme": "grid"}}}]}"#,
        r#"{"ops": [{"reset": {}}]}"#,
        r#"{"ops": [{"reset": {"descriptor": {"scheme": "list", "forms": [{"list": "o"}]}}}]}"#,
    ] {
        falls_back(text);
    }
}

fn line(model: &str, rest: &str) -> String {
    format!(
        r#"{{"event":"view-model","data":{{"base":3,"hub":1,"model":{model}{rest},"version":4,"view":"v"}}}}"#
    )
}

#[test]
fn a_line_reads_typed_as_the_value_read_reads_it() {
    let model = r#"{"ops":[{"remove":["a"]}]}"#;
    for text in [
        line(model, ""),
        line(model, r#","reset":true"#),
        line(model, r#","reset":false,"x":[1]"#),
    ] {
        let (e, ops) = read_line_value(&text).unwrap();
        assert_eq!(
            read_line_typed(&text),
            Some((e.clone(), ops.clone().unwrap())),
            "{text}"
        );
        assert_eq!(read_line(&text), Some((e, ops)), "{text}");
    }
    // the event first or last, other keys around it
    let t = r#"{"data":{"view":"v","version":2,"base":1,"hub":9,"model":{"ops":[]}},"seq":1,"event":"view-model"}"#;
    assert!(read_line_typed(t).is_some());
    assert_eq!(read_line(t), read_line_value(t));
}

#[test]
fn a_line_the_typed_read_cant_read_falls_back() {
    for text in [
        line(r#"{"ops":[{"remove":[1.5]}]}"#, ""),
        line(r#"{"ops":[]}"#, r#","reset":1"#),
        line(r#"{"ops":[]}"#, r#","view":"w""#),
        r#"{"event":"view-model","data":{"base":0,"hub":1,"version":1,"view":"v"}}"#.into(),
        r#"{"event":"view-model","data":{"base":-1,"hub":1,"model":{"ops":[]},"version":1,"view":"v"}}"#.into(),
        r#"{"event":"view-model","data":{"base":0,"hub":1,"model":{"ops":[]},"version":1,"view":3}}"#.into(),
        r#"{"event":"rows","data":{}}"#.into(),
        r#"{"event":"view-model"}"#.into(),
        "not json".into(),
    ] {
        assert_eq!(read_line_typed(&text), None, "typed read {text}");
        assert_eq!(read_line(&text), read_line_value(&text), "{text}");
    }
}

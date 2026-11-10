//! The decode's share of a push and drain at 10k nodes, as a Search model of 10,000 matches (an ignored test that prints;
//! run with `cargo test --release --test decode_bench -- --ignored --nocapture`).  It reads where the time of a
//! `view-model` line goes: the JSON parse, the typed ops, the apply.

use serde_json::{Value, json};
use sja::queue::{read_line, read_line_typed, read_line_value};
use sja::wire::decode_value;
use sja::{Status, View};
use std::time::Instant;

/// A Search model as the hub sends it: 500 files open, 20 matches each, as a `tree`.
fn grep_line(files: usize, per: usize) -> String {
    let mut nodes = Vec::new();
    let mut order = Vec::new();
    for f in 0..files {
        let file = format!("src/module_{f:04}/some/longer/path/file_{f}.rs");
        let fid = format!("f:{file}");
        order.push(fid.clone());
        nodes.push(json!({"id": fid, "kind": "file", "flags": 1, "fields": [["file", file]], "rows": [format!("h{f}")]}));
        for m in 0..per {
            let id = format!("m:{file}:{}:{}", m * 7 + 1, 4);
            order.push(id.clone());
            nodes.push(json!({"id": id, "kind": "match", "parent": fid,
                "fields": [["file", file], ["line", m * 7 + 1], ["col", 4], ["end", 12],
                           ["text", format!("    let value_{m} = compute(\"needle\", {m}); // a line of code")]],
                "rows": [format!("r{f}-{m}")]}));
        }
    }
    let desc = json!({"scheme": "tree", "lists": ["order"], "classes": ["selected", "match", "dim"],
                      "flags": ["expanded"], "match_fields": ["text"], "start": "order"});
    let model = json!({"ops": [{"reset": {"descriptor": desc, "nodes": nodes, "lists": [{"name": "order", "ids": order}]}}]});
    json!({"event": "view-model", "data": {"base": 0, "hub": 1, "model": model, "reset": true, "version": 1, "view": "bench"}})
        .to_string()
}

fn best<T>(n: usize, mut f: impl FnMut() -> T) -> f64 {
    let mut best = f64::MAX;
    for _ in 0..n {
        let t = Instant::now();
        std::hint::black_box(f());
        best = best.min(t.elapsed().as_secs_f64() * 1000.0);
    }
    best
}

#[test]
#[ignore]
fn where_a_push_spends_its_time() {
    let line = grep_line(500, 20);
    let parse = best(10, || serde_json::from_str::<Value>(&line).unwrap());
    let v: Value = serde_json::from_str(&line).unwrap();
    let ops = best(10, || decode_value(&v["data"]["model"]).unwrap());
    let read = best(10, || read_line_value(&line).unwrap());
    let typed = best(10, || read_line_typed(&line).unwrap());
    assert!(matches!(read_line(&line), Some((_, Ok(_)))));
    let decoded = decode_value(&v["data"]["model"]).unwrap();
    let apply = best(10, || {
        let mut view = View::default();
        view.open(1, None);
        assert_eq!(
            view.apply_ops("1/1", 1, &decoded, 64 << 20),
            Status::Applied
        );
        view
    });
    println!(
        "DECODE-BENCH bytes {} parse {parse:.2} ms, typed ops {ops:.2} ms, read_line through Value {read:.2} ms, \
         typed read {typed:.2} ms ({:.2}x), apply {apply:.2} ms",
        line.len(),
        read / typed
    );
}

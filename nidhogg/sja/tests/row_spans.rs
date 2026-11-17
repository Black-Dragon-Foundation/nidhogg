//! A node's whole rows (`rowspans`): what a selection's row block covers.

use sja::abi::*;
use std::sync::Mutex;

static EMACS: Mutex<()> = Mutex::new(());

fn ok(op: i32, args: &[SjaArg]) -> Ans {
    call_for_test(op, args).unwrap_or_else(|rc| panic!("op {op}: code {rc}"))
}

fn rows(h: i64, what: &str, a: SjaArg, b: SjaArg) -> Ans {
    ok(SJA_ROWS, &[int(h), text(what), a, b])
}

/// A row of three runs, two of them boxed (a chip and a button), holding node n1 over part of its text, and a second
/// row of n1 below another node's: one pair per row of n1, from the row's first character to past its newline, in
/// buffer order; another node's row is not in it.
#[test]
fn row_spans_cover_the_whole_row_once() {
    let _l = EMACS.lock().unwrap_or_else(|e| e.into_inner());
    let Ans::Int(h) = ok(SJA_OPEN, &[text("row-spans"), nil()]) else {
        panic!("open")
    };
    let line = serde_json::json!({"event": "view-model", "data": {"view": "row-spans", "rows": {
        "view": "row-spans", "class": "c", "hub": 5, "base": 0, "version": 1, "env_epoch": 0,
        "styles": [[3, {"box": {"line": 1}}], [4, {"box": {"line": 1}}]],
        "ops": [{"reset": [
            {"key": "a", "text": "name chip Accept",
             "runs": [[0, 4, "drgnx-ui-text"], [5, 9, "s:3"], [10, 16, "s:4"]],
             "props": [[0, 9, "drgnx-work-id", "n1"]]},
            {"key": "b", "text": "other", "props": [[0, 5, "drgnx-work-id", "n2"]]},
            {"key": "c", "text": "more", "props": [[1, 3, "drgnx-work-id", "n1"]]}]}]}}})
    .to_string();
    rows(h, "on", int(1), text("c"));
    rows(h, "idprop", nil(), text("drgnx-work-id"));
    ok(SJA_PUSH, &[text(&line)]);
    let Ans::List(d) = rows(h, "draw", int(0), nil()) else {
        panic!("no draw")
    };
    let Ans::Int(id) = d[0] else {
        panic!("no draw id")
    };
    rows(h, "drawn", int(id), nil());
    assert_eq!(
        rows(h, "rowspans", nil(), text("drgnx-work-id\tn1")),
        Ans::Vector(vec![Ans::Int(0), Ans::Int(17), Ans::Int(23), Ans::Int(28)])
    );
    assert_eq!(
        rows(h, "rowspans", nil(), text("drgnx-work-id\tn2")),
        Ans::Vector(vec![Ans::Int(17), Ans::Int(23)])
    );
    assert_eq!(
        rows(h, "rowspans", nil(), text("drgnx-work-id\tnone")),
        Ans::Vector(vec![])
    );
    ok(SJA_CLOSE, &[int(h)]);
}

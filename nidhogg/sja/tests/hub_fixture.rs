//! A model stream as the hub writes it, loaded by Sjá: the file is the same JSON as drgnx's
//! `runtime/crates/drgnx-ratatoskr/tests/fixtures/grep-append.json` (written by the hub's test of the same name).
//! A list that grew by appending arrives as a `splice` at its end, which Sjá applies with no new op.

mod common;
use common::BIG;
use serde_json::Value;
use sja::{Status, View};

#[test]
fn a_list_the_hub_grew_by_appending_loads_as_splices() {
    let fixture: Value = serde_json::from_str(include_str!("fixtures/grep-append.json")).unwrap();
    let mut v = View::default();
    v.open(1, None);
    for step in fixture["steps"].as_array().unwrap() {
        let version = step["version"].as_u64().unwrap();
        let model = step["model"].as_str().unwrap();
        assert_eq!(
            v.apply("h1", version, model, BIG),
            Status::Applied,
            "version {version}"
        );
    }
    let want: Vec<&str> = fixture["ids"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i.as_str().unwrap())
        .collect();
    let have = v.list_of(1, fixture["list"].as_str().unwrap());
    assert_eq!(have.iter().map(|i| &**i).collect::<Vec<_>>(), want);
}

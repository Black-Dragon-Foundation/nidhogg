//! Property rows on a seeded generator (pattern A, every order: the seeds pick the interleavings; no clock).

mod common;
use common::*;
use serde_json::json;
use sja::{Status, View};

const SEEDS: u64 = 64;
const STEPS: usize = 120;

/// A view that sees every patch and a view that misses some (each miss answered by a resync and the hub's reset) end
/// with the same model and the same selection, whatever order the verbs came in between.
#[test]
fn every_order_converges() {
    for seed in 1..=SEEDS {
        let mut rng = Rng::new(seed);
        let mut hub = Hub::new();
        let (mut a, mut b) = (View::default(), View::default());
        a.open(1, None);
        b.open(1, None);
        let first = hub.snapshot();
        assert_eq!(a.apply("h", 1, &first, BIG), Status::Applied);
        assert_eq!(b.apply("h", 1, &first, BIG), Status::Applied);
        let mut missed = false;
        for _ in 0..STEPS {
            if rng.below(3) == 0 {
                // the same key on both
                let ids = a.list("all");
                let pick = ids[rng.below(ids.len())].to_string();
                a.select(1, Some(&pick));
                if b.model
                    .as_ref()
                    .is_some_and(|m| m.nodes.contains_key(pick.as_str()))
                {
                    b.select(1, Some(&pick));
                } else {
                    // b can't select what it doesn't hold yet: catch up first, then the same key
                    assert_eq!(
                        b.apply("h", hub.version, &hub.snapshot(), BIG),
                        Status::Applied
                    );
                    missed = false;
                    b.select(1, Some(&pick));
                }
                continue;
            }
            let p = hub.step(&mut rng);
            assert_eq!(
                a.apply("h", hub.version, &p, BIG),
                Status::Applied,
                "seed {seed}"
            );
            if rng.below(4) == 0 {
                missed = true;
                continue;
            }
            if missed {
                // a gap: the view asks for a reset and gets the hub's state
                assert_eq!(
                    b.apply("h", hub.version, &hub.snapshot(), BIG),
                    Status::Applied
                );
                missed = false;
            } else {
                assert_eq!(
                    b.apply("h", hub.version, &p, BIG),
                    Status::Applied,
                    "seed {seed}"
                );
            }
        }
        if missed {
            assert_eq!(
                b.apply("h", hub.version, &hub.snapshot(), BIG),
                Status::Applied
            );
        }
        assert_eq!(snap(&a), snap(&b), "seed {seed}: models");
        assert_eq!(a.selection(1), b.selection(1), "seed {seed}: selections");
    }
}

/// After any step the raw selection is a node of the model, or none.
#[test]
fn selection_always_exists_or_none() {
    for seed in 1..=SEEDS {
        let mut rng = Rng::new(seed);
        let mut hub = Hub::new();
        let mut v = View::default();
        v.open(1, Some("g"));
        v.open(2, None);
        assert_eq!(v.apply("h", 1, &hub.snapshot(), BIG), Status::Applied);
        for _ in 0..STEPS {
            match rng.below(4) {
                0 => {
                    let ids = v.list("all");
                    let pick = ids[rng.below(ids.len())].to_string();
                    v.select(1 + rng.below(2) as u32, Some(&pick));
                }
                1 => {
                    v.walk(2, "all", rng.below(5) as i64 - 2);
                }
                2 => {
                    v.follow(1, ["left", "right"][rng.below(2)]);
                }
                _ => {
                    let p = hub.step(&mut rng);
                    assert_eq!(v.apply("h", hub.version, &p, BIG), Status::Applied);
                }
            }
            let m = v.model.as_ref().unwrap();
            for sel in v.sels.values() {
                if let Some(id) = &sel.id {
                    assert!(
                        m.nodes.contains_key(id),
                        "seed {seed}: selection {id} is not in the model"
                    );
                }
            }
        }
    }
}

/// The classes an instance accumulates from its change reports equal a fresh recompute, after any steps.
#[test]
fn classes_match_a_recompute() {
    for seed in 1..=SEEDS {
        let mut rng = Rng::new(seed);
        let mut hub = Hub::new();
        let mut v = View::default();
        v.open(1, None);
        assert_eq!(v.apply("h", 1, &hub.snapshot(), BIG), Status::Applied);
        let mut seen: std::collections::BTreeMap<String, u8> = Default::default();
        for _ in 0..STEPS {
            match rng.below(5) {
                0 => {
                    let ids = v.list("all");
                    let pick = ids[rng.below(ids.len())].to_string();
                    v.select(1, Some(&pick));
                }
                1 => v.set_search(1, [None, Some("n0"), Some("NODE R")][rng.below(3)]),
                2 => v.set_lit(1, [None, Some("all")][rng.below(2)]),
                _ => {
                    let p = hub.step(&mut rng);
                    assert_eq!(v.apply("h", hub.version, &p, BIG), Status::Applied);
                }
            }
            for (id, k) in v.classes(1) {
                match k {
                    Some(k) => seen.insert(id.to_string(), k),
                    None => seen.remove(&*id),
                };
            }
            // nodes that went take their classes with them (their rows go)
            let m = v.model.as_ref().unwrap();
            seen.retain(|id, _| m.nodes.contains_key(id.as_str()));
            let mut fresh: std::collections::BTreeMap<String, u8> = Default::default();
            let ids: Vec<String> = m.nodes.keys().map(|i| i.to_string()).collect();
            for id in ids {
                if let Some(k) = v.class_of(1, &id) {
                    fresh.insert(id, k);
                }
            }
            assert_eq!(seen, fresh, "seed {seed}");
        }
    }
}

/// A refused patch (malformed, a splice outside its list, over a cap) leaves the model and the selection as they were.
#[test]
fn apply_rejects_without_mutating() {
    for seed in 1..=SEEDS {
        let mut rng = Rng::new(seed);
        let mut hub = Hub::new();
        let mut v = View::default();
        v.open(1, None);
        assert_eq!(v.apply("h", 1, &hub.snapshot(), BIG), Status::Applied);
        for _ in 0..30 {
            let p = hub.step(&mut rng);
            assert_eq!(v.apply("h", hub.version, &p, BIG), Status::Applied);
        }
        let ids = v.list("all");
        v.select(1, Some(&ids[rng.below(ids.len())]));
        let (before, sel) = (snap(&v), v.selection(1));
        let good = hub.step(&mut rng);
        let cut = good.len() / 2 + rng.below(good.len() / 2);
        let bad_splice = ops(vec![
            json!({"remove": [ids[0].to_string()]}),
            json!({"splice": {"name": "all", "at": ids.len(), "remove": 1, "ids": []}}),
        ]);
        let (status, budget, text) = match rng.below(3) {
            0 => (Status::Resync, BIG, good[..cut].to_string()),
            1 => (Status::Resync, BIG, bad_splice),
            _ => (Status::TooLarge, v.bytes() / 2, good),
        };
        assert_eq!(
            v.apply("h", hub.version, &text, budget),
            status,
            "seed {seed}"
        );
        assert_eq!(snap(&v), before, "seed {seed}: the model changed");
        assert_eq!(v.selection(1), sel, "seed {seed}: the selection changed");
    }
}

/// A resync's reset keeps every instance's handle and selection by id.
#[test]
fn handles_survive_resync() {
    let mut rng = Rng::new(7);
    let mut hub = Hub::new();
    let mut v = View::default();
    for h in 1..=3 {
        v.open(h, None);
    }
    assert_eq!(v.apply("h", 1, &hub.snapshot(), BIG), Status::Applied);
    for _ in 0..20 {
        let p = hub.step(&mut rng);
        assert_eq!(v.apply("h", hub.version, &p, BIG), Status::Applied);
    }
    v.select(1, Some("r1"));
    v.walk(2, "all", 3);
    let before: Vec<_> = (1..=3).map(|h| v.selection(h)).collect();
    // the view dropped its model (as a panic does), then the reset arrives
    v.model = None;
    hub.version += 1;
    assert_eq!(
        v.apply("h", hub.version, &hub.snapshot(), BIG),
        Status::Applied
    );
    let after: Vec<_> = (1..=3).map(|h| v.selection(h)).collect();
    assert_eq!(before, after);
}

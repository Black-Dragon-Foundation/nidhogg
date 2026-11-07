//! Test helpers: patch builders, a seeded generator and a small hub that sends random patches.
#![allow(dead_code)]

use serde_json::{Value, json};
use sja::{Id, View};
use std::collections::{BTreeMap, BTreeSet};

pub const BIG: usize = 32 << 20;

/// A links descriptor like the Work map's (words are the test's own).
pub fn map_desc() -> Value {
    json!({
        "scheme": "links",
        "chain_kinds": ["came-from", "edits"],
        "match_fields": ["label", "task"],
        "lists": ["team", "outline", "needs-you"],
        "classes": ["selected", "chain", "dim", "match", "lit"],
        "flags": ["live", "needs_you"],
        "forms": [{"name": "graph"}, {"name": "outline", "list": "outline"}, {"name": "board"}],
        "start": "team"
    })
}

pub fn reset(desc: Value, nodes: Vec<Value>, edges: Vec<Value>, lists: Vec<Value>) -> String {
    json!({"ops": [{"reset": {"descriptor": desc, "nodes": nodes, "edges": edges, "lists": lists}}]}).to_string()
}

pub fn ops(ops: Vec<Value>) -> String {
    json!({ "ops": ops }).to_string()
}

pub fn node(id: &str, kind: &str, nav: Value) -> Value {
    json!({"id": id, "kind": kind, "nav": nav, "fields": [["label", id.to_uppercase()]], "rows": [format!("r-{id}")]})
}

/// A small map:
///   ask:1 → plan:1 → agent:a, agent:b (came-from); agent:a edits file:x; agent:b edits file:y; agent:c alone.
///   Layers: ask (0), plan (1), agents a b c (2), files x y (3).  Links as a hub would send them.
pub fn small_map() -> String {
    let n = |id: &str, kind: &str, nav: Value| node(id, kind, nav);
    reset(
        map_desc(),
        vec![
            n("ask:1", "ask", json!({"down": "plan:1"})),
            n("plan:1", "plan", json!({"up": "ask:1", "down": "agent:a"})),
            n(
                "agent:a",
                "agent",
                json!({"right": "agent:b", "up": "plan:1", "down": "file:x"}),
            ),
            n(
                "agent:b",
                "agent",
                json!({"left": "agent:a", "right": "agent:c", "up": "plan:1", "down": "file:y"}),
            ),
            n("agent:c", "agent", json!({"left": "agent:b", "up": null})),
            n(
                "file:x",
                "file",
                json!({"right": "file:y", "up": "agent:a"}),
            ),
            n("file:y", "file", json!({"left": "file:x", "up": "agent:b"})),
        ],
        vec![
            json!(["ask:1", "plan:1", "came-from"]),
            json!(["plan:1", "agent:a", "came-from"]),
            json!(["plan:1", "agent:b", "came-from"]),
            json!(["agent:a", "file:x", "edits"]),
            json!(["agent:b", "file:y", "edits"]),
            json!(["agent:a", "agent:c", "overlap"]),
        ],
        vec![
            json!({"name": "team", "ids": ["agent:a", "agent:b", "agent:c"]}),
            json!({"name": "outline", "ids": ["ask:1", "plan:1", "agent:a", "file:x", "agent:b", "file:y", "agent:c"]}),
            json!({"name": "needs-you", "ids": ["agent:b"]}),
        ],
    )
}

/// A view holding `patch` (applied at version 1 from hub "h1") with instance 1 open.
pub fn view_with(patch: &str) -> View {
    let mut v = View::default();
    v.open(1, None);
    assert_eq!(v.apply("h1", 1, patch, BIG), sja::Status::Applied);
    v
}

pub fn s(id: &Option<Id>) -> Option<&str> {
    id.as_deref()
}

/// xorshift64*, deterministic for a given seed.
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Rng {
        Rng(seed.max(1))
    }
    pub fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    pub fn below(&mut self, n: usize) -> usize {
        (self.next() >> 33) as usize % n.max(1)
    }
}

/// A hub that changes a list view's model at random and sends each change as a patch.  Roots `r0`–`r2` never go;
/// every other node's `then` is a root, so where a selection goes doesn't depend on which patches a view saw.
pub struct Hub {
    pub nodes: BTreeMap<String, (String, i64)>,
    pub edges: BTreeSet<(String, String, String)>,
    pub version: u64,
    next: u64,
}

impl Hub {
    pub fn new() -> Hub {
        let mut h = Hub {
            nodes: BTreeMap::new(),
            edges: BTreeSet::new(),
            version: 1,
            next: 0,
        };
        for r in ["r0", "r1", "r2"] {
            h.nodes.insert(r.into(), ("root".into(), 0));
        }
        h
    }

    fn then(id: &str) -> &'static str {
        ["r0", "r1", "r2"][id.len() % 3]
    }

    fn node_json(&self, id: &str) -> Value {
        let (kind, n) = &self.nodes[id];
        let ids: Vec<&String> = self.nodes.keys().collect();
        let at = ids.iter().position(|x| *x == id).unwrap();
        let nav = json!({
            "left": if at > 0 { json!(ids[at - 1]) } else { Value::Null },
            "right": ids.get(at + 1).map_or(Value::Null, |x| json!(x)),
        });
        let then = if kind == "root" {
            Value::Null
        } else {
            json!(Hub::then(id))
        };
        json!({"id": id, "kind": kind, "nav": nav, "then": then, "fields": [["label", format!("Node {id} n{n}")], ["n", n]], "rows": [id]})
    }

    fn all_list(&self) -> Value {
        json!({"name": "all", "ids": self.nodes.keys().collect::<Vec<_>>()})
    }

    pub fn desc() -> Value {
        json!({"scheme": "list", "chain_kinds": ["c"], "match_fields": ["label"], "lists": ["all"],
               "classes": ["selected", "chain", "dim", "match", "lit"], "flags": []})
    }

    /// The whole state as a reset.
    pub fn snapshot(&self) -> String {
        let nodes: Vec<Value> = self.nodes.keys().map(|id| self.node_json(id)).collect();
        let edges: Vec<Value> = self
            .edges
            .iter()
            .map(|(a, b, k)| json!([a, b, k]))
            .collect();
        reset(Hub::desc(), nodes, edges, vec![self.all_list()])
    }

    /// One random change, as a patch (the version is `self.version` after it).  Neighbours' links change with it, so
    /// the patch resends every node (simple and always consistent).
    pub fn step(&mut self, rng: &mut Rng) -> String {
        let ids: Vec<String> = self.nodes.keys().cloned().collect();
        let mut out = Vec::new();
        match rng.below(5) {
            0 | 1 => {
                self.next += 1;
                let id = format!("n{:03}", self.next);
                self.nodes.insert(id, ("item".into(), 0));
            }
            2 => {
                let victims: Vec<&String> = ids.iter().filter(|i| !i.starts_with('r')).collect();
                if !victims.is_empty() {
                    let v = victims[rng.below(victims.len())].clone();
                    self.nodes.remove(&v);
                    // a hub removes a node's edges with it (the model keeps edges until told)
                    let gone: Vec<Value> = self
                        .edges
                        .iter()
                        .filter(|(a, b, _)| *a == v || *b == v)
                        .map(|(a, b, k)| json!([a, b, k]))
                        .collect();
                    self.edges.retain(|(a, b, _)| *a != v && *b != v);
                    out.push(json!({"remove": [v]}));
                    out.push(json!({"edges": {"remove": gone}}));
                }
            }
            3 => {
                let id = ids[rng.below(ids.len())].clone();
                self.nodes.get_mut(&id).unwrap().1 += 1;
            }
            _ => {
                let a = ids[rng.below(ids.len())].clone();
                let b = ids[rng.below(ids.len())].clone();
                let e = (a, b, "c".to_string());
                let edge = json!([e.0, e.1, e.2]);
                if self.edges.remove(&e) {
                    out.push(json!({"edges": {"remove": [edge]}}));
                } else {
                    self.edges.insert(e);
                    out.push(json!({"edges": {"add": [edge]}}));
                }
            }
        }
        let nodes: Vec<Value> = self.nodes.keys().map(|id| self.node_json(id)).collect();
        out.push(json!({ "nodes": nodes }));
        out.push(json!({"list": self.all_list()}));
        self.version += 1;
        ops(out)
    }
}

/// What a view's model holds, comparable: node → (kind, fields), edges, lists.
pub type Snap = (
    BTreeMap<String, String>,
    BTreeSet<String>,
    BTreeMap<String, Vec<String>>,
);

pub fn snap(v: &View) -> Snap {
    let Some(m) = v.model.as_ref() else {
        return Default::default();
    };
    let nodes = m
        .nodes
        .iter()
        .map(|(id, n)| {
            (
                id.to_string(),
                format!(
                    "{} {:?} {:?}",
                    m.names.word(n.kind),
                    n.fields.len(),
                    v_fields(m, n)
                ),
            )
        })
        .collect();
    let edges = m
        .edges
        .iter()
        .map(|(a, b, k)| format!("{a}>{b}:{}", m.names.word(*k)))
        .collect();
    let lists = m
        .lists
        .iter()
        .map(|(k, l)| (k.clone(), l.ids.iter().map(|i| i.to_string()).collect()))
        .collect();
    (nodes, edges, lists)
}

fn v_fields(m: &sja::model::Model, n: &sja::model::Node) -> Vec<(String, sja::wire::Val)> {
    n.fields
        .iter()
        .map(|(k, v)| (m.names.word(*k).to_string(), v.clone()))
        .collect()
}

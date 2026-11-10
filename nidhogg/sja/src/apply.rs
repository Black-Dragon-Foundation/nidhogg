//! Applying one model patch to a view.  Every op is decoded and checked first: the caps are projected over the whole
//! batch, and a splice outside its list is refused, before anything changes.  So a refused patch leaves the model, the
//! selections and the classes exactly as they were.

use std::collections::{HashMap, HashSet};

use crate::model::{
    MAX_EDGES, MAX_LISTS, MAX_NODES, MAX_UNWINDOWED, Model, edge_bytes, list_bytes, node_bytes,
};
use crate::names::{Id, MAX_WORDS};
use crate::view::{Status, View};
use crate::wire::{self, EdgeIn, NodeIn, Op};

/// Why a batch can't apply.
enum No {
    Resync,
    TooLarge,
}

/// The words a node, an edge or a descriptor would add to the vocabulary.
fn node_words<'a>(n: &'a NodeIn, out: &mut Vec<&'a str>) {
    out.push(&n.kind);
    out.extend(n.nav.iter().map(|(l, _)| l.as_str()));
    for (f, s) in &n.navs {
        out.push(f);
        out.extend(s.iter().map(|(l, _)| l.as_str()));
    }
    out.extend(n.fields.iter().map(|(k, _)| k.as_str()));
}

/// Project an incremental batch over `m`: the counts, bytes and words it leaves, and its splices' ranges.
fn project(m: &Model, ops: &[Op], budget: usize) -> Result<(), No> {
    // node id → its bytes after the batch (None: gone)
    let mut nodes: HashMap<&str, Option<usize>> = HashMap::new();
    let mut edges: HashMap<(&str, &str, &str), bool> = HashMap::new();
    // list name → (length, windowed, bytes)
    let mut lists: HashMap<&str, (usize, bool, usize)> = HashMap::new();
    let mut words: HashSet<&str> = HashSet::new();
    let mut tally = m.tally as i64;
    let had_node = |id: &str| m.nodes.get(id).map(|n| n.bytes);
    let had_edge = |e: &EdgeIn| {
        m.names.get(&e.kind).is_some_and(|k| {
            m.edges
                .contains(&(Id::from(e.from.as_str()), Id::from(e.to.as_str()), k))
        })
    };
    let had_list = |name: &str| {
        m.lists.get(name).map(|l| {
            (
                l.ids.len(),
                l.total.is_some(),
                m.list_bytes.get(name).copied().unwrap_or(0),
            )
        })
    };
    for op in ops {
        match op {
            Op::Reset(_) => return Err(No::Resync),
            Op::Nodes(ns) => {
                for n in ns {
                    let mut w = Vec::new();
                    node_words(n, &mut w);
                    words.extend(w.into_iter().filter(|w| m.names.get(w).is_none()));
                    let old = nodes
                        .get(n.id.as_str())
                        .copied()
                        .unwrap_or_else(|| had_node(&n.id));
                    let new = node_bytes(n);
                    tally += new as i64 - old.unwrap_or(0) as i64;
                    nodes.insert(&n.id, Some(new));
                }
            }
            Op::Remove(ids) => {
                for id in ids {
                    let old = nodes
                        .get(id.as_str())
                        .copied()
                        .unwrap_or_else(|| had_node(id));
                    tally -= old.unwrap_or(0) as i64;
                    nodes.insert(id, None);
                }
            }
            Op::Edges { add, remove } => {
                for e in remove {
                    let key = (e.from.as_str(), e.to.as_str(), e.kind.as_str());
                    if edges.get(&key).copied().unwrap_or_else(|| had_edge(e)) {
                        tally -= edge_bytes(e) as i64;
                    }
                    edges.insert(key, false);
                }
                for e in add {
                    if m.names.get(&e.kind).is_none() {
                        words.insert(&e.kind);
                    }
                    let key = (e.from.as_str(), e.to.as_str(), e.kind.as_str());
                    if !edges.get(&key).copied().unwrap_or_else(|| had_edge(e)) {
                        tally += edge_bytes(e) as i64;
                    }
                    edges.insert(key, true);
                }
            }
            Op::List(l) => {
                let old = lists
                    .get(l.name.as_str())
                    .copied()
                    .or_else(|| had_list(&l.name));
                let b = list_bytes(&l.ids);
                tally += b as i64 - old.map_or(0, |o| o.2) as i64;
                lists.insert(&l.name, (l.ids.len(), l.total.is_some(), b));
            }
            Op::Splice {
                name,
                at,
                remove,
                ids,
            } => {
                let Some((len, win, b)) =
                    lists.get(name.as_str()).copied().or_else(|| had_list(name))
                else {
                    return Err(No::Resync);
                };
                if at.checked_add(*remove).is_none_or(|end| end > len) {
                    return Err(No::Resync);
                }
                // the bytes a splice removes are unknown here without the ids; count the worst case (none removed)
                let came: usize = ids.iter().map(|i| 16 + i.len()).sum();
                tally += came as i64;
                lists.insert(name, (len - remove + ids.len(), win, b + came));
            }
        }
    }
    let node_count = m.nodes.len() as i64
        + nodes
            .iter()
            .map(|(id, after)| i64::from(after.is_some()) - i64::from(m.nodes.contains_key(*id)))
            .sum::<i64>();
    let edge_count = m.edges.len() as i64
        + edges
            .iter()
            .map(|((f, t, k), after)| {
                let had = m
                    .names
                    .get(k)
                    .is_some_and(|k| m.edges.contains(&(Id::from(*f), Id::from(*t), k)));
                i64::from(*after) - i64::from(had)
            })
            .sum::<i64>();
    let list_count = m.lists.len() + lists.keys().filter(|n| !m.lists.contains_key(**n)).count();
    let unwindowed = lists
        .values()
        .any(|(len, win, _)| !win && *len > MAX_UNWINDOWED);
    let bytes = tally.max(0) as usize
        + m.names.bytes()
        + words.iter().map(|w| 2 * w.len() + 48).sum::<usize>();
    if node_count > MAX_NODES as i64
        || edge_count > MAX_EDGES as i64
        || list_count > MAX_LISTS
        || unwindowed
        || m.names.len() + words.len() > MAX_WORDS
        || bytes > budget
    {
        return Err(No::TooLarge);
    }
    Ok(())
}

/// Apply an incremental batch that `project` passed: nothing here can fail.  The answer is the removed nodes' `then`.
fn mutate(m: &mut Model, ops: &[Op]) -> HashMap<Id, Option<Id>> {
    let mut gone: HashMap<Id, Option<Id>> = HashMap::new();
    for op in ops {
        match op {
            Op::Reset(_) => {}
            Op::Nodes(ns) => {
                for n in ns {
                    gone.remove(n.id.as_str());
                    let _ = m.upsert(n);
                }
            }
            Op::Remove(ids) => {
                for id in ids {
                    if let Some(n) = m.remove(id) {
                        gone.insert(Id::from(id.as_str()), n.then);
                    }
                }
            }
            Op::Edges { add, remove } => {
                for e in remove {
                    m.remove_edge(e);
                }
                for e in add {
                    let _ = m.add_edge(e);
                }
            }
            Op::List(l) => m.set_list(l),
            Op::Splice {
                name,
                at,
                remove,
                ids,
            } => m.splice(name, *at, *remove, ids),
        }
    }
    gone
}

/// A new model from a reset and the ops after it; `Err` when it can't apply.
fn build(ops: &[Op], budget: usize) -> Result<Model, No> {
    let Some(Op::Reset(r)) = ops.first() else {
        return Err(No::Resync);
    };
    let mut m = Model::new(&r.desc).ok_or(No::TooLarge)?;
    if r.nodes.len() > MAX_NODES || r.edges.len() > MAX_EDGES || r.lists.len() > MAX_LISTS {
        return Err(No::TooLarge);
    }
    let seed = vec![
        Op::Nodes(r.nodes.clone()),
        Op::Edges {
            add: r.edges.clone(),
            remove: Vec::new(),
        },
    ];
    let lists: Vec<Op> = r.lists.iter().cloned().map(Op::List).collect();
    for batch in [seed.as_slice(), lists.as_slice(), &ops[1..]] {
        project(&m, batch, budget)?;
        mutate(&mut m, batch);
    }
    Ok(m)
}

impl View {
    /// Apply one model patch from hub `hub` at `version`; `budget` is the most bytes the view may hold (the
    /// process-wide cap less the other views').
    pub fn apply(&mut self, hub: &str, version: u64, text: &str, budget: usize) -> Status {
        let Ok(ops) = wire::decode(text) else {
            return Status::Resync;
        };
        self.apply_ops(hub, version, &ops, budget)
    }

    /// Apply the queued models up to version `upto` (all with `None`), oldest first, until one does not apply: a
    /// `resync` or `too-large` drops the queue and its stream.  The answer: how many applied, and the last status.
    pub fn drain(&mut self, upto: Option<u64>, budget: usize) -> (usize, Status) {
        let (mut n, mut last) = (0, Status::Applied);
        let hub = self.queue.hub_name();
        while let Some(q) = self.queue.next_upto(upto) {
            last = match &q.ops {
                Ok(ops) => self.apply_ops(&hub, q.version, ops, budget),
                Err(_) => Status::Resync,
            };
            match last {
                Status::Applied => n += 1,
                Status::Dropped => {}
                _ => {
                    self.queue.lose();
                    break;
                }
            }
        }
        (n, last)
    }

    /// Apply decoded `ops` (see `apply`).
    pub fn apply_ops(&mut self, hub: &str, version: u64, ops: &[Op], budget: usize) -> Status {
        let reset = matches!(ops.first(), Some(Op::Reset(_)));
        let same_hub = self.hub.as_deref() == Some(hub);
        if same_hub && version <= self.version {
            return Status::Dropped;
        }
        if !reset && (!same_hub || self.model.is_none()) {
            return Status::Resync;
        }
        let result = if reset {
            build(ops, budget).map(|mut m| {
                let old = self.model.take();
                m.generation = old.as_ref().map_or(0, |o| o.generation) + 1;
                self.model = Some(m);
                let then_of = |id: &str| old.as_ref()?.nodes.get(id)?.then.clone();
                self.settle(&then_of);
            })
        } else {
            let m = self.model.as_mut().expect("checked above");
            match project(m, ops, budget) {
                Ok(()) => {
                    let gone = mutate(m, ops);
                    m.generation += 1;
                    let then_of = |id: &str| gone.get(id).cloned().flatten();
                    self.settle(&then_of);
                    Ok(())
                }
                Err(e) => Err(e),
            }
        };
        match result {
            Ok(()) => {
                self.hub = Some(hub.to_string());
                self.version = version;
                self.stale = false;
                Status::Applied
            }
            Err(No::Resync) => Status::Resync,
            Err(No::TooLarge) => Status::TooLarge,
        }
    }
}

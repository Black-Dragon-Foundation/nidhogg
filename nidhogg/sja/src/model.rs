//! One view's model as drawn: the descriptor, the nodes, the edges and the lists the hub computed.  Nothing here
//! knows what a node stands for: kinds, fields, links and classes are words the descriptor and the hub give.

use std::collections::{HashMap, HashSet};

use crate::names::{Id, Names, Sym};
use crate::wire::{DescIn, EdgeIn, ListIn, NodeIn, Scheme, Val};

/// The caps of one view (design §2.7); over one, the apply answers `too-large`.
pub const MAX_NODES: usize = 200_000;
pub const MAX_EDGES: usize = 400_000;
pub const MAX_LISTS: usize = 64;
/// A list longer than this must be windowed (`total` set).
pub const MAX_UNWINDOWED: usize = 20_000;

#[derive(Clone, Debug)]
pub struct Form {
    pub name: Sym,
    pub list: Option<String>,
}

#[derive(Clone, Debug)]
pub struct Desc {
    pub scheme: Scheme,
    pub chain_kinds: HashSet<Sym>,
    pub match_fields: Vec<Sym>,
    pub lists: Vec<String>,
    pub classes: Vec<String>,
    pub flags: Vec<String>,
    pub forms: Vec<Form>,
    pub start: Option<String>,
}

impl Desc {
    /// The position of the class named `name`, as the face index Lisp gave at open.
    pub fn class(&self, name: &str) -> Option<u8> {
        self.classes
            .iter()
            .position(|c| c == name)
            .and_then(|i| u8::try_from(i).ok())
    }

    pub fn form(&self, name: Sym) -> Option<&Form> {
        self.forms.iter().find(|f| f.name == name)
    }
}

#[derive(Clone, Debug)]
pub struct Node {
    pub kind: Sym,
    pub parent: Option<Id>,
    pub flags: u32,
    /// The default link set: (link, target).
    pub nav: Vec<(Sym, Id)>,
    /// Link sets per form.
    pub navs: Vec<(Sym, Vec<(Sym, Id)>)>,
    pub fields: Vec<(Sym, Val)>,
    pub rows: Vec<Box<str>>,
    pub then: Option<Id>,
    pub bytes: usize,
}

impl Node {
    pub fn field(&self, f: Sym) -> Option<&Val> {
        self.fields.iter().find(|(k, _)| *k == f).map(|(_, v)| v)
    }

    /// The target of `link` in the link set of `form` (the default set when the form has none of its own).
    pub fn link(&self, form: Option<Sym>, link: Sym) -> Option<&Id> {
        let set = form
            .and_then(|f| self.navs.iter().find(|(n, _)| *n == f))
            .map_or(&self.nav, |(_, s)| s);
        set.iter().find(|(l, _)| *l == link).map(|(_, t)| t)
    }
}

#[derive(Clone, Debug, Default)]
pub struct List {
    pub ids: Vec<Id>,
    /// The whole list's length when this is a window of it.
    pub total: Option<usize>,
    /// The window's first position in the whole list.
    pub from: usize,
}

impl List {
    pub fn windowed(&self) -> bool {
        self.total
            .is_some_and(|t| self.from > 0 || self.from + self.ids.len() < t)
    }
}

/// Bytes the tally counts for a node, an edge and a list entry (an estimate: ids are counted at every use).
pub fn node_bytes(n: &NodeIn) -> usize {
    let links =
        |s: &[(String, String)]| s.iter().map(|(a, b)| 40 + a.len() + b.len()).sum::<usize>();
    160 + 2 * n.id.len()
        + n.parent.as_ref().map_or(0, |p| p.len())
        + n.then.as_ref().map_or(0, |p| p.len())
        + links(&n.nav)
        + n.navs.iter().map(|(_, s)| 32 + links(s)).sum::<usize>()
        + n.fields
            .iter()
            .map(|(k, v)| {
                40 + k.len()
                    + match v {
                        Val::Str(s) => s.len(),
                        Val::Int(_) => 8,
                    }
            })
            .sum::<usize>()
        + n.rows.iter().map(|r| 24 + r.len()).sum::<usize>()
}

pub fn edge_bytes(e: &EdgeIn) -> usize {
    // the set entry and both adjacency entries
    160 + 2 * (e.from.len() + e.to.len())
}

pub fn list_bytes(ids: &[String]) -> usize {
    64 + ids.iter().map(|i| 16 + i.len()).sum::<usize>()
}

#[derive(Clone, Debug)]
pub struct Model {
    pub desc: Desc,
    pub names: Names,
    pub nodes: HashMap<Id, Node>,
    pub edges: HashSet<(Id, Id, Sym)>,
    /// Chain-kind edges only: id → (to) and id → (from).
    pub fwd: HashMap<Id, Vec<Id>>,
    pub back: HashMap<Id, Vec<Id>>,
    pub lists: HashMap<String, List>,
    pub list_bytes: HashMap<String, usize>,
    /// The tally of nodes, edges and lists (the vocabulary is added in `bytes`).
    pub tally: usize,
    /// Bumped by every apply: caches key on it.
    pub generation: u64,
}

impl Model {
    /// An empty model with the descriptor `d`; `None` when its vocabulary is too large.
    pub fn new(d: &DescIn) -> Option<Model> {
        let mut names = Names::default();
        let chain_kinds = d
            .chain_kinds
            .iter()
            .map(|k| names.intern(k))
            .collect::<Option<HashSet<_>>>()?;
        let match_fields = d
            .match_fields
            .iter()
            .map(|k| names.intern(k))
            .collect::<Option<Vec<_>>>()?;
        let forms = d
            .forms
            .iter()
            .map(|f| {
                Some(Form {
                    name: names.intern(&f.name)?,
                    list: f.list.clone(),
                })
            })
            .collect::<Option<Vec<_>>>()?;
        Some(Model {
            desc: Desc {
                scheme: d.scheme,
                chain_kinds,
                match_fields,
                lists: d.lists.clone(),
                classes: d.classes.clone(),
                flags: d.flags.clone(),
                forms,
                start: d.start.clone(),
            },
            names,
            nodes: HashMap::new(),
            edges: HashSet::new(),
            fwd: HashMap::new(),
            back: HashMap::new(),
            lists: HashMap::new(),
            list_bytes: HashMap::new(),
            tally: 0,
            generation: 0,
        })
    }

    pub fn bytes(&self) -> usize {
        self.tally + self.names.bytes()
    }

    /// The shared id for `s`: the node's own when it exists, so every use shares one allocation.
    pub fn id(&self, s: &str) -> Id {
        self.nodes
            .get_key_value(s)
            .map_or_else(|| Id::from(s), |(k, _)| k.clone())
    }

    fn links(&mut self, s: &[(String, String)]) -> Option<Vec<(Sym, Id)>> {
        s.iter()
            .map(|(l, t)| Some((self.names.intern(l)?, self.id(t))))
            .collect()
    }

    /// Insert or replace a node; `None` when the vocabulary is full.
    pub fn upsert(&mut self, n: &NodeIn) -> Option<()> {
        let kind = self.names.intern(&n.kind)?;
        let nav = self.links(&n.nav)?;
        let navs = n
            .navs
            .iter()
            .map(|(f, s)| Some((self.names.intern(f)?, self.links(s)?)))
            .collect::<Option<Vec<_>>>()?;
        let fields = n
            .fields
            .iter()
            .map(|(k, v)| Some((self.names.intern(k)?, v.clone())))
            .collect::<Option<Vec<_>>>()?;
        let bytes = node_bytes(n);
        let node = Node {
            kind,
            parent: n.parent.as_deref().map(|p| self.id(p)),
            flags: n.flags,
            nav,
            navs,
            fields,
            rows: n.rows.iter().map(|r| r.as_str().into()).collect(),
            then: n.then.as_deref().map(|t| self.id(t)),
            bytes,
        };
        let id = self.id(&n.id);
        if let Some(old) = self.nodes.insert(id, node) {
            self.tally -= old.bytes;
        }
        self.tally += bytes;
        Some(())
    }

    /// Remove a node (its edges stay until the hub removes them; an edge to a missing node is never followed).
    pub fn remove(&mut self, id: &str) -> Option<Node> {
        let n = self.nodes.remove(id)?;
        self.tally -= n.bytes;
        Some(n)
    }

    pub fn add_edge(&mut self, e: &EdgeIn) -> Option<()> {
        let kind = self.names.intern(&e.kind)?;
        let (from, to) = (self.id(&e.from), self.id(&e.to));
        // the adjacency holds a pair once, whatever chain kinds join it
        let joined = self
            .desc
            .chain_kinds
            .iter()
            .any(|k| self.edges.contains(&(from.clone(), to.clone(), *k)));
        if !self.edges.insert((from.clone(), to.clone(), kind)) {
            return Some(());
        }
        self.tally += edge_bytes(e);
        if self.desc.chain_kinds.contains(&kind) && !joined {
            self.fwd.entry(from.clone()).or_default().push(to.clone());
            self.back.entry(to).or_default().push(from);
        }
        Some(())
    }

    pub fn remove_edge(&mut self, e: &EdgeIn) {
        let Some(kind) = self.names.get(&e.kind) else {
            return;
        };
        let (from, to) = (Id::from(e.from.as_str()), Id::from(e.to.as_str()));
        if !self.edges.remove(&(from.clone(), to.clone(), kind)) {
            return;
        }
        self.tally -= edge_bytes(e);
        if self.desc.chain_kinds.contains(&kind) {
            // another chain-kind edge between the same two keeps the adjacency entry
            let still = self
                .desc
                .chain_kinds
                .iter()
                .any(|k| self.edges.contains(&(from.clone(), to.clone(), *k)));
            if !still {
                drop_one(&mut self.fwd, &e.from, &e.to);
                drop_one(&mut self.back, &e.to, &e.from);
            }
        }
    }

    pub fn set_list(&mut self, l: &ListIn) {
        let ids = l.ids.iter().map(|i| self.id(i)).collect();
        let b = list_bytes(&l.ids);
        if let Some(old) = self.list_bytes.insert(l.name.clone(), b) {
            self.tally -= old;
        }
        self.tally += b;
        self.lists.insert(
            l.name.clone(),
            List {
                ids,
                total: l.total,
                from: l.from,
            },
        );
    }

    pub fn splice(&mut self, name: &str, at: usize, remove: usize, ids: &[String]) {
        let new: Vec<Id> = ids.iter().map(|i| self.id(i)).collect();
        let l = self.lists.entry(name.to_string()).or_default();
        let gone: usize = l.ids[at..at + remove].iter().map(|i| 16 + i.len()).sum();
        let came: usize = ids.iter().map(|i| 16 + i.len()).sum();
        l.ids.splice(at..at + remove, new);
        if let Some(t) = l.total.as_mut() {
            *t = (*t + ids.len()).saturating_sub(remove);
        }
        let b = self.list_bytes.entry(name.to_string()).or_insert(64);
        *b = *b + came - gone;
        self.tally = self.tally + came - gone;
    }

    /// Every node reachable from `id` backward (`up`) and forward (`down`) over chain-kind edges, `id` excluded; nodes
    /// the model doesn't hold are skipped but walked through.
    pub fn chain(&self, id: &str) -> (HashSet<Id>, HashSet<Id>) {
        let reach = |adj: &HashMap<Id, Vec<Id>>| {
            let mut seen: HashSet<Id> = HashSet::new();
            let mut stack: Vec<&str> = vec![id];
            while let Some(y) = stack.pop() {
                for z in adj.get(y).into_iter().flatten() {
                    if **z != *id && seen.insert(z.clone()) {
                        stack.push(z);
                    }
                }
            }
            seen.retain(|z| self.nodes.contains_key(z));
            seen
        };
        (reach(&self.back), reach(&self.fwd))
    }
}

fn drop_one(adj: &mut HashMap<Id, Vec<Id>>, k: &str, v: &str) {
    if let Some(vs) = adj.get_mut(k) {
        if let Some(i) = vs.iter().position(|x| **x == *v) {
            vs.swap_remove(i);
        }
        if vs.is_empty() {
            adj.remove(k);
        }
    }
}

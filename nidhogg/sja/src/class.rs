//! Classes: what each node looks like for an instance.  In order: `selected` for the selection; `ticked` for the nodes of
//! the instance's ticked set (the Explorer's marks; a descriptor that doesn't name it, Search's, leaves it); while a
//! search is set,
//! `match` or `dim`; else `lit` for the nodes of the lit list; else, while there is a selection in a view with a chain
//! (the descriptor's `chain_kinds`) or a lit list, `chain` (reachable from it over `chain_kinds`, either way) or `dim`;
//! else none.  A view with neither has no chain to dim off: dimming every node but the selection would only cost the
//! editor a face on every row.  `classes` reports the changes; when only the selection and the ticked nodes can have a
//! class (no search, no lit list, no chain kinds) it looks at those, never at every node.  A class the descriptor doesn't name is
//! none.  A class is the face index Lisp gave at open, the class's position in the descriptor.

use std::collections::{HashMap, HashSet};

use crate::model::Model;
use crate::names::Id;
use crate::view::{Inst, View};
use crate::wire::Val;

/// The descriptor's class indices: selected, match, dim, chain, lit, ticked.
struct Idx([Option<u8>; 6]);

impl Idx {
    fn of(m: &Model) -> Idx {
        Idx(["selected", "match", "dim", "chain", "lit", "ticked"].map(|c| m.desc.class(c)))
    }
}

/// Whether node `id` of `m` matches the (lower-cased) search `q` in one of the descriptor's match fields.
fn matches(m: &Model, id: &Id, q: &str) -> bool {
    let Some(n) = m.nodes.get(id) else {
        return false;
    };
    m.desc.match_fields.iter().any(|f| match n.field(*f) {
        Some(Val::Str(s)) => s.to_lowercase().contains(q),
        Some(Val::Int(i)) => i.to_string().contains(q),
        None => false,
    })
}

/// What a class decision needs, computed once per call (the chain and the matches are cached on the instance by the
/// model's generation).
struct Ctx {
    idx: Idx,
    sel: Option<Id>,
    searching: bool,
    lit: HashSet<Id>,
    /// Whether the descriptor names chain kinds: only then is a node off the selection's chain dim.
    chained: bool,
}

fn ctx(m: &Model, inst: &mut Inst, sel: Option<Id>) -> Ctx {
    if let Some(s) = &sel
        && inst
            .chain
            .as_ref()
            .is_none_or(|(g, id, _)| *g != m.generation || id != s)
    {
        let (up, down) = m.chain(s);
        inst.chain = Some((
            m.generation,
            s.clone(),
            up.into_iter().chain(down).collect(),
        ));
    }
    if let Some(q) = &inst.search
        && inst
            .matches
            .as_ref()
            .is_none_or(|(g, t, _)| *g != m.generation || t != q)
    {
        let set = m
            .nodes
            .keys()
            .filter(|id| matches(m, id, q))
            .cloned()
            .collect();
        inst.matches = Some((m.generation, q.clone(), set));
    }
    let lit = inst
        .lit
        .as_ref()
        .and_then(|l| m.lists.get(l))
        .map(|l| l.ids.iter().cloned().collect())
        .unwrap_or_default();
    Ctx {
        idx: Idx::of(m),
        searching: inst.search.is_some(),
        sel,
        lit,
        chained: !m.desc.chain_kinds.is_empty(),
    }
}

fn class(c: &Ctx, inst: &Inst, id: &Id) -> Option<u8> {
    let [selected, matched, dim, chain, lit, ticked] = c.idx.0;
    if c.sel.as_ref() == Some(id) {
        return selected;
    }
    if ticked.is_some() && inst.ticked.contains(&**id) {
        return ticked;
    }
    if c.searching {
        let hit = inst
            .matches
            .as_ref()
            .is_some_and(|(_, _, s)| s.contains(id));
        return if hit { matched } else { dim };
    }
    if c.lit.contains(id) {
        return lit;
    }
    if c.sel.is_some() && (c.chained || !c.lit.is_empty()) {
        let on = inst.chain.as_ref().is_some_and(|(_, _, s)| s.contains(id));
        return if on { chain } else { dim };
    }
    None
}

/// Whether only the selection and the ticked nodes can have a class under `c`: no search, no lit list, no chain.
fn sparse(c: &Ctx) -> bool {
    !c.searching && c.lit.is_empty() && !c.chained
}

impl View {
    /// The current class of `id` for instance `h`.
    pub fn class_of(&mut self, h: u32, id: &str) -> Option<u8> {
        let sel = self.selection(h);
        let m = self.model.as_ref()?;
        let id = m.nodes.get_key_value(id)?.0.clone();
        let inst = self.insts.get_mut(&h)?;
        let c = ctx(m, inst, sel);
        class(&c, inst, &id)
    }

    /// The nodes whose class changed since the last call, with their class now (`None`: no class), sorted by id; the
    /// instance remembers what it reported.
    pub fn classes(&mut self, h: u32) -> Vec<(Id, Option<u8>)> {
        let sel = self.selection(h);
        let Some(m) = self.model.as_ref() else {
            return Vec::new();
        };
        let Some(inst) = self.insts.get_mut(&h) else {
            return Vec::new();
        };
        let c = ctx(m, inst, sel);
        let mut now: HashMap<Id, u8> = HashMap::new();
        if sparse(&c) {
            // the selection and the ticked nodes: every other node has no class
            let ticked: Vec<&str> = inst.ticked.iter().map(String::as_str).collect();
            for id in c.sel.iter().map(|s| &**s).chain(ticked) {
                if let Some((id, _)) = m.nodes.get_key_value(id)
                    && let Some(k) = class(&c, inst, id)
                {
                    now.insert(id.clone(), k);
                }
            }
        } else {
            for id in m.nodes.keys() {
                if let Some(k) = class(&c, inst, id) {
                    now.insert(id.clone(), k);
                }
            }
        }
        let mut out: Vec<(Id, Option<u8>)> = now
            .iter()
            .filter(|(id, k)| inst.last.get(*id) != Some(k))
            .map(|(id, k)| (id.clone(), Some(*k)))
            .collect();
        out.extend(
            inst.last
                .keys()
                .filter(|id| !now.contains_key(*id) && m.nodes.contains_key(*id))
                .map(|id| (id.clone(), None)),
        );
        out.sort_by(|a, b| a.0.cmp(&b.0));
        inst.last = now;
        out
    }

    /// Forget what instance `h` reported, so the next `classes` answers every node that has a class (a redraw).
    pub fn reclass(&mut self, h: u32) {
        if let Some(i) = self.insts.get_mut(&h) {
            i.last.clear();
        }
    }
}

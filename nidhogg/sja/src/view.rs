//! One view: its model as drawn, the hub and version it came from, and the instances that show it with their user
//! state (selection, form, search, lit list, expanded and ticked sets, the classes last reported).
//!
//! Selections live apart from the model, by id: a resync, a reset or a dropped model leaves them, and the next model
//! re-resolves them.  An instance in a group shares the group's selection (the map and its lenses).

use std::collections::{BTreeMap, HashMap, HashSet};

use crate::model::Model;
use crate::names::Id;
use crate::wire::Scheme;

/// What an apply answers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Applied = 0,
    /// Not newer than what is drawn, from the same hub: nothing changed.
    Dropped = 1,
    /// The patch can't apply (malformed, before a reset, a gap, a panic): ask the hub for a reset.
    Resync = 2,
    /// Over a cap: a bug, not a mode.
    TooLarge = 3,
    /// The view's breaker tripped.
    Broken = 4,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum SelKey {
    Group(String),
    Own(u32),
}

#[derive(Clone, Debug, Default)]
pub struct Sel {
    pub id: Option<Id>,
    /// The selection's position in the view's primary list when it was set: the fallback of a list view whose
    /// selection went with no `then`.
    pub index: usize,
    /// The id that went from under the selection, until read.
    pub finished: Option<Id>,
}

#[derive(Clone, Debug)]
pub struct Inst {
    pub sel: SelKey,
    pub form: Option<String>,
    pub search: Option<String>,
    pub lit: Option<String>,
    pub expanded: HashSet<String>,
    pub ticked: HashSet<String>,
    /// The classes last reported: only nodes that have one.
    pub last: HashMap<Id, u8>,
    pub(crate) chain: Option<(u64, Id, HashSet<Id>)>,
    pub(crate) matches: Option<(u64, String, HashSet<Id>)>,
}

/// A step's answer: the selection, and a direction when a windowed list needs its next window first.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Step {
    pub id: Option<Id>,
    pub more: Option<i8>,
}

#[derive(Debug, Default)]
pub struct View {
    pub model: Option<Model>,
    pub hub: Option<String>,
    pub version: u64,
    /// The hub connection went: keys keep working on what is drawn.
    pub stale: bool,
    pub sels: HashMap<SelKey, Sel>,
    pub insts: BTreeMap<u32, Inst>,
}

impl View {
    /// A new instance with handle `h`, sharing `group`'s selection when given.
    pub fn open(&mut self, h: u32, group: Option<&str>) {
        let sel = group.map_or(SelKey::Own(h), |g| SelKey::Group(g.to_string()));
        self.sels.entry(sel.clone()).or_default();
        self.insts.insert(
            h,
            Inst {
                sel,
                form: None,
                search: None,
                lit: None,
                expanded: HashSet::new(),
                ticked: HashSet::new(),
                last: HashMap::new(),
                chain: None,
                matches: None,
            },
        );
    }

    /// Drop instance `h`; true when no instance is left (the view can go).
    pub fn close(&mut self, h: u32) -> bool {
        if let Some(i) = self.insts.remove(&h)
            && !self.insts.values().any(|o| o.sel == i.sel)
        {
            self.sels.remove(&i.sel);
        }
        self.insts.is_empty()
    }

    pub fn inst(&self, h: u32) -> Option<&Inst> {
        self.insts.get(&h)
    }

    /// The selection instance `h` sees, if it is in the model.
    pub fn selection(&self, h: u32) -> Option<Id> {
        let m = self.model.as_ref()?;
        let i = self.insts.get(&h)?;
        self.sels
            .get(&i.sel)?
            .id
            .clone()
            .filter(|id| m.nodes.contains_key(id))
    }

    /// The list a list view's index fallback and `start` use.
    pub(crate) fn primary_list(m: &Model) -> Option<&str> {
        m.desc
            .start
            .as_deref()
            .or(m.desc.lists.first().map(String::as_str))
    }

    /// Set `h`'s selection to `id` (which the model holds), keeping its index in the primary list.
    pub(crate) fn put(&mut self, h: u32, id: Option<Id>) -> Option<Id> {
        let key = self.insts.get(&h).map(|i| i.sel.clone())?;
        let index = match (&id, self.model.as_ref()) {
            (Some(id), Some(m)) => Self::primary_list(m)
                .and_then(|l| m.lists.get(l))
                .and_then(|l| l.ids.iter().position(|x| x == id).map(|p| p + l.from)),
            _ => None,
        };
        let s = self.sels.entry(key).or_default();
        if let Some(i) = index {
            s.index = i;
        }
        s.id = id.clone();
        id
    }

    /// After the model changed: each selection the new model no longer holds moves to the removed node's `then` (from
    /// `then_of`, the model before), else, in a list view, to the primary list's entry at its old index, else to nil;
    /// `finished` records the id that went.
    pub(crate) fn settle(&mut self, then_of: &dyn Fn(&str) -> Option<Id>) {
        let Some(m) = self.model.as_ref() else {
            return;
        };
        for s in self.sels.values_mut() {
            let Some(id) = s.id.clone() else { continue };
            if m.nodes.contains_key(&id) {
                continue;
            }
            let mut to = then_of(&id).filter(|t| m.nodes.contains_key(t));
            if to.is_none() && m.desc.scheme == Scheme::List {
                to = Self::primary_list(m)
                    .and_then(|l| m.lists.get(l))
                    .and_then(|l| {
                        let at = s
                            .index
                            .saturating_sub(l.from)
                            .min(l.ids.len().saturating_sub(1));
                        l.ids.get(at).cloned()
                    });
            }
            s.finished = Some(id);
            s.id = to.map(|t| m.id(&t));
        }
    }

    /// Mark the view stale (the hub went).
    pub fn hub_lost(&mut self) {
        self.stale = true;
    }

    // ---- verbs ----

    /// Follow `link` from `h`'s selection: in a list form `up` and `down` step along the form's list; otherwise the
    /// link set of the instance's form.  With no selection, the first entry of the primary list.  A link that is null
    /// or leads outside the model leaves the selection.  The answer is the selection.
    pub fn follow(&mut self, h: u32, link: &str) -> Option<Id> {
        let m = self.model.as_ref()?;
        let inst = self.insts.get(&h)?;
        let form = inst.form.as_deref().and_then(|f| m.names.get(f));
        let Some(cur) = self.selection(h) else {
            let first = Self::primary_list(m)
                .and_then(|l| m.lists.get(l))
                .and_then(|l| l.ids.iter().find(|i| m.nodes.contains_key(*i)).cloned());
            return self.put(h, first);
        };
        if let Some(list) = form
            .and_then(|f| m.desc.form(f))
            .and_then(|f| f.list.clone())
            && (link == "up" || link == "down")
        {
            let step = if link == "up" { -1 } else { 1 };
            return self.walk_inner(h, &list, step, false).id;
        }
        let target = m
            .names
            .get(link)
            .and_then(|l| m.nodes.get(&cur)?.link(form, l))
            .filter(|t| m.nodes.contains_key(*t))
            .cloned();
        match target {
            Some(t) => self.put(h, Some(t)),
            None => Some(cur),
        }
    }

    /// Step `step` entries along `list` from `h`'s selection, wrapping (a windowed list stops at its window's edge and
    /// answers `more`).  From no selection or one not in the list: the first entry (the last for a negative step).
    pub fn walk(&mut self, h: u32, list: &str, step: i64) -> Step {
        self.walk_inner(h, list, step, true)
    }

    fn walk_inner(&mut self, h: u32, list: &str, step: i64, wrap: bool) -> Step {
        let cur = self.selection(h);
        let Some(m) = self.model.as_ref() else {
            return Step {
                id: None,
                more: None,
            };
        };
        let Some(l) = m.lists.get(list).filter(|l| !l.ids.is_empty()) else {
            return Step {
                id: cur,
                more: None,
            };
        };
        let len = l.ids.len() as i64;
        let pos = cur.as_ref().and_then(|c| l.ids.iter().position(|x| x == c));
        let mut more = None;
        let at = match pos {
            None => {
                if step >= 0 {
                    0
                } else {
                    len - 1
                }
            }
            Some(p) => {
                let n = p as i64 + step;
                if l.windowed() {
                    if n < 0 && l.from > 0 {
                        more = Some(-1);
                    } else if n >= len && l.from + l.ids.len() < l.total.unwrap_or(0) {
                        more = Some(1);
                    }
                    n.clamp(0, len - 1)
                } else if wrap {
                    n.rem_euclid(len)
                } else {
                    n.clamp(0, len - 1)
                }
            }
        };
        let target = l.ids[at as usize].clone();
        let id = if m.nodes.contains_key(&target) {
            self.put(h, Some(target))
        } else {
            cur
        };
        Step { id, more }
    }

    /// Jump to position `n` (0-based, in the whole list) of `list`.  Outside a window: `more` toward it, no move;
    /// outside the list: nil, no move.
    pub fn walk_to(&mut self, h: u32, list: &str, n: usize) -> Step {
        let cur = self.selection(h);
        let Some(l) = self.model.as_ref().and_then(|m| m.lists.get(list)) else {
            return Step {
                id: None,
                more: None,
            };
        };
        let total = l.total.unwrap_or(l.from + l.ids.len());
        if n >= total {
            return Step {
                id: None,
                more: None,
            };
        }
        if n < l.from {
            return Step {
                id: cur,
                more: Some(-1),
            };
        }
        if n >= l.from + l.ids.len() {
            return Step {
                id: cur,
                more: Some(1),
            };
        }
        let target = l.ids[n - l.from].clone();
        let id = if self
            .model
            .as_ref()
            .is_some_and(|m| m.nodes.contains_key(&target))
        {
            self.put(h, Some(target))
        } else {
            cur
        };
        Step { id, more: None }
    }

    /// Select `id` (nil clears); an id the model doesn't hold leaves the selection.  The answer is the selection.
    pub fn select(&mut self, h: u32, id: Option<&str>) -> Option<Id> {
        match id {
            None => {
                self.insts.get(&h)?;
                self.put(h, None)
            }
            Some(id) => {
                let known = self
                    .model
                    .as_ref()
                    .and_then(|m| m.nodes.get_key_value(id))
                    .map(|(k, _)| k.clone());
                match known {
                    Some(k) => self.put(h, Some(k)),
                    None => self.selection(h),
                }
            }
        }
    }

    pub fn set_search(&mut self, h: u32, text: Option<&str>) {
        if let Some(i) = self.insts.get_mut(&h) {
            i.search = text.filter(|t| !t.is_empty()).map(str::to_lowercase);
        }
    }

    pub fn set_form(&mut self, h: u32, form: Option<&str>) {
        if let Some(i) = self.insts.get_mut(&h) {
            i.form = form.map(str::to_string);
        }
    }

    pub fn set_lit(&mut self, h: u32, list: Option<&str>) {
        if let Some(i) = self.insts.get_mut(&h) {
            i.lit = list.map(str::to_string);
        }
    }

    /// Toggle `id` in the instance's expanded (`tick` false) or ticked set; the answer is whether it is in it now.
    pub fn toggle(&mut self, h: u32, id: &str, tick: bool) -> bool {
        let Some(i) = self.insts.get_mut(&h) else {
            return false;
        };
        let set = if tick { &mut i.ticked } else { &mut i.expanded };
        if set.remove(id) {
            false
        } else {
            set.insert(id.to_string());
            true
        }
    }

    /// The id that went from under `h`'s selection, once.
    pub fn take_finished(&mut self, h: u32) -> Option<Id> {
        let key = self.insts.get(&h)?.sel.clone();
        self.sels.get_mut(&key)?.finished.take()
    }

    /// The kind of `id` (the selection when `None`).
    pub fn kind(&self, h: u32, id: Option<&str>) -> Option<&str> {
        let m = self.model.as_ref()?;
        let id = id.map(Id::from).or_else(|| self.selection(h))?;
        m.nodes.get(&id).map(|n| m.names.word(n.kind))
    }

    /// The value of field `f` of `id` (the selection when `None`).
    pub fn field(&self, h: u32, f: &str, id: Option<&str>) -> Option<&crate::wire::Val> {
        let m = self.model.as_ref()?;
        let id = id.map(Id::from).or_else(|| self.selection(h))?;
        m.nodes.get(&id)?.field(m.names.get(f)?)
    }

    /// Whether flag `f` (a name the descriptor declares) is set on `id` (the selection when `None`).
    pub fn flag(&self, h: u32, f: &str, id: Option<&str>) -> bool {
        let Some(m) = self.model.as_ref() else {
            return false;
        };
        let Some(bit) = m.desc.flags.iter().position(|x| x == f).filter(|b| *b < 32) else {
            return false;
        };
        let Some(id) = id.map(Id::from).or_else(|| self.selection(h)) else {
            return false;
        };
        m.nodes.get(&id).is_some_and(|n| n.flags & (1 << bit) != 0)
    }

    /// The parent of `id` (the selection when `None`).
    pub fn parent(&self, h: u32, id: Option<&str>) -> Option<Id> {
        let m = self.model.as_ref()?;
        let id = id.map(Id::from).or_else(|| self.selection(h))?;
        m.nodes.get(&id)?.parent.clone()
    }

    /// The rows `id` (the selection when `None`) is drawn in.
    pub fn rows(&self, h: u32, id: Option<&str>) -> Vec<Box<str>> {
        let Some(m) = self.model.as_ref() else {
            return Vec::new();
        };
        let Some(id) = id.map(Id::from).or_else(|| self.selection(h)) else {
            return Vec::new();
        };
        m.nodes.get(&id).map(|n| n.rows.clone()).unwrap_or_default()
    }

    /// The ids of list `name` (its window).
    pub fn list(&self, name: &str) -> Vec<Id> {
        self.model
            .as_ref()
            .and_then(|m| m.lists.get(name))
            .map(|l| l.ids.clone())
            .unwrap_or_default()
    }

    /// The descriptor's class names, in face-index order.
    pub fn class_names(&self) -> Vec<String> {
        self.model
            .as_ref()
            .map(|m| m.desc.classes.clone())
            .unwrap_or_default()
    }

    /// Counts: nodes, edges, lists, bytes, instances.
    pub fn counts(&self) -> [usize; 5] {
        let m = self.model.as_ref();
        [
            m.map_or(0, |m| m.nodes.len()),
            m.map_or(0, |m| m.edges.len()),
            m.map_or(0, |m| m.lists.len()),
            self.bytes(),
            self.insts.len(),
        ]
    }

    pub fn bytes(&self) -> usize {
        self.model.as_ref().map_or(0, Model::bytes)
    }
}

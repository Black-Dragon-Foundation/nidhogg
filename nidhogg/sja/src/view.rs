//! One view: its model as drawn, the hub and version it came from, and the instances that show it with their user
//! state (selection, form, search, lit list, expanded and ticked sets, the classes last reported).
//!
//! Selections live apart from the model, by id: a resync, a reset or a dropped model leaves them, and the next model
//! re-resolves them.  An instance in a group shares the group's selection (the map and its lenses).

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};

use crate::model::Model;
use crate::names::Id;
use crate::queue::Queue;
use crate::wire::Scheme;

/// The name of a tree's visible list, which each instance computes (`View::visible`).
pub const VISIBLE: &str = "visible";

/// How many moves each of an instance's history rings keeps (the reserved links `back` and `forward`).
pub const HISTORY: usize = 50;

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
    /// What the classes last reported were computed from (`classes`); bumped on each tick.
    pub(crate) classed: Option<crate::class::Basis>,
    pub(crate) ticks: u64,
    /// The class passes `classes` ran (for tests and the perf lab): a call with nothing changed runs none.
    pub passes: u64,
    /// Where the selection was before each move, newest last, and the moves `back` undid, newest last: each at most
    /// [`HISTORY`].
    pub back: VecDeque<Id>,
    pub forward: VecDeque<Id>,
    /// The last move was a step (an arrow, a walk, type-to-navigate): the next step continues its run.
    stepping: bool,
}

/// What a move is to the history: a jump (a link that is no direction, a select) is one move; a run of steps (the
/// directions, a walk, type-to-navigate) is one move too, recorded where the run began.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Move {
    Jump,
    Step,
}

/// The links the history answers, before any of the model's.
const BACK: &str = "back";
const FORWARD: &str = "forward";

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
    /// The models the hub sent that the draw has not applied yet (`queue`).
    pub queue: Queue,
    /// The view's rows, when the editor draws them from Sjá (`rows on`).
    pub rows: Option<crate::rows::Store>,
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
                classed: None,
                ticks: 0,
                passes: 0,
                back: VecDeque::new(),
                forward: VecDeque::new(),
                stepping: false,
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

    /// Mark the view stale (the hub went): its queue and stream go too.
    pub fn hub_lost(&mut self) {
        self.stale = true;
        self.queue.lose();
        if let Some(r) = &mut self.rows {
            r.forget();
        }
    }

    // ---- the history ----

    /// After a move of `h` (`how`) from `before`: a jump, or the first step of a run, records `before` and drops what
    /// was ahead.  No move, or none from a selection, records nothing.
    fn record(&mut self, h: u32, before: Option<Id>, how: Move) {
        let after = self.selection(h);
        let Some(i) = self.insts.get_mut(&h) else {
            return;
        };
        let Some(before) = before.filter(|b| after.as_ref() != Some(b)) else {
            return;
        };
        if how == Move::Jump || !i.stepping {
            i.back.push_back(before);
            if i.back.len() > HISTORY {
                i.back.pop_front();
            }
            i.forward.clear();
        }
        i.stepping = how == Move::Step;
    }

    /// `back` (`fwd` false) or `forward`: the newest entry of that ring the model still holds and that is not the
    /// selection; the selection goes to the other ring.  None left: the selection stays.
    fn travel(&mut self, h: u32, fwd: bool) -> Option<Id> {
        let cur = self.selection(h);
        let i = self.insts.get_mut(&h)?;
        i.stepping = false;
        let m = self.model.as_ref()?;
        let (from, to) = if fwd {
            (&mut i.forward, &mut i.back)
        } else {
            (&mut i.back, &mut i.forward)
        };
        let target = loop {
            match from.pop_back() {
                None => return cur,
                Some(t) if m.nodes.contains_key(&t) && Some(&t) != cur.as_ref() => break t,
                Some(_) => {}
            }
        };
        if let Some(c) = cur {
            to.push_back(c);
            if to.len() > HISTORY {
                to.pop_front();
            }
        }
        self.put(h, Some(target))
    }

    /// Whether `link` names a direction (`up`, `down`, `left`, `right`, any with `:KIND`): a step, not a jump.
    fn direction(link: &str) -> bool {
        matches!(
            link.split_once(':').map_or(link, |(d, _)| d),
            "up" | "down" | "left" | "right"
        )
    }

    // ---- verbs ----

    /// Follow `link` from `h`'s selection: in a list form `up` and `down` step along the form's list; otherwise the
    /// link set of the instance's form.  With no selection, the first entry of the primary list.  A link that is null
    /// or leads outside the model leaves the selection.  The answer is the selection.
    pub fn follow(&mut self, h: u32, link: &str) -> Option<Id> {
        self.follow_step(h, link).id
    }

    /// [`View::follow`], answering `more` when the selection stayed, the link names a direction (`up`, `down`, either
    /// with `:KIND`; `left` and `right` too in the links scheme) and the primary list is a window that goes on that
    /// way: the next row may be in the next window.
    pub fn follow_step(&mut self, h: u32, link: &str) -> Step {
        if link == BACK || link == FORWARD {
            return Step {
                id: self.travel(h, link == FORWARD),
                more: None,
            };
        }
        let cur = self.selection(h);
        let id = self.follow_inner(h, link);
        let how = if Self::direction(link) {
            Move::Step
        } else {
            Move::Jump
        };
        self.record(h, cur.clone(), how);
        let more = match (&cur, &id) {
            (Some(c), Some(i)) if c == i => self.model.as_ref().and_then(|m| {
                let dir = link.split_once(':').map_or(link, |(d, _)| d);
                let d: i8 = match (dir, m.desc.scheme == Scheme::Tree) {
                    ("up", _) | ("left", false) => -1,
                    ("down", _) | ("right", false) => 1,
                    _ => return None,
                };
                let l = Self::primary_list(m).and_then(|l| m.lists.get(l))?;
                let goes_on = if d < 0 {
                    l.from > 0
                } else {
                    l.from + l.ids.len() < l.total.unwrap_or(0)
                };
                goes_on.then_some(d)
            }),
            _ => None,
        };
        Step { id, more }
    }

    /// LIST's window: (from, length, total) when the model holds a window of it, else none.
    pub fn window(&self, list: &str) -> Option<(usize, usize, usize)> {
        let l = self.model.as_ref()?.lists.get(list)?;
        let total = l.total?;
        (l.from > 0 || l.from + l.ids.len() < total).then_some((l.from, l.ids.len(), total))
    }

    fn follow_inner(&mut self, h: u32, link: &str) -> Option<Id> {
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
        if m.desc.scheme == Scheme::Tree {
            return self.tree_follow(h, &cur, link);
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
        let before = self.selection(h);
        let s = self.walk_inner(h, list, step, true);
        self.record(h, before, Move::Step);
        s
    }

    fn walk_inner(&mut self, h: u32, list: &str, step: i64, wrap: bool) -> Step {
        let cur = self.selection(h);
        let Some(m) = self.model.as_ref() else {
            return Step {
                id: None,
                more: None,
            };
        };
        // a tree's `visible` is the instance's own (never a window); every other list is the model's
        let shown;
        let (ids, from, total): (&[Id], usize, Option<usize>) =
            if list == VISIBLE && m.desc.scheme == Scheme::Tree {
                shown = self.visible(h);
                (&shown[..], 0, None)
            } else {
                match m.lists.get(list) {
                    Some(l) => (&l.ids[..], l.from, l.total),
                    None => (&[], 0, None),
                }
            };
        if ids.is_empty() {
            return Step {
                id: cur,
                more: None,
            };
        }
        let len = ids.len() as i64;
        let windowed = total.is_some_and(|t| from > 0 || from + ids.len() < t);
        let pos = cur.as_ref().and_then(|c| ids.iter().position(|x| x == c));
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
                if windowed {
                    if n < 0 && from > 0 {
                        more = Some(-1);
                    } else if n >= len && from + ids.len() < total.unwrap_or(0) {
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
        let target = ids[at as usize].clone();
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
        let before = self.selection(h);
        let s = self.walk_to_inner(h, list, n);
        self.record(h, before, Move::Step);
        s
    }

    fn walk_to_inner(&mut self, h: u32, list: &str, n: usize) -> Step {
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

    // ---- the tree scheme ----

    /// Whether `id` is open for `h`: the model's flag `expanded` (when the descriptor names one), flipped by the
    /// instance's expanded set (`toggle`).
    pub fn is_open(&self, h: u32, id: &str) -> bool {
        let Some(m) = self.model.as_ref() else {
            return false;
        };
        let flag = m
            .desc
            .flags
            .iter()
            .position(|f| f == "expanded")
            .filter(|b| *b < 32)
            .is_some_and(|b| m.nodes.get(id).is_some_and(|n| n.flags & (1 << b) != 0));
        let toggled = self.insts.get(&h).is_some_and(|i| i.expanded.contains(id));
        flag != toggled
    }

    /// A tree's visible list for `h`: the entries of the primary list (the whole tree, parents before their
    /// children) whose ancestors are all open.  Empty for another scheme.
    pub fn visible(&self, h: u32) -> Vec<Id> {
        let Some(m) = self
            .model
            .as_ref()
            .filter(|m| m.desc.scheme == Scheme::Tree)
        else {
            return Vec::new();
        };
        let Some(l) = Self::primary_list(m).and_then(|l| m.lists.get(l)) else {
            return Vec::new();
        };
        let shown = |id: &Id| {
            let mut p = m.nodes.get(id).and_then(|n| n.parent.clone());
            // a model's depth is small; the bound only guards a cycle the hub sent by mistake
            for _ in 0..256 {
                match p {
                    None => return true,
                    Some(q) if !self.is_open(h, &q) => return false,
                    Some(q) => p = m.nodes.get(&q).and_then(|n| n.parent.clone()),
                }
            }
            false
        };
        l.ids
            .iter()
            .filter(|id| m.nodes.contains_key(*id) && shown(id))
            .cloned()
            .collect()
    }

    /// List `name` as `h` sees it: a tree's `visible` is the instance's, any other the model's (its window).
    pub fn list_of(&self, h: u32, name: &str) -> Vec<Id> {
        if name == VISIBLE
            && self
                .model
                .as_ref()
                .is_some_and(|m| m.desc.scheme == Scheme::Tree)
        {
            self.visible(h)
        } else {
            self.list(name)
        }
    }

    /// A tree's moves from `cur`: `up`/`down` the visible list (no wrap); `up:KIND`/`down:KIND` to the nearest visible
    /// node of that kind; `left` the parent; `right` the first child, when open.  Anything else stays.
    fn tree_follow(&mut self, h: u32, cur: &Id, link: &str) -> Option<Id> {
        let (dir, kind) = link
            .split_once(':')
            .map_or((link, None), |(d, k)| (d, Some(k)));
        let m = self.model.as_ref()?;
        let vis = self.visible(h);
        let pos = vis.iter().position(|x| x == cur);
        let target = match (dir, kind, pos) {
            ("up" | "down", None, _) => {
                return self
                    .walk_inner(h, VISIBLE, if dir == "up" { -1 } else { 1 }, false)
                    .id;
            }
            ("up" | "down", Some(k), Some(p)) => {
                let of_kind =
                    |id: &&Id| m.nodes.get(*id).is_some_and(|n| m.names.word(n.kind) == k);
                if dir == "up" {
                    vis[..p].iter().rev().find(of_kind).cloned()
                } else {
                    vis[p + 1..].iter().find(of_kind).cloned()
                }
            }
            ("left", None, _) => m
                .nodes
                .get(cur)
                .and_then(|n| n.parent.clone())
                .filter(|p| m.nodes.contains_key(p)),
            ("right", None, Some(p)) => vis
                .get(p + 1)
                .filter(|c| m.nodes.get(*c).and_then(|n| n.parent.as_ref()) == Some(cur))
                .cloned(),
            _ => None,
        };
        match target {
            Some(t) => self.put(h, Some(t)),
            None => Some(cur.clone()),
        }
    }

    /// Type-to-navigate: the visible sibling of `h`'s selection (none: the top level) whose label starts with `text`
    /// (case folded), searching from the one after the selection, wrapping; a longer `text` (the user typed on) from
    /// the selection itself.  No match leaves the selection.  The answer is the selection.
    pub fn prefix(&mut self, h: u32, text: &str) -> Option<Id> {
        let before = self.selection(h);
        let id = self.prefix_inner(h, text);
        self.record(h, before, Move::Step);
        id
    }

    fn prefix_inner(&mut self, h: u32, text: &str) -> Option<Id> {
        let m = self.model.as_ref()?;
        let cur = self.selection(h);
        let parent = cur
            .as_ref()
            .and_then(|c| m.nodes.get(c))
            .and_then(|n| n.parent.clone());
        let vis = self.visible(h);
        let sibs: Vec<&Id> = vis
            .iter()
            .filter(|id| m.nodes.get(*id).is_some_and(|n| n.parent == parent))
            .collect();
        let want = text.to_lowercase();
        let label = m.names.get("label");
        let matches = |id: &Id| {
            label
                .and_then(|l| m.nodes.get(id)?.field(l))
                .is_some_and(|v| match v {
                    crate::wire::Val::Str(s) => s.to_lowercase().starts_with(&want),
                    crate::wire::Val::Int(i) => i.to_string().starts_with(&want),
                })
        };
        let n = sibs.len();
        let at = cur
            .as_ref()
            .and_then(|c| sibs.iter().position(|s| *s == c))
            .map_or(0, |p| p + usize::from(text.chars().count() <= 1));
        let hit = (0..n)
            .map(|k| sibs[(at + k) % n])
            .find(|id| matches(id))
            .cloned();
        match hit {
            Some(id) => self.put(h, Some(id)),
            None => cur,
        }
    }

    /// Select `id` (nil clears); an id the model doesn't hold leaves the selection.  The answer is the selection.
    pub fn select(&mut self, h: u32, id: Option<&str>) -> Option<Id> {
        let before = self.selection(h);
        let to = self.select_inner(h, id);
        self.record(h, before, Move::Jump);
        to
    }

    fn select_inner(&mut self, h: u32, id: Option<&str>) -> Option<Id> {
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
        if tick {
            i.ticks += 1;
        }
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

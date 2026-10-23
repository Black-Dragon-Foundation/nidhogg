//! Stacking: each layer's parent, and each parent's children bottom to top.
//!
//! Parent 0 is the top level (a root's frame layer). A child placed, raised or reparented goes on top of its
//! parent's children; lowered, to the bottom. The parent links never form a cycle: a move that would is refused.

use std::collections::BTreeMap;

#[derive(Debug, Default, Clone)]
pub struct Stack {
    parent: BTreeMap<u64, u64>,
    children: BTreeMap<u64, Vec<u64>>,
}

impl Stack {
    pub fn parent_of(&self, id: u64) -> Option<u64> {
        self.parent.get(&id).copied()
    }

    /// PARENT's children, bottom to top.
    pub fn children(&self, parent: u64) -> &[u64] {
        self.children.get(&parent).map_or(&[], Vec::as_slice)
    }

    /// Whether `a` is `b` or one of `b`'s ancestors.
    pub fn is_ancestor(&self, a: u64, b: u64) -> bool {
        let mut at = b;
        loop {
            if at == a {
                return true;
            }
            match self.parent.get(&at) {
                Some(&p) if p != 0 => at = p,
                _ => return false,
            }
        }
    }

    /// Whether putting ID under PARENT would make a cycle.
    pub fn would_cycle(&self, id: u64, parent: u64) -> bool {
        parent != 0 && self.is_ancestor(id, parent)
    }

    fn detach(&mut self, id: u64) {
        if let Some(p) = self.parent.remove(&id)
            && let Some(v) = self.children.get_mut(&p)
        {
            v.retain(|&c| c != id);
            if v.is_empty() {
                self.children.remove(&p);
            }
        }
    }

    /// Put ID on top of PARENT's children (wherever it was). The caller has checked [`Stack::would_cycle`].
    pub fn place_top(&mut self, id: u64, parent: u64) {
        self.detach(id);
        self.parent.insert(id, parent);
        self.children.entry(parent).or_default().push(id);
    }

    /// Raise ID to the top of its siblings; whether the order changed.
    pub fn raise(&mut self, id: u64) -> bool {
        let Some(p) = self.parent_of(id) else {
            return false;
        };
        let v = self.children.entry(p).or_default();
        if v.last() == Some(&id) {
            return false;
        }
        v.retain(|&c| c != id);
        v.push(id);
        true
    }

    /// Lower ID to the bottom of its siblings; whether the order changed.
    pub fn lower(&mut self, id: u64) -> bool {
        let Some(p) = self.parent_of(id) else {
            return false;
        };
        let v = self.children.entry(p).or_default();
        if v.first() == Some(&id) {
            return false;
        }
        v.retain(|&c| c != id);
        v.insert(0, id);
        true
    }

    /// ID and everything under it, ID first.
    pub fn subtree(&self, id: u64) -> Vec<u64> {
        let mut out = vec![id];
        let mut i = 0;
        while i < out.len() {
            out.extend_from_slice(self.children(out[i]));
            i += 1;
        }
        out
    }

    /// Remove ID and everything under it; the ids removed, ID first.
    pub fn remove_subtree(&mut self, id: u64) -> Vec<u64> {
        let ids = self.subtree(id);
        self.detach(id);
        for &c in &ids {
            self.parent.remove(&c);
            self.children.remove(&c);
        }
        ids
    }
}

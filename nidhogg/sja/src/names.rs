//! A view's vocabulary: kinds, field names, link names, forms and edge kinds interned into `u32` symbols.  Node ids are
//! not interned: they are shared `Arc<str>`s, freed with the last node, edge or list that holds them.

use std::collections::HashMap;
use std::sync::Arc;

/// A node id.
pub type Id = Arc<str>;

/// An interned word of the vocabulary.
pub type Sym = u32;

/// The most words one view's vocabulary holds; a model that needs more is too large (a bug, not a mode).
pub const MAX_WORDS: usize = 65_536;

#[derive(Default, Debug, Clone)]
pub struct Names {
    map: HashMap<Box<str>, Sym>,
    words: Vec<Box<str>>,
}

impl Names {
    /// The symbol of `w`, interned if new; `None` when the vocabulary is full.
    pub fn intern(&mut self, w: &str) -> Option<Sym> {
        if let Some(&s) = self.map.get(w) {
            return Some(s);
        }
        if self.words.len() >= MAX_WORDS {
            return None;
        }
        let s = Sym::try_from(self.words.len()).ok()?;
        self.words.push(w.into());
        self.map.insert(w.into(), s);
        Some(s)
    }

    /// The symbol of `w`, if interned.
    pub fn get(&self, w: &str) -> Option<Sym> {
        self.map.get(w).copied()
    }

    /// The word of `s`.
    pub fn word(&self, s: Sym) -> &str {
        self.words.get(s as usize).map_or("", |w| w)
    }

    pub fn len(&self) -> usize {
        self.words.len()
    }

    pub fn is_empty(&self) -> bool {
        self.words.is_empty()
    }

    /// The bytes the vocabulary holds, for the tally.
    pub fn bytes(&self) -> usize {
        self.words.iter().map(|w| 2 * w.len() + 48).sum()
    }
}

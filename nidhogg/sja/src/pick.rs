//! A local list (`sja::pick`): candidates loaded once, then for each key filtered, scored, sorted and grouped here, so
//! the editor's Lisp only maps the answer's indices to its labels (the command palette).
//!
//! A list is text, one candidate per line: `KIND RECENT LABEL`, tab separated.  KIND is 0 for a command, 1 for a
//! file; RECENT the candidate's place in the recent runs (0 the newest) or -1.  A label is `Category: Title`.
//!
//! A candidate matches when its label (any case) holds every typed word.  Its score is the palette's: each word 2 at
//! a word's start in the title, 1 inside a word, 0 when only the category holds it; 1 more when it starts the word
//! right after the previous word's; and 1 for a recent run, with text typed.  The order is the higher score, then the
//! newer run, then the list's own order; then the groups, Commands, Files, Agents (a file, or a label in `Files:`,
//! `File:` or `Go:`, is a file's; `Agents:`, `Agent:` or `Work:` an agent's).  The lists are kept by handle, at most
//! [`LISTS_MAX`]; the oldest goes first.

use std::collections::BTreeMap;
use std::sync::Mutex;

/// The most lists kept at once.
pub const LISTS_MAX: usize = 16;

struct Item {
    group: u8,
    recent: Option<u32>,
    /// The label, lower case, a character for a character.
    lower: Vec<char>,
    /// Where the title starts in it.
    title: usize,
}

/// A list of candidates, as loaded.
pub struct List {
    items: Vec<Item>,
}

fn lower(c: char) -> char {
    c.to_lowercase().next().unwrap_or(c)
}

fn alnum(c: char) -> bool {
    c.is_alphabetic() || c.is_numeric()
}

fn starts(s: &str, prefixes: &[&str]) -> bool {
    prefixes.iter().any(|p| s.starts_with(p))
}

/// The first place at or after FROM where W is in S.
fn find(s: &[char], w: &[char], from: usize) -> Option<usize> {
    if w.is_empty() || w.len() > s.len() {
        return None;
    }
    (from..=s.len() - w.len()).find(|&i| s[i..i + w.len()] == *w)
}

/// The start of the word after the one at POS in T.
fn word_after(t: &[char], pos: usize) -> Option<usize> {
    let mut i = pos;
    while i < t.len() && alnum(t[i]) {
        i += 1;
    }
    while i < t.len() && !alnum(t[i]) {
        i += 1;
    }
    (i < t.len()).then_some(i)
}

/// The palette's score of title T for WORDS (see the module's doc).
fn score(t: &[char], words: &[Vec<char>]) -> u32 {
    let (mut score, mut prev) = (0, None);
    for w in words {
        let (mut pos, mut inside, mut from) = (None, None, 0);
        while pos.is_none() {
            let Some(at) = find(t, w, from) else { break };
            if at == 0 || !alnum(t[at - 1]) {
                pos = Some(at);
            } else {
                inside.get_or_insert(at);
                from = at + 1;
            }
        }
        match (pos, inside) {
            (Some(p), _) => {
                let next = prev.is_some_and(|q| word_after(t, q) == Some(p));
                score += 2 + u32::from(next);
                prev = Some(p);
            }
            (None, Some(i)) => {
                score += 1;
                prev = Some(i);
            }
            (None, None) => prev = None,
        }
    }
    score
}

impl List {
    /// The list TEXT describes, or none when a line is not `KIND RECENT LABEL`.
    pub fn read(text: &str) -> Option<List> {
        let mut items = Vec::new();
        for line in text.lines().filter(|l| !l.is_empty()) {
            let mut f = line.splitn(3, '\t');
            let kind: u8 = f.next()?.parse().ok().filter(|k| *k <= 1)?;
            let recent: i64 = f.next()?.parse().ok()?;
            let label = f.next()?;
            let group = if kind == 1 || starts(label, &["Files: ", "File: ", "Go: "]) {
                1
            } else if starts(label, &["Agents: ", "Agent: ", "Work: "]) {
                2
            } else {
                0
            };
            let lower: Vec<char> = label.chars().map(lower).collect();
            let title = label
                .find(": ")
                .map_or(0, |b| label[..b].chars().count() + 2);
            items.push(Item {
                group,
                recent: u32::try_from(recent).ok(),
                lower,
                title,
            });
        }
        Some(List { items })
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// The indices of the first N candidates for INPUT, in the palette's order (see the module's doc).
    pub fn top(&self, input: &str, n: usize) -> Vec<usize> {
        let words: Vec<Vec<char>> = input
            .split_whitespace()
            .map(|w| w.chars().map(lower).collect())
            .collect();
        let mut keyed: Vec<(u8, u32, u32, usize)> = self
            .items
            .iter()
            .enumerate()
            .filter(|(_, it)| words.iter().all(|w| find(&it.lower, w, 0).is_some()))
            .map(|(i, it)| {
                let s = if words.is_empty() {
                    0
                } else {
                    score(&it.lower[it.title..], &words) + u32::from(it.recent.is_some())
                };
                (it.group, u32::MAX - s, it.recent.unwrap_or(u32::MAX), i)
            })
            .collect();
        keyed.sort_unstable();
        keyed.into_iter().take(n).map(|k| k.3).collect()
    }
}

struct Lists {
    next: u32,
    lists: BTreeMap<u32, List>,
}

static LISTS: Mutex<Lists> = Mutex::new(Lists {
    next: 0,
    lists: BTreeMap::new(),
});

fn lists<R>(f: impl FnOnce(&mut Lists) -> R) -> R {
    let mut l = LISTS.lock().unwrap_or_else(|e| e.into_inner());
    f(&mut l)
}

/// Keep LIST: its handle (the oldest list goes past [`LISTS_MAX`]).
pub fn load(list: List) -> u32 {
    lists(|l| {
        l.next = l.next.wrapping_add(1).max(1);
        let h = l.next;
        l.lists.insert(h, list);
        while l.lists.len() > LISTS_MAX {
            let Some(&old) = l.lists.keys().find(|&&k| k != h) else {
                break;
            };
            l.lists.remove(&old);
        }
        h
    })
}

/// List H's first N for INPUT; none when H is no list.
pub fn top(h: u32, input: &str, n: usize) -> Option<Vec<usize>> {
    lists(|l| l.lists.get(&h).map(|list| list.top(input, n)))
}

/// Drop list H; false when it was none.
pub fn close(h: u32) -> bool {
    lists(|l| l.lists.remove(&h).is_some())
}

/// How many lists are kept.
pub fn kept() -> usize {
    lists(|l| l.lists.len())
}

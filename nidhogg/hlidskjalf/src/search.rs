//! Minimap search (design X1): a query's hits over the whole text, found in slices of the main thread's time.
//!
//! A literal query, or an Emacs regexp in the subset [`translate`] carries, runs here; anything else is Lisp's
//! (`re-search-forward` in idle slices), so the counts stay Emacs's own.  The subset never matches a newline, so the
//! text is scanned a line at a time, each line its own haystack: `^` and `$` are the line's ends, as in Emacs.  A
//! scan starts over when the text changes; its hits are positions (with the column a hit starts at, tabs expanded),
//! kept sorted.  Empty matches are not hits (`how-many` counts them).

use regex_lite::{Regex, RegexBuilder};
use std::time::{Duration, Instant};

/// The most hits kept (the count goes on past it).
pub const HITS_MAX: usize = 1 << 20;

/// One hit: its first position, the position after it and the column it starts at.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Hit {
    pub pos: u64,
    pub end: u64,
    pub col: u32,
}

/// Where a scan stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Running = 1,
    Done = 2,
    /// The text holds what the scan cannot read (bytes that are not UTF-8): the query is Lisp's.
    Fallback = 3,
}

/// A query being scanned for, and what it found.
pub struct Search {
    re: Regex,
    /// The next line's byte offset and position.
    at: usize,
    pos: u64,
    hits: Vec<Hit>,
    count: u64,
    state: State,
    current: Option<u64>,
}

/// Emacs regexp QUERY (or, with LITERAL, its plain text) in Rust regex syntax, or `None` when it is outside the subset
/// this module carries: a newline anywhere; syntax classes and categories (`\w`, `\s-`, `\_<`, `\b`, `\<`, `[:alpha:]`,
/// …); buffer and point anchors; back references; numbered groups; a negated bracket (it matches a newline in Emacs);
/// an escape Emacs gives no meaning.  With CASE-FOLD the query must be ASCII (the regex folds ASCII only).
pub fn translate(query: &str, literal: bool, case_fold: bool) -> Option<String> {
    if query.contains('\n') || (case_fold && !query.is_ascii()) {
        return None;
    }
    let lit = |c: char, out: &mut String| {
        if c.is_ascii_punctuation() || c == ' ' {
            out.push_str(&format!("\\x{{{:02x}}}", c as u32));
        } else {
            out.push(c);
        }
    };
    let mut out = String::new();
    if literal {
        query.chars().for_each(|c| lit(c, &mut out));
        return Some(out);
    }
    let cs: Vec<char> = query.chars().collect();
    let mut i = 0;
    // at a place where Emacs reads `^` as an anchor and `*`, `+`, `?` as plain characters
    let mut start = true;
    while i < cs.len() {
        let c = cs[i];
        let was_start = start;
        start = false;
        match c {
            '\\' => {
                let d = *cs.get(i + 1)?;
                i += 2;
                match d {
                    '(' => {
                        if cs.get(i) == Some(&'?') {
                            if cs.get(i + 1) != Some(&':') {
                                return None;
                            }
                            i += 2;
                            out.push_str("(?:");
                        } else {
                            out.push('(');
                        }
                        start = true;
                    }
                    ')' => out.push(')'),
                    '|' => {
                        out.push('|');
                        start = true;
                    }
                    '{' => {
                        let close = (i..cs.len().saturating_sub(1))
                            .find(|&k| cs[k] == '\\' && cs[k + 1] == '}')?;
                        let body: String = cs[i..close].iter().collect();
                        if was_start || !body.chars().all(|c| c.is_ascii_digit() || c == ',') {
                            return None;
                        }
                        let body = if body.starts_with(',') {
                            format!("0{body}")
                        } else {
                            body
                        };
                        out.push('{');
                        out.push_str(&body);
                        out.push('}');
                        i = close + 2;
                    }
                    '.' | '*' | '+' | '?' | '[' | ']' | '^' | '$' | '\\' => lit(d, &mut out),
                    _ => return None,
                }
                continue;
            }
            '[' => {
                let (class, next) = bracket(&cs, i)?;
                out.push_str(&class);
                i = next;
                continue;
            }
            '^' if was_start => out.push('^'),
            '$' if i + 1 == cs.len()
                || (cs[i + 1] == '\\' && matches!(cs.get(i + 2), Some(')' | '|'))) =>
            {
                out.push('$')
            }
            '*' | '+' | '?' if !was_start => out.push(c),
            '.' => out.push('.'),
            _ => lit(c, &mut out),
        }
        i += 1;
    }
    Some(out)
}

/// The bracket expression at CS[I] (`[`) in Rust syntax, and the index after it; `None` outside the subset.
fn bracket(cs: &[char], i: usize) -> Option<(String, usize)> {
    let mut k = i + 1;
    if cs.get(k) == Some(&'^') {
        return None;
    }
    let enc = |c: char| {
        if c.is_ascii_punctuation() || c == ' ' {
            format!("\\x{{{:02x}}}", c as u32)
        } else {
            c.to_string()
        }
    };
    let mut out = String::from("[");
    let mut first = true;
    loop {
        let c = *cs.get(k)?;
        if c == ']' && !first {
            out.push(']');
            return Some((out, k + 1));
        }
        if c == '[' && cs.get(k + 1) == Some(&':') {
            return None;
        }
        first = false;
        // a range: C-E, E not the closing bracket
        if cs.get(k + 1) == Some(&'-') && cs.get(k + 2).is_some_and(|&e| e != ']') {
            let e = cs[k + 2];
            if e < c {
                return None;
            }
            out.push_str(&enc(c));
            out.push('-');
            out.push_str(&enc(e));
            k += 3;
        } else {
            out.push_str(&enc(c));
            k += 1;
        }
    }
}

impl Search {
    /// A scan for QUERY (`translate`'s arguments); `None` outside the subset.
    pub fn new(query: &str, literal: bool, case_fold: bool) -> Option<Search> {
        let s = translate(query, literal, case_fold)?;
        let re = RegexBuilder::new(&s)
            .case_insensitive(case_fold)
            .build()
            .ok()?;
        Some(Search {
            re,
            at: 0,
            pos: 1,
            hits: Vec::new(),
            count: 0,
            state: State::Running,
            current: None,
        })
    }

    /// Start over (the text changed).
    pub fn restart(&mut self) {
        self.at = 0;
        self.pos = 1;
        self.hits.clear();
        self.count = 0;
        self.state = State::Running;
    }

    pub fn state(&self) -> State {
        self.state
    }

    /// The hits found (all of them, past `HITS_MAX` not kept).
    pub fn count(&self) -> u64 {
        self.count
    }

    /// The hits whose first position is in [BEG, END).
    pub fn hits(&self, beg: u64, end: u64) -> &[Hit] {
        let a = self.hits.partition_point(|h| h.pos < beg);
        let b = self.hits.partition_point(|h| h.pos < end).max(a);
        &self.hits[a..b]
    }

    /// The current hit (drawn with a ring): the one starting at POS, or none.
    pub fn set_current(&mut self, pos: Option<u64>) {
        self.current = pos;
    }

    pub fn current(&self) -> Option<u64> {
        self.current
    }

    /// How many hits start before POS.
    pub fn index(&self, pos: u64) -> usize {
        self.hits.partition_point(|h| h.pos < pos)
    }

    /// Scan the text in PARTS (the whole text, its parts in order; MULTIBYTE as the model's, tabs TAB-WIDTH wide)
    /// for up to BUDGET, then stop at a line's end.
    pub fn step(
        &mut self,
        parts: &[&[u8]],
        multibyte: bool,
        tab_width: u16,
        budget: Duration,
    ) -> State {
        if self.state != State::Running {
            return self.state;
        }
        let t0 = Instant::now();
        let total: usize = parts.iter().map(|p| p.len()).sum();
        let mut scratch = Vec::new();
        let mut n = 0u32;
        while self.at < total {
            let line = line_at(parts, self.at, &mut scratch);
            let len = line.len();
            let body = line.strip_suffix(b"\n").unwrap_or(line);
            if !multibyte && !body.is_ascii() {
                self.state = State::Fallback;
                return self.state;
            }
            let Ok(s) = std::str::from_utf8(body) else {
                self.state = State::Fallback;
                return self.state;
            };
            for m in self.re.find_iter(s).filter(|m| !m.is_empty()) {
                self.count += 1;
                if self.hits.len() < HITS_MAX {
                    let before = &s[..m.start()];
                    let pos = self.pos + before.chars().count() as u64;
                    let col = before.chars().fold(0u32, |c, ch| {
                        if ch == '\t' {
                            let tw = u32::from(tab_width.max(1));
                            (c / tw + 1) * tw
                        } else {
                            c + 1
                        }
                    });
                    self.hits.push(Hit {
                        pos,
                        end: pos + m.as_str().chars().count() as u64,
                        col,
                    });
                }
            }
            self.pos += crate::sections::chars(line, multibyte);
            self.at += len;
            n += 1;
            if n.is_multiple_of(256) && t0.elapsed() >= budget {
                return self.state;
            }
        }
        self.state = State::Done;
        self.state
    }
}

/// The line (its newline included) at byte AT of the text in PARTS; one that runs from a part into the next is copied
/// into SCRATCH.
fn line_at<'a>(parts: &[&'a [u8]], at: usize, scratch: &'a mut Vec<u8>) -> &'a [u8] {
    let mut off = at;
    let mut k = 0;
    while k < parts.len() && off >= parts[k].len() {
        off -= parts[k].len();
        k += 1;
    }
    let p = parts[k];
    if let Some(nl) = p[off..].iter().position(|&b| b == b'\n') {
        return &p[off..off + nl + 1];
    }
    if k + 1 == parts.len() {
        return &p[off..];
    }
    scratch.clear();
    scratch.extend_from_slice(&p[off..]);
    for q in &parts[k + 1..] {
        match q.iter().position(|&b| b == b'\n') {
            Some(nl) => {
                scratch.extend_from_slice(&q[..nl + 1]);
                break;
            }
            None => scratch.extend_from_slice(q),
        }
    }
    scratch
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_subset_translates_and_the_rest_does_not() {
        assert_eq!(translate("a.b", true, false).as_deref(), Some(r"a\x{2e}b"));
        assert_eq!(
            translate(r"\(foo\|ba[rz]\)+$", false, false).as_deref(),
            Some(r"(foo|ba[rz])+$")
        );
        assert_eq!(
            translate(r"^x\{2,\}", false, false).as_deref(),
            Some(r"^x{2,}")
        );
        assert_eq!(
            translate(r"x\{,3\}", false, false).as_deref(),
            Some(r"x{0,3}")
        );
        // `^` not at a start, `*` at one, `(` and `{` are plain characters in Emacs
        assert_eq!(translate("a^*", false, false).as_deref(), Some(r"a\x{5e}*"));
        assert_eq!(translate("*a", false, false).as_deref(), Some(r"\x{2a}a"));
        assert_eq!(
            translate("f(x){", false, false).as_deref(),
            Some(r"f\x{28}x\x{29}\x{7b}")
        );
        assert_eq!(
            translate("[]a-]", false, false).as_deref(),
            Some(r"[\x{5d}a\x{2d}]")
        );
        for q in [
            r"\w+",
            r"\_<x",
            r"\bx",
            r"\(?1:x\)",
            r"\1",
            "[^a]",
            "[[:alpha:]]",
            "a\nb",
            r"\`a",
            r"\s-",
        ] {
            assert_eq!(translate(q, false, false), None, "{q}");
        }
        assert_eq!(translate("é", false, true), None);
        assert!(translate("é", false, false).is_some());
    }

    fn scan(text: &str, q: &str, literal: bool, fold: bool) -> (State, Vec<Hit>) {
        let mut s = Search::new(q, literal, fold).unwrap();
        // the text in two parts, split inside a line, as a gap would
        let b = text.as_bytes();
        let cut = b.len() / 2;
        let st = s.step(&[&b[..cut], &b[cut..]], true, 8, Duration::from_secs(10));
        (st, s.hits.clone())
    }

    #[test]
    fn hits_are_positions_with_their_columns() {
        let text = "ab foo\n\tfoo é foo\nFOO\n";
        let (st, hits) = scan(text, "foo", true, false);
        assert_eq!(st, State::Done);
        let at: Vec<(u64, u64, u32)> = hits.iter().map(|h| (h.pos, h.end, h.col)).collect();
        assert_eq!(at, [(4, 7, 3), (9, 12, 8), (15, 18, 14)]);
        let (_, hits) = scan(text, "foo", true, true);
        assert_eq!(hits.len(), 4);
        // empty matches are not hits; anchors are the lines'
        let (_, hits) = scan(text, "x*", false, false);
        assert!(hits.is_empty());
        let (_, hits) = scan(text, "^.", false, false);
        assert_eq!(hits.iter().map(|h| h.pos).collect::<Vec<_>>(), [1, 8, 19]);
        let (_, hits) = scan(text, "o$", false, false);
        assert_eq!(hits.iter().map(|h| h.pos).collect::<Vec<_>>(), [6, 17]);
    }

    #[test]
    fn a_text_that_is_not_utf8_is_lisps() {
        let mut s = Search::new("a", true, false).unwrap();
        assert_eq!(
            s.step(&[b"a\n\xc0\x80a\n"], true, 8, Duration::from_secs(1)),
            State::Fallback
        );
        let mut s = Search::new("a", true, false).unwrap();
        assert_eq!(
            s.step(&[b"a\n\xe9a\n"], false, 8, Duration::from_secs(1)),
            State::Fallback
        );
    }

    #[test]
    fn a_scan_stops_after_its_budget_and_goes_on() {
        let text = "foo\n".repeat(20_000);
        let mut s = Search::new("foo", true, false).unwrap();
        assert_eq!(
            s.step(&[text.as_bytes()], true, 8, Duration::ZERO),
            State::Running
        );
        assert_eq!(s.count(), 256);
        while s.step(&[text.as_bytes()], true, 8, Duration::ZERO) == State::Running {}
        assert_eq!(s.count(), 20_000);
        assert_eq!(s.index(9), 2);
        assert_eq!(s.hits(5, 13).len(), 2);
    }
}

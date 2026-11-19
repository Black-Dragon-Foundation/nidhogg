//! Hlidskjalf ("the high seat"): a buffer text's line model for the minimap, the scroll bar's marks and folding.
//!
//! One model per buffer text, keyed by the host (Nidhogg keys it by the text's address, so indirect buffers share
//! it).  It holds the text's lines (`index`), the marks the owner sends, the fold table, the hidden lines and the
//! trail of visited positions.  Positions are Emacs's: 1-based characters over the whole text, narrowing ignored.
//! Marks, folds, hidden ranges and the trail are positions shifted on every edit; lines come from the index.
//!
//! The first build of a large text runs on a thread of its own from a copy of the text; edits made meanwhile are
//! kept and replayed when it is installed.  Everything else runs on the host's main thread.

pub mod abi;
pub mod derived;
pub mod index;
pub mod search;
pub mod sections;

use std::collections::VecDeque;
use std::thread::JoinHandle;

pub use index::{BLANK, Line, LineIndex, RUNS, Run, Widths};
use regex_lite::Regex;
pub use sections::Section;
use std::sync::{Arc, Mutex};

/// The characters of a line that class runs cover (a minimap row is narrower).
pub const CLASS_COLUMNS: u64 = 512;

/// A mark: an owner's position (with an optional end, for a run of lines) in a lane, of a class and kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub struct Mark {
    pub pos: u64,
    /// The end of the run the mark covers, or 0.
    pub end: u64,
    pub lane: u8,
    pub class: u8,
    pub prio: u8,
    pub kind: u8,
}

/// A span of a display class from a class producer (a highlights query's capture): BEG and END positions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub struct Span {
    pub beg: u64,
    pub end: u64,
    pub class: u8,
}

/// A fold range: BEG and END positions, its KIND, DEPTH (set by the model: 0 outermost) and SOURCE.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub struct Fold {
    pub beg: u64,
    pub end: u64,
    pub kind: u8,
    pub depth: u8,
    pub source: u8,
}

/// The trail's length (design §5.3, M4).
pub const TRAIL: usize = 64;

/// An edit kept while the first build runs: START, OLD-END, NEW-END and the new text of its whole lines.
struct Pending {
    start: u64,
    old_end: u64,
    new_end: u64,
    seg: Vec<u8>,
}

/// Why an edit was not applied: the model's lines no longer match the text, and it must be built again.
#[derive(Debug, PartialEq, Eq)]
pub struct Stale;

/// What the build thread hands back: the lines, the MARK sections and the headings found in them.
type Built = (LineIndex, Vec<Section>, Vec<Section>);

/// The host's current character widths (`hlid_set_widths`): a model's lines are built with the snapshot current then.
static WIDTHS: Mutex<Option<Arc<Widths>>> = Mutex::new(None);

/// The current character widths snapshot (all 1 until the host sends one).
pub fn widths() -> Arc<Widths> {
    WIDTHS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get_or_insert_with(Default::default)
        .clone()
}

/// Make RANGES (`(first, last, width)`, the characters whose width is not 1) the current widths; returns its
/// generation.  Models built before keep theirs until they are built again.
pub fn set_widths(ranges: Vec<(u32, u32, u8)>) -> u64 {
    let mut w = WIDTHS.lock().unwrap_or_else(|e| e.into_inner());
    let next = w.as_ref().map_or(0, |w| w.generation) + 1;
    *w = Some(Arc::new(Widths::new(next, ranges)));
    next
}

/// The most buffers whose widest line a model keeps (hscroll design §3.2: one entry per buffer).
const WIDEST_KEPT: usize = 8;

pub struct Model {
    multibyte: bool,
    tab_width: u16,
    /// The character widths the lines were built with.
    widths: Arc<Widths>,
    /// The widest line per buffer: (buffer, generation, BEGV, ZV, columns), the latest first.
    widest: Vec<(u64, u64, u64, u64, u32)>,
    idx: Option<LineIndex>,
    building: Option<(JoinHandle<Built>, Vec<Pending>)>,
    /// The MARK sections' expression, and the sections found with it (kept with the lines: built and edited with them).
    section_re: Option<Regex>,
    sections: Vec<Section>,
    /// The headings' expression (its first group the heading's marks, their length its level) and the headings found.
    heading_re: Option<Regex>,
    headings: Vec<Section>,
    /// Whether the model finds indentation ranges itself.
    indent_folds: bool,
    /// The most fold ranges kept, over all sources.
    fold_max: usize,
    /// The sources whose folds are nodes not yet widened to whole lines (sent while the lines were built).
    raw_folds: Vec<u8>,
    /// Class runs were refused while the lines were built: the producer runs over the whole text again.
    classes_missed: bool,
    /// Bumped by every change of the lines (not of the hidden ones); the found ranges are of `derived_at`'s lines.
    text_gen: u64,
    derived_at: u64,
    stale: bool,
    marks: Vec<Mark>,
    folds: Vec<Fold>,
    hidden: Vec<(u64, u64)>,
    /// Each hidden range's name (a named fold's, F4), by the range's place in `hidden`.
    hidden_names: Vec<Option<Box<str>>>,
    /// The hidden ranges as line ranges [a, b), merged, with the hidden lines before each (rebuilt on demand).
    hidden_lines: Option<Vec<(u64, u64, u64)>>,
    trail: VecDeque<u64>,
    /// The minimap search's query and hits (design X1).
    search: Option<search::Search>,
    pub edits: u64,
    /// Bumped by every change of the lines or of the hidden ones (what a drawing of the rows depends on).
    generation: u64,
}

impl Model {
    /// A model of the text in PARTS; built on a thread of its own when BACKGROUND.
    pub fn build(parts: &[&[u8]], multibyte: bool, tab_width: u16, background: bool) -> Model {
        let mut m = Model {
            multibyte,
            tab_width,
            widths: widths(),
            widest: Vec::new(),
            idx: None,
            building: None,
            section_re: None,
            sections: Vec::new(),
            heading_re: None,
            headings: Vec::new(),
            indent_folds: false,
            fold_max: 20000,
            raw_folds: Vec::new(),
            classes_missed: false,
            text_gen: 0,
            derived_at: u64::MAX,
            stale: false,
            marks: Vec::new(),
            folds: Vec::new(),
            hidden: Vec::new(),
            hidden_names: Vec::new(),
            hidden_lines: None,
            trail: VecDeque::new(),
            search: None,
            edits: 0,
            generation: 0,
        };
        m.rebuild(parts, background);
        m
    }

    /// Read the text again (lines only: marks, folds and the trail are positions and stay).
    pub fn rebuild(&mut self, parts: &[&[u8]], background: bool) {
        self.generation += 1;
        self.text_gen += 1;
        self.widths = widths();
        self.stale = false;
        self.hidden_lines = None;
        if let Some(s) = &mut self.search {
            s.restart();
        }
        if background {
            let text: Vec<u8> = parts.concat();
            let (mb, tw, ws) = (self.multibyte, self.tab_width, self.widths.clone());
            let (re, hre) = (self.section_re.clone(), self.heading_re.clone());
            let h = std::thread::spawn(move || {
                let idx = LineIndex::new(index::parse_with(&[&text], mb, tw, true, &ws));
                let found = re.map_or_else(Vec::new, |re| sections::scan(&re, &text, 1, mb));
                let heads = hre.map_or_else(Vec::new, |re| sections::scan(&re, &text, 1, mb));
                (idx, found, heads)
            });
            self.idx = None;
            self.sections.clear();
            self.headings.clear();
            self.building = Some((h, Vec::new()));
        } else {
            self.building = None;
            self.idx = Some(LineIndex::new(index::parse_with(
                parts,
                self.multibyte,
                self.tab_width,
                true,
                &self.widths,
            )));
            let text = parts.concat();
            let scan = |re: &Option<Regex>| match re {
                Some(re) => sections::scan(re, &text, 1, self.multibyte),
                None => Vec::new(),
            };
            self.sections = scan(&self.section_re);
            self.headings = scan(&self.heading_re);
        }
    }

    pub fn set_text_kind(&mut self, multibyte: bool, tab_width: u16) {
        self.multibyte = multibyte;
        self.tab_width = tab_width;
    }

    /// Whether the lines are being built.
    pub fn building(&self) -> bool {
        self.building.is_some()
    }

    pub fn stale(&self) -> bool {
        self.stale
    }

    /// Install a finished build (replaying the edits made meanwhile); WAIT blocks until it finishes.
    pub fn poll(&mut self, wait: bool) {
        let done = match &self.building {
            Some((h, _)) => wait || h.is_finished(),
            None => false,
        };
        if !done {
            return;
        }
        let (h, log) = self.building.take().expect("building");
        self.generation += 1;
        self.text_gen += 1;
        match h.join() {
            Ok((idx, found, heads)) => {
                self.idx = Some(idx);
                self.sections = found;
                self.headings = heads;
                for p in log {
                    if self
                        .edit_lines(p.start, p.old_end, p.new_end, &[&p.seg])
                        .is_err()
                    {
                        self.idx = None;
                        self.stale = true;
                        return;
                    }
                }
            }
            Err(_) => {
                self.stale = true;
            }
        }
        self.hidden_lines = None;
    }

    /// The line index, when built.
    pub fn index(&mut self) -> Option<&LineIndex> {
        self.poll(false);
        self.idx.as_ref()
    }

    /// The text changed: the OLD-END − START characters from START were replaced by NEW-END − START ones.  SEG is
    /// the new text of the whole lines the change touched: from the start of START's line to the end of NEW-END's
    /// line, its newline included.
    pub fn edit(
        &mut self,
        start: u64,
        old_end: u64,
        new_end: u64,
        seg: &[&[u8]],
    ) -> Result<(), Stale> {
        self.edits += 1;
        self.generation += 1;
        self.text_gen += 1;
        let shift = |p: u64| -> u64 {
            if p <= start {
                p
            } else if p >= old_end {
                p - old_end + new_end
            } else {
                start
            }
        };
        for m in &mut self.marks {
            m.pos = shift(m.pos);
            if m.end != 0 {
                m.end = shift(m.end);
            }
        }
        for f in &mut self.folds {
            f.beg = shift(f.beg);
            f.end = shift(f.end);
        }
        self.folds.retain(|f| f.beg < f.end);
        // a hidden range the change cuts into hides other lines past it: every chunk's widest visible line again
        let cut = self
            .hidden
            .iter()
            .any(|r| (r.0 > start && r.0 < old_end) || (r.1 > start && r.1 < old_end));
        for r in &mut self.hidden {
            *r = (shift(r.0), shift(r.1));
        }
        let mut keep = self.hidden.iter().map(|r| r.0 < r.1);
        self.hidden_names.retain(|_| keep.next().unwrap_or(false));
        self.hidden.retain(|r| r.0 < r.1);
        for t in &mut self.trail {
            *t = shift(*t);
        }
        if let Some(s) = &mut self.search {
            s.restart();
        }
        self.hidden_lines = None;
        self.poll(false);
        if let Some((_, log)) = &mut self.building {
            log.push(Pending {
                start,
                old_end,
                new_end,
                seg: seg.concat(),
            });
            return Ok(());
        }
        if self.stale || self.idx.is_none() {
            return Err(Stale);
        }
        let r = self.edit_lines(start, old_end, new_end, seg);
        if r.is_err() {
            self.idx = None;
            self.stale = true;
        } else if cut && let Some(idx) = self.idx.as_mut() {
            idx.clear_visible();
        }
        r
    }

    fn edit_lines(
        &mut self,
        start: u64,
        old_end: u64,
        new_end: u64,
        seg: &[&[u8]],
    ) -> Result<(), Stale> {
        let idx = self.idx.as_mut().ok_or(Stale)?;
        if start < 1 || old_end < start || new_end < start || old_end > idx.chars() + 1 {
            return Err(Stale);
        }
        let l0 = idx.line_at(start - 1);
        let l1 = idx.line_at(old_end - 1);
        let old_chars = idx.start_of(l1 + 1) - idx.start_of(l0);
        // the segment reaches the end of the text when its last line is the text's last
        let to_end = l1 + 1 == idx.lines();
        let new = index::parse_with(seg, self.multibyte, self.tab_width, to_end, &self.widths);
        let new_chars: u64 = new.iter().map(|l| u64::from(l.len)).sum();
        if new_chars + old_end != old_chars + new_end {
            return Err(Stale);
        }
        let ls = idx.start_of(l0) + 1;
        idx.replace(l0, l1 - l0 + 1, new);
        let text = seg.concat();
        for (re, list) in [
            (&self.section_re, &mut self.sections),
            (&self.heading_re, &mut self.headings),
        ] {
            if let Some(re) = re {
                sections::edit(list, re, ls, old_chars, &text, self.multibyte);
            }
        }
        Ok(())
    }

    /// The MARK sections' expression (Rust regex syntax; `None`: none are looked for).  The sections are found again
    /// when the lines are next built, which the caller asks for (the text is the host's).  An expression that does not
    /// compile changes nothing.
    pub fn set_section_regex(&mut self, re: Option<&str>) -> Result<(), String> {
        self.section_re = match re {
            Some(r) => Some(Regex::new(r).map_err(|e| e.to_string())?),
            None => None,
        };
        self.sections.clear();
        self.generation += 1;
        Ok(())
    }

    /// Which fold ranges the model finds itself: by indentation (INDENT), and by headings (HEADING-RE, Rust regex
    /// syntax: its first group is the heading's marks, their length the level; `None`: none).  The headings are found
    /// again when the lines are next built, which the caller asks for.  An expression that does not compile changes
    /// nothing.
    pub fn set_derived_folds(
        &mut self,
        indent: bool,
        heading_re: Option<&str>,
    ) -> Result<(), String> {
        let re = match heading_re {
            Some(r) => Some(Regex::new(r).map_err(|e| e.to_string())?),
            None => None,
        };
        self.indent_folds = indent;
        self.heading_re = re;
        self.headings.clear();
        self.derived_at = u64::MAX;
        self.generation += 1;
        Ok(())
    }

    /// The indentation and heading ranges again, when the lines changed since they were found.
    fn derive(&mut self) {
        self.poll(false);
        if !self.raw_folds.is_empty() {
            self.widen_raw_folds();
        }
        if self.derived_at == self.text_gen {
            return;
        }
        let Some(idx) = self.idx.as_ref() else {
            return;
        };
        let ind = if self.indent_folds {
            derived::indent(idx)
        } else {
            Vec::new()
        };
        let heads = if self.heading_re.is_some() {
            derived::headings(idx, &self.headings)
        } else {
            Vec::new()
        };
        self.derived_at = self.text_gen;
        let max = self.fold_max;
        self.set_folds(derived::SOURCE_INDENT, &ind, max);
        self.set_folds(derived::SOURCE_HEADINGS, &heads, max);
    }

    /// The MARK sections whose line starts at BEG or after and before END, in order (none while the lines are built).
    pub fn sections(&mut self, beg: u64, end: u64) -> &[Section] {
        self.poll(false);
        let a = self.sections.partition_point(|s| s.pos < beg);
        let b = self.sections.partition_point(|s| s.pos < end).max(a);
        &self.sections[a..b]
    }

    /// The 0-based line of position POS.
    pub fn line_of(&mut self, pos: u64) -> Option<u64> {
        Some(self.index()?.line_at(pos.saturating_sub(1)))
    }

    /// The position line LINE (0-based) starts at.
    pub fn pos_of(&mut self, line: u64) -> Option<u64> {
        Some(self.index()?.start_of(line) + 1)
    }

    // ---- marks

    /// Replace the marks of KIND with MARKS.
    pub fn set_marks(&mut self, kind: u8, marks: &[Mark]) {
        self.marks.retain(|m| m.kind != kind);
        self.marks.extend(marks.iter().map(|m| Mark { kind, ..*m }));
        self.marks.sort_by_key(|m| (m.pos, m.kind));
    }

    pub fn marks(&self) -> &[Mark] {
        &self.marks
    }

    /// The next mark after POS (DIR > 0) or the previous one before it (DIR < 0) whose kind is in the bit set KINDS.
    pub fn next_mark(&self, pos: u64, kinds: u64, dir: i32) -> Option<u64> {
        let ok = |m: &&Mark| kinds & (1u64 << (m.kind & 63)) != 0;
        if dir >= 0 {
            let i = self.marks.partition_point(|m| m.pos <= pos);
            self.marks[i..].iter().find(ok).map(|m| m.pos)
        } else {
            let i = self.marks.partition_point(|m| m.pos < pos);
            self.marks[..i].iter().rev().find(ok).map(|m| m.pos)
        }
    }

    /// The marks of each kind (0–63) at positions in [BEG, END).
    pub fn counts(&self, beg: u64, end: u64) -> [u32; 64] {
        let mut out = [0u32; 64];
        let i = self.marks.partition_point(|m| m.pos < beg);
        for m in self.marks[i..].iter().take_while(|m| m.pos < end) {
            out[usize::from(m.kind & 63)] += 1;
        }
        out
    }

    // ---- folds

    /// Replace SOURCE's folds with FOLDS, keeping at most MAX folds in all (the smallest dropped first); depths are
    /// set by nesting.
    pub fn set_folds(&mut self, source: u8, folds: &[Fold], max: usize) {
        self.fold_max = max;
        self.folds.retain(|f| f.source != source);
        self.folds.extend(
            folds
                .iter()
                .filter(|f| f.beg < f.end)
                .map(|f| Fold { source, ..*f }),
        );
        if self.folds.len() > max {
            self.folds.sort_by_key(|f| std::cmp::Reverse(f.end - f.beg));
            self.folds.truncate(max);
        }
        self.folds
            .sort_by_key(|f| (f.beg, std::cmp::Reverse(f.end)));
        let mut stack: Vec<u64> = Vec::new();
        for f in &mut self.folds {
            while stack.last().is_some_and(|&e| e <= f.beg) {
                stack.pop();
            }
            f.depth = stack.len().min(255) as u8;
            stack.push(f.end);
        }
    }

    /// Replace SOURCE's folds that touch BEG..END (a parse's changed range) with FOLDS, the nodes found there (design
    /// §7.4): each node widened to its whole lines (its first line's start to its last line's end, the newline out; a
    /// node ending at a line's start ends on the line before), a node of one line dropped, and of the nodes starting
    /// on one line only the largest kept (on a tie, the higher kind).  SOURCE's folds elsewhere stay.  Nodes sent while the lines are built move
    /// with the edits as they are, and are widened once the lines are in.
    pub fn replace_folds(&mut self, source: u8, beg: u64, end: u64, folds: &[Fold]) {
        self.poll(false);
        let mut all: Vec<Fold> = self
            .folds
            .iter()
            .filter(|f| f.source == source && !(f.beg <= end && f.end >= beg))
            .copied()
            .collect();
        match self.idx.as_ref() {
            Some(idx) if !self.raw_folds.contains(&source) => {
                all.extend(folds.iter().filter_map(|f| whole_lines(idx, f, source)))
            }
            _ => {
                all.extend(folds.iter().map(|f| Fold { source, ..*f }));
                if !self.raw_folds.contains(&source) {
                    self.raw_folds.push(source);
                }
            }
        }
        self.set_one_a_line(source, all);
    }

    /// Replace the class runs of the lines touching BEG..END (positions) with SPANS, cut to those lines (design §7.2).  A span over several lines gives a run on each.  Shorter spans
    /// are painted over longer ones, and of two alike the later over the earlier (a highlights query's order); class
    /// 0 (text) paints nothing.  A line keeps its first [`RUNS`] runs, within its first [`CLASS_COLUMNS`]
    /// characters.  False, and nothing changed, while the lines are built ([`Model::take_classes_missed`] then
    /// answers true once they are).
    pub fn replace_classes(&mut self, beg: u64, end: u64, spans: &[Span]) -> bool {
        self.poll(false);
        let Some(idx) = self.idx.as_mut() else {
            self.classes_missed = true;
            return false;
        };
        let (b, e) = (beg.max(1) - 1, end.max(beg).max(1) - 1);
        let (first, last) = (idx.line_at(b), idx.line_at(e));
        let lo = idx.start_of(first);
        let hi = idx.start_of(last + 1);
        let mut order: Vec<&Span> = spans
            .iter()
            .filter(|s| {
                s.class != 0 && s.end > s.beg && s.end - 1 > lo && s.beg.saturating_sub(1) < hi
            })
            .collect();
        order.sort_by_key(|s| std::cmp::Reverse(s.end - s.beg));
        let n = (last - first + 1) as usize;
        let mut paint: Vec<Vec<u8>> = vec![Vec::new(); n];
        for &&Span {
            beg: sb,
            end: se,
            class,
        } in &order
        {
            let (sb, se) = (sb.saturating_sub(1).max(lo), (se - 1).min(hi));
            let mut l = idx.line_at(sb);
            while l <= last && idx.start_of(l) < se {
                let start = idx.start_of(l);
                let width = (u64::from(idx.line(l).len).saturating_sub(1)).min(CLASS_COLUMNS);
                let a = sb.saturating_sub(start).min(width) as usize;
                let z = (se - start).min(width) as usize;
                let row = &mut paint[(l - first) as usize];
                if row.len() < z {
                    row.resize(z, 0);
                }
                row[a..z].fill(class);
                l += 1;
            }
        }
        for (i, row) in paint.iter().enumerate() {
            let mut runs = [Run::default(); RUNS];
            let mut k = 0;
            let mut c = 0;
            while c < row.len() && k < RUNS {
                let class = row[c];
                let mut z = c + 1;
                while z < row.len() && row[z] == class && z - c < usize::from(u8::MAX) {
                    z += 1;
                }
                if class != 0 {
                    runs[k] = Run {
                        col: c as u16,
                        len: (z - c) as u8,
                        class,
                    };
                    k += 1;
                }
                c = z;
            }
            idx.set_runs(first + i as u64, runs);
        }
        self.generation += 1;
        true
    }

    /// Whether class runs were refused while the lines were built and the lines are there now; the answer clears it
    /// (the producer then runs over the whole text).
    pub fn take_classes_missed(&mut self) -> bool {
        self.poll(false);
        let missed = self.classes_missed && self.idx.is_some();
        if missed {
            self.classes_missed = false;
        }
        missed
    }

    /// Whether SOURCE has any fold.
    pub fn has_folds_of(&self, source: u8) -> bool {
        self.folds.iter().any(|f| f.source == source)
    }

    /// Replace SOURCE's folds with LINES, each (first line, last line, kind) with lines from 1 (the hub's ranges of a
    /// buffer with no parser of its own, design §7.4): each from its first line's start to its last line's end, the
    /// newline left out; a range past the text's lines is cut to them, one of a single line dropped, and of those
    /// starting on one line the largest kept.  False, and nothing changed, while the lines are built.
    pub fn set_fold_lines(&mut self, source: u8, lines: &[(u32, u32, u8)]) -> bool {
        self.poll(false);
        let Some(idx) = self.idx.as_ref() else {
            return false;
        };
        let n = idx.lines();
        let all: Vec<Fold> = lines
            .iter()
            .filter_map(|&(a, b, kind)| {
                let (a, b) = (
                    u64::from(a).checked_sub(1)?,
                    u64::from(b).checked_sub(1)?.min(n.saturating_sub(1)),
                );
                (b > a).then(|| {
                    let nl = u64::from(b + 1 < n);
                    Fold {
                        beg: idx.start_of(a) + 1,
                        end: idx.start_of(b) + 1 + u64::from(idx.line(b).len) - nl,
                        kind,
                        depth: 0,
                        source,
                    }
                })
            })
            .collect();
        self.set_one_a_line(source, all);
        true
    }

    /// SOURCE's folds made ALL, of the folds starting on one line only the largest (on a tie, the more specific kind:
    /// the higher number, so `describe(…)`'s call is a test, not its arguments' block).
    fn set_one_a_line(&mut self, source: u8, mut all: Vec<Fold>) {
        all.sort_by_key(|f| (f.beg, std::cmp::Reverse(f.end), std::cmp::Reverse(f.kind)));
        all.dedup_by_key(|f| f.beg);
        let max = self.fold_max;
        self.set_folds(source, &all, max);
    }

    /// The nodes kept while the lines were built, widened now that they are.
    fn widen_raw_folds(&mut self) {
        let Some(idx) = self.idx.as_ref() else {
            return;
        };
        let mut done = Vec::new();
        for &s in &self.raw_folds {
            let all: Vec<Fold> = self
                .folds
                .iter()
                .filter(|f| f.source == s)
                .filter_map(|f| whole_lines(idx, f, s))
                .collect();
            done.push((s, all));
        }
        self.raw_folds.clear();
        for (s, all) in done {
            self.set_one_a_line(s, all);
        }
    }

    /// The folds overlapping [BEG, END), in order (the model's own ranges found again first, when the lines changed).
    pub fn folds(&mut self, beg: u64, end: u64) -> impl Iterator<Item = &Fold> {
        self.derive();
        self.folds
            .iter()
            .filter(move |f| f.end > beg && f.beg < end)
    }

    /// The display width in columns of POS's line (`Line::cols`), for the horizontal strip's rows that are cut while
    /// their line fits; `None` before the first build.
    pub fn cols_at(&mut self, pos: u64) -> Option<u32> {
        let l = self.line_of(pos)?;
        Some(self.index()?.line(l).cols)
    }

    /// Whether POS's visible line is folded: a fold starts on it and the lines after it, up to the next visible line,
    /// are hidden (what a fold arrow shows pointing right, and the terminal's `▸`).
    pub fn folded_at(&mut self, pos: u64) -> bool {
        let Some(l) = self.line_of(pos) else {
            return false;
        };
        let (Some(a), b) = (self.pos_of(l), self.pos_of(l + 1).unwrap_or(u64::MAX)) else {
            return false;
        };
        self.folds(a, b).any(|f| f.beg >= a && f.beg < b) && !self.hidden_in(a, b).is_empty()
    }

    /// The fold to toggle at POS: the largest one that starts on POS's line, else the innermost one holding POS.
    pub fn fold_at(&mut self, pos: u64) -> Option<Fold> {
        self.derive();
        let line = self.line_of(pos);
        if let Some(l) = line {
            let mut best: Option<Fold> = None;
            for i in 0..self.folds.len() {
                let f = self.folds[i];
                if self.line_of(f.beg) == Some(l)
                    && best.is_none_or(|b| f.end - f.beg > b.end - b.beg)
                {
                    best = Some(f);
                }
            }
            if best.is_some() {
                return best;
            }
        }
        self.folds
            .iter()
            .filter(|f| f.beg <= pos && pos < f.end)
            .max_by_key(|f| f.depth)
            .copied()
    }

    // ---- hidden lines and fractions

    /// The ranges an owner made invisible: a line whose start lies in (BEG, END] is hidden.
    pub fn set_hidden(&mut self, ranges: &[(u64, u64)]) {
        let named: Vec<(u64, u64, Option<&str>)> =
            ranges.iter().map(|&(b, e)| (b, e, None)).collect();
        self.set_hidden_named(&named);
    }

    /// [`Model::set_hidden`] with a name for each range (a named fold's, or `None`): the minimap's fold pill shows it.
    pub fn set_hidden_named(&mut self, ranges: &[(u64, u64, Option<&str>)]) {
        let mut r: Vec<(u64, u64, Option<Box<str>>)> = ranges
            .iter()
            .filter(|r| r.0 < r.1)
            .map(|&(b, e, n)| (b, e, n.map(Into::into)))
            .collect();
        r.sort_unstable_by_key(|x| (x.0, x.1));
        self.hidden = r.iter().map(|x| (x.0, x.1)).collect();
        self.hidden_names = r.into_iter().map(|x| x.2).collect();
        self.hidden_lines = None;
        self.generation += 1;
        if let Some(idx) = self.idx.as_mut() {
            idx.clear_visible();
        }
    }

    /// The lines the widest line's queries have walked since the lines were built (the tests' locality measure).
    pub fn widest_scanned(&mut self) -> u64 {
        self.index().map_or(0, LineIndex::scanned)
    }

    /// The tab width and the character widths' generation the lines were built with: a host whose own differ builds
    /// the lines again before it trusts [`Model::widest`].
    pub fn built_with(&self) -> (u16, u64) {
        (self.tab_width, self.widths.generation)
    }

    /// The columns of the widest visible line of BUFFER's accessible part BEGV..ZV (the folded lines left out), `None`
    /// while the lines are built.  Kept per buffer until the lines or the hidden ones change, so a publish with no edit
    /// asks nothing of the index; after an edit only the chunks it rewrote walk their lines.
    pub fn widest(&mut self, buffer: u64, begv: u64, zv: u64) -> Option<u32> {
        let generation = self.generation;
        if let Some(i) = self.widest.iter().position(|e| e.0 == buffer) {
            let e = self.widest[i];
            if (e.1, e.2, e.3) == (generation, begv, zv) {
                return Some(e.4);
            }
            self.widest.remove(i);
        }
        let first = self.line_of(begv)?;
        let last = self.line_of(zv)?;
        self.hidden_lines()?;
        let (h, idx) = (self.hidden_lines.as_deref()?, self.idx.as_mut()?);
        let cols = idx.widest(first, last, h);
        self.widest.insert(0, (buffer, generation, begv, zv, cols));
        self.widest.truncate(WIDEST_KEPT);
        Some(cols)
    }

    /// The hidden ranges whose start lies in [BEG, END): each one's line (0-based, the line its start is on: the fold's
    /// first line, which stays visible unless another range hides it), how many lines it hides, and its name.
    pub fn hidden_in(&mut self, beg: u64, end: u64) -> Vec<(u64, u64, Option<Box<str>>)> {
        self.poll(false);
        let Some(idx) = self.idx.as_ref() else {
            return Vec::new();
        };
        let a = self.hidden.partition_point(|r| r.0 < beg);
        self.hidden[a..]
            .iter()
            .zip(&self.hidden_names[a..])
            .take_while(|(r, _)| r.0 < end)
            .filter_map(|(&(b, e), n)| {
                let head = idx.line_at(b - 1);
                let z = idx.line_at(e - 1) + 1;
                (z > head + 1).then(|| (head, z - head - 1, n.clone()))
            })
            .collect()
    }

    /// A count bumped by every change of the lines or of the hidden ones: a drawing of the rows made at one
    /// generation holds while it does not change.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// The visible lines (the text's lines but the hidden ones), when the lines are built.
    pub fn visible_lines(&mut self) -> Option<u64> {
        let hidden: u64 = self.hidden_lines()?.iter().map(|r| r.1 - r.0).sum();
        Some(self.idx.as_ref()?.lines() - hidden)
    }

    /// The line (0-based) of visible row ROW: past each hidden run that starts at or before it; `None` past the text's
    /// end or while the lines are not built.
    pub fn visible_line(&mut self, row: u64) -> Option<u64> {
        self.hidden_lines()?;
        let (h, idx) = (self.hidden_lines.as_deref()?, self.idx.as_ref()?);
        let mut line = row;
        for r in h {
            if r.0 > line {
                break;
            }
            line += r.1 - r.0;
        }
        (line < idx.lines()).then_some(line)
    }

    /// The lines of at most N visible rows from visible row FIRST (0-based), the hidden lines left out; fewer at the
    /// text's end.
    pub fn visible_rows(&mut self, first: u64, n: usize) -> Option<Vec<Line>> {
        self.hidden_lines()?;
        let (h, idx) = (self.hidden_lines.as_deref()?, self.idx.as_ref()?);
        // row FIRST's line: past each hidden run that starts at or before it
        let (mut line, mut i) = (first, 0);
        while i < h.len() && h[i].0 <= line {
            line += h[i].1 - h[i].0;
            i += 1;
        }
        let mut out = Vec::with_capacity(n.min(4096));
        while out.len() < n && line < idx.lines() {
            if i < h.len() && h[i].0 == line {
                line = h[i].1;
                i += 1;
                continue;
            }
            out.push(idx.line(line));
            line += 1;
        }
        Some(out)
    }

    fn hidden_lines(&mut self) -> Option<&[(u64, u64, u64)]> {
        if self.hidden_lines.is_none() {
            self.poll(false);
            let idx = self.idx.as_ref()?;
            let mut v: Vec<(u64, u64)> = Vec::new();
            for &(b, e) in &self.hidden {
                // the first line starting after B, through the last starting at or before E
                let a = idx.line_at(b - 1) + 1;
                let z = idx.line_at(e - 1) + 1;
                if a < z {
                    match v.last_mut() {
                        Some(last) if a <= last.1 => last.1 = last.1.max(z),
                        _ => v.push((a, z)),
                    }
                }
            }
            let mut before = 0;
            let out = v
                .into_iter()
                .map(|(a, z)| {
                    let x = (a, z, before);
                    before += z - a;
                    x
                })
                .collect();
            self.hidden_lines = Some(out);
        }
        self.hidden_lines.as_deref()
    }

    /// The visible lines before 0-based LINE, and whether LINE itself is hidden.
    pub fn visible_before(&mut self, line: u64) -> Option<(u64, bool)> {
        let h = self.hidden_lines()?;
        let i = h.partition_point(|r| r.0 <= line);
        if i == 0 {
            return Some((line, false));
        }
        let (a, z, before) = h[i - 1];
        if line < z {
            Some((a - before, true))
        } else {
            Some((line - before - (z - a), false))
        }
    }

    /// The fraction of the accessible part BEGV..ZV's visible lines before POS (with POS's place in its line), 0 to 1.
    pub fn fraction(&mut self, pos: u64, begv: u64, zv: u64) -> Option<f64> {
        let first = self.line_of(begv)?;
        let last = self.line_of(zv)?;
        let pos = pos.clamp(begv, zv);
        let line = self.line_of(pos)?;
        let (v0, _) = self.visible_before(first)?;
        let (vl, last_hidden) = self.visible_before(last)?;
        let (vp, hidden) = self.visible_before(line)?;
        let idx = self.index()?;
        let ll = idx.line(last);
        let last_start = idx.start_of(last) + 1;
        // the last line counts up to ZV (0 when ZV starts it: a final newline's empty line)
        let last_part = if last_hidden || ll.len == 0 {
            0.0
        } else {
            ((zv - last_start) as f64 / f64::from(ll.len)).min(1.0)
        };
        let total = (vl - v0) as f64 + last_part;
        if total <= 0.0 {
            return Some(0.0);
        }
        let within = if hidden {
            0.0
        } else {
            let l = idx.line(line);
            let s = idx.start_of(line) + 1;
            if l.len == 0 {
                0.0
            } else {
                (pos - s) as f64 / f64::from(l.len)
            }
        };
        Some((((vp - v0) as f64 + within) / total).clamp(0.0, 1.0))
    }

    // ---- the trail

    /// Note a visit to POS: the trail keeps the last `TRAIL` visits, one per line, newest first.
    pub fn trail_note(&mut self, pos: u64) {
        let line = self.line_of(pos);
        let mut same: Vec<usize> = Vec::new();
        for (i, &t) in self.trail.iter().enumerate() {
            if t == pos
                || (line.is_some()
                    && self.idx.as_ref().map(|x| x.line_at(t.saturating_sub(1))) == line)
            {
                same.push(i);
            }
        }
        for i in same.into_iter().rev() {
            self.trail.remove(i);
        }
        self.trail.push_front(pos);
        self.trail.truncate(TRAIL);
    }

    pub fn trail(&self) -> impl Iterator<Item = &u64> {
        self.trail.iter()
    }

    /// Forget the trail.
    pub fn trail_clear(&mut self) {
        self.trail.clear();
    }

    // ---- the minimap search

    /// Search for QUERY (`search::translate`'s arguments), or for nothing: false (and no search) when the query is
    /// outside the subset Rust carries.
    pub fn search_start(&mut self, query: Option<(&str, bool, bool)>) -> bool {
        self.search = query.and_then(|(q, literal, fold)| search::Search::new(q, literal, fold));
        self.search.is_some()
    }

    /// Scan on for up to BUDGET through the whole text in PARTS; `None` with no search.
    pub fn search_step(
        &mut self,
        parts: &[&[u8]],
        budget: std::time::Duration,
    ) -> Option<search::State> {
        let (mb, tw) = (self.multibyte, self.tab_width);
        let s = self.search.as_mut()?;
        Some(s.step(parts, mb, tw, budget))
    }

    pub fn search(&self) -> Option<&search::Search> {
        self.search.as_ref()
    }

    /// The current hit (drawn with a ring), or none.
    pub fn search_set_current(&mut self, pos: Option<u64>) {
        if let Some(s) = &mut self.search {
            s.set_current(pos);
        }
    }

    /// The bytes the model holds.
    pub fn heap_bytes(&self) -> usize {
        self.idx.as_ref().map_or(0, LineIndex::heap_bytes)
            + self.marks.capacity() * std::mem::size_of::<Mark>()
            + self.folds.capacity() * std::mem::size_of::<Fold>()
            + self.hidden.capacity() * 16
    }

    pub fn fold_count(&self) -> usize {
        self.folds.len()
    }

    pub fn hidden_count(&self) -> usize {
        self.hidden.len()
    }
}

/// Node F widened to its whole lines in IDX as a fold of SOURCE: its first line's start to its last line's end, the
/// newline left out (a node ending at a line's start ends on the line before); `None` for a node within one line.
fn whole_lines(idx: &LineIndex, f: &Fold, source: u8) -> Option<Fold> {
    if f.beg >= f.end {
        return None;
    }
    let last = idx.lines().saturating_sub(1);
    let a = idx.line_at(f.beg.saturating_sub(1)).min(last);
    let b = idx.line_at(f.end.saturating_sub(2)).min(last);
    if b <= a {
        return None;
    }
    let nl = u64::from(b + 1 < idx.lines());
    Some(Fold {
        beg: idx.start_of(a) + 1,
        end: idx.start_of(b) + 1 + u64::from(idx.line(b).len) - nl,
        kind: f.kind,
        depth: 0,
        source,
    })
}

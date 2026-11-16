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
pub mod index;

use std::collections::VecDeque;
use std::thread::JoinHandle;

pub use index::{BLANK, Line, LineIndex};

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
pub const TRAIL: usize = 20;

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

pub struct Model {
    multibyte: bool,
    tab_width: u16,
    idx: Option<LineIndex>,
    building: Option<(JoinHandle<LineIndex>, Vec<Pending>)>,
    stale: bool,
    marks: Vec<Mark>,
    folds: Vec<Fold>,
    hidden: Vec<(u64, u64)>,
    /// The hidden ranges as line ranges [a, b), merged, with the hidden lines before each (rebuilt on demand).
    hidden_lines: Option<Vec<(u64, u64, u64)>>,
    trail: VecDeque<u64>,
    pub edits: u64,
}

impl Model {
    /// A model of the text in PARTS; built on a thread of its own when BACKGROUND.
    pub fn build(parts: &[&[u8]], multibyte: bool, tab_width: u16, background: bool) -> Model {
        let mut m = Model {
            multibyte,
            tab_width,
            idx: None,
            building: None,
            stale: false,
            marks: Vec::new(),
            folds: Vec::new(),
            hidden: Vec::new(),
            hidden_lines: None,
            trail: VecDeque::new(),
            edits: 0,
        };
        m.rebuild(parts, background);
        m
    }

    /// Read the text again (lines only: marks, folds and the trail are positions and stay).
    pub fn rebuild(&mut self, parts: &[&[u8]], background: bool) {
        self.stale = false;
        self.hidden_lines = None;
        if background {
            let text: Vec<u8> = parts.concat();
            let (mb, tw) = (self.multibyte, self.tab_width);
            let h =
                std::thread::spawn(move || LineIndex::new(index::parse(&[&text], mb, tw, true)));
            self.idx = None;
            self.building = Some((h, Vec::new()));
        } else {
            self.building = None;
            self.idx = Some(LineIndex::new(index::parse(
                parts,
                self.multibyte,
                self.tab_width,
                true,
            )));
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
        match h.join() {
            Ok(idx) => {
                self.idx = Some(idx);
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
        for r in &mut self.hidden {
            *r = (shift(r.0), shift(r.1));
        }
        self.hidden.retain(|r| r.0 < r.1);
        for t in &mut self.trail {
            *t = shift(*t);
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
        let new = index::parse(seg, self.multibyte, self.tab_width, to_end);
        let new_chars: u64 = new.iter().map(|l| u64::from(l.len)).sum();
        if new_chars + old_end != old_chars + new_end {
            return Err(Stale);
        }
        idx.replace(l0, l1 - l0 + 1, new);
        Ok(())
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

    /// The folds overlapping [BEG, END), in order.
    pub fn folds(&self, beg: u64, end: u64) -> impl Iterator<Item = &Fold> {
        self.folds
            .iter()
            .filter(move |f| f.end > beg && f.beg < end)
    }

    /// The fold to toggle at POS: the largest one that starts on POS's line, else the innermost one holding POS.
    pub fn fold_at(&mut self, pos: u64) -> Option<Fold> {
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
        let mut r: Vec<(u64, u64)> = ranges.iter().copied().filter(|r| r.0 < r.1).collect();
        r.sort_unstable();
        self.hidden = r;
        self.hidden_lines = None;
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

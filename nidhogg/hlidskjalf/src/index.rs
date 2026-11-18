//! The line index: a buffer text's lines in chunks, with a Fenwick tree over the chunks' character and line totals.
//!
//! An edit rewrites the chunk (or the few chunks) holding the lines it touched and adds the size changes to the
//! Fenwick tree; nothing after it moves.  Chunks are split past `CHUNK_MAX` lines and merged below `CHUNK_MIN`, and
//! only a change in the number of chunks rebuilds the tree.

/// A line: its length in characters (its newline included), its indent in columns (`BLANK` when it holds only
/// spaces and tabs), the characters of that indent, up to [`RUNS`] runs of a display class (design §7.2: none
/// until a class producer sends them; an edit that rewrites the line clears them), and its display width in columns
/// (`cols`, its newline not counted: hscroll design §3.2).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Line {
    pub len: u32,
    pub indent: u16,
    pub lead: u16,
    pub runs: [Run; RUNS],
    pub cols: u32,
}

/// A snapshot of the host's character widths (Emacs's `char-width-table`): sorted, disjoint ranges `(first, last,
/// width)` of the characters whose width is not 1; GENERATION tells snapshots apart.  ASCII never looks here.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Widths {
    pub generation: u64,
    ranges: Vec<(u32, u32, u8)>,
}

thread_local! {
    /// The width lookups this thread's parses made (the tests' measure of the ASCII fast path).
    static LOOKUPS: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// The width lookups this thread's parses have made.
pub fn lookups() -> u64 {
    LOOKUPS.with(std::cell::Cell::get)
}

impl Widths {
    /// The snapshot GENERATION of RANGES (any order).
    pub fn new(generation: u64, mut ranges: Vec<(u32, u32, u8)>) -> Widths {
        ranges.retain(|r| r.0 <= r.1 && r.2 != 1);
        ranges.sort_unstable_by_key(|r| r.0);
        Widths { generation, ranges }
    }

    /// Character C's width (a non-ASCII character).
    pub fn of(&self, c: u32) -> u32 {
        LOOKUPS.with(|n| n.set(n.get() + 1));
        let i = self.ranges.partition_point(|r| r.0 <= c);
        match i.checked_sub(1).map(|i| self.ranges[i]) {
            Some((_, last, w)) if c <= last => u32::from(w),
            _ => 1,
        }
    }
}

/// The most class runs a line keeps.
pub const RUNS: usize = 6;

/// A run of a display class: from character COL of its line (the indent's characters counted), LEN characters (0:
/// no run).  The runs in use come first, in column order.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(C)]
pub struct Run {
    pub col: u16,
    pub len: u8,
    pub class: u8,
}

/// The indent of a blank line.
pub const BLANK: u16 = u16::MAX;

pub(crate) const CHUNK_MAX: usize = 4096;
pub(crate) const CHUNK_MIN: usize = 1024;
const CHUNK_TARGET: usize = 2048;

#[derive(Clone, Debug, Default)]
struct Chunk {
    lines: Vec<Line>,
    chars: u64,
    /// The widest line's columns, and the widest visible line's (hidden lines skipped; `None` until asked, and again
    /// when the hidden lines change).
    max_cols: u32,
    max_visible: Option<u32>,
}

impl Chunk {
    fn new(lines: Vec<Line>) -> Chunk {
        let chars = lines.iter().map(|l| u64::from(l.len)).sum();
        let max_cols = lines.iter().map(|l| l.cols).max().unwrap_or(0);
        Chunk {
            lines,
            chars,
            max_cols,
            max_visible: None,
        }
    }
}

/// A Fenwick tree of (characters, lines) per chunk.
#[derive(Clone, Debug, Default)]
struct Fenwick {
    chars: Vec<i64>,
    lines: Vec<i64>,
}

impl Fenwick {
    fn build(chunks: &[Chunk]) -> Fenwick {
        let n = chunks.len();
        let mut f = Fenwick {
            chars: vec![0; n + 1],
            lines: vec![0; n + 1],
        };
        for (i, c) in chunks.iter().enumerate() {
            f.add(i, c.chars as i64, c.lines.len() as i64);
        }
        f
    }

    fn add(&mut self, i: usize, chars: i64, lines: i64) {
        let mut k = i + 1;
        while k < self.chars.len() {
            self.chars[k] += chars;
            self.lines[k] += lines;
            k += k & k.wrapping_neg();
        }
    }

    /// The totals of the chunks before chunk I.
    fn before(&self, i: usize) -> (u64, u64) {
        let (mut c, mut l) = (0i64, 0i64);
        let mut k = i;
        while k > 0 {
            c += self.chars[k];
            l += self.lines[k];
            k -= k & k.wrapping_neg();
        }
        (c as u64, l as u64)
    }

    /// The chunk holding the item at 0-based OFFSET of the totals TOTALS picks (characters or lines), and the
    /// totals before it: the last chunk when OFFSET is past the end.
    fn find(&self, offset: u64, lines: bool) -> (usize, u64, u64) {
        let n = self.chars.len() - 1;
        let tree = if lines { &self.lines } else { &self.chars };
        let mut pos = 0usize;
        let mut rest = offset as i64;
        let mut step = if n == 0 {
            0
        } else {
            1usize << (usize::BITS - 1 - n.leading_zeros())
        };
        while step > 0 {
            let next = pos + step;
            if next <= n && tree[next] <= rest {
                pos = next;
                rest -= tree[next];
            }
            step >>= 1;
        }
        let ci = pos.min(n.saturating_sub(1));
        let (c, l) = self.before(ci);
        (ci, c, l)
    }
}

/// The line index of one text.
#[derive(Clone, Debug)]
pub struct LineIndex {
    chunks: Vec<Chunk>,
    fen: Fenwick,
    /// Chunks rewritten by edits since the index was built (the tests' locality measure).
    touched: u64,
    /// Lines walked for the widest visible line since the index was built (the same, for [`LineIndex::widest`]).
    scanned: u64,
}

impl LineIndex {
    /// The index of LINES (at least one: an empty text has one empty line).
    pub fn new(mut lines: Vec<Line>) -> LineIndex {
        if lines.is_empty() {
            lines.push(Line {
                len: 0,
                indent: BLANK,
                ..Default::default()
            });
        }
        let chunks = chunked(lines);
        let fen = Fenwick::build(&chunks);
        LineIndex {
            chunks,
            fen,
            touched: 0,
            scanned: 0,
        }
    }

    /// The lines [`LineIndex::widest`] has walked since the index was built.
    pub fn scanned(&self) -> u64 {
        self.scanned
    }

    /// The hidden lines changed: each chunk's widest visible line is found again when next asked.
    pub fn clear_visible(&mut self) {
        for c in &mut self.chunks {
            c.max_visible = None;
        }
    }

    /// The columns of the widest line from 0-based FIRST through LAST, the lines in HIDDEN (merged line ranges [a, z),
    /// sorted, as `Model::hidden_lines` keeps them) left out.  A chunk wholly inside gives its kept maximum, so only
    /// the chunks at a narrowing's ends, and the chunks an edit or a fold change rewrote, walk their lines.
    pub fn widest(&mut self, first: u64, last: u64, hidden: &[(u64, u64, u64)]) -> u32 {
        let last = last.min(self.lines().saturating_sub(1));
        if first > last {
            return 0;
        }
        let (c0, _, mut l) = self.fen.find(first, true);
        let mut best = 0u32;
        let mut ci = c0;
        while ci < self.chunks.len() && l <= last {
            let n = self.chunks[ci].lines.len() as u64;
            let whole = first <= l && l + n - 1 <= last;
            let h0 = hidden.partition_point(|r| r.1 <= l);
            let any_hidden = h0 < hidden.len() && hidden[h0].0 < l + n;
            if whole && !any_hidden {
                best = best.max(self.chunks[ci].max_cols);
            } else if whole && self.chunks[ci].max_visible.is_some() {
                best = best.max(self.chunks[ci].max_visible.unwrap_or(0));
            } else {
                let (a, z) = (first.max(l), last.min(l + n - 1));
                let mut m = 0u32;
                let mut h = h0;
                for line in a..=z {
                    while h < hidden.len() && hidden[h].1 <= line {
                        h += 1;
                    }
                    if h < hidden.len() && hidden[h].0 <= line {
                        continue;
                    }
                    m = m.max(self.chunks[ci].lines[(line - l) as usize].cols);
                }
                self.scanned += z - a + 1;
                if whole {
                    self.chunks[ci].max_visible = Some(m);
                }
                best = best.max(m);
            }
            l += n;
            ci += 1;
        }
        best
    }

    pub fn lines(&self) -> u64 {
        let (_, l) = self.fen.before(self.chunks.len());
        l
    }

    pub fn chars(&self) -> u64 {
        let (c, _) = self.fen.before(self.chunks.len());
        c
    }

    /// The chunks edits have rewritten since the index was built.
    pub fn touched(&self) -> u64 {
        self.touched
    }

    pub fn chunks(&self) -> usize {
        self.chunks.len()
    }

    /// The bytes the index holds (the lines' storage, not the allocator's slack).
    pub fn heap_bytes(&self) -> usize {
        self.chunks
            .iter()
            .map(|c| c.lines.capacity() * std::mem::size_of::<Line>())
            .sum::<usize>()
            + self.chunks.capacity() * std::mem::size_of::<Chunk>()
            + (self.fen.chars.capacity() + self.fen.lines.capacity()) * 8
    }

    /// The 0-based line holding the character at 0-based OFFSET (the last line when OFFSET is at or past the end).
    pub fn line_at(&self, offset: u64) -> u64 {
        let (ci, mut c, l) = self.fen.find(offset, false);
        let chunk = &self.chunks[ci];
        for (i, line) in chunk.lines.iter().enumerate() {
            c += u64::from(line.len);
            if c > offset {
                return l + i as u64;
            }
        }
        l + chunk.lines.len() as u64 - 1
    }

    /// The 0-based offset LINE starts at (the end of the text past the last line).
    pub fn start_of(&self, line: u64) -> u64 {
        if line >= self.lines() {
            return self.chars();
        }
        let (ci, c, l) = self.fen.find(line, true);
        let k = (line - l) as usize;
        c + self.chunks[ci].lines[..k]
            .iter()
            .map(|x| u64::from(x.len))
            .sum::<u64>()
    }

    /// Set line LINE's class runs (nothing past the last line).
    pub fn set_runs(&mut self, line: u64, runs: [Run; RUNS]) {
        if line >= self.lines() {
            return;
        }
        let (ci, _, l) = self.fen.find(line, true);
        self.chunks[ci].lines[(line - l) as usize].runs = runs;
    }

    pub fn line(&self, line: u64) -> Line {
        let (ci, _, l) = self.fen.find(line.min(self.lines() - 1), true);
        self.chunks[ci].lines[(line.min(self.lines() - 1) - l) as usize]
    }

    /// Replace the COUNT lines from FIRST with NEW (at least one line remains in the index).
    pub fn replace(&mut self, first: u64, count: u64, new: Vec<Line>) {
        let total = self.lines();
        debug_assert!(count >= 1 && first + count <= total);
        let (c0, _, l0) = self.fen.find(first, true);
        let (mut c1, _, _) = self.fen.find(first + count - 1, true);
        let mut lo = c0;
        let mut lines: Vec<Line> = Vec::new();
        for c in &self.chunks[c0..=c1] {
            lines.extend_from_slice(&c.lines);
        }
        let k = (first - l0) as usize;
        lines.splice(k..k + count as usize, new);
        // a small result takes a neighbour, so chunks stay above CHUNK_MIN
        if lines.len() < CHUNK_MIN && self.chunks.len() > 1 + (c1 - c0) {
            if c1 + 1 < self.chunks.len() {
                c1 += 1;
                lines.extend_from_slice(&self.chunks[c1].lines);
            } else {
                lo -= 1;
                let mut l = self.chunks[lo].lines.clone();
                l.extend_from_slice(&lines);
                lines = l;
            }
        }
        if lines.is_empty() {
            lines.push(Line {
                len: 0,
                indent: BLANK,
                ..Default::default()
            });
        }
        let new_chunks = if lines.len() <= CHUNK_MAX {
            vec![Chunk::new(lines)]
        } else {
            chunked(lines)
        };
        self.touched += new_chunks.len() as u64;
        if new_chunks.len() == c1 - lo + 1 {
            for (i, nc) in new_chunks.into_iter().enumerate() {
                let old = &self.chunks[lo + i];
                let dc = nc.chars as i64 - old.chars as i64;
                let dl = nc.lines.len() as i64 - old.lines.len() as i64;
                self.fen.add(lo + i, dc, dl);
                self.chunks[lo + i] = nc;
            }
        } else {
            self.chunks.splice(lo..=c1, new_chunks);
            self.fen = Fenwick::build(&self.chunks);
        }
    }
}

fn chunked(lines: Vec<Line>) -> Vec<Chunk> {
    if lines.len() <= CHUNK_MAX {
        return vec![Chunk::new(lines)];
    }
    let n = lines.len().div_ceil(CHUNK_TARGET);
    let per = lines.len().div_ceil(n);
    lines.chunks(per).map(|c| Chunk::new(c.to_vec())).collect()
}

/// Split TEXT (PARTS in order: the bytes of whole lines) into lines.  MULTIBYTE counts a character per UTF-8-style
/// lead byte (Emacs's internal form), else one per byte; TAB_WIDTH sets the columns a tab reaches.  Text with no
/// newline at its end ends with a line of its own; so does text TO_END (reaching the end of the buffer text), whose
/// last line is empty after a final newline.  Every character has width 1 but ASCII's own ([`parse_with`]).
pub fn parse(parts: &[&[u8]], multibyte: bool, tab_width: u16, to_end: bool) -> Vec<Line> {
    parse_with(parts, multibyte, tab_width, to_end, &Widths::default())
}

/// [`parse`], each line's display width counted in the same pass: a printable ASCII byte 1 with no lookup, a tab to
/// the next TAB_WIDTH stop, another control character 2 (`^X`), a unibyte text's byte past ASCII 4 (`\NNN`), any
/// other character its width in WIDTHS.
pub fn parse_with(
    parts: &[&[u8]],
    multibyte: bool,
    tab_width: u16,
    to_end: bool,
    widths: &Widths,
) -> Vec<Line> {
    let tw = u32::from(tab_width.max(1));
    let mut out = Vec::new();
    let (mut len, mut col, mut lead, mut last_nl) = (0u32, 0u32, true, false);
    let mut lc = 0u32;
    // the display columns so far, and the character being decoded: its bits and the continuation bytes still due
    let (mut cols, mut cp, mut due) = (0u32, 0u32, 0u32);
    let mut raw = false;
    for part in parts {
        for &b in *part {
            last_nl = false;
            if !multibyte || b & 0xC0 != 0x80 {
                len += 1;
            }
            if b < 0x80 {
                due = 0;
                cols = cols.saturating_add(match b {
                    0x20..=0x7e => 1,
                    b'\t' => tw - cols % tw,
                    b'\n' => 0,
                    _ => 2,
                });
            } else if !multibyte {
                cols = cols.saturating_add(4);
            } else if b & 0xC0 == 0x80 {
                if due > 0 {
                    cp = if raw {
                        cp + u32::from(b & 0x3F)
                    } else {
                        cp << 6 | u32::from(b & 0x3F)
                    };
                    due -= 1;
                    if due == 0 {
                        cols = cols.saturating_add(widths.of(cp));
                    }
                }
            } else {
                raw = b < 0xC2;
                (cp, due) = match b {
                    // a raw byte (Emacs's eight-bit characters, #x3FFF80 up)
                    0xC0..=0xC1 => (0x3F_FF80 + (u32::from(b & 1) << 6), 1),
                    0xC2..=0xDF => (u32::from(b & 0x1F), 1),
                    0xE0..=0xEF => (u32::from(b & 0x0F), 2),
                    0xF0..=0xF7 => (u32::from(b & 0x07), 3),
                    _ => (0, 4),
                };
            }
            if b == b'\n' {
                out.push(Line {
                    len,
                    indent: if lead {
                        BLANK
                    } else {
                        col.min(u32::from(BLANK) - 1) as u16
                    },
                    lead: lc.min(u32::from(u16::MAX)) as u16,
                    cols,
                    ..Default::default()
                });
                (len, col, lead, last_nl, lc, cols) = (0, 0, true, true, 0, 0);
            } else if lead {
                match b {
                    b' ' => col += 1,
                    b'\t' => col = (col / tw + 1) * tw,
                    b'\r' | b'\x0c' => {}
                    _ => lead = false,
                }
                if lead {
                    lc += 1;
                }
            }
        }
    }
    if !last_nl || to_end {
        out.push(Line {
            len,
            indent: if lead {
                BLANK
            } else {
                col.min(u32::from(BLANK) - 1) as u16
            },
            lead: lc.min(u32::from(u16::MAX)) as u16,
            cols,
            ..Default::default()
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(n: usize) -> Vec<Line> {
        (0..n)
            .map(|i| Line {
                len: (i % 7 + 1) as u32,
                indent: 0,
                ..Default::default()
            })
            .collect()
    }

    #[test]
    fn parse_counts_characters_and_indent() {
        let l = parse(&[b"  a\n\tb\xc3\xa9\n", b"\n  "], true, 8, true);
        assert_eq!(
            l,
            vec![
                Line {
                    len: 4,
                    indent: 2,
                    lead: 2,
                    cols: 3,
                    ..Default::default()
                },
                Line {
                    len: 4,
                    indent: 8,
                    lead: 1,
                    cols: 10,
                    ..Default::default()
                },
                Line {
                    len: 1,
                    indent: BLANK,
                    ..Default::default()
                },
                Line {
                    len: 2,
                    indent: BLANK,
                    lead: 2,
                    cols: 2,
                    ..Default::default()
                },
            ]
        );
        assert_eq!(
            parse(&[b"x\n"], false, 8, false),
            vec![Line {
                len: 2,
                indent: 0,
                cols: 1,
                ..Default::default()
            }]
        );
        let blank = Line {
            len: 0,
            indent: BLANK,
            ..Default::default()
        };
        assert_eq!(
            parse(&[b"x\n"], false, 8, true),
            vec![
                Line {
                    len: 2,
                    indent: 0,
                    cols: 1,
                    ..Default::default()
                },
                blank
            ]
        );
        assert_eq!(
            parse(&[], true, 8, false),
            vec![Line {
                len: 0,
                indent: BLANK,
                ..Default::default()
            }]
        );
    }

    #[test]
    fn line_at_and_start_of_agree_over_chunks() {
        let ls = lines(10_000);
        let idx = LineIndex::new(ls.clone());
        assert!(idx.chunks() > 1);
        let mut off = 0u64;
        for (i, l) in ls.iter().enumerate() {
            assert_eq!(idx.start_of(i as u64), off);
            assert_eq!(idx.line_at(off), i as u64);
            assert_eq!(idx.line_at(off + u64::from(l.len) - 1), i as u64);
            off += u64::from(l.len);
        }
        assert_eq!(idx.chars(), off);
        assert_eq!(idx.line_at(off), 9_999);
    }
}

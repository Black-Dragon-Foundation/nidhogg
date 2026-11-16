//! The line index: a buffer text's lines in chunks, with a Fenwick tree over the chunks' character and line totals.
//!
//! An edit rewrites the chunk (or the few chunks) holding the lines it touched and adds the size changes to the
//! Fenwick tree; nothing after it moves.  Chunks are split past `CHUNK_MAX` lines and merged below `CHUNK_MIN`, and
//! only a change in the number of chunks rebuilds the tree.

/// A line: its length in characters (its newline included) and its indent in columns (`BLANK` when it holds only
/// spaces and tabs).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Line {
    pub len: u32,
    pub indent: u16,
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
}

impl Chunk {
    fn new(lines: Vec<Line>) -> Chunk {
        let chars = lines.iter().map(|l| u64::from(l.len)).sum();
        Chunk { lines, chars }
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
}

impl LineIndex {
    /// The index of LINES (at least one: an empty text has one empty line).
    pub fn new(mut lines: Vec<Line>) -> LineIndex {
        if lines.is_empty() {
            lines.push(Line {
                len: 0,
                indent: BLANK,
            });
        }
        let chunks = chunked(lines);
        let fen = Fenwick::build(&chunks);
        LineIndex {
            chunks,
            fen,
            touched: 0,
        }
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
/// last line is empty after a final newline.
pub fn parse(parts: &[&[u8]], multibyte: bool, tab_width: u16, to_end: bool) -> Vec<Line> {
    let tw = u32::from(tab_width.max(1));
    let mut out = Vec::new();
    let (mut len, mut col, mut lead, mut last_nl) = (0u32, 0u32, true, false);
    for part in parts {
        for &b in *part {
            last_nl = false;
            if !multibyte || b & 0xC0 != 0x80 {
                len += 1;
            }
            if b == b'\n' {
                out.push(Line {
                    len,
                    indent: if lead {
                        BLANK
                    } else {
                        col.min(u32::from(BLANK) - 1) as u16
                    },
                });
                (len, col, lead, last_nl) = (0, 0, true, true);
            } else if lead {
                match b {
                    b' ' => col += 1,
                    b'\t' => col = (col / tw + 1) * tw,
                    b'\r' | b'\x0c' => {}
                    _ => lead = false,
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
            })
            .collect()
    }

    #[test]
    fn parse_counts_characters_and_indent() {
        let l = parse(&[b"  a\n\tb\xc3\xa9\n", b"\n  "], true, 8, true);
        assert_eq!(
            l,
            vec![
                Line { len: 4, indent: 2 },
                Line { len: 4, indent: 8 },
                Line {
                    len: 1,
                    indent: BLANK
                },
                Line {
                    len: 2,
                    indent: BLANK
                },
            ]
        );
        assert_eq!(
            parse(&[b"x\n"], false, 8, false),
            vec![Line { len: 2, indent: 0 }]
        );
        let blank = Line {
            len: 0,
            indent: BLANK,
        };
        assert_eq!(
            parse(&[b"x\n"], false, 8, true),
            vec![Line { len: 2, indent: 0 }, blank]
        );
        assert_eq!(
            parse(&[], true, 8, false),
            vec![Line {
                len: 0,
                indent: BLANK
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

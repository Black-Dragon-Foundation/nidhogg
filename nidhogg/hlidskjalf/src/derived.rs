//! The fold ranges the model finds itself (design §7.4, sources 3 and 4): by indentation, from a line to the last
//! following line indented deeper (blank lines between attached, as VS Code's indentation strategy); and by headings,
//! from a heading's line to the line before the next heading of its level or above (Markdown's `#`, Org's `*`: the
//! heading expression's first group, its length the level).  Each range starts at its first line's start and ends at
//! its last line's end (its newline left out), so the first line stays shown when it is folded.

use crate::index::{BLANK, LineIndex};
use crate::{Fold, Section};

/// The source numbers of the ranges found here (the owners' other sources: 0 the tree-sitter parser, 1 the hub, 3
/// notebook cells, 5 manual folds).
pub const SOURCE_HEADINGS: u8 = 2;
pub const SOURCE_INDENT: u8 = 4;

/// The kinds (design §7.4, the fold query's `@fold.KIND` names): 0 `block` (plain `@fold`, indentation), 1 `function`,
/// 2 `class`, 3 `comment`, 4 `import`, 5 `test`, 6 `region`, 7 `cell`, 8 `heading`.
pub const KIND_BLOCK: u8 = 0;
pub const KIND_HEADING: u8 = 8;

/// The range of lines A..=B as positions: A's start to B's end, its newline left out.
fn range(idx: &LineIndex, a: u64, b: u64, kind: u8) -> Fold {
    let beg = idx.start_of(a) + 1;
    let len = u64::from(idx.line(b).len);
    // every line but the text's last ends in a newline
    let nl = u64::from(b + 1 < idx.lines());
    let end = idx.start_of(b) + 1 + len - nl;
    Fold {
        beg,
        end,
        kind,
        depth: 0,
        source: SOURCE_INDENT,
    }
}

/// The indentation ranges of IDX's lines.
pub fn indent(idx: &LineIndex) -> Vec<Fold> {
    let mut out = Vec::new();
    // the open lines (line, indent), each indented deeper than the one under it
    let mut open: Vec<(u64, u16)> = Vec::new();
    let mut last = None::<u64>;
    let close = |open: &mut Vec<(u64, u16)>, out: &mut Vec<Fold>, upto: u16, last: Option<u64>| {
        while let Some(&(a, ind)) = open.last() {
            if ind < upto {
                break;
            }
            open.pop();
            if let Some(b) = last.filter(|&b| b > a) {
                out.push(range(idx, a, b, KIND_BLOCK));
            }
        }
    };
    for i in 0..idx.lines() {
        let ind = idx.line(i).indent;
        if ind == BLANK {
            continue;
        }
        close(&mut open, &mut out, ind, last);
        open.push((i, ind));
        last = Some(i);
    }
    close(&mut open, &mut out, 0, last);
    out
}

/// The heading ranges of IDX's lines from HEADINGS (their lines' starts, each name the heading's marks).
pub fn headings(idx: &LineIndex, headings: &[Section]) -> Vec<Fold> {
    let level = |s: &Section| s.name.chars().count();
    let lines = idx.lines();
    let mut out = Vec::new();
    for (i, h) in headings.iter().enumerate() {
        let a = idx.line_at(h.pos - 1);
        let next = headings[i + 1..].iter().find(|n| level(n) <= level(h));
        let b = match next {
            Some(n) => idx.line_at(n.pos - 1).saturating_sub(1),
            None => lines.saturating_sub(1),
        };
        // a final empty line (the text's last newline) is not the section's
        let b = if next.is_none() && b > a && idx.line(b).len == 0 {
            b - 1
        } else {
            b
        };
        if b > a {
            out.push(Fold {
                source: SOURCE_HEADINGS,
                ..range(idx, a, b, KIND_HEADING)
            });
        }
    }
    out
}

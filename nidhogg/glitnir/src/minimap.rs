//! The minimap's tiles (state B of the minimap design): a buffer's lines as bars, rasterised in Rust into tiles of
//! [`TILE_ROWS`] minimap rows, each cached by the host and drawn again only when one of its lines changed or the
//! colours did.  A line is one minimap row, `row_px` device px tall: a bar from its indent to its end, `col_px` device
//! px a column, in its class's colour; a blank line draws nothing.  Bars never reach past the tile's width.  Pixels
//! are premultiplied ARGB in Cairo's order, transparent where nothing is drawn, so the host composes a tile over the
//! window's text without resampling it.

use crate::geom::Rect;
use std::sync::Arc;

/// Minimap rows in one tile.
pub const TILE_ROWS: usize = 256;

/// A window layer's minimap as the scene draws it ([`crate::scene::Scene::set_minimap`]), over the frame's pixels
/// inside the window: the surface, the tiles and the bands, all clipped to the part revealed.  Root device px;
/// colours premultiplied ARGB.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Minimap {
    /// The whole minimap when open: over the text's right edge (or left, with `left`).
    pub rect: Rect,
    /// How much of `rect` shows, device px from its outer edge (the reveal grows from there; tiles never move).
    pub reveal_px: u32,
    /// The minimap sits on the text's left edge: it reveals from `rect`'s left side.
    pub left: bool,
    /// The surface under the bars (0: none).
    pub fill_argb: u32,
    /// The tiles, each at its top (root px), `rect.w` wide.
    pub tiles: Arc<[TileAt]>,
    /// Viewports and other rectangles, drawn over the tiles in order.
    pub bands: Arc<[Band]>,
    /// Rectangles drawn under the tiles, over the surface (nesting shades), in order.
    pub shades: Arc<[Band]>,
    /// The labels, over the tiles and under the bands ([`place_labels`] chose them).
    pub labels: Arc<[LabelAt]>,
}

/// One rasterised tile ([`raster`]) placed in the minimap.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TileAt {
    /// Its top, root device px.
    pub y: i32,
    /// Its height, device px (its pixels are `rect.w` × `h`).
    pub h: u32,
    pub px: Arc<[u32]>,
}

/// A rectangle over the tiles: a fill and a one device px outline (either 0: none).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Band {
    pub rect: Rect,
    pub fill_argb: u32,
    pub line_argb: u32,
}

impl Minimap {
    /// The part that shows: the `reveal_px` of `rect` at its outer edge.
    pub fn shown(&self) -> Rect {
        let w = (self.reveal_px as i32).clamp(0, self.rect.w);
        let x = if self.left {
            self.rect.x
        } else {
            self.rect.x + self.rect.w - w
        };
        Rect::new(x, self.rect.y, w, self.rect.h)
    }
}

/// A label drawn over the minimap (design M1): a host-rasterised sprite of a name, over a backing that hides the bars
/// under it.  Root device px; pixels premultiplied ARGB, `w` × `h`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LabelAt {
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
    pub px: Arc<[u32]>,
    /// Drawn under the sprite over its box (the minimap's surface: the bars under a label make way); 0: none.
    pub back_argb: u32,
}

/// A label that may be drawn: its top (device px, the row of its line), its height, its depth (0 outermost) and
/// whether it is a MARK section (placed before every definition).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Candidate {
    pub y: i32,
    pub h: u32,
    pub depth: u32,
    pub mark: bool,
}

/// The candidates drawn, as indices into `cands` in their order: none overlaps another (a `gap` px apart), each lies
/// within `top`..`bottom`, and where two would overlap the shallower stays (a MARK section before any definition; at
/// the same depth the one above).
pub fn place_labels(cands: &[Candidate], top: i32, bottom: i32, gap: i32) -> Vec<usize> {
    let mut order: Vec<usize> = (0..cands.len())
        .filter(|&i| cands[i].y >= top && cands[i].y + cands[i].h as i32 <= bottom)
        .collect();
    order.sort_by_key(|&i| (!cands[i].mark, cands[i].depth, cands[i].y, i));
    let mut taken: Vec<(i32, i32)> = Vec::new();
    let mut out = Vec::new();
    for i in order {
        let (a, b) = (cands[i].y, cands[i].y + cands[i].h as i32);
        let at = taken.partition_point(|&(_, e)| e + gap <= a);
        if taken.get(at).is_some_and(|&(s, _)| s < b + gap) {
            continue;
        }
        taken.insert(at, (a, b));
        out.push(i);
    }
    out.sort_unstable();
    out
}

/// The display classes a line's bar is drawn in (the line model's classes; 0 is plain text).
pub const CLASSES: usize = 16;

/// How the tiles of one minimap are drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TileSpec {
    /// The tile's width, device px.
    pub width: u32,
    /// One minimap row's height, device px (at least 1).
    pub row_px: u32,
    /// One column's width, device px (at least 1).
    pub col_px: u32,
    /// Each class's colour, premultiplied ARGB.
    pub argb: [u32; CLASSES],
}

/// One line as the minimap draws it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Row {
    /// Columns before the first non-blank character.
    pub indent: u32,
    /// Columns to the line's end (its newline left out); 0 or at most `indent` for a blank line.
    pub end: u32,
    /// The bar's class (below [`CLASSES`]; another is drawn as plain text).
    pub class: u8,
}

impl TileSpec {
    /// A tile's height in device px for ROWS rows.
    pub fn height(&self, rows: usize) -> u32 {
        rows as u32 * self.row_px.max(1)
    }
}

/// ROWS (at most [`TILE_ROWS`]) as one tile: `spec.width` × `spec.height(rows.len())` pixels, row-major.
pub fn raster(spec: &TileSpec, rows: &[Row]) -> Vec<u32> {
    let rows = &rows[..rows.len().min(TILE_ROWS)];
    let (w, rp, cp) = (
        spec.width as usize,
        spec.row_px.max(1) as usize,
        spec.col_px.max(1) as usize,
    );
    let mut px = vec![0u32; w * rows.len() * rp];
    for (i, r) in rows.iter().enumerate() {
        let x0 = (r.indent as usize).saturating_mul(cp).min(w);
        let x1 = (r.end as usize).saturating_mul(cp).min(w);
        if x1 <= x0 {
            continue;
        }
        let c = spec.argb[if (r.class as usize) < CLASSES {
            r.class as usize
        } else {
            0
        }];
        for y in i * rp..(i + 1) * rp {
            px[y * w + x0..y * w + x1].fill(c);
        }
    }
    px
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec() -> TileSpec {
        let mut argb = [0xff_80_80_80; CLASSES];
        argb[3] = 0xff_00_00_ff;
        TileSpec {
            width: 20,
            row_px: 2,
            col_px: 1,
            argb,
        }
    }

    fn at(px: &[u32], x: usize, y: usize) -> u32 {
        px[y * 20 + x]
    }

    #[test]
    fn a_line_is_a_bar_from_its_indent_to_its_end() {
        let rows = [
            Row {
                indent: 0,
                end: 5,
                class: 0,
            },
            Row {
                indent: 4,
                end: 9,
                class: 3,
            },
            Row::default(),
            Row {
                indent: 2,
                end: 99,
                class: 0,
            },
        ];
        let px = raster(&spec(), &rows);
        assert_eq!(px.len(), 20 * 8, "4 rows of 2 px");
        // row 0: columns 0..5, both pixel rows
        for y in 0..2 {
            assert_eq!(at(&px, 0, y), 0xff_80_80_80);
            assert_eq!(at(&px, 4, y), 0xff_80_80_80);
            assert_eq!(at(&px, 5, y), 0);
        }
        // row 1: indented, in its class's colour
        assert_eq!(at(&px, 3, 2), 0);
        assert_eq!(at(&px, 4, 2), 0xff_00_00_ff);
        assert_eq!(at(&px, 8, 3), 0xff_00_00_ff);
        assert_eq!(at(&px, 9, 3), 0);
        // a blank line: nothing
        assert!(px[4 * 20..6 * 20].iter().all(|&p| p == 0));
        // a long line stops at the tile's edge
        assert_eq!(at(&px, 19, 6), 0xff_80_80_80);
        assert_eq!(at(&px, 1, 7), 0);
    }

    fn cand(y: i32, depth: u32, mark: bool) -> Candidate {
        Candidate {
            y,
            h: 13,
            depth,
            mark,
        }
    }

    #[test]
    fn labels_never_overlap_and_a_deeper_one_yields() {
        // a method 4 px under its class: the class stays; the next class far enough below stays too
        let c = [cand(0, 0, false), cand(4, 1, false), cand(40, 0, false)];
        assert_eq!(place_labels(&c, 0, 100, 1), [0, 2]);
        // the deeper one first in the file: it still yields to the shallower one below that it would overlap
        let c = [cand(0, 2, false), cand(10, 0, false), cand(30, 2, false)];
        assert_eq!(place_labels(&c, 0, 100, 1), [1, 2]);
        // a MARK section before a definition of any depth
        let c = [cand(0, 0, false), cand(6, 3, true)];
        assert_eq!(place_labels(&c, 0, 100, 1), [1]);
        // the gap: 13 tall at 0 and at 14 touch with a gap of 1; at 13 they do not keep it
        assert_eq!(
            place_labels(&[cand(0, 0, false), cand(14, 0, false)], 0, 100, 1),
            [0, 1]
        );
        assert_eq!(
            place_labels(&[cand(0, 0, false), cand(13, 0, false)], 0, 100, 1),
            [0]
        );
        // only inside top..bottom
        assert_eq!(
            place_labels(
                &[cand(-2, 0, false), cand(90, 0, false), cand(40, 0, false)],
                0,
                100,
                1
            ),
            [2]
        );
        // many at one depth, every 5 px: every third, from the top
        let c: Vec<_> = (0..9).map(|i| cand(i * 5, 1, false)).collect();
        assert_eq!(place_labels(&c, 0, 100, 1), [0, 3, 6]);
    }

    #[test]
    fn columns_and_rows_scale_and_a_tile_holds_at_most_its_rows() {
        let s = TileSpec {
            col_px: 2,
            row_px: 3,
            ..spec()
        };
        let px = raster(
            &s,
            &[Row {
                indent: 1,
                end: 3,
                class: 200,
            }],
        );
        assert_eq!(px.len(), 20 * 3);
        assert_eq!(px[0], 0);
        assert_eq!(px[2], 0xff_80_80_80, "an unknown class draws as text");
        assert_eq!(px[5], 0xff_80_80_80);
        assert_eq!(px[6], 0);
        assert_eq!(px[2 * 20 + 5], 0xff_80_80_80);
        let many = vec![
            Row {
                indent: 0,
                end: 1,
                class: 0
            };
            TILE_ROWS + 9
        ];
        assert_eq!(raster(&spec(), &many).len(), 20 * 2 * TILE_ROWS);
    }
}

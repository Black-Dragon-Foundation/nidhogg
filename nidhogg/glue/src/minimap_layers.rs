//! The minimap on a shell that shows window images on layers of its own (`cards_on_layers`: Naglfar, design §6
//! "Naglfar", stage 3m).  There one picture of the whole minimap would be drawn and uploaded again at every scroll, so
//! each minimap is shown as pieces instead, each its own image (a layer on macOS), in the order glitnir's compose draws
//! them: the surface, the nesting shades, the tiles, the labels (each over its backing), then the bands.
//!
//! - **A piece keeps its id and image while its pixels are the same:** a tile or a label sprite by the identity of its
//!   pixels (the glue keeps both cached until they change), a surface, shade or band by its size and colours.  A scroll
//!   so moves pieces and uploads only the tiles and labels that came into view.
//! - **Only the revealed part shows:** each piece is given with the part of it inside the minimap's revealed part
//!   ([`CardImage::shown`]), so neither a scroll that slides a tile past the minimap's edge nor the reveal renders a
//!   piece again.
//! - **Ids:** [`piece_id`]: the window's minimap id (`MINIMAP_TAG`) with a piece number in bits 40–57.

use crate::backing::MINIMAP_TAG;
use glitnir::geom::Rect;
use glitnir::minimap::Minimap;
use nhshell::api::CardImage;
use std::sync::Arc;

/// The bits of a piece id that hold the window's `sequence_number`.
const SEQ_BITS: u32 = 40;
/// How many piece numbers a window cycles through.
const PIECES_MAX: u64 = 1 << (58 - SEQ_BITS);

/// Window `seq`'s minimap piece `n` (a card id).
pub fn piece_id(seq: u64, n: u64) -> u64 {
    MINIMAP_TAG | ((n % PIECES_MAX) << SEQ_BITS) | (seq & ((1 << SEQ_BITS) - 1))
}

/// What a piece's pixels are.
#[derive(Debug, Clone)]
enum Pix {
    /// One colour (premultiplied ARGB) over the whole piece.
    Fill(u32),
    /// A fill and a one device px outline.
    Band { fill: u32, line: u32 },
    /// A tile's pixels (the piece's size).
    Image(Arc<[u32]>),
    /// A label: its backing colour, then its sprite over it.
    Label { px: Arc<[u32]>, back: u32 },
}

impl Pix {
    /// The same pixels (a tile or sprite: the same cached pixels, not merely equal ones).
    fn same(&self, o: &Pix) -> bool {
        match (self, o) {
            (Pix::Fill(a), Pix::Fill(b)) => a == b,
            (Pix::Band { fill: a, line: b }, Pix::Band { fill: c, line: d }) => a == c && b == d,
            (Pix::Image(a), Pix::Image(b)) => Arc::ptr_eq(a, b),
            (Pix::Label { px: a, back: x }, Pix::Label { px: b, back: y }) => {
                Arc::ptr_eq(a, b) && x == y
            }
            _ => false,
        }
    }
}

/// One piece: its place (root device px) and pixels.
#[derive(Debug, Clone)]
struct Piece {
    rect: Rect,
    pix: Pix,
}

/// Window's pieces as last given: id, size and pixels of each, and the next piece number.
#[derive(Debug, Default)]
pub struct Given {
    pieces: Vec<(u64, i32, i32, Pix)>,
    next: u64,
}

impl Given {
    /// How many pieces were given last.
    pub fn len(&self) -> usize {
        self.pieces.len()
    }

    pub fn is_empty(&self) -> bool {
        self.pieces.is_empty()
    }
}

/// `m`'s pieces in drawing order (empty ones left out).
fn pieces(m: &Minimap) -> Vec<Piece> {
    let mut v = Vec::new();
    if m.fill_argb != 0 {
        v.push(Piece {
            rect: m.rect,
            pix: Pix::Fill(m.fill_argb),
        });
    }
    for b in m.shades.iter().filter(|b| b.fill_argb != 0) {
        v.push(Piece {
            rect: b.rect,
            pix: Pix::Fill(b.fill_argb),
        });
    }
    let w = m.rect.w.max(0) as usize;
    for t in m.tiles.iter() {
        if t.px.len() >= w * t.h as usize {
            v.push(Piece {
                rect: Rect::new(m.rect.x, t.y, m.rect.w, t.h as i32),
                pix: Pix::Image(Arc::clone(&t.px)),
            });
        }
    }
    for l in m.labels.iter() {
        if l.px.len() >= (l.w * l.h) as usize {
            v.push(Piece {
                rect: Rect::new(l.x, l.y, l.w as i32, l.h as i32),
                pix: Pix::Label {
                    px: Arc::clone(&l.px),
                    back: l.back_argb,
                },
            });
        }
    }
    for b in m
        .bands
        .iter()
        .filter(|b| b.fill_argb != 0 || b.line_argb != 0)
    {
        v.push(Piece {
            rect: b.rect,
            pix: Pix::Band {
                fill: b.fill_argb,
                line: b.line_argb,
            },
        });
    }
    v.retain(|p| !p.rect.is_empty());
    v
}

/// Premultiplied `src` over `dst`.
fn over(dst: u32, src: u32) -> u32 {
    let a = src >> 24;
    if a == 0xff {
        return src;
    }
    let k = 255 - a;
    let ch = |s: u32| {
        let d = (dst >> s) & 0xff;
        let t = d * k + 0x80;
        ((((t >> 8) + t) >> 8) + ((src >> s) & 0xff)).min(255)
    };
    (ch(24) << 24) | (ch(16) << 16) | (ch(8) << 8) | ch(0)
}

/// Piece `p`'s pixels (`p.rect`'s size, row by row).
fn render(p: &Piece) -> Vec<u32> {
    let (w, h) = (p.rect.w.max(0) as usize, p.rect.h.max(0) as usize);
    match &p.pix {
        Pix::Fill(c) => vec![*c; w * h],
        Pix::Band { fill, line } => {
            let mut v = vec![*fill; w * h];
            if *line != 0 {
                for y in 0..h {
                    for x in 0..w {
                        if y == 0 || y + 1 == h || x == 0 || x + 1 == w {
                            v[y * w + x] = over(*fill, *line);
                        }
                    }
                }
            }
            v
        }
        Pix::Image(px) => px[..w * h].to_vec(),
        Pix::Label { px, back } => px[..w * h].iter().map(|&s| over(*back, s)).collect(),
    }
}

/// Window `seq`'s minimap `m` as cards placed in a native area at `origin` (root device px), after `given` (updated):
/// a piece the same as one given keeps that one's id and image (`argb` none), a new one gets a new id and its pixels.
pub fn cards(seq: u64, m: &Minimap, origin: (i32, i32), given: &mut Given) -> Vec<CardImage> {
    let shown = m.shown();
    let mut free: Vec<Option<(u64, i32, i32, Pix)>> = std::mem::take(&mut given.pieces)
        .into_iter()
        .map(Some)
        .collect();
    let mut out = Vec::new();
    for p in pieces(m) {
        let vis = p.rect.intersect(&shown);
        if vis.is_empty() {
            continue;
        }
        let kept = free.iter_mut().find_map(|slot| {
            let hit = slot
                .as_ref()
                .is_some_and(|(_, w, h, pix)| *w == p.rect.w && *h == p.rect.h && pix.same(&p.pix));
            if hit { slot.take() } else { None }
        });
        let (id, argb) = match kept {
            Some((id, ..)) => (id, None),
            None => {
                // a number no piece still given has
                let id = loop {
                    let id = piece_id(seq, given.next);
                    given.next = given.next.wrapping_add(1);
                    if !free.iter().flatten().any(|g| g.0 == id)
                        && !given.pieces.iter().any(|g| g.0 == id)
                    {
                        break id;
                    }
                };
                (id, Some(Arc::from(render(&p))))
            }
        };
        given.pieces.push((id, p.rect.w, p.rect.h, p.pix.clone()));
        let at = |r: Rect| (r.x - origin.0, r.y - origin.1, r.w as u32, r.h as u32);
        out.push(CardImage {
            id,
            rect: at(p.rect),
            argb,
            shown: (vis != p.rect).then(|| at(vis)),
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use glitnir::minimap::{Band, LabelAt, TileAt};

    fn tile(y: i32, h: u32, w: i32, c: u32) -> TileAt {
        TileAt {
            y,
            h,
            px: vec![c; (w as u32 * h) as usize].into(),
        }
    }

    thread_local! {
        /// The label's sprite: the same pixels at each call (the glue's sprite cache keeps them so).
        static SPRITE: Arc<[u32]> = vec![0x8080_0000; 32].into();
    }

    fn minimap(tiles: &[TileAt], band_y: i32) -> Minimap {
        Minimap {
            rect: Rect::new(100, 10, 40, 300),
            reveal_px: 40,
            left: false,
            fill_argb: 0xff10_2030,
            tiles: tiles.to_vec().into(),
            bands: vec![Band {
                rect: Rect::new(100, band_y, 40, 30),
                fill_argb: 0x4020_2020,
                line_argb: 0xff80_8080,
            }]
            .into(),
            shades: Arc::from([]),
            labels: vec![LabelAt {
                x: 112,
                y: 50,
                w: 8,
                h: 4,
                px: SPRITE.with(Arc::clone),
                back_argb: 0xff10_2030,
            }]
            .into(),
        }
    }

    /// A scroll moves the tiles and the band: no piece is drawn again but the tile that came into view, and every
    /// piece shows only its part inside the minimap.
    #[test]
    fn a_scroll_moves_pieces_and_uploads_only_what_came_into_view() {
        let (t0, t1, t2) = (
            tile(10, 200, 40, 1),
            tile(210, 200, 40, 2),
            tile(410, 200, 40, 3),
        );
        let mut g = Given::default();
        let first = cards(7, &minimap(&[t0.clone(), t1.clone()], 20), (100, 0), &mut g);
        // surface, two tiles, the label, the band: each new
        assert_eq!(first.len(), 5);
        assert!(first.iter().all(|c| c.argb.is_some()));
        assert!(
            first
                .iter()
                .all(|c| c.id & MINIMAP_TAG != 0 && c.id & 0xff_ffff_ffff == 7)
        );
        // tile 1 reaches past the minimap's bottom (310): it shows 100 of its 200 rows
        let c1 = &first[2];
        assert_eq!(c1.rect, (0, 210, 40, 200));
        assert_eq!(c1.shown, Some((0, 210, 40, 100)));
        assert_eq!(first[0].shown, None, "the surface is the minimap");
        // the band's pixels: the outline over the fill, the fill inside
        let band = first[4].argb.as_ref().unwrap();
        assert_eq!(band[0], 0xff80_8080);
        assert_eq!(band[41], 0x4020_2020);
        // scrolled by 150 rows: tile 0 mostly above, tile 1 in, tile 2 new; the band moved
        let (s0, s1, s2) = (
            TileAt {
                y: t0.y - 150,
                ..t0.clone()
            },
            TileAt {
                y: t1.y - 150,
                ..t1.clone()
            },
            TileAt {
                y: t2.y - 150,
                ..t2
            },
        );
        let second = cards(7, &minimap(&[s0, s1, s2], 80), (100, 0), &mut g);
        let uploaded: Vec<u64> = second
            .iter()
            .filter(|c| c.argb.is_some())
            .map(|c| c.id)
            .collect();
        assert_eq!(uploaded.len(), 1, "only the new tile: {second:?}");
        assert_eq!(second[1].id, first[1].id, "tile 0 kept its id");
        assert_eq!(second[1].shown, Some((0, 10, 40, 50)));
        assert_eq!(second[2].id, first[2].id);
        assert_eq!(second[2].shown, None, "tile 1 inside now");
        assert_eq!(second[5].id, first[4].id, "the band moved, same image");
        assert_eq!(second[5].rect.1, 80);
        // a piece number is never one still given
        let ids: std::collections::BTreeSet<u64> = second.iter().map(|c| c.id).collect();
        assert_eq!(ids.len(), second.len());
    }

    /// While the minimap reveals, each piece shows its part inside the revealed edge, and no piece is drawn again;
    /// a piece wholly outside is left out.
    #[test]
    fn the_reveal_only_changes_what_shows() {
        let t = tile(10, 300, 40, 1);
        let mut g = Given::default();
        let mut m = minimap(std::slice::from_ref(&t), 20);
        m.reveal_px = 10;
        let half = cards(3, &m, (0, 0), &mut g);
        // the label (x 112..120) is outside the revealed 130..140
        assert_eq!(half.len(), 3);
        assert_eq!(half[1].shown, Some((130, 10, 10, 300)));
        m.reveal_px = 40;
        let full = cards(3, &m, (0, 0), &mut g);
        assert_eq!(full.len(), 4);
        assert_eq!(
            full.iter().filter(|c| c.argb.is_some()).count(),
            1,
            "only the label, new in view"
        );
        assert_eq!(full[1].shown, None);
    }

    /// A label's pixels: its sprite over its backing.
    #[test]
    fn a_label_is_its_sprite_over_its_backing() {
        let mut g = Given::default();
        let c = cards(1, &minimap(&[], 20), (0, 0), &mut g);
        let label = c.iter().find(|c| c.rect == (112, 50, 8, 4)).unwrap();
        // 0x80800000 over opaque 0xff102030: red half, the rest half the backing
        assert_eq!(label.argb.as_ref().unwrap()[0], 0xff88_1018);
    }
}

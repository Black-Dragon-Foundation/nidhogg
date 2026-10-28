//! Decorations: label anchors, label placement and the sprite cache (MP-28).
//!
//! An anchor states something about a window layer's retained pixels: where the first glyph of a `:label` run sits
//! (layer-local device px), the clip the label is limited to, and the key of the sprite (a name tag, rendered once
//! by the host) to put over it.  The host keeps anchors exactly as true as the pixels: every glyph draw drops the
//! anchors of the glyphs it overdraws and records the ones it contains, clears drop what they cover, and a scroll
//! moves the anchors in its band.  So the set of anchors depends only on the draw sequence, never on how a redraw
//! was cut.
//!
//! At commit the scene places the labels ([`place`]): above the anchor glyph, below it on the window's first text
//! row, shifted right past an earlier label on the same row, and one row further from the text (up for a label above,
//! down for one below) when the shift would take it out of the layer.  A label that appears, moves or goes damages
//! its old and new bounds, and compose draws the sprites over the window layer's pixels ([`crate::compose`]).
//!
//! Sprites are premultiplied ARGB at the layer's device scale, so compose never resamples them.  The cache keeps
//! the [`SPRITE_CAP`] most recently used.

use crate::geom::Rect;
use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};

/// How many sprites the cache keeps.
pub const SPRITE_CAP: usize = 64;

/// The most rows or columns of stacking tried for one label.
const MAX_TRIES: usize = 16;

/// One anchor, in window-layer coordinates (device px).  Layout equals `GlitnirAnchor`.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Anchor {
    /// The anchor glyph's rectangle.
    pub glyph: Rect,
    /// The label is clipped to this (and to the layer).
    pub clip: Rect,
    /// The sprite's key.
    pub sprite: u64,
    /// The window's first text row's top; a glyph on that row (`glyph.y <= row_top`) gets its label below.
    pub row_top: i32,
}

/// A window layer's anchors.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Anchors(Vec<Anchor>);

impl Anchors {
    pub fn as_slice(&self) -> &[Anchor] {
        &self.0
    }

    /// Record A, replacing the anchors whose glyph it overlaps (the same pixels, drawn again).
    pub fn put(&mut self, a: &Anchor) {
        self.0.retain(|o| o.glyph.intersect(&a.glyph).is_empty());
        self.0.push(*a);
    }

    /// Drop the anchors whose glyph meets EXTENT (a glyph string drew over it).
    pub fn drop_extent(&mut self, extent: Rect) {
        self.0.retain(|o| o.glyph.intersect(&extent).is_empty());
    }

    /// Drop the anchors whose glyph lies wholly inside R (a clear covered it).
    pub fn drop_rect(&mut self, r: Rect) {
        self.0.retain(|o| r.intersect(&o.glyph) != o.glyph);
    }

    pub fn clear(&mut self) {
        self.0.clear();
    }

    /// Move the anchors whose glyph lies in BAND by DY; those that would end outside BAND or LAYER (the layer's
    /// extent, layer-local) are dropped, as the pixels they describe are.
    pub fn scroll(&mut self, band: Rect, dy: i32, layer: Rect) {
        let room = band.intersect(&layer);
        self.0.retain_mut(|a| {
            if band.intersect(&a.glyph) != a.glyph {
                return true;
            }
            a.glyph = a.glyph.translate(0, dy);
            a.clip = a.clip.translate(0, dy);
            room.intersect(&a.glyph) == a.glyph
        });
    }
}

/// A placed label in root device px.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Label {
    /// The window layer it belongs to.
    pub layer: u64,
    /// Where the sprite's first pixel goes.
    pub rect: Rect,
    /// The sprite is drawn only inside this (root px).
    pub clip: Rect,
    pub sprite: u64,
    /// The sprite's generation when placed; a replaced sprite no longer matches.
    pub generation: u64,
}

impl Label {
    /// The pixels the label touches.
    pub fn visible(&self) -> Rect {
        self.rect.intersect(&self.clip)
    }
}

/// A cached sprite.  The pixels are shared by the copies a committed snapshot holds.
#[derive(Debug)]
pub struct Sprite {
    pub w: i32,
    pub h: i32,
    /// Premultiplied ARGB, `w` pixels a row.
    pub px: Arc<[u32]>,
    generation: u64,
    used: AtomicU64,
}

impl Sprite {
    pub fn generation(&self) -> u64 {
        self.generation
    }
}

impl Clone for Sprite {
    fn clone(&self) -> Sprite {
        Sprite {
            px: Arc::clone(&self.px),
            used: AtomicU64::new(self.used.load(Relaxed)),
            ..*self
        }
    }
}

/// The labels of a committed scene, in paint order within each window layer.
pub type Labels = Vec<Label>;

/// The sprite cache, least recently used out first.  A commit shares it with its snapshot (`Arc<Sprites>`), and a
/// put copies it on write, so a snapshot keeps the sprites its labels were placed from.  Uses are counted through
/// `&self`, so a lookup never copies the cache.
#[derive(Debug, Default)]
pub struct Sprites {
    map: BTreeMap<u64, Sprite>,
    clock: AtomicU64,
}

impl Clone for Sprites {
    fn clone(&self) -> Sprites {
        Sprites {
            map: self.map.clone(),
            clock: AtomicU64::new(self.clock.load(Relaxed)),
        }
    }
}

impl Sprites {
    pub const fn new() -> Sprites {
        Sprites {
            map: BTreeMap::new(),
            clock: AtomicU64::new(0),
        }
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    /// Store KEY's pixels (W×H, `w` pixels a row), replacing an earlier sprite of that key, and evict the least
    /// recently used beyond [`SPRITE_CAP`].
    pub fn put(&mut self, key: u64, px: Vec<u32>, w: i32, h: i32) {
        let generation = *self.clock.get_mut() + 1;
        *self.clock.get_mut() = generation;
        self.map.insert(
            key,
            Sprite {
                w,
                h,
                px: px.into(),
                generation,
                used: AtomicU64::new(generation),
            },
        );
        while self.map.len() > SPRITE_CAP {
            let oldest = self
                .map
                .iter()
                .filter(|(k, _)| **k != key)
                .min_by_key(|(_, s)| s.used.load(Relaxed))
                .map(|(k, _)| *k);
            match oldest {
                Some(k) => self.map.remove(&k),
                None => break,
            };
        }
    }

    /// Whether KEY is cached; counts as a use.
    pub fn has(&self, key: u64) -> bool {
        self.get(key).is_some()
    }

    /// KEY's sprite; counts as a use.
    pub fn get(&self, key: u64) -> Option<&Sprite> {
        let c = self.clock.fetch_add(1, Relaxed) + 1;
        let s = self.map.get(&key)?;
        s.used.store(c, Relaxed);
        Some(s)
    }

    /// KEY's sprite, without counting a use (compose reads a snapshot's sprites).
    pub fn lookup(&self, key: u64) -> Option<&Sprite> {
        self.map.get(&key)
    }

    /// KEY's size and generation, without counting a use.
    pub fn peek(&self, key: u64) -> Option<(i32, i32, u64)> {
        self.map.get(&key).map(|s| (s.w, s.h, s.generation))
    }
}

/// Place the labels of window layer LAYER, whose rectangle is LAYER_RECT (root px), from its ANCHORS; appended to
/// OUT.  The result depends only on the set of anchors and the sprites (anchors are taken in reading order).
/// An anchor whose sprite is not cached gets no label.
pub fn place(
    layer: u64,
    layer_rect: Rect,
    anchors: &[Anchor],
    sprites: &Sprites,
    out: &mut Vec<Label>,
) {
    let extent = Rect::new(0, 0, layer_rect.w, layer_rect.h);
    let mut order: Vec<&Anchor> = anchors.iter().collect();
    order.sort_by_key(|a| (a.glyph.y, a.glyph.x, a.sprite));
    let mut placed: Vec<Rect> = Vec::new();
    for a in order {
        let Some((sw, sh, generation)) = sprites.peek(a.sprite) else {
            continue;
        };
        let below = a.glyph.y <= a.row_top;
        let (mut y, step) = if below {
            (a.glyph.y + a.glyph.h, sh)
        } else {
            (a.glyph.y - sh, -sh)
        };
        let mut x = a.glyph.x;
        for _ in 0..MAX_TRIES {
            x = a.glyph.x;
            // past every earlier label on this row that the label would overlap
            while let Some(r) = placed.iter().find(|r| {
                r.y == y && !r.is_empty() && x < r.x + r.w && r.x < x + sw && r.x + r.w > x
            }) {
                x = r.x + r.w;
            }
            if x > a.glyph.x && x + sw > extent.w {
                y += step;
                continue;
            }
            break;
        }
        let rect = Rect::new(x, y, sw, sh);
        placed.push(rect);
        let clip = a.clip.intersect(&extent);
        out.push(Label {
            layer,
            rect: rect.translate(layer_rect.x, layer_rect.y),
            clip: clip.translate(layer_rect.x, layer_rect.y),
            sprite: a.sprite,
            generation,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compose::{SourceView, TargetView, compose_with};
    use crate::damage::Frames;
    use crate::kernels::kernels;
    use crate::rng::XorShift;
    use crate::scene::{LayerDesc, Scene};
    use crate::shadow::ShadowCache;

    const ROOT: u64 = 1;
    const WIN: u64 = 2;
    const CW: i32 = 8;
    const CH: i32 = 16;
    const COLS: i32 = 10;
    const ROWS: i32 = 8;

    fn cell(c: i32, r: i32) -> Rect {
        Rect::new(c * CW, r * CH, CW, CH)
    }

    fn anchor(glyph: Rect, sprite: u64) -> Anchor {
        Anchor {
            glyph,
            clip: Rect::new(0, 0, COLS * CW, ROWS * CH),
            sprite,
            row_top: 0,
        }
    }

    fn sorted(a: &Anchors) -> Vec<Anchor> {
        // the clip travels with its glyph; it is the host's per-draw choice, not part of what the pixels say
        let mut v: Vec<Anchor> = a
            .as_slice()
            .iter()
            .map(|a| Anchor {
                clip: Rect::default(),
                ..*a
            })
            .collect();
        v.sort_by_key(|a| (a.glyph.y, a.glyph.x));
        v
    }

    /// The anchors a fresh full redraw of CANVAS (one label id per cell, 0 none) records.
    fn redraw(canvas: &[u64]) -> Vec<Anchor> {
        let mut a = Anchors::default();
        for r in 0..ROWS {
            for c in 0..COLS {
                let id = canvas[(r * COLS + c) as usize];
                if id != 0 {
                    a.put(&anchor(cell(c, r), id));
                }
            }
        }
        sorted(&a)
    }

    #[test]
    fn anchors_depend_only_on_draw_sequence() {
        let layer = Rect::new(0, 0, COLS * CW, ROWS * CH);
        for seed in 1..=60u64 {
            let mut g = XorShift::new(seed);
            let mut canvas = vec![0u64; (COLS * ROWS) as usize];
            let mut a = Anchors::default();
            for step in 0..120 {
                match g.below(4) {
                    0 | 1 => {
                        // a glyph draw: drop what it overdraws, record what it contains
                        let (c, r) = (g.below(COLS as u32) as i32, g.below(ROWS as u32) as i32);
                        let id = g.below(4) as u64;
                        a.drop_extent(cell(c, r));
                        if id != 0 {
                            a.put(&anchor(cell(c, r), id));
                        }
                        canvas[(r * COLS + c) as usize] = id;
                    }
                    2 => {
                        let (c0, r0) = (g.below(COLS as u32) as i32, g.below(ROWS as u32) as i32);
                        let (cn, rn) = (1 + g.below(4) as i32, 1 + g.below(3) as i32);
                        let rect = Rect::new(c0 * CW, r0 * CH, cn * CW, rn * CH);
                        a.drop_rect(rect);
                        for r in r0..(r0 + rn).min(ROWS) {
                            for c in c0..(c0 + cn).min(COLS) {
                                canvas[(r * COLS + c) as usize] = 0;
                            }
                        }
                    }
                    _ => {
                        let (r0, rn) = (g.below(ROWS as u32) as i32, 1 + g.below(5) as i32);
                        let rn = rn.min(ROWS - r0);
                        let dy_rows = g.below(5) as i32 - 2;
                        let band = Rect::new(0, r0 * CH, COLS * CW, rn * CH);
                        a.scroll(band, dy_rows * CH, layer);
                        let old = canvas.clone();
                        for r in r0..r0 + rn {
                            for c in 0..COLS {
                                canvas[(r * COLS + c) as usize] = 0;
                            }
                        }
                        for r in r0..r0 + rn {
                            let to = r + dy_rows;
                            if to >= r0 && to < r0 + rn {
                                for c in 0..COLS {
                                    canvas[(to * COLS + c) as usize] = old[(r * COLS + c) as usize];
                                }
                            }
                        }
                    }
                }
                assert_eq!(sorted(&a), redraw(&canvas), "seed {seed} step {step}");
            }
        }
    }

    #[test]
    fn translation_equals_redraw() {
        let layer = Rect::new(0, 0, 100, 100);
        let band = Rect::new(0, 20, 100, 60);
        let at = [(0, 0), (10, 20), (30, 40), (50, 64), (70, 90)];
        for dy in [-30, -10, 0, 7, 25] {
            let mut moved = Anchors::default();
            let mut fresh = Anchors::default();
            for (i, (x, y)) in at.iter().enumerate() {
                let g = Rect::new(*x, *y, 8, 16);
                moved.put(&anchor(g, i as u64 + 1));
                let to = g.translate(0, dy);
                let inside = |r: Rect, b: Rect| b.intersect(&r) == r;
                // the redraw: content of the band lands moved; what leaves the band or the layer is gone
                if !inside(g, band) {
                    fresh.put(&anchor(g, i as u64 + 1));
                } else if inside(to, band) && inside(to, layer) {
                    fresh.put(&anchor(to, i as u64 + 1));
                }
            }
            moved.scroll(band, dy, layer);
            let key = |a: &Anchors| {
                let mut v: Vec<_> = a.as_slice().iter().map(|a| (a.glyph, a.sprite)).collect();
                v.sort_by_key(|(g, s)| (g.y, g.x, *s));
                v
            };
            assert_eq!(key(&moved), key(&fresh), "dy {dy}");
        }
    }

    fn sprite_px(g: &mut XorShift, n: usize) -> Vec<u32> {
        (0..n)
            .map(|_| {
                let a = g.below(256);
                let c = |g: &mut XorShift| g.below(a + 1);
                a << 24 | c(g) << 16 | c(g) << 8 | c(g)
            })
            .collect()
    }

    fn sprites_of(sizes: &[(u64, i32, i32)]) -> Arc<Sprites> {
        let mut s = Sprites::new();
        let mut g = XorShift::new(5);
        for &(k, w, h) in sizes {
            s.put(k, sprite_px(&mut g, (w * h) as usize), w, h);
        }
        Arc::new(s)
    }

    fn labels(layer_rect: Rect, anchors: &[Anchor], sprites: &Sprites) -> Vec<Label> {
        let mut out = Vec::new();
        place(WIN, layer_rect, anchors, sprites, &mut out);
        out
    }

    #[test]
    fn label_goes_below_on_first_row() {
        let s = sprites_of(&[(1, 40, 12)]);
        let layer = Rect::new(100, 50, 200, 160);
        let first = Anchor {
            glyph: Rect::new(20, 0, 8, 16),
            row_top: 0,
            ..anchor(Rect::default(), 1)
        };
        let later = Anchor {
            glyph: Rect::new(20, 64, 8, 16),
            row_top: 0,
            ..first
        };
        let l = labels(layer, &[first, later], &s);
        assert_eq!(
            l[0].rect,
            Rect::new(120, 50 + 16, 40, 12),
            "below the first row"
        );
        assert_eq!(
            l[1].rect,
            Rect::new(120, 50 + 64 - 12, 40, 12),
            "above the others"
        );
    }

    #[test]
    fn overlapping_labels_shift_right_then_stack_up() {
        let s = sprites_of(&[(1, 40, 12)]);
        let layer = Rect::new(0, 0, 100, 160);
        let at = |x| Anchor {
            glyph: Rect::new(x, 64, 8, 16),
            row_top: 0,
            ..anchor(Rect::default(), 1)
        };
        // reading order, whatever the order given
        let l = labels(layer, &[at(30), at(10), at(20)], &s);
        assert_eq!(l[0].rect, Rect::new(10, 52, 40, 12));
        assert_eq!(l[1].rect, Rect::new(50, 52, 40, 12), "right past the first");
        // 90 + 40 leaves the 100-wide layer: one row up, back at its own x
        assert_eq!(l[2].rect, Rect::new(30, 40, 40, 12));
        // a label on the first row stacks down instead
        let top = |x| Anchor {
            glyph: Rect::new(x, 0, 8, 16),
            row_top: 0,
            ..anchor(Rect::default(), 1)
        };
        let l = labels(layer, &[top(10), top(20), top(30)], &s);
        assert_eq!(l[2].rect, Rect::new(30, 28, 40, 12));
    }

    fn win_scene() -> (Scene, Frames) {
        let (mut s, f) = (Scene::default(), Frames::new());
        let d = |id, parent, kind, rect| LayerDesc {
            id,
            parent,
            kind,
            rect,
            scale: 1.0,
            visible: 1,
        };
        s.upsert(&d(ROOT, 0, 1, Rect::new(0, 0, 200, 120))).unwrap();
        s.upsert(&d(WIN, ROOT, 2, Rect::new(10, 10, 150, 90)))
            .unwrap();
        (s, f)
    }

    fn covers(damage: &[Rect], r: Rect) -> bool {
        (r.y..r.y + r.h).all(|y| (r.x..r.x + r.w).all(|x| damage.iter().any(|d| d.contains(x, y))))
    }

    #[test]
    fn label_change_damages_old_and_new_bounds() {
        let sp = sprites_of(&[(1, 30, 10), (2, 20, 8)]);
        let (mut s, mut f) = win_scene();
        s.commit_with(ROOT, &mut f, &sp);
        s.decor_put(WIN, &anchor(cell(2, 3), 1)).unwrap();
        let d = s.commit_with(ROOT, &mut f, &sp).to_vec();
        let a = labels(Rect::new(10, 10, 150, 90), &[anchor(cell(2, 3), 1)], &sp)[0].visible();
        assert!(covers(&d, a), "appears: {d:?}");
        assert!(
            s.commit_with(ROOT, &mut f, &sp).is_empty(),
            "nothing changed"
        );
        // moves
        s.decor_drop_extent(WIN, cell(2, 3)).unwrap();
        s.decor_put(WIN, &anchor(cell(5, 1), 1)).unwrap();
        let b = labels(Rect::new(10, 10, 150, 90), &[anchor(cell(5, 1), 1)], &sp)[0].visible();
        let d = s.commit_with(ROOT, &mut f, &sp).to_vec();
        assert!(covers(&d, a) && covers(&d, b), "moves: {d:?}");
        // a different sprite under the same key
        let sp2 = sprites_of(&[(1, 30, 10), (2, 20, 8), (1, 24, 10)]);
        let d = s.commit_with(ROOT, &mut f, &sp2).to_vec();
        assert!(covers(&d, b), "replaced: {d:?}");
        // goes
        s.decor_drop_all(0).unwrap();
        let d = s.commit_with(ROOT, &mut f, &sp2).to_vec();
        assert!(
            !d.is_empty() && d.iter().all(|r| r.intersect(&b.bounds(&a)) == *r),
            "{d:?}"
        );
    }

    struct Img {
        w: i32,
        px: Vec<u32>,
    }

    fn compose_into(s: &Scene, sp: &Sprites, src: &Img, out: &mut Img, clips: &[Rect]) {
        let h = src.px.len() as i32 / src.w;
        let sv = SourceView {
            layer: ROOT,
            data: &src.px,
            stride: src.w as usize,
            width: src.w,
            height: h,
            scale: 1.0,
        };
        let mut t = TargetView {
            data: &mut out.px,
            stride: out.w as usize,
            rect: Rect::new(0, 0, out.w, h),
            scale: 1.0,
        };
        compose_with(
            kernels(),
            s.committed(),
            s.labels(),
            &mut t,
            clips,
            &[sv],
            &mut ShadowCache::new(),
            sp,
        )
        .unwrap();
    }

    #[test]
    fn sprite_compose_matches_full() {
        let (w, h) = (200, 120);
        for seed in 1..=20u64 {
            let mut g = XorShift::new(seed);
            let src = Img {
                w,
                px: (0..(w * h) as usize)
                    .map(|_| 0xff00_0000 | g.below(1 << 24))
                    .collect(),
            };
            let sp = sprites_of(&[(1, 30, 10), (2, 20, 8), (3, 44, 12)]);
            let (mut s, mut f) = win_scene();
            let random = |g: &mut XorShift, s: &mut Scene| {
                for _ in 0..1 + g.below(3) {
                    let c = (g.below(COLS as u32) as i32, g.below(ROWS as u32) as i32);
                    match g.below(3) {
                        0 => s.decor_drop_extent(WIN, cell(c.0, c.1)).unwrap(),
                        _ => {
                            let a = Anchor {
                                clip: Rect::new(0, 0, 70 + g.below(80) as i32, 90),
                                ..anchor(cell(c.0, c.1), 1 + g.below(3) as u64)
                            };
                            s.decor_put(WIN, &a).unwrap()
                        }
                    }
                }
            };
            random(&mut g, &mut s);
            s.commit_with(ROOT, &mut f, &sp);
            let mut inc = Img {
                w,
                px: vec![0; (w * h) as usize],
            };
            compose_into(&s, &sp, &src, &mut inc, &[Rect::new(0, 0, w, h)]);
            for round in 0..6 {
                random(&mut g, &mut s);
                let d = s.commit_with(ROOT, &mut f, &sp).to_vec();
                compose_into(&s, &sp, &src, &mut inc, &d);
                let mut full = Img {
                    w,
                    px: vec![0; (w * h) as usize],
                };
                compose_into(&s, &sp, &src, &mut full, &[Rect::new(0, 0, w, h)]);
                assert!(inc.px == full.px, "seed {seed} round {round}");
            }
        }
    }

    #[test]
    fn sprite_cache_is_bounded() {
        let mut s = Sprites::new();
        for k in 0..100u64 {
            s.put(k, vec![0; 4], 2, 2);
            // key 0 stays in use
            assert!(k == 0 || s.has(0));
            assert!(s.len() <= SPRITE_CAP);
        }
        assert_eq!(s.len(), SPRITE_CAP);
        assert!(s.has(0), "the used sprite stayed");
        assert!(s.has(99) && !s.has(1), "the least recently used went");
        // replacing a key bumps its generation
        let g = s.peek(99).unwrap().2;
        s.put(99, vec![0; 4], 2, 2);
        assert!(s.peek(99).unwrap().2 > g);
    }
}

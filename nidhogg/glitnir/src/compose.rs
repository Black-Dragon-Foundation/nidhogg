//! Compose: a root's committed scene into an output buffer, within the damage.
//!
//! The host hands the output (a target: its pixels and where they sit in the root's device pixels), the rectangles to
//! compose (the damage) and the layers' pixels (sources: each frame's retained surface, by layer id). Within the
//! rectangles the committed layers are drawn bottom to top:
//!
//! - a frame or child-node layer without chrome is copied;
//! - a chromed layer's shadow (the nine-slice tiles of [`crate::shadow`]) goes over what lies below; then the layer is
//!   copied, except its four corner squares, which go through the corner mask OVER what lies below, with the border
//!   ring along the rounded path when the layer has a border;
//! - a window layer adds nothing of its own: its pixels are its frame's (P1 composes a scroll as damage, so a window
//!   has no offset of its own), copied with the frame; but the label sprites ([`crate::decor`]) of the window go over
//!   them, clipped to the label's clip, in the layer's place in the paint order;
//! - a window layer with a card ([`crate::scene::Card`]) then draws, OVER the frame's pixels and the labels: in the
//!   gap around the window (outside its rectangle, inside the frame, never inside a rectangle the card keeps out: the
//!   other windows), its chrome's shadow and then its edge ring, a band just outside the rounded rectangle; and in
//!   its four corner squares, outside the rounded rectangle of the chrome's radius, what lies beneath the window (the
//!   card's colour with the shadow and the ring over it) through what the disc leaves of each pixel.  The rest of the
//!   window is the plain copy.  On an own output the corners are composed into its buffer with the rest; the scene
//!   damages them when the output scrolls.  A card group's card (one card around several windows,
//!   [`crate::scene::card_group`]) also paints its dividers, the frame's pixels between the windows, in its fill.
//!
//! A layer without a source is not drawn, so the target keeps its pixels there, as it does where no layer lies. The
//! shadow beneath a layer's copied part is skipped. A pixel's value depends only on the scene and the sources, never
//! on how the rectangles cut it, so composing the damage equals composing everything wherever the damage covers what
//! changed.
//!
//! A layer with a backdrop ([`Extras`]) has the blurred picture of what lies beneath it under its pixels: the copy
//! becomes the blur's row with the layer OVER it, in the same pass, and the corner squares take that through their mask.
//!
//! [`compose_committed`] composes a commit's snapshot with a caller-owned [`Composer`] and takes no lock, so the
//! host's thread can go on drawing while another thread composes.

use crate::blur::Backdrop;
use crate::committed::Committed;
use crate::decor::{Label, Sprites};
use crate::ffi::{GLITNIR_E_ARG, GLITNIR_E_STATE};
use crate::geom::Rect;
use crate::kernels::{
    Kernels, Mask, Pixels, PixelsMut, RingCoverage, clamp_radius, corner_coverage,
    outside_coverage, ring_coverage,
};
use crate::scene::{Kind, Placed};
use crate::shadow::{ShadowCache, ShadowSpec, ShadowTiles};
use crate::stats::StatsEntry;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Instant;

/// One layer's pixels, borrowed for one call.  Layout equals `GlitnirSource`.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct Source {
    /// The layer (a frame or child node) whose surface this is; its first pixel is the layer's top-left.
    pub layer: u64,
    /// Premultiplied ARGB32, `height` rows of `stride` bytes.
    pub pixels: *const u8,
    pub stride: i32,
    pub width: i32,
    pub height: i32,
    pub scale: f64,
}

/// The output, borrowed for one call.  Layout equals `GlitnirTarget`.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct Target {
    /// Premultiplied ARGB32, `height` rows of `stride` bytes.
    pub pixels: *mut u8,
    pub stride: i32,
    pub width: i32,
    pub height: i32,
    /// The first pixel's place in the root's device pixels.
    pub origin_x: i32,
    pub origin_y: i32,
    pub scale: f64,
}

/// A layer's pixels: row `r` starts at `data[r * stride]`.
pub struct SourceView<'a> {
    pub layer: u64,
    pub data: &'a [u32],
    pub stride: usize,
    pub width: i32,
    pub height: i32,
    pub scale: f64,
}

/// The output: row `r` starts at `data[r * stride]`; `rect` is where it lies in the root's device pixels.
pub struct TargetView<'a> {
    pub data: &'a mut [u32],
    pub stride: usize,
    pub rect: Rect,
    pub scale: f64,
}

/// What a compose draws beyond the scene.
#[derive(Clone, Copy, Default)]
pub struct Extras<'a> {
    /// A blurred backdrop and the layer it lies under. The image's region is the layer's rectangle in the image's source.
    pub backdrop: Option<(&'a Backdrop, u64)>,
}

impl Extras<'_> {
    /// Nothing beyond the scene.
    pub const NONE: Extras<'static> = Extras { backdrop: None };

    /// The backdrop of layer `p`, and what takes a root px to its source's coordinates.
    fn beneath(&self, p: &Placed) -> Option<Beneath<'_>> {
        let (image, _) = self.backdrop.filter(|(_, l)| *l == p.id)?;
        let r = image.region()?;
        Some(Beneath {
            image,
            shift: (r.x - p.rect.x, r.y - p.rect.y),
        })
    }
}

/// A layer's backdrop, for drawing it.
#[derive(Clone, Copy)]
struct Beneath<'a> {
    image: &'a Backdrop,
    shift: (i32, i32),
}

impl Beneath<'_> {
    /// Root row `y` across `q`, from the image into `out`.
    fn row(&self, y: i32, q: Rect, out: &mut [u32]) {
        self.image.sample_row(
            y + self.shift.1,
            q.x + self.shift.0,
            &mut out[..q.w as usize],
        );
    }
}

/// The stride in pixels and the slice length (up to the last row's last pixel) of a strided buffer.
/// `GLITNIR_E_ARG` for a negative size, or a stride that is not whole pixels or is shorter than a row.
pub(crate) fn layout(stride: i32, width: i32, height: i32) -> Result<(usize, usize), i32> {
    if width < 0 || height < 0 || stride < 0 || stride % 4 != 0 || stride / 4 < width {
        return Err(GLITNIR_E_ARG);
    }
    let s = (stride / 4) as usize;
    let len = if width == 0 || height == 0 {
        0
    } else {
        s * (height as usize - 1) + width as usize
    };
    Ok((s, len))
}

impl Source {
    /// The pixels as a view.  `GLITNIR_E_ARG` for a bad layout, or null or misaligned pixels.
    ///
    /// # Safety
    /// `pixels` must be readable as the struct describes for `'a`, and not written meanwhile.
    pub unsafe fn view<'a>(&self) -> Result<SourceView<'a>, i32> {
        let (stride, len) = layout(self.stride, self.width, self.height)?;
        let data: &[u32] = if len == 0 {
            &[]
        } else {
            let p = self.pixels.cast::<u32>();
            if p.is_null() || !p.is_aligned() {
                return Err(GLITNIR_E_ARG);
            }
            // SAFETY: non-null and aligned; the caller promises LEN readable pixels.
            unsafe { std::slice::from_raw_parts(p, len) }
        };
        Ok(SourceView {
            layer: self.layer,
            data,
            stride,
            width: self.width,
            height: self.height,
            scale: self.scale,
        })
    }
}

impl Target {
    /// The pixels as a view.  `GLITNIR_E_ARG` for a bad layout, or null or misaligned pixels.
    ///
    /// # Safety
    /// `pixels` must be writable as the struct describes for `'a`, and overlap no source.
    pub unsafe fn view<'a>(&self) -> Result<TargetView<'a>, i32> {
        let (stride, len) = layout(self.stride, self.width, self.height)?;
        let data: &mut [u32] = if len == 0 {
            &mut []
        } else {
            let p = self.pixels.cast::<u32>();
            if p.is_null() || !p.is_aligned() {
                return Err(GLITNIR_E_ARG);
            }
            // SAFETY: non-null and aligned; the caller promises LEN writable pixels that nothing else reads.
            unsafe { std::slice::from_raw_parts_mut(p, len) }
        };
        Ok(TargetView {
            data,
            stride,
            rect: Rect::new(self.origin_x, self.origin_y, self.width, self.height),
            scale: self.scale,
        })
    }
}

impl SourceView<'_> {
    /// The pixels from root px (Q.x, Q.y), the source's first pixel being root px ORIGIN.
    fn at(&self, q: Rect, origin: (i32, i32)) -> Pixels<'_> {
        let off = (q.y - origin.1) as usize * self.stride + (q.x - origin.0) as usize;
        Pixels {
            data: &self.data[off..],
            stride: self.stride,
        }
    }
}

impl TargetView<'_> {
    /// The pixels from root px (Q.x, Q.y).
    fn at(&mut self, q: Rect) -> PixelsMut<'_> {
        let off = (q.y - self.rect.y) as usize * self.stride + (q.x - self.rect.x) as usize;
        PixelsMut {
            data: &mut self.data[off..],
            stride: self.stride,
        }
    }
}

/// A − B: up to four rectangles (the rows above and below B, then the columns beside it), empty ones dropped.
pub(crate) fn subtract(a: Rect, b: Rect) -> impl Iterator<Item = Rect> {
    let i = a.intersect(&b);
    let parts = if i.is_empty() {
        [a, Rect::default(), Rect::default(), Rect::default()]
    } else {
        let (a_right, a_bottom) = (a.x + a.w, a.y + a.h);
        let (i_right, i_bottom) = (i.x + i.w, i.y + i.h);
        [
            Rect::new(a.x, a.y, a.w, i.y - a.y),
            Rect::new(a.x, i_bottom, a.w, a_bottom - i_bottom),
            Rect::new(a.x, i.y, i.x - a.x, i.h),
            Rect::new(i_right, i.y, a_right - i_right, i.h),
        ]
    };
    parts.into_iter().filter(|r| !r.is_empty())
}

/// CLIPS within WITHIN, cut into rectangles that do not overlap (so nothing is composed twice).
fn disjoint(clips: &[Rect], within: Rect) -> Vec<Rect> {
    let mut out: Vec<Rect> = Vec::new();
    for c in clips {
        let mut parts = vec![c.intersect(&within)];
        for o in &out {
            parts = parts.iter().flat_map(|p| subtract(*p, *o)).collect();
        }
        out.extend(parts.into_iter().filter(|r| !r.is_empty()));
    }
    out
}

/// A length in whole device px (0 for a negative or non-finite one).
pub(crate) fn whole(v: f32) -> u32 {
    if v.is_finite() && v > 0.0 {
        v.round() as u32
    } else {
        0
    }
}

/// A premultiplied colour times an opacity (pixman's x·a/255, rounded).  The macOS shell colours its shadow image
/// with it too.
pub fn with_opacity(argb: u32, opacity: f32) -> u32 {
    let a = (opacity.clamp(0.0, 1.0) * 255.0).round() as u32;
    let mul = |x: u32| {
        let t = x * a + 0x80;
        ((t >> 8) + t) >> 8
    };
    (0..4).fold(0, |acc, k| acc | (mul((argb >> (8 * k)) & 0xff) << (8 * k)))
}

/// How a layer's corner squares are drawn.
enum Corners {
    /// Square corners: the whole layer is copied.
    Square,
    /// Through the disc's coverage.
    Round(Arc<[u8]>),
    /// Through the inner path's coverage, then the border colour along the ring.
    Bordered(Arc<RingCoverage>, u32),
}

struct Shadow {
    tiles: Arc<ShadowTiles>,
    /// The layer's size, which the tiles are read for.
    size: (u32, u32),
    /// The shadow image, root px.
    image: Rect,
}

/// One layer, ready to draw within any rectangle.
struct Plan {
    /// Root px of the source's first pixel.
    origin: (i32, i32),
    /// The parts copied: the middle columns, then the left and right strips between the corner squares.
    copied: [Rect; 3],
    /// The corner squares (root px) and their quadrant's place in the 2r×2r masks.
    corner_at: [(Rect, usize, usize); 4],
    /// The masks' side, 2r.
    side: usize,
    corners: Corners,
    shadow: Option<Shadow>,
}

impl Plan {
    fn new(p: &Placed, src: &SourceView, scale: f64, shadows: &mut ShadowCache) -> Plan {
        // what the source covers of the layer
        let (x, y) = (p.rect.x, p.rect.y);
        let (w, h) = (p.rect.w.min(src.width), p.rect.h.min(src.height));
        let c = p.chrome.unwrap_or_default();
        let radius = whole(c.radius_px);
        let r = if w <= 0 || h <= 0 {
            0
        } else {
            clamp_radius(w as usize, h as usize, radius)
        };
        let border = whole(c.border_px);
        let corners = match (r, border) {
            (0, _) => Corners::Square,
            (_, 0) => Corners::Round(corner_coverage(r)),
            _ => Corners::Bordered(ring_coverage(r, border), c.border_argb),
        };
        let (r, ru) = (r as i32, r as usize);
        let shadow = (c.has_shadow() && !p.rect.is_empty()).then(|| {
            let size = (p.rect.w as u32, p.rect.h as u32);
            let spec = ShadowSpec {
                radius: radius as f32,
                blur: c.blur_px,
                spread: c.spread_px,
                colour: with_opacity(c.shadow_argb, c.shadow_opacity),
                scale,
            };
            let tiles = shadows.get(size.0, size.1, &spec);
            let (iw, ih) = tiles.image_size(size.0, size.1);
            // a fractional offset rounds to the nearest pixel, inside the bounds the scene damages
            let image = Rect::new(
                x - tiles.pad() + c.offset_x.round() as i32,
                y - tiles.pad() + c.offset_y.round() as i32,
                iw as i32,
                ih as i32,
            );
            Shadow { tiles, size, image }
        });
        Plan {
            origin: (x, y),
            copied: [
                Rect::new(x + r, y, w - 2 * r, h),
                Rect::new(x, y + r, r, h - 2 * r),
                Rect::new(x + w - r, y + r, r, h - 2 * r),
            ],
            corner_at: [
                (Rect::new(x, y, r, r), 0, 0),
                (Rect::new(x + w - r, y, r, r), ru, 0),
                (Rect::new(x, y + h - r, r, r), 0, ru),
                (Rect::new(x + w - r, y + h - r, r, r), ru, ru),
            ],
            side: 2 * ru,
            corners,
            shadow,
        }
    }

    /// Draw the layer within CLIP (inside the target), over BENEATH when it has a backdrop.
    fn draw(
        &self,
        k: &Kernels,
        t: &mut TargetView,
        src: &SourceView,
        clip: Rect,
        beneath: Option<Beneath>,
    ) {
        if let Some(s) = &self.shadow {
            let mut parts = vec![s.image.intersect(&clip)];
            for cut in &self.copied {
                parts = parts.iter().flat_map(|a| subtract(*a, *cut)).collect();
            }
            for q in parts.into_iter().filter(|q| !q.is_empty()) {
                let from = ((q.x - s.image.x) as u32, (q.y - s.image.y) as u32);
                let span = (q.w as usize, q.h as usize);
                s.tiles.composite(k, &mut t.at(q), s.size, from, span);
            }
        }
        for cut in &self.copied {
            let q = cut.intersect(&clip);
            if q.is_empty() {
                continue;
            }
            let (w, h) = (q.w as usize, q.h as usize);
            let Some(b) = beneath else {
                k.copy(&mut t.at(q), &src.at(q, self.origin), w, h);
                continue;
            };
            // row by row, so that a row is in the cache when the layer goes over it
            for y in q.y..q.y + q.h {
                let row = Rect::new(q.x, y, q.w, 1);
                b.row(y, q, t.at(row).data);
                k.over(&mut t.at(row), &src.at(row, self.origin), w, 1);
            }
        }
        let (inside, ring, colour): (&[u8], Option<&[u8]>, u32) = match &self.corners {
            Corners::Square => return,
            Corners::Round(m) => (&m[..], None, 0),
            Corners::Bordered(c, colour) => (&c.inner[..], Some(&c.ring[..]), *colour),
        };
        let n = self.side;
        for &(at, mx, my) in &self.corner_at {
            let q = at.intersect(&clip);
            if q.is_empty() {
                continue;
            }
            let off = (my + (q.y - at.y) as usize) * n + mx + (q.x - at.x) as usize;
            let (w, h) = (q.w as usize, q.h as usize);
            let mut dst = t.at(q);
            let mask = Mask {
                data: &inside[off..],
                stride: n,
            };
            match beneath {
                None => k.mask_over(&mut dst, &src.at(q, self.origin), &mask, w, h, 255),
                Some(b) => {
                    let mut px = vec![0u32; w * h];
                    for (i, row) in px.chunks_exact_mut(w).enumerate() {
                        b.row(q.y + i as i32, q, row);
                    }
                    let mut fused = PixelsMut {
                        data: &mut px,
                        stride: w,
                    };
                    k.over(&mut fused, &src.at(q, self.origin), w, h);
                    let fused = Pixels {
                        data: &px,
                        stride: w,
                    };
                    k.mask_over(&mut dst, &fused, &mask, w, h, 255);
                }
            }
            if let Some(ring) = ring {
                let mask = Mask {
                    data: &ring[off..],
                    stride: n,
                };
                k.solid_over(&mut dst, colour, &mask, w, h);
            }
        }
    }
}

/// Compose SCENE (a committed scene's layers in paint order) into T within CLIPS (root px), from SOURCES; the number
/// of layers drawn.  `GLITNIR_E_STATE`, with nothing drawn, when a source's scale is not the target's.
pub fn compose(
    k: &Kernels,
    scene: &[Placed],
    t: &mut TargetView,
    clips: &[Rect],
    sources: &[SourceView],
    shadows: &mut ShadowCache,
) -> Result<u32, i32> {
    compose_with(k, scene, &[], t, clips, sources, shadows, &Sprites::new())
}

/// Draw LABEL's sprite within PIECES (disjoint, inside the target).  A sprite replaced or evicted since the label was
/// placed is skipped; the next commit damages the label.  (A snapshot's sprites are those its labels were placed
/// from.)
fn draw_label(k: &Kernels, t: &mut TargetView, pieces: &[Rect], l: &Label, sprites: &Sprites) {
    let Some(sp) = sprites.lookup(l.sprite) else {
        return;
    };
    if sp.generation() != l.generation {
        return;
    }
    let vis = l.visible();
    for c in pieces {
        let q = vis.intersect(c);
        if q.is_empty() {
            continue;
        }
        let off = (q.y - l.rect.y) as usize * sp.w as usize + (q.x - l.rect.x) as usize;
        let src = Pixels {
            data: &sp.px[off..],
            stride: sp.w as usize,
        };
        k.over(&mut t.at(q), &src, q.w as usize, q.h as usize);
    }
}

/// [`compose`] with the labels (from [`crate::scene::Scene::labels`]) and the sprite cache they draw from.
#[allow(clippy::too_many_arguments)]
pub fn compose_with(
    k: &Kernels,
    scene: &[Placed],
    labels: &[Label],
    t: &mut TargetView,
    clips: &[Rect],
    sources: &[SourceView],
    shadows: &mut ShadowCache,
    sprites: &Sprites,
) -> Result<u32, i32> {
    let x = Extras::NONE;
    compose_layers(k, scene, labels, t, clips, sources, shadows, sprites, &x)
}

/// [`compose_with`] with X drawn too.
#[allow(clippy::too_many_arguments)]
fn compose_layers(
    k: &Kernels,
    scene: &[Placed],
    labels: &[Label],
    t: &mut TargetView,
    clips: &[Rect],
    sources: &[SourceView],
    shadows: &mut ShadowCache,
    sprites: &Sprites,
    x: &Extras,
) -> Result<u32, i32> {
    if sources.iter().any(|s| s.scale != t.scale) {
        return Err(GLITNIR_E_STATE);
    }
    let pieces = disjoint(clips, t.rect);
    let mut drawn = 0u32;
    for p in scene {
        let source = sources.iter().find(|s| s.layer == p.id);
        if let Some(src) = source.filter(|_| {
            p.kind != Kind::Window && pieces.iter().any(|c| !c.intersect(&p.bounds).is_empty())
        }) {
            let plan = Plan::new(p, src, t.scale, shadows);
            let beneath = x.beneath(p);
            for c in &pieces {
                plan.draw(k, t, src, *c, beneath);
            }
            drawn = drawn.saturating_add(1);
        }
        // a layer's labels go over its pixels (a window layer's pixels are its frame's, drawn before it)
        for l in labels.iter().filter(|l| l.layer == p.id) {
            draw_label(k, t, &pieces, l, sprites);
        }
        // a window card goes over its frame's pixels and the labels, when that frame is drawn here
        if let Some(card) = &p.card
            && p.kind == Kind::Window
            && touches_card(p, &pieces)
            && sources.iter().any(|s| s.layer == p.parent)
            && let Some(frame) = scene.iter().find(|q| q.id == p.parent).map(|q| q.rect)
        {
            let plan = CardPlan::new(p, card, t.scale, shadows);
            let (below, above) = neighbours(scene, p, &pieces, t.scale, shadows);
            for c in &pieces {
                plan.draw(k, t, card, c.intersect(&frame), (&below, &above));
            }
        }
    }
    Ok(drawn)
}

/// The other cards of card P's frame whose chrome reaches one of P's corner squares within PIECES, below P and above it
/// in SCENE's paint order: what a corner shows of the gap has their shadows in it too.  None while no corner is drawn.
fn neighbours(
    scene: &[Placed],
    p: &Placed,
    pieces: &[Rect],
    scale: f64,
    shadows: &mut ShadowCache,
) -> (Vec<CardPlan>, Vec<CardPlan>) {
    let (mut below, mut above) = (Vec::new(), Vec::new());
    let Some((_, squares)) = crate::scene::card_corners(p) else {
        return (below, above);
    };
    let drawn: Vec<Rect> = squares
        .iter()
        .flat_map(|s| pieces.iter().map(|c| s.intersect(c)))
        .filter(|r| !r.is_empty())
        .collect();
    if drawn.is_empty() {
        return (below, above);
    }
    let mut past = false;
    for q in scene {
        if q.id == p.id {
            past = true;
            continue;
        }
        let Some(card) = &q.card else {
            continue;
        };
        if q.kind != Kind::Window
            || q.parent != p.parent
            || !drawn.iter().any(|d| !d.intersect(&q.bounds).is_empty())
        {
            continue;
        }
        let plan = CardPlan::new(q, card, scale, shadows);
        if past {
            above.push(plan);
        } else {
            below.push(plan);
        }
    }
    (below, above)
}

/// Whether any of PIECES meets what window card P draws: its bounds outside its rectangle, or a corner square.  Typing
/// inside a card window meets neither.
fn touches_card(p: &Placed, pieces: &[Rect]) -> bool {
    let corners = crate::scene::card_corners(p);
    pieces.iter().any(|c| {
        let q = c.intersect(&p.bounds);
        !q.is_empty()
            && (q.intersect(&p.rect) != q
                || corners.is_some_and(|(_, sq)| sq.iter().any(|s| !s.intersect(&q).is_empty()))
                || p.card
                    .as_ref()
                    .is_some_and(|k| k.fill.iter().any(|f| !f.intersect(&q).is_empty())))
    })
}

#[cfg(test)]
thread_local! {
    /// The pixels window cards drew on this thread: the tests' way to see that typing composes no card pixel.
    static CARD_PX: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// Count `q`'s pixels as drawn by a card (tests only).
fn card_drew(_q: Rect) {
    #[cfg(test)]
    CARD_PX.with(|n| n.set(n.get() + _q.area()));
}

/// The pixels of a mask quadrant: the square SQ's part Q, the quadrant at (MX, MY) of a mask N px a row.
fn quadrant<'a>(m: &'a [u8], n: usize, (mx, my): (usize, usize), sq: Rect, q: Rect) -> Mask<'a> {
    let off = (my + (q.y - sq.y) as usize) * n + mx + (q.x - sq.x) as usize;
    Mask {
        data: &m[off..],
        stride: n,
    }
}

/// A card's edge ring, ready to draw.
struct CardRing {
    colour: u32,
    /// The ring's corners: the band between the radius r and r + w, 2(r + w) px a side.
    cov: Arc<RingCoverage>,
    /// r + w.
    side: usize,
    /// The (r + w)-squares at the four corners (top-left, top-right, bottom-left, bottom-right; root px), each holding a
    /// quarter of the band: their inner r×r part lies in the window's corner square.
    squares: [Rect; 4],
    /// The straight parts just outside the window: top, bottom, left, right.
    strips: [Rect; 4],
}

impl CardRing {
    /// Draw the ring within Q into DST, whose first pixel is root px Q's top-left.
    fn draw(&self, k: &Kernels, dst: &mut PixelsMut, q: Rect) {
        for s in &self.strips {
            let p = s.intersect(&q);
            if p.is_empty() {
                continue;
            }
            // the straight parts are covered whole: one row of 255, read for every row
            let full = vec![255u8; p.w as usize];
            let mask = Mask {
                data: &full,
                stride: 0,
            };
            let at = (p.y - q.y) as usize * dst.stride + (p.x - q.x) as usize;
            let stride = dst.stride;
            let mut d = PixelsMut {
                data: &mut dst.data[at..],
                stride,
            };
            k.solid_over(&mut d, self.colour, &mask, p.w as usize, p.h as usize);
        }
        let n = 2 * self.side;
        for (i, sq) in self.squares.iter().enumerate() {
            let p = sq.intersect(&q);
            if p.is_empty() {
                continue;
            }
            let quad = ((i % 2) * self.side, (i / 2) * self.side);
            let mask = quadrant(&self.cov.ring, n, quad, *sq, p);
            let at = (p.y - q.y) as usize * dst.stride + (p.x - q.x) as usize;
            let stride = dst.stride;
            let mut d = PixelsMut {
                data: &mut dst.data[at..],
                stride,
            };
            k.solid_over(&mut d, self.colour, &mask, p.w as usize, p.h as usize);
        }
    }
}

/// A window card, ready to draw within any rectangle.
struct CardPlan {
    rect: Rect,
    /// The corner radius (clamped) and the four corner squares.
    corners: Option<(u32, [Rect; 4])>,
    /// The chrome's shadow, in the gap and in the corners outside the rounded rectangle.
    shadow: Option<Shadow>,
    ring: Option<CardRing>,
    /// A group's dividers (inside the rectangle) and their colour.
    fill: Vec<Rect>,
    fill_argb: u32,
}

impl CardPlan {
    fn new(
        p: &Placed,
        card: &crate::scene::Card,
        scale: f64,
        shadows: &mut ShadowCache,
    ) -> CardPlan {
        let rect = p.rect;
        let c = p.chrome.unwrap_or_default();
        // the radius the corners have, clamped to half the shorter side: the shadow and the ring take it too
        let corners = crate::scene::card_corners(p);
        let r = corners.map_or(0, |(r, _)| r);
        let shadow = (c.has_shadow() && !rect.is_empty()).then(|| {
            let size = (rect.w as u32, rect.h as u32);
            let spec = ShadowSpec {
                radius: r as f32,
                blur: c.blur_px,
                spread: c.spread_px,
                colour: with_opacity(c.shadow_argb, c.shadow_opacity),
                scale,
            };
            let tiles = shadows.get(size.0, size.1, &spec);
            let (iw, ih) = tiles.image_size(size.0, size.1);
            let image = Rect::new(
                rect.x - tiles.pad() + c.offset_x.round() as i32,
                rect.y - tiles.pad() + c.offset_y.round() as i32,
                iw as i32,
                ih as i32,
            );
            Shadow { tiles, size, image }
        });
        let w = card.ring().min(1 << 12);
        let ring = (w > 0 && !rect.is_empty()).then(|| {
            let (ri, wi) = (r as i32, w as i32);
            let s = ri + wi;
            let Rect { x, y, w: rw, h: rh } = rect;
            CardRing {
                colour: card.ring_argb,
                cov: ring_coverage(r + w, w),
                side: s as usize,
                squares: [
                    Rect::new(x - wi, y - wi, s, s),
                    Rect::new(x + rw - ri, y - wi, s, s),
                    Rect::new(x - wi, y + rh - ri, s, s),
                    Rect::new(x + rw - ri, y + rh - ri, s, s),
                ],
                strips: [
                    Rect::new(x + ri, y - wi, rw - 2 * ri, wi),
                    Rect::new(x + ri, y + rh, rw - 2 * ri, wi),
                    Rect::new(x - wi, y + ri, wi, rh - 2 * ri),
                    Rect::new(x + rw, y + ri, wi, rh - 2 * ri),
                ],
            }
        });
        CardPlan {
            rect,
            corners,
            shadow,
            ring,
            fill: card
                .fill
                .iter()
                .map(|f| f.intersect(&rect))
                .filter(|f| !f.is_empty())
                .collect(),
            fill_argb: card.fill_argb,
        }
    }

    /// The shadow within Q into DST, whose first pixel is root px Q's top-left.
    fn shadow(&self, k: &Kernels, dst: &mut PixelsMut, q: Rect) {
        let Some(s) = &self.shadow else {
            return;
        };
        let p = s.image.intersect(&q);
        if p.is_empty() {
            return;
        }
        let at = (p.y - q.y) as usize * dst.stride + (p.x - q.x) as usize;
        let stride = dst.stride;
        let mut d = PixelsMut {
            data: &mut dst.data[at..],
            stride,
        };
        let from = ((p.x - s.image.x) as u32, (p.y - s.image.y) as u32);
        s.tiles
            .composite(k, &mut d, s.size, from, (p.w as usize, p.h as usize));
    }

    /// The shadow and the ring within Q into DST, Q's pixels row by row, but not inside the window: in the gap, where
    /// they draw.
    fn chrome_outside(&self, k: &Kernels, dst: &mut [u32], q: Rect) {
        for part in subtract(q, self.rect) {
            let at = (part.y - q.y) as usize * q.w as usize + (part.x - q.x) as usize;
            let mut d = PixelsMut {
                data: &mut dst[at..],
                stride: q.w as usize,
            };
            self.shadow(k, &mut d, part);
            if let Some(ring) = &self.ring {
                ring.draw(k, &mut d, part);
            }
        }
    }

    /// Draw the card within CLIP (inside the target and the frame).  A corner shows the gap as it is beside it: the
    /// colour beneath, then the chrome of the frame's other cards BELOW it in paint order, its own, and theirs ABOVE
    /// it (each card keeps its chrome out of the others' windows, so none of it lies in the corner's square itself).
    fn draw(
        &self,
        k: &Kernels,
        t: &mut TargetView,
        card: &crate::scene::Card,
        clip: Rect,
        (below, above): (&[CardPlan], &[CardPlan]),
    ) {
        if clip.is_empty() {
            return;
        }
        // in the gap: the shadow, then the ring, OVER the frame, never inside another window
        if self.shadow.is_some() || self.ring.is_some() {
            let mut parts: Vec<Rect> = subtract(clip, self.rect).collect();
            for keep in card.keep_out.iter() {
                parts = parts.iter().flat_map(|a| subtract(*a, *keep)).collect();
            }
            for q in parts {
                let mut dst = t.at(q);
                self.shadow(k, &mut dst, q);
                if let Some(ring) = &self.ring {
                    ring.draw(k, &mut dst, q);
                }
                card_drew(q);
            }
        }
        // a group's dividers: its fill over the frame, before the corners (a short window's corner may reach one)
        for f in &self.fill {
            let q = f.intersect(&clip);
            if q.is_empty() {
                continue;
            }
            let full = vec![255u8; q.w as usize];
            let mask = Mask {
                data: &full,
                stride: 0,
            };
            k.solid_over(
                &mut t.at(q),
                self.fill_argb,
                &mask,
                q.w as usize,
                q.h as usize,
            );
            card_drew(q);
        }
        // the corners: what lies beneath the window (the gap with its shadow and ring), through what the disc leaves
        // of each pixel, so a pixel is gap·(1 − c) + window·c
        let Some((r, squares)) = self.corners else {
            return;
        };
        let outside = outside_coverage(r);
        let (n, ru) = (2 * r as usize, r as usize);
        let rich =
            self.shadow.is_some() || self.ring.is_some() || !below.is_empty() || !above.is_empty();
        for (i, sq) in squares.iter().enumerate() {
            let q = sq.intersect(&clip);
            if q.is_empty() {
                continue;
            }
            let mask = quadrant(&outside, n, ((i % 2) * ru, (i / 2) * ru), *sq, q);
            let (w, h) = (q.w as usize, q.h as usize);
            if rich {
                let mut px = vec![card.beneath_argb; w * h];
                for o in below {
                    o.chrome_outside(k, &mut px, q);
                }
                let mut gap = PixelsMut {
                    data: &mut px,
                    stride: w,
                };
                self.shadow(k, &mut gap, q);
                if let Some(ring) = &self.ring {
                    ring.draw(k, &mut gap, q);
                }
                for o in above {
                    o.chrome_outside(k, &mut px, q);
                }
                let gap = Pixels {
                    data: &px,
                    stride: w,
                };
                k.mask_over(&mut t.at(q), &gap, &mask, w, h, 255);
            } else {
                k.solid_over(&mut t.at(q), card.beneath_argb, &mask, w, h);
            }
            card_drew(q);
        }
    }
}

/// A root's composes since its last frame-time record.
#[derive(Debug, Default, Clone, Copy)]
struct Tally {
    ns: u64,
    layers: u32,
}

/// What composing keeps between calls: the shadow tiles, and each root's composes since its last frame-time record.
/// The thread that composes owns it; the C ABI has one for the process.
#[derive(Default)]
pub struct Composer {
    shadows: Option<ShadowCache>,
    tallies: BTreeMap<u64, Tally>,
    /// A gate at the start of each compose, so tests can hold a compose in progress until they release it (at most 5 s).
    #[cfg(test)]
    gate: Option<std::sync::mpsc::Receiver<()>>,
}

impl Composer {
    pub const fn new() -> Composer {
        Composer {
            shadows: None,
            tallies: BTreeMap::new(),
            #[cfg(test)]
            gate: None,
        }
    }

    /// Fill ROOT's entry from its composes since its last record, which are then forgotten: compose times when the
    /// entry has none (the summed time, ending at the draw's end), the layers drawn when it has none.
    pub fn fill_stats(&mut self, root: u64, e: &mut StatsEntry) {
        let Some(t) = self.tallies.remove(&root) else {
            return;
        };
        if e.t_compose_start_ns == 0 && e.t_compose_end_ns == 0 && t.ns > 0 {
            let end = if e.t_draw_end_ns > t.ns {
                e.t_draw_end_ns
            } else {
                t.ns + 1
            };
            e.t_compose_start_ns = end - t.ns;
            e.t_compose_end_ns = end;
        }
        if e.layers == 0 {
            e.layers = t.layers;
        }
    }

    /// Forget ROOT's composes (the root is gone).
    pub fn forget(&mut self, root: u64) {
        self.tallies.remove(&root);
    }
}

impl std::fmt::Debug for Composer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Composer")
            .field("shadows", &self.shadows.as_ref().map(ShadowCache::len))
            .field("tallies", &self.tallies)
            .finish()
    }
}

// A composer moves to the thread that composes.
const _: fn() = || {
    fn sendable<T: Send>() {}
    sendable::<Composer>();
};

/// Compose snapshot C into T within CLIPS (root px), from SOURCES, with K's shadow tiles; the number of layers drawn.
/// That number and the time taken go to K's tally for C's root.  `GLITNIR_E_STATE`, with nothing drawn, when a
/// source's scale is not the target's.  It takes no lock, so any thread may compose a snapshot while the host goes on
/// changing the scene.
pub fn compose_committed(
    c: &Committed,
    k: &mut Composer,
    t: &mut TargetView,
    clips: &[Rect],
    sources: &[SourceView],
) -> Result<u32, i32> {
    compose_committed_with(c, k, t, clips, sources, &Extras::NONE)
}

/// [`compose_committed`] with X drawn too.
pub fn compose_committed_with(
    c: &Committed,
    k: &mut Composer,
    t: &mut TargetView,
    clips: &[Rect],
    sources: &[SourceView],
    x: &Extras,
) -> Result<u32, i32> {
    let start = Instant::now();
    #[cfg(test)]
    if let Some(g) = &k.gate {
        let _ = g.recv_timeout(std::time::Duration::from_secs(5));
    }
    let shadows = k.shadows.get_or_insert_with(ShadowCache::new);
    let layers = compose_layers(
        crate::kernels::kernels(),
        &c.layers,
        &c.labels,
        t,
        clips,
        sources,
        shadows,
        &c.sprites,
        x,
    )?;
    let tally = k.tallies.entry(c.root).or_default();
    let ns = u64::try_from(start.elapsed().as_nanos()).unwrap_or(u64::MAX);
    tally.ns = tally.ns.saturating_add(ns);
    tally.layers = tally.layers.saturating_add(layers);
    Ok(layers)
}

/// The C ABI's composer: the host composes on its main thread.
static PROCESS: Mutex<Composer> = Mutex::new(Composer::new());

fn process() -> MutexGuard<'static, Composer> {
    PROCESS.lock().unwrap_or_else(|p| p.into_inner())
}

/// Compose ROOT's committed scene (`glitnir_compose`) with the C ABI's composer.  `GLITNIR_E_STATE` when ROOT has no
/// scene.
pub fn compose_root(
    root: u64,
    t: &mut TargetView,
    clips: &[Rect],
    sources: &[SourceView],
) -> Result<(), i32> {
    let c = crate::scene::committed(root).ok_or(GLITNIR_E_STATE)?;
    compose_committed(&c, &mut process(), t, clips, sources).map(|_| ())
}

/// [`Composer::fill_stats`] from the C ABI's composer (`glitnir_stats_record`).
pub fn fill_stats(root: u64, e: &mut StatsEntry) {
    process().fill_stats(root, e);
}

/// Forget ROOT's composes in the C ABI's composer (the root is gone).
pub fn forget(root: u64) {
    process().forget(root);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::damage::Frames;
    use crate::decor::Anchor;
    use crate::ffi::{
        GLITNIR_OK, glitnir_commit, glitnir_compose, glitnir_damage_forget, glitnir_decor_drop_all,
        glitnir_decor_put, glitnir_decor_sprite_put, glitnir_layer_set_chrome,
        glitnir_layer_upsert, glitnir_root_drop, glitnir_root_new,
    };
    use crate::kernels::kernels;
    use crate::rng::XorShift;
    use crate::scene::{Chrome, LayerDesc, Scene};
    use std::time::Duration;

    const ROOT: u64 = 1;

    /// An owned premultiplied ARGB32 buffer, `w` pixels a row.
    #[derive(Clone, PartialEq)]
    struct Buf {
        w: i32,
        h: i32,
        px: Vec<u32>,
    }

    /// A premultiplied pixel, opaque three times in four.
    fn pixel(g: &mut XorShift) -> u32 {
        let a = if g.below(4) == 0 { g.below(256) } else { 255 };
        let mut c = || g.below(a + 1);
        (a << 24) | (c() << 16) | (c() << 8) | c()
    }

    impl Buf {
        fn new(w: i32, h: i32, fill: u32) -> Buf {
            Buf {
                w,
                h,
                px: vec![fill; (w * h) as usize],
            }
        }

        fn random(g: &mut XorShift, w: i32, h: i32) -> Buf {
            Buf {
                w,
                h,
                px: (0..w * h).map(|_| pixel(g)).collect(),
            }
        }

        fn at(&self, x: i32, y: i32) -> u32 {
            self.px[(y * self.w + x) as usize]
        }

        fn set(&mut self, x: i32, y: i32, v: u32) {
            self.px[(y * self.w + x) as usize] = v;
        }

        fn source(&self, layer: u64, scale: f64) -> SourceView<'_> {
            SourceView {
                layer,
                data: &self.px,
                stride: self.w as usize,
                width: self.w,
                height: self.h,
                scale,
            }
        }

        fn target(&mut self, scale: f64) -> TargetView<'_> {
            TargetView {
                data: &mut self.px,
                stride: self.w as usize,
                rect: Rect::new(0, 0, self.w, self.h),
                scale,
            }
        }

        /// New pixels in R (clipped to the buffer).
        fn paint(&mut self, g: &mut XorShift, r: Rect) {
            let r = r.intersect(&Rect::new(0, 0, self.w, self.h));
            for y in r.y..r.y + r.h {
                for x in r.x..r.x + r.w {
                    let v = pixel(g);
                    self.set(x, y, v);
                }
            }
        }

        /// Scroll AREA's rows by DY (content moves down when positive); the rows it leaves stale get new pixels.
        /// Returns those rows.
        fn scroll(&mut self, g: &mut XorShift, area: Rect, dy: i32) -> Rect {
            let old = self.clone();
            let rows = area.y..area.y + area.h;
            for y in rows.clone() {
                for x in area.x..area.x + area.w {
                    let v = if rows.contains(&(y - dy)) {
                        old.at(x, y - dy)
                    } else {
                        pixel(g)
                    };
                    self.set(x, y, v);
                }
            }
            if dy < 0 {
                Rect::new(area.x, area.y + area.h + dy, area.w, -dy)
            } else {
                Rect::new(area.x, area.y, area.w, dy)
            }
        }
    }

    fn first_difference(a: &Buf, b: &Buf) -> Option<(i32, i32, u32, u32)> {
        let i = a.px.iter().zip(&b.px).position(|(x, y)| x != y)?;
        let (x, y) = (i as i32 % a.w, i as i32 / a.w);
        Some((x, y, a.px[i], b.px[i]))
    }

    fn desc(id: u64, parent: u64, kind: u32, rect: Rect, scale: f64, visible: bool) -> LayerDesc {
        LayerDesc {
            id,
            parent,
            kind,
            rect,
            scale,
            visible: visible as u8,
        }
    }

    /// Compose S's committed scene into OUT within CLIPS, from SOURCES (by layer id): its snapshot through
    /// [`compose_committed`], which must draw what composing the scene's committed layers draws.
    fn run(
        s: &Scene,
        out: &mut Buf,
        scale: f64,
        clips: &[Rect],
        sources: &BTreeMap<u64, Buf>,
        shadows: &mut ShadowCache,
    ) -> u32 {
        let views: Vec<SourceView> = sources.iter().map(|(&id, b)| b.source(id, scale)).collect();
        let mut locked = out.clone();
        let n = compose(
            kernels(),
            s.committed(),
            &mut locked.target(scale),
            clips,
            &views,
            shadows,
        )
        .unwrap();
        let mut k = Composer::new();
        let got = compose_committed(&s.snapshot(), &mut k, &mut out.target(scale), clips, &views);
        assert_eq!(got, Ok(n));
        assert_eq!(first_difference(out, &locked), None);
        n
    }

    fn covers(damage: &[Rect], r: Rect) -> bool {
        (r.y..r.y + r.h).all(|y| (r.x..r.x + r.w).all(|x| damage.iter().any(|d| d.contains(x, y))))
    }

    fn rand_chrome(g: &mut XorShift) -> Chrome {
        let half = |g: &mut XorShift| if g.below(4) == 0 { 0.5 } else { 0.0 };
        Chrome {
            radius_px: g.below(12) as f32,
            blur_px: g.below(13) as f32,
            offset_x: g.below(9) as f32 - 4.0 + half(g),
            offset_y: g.below(9) as f32 - 4.0 + half(g),
            spread_px: g.below(5) as f32 - 2.0,
            shadow_opacity: if g.below(4) == 0 {
                0.0
            } else {
                (1 + g.below(100)) as f32 / 100.0
            },
            shadow_argb: pixel(g),
            border_px: if g.below(2) == 0 {
                0.0
            } else {
                (1 + g.below(3)) as f32
            },
            border_argb: pixel(g),
        }
    }

    /// A child frame of the random scenes: a child node, with one window layer (id 100 + id) inset by 2 px.
    #[derive(Clone, Copy)]
    struct Kid {
        id: u64,
        parent: u64,
        rect: Rect,
        visible: bool,
    }

    impl Kid {
        fn window(&self) -> Rect {
            Rect::new(
                self.rect.x + 2,
                self.rect.y + 2,
                self.rect.w - 4,
                self.rect.h - 4,
            )
        }

        fn put(&self, s: &mut Scene, scale: f64) {
            s.upsert(&desc(
                self.id,
                self.parent,
                3,
                self.rect,
                scale,
                self.visible,
            ))
            .unwrap();
            s.upsert(&desc(100 + self.id, self.id, 2, self.window(), scale, true))
                .unwrap();
        }
    }

    #[test]
    fn composing_the_damage_equals_composing_everything() {
        const W: i32 = 96;
        const H: i32 = 72;
        const ROOT_WINDOW: u64 = 99;
        let all = Rect::new(0, 0, W, H);
        let root_window = Rect::new(4, 4, W - 8, H - 16);
        let mut shadows = ShadowCache::new();
        let rand_rect = |g: &mut XorShift| {
            Rect::new(
                g.below(W as u32 + 16) as i32 - 16,
                g.below(H as u32 + 16) as i32 - 16,
                1 + g.below(48) as i32,
                1 + g.below(40) as i32,
            )
        };
        for seed in 1..=500u64 {
            let mut g = XorShift::new(seed);
            let scale = if g.below(2) == 0 { 1.0 } else { 2.0 };
            let (mut s, mut f) = (Scene::default(), Frames::new());
            let mut src: BTreeMap<u64, Buf> = BTreeMap::new();
            s.upsert(&desc(ROOT, 0, 1, all, scale, true)).unwrap();
            s.upsert(&desc(ROOT_WINDOW, ROOT, 2, root_window, scale, true))
                .unwrap();
            src.insert(ROOT, Buf::random(&mut g, W, H));
            let mut kids: Vec<Kid> = Vec::new();
            for i in 0..1 + g.below(4) as u64 {
                let parent = if i > 0 && g.below(3) == 0 {
                    10 + g.below(i as u32) as u64
                } else {
                    ROOT
                };
                let k = Kid {
                    id: 10 + i,
                    parent,
                    rect: rand_rect(&mut g),
                    visible: g.below(5) != 0,
                };
                k.put(&mut s, scale);
                src.insert(k.id, Buf::random(&mut g, k.rect.w, k.rect.h));
                if g.below(3) != 0 {
                    s.set_chrome(k.id, Some(rand_chrome(&mut g))).unwrap();
                }
                kids.push(k);
            }
            s.commit(ROOT, &mut f);
            let mut out = Buf::random(&mut g, W, H);
            run(&s, &mut out, scale, &[all], &src, &mut shadows);
            for round in 0..4 {
                for _ in 0..1 + g.below(3) {
                    let pick = g.below(8);
                    if kids.is_empty() && pick != 3 && pick != 4 {
                        continue;
                    }
                    let i = g.below(kids.len().max(1) as u32) as usize;
                    match pick {
                        // move or resize; a new size is a new surface
                        0 => {
                            let k = &mut kids[i];
                            let r = rand_rect(&mut g);
                            if (r.w, r.h) != (k.rect.w, k.rect.h) {
                                src.insert(k.id, Buf::random(&mut g, r.w, r.h));
                            }
                            k.rect = r;
                            k.put(&mut s, scale);
                        }
                        1 => {
                            kids[i].visible = !kids[i].visible;
                            kids[i].put(&mut s, scale);
                        }
                        2 => {
                            let c = (g.below(4) != 0).then(|| rand_chrome(&mut g));
                            s.set_chrome(kids[i].id, c).unwrap();
                        }
                        // a frame draws
                        3 => {
                            let id = if kids.is_empty() || g.below(3) == 0 {
                                ROOT
                            } else {
                                kids[i].id
                            };
                            let b = src.get_mut(&id).unwrap();
                            let r = Rect::new(
                                g.below(b.w as u32 + 4) as i32 - 2,
                                g.below(b.h as u32 + 4) as i32 - 2,
                                1 + g.below(16) as i32,
                                1 + g.below(16) as i32,
                            );
                            b.paint(&mut g, r);
                            f.entry(id).or_default().add(r);
                        }
                        // a frame scrolls one of its windows (frame coordinates)
                        4 => {
                            let (id, window, area) = if kids.is_empty() || g.below(3) == 0 {
                                (ROOT, ROOT_WINDOW, root_window)
                            } else {
                                let k = &kids[i];
                                let w = k.window();
                                (k.id, 100 + k.id, w.translate(-k.rect.x, -k.rect.y))
                            };
                            if area.is_empty() {
                                continue;
                            }
                            let dy = (1 + g.below((area.h as u32 / 2).max(1)) as i32)
                                * if g.below(2) == 0 { 1 } else { -1 };
                            let exposed = src.get_mut(&id).unwrap().scroll(&mut g, area, dy);
                            let d = f.entry(id).or_default();
                            d.scroll(area, exposed);
                            d.scrolled(window);
                        }
                        5 => {
                            let id = kids[i].id;
                            if g.below(2) == 0 {
                                s.raise(id).unwrap();
                            } else {
                                s.lower(id).unwrap();
                            }
                        }
                        6 => {
                            let j = g.below(kids.len() as u32 + 1) as usize;
                            let parent = kids.get(j).map_or(ROOT, |k| k.id);
                            if s.reparent(kids[i].id, parent).is_ok() {
                                kids[i].parent = parent;
                            }
                        }
                        _ => {
                            s.remove(kids[i].id);
                            let live = s.list().to_vec();
                            kids.retain(|k| live.contains(&k.id));
                        }
                    }
                }
                let damage = s.commit(ROOT, &mut f).to_vec();
                run(&s, &mut out, scale, &damage, &src, &mut shadows);
                let mut want = Buf::random(&mut g, W, H);
                run(&s, &mut want, scale, &[all], &src, &mut shadows);
                if let Some((x, y, got, all_px)) = first_difference(&out, &want) {
                    panic!(
                        "seed {seed} round {round}: pixel {x},{y} is {got:08x}, composing everything gives \
                         {all_px:08x}; damage {damage:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn scroll_op_compose_equals_scrolled_content() {
        let (w, h, scale) = (160, 120, 2.0);
        let all = Rect::new(0, 0, w, h);
        let mut g = XorShift::new(7);
        let (mut s, mut f) = (Scene::default(), Frames::new());
        let mut shadows = ShadowCache::new();
        let mut src: BTreeMap<u64, Buf> = BTreeMap::new();
        s.upsert(&desc(ROOT, 0, 1, all, scale, true)).unwrap();
        let win = Rect::new(8, 8, 144, 96);
        s.upsert(&desc(50, ROOT, 2, win, scale, true)).unwrap();
        // a chromed popup over the window, with a window of its own
        let pop = Rect::new(90, 20, 60, 40);
        s.upsert(&desc(10, ROOT, 3, pop, scale, true)).unwrap();
        s.set_chrome(
            10,
            Some(Chrome {
                radius_px: 8.0,
                blur_px: 6.0,
                offset_y: 3.0,
                shadow_opacity: 0.4,
                shadow_argb: 0xff00_0000,
                border_px: 2.0,
                border_argb: 0xff40_80c0,
                ..Chrome::default()
            }),
        )
        .unwrap();
        let pwin = Rect::new(92, 22, 56, 36);
        s.upsert(&desc(51, 10, 2, pwin, scale, true)).unwrap();
        src.insert(ROOT, Buf::random(&mut g, w, h));
        src.insert(10, Buf::random(&mut g, pop.w, pop.h));
        s.commit(ROOT, &mut f);
        let mut out = Buf::new(w, h, 0);
        run(&s, &mut out, scale, &[all], &src, &mut shadows);

        // the root's window scrolls up 12 rows; the popup's window down 5 (in the popup's coordinates)
        let before = src[&ROOT].clone();
        let dy = 12;
        let exposed = src.get_mut(&ROOT).unwrap().scroll(&mut g, win, -dy);
        let d = f.entry(ROOT).or_default();
        d.scroll(win, exposed);
        d.scrolled(50);
        let local = pwin.translate(-pop.x, -pop.y);
        let exposed = src.get_mut(&10).unwrap().scroll(&mut g, local, 5);
        let d = f.entry(10).or_default();
        d.scroll(local, exposed);
        d.scrolled(51);
        let damage = s.commit(ROOT, &mut f).to_vec();
        assert!(covers(&damage, win) && covers(&damage, pwin), "{damage:?}");
        run(&s, &mut out, scale, &damage, &src, &mut shadows);

        // beside the popup and its shadow the window shows its content 12 rows up
        let popup = s.committed().iter().find(|p| p.id == 10).unwrap().bounds;
        for y in win.y..win.y + win.h - dy {
            for x in win.x..win.x + win.w {
                if !popup.contains(x, y) {
                    assert_eq!(out.at(x, y), before.at(x, y + dy), "pixel {x},{y}");
                }
            }
        }
        // and the whole picture is the scrolled sources composed whole
        let mut want = Buf::new(w, h, 0);
        run(&s, &mut want, scale, &[all], &src, &mut shadows);
        assert_eq!(first_difference(&out, &want), None);
    }

    #[test]
    fn no_chrome_is_a_plain_copy() {
        let (w, h, scale) = (64, 48, 1.0);
        let all = Rect::new(0, 0, w, h);
        let mut g = XorShift::new(3);
        let (mut s, mut f) = (Scene::default(), Frames::new());
        let mut shadows = ShadowCache::new();
        let (a, b) = (Rect::new(10, 8, 20, 16), Rect::new(30, 20, 25, 20));
        s.upsert(&desc(ROOT, 0, 1, all, scale, true)).unwrap();
        s.upsert(&desc(10, ROOT, 3, a, scale, true)).unwrap();
        s.upsert(&desc(11, 10, 2, Rect::new(10, 8, 20, 12), scale, true))
            .unwrap();
        s.upsert(&desc(20, ROOT, 3, b, scale, true)).unwrap();
        // all-zero chrome: no radius, no shadow, no border
        s.set_chrome(20, Some(Chrome::default())).unwrap();
        s.commit(ROOT, &mut f);
        let mut src: BTreeMap<u64, Buf> = BTreeMap::new();
        src.insert(ROOT, Buf::random(&mut g, w, h));
        src.insert(10, Buf::random(&mut g, a.w, a.h));
        src.insert(20, Buf::random(&mut g, b.w, b.h));
        // a source no layer names is ignored
        src.insert(77, Buf::random(&mut g, 8, 8));

        // the sources copied in stacking order, translucent pixels included
        let mut want = src[&ROOT].clone();
        for (id, r) in [(10, a), (20, b)] {
            for y in 0..r.h {
                for x in 0..r.w {
                    want.set(r.x + x, r.y + y, src[&id].at(x, y));
                }
            }
        }
        let garbage = Buf::random(&mut g, w, h);
        let mut out = garbage.clone();
        assert_eq!(run(&s, &mut out, scale, &[all], &src, &mut shadows), 3);
        assert_eq!(first_difference(&out, &want), None);

        // within overlapping clips: those pixels, once; the rest stays as it was
        let clips = [Rect::new(5, 5, 30, 20), Rect::new(20, 15, 20, 20)];
        let mut out = garbage.clone();
        run(&s, &mut out, scale, &clips, &src, &mut shadows);
        for y in 0..h {
            for x in 0..w {
                let inside = clips.iter().any(|c| c.contains(x, y));
                let expect = if inside {
                    want.at(x, y)
                } else {
                    garbage.at(x, y)
                };
                assert_eq!(out.at(x, y), expect, "pixel {x},{y}");
            }
        }
    }

    #[test]
    fn scale_mismatch_is_refused() {
        let r = 8301;
        let (w, h) = (32, 24);
        assert_eq!(glitnir_root_new(r), GLITNIR_OK);
        let layer = desc(r, 0, 1, Rect::new(0, 0, w, h), 2.0, true);
        assert_eq!(unsafe { glitnir_layer_upsert(r, &layer) }, GLITNIR_OK);
        let mut committed = crate::ffi::CommitOut {
            rects: std::ptr::null(),
            n: 0,
        };
        assert_eq!(unsafe { glitnir_commit(r, &mut committed) }, GLITNIR_OK);
        let root_px = vec![0xff10_2030u32; (w * h) as usize];
        let mut out = vec![0u32; (w * h) as usize];
        let target = Target {
            pixels: out.as_mut_ptr().cast(),
            stride: w * 4,
            width: w,
            height: h,
            origin_x: 0,
            origin_y: 0,
            scale: 2.0,
        };
        let source = |scale| Source {
            layer: r,
            pixels: root_px.as_ptr().cast(),
            stride: w * 4,
            width: w,
            height: h,
            scale,
        };
        let clip = [Rect::new(0, 0, w, h)];
        let call =
            |t: &Target, s: &Source| unsafe { glitnir_compose(r, t, clip.as_ptr(), 1, s, 1) };

        // a source at scale 1 into a scale-2 target: refused, nothing drawn
        assert_eq!(call(&target, &source(1.0)), GLITNIR_E_STATE);
        assert!(out.iter().all(|&p| p == 0));
        // the same scale: composed
        assert_eq!(call(&target, &source(2.0)), GLITNIR_OK);
        assert_eq!(out, root_px);
        // the time it took and the layers it drew reach the next record, once
        let mut e = StatsEntry {
            t_draw_end_ns: 50_000_000_000,
            ..StatsEntry::default()
        };
        fill_stats(r, &mut e);
        assert_eq!((e.layers, e.t_compose_end_ns), (1, 50_000_000_000));
        assert!(e.t_compose_start_ns > 0 && e.t_compose_start_ns < e.t_compose_end_ns);
        let mut again = StatsEntry::default();
        fill_stats(r, &mut again);
        assert_eq!(again, StatsEntry::default());

        // no scene; no target; a stride shorter than a row or not whole pixels; clips missing
        let ok = source(2.0);
        let compose_into =
            |root, t: *const Target| unsafe { glitnir_compose(root, t, clip.as_ptr(), 1, &ok, 1) };
        assert_eq!(
            compose_into(r + 1, std::ptr::from_ref(&target)),
            GLITNIR_E_STATE
        );
        assert_eq!(compose_into(r, std::ptr::null()), GLITNIR_E_ARG);
        for stride in [w * 4 - 4, w * 4 + 2] {
            let bad = Target { stride, ..target };
            assert_eq!(compose_into(r, std::ptr::from_ref(&bad)), GLITNIR_E_ARG);
        }
        assert_eq!(
            unsafe { glitnir_compose(r, &target, std::ptr::null(), 1, &ok, 1) },
            GLITNIR_E_ARG
        );
        assert_eq!(glitnir_root_drop(r), GLITNIR_OK);
        assert_eq!(glitnir_damage_forget(r), GLITNIR_OK);
        forget(r);
    }

    fn put_sprite(key: u64, b: &Buf) {
        let rc = unsafe { glitnir_decor_sprite_put(key, b.px.as_ptr().cast(), b.w * 4, b.w, b.h) };
        assert_eq!(rc, GLITNIR_OK);
    }

    fn ffi_sources(sources: &BTreeMap<u64, Buf>) -> Vec<Source> {
        sources
            .iter()
            .map(|(&layer, b)| Source {
                layer,
                pixels: b.px.as_ptr().cast(),
                stride: b.w * 4,
                width: b.w,
                height: b.h,
                scale: 1.0,
            })
            .collect()
    }

    /// The native shell keeps its windows' anchors on the frame's one layer: the label is placed there, damages its
    /// bounds and goes over the frame's pixels.
    #[test]
    fn a_label_on_the_frame_layer_goes_over_its_pixels() {
        const W: i32 = 64;
        const H: i32 = 48;
        let mut s = Scene::default();
        let mut frames = crate::damage::Frames::new();
        let up = desc(1, 0, 1, Rect::new(0, 0, W, H), 1.0, true);
        s.upsert(&up).unwrap();
        let mut sprites = Sprites::new();
        sprites.put(7, vec![0xffff_8000; 12 * 6], 12, 6);
        let sprites = std::sync::Arc::new(sprites);
        s.commit_with(1, &mut frames, &sprites);
        let a = Anchor {
            glyph: Rect::new(16, 24, 8, 12),
            clip: Rect::new(0, 0, W, H),
            sprite: 7,
            row_top: 0,
        };
        s.decor_put(1, &a).unwrap();
        let damage = s.commit_with(1, &mut frames, &sprites).to_vec();
        let pill = Rect::new(16, 18, 12, 6);
        assert!(
            damage.iter().any(|r| !r.intersect(&pill).is_empty()),
            "{damage:?}"
        );
        let src = Buf::new(W, H, 0xff20_2020);
        let mut out = Buf::new(W, H, 0);
        let drawn = compose_committed(
            &s.snapshot(),
            &mut Composer::new(),
            &mut out.target(1.0),
            &[Rect::new(0, 0, W, H)],
            &[src.source(1, 1.0)],
        );
        assert_eq!(drawn, Ok(1));
        assert_eq!(out.at(16, 18), 0xffff_8000, "the pill");
        assert_eq!(out.at(16, 24), 0xff20_2020, "the glyph below it");
    }

    #[test]
    fn committed_compose_equals_locked_compose() {
        // Random scenes with chrome and labels, built through the C ABI.  What `glitnir_compose` draws right after a
        // commit (the host composing under its own thread, as pgtk does) equals what the commit's snapshot draws
        // after the host has moved on: layers moved and re-chromed, anchors dropped, sprites replaced.
        const W: i32 = 96;
        const H: i32 = 72;
        let all = Rect::new(0, 0, W, H);
        for seed in 1..=60u64 {
            let mut g = XorShift::new(seed);
            let root = 870_000 + seed * 100;
            let win = root + 1;
            // two keys for every seed, so the shared sprite cache keeps the other tests' sprites
            let keys = [0x8700_0001, 0x8700_0002];
            assert_eq!(glitnir_root_new(root), GLITNIR_OK);
            let upsert = |id, parent, kind, rect| {
                let d = desc(id, parent, kind, rect, 1.0, true);
                assert_eq!(unsafe { glitnir_layer_upsert(root, &d) }, GLITNIR_OK);
            };
            upsert(root, 0, 1, all);
            upsert(win, root, 2, Rect::new(4, 4, W - 8, H - 16));
            let mut src: BTreeMap<u64, Buf> = BTreeMap::new();
            src.insert(root, Buf::random(&mut g, W, H));
            let mut kids: Vec<(u64, Rect)> = Vec::new();
            for i in 0..1 + g.below(3) as u64 {
                let id = root + 10 + i;
                let r = Rect::new(
                    g.below(W as u32) as i32 - 8,
                    g.below(H as u32) as i32 - 8,
                    8 + g.below(40) as i32,
                    8 + g.below(30) as i32,
                );
                upsert(id, root, 3, r);
                src.insert(id, Buf::random(&mut g, r.w, r.h));
                kids.push((id, r));
            }
            let change = |g: &mut XorShift, kids: &[(u64, Rect)]| {
                for _ in 0..1 + g.below(4) {
                    let (id, r) = kids[g.below(kids.len() as u32) as usize];
                    match g.below(5) {
                        // a move keeps the size, so the sources stay valid
                        0 => upsert(id, root, 3, r.translate(g.below(9) as i32 - 4, 3)),
                        1 => {
                            let c = rand_chrome(g);
                            let rc = unsafe { glitnir_layer_set_chrome(root, id, &c) };
                            assert_eq!(rc, GLITNIR_OK);
                        }
                        2 => {
                            let key = keys[g.below(2) as usize];
                            let (sw, sh) = (4 + g.below(12) as i32, 3 + g.below(6) as i32);
                            put_sprite(key, &Buf::random(g, sw, sh));
                        }
                        3 => {
                            let cell =
                                |c: u32, r: u32| Rect::new(c as i32 * 8, r as i32 * 12, 8, 12);
                            let a = Anchor {
                                glyph: cell(g.below(10), 1 + g.below(4)),
                                clip: Rect::new(0, 0, W - 8, H - 16),
                                sprite: keys[g.below(2) as usize],
                                row_top: 0,
                            };
                            assert_eq!(unsafe { glitnir_decor_put(root, win, &a) }, GLITNIR_OK);
                        }
                        _ => assert_eq!(glitnir_decor_drop_all(root, win), GLITNIR_OK),
                    }
                }
            };
            for key in keys {
                put_sprite(key, &Buf::random(&mut g, 10, 5));
            }
            let sources = ffi_sources(&src);
            let views: Vec<SourceView> = src.iter().map(|(&id, b)| b.source(id, 1.0)).collect();
            let mut k = Composer::new();
            for round in 0..4 {
                change(&mut g, &kids);
                let mut out = crate::ffi::CommitOut {
                    rects: std::ptr::null(),
                    n: 0,
                };
                assert_eq!(unsafe { glitnir_commit(root, &mut out) }, GLITNIR_OK);
                let mut locked = Buf::new(W, H, 0);
                let t = Target {
                    pixels: locked.px.as_mut_ptr().cast(),
                    stride: W * 4,
                    width: W,
                    height: H,
                    origin_x: 0,
                    origin_y: 0,
                    scale: 1.0,
                };
                let rc =
                    unsafe { glitnir_compose(root, &t, &all, 1, sources.as_ptr(), sources.len()) };
                assert_eq!(rc, GLITNIR_OK);
                let snap = crate::scene::committed(root).unwrap();
                change(&mut g, &kids);
                let mut later = Buf::new(W, H, 0);
                compose_committed(&snap, &mut k, &mut later.target(1.0), &[all], &views).unwrap();
                if let Some((x, y, a, b)) = first_difference(&later, &locked) {
                    panic!(
                        "seed {seed} round {round}: pixel {x},{y} is {a:08x}, composed at the commit {b:08x}"
                    );
                }
            }
            assert_eq!(glitnir_root_drop(root), GLITNIR_OK);
            for id in std::iter::once(root).chain(kids.iter().map(|k| k.0)) {
                assert_eq!(glitnir_damage_forget(id), GLITNIR_OK);
            }
            forget(root);
        }
    }

    #[test]
    fn no_global_lock_during_compose() {
        // a 4K root with a chromed popup, committed; thread A composes it and is held inside the compose until the
        // host has drawn: a lock held across the compose would block the host until the gate's 5 s timeout
        let (w, h) = (3840, 2160);
        let root = 8801;
        let pop = Rect::new(800, 500, 1280, 800);
        assert_eq!(glitnir_root_new(root), GLITNIR_OK);
        for (id, kind, r) in [(root, 1, Rect::new(0, 0, w, h)), (root + 1, 3, pop)] {
            let d = desc(id, if id == root { 0 } else { root }, kind, r, 1.0, true);
            assert_eq!(unsafe { glitnir_layer_upsert(root, &d) }, GLITNIR_OK);
        }
        let mut g = XorShift::new(11);
        let c = rand_chrome(&mut g);
        assert_eq!(
            unsafe { glitnir_layer_set_chrome(root, root + 1, &c) },
            GLITNIR_OK
        );
        let (_, snap) = crate::damage::commit_snapshot(root).unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let (release, gate) = std::sync::mpsc::channel();
        let a = std::thread::spawn(move || {
            crate::mark_compose_thread();
            let root_px = Buf::new(w, h, 0xfff0_f0f0);
            let popup = Buf::new(pop.w, pop.h, 0xff20_2428);
            let sources = [root_px.source(root, 1.0), popup.source(root + 1, 1.0)];
            let mut out = Buf::new(w, h, 0);
            let mut k = Composer::new();
            k.gate = Some(gate);
            tx.send(()).unwrap();
            compose_committed(
                &snap,
                &mut k,
                &mut out.target(1.0),
                &[Rect::new(0, 0, w, h)],
                &sources,
            )
        });
        rx.recv().unwrap();
        // A is inside its compose; the host goes on drawing into the same root
        std::thread::sleep(Duration::from_millis(5));
        let t0 = Instant::now();
        for i in 0..1000 {
            crate::damage::with(root, |d| d.add(Rect::new(i % 640, i / 640, 1, 1)));
        }
        let took = t0.elapsed();
        release.send(()).unwrap();
        assert_eq!(a.join().unwrap(), Ok(2));
        assert_eq!(glitnir_root_drop(root), GLITNIR_OK);
        for id in [root, root + 1] {
            assert_eq!(glitnir_damage_forget(id), GLITNIR_OK);
        }
        assert!(
            took < Duration::from_secs(1),
            "1000 damage calls took {took:?}"
        );
    }

    /// A child frame's scene: its one layer at `rect`, with `chrome`.
    fn kid_scene(rect: Rect, chrome: Option<Chrome>) -> Scene {
        let (mut s, mut f) = (Scene::default(), Frames::new());
        s.upsert(&desc(ROOT, 0, 1, rect, 1.0, true)).unwrap();
        s.set_chrome(ROOT, chrome).unwrap();
        s.commit(ROOT, &mut f);
        s
    }

    fn with_extras(s: &Scene, x: &Extras, content: &Buf, out: &mut Buf, clips: &[Rect]) {
        compose_committed_with(
            &s.snapshot(),
            &mut Composer::new(),
            &mut out.target(1.0),
            clips,
            &[content.source(ROOT, 1.0)],
            x,
        )
        .unwrap();
    }

    /// The blurred `parent` for a layer lying at `region` in it.
    fn blurred(parent: &Buf, region: Rect, sigma: f32) -> Backdrop {
        let mut b = Backdrop::new();
        let size = (parent.w, parent.h);
        assert_eq!(
            b.update(&parent.px, parent.w as usize, size, region, sigma, 1.0, &[]),
            Some(region)
        );
        b
    }

    #[test]
    fn backdrop_under_content_inside_corners() {
        const BENEATH: u32 = 0xff40_80c0;
        let (surface, rect) = (Rect::new(0, 0, 48, 48), Rect::new(8, 8, 32, 32));
        let chrome = Chrome {
            radius_px: 8.0,
            blur_px: 6.0,
            shadow_opacity: 0.5,
            shadow_argb: 0xff00_0000,
            ..Chrome::default()
        };
        let s = kid_scene(rect, Some(chrome));
        let parent = Buf::new(96, 96, BENEATH);
        let b = blurred(&parent, Rect::new(20, 30, 32, 32), 8.0);
        let x = Extras {
            backdrop: Some((&b, ROOT)),
        };
        let content = Buf::new(32, 32, 0x8080_8080);
        let mut out = Buf::new(48, 48, 0);
        with_extras(&s, &x, &content, &mut out, &[surface]);
        // the middle: half white OVER the flat backdrop
        let over = |c: u32| 0x80 + (c * 0x7f + 127) / 255;
        let middle = out.at(24, 24);
        for (i, c) in [0xc0, 0x80, 0x40].into_iter().enumerate() {
            let got = (middle >> (8 * i)) & 0xff;
            assert!(got.abs_diff(over(c)) <= 1, "channel {i}: {middle:08x}");
        }
        assert_eq!(middle >> 24, 0xff);
        // past the rounded corner the backdrop is not drawn: a compose without one gives the same pixel
        let mut plain = Buf::new(48, 48, 0);
        with_extras(&s, &Extras::NONE, &content, &mut plain, &[surface]);
        assert_eq!(out.at(8, 8), plain.at(8, 8));
        assert_ne!(out.at(8, 8) >> 24, 0xff);
        assert_eq!(out.at(2, 24), plain.at(2, 24), "the shadow is the same");
        // the edge of the disc takes some of each
        let edge = (out.at(10, 10) >> 24, plain.at(10, 10) >> 24);
        assert!(edge.0 > edge.1, "{edge:?}");
    }

    #[test]
    fn extras_none_equals_compose_committed() {
        const W: i32 = 64;
        const H: i32 = 48;
        let all = Rect::new(0, 0, W, H);
        for seed in 1..=100 {
            let mut g = XorShift::new(seed);
            let rect = Rect::new(6, 5, 40, 30);
            let s = kid_scene(rect, Some(rand_chrome(&mut g)));
            let content = Buf::random(&mut g, rect.w, rect.h);
            let parent = Buf::random(&mut g, 96, 96);
            let garbage = Buf::random(&mut g, W, H);
            let (mut a, mut b, mut c) = (garbage.clone(), garbage.clone(), garbage);
            let mut shadows = ShadowCache::new();
            let views = [content.source(ROOT, 1.0)];
            let scene = s.committed();
            compose(
                kernels(),
                scene,
                &mut a.target(1.0),
                &[all],
                &views,
                &mut shadows,
            )
            .unwrap();
            with_extras(&s, &Extras::NONE, &content, &mut b, &[all]);
            assert_eq!(first_difference(&a, &b), None, "seed {seed}");
            // a backdrop for a layer that is not here draws nothing
            let beneath = blurred(&parent, rect, 8.0);
            let x = Extras {
                backdrop: Some((&beneath, ROOT + 1)),
            };
            with_extras(&s, &x, &content, &mut c, &[all]);
            assert_eq!(first_difference(&a, &c), None, "seed {seed}");
        }
    }

    /// The key property with a backdrop: a change beneath, or in the layer, composed within what it damaged, equals
    /// composing everything.  (A surface is cleared where it is composed: nothing lies under a child frame's layer.)
    #[test]
    fn damage_only_equals_full_with_backdrop() {
        const W: i32 = 64;
        const H: i32 = 48;
        let all = Rect::new(0, 0, W, H);
        let clear = |b: &mut Buf, clips: &[Rect]| {
            for c in clips {
                let c = c.intersect(&all);
                for y in c.y..c.y + c.h {
                    b.px[(y * W + c.x) as usize..][..c.w as usize].fill(0);
                }
            }
        };
        for seed in 1..=200 {
            let mut g = XorShift::new(seed);
            let rect = Rect::new(4 + g.below(8) as i32, 3 + g.below(6) as i32, 40, 30);
            let chrome = (g.below(4) != 0).then(|| rand_chrome(&mut g));
            let s = kid_scene(rect, chrome);
            let mut content = Buf::random(&mut g, rect.w, rect.h);
            let mut parent = Buf::random(&mut g, 96, 80);
            // the region may hang over the parent's edge
            let region = Rect::new(
                g.below(96) as i32 - 20,
                g.below(80) as i32 - 16,
                rect.w,
                rect.h,
            );
            let sigma = [4.0, 8.0, 10.5][g.below(3) as usize];
            let mut b = Backdrop::new();
            let size = (parent.w, parent.h);
            b.update(&parent.px, 96, size, region, sigma, 1.0, &[]);
            let mut out = Buf::new(W, H, 0);
            with_extras(
                &s,
                &Extras {
                    backdrop: Some((&b, ROOT)),
                },
                &content,
                &mut out,
                &[all],
            );
            for round in 0..6 {
                let mut clips = Vec::new();
                if g.below(2) == 0 {
                    let r = Rect::new(
                        g.below(96) as i32,
                        g.below(80) as i32,
                        1 + g.below(30) as i32,
                        1 + g.below(30) as i32,
                    );
                    parent.paint(&mut g, r);
                    let changed = b.update(&parent.px, 96, size, region, sigma, 1.0, &[r]);
                    let to_layer = (rect.x - region.x, rect.y - region.y);
                    clips.extend(changed.map(|c| c.translate(to_layer.0, to_layer.1)));
                } else {
                    let r = Rect::new(
                        g.below(40) as i32,
                        g.below(30) as i32,
                        1 + g.below(12) as i32,
                        1 + g.below(12) as i32,
                    );
                    content.paint(&mut g, r);
                    clips.push(r.translate(rect.x, rect.y));
                }
                let x = Extras {
                    backdrop: Some((&b, ROOT)),
                };
                clear(&mut out, &clips);
                with_extras(&s, &x, &content, &mut out, &clips);
                let mut want = Buf::new(W, H, 0);
                with_extras(&s, &x, &content, &mut want, &[all]);
                if let Some(d) = first_difference(&out, &want) {
                    panic!("seed {seed} round {round}: {d:?}; clips {clips:?}");
                }
            }
        }
    }

    #[test]
    #[ignore = "a bench: it runs on the bench machine; never a CI gate"]
    fn compose_bench_fits_budget() {
        // a 640×400 logical popup at scale 2 with the default look (radius 8, blur 16, offset (0, 6), opacity 0.35;
        // a 1 px border) shows on a 2880×1800 root: the damage is its bounds, composed with the baseline kernels
        let (w, h, scale) = (2880, 1800, 2.0);
        let pop = Rect::new(800, 500, 1280, 800);
        let (mut s, mut f) = (Scene::default(), Frames::new());
        s.upsert(&desc(ROOT, 0, 1, Rect::new(0, 0, w, h), scale, true))
            .unwrap();
        s.commit(ROOT, &mut f);
        s.upsert(&desc(10, ROOT, 3, pop, scale, true)).unwrap();
        s.set_chrome(
            10,
            Some(Chrome {
                radius_px: 16.0,
                blur_px: 32.0,
                offset_x: 0.0,
                offset_y: 12.0,
                spread_px: 0.0,
                shadow_opacity: 0.35,
                shadow_argb: 0xff00_0000,
                border_px: 2.0,
                border_argb: 0xff60_6060,
            }),
        )
        .unwrap();
        let damage = s.commit(ROOT, &mut f).to_vec();
        let root = Buf::new(w, h, 0xfff0_f0f0);
        let popup = Buf::new(pop.w, pop.h, 0xff20_2428);
        let sources = [root.source(ROOT, scale), popup.source(10, scale)];
        let mut out = Buf::new(w, h, 0);
        let mut shadows = ShadowCache::new();
        let k = crate::kernels::baseline();
        let mut once = || {
            let t0 = Instant::now();
            let n = compose(
                k,
                s.committed(),
                &mut out.target(scale),
                &damage,
                &sources,
                &mut shadows,
            )
            .unwrap();
            assert_eq!(n, 2);
            t0.elapsed().as_secs_f64() * 1e3
        };
        once();
        let mut ms: Vec<f64> = (0..31).map(|_| once()).collect();
        ms.sort_by(f64::total_cmp);
        let (median, max) = (ms[ms.len() / 2], ms[ms.len() - 1]);
        eprintln!(
            "compose bench (baseline): median {median:.3} ms, max {max:.3} ms over {damage:?}"
        );
        assert!(median <= 2.0, "median {median:.3} ms > 2 ms");
    }

    /// Window cards: the corners over what lies beneath the window, on the frame and on an own output that scrolls.
    mod card {
        use super::*;
        use crate::committed::OutputFrame;
        use crate::kernels::corner_coverage;
        use crate::scene::{Card, card_corners};

        const WIN: u64 = (1 << 63) | 7;
        const CARD: u64 = (1 << 62) | 7;
        /// A second window layer.
        const NEXT: u64 = (1 << 63) | 8;
        const BENEATH: u32 = 0xff11_2233;
        /// A ring at 60 %, premultiplied.
        const RING: u32 = 0x9926_3039;

        fn radius(r: f32) -> Chrome {
            Chrome {
                radius_px: r,
                ..Chrome::default()
            }
        }

        /// A W x H root at SCALE over FRONT's pixels, with window layer id at RECT, a card of radius R, committed.
        fn carded(w: i32, h: i32, scale: f64, id: u64, rect: Rect, r: f32) -> (Scene, Frames) {
            let (mut s, mut f) = (Scene::default(), Frames::new());
            s.upsert(&desc(ROOT, 0, 1, Rect::new(0, 0, w, h), scale, true))
                .unwrap();
            s.upsert(&desc(id, ROOT, 2, rect, scale, true)).unwrap();
            s.set_chrome(id, Some(radius(r))).unwrap();
            s.set_card(
                id,
                Some(Card {
                    beneath_argb: BENEATH,
                    ..Card::default()
                }),
            )
            .unwrap();
            s.commit(ROOT, &mut f);
            (s, f)
        }

        fn compose_all(s: &Scene, front: &Buf, scale: f64) -> Buf {
            let mut out = Buf::new(front.w, front.h, 0x5a5a_5a5a);
            let all = Rect::new(0, 0, front.w, front.h);
            let mut src = BTreeMap::new();
            src.insert(ROOT, front.clone());
            run(s, &mut out, scale, &[all], &src, &mut ShadowCache::new());
            out
        }

        /// Compose S's committed scene into OUT, a buffer whose first pixel is root px AT, within CLIPS (root px).
        fn compose_at(
            s: &Scene,
            front: &Buf,
            scale: f64,
            out: &mut Buf,
            at: (i32, i32),
            clips: &[Rect],
        ) {
            let mut t = TargetView {
                data: &mut out.px,
                stride: out.w as usize,
                rect: Rect::new(at.0, at.1, out.w, out.h),
                scale,
            };
            let views = [front.source(ROOT, scale)];
            compose_committed(&s.snapshot(), &mut Composer::new(), &mut t, clips, &views).unwrap();
        }

        fn area(rs: &[Rect]) -> u64 {
            rs.iter().map(Rect::area).sum()
        }

        fn output(s: &Scene, id: u64) -> OutputFrame {
            s.snapshot()
                .outputs
                .iter()
                .find(|o| o.layer == id)
                .cloned()
                .expect("an own output")
        }

        #[test]
        fn corner_shows_beneath() {
            for scale in [1.0, 2.0, 1.25] {
                let rect = Rect::new(10, 10, 40, 30);
                let (s, _) = carded(64, 48, scale, WIN, rect, 8.0);
                let front = Buf::new(64, 48, 0xffaa_bbcc);
                let out = compose_all(&s, &front, scale);
                // the very corner pixels lie outside the disc; the middle is the frame's
                for (x, y) in [(10, 10), (49, 10), (10, 39), (49, 39)] {
                    assert_eq!(out.at(x, y), BENEATH, "scale {scale}: corner {x},{y}");
                }
                assert_eq!(out.at(30, 25), 0xffaa_bbcc, "scale {scale}");
                // an arc pixel mixes the two
                let m = corner_coverage(8);
                // the top-left quadrant: the first 8 columns of the first 8 rows
                let i = (0..128)
                    .find(|&i| i % 16 < 8 && m[i] > 0 && m[i] < 255)
                    .unwrap();
                let (x, y) = (10 + (i % 16) as i32, 10 + (i / 16) as i32);
                let v = out.at(x, y);
                assert!(
                    v != BENEATH && v != 0xffaa_bbcc,
                    "scale {scale}: arc {x},{y} {v:08x}"
                );
            }
        }

        #[test]
        fn interior_is_copy() {
            let mut g = XorShift::new(11);
            let rect = Rect::new(6, 5, 50, 36);
            let (s, _) = carded(64, 48, 1.0, WIN, rect, 9.0);
            let front = Buf::random(&mut g, 64, 48);
            let out = compose_all(&s, &front, 1.0);
            let p = s.committed().iter().find(|p| p.id == WIN).cloned().unwrap();
            let (r, squares) = card_corners(&p).unwrap();
            assert_eq!(r, 9);
            let inside = corner_coverage(r);
            let n = 2 * r as i32;
            for y in 0..48 {
                for x in 0..64 {
                    let full = squares.iter().enumerate().all(|(i, q)| {
                        if !q.contains(x, y) {
                            return true;
                        }
                        let (mx, my) = ((i as i32 % 2) * r as i32, (i as i32 / 2) * r as i32);
                        inside[((my + y - q.y) * n + mx + x - q.x) as usize] == 255
                    });
                    if full {
                        assert_eq!(out.at(x, y), front.at(x, y), "pixel {x},{y}");
                    }
                }
            }
        }

        #[test]
        fn removed_restores_backing_pixels() {
            let mut g = XorShift::new(12);
            let rect = Rect::new(6, 5, 50, 36);
            let (mut s, mut f) = carded(64, 48, 2.0, WIN, rect, 10.0);
            let front = Buf::random(&mut g, 64, 48);
            let mut out = compose_all(&s, &front, 2.0);
            assert_ne!(first_difference(&out, &front), None);
            s.set_card(WIN, None).unwrap();
            let damage = s.commit(ROOT, &mut f).to_vec();
            compose_at(&s, &front, 2.0, &mut out, (0, 0), &damage);
            assert_eq!(first_difference(&out, &front), None, "{damage:?}");
        }

        #[test]
        fn chrome_without_card_draws_nothing() {
            let mut g = XorShift::new(13);
            let (mut s, mut f) = carded(64, 48, 1.0, WIN, Rect::new(6, 5, 50, 36), 10.0);
            s.set_card(WIN, None).unwrap();
            s.commit(ROOT, &mut f);
            let front = Buf::random(&mut g, 64, 48);
            assert_eq!(
                first_difference(&compose_all(&s, &front, 1.0), &front),
                None
            );
            // a card is for window layers only
            assert_eq!(s.set_card(ROOT, Some(Card::default())), Err(GLITNIR_E_ARG));
            assert_eq!(s.set_card(404, None), Err(GLITNIR_E_ARG));
        }

        /// WIN at R on an 800x600 root, its own output, with a card of radius 10 or none; committed.
        fn own(r: Rect, card: bool) -> (Scene, Frames) {
            let (mut s, mut f) = carded(800, 600, 1.0, WIN, r, 10.0);
            if !card {
                s.set_card(WIN, None).unwrap();
            }
            s.set_output(WIN, true).unwrap();
            s.commit(ROOT, &mut f);
            (s, f)
        }

        #[test]
        fn output_scroll_damages_corners() {
            let r = Rect::new(10, 20, 400, 300);
            let (mut s, mut f) = own(r, true);
            let exposed = Rect::new(10, 304, 400, 16);
            s.scroll(ROOT, f.entry(ROOT).or_default(), WIN, r, -16, exposed);
            s.commit(ROOT, &mut f);
            let o = output(&s, WIN);
            assert_eq!((o.scroll, o.full), (-16, false));
            let want = [
                Rect::new(0, 284, 400, 16),
                // the corners where they are now
                Rect::new(0, 0, 10, 10),
                Rect::new(390, 0, 10, 10),
                Rect::new(0, 290, 10, 10),
                Rect::new(390, 290, 10, 10),
                // and where the scroll took the old bottom ones (the top ones left the view)
                Rect::new(0, 274, 10, 10),
                Rect::new(390, 274, 10, 10),
            ];
            for w in want {
                assert!(covers(&o.damage, w), "{w:?} not in {:?}", o.damage);
            }
            assert!(area(&o.damage) <= 400 * 16 + 8 * 100, "{:?}", o.damage);
        }

        #[test]
        fn output_scroll_without_card_is_unchanged() {
            let r = Rect::new(10, 20, 400, 300);
            let (mut s, mut f) = own(r, false);
            s.scroll(
                ROOT,
                f.entry(ROOT).or_default(),
                WIN,
                r,
                -16,
                Rect::new(10, 304, 400, 16),
            );
            s.commit(ROOT, &mut f);
            let o = output(&s, WIN);
            assert!(covers(&o.damage, Rect::new(0, 284, 400, 16)));
            assert_eq!(area(&o.damage), 400 * 16, "{:?}", o.damage);
        }

        /// The look's card chrome at radius R: a soft shadow one px down.
        fn shadowed(r: f32) -> Chrome {
            Chrome {
                blur_px: 6.0,
                offset_y: 2.0,
                shadow_opacity: 0.45,
                shadow_argb: 0xff00_0000,
                ..radius(r)
            }
        }

        fn rich(ring_px: u32, keep_out: &[Rect]) -> Card {
            Card {
                beneath_argb: BENEATH,
                ring_argb: RING,
                ring_px,
                keep_out: keep_out.into(),
                ..Card::default()
            }
        }

        /// A W x H root at SCALE with window layer WIN at RECT, chrome C and card K, committed.
        fn with_card(
            w: i32,
            h: i32,
            scale: f64,
            rect: Rect,
            c: Chrome,
            k: Card,
        ) -> (Scene, Frames) {
            let (mut s, mut f) = carded(w, h, scale, WIN, rect, c.radius_px);
            s.set_chrome(WIN, Some(c)).unwrap();
            s.set_card(WIN, Some(k)).unwrap();
            s.commit(ROOT, &mut f);
            (s, f)
        }

        /// Whether every pixel of R in A equals B's.
        fn same_in(a: &Buf, b: &Buf, r: Rect) -> bool {
            (r.y..r.y + r.h).all(|y| (r.x..r.x + r.w).all(|x| a.at(x, y) == b.at(x, y)))
        }

        /// The pixels of RECT the corner masks leave whole: the window's own, where nothing of the card may go.
        fn interior(rect: Rect, r: u32) -> Vec<(i32, i32)> {
            let inside = corner_coverage(r);
            let n = 2 * r as i32;
            let ri = r as i32;
            let mut out = Vec::new();
            for y in rect.y..rect.y + rect.h {
                for x in rect.x..rect.x + rect.w {
                    // the pixel's place in the 2r×2r mask, when it lies in a corner square
                    let mx = if x < rect.x + ri {
                        Some(x - rect.x)
                    } else if x >= rect.x + rect.w - ri {
                        Some(x - (rect.x + rect.w - ri) + ri)
                    } else {
                        None
                    };
                    let my = if y < rect.y + ri {
                        Some(y - rect.y)
                    } else if y >= rect.y + rect.h - ri {
                        Some(y - (rect.y + rect.h - ri) + ri)
                    } else {
                        None
                    };
                    let whole = match (mx, my) {
                        (Some(mx), Some(my)) => inside[(my * n + mx) as usize] == 255,
                        _ => true,
                    };
                    if whole {
                        out.push((x, y));
                    }
                }
            }
            out
        }

        /// A card group (two windows in a column, one card around both): the divider between them is the group's
        /// fill, the outer corners are rounded, the windows' inner corners are not, and the rest is the frame's.
        #[test]
        fn group_fills_its_dividers_and_rounds_only_the_outside() {
            const FILL: u32 = 0xff20_3040;
            for scale in [1.0, 2.0] {
                let mut g = XorShift::new(31);
                // the windows (10,10,40,20) and (10,36,40,24); the divider (10,30,40,6) between them
                let rect = Rect::new(10, 10, 40, 50);
                let divider = Rect::new(10, 30, 40, 6);
                let front = Buf::random(&mut g, 64, 72);
                let card = Card {
                    fill_argb: FILL,
                    fill: vec![divider].into(),
                    ..rich(1, &[])
                };
                let (s, _) = with_card(64, 72, scale, rect, radius(8.0), card);
                let out = compose_all(&s, &front, scale);
                for y in divider.y..divider.y + divider.h {
                    for x in divider.x..divider.x + divider.w {
                        assert_eq!(out.at(x, y), FILL, "divider pixel {x},{y} at {scale}");
                    }
                }
                // the windows' inner corners, beside the divider, are their own pixels
                for (x, y) in [(10, 29), (49, 29), (10, 36), (49, 36)] {
                    assert_eq!(
                        out.at(x, y),
                        front.at(x, y),
                        "inner corner {x},{y} at {scale}"
                    );
                }
                // the group's outer corners show the colour beneath
                for (x, y) in [(10, 10), (49, 10), (10, 59), (49, 59)] {
                    assert_ne!(
                        out.at(x, y),
                        front.at(x, y),
                        "outer corner {x},{y} at {scale}"
                    );
                }
                // damage at the divider alone composes the fill
                let mut part = front.clone();
                compose_at(
                    &s,
                    &front,
                    scale,
                    &mut part,
                    (0, 0),
                    &[Rect::new(20, 31, 4, 2)],
                );
                assert_eq!(part.at(21, 32), FILL);
            }
        }

        /// The shadow and the ring go in the gap but never inside a window the card keeps out, and the frame's own
        /// pixels elsewhere stay.
        #[test]
        fn shadow_never_inside_keep_out() {
            let mut g = XorShift::new(21);
            let rect = Rect::new(10, 10, 40, 40);
            // the neighbour 6 px to the right, well inside the shadow's reach
            let next = Rect::new(56, 4, 40, 60);
            let front = Buf::random(&mut g, 120, 80);
            let (s, _) = with_card(120, 80, 1.0, rect, shadowed(8.0), rich(1, &[next]));
            let out = compose_all(&s, &front, 1.0);
            assert!(same_in(&out, &front, next), "the neighbour is untouched");
            // the gap between them is not
            let gap = Rect::new(50, 20, 6, 20);
            assert!(
                !same_in(&out, &front, gap),
                "the gap has the shadow and the ring"
            );
            // what keeps it out is the list: without it the shadow reaches the neighbour
            let (s, _) = with_card(120, 80, 1.0, rect, shadowed(8.0), rich(1, &[]));
            assert!(!same_in(&compose_all(&s, &front, 1.0), &front, next));
        }

        /// The ring lies just outside the rounded rectangle, the window's interior is the frame's, and a ring needs
        /// no shadow.
        #[test]
        fn ring_outside_never_touches_interior() {
            let mut g = XorShift::new(22);
            let rect = Rect::new(10, 10, 40, 30);
            let front = Buf::random(&mut g, 64, 56);
            let (s, _) = with_card(64, 56, 1.0, rect, radius(8.0), rich(2, &[]));
            let out = compose_all(&s, &front, 1.0);
            for (x, y) in interior(rect, 8) {
                assert_eq!(out.at(x, y), front.at(x, y), "interior pixel {x},{y}");
            }
            // two rows above the top edge and two columns beside the left: the ring
            for (x, y) in [(30, 9), (30, 8), (9, 25), (8, 25), (30, 40), (50, 25)] {
                assert_ne!(out.at(x, y), front.at(x, y), "ring pixel {x},{y}");
            }
            // past it, the frame
            for (x, y) in [(30, 7), (7, 25), (30, 42), (52, 25)] {
                assert_eq!(out.at(x, y), front.at(x, y), "gap pixel {x},{y}");
            }
            // the very corner of the window shows the colour beneath, the ring passing over its arc's outside
            assert_ne!(out.at(10, 10), front.at(10, 10));
        }

        /// A ring's change damages the ring and the corners, not the window.
        #[test]
        fn ring_change_damages_the_ring() {
            let mut g = XorShift::new(23);
            let rect = Rect::new(10, 10, 40, 30);
            let front = Buf::random(&mut g, 64, 56);
            let (mut s, mut f) = with_card(64, 56, 1.0, rect, radius(8.0), rich(2, &[]));
            let mut out = compose_all(&s, &front, 1.0);
            s.set_card(
                WIN,
                Some(Card {
                    ring_argb: 0xff80_9010,
                    ..rich(2, &[])
                }),
            )
            .unwrap();
            let damage = s.commit(ROOT, &mut f).to_vec();
            for r in [
                Rect::new(18, 8, 24, 2),
                Rect::new(18, 40, 24, 2),
                Rect::new(8, 18, 2, 14),
                Rect::new(50, 18, 2, 14),
                Rect::new(8, 8, 10, 10),
            ] {
                assert!(covers(&damage, r), "{r:?} not in {damage:?}");
            }
            assert!(!damage.iter().any(|d| d.contains(30, 25)), "{damage:?}");
            compose_at(&s, &front, 1.0, &mut out, (0, 0), &damage);
            assert_eq!(first_difference(&out, &compose_all(&s, &front, 1.0)), None);
        }

        /// Damage inside a card window away from its corners composes no pixel of the card; damage at a corner or in
        /// the gap does.
        #[test]
        fn typing_damage_inside_skips_corners() {
            let mut g = XorShift::new(24);
            let rect = Rect::new(10, 10, 60, 40);
            let front = Buf::random(&mut g, 96, 72);
            let (s, _) = with_card(96, 72, 1.0, rect, shadowed(8.0), rich(1, &[]));
            let mut out = compose_all(&s, &front, 1.0);
            let drawn = |out: &mut Buf, clip: Rect| {
                CARD_PX.with(|n| n.set(0));
                compose_at(&s, &front, 1.0, out, (0, 0), &[clip]);
                CARD_PX.with(|n| n.get())
            };
            // a line of text, and a cursor at the window's left edge, between the corners
            assert_eq!(drawn(&mut out, Rect::new(20, 30, 40, 12)), 0);
            assert_eq!(drawn(&mut out, Rect::new(10, 20, 2, 20)), 0);
            assert!(drawn(&mut out, Rect::new(10, 10, 4, 4)) > 0, "a corner");
            assert!(drawn(&mut out, Rect::new(30, 51, 4, 2)) > 0, "the gap");
            assert_eq!(first_difference(&out, &compose_all(&s, &front, 1.0)), None);
        }

        /// The corners, the shadow and the ring at scales 1, 2 and 1.25: the corner shows the gap, the gap has the
        /// chrome, the interior and the window kept out are the frame's.
        #[test]
        fn scales_1_2_and_1_25() {
            for scale in [1.0, 2.0, 1.25] {
                let rect = Rect::new(20, 20, 80, 60);
                let next = Rect::new(20, 92, 80, 20);
                let front = Buf::new(128, 120, 0xffaa_bbcc);
                let (s, _) = with_card(128, 120, scale, rect, shadowed(10.0), rich(2, &[next]));
                let out = compose_all(&s, &front, scale);
                assert_ne!(out.at(20, 20), 0xffaa_bbcc, "scale {scale}: corner");
                assert_ne!(out.at(60, 81), 0xffaa_bbcc, "scale {scale}: ring below");
                assert_ne!(out.at(60, 86), 0xffaa_bbcc, "scale {scale}: shadow below");
                assert!(
                    same_in(&out, &front, next),
                    "scale {scale}: the window kept out"
                );
                for (x, y) in interior(rect, 10) {
                    assert_eq!(out.at(x, y), 0xffaa_bbcc, "scale {scale}: interior {x},{y}");
                }
            }
        }

        /// A card's corner shows the gap as it is there, a neighbour's shadow in it: the neighbour keeps its shadow out
        /// of the card's window, but it lies under the corner's outside all the same.  Over a frame of the colour
        /// beneath, the corner's outermost pixel equals that pixel with no corner and the shadow let in.
        #[test]
        fn corner_shows_a_neighbours_shadow() {
            for scale in [1.0, 2.0, 1.25] {
                let left = Rect::new(10, 10, 40, 40);
                // 4 px of gap, well inside the neighbour's shadow
                let right = Rect::new(54, 10, 40, 40);
                let front = Buf::new(110, 64, BENEATH);
                let soft = Chrome {
                    blur_px: 12.0,
                    shadow_opacity: 0.6,
                    shadow_argb: 0xff00_0000,
                    ..radius(8.0)
                };
                let build = |r: f32, keep: &[Rect]| {
                    let (mut s, mut f) = carded(110, 64, scale, WIN, left, r);
                    s.upsert(&desc(NEXT, ROOT, 2, right, scale, true)).unwrap();
                    s.set_chrome(NEXT, Some(soft)).unwrap();
                    s.set_card(NEXT, Some(rich(0, keep))).unwrap();
                    s.commit(ROOT, &mut f);
                    s
                };
                let with = compose_all(&build(8.0, &[left]), &front, scale);
                let open = compose_all(&build(0.0, &[]), &front, scale);
                assert_ne!(
                    open.at(49, 30),
                    BENEATH,
                    "scale {scale}: the shadow reaches the window"
                );
                // the top-right and bottom-right corners' outermost pixels, beside the neighbour
                for (x, y) in [(49, 10), (49, 49)] {
                    assert_eq!(
                        with.at(x, y),
                        open.at(x, y),
                        "scale {scale}: corner {x},{y}"
                    );
                }
            }
        }

        /// A radius past half the window's shorter side draws as that half, the shadow too: a card 13 px high with a
        /// spread of 1 is the same with radius 16 as with 6.
        #[test]
        fn shadow_radius_clamps_as_the_corners() {
            let rect = Rect::new(20, 30, 60, 13);
            let front = Buf::new(100, 72, 0xffaa_bbcc);
            let look = |r: f32| Chrome {
                spread_px: 1.0,
                ..shadowed(r)
            };
            let (big, _) = with_card(100, 72, 1.0, rect, look(16.0), rich(1, &[]));
            let (half, _) = with_card(100, 72, 1.0, rect, look(6.0), rich(1, &[]));
            assert_eq!(
                first_difference(
                    &compose_all(&big, &front, 1.0),
                    &compose_all(&half, &front, 1.0)
                ),
                None
            );
        }

        /// Raising a card window damages where its card draws (the corners, the shadow and the ring), not its inside,
        /// which shows the frame's pixels in any order.
        #[test]
        fn raise_damages_the_card_alone() {
            let rect = Rect::new(10, 10, 60, 40);
            let (mut s, mut f) = with_card(96, 72, 1.0, rect, shadowed(8.0), rich(1, &[]));
            s.upsert(&desc(NEXT, ROOT, 2, Rect::new(0, 60, 96, 12), 1.0, true))
                .unwrap();
            s.commit(ROOT, &mut f);
            s.raise(WIN).unwrap();
            let damage = s.commit(ROOT, &mut f).to_vec();
            assert!(covers(&damage, Rect::new(10, 10, 8, 8)), "{damage:?}");
            assert!(!damage.iter().any(|d| d.contains(40, 30)), "{damage:?}");
        }

        /// The key property with cards: composing each commit's damage into a kept buffer equals composing the whole
        /// frame, for frames with several cards (corners, shadows, rings, each keeping the others out) as the frame
        /// draws, cards change, move, come and go, and the rectangles they keep out change.
        #[test]
        fn damage_only_equals_full_with_cards() {
            const W: i32 = 120;
            const H: i32 = 96;
            let all = Rect::new(0, 0, W, H);
            let mut shadows = ShadowCache::new();
            let rand_rect = |g: &mut XorShift| {
                Rect::new(
                    g.below(W as u32) as i32 - 8,
                    g.below(H as u32) as i32 - 8,
                    8 + g.below(56) as i32,
                    8 + g.below(48) as i32,
                )
            };
            let rand_card = |g: &mut XorShift, keep_out: Vec<Rect>| Card {
                beneath_argb: 0xff00_0000 | g.below(1 << 24),
                ring_argb: if g.below(3) == 0 { 0 } else { pixel(g) },
                ring_px: 1 + g.below(3),
                keep_out: keep_out.into(),
                ..Card::default()
            };
            let rand_chrome = |g: &mut XorShift| Chrome {
                border_px: 0.0,
                ..rand_chrome(g)
            };
            for seed in 1..=300u64 {
                let mut g = XorShift::new(seed);
                let scale = [1.0, 2.0, 1.25][g.below(3) as usize];
                let (mut s, mut f) = (Scene::default(), Frames::new());
                let mut src: BTreeMap<u64, Buf> = BTreeMap::new();
                s.upsert(&desc(ROOT, 0, 1, all, scale, true)).unwrap();
                src.insert(ROOT, Buf::random(&mut g, W, H));
                let mut rects: Vec<Rect> = (0..2 + g.below(3)).map(|_| rand_rect(&mut g)).collect();
                let ids: Vec<u64> = (0..rects.len() as u64).map(|i| 200 + i).collect();
                let others = |rects: &[Rect], i: usize| -> Vec<Rect> {
                    rects
                        .iter()
                        .enumerate()
                        .filter(|(j, _)| *j != i)
                        .map(|(_, r)| *r)
                        .collect()
                };
                for (i, id) in ids.iter().enumerate() {
                    s.upsert(&desc(*id, ROOT, 2, rects[i], scale, true))
                        .unwrap();
                    s.set_chrome(*id, Some(rand_chrome(&mut g))).unwrap();
                    let k = rand_card(&mut g, others(&rects, i));
                    s.set_card(*id, Some(k)).unwrap();
                }
                s.commit(ROOT, &mut f);
                let mut out = Buf::random(&mut g, W, H);
                run(&s, &mut out, scale, &[all], &src, &mut shadows);
                for round in 0..6 {
                    for _ in 0..1 + g.below(3) {
                        let i = g.below(ids.len() as u32) as usize;
                        match g.below(6) {
                            // the frame draws
                            0 | 1 => {
                                let r = rand_rect(&mut g);
                                src.get_mut(&ROOT).unwrap().paint(&mut g, r);
                                f.entry(ROOT).or_default().add(r);
                            }
                            // a card changes, goes or comes back
                            2 => {
                                let k =
                                    (g.below(4) != 0).then(|| rand_card(&mut g, others(&rects, i)));
                                s.set_card(ids[i], k).unwrap();
                            }
                            3 => {
                                s.set_chrome(ids[i], Some(rand_chrome(&mut g))).unwrap();
                            }
                            // a window moves: it and every card keeping it out change
                            4 => {
                                rects[i] = rand_rect(&mut g);
                                s.upsert(&desc(ids[i], ROOT, 2, rects[i], scale, true))
                                    .unwrap();
                                for (j, id) in ids.iter().enumerate() {
                                    let keep: Arc<[Rect]> = others(&rects, j).into();
                                    let k = s
                                        .committed()
                                        .iter()
                                        .find(|p| p.id == *id)
                                        .and_then(|p| p.card.clone())
                                        .map(|k| Card {
                                            keep_out: keep,
                                            ..k
                                        });
                                    if k.is_some() {
                                        s.set_card(*id, k).unwrap();
                                    }
                                }
                            }
                            _ => {
                                if g.below(2) == 0 {
                                    s.raise(ids[i]).unwrap();
                                } else {
                                    s.lower(ids[i]).unwrap();
                                }
                            }
                        }
                    }
                    let damage = s.commit(ROOT, &mut f).to_vec();
                    run(&s, &mut out, scale, &damage, &src, &mut shadows);
                    let mut want = Buf::random(&mut g, W, H);
                    run(&s, &mut want, scale, &[all], &src, &mut shadows);
                    if let Some((x, y, got, all_px)) = first_difference(&out, &want) {
                        panic!(
                            "seed {seed} round {round}: pixel {x},{y} is {got:08x}, composing everything gives \
                             {all_px:08x}; damage {damage:?}"
                        );
                    }
                }
            }
        }

        /// The key property: an own output's buffer, kept across commits as the ring keeps it (its rows move with
        /// each scroll, and then only its damage is composed), equals the frame composed whole, with a card over the
        /// output or over a taller rectangle (a mode line below it in the frame), as the card changes.
        #[test]
        fn scrolled_output_equals_frame_compose() {
            const W: i32 = 160;
            const H: i32 = 140;
            let all = Rect::new(0, 0, W, H);
            for seed in 1..=120u64 {
                let mut g = XorShift::new(seed);
                let scale = [1.0, 2.0, 1.25][g.below(3) as usize];
                // on the device grid of every scale, so the output is the window's rectangle
                let win = Rect::new(10 * (1 + g.below(3) as i32), 20, 100, 80);
                let outer = if g.below(2) == 0 {
                    win
                } else {
                    Rect::new(win.x, win.y, win.w, win.h + 20)
                };
                let (mut s, mut f) = (Scene::default(), Frames::new());
                s.upsert(&desc(ROOT, 0, 1, all, scale, true)).unwrap();
                s.upsert(&desc(WIN, ROOT, 2, win, scale, true)).unwrap();
                s.set_output(WIN, true).unwrap();
                s.upsert(&desc(CARD, ROOT, 2, outer, scale, true)).unwrap();
                // the card's corners, with a shadow and a ring over the colour beneath now and then
                let card = |g: &mut XorShift| Card {
                    beneath_argb: 0xff00_0000 | g.below(1 << 24),
                    ring_argb: if g.below(2) == 0 { 0 } else { RING },
                    ring_px: 1 + g.below(3),
                    keep_out: Arc::from([Rect::new(0, 0, W, 10)]),
                    ..Card::default()
                };
                let chrome = |g: &mut XorShift, r: u32| Chrome {
                    shadow_opacity: if g.below(2) == 0 { 0.0 } else { 0.45 },
                    blur_px: g.below(8) as f32,
                    offset_y: g.below(3) as f32,
                    shadow_argb: 0xff00_0000,
                    ..radius(r as f32)
                };
                let r = 2 + g.below(14);
                let c = chrome(&mut g, r);
                s.set_chrome(CARD, Some(c)).unwrap();
                s.set_card(CARD, Some(card(&mut g))).unwrap();
                let mut front = Buf::random(&mut g, W, H);
                s.commit(ROOT, &mut f);
                let o = output(&s, WIN);
                assert_eq!(o.rect, win, "seed {seed}");
                let mut layer = Buf::random(&mut g, win.w, win.h);
                compose_at(&s, &front, scale, &mut layer, (win.x, win.y), &[win]);
                for round in 0..12 {
                    for _ in 0..1 + g.below(3) {
                        match g.below(6) {
                            // a full-window scroll, either way
                            0..=2 => {
                                let dy =
                                    (1 + g.below(30) as i32) * if g.below(2) == 0 { 1 } else { -1 };
                                let exposed = front.scroll(&mut g, win, dy);
                                s.scroll(ROOT, f.entry(ROOT).or_default(), WIN, win, dy, exposed);
                            }
                            // the frame draws somewhere
                            3 => {
                                let r = Rect::new(
                                    g.below(W as u32) as i32 - 8,
                                    g.below(H as u32) as i32 - 8,
                                    1 + g.below(30) as i32,
                                    1 + g.below(20) as i32,
                                );
                                front.paint(&mut g, r);
                                f.entry(ROOT).or_default().add(r);
                            }
                            // the card changes colour or radius, goes, or comes back
                            4 => {
                                let c = (g.below(4) != 0).then(|| card(&mut g));
                                s.set_card(CARD, c).unwrap();
                            }
                            _ => {
                                let r = g.below(16);
                                let c = chrome(&mut g, r);
                                s.set_chrome(CARD, Some(c)).unwrap();
                            }
                        }
                    }
                    s.commit(ROOT, &mut f);
                    let o = output(&s, WIN);
                    if o.full {
                        layer = Buf::random(&mut g, win.w, win.h);
                    } else if o.scroll != 0 {
                        // the rows keep their place in the ring: in the view they move by the scroll
                        let old = layer.clone();
                        for y in 0..win.h {
                            for x in 0..win.w {
                                let from = y - o.scroll;
                                let v = if (0..win.h).contains(&from) {
                                    old.at(x, from)
                                } else {
                                    pixel(&mut g)
                                };
                                layer.set(x, y, v);
                            }
                        }
                    }
                    let clips: Vec<Rect> =
                        o.damage.iter().map(|r| r.translate(win.x, win.y)).collect();
                    compose_at(&s, &front, scale, &mut layer, (win.x, win.y), &clips);
                    let want = compose_all(&s, &front, scale);
                    for y in 0..win.h {
                        for x in 0..win.w {
                            let (got, w) = (layer.at(x, y), want.at(win.x + x, win.y + y));
                            assert_eq!(
                                got, w,
                                "seed {seed} round {round} scale {scale}: output pixel {x},{y}; damage {:?}, \
                                 scroll {}",
                                o.damage, o.scroll
                            );
                        }
                    }
                }
            }
        }
    }
}

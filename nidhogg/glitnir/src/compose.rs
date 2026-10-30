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
//!   them, clipped to the label's clip, in the layer's place in the paint order.
//!
//! A layer without a source is not drawn, so the target keeps its pixels there, as it does where no layer lies. The
//! shadow beneath a layer's copied part is skipped. A pixel's value depends only on the scene and the sources, never
//! on how the rectangles cut it, so composing the damage equals composing everything wherever the damage covers what
//! changed.
//!
//! [`compose_committed`] composes a commit's snapshot with a caller-owned [`Composer`] and takes no lock, so the
//! host's thread can go on drawing while another thread composes.

use crate::committed::Committed;
use crate::decor::{Label, Sprites};
use crate::ffi::{GLITNIR_E_ARG, GLITNIR_E_STATE};
use crate::geom::Rect;
use crate::kernels::{
    Kernels, Mask, Pixels, PixelsMut, RingCoverage, clamp_radius, corner_coverage, ring_coverage,
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
fn whole(v: f32) -> u32 {
    if v.is_finite() && v > 0.0 {
        v.round() as u32
    } else {
        0
    }
}

/// A premultiplied colour times an opacity (pixman's x·a/255, rounded).
fn with_opacity(argb: u32, opacity: f32) -> u32 {
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

    /// Draw the layer within CLIP (inside the target).
    fn draw(&self, k: &Kernels, t: &mut TargetView, src: &SourceView, clip: Rect) {
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
            if !q.is_empty() {
                let (w, h) = (q.w as usize, q.h as usize);
                k.copy(&mut t.at(q), &src.at(q, self.origin), w, h);
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
            k.mask_over(&mut dst, &src.at(q, self.origin), &mask, w, h, 255);
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
    if sources.iter().any(|s| s.scale != t.scale) {
        return Err(GLITNIR_E_STATE);
    }
    let pieces = disjoint(clips, t.rect);
    let mut drawn = 0u32;
    for p in scene {
        if p.kind == Kind::Window {
            for l in labels.iter().filter(|l| l.layer == p.id) {
                draw_label(k, t, &pieces, l, sprites);
            }
            continue;
        }
        if !pieces.iter().any(|c| !c.intersect(&p.bounds).is_empty()) {
            continue;
        }
        let Some(src) = sources.iter().find(|s| s.layer == p.id) else {
            continue;
        };
        let plan = Plan::new(p, src, t.scale, shadows);
        for c in &pieces {
            plan.draw(k, t, src, *c);
        }
        drawn = drawn.saturating_add(1);
    }
    Ok(drawn)
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
    let start = Instant::now();
    #[cfg(test)]
    if let Some(g) = &k.gate {
        let _ = g.recv_timeout(std::time::Duration::from_secs(5));
    }
    let shadows = k.shadows.get_or_insert_with(ShadowCache::new);
    let layers = compose_with(
        crate::kernels::kernels(),
        &c.layers,
        &c.labels,
        t,
        clips,
        sources,
        shadows,
        &c.sprites,
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
}

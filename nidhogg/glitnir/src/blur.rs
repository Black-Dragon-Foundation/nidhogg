//! Backdrop blur: what lies beneath a translucent layer, blurred, and kept at ¼ resolution.
//!
//! The source is averaged in 4×4 blocks on a grid fixed to its origin, then three running-sum box passes per axis
//! approximate the Gaussian, at a cost that does not depend on σ. The image stays at ¼ resolution and the composite
//! samples it bilinearly ([`Backdrop::sample_row`]), so there is no upsample pass and the cache is 16× smaller.
//!
//! A cell of the image is a function of the source pixels in [`Backdrop`]'s `area` alone (past the source, and past the
//! image, the edge repeats), so recomputing some rows gives what recomputing everything gives.

use crate::geom::Rect;
use crate::kernels::{Kernels, MAX_RADIUS, Px, bilerp_px, kernels};

/// A larger σ blurs as this one: the widest box the passes have.
const MAX_SIGMA: f32 = 4.0 * MAX_RADIUS as f32;

/// How far the blur reaches, in device pixels, for a σ in device pixels: `ceil(3σ)`, a multiple of 4.
pub fn margin(sigma_px: f32) -> i32 {
    ((3.0 * sigma_px.min(MAX_SIGMA)).ceil() as i32 + 3) & !3
}

/// The three box radii, in cells, that come nearest σ. Variances add: the 4×4 average and the bilinear lookup bring ¼
/// cell² between them, a box of width w (w² - 1) / 12, so a σ below 2 device pixels blurs as 2. Odd widths keep the
/// result centred, which steps the radii: σ between them gets the nearest.
fn radii(sigma_px: f32) -> [usize; 3] {
    let cells = (sigma_px.min(MAX_SIGMA) / 4.0) as f64;
    let want = (cells * cells - 0.25).max(0.0);
    let r = ((4.0 * want + 1.0).sqrt() as usize - 1) / 2;
    let variance = |r: usize| (r * (r + 1)) as f64 / 3.0;
    let off = |wider: usize| {
        let got = (3 - wider) as f64 * variance(r) + wider as f64 * variance(r + 1);
        (got - want).abs()
    };
    let wider = (0..=3).min_by(|a, b| off(*a).total_cmp(&off(*b)));
    let wider = wider.unwrap_or(0);
    [
        r + usize::from(wider > 2),
        r + usize::from(wider > 1),
        r + usize::from(wider > 0),
    ]
}

/// The source pixels `update` reads.
struct Source<'a> {
    data: &'a [u32],
    stride: usize,
    size: (i32, i32),
}

impl Source<'_> {
    /// `count` pixels of row `y`, from `x`; past the bottom the last row repeats.
    fn row(&self, y: i32, x: i32, count: usize) -> &[u32] {
        let y = y.min(self.size.1 - 1) as usize;
        &self.data[y * self.stride + x as usize..][..count]
    }
}

/// What the image was made from; a change of any of these recomputes it all.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Key {
    sigma: u32,
    scale: u64,
    region: Rect,
    src_size: (i32, i32),
}

/// Buffers `update` reuses between calls.
#[derive(Debug, Default)]
struct Scratch {
    line: [Vec<Px>; 2],
    plane: [Vec<Px>; 2],
    sums: Vec<[u16; 4]>,
    /// The rows just recomputed.
    fresh: Vec<Px>,
}

/// The blurred image of what lies beneath one layer.
#[derive(Debug, Default)]
pub struct Backdrop {
    key: Option<Key>,
    /// The source pixels the image holds: the region and its margin within the source, out to the grid of 4.
    area: Rect,
    /// Premultiplied, `area.w / 4` cells by `area.h / 4`.
    image: Vec<Px>,
    radii: [usize; 3],
    /// Device pixels the last `update` recomputed.
    recomputed: u64,
    scratch: Scratch,
}

impl Backdrop {
    pub fn new() -> Backdrop {
        Backdrop::default()
    }

    /// Bring the image up to date with `src`, of `src_size` device pixels. `region` is the layer's area and `dirty` the
    /// source's changes since the last call, both in source coordinates; `sigma_px` is in device pixels, the logical
    /// σ times `scale`. Rows that `dirty` touches are recomputed, with the reach of the blur on both sides; a change
    /// of the key, `region` or `scale` recomputes everything. Returns the part of `region` whose samples changed.
    /// An empty `region`, one that misses the source, or a σ that is not positive leaves nothing cached.
    #[allow(clippy::too_many_arguments)]
    pub fn update(
        &mut self,
        src: &[u32],
        src_stride: usize,
        src_size: (i32, i32),
        region: Rect,
        sigma_px: f32,
        scale: f64,
        dirty: &[Rect],
    ) -> Option<Rect> {
        let source = Source {
            data: src,
            stride: src_stride,
            size: src_size,
        };
        self.update_on(kernels(), &source, region, sigma_px, scale, dirty)
    }

    fn update_on(
        &mut self,
        k: &Kernels,
        src: &Source,
        region: Rect,
        sigma_px: f32,
        scale: f64,
        dirty: &[Rect],
    ) -> Option<Rect> {
        self.recomputed = 0;
        let m = margin(sigma_px);
        let grown = Rect::new(
            region.x - m,
            region.y - m,
            region.w + 2 * m,
            region.h + 2 * m,
        );
        let around = grown.intersect(&Rect::new(0, 0, src.size.0, src.size.1));
        if region.is_empty() || around.is_empty() || sigma_px.is_nan() || sigma_px <= 0.0 {
            self.invalidate();
            return None;
        }
        let key = Key {
            sigma: sigma_px.to_bits(),
            scale: scale.to_bits(),
            region,
            src_size: src.size,
        };
        if self.key != Some(key) {
            self.key = Some(key);
            self.radii = radii(sigma_px);
            let (x0, y0) = (around.x & !3, around.y & !3);
            let x1 = (around.x + around.w + 3) & !3;
            let y1 = (around.y + around.h + 3) & !3;
            self.area = Rect::new(x0, y0, x1 - x0, y1 - y0);
            self.recompute(k, src, 0, self.cells().1);
            std::mem::swap(&mut self.image, &mut self.scratch.fresh);
            return Some(region);
        }
        let mut changed = Rect::default();
        for (a, b) in self.bands(dirty) {
            self.recompute(k, src, a, b);
            self.store(a, b, &mut changed);
        }
        let changed = self.beneath(changed).intersect(&region);
        (!changed.is_empty()).then_some(changed)
    }

    /// Cells across, and down.
    fn cells(&self) -> (usize, usize) {
        (self.area.w as usize / 4, self.area.h as usize / 4)
    }

    /// The rows of cells that `dirty` reaches, sorted and apart.
    fn bands(&self, dirty: &[Rect]) -> Vec<(usize, usize)> {
        let reach = self.radii.iter().sum::<usize>();
        let h = self.cells().1;
        let mut bands: Vec<(usize, usize)> = Vec::new();
        for d in dirty {
            let d = d.intersect(&self.area);
            if !d.is_empty() {
                let top = (d.y - self.area.y) as usize / 4;
                let bottom = (d.y + d.h - self.area.y + 3) as usize / 4;
                bands.push((top.saturating_sub(reach), (bottom + reach).min(h)));
            }
        }
        bands.sort_unstable();
        bands.dedup_by(|next, last| {
            let touch = next.0 <= last.1;
            if touch {
                last.1 = last.1.max(next.1);
            }
            touch
        });
        bands
    }

    /// Rows `a..b` of the image, into `scratch.fresh`.
    fn recompute(&mut self, k: &Kernels, src: &Source, a: usize, b: usize) {
        let (w, h) = self.cells();
        let [r0, r1, r2] = self.radii;
        let reach = r0 + r1 + r2;
        let rows = b - a + 2 * reach;
        self.recomputed += ((b - a) * w * 16) as u64;

        let Scratch {
            line: [line0, line1],
            plane: [plane0, plane1],
            sums,
            fresh,
        } = &mut self.scratch;
        line0.resize(w + 2 * reach, [0; 4]);
        line1.resize(w + 2 * reach, [0; 4]);
        plane0.resize(rows * w, [0; 4]);
        plane1.resize(rows * w, [0; 4]);
        fresh.resize((b - a) * w, [0; 4]);

        // across: the rows that the passes down read, as far as the image goes
        let top = a.saturating_sub(reach);
        let bottom = (b + reach).min(h);
        let above = reach - (a - top);
        let long = w + 2 * (r1 + r2);
        let short = w + 2 * r2;
        for (row, d) in (top..bottom).zip(plane0[above * w..].chunks_exact_mut(w)) {
            down_row(k, src, self.area, row, &mut line0[reach..reach + w]);
            let (first, last) = (line0[reach], line0[reach + w - 1]);
            line0[..reach].fill(first);
            line0[reach + w..].fill(last);
            k.box_h(&mut line1[..long], &line0[..long + 2 * r0], r0);
            k.box_h(&mut line0[..short], &line1[..long], r1);
            k.box_h(d, &line0[..short], r2);
        }
        // past the image the edge rows repeat
        let filled = above + bottom - top;
        for y in 0..above {
            plane0.copy_within(above * w..(above + 1) * w, y * w);
        }
        for y in filled..rows {
            plane0.copy_within((filled - 1) * w..filled * w, y * w);
        }
        // down
        let tall = (rows - 2 * r0) * w;
        let low = tall - 2 * r1 * w;
        k.box_v(&mut plane1[..tall], &plane0[..rows * w], w, r0, sums);
        k.box_v(&mut plane0[..low], &plane1[..tall], w, r1, sums);
        k.box_v(&mut fresh[..(b - a) * w], &plane0[..low], w, r2, sums);
    }

    /// `scratch.fresh` becomes rows `a..b` of the image; `changed` grows by the cells that differ.
    fn store(&mut self, a: usize, b: usize, changed: &mut Rect) {
        let w = self.cells().0;
        let rows = self.scratch.fresh.chunks_exact(w);
        let old_rows = self.image[a * w..b * w].chunks_exact_mut(w);
        for (y, (new, old)) in (a..).zip(rows.zip(old_rows)) {
            if new == old {
                continue;
            }
            let same = |x: &usize| new[*x] == old[*x];
            let first = (0..w).find(|x| !same(x)).unwrap_or(0);
            let last = (0..w).rfind(|x| !same(x)).unwrap_or(0);
            let cells = Rect::new(first as i32, y as i32, (last + 1 - first) as i32, 1);
            *changed = changed.bounds(&cells);
            old.copy_from_slice(new);
        }
    }

    /// The source pixels that sample the given cells: each cell reaches its neighbours' centres. Cells on the image's
    /// edge also serve everything past it.
    fn beneath(&self, cells: Rect) -> Rect {
        if cells.is_empty() {
            return cells;
        }
        let (a, (w, h)) = (self.area, self.cells());
        let span = |first: i32, count: i32, origin: i32, all: usize| {
            let end = first + count;
            let start = if first == 0 {
                i32::MIN / 2
            } else {
                origin + 4 * first - 2
            };
            let stop = if end == all as i32 {
                i32::MAX / 2
            } else {
                origin + 4 * end + 2
            };
            (start, stop)
        };
        let (x0, x1) = span(cells.x, cells.w, a.x, w);
        let (y0, y1) = span(cells.y, cells.h, a.y, h);
        Rect::new(x0, y0, x1 - x0, y1 - y0)
    }

    /// Row `y` of the blurred source from `x0`, in source coordinates, into `out`: bilinear between the cells' centres,
    /// clear when nothing is cached.
    pub fn sample_row(&self, y: i32, x0: i32, out: &mut [u32]) {
        self.sample_on(kernels(), y, x0, out)
    }

    fn sample_on(&self, k: &Kernels, y: i32, x0: i32, out: &mut [u32]) {
        if self.image.is_empty() {
            out.fill(0);
            return;
        }
        let (w, h) = self.cells();
        // a pixel `p` from the image's edge lies 2p - 3 eighths of a cell past the centre of cell 0
        let cell = |p: i32| ((2 * p - 3) >> 3, ((2 * p - 3) & 7) as u16);
        let (row, fy) = cell(y - self.area.y);
        let at = |i: i32, n: usize| i.clamp(0, n as i32 - 1) as usize;
        let t = &self.image[at(row, h) * w..][..w];
        let b = &self.image[at(row + 1, h) * w..][..w];
        let one = |p: i32| {
            let (i, fx) = cell(p);
            let (i0, i1) = (at(i, w), at(i + 1, w));
            bilerp_px(&t[i0], &t[i1], &b[i0], &b[i1], fx, fy)
        };

        // whole pairs of cells go to the kernel: they start 2 pixels past a cell's edge, inside the image
        let p0 = x0 - self.area.x;
        let n = out.len() as i32;
        let first = (2 - p0).max((2 - p0).rem_euclid(4)).min(n);
        let start = (p0 + first - 2).div_euclid(4);
        let pairs = (w as i32 - 1 - start).min((n - first) / 4).max(0);
        let last = first + 4 * pairs;
        for (i, o) in out[..first as usize].iter_mut().enumerate() {
            *o = one(p0 + i as i32);
        }
        if pairs > 0 {
            let cells = start as usize..(start + pairs + 1) as usize;
            let spans = &mut out[first as usize..last as usize];
            k.bilerp(spans, &t[cells.clone()], &b[cells], fy);
        }
        for (i, o) in out[last as usize..].iter_mut().enumerate() {
            *o = one(p0 + last + i as i32);
        }
    }

    /// Forget the image: the next `update` recomputes everything.
    pub fn invalidate(&mut self) {
        self.key = None;
        self.image.clear();
    }

    /// The bytes the cache holds, scratch included.
    pub fn bytes(&self) -> usize {
        let s = &self.scratch;
        let px = [
            &self.image,
            &s.line[0],
            &s.line[1],
            &s.plane[0],
            &s.plane[1],
            &s.fresh,
        ];
        let px: usize = px.iter().map(|v| v.capacity()).sum();
        px * size_of::<Px>() + s.sums.capacity() * size_of::<[u16; 4]>()
    }

    /// Device pixels the last `update` recomputed: 0 when it kept the cache.
    pub fn recomputed_px(&self) -> u64 {
        self.recomputed
    }

    /// The region the image was made for, in source coordinates; `None` while nothing is cached.
    pub fn region(&self) -> Option<Rect> {
        self.key.map(|k| k.region)
    }
}

/// One row of cells across `area`, from the source. The last cell of a row may hang over the source's right edge, whose
/// pixels then repeat. `area`'s corner lies inside the source.
fn down_row(k: &Kernels, src: &Source, area: Rect, row: usize, d: &mut [Px]) {
    let have = area.w.min(src.size.0 - area.x) as usize;
    let whole = have / 4;
    let y = area.y + 4 * row as i32;
    let rows: [&[u32]; 4] = std::array::from_fn(|i| src.row(y + i as i32, area.x, have));
    k.down4(&mut d[..whole], rows);
    if whole < d.len() {
        let tails: [[u32; 4]; 4] = std::array::from_fn(|i| {
            let mut tail = [rows[i][have - 1]; 4];
            tail[..have % 4].copy_from_slice(&rows[i][whole * 4..]);
            tail
        });
        let tails = tails.each_ref().map(|t| &t[..]);
        k.down4(&mut d[whole..], tails);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kernels::{avx2, baseline};
    use crate::rng::XorShift;

    const SIZE: (i32, i32) = (256, 200);
    const STRIDE: usize = 256;
    const INSIDE: Rect = Rect::new(40, 30, 176, 140);

    /// The largest error against the reference over every fixture, σ and region, in 1/255: 5.29 measured, plus 1. The
    /// ceiling holds from σ 10 up. At σ 5, ¼ resolution is too coarse for it: 10.69 on hard edges, whose slope is then
    /// 51/255 per pixel.
    const TOLERANCE: f64 = 6.29;

    fn grey(v: u32) -> u32 {
        0xff00_0000 | (v * 0x01_0101)
    }

    /// Dark strokes on a light page, in lines, as a window of text looks.
    fn text() -> Vec<u32> {
        let mut r = XorShift::new(11);
        let mut px = vec![grey(0xf0); STRIDE * SIZE.1 as usize];
        for line in (6..SIZE.1 - 12).step_by(16) {
            let mut x = 4;
            while x < SIZE.0 - 8 {
                let (w, h) = (1 + r.below(2) as i32, 3 + r.below(8) as i32);
                for dy in 0..h {
                    for dx in 0..w {
                        px[(line + dy) as usize * STRIDE + (x + dx) as usize] = grey(0x20);
                    }
                }
                x += 2 + r.below(6) as i32;
            }
        }
        px
    }

    /// Hard edges between black, white and clear cells.
    fn edges() -> Vec<u32> {
        let mut px = Vec::new();
        for y in 0..SIZE.1 {
            for x in 0..SIZE.0 {
                px.push(match (x / 37 + y / 29) % 3 {
                    0 => 0xff00_0000,
                    1 => 0xffff_ffff,
                    _ => 0,
                });
            }
        }
        px
    }

    fn flat() -> Vec<u32> {
        vec![0x8040_6020; STRIDE * SIZE.1 as usize]
    }

    fn gradient() -> Vec<u32> {
        let mut px = Vec::new();
        for y in 0..SIZE.1 as u32 {
            for x in 0..SIZE.0 as u32 {
                let (r, g) = (x, y * 255 / 199);
                px.push(0xff00_0000 | (r << 16) | (g << 8) | ((r + g) / 2));
            }
        }
        px
    }

    fn noise(seed: u64) -> Vec<u32> {
        let mut r = XorShift::new(seed);
        (0..STRIDE * SIZE.1 as usize)
            .map(|_| 0xff00_0000 | (r.next_u64() >> 32) as u32 & 0xff_ffff)
            .collect()
    }

    /// The exact Gaussian at full resolution, past the edges the edge pixels repeat.
    fn gaussian(src: &[u32], sigma: f64) -> Vec<[f64; 4]> {
        let reach = (4.0 * sigma).ceil() as i32;
        let weights: Vec<f64> = (-reach..=reach)
            .map(|i| (-0.5 * (i as f64 / sigma).powi(2)).exp())
            .collect();
        let total: f64 = weights.iter().sum();
        let weights: Vec<f64> = weights.iter().map(|w| w / total).collect();
        let (w, h) = SIZE;
        let at = |x: i32, y: i32| (y.clamp(0, h - 1) * w + x.clamp(0, w - 1)) as usize;
        let mut across = vec![[0.0; 4]; (w * h) as usize];
        for y in 0..h {
            for x in 0..w {
                let o = &mut across[at(x, y)];
                for (i, k) in (-reach..=reach).zip(&weights) {
                    let p = src[at(x + i, y)].to_ne_bytes();
                    for c in 0..4 {
                        o[c] += p[c] as f64 * k;
                    }
                }
            }
        }
        let mut out = vec![[0.0; 4]; (w * h) as usize];
        for y in 0..h {
            for x in 0..w {
                let o = &mut out[at(x, y)];
                for (i, k) in (-reach..=reach).zip(&weights) {
                    let p = &across[at(x, y + i)];
                    for c in 0..4 {
                        o[c] += p[c] * k;
                    }
                }
            }
        }
        out
    }

    /// Every pixel of `region`, sampled.
    fn sampled(b: &Backdrop, region: Rect) -> Vec<u32> {
        let mut out = vec![0; region.area() as usize];
        let rows = out.chunks_exact_mut(region.w as usize);
        for (y, row) in (region.y..).zip(rows) {
            b.sample_row(y, region.x, row);
        }
        out
    }

    fn update(
        b: &mut Backdrop,
        src: &[u32],
        region: Rect,
        sigma: f32,
        dirty: &[Rect],
    ) -> Option<Rect> {
        b.update(src, STRIDE, SIZE, region, sigma, 2.0, dirty)
    }

    fn full(src: &[u32], region: Rect, sigma: f32) -> Backdrop {
        let mut b = Backdrop::new();
        assert_eq!(update(&mut b, src, region, sigma, &[]), Some(region));
        b
    }

    fn paint(src: &mut [u32], rect: Rect, value: u32) {
        for y in rect.y..rect.y + rect.h {
            src[y as usize * STRIDE..][rect.x as usize..][..rect.w as usize].fill(value);
        }
    }

    #[test]
    fn margin_is_three_sigma_on_the_grid() {
        assert_eq!(margin(0.0), 0);
        assert_eq!(margin(0.25), 4);
        assert_eq!(margin(10.0), 32);
        assert_eq!(margin(10.25), 32);
        assert_eq!(margin(20.0), 60);
    }

    #[test]
    fn matches_gaussian_within_tolerance() {
        const { assert!(TOLERANCE <= 8.0, "the design's ceiling") };
        let fixtures: [(&str, Vec<u32>); 5] = [
            ("text", text()),
            ("edges", edges()),
            ("flat", flat()),
            ("gradient", gradient()),
            ("noise", noise(7)),
        ];
        let mut report = String::new();
        let mut worst: f64 = 0.0;
        for (name, src) in &fixtures {
            for sigma in [10.0_f32, 20.0] {
                let want = gaussian(src, sigma as f64);
                // at the source's edge the reference repeats one pixel where the image repeats a cell's mean, which on
                // noise is no measure of the blur
                let regions = [
                    INSIDE,
                    Rect::new(0, 0, 120, 100),
                    Rect::new(150, 110, 106, 90),
                ];
                let regions = &regions[..if *name == "noise" { 1 } else { 3 }];
                for &region in regions {
                    let got = sampled(&full(src, region, sigma), region);
                    let mut max: f64 = 0.0;
                    for (i, px) in got.iter().enumerate() {
                        let x = region.x as usize + i % region.w as usize;
                        let y = region.y as usize + i / region.w as usize;
                        let exact = &want[y * STRIDE + x];
                        for (a, b) in px.to_ne_bytes().iter().zip(exact) {
                            max = max.max((*a as f64 - b).abs());
                        }
                    }
                    report += &format!("{name} σ{sigma} {region:?}: {max:.2}\n");
                    if *name == "flat" {
                        assert!(max < 1e-6, "a flat colour stays exact: {max}");
                    }
                    worst = worst.max(max);
                }
            }
        }
        assert!(worst <= TOLERANCE, "worst {worst:.2}/255\n{report}");
    }

    #[test]
    fn avx2_equals_baseline() {
        let Some(wide) = avx2() else {
            eprintln!("no AVX2 build on this machine");
            return;
        };
        for (sigma, seed) in [(3.0, 21), (10.0, 22), (20.5, 23), (64.0, 24)] {
            let mut src = noise(seed);
            let region = Rect::new(13, 9, 201, 170);
            let (mut a, mut b) = (Backdrop::new(), Backdrop::new());
            for dirty in [&[][..], &[Rect::new(30, 60, 90, 11)][..]] {
                paint(
                    &mut src,
                    Rect::new(30, 60, 90, 11),
                    0xff12_3456 + seed as u32,
                );
                let s = Source {
                    data: &src,
                    stride: STRIDE,
                    size: SIZE,
                };
                let ra = a.update_on(baseline(), &s, region, sigma, 1.0, dirty);
                let rb = b.update_on(wide, &s, region, sigma, 1.0, dirty);
                assert_eq!(ra, rb);
                assert!(a.image == b.image, "σ {sigma}");
                for y in region.y..region.y + region.h {
                    for x0 in [region.x, region.x + 1, region.x + 3] {
                        let (mut oa, mut ob) = ([0; 150], [0; 150]);
                        a.sample_on(baseline(), y, x0, &mut oa);
                        b.sample_on(wide, y, x0, &mut ob);
                        assert_eq!(oa, ob, "σ {sigma} row {y}");
                    }
                }
            }
        }
    }

    #[test]
    fn dirty_outside_margin_keeps_cache() {
        let mut src = noise(1);
        let region = Rect::new(100, 80, 40, 30);
        let mut b = full(&src, region, 4.0);
        let m = margin(4.0);
        assert_eq!(m, 12);
        let image = b.image.clone();
        // past the grid cell that holds the margin's edge, on all four sides
        let outside = [
            Rect::new(0, 0, 256, 80 - m - 4),
            Rect::new(0, 80 + 30 + m + 4, 256, 20),
            Rect::new(0, 60, 100 - m - 4, 60),
            Rect::new(100 + 40 + m + 4, 60, 60, 60),
        ];
        for rect in outside {
            paint(&mut src, rect, 0xff00_ff00);
        }
        assert_eq!(update(&mut b, &src, region, 4.0, &outside), None);
        assert_eq!(b.recomputed_px(), 0);
        assert!(b.image == image, "no pixel written");
    }

    #[test]
    fn dirty_inside_recomputes_band_only() {
        let mut src = noise(2);
        let mut b = full(&src, INSIDE, 10.0);
        let whole = b.recomputed_px();
        let before = sampled(&b, INSIDE);
        let strip = Rect::new(90, 100, 40, 8);
        paint(&mut src, strip, 0xff00_0000);
        let changed = update(&mut b, &src, INSIDE, 10.0, &[strip]).expect("a change beneath");
        let m = margin(10.0);
        assert!(b.recomputed_px() > 0);
        assert!(
            b.recomputed_px() <= (strip.h + 8 + 2 * m) as u64 * (INSIDE.w + 2 * m + 8) as u64,
            "{} of {whole}",
            b.recomputed_px()
        );
        let after = sampled(&b, INSIDE);
        assert_ne!(before, after);
        for (i, (x, y)) in before.iter().zip(&after).enumerate() {
            let px = INSIDE.x + (i % INSIDE.w as usize) as i32;
            let py = INSIDE.y + (i / INSIDE.w as usize) as i32;
            assert!(
                x == y || changed.contains(px, py),
                "({px},{py}) outside {changed:?}"
            );
        }
        assert!(changed.area() < INSIDE.area());
        assert!(b.image == full(&src, INSIDE, 10.0).image);
    }

    #[test]
    fn random_damage_equals_full_recompute() {
        let mut r = XorShift::new(0x0b1a_d3d6);
        for _ in 0..40 {
            let sigma = [3.0, 8.0, 10.0, 17.5][r.below(4) as usize];
            let (x, y) = (r.below(200) as i32, r.below(150) as i32);
            let region = Rect::new(
                x,
                y,
                1 + r.below((SIZE.0 - x) as u32) as i32,
                1 + r.below((SIZE.1 - y) as u32) as i32,
            );
            let mut src = noise(r.next_u64());
            let mut b = full(&src, region, sigma);
            for _ in 0..4 {
                let dirty: Vec<Rect> = (0..1 + r.below(3))
                    .map(|_| {
                        let (x, y) = (r.below(250) as i32, r.below(190) as i32);
                        let w = 1 + r.below((SIZE.0 - x).min(40) as u32) as i32;
                        Rect::new(x, y, w, 1 + r.below((SIZE.1 - y).min(30) as u32) as i32)
                    })
                    .collect();
                for rect in &dirty {
                    paint(&mut src, *rect, 0xff00_0000 | (r.next_u64() >> 40) as u32);
                }
                let before = sampled(&b, region);
                let changed = update(&mut b, &src, region, sigma, &dirty);
                assert!(
                    b.image == full(&src, region, sigma).image,
                    "{region:?} σ {sigma} {dirty:?}"
                );
                for (i, (p, q)) in before.iter().zip(sampled(&b, region)).enumerate() {
                    let px = region.x + (i % region.w as usize) as i32;
                    let py = region.y + (i / region.w as usize) as i32;
                    assert!(
                        *p == q || changed.is_some_and(|c| c.contains(px, py)),
                        "({px},{py}) {changed:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn subpixel_sigma_is_in_key() {
        let src = noise(3);
        let mut b = full(&src, INSIDE, 10.0);
        let whole = b.recomputed_px();
        assert_eq!(update(&mut b, &src, INSIDE, 10.0, &[]), None);
        assert_eq!(b.recomputed_px(), 0);
        assert_eq!(update(&mut b, &src, INSIDE, 10.25, &[]), Some(INSIDE));
        assert_eq!(b.recomputed_px(), whole);
        // the scale is in the key too
        let rescaled = |b: &mut Backdrop| b.update(&src, STRIDE, SIZE, INSIDE, 10.25, 1.25, &[]);
        assert_eq!(rescaled(&mut b), Some(INSIDE));
        assert_eq!(b.recomputed_px(), whole);
        assert_eq!(rescaled(&mut b), None);
        b.invalidate();
        assert_eq!(rescaled(&mut b), Some(INSIDE));
    }

    #[test]
    fn region_move_recomputes() {
        let src = noise(4);
        let mut b = full(&src, INSIDE, 10.0);
        for moved in [INSIDE.translate(8, 0), INSIDE.translate(1, 3)] {
            assert_eq!(update(&mut b, &src, moved, 10.0, &[]), Some(moved));
            assert!(b.recomputed_px() > 0);
            assert_eq!(sampled(&b, moved), sampled(&full(&src, moved, 10.0), moved));
        }
    }

    #[test]
    fn empty_region_is_none() {
        let src = noise(5);
        let mut b = Backdrop::new();
        assert_eq!(
            update(&mut b, &src, Rect::new(10, 10, 0, 20), 10.0, &[]),
            None
        );
        assert_eq!(
            update(&mut b, &src, Rect::new(4000, 10, 20, 20), 10.0, &[]),
            None
        );
        assert_eq!(b.recomputed_px(), 0);
        let mut row = [7; 4];
        b.sample_row(10, 10, &mut row);
        assert_eq!(row, [0; 4], "nothing cached samples clear");
    }

    #[test]
    fn region_past_the_source_clamps_to_its_edge() {
        let src = flat();
        let region = Rect::new(240, 190, 40, 30);
        let b = full(&src, region, 10.0);
        assert!(sampled(&b, region).iter().all(|&p| p == src[0]));
    }

    /// A source whose edge is off the grid gives what the same source gives with its edge repeated up to the grid.
    #[test]
    fn source_off_the_grid_repeats_its_edge() {
        let src = noise(6);
        let (w, h) = (253, 199);
        let mut padded = src.clone();
        for y in 0..SIZE.1 {
            for x in 0..SIZE.0 {
                padded[y as usize * STRIDE + x as usize] =
                    src[y.min(h - 1) as usize * STRIDE + x.min(w - 1) as usize];
            }
        }
        let region = Rect::new(200, 150, 53, 49);
        let mut b = Backdrop::new();
        assert_eq!(
            b.update(&src, STRIDE, (w, h), region, 10.0, 1.0, &[]),
            Some(region)
        );
        assert!(b.image == full(&padded, region, 10.0).image);
    }

    #[test]
    #[ignore = "timing, by hand"]
    fn bench_palette_region() {
        let (w, h) = (1920_i32, 1200_i32);
        let mut r = XorShift::new(9);
        let src: Vec<u32> = (0..w * h)
            .map(|i| match (i % w) / 7 + (i / w) / 16 + r.below(3) as i32 {
                n if n % 5 == 0 => 0xff20_2020,
                n if n % 5 == 1 => 0xff30_4050,
                _ => 0xfff0_f0f0,
            })
            .collect();
        // `blur(10px)`: 1400×920 as the design's row names it, and the palette at scale 2
        for (region, sigma) in [
            (Rect::new(260, 140, 1400, 920), 10.0),
            (Rect::new(320, 200, 1280, 800), 20.0),
        ] {
            let mut b = Backdrop::new();
            let mut row = vec![0; region.w as usize];
            let (mut update_ms, mut sample_ms) = (Vec::new(), Vec::new());
            for _ in 0..41 {
                b.invalidate();
                let t = std::time::Instant::now();
                b.update(&src, w as usize, (w, h), region, sigma, 2.0, &[]);
                update_ms.push(t.elapsed().as_secs_f64() * 1e3);
                let t = std::time::Instant::now();
                for y in region.y..region.y + region.h {
                    b.sample_row(y, region.x, &mut row);
                    std::hint::black_box(&row);
                }
                sample_ms.push(t.elapsed().as_secs_f64() * 1e3);
            }
            update_ms.sort_by(f64::total_cmp);
            sample_ms.sort_by(f64::total_cmp);
            println!(
                "{} {}x{} σ {sigma}: update {:.2} ms, fused sample {:.2} ms, cache {} kB",
                kernels().name,
                region.w,
                region.h,
                update_ms[20],
                sample_ms[20],
                b.bytes() / 1024
            );
        }
    }
}

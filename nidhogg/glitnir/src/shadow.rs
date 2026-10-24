//! Shadows: the Gaussian-blurred rounded rectangle in closed form (Evan Wallace's method: erf across one axis, 8
//! Gaussian-weighted samples along the other), rendered once per parameter set into nine-slice tiles.
//!
//! The shadow is symmetric about its centre, so one corner tile, mirrored four ways, holds every pixel whose value
//! varies along both axes. Past the corner tile the value varies along one axis only: the top and bottom edges read
//! one column profile, the left and right edges one row profile, and the middle is one value. Offset is a translation
//! (the caller places the image); spread grows the box, and the radius as CSS says. When σ ≥ 8 device px the tiles
//! hold every 2nd or 4th pixel and are sampled bilinearly. 8 samples measure ≤ 1.21/255 against a supersampled true
//! Gaussian; 4 samples miss the 2/255 gate.
//!
//! Every length here is in device px. σ = blur / 2 (CSS).

use crate::kernels::{Kernels, Lru, Mask, PixelsMut};
use std::sync::Arc;

/// Gaussian-weighted samples along the second axis.
pub const SAMPLES: usize = 8;

/// A sharp shadow (blur 0) is drawn with this σ: a quarter pixel, nearly a hard edge.
const MIN_SIGMA: f32 = 0.25;

/// One shadow's parameters.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShadowSpec {
    /// The layer's corner radius.
    pub radius: f32,
    /// The CSS blur radius; σ = blur / 2.
    pub blur: f32,
    /// The CSS spread (rounded to whole pixels); negative shrinks the shadow.
    pub spread: f32,
    /// Premultiplied colour in Cairo's ARGB32 order, the opacity already applied.
    pub colour: u32,
    /// The device scale the parameters were converted at. Part of the cache key only: never a transition's scale
    /// (a transition stretches the cached tiles).
    pub scale: f64,
}

/// σ for a CSS blur radius.
pub fn sigma(blur: f32) -> f32 {
    (blur / 2.0).max(MIN_SIGMA)
}

/// How far the shadow reaches past its box (the layer grown by the spread): 3σ, where the Gaussian's tail is
/// 0.13 %, a third of 1/255.
pub fn reach(blur: f32) -> u32 {
    (3.0 * sigma(blur)).ceil() as u32
}

/// The tiles' resolution, one sample every `step` px: 1 below σ 8, 2 from σ 8, 4 from σ 16 — so a step is at most
/// σ/4 (DP-5: the coarsest that keeps the error within 2/255 as sampled; the tests record each case).
pub fn step_for(sigma: f32) -> u32 {
    if sigma >= 16.0 {
        4
    } else if sigma >= 8.0 {
        2
    } else {
        1
    }
}

/// The CSS spread radius: r + s, but for a radius smaller than the spread, r + s·(1 + (r/s − 1)³), so nearly square
/// corners stay nearly square (CSS Backgrounds 3, "box-shadow").
fn spread_radius(r: f32, s: f32) -> f32 {
    if s > 0.0 && r < s {
        let t = r / s - 1.0;
        r + s * (1.0 + t * t * t)
    } else {
        (r + s).max(0.0)
    }
}

/// erf, Abramowitz and Stegun 7.1.27 (error ≤ 5·10⁻⁴).
fn erf(x: f32) -> f32 {
    let s = x.signum();
    let a = x.abs();
    let mut y = 1.0 + (0.278393 + (0.230389 + 0.078108 * (a * a)) * a) * a;
    y *= y;
    s - s / (y * y)
}

fn gaussian(x: f32, sigma: f32) -> f32 {
    (-(x * x) / (2.0 * sigma * sigma)).exp() / ((2.0 * std::f32::consts::PI).sqrt() * sigma)
}

/// The blurred row through height `y` of the box (half sizes hw, hh, corner radius r), at `x`: erf in closed form.
fn shadow_x(x: f32, y: f32, sigma: f32, r: f32, hw: f32, hh: f32) -> f32 {
    let delta = (hh - r - y.abs()).min(0.0);
    let curved = hw - r + (r * r - delta * delta).max(0.0).sqrt();
    let k = std::f32::consts::FRAC_1_SQRT_2 / sigma;
    (0.5 + 0.5 * erf((x + curved) * k)) - (0.5 + 0.5 * erf((x - curved) * k))
}

/// The shadow's coverage at (x, y) from the box's centre: `samples` rows within ±3σ, each weighted by the Gaussian.
fn wallace(x: f32, y: f32, hw: f32, hh: f32, sigma: f32, r: f32, samples: usize) -> f32 {
    let start = (-3.0 * sigma).clamp(y - hh, y + hh);
    let end = (3.0 * sigma).clamp(y - hh, y + hh);
    let step = (end - start) / samples as f32;
    let mut t = start + step * 0.5;
    let mut v = 0.0;
    for _ in 0..samples {
        v += shadow_x(x, y - t, sigma, r, hw, hh) * gaussian(t, sigma) * step;
        t += step;
    }
    v
}

fn unit(v: f32) -> u16 {
    (v.clamp(0.0, 1.0) * 65535.0 + 0.5) as u16
}

/// A shadow's geometry for a `w`×`h` layer: everything the tiles depend on.
struct Geometry {
    spread: i32,
    sigma: f32,
    radius: f32,
    reach: u32,
    /// The box's size class: its size, capped where the far edges stop mattering (2 × (radius + 4σ) per axis).
    class_w: u32,
    class_h: u32,
}

impl Geometry {
    fn new(w: u32, h: u32, spec: &ShadowSpec) -> Geometry {
        let spread = spec.spread.round() as i32;
        let (bw, bh) = (grow(w, spread), grow(h, spread));
        let sigma = sigma(spec.blur);
        let radius =
            spread_radius(spec.radius.max(0.0), spread as f32).min(bw.min(bh) as f32 / 2.0);
        // Past radius + 4σ inside the box the far edge adds under 4·10⁻⁵ (Φ(−4)): the class stops there.
        let flat = radius.ceil() as u32 + (4.0 * sigma).ceil() as u32;
        Geometry {
            spread,
            sigma,
            radius,
            reach: reach(spec.blur),
            class_w: bw.min(2 * flat),
            class_h: bh.min(2 * flat),
        }
    }
}

/// The box size: the layer's side grown by the spread on both ends.
fn grow(side: u32, spread: i32) -> u32 {
    (side as i64 + 2 * spread as i64).max(0) as u32
}

/// A shadow's nine-slice tiles: coverage in 1/65535, sampled every `step` px at the blocks' centres.
pub struct ShadowTiles {
    step: u32,
    reach: u32,
    spread: i32,
    /// The corner tile covers image px [0, qx) × [0, qy) (the image is the box plus `reach` on every side).
    qx: u32,
    qy: u32,
    /// Samples per corner-tile row; `top` holds one per corner-tile row.
    tw: usize,
    corner: Box<[u16]>,
    /// Along a column through the centre: the top and bottom edges past the corner tile.
    top: Box<[u16]>,
    /// Along a row through the centre (`tw` samples): the left and right edges past the corner tile.
    left: Box<[u16]>,
    /// The middle, past the corner tile on both axes.
    centre: u16,
    colour: u32,
}

impl ShadowTiles {
    /// The tiles for a `w`×`h` layer with `samples` samples at one sample every `step` px.
    pub(crate) fn build(
        w: u32,
        h: u32,
        spec: &ShadowSpec,
        samples: usize,
        step: u32,
    ) -> ShadowTiles {
        let g = Geometry::new(w, h, spec);
        let (hw, hh) = (g.class_w as f32 / 2.0, g.class_h as f32 / 2.0);
        let e = g.reach;
        let (qx, qy) = (e + g.class_w.div_ceil(2), e + g.class_h.div_ceil(2));
        // one sample past the tile's end, so bilinear sampling never reads outside it
        let (tw, th) = (
            qx.div_ceil(step) as usize + 1,
            qy.div_ceil(step) as usize + 1,
        );
        let at =
            |k: usize, half: f32| (k as u32 * step) as f32 + step as f32 / 2.0 - e as f32 - half;
        let value = |x: f32, y: f32| unit(wallace(x, y, hw, hh, g.sigma, g.radius, samples));
        let mut corner = vec![0u16; tw * th];
        for j in 0..th {
            for i in 0..tw {
                corner[j * tw + i] = value(at(i, hw), at(j, hh));
            }
        }
        ShadowTiles {
            step,
            reach: e,
            spread: g.spread,
            qx,
            qy,
            tw,
            corner: corner.into(),
            top: (0..th).map(|j| value(0.0, at(j, hh))).collect(),
            left: (0..tw).map(|i| value(at(i, hw), 0.0)).collect(),
            centre: value(0.0, 0.0),
            colour: spec.colour,
        }
    }

    /// One sample every `step()` px.
    pub fn step(&self) -> u32 {
        self.step
    }

    /// The colour the shadow is drawn in.
    pub fn colour(&self) -> u32 {
        self.colour
    }

    /// How far the image starts above and left of the layer (before the offset): reach + spread.
    pub fn pad(&self) -> i32 {
        self.reach as i32 + self.spread
    }

    /// The shadow image's size for a `w`×`h` layer.
    pub fn image_size(&self, w: u32, h: u32) -> (u32, u32) {
        (
            grow(w, self.spread) + 2 * self.reach,
            grow(h, self.spread) + 2 * self.reach,
        )
    }

    /// Image px → (sample index, fraction in 1/256) on a tile axis: centres at k·step + step/2.
    fn split(&self, px: u32) -> (usize, u32) {
        let (num, den) = (2 * px as i64 + 1 - self.step as i64, 2 * self.step as i64);
        if num <= 0 {
            return (0, 0);
        }
        ((num / den) as usize, ((num % den) * 256 / den) as u32)
    }

    /// Coverage (0–255) of image row `y`, columns `x0..x0 + out.len()`, for a `w`×`h` layer.
    pub fn coverage_row(&self, w: u32, h: u32, y: u32, x0: u32, out: &mut [u8]) {
        let (iw, ih) = self.image_size(w, h);
        assert!(
            y < ih && x0 as usize + out.len() <= iw as usize,
            "outside the shadow image"
        );
        let lerp = |a: u16, b: u16, f: u32| a as u32 * (256 - f) + b as u32 * f;
        let to_u8 = |v: u64| ((v * 255 + (65535 << 15)) / (65535 << 16)) as u8;
        let yy = y.min(ih - 1 - y);
        let (k, fy) = self.split(yy.min(self.qy - 1));
        // this row's slice of the corner tile (or of the row profile), interpolated vertically, in 1/65535 × 256
        let row: Vec<u32> = if yy < self.qy {
            let (t0, t1) = (
                &self.corner[k * self.tw..][..self.tw],
                &self.corner[(k + 1) * self.tw..][..self.tw],
            );
            t0.iter().zip(t1).map(|(&a, &b)| lerp(a, b, fy)).collect()
        } else {
            self.left.iter().map(|&a| a as u32 * 256).collect()
        };
        let beyond = if yy < self.qy {
            lerp(self.top[k], self.top[k + 1], fy) as u64 * 256
        } else {
            self.centre as u64 * 65536
        };
        for (x, o) in (x0..).zip(out.iter_mut()) {
            let xx = x.min(iw - 1 - x);
            *o = to_u8(if xx < self.qx {
                let (c, fx) = self.split(xx);
                row[c] as u64 * (256 - fx) as u64 + row[c + 1] as u64 * fx as u64
            } else {
                beyond
            });
        }
    }

    /// Draw the shadow of a `w`×`h` layer (`size`): its image's `span` (width, height) from image px `from`, into
    /// `dst`, whose first pixel is the image's pixel `from`.
    pub fn composite(
        &self,
        k: &Kernels,
        dst: &mut PixelsMut,
        size: (u32, u32),
        from: (u32, u32),
        span: (usize, usize),
    ) {
        let (cw, ch) = span;
        let mut cov = vec![0u8; cw];
        for r in 0..ch {
            self.coverage_row(size.0, size.1, from.1 + r as u32, from.0, &mut cov);
            let stride = dst.stride;
            let mut row = PixelsMut {
                data: &mut dst.data[r * stride..],
                stride,
            };
            let mask = Mask {
                data: &cov,
                stride: cw,
            };
            k.solid_over(&mut row, self.colour, &mask, cw, 1);
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
struct Key {
    class: (u32, u32),
    radius: u32,
    blur: u32,
    spread: u32,
    colour: u32,
    scale: u64,
}

/// Shadow tiles by (size class, radius, blur, spread, colour, scale), the most recently used 64.
pub struct ShadowCache {
    lru: Lru<Key, Arc<ShadowTiles>>,
}

impl Default for ShadowCache {
    fn default() -> Self {
        ShadowCache::new()
    }
}

impl ShadowCache {
    /// The most entries kept.
    pub const CAPACITY: usize = 64;

    pub fn new() -> ShadowCache {
        ShadowCache {
            lru: Lru::new(Self::CAPACITY),
        }
    }

    /// The tiles of a `w`×`h` layer's shadow; made on a miss.
    pub fn get(&mut self, w: u32, h: u32, spec: &ShadowSpec) -> Arc<ShadowTiles> {
        let g = Geometry::new(w, h, spec);
        let key = Key {
            class: (g.class_w, g.class_h),
            radius: spec.radius.to_bits(),
            blur: spec.blur.to_bits(),
            spread: spec.spread.to_bits(),
            colour: spec.colour,
            scale: spec.scale.to_bits(),
        };
        let step = step_for(g.sigma);
        self.lru.get_or_insert_with(key, || {
            Arc::new(ShadowTiles::build(w, h, spec, SAMPLES, step))
        })
    }

    /// Entries held.
    pub fn len(&self) -> usize {
        self.lru.len()
    }

    pub fn is_empty(&self) -> bool {
        self.lru.len() == 0
    }
}

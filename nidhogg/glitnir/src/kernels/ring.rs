//! The border ring: the host draws a square border of width b; the ring redraws it along the rounded path, in the
//! four corner squares only, so the straight edges stay the host's own pixels.
//!
//! In a corner square the outer path is the arc of radius r and the inner path the concentric arc of radius r − b
//! (the inner rectangle is inset by b with radius r − b, so both arcs share the centre). A bordered layer's corners
//! are masked with `inner` and the ring is drawn over them with `ring`; at the outer edge `inner` is 0, so the ring's
//! antialiasing meets the background alone.

use super::corner::band;
use super::over::mul_un8x4;
use super::{Kernels, Lru, Mask, Pixels, PixelsMut, lock};
use std::sync::{Arc, Mutex};

/// The two masks of a bordered corner, 2r×2r like [`super::corner_coverage`]'s.
pub struct RingCoverage {
    /// 2r: the side of each mask.
    pub size: usize,
    /// Inside the inner path: where the layer's own pixels show.
    pub inner: Box<[u8]>,
    /// Between the inner and the outer path: the border colour.
    pub ring: Box<[u8]>,
}

static RINGS: Mutex<Lru<(u32, u32), Arc<RingCoverage>>> = Mutex::new(Lru::new(16));

/// The masks for corner radius `radius` and border width `border` (device px), cached.
pub fn ring_coverage(radius: u32, border: u32) -> Arc<RingCoverage> {
    lock(&RINGS).get_or_insert_with((radius, border), || {
        let inner_r = radius.saturating_sub(border) as f64;
        Arc::new(RingCoverage {
            size: 2 * radius as usize,
            inner: band(radius, None, inner_r),
            ring: band(radius, Some(inner_r), radius as f64),
        })
    })
}

type TileKey = (u32, u32, u32, u32, u32);
static TILES: Mutex<Lru<TileKey, Arc<[u32]>>> = Mutex::new(Lru::new(32));

/// A decorated box's corners (PD) as one premultiplied 2r×2r tile around the arcs' centre, its quadrants in place
/// like [`super::corner_coverage`]'s: `beneath` outside the outer arc, `ring` between the arcs (border `border`),
/// clear inside the inner one, so the tile goes OVER what is drawn there. With `fill` not 0 that tile is composed
/// OVER `fill` with the OVER kernel's arithmetic, so a repaint of the square on `fill` gives the very pixels the
/// box drawn over `fill` has. Built from the corner and ring masks; cached per (radius, border, colours), the 32
/// most recently used.
pub fn decor_tile(radius: u32, border: u32, fill: u32, ring: u32, beneath: u32) -> Arc<[u32]> {
    lock(&TILES).get_or_insert_with((radius, border, fill, ring, beneath), || {
        let disc = super::corner_coverage(radius);
        let band = (border > 0).then(|| ring_coverage(radius, border));
        let mix = |i: usize| {
            // the ring lies within the disc
            let out = 255 - disc[i] as u32;
            let r = band.as_ref().map_or(0, |b| b.ring[i] as u32).min(255 - out);
            let mut px = 0;
            for shift in [0, 8, 16, 24] {
                let ch = |c: u32| (c >> shift) & 0xff;
                px |= ((ch(beneath) * out + ch(ring) * r + 127) / 255) << shift;
            }
            px.wrapping_add(mul_un8x4(fill, 255 - (px >> 24)))
        };
        (0..disc.len()).map(mix).collect()
    })
}

/// The corner radius a `w`×`h` rectangle can have: at most half its shorter side.
pub fn clamp_radius(w: usize, h: usize, radius: u32) -> u32 {
    radius.min((w.min(h) / 2).try_into().unwrap_or(u32::MAX))
}

impl Kernels {
    /// Draw `colour` (premultiplied) along the rounded border path of the `w`×`h` layer whose top-left pixel is
    /// `dst`'s first: in the four corner squares only. The radius is clamped with [`clamp_radius`].
    pub fn ring(
        &self,
        dst: &mut PixelsMut,
        w: usize,
        h: usize,
        radius: u32,
        border: u32,
        colour: u32,
    ) {
        let r = clamp_radius(w, h, radius);
        if r == 0 || border == 0 {
            return;
        }
        let cov = ring_coverage(r, border);
        let (n, r) = (cov.size, r as usize);
        // (mask x, mask y) of each quadrant, (x, y) of its corner square in the layer
        for (mx, my, x, y) in [
            (0, 0, 0, 0),
            (r, 0, w - r, 0),
            (0, r, 0, h - r),
            (r, r, w - r, h - r),
        ] {
            let mask = Mask {
                data: &cov.ring[my * n + mx..],
                stride: n,
            };
            let stride = dst.stride;
            let mut d = PixelsMut {
                data: &mut dst.data[y * stride + x..],
                stride,
            };
            self.solid_over(&mut d, colour, &mask, r, r);
        }
    }

    /// Draw the corners in `mask` (1 top left, 2 top right, 4 bottom right, 8 bottom left) of the `w`×`h` decorated
    /// box whose top-left pixel is `dst`'s first, from [`decor_tile`]: OVER the pixels when `fill` is 0 (a box just
    /// drawn keeps its inside), else replacing the corner squares (the repaint of a run's end). The radius is clamped
    /// with [`clamp_radius`]; the straight edges are the caller's.
    #[allow(clippy::too_many_arguments)]
    pub fn decor_corners(
        &self,
        dst: &mut PixelsMut,
        w: usize,
        h: usize,
        radius: u32,
        border: u32,
        fill: u32,
        ring: u32,
        beneath: u32,
        mask: u32,
    ) {
        let r = clamp_radius(w, h, radius);
        if r == 0 {
            return;
        }
        let tile = decor_tile(r, border, fill, ring, beneath);
        let (n, r) = (2 * r as usize, r as usize);
        for (bit, mx, my, x, y) in [
            (1, 0, 0, 0, 0),
            (2, r, 0, w - r, 0),
            (4, r, r, w - r, h - r),
            (8, 0, r, 0, h - r),
        ] {
            if mask & bit == 0 {
                continue;
            }
            let src = Pixels {
                data: &tile[my * n + mx..],
                stride: n,
            };
            let stride = dst.stride;
            let mut d = PixelsMut {
                data: &mut dst.data[y * stride + x..],
                stride,
            };
            if fill == 0 {
                self.over(&mut d, &src, r, r);
            } else {
                self.copy(&mut d, &src, r, r);
            }
        }
    }
}

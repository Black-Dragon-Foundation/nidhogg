//! The border ring: the host draws a square border of width b; the ring redraws it along the rounded path, in the
//! four corner squares only, so the straight edges stay the host's own pixels.
//!
//! In a corner square the outer path is the arc of radius r and the inner path the concentric arc of radius r − b
//! (the inner rectangle is inset by b with radius r − b, so both arcs share the centre). A bordered layer's corners
//! are masked with `inner` and the ring is drawn over them with `ring`; at the outer edge `inner` is 0, so the ring's
//! antialiasing meets the background alone.

use super::corner::band;
use super::{Kernels, Lru, Mask, PixelsMut, lock};
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
}

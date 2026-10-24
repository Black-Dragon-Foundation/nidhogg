//! Corner coverage: how much of each pixel lies inside a rounded rectangle's corner arc.
//!
//! The masks are 2r×2r squares around the arcs' common centre: their four r×r quadrants are the four corner squares
//! of any rectangle with corner radius r, in place (top-left quadrant = top-left corner, and so on). Pixels the arc
//! does not cross are exact (0 or 255); on the arc each pixel takes 16×16 point samples. (Coverage from the signed
//! distance alone, a linear ramp, errs by up to ~11/255 where the arc runs at 45°; the gate is 1/255.)

use super::{Lru, lock};
use std::sync::{Arc, Mutex};

static CORNERS: Mutex<Lru<u32, Arc<[u8]>>> = Mutex::new(Lru::new(16));

/// The coverage of a 2r×2r square by the disc of radius r at its centre (row-major, 2r bytes a row), cached per
/// radius. Radius 0 is empty.
pub fn corner_coverage(radius: u32) -> Arc<[u8]> {
    lock(&CORNERS).get_or_insert_with(radius, || band(radius, None, radius as f64).into())
}

/// The coverage of the 2r×2r square by the points whose distance d from its centre is in (lo, hi] (lo `None`: no
/// lower bound).
pub(super) fn band(r: u32, lo: Option<f64>, hi: f64) -> Box<[u8]> {
    const HALF_DIAGONAL: f64 = std::f64::consts::FRAC_1_SQRT_2;
    let n = 2 * r as usize;
    let c = r as f64;
    let mut m = vec![0u8; n * n];
    let wanted = |d2: f64| d2 <= hi * hi && lo.is_none_or(|lo| d2 > lo * lo);
    // the top-left quadrant, mirrored into the other three (the sample grid is symmetric within a pixel, so the
    // mirrored pixels' samples are the mirrored samples)
    for j in 0..r as usize {
        for i in 0..r as usize {
            let (dx, dy) = (c - (i as f64 + 0.5), c - (j as f64 + 0.5));
            let d = (dx * dx + dy * dy).sqrt();
            let (near, far) = (d - HALF_DIAGONAL, d + HALF_DIAGONAL);
            let out = near >= hi || lo.is_some_and(|lo| far <= lo);
            let full = far <= hi && lo.is_none_or(|lo| near > lo);
            let v = if out {
                0
            } else if full {
                255
            } else {
                let mut k = 0u32;
                for sy in 0..16 {
                    let y = j as f64 + (sy as f64 + 0.5) / 16.0 - c;
                    for sx in 0..16 {
                        let x = i as f64 + (sx as f64 + 0.5) / 16.0 - c;
                        k += wanted(x * x + y * y) as u32;
                    }
                }
                ((k * 255 + 128) / 256) as u8
            };
            let (i2, j2) = (n - 1 - i, n - 1 - j);
            m[j * n + i] = v;
            m[j * n + i2] = v;
            m[j2 * n + i] = v;
            m[j2 * n + i2] = v;
        }
    }
    m.into_boxed_slice()
}

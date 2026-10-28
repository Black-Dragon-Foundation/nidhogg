//! Stable rounding of logical rectangles to device pixels and snapping to the device grid.
//!
//! Stable rounding follows mutter's fractional-scale rule: an origin is `roundf(pos * s)` and an extent is
//! `roundf((pos + size) * s) - roundf(pos * s)`, in `f32`, half away from zero, so a child's edges land where its
//! parent's rounding puts them.  Snapping moves a rectangle's edges to `round(e * s) / s` and treats a rectangle
//! under two device pixels as a line width (CSS Values 4); at an integer scale nothing changes.

use crate::geom::Rect;

/// A rectangle in logical units.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct RectF {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

fn scale_of(scale120: u32) -> f32 {
    scale120 as f32 / 120.0
}

/// `roundf(pos * s)`: the device origin of a surface at logical `pos`.
pub fn stable_origin(pos: i32, scale120: u32) -> i32 {
    (pos as f32 * scale_of(scale120)).round() as i32
}

/// `roundf((pos + size) * s) - roundf(pos * s)`: the device extent of the span `pos .. pos + size`.
pub fn stable_extent(pos: i32, size: i32, scale120: u32) -> i32 {
    let s = scale_of(scale120);
    ((pos + size) as f32 * s).round() as i32 - (pos as f32 * s).round() as i32
}

/// The device rectangle of `logical`, relative to the device origin of a surface at logical (`ox`, `oy`).
pub fn stable_rect(logical: Rect, ox: i32, oy: i32, scale120: u32) -> Rect {
    let (x, y) = (ox + logical.x, oy + logical.y);
    Rect::new(
        stable_origin(x, scale120) - stable_origin(ox, scale120),
        stable_origin(y, scale120) - stable_origin(oy, scale120),
        stable_extent(x, logical.w, scale120),
        stable_extent(y, logical.h, scale120),
    )
}

/// The smallest logical rectangle on a surface's grid (its origin at logical 0) whose stable device rectangle
/// (`stable_rect(_, 0, 0, scale120)`) holds the device rectangle `r`: `r`'s edges moved outward to device rows and
/// columns a compositor can place a subsurface on.  Empty for an empty `r` or a zero scale.
pub fn outward(r: Rect, scale120: u32) -> Rect {
    if r.is_empty() || scale120 == 0 {
        return Rect::default();
    }
    let guess = |d: i32| (d as f64 * 120.0 / scale120 as f64) as i32;
    // the last logical position whose device origin is at or before d
    let before = |d: i32| {
        let mut l = guess(d);
        while stable_origin(l, scale120) > d {
            l -= 1;
        }
        while stable_origin(l + 1, scale120) <= d {
            l += 1;
        }
        l
    };
    // the first logical position whose device origin is at or after d
    let after = |d: i32| {
        let mut l = guess(d);
        while stable_origin(l, scale120) < d {
            l += 1;
        }
        while stable_origin(l - 1, scale120) >= d {
            l -= 1;
        }
        l
    };
    let (x0, y0) = (before(r.x), before(r.y));
    let (x1, y1) = (after(r.x + r.w), after(r.y + r.h));
    Rect::new(x0, y0, x1 - x0, y1 - y0)
}

fn identity_scale(scale: f64) -> bool {
    !scale.is_finite() || scale <= 0.0 || scale.fract() == 0.0
}

/// One axis: the snapped start and length, in logical units.
fn snap_span(pos: f64, len: f64, scale: f64) -> (f64, f64) {
    if len * scale < 2.0 {
        (
            (pos * scale).round() / scale,
            (len * scale).floor().max(1.0) / scale,
        )
    } else {
        let a = (pos * scale).round();
        let b = ((pos + len) * scale).round();
        (a / scale, (b - a) / scale)
    }
}

/// Snap a rectangle's edges to the device grid at `scale`.
pub fn snap_rect(x: f64, y: f64, w: f64, h: f64, scale: f64) -> RectF {
    if identity_scale(scale) {
        return RectF { x, y, w, h };
    }
    let (x, w) = snap_span(x, w, scale);
    let (y, h) = snap_span(y, h, scale);
    RectF { x, y, w, h }
}

/// Snap a line width: under two device pixels `max(1, floor(w * s))` of them, else the nearest whole count.
pub fn snap_line(w: f64, scale: f64) -> f64 {
    if identity_scale(scale) {
        return w;
    }
    let px = w * scale;
    if px < 2.0 {
        px.floor().max(1.0) / scale
    } else {
        px.round() / scale
    }
}

/// Whether pixel `x` of the A1 row `row` is set (cairo's A1: bit `x % 8` of byte `x / 8` from the least significant
/// end on a little-endian host, from the most significant end on a big-endian one).
fn a1_bit(row: &[u8], x: usize) -> bool {
    let shift = if cfg!(target_endian = "little") {
        x % 8
    } else {
        7 - x % 8
    };
    row[x / 8] >> shift & 1 != 0
}

/// Resample the `w` x `h` mask `src` (A1 when `a1`, else A8) to A8 on the device grid: logical cell `i` covers the
/// device pixels `[stable_origin(i), stable_origin(i + 1))`, each taking the cell's value from the nearest source, so
/// every device pixel of an A1 mask is fully on or off.  `out` holds `stable_origin(w)` x `stable_origin(h)` pixels.
/// `None` when a size or stride is too small for the slices.
#[allow(clippy::too_many_arguments)]
pub fn mask_to_device(
    src: &[u8],
    src_stride: usize,
    w: usize,
    h: usize,
    a1: bool,
    scale120: u32,
    out: &mut [u8],
    out_stride: usize,
) -> Option<()> {
    let (dw, dh) = (
        stable_origin(w as i32, scale120).max(0) as usize,
        stable_origin(h as i32, scale120).max(0) as usize,
    );
    let row_bytes = if a1 { w.div_ceil(8) } else { w };
    if w == 0 || h == 0 || src_stride < row_bytes || out_stride < dw {
        return None;
    }
    if src.len() < src_stride * (h - 1) + row_bytes
        || (dh > 0 && out.len() < out_stride * (dh - 1) + dw)
    {
        return None;
    }
    // the source column of each device column, and the source row of each device row
    let span = |n: usize, d: usize| -> Vec<usize> {
        let mut v = Vec::with_capacity(d);
        for i in 0..n {
            let (a, b) = (
                stable_origin(i as i32, scale120),
                stable_origin(i as i32 + 1, scale120),
            );
            v.extend((a..b).map(|_| i));
        }
        v
    };
    let (cols, rows) = (span(w, dw), span(h, dh));
    for (dy, &sy) in rows.iter().enumerate() {
        let srow = &src[sy * src_stride..sy * src_stride + row_bytes];
        let orow = &mut out[dy * out_stride..dy * out_stride + dw];
        for (o, &sx) in orow.iter_mut().zip(&cols) {
            *o = if a1 {
                if a1_bit(srow, sx) { 255 } else { 0 }
            } else {
                srow[sx]
            };
        }
    }
    Some(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rng::XorShift;

    #[test]
    fn stable_extent_matches_mutter() {
        // roundf(1845) - roundf(32.5) = 1845 - 33; the naive round(1450 * 1.25) is 1813.
        assert_eq!(stable_extent(26, 1450, 150), 1812);
        assert_eq!((1450f64 * 1.25).round() as i32, 1813);
        let mut differ = 0;
        for scale120 in [150u32, 180] {
            let s = scale120 as f64 / 120.0;
            for pos in 0..40 {
                for size in 1..200 {
                    let naive = (size as f64 * s).round() as i32;
                    let stable = stable_extent(pos, size, scale120);
                    assert!((stable - naive).abs() <= 1);
                    // the extents of adjacent spans tile exactly
                    let right = stable_origin(pos + size, scale120) - stable_origin(pos, scale120);
                    assert_eq!(stable, right);
                    if stable != naive {
                        differ += 1;
                    }
                }
            }
        }
        assert!(differ > 0);
    }

    #[test]
    fn rounding_is_half_away_from_zero() {
        assert_eq!(stable_origin(1, 60), 1);
        assert_eq!(stable_origin(5, 60), 3);
        assert_eq!(stable_origin(-1, 60), -1);
    }

    #[test]
    fn stable_rect_is_relative_to_the_surface() {
        let r = stable_rect(Rect::new(10, 4, 20, 8), 26, 0, 150);
        assert_eq!(r.x, stable_origin(36, 150) - stable_origin(26, 150));
        assert_eq!(r.w, stable_extent(36, 20, 150));
        assert_eq!((r.y, r.h), (5, 10));
    }

    #[test]
    fn outward_is_the_smallest_placeable_cover() {
        let mut g = XorShift::new(0x5EED);
        for scale120 in [120u32, 150, 180, 210, 240, 300] {
            for _ in 0..500 {
                let r = Rect::new(
                    g.below(2000) as i32 - 100,
                    g.below(2000) as i32 - 100,
                    1 + g.below(900) as i32,
                    1 + g.below(900) as i32,
                );
                let l = outward(r, scale120);
                let d = stable_rect(l, 0, 0, scale120);
                assert_eq!(d.intersect(&r), r, "{r:?} at {scale120}: {d:?}");
                // one logical unit in from any side no longer holds it
                for (dx, dy, dw, dh) in [(1, 0, -1, 0), (0, 1, 0, -1), (0, 0, -1, 0), (0, 0, 0, -1)]
                {
                    let s = Rect::new(l.x + dx, l.y + dy, l.w + dw, l.h + dh);
                    let sd = stable_rect(s, 0, 0, scale120);
                    assert_ne!(
                        sd.intersect(&r),
                        r,
                        "{r:?} at {scale120}: {l:?} not the smallest"
                    );
                }
            }
        }
        // an integer scale's cover of an aligned rectangle is that rectangle
        assert_eq!(
            outward(Rect::new(10, 20, 30, 40), 120),
            Rect::new(10, 20, 30, 40)
        );
        assert_eq!(
            outward(Rect::new(11, 20, 30, 40), 240),
            Rect::new(5, 10, 16, 20)
        );
        assert_eq!(outward(Rect::default(), 150), Rect::default());
    }

    #[test]
    fn snap_moves_edges_to_device_grid() {
        let r = snap_rect(10.1, 0.0, 20.3, 40.0, 1.25);
        assert_eq!(r.x, (10.1f64 * 1.25).round() / 1.25);
        assert_eq!(r.x, 10.4);
        assert_eq!(r.w, 20.0);
        assert_eq!(r.x + r.w, (30.4f64 * 1.25).round() / 1.25);
    }

    #[test]
    fn thin_rect_snaps_as_line_width() {
        assert_eq!(snap_rect(3.0, 0.0, 1.0, 10.0, 1.25).w, 0.8);
        assert_eq!(snap_rect(3.0, 0.0, 1.5, 10.0, 1.25).w, 0.8);
        assert_eq!(snap_line(1.0, 1.25), 0.8);
        assert_eq!(snap_line(1.5, 1.25), 0.8);
        assert_eq!(snap_line(0.1, 1.25), 0.8);
        assert_eq!(snap_line(4.0, 1.25), 5.0 / 1.25);
    }

    #[test]
    fn integer_scale_is_identity() {
        let mut rng = XorShift::new(0xC0FFEE);
        let mut f = || rng.next_u64() as f64 / u64::MAX as f64 * 200.0 - 50.0;
        for _ in 0..1000 {
            let (x, y, w, h) = (f(), f(), f(), f());
            for s in [1.0, 2.0] {
                let r = snap_rect(x, y, w, h, s);
                assert_eq!(
                    [r.x.to_bits(), r.y.to_bits(), r.w.to_bits(), r.h.to_bits()],
                    [x.to_bits(), y.to_bits(), w.to_bits(), h.to_bits()]
                );
                assert_eq!(snap_line(w, s).to_bits(), w.to_bits());
            }
        }
    }

    #[test]
    fn mask_cells_cover_stable_extents() {
        for scale120 in [150u32, 180] {
            let (w, h) = (9usize, 7usize);
            // a checkerboard A8 mask: cell value tells the cell
            let src: Vec<u8> = (0..w * h).map(|i| (i % w * 20 + i / w) as u8 + 1).collect();
            let (dw, dh) = (
                stable_origin(w as i32, scale120) as usize,
                stable_origin(h as i32, scale120) as usize,
            );
            let mut out = vec![0u8; dw * dh];
            mask_to_device(&src, w, w, h, false, scale120, &mut out, dw).unwrap();
            for cy in 0..h {
                for cx in 0..w {
                    let (x0, x1) = (
                        stable_origin(cx as i32, scale120),
                        stable_origin(cx as i32 + 1, scale120),
                    );
                    let (y0, y1) = (
                        stable_origin(cy as i32, scale120),
                        stable_origin(cy as i32 + 1, scale120),
                    );
                    for y in y0..y1 {
                        for x in x0..x1 {
                            assert_eq!(out[y as usize * dw + x as usize], src[cy * w + cx]);
                        }
                    }
                }
            }
            // an A1 mask comes out 0 or 255 only
            let bits = [0b0101_0101u8, 0b1];
            let mut o1 = vec![7u8; dw * dh];
            mask_to_device(&bits.repeat(h), 2, w, h, true, scale120, &mut o1, dw).unwrap();
            assert!(o1.iter().all(|&v| v == 0 || v == 255));
        }
    }

    #[test]
    fn mask_integer_scale_is_identity() {
        let (w, h) = (6usize, 5usize);
        let src: Vec<u8> = (0..w * h).map(|i| (i * 7 % 251) as u8).collect();
        let mut one = vec![0u8; w * h];
        mask_to_device(&src, w, w, h, false, 120, &mut one, w).unwrap();
        assert_eq!(one, src);
        let mut two = vec![0u8; 4 * w * h];
        mask_to_device(&src, w, w, h, false, 240, &mut two, 2 * w).unwrap();
        for y in 0..2 * h {
            for x in 0..2 * w {
                assert_eq!(two[y * 2 * w + x], src[y / 2 * w + x / 2]);
            }
        }
        // too small an output is refused
        assert!(mask_to_device(&src, w, w, h, false, 240, &mut one, w).is_none());
    }
}

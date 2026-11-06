//! Bilinear scale of premultiplied pixels about the source's centre (a pop transition).
//!
//! Each destination pixel's centre maps back into the source; where it lands inside the source it takes the bilinear
//! mix of the four nearest pixels (the edge pixel repeated at the border), elsewhere it is 0.  Weights are 8-bit; two
//! channels share one 32-bit multiply, as OVER's do (each 16-bit lane stays below 65536, because the two weights add
//! up to 256), and each pass rounds.  At scale 1 every weight is 0 and the result is a copy.

use super::{Kernels, row_kernel};
use crate::geom::Rect;

const LANES: u32 = 0x00ff_00ff;
const HALF: u32 = 0x0080_0080;

/// A destination column's two source columns and the weight of the second (0 to 256).
#[derive(Clone, Copy)]
pub(super) struct Col {
    i0: u32,
    i1: u32,
    w: u32,
}

/// The row kernel: a destination row from two source rows, a column table and the vertical weight.
pub(super) type ScaleRow = fn(&mut [u32], &[u32], &[u32], &[Col], u32);

/// `a`·(256 − w)/256 + `b`·w/256 per channel, rounded; `w` in 0 to 256.
#[inline(always)]
fn lerp(a: u32, b: u32, w: u32) -> u32 {
    let rb = (((a & LANES) * (256 - w) + (b & LANES) * w + HALF) >> 8) & LANES;
    let ag = (((a >> 8) & LANES) * (256 - w) + ((b >> 8) & LANES) * w + HALF) & 0xff00_ff00;
    rb | ag
}

#[inline(always)]
fn scale_row(d: &mut [u32], r0: &[u32], r1: &[u32], cols: &[Col], wy: u32) {
    for (dp, c) in d.iter_mut().zip(cols) {
        let (i0, i1) = (c.i0 as usize, c.i1 as usize);
        let top = lerp(r0[i0], r0[i1], c.w);
        let bottom = lerp(r1[i0], r1[i1], c.w);
        *dp = lerp(top, bottom, wy);
    }
}

row_kernel!(scale_base, scale_avx2, scale_row, (d: &mut [u32], r0: &[u32], r1: &[u32], cols: &[Col], wy: u32));

/// The source index pair and weight for one axis, at destination pixel `p` of an axis `n` pixels long scaled by `s`
/// about its centre; `None` where the pixel centre maps outside the source.
fn tap(p: i32, n: usize, s: f32) -> Option<(u32, u32, u32)> {
    let c = n as f32 / 2.0;
    let u = (p as f32 + 0.5 - c) / s + c;
    if !(0.0..=n as f32).contains(&u) {
        return None;
    }
    let f = u - 0.5;
    let base = f.floor();
    let w = ((f - base) * 256.0).round() as u32;
    let last = n as i64 - 1;
    let at = |i: i64| i.clamp(0, last) as u32;
    Some((at(base as i64), at(base as i64 + 1), w))
}

impl Kernels {
    /// Scale the `w`×`h` source by `s` about its centre into `dst` (the same size, the same place), writing the pixels
    /// of `clip` only (clipped to the buffer); pixels the scaled source does not cover are 0.  `s` must be positive.
    #[allow(clippy::too_many_arguments)]
    pub fn scale(
        &self,
        dst: &mut [u32],
        dst_stride: usize,
        src: &[u32],
        src_stride: usize,
        w: usize,
        h: usize,
        s: f32,
        clip: Rect,
    ) {
        let clip = clip.intersect(&Rect::new(0, 0, w as i32, h as i32));
        if clip.is_empty() || s <= 0.0 || s.is_nan() {
            return;
        }
        // the columns the scaled source covers, with their taps
        let mut cols = Vec::with_capacity(clip.w as usize);
        let mut first = clip.w;
        for x in 0..clip.w {
            match tap(clip.x + x, w, s) {
                Some((i0, i1, w)) => {
                    first = first.min(x);
                    cols.push(Col { i0, i1, w });
                }
                None if !cols.is_empty() => break,
                None => {}
            }
        }
        let (from, to) = (first as usize, first as usize + cols.len());
        for y in clip.y..clip.y + clip.h {
            let row = &mut dst[y as usize * dst_stride + clip.x as usize..][..clip.w as usize];
            match tap(y, h, s) {
                Some((j0, j1, wy)) if !cols.is_empty() => {
                    row[..from].fill(0);
                    row[to..].fill(0);
                    let r0 = &src[j0 as usize * src_stride..][..w];
                    let r1 = &src[j1 as usize * src_stride..][..w];
                    (self.scale_row)(&mut row[from..to], r0, r1, &cols, wy);
                }
                _ => row.fill(0),
            }
        }
    }
}

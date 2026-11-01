//! The backdrop blur's passes: a 4×4 box average, running-sum boxes across and down, and the bilinear lookup.
//!
//! Each works on the four channels of a pixel alike, so none depends on their order. A sum fits 16 bits and a division by
//! the box's width is a multiply by its inverse taking the high 16 bits, so the loops vectorise (`pmulhuw` and its NEON
//! peer). The running sums start at half the width, which rounds to nearest.

use super::row_kernel;
use std::array::from_fn;

/// A pixel by channel.
pub(crate) type Px = [u8; 4];

/// The largest box radius. Up to this width the inverse, rounded up, adds less than the half that rounds, so a flat
/// colour stays exactly itself; a box twice as wide turns 255 into 256.
pub(crate) const MAX_RADIUS: usize = 63;

/// 2¹⁶ / `span`, rounded up.
pub(super) fn inverse(span: usize) -> u16 {
    65536_u32.div_ceil(span as u32) as u16
}

const LOW: u32 = 0x00ff_00ff;

#[inline(always)]
fn down4_row(d: &mut [Px], r0: &[u32], r1: &[u32], r2: &[u32], r3: &[u32]) {
    let (r0, r1) = (r0.as_chunks::<4>().0, r1.as_chunks::<4>().0);
    let (r2, r3) = (r2.as_chunks::<4>().0, r3.as_chunks::<4>().0);
    let rows = r0.iter().zip(r1).zip(r2.iter().zip(r3));
    for (o, ((a, b), (c, e))) in d.iter_mut().zip(rows) {
        let (mut rb, mut ag) = (0x0008_0008, 0x0008_0008);
        for i in 0..4 {
            for p in [a[i], b[i], c[i], e[i]] {
                rb += p & LOW;
                ag += (p >> 8) & LOW;
            }
        }
        *o = (((rb >> 4) & LOW) | (((ag >> 4) & LOW) << 8)).to_ne_bytes();
    }
}

/// `d[i]` = the mean of `s[i..=i + 2r]`, so `s` is `2r` longer than `d`; `r` > 0.
#[inline(always)]
fn box_h_row(d: &mut [Px], s: &[Px], r: usize) {
    let span = 2 * r + 1;
    let inv = inverse(span) as u32;
    let mut sum = [r as u16; 4];
    for p in &s[..span - 1] {
        sum = from_fn(|c| sum[c] + p[c] as u16);
    }
    for ((o, add), sub) in d.iter_mut().zip(&s[span - 1..]).zip(s) {
        sum = from_fn(|c| sum[c] + add[c] as u16);
        *o = from_fn(|c| ((sum[c] as u32 * inv) >> 16) as u8);
        sum = from_fn(|c| sum[c] - sub[c] as u16);
    }
}

/// One row of a pass down: `sum` gains `add`, gives `d` its mean and loses `sub`.
#[inline(always)]
fn box_v_row(d: &mut [Px], sum: &mut [[u16; 4]], add: &[Px], sub: &[Px], inv: u16) {
    let inv = inv as u32;
    for (((o, sum), add), sub) in d.iter_mut().zip(sum).zip(add).zip(sub) {
        let s: [u16; 4] = from_fn(|c| sum[c] + add[c] as u16);
        *o = from_fn(|c| ((s[c] as u32 * inv) >> 16) as u8);
        *sum = from_fn(|c| s[c] - sub[c] as u16);
    }
}

/// `a` to `b` at `f`/8 of the way, per channel, in eighths.
#[inline(always)]
fn lerp8(a: &Px, b: &Px, f: u16) -> [u16; 4] {
    from_fn(|c| a[c] as u16 * (8 - f) + b[c] as u16 * f)
}

/// `a` to `b`, both in eighths, at `f`/8 of the way: a pixel, rounded.
#[inline(always)]
fn settle(a: &[u16; 4], b: &[u16; 4], f: u16) -> u32 {
    u32::from_ne_bytes(from_fn(|c| ((a[c] * (8 - f) + b[c] * f + 32) >> 6) as u8))
}

/// The bilinear lookup of one pixel, between cells `t0`, `t1` above and `b0`, `b1` below, in eighths of a cell.
pub(crate) fn bilerp_px(t0: &Px, t1: &Px, b0: &Px, b1: &Px, fx: u16, fy: u16) -> u32 {
    settle(&lerp8(t0, b0, fy), &lerp8(t1, b1, fy), fx)
}

/// Cells at a time, on the stack.
const BLOCK: usize = 64;

/// Four pixels per pair of neighbouring cells, at 1/8, 3/8, 5/8 and 7/8 from the first to the second: the phases of a
/// ¼-resolution image's cell centres. `t` and `b` hold one cell more than `o` has quarters.
#[inline(always)]
fn bilerp_row(o: &mut [u32], t: &[Px], b: &[Px], fy: u16) {
    for (i, o) in o.chunks_mut(4 * BLOCK).enumerate() {
        let cells = i * BLOCK..i * BLOCK + o.len() / 4 + 1;
        let mut v = [[0; 4]; BLOCK + 1];
        for ((v, t), b) in v.iter_mut().zip(&t[cells.clone()]).zip(&b[cells]) {
            *v = lerp8(t, b, fy);
        }
        for (o, v) in o.as_chunks_mut::<4>().0.iter_mut().zip(v.windows(2)) {
            let (a, c) = (&v[0], &v[1]);
            *o = [
                settle(a, c, 1),
                settle(a, c, 3),
                settle(a, c, 5),
                settle(a, c, 7),
            ];
        }
    }
}

row_kernel!(down4_base, down4_avx2, down4_row, (d: &mut [Px], r0: &[u32], r1: &[u32], r2: &[u32], r3: &[u32]));
row_kernel!(box_h_base, box_h_avx2, box_h_row, (d: &mut [Px], s: &[Px], r: usize));
row_kernel!(box_v_base, box_v_avx2, box_v_row, (d: &mut [Px], sum: &mut [[u16; 4]], add: &[Px], sub: &[Px], inv: u16));
row_kernel!(bilerp_base, bilerp_avx2, bilerp_row, (o: &mut [u32], t: &[Px], b: &[Px], fy: u16));

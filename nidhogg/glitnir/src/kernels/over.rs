//! OVER on premultiplied pixels with pixman's exact arithmetic: x·a/255 rounded, two channels per 32-bit multiply
//! (`UN8x4_MUL_UN8`). The loops have no branches, so the compiler vectorises them (SSE2, AVX2, NEON).
//!
//! Input is premultiplied (no colour channel above the alpha), as every Cairo image surface is; then no channel sum
//! exceeds 255 and the plain add equals pixman's saturating one.

use super::row_kernel;

/// x·a/255 rounded, for x, a ≤ 255.
#[inline(always)]
pub(crate) fn mul_un8(x: u32, a: u32) -> u32 {
    let t = x * a + 0x80;
    ((t >> 8) + t) >> 8
}

/// Each of the four channels of `p` times `a`/255, rounded.
#[inline(always)]
pub(crate) fn mul_un8x4(p: u32, a: u32) -> u32 {
    let mut rb = (p & 0x00ff_00ff) * a + 0x0080_0080;
    rb = ((rb + ((rb >> 8) & 0x00ff_00ff)) >> 8) & 0x00ff_00ff;
    let mut ag = ((p >> 8) & 0x00ff_00ff) * a + 0x0080_0080;
    ag = (ag + ((ag >> 8) & 0x00ff_00ff)) & 0xff00_ff00;
    rb | ag
}

/// `s` OVER `d`.
#[inline(always)]
fn over_px(d: u32, s: u32) -> u32 {
    s.wrapping_add(mul_un8x4(d, 255 - (s >> 24)))
}

#[inline(always)]
fn over_row(d: &mut [u32], s: &[u32]) {
    for (dp, &sp) in d.iter_mut().zip(s) {
        *dp = over_px(*dp, sp);
    }
}

#[inline(always)]
fn over_const_row(d: &mut [u32], s: &[u32], alpha: u8) {
    let a = alpha as u32;
    for (dp, &sp) in d.iter_mut().zip(s) {
        *dp = over_px(*dp, mul_un8x4(sp, a));
    }
}

#[inline(always)]
fn mask_over_row(d: &mut [u32], s: &[u32], m: &[u8], alpha: u8) {
    let a = alpha as u32;
    for ((dp, &sp), &mp) in d.iter_mut().zip(s).zip(m) {
        *dp = over_px(*dp, mul_un8x4(sp, mul_un8(mp as u32, a)));
    }
}

#[inline(always)]
fn solid_over_row(d: &mut [u32], colour: u32, m: &[u8]) {
    for (dp, &mp) in d.iter_mut().zip(m) {
        *dp = over_px(*dp, mul_un8x4(colour, mp as u32));
    }
}

row_kernel!(over_base, over_avx2, over_row, (d: &mut [u32], s: &[u32]));
row_kernel!(over_const_base, over_const_avx2, over_const_row, (d: &mut [u32], s: &[u32], alpha: u8));
row_kernel!(mask_over_base, mask_over_avx2, mask_over_row, (d: &mut [u32], s: &[u32], m: &[u8], alpha: u8));
row_kernel!(solid_over_base, solid_over_avx2, solid_over_row, (d: &mut [u32], colour: u32, m: &[u8]));

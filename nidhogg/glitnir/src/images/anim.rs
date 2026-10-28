//! The state an animation keeps between decodes, owned by the host.
//!
//! Frame N of a GIF or an animated WebP is composed on the canvas of frame N-1. The crate keeps nothing between
//! calls: a decode hands the host an [`Anim`] (`out.anim`) holding the canvas, the frame decoded and what the
//! format needs to go on, and the host may hand it back with the next request for the same image (`params.anim`).
//! The decode then owns it again: it continues from it, or drops it, and returns it (or a new one) in `out.anim`.

use super::gif::GifAnim;
use super::webp::WebpAnim;

/// A format's animation state.
pub enum Anim {
    Gif(GifAnim),
    Webp(WebpAnim),
}

impl Anim {
    /// The bytes it holds (for the host's cache bounds).
    pub fn bytes(&self) -> usize {
        match self {
            Anim::Gif(g) => g.bytes(),
            Anim::Webp(w) => w.bytes(),
        }
    }
}

/// libwebp's `BlendPixelNonPremult` (`anim_decode.c`): SRC over DST, both non-premultiplied RGBA as libwebp
/// stores them (`[r, g, b, a]`), with its integer approximations.
pub fn blend_non_premult(src: [u8; 4], dst: [u8; 4]) -> [u8; 4] {
    let src_a = u32::from(src[3]);
    if src_a == 0 {
        return dst;
    }
    let dst_a = u32::from(dst[3]);
    // dst_factor_a = (dst_a * (255 - src_a)) / 255, approximately
    let dst_factor_a = ((dst_a * (256 - src_a)) >> 8) & 0xff;
    let blend_a = (src_a + dst_factor_a) & 0xff;
    let scale = (1u32 << 24) / blend_a;
    let ch = |k: usize| -> u8 {
        let unscaled = u32::from(src[k]) * src_a + u32::from(dst[k]) * dst_factor_a;
        ((unscaled * scale) >> 24) as u8
    };
    [ch(0), ch(1), ch(2), blend_a as u8]
}

/// libwebp's `BlendPixelRowNonPremult`: every pixel of SRC that is not opaque blends over DST's.
pub fn blend_row(src: &mut [[u8; 4]], dst: &[[u8; 4]]) {
    for (s, d) in src.iter_mut().zip(dst) {
        if s[3] != 0xff {
            *s = blend_non_premult(*s, *d);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// libwebp's formula, transcribed with its C types (uint8_t truncations), for the exhaustive comparison.
    fn libwebp(src: [u8; 4], dst: [u8; 4]) -> [u8; 4] {
        let src_a: u8 = src[3];
        if src_a == 0 {
            return dst;
        }
        let dst_a: u8 = dst[3];
        let dst_factor_a: u8 = ((u32::from(dst_a) * (256 - u32::from(src_a))) >> 8) as u8;
        let blend_a: u8 = src_a.wrapping_add(dst_factor_a);
        let scale: u32 = (1u32 << 24) / u32::from(blend_a);
        let ch = |k: usize| -> u8 {
            let blend_unscaled: u32 =
                u32::from(src[k]) * u32::from(src_a) + u32::from(dst[k]) * u32::from(dst_factor_a);
            (blend_unscaled.wrapping_mul(scale) >> 24) as u8
        };
        [ch(0), ch(1), ch(2), blend_a]
    }

    #[test]
    fn the_blend_matches_libwebp() {
        // every alpha pair, over a spread of channel values
        let vals = [0u8, 1, 2, 63, 64, 127, 128, 129, 200, 254, 255];
        for sa in 0..=255u8 {
            for da in 0..=255u8 {
                for &s in &vals {
                    for &d in &vals {
                        let src = [s, d, s ^ 0x55, sa];
                        let dst = [d, s, d ^ 0xaa, da];
                        assert_eq!(
                            blend_non_premult(src, dst),
                            libwebp(src, dst),
                            "{src:?} over {dst:?}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn known_blends() {
        // fully transparent source: the destination; opaque destination stays opaque
        assert_eq!(blend_non_premult([9, 9, 9, 0], [1, 2, 3, 4]), [1, 2, 3, 4]);
        assert_eq!(
            blend_non_premult([255, 0, 0, 128], [0, 0, 255, 255])[3],
            255
        );
        // libwebp's integer scale loses one on an opaque source (255 * 255 * (2^24 / 255) >> 24 = 254); its row
        // blend never blends an opaque pixel, so that never shows (`blend_row` below)
        assert_eq!(
            blend_non_premult([255, 0, 0, 255], [0, 0, 255, 255]),
            [254, 0, 0, 255]
        );
        // over a transparent destination the source keeps its colour
        assert_eq!(
            blend_non_premult([200, 100, 50, 77], [0, 0, 0, 0]),
            [199, 99, 49, 77]
        );
        let mut row = [[10, 20, 30, 255], [10, 20, 30, 0], [10, 20, 30, 128]];
        blend_row(&mut row, &[[1, 1, 1, 255]; 3]);
        assert_eq!(row[0], [10, 20, 30, 255]);
        assert_eq!(row[1], [1, 1, 1, 255]);
    }
}

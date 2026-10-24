//! pixman as the test oracle (feature `pixman-oracle`, Linux, tests only; `build.rs` links the system's
//! `pixman-1`): `pixman_image_composite32` with OVER, which the kernels must equal bit for bit.

use std::ffi::c_void;
use std::ptr::null_mut;

#[repr(C)]
struct PixmanColor {
    red: u16,
    green: u16,
    blue: u16,
    alpha: u16,
}

type Image = *mut c_void;

unsafe extern "C" {
    fn pixman_image_create_bits(
        format: u32,
        width: i32,
        height: i32,
        bits: *mut u32,
        stride: i32,
    ) -> Image;
    fn pixman_image_create_solid_fill(color: *const PixmanColor) -> Image;
    #[allow(clippy::too_many_arguments)]
    fn pixman_image_composite32(
        op: u32,
        src: Image,
        mask: Image,
        dest: Image,
        src_x: i32,
        src_y: i32,
        mask_x: i32,
        mask_y: i32,
        dest_x: i32,
        dest_y: i32,
        width: i32,
        height: i32,
    );
    fn pixman_image_unref(image: Image) -> i32;
}

const A8R8G8B8: u32 = 0x2002_8888;
const A8: u32 = 0x0801_8000;
const OP_OVER: u32 = 3;

/// The source: an image (pixels, stride in pixels) or one premultiplied colour.
pub(crate) enum Source<'a> {
    Image(&'a [u32], usize),
    Solid(u32),
}

/// The mask: none, one alpha, or an 8-bit image (bytes, stride in bytes).
pub(crate) enum Masking<'a> {
    None,
    Solid(u8),
    A8(&'a [u8], usize),
}

fn solid(argb: u32) -> Image {
    let c = |s: u32| ((argb >> s) & 0xff) as u16 * 257;
    let color = PixmanColor {
        red: c(16),
        green: c(8),
        blue: c(0),
        alpha: c(24),
    };
    // SAFETY: `color` is a valid pixman_color_t for the call.
    unsafe { pixman_image_create_solid_fill(&color) }
}

/// `src` (through `mask`) OVER `dst`, `w`×`h` pixels, by pixman.
pub(crate) fn over(
    dst: &mut [u32],
    dst_stride: usize,
    src: Source,
    mask: Masking,
    w: usize,
    h: usize,
) {
    assert!(dst.len() >= (h - 1) * dst_stride + w);
    // pixman wants its own aligned copies: 4-byte rows for the mask
    let mut src_px;
    let mut mask_px;
    // SAFETY: every image wraps a buffer that outlives it (all are unreferenced below) and is large enough for
    // `w`×`h` at its stride.
    unsafe {
        let d = pixman_image_create_bits(
            A8R8G8B8,
            w as i32,
            h as i32,
            dst.as_mut_ptr(),
            (dst_stride * 4) as i32,
        );
        let s = match src {
            Source::Image(px, stride) => {
                src_px = px.to_vec();
                pixman_image_create_bits(
                    A8R8G8B8,
                    w as i32,
                    h as i32,
                    src_px.as_mut_ptr(),
                    (stride * 4) as i32,
                )
            }
            Source::Solid(argb) => solid(argb),
        };
        let m = match mask {
            Masking::None => null_mut(),
            Masking::Solid(a) => solid((a as u32) << 24),
            Masking::A8(bytes, stride) => {
                let words = w.div_ceil(4);
                mask_px = vec![0u32; words * h];
                let dst_bytes =
                    std::slice::from_raw_parts_mut(mask_px.as_mut_ptr() as *mut u8, words * 4 * h);
                for y in 0..h {
                    dst_bytes[y * words * 4..][..w].copy_from_slice(&bytes[y * stride..][..w]);
                }
                pixman_image_create_bits(
                    A8,
                    w as i32,
                    h as i32,
                    mask_px.as_mut_ptr(),
                    (words * 4) as i32,
                )
            }
        };
        pixman_image_composite32(OP_OVER, s, m, d, 0, 0, 0, 0, 0, 0, w as i32, h as i32);
        if !m.is_null() {
            pixman_image_unref(m);
        }
        pixman_image_unref(s);
        pixman_image_unref(d);
    }
}

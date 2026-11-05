//! Arbitrary bytes as a PNG, GIF or WebP through `glitnir_img_decode`, the entry the host calls on files it did not
//! write.  The first byte picks the format, the size limit and (when its top bit is clear) whether the format's
//! signature is put in front of the rest, so the fuzzer starts past the magic number; the next bytes pick the
//! parameters.  Animations are decoded again from the state the first decode handed back, as the host does for the
//! next frame.  A panic (`GLITNIR_E_PANIC`), a broken contract or a leak of a result on failure is a crash.
#![no_main]

use glitnir::ffi::{GLITNIR_E_PANIC, GLITNIR_OK};
use glitnir::images::{
    GLITNIR_IMG_GIF, GLITNIR_IMG_PNG, GLITNIR_IMG_WEBP, Out, Params, glitnir_img_anim_free,
    glitnir_img_decode, glitnir_img_free,
};
use libfuzzer_sys::fuzz_target;

const PNG: &[u8] = &[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
const GIF: &[u8] = b"GIF89a";
const WEBP: &[u8] = b"RIFF\0\0\0\0WEBP";

fuzz_target!(|data: &[u8]| {
    let Some((&sel, rest)) = data.split_first() else {
        return;
    };
    let Some((knobs, rest)) = rest.split_at_checked(5) else {
        return;
    };
    let (format, sig) = match sel % 3 {
        0 => (GLITNIR_IMG_PNG, PNG),
        1 => (GLITNIR_IMG_GIF, GIF),
        _ => (GLITNIR_IMG_WEBP, WEBP),
    };
    let mut file = Vec::with_capacity(sig.len() + rest.len());
    if sel & 0x80 == 0 {
        file.extend_from_slice(sig);
    }
    file.extend_from_slice(rest);
    // 64, 1024 or 8192 on a side: the last is as large as a decode may go (2^26 pixels) without the run's memory
    // limit counting a lazily zeroed allocation
    let limit: i64 = [64, 1024, 8192][usize::from((sel >> 2) & 3).min(2)];
    let mut p = Params {
        limit_w: limit,
        limit_h: limit,
        bg_set: knobs[0] & 1,
        bg_r: u16::from(knobs[0]) << 8,
        bg_g: u16::from(knobs[1]) << 8,
        bg_b: u16::from(knobs[2]) << 8,
        frame_index: i32::from(knobs[3] % 6) - 1,
        frame_bg: u32::from(knobs[4]) * 0x0001_0101,
        gif_bg_set: (knobs[0] >> 1) & 1,
        gif_bg: u32::from(knobs[1]) * 0x0001_0101,
        keep: (knobs[0] >> 2) & 1,
        anim: std::ptr::null_mut(),
    };
    let mut anim = std::ptr::null_mut();
    // up to three decodes of the same file: the second and third continue from the state of the one before
    for round in 0..3 {
        p.anim = anim;
        anim = std::ptr::null_mut();
        // SAFETY: Out is integers, pointers and arrays of them; all-zero is a valid value.
        let mut out: Out = unsafe { std::mem::zeroed() };
        // SAFETY: FILE and P outlive the call and OUT is writable; the state in P.anim came from the last decode.
        let r = unsafe { glitnir_img_decode(format, file.as_ptr(), file.len(), &p, &mut out) };
        assert_ne!(r, GLITNIR_E_PANIC, "a decode panicked");
        assert_eq!(r, out.err, "the return value and out.err differ");
        if r == GLITNIR_OK {
            assert!(!out.pixels.is_null(), "no pixels on success");
            assert!(
                out.width > 0 && out.height > 0 && i64::from(out.width) <= limit && i64::from(out.height) <= limit,
                "size {}x{} against limit {limit}",
                out.width,
                out.height
            );
            assert!(out.anim.is_null() || p.keep != 0, "anim state handed out unasked");
            anim = out.anim;
        } else {
            assert!(
                out.pixels.is_null() && out.mask.is_null() && out.ext.is_null() && out.anim.is_null(),
                "a result left behind on error {r}"
            );
        }
        // SAFETY: OUT was filled by the decode and is freed once.
        unsafe { glitnir_img_free(&mut out) };
        if anim.is_null() {
            break;
        }
        p.frame_index = p.frame_index.saturating_add(i32::from(round) + 1).rem_euclid(6) - 1;
    }
    // SAFETY: a state from the last decode, not handed back since; NULL is allowed.
    unsafe { glitnir_img_anim_free(anim) };
});

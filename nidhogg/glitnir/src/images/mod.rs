//! Image decoders behind a stateless C ABI (feature `images`): `glitnir_img_decode` turns encoded bytes into
//! 0x00RRGGBB pixels and an optional 1-bit mask, as a host built on the C libraries would, and `glitnir_img_free`
//! releases them.
//!
//! The decoders run on whatever thread calls them (the host's image worker): they take no lock, keep no global
//! state, start no thread and call nothing back. Every allocation they make for the result is bounded by the
//! image's size, which is checked against the caller's limits and [`MAX_PIXELS`] before anything is allocated;
//! the result is allocated with the C library's `calloc`, so the host frees it with `free`. A panic is caught at
//! the boundary and is a decode error.
//!
//! An animation (GIF, WebP) is composed frame by frame on a canvas. The crate keeps no state between calls: with
//! `params.keep` a decode hands the canvas and what the format needs to go on to the host (`out.anim`, an opaque
//! [`anim::Anim`]), and the host may hand it back with the next request for the same image (`params.anim`). The
//! decode then owns it: it continues from it, or drops it.

pub mod anim;
pub mod composite;
pub mod gif;
pub mod png;
pub mod webp;

use crate::ffi::{GLITNIR_E_ARG, GLITNIR_E_PANIC, GLITNIR_OK};
use std::ffi::c_void;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::ptr::NonNull;

/// The formats.
pub const GLITNIR_IMG_PNG: u32 = 0;
pub const GLITNIR_IMG_GIF: u32 = 1;
pub const GLITNIR_IMG_WEBP: u32 = 2;

/// The data is not of the format (PNG: the signature is wrong).
pub const GLITNIR_IMG_E_NOT_IMAGE: i32 = 1;
/// The data is malformed or truncated; `errmsg` says how.
pub const GLITNIR_IMG_E_DECODE: i32 = 2;
/// The image is larger than the limits allow.
pub const GLITNIR_IMG_E_SIZE: i32 = 3;
/// The result could not be allocated.
pub const GLITNIR_IMG_E_NOMEM: i32 = 4;
/// The host's own decoder must decode this image (PNG: libpng would composite it through gamma tables).
pub const GLITNIR_IMG_E_HOST: i32 = 5;
/// No decoder for this format.
pub const GLITNIR_IMG_E_FORMAT: i32 = 6;
/// GIF: a frame does not fit in the screen.
pub const GLITNIR_IMG_E_SUBIMAGE: i32 = 7;
/// GIF: no frame `err_arg`.
pub const GLITNIR_IMG_E_INDEX: i32 = 8;
/// WebP: frame `err_arg` of an animation does not decode.
pub const GLITNIR_IMG_E_FRAME: i32 = 9;
/// WebP: the animation's container does not parse.
pub const GLITNIR_IMG_E_PARSE: i32 = 10;

/// The pixels the decoders refuse whatever the limits say (512 MiB of pixels).
pub const MAX_PIXELS: i64 = 1 << 27;

/// The frames an animation may have.  A frame costs its bookkeeping (about a hundred bytes) whatever its size, and a
/// file can spell one in a dozen bytes; past this many the decode fails as an allocation would (`GLITNIR_IMG_E_NOMEM`).
pub const MAX_FRAMES: usize = 1 << 18;

/// What the decode needs from the host, resolved by the host beforehand.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct Params {
    /// The largest width and height; negative: no limit.
    pub limit_w: i64,
    pub limit_h: i64,
    /// Non-zero: a real alpha channel is composited over `bg_*` (16 bits per channel).
    pub bg_set: u8,
    pub bg_r: u16,
    pub bg_g: u16,
    pub bg_b: u16,
    /// GIF, WebP: the frame wanted; negative: none (the first).
    pub frame_index: i32,
    /// GIF: 0x00RRGGBB outside the first frame, and for transparency unless `gif_bg_set`.
    pub frame_bg: u32,
    pub gif_bg_set: u8,
    pub gif_bg: u32,
    /// Non-zero: hand the animation state back (`Out::anim`).
    pub keep: u8,
    /// An earlier decode's state for the same data, or NULL; the decode owns it.
    pub anim: *mut c_void,
}

impl Default for Params {
    /// No limits, no background, no frame.
    fn default() -> Self {
        Params {
            limit_w: -1,
            limit_h: -1,
            bg_set: 0,
            bg_r: 0,
            bg_g: 0,
            bg_b: 0,
            frame_index: -1,
            frame_bg: 0,
            gif_bg_set: 0,
            gif_bg: 0,
            keep: 0,
            anim: std::ptr::null_mut(),
        }
    }
}

/// The result. The caller zeroes nothing: the decode fills every field.
#[repr(C)]
#[derive(Debug)]
pub struct Out {
    /// `width * height` words 0x00RRGGBB, from `calloc`.
    pub pixels: *mut u32,
    /// `width * height` bytes, 255 draw and 0 transparent, from `calloc`; NULL when the image has no mask.
    pub mask: *mut u8,
    pub width: i32,
    pub height: i32,
    /// PNG: the `bKGD` chunk's red, green and blue, as libpng returns them.
    pub bkgd_valid: u8,
    pub bkgd: [u16; 3],
    /// The error (`GLITNIR_IMG_E_*`, or `GLITNIR_E_PANIC`), its frame for `_INDEX` and `_FRAME`, and its text.
    pub err: i32,
    pub err_arg: i32,
    pub errmsg: [u8; 128],
    /// GIF, WebP animation: the frames; 0 otherwise.
    pub frame_count: i32,
    /// WebP: an animation.
    pub animated: u8,
    /// GIF: the frame has extension blocks (`delay` from its graphic control block); WebP animation: always.
    pub has_delay: u8,
    pub delay: i32,
    pub has_corners: u8,
    /// Top, left, bottom, right.
    pub corners: [i32; 4],
    /// GIF: the frame's extension blocks as giflib keeps them (a function byte, a length byte and that many bytes
    /// each; function 0 continues the block before), from `calloc`; NULL with `ext_len` 0 when none.
    pub ext: *mut u8,
    pub ext_len: usize,
    /// With `Params::keep`: the state for the next frame (an `anim::Anim`), or NULL; and the memory it holds.
    pub anim: *mut c_void,
    pub anim_bytes: usize,
}

impl Out {
    fn empty() -> Self {
        Out {
            pixels: std::ptr::null_mut(),
            mask: std::ptr::null_mut(),
            width: 0,
            height: 0,
            bkgd_valid: 0,
            bkgd: [0; 3],
            err: 0,
            err_arg: 0,
            errmsg: [0; 128],
            frame_count: 0,
            animated: 0,
            has_delay: 0,
            delay: 0,
            has_corners: 0,
            corners: [0; 4],
            ext: std::ptr::null_mut(),
            ext_len: 0,
            anim: std::ptr::null_mut(),
            anim_bytes: 0,
        }
    }
}

/// Why a decode failed.
#[derive(Debug)]
pub enum Error {
    NotImage,
    /// Not of the format, with the host library's text.
    NotFormat(&'static str),
    Decode(String),
    Size,
    NoMem,
    Host,
    Subimage,
    Index(i32),
    Frame(i32, String),
    Parse(String),
}

impl Error {
    fn code(&self) -> i32 {
        match self {
            Error::NotImage | Error::NotFormat(_) => GLITNIR_IMG_E_NOT_IMAGE,
            Error::Decode(_) => GLITNIR_IMG_E_DECODE,
            Error::Size => GLITNIR_IMG_E_SIZE,
            Error::NoMem => GLITNIR_IMG_E_NOMEM,
            Error::Host => GLITNIR_IMG_E_HOST,
            Error::Subimage => GLITNIR_IMG_E_SUBIMAGE,
            Error::Index(_) => GLITNIR_IMG_E_INDEX,
            Error::Frame(..) => GLITNIR_IMG_E_FRAME,
            Error::Parse(_) => GLITNIR_IMG_E_PARSE,
        }
    }

    fn arg(&self) -> i32 {
        match self {
            Error::Index(i) | Error::Frame(i, _) => *i,
            _ => 0,
        }
    }

    fn text(&self) -> Option<&str> {
        match self {
            Error::Decode(m) | Error::Frame(_, m) | Error::Parse(m) => Some(m),
            Error::NotFormat(m) => Some(m),
            _ => None,
        }
    }
}

unsafe extern "C" {
    fn calloc(n: usize, size: usize) -> *mut c_void;
    fn free(p: *mut c_void);
}

/// A buffer from the C library's allocator, freed on drop unless handed over.
pub struct CBuf<T: Copy> {
    ptr: NonNull<T>,
    len: usize,
}

impl<T: Copy> CBuf<T> {
    /// LEN zeroed elements (T is an integer type or an array of them).
    pub fn zeroed(len: usize) -> Result<Self, Error> {
        len.checked_mul(size_of::<T>()).ok_or(Error::NoMem)?;
        // SAFETY: calloc checks the product itself; a zero-length request still gets a unique pointer.
        let p = unsafe { calloc(len.max(1), size_of::<T>()) } as *mut T;
        NonNull::new(p)
            .map(|ptr| CBuf { ptr, len })
            .ok_or(Error::NoMem)
    }

    pub fn as_slice(&self) -> &[T] {
        // SAFETY: LEN initialised elements, owned by self.
        unsafe { std::slice::from_raw_parts(self.ptr.as_ptr(), self.len) }
    }

    pub fn as_mut_slice(&mut self) -> &mut [T] {
        // SAFETY: as above, and borrowed mutably.
        unsafe { std::slice::from_raw_parts_mut(self.ptr.as_ptr(), self.len) }
    }

    /// Hand the buffer to the caller, who frees it with `free`.
    pub fn into_raw(self) -> *mut T {
        let p = self.ptr.as_ptr();
        std::mem::forget(self);
        p
    }
}

impl<T: Copy> Drop for CBuf<T> {
    fn drop(&mut self) {
        // SAFETY: allocated by calloc and not handed over.
        unsafe { free(self.ptr.as_ptr() as *mut c_void) }
    }
}

/// A decoded image.
pub struct Image {
    pub pixels: CBuf<u32>,
    pub mask: Option<CBuf<u8>>,
    pub width: i32,
    pub height: i32,
    pub bkgd: Option<[u16; 3]>,
    /// The frames of an animation (GIF: every image); 0: none.
    pub frame_count: i32,
    pub animated: bool,
    pub delay: Option<i32>,
    pub corners: Option<[i32; 4]>,
    /// GIF extension blocks in `Out::ext`'s form.
    pub ext: Option<CBuf<u8>>,
    /// The state to hand back.
    pub anim: Option<Box<anim::Anim>>,
}

impl Image {
    /// A still image's result.
    pub fn still(pixels: CBuf<u32>, width: i32, height: i32) -> Self {
        Image {
            pixels,
            mask: None,
            width,
            height,
            bkgd: None,
            frame_count: 0,
            animated: false,
            delay: None,
            corners: None,
            ext: None,
            anim: None,
        }
    }
}

/// N copies of V in a vector whose allocation failure is an error, not an abort.
pub fn try_vec<T: Clone>(n: usize, v: T) -> Result<Vec<T>, Error> {
    let mut out = Vec::new();
    out.try_reserve_exact(n).map_err(|_| Error::NoMem)?;
    out.resize(n, v);
    Ok(out)
}

/// Whether a W×H image may be decoded under P's limits.
pub fn size_ok(p: &Params, w: i64, h: i64) -> Result<(), Error> {
    let ok = w > 0
        && h > 0
        && w <= i64::from(i32::MAX)
        && h <= i64::from(i32::MAX)
        && (p.limit_w < 0 || w <= p.limit_w)
        && (p.limit_h < 0 || h <= p.limit_h)
        && w * h <= MAX_PIXELS;
    if ok { Ok(()) } else { Err(Error::Size) }
}

fn decode(
    format: u32,
    data: &[u8],
    p: &Params,
    state: Option<Box<anim::Anim>>,
) -> Result<Image, Option<Error>> {
    match format {
        GLITNIR_IMG_PNG => png::decode(data, p).map_err(Some),
        GLITNIR_IMG_GIF => gif::decode(data, p, state).map_err(Some),
        GLITNIR_IMG_WEBP => webp::decode(data, p, state).map_err(Some),
        _ => Err(None),
    }
}

/// Decode LEN bytes at DATA of FORMAT (`GLITNIR_IMG_*`) under PARAMS into OUT.
///
/// Returns `GLITNIR_OK`, a `GLITNIR_IMG_E_*` code (also in `out.err`), `GLITNIR_E_PANIC` (a decode error), or
/// `GLITNIR_E_ARG` when PARAMS or OUT is NULL, or DATA is NULL with a non-zero LEN.
///
/// # Safety
/// DATA must point to LEN readable bytes, PARAMS to a `Params` and OUT to a writable `Out`; none may change during
/// the call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn glitnir_img_decode(
    format: u32,
    data: *const u8,
    len: usize,
    params: *const Params,
    out: *mut Out,
) -> i32 {
    // SAFETY: a non-null anim is a state this ABI handed out (`Out::anim`), now owned by this call.
    let state = (!params.is_null())
        .then(|| unsafe { (*params).anim })
        .filter(|a| !a.is_null())
        .map(|a| unsafe { Box::from_raw(a as *mut anim::Anim) });
    if params.is_null() || out.is_null() || (data.is_null() && len > 0) {
        return GLITNIR_E_ARG;
    }
    // SAFETY: checked non-null; valid per the contract.
    let (p, out) = unsafe { (&*params, &mut *out) };
    *out = Out::empty();
    let bytes: &[u8] = if len == 0 {
        &[]
    } else {
        // SAFETY: LEN readable bytes per the contract.
        unsafe { std::slice::from_raw_parts(data, len) }
    };
    let code = match catch_unwind(AssertUnwindSafe(|| decode(format, bytes, p, state))) {
        Ok(Ok(img)) => {
            out.width = img.width;
            out.height = img.height;
            if let Some(b) = img.bkgd {
                out.bkgd_valid = 1;
                out.bkgd = b;
            }
            out.frame_count = img.frame_count;
            out.animated = u8::from(img.animated);
            if let Some(d) = img.delay {
                out.has_delay = 1;
                out.delay = d;
            }
            if let Some(c) = img.corners {
                out.has_corners = 1;
                out.corners = c;
            }
            if let Some(ext) = img.ext {
                out.ext_len = ext.as_slice().len();
                out.ext = ext.into_raw();
            }
            if let Some(a) = img.anim.filter(|_| p.keep != 0) {
                out.anim_bytes = a.bytes();
                out.anim = Box::into_raw(a) as *mut c_void;
            }
            out.pixels = img.pixels.into_raw();
            out.mask = img.mask.map_or(std::ptr::null_mut(), CBuf::into_raw);
            GLITNIR_OK
        }
        Ok(Err(None)) => GLITNIR_IMG_E_FORMAT,
        Ok(Err(Some(e))) => {
            if let Some(msg) = e.text() {
                let n = msg.len().min(out.errmsg.len() - 1);
                for (d, &s) in out.errmsg.iter_mut().zip(&msg.as_bytes()[..n]) {
                    *d = if s == 0 { b' ' } else { s };
                }
            }
            out.err_arg = e.arg();
            e.code()
        }
        Err(_) => GLITNIR_E_PANIC,
    };
    out.err = code;
    code
}

/// Free what a decode left in OUT (its pixels, mask and extension blocks) and clear them; not its anim. OUT may be
/// NULL.
///
/// # Safety
/// OUT must be NULL or an `Out` filled by `glitnir_img_decode`, not freed since.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn glitnir_img_free(out: *mut Out) {
    if out.is_null() {
        return;
    }
    // SAFETY: per the contract; free takes NULL.
    unsafe {
        free((*out).pixels as *mut c_void);
        free((*out).mask as *mut c_void);
        free((*out).ext as *mut c_void);
        (*out).pixels = std::ptr::null_mut();
        (*out).mask = std::ptr::null_mut();
        (*out).ext = std::ptr::null_mut();
        (*out).ext_len = 0;
    }
}

/// Free an animation state handed out in `Out::anim`. ANIM may be NULL.
///
/// # Safety
/// ANIM must be NULL or a state from `Out::anim`, not handed back or freed since.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn glitnir_img_anim_free(anim: *mut c_void) {
    if !anim.is_null() {
        // SAFETY: per the contract, a Box<Anim> handed out by glitnir_img_decode.
        let state = unsafe { Box::from_raw(anim as *mut anim::Anim) };
        // dropping frees plain memory only; a panic cannot happen, but must not cross the boundary either
        let _ = catch_unwind(AssertUnwindSafe(move || drop(state)));
    }
}

/// The fuzz smoke of the C ABI for one format: mutated SEEDS (bit flips, byte sets, cuts, copies; a fixed random
/// seed, so a failure repeats), with frame indices, backgrounds and animation states handed back at random, never
/// panic, never break the ABI's contract and always free cleanly.
#[cfg(test)]
pub(crate) fn fuzz_abi(format: u32, seeds: &[Vec<u8>], runs: usize) {
    let mut rng = 0x9e37_79b9_7f4a_7c15u64 ^ u64::from(format);
    let mut next = move || {
        rng ^= rng << 13;
        rng ^= rng >> 7;
        rng ^= rng << 17;
        rng
    };
    let mut p = Params {
        limit_w: 64,
        limit_h: 64,
        ..Params::default()
    };
    let mut anim: *mut c_void = std::ptr::null_mut();
    let mut ok = 0;
    for run in 0..runs {
        let mut b = seeds[(next() % seeds.len() as u64) as usize].clone();
        for _ in 0..1 + next() % 6 {
            let n = b.len() as u64;
            if n <= 2 {
                break;
            }
            match next() % 4 {
                0 => b[(next() % n) as usize] ^= 1 << (next() % 8),
                1 => b[(next() % n) as usize] = next() as u8,
                2 => b.truncate(1 + (next() % (n - 1)) as usize),
                _ => {
                    let (x, y) = ((next() % n) as usize, (next() % n) as usize);
                    b[x] = b[y];
                }
            }
        }
        p.bg_set = (run & 1) as u8;
        p.bg_r = next() as u16;
        p.frame_index = (next() % 5) as i32 - 1;
        p.frame_bg = next() as u32 & 0xffffff;
        p.gif_bg_set = (next() % 2) as u8;
        p.keep = (next() % 4 != 0) as u8;
        p.anim = if next() % 2 == 0 {
            std::mem::replace(&mut anim, std::ptr::null_mut())
        } else {
            unsafe { glitnir_img_anim_free(std::mem::replace(&mut anim, std::ptr::null_mut())) };
            std::ptr::null_mut()
        };
        let mut out = Out::empty();
        let r = unsafe { glitnir_img_decode(format, b.as_ptr(), b.len(), &p, &mut out) };
        assert_ne!(r, GLITNIR_E_PANIC, "run {run}: {b:?}");
        assert_eq!(r, out.err);
        if r == GLITNIR_OK {
            ok += 1;
            assert!(!out.pixels.is_null() && out.width > 0 && out.height > 0);
            assert!(out.width <= 64 && out.height <= 64);
            assert!(out.anim.is_null() || p.keep != 0);
            anim = out.anim;
        } else {
            assert!(
                out.pixels.is_null()
                    && out.mask.is_null()
                    && out.ext.is_null()
                    && out.anim.is_null()
            );
        }
        unsafe { glitnir_img_free(&mut out) };
    }
    unsafe { glitnir_img_anim_free(anim) };
    assert!(ok > 0, "no mutation decoded");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decode_c(format: u32, data: &[u8], p: &Params) -> (i32, Out) {
        let mut out = Out::empty();
        let r = unsafe { glitnir_img_decode(format, data.as_ptr(), data.len(), p, &mut out) };
        (r, out)
    }

    #[test]
    fn the_abi_checks_its_arguments() {
        let p = Params::default();
        let mut out = Out::empty();
        assert_eq!(
            unsafe { glitnir_img_decode(GLITNIR_IMG_PNG, std::ptr::null(), 4, &p, &mut out) },
            GLITNIR_E_ARG
        );
        assert_eq!(
            unsafe {
                glitnir_img_decode(
                    GLITNIR_IMG_PNG,
                    std::ptr::null(),
                    0,
                    std::ptr::null(),
                    &mut out,
                )
            },
            GLITNIR_E_ARG
        );
        let (r, out) = decode_c(99, b"x", &p);
        assert_eq!((r, out.err), (GLITNIR_IMG_E_FORMAT, GLITNIR_IMG_E_FORMAT));
        let (r, mut out) = decode_c(GLITNIR_IMG_PNG, b"not a png", &p);
        assert_eq!(r, GLITNIR_IMG_E_NOT_IMAGE);
        assert!(out.pixels.is_null());
        unsafe { glitnir_img_free(&mut out) };
        unsafe { glitnir_img_free(std::ptr::null_mut()) };
    }

    #[test]
    fn a_decode_error_carries_its_text() {
        let mut f = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
        f.extend_from_slice(&[0, 0, 0, 13, b'I', b'H', b'D', b'R']);
        let (r, out) = decode_c(GLITNIR_IMG_PNG, &f, &Params::default());
        assert_eq!(r, GLITNIR_IMG_E_DECODE);
        assert_ne!(out.errmsg[0], 0);
        assert_eq!(*out.errmsg.last().unwrap(), 0);
    }

    #[test]
    fn size_limits() {
        let mut p = Params::default();
        assert!(size_ok(&p, 1 << 13, 1 << 14).is_ok());
        assert!(size_ok(&p, 1 << 14, 1 << 14).is_err());
        assert!(size_ok(&p, 0, 5).is_err());
        p.limit_h = 10;
        assert!(size_ok(&p, 5, 11).is_err());
    }
}

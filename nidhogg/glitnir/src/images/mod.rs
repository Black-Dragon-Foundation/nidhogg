//! Image decoders behind a stateless C ABI (feature `images`): `glitnir_img_decode` turns encoded bytes into
//! 0x00RRGGBB pixels and an optional 1-bit mask, as a host built on the C libraries would, and `glitnir_img_free`
//! releases them.
//!
//! The decoders run on whatever thread calls them (the host's image worker): they take no lock, keep no global
//! state, start no thread and call nothing back. Every allocation they make for the result is bounded by the
//! image's size, which is checked against the caller's limits and [`MAX_PIXELS`] before anything is allocated;
//! the result is allocated with the C library's `calloc`, so the host frees it with `free`. A panic is caught at
//! the boundary and is a decode error.

pub mod composite;
pub mod png;

use crate::ffi::{GLITNIR_E_ARG, GLITNIR_E_PANIC, GLITNIR_OK};
use std::ffi::c_void;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::ptr::NonNull;

/// The formats.
pub const GLITNIR_IMG_PNG: u32 = 0;

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

/// The pixels the decoders refuse whatever the limits say (512 MiB of pixels).
pub const MAX_PIXELS: i64 = 1 << 27;

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
}

impl Default for Params {
    /// No limits, no background.
    fn default() -> Self {
        Params {
            limit_w: -1,
            limit_h: -1,
            bg_set: 0,
            bg_r: 0,
            bg_g: 0,
            bg_b: 0,
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
    /// The error (`GLITNIR_IMG_E_*`, or `GLITNIR_E_PANIC`), and its text for `GLITNIR_IMG_E_DECODE`.
    pub err: i32,
    pub errmsg: [u8; 128],
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
            errmsg: [0; 128],
        }
    }
}

/// Why a decode failed.
#[derive(Debug)]
pub enum Error {
    NotImage,
    Decode(String),
    Size,
    NoMem,
    Host,
}

impl Error {
    fn code(&self) -> i32 {
        match self {
            Error::NotImage => GLITNIR_IMG_E_NOT_IMAGE,
            Error::Decode(_) => GLITNIR_IMG_E_DECODE,
            Error::Size => GLITNIR_IMG_E_SIZE,
            Error::NoMem => GLITNIR_IMG_E_NOMEM,
            Error::Host => GLITNIR_IMG_E_HOST,
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
    /// LEN zeroed elements (T is an integer type).
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

fn decode(format: u32, data: &[u8], p: &Params) -> Result<Image, Option<Error>> {
    match format {
        GLITNIR_IMG_PNG => png::decode(data, p).map_err(Some),
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
    let code = match catch_unwind(AssertUnwindSafe(|| decode(format, bytes, p))) {
        Ok(Ok(img)) => {
            out.width = img.width;
            out.height = img.height;
            if let Some(b) = img.bkgd {
                out.bkgd_valid = 1;
                out.bkgd = b;
            }
            out.pixels = img.pixels.into_raw();
            out.mask = img.mask.map_or(std::ptr::null_mut(), CBuf::into_raw);
            GLITNIR_OK
        }
        Ok(Err(None)) => GLITNIR_IMG_E_FORMAT,
        Ok(Err(Some(e))) => {
            if let Error::Decode(msg) = &e {
                let n = msg.len().min(out.errmsg.len() - 1);
                for (d, &s) in out.errmsg.iter_mut().zip(&msg.as_bytes()[..n]) {
                    *d = if s == 0 { b' ' } else { s };
                }
            }
            e.code()
        }
        Err(_) => GLITNIR_E_PANIC,
    };
    out.err = code;
    code
}

/// Free what a decode left in OUT (its pixels and mask) and clear them. OUT may be NULL.
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
        (*out).pixels = std::ptr::null_mut();
        (*out).mask = std::ptr::null_mut();
    }
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

//! Images (design §5.5): ImageIO decodes PNG, JPEG, GIF (each frame), TIFF, WebP and HEIC from the OS, and a drawn
//! image is wrapped once as a `CGImage`, kept until its pixels are freed.  Only Core Graphics' and ImageIO's C functions
//! are used, declared here.

use std::collections::HashMap;
use std::ffi::c_void;
use std::sync::{Mutex, OnceLock};

type CFRef = *const c_void;

#[repr(C)]
#[derive(Clone, Copy)]
struct CGRect {
    x: f64,
    y: f64,
    w: f64,
    h: f64,
}

/// `kCFNumberDoubleType`.
const CF_DOUBLE: isize = 13;
/// `kCGImageAlphaPremultipliedFirst | kCGBitmapByteOrder32Little`, as `draw.rs`'s canvas.
const BITMAP_INFO: u32 = 2 | (2 << 12);

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFDataCreate(alloc: CFRef, bytes: *const u8, len: isize) -> CFRef;
    fn CFRelease(o: CFRef);
    fn CFDictionaryGetValue(d: CFRef, key: CFRef) -> CFRef;
    fn CFNumberGetValue(n: CFRef, ty: isize, out: *mut c_void) -> bool;
}

#[link(name = "ImageIO", kind = "framework")]
unsafe extern "C" {
    static kCGImagePropertyGIFDictionary: CFRef;
    static kCGImagePropertyGIFUnclampedDelayTime: CFRef;
    static kCGImagePropertyGIFDelayTime: CFRef;
    static kCGImagePropertyPNGDictionary: CFRef;
    static kCGImagePropertyAPNGUnclampedDelayTime: CFRef;
    static kCGImagePropertyWebPDictionary: CFRef;
    static kCGImagePropertyWebPUnclampedDelayTime: CFRef;
    fn CGImageSourceCreateWithData(data: CFRef, options: CFRef) -> CFRef;
    fn CGImageSourceGetCount(src: CFRef) -> usize;
    fn CGImageSourceCreateImageAtIndex(src: CFRef, index: usize, options: CFRef) -> CFRef;
    fn CGImageSourceCopyPropertiesAtIndex(src: CFRef, index: usize, options: CFRef) -> CFRef;
}

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    static kCGColorSpaceSRGB: CFRef;
    fn CGColorSpaceCreateWithName(name: CFRef) -> CFRef;
    fn CGColorSpaceRelease(cs: CFRef);
    fn CGImageGetWidth(img: CFRef) -> usize;
    fn CGImageGetHeight(img: CFRef) -> usize;
    fn CGImageRelease(img: CFRef);
    fn CGBitmapContextCreate(
        data: *mut c_void,
        w: usize,
        h: usize,
        bits: usize,
        row: usize,
        cs: CFRef,
        info: u32,
    ) -> *mut c_void;
    fn CGBitmapContextCreateImage(c: *mut c_void) -> CFRef;
    fn CGContextRelease(c: *mut c_void);
    fn CGContextDrawImage(c: *mut c_void, r: CGRect, img: CFRef);
}

/// One decoded frame: premultiplied ARGB words, rows top down, and the file's frame count and this frame's delay.
#[derive(Debug, Clone, PartialEq)]
pub struct Decoded {
    pub width: i32,
    pub height: i32,
    pub frames: u32,
    /// Seconds; `None` for a still image.
    pub delay: Option<f64>,
    pub argb: Vec<u32>,
}

/// A bitmap context over `px` (`w` x `h` words) in sRGB; null when Core Graphics refuses.
///
/// # Safety
/// `px` holds `w * h` words and outlives the context.
unsafe fn context(px: &mut [u32], w: usize, h: usize) -> *mut c_void {
    // SAFETY: the caller's promise; the colour space is released once the context holds it.
    unsafe {
        let cs = CGColorSpaceCreateWithName(kCGColorSpaceSRGB);
        if cs.is_null() {
            return std::ptr::null_mut();
        }
        let c = CGBitmapContextCreate(px.as_mut_ptr().cast(), w, h, 8, w * 4, cs, BITMAP_INFO);
        CGColorSpaceRelease(cs);
        c
    }
}

/// A frame's delay from its properties (`props`, a dictionary): GIF's, APNG's or WebP's, unclamped first.
///
/// # Safety
/// `props` is a live dictionary.
unsafe fn delay(props: CFRef) -> Option<f64> {
    // SAFETY: lookups in live dictionaries; a missing key gives null.
    unsafe {
        let keys = [
            (
                kCGImagePropertyGIFDictionary,
                kCGImagePropertyGIFUnclampedDelayTime,
            ),
            (kCGImagePropertyGIFDictionary, kCGImagePropertyGIFDelayTime),
            (
                kCGImagePropertyPNGDictionary,
                kCGImagePropertyAPNGUnclampedDelayTime,
            ),
            (
                kCGImagePropertyWebPDictionary,
                kCGImagePropertyWebPUnclampedDelayTime,
            ),
        ];
        for (dict, key) in keys {
            let d = CFDictionaryGetValue(props, dict);
            if d.is_null() {
                continue;
            }
            let n = CFDictionaryGetValue(d, key);
            let mut v = 0.0f64;
            if !n.is_null() && CFNumberGetValue(n, CF_DOUBLE, (&raw mut v).cast()) && v > 0.0 {
                return Some(v);
            }
        }
        None
    }
}

/// Frame `index` of the image in `bytes`, in sRGB; `None` when ImageIO cannot read it or has no such frame.
pub fn decode(bytes: &[u8], index: u32) -> Option<Decoded> {
    let len = isize::try_from(bytes.len()).ok()?;
    // SAFETY: CF and ImageIO calls on live objects; every object made here is released before returning.
    unsafe {
        let data = CFDataCreate(std::ptr::null(), bytes.as_ptr(), len);
        if data.is_null() {
            return None;
        }
        let src = CGImageSourceCreateWithData(data, std::ptr::null());
        CFRelease(data);
        if src.is_null() {
            return None;
        }
        let frames = CGImageSourceGetCount(src);
        let index = index as usize;
        let img = if index < frames {
            CGImageSourceCreateImageAtIndex(src, index, std::ptr::null())
        } else {
            std::ptr::null()
        };
        let props = if img.is_null() {
            std::ptr::null()
        } else {
            CGImageSourceCopyPropertiesAtIndex(src, index, std::ptr::null())
        };
        CFRelease(src);
        if img.is_null() {
            return None;
        }
        let delay = if props.is_null() {
            None
        } else {
            let d = delay(props);
            CFRelease(props);
            d
        };
        let (w, h) = (CGImageGetWidth(img), CGImageGetHeight(img));
        let out = (w > 0 && h > 0 && w <= 1 << 15 && h <= 1 << 15).then(|| {
            let mut argb = vec![0u32; w * h];
            let c = context(&mut argb, w, h);
            if c.is_null() {
                return None;
            }
            // the image upright: its top row is the memory's first
            let r = CGRect {
                x: 0.0,
                y: 0.0,
                w: w as f64,
                h: h as f64,
            };
            CGContextDrawImage(c, r, img);
            CGContextRelease(c);
            Some(Decoded {
                width: w as i32,
                height: h as i32,
                frames: frames as u32,
                delay,
                argb,
            })
        });
        CGImageRelease(img);
        out.flatten()
    }
}

/// A `CGImage` over premultiplied ARGB words it keeps.
struct Picture {
    img: CFRef,
    w: i32,
    h: i32,
}

// SAFETY: a CGImage is immutable and documented thread-safe.
unsafe impl Send for Picture {}

impl Drop for Picture {
    fn drop(&mut self) {
        // SAFETY: the reference this value owns.
        unsafe { CGImageRelease(self.img) };
    }
}

/// The pictures drawn so far, by their key (the pixels' address and size), and the mask address each was made with.
type Pictures = HashMap<(usize, i32, i32), (usize, Picture)>;

fn pictures() -> &'static Mutex<Pictures> {
    static P: OnceLock<Mutex<Pictures>> = OnceLock::new();
    P.get_or_init(Mutex::default)
}

/// The `CGImage` for `key` (pixels at `key.0`, `key.1` x `key.2`) with mask `mask`, made from `make` (premultiplied
/// ARGB, rows top down) the first time; `f` runs on it.  `None` when Core Graphics refuses.
pub fn with_picture<R>(
    key: (usize, i32, i32),
    mask: usize,
    make: impl FnOnce() -> Vec<u32>,
    f: impl FnOnce(*const c_void) -> R,
) -> Option<R> {
    let mut all = pictures().lock().unwrap_or_else(|p| p.into_inner());
    if all.get(&key).is_none_or(|(m, _)| *m != mask) {
        let mut px = make();
        let (w, h) = (key.1 as usize, key.2 as usize);
        if px.len() != w * h {
            return None;
        }
        // SAFETY: `px` holds w * h words and lives until the image is made (the image copies them).
        let img = unsafe {
            let c = context(&mut px, w, h);
            if c.is_null() {
                return None;
            }
            let img = CGBitmapContextCreateImage(c);
            CGContextRelease(c);
            img
        };
        if img.is_null() {
            return None;
        }
        all.insert(
            key,
            (
                mask,
                Picture {
                    img,
                    w: key.1,
                    h: key.2,
                },
            ),
        );
    }
    all.get(&key).map(|(_, p)| {
        debug_assert!(p.w == key.1 && p.h == key.2);
        f(p.img)
    })
}

/// Forget the pictures made from the pixels or the mask at `addr` (they are being freed).
pub fn forget(addr: usize) {
    pictures()
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .retain(|k, (m, _)| k.0 != addr && *m != addr);
}

//! A child's chrome on its layers (design §5.7), set inside its root tree's commit: the content layer's
//! `cornerRadius` with `masksToBounds` and a circular `cornerCurve`, its `borderWidth` and `borderColor`, the ring
//! sublayer (a border alone, on a frame just outside the content's), and the shadow sublayer showing one nine-slice
//! image (`crate::mac_chrome`) stretched over its frame.  A move changes none of it; a resize changes the ring's and
//! the shadow layer's frames alone, unless the child is smaller than the image's stretch minimum (then its image is
//! rendered again at its size).  The container's opacity covers content, ring and shadow alike.

use super::super::layers::{PROPS, count};
use crate::mac_chrome::{self, ShadowImage};
use glitnir::scene::Chrome;
use objc2::encode::{Encoding, RefEncode};
use objc2::msg_send;
use objc2::runtime::AnyObject;
use objc2_foundation::{NSPoint, NSRect, NSSize};
use objc2_quartz_core::{CALayer, kCACornerCurveCircular};
use std::ffi::c_void;

type CFRef = *const c_void;

/// Core Graphics' colour, as `borderColor` takes it.
#[repr(C)]
struct CGColor {
    _p: [u8; 0],
}

// SAFETY: a CGColorRef is a pointer to an opaque `struct CGColor`, encoded as such.
unsafe impl RefEncode for CGColor {
    const ENCODING_REF: Encoding = Encoding::Pointer(&Encoding::Struct("CGColor", &[]));
}

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    static kCGColorSpaceSRGB: CFRef;
    fn CGColorSpaceCreateWithName(name: CFRef) -> CFRef;
    fn CGColorSpaceRelease(cs: CFRef);
    fn CGColorCreateSRGB(r: f64, g: f64, b: f64, a: f64) -> *const CGColor;
    fn CGColorRelease(c: *const CGColor);
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
    pub(super) fn CGImageRelease(img: CFRef);
}

/// Premultiplied alpha first, 32-bit little-endian: an ARGB word as the shell's pixels are.
const ARGB_PREMULTIPLIED: u32 = 2 | (2 << 12);

/// What a child's layers show of its chrome, as last set.
#[derive(Default)]
pub struct Shown {
    chrome: Option<Chrome>,
    scale: f64,
    /// The shadow image's key: the chrome it was made for and the size it was made at (`None`: it stretches).
    image: Option<(Chrome, Option<(u32, u32)>)>,
    frame: Option<(f64, f64, f64, f64)>,
    ring: Option<RingShown>,
}

/// The ring layer's frame (points, from the container's bottom left: x, y, width, height), corner radius and width
/// (points), and colour.
type RingShown = ((f64, f64, f64, f64), f64, f64, u32);

impl Shown {
    /// Whether the shadow image shown stretches (one image for every size); `None` without one.  For tests.
    #[cfg_attr(not(feature = "test-hooks"), allow(dead_code))]
    pub fn stretches(&self) -> Option<bool> {
        self.image.map(|(_, size)| size.is_none())
    }
}

/// One CGImage of `img`'s pixels (+1, the caller releases it); null on failure.
fn cg_image(img: &ShadowImage) -> CFRef {
    argb_image(&img.argb, img.w, img.h)
}

/// One CGImage of `w` x `h` premultiplied ARGB pixels `argb`, row by row (+1, the caller releases it); null on failure.
pub(super) fn argb_image(argb: &[u32], w: u32, h: u32) -> CFRef {
    let mut px = argb.to_vec();
    // SAFETY: the context draws into `px`, alive for the calls; the image copies it; every +1 is released.
    unsafe {
        let cs = CGColorSpaceCreateWithName(kCGColorSpaceSRGB);
        let c = CGBitmapContextCreate(
            px.as_mut_ptr().cast(),
            w as usize,
            h as usize,
            8,
            w as usize * 4,
            cs,
            ARGB_PREMULTIPLIED,
        );
        CGColorSpaceRelease(cs);
        if c.is_null() {
            return std::ptr::null();
        }
        let i = CGBitmapContextCreateImage(c);
        CGContextRelease(c);
        i
    }
}

/// Give `layer`'s border the premultiplied colour `argb`.
fn border_colour(layer: &CALayer, argb: u32) {
    let a = (argb >> 24) as f64 / 255.0;
    let ch = |k: u32| {
        let v = ((argb >> k) & 0xff) as f64 / 255.0;
        if a > 0.0 { (v / a).min(1.0) } else { 0.0 }
    };
    // SAFETY: the colour is made here and released after the layer retained it.
    unsafe {
        let colour = CGColorCreateSRGB(ch(16), ch(8), ch(0), a);
        let () = msg_send![layer, setBorderColor: colour];
        CGColorRelease(colour);
    }
}

/// Set `content`'s corners and border, `ring`'s line just outside it, and `shadow`'s image and frame from `chrome`
/// for a `size` (points) child at `scale`, changing only what differs from `shown`.
pub fn apply(
    shown: &mut Shown,
    chrome: Option<&Chrome>,
    content: &CALayer,
    ring: &CALayer,
    shadow: &CALayer,
    size: (i32, i32),
    scale: f64,
) {
    let c = chrome.copied().unwrap_or_default();
    if shown.chrome != chrome.copied() || shown.scale != scale {
        content.setCornerRadius(f64::from(c.radius_px) / scale);
        content.setMasksToBounds(c.radius_px > 0.0);
        // SAFETY: a constant of the framework.
        content.setCornerCurve(unsafe { kCACornerCurveCircular });
        content.setBorderWidth(f64::from(c.border_px) / scale);
        border_colour(content, c.border_argb);
        shown.chrome = chrome.copied();
        shown.scale = scale;
        count(&PROPS.chrome);
    }
    let dev = |v: i32| (f64::from(v) * scale).round().max(1.0) as u32;
    let (w, h) = (dev(size.0), dev(size.1));
    // the ring: its frame from the content's top left in device pixels, to points from its bottom left
    let r = chrome.and_then(|c| mac_chrome::ring(c, w, h)).map(|r| {
        let (fx, fy, fw, fh) = r.frame;
        (
            (
                fx / scale,
                f64::from(size.1) - (fy + fh) / scale,
                fw / scale,
                fh / scale,
            ),
            r.radius / scale,
            r.width / scale,
            c.ring_argb,
        )
    });
    if shown.ring != r {
        match r {
            None => ring.setHidden(true),
            Some(((x, y, fw, fh), radius, width, argb)) => {
                ring.setFrame(NSRect::new(NSPoint::new(x, y), NSSize::new(fw, fh)));
                ring.setCornerRadius(radius);
                // SAFETY: a constant of the framework.
                ring.setCornerCurve(unsafe { kCACornerCurveCircular });
                ring.setBorderWidth(width);
                border_colour(ring, argb);
                ring.setHidden(false);
            }
        }
        shown.ring = r;
        count(&PROPS.chrome);
    }
    let has = chrome.is_some_and(Chrome::has_shadow);
    if !has {
        if shown.image.take().is_some() || shown.frame.take().is_some() {
            shadow.setHidden(true);
            // SAFETY: nil contents.
            unsafe { shadow.setContents(None) };
        }
        return;
    }
    let key = (c, (!mac_chrome::stretches(&c, w, h)).then_some((w, h)));
    if shown.image != Some(key) || shown.scale != scale {
        let img = mac_chrome::image(&c, w, h, scale);
        let i = cg_image(&img);
        if !i.is_null() {
            let (cx, cy, cw, ch) = img.centre;
            let (iw, ih) = (f64::from(img.w), f64::from(img.h));
            // SAFETY: a CGImage is an object `contents` takes; released once the layer retained it.
            unsafe {
                shadow.setContents(Some(&*i.cast::<AnyObject>()));
                CGImageRelease(i);
            }
            shadow.setContentsScale(scale);
            shadow.setContentsCenter(NSRect::new(
                NSPoint::new(f64::from(cx) / iw, f64::from(cy) / ih),
                NSSize::new(f64::from(cw) / iw, f64::from(ch) / ih),
            ));
            shown.image = Some(key);
            count(&PROPS.shadow_renders);
        }
    }
    let (fx, fy, fw, fh) = mac_chrome::frame(&c, w, h);
    // the frame from the content's top left in device pixels, to points from its bottom left
    let f = (
        fx / scale,
        f64::from(size.1) - (fy + fh) / scale,
        fw / scale,
        fh / scale,
    );
    if shown.frame != Some(f) {
        shadow.setFrame(NSRect::new(NSPoint::new(f.0, f.1), NSSize::new(f.2, f.3)));
        shadow.setHidden(false);
        shown.frame = Some(f);
    }
}

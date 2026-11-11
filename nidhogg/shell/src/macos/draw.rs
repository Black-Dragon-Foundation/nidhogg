//! Shapes painted with Core Graphics (design §5.5): a `CGBitmapContext` over a frame's backing (premultiplied BGRA
//! words, the same bytes the surfaces take), y down, in the colour space `ns-use-srgb-colorspace` names, so a colour
//! is stored without conversion.  Each record is painted as it arrives.  A `CGBitmapContext` needs no main thread,
//! so a Lisp thread that redisplays paints here too (under the global lock); its commit waits for the main thread's
//! pump.  Only Core Graphics' C functions are used, declared here, so the shell takes no further crates.

use crate::shapes::{Paint, Shape};
use glitnir::geom::Rect;
use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

/// `ns-use-srgb-colorspace` as the shell was last told (`MacShell::variable`); a canvas made under the other value
/// is made again.
pub static SRGB: AtomicBool = AtomicBool::new(true);

/// Records painted since start (`(nhmac-stats)`'s `:draws`).
pub static DRAWS: AtomicU64 = AtomicU64::new(0);

/// `ns-antialias-text` and `ns-use-thin-smoothing` as the shell was last told.
pub static ANTIALIAS: AtomicBool = AtomicBool::new(true);
pub static THIN_SMOOTHING: AtomicBool = AtomicBool::new(false);

type CFRef = *const c_void;
type Ctx = *mut c_void;

#[repr(C)]
#[derive(Clone, Copy)]
struct CGRect {
    x: f64,
    y: f64,
    w: f64,
    h: f64,
}

fn cg(r: &Rect) -> CGRect {
    CGRect {
        x: f64::from(r.x),
        y: f64::from(r.y),
        w: f64::from(r.w),
        h: f64::from(r.h),
    }
}

/// `kCGImageAlphaPremultipliedFirst | kCGBitmapByteOrder32Little`: BGRA bytes, the backing's ARGB words.
const BITMAP_INFO: u32 = 2 | (2 << 12);
/// `kCGBlendModeNormal`, `kCGBlendModeCopy`.
const BLEND_NORMAL: i32 = 0;
const BLEND_COPY: i32 = 17;

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    static kCGColorSpaceSRGB: CFRef;
    static kCGColorSpaceGenericRGB: CFRef;
    fn CGColorSpaceCreateWithName(name: CFRef) -> CFRef;
    fn CGColorSpaceRelease(cs: CFRef);
    fn CGBitmapContextCreate(
        data: *mut c_void,
        w: usize,
        h: usize,
        bits: usize,
        row: usize,
        cs: CFRef,
        info: u32,
    ) -> Ctx;
    fn CGContextRelease(c: Ctx);
    fn CGContextSaveGState(c: Ctx);
    fn CGContextRestoreGState(c: Ctx);
    fn CGContextTranslateCTM(c: Ctx, x: f64, y: f64);
    fn CGContextScaleCTM(c: Ctx, x: f64, y: f64);
    fn CGContextClipToRects(c: Ctx, rects: *const CGRect, n: usize);
    fn CGContextClip(c: Ctx);
    fn CGContextSetFillColorSpace(c: Ctx, cs: CFRef);
    fn CGContextSetStrokeColorSpace(c: Ctx, cs: CFRef);
    fn CGContextSetFillColor(c: Ctx, comps: *const f64);
    fn CGContextSetStrokeColor(c: Ctx, comps: *const f64);
    fn CGContextSetBlendMode(c: Ctx, mode: i32);
    fn CGContextFillRect(c: Ctx, r: CGRect);
    fn CGContextStrokeRectWithWidth(c: Ctx, r: CGRect, w: f64);
    fn CGContextSetLineWidth(c: Ctx, w: f64);
    fn CGContextSetLineDash(c: Ctx, phase: f64, lengths: *const f64, n: usize);
    fn CGContextBeginPath(c: Ctx);
    fn CGContextMoveToPoint(c: Ctx, x: f64, y: f64);
    fn CGContextAddLineToPoint(c: Ctx, x: f64, y: f64);
    fn CGContextAddArc(c: Ctx, x: f64, y: f64, r: f64, a0: f64, a1: f64, clockwise: i32);
    fn CGContextClosePath(c: Ctx);
    fn CGContextFillPath(c: Ctx);
    fn CGContextStrokePath(c: Ctx);
    fn CGContextSetTextMatrix(c: Ctx, m: CGAffineTransform);
    fn CGContextSetShouldAntialias(c: Ctx, on: bool);
    fn CGContextSetShouldSmoothFonts(c: Ctx, on: bool);
    /// Private, as `macfont_draw` uses it for `ns-use-thin-smoothing`.
    fn CGContextSetFontSmoothingStyle(c: Ctx, style: i32);
}

#[repr(C)]
struct CGAffineTransform {
    a: f64,
    b: f64,
    c: f64,
    d: f64,
    tx: f64,
    ty: f64,
}

#[repr(C)]
struct CGPoint {
    x: f64,
    y: f64,
}

#[link(name = "CoreText", kind = "framework")]
unsafe extern "C" {
    fn CTFontDrawGlyphs(font: CFRef, glyphs: *const u16, pos: *const CGPoint, n: usize, c: Ctx);
}

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGContextDrawImage(c: Ctx, r: CGRect, img: CFRef);
    fn CGContextConcatCTM(c: Ctx, t: CGAffineTransform);
    fn CGContextSetInterpolationQuality(c: Ctx, q: i32);
}

/// A bitmap context over one backing: made for its pixels, size and colour space, released on drop.
#[derive(Debug)]
pub struct Canvas {
    ctx: Ctx,
    cs: CFRef,
    key: (usize, i32, i32, bool),
}

// SAFETY: the context is used by one thread at a time, under the glue's display lock.
unsafe impl Send for Canvas {}

impl Drop for Canvas {
    fn drop(&mut self) {
        // SAFETY: the references this value owns.
        unsafe {
            CGContextRelease(self.ctx);
            CGColorSpaceRelease(self.cs);
        }
    }
}

impl Canvas {
    /// A context over `px` (`w` x `h` words, rows `w` words apart), y down; `None` when Core Graphics refuses.
    fn new(px: &mut [u32], w: i32, h: i32) -> Option<Canvas> {
        let srgb = SRGB.load(Ordering::Relaxed);
        // SAFETY: CG calls on memory that outlives the canvas (the canvas is remade whenever the backing is).
        unsafe {
            let cs = CGColorSpaceCreateWithName(if srgb {
                kCGColorSpaceSRGB
            } else {
                kCGColorSpaceGenericRGB
            });
            if cs.is_null() {
                return None;
            }
            let ctx = CGBitmapContextCreate(
                px.as_mut_ptr().cast(),
                w as usize,
                h as usize,
                8,
                w as usize * 4,
                cs,
                BITMAP_INFO,
            );
            if ctx.is_null() {
                CGColorSpaceRelease(cs);
                return None;
            }
            CGContextTranslateCTM(ctx, 0.0, f64::from(h));
            CGContextScaleCTM(ctx, 1.0, -1.0);
            CGContextSetFillColorSpace(ctx, cs);
            CGContextSetStrokeColorSpace(ctx, cs);
            Some(Canvas {
                ctx,
                cs,
                key: (px.as_ptr() as usize, w, h, srgb),
            })
        }
    }

    /// The canvas in `slot` for `px`, made again when the backing or the colour space changed.
    pub fn over<'a>(
        slot: &'a mut Option<Canvas>,
        px: &mut [u32],
        w: i32,
        h: i32,
    ) -> Option<&'a mut Canvas> {
        let key = (px.as_ptr() as usize, w, h, SRGB.load(Ordering::Relaxed));
        if slot.as_ref().is_none_or(|c| c.key != key) {
            *slot = None;
            *slot = Canvas::new(px, w, h);
        }
        slot.as_mut()
    }

    /// Paint `p` now.
    pub fn paint(&mut self, p: &Paint) {
        let _t = super::surface::Timed::start(&super::surface::STATS.paint_ns);
        let c = self.ctx;
        if DRAWS.fetch_add(1, Ordering::Relaxed) == 0 {
            crate::trace::note(|| "first draw".into());
        }
        // the pump trace names what was painted (tests read it beside the publishes)
        crate::trace::pump(|| {
            let shapes: Vec<String> = p
                .shapes
                .iter()
                .map(|s| format!("{s:?}").chars().take(80).collect())
                .collect();
            format!("paint: clip {:?}, {shapes:?}", p.clip)
        });
        // SAFETY: CG calls on our context; every pointer passed lives for the call.
        unsafe {
            CGContextSaveGState(c);
            if !p.clip.is_empty() {
                let rs: Vec<CGRect> = p.clip.iter().map(cg).collect();
                CGContextClipToRects(c, rs.as_ptr(), rs.len());
            }
            for s in &p.shapes {
                self.shape(s);
            }
            CGContextRestoreGState(c);
        }
    }

    fn fill_colour(&self, argb: u32) {
        let comps = comps(argb);
        // SAFETY: four components for an RGB space plus alpha.
        unsafe { CGContextSetFillColor(self.ctx, comps.as_ptr()) };
    }

    fn stroke_colour(&self, argb: u32) {
        let comps = comps(argb);
        // SAFETY: as above.
        unsafe { CGContextSetStrokeColor(self.ctx, comps.as_ptr()) };
    }

    fn path(&self, pts: &[(f64, f64)]) {
        let c = self.ctx;
        // SAFETY: CG path calls on our context.
        unsafe {
            CGContextBeginPath(c);
            if let Some((first, rest)) = pts.split_first() {
                CGContextMoveToPoint(c, first.0, first.1);
                for p in rest {
                    CGContextAddLineToPoint(c, p.0, p.1);
                }
            }
        }
    }

    fn shape(&self, s: &Shape) {
        let c = self.ctx;
        // SAFETY: CG calls on our context.
        unsafe {
            match s {
                Shape::Fill { r, argb, copy } => {
                    self.fill_colour(*argb);
                    CGContextSetBlendMode(c, if *copy { BLEND_COPY } else { BLEND_NORMAL });
                    CGContextFillRect(c, cg(r));
                    CGContextSetBlendMode(c, BLEND_NORMAL);
                }
                Shape::Outline { r, argb } => {
                    self.stroke_colour(*argb);
                    let mut o = cg(r);
                    o.x += 0.5;
                    o.y += 0.5;
                    CGContextStrokeRectWithWidth(c, o, 1.0);
                }
                Shape::Polygon { pts, argb } => {
                    self.fill_colour(*argb);
                    self.path(pts);
                    CGContextClosePath(c);
                    CGContextFillPath(c);
                }
                Shape::Path {
                    pts,
                    width,
                    dash,
                    argb,
                } => {
                    self.stroke_colour(*argb);
                    CGContextSetLineWidth(c, *width);
                    if let Some((len, phase)) = dash {
                        CGContextSetLineDash(c, *phase, len, 1);
                    }
                    self.path(pts);
                    CGContextStrokePath(c);
                    CGContextSetLineDash(c, 0.0, std::ptr::null(), 0);
                }
                Shape::Corners {
                    r,
                    radius,
                    margin,
                    corners,
                    argb,
                } => {
                    // nhwl_erase_corners_for_relief: each corner's piece outside its arc, then the box filled there
                    CGContextSaveGState(c);
                    CGContextBeginPath(c);
                    let (x, y, w, h) = (
                        f64::from(r.x),
                        f64::from(r.y),
                        f64::from(r.w),
                        f64::from(r.h),
                    );
                    for i in 0..4u32 {
                        if corners & (1 << i) == 0 {
                            continue;
                        }
                        let left = i == 1 || i == 2;
                        let top = i == 2 || i == 3;
                        let (xm, xc) = if left {
                            (x - margin, x - margin + radius)
                        } else {
                            (x + w + margin, x + w + margin - radius)
                        };
                        let (ym, yc) = if top {
                            (y - margin, y - margin + radius)
                        } else {
                            (y + h + margin, y + h + margin - radius)
                        };
                        let q = std::f64::consts::FRAC_PI_2;
                        CGContextMoveToPoint(c, xm, ym);
                        CGContextAddArc(
                            c,
                            xc,
                            yc,
                            *radius,
                            f64::from(i) * q,
                            f64::from(i + 1) * q,
                            0,
                        );
                        CGContextClosePath(c);
                    }
                    CGContextClip(c);
                    self.fill_colour(*argb);
                    CGContextFillRect(c, cg(r));
                    CGContextRestoreGState(c);
                }
                Shape::Glyphs {
                    font,
                    x,
                    y,
                    glyphs,
                    xs,
                    argb,
                    integral,
                } => {
                    // macfont_draw: y up from the baseline, the text matrix the identity
                    let pos: Vec<CGPoint> = xs.iter().map(|&x| CGPoint { x, y: 0.0 }).collect();
                    self.fill_colour(*argb);
                    super::fonts::with_font(*font, |ct| {
                        CGContextSaveGState(c);
                        CGContextTranslateCTM(c, *x, *y);
                        CGContextScaleCTM(c, 1.0, -1.0);
                        if *integral {
                            CGContextSetShouldAntialias(c, false);
                        }
                        if THIN_SMOOTHING.load(Ordering::Relaxed) {
                            CGContextSetShouldSmoothFonts(c, true);
                            CGContextSetFontSmoothingStyle(c, 16);
                        }
                        CGContextSetTextMatrix(
                            c,
                            CGAffineTransform {
                                a: 1.0,
                                b: 0.0,
                                c: 0.0,
                                d: 1.0,
                                tx: 0.0,
                                ty: 0.0,
                            },
                        );
                        CTFontDrawGlyphs(ct, glyphs.as_ptr(), pos.as_ptr(), glyphs.len(), c);
                        CGContextRestoreGState(c);
                    });
                }
                Shape::Image {
                    key,
                    mask,
                    r,
                    sx,
                    sy,
                    bg,
                    to,
                    smooth,
                } => {
                    if let Some(bg) = bg {
                        self.fill_colour(*bg);
                        CGContextFillRect(c, cg(r));
                    }
                    // made by the glue before the paint; nothing is drawn for a picture that is not there
                    super::images::with_picture(*key, *mask, Vec::new, |img| {
                        CGContextSaveGState(c);
                        CGContextClipToRects(c, &cg(r), 1);
                        // the drawn image's origin at (r.x - sx, r.y - sy), the picture through `to` into it, upright
                        // in the y-down canvas
                        CGContextTranslateCTM(c, f64::from(r.x - sx), f64::from(r.y - sy));
                        let [a, b, cc, d, tx, ty] = *to;
                        CGContextConcatCTM(
                            c,
                            CGAffineTransform {
                                a,
                                b,
                                c: cc,
                                d,
                                tx,
                                ty,
                            },
                        );
                        CGContextTranslateCTM(c, 0.0, f64::from(key.2));
                        CGContextScaleCTM(c, 1.0, -1.0);
                        // kCGInterpolationHigh, or kCGInterpolationNone: real pixels when scaling up
                        CGContextSetInterpolationQuality(c, if *smooth { 3 } else { 1 });
                        let whole = CGRect {
                            x: 0.0,
                            y: 0.0,
                            w: f64::from(key.1),
                            h: f64::from(key.2),
                        };
                        CGContextDrawImage(c, whole, img);
                        CGContextRestoreGState(c);
                    });
                }
                Shape::Bitmap {
                    x,
                    y,
                    w,
                    rows,
                    fg,
                    bg,
                } => {
                    if let Some(bg) = bg {
                        self.fill_colour(*bg);
                        CGContextFillRect(c, cg(&Rect::new(*x, *y, *w, rows.len() as i32)));
                    }
                    self.fill_colour(*fg);
                    for (i, bits) in rows.iter().enumerate() {
                        // runs of set pixels, leftmost pixel in bit w - 1
                        let mut j = 0;
                        while j < *w {
                            if bits >> (w - 1 - j) & 1 == 0 {
                                j += 1;
                                continue;
                            }
                            let start = j;
                            while j < *w && bits >> (w - 1 - j) & 1 != 0 {
                                j += 1;
                            }
                            CGContextFillRect(
                                c,
                                cg(&Rect::new(x + start, y + i as i32, j - start, 1)),
                            );
                        }
                    }
                }
            }
        }
    }
}

/// An ARGB word as CG components (red, green, blue, alpha).
fn comps(argb: u32) -> [f64; 4] {
    let ch = |s: u32| f64::from((argb >> s) & 0xff) / 255.0;
    [ch(16), ch(8), ch(0), ch(24)]
}

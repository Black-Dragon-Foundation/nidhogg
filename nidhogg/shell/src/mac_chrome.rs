//! Glitnir's child chrome on Core Animation, the pure half (design §5.7): the shadow as one nine-slice image and
//! where its layer goes.  Platform-neutral, so its tests run on Linux; `macos::chrome` sets the layers from it.
//!
//! The corners and the border are the content layer's own properties; only the shadow needs pixels.  Glitnir's
//! shadow of a box varies along both axes only near the box's corners: past the radius and 4σ inside the box the far
//! edge stops mattering (`glitnir::shadow`'s size class), so a column through the middle of a wide box's image is the
//! same at every x there, and a row likewise.  The image is therefore rendered once for a box just wider and taller
//! than that (one middle column and row), and the layer stretches its middle (`contentsCenter`) to any larger size:
//! a move or a resize changes the shadow layer's frame alone, never its pixels.  A box smaller than that gets an image
//! of its own size, rendered again when its size changes.  Every length is in device pixels.

use glitnir::compose::with_opacity;
use glitnir::scene::Chrome;
use glitnir::shadow::{self, ShadowCache, ShadowSpec};

/// A shadow image: premultiplied ARGB words, rows top down, and its stretchable middle (one column and one row,
/// image pixels; the whole image when it was rendered at the layer's own size).
#[derive(Clone, Debug, PartialEq)]
pub struct ShadowImage {
    pub w: u32,
    pub h: u32,
    pub argb: Vec<u32>,
    pub centre: (u32, u32, u32, u32),
    /// The layer size it was rendered for (the box's size before the spread), when that is the layer's own size; `None`
    /// when it stretches to any size at least `min`.
    pub own: Option<(u32, u32)>,
    pub min: (u32, u32),
}

/// The shadow's parameters for Glitnir's tiles, as the Linux compositor makes them.
pub fn spec(c: &Chrome, scale: f64) -> ShadowSpec {
    ShadowSpec {
        radius: c.radius_px,
        blur: c.blur_px,
        spread: c.spread_px,
        colour: with_opacity(c.shadow_argb, c.shadow_opacity),
        scale,
    }
}

/// The smallest layer side whose image stretches: the box (the side grown by the spread on both ends) reaches past the
/// corner radius (grown by a positive spread, an upper bound of the CSS spread radius) and 4σ from each end, plus one
/// middle pixel.
fn stretch_min(c: &Chrome) -> u32 {
    let spread = c.spread_px.round() as i64;
    let flat = (c.radius_px.max(0.0) + spread.max(0) as f32).ceil() as i64
        + (4.0 * shadow::sigma(c.blur_px)).ceil() as i64;
    (2 * flat + 1 - 2 * spread).max(1) as u32
}

/// Whether a `w`×`h` layer's shadow is shown by an image that stretches (else one of its own size).
pub fn stretches(c: &Chrome, w: u32, h: u32) -> bool {
    let m = stretch_min(c);
    w >= m && h >= m
}

/// The shadow image for a `w`×`h` layer (both at least 1) at `scale`.
pub fn image(c: &Chrome, w: u32, h: u32, scale: f64) -> ShadowImage {
    let m = stretch_min(c);
    let stretch = w >= m && h >= m;
    let (bw, bh) = if stretch { (m, m) } else { (w, h) };
    let tiles = ShadowCache::new().get(bw, bh, &spec(c, scale));
    let (iw, ih) = tiles.image_size(bw, bh);
    let colour = tiles.colour();
    let mut argb = Vec::with_capacity((iw * ih) as usize);
    let mut cov = vec![0u8; iw as usize];
    for y in 0..ih {
        tiles.coverage_row(bw, bh, y, 0, &mut cov);
        argb.extend(
            cov.iter()
                .map(|&a| with_opacity(colour, f32::from(a) / 255.0)),
        );
    }
    let centre = if stretch {
        (iw / 2, ih / 2, 1, 1)
    } else {
        (0, 0, iw, ih)
    };
    ShadowImage {
        w: iw,
        h: ih,
        argb,
        centre,
        own: (!stretch).then_some((w, h)),
        min: (m, m),
    }
}

/// Where a `w`×`h` layer's shadow layer goes, from the layer's top left (x, y, width, height): the layer grown by
/// the spread and the blur's reach, moved by the offset.
pub fn frame(c: &Chrome, w: u32, h: u32) -> (f64, f64, f64, f64) {
    let spread = c.spread_px.round() as i64;
    let reach = i64::from(shadow::reach(c.blur_px));
    let pad = (reach + spread) as f64;
    let side = |s: u32| ((i64::from(s) + 2 * spread).max(0) + 2 * reach) as f64;
    (
        f64::from(c.offset_x) - pad,
        f64::from(c.offset_y) - pad,
        side(w),
        side(h),
    )
}

/// How far a `w`×`h` layer's shadow reaches past it on each side (left, top, right, bottom), device pixels, 0 where
/// it does not: what a window holding the layer must add around it to show the whole shadow.
pub fn reach(c: &Chrome, w: u32, h: u32) -> (u32, u32, u32, u32) {
    if !c.has_shadow() {
        return (0, 0, 0, 0);
    }
    let (x, y, fw, fh) = frame(c, w, h);
    let past = |v: f64| v.max(0.0).ceil() as u32;
    (
        past(-x),
        past(-y),
        past(x + fw - f64::from(w)),
        past(y + fh - f64::from(h)),
    )
}

/// `img` drawn at `w`×`h` (the image's size for that layer) as Core Animation draws it with `contentsCenter` at
/// the scale the image was made at: the corners as they are, the middle column and row repeated.  For the tests.
pub fn stretched(img: &ShadowImage, w: u32, h: u32) -> Vec<u32> {
    let (cx, cy, cw, ch) = img.centre;
    let map = |v: u32, size: u32, c: u32, cs: u32, out: u32| {
        let tail = size - c - cs;
        if v < c {
            v
        } else if v >= out - tail {
            size - (out - v)
        } else {
            c + (v - c) * cs / (out - c - tail).max(1)
        }
    };
    let mut px = Vec::with_capacity((w * h) as usize);
    for y in 0..h {
        let sy = map(y, img.h, cy, ch, h);
        for x in 0..w {
            let sx = map(x, img.w, cx, cw, w);
            px.push(img.argb[(sy * img.w + sx) as usize]);
        }
    }
    px
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chrome(radius: f32, blur: f32, spread: f32) -> Chrome {
        Chrome {
            radius_px: radius,
            blur_px: blur,
            offset_x: 0.0,
            offset_y: 12.0,
            spread_px: spread,
            shadow_opacity: 0.35,
            shadow_argb: 0xff00_0000,
            border_px: 0.0,
            border_argb: 0,
            ring_px: 0.0,
            ring_argb: 0,
        }
    }

    /// The reach is the frame's overhang on each side: the offset down makes the bottom reach further than the top, and
    /// a chrome with no shadow reaches nowhere.
    #[test]
    fn the_reach_is_the_shadow_frames_overhang() {
        let c = chrome(12.0, 16.0, 2.0);
        let (x, y, fw, fh) = frame(&c, 200, 100);
        let (l, t, r, b) = reach(&c, 200, 100);
        assert_eq!((l, t), ((-x) as u32, (-y) as u32));
        assert_eq!((r, b), ((x + fw) as u32 - 200, (y + fh) as u32 - 100));
        assert_eq!(l, r);
        assert!(b > t);
        assert_eq!(b - t, 24);
        let flat = Chrome {
            shadow_opacity: 0.0,
            ..c
        };
        assert_eq!(reach(&flat, 200, 100), (0, 0, 0, 0));
    }

    /// The image rendered once and stretched equals the image rendered at the layer's own size, pixel for pixel, for
    /// sizes from the smallest that stretches up, at several radii, blurs and spreads.
    #[test]
    fn a_stretched_shadow_equals_the_one_rendered_at_its_size() {
        for (r, blur, spread) in [
            (12.0, 32.0, 0.0),
            (0.0, 8.0, 2.0),
            (24.0, 4.0, -3.0),
            (8.0, 0.0, 0.0),
        ] {
            let c = chrome(r, blur, spread);
            let m = stretch_min(&c);
            let img = image(&c, m, m, 2.0);
            assert!(img.own.is_none() && img.centre.2 == 1, "{img:?}");
            for (w, h) in [(m, m), (m + 1, m + 7), (3 * m, m + 40), (m + 200, 2 * m)] {
                assert!(stretches(&c, w, h));
                let tiles = ShadowCache::new().get(w, h, &spec(&c, 2.0));
                let (iw, ih) = tiles.image_size(w, h);
                let mut want = Vec::new();
                let mut cov = vec![0u8; iw as usize];
                for y in 0..ih {
                    tiles.coverage_row(w, h, y, 0, &mut cov);
                    want.extend(
                        cov.iter()
                            .map(|&a| with_opacity(tiles.colour(), f32::from(a) / 255.0)),
                    );
                }
                assert_eq!(
                    stretched(&img, iw, ih),
                    want,
                    "r {r} blur {blur} spread {spread} at {w}x{h}"
                );
                let f = frame(&c, w, h);
                assert_eq!((f.2, f.3), (f64::from(iw), f64::from(ih)));
            }
        }
    }

    /// A layer smaller than the stretch minimum gets an image of its own size, with nothing to stretch.
    #[test]
    fn a_small_layer_gets_its_own_image() {
        let c = chrome(12.0, 32.0, 0.0);
        let m = stretch_min(&c);
        assert!(!stretches(&c, m - 1, m + 10));
        let img = image(&c, m - 1, m + 10, 1.0);
        assert_eq!(img.own, Some((m - 1, m + 10)));
        assert_eq!(img.centre, (0, 0, img.w, img.h));
    }

    /// The frame starts the reach and the spread before the layer, moved by the offset, and the image darkens below
    /// the layer more than above it (the offset is down).
    #[test]
    fn the_frame_follows_the_offset() {
        let c = chrome(12.0, 16.0, 2.0);
        let f = frame(&c, 200, 100);
        let pad = f64::from(shadow::reach(16.0)) + 2.0;
        assert_eq!(f, (-pad, 12.0 - pad, 200.0 + 2.0 * pad, 100.0 + 2.0 * pad));
        let img = image(&c, 200, 100, 1.0);
        let alpha = |x: u32, y: u32| img.argb[(y * img.w + x) as usize] >> 24;
        assert!(alpha(img.w / 2, img.h / 2) > alpha(img.w / 2, 2));
    }
}

//! Draw records (Naglfar, design §5.5): the primitives `nhmacdraw.c` emits under `nhwldraw.c`'s geometry, which both
//! shells share (backgrounds, boxes and reliefs, stretches, decorations, cursors, fringe bitmaps).  Each record is
//! decoded here into shapes (`nhshell::shapes`) and painted at once into the frame's backing by the shell's Core
//! Graphics code; there is no frame-long list.  The decoding is platform-free and tested on Linux; each kind follows
//! its Cairo twin in `nhwldraw.c`, so both shells draw the same pixels.

use crate::backing::NhgRect;
use glitnir::geom::Rect;
use nhshell::shapes::{Paint, RRect, Shape};

/// `r` filled with `fg`; `NHG_DRAW_COPY` replaces the pixels, alpha included.
pub const NHG_DRAW_FILL: u32 = 1;
/// A one-pixel outline: lines through the pixel centres of `r`'s edges, `r.w + 1` by `r.h + 1` pixels.
pub const NHG_DRAW_OUTLINE: u32 = 2;
/// A relief's trapezoid over `r` (`nhwl_fill_trapezoid_for_relief`): the top edge's with `NHG_DRAW_TOP`, else the
/// bottom edge's.
pub const NHG_DRAW_TRAPEZOID: u32 = 3;
/// A wave in `r` (its height and width), `a` pixels per half wave.
pub const NHG_DRAW_WAVE: u32 = 4;
/// A dashed line `r.h` thick along `r`'s top (`r.y` is the line's top), dashes and gaps `a` long from phase `b`.
pub const NHG_DRAW_DASH: u32 = 5;
/// A fringe bitmap: `r.h` rows at `bits`, `r.w` bits wide, each bit `a` pixels square (0 or 1: one pixel; at most
/// 8), set bits in `fg`, the rest in `bg` unless `NHG_DRAW_OVERLAY`.
pub const NHG_DRAW_BITMAP: u32 = 6;
/// `r` filled with `fg` outside quarter circles of radius `a` set `b` pixels outside the corners named by the flags'
/// low four bits (bit 0 bottom right, 1 bottom left, 2 top left, 3 top right): `nhwl_erase_corners_for_relief`.
pub const NHG_DRAW_CORNERS: u32 = 7;
/// A glyph run (`nhg_draw_glyphs`): `bits` holds the glyphs, drawn in `fg` from x `a` on the baseline `b`; with
/// `NHG_DRAW_BACKGROUND`, `r` is filled with `bg` first.  `NHG_DRAW_PADDING`: each glyph is one pixel wide.
pub const NHG_DRAW_GLYPHS: u32 = 8;
/// An image (`nhg_draw_image`): the part of the container from (`a`, `b`) drawn into `r`, over `bg` unless
/// `NHG_DRAW_OVERLAY`; an 8-bit container is `fg` through it.
pub const NHG_DRAW_IMAGE: u32 = 9;
/// A box's ring on `r` (glitnir-decor.c's draw_ring): radius `a`, the horizontal lines `b & 0xff` high and the
/// vertical ones `b >> 8 & 0xff` wide; the flags' low four bits name the rounded corners (top left, then clockwise),
/// the next four the sides with a line (top, right, bottom, left; an open side or a run's continuation has none).
/// `NHG_DRAW_RELIEF`: `fg` above the anti-diagonal, `bg` below (raised or sunken), else all `fg`.
pub const NHG_DRAW_RING: u32 = 10;

pub const NHG_DRAW_COPY: u32 = 1 << 0;
pub const NHG_DRAW_TOP: u32 = 1 << 3;
pub const NHG_DRAW_BACKGROUND: u32 = 1 << 4;
pub const NHG_DRAW_PADDING: u32 = 1 << 5;
pub const NHG_DRAW_OVERLAY: u32 = 1 << 7;
pub const NHG_DRAW_RELIEF: u32 = 1 << 8;

/// One record (`NhgDrawRec`).  Rectangles have the native area's origin, as damage has.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct NhgDrawRec {
    pub kind: u32,
    pub flags: u32,
    pub r: NhgRect,
    pub fg: u32,
    pub bg: u32,
    pub a: i32,
    pub b: i32,
    /// How many of `clip` apply (0–2); none means unclipped.
    pub nclip: i32,
    pub clip: [NhgRect; 2],
    /// `NHG_DRAW_BITMAP`'s rows.
    pub bits: *const u16,
}

/// Why a record is refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Bad {
    Kind,
    Size,
    Clip,
    Bits,
}

fn rect(r: NhgRect) -> Rect {
    Rect::new(r.x, r.y, r.w, r.h)
}

/// `rec`'s rectangle and clip, checked.
fn frame(rec: &NhgDrawRec) -> Result<(Rect, Vec<Rect>), Bad> {
    let r = rect(rec.r);
    if r.w < 0 || r.h < 0 || r.w > 1 << 15 || r.h > 1 << 15 {
        return Err(Bad::Size);
    }
    let clip = match rec.nclip {
        0 => Vec::new(),
        n @ 1..=2 => rec.clip[..n as usize].iter().map(|c| rect(*c)).collect(),
        _ => return Err(Bad::Clip),
    };
    Ok((r, clip))
}

/// `rec` as shapes.  `bits` are the bitmap's rows (read by the caller from `rec.bits`).
pub fn decode(rec: &NhgDrawRec, bits: &[u16]) -> Result<Paint, Bad> {
    let (r, mut clip) = frame(rec)?;
    // a wave is also clipped to its own rectangle
    if rec.kind == NHG_DRAW_WAVE {
        clip = if clip.is_empty() {
            vec![r]
        } else {
            clip.iter().map(|c| c.intersect(&r)).collect()
        };
    }
    let shapes = match rec.kind {
        NHG_DRAW_FILL => vec![Shape::Fill {
            r,
            argb: rec.fg,
            copy: rec.flags & NHG_DRAW_COPY != 0,
        }],
        NHG_DRAW_OUTLINE => vec![Shape::Outline { r, argb: rec.fg }],
        NHG_DRAW_TRAPEZOID => vec![trapezoid(
            rec.fg,
            r.x,
            r.y,
            r.w,
            r.h,
            rec.flags & NHG_DRAW_TOP != 0,
        )],
        NHG_DRAW_CORNERS => vec![Shape::Corners {
            r,
            radius: f64::from(rec.a),
            margin: f64::from(rec.b),
            corners: rec.flags & 0xf,
            argb: rec.fg,
        }],
        NHG_DRAW_RING => vec![ring(rec, r)?],
        NHG_DRAW_WAVE => wave(r, rec.a.max(1), rec.fg),
        NHG_DRAW_DASH => {
            let y = f64::from(r.y) + f64::from(r.h) / 2.0;
            vec![Shape::Path {
                pts: vec![(f64::from(r.x), y), (f64::from(r.x + r.w), y)],
                width: f64::from(r.h),
                dash: Some((f64::from(rec.a.max(1)), f64::from(rec.b))),
                argb: rec.fg,
            }]
        }
        NHG_DRAW_BITMAP => {
            if r.w > 16 || bits.len() != r.h as usize {
                return Err(Bad::Bits);
            }
            // `a`: each bit's size in pixels (a fringe bitmap scaled to its fringe), 0 or 1 unscaled
            if !(0..=8).contains(&rec.a) {
                return Err(Bad::Size);
            }
            vec![Shape::Bitmap {
                x: r.x,
                y: r.y,
                w: r.w,
                rows: bits.to_vec(),
                scale: rec.a.max(1),
                fg: rec.fg,
                bg: (rec.flags & NHG_DRAW_OVERLAY == 0).then_some(rec.bg),
            }]
        }
        _ => return Err(Bad::Kind),
    };
    Ok(Paint { clip, shapes })
}

/// A glyph run's record as shapes: its background, then `glyphs` of the open font `font` at the offsets `xs` (the
/// font's, `font::run`).
pub fn decode_glyphs(
    rec: &NhgDrawRec,
    font: u64,
    glyphs: &[u16],
    xs: Vec<f64>,
    integral: bool,
) -> Result<Paint, Bad> {
    if rec.kind != NHG_DRAW_GLYPHS {
        return Err(Bad::Kind);
    }
    if xs.len() != glyphs.len() {
        return Err(Bad::Bits);
    }
    let (r, clip) = frame(rec)?;
    let mut shapes = Vec::with_capacity(2);
    if rec.flags & NHG_DRAW_BACKGROUND != 0 {
        shapes.push(Shape::Fill {
            r,
            argb: rec.bg,
            copy: false,
        });
    }
    shapes.push(Shape::Glyphs {
        font,
        x: f64::from(rec.a),
        y: f64::from(rec.b),
        glyphs: glyphs.to_vec(),
        xs,
        argb: rec.fg,
        integral,
    });
    Ok(Paint { clip, shapes })
}

/// An image record as a shape: the part of picture `key` (made with `mask`) from (`a`, `b`) into `r`, through the
/// image's `matrix` (`NhgPix`'s), smoothed when `smooth`.
pub fn decode_image(
    rec: &NhgDrawRec,
    key: (usize, i32, i32),
    mask: usize,
    matrix: &crate::image::Matrix,
    smooth: bool,
) -> Result<Paint, Bad> {
    if rec.kind != NHG_DRAW_IMAGE {
        return Err(Bad::Kind);
    }
    let (r, clip) = frame(rec)?;
    Ok(Paint {
        clip,
        shapes: vec![Shape::Image {
            key,
            mask,
            r,
            sx: rec.a,
            sy: rec.b,
            bg: (rec.flags & NHG_DRAW_OVERLAY == 0).then_some(rec.bg),
            to: crate::image::drawn(matrix),
            smooth,
        }],
    })
}

/// An `NHG_DRAW_RING` record on `r` as glitnir-decor.c's draw_ring has it: the outer path with the radius (at most
/// half the box) at the rounded corners, the inner one inset by the lines on the sides that have one, its radii less
/// the wider line.
fn ring(rec: &NhgDrawRec, r: Rect) -> Result<Shape, Bad> {
    if rec.a < 0 || !(0..1 << 16).contains(&rec.b) {
        return Err(Bad::Size);
    }
    let (x, y, w, h) = (
        f64::from(r.x),
        f64::from(r.y),
        f64::from(r.w),
        f64::from(r.h),
    );
    let (hw, vw) = (f64::from(rec.b & 0xff), f64::from(rec.b >> 8));
    let rad = f64::from(rec.a).min(h / 2.0).min(w / 2.0);
    let radii: [f64; 4] = std::array::from_fn(|i| if rec.flags & 1 << i != 0 { rad } else { 0.0 });
    let side = |i: u32| rec.flags & 1 << (4 + i) != 0;
    let lw = hw.max(vw);
    let x0 = if side(3) { x + vw } else { x };
    let x1 = if side(1) { x + w - vw } else { x + w };
    let y0 = if side(0) { y + hw } else { y };
    let y1 = if side(2) { y + h - hw } else { y + h };
    Ok(Shape::Ring {
        outer: RRect { x, y, w, h, radii },
        inner: RRect {
            x: x0,
            y: y0,
            w: x1 - x0,
            h: y1 - y0,
            radii: radii.map(|q| (q - lw).max(0.0)),
        },
        argb: rec.fg,
        relief: (rec.flags & NHG_DRAW_RELIEF != 0).then_some(rec.bg),
    })
}

/// `nhwl_fill_trapezoid_for_relief`.
fn trapezoid(argb: u32, x: i32, y: i32, w: i32, h: i32, top: bool) -> Shape {
    let (x, y, w, h) = (f64::from(x), f64::from(y), f64::from(w), f64::from(h));
    Shape::Polygon {
        pts: vec![
            (if top { x } else { x + h }, y),
            (x, y + h),
            (if top { x + w - h } else { x + w }, y + h),
            (x + w, y),
        ],
        argb,
    }
}

/// `nhwl_draw_horizontal_wave`: a zigzag of one-pixel lines inside `r`, in phase with x = 0.
fn wave(r: Rect, len: i32, argb: u32) -> Vec<Shape> {
    let (dx, mut dy) = (f64::from(len), f64::from(r.h - 1));
    let mut xoffset = if r.x >= 0 {
        match r.x % (len * 2) {
            0 => len * 2,
            o => o,
        }
    } else {
        r.x % (len * 2) + len * 2
    };
    let mut n = (r.w + xoffset) / len + 1;
    let mut y = r.y;
    if xoffset > len {
        xoffset -= len;
        n -= 1;
        y += r.h - 1;
        dy = -dy;
    }
    let mut p = (f64::from(r.x - xoffset) + 0.5, f64::from(y) + 0.5);
    let mut pts = vec![p];
    for _ in 0..n.max(0) {
        p = (p.0 + dx, p.1 + dy);
        pts.push(p);
        dy = -dy;
    }
    vec![Shape::Path {
        pts,
        width: 1.0,
        dash: None,
        argb,
    }]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(kind: u32, flags: u32, r: (i32, i32, i32, i32)) -> NhgDrawRec {
        NhgDrawRec {
            kind,
            flags,
            r: NhgRect {
                x: r.0,
                y: r.1,
                w: r.2,
                h: r.3,
            },
            fg: 0xff20_4060,
            bg: 0xffff_ffff,
            a: 1,
            b: 1,
            nclip: 0,
            clip: [NhgRect::default(); 2],
            bits: std::ptr::null(),
        }
    }

    #[test]
    fn records_are_validated() {
        assert_eq!(decode(&rec(99, 0, (0, 0, 1, 1)), &[]), Err(Bad::Kind));
        assert_eq!(
            decode(&rec(NHG_DRAW_FILL, 0, (0, 0, -1, 1)), &[]),
            Err(Bad::Size)
        );
        let mut c = rec(NHG_DRAW_FILL, 0, (0, 0, 1, 1));
        c.nclip = 3;
        assert_eq!(decode(&c, &[]), Err(Bad::Clip));
        // a bitmap needs exactly its rows, at most 16 wide
        assert_eq!(
            decode(&rec(NHG_DRAW_BITMAP, 0, (0, 0, 8, 2)), &[1]),
            Err(Bad::Bits)
        );
        assert_eq!(
            decode(&rec(NHG_DRAW_BITMAP, 0, (0, 0, 17, 1)), &[1]),
            Err(Bad::Bits)
        );
        assert!(decode(&rec(NHG_DRAW_BITMAP, 0, (0, 0, 8, 2)), &[1, 2]).is_ok());
    }

    #[test]
    fn a_fill_keeps_its_clip_and_mode() {
        let mut c = rec(NHG_DRAW_FILL, NHG_DRAW_COPY, (1, 2, 3, 4));
        c.nclip = 2;
        c.clip = [
            NhgRect {
                x: 0,
                y: 0,
                w: 5,
                h: 5,
            },
            NhgRect {
                x: 9,
                y: 9,
                w: 1,
                h: 1,
            },
        ];
        let p = decode(&c, &[]).unwrap();
        assert_eq!(p.clip, vec![Rect::new(0, 0, 5, 5), Rect::new(9, 9, 1, 1)]);
        assert_eq!(
            p.shapes,
            vec![Shape::Fill {
                r: Rect::new(1, 2, 3, 4),
                argb: 0xff20_4060,
                copy: true
            }]
        );
    }

    #[test]
    fn a_trapezoid_slants_its_free_end() {
        // the top edge of a relief: full width at the top, cut by its height at the bottom right
        let p = decode(&rec(NHG_DRAW_TRAPEZOID, NHG_DRAW_TOP, (0, 0, 20, 2)), &[]).unwrap();
        assert_eq!(
            p.shapes,
            vec![Shape::Polygon {
                pts: vec![(0.0, 0.0), (0.0, 2.0), (18.0, 2.0), (20.0, 0.0)],
                argb: 0xff20_4060
            }]
        );
        // the bottom edge: cut at the top left
        let q = decode(&rec(NHG_DRAW_TRAPEZOID, 0, (0, 8, 20, 2)), &[]).unwrap();
        assert!(
            matches!(&q.shapes[0], Shape::Polygon { pts, .. } if pts[0] == (2.0, 8.0) && pts[2] == (20.0, 10.0))
        );
    }

    #[test]
    fn corners_carry_radius_margin_and_which() {
        let mut c = rec(NHG_DRAW_CORNERS, 0b0101 | 0x30, (1, 2, 30, 10));
        c.a = 6;
        c.b = 1;
        let p = decode(&c, &[]).unwrap();
        assert_eq!(
            p.shapes,
            vec![Shape::Corners {
                r: Rect::new(1, 2, 30, 10),
                radius: 6.0,
                margin: 1.0,
                corners: 0b0101,
                argb: 0xff20_4060
            }]
        );
    }

    #[test]
    fn a_ring_is_the_box_outline_less_its_lines() {
        // a 40 x 20 box at (10, 5), radius 8, lines 2 high and 3 wide; rounded at the top left and bottom right,
        // lines on all four sides
        let mut c = rec(NHG_DRAW_RING, 0b0101 | 0xf << 4, (10, 5, 40, 20));
        c.a = 8;
        c.b = 2 | 3 << 8;
        let p = decode(&c, &[]).unwrap();
        assert_eq!(
            p.shapes,
            vec![Shape::Ring {
                outer: RRect {
                    x: 10.0,
                    y: 5.0,
                    w: 40.0,
                    h: 20.0,
                    radii: [8.0, 0.0, 8.0, 0.0]
                },
                // inside the lines, the radii less the wider line
                inner: RRect {
                    x: 13.0,
                    y: 7.0,
                    w: 34.0,
                    h: 16.0,
                    radii: [5.0, 0.0, 5.0, 0.0]
                },
                argb: 0xff20_4060,
                relief: None
            }]
        );
        // open at the top and the left (a run's continuation): the inner edge lies on the outer one there; a
        // relief's second colour; the radius at most half the height
        let mut c = rec(
            NHG_DRAW_RING,
            0b0110 | 0b0110 << 4 | NHG_DRAW_RELIEF,
            (0, 0, 40, 10),
        );
        c.a = 9;
        c.b = 1 | 1 << 8;
        c.bg = 0xff00_0000;
        let Shape::Ring {
            outer,
            inner,
            relief,
            ..
        } = decode(&c, &[]).unwrap().shapes[0]
        else {
            panic!()
        };
        assert_eq!(outer.radii, [0.0, 5.0, 5.0, 0.0]);
        assert_eq!((inner.x, inner.y, inner.w, inner.h), (0.0, 0.0, 39.0, 9.0));
        assert_eq!(inner.radii, [0.0, 4.0, 4.0, 0.0]);
        assert_eq!(relief, Some(0xff00_0000));
        // line widths past a byte each are refused
        c.b = 1 << 16;
        assert_eq!(decode(&c, &[]), Err(Bad::Size));
    }

    #[test]
    fn a_wave_zigzags_in_phase_with_x_zero() {
        let p = decode(&rec(NHG_DRAW_WAVE, 0, (4, 10, 6, 3)), &[]).unwrap();
        let Shape::Path { pts, width, .. } = &p.shapes[0] else {
            panic!()
        };
        assert_eq!(*width, 1.0);
        assert_eq!(p.clip, vec![Rect::new(4, 10, 6, 3)]);
        // x 4 with half waves of 1 (a = 1): offset 2 > 1, so it starts at the bottom, one half wave left of x
        assert_eq!(pts[0], (3.5, 12.5));
        assert_eq!(pts[1], (4.5, 10.5));
        assert!(pts.last().unwrap().0 >= 10.0);
    }

    #[test]
    fn a_dash_runs_along_the_middle_of_its_thickness() {
        let mut c = rec(NHG_DRAW_DASH, 0, (5, 20, 30, 2));
        c.a = 6;
        c.b = 5;
        let p = decode(&c, &[]).unwrap();
        assert_eq!(
            p.shapes,
            vec![Shape::Path {
                pts: vec![(5.0, 21.0), (35.0, 21.0)],
                width: 2.0,
                dash: Some((6.0, 5.0)),
                argb: 0xff20_4060
            }]
        );
    }

    #[test]
    fn an_overlay_bitmap_leaves_its_background() {
        let p = decode(
            &rec(NHG_DRAW_BITMAP, NHG_DRAW_OVERLAY, (0, 0, 8, 1)),
            &[0x81],
        )
        .unwrap();
        assert!(matches!(&p.shapes[0], Shape::Bitmap { bg: None, rows, .. } if rows == &[0x81]));
        let q = decode(&rec(NHG_DRAW_BITMAP, 0, (0, 0, 8, 1)), &[0x81]).unwrap();
        assert!(matches!(
            &q.shapes[0],
            Shape::Bitmap {
                bg: Some(0xffff_ffff),
                ..
            }
        ));
    }

    #[test]
    fn a_bitmap_keeps_its_scale() {
        let mut r = rec(NHG_DRAW_BITMAP, 0, (0, 0, 8, 1));
        r.a = 2;
        let p = decode(&r, &[0x81]).unwrap();
        assert!(matches!(&p.shapes[0], Shape::Bitmap { scale: 2, .. }));
        r.a = 0;
        let p = decode(&r, &[0x81]).unwrap();
        assert!(matches!(&p.shapes[0], Shape::Bitmap { scale: 1, .. }));
        r.a = 9;
        assert_eq!(decode(&r, &[0x81]).err(), Some(Bad::Size));
    }

    #[test]
    fn paint_translates_every_coordinate() {
        let mut c = rec(NHG_DRAW_FILL, 0, (1, 1, 2, 2));
        c.nclip = 1;
        c.clip[0] = NhgRect {
            x: 0,
            y: 0,
            w: 4,
            h: 4,
        };
        let mut p = decode(&c, &[]).unwrap();
        p.translate(10, 20);
        assert_eq!(p.clip, vec![Rect::new(10, 20, 4, 4)]);
        assert!(matches!(p.shapes[0], Shape::Fill { r, .. } if r == Rect::new(11, 21, 2, 2)));
    }

    #[test]
    fn a_glyph_run_fills_its_background_then_draws_at_the_baseline() {
        let mut c = rec(NHG_DRAW_GLYPHS, NHG_DRAW_BACKGROUND, (10, 2, 21, 14));
        (c.a, c.b) = (10, 13);
        let p = decode_glyphs(&c, 7, &[36, 37, 38], vec![0.0, 7.0, 14.0], false).unwrap();
        assert_eq!(
            p.shapes[0],
            Shape::Fill {
                r: Rect::new(10, 2, 21, 14),
                argb: 0xffff_ffff,
                copy: false
            }
        );
        assert_eq!(
            p.shapes[1],
            Shape::Glyphs {
                font: 7,
                x: 10.0,
                y: 13.0,
                glyphs: vec![36, 37, 38],
                xs: vec![0.0, 7.0, 14.0],
                argb: 0xff20_4060,
                integral: false
            }
        );
        // no background without the flag; the offsets must cover every glyph
        let c = rec(NHG_DRAW_GLYPHS, 0, (0, 0, 7, 14));
        let p = decode_glyphs(&c, 7, &[36], vec![0.0], true).unwrap();
        assert_eq!(p.shapes.len(), 1);
        assert_eq!(
            decode_glyphs(&c, 7, &[36, 37], vec![0.0], true),
            Err(Bad::Bits)
        );
        assert_eq!(
            decode_glyphs(&rec(NHG_DRAW_FILL, 0, (0, 0, 1, 1)), 7, &[], vec![], true),
            Err(Bad::Kind)
        );
        // translated with the rest, from x 1 on the baseline 1
        let mut p = decode_glyphs(&c, 7, &[36], vec![0.0], true).unwrap();
        p.translate(5, 6);
        assert!(matches!(p.shapes[0], Shape::Glyphs { x: 6.0, y: 7.0, .. }));
    }

    #[test]
    fn an_image_draws_its_part_over_the_background_unless_overlaid() {
        let mut c = rec(NHG_DRAW_IMAGE, 0, (10, 20, 16, 8));
        (c.a, c.b) = (4, 2);
        let p = decode_image(
            &c,
            (0x1000, 32, 32),
            0,
            &[2.0, 0.0, 0.0, 2.0, 0.0, 0.0],
            true,
        )
        .unwrap();
        assert_eq!(
            p.shapes,
            vec![Shape::Image {
                key: (0x1000, 32, 32),
                mask: 0,
                r: Rect::new(10, 20, 16, 8),
                sx: 4,
                sy: 2,
                bg: Some(0xffff_ffff),
                to: [0.5, 0.0, 0.0, 0.5, 0.0, 0.0],
                smooth: true
            }]
        );
        let c = rec(NHG_DRAW_IMAGE, NHG_DRAW_OVERLAY, (0, 0, 1, 1));
        assert!(matches!(
            decode_image(&c, (1, 1, 1), 0, &[0.0; 6], false)
                .unwrap()
                .shapes[0],
            Shape::Image { bg: None, .. }
        ));
        assert_eq!(
            decode_image(
                &rec(NHG_DRAW_FILL, 0, (0, 0, 1, 1)),
                (1, 1, 1),
                0,
                &[0.0; 6],
                false
            ),
            Err(Bad::Kind)
        );
        let mut p = decode_image(&c, (1, 1, 1), 0, &[0.0; 6], false).unwrap();
        p.translate(3, 4);
        assert!(matches!(p.shapes[0], Shape::Image { r, .. } if r == Rect::new(3, 4, 1, 1)));
    }
}

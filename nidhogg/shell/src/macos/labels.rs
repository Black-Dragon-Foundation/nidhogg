//! The minimap's labels on Naglfar (design M1, stage 3m): a name drawn in Core Text in the label face's font, as
//! glitnir-decor.c draws it with Cairo on Linux, so the labels match the UI text Emacs draws.  The glue asks through
//! C's label renderer (`nhmac_minimap_label_render`, `nhg_label_raster`); the sprite it gets is cached and placed by
//! the glue, and shown as one of the minimap's pieces.

use super::draw::Canvas;
use super::fonts;
use super::text;
use crate::shapes::{Paint, Shape};
use glitnir::geom::Rect;

/// The ellipsis a name too wide for its room ends with.
const ELLIPSIS: &str = "\u{2026}";

/// `units` in the open font `font`: each glyph and its x, and the line's width.  `None`: not open, or Core Text needed
/// another font for some of them.
fn laid_out(font: u64, units: &[u16]) -> Option<(Vec<u16>, Vec<f64>, f64)> {
    let line = text::shape(font, units)?;
    let (mut glyphs, mut xs, mut width) = (Vec::new(), Vec::new(), 0.0f64);
    for run in &line.runs {
        for ((g, (x, _)), w) in run.glyphs.iter().zip(&run.positions).zip(&run.widths) {
            glyphs.push(*g);
            xs.push(*x);
            width = width.max(x + w);
        }
    }
    Some((glyphs, xs, width))
}

/// `name` in the open font `handle` at `px` device pixels in `rgb` (0xRRGGBB), drawn into `buf` (premultiplied ARGB,
/// `cap_w` × `cap_h`, cleared) from its top-left, centred in the rows, cut with an ellipsis when wider than `cap_w`: the
/// width and height it used.  `None` when nothing was drawn.
pub fn render(
    handle: u64,
    px: f64,
    name: &str,
    rgb: u32,
    buf: &mut [u32],
    cap_w: i32,
    cap_h: i32,
) -> Option<(i32, i32)> {
    if cap_w <= 0 || cap_h <= 0 || buf.len() < (cap_w * cap_h) as usize || px.is_nan() || px <= 0.0
    {
        return None;
    }
    let font = fonts::sized(handle, px)?;
    let m = fonts::raw(font)?;
    let units: Vec<u16> = name.encode_utf16().collect();
    let (mut glyphs, mut xs, mut width) = laid_out(font, &units)?;
    if width > f64::from(cap_w) {
        // the longest start of the name that fits with the ellipsis after it
        let chars: Vec<char> = name.chars().collect();
        let mut n = chars.len();
        loop {
            if n == 0 {
                return None;
            }
            n -= 1;
            let cut: String =
                chars[..n].iter().collect::<String>().trim_end().to_string() + ELLIPSIS;
            let u: Vec<u16> = cut.encode_utf16().collect();
            let (g, x, w) = laid_out(font, &u)?;
            if w <= f64::from(cap_w) {
                (glyphs, xs, width) = (g, x, w);
                break;
            }
        }
    }
    let used = (width.ceil() as i32).clamp(1, cap_w);
    // the text's box centred in the rows: ascent above the baseline, descent below
    let base = ((f64::from(cap_h) - (m.ascent + m.descent)) / 2.0 + m.ascent).round();
    let buf = &mut buf[..(cap_w * cap_h) as usize];
    {
        let mut slot = None;
        let canvas = Canvas::over(&mut slot, buf, cap_w, cap_h)?;
        canvas.paint(&Paint {
            clip: vec![Rect {
                x: 0,
                y: 0,
                w: used,
                h: cap_h,
            }],
            shapes: vec![Shape::Glyphs {
                font,
                x: 0.0,
                y: base,
                glyphs,
                xs,
                argb: 0xff00_0000 | (rgb & 0xff_ffff),
                integral: fonts::integral(font).unwrap_or(false),
            }],
        });
    }
    Some((used, cap_h))
}

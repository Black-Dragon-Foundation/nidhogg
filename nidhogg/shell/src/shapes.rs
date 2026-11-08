//! The shapes a draw record becomes (the glue's `draw.rs` decodes records into them; `macos::draw` paints them with
//! Core Graphics).  Platform-free, so the decoding is tested on Linux.  Coordinates are device pixels of the backing,
//! y down; colours are ARGB words, not premultiplied.

use glitnir::geom::Rect;

/// One shape.
#[derive(Debug, Clone, PartialEq)]
pub enum Shape {
    /// `r` filled with `argb`; `copy` replaces what is there, alpha included (Cairo's `SOURCE`), else it is drawn
    /// over.
    Fill { r: Rect, argb: u32, copy: bool },
    /// A one-pixel outline whose lines run through the pixel centres from (`r.x`, `r.y`) to (`r.x + r.w`,
    /// `r.y + r.h`), so it covers `r.w + 1` by `r.h + 1` pixels.
    Outline { r: Rect, argb: u32 },
    /// A filled polygon.
    Polygon { pts: Vec<(f64, f64)>, argb: u32 },
    /// An open path stroked `width` wide; `dash` is the dash (and gap) length and its phase.
    Path {
        pts: Vec<(f64, f64)>,
        width: f64,
        dash: Option<(f64, f64)>,
        argb: u32,
    },
    /// `r` filled with `argb` only outside quarter circles of `radius` at the `corners` named (bit 0 bottom right,
    /// 1 bottom left, 2 top left, 3 top right), each set `margin` pixels outside `r`'s corner.
    Corners {
        r: Rect,
        radius: f64,
        margin: f64,
        corners: u32,
        argb: u32,
    },
    /// A 1-bit bitmap at (`x`, `y`), `w` bits wide: bit `j` of row `i` is set when bit `w - 1 - j` of `rows[i]` is,
    /// and each bit covers `scale` by `scale` pixels.  Set bits take `fg`; unset ones take `bg`, or are left alone when
    /// it is `None`.
    Bitmap {
        x: i32,
        y: i32,
        w: i32,
        rows: Vec<u16>,
        scale: i32,
        fg: u32,
        bg: Option<u32>,
    },
    /// Glyphs of the open font `font` (`macos::fonts`) in `argb`, glyph `i` with its origin at (`x + xs[i]`, `y`), `y`
    /// the baseline; `integral` draws them without antialiasing.
    Glyphs {
        font: u64,
        x: f64,
        y: f64,
        glyphs: Vec<u16>,
        xs: Vec<f64>,
        argb: u32,
        integral: bool,
    },
    /// The part of picture `key` (`macos::images`: the pixels' address and size, made with `mask`) from (`sx`, `sy`)
    /// drawn into `r`, over `bg` when it is given.  `to` takes a picture pixel to the drawn image's (an affine map
    /// `(a, b, c, d, tx, ty)`: x' = a x + c y + tx, y' = b x + d y + ty; the identity for a picture drawn as it is),
    /// smoothed when `smooth`.
    Image {
        key: (usize, i32, i32),
        mask: usize,
        r: Rect,
        sx: i32,
        sy: i32,
        bg: Option<u32>,
        to: [f64; 6],
        smooth: bool,
    },
}

/// What one record paints: its shapes, inside the union of `clip` (no clip when empty).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Paint {
    pub clip: Vec<Rect>,
    pub shapes: Vec<Shape>,
}

impl Paint {
    /// Move every coordinate by (`dx`, `dy`).
    pub fn translate(&mut self, dx: i32, dy: i32) {
        let (fx, fy) = (f64::from(dx), f64::from(dy));
        let mv = |r: &mut Rect| {
            r.x += dx;
            r.y += dy;
        };
        self.clip.iter_mut().for_each(mv);
        for s in &mut self.shapes {
            match s {
                Shape::Fill { r, .. }
                | Shape::Outline { r, .. }
                | Shape::Corners { r, .. }
                | Shape::Image { r, .. } => mv(r),
                Shape::Polygon { pts, .. } | Shape::Path { pts, .. } => {
                    for p in pts.iter_mut() {
                        p.0 += fx;
                        p.1 += fy;
                    }
                }
                Shape::Bitmap { x, y, .. } => {
                    *x += dx;
                    *y += dy;
                }
                Shape::Glyphs { x, y, .. } => {
                    *x += fx;
                    *y += fy;
                }
            }
        }
    }
}

/// The rectangles a bitmap's set bits cover (`Shape::Bitmap`): one per run of set bits in a row, the leftmost bit in
/// bit `w - 1`, each bit `scale` pixels square.
pub fn bitmap_runs(x: i32, y: i32, w: i32, rows: &[u16], scale: i32) -> Vec<Rect> {
    let mut out = Vec::new();
    for (i, bits) in rows.iter().enumerate() {
        let mut j = 0;
        while j < w {
            if bits >> (w - 1 - j) & 1 == 0 {
                j += 1;
                continue;
            }
            let start = j;
            while j < w && bits >> (w - 1 - j) & 1 != 0 {
                j += 1;
            }
            out.push(Rect::new(
                x + start * scale,
                y + i as i32 * scale,
                (j - start) * scale,
                scale,
            ));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bitmap_runs_cover_the_set_bits() {
        // 1001 / 0110, 4 wide
        let r = bitmap_runs(10, 20, 4, &[0b1001, 0b0110], 1);
        assert_eq!(
            r,
            vec![
                Rect::new(10, 20, 1, 1),
                Rect::new(13, 20, 1, 1),
                Rect::new(11, 21, 2, 1)
            ]
        );
    }

    #[test]
    fn a_scaled_bitmap_covers_scale_squares() {
        let r = bitmap_runs(10, 20, 4, &[0b1001, 0b0110], 2);
        assert_eq!(
            r,
            vec![
                Rect::new(10, 20, 2, 2),
                Rect::new(16, 20, 2, 2),
                Rect::new(12, 22, 4, 2)
            ]
        );
    }
}

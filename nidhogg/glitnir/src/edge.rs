//! Edges: thin lines drawn over a window's text from one place to another (the window parameter `glitnir-edges`; the
//! heap graph's references).  An edge is a polyline in root device px whose bends are rounded (a quadratic curve with
//! the bend as its control point, starting and ending `radius_px` from it) and which may end in an arrowhead or be
//! dashed.  [`outline`] is its geometry, the one model every shell draws from: compose rasterises it here
//! ([`coverage`]); a shell that draws edges itself strokes the same outline.

use crate::geom::Rect;
use std::sync::Arc;

/// One edge ([`crate::scene::Scene::set_edges`]).  Colours are premultiplied ARGB.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Edge {
    /// The start, the bends and the end, root device px (the centre of each pixel is at +0.5).
    pub points: Arc<[(i32, i32)]>,
    /// The line's width, whole device px (at least 1).
    pub width_px: u32,
    /// The bends' radius, device px (less where a side is too short for it).
    pub radius_px: u32,
    pub argb: u32,
    /// The arrowhead's length at the end, device px (0: none); it is 7/6 of that wide.
    pub arrow_px: u32,
    /// A dash and the gap after it, device px (0: a solid line).
    pub dash_px: u32,
}

/// A piece of an edge's path.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Seg {
    Line((f32, f32), (f32, f32)),
    /// From, control, to.
    Quad((f32, f32), (f32, f32), (f32, f32)),
}

/// An edge's geometry: its path, which stops at the arrow's base, and the arrow (tip, then the base's two corners).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Outline {
    pub segs: Vec<Seg>,
    pub arrow: Option<[(f32, f32); 3]>,
}

/// The unit vector from A to B and the distance (0 and 0 for one point).
fn dir(a: (f32, f32), b: (f32, f32)) -> (f32, f32, f32) {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let len = (dx * dx + dy * dy).sqrt();
    if len == 0.0 {
        (0.0, 0.0, 0.0)
    } else {
        (dx / len, dy / len, len)
    }
}

/// EDGE's geometry in root device px.  A point is a pixel's centre for an odd width (a 1 px line covers whole
/// pixels), its corner for an even one.  A bend's radius is at most the whole of an end side, half of a side between
/// two bends.
pub fn outline(edge: &Edge) -> Outline {
    let off = if edge.width_px.max(1) % 2 == 1 {
        0.5
    } else {
        0.0
    };
    let mut pts: Vec<(f32, f32)> = Vec::with_capacity(edge.points.len());
    for &(x, y) in edge.points.iter() {
        let p = (x as f32 + off, y as f32 + off);
        if pts.last() != Some(&p) {
            pts.push(p);
        }
    }
    if pts.len() < 2 {
        return Outline::default();
    }
    let mut arrow = None;
    if edge.arrow_px > 0 {
        let n = pts.len();
        let (dx, dy, len) = dir(pts[n - 2], pts[n - 1]);
        let tip = pts[n - 1];
        let l = (edge.arrow_px as f32).min(len);
        let half = edge.arrow_px as f32 * 7.0 / 12.0;
        let base = (tip.0 - dx * l, tip.1 - dy * l);
        arrow = Some([
            tip,
            (base.0 + dy * half, base.1 - dx * half),
            (base.0 - dy * half, base.1 + dx * half),
        ]);
        pts[n - 1] = base;
        if dir(pts[n - 2], base).2 < 0.01 {
            pts.pop();
        }
    }
    let n = pts.len();
    let mut segs = Vec::with_capacity(2 * n);
    let mut cur = pts[0];
    for i in 1..n - 1 {
        let (p, c, q) = (pts[i - 1], pts[i], pts[i + 1]);
        let (ix, iy, li) = dir(p, c);
        let (ox, oy, lo) = dir(c, q);
        let room_in = if i == 1 { li } else { li / 2.0 };
        let room_out = if i + 1 == n - 1 { lo } else { lo / 2.0 };
        let r = (edge.radius_px as f32).min(room_in).min(room_out);
        let a = (c.0 - ix * r, c.1 - iy * r);
        let b = (c.0 + ox * r, c.1 + oy * r);
        if a != cur {
            segs.push(Seg::Line(cur, a));
        }
        if r > 0.0 {
            segs.push(Seg::Quad(a, c, b));
        }
        cur = b;
    }
    if pts[n - 1] != cur {
        segs.push(Seg::Line(cur, pts[n - 1]));
    }
    Outline { segs, arrow }
}

/// How far an edge's pixels reach past its points, device px: half its width, the arrow's half width, one for the
/// anti-aliasing.
fn reach(edge: &Edge) -> i32 {
    let w = edge.width_px.max(1) as f32 / 2.0;
    let a = edge.arrow_px as f32 * 7.0 / 12.0;
    (w.max(a) + 1.0).ceil() as i32
}

/// The rectangle EDGE's pixels lie in (root device px), its width and arrow included; empty for fewer than 2 points.
pub fn bounds(edge: &Edge) -> Rect {
    if edge.points.len() < 2 {
        return Rect::default();
    }
    let (mut x0, mut y0, mut x1, mut y1) = (i32::MAX, i32::MAX, i32::MIN, i32::MIN);
    for &(x, y) in edge.points.iter() {
        (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x), y1.max(y));
    }
    let r = reach(edge);
    Rect::new(
        x0.saturating_sub(r),
        y0.saturating_sub(r),
        x1.saturating_sub(x0).saturating_add(2 * r + 1),
        y1.saturating_sub(y0).saturating_add(2 * r + 1),
    )
}

/// A point, root device px.
type Pt = (f32, f32);

/// The lines a quadratic is drawn as: enough that none strays a tenth of a pixel from the curve.
fn flatten(a: Pt, c: Pt, b: Pt, out: &mut Vec<(Pt, Pt)>) {
    let span = dir(a, c).2 + dir(c, b).2;
    let n = ((span / 2.0).ceil() as usize).clamp(2, 64);
    let mut prev = a;
    for k in 1..=n {
        let t = k as f32 / n as f32;
        let u = 1.0 - t;
        let p = (
            u * u * a.0 + 2.0 * u * t * c.0 + t * t * b.0,
            u * u * a.1 + 2.0 * u * t * c.1 + t * t * b.1,
        );
        out.push((prev, p));
        prev = p;
    }
}

/// EDGE's coverage (0 to 255) of each pixel of AREA, row by row (`area.w * area.h` bytes), anti-aliased: a pixel's
/// distance from the path against half the width, the dashes by the length along the path, and the arrow filled.
/// Each pixel's value depends only on the edge, so a piece of AREA gives the pixels of the whole.
pub fn coverage(edge: &Edge, area: Rect) -> Vec<u8> {
    let (w, h) = (area.w.max(0) as usize, area.h.max(0) as usize);
    let mut out = vec![0u8; w * h];
    if w == 0 || h == 0 {
        return out;
    }
    let o = outline(edge);
    let mut lines = Vec::new();
    for s in &o.segs {
        match *s {
            Seg::Line(a, b) => lines.push((a, b)),
            Seg::Quad(a, c, b) => flatten(a, c, b, &mut lines),
        }
    }
    let hw = edge.width_px.max(1) as f32 / 2.0;
    let dash = edge.dash_px as f32;
    let mut put = |x: i32, y: i32, cov: f32| {
        let i = (y - area.y) as usize * w + (x - area.x) as usize;
        let v = (cov.clamp(0.0, 1.0) * 255.0).round() as u8;
        if v > out[i] {
            out[i] = v;
        }
    };
    // the pixels of AREA within the rectangle of corners P0, P1 grown by G
    let span = |p0: (f32, f32), p1: (f32, f32), g: f32| {
        let x0 = ((p0.0.min(p1.0) - g).floor() as i32).max(area.x);
        let y0 = ((p0.1.min(p1.1) - g).floor() as i32).max(area.y);
        let x1 = ((p0.0.max(p1.0) + g).ceil() as i32).min(area.x + area.w - 1);
        let y1 = ((p0.1.max(p1.1) + g).ceil() as i32).min(area.y + area.h - 1);
        (x0, y0, x1, y1)
    };
    let mut along = 0.0f32;
    for (a, b) in lines {
        let (dx, dy, len) = dir(a, b);
        let (x0, y0, x1, y1) = span(a, b, hw + 1.0);
        for y in y0..=y1 {
            for x in x0..=x1 {
                let p = (x as f32 + 0.5, y as f32 + 0.5);
                let t = ((p.0 - a.0) * dx + (p.1 - a.1) * dy).clamp(0.0, len);
                if dash > 0.0 && ((along + t) / dash) as i64 % 2 == 1 {
                    continue;
                }
                let (qx, qy) = (a.0 + dx * t - p.0, a.1 + dy * t - p.1);
                put(x, y, hw + 0.5 - (qx * qx + qy * qy).sqrt());
            }
        }
        along += len;
    }
    if let Some(tri) = o.arrow {
        // each side's outward normal: a pixel's coverage from its distance past the farthest side
        let centre = (
            (tri[0].0 + tri[1].0 + tri[2].0) / 3.0,
            (tri[0].1 + tri[1].1 + tri[2].1) / 3.0,
        );
        let sides: Vec<(f32, f32, f32)> = (0..3)
            .filter_map(|i| {
                let (p, q) = (tri[i], tri[(i + 1) % 3]);
                let (dx, dy, len) = dir(p, q);
                (len > 0.0).then(|| {
                    let (mut nx, mut ny) = (dy, -dx);
                    if nx * (centre.0 - p.0) + ny * (centre.1 - p.1) > 0.0 {
                        (nx, ny) = (-nx, -ny);
                    }
                    (nx, ny, nx * p.0 + ny * p.1)
                })
            })
            .collect();
        let lo = (
            tri.iter().map(|p| p.0).fold(f32::MAX, f32::min),
            tri.iter().map(|p| p.1).fold(f32::MAX, f32::min),
        );
        let hi = (
            tri.iter().map(|p| p.0).fold(f32::MIN, f32::max),
            tri.iter().map(|p| p.1).fold(f32::MIN, f32::max),
        );
        let (x0, y0, x1, y1) = span(lo, hi, 1.0);
        for y in y0..=y1 {
            for x in x0..=x1 {
                let p = (x as f32 + 0.5, y as f32 + 0.5);
                let d = sides
                    .iter()
                    .map(|&(nx, ny, c)| nx * p.0 + ny * p.1 - c)
                    .fold(f32::MIN, f32::max);
                put(x, y, 0.5 - d);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn edge(points: &[(i32, i32)]) -> Edge {
        Edge {
            points: points.into(),
            width_px: 1,
            radius_px: 6,
            argb: 0xffff_ffff,
            ..Edge::default()
        }
    }

    fn at(cov: &[u8], area: Rect, x: i32, y: i32) -> u8 {
        cov[((y - area.y) * area.w + (x - area.x)) as usize]
    }

    /// A straight edge is one line through its pixels' centres.
    #[test]
    fn a_straight_edge_is_a_line() {
        let o = outline(&edge(&[(10, 20), (50, 20)]));
        assert_eq!(o.segs, vec![Seg::Line((10.5, 20.5), (50.5, 20.5))]);
        assert_eq!(o.arrow, None);
    }

    /// A bend is a quadratic from `radius` before the corner to `radius` after it, the corner its control point.
    #[test]
    fn a_bend_is_rounded_by_the_radius() {
        let o = outline(&edge(&[(10, 20), (50, 20), (50, 60)]));
        assert_eq!(
            o.segs,
            vec![
                Seg::Line((10.5, 20.5), (44.5, 20.5)),
                Seg::Quad((44.5, 20.5), (50.5, 20.5), (50.5, 26.5)),
                Seg::Line((50.5, 26.5), (50.5, 60.5)),
            ]
        );
    }

    /// Two bends closer than twice the radius share the side between them: each takes half of it.
    #[test]
    fn close_bends_share_the_side() {
        let o = outline(&edge(&[(0, 0), (20, 0), (20, 4), (40, 4)]));
        assert_eq!(o.segs[1], Seg::Quad((18.5, 0.5), (20.5, 0.5), (20.5, 2.5)));
        assert_eq!(o.segs[2], Seg::Quad((20.5, 2.5), (20.5, 4.5), (22.5, 4.5)));
    }

    /// An arrowhead ends the edge: the line stops at its base, its tip is the end point.
    #[test]
    fn an_arrow_ends_at_the_tip() {
        let o = outline(&Edge {
            arrow_px: 6,
            ..edge(&[(10, 20), (50, 20)])
        });
        assert_eq!(o.segs, vec![Seg::Line((10.5, 20.5), (44.5, 20.5))]);
        assert_eq!(o.arrow, Some([(50.5, 20.5), (44.5, 17.0), (44.5, 24.0)]));
    }

    /// The bounds hold the line's width and the arrow; one point draws nothing.
    #[test]
    fn bounds_hold_width_and_arrow() {
        assert!(bounds(&edge(&[(5, 5)])).is_empty());
        let b = bounds(&Edge {
            arrow_px: 6,
            ..edge(&[(10, 20), (50, 20)])
        });
        assert!(b.x <= 10 && b.x + b.w >= 51, "{b:?}");
        assert!(b.y <= 17 && b.y + b.h >= 25, "{b:?}");
    }

    /// A 1 px horizontal edge covers its row fully and the rows beside it not at all; a 2 px one two rows.
    #[test]
    fn a_line_covers_its_pixels() {
        let area = Rect::new(0, 10, 60, 20);
        let c = coverage(&edge(&[(10, 20), (50, 20)]), area);
        assert_eq!(at(&c, area, 30, 20), 255);
        assert_eq!(at(&c, area, 30, 19), 0);
        assert_eq!(at(&c, area, 30, 21), 0);
        assert_eq!(at(&c, area, 5, 20), 0, "nothing before the start");
        let c = coverage(
            &Edge {
                width_px: 2,
                ..edge(&[(10, 20), (50, 20)])
            },
            area,
        );
        assert_eq!((at(&c, area, 30, 19), at(&c, area, 30, 20)), (255, 255));
        assert_eq!((at(&c, area, 30, 18), at(&c, area, 30, 21)), (0, 0));
    }

    /// A bend is anti-aliased: on the curve some pixels are partly covered, and the corner itself is not reached.
    #[test]
    fn a_bend_is_smooth() {
        let area = Rect::new(0, 0, 80, 80);
        let c = coverage(&edge(&[(10, 20), (50, 20), (50, 60)]), area);
        assert_eq!(at(&c, area, 50, 20), 0, "the corner is cut by the curve");
        let partial = (44..=50)
            .flat_map(|x| (20..=26).map(move |y| (x, y)))
            .filter(|&(x, y)| (1..255).contains(&at(&c, area, x, y)))
            .count();
        assert!(partial >= 3, "anti-aliased: {partial} partial pixels");
        assert_eq!(at(&c, area, 50, 40), 255, "the vertical side");
    }

    /// The arrowhead is filled; a dashed edge has gaps of the dash's length.
    #[test]
    fn arrow_filled_and_dashes_gapped() {
        let area = Rect::new(0, 0, 80, 40);
        let c = coverage(
            &Edge {
                arrow_px: 6,
                ..edge(&[(10, 20), (50, 20)])
            },
            area,
        );
        assert_eq!(at(&c, area, 47, 20), 255, "inside the arrow");
        assert!(at(&c, area, 46, 18) > 0, "the arrow is wider than the line");
        let c = coverage(
            &Edge {
                dash_px: 3,
                ..edge(&[(10, 20), (50, 20)])
            },
            area,
        );
        let on: Vec<bool> = (10..22).map(|x| at(&c, area, x, 20) > 127).collect();
        assert_eq!(
            on,
            [
                true, true, true, false, false, false, true, true, true, false, false, false
            ]
        );
    }

    /// Only AREA is computed: a piece of the edge gives the same pixels as the whole.
    #[test]
    fn a_piece_matches_the_whole() {
        let e = Edge {
            arrow_px: 6,
            ..edge(&[(10, 20), (50, 20), (50, 60)])
        };
        let all = Rect::new(0, 0, 80, 80);
        let piece = Rect::new(40, 15, 20, 20);
        let (ca, cp) = (coverage(&e, all), coverage(&e, piece));
        for y in piece.y..piece.y + piece.h {
            for x in piece.x..piece.x + piece.w {
                assert_eq!(at(&ca, all, x, y), at(&cp, piece, x, y), "{x},{y}");
            }
        }
    }
}

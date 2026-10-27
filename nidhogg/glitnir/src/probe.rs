//! Image probes: measurements on an ARGB32 picture (native byte order) that tests and the visual gates read.
//!
//! Luminance is taken from the sRGB-encoded values (no linearisation), 0.0 to 1.0.

use crate::geom::Rect;

/// An edge measurement (see [`edges`]).  Layout equals `GlitnirEdgeStats`.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct EdgeStats {
    /// Mean 10-90 % rise distance, in pixels, of the runs with amplitude >= 0.5.
    pub spread_px: f64,
    /// Mean amplitude of the runs with amplitude >= 0.2.
    pub contrast: f64,
    /// The number of runs with amplitude >= 0.5.
    pub runs: u32,
}

/// A rule measurement (see [`rule`]).  Layout equals `GlitnirRuleStats`.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct RuleStats {
    /// Pixels in the heaviest run of the darkness profile.
    pub width: f64,
    /// The largest darkness above the profile's minimum in that run.
    pub peak: f64,
    /// The sum of darkness above the minimum over that run (1.0 for a one-pixel black rule on white).
    pub mass: f64,
}

/// A picture: `argb` holds `h` rows of `stride` bytes, the first `4 * w` of each being pixels.
#[derive(Clone, Copy)]
pub struct Image<'a> {
    pub argb: &'a [u8],
    pub stride: usize,
    pub w: usize,
    pub h: usize,
}

fn lum_of(p: u32) -> f64 {
    let r = f64::from((p >> 16) & 255);
    let g = f64::from((p >> 8) & 255);
    let b = f64::from(p & 255);
    (0.2126 * r + 0.7152 * g + 0.0722 * b) / 255.0
}

impl Image<'_> {
    /// Whether the buffer holds `w` x `h` pixels at `stride`.
    pub fn is_valid(&self) -> bool {
        let Some(row) = self.w.checked_mul(4) else {
            return false;
        };
        if self.w == 0 || self.h == 0 || self.stride < row {
            return false;
        }
        self.stride
            .checked_mul(self.h - 1)
            .and_then(|n| n.checked_add(row))
            .is_some_and(|n| n <= self.argb.len())
    }

    fn pixel(&self, x: usize, y: usize) -> u32 {
        let i = y * self.stride + x * 4;
        u32::from_ne_bytes([
            self.argb[i],
            self.argb[i + 1],
            self.argb[i + 2],
            self.argb[i + 3],
        ])
    }

    fn lum(&self, x: usize, y: usize) -> f64 {
        lum_of(self.pixel(x, y))
    }

    /// `r` as unsigned bounds inside the picture, or `None` when empty or outside.
    fn clip(&self, r: Rect) -> Option<(usize, usize, usize, usize)> {
        if r.is_empty() || r.x < 0 || r.y < 0 {
            return None;
        }
        let (x, y, w, h) = (r.x as usize, r.y as usize, r.w as usize, r.h as usize);
        (x.checked_add(w)? <= self.w && y.checked_add(h)? <= self.h).then_some((x, y, w, h))
    }
}

/// Walk `v` and call `f(run)` for every maximal monotonic run (a step of zero ends one).
fn for_runs(v: &[f64], mut f: impl FnMut(&[f64])) {
    let mut start = 0;
    let mut sign = 0i32;
    for i in 1..v.len() {
        let d = v[i] - v[i - 1];
        let s = if d > 1e-9 {
            1
        } else if d < -1e-9 {
            -1
        } else {
            0
        };
        if s != sign {
            if sign != 0 {
                f(&v[start..i]);
            }
            start = i - 1;
            sign = s;
        }
    }
    if sign != 0 {
        f(&v[start..]);
    }
}

/// Where the run (monotonic, first to last) crosses `frac` of its amplitude, as a position in samples.
fn crossing(run: &[f64], frac: f64) -> f64 {
    let (a, b) = (run[0], run[run.len() - 1]);
    let target = a + (b - a) * frac;
    for i in 1..run.len() {
        let (p, q) = (run[i - 1], run[i]);
        if (q - target) * (p - target) <= 0.0 && q != p {
            return (i - 1) as f64 + (target - p) / (q - p);
        }
    }
    (run.len() - 1) as f64
}

/// R9's edge metric over `region`: monotonic runs of luminance along every row and every column.
pub fn edges(img: Image, region: Rect) -> Option<EdgeStats> {
    let (x0, y0, w, h) = img.clip(region)?;
    let (mut spread, mut n_spread, mut contrast, mut n_contrast) = (0.0, 0u32, 0.0, 0u32);
    let mut take = |run: &[f64]| {
        let amp = (run[run.len() - 1] - run[0]).abs();
        if amp >= 0.5 {
            spread += crossing(run, 0.9) - crossing(run, 0.1);
            n_spread += 1;
        }
        if amp >= 0.2 {
            contrast += amp;
            n_contrast += 1;
        }
    };
    let mut line = Vec::new();
    for y in y0..y0 + h {
        line.clear();
        line.extend((x0..x0 + w).map(|x| img.lum(x, y)));
        for_runs(&line, &mut take);
    }
    for x in x0..x0 + w {
        line.clear();
        line.extend((y0..y0 + h).map(|y| img.lum(x, y)));
        for_runs(&line, &mut take);
    }
    Some(EdgeStats {
        spread_px: if n_spread > 0 {
            spread / f64::from(n_spread)
        } else {
            0.0
        },
        contrast: if n_contrast > 0 {
            contrast / f64::from(n_contrast)
        } else {
            0.0
        },
        runs: n_spread,
    })
}

/// A rule's darkness profile across `band` (along x for a vertical rule, along y otherwise): darkness is
/// `1 - luminance` averaged along the rule; the runs above `min + 0.02` are measured, the heaviest is reported.
pub fn rule(img: Image, band: Rect, vertical: bool) -> Option<RuleStats> {
    let (x0, y0, w, h) = img.clip(band)?;
    let (across, along) = if vertical { (w, h) } else { (h, w) };
    let profile: Vec<f64> = (0..across)
        .map(|i| {
            let sum: f64 = (0..along)
                .map(|j| {
                    if vertical {
                        img.lum(x0 + i, y0 + j)
                    } else {
                        img.lum(x0 + j, y0 + i)
                    }
                })
                .sum();
            1.0 - sum / along as f64
        })
        .collect();
    let min = profile.iter().copied().fold(f64::INFINITY, f64::min);
    let mut best = RuleStats::default();
    let mut cur = RuleStats::default();
    for &d in profile.iter().chain(std::iter::once(&f64::NEG_INFINITY)) {
        if d > min + 0.02 {
            cur.width += 1.0;
            cur.mass += d - min;
            cur.peak = cur.peak.max(d - min);
        } else {
            if cur.mass > best.mass {
                best = cur;
            }
            cur = RuleStats::default();
        }
    }
    Some(best)
}

/// The pixels of `fill`, inset by `inset` on every side, whose luminance is off `argb`'s by more than 1/255.
pub fn seams(img: Image, fill: Rect, argb: u32, inset: u32) -> Option<u32> {
    let (x0, y0, w, h) = img.clip(fill)?;
    let inset = inset as usize;
    if w <= 2 * inset || h <= 2 * inset {
        return Some(0);
    }
    let want = lum_of(argb);
    let mut count = 0u32;
    for y in y0 + inset..y0 + h - inset {
        for x in x0 + inset..x0 + w - inset {
            if (img.lum(x, y) - want).abs() > 1.0 / 255.0 {
                count += 1;
            }
        }
    }
    Some(count)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A gray picture from `rows` of 0..=255 values (opaque).
    fn gray<const N: usize>(rows: &[[u8; N]]) -> (Vec<u8>, usize, usize) {
        let (h, w) = (rows.len(), rows[0].len());
        let mut v = Vec::new();
        for r in rows {
            for &g in r {
                v.extend_from_slice(&u32::to_ne_bytes(
                    0xff00_0000 | u32::from(g) << 16 | u32::from(g) << 8 | u32::from(g),
                ));
            }
        }
        (v, w, h)
    }

    fn img(v: &[u8], w: usize, h: usize) -> Image<'_> {
        Image {
            argb: v,
            stride: w * 4,
            w,
            h,
        }
    }

    #[test]
    fn ideal_step_reads_0_8() {
        let (v, w, h) = gray(&[[0, 0, 0, 255, 255, 255]; 4]);
        let e = edges(img(&v, w, h), Rect::new(0, 0, 6, 4)).unwrap();
        assert!((e.spread_px - 0.8).abs() < 1e-9, "{e:?}");
        assert!((e.contrast - 1.0).abs() < 1e-9);
        assert_eq!(e.runs, 4);
    }

    #[test]
    fn blurred_step_is_wider() {
        let (v, w, h) = gray(&[[0, 0, 85, 170, 255, 255]; 4]);
        let e = edges(img(&v, w, h), Rect::new(0, 0, 6, 4)).unwrap();
        assert!(e.spread_px > 2.0, "{e:?}");
    }

    #[test]
    fn one_px_rule_mass_is_1() {
        let (v, w, h) = gray(&[[255, 255, 0, 255, 255]; 6]);
        let r = rule(img(&v, w, h), Rect::new(0, 0, 5, 6), true).unwrap();
        assert!((r.mass - 1.0).abs() < 1e-9, "{r:?}");
        assert!((r.width - 1.0).abs() < 1e-9 && (r.peak - 1.0).abs() < 1e-9);
        let (v, w, h) = gray(&[[255; 6], [0; 6], [255; 6]]);
        let r = rule(img(&v, w, h), Rect::new(0, 0, 6, 3), false).unwrap();
        assert!((r.mass - 1.0).abs() < 1e-9, "{r:?}");
    }

    #[test]
    fn seams_counts_off_colour_pixels() {
        let (v, w, h) = gray(&[
            [9, 9, 9, 9, 9],
            [9, 100, 9, 9, 9],
            [9, 9, 9, 9, 9],
            [9, 9, 9, 9, 9],
        ]);
        let fill = 0xff09_0909;
        assert_eq!(
            seams(img(&v, w, h), Rect::new(0, 0, 5, 4), fill, 0),
            Some(1)
        );
        // inset 2 leaves no interior pixel off the colour
        assert_eq!(
            seams(img(&v, w, h), Rect::new(0, 0, 5, 4), fill, 2),
            Some(0)
        );
        assert_eq!(seams(img(&v, w, h), Rect::new(0, 0, 6, 4), fill, 0), None);
    }
}

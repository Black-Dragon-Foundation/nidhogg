//! The compose kernels against a per-channel scalar reference and pixman, corner coverage and rings against
//! 16×16 supersampling, shadow tiles against an 8×8-supersampled rounded rectangle ⊛ an exact Gaussian.
#![allow(clippy::needless_range_loop)]

use super::*;
use crate::shadow::{ShadowCache, ShadowSpec, ShadowTiles, step_for};
use std::sync::{Arc, OnceLock};

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
    fn byte(&mut self) -> u8 {
        (self.next() >> 24) as u8
    }
    /// A premultiplied pixel (no channel above the alpha); often fully clear or fully opaque.
    fn premul(&mut self) -> u32 {
        let x = (self.next() >> 16) as u32;
        let a = match self.below(8) {
            0 => 0,
            1 => 255,
            _ => x >> 24,
        };
        let c = |s: u32| ((x >> s) & 0xff).min(a);
        (a << 24) | (c(16) << 16) | (c(8) << 8) | c(0)
    }
}

fn seed(what: &str) -> u64 {
    let s = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(1)
        | 1;
    eprintln!("{what} test seed {s}");
    s
}

// ---- the scalar reference: one channel at a time, x·a/255 rounded as pixman does --------------------------------

fn mul(x: u32, a: u32) -> u32 {
    let t = x * a + 128;
    (t + (t >> 8)) >> 8
}
fn chan(p: u32, i: u32) -> u32 {
    (p >> (8 * i)) & 0xff
}
fn scale(p: u32, a: u32) -> u32 {
    (0..4).fold(0, |o, i| o | (mul(chan(p, i), a) << (8 * i)))
}
fn over_ref(d: u32, s: u32) -> u32 {
    let ia = 255 - (s >> 24);
    (0..4).fold(0, |o, i| {
        o | ((chan(s, i) + mul(chan(d, i), ia)).min(255) << (8 * i))
    })
}

/// The five row operations, by number: copy, constant-alpha OVER, OVER, masked OVER, solid colour through a mask.
const OPS: [&str; 5] = ["copy", "over_const", "over", "mask_over", "solid_over"];

struct Job {
    w: usize,
    h: usize,
    ds: usize,
    ss: usize,
    ms: usize,
    dst: Vec<u32>,
    src: Vec<u32>,
    mask: Vec<u8>,
    alpha: u8,
    colour: u32,
}

impl Job {
    fn random(r: &mut Rng, max_w: usize, max_h: usize) -> Job {
        let (w, h) = (1 + r.below(max_w), 1 + r.below(max_h));
        let (ds, ss, ms) = (w + r.below(9), w + r.below(9), w + r.below(9));
        Job {
            w,
            h,
            ds,
            ss,
            ms,
            dst: (0..ds * h).map(|_| r.premul()).collect(),
            src: (0..ss * h).map(|_| r.premul()).collect(),
            mask: (0..ms * h).map(|_| r.byte()).collect(),
            alpha: r.byte(),
            colour: r.premul(),
        }
    }

    fn run(&self, k: &Kernels, op: usize) -> Vec<u32> {
        let mut d = self.dst.clone();
        let mut dv = PixelsMut {
            data: &mut d,
            stride: self.ds,
        };
        let sv = Pixels {
            data: &self.src,
            stride: self.ss,
        };
        let mv = Mask {
            data: &self.mask,
            stride: self.ms,
        };
        let (w, h) = (self.w, self.h);
        match op {
            0 => k.copy(&mut dv, &sv, w, h),
            1 => k.over_const(&mut dv, &sv, w, h, self.alpha),
            2 => k.over(&mut dv, &sv, w, h),
            3 => k.mask_over(&mut dv, &sv, &mv, w, h, self.alpha),
            _ => k.solid_over(&mut dv, self.colour, &mv, w, h),
        }
        d
    }

    fn reference(&self, op: usize) -> Vec<u32> {
        let mut d = self.dst.clone();
        for y in 0..self.h {
            for x in 0..self.w {
                let (dp, s, m) = (
                    &mut d[y * self.ds + x],
                    self.src[y * self.ss + x],
                    self.mask[y * self.ms + x] as u32,
                );
                let a = self.alpha as u32;
                *dp = match op {
                    0 => s,
                    1 => over_ref(*dp, scale(s, a)),
                    2 => over_ref(*dp, s),
                    3 => over_ref(*dp, scale(s, mul(m, a))),
                    _ => over_ref(*dp, scale(self.colour, m)),
                };
            }
        }
        d
    }
}

#[test]
fn baseline_and_avx2_identical() {
    let mut r = Rng(seed("kernels"));
    let mut tables = vec![baseline()];
    match avx2() {
        Some(k) => {
            assert_eq!(kernels().name, k.name, "AVX2 is here, so it is the choice");
            tables.push(k);
        }
        None => eprintln!("no AVX2 build on this machine: the baseline against the reference only"),
    }
    for _ in 0..300 {
        let job = Job::random(&mut r, 70, 6);
        for op in 0..OPS.len() {
            let want = job.reference(op);
            for k in &tables {
                assert!(
                    job.run(k, op) == want,
                    "{} {} differs from the scalar reference ({}x{})",
                    k.name,
                    OPS[op],
                    job.w,
                    job.h
                );
            }
        }
    }
}

#[cfg(feature = "pixman-oracle")]
#[test]
fn over_equals_pixman_bit_for_bit() {
    use crate::oracle::{Masking, Source, over};
    let mut r = Rng(seed("pixman"));
    let (w, h) = (100, 100); // 10 000 pixels; every row its own alpha
    let mut tables = vec![baseline()];
    tables.extend(avx2());
    for k in tables {
        for op in 1..OPS.len() {
            let mut job = Job::random(&mut r, 1, 1);
            job.w = w;
            job.h = h;
            job.ds = w + 3;
            job.ss = w + 5;
            job.ms = w + 7;
            job.dst = (0..job.ds * h).map(|_| r.premul()).collect();
            job.src = (0..job.ss * h).map(|_| r.premul()).collect();
            job.mask = (0..job.ms * h).map(|_| r.byte()).collect();
            let alphas: Vec<u8> = (0..h).map(|_| r.byte()).collect();
            let mut ours = job.dst.clone();
            let mut theirs = job.dst.clone();
            for y in 0..h {
                let a = alphas[y];
                let src = &job.src[y * job.ss..y * job.ss + w];
                let mask = &job.mask[y * job.ms..y * job.ms + w];
                let mut dv = PixelsMut {
                    data: &mut ours[y * job.ds..y * job.ds + w],
                    stride: w,
                };
                let sv = Pixels {
                    data: src,
                    stride: w,
                };
                let mv = Mask {
                    data: mask,
                    stride: w,
                };
                // pixman takes one mask: the masked OVER's mask × alpha is formed first, exactly as ours does.
                let scaled: Vec<u8> = mask
                    .iter()
                    .map(|&m| mul(m as u32, a as u32) as u8)
                    .collect();
                let td = &mut theirs[y * job.ds..y * job.ds + w];
                match op {
                    1 => {
                        k.over_const(&mut dv, &sv, w, 1, a);
                        over(td, w, Source::Image(src, w), Masking::Solid(a), w, 1);
                    }
                    2 => {
                        k.over(&mut dv, &sv, w, 1);
                        over(td, w, Source::Image(src, w), Masking::None, w, 1);
                    }
                    3 => {
                        k.mask_over(&mut dv, &sv, &mv, w, 1, a);
                        over(td, w, Source::Image(src, w), Masking::A8(&scaled, w), w, 1);
                    }
                    _ => {
                        k.solid_over(&mut dv, job.colour, &mv, w, 1);
                        over(td, w, Source::Solid(job.colour), Masking::A8(mask, w), w, 1);
                    }
                }
            }
            for (i, (o, t)) in ours.iter().zip(&theirs).enumerate() {
                assert_eq!(
                    o, t,
                    "{} {} differs from pixman at pixel {i}",
                    k.name, OPS[op]
                );
            }
        }
    }
}

/// 16×16 point samples per pixel of the 2r×2r square, counting those whose distance from (r, r) lies in (lo, hi].
fn supersampled(r: u32, lo: f64, hi: f64) -> Vec<f64> {
    let n = 2 * r as usize;
    let c = r as f64;
    let mut out = vec![0.0; n * n];
    for j in 0..n {
        for i in 0..n {
            let mut k = 0;
            for sy in 0..16 {
                for sx in 0..16 {
                    let x = i as f64 + (sx as f64 + 0.5) / 16.0 - c;
                    let y = j as f64 + (sy as f64 + 0.5) / 16.0 - c;
                    let d2 = x * x + y * y;
                    if d2 <= hi * hi && (lo < 0.0 || d2 > lo * lo) {
                        k += 1;
                    }
                }
            }
            out[j * n + i] = k as f64 / 256.0 * 255.0;
        }
    }
    out
}

#[test]
fn corner_coverage_within_1_of_255() {
    assert!(corner_coverage(0).is_empty());
    for r in (1..=40).chain([48, 64, 97]) {
        let got = corner_coverage(r);
        let want = supersampled(r, -1.0, r as f64);
        assert_eq!(got.len(), want.len(), "radius {r}: a 2r×2r square");
        for (i, (&g, &w)) in got.iter().zip(&want).enumerate() {
            assert!(
                (g as f64 - w).abs() <= 1.0,
                "radius {r}, pixel {i}: {g} vs {w:.2}/255"
            );
        }
    }
    assert!(
        Arc::ptr_eq(&corner_coverage(12), &corner_coverage(12)),
        "cached per radius"
    );
}

/// PD's corners (Task 14) against a Cairo-free reference: each pixel of a corner square takes 16×16 point samples,
/// each sample `beneath` beyond the outer arc, `ring` between the arcs, else the inside (`fill`, or the pixel as drawn
/// when `fill` is 0); every channel within 1/255. Outside the squares of the mask nothing changes.
#[test]
fn decor_corners_match_cairo_within_1() {
    let k = kernels();
    let (w, h, stride) = (70usize, 40usize, 75usize);
    let (bg, ring, beneath) = (0xff30_3060u32, 0xffff_8000u32, 0xff20_2020u32);
    let ch = |p: u32, s: u32| ((p >> s) & 0xff) as f64;
    for (r, b) in [
        (6u32, 1u32),
        (6, 2),
        (8, 2),
        (12, 2),
        (12, 4),
        (7, 1),
        (15, 3),
        (24, 2),
        (3, 3),
        (5, 0),
    ] {
        // 24 is clamped to the box's half height, 20
        let r = clamp_radius(w, h, r);
        for fill in [0u32, 0xff40_6040] {
            for mask in [15u32, 6, 9, 1] {
                let mut px = vec![bg; stride * h];
                k.decor_corners(
                    &mut PixelsMut {
                        data: &mut px,
                        stride,
                    },
                    w,
                    h,
                    r,
                    b,
                    fill,
                    ring,
                    beneath,
                    mask,
                );
                let rr = r as usize;
                let inside = if fill == 0 { bg } else { fill };
                if fill != 0 {
                    // the repaint of a run's end equals the box drawn over its background, bit for bit
                    let mut over = vec![fill; stride * h];
                    k.decor_corners(
                        &mut PixelsMut {
                            data: &mut over,
                            stride,
                        },
                        w,
                        h,
                        r,
                        b,
                        0,
                        ring,
                        beneath,
                        mask,
                    );
                    for (i, (&o, &p)) in over.iter().zip(&px).enumerate() {
                        assert!(
                            o == p || p == bg,
                            "r {r} b {b} mask {mask}: pixel {i} {o:x} vs {p:x}"
                        );
                    }
                }
                for y in 0..h {
                    for x in 0..stride {
                        let left = x < rr;
                        let top = y < rr;
                        let corner = match (left, x >= w - rr && x < w, top, y >= h - rr) {
                            (true, _, true, _) => Some(1),
                            (_, true, true, _) => Some(2),
                            (_, true, _, true) => Some(4),
                            (true, _, _, true) => Some(8),
                            _ => None,
                        };
                        let got = px[y * stride + x];
                        let Some(bit) = corner.filter(|c| mask & c != 0) else {
                            assert_eq!(got, bg, "r {r} b {b} mask {mask}: ({x}, {y}) changed");
                            continue;
                        };
                        // the arcs' centre in this corner, and the samples per region
                        let cx = if bit == 1 || bit == 8 {
                            rr as f64
                        } else {
                            (w - rr) as f64
                        };
                        let cy = if bit < 4 { rr as f64 } else { (h - rr) as f64 };
                        let (lo, hi) = ((r - b.min(r)) as f64, r as f64);
                        let mut n = [0u32; 3];
                        for sy in 0..16 {
                            for sx in 0..16 {
                                let dx = x as f64 + (sx as f64 + 0.5) / 16.0 - cx;
                                let dy = y as f64 + (sy as f64 + 0.5) / 16.0 - cy;
                                let d2 = dx * dx + dy * dy;
                                n[if d2 > hi * hi {
                                    0
                                } else if b > 0 && d2 > lo * lo {
                                    1
                                } else {
                                    2
                                }] += 1;
                            }
                        }
                        for s in [0, 8, 16, 24] {
                            let want = (ch(beneath, s) * n[0] as f64
                                + ch(ring, s) * n[1] as f64
                                + ch(inside, s) * n[2] as f64)
                                / 256.0;
                            assert!(
                                (ch(got, s) - want).abs() <= 1.0,
                                "r {r} b {b} fill {fill:x} ({x}, {y}) channel {s}: {} vs {want:.2}",
                                ch(got, s)
                            );
                        }
                    }
                }
            }
        }
    }
    assert!(
        Arc::ptr_eq(
            &decor_tile(6, 1, 0, ring, beneath),
            &decor_tile(6, 1, 0, ring, beneath)
        ),
        "cached per (radius, border, colours)"
    );
}

#[test]
fn ring_leaves_straight_edges_untouched() {
    let mut rng = Rng(seed("ring"));
    let k = kernels();
    let (w, h, stride) = (90usize, 60usize, 97usize);
    let colour = 0xff20_4060;
    for (r, b) in [(12u32, 2u32), (8, 1), (16, 3), (5, 5), (4, 7), (40, 2)] {
        let before: Vec<u32> = (0..stride * (h + 2)).map(|_| rng.premul()).collect();
        let mut px = before.clone();
        k.ring(
            &mut PixelsMut {
                data: &mut px,
                stride,
            },
            w,
            h,
            r,
            b,
            colour,
        );
        let rr = clamp_radius(w, h, r) as usize;
        let mut changed = [0usize; 4];
        for y in 0..h + 2 {
            for x in 0..stride {
                let corner_x = x < rr || (x >= w - rr && x < w);
                let corner_y = y < rr || (y >= h - rr && y < h);
                let i = y * stride + x;
                if corner_x && corner_y {
                    if px[i] != before[i] {
                        changed[(x >= rr) as usize + 2 * (y >= rr) as usize] += 1;
                    }
                } else {
                    assert_eq!(
                        px[i], before[i],
                        "r {r} b {b}: ({x}, {y}) is outside the corner squares"
                    );
                }
            }
        }
        assert!(
            changed.iter().all(|&c| c > 0),
            "r {r} b {b}: a ring in every corner ({changed:?})"
        );
        // The masks: ring + inside = the corner coverage; nothing inside within the straight border's width.
        let cov = ring_coverage(rr as u32, b);
        let outer = corner_coverage(rr as u32);
        let n = 2 * rr;
        assert_eq!(cov.size, n);
        for j in 0..n {
            for i in 0..n {
                let p = j * n + i;
                let sum = cov.ring[p] as i32 + cov.inner[p] as i32;
                assert!(
                    (sum - outer[p] as i32).abs() <= 1,
                    "r {r} b {b}: ring + inner at ({i}, {j})"
                );
                let band = b as usize;
                if i < band || j < band || i >= n - band || j >= n - band {
                    assert_eq!(
                        cov.inner[p], 0,
                        "r {r} b {b}: inner coverage under the border at ({i}, {j})"
                    );
                }
            }
        }
    }
}

// ---- shadows -----------------------------------------------------------------------------------------------

/// L-eval's three cases: rectangle width, height, corner radius, σ (device px).
const CASES: [(u32, u32, f32, f32); 3] = [
    (200, 120, 24.0, 20.0),
    (400, 240, 24.0, 60.0),
    (300, 200, 16.0, 8.0),
];

/// The 8×8-supersampled rounded rectangle ⊛ the exact separable Gaussian, with a 3.5σ margin.
struct Reference {
    margin: usize,
    w: usize,
    h: usize,
    img: Vec<f64>,
}

fn build_reference(rw: u32, rh: u32, r: f32, sigma: f32) -> Reference {
    let (rw, rh, r, sigma) = (rw as f64, rh as f64, r as f64, sigma as f64);
    let m = (3.5 * sigma).ceil() as usize;
    let (w, h) = (rw as usize + 2 * m, rh as usize + 2 * m);
    let inside = |x: f64, y: f64| {
        if x < 0.0 || y < 0.0 || x > rw || y > rh {
            return false;
        }
        let qx = (r - x).max(x - (rw - r)).max(0.0);
        let qy = (r - y).max(y - (rh - r)).max(0.0);
        qx * qx + qy * qy <= r * r
    };
    let mut cov = vec![0.0; w * h];
    for j in 0..h {
        for i in 0..w {
            let mut c = 0;
            for sy in 0..8 {
                for sx in 0..8 {
                    let x = i as f64 + (sx as f64 + 0.5) / 8.0 - m as f64;
                    let y = j as f64 + (sy as f64 + 0.5) / 8.0 - m as f64;
                    c += inside(x, y) as u32;
                }
            }
            cov[j * w + i] = c as f64 / 64.0;
        }
    }
    let kr = (4.0 * sigma).ceil() as isize;
    let k: Vec<f64> = (-kr..=kr)
        .map(|t| (-((t * t) as f64) / (2.0 * sigma * sigma)).exp())
        .collect();
    let ks: f64 = k.iter().sum();
    let mut tmp = vec![0.0; w * h];
    for j in 0..h {
        for i in 0..w {
            let mut a = 0.0;
            for (t, kv) in k.iter().enumerate() {
                let x = i as isize + t as isize - kr;
                if x >= 0 && (x as usize) < w {
                    a += kv * cov[j * w + x as usize];
                }
            }
            tmp[j * w + i] = a / ks;
        }
    }
    let mut img = vec![0.0; w * h];
    for j in 0..h {
        for i in 0..w {
            let mut a = 0.0;
            for (t, kv) in k.iter().enumerate() {
                let y = j as isize + t as isize - kr;
                if y >= 0 && (y as usize) < h {
                    a += kv * tmp[y as usize * w + i];
                }
            }
            img[j * w + i] = a / ks;
        }
    }
    Reference {
        margin: m,
        w,
        h,
        img,
    }
}

fn reference(case: usize) -> &'static Reference {
    static REFS: OnceLock<Vec<Reference>> = OnceLock::new();
    &REFS.get_or_init(|| {
        CASES
            .iter()
            .map(|&(w, h, r, s)| build_reference(w, h, r, s))
            .collect()
    })[case]
}

fn spec(radius: f32, blur: f32, scale: f64) -> ShadowSpec {
    ShadowSpec {
        radius,
        blur,
        spread: 0.0,
        colour: 0xff00_0000,
        scale,
    }
}

/// The largest error, in 1/255, of TILES as sampled against case CASE's reference.
fn max_error(case: usize, tiles: &ShadowTiles) -> f64 {
    let (w, h, _, _) = CASES[case];
    let rf = reference(case);
    let (iw, ih) = tiles.image_size(w, h);
    let shift = tiles.pad() as isize - rf.margin as isize;
    let mut row = vec![0u8; iw as usize];
    let mut worst: f64 = 0.0;
    for j in 0..rf.h {
        let ly = j as isize + shift;
        let in_y = ly >= 0 && ly < ih as isize;
        if in_y {
            tiles.coverage_row(w, h, ly as u32, 0, &mut row);
        }
        for i in 0..rf.w {
            let lx = i as isize + shift;
            let ours = if in_y && lx >= 0 && lx < iw as isize {
                row[lx as usize] as f64
            } else {
                0.0
            };
            worst = worst.max((ours - rf.img[j * rf.w + i] * 255.0).abs());
        }
    }
    worst
}

#[test]
fn shadow_tile_within_2_of_255() {
    let mut cache = ShadowCache::new();
    for (c, &(w, h, r, sigma)) in CASES.iter().enumerate() {
        let tiles = cache.get(w, h, &spec(r, 2.0 * sigma, 1.0));
        assert!(
            tiles.step() >= 2,
            "σ {sigma} ≥ 8: tiles at reduced resolution"
        );
        let err = max_error(c, &tiles);
        let mut by_step = Vec::new();
        for step in [1, 2, 4] {
            let t = ShadowTiles::build(w, h, &spec(r, 2.0 * sigma, 1.0), 8, step);
            by_step.push((step, max_error(c, &t)));
        }
        eprintln!(
            "{w}x{h} r{r} σ{sigma}: as cached (1/{}) {err:.2}/255; by resolution {by_step:.2?}",
            tiles.step()
        );
        assert!(err <= 2.0, "{w}x{h} r{r} σ{sigma}: {err:.2}/255 > 2/255");
        // DP-5: the coarsest resolution that passes
        if let Some(&(step, coarser)) = by_step.iter().find(|(s, _)| *s == 2 * tiles.step()) {
            assert!(
                coarser > 2.0,
                "{w}x{h} r{r} σ{sigma}: 1/{step} passes too ({coarser:.2}/255)"
            );
        }
    }
}

#[test]
fn four_samples_fail_the_gate() {
    for (c, &(w, h, r, sigma)) in CASES.iter().enumerate() {
        let t = ShadowTiles::build(w, h, &spec(r, 2.0 * sigma, 1.0), 4, step_for(sigma));
        let err = max_error(c, &t);
        eprintln!("{w}x{h} r{r} σ{sigma}: 4 samples: max error {err:.2}/255");
        assert!(
            err > 2.0,
            "{w}x{h} r{r} σ{sigma}: 4 samples pass ({err:.2}/255)"
        );
    }
}

#[test]
fn shadow_cache_key_includes_scale() {
    let mut cache = ShadowCache::new();
    let one = cache.get(100, 60, &spec(8.0, 16.0, 1.0));
    let two = cache.get(100, 60, &spec(8.0, 16.0, 2.0));
    assert!(!Arc::ptr_eq(&one, &two), "another scale is another entry");
    assert_eq!(cache.len(), 2);
    assert!(
        Arc::ptr_eq(&one, &cache.get(100, 60, &spec(8.0, 16.0, 1.0))),
        "a hit"
    );
    // Sizes past the size class's cap share their tiles; a small size has its own.
    let big = cache.get(900, 700, &spec(8.0, 16.0, 1.0));
    assert!(Arc::ptr_eq(
        &big,
        &cache.get(1000, 800, &spec(8.0, 16.0, 1.0))
    ));
    assert!(!Arc::ptr_eq(
        &big,
        &cache.get(40, 30, &spec(8.0, 16.0, 1.0))
    ));
    assert_eq!(cache.len(), 4);
}

#[test]
fn shadow_cache_is_bounded() {
    let mut cache = ShadowCache::new();
    let blur = |i: usize| 2.0 + i as f32 * 0.25;
    let held: Vec<_> = (0..ShadowCache::CAPACITY)
        .map(|i| cache.get(40, 30, &spec(4.0, blur(i), 1.0)))
        .collect();
    assert_eq!(cache.len(), ShadowCache::CAPACITY);
    // Use the oldest again; the next new entry evicts the least recently used one, the second.
    assert!(Arc::ptr_eq(
        &held[0],
        &cache.get(40, 30, &spec(4.0, blur(0), 1.0))
    ));
    cache.get(40, 30, &spec(4.0, 50.0, 1.0));
    assert_eq!(cache.len(), ShadowCache::CAPACITY);
    assert!(Arc::ptr_eq(
        &held[0],
        &cache.get(40, 30, &spec(4.0, blur(0), 1.0))
    ));
    assert!(!Arc::ptr_eq(
        &held[1],
        &cache.get(40, 30, &spec(4.0, blur(1), 1.0))
    ));
    for i in 0..3 * ShadowCache::CAPACITY {
        cache.get(40, 30, &spec(4.0, blur(ShadowCache::CAPACITY + i), 1.0));
    }
    assert_eq!(cache.len(), ShadowCache::CAPACITY);
}

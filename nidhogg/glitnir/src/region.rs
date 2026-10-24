//! Damage regions: a set of device-pixel rectangles that covers at least every added pixel.
//!
//! Past [`MAX_RECTS`] rectangles the pair whose bounding box adds the least area is merged until
//! the limit holds, so a region may cover more than was added but never less.

use crate::geom::Rect;

/// The most rectangles a region keeps.
pub const MAX_RECTS: usize = 32;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Region {
    rects: Vec<Rect>,
}

impl Region {
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds `r`; empty rectangles are ignored.
    pub fn add(&mut self, r: Rect) {
        if r.is_empty() {
            return;
        }
        self.rects.push(r);
        self.merge_down();
    }

    pub fn union(&mut self, o: &Region) {
        self.rects.extend_from_slice(&o.rects);
        self.merge_down();
    }

    /// The part of the region inside `r`.
    pub fn clip(&self, r: Rect) -> Region {
        Region {
            rects: self
                .rects
                .iter()
                .map(|x| x.intersect(&r))
                .filter(|x| !x.is_empty())
                .collect(),
        }
    }

    pub fn translate(&mut self, dx: i32, dy: i32) {
        for r in &mut self.rects {
            *r = r.translate(dx, dy);
        }
    }

    /// The summed area of the rectangles (overlaps counted once per rectangle).
    pub fn area(&self) -> u64 {
        self.rects.iter().map(Rect::area).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.rects.is_empty()
    }

    pub fn iter(&self) -> std::slice::Iter<'_, Rect> {
        self.rects.iter()
    }

    fn merge_down(&mut self) {
        while self.rects.len() > MAX_RECTS {
            let (mut bi, mut bj, mut best) = (0, 1, u64::MAX);
            for i in 0..self.rects.len() {
                for j in i + 1..self.rects.len() {
                    let (a, b) = (&self.rects[i], &self.rects[j]);
                    let cost = a.bounds(b).area().saturating_sub(a.area() + b.area());
                    if cost < best {
                        (bi, bj, best) = (i, j, cost);
                    }
                }
            }
            let b = self.rects.swap_remove(bj);
            self.rects[bi] = self.rects[bi].bounds(&b);
        }
    }
}

impl<'a> IntoIterator for &'a Region {
    type Item = &'a Rect;
    type IntoIter = std::slice::Iter<'a, Rect>;
    fn into_iter(self) -> Self::IntoIter {
        self.rects.iter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rng::XorShift;

    const N: i32 = 64;

    fn rand_rect(g: &mut XorShift) -> Rect {
        Rect::new(
            g.below(N as u32 + 8) as i32 - 4,
            g.below(N as u32 + 8) as i32 - 4,
            g.below(20) as i32 - 3,
            g.below(20) as i32 - 3,
        )
    }

    fn covers(reg: &Region, x: i32, y: i32) -> bool {
        reg.iter().any(|r| r.contains(x, y))
    }

    #[test]
    fn region_covers_every_added_pixel() {
        for seed in 1..=1000u64 {
            let mut g = XorShift::new(seed);
            let mut reg = Region::new();
            let mut bitmap = vec![false; (N * N) as usize];
            for _ in 0..g.below(80) {
                let r = rand_rect(&mut g);
                reg.add(r);
                for y in 0..N {
                    for x in 0..N {
                        if r.contains(x, y) {
                            bitmap[(y * N + x) as usize] = true;
                        }
                    }
                }
            }
            for y in 0..N {
                for x in 0..N {
                    assert!(
                        !bitmap[(y * N + x) as usize] || covers(&reg, x, y),
                        "seed {seed}: pixel {x},{y} lost"
                    );
                }
            }
            assert!(reg.iter().count() <= MAX_RECTS);
        }
    }

    #[test]
    fn merge_keeps_at_most_32() {
        let mut reg = Region::new();
        for i in 0..200 {
            reg.add(Rect::new(i * 10, (i % 7) * 10, 4, 4));
            assert!(reg.iter().count() <= MAX_RECTS);
        }
        let mut other = Region::new();
        for i in 0..40 {
            other.add(Rect::new(i * 3, 500, 1, 1));
        }
        reg.union(&other);
        assert!(reg.iter().count() <= MAX_RECTS);
        // The cheapest pair merges: two adjacent rects collapse before a distant one joins.
        let mut r = Region::new();
        for i in 0..32 {
            r.add(Rect::new(i * 100, 0, 10, 10));
        }
        r.add(Rect::new(10, 0, 10, 10));
        assert_eq!(r.iter().count(), 32);
        assert!(r.iter().any(|x| *x == Rect::new(0, 0, 20, 10)));
    }

    #[test]
    fn clip_and_translate_commute() {
        for seed in 1..=300u64 {
            let mut g = XorShift::new(seed);
            let mut reg = Region::new();
            for _ in 0..g.below(40) {
                reg.add(rand_rect(&mut g));
            }
            let c = rand_rect(&mut g);
            let (dx, dy) = (g.below(30) as i32 - 15, g.below(30) as i32 - 15);
            let mut a = reg.clip(c);
            a.translate(dx, dy);
            let mut t = reg.clone();
            t.translate(dx, dy);
            let b = t.clip(c.translate(dx, dy));
            assert_eq!(a, b, "seed {seed}");
        }
    }

    #[test]
    fn empty_rects_are_ignored() {
        let mut reg = Region::new();
        reg.add(Rect::new(0, 0, 0, 5));
        reg.add(Rect::new(0, 0, 5, -1));
        reg.add(Rect::default());
        assert!(reg.is_empty());
        assert_eq!(reg.area(), 0);
        reg.add(Rect::new(1, 1, 2, 3));
        assert_eq!(reg.area(), 6);
        assert!(reg.clip(Rect::new(10, 10, 5, 5)).is_empty());
    }
}

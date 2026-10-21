//! Pending damage per frame: what changed on a frame since its damage was last taken.
//!
//! The host reports the rectangles its drawing touched (device pixels), whole-frame damage and scroll operations.
//! Whole-frame damage is kept as [`ALL`], which the reader clips to the frame (the library does not know frame
//! sizes).  A rectangle that an earlier one already contains adds nothing, and a new rectangle drops the ones it
//! contains, so a frame cleared whole reads back as one rectangle.

use crate::geom::Rect;
use crate::region::Region;
use std::collections::BTreeMap;

/// Whole-frame damage: contains every rectangle a frame can have.
pub const ALL: Rect = Rect::new(0, 0, i32::MAX, i32::MAX);

/// The pending damage of one frame, and the last damage taken (borrowed by the caller until the next take).
#[derive(Debug, Default)]
pub struct Damage {
    region: Region,
    taken: Vec<Rect>,
}

fn contains(outer: &Rect, inner: &Rect) -> bool {
    !outer.is_empty()
        && inner.x >= outer.x
        && inner.y >= outer.y
        && inner.x as i64 + inner.w as i64 <= outer.x as i64 + outer.w as i64
        && inner.y as i64 + inner.h as i64 <= outer.y as i64 + outer.h as i64
}

impl Damage {
    /// Add `r`; an empty rectangle, or one the damage already contains, adds nothing.
    pub fn add(&mut self, r: Rect) {
        if r.is_empty() || self.region.iter().any(|x| contains(x, &r)) {
            return;
        }
        if self.region.iter().any(|x| contains(&r, x)) {
            let keep: Vec<Rect> = self
                .region
                .iter()
                .filter(|x| !contains(&r, x))
                .copied()
                .collect();
            self.region = Region::new();
            for k in keep {
                self.region.add(k);
            }
        }
        self.region.add(r);
    }

    /// A scroll of `area`'s pixels; `exposed` is the part the scroll left stale.  Without a scene to move, the
    /// whole area changed on screen.
    pub fn scroll(&mut self, area: Rect, exposed: Rect) {
        self.add(area);
        self.add(exposed);
    }

    /// Move the pending damage out (the frame then has none) and return it.
    pub fn take(&mut self) -> &[Rect] {
        self.taken.clear();
        self.taken.extend(self.region.iter().copied());
        self.region = Region::new();
        &self.taken
    }

    pub fn is_empty(&self) -> bool {
        self.region.is_empty()
    }
}

/// The pending damage of every frame.
pub type Frames = BTreeMap<u64, Damage>;

/// Run `f` on FRAME's damage (allocated at the first use).
pub fn with<R>(frame: u64, f: impl FnOnce(&mut Damage) -> R) -> R {
    let mut st = crate::STATE.lock().unwrap_or_else(|p| p.into_inner());
    f(st.damage.entry(frame).or_default())
}

/// Take FRAME's pending damage: a pointer to its rectangles and their number, valid until FRAME's next take or
/// forget.  An unknown frame has none (null, 0).
pub fn take(frame: u64) -> (*const Rect, usize) {
    let mut st = crate::STATE.lock().unwrap_or_else(|p| p.into_inner());
    match st.damage.get_mut(&frame) {
        Some(d) => {
            let t = d.take();
            (t.as_ptr(), t.len())
        }
        None => (std::ptr::null(), 0),
    }
}

/// Drop FRAME's damage (the frame is gone).
pub fn forget(frame: u64) {
    let mut st = crate::STATE.lock().unwrap_or_else(|p| p.into_inner());
    st.damage.remove(&frame);
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
            g.below(24) as i32 - 3,
            g.below(24) as i32 - 3,
        )
    }

    #[test]
    fn damage_covers_every_added_pixel() {
        for seed in 1..=500u64 {
            let mut g = XorShift::new(seed);
            let mut d = Damage::default();
            let mut added = Vec::new();
            for _ in 0..g.below(80) {
                let r = rand_rect(&mut g);
                if g.below(10) == 0 {
                    let e = rand_rect(&mut g);
                    d.scroll(r, e);
                    added.push(e);
                } else {
                    d.add(r);
                }
                added.push(r);
            }
            let out = d.take().to_vec();
            assert!(out.len() <= crate::region::MAX_RECTS, "seed {seed}");
            for r in &added {
                for y in r.y..r.y + r.h.max(0) {
                    for x in r.x..r.x + r.w.max(0) {
                        assert!(
                            out.iter().any(|o| o.contains(x, y)),
                            "seed {seed}: pixel {x},{y} lost"
                        );
                    }
                }
            }
            // no rectangle taken lies inside another one
            for (i, a) in out.iter().enumerate() {
                for (j, b) in out.iter().enumerate() {
                    assert!(i == j || !contains(a, b), "seed {seed}: {b:?} inside {a:?}");
                }
            }
        }
    }

    #[test]
    fn whole_frame_damage_absorbs_the_rest() {
        let mut d = Damage::default();
        d.add(Rect::new(5, 5, 10, 10));
        d.add(ALL);
        d.add(Rect::new(100, 200, 30, 4));
        assert_eq!(d.take(), &[ALL]);
    }

    #[test]
    fn a_frame_cleared_whole_reads_back_as_one_rect() {
        let mut d = Damage::default();
        d.add(Rect::new(0, 0, 800, 600));
        for y in 0..30 {
            d.add(Rect::new(8, y * 20, 600, 20));
        }
        assert_eq!(d.take(), &[Rect::new(0, 0, 800, 600)]);
    }

    #[test]
    fn take_empties_the_damage() {
        let mut d = Damage::default();
        d.add(Rect::new(1, 2, 3, 4));
        d.add(Rect::new(0, 0, 0, 9));
        assert!(!d.is_empty());
        assert_eq!(d.take(), &[Rect::new(1, 2, 3, 4)]);
        assert!(d.is_empty());
        assert!(d.take().is_empty());
    }

    #[test]
    fn scroll_damages_the_area_and_the_exposed_strip() {
        let mut d = Damage::default();
        d.scroll(Rect::new(0, 20, 400, 300), Rect::new(0, 280, 400, 40));
        let out = d.take().to_vec();
        assert!(out.contains(&Rect::new(0, 20, 400, 300)));
        assert!(
            out.iter()
                .any(|o| o.contains(0, 319) && o.contains(399, 300))
        );
    }

    #[test]
    fn frames_are_separate_and_forgotten() {
        let (a, b) = (7001, 7002);
        with(a, |d| d.add(Rect::new(0, 0, 4, 4)));
        with(b, |d| d.add(Rect::new(9, 9, 1, 1)));
        let (p, n) = take(a);
        assert_eq!(n, 1);
        // SAFETY: valid until A's next take or forget; nothing else touches A here.
        assert_eq!(unsafe { *p }, Rect::new(0, 0, 4, 4));
        assert_eq!(take(a).1, 0);
        forget(b);
        assert_eq!(take(b), (std::ptr::null(), 0));
        forget(a);
    }
}

//! The surface ring's rule (Naglfar, design §5.4): which of a window's 2–3 surfaces the next commit draws into, and
//! which of its pixels must be copied in first.  Platform-free, so the Linux checks test it; `macos::surface` holds
//! the IOSurfaces it names.
//!
//! **Ages and what a slot owes.**  A slot's age counts the commits since its pixels were the window's: the newest
//! committed slot is 1, the one before it 2, and 0 means a new slot, whose pixels are unknown.  A slot lacks the damage
//! of every commit made since it was drawn, which it keeps as it goes (a bounded number of rectangles, past which their
//! bounding box); that, plus the damage of the commit being made, is all that is copied into it.  So a slot the pick
//! leaves out for many commits (typing alternates the other two) copies what changed meanwhile, not the whole surface.
//!
//! **The pick.**  The newest slot is on screen, so it is drawn into only when it is the ring's only slot.  Among the
//! others that the window server does not hold (`IOSurfaceIsInUse`): the youngest drawn one, else a new one's
//! slot of unknown pixels, else a new slot while the ring is below its size.  When every other slot is held the oldest is reused
//! anyway, as NS does (`nsterm.m:11221-11228`), and counted as busy.  Committing the slot that is already the layer's
//! contents needs the contents set to nil first (NS `:11311-11312`): [`Ring::commit`] says when.

use glitnir::geom::Rect;

/// How many rectangles a slot keeps of what it owes; past it, their bounding box.
pub const OWED_RECTS: usize = 32;

/// What must be copied into a slot before it is committed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Fill {
    /// The whole surface.
    All,
    /// These rectangles, which do not overlap, inside the surface.
    Rects(Vec<Rect>),
}

impl Fill {
    /// The bytes the copy moves for a `w` x `h` surface of 4-byte pixels.
    pub fn bytes(&self, w: i32, h: i32) -> u64 {
        match self {
            Fill::All => Rect::new(0, 0, w, h).area() * 4,
            Fill::Rects(r) => r.iter().map(|r| r.area() * 4).sum(),
        }
    }
}

/// [`Ring::pick`]'s answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pick {
    /// The slot to draw into; equal to the slot count when a new one must be made.
    pub slot: usize,
    /// Every slot but the newest was held: the oldest is reused (`surface_busy`).
    pub busy: bool,
}

/// One window's ring.
#[derive(Debug)]
pub struct Ring {
    /// The most slots it may have (2 or 3; 1 in tests).
    size: usize,
    /// Each slot's age.
    ages: Vec<u32>,
    /// What each slot owes: the damage of the commits since it was drawn, newest first, not overlapping (`None`: a
    /// slot never drawn).
    owed: Vec<Option<Vec<Rect>>>,
    newest: Option<usize>,
}

/// `rects` clipped to `w` x `h` and cut so that none overlaps another (no pixel is copied twice).
pub fn disjoint(rects: &[Rect], w: i32, h: i32) -> Vec<Rect> {
    let all = Rect::new(0, 0, w, h);
    let mut out: Vec<Rect> = Vec::new();
    for r in rects {
        let mut parts = vec![r.intersect(&all)];
        for o in &out {
            parts = parts.iter().flat_map(|p| subtract(*p, *o)).collect();
        }
        out.extend(parts.into_iter().filter(|p| !p.is_empty()));
    }
    out
}

/// A − B: up to four rectangles, empty ones dropped.
fn subtract(a: Rect, b: Rect) -> impl Iterator<Item = Rect> {
    let i = a.intersect(&b);
    let parts = if i.is_empty() {
        [a, Rect::default(), Rect::default(), Rect::default()]
    } else {
        [
            Rect::new(a.x, a.y, a.w, i.y - a.y),
            Rect::new(a.x, i.y + i.h, a.w, a.y + a.h - (i.y + i.h)),
            Rect::new(a.x, i.y, i.x - a.x, i.h),
            Rect::new(i.x + i.w, i.y, a.x + a.w - (i.x + i.w), i.h),
        ]
    };
    parts.into_iter().filter(|r| !r.is_empty())
}

impl Ring {
    /// An empty ring of at most `size` slots.
    pub fn new(size: usize) -> Ring {
        Ring {
            size: size.max(1),
            ages: Vec::new(),
            owed: Vec::new(),
            newest: None,
        }
    }

    pub fn slots(&self) -> usize {
        self.ages.len()
    }

    pub fn age(&self, slot: usize) -> u32 {
        self.ages.get(slot).copied().unwrap_or(0)
    }

    /// The slot that was committed last.
    pub fn newest(&self) -> Option<usize> {
        self.newest
    }

    /// Forget every slot (the window's size changed: the surfaces are made again).
    pub fn clear(&mut self) {
        self.ages.clear();
        self.owed.clear();
        self.newest = None;
    }

    /// The slot the next commit draws into; `held` says whether the window server holds slot `i`.
    pub fn pick(&self, held: impl Fn(usize) -> bool) -> Pick {
        let free: Vec<usize> = (0..self.ages.len())
            .filter(|&i| Some(i) != self.newest && !held(i))
            .collect();
        if let Some(&i) = free
            .iter()
            .filter(|&&i| self.owed(i).is_some())
            .min_by_key(|&&i| self.ages[i])
        {
            return Pick {
                slot: i,
                busy: false,
            };
        }
        if let Some(&i) = free.first() {
            return Pick {
                slot: i,
                busy: false,
            };
        }
        if self.ages.len() < self.size {
            return Pick {
                slot: self.ages.len(),
                busy: false,
            };
        }
        // every other slot is held (or there is none): the oldest, or the newest when it is alone
        let others = (0..self.ages.len()).filter(|&i| Some(i) != self.newest);
        let oldest = others.max_by_key(|&i| match self.ages[i] {
            0 => u32::MAX,
            a => a,
        });
        match oldest {
            Some(i) => Pick {
                slot: i,
                busy: true,
            },
            None => {
                let i = self.newest.unwrap_or(0);
                Pick {
                    slot: i,
                    busy: held(i),
                }
            }
        }
    }

    /// What must be copied into `slot` for a commit whose own damage is `damage` (clipped to `w` x `h`; a rectangle
    /// that covers the surface makes it all).  A new slot (`slot` = the slot count) is copied whole.
    pub fn fill(&self, slot: usize, damage: &[Rect], w: i32, h: i32) -> Fill {
        let Some(owed) = self.owed(slot) else {
            return Fill::All;
        };
        let mut rects = owed.to_vec();
        rects.extend_from_slice(damage);
        let out = disjoint(&rects, w, h);
        let area: u64 = out.iter().map(Rect::area).sum();
        if area >= Rect::new(0, 0, w, h).area() {
            Fill::All
        } else {
            Fill::Rects(out)
        }
    }

    /// `slot` (or a new one, at the slot count) was committed with `damage`: it is the newest now.  True when it
    /// already was, so the layer's contents must be set to nil before they are set to it again.
    pub fn commit(&mut self, slot: usize, damage: &[Rect]) -> bool {
        if slot >= self.ages.len() {
            self.ages.resize(slot + 1, 0);
            self.owed.resize(slot + 1, None);
        }
        let same = self.newest == Some(slot);
        for (i, a) in self.ages.iter_mut().enumerate() {
            if i != slot && *a != 0 {
                *a = a.saturating_add(1);
            }
        }
        for (i, o) in self.owed.iter_mut().enumerate() {
            if let Some(o) = o.as_mut().filter(|_| i != slot) {
                owe(o, damage);
            }
        }
        self.ages[slot] = 1;
        self.owed[slot] = Some(Vec::new());
        self.newest = Some(slot);
        same
    }

    /// What `slot` owes (`None`: it was never drawn, or there is no such slot).
    pub fn owed(&self, slot: usize) -> Option<&[Rect]> {
        self.owed.get(slot).and_then(|o| o.as_deref())
    }
}

/// Add a commit's `damage` (inside the surface) to what a slot owes, newest first: cut so none overlaps, and past
/// [`OWED_RECTS`] rectangles their bounding box (it copies more, never less).
fn owe(owed: &mut Vec<Rect>, damage: &[Rect]) {
    if damage.is_empty() {
        return;
    }
    let mut all = damage.to_vec();
    all.append(owed);
    let bound = all.iter().fold(Rect::default(), |b, r| b.bounds(r));
    let all = disjoint(&all, bound.x + bound.w, bound.y + bound.h);
    *owed = if all.len() > OWED_RECTS {
        vec![bound]
    } else {
        all
    };
}

/// How long a commit may be held while the window server holds every other slot: three frames at 60 Hz.  Past it the
/// oldest slot is reused, as before the pacer, so a window whose surfaces stay in use (occluded, or a stuck window
/// server) still shows its frames.
pub const HOLD_BOUND_NS: u64 = 50_000_000;

/// [`Pacer::pace`]'s answer: what the publish does with its pick.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Paced {
    /// Draw into the picked slot: the held damage and this commit's.
    Draw(Vec<Rect>),
    /// Draw nothing: every other slot is held; the damage waits with the pacer.
    Hold,
    /// The hold outlasted [`HOLD_BOUND_NS`]: draw into the picked (held) slot anyway, the held damage and this
    /// commit's.
    Reuse(Vec<Rect>),
}

/// A window's commit pacer: a held slot is never drawn into; a commit that finds every other slot held waits, its
/// damage kept, until a publish finds one free.
#[derive(Debug, Default)]
pub struct Pacer {
    /// When the first held commit of the current hold was (ns), `None` when nothing is held.
    since: Option<u64>,
    /// The held commits' damage.
    held: Vec<Rect>,
}

impl Pacer {
    /// The publish at `now` (ns) picked `pick` for a commit with `damage`.
    pub fn pace(&mut self, pick: Pick, damage: &[Rect], now: u64) -> Paced {
        self.held.extend_from_slice(damage);
        if pick.busy {
            let since = *self.since.get_or_insert(now);
            if now.saturating_sub(since) < HOLD_BOUND_NS {
                return Paced::Hold;
            }
            self.since = None;
            return Paced::Reuse(std::mem::take(&mut self.held));
        }
        self.since = None;
        Paced::Draw(std::mem::take(&mut self.held))
    }

    /// Whether a commit is held: a publish is owed once a slot frees.
    pub fn owes(&self) -> bool {
        self.since.is_some()
    }

    /// Whether the held commit should be published now, at `now` (ns): a slot is free (`busy` false: the pick would
    /// not be busy), or the hold reached its bound.  False when nothing is held.
    pub fn due(&self, busy: bool, now: u64) -> bool {
        self.since
            .is_some_and(|s| !busy || now.saturating_sub(s) >= HOLD_BOUND_NS)
    }

    /// Forget the held commit (the surfaces are made again, copied whole).
    pub fn clear(&mut self) {
        self.since = None;
        self.held.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const W: i32 = 100;
    const H: i32 = 50;

    fn r(x: i32, y: i32, w: i32, h: i32) -> Rect {
        Rect::new(x, y, w, h)
    }

    fn pk(slot: usize, busy: bool) -> Pick {
        Pick { slot, busy }
    }

    fn area(f: &Fill) -> u64 {
        f.bytes(W, H) / 4
    }

    /// Commit `damage` into whatever the ring picks with nothing held; the slot.
    fn step(ring: &mut Ring, damage: &[Rect]) -> usize {
        let p = ring.pick(|_| false);
        ring.commit(p.slot, damage);
        p.slot
    }

    #[test]
    fn a_new_slot_is_copied_whole() {
        let ring = Ring::new(3);
        let p = ring.pick(|_| false);
        assert_eq!(p, pk(0, false));
        assert_eq!(ring.fill(p.slot, &[r(0, 0, 1, 1)], W, H), Fill::All);
    }

    #[test]
    fn the_damage_union_over_ages_one_to_three() {
        // ring of 3, nothing held: slots fill 0, 1, 2, then the youngest free slot is reused
        let mut ring = Ring::new(3);
        let a = r(0, 0, 10, 10);
        let b = r(20, 0, 10, 10);
        let c = r(40, 0, 10, 10);
        let d = r(60, 0, 10, 10);
        assert_eq!(step(&mut ring, &[a]), 0);
        assert_eq!(step(&mut ring, &[b]), 1);
        // age 2 (slot 0 after slot 1): the latest commit's damage plus this one's
        let p = ring.pick(|_| false);
        assert_eq!(p.slot, 0);
        assert_eq!(ring.age(0), 2);
        assert_eq!(ring.fill(0, &[c], W, H), Fill::Rects(vec![b, c]));
        // age 1 (the newest, alone in a ring of 1): only this commit's damage
        let mut one = Ring::new(1);
        step(&mut one, &[a]);
        let p1 = one.pick(|_| false);
        assert_eq!(p1.slot, 0);
        assert_eq!(one.age(0), 1);
        assert_eq!(one.fill(0, &[d], W, H), Fill::Rects(vec![d]));
        // age 3: with slot 2 held, slot 0 waited two commits
        let mut ring = Ring::new(3);
        step(&mut ring, &[a]); // 0
        step(&mut ring, &[b]); // 1
        let p = ring.pick(|_| false);
        assert_eq!(p.slot, 0);
        // make slot 2, then commit into it while 0 is held
        ring.commit(2, &[c]);
        assert_eq!(ring.age(0), 3);
        assert_eq!(ring.age(1), 2);
        let p = ring.pick(|i| i == 1);
        assert_eq!(p, pk(0, false));
        assert_eq!(ring.fill(0, &[d], W, H), Fill::Rects(vec![c, b, d]));
        assert_eq!(area(&ring.fill(0, &[d], W, H)), 300);
    }

    #[test]
    fn overlapping_damage_is_copied_once() {
        let mut ring = Ring::new(2);
        step(&mut ring, &[r(0, 0, 10, 10)]);
        step(&mut ring, &[r(0, 0, 10, 10)]);
        let p = ring.pick(|_| false);
        assert_eq!(ring.age(p.slot), 2);
        let f = ring.fill(p.slot, &[r(5, 5, 10, 10)], W, H);
        assert_eq!(area(&f), 100 + 75);
    }

    #[test]
    fn damage_over_the_whole_surface_is_a_full_copy() {
        let mut ring = Ring::new(2);
        step(&mut ring, &[r(0, 0, 1, 1)]);
        step(&mut ring, &[r(0, 0, W, H)]);
        let p = ring.pick(|_| false);
        assert_eq!(ring.fill(p.slot, &[], W, H), Fill::All);
        assert_eq!(ring.fill(p.slot, &[r(-5, -5, 1000, 1000)], W, H), Fill::All);
    }

    #[test]
    fn the_oldest_is_reused_when_all_are_busy() {
        let mut ring = Ring::new(3);
        for i in 0..3 {
            ring.commit(i, &[]); // slots 0, 1, 2; 2 newest, 0 oldest
        }
        assert_eq!(ring.slots(), 3);
        let p = ring.pick(|_| true);
        assert_eq!(p, pk(0, true));
        // a free one is preferred to the held ones, however old
        assert_eq!(ring.pick(|i| i != 1), pk(1, false));
        // the ring does not grow past its size
        ring.commit(0, &[]);
        assert_eq!(ring.slots(), 3);
    }

    #[test]
    fn a_held_newest_makes_a_new_slot_before_any_reuse() {
        let mut ring = Ring::new(2);
        step(&mut ring, &[]);
        assert_eq!(ring.pick(|_| true), pk(1, false));
    }

    #[test]
    fn same_surface_handling() {
        // a ring of 1 commits its only slot again: contents go to nil first
        let mut one = Ring::new(1);
        assert!(!one.commit(0, &[]));
        let p = one.pick(|_| false);
        assert_eq!(p, pk(0, false));
        assert!(one.commit(p.slot, &[]));
        // held, it is still the only choice, counted busy
        assert_eq!(one.pick(|_| true), pk(0, true));
        // a ring of 2 never picks the newest
        let mut two = Ring::new(2);
        for _ in 0..5 {
            let p = two.pick(|_| false);
            assert_ne!(Some(p.slot), two.newest());
            assert!(!two.commit(p.slot, &[]));
        }
    }

    #[test]
    fn a_slot_left_out_for_many_commits_copies_only_their_damage() {
        // typing: slots 0 and 1 alternate (the youngest free one is picked) while 2 waits, then 0 and 1 are held
        let mut ring = Ring::new(3);
        step(&mut ring, &[]); // 0
        step(&mut ring, &[]); // 1
        ring.commit(2, &[]);
        for k in 0..40 {
            let p = ring.pick(|i| i == 2);
            ring.commit(p.slot, &[r(k, 0, 1, 1)]);
        }
        let p = ring.pick(|i| i != 2);
        assert_eq!(p.slot, 2);
        // the 40 cells it missed and this commit's, not the whole surface
        let f = ring.fill(2, &[r(0, 10, 1, 1)], W, H);
        assert_eq!(area(&f), 41);
    }

    #[test]
    fn a_slot_owing_the_whole_surface_is_copied_whole() {
        let mut ring = Ring::new(3);
        step(&mut ring, &[]);
        step(&mut ring, &[]);
        ring.commit(2, &[]);
        for k in 0..H {
            let p = ring.pick(|i| i == 2);
            ring.commit(p.slot, &[r(0, k, W, 1)]);
        }
        assert_eq!(ring.fill(2, &[], W, H), Fill::All);
    }

    #[test]
    fn many_scattered_cells_owed_stay_bounded() {
        // what a slot owes is kept in a bounded number of rectangles: past it, their bounding box
        let mut ring = Ring::new(3);
        step(&mut ring, &[]);
        step(&mut ring, &[]);
        ring.commit(2, &[]);
        for k in 0..400 {
            let p = ring.pick(|i| i == 2);
            ring.commit(p.slot, &[r((k * 7) % W, (k * 3) % H, 1, 1)]);
        }
        assert!(ring.owed(2).is_some_and(|o| o.len() <= OWED_RECTS));
        // all of it still covered
        let f = ring.fill(2, &[], W, H);
        assert!(area(&f) >= 400);
    }

    #[test]
    fn scroll_carry_over_bytes_equal_the_scrolled_rows() {
        // a scroll of rows 10..40 by 5 in a 100-wide window damages the area it moved (the glue's Backings::scroll):
        // the next slot copies those rows and nothing else
        let mut ring = Ring::new(2);
        step(&mut ring, &[r(0, 0, W, H)]);
        step(&mut ring, &[]);
        let area_rows = r(0, 5, W, 35);
        let p = ring.pick(|_| false);
        assert_eq!(ring.age(p.slot), 2);
        let f = ring.fill(p.slot, &[area_rows], W, H);
        assert_eq!(f.bytes(W, H), 35 * W as u64 * 4);
        ring.commit(p.slot, &[area_rows]);
        // and the slot after it carries the same rows over, no more
        let p = ring.pick(|_| false);
        assert_eq!(ring.fill(p.slot, &[], W, H).bytes(W, H), 35 * W as u64 * 4);
    }

    #[test]
    fn clear_forgets_every_slot() {
        let mut ring = Ring::new(2);
        step(&mut ring, &[]);
        ring.clear();
        assert_eq!(ring.slots(), 0);
        assert_eq!(ring.newest(), None);
        assert_eq!(ring.pick(|_| false), pk(0, false));
    }

    const MS: u64 = 1_000_000;

    #[test]
    fn the_pacer_holds_while_every_other_slot_is_held() {
        let mut p = Pacer::default();
        assert!(!p.owes());
        assert_eq!(p.pace(pk(0, true), &[r(0, 0, 1, 1)], 10 * MS), Paced::Hold);
        assert!(p.owes());
        // still held a frame later: the damage adds up
        assert_eq!(p.pace(pk(0, true), &[r(5, 0, 1, 1)], 26 * MS), Paced::Hold);
        assert!(p.owes());
    }

    #[test]
    fn a_freed_slot_draws_the_held_damage_and_the_new() {
        let mut p = Pacer::default();
        p.pace(pk(0, true), &[r(0, 0, 1, 1)], 10 * MS);
        p.pace(pk(0, true), &[r(5, 0, 1, 1)], 11 * MS);
        assert_eq!(
            p.pace(pk(1, false), &[r(9, 0, 1, 1)], 20 * MS),
            Paced::Draw(vec![r(0, 0, 1, 1), r(5, 0, 1, 1), r(9, 0, 1, 1)])
        );
        assert!(!p.owes());
        // the next commit draws its own damage alone
        assert_eq!(
            p.pace(pk(0, false), &[r(2, 2, 1, 1)], 21 * MS),
            Paced::Draw(vec![r(2, 2, 1, 1)])
        );
    }

    #[test]
    fn a_hold_past_the_bound_reuses_the_held_slot() {
        let mut p = Pacer::default();
        p.pace(pk(0, true), &[r(0, 0, 1, 1)], 10 * MS);
        // the bound counts from the first held commit, not the latest
        assert_eq!(
            p.pace(pk(0, true), &[r(1, 0, 1, 1)], 10 * MS + HOLD_BOUND_NS - 1),
            Paced::Hold
        );
        assert_eq!(
            p.pace(pk(0, true), &[r(2, 0, 1, 1)], 10 * MS + HOLD_BOUND_NS),
            Paced::Reuse(vec![r(0, 0, 1, 1), r(1, 0, 1, 1), r(2, 0, 1, 1)])
        );
        assert!(!p.owes());
        // a new hold starts its own bound
        assert_eq!(
            p.pace(pk(0, true), &[], 10 * MS + HOLD_BOUND_NS + 1),
            Paced::Hold
        );
    }

    #[test]
    fn a_held_commit_is_due_when_a_slot_frees_or_at_the_bound() {
        let mut p = Pacer::default();
        assert!(!p.due(false, 0), "nothing held: nothing due");
        p.pace(pk(0, true), &[r(0, 0, 1, 1)], 10 * MS);
        assert!(!p.due(true, 11 * MS));
        assert!(p.due(false, 11 * MS));
        assert!(p.due(true, 10 * MS + HOLD_BOUND_NS));
        p.pace(pk(1, false), &[], 12 * MS);
        assert!(!p.due(false, 12 * MS));
    }

    #[test]
    fn clearing_drops_the_held_damage() {
        // the window's size changed: the new ring's slots are copied whole, the held damage means nothing
        let mut p = Pacer::default();
        p.pace(pk(0, true), &[r(0, 0, 1, 1)], 10 * MS);
        p.clear();
        assert!(!p.owes());
        assert_eq!(
            p.pace(pk(0, false), &[r(3, 3, 1, 1)], 11 * MS),
            Paced::Draw(vec![r(3, 3, 1, 1)])
        );
    }

    #[test]
    fn a_stale_release_finds_nothing_to_draw() {
        // a release asked for a publish, but a redisplay already drew the held damage: nothing is owed or drawn
        let mut p = Pacer::default();
        p.pace(pk(0, true), &[r(0, 0, 1, 1)], 10 * MS);
        p.pace(pk(1, false), &[], 12 * MS);
        assert!(!p.owes());
        assert_eq!(p.pace(pk(0, false), &[], 13 * MS), Paced::Draw(vec![]));
        assert!(!p.owes());
    }
}

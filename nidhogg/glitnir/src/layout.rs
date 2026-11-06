//! A layout leg: the layout of one window of a root moves to a new one in the presenter, from a picture of the old
//! layout taken before the redraw (the capture) to the root's front, which holds the new one.
//!
//! The leg is drawn over the window's rectangle `rect` (root device pixels) at each frame, from two pictures: the
//! capture (`rect`'s pixels, row by row) and the new front.  It writes the window's *source* pixels only: the scene
//! (card corners, rings, shadows, labels) is composed over them afterwards, as over any frame.
//! - **The background** is the capture with the old rectangles of the moved and exiting items filled with the
//!   background colour, cross-faded to the new front with the new rectangles of the moved and entering items filled
//!   the same way (`step`: the cross-fade switches at the midpoint, for high contrast).
//! - **Exits** fade out from the capture over the first [`EXIT_NS`]; **enters** fade in from the new front over the
//!   last [`ENTER_NS`].
//! - **A move** is the new front's pixels of its new rectangle, drawn at the rectangle interpolated between its start
//!   and its new rectangle (nearest-neighbour scaled when the sizes differ).
//!
//! At the leg's end every pixel equals the new front's, so the last frame is the plain compose.

use crate::ease::Curve;
use crate::geom::Rect;

/// The side of the tiles a leg diffs once at its start ([`active_tiles`]).
pub const TILE: i32 = 64;
/// How long an exiting item fades out, from the leg's start.
pub const EXIT_NS: u64 = 80_000_000;
/// How long an entering item fades in, up to the leg's end.
pub const ENTER_NS: u64 = 120_000_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ItemKind {
    Move,
    Enter,
    Exit,
}

/// One item of a layout: `from` is where the capture has it (moves and exits), `to` where the new front has it (moves
/// and enters), in root device pixels; a move is drawn from `start` (`from`, unless a retarget moved it) to `to`.
/// `id` names it across legs.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Item {
    pub id: u32,
    pub kind: ItemKind,
    pub from: Rect,
    pub start: Rect,
    pub to: Rect,
}

/// A window's layout leg.
#[derive(Clone, Debug, PartialEq)]
pub struct LayoutLeg {
    /// The number of the capture it moves from.
    pub capture: u64,
    /// The window's rectangle, root device pixels: what the capture holds and the leg draws.
    pub rect: Rect,
    pub items: Vec<Item>,
    /// Opaque premultiplied ARGB.
    pub background: u32,
    pub duration_ns: u64,
    pub curve: Curve,
    pub step: bool,
}

impl LayoutLeg {
    /// How far the moves and the background are at `t_ns` from the start: the curve at the time's fraction.
    pub fn progress(&self, t_ns: u64) -> f32 {
        if t_ns >= self.duration_ns {
            return 1.0;
        }
        let x = t_ns as f32 / self.duration_ns as f32;
        // the linear curve is the time itself, exactly
        if self.curve == Curve::LINEAR {
            x
        } else {
            self.curve.y(x)
        }
    }

    /// Item `it`'s rectangle at progress `p`: from its start to its new place.
    pub fn at(it: &Item, p: f32) -> Rect {
        let l = |a: i32, b: i32| a + ((b - a) as f32 * p).round() as i32;
        let (s, t) = (it.start, it.to);
        Rect::new(l(s.x, t.x), l(s.y, t.y), l(s.w, t.w), l(s.h, t.h))
    }

    /// The leg starts while `running` runs (`t_ns` into it): each move whose id `running` has starts from where
    /// `running` shows it now.
    pub fn retarget(&mut self, running: &LayoutLeg, t_ns: u64) {
        let p = running.progress(t_ns);
        for it in self.items.iter_mut().filter(|i| i.kind == ItemKind::Move) {
            match running.items.iter().find(|r| r.id == it.id) {
                Some(r) if r.kind == ItemKind::Move => it.start = LayoutLeg::at(r, p),
                Some(r) if r.kind == ItemKind::Enter => it.start = r.to,
                _ => {}
            }
        }
    }

    /// The weight of the new picture in the background (0 to 256).
    fn background_weight(&self, t_ns: u64) -> u32 {
        let p = self.progress(t_ns);
        let p = if !self.step {
            p
        } else if p >= 0.5 {
            1.0
        } else {
            0.0
        };
        weight(p)
    }

    /// The weights of the exits (fading out) and the enters (fading in) at `t_ns` (0 to 256).
    fn fades(&self, t_ns: u64) -> (u32, u32) {
        let d = self.duration_ns;
        if t_ns >= d {
            return (0, 256);
        }
        let exit = EXIT_NS.min(d);
        let start = d - ENTER_NS.min(d);
        let out = 1.0 - t_ns as f32 / exit as f32;
        let enter = t_ns.saturating_sub(start) as f32 / (d - start) as f32;
        (weight(out), weight(enter))
    }
}

/// `f`, 0 to 1, as a weight of 0 to 256.
fn weight(f: f32) -> u32 {
    (f.clamp(0.0, 1.0) * 256.0).round() as u32
}

/// `a` towards `b` by `w` / 256, per channel.
fn lerp(a: u32, b: u32, w: u32) -> u32 {
    [24, 16, 8, 0].iter().fold(0, |v, sh| {
        let (x, y) = ((a >> sh) & 0xff, (b >> sh) & 0xff);
        v | (((x * (256 - w) + y * w) >> 8) << sh)
    })
}

/// Premultiplied `src` at weight `w` / 256 OVER `dst`.
fn over(dst: u32, src: u32, w: u32) -> u32 {
    let s = [24, 16, 8, 0]
        .iter()
        .fold(0, |v, sh| v | (((((src >> sh) & 0xff) * w) >> 8) << sh));
    let keep = 255 - (s >> 24);
    [24, 16, 8, 0].iter().fold(0, |v, sh| {
        let d = ((dst >> sh) & 0xff) * keep;
        v | ((((s >> sh) & 0xff) + (d + 127) / 255) << sh)
    })
}

/// `r` without `hole`: at most four rectangles.
pub fn subtract(r: Rect, hole: Rect) -> Vec<Rect> {
    let h = r.intersect(&hole);
    if h.is_empty() {
        return if r.is_empty() { Vec::new() } else { vec![r] };
    }
    let (r1, h1) = (r.y + r.h, h.y + h.h);
    let (rx1, hx1) = (r.x + r.w, h.x + h.w);
    let parts = [
        Rect::new(r.x, r.y, r.w, h.y - r.y),
        Rect::new(r.x, h1, r.w, r1 - h1),
        Rect::new(r.x, h.y, h.x - r.x, h.h),
        Rect::new(hx1, h.y, rx1 - hx1, h.h),
    ];
    parts.into_iter().filter(|p| !p.is_empty()).collect()
}

/// The tile at (`tx`, `ty`), `tw` x `th`, of the leg's rectangle `r` differs between `old` and `new`.
#[allow(clippy::too_many_arguments)]
fn differs(
    old: &[u32],
    new: &[u32],
    stride: usize,
    r: Rect,
    tx: i32,
    ty: i32,
    tw: i32,
    th: i32,
) -> bool {
    (ty..ty + th).any(|y| {
        let o = (y * r.w + tx) as usize;
        let n = (r.y + y) as usize * stride + (r.x + tx) as usize;
        let w = tw as usize;
        match (old.get(o..o + w), new.get(n..n + w)) {
            (Some(a), Some(b)) => a != b,
            _ => true,
        }
    })
}

/// What a leg's frames damage: the tiles of `leg.rect` whose pixels differ between the capture `old` (`rect`'s pixels)
/// and the new front `new` (`stride` pixels a row, root coordinates), joined along each row of tiles, and each item's
/// bounds from start to end.
pub fn active_tiles(leg: &LayoutLeg, old: &[u32], new: &[u32], stride: usize) -> Vec<Rect> {
    let r = leg.rect;
    let mut out = Vec::new();
    for ty in (0..r.h.max(0)).step_by(TILE as usize) {
        let th = TILE.min(r.h - ty);
        let mut run: Option<Rect> = None;
        for tx in (0..r.w.max(0)).step_by(TILE as usize) {
            let tw = TILE.min(r.w - tx);
            if differs(old, new, stride, r, tx, ty, tw, th) {
                let t = Rect::new(r.x + tx, r.y + ty, tw, th);
                run = Some(run.map_or(t, |a| a.bounds(&t)));
            } else if let Some(a) = run.take() {
                out.push(a);
            }
        }
        out.extend(run);
    }
    for it in &leg.items {
        let b = it.from.bounds(&it.start).bounds(&it.to).intersect(&r);
        if !b.is_empty() {
            out.push(b);
        }
    }
    out
}

/// The pixels a frame of [`active_tiles`] composes.
pub fn tick_px(active: &[Rect]) -> u64 {
    active.iter().map(Rect::area).sum()
}

/// Draw `leg` at `t_ns` from its start into `out` within `clip` (root device pixels), from the capture `old` (the
/// leg's rectangle, row by row) and the new front `new`; `out` and `new` have `stride` pixels a row.
pub fn compose_leg(
    leg: &LayoutLeg,
    t_ns: u64,
    old: &[u32],
    new: &[u32],
    out: &mut [u32],
    stride: usize,
    clip: Rect,
) {
    let r = leg.rect;
    let c = clip.intersect(&r);
    if c.is_empty() {
        return;
    }
    let at_old = |x: i32, y: i32| ((y - r.y) * r.w + (x - r.x)) as usize;
    let at = |x: i32, y: i32| y as usize * stride + x as usize;
    // the background: old places filled in the capture, new places in the front, one cross-faded to the other
    let (w, bg) = (leg.background_weight(t_ns), leg.background);
    let mut mask = vec![0u8; c.w as usize];
    for y in c.y..c.y + c.h {
        mask.fill(0);
        for it in &leg.items {
            if it.kind != ItemKind::Enter {
                mark(&mut mask, c, it.from, y, 1);
            }
            if it.kind != ItemKind::Exit {
                mark(&mut mask, c, it.to, y, 2);
            }
        }
        let (o, n) = (at_old(c.x, y), at(c.x, y));
        for (i, m) in mask.iter().enumerate() {
            let a = if m & 1 != 0 { bg } else { old[o + i] };
            let b = if m & 2 != 0 { bg } else { new[n + i] };
            out[n + i] = lerp(a, b, w);
        }
    }
    let (exit, enter) = leg.fades(t_ns);
    for it in &leg.items {
        match it.kind {
            ItemKind::Exit if exit > 0 => {
                let d = it.from.intersect(&c);
                for (x, y) in pixels(d) {
                    out[at(x, y)] = over(out[at(x, y)], old[at_old(x, y)], exit);
                }
            }
            ItemKind::Enter if enter > 0 => {
                let d = it.to.intersect(&c);
                for (x, y) in pixels(d) {
                    out[at(x, y)] = over(out[at(x, y)], new[at(x, y)], enter);
                }
            }
            _ => {}
        }
    }
    // the moves last, over everything: each its new picture at its place now
    let p = leg.progress(t_ns);
    for it in leg.items.iter().filter(|i| i.kind == ItemKind::Move) {
        let (now, to) = (LayoutLeg::at(it, p), it.to);
        if now.is_empty() || to.is_empty() {
            continue;
        }
        for (x, y) in pixels(now.intersect(&c)) {
            let sx = to.x + ((x - now.x) as i64 * to.w as i64 / now.w as i64) as i32;
            let sy = to.y + ((y - now.y) as i64 * to.h as i64 / now.h as i64) as i32;
            if r.contains(sx, sy) {
                out[at(x, y)] = over(out[at(x, y)], new[at(sx, sy)], 256);
            }
        }
    }
}

/// Set `bit` in `mask` (row `y` of `c`) where `rect` covers it.
fn mark(mask: &mut [u8], c: Rect, rect: Rect, y: i32, bit: u8) {
    if y < rect.y || y >= rect.y + rect.h {
        return;
    }
    let x0 = (rect.x.max(c.x) - c.x).max(0) as usize;
    let x1 = ((rect.x + rect.w).min(c.x + c.w) - c.x).max(0) as usize;
    if x0 < x1 {
        for m in &mut mask[x0..x1] {
            *m |= bit;
        }
    }
}

/// Every pixel of `r`, row by row.
fn pixels(r: Rect) -> impl Iterator<Item = (i32, i32)> {
    (r.y..r.y + r.h.max(0)).flat_map(move |y| (r.x..r.x + r.w.max(0)).map(move |x| (x, y)))
}

#[cfg(test)]
mod tests {
    use super::*;

    const W: i32 = 40;
    const H: i32 = 30;
    /// The background colours have little red; a card is white, so its red says whether it shows.
    const BG: u32 = 0xff10_2030;
    const OLD: u32 = 0xff00_0080;
    const NEW: u32 = 0xff00_8000;
    const CARD: u32 = 0xffff_ffff;
    const MS: u64 = 1_000_000;

    /// A leg over a `W` x `H` window at (0, 0), 160 ms linear, with `items`.
    fn leg(items: Vec<Item>) -> LayoutLeg {
        LayoutLeg {
            capture: 1,
            rect: Rect::new(0, 0, W, H),
            items,
            background: BG,
            duration_ns: 160 * MS,
            curve: Curve::LINEAR,
            step: false,
        }
    }

    fn mv(id: u32, from: Rect, to: Rect) -> Item {
        Item {
            id,
            kind: ItemKind::Move,
            from,
            start: from,
            to,
        }
    }

    /// A picture of `fill` with the rectangles `cards` white.
    fn picture(fill: u32, cards: &[Rect]) -> Vec<u32> {
        let mut v = vec![fill; (W * H) as usize];
        for r in cards {
            for y in r.y..r.y + r.h {
                for x in r.x..r.x + r.w {
                    v[(y * W + x) as usize] = CARD;
                }
            }
        }
        v
    }

    fn draw(l: &LayoutLeg, t: u64, old: &[u32], new: &[u32]) -> Vec<u32> {
        let mut out = vec![0; (W * H) as usize];
        compose_leg(l, t, old, new, &mut out, W as usize, l.rect);
        out
    }

    fn px(v: &[u32], x: i32, y: i32) -> u32 {
        v[(y * W + x) as usize]
    }

    /// The red of pixel (`x`, `y`): 0xff for a card, at most 0x10 where none shows.
    fn red(v: &[u32], x: i32, y: i32) -> u32 {
        (px(v, x, y) >> 16) & 0xff
    }

    const A: Rect = Rect::new(2, 2, 8, 6);
    const B: Rect = Rect::new(22, 12, 8, 6);

    #[test]
    fn end_equals_the_new_front() {
        // pattern D: pure
        let l = leg(vec![mv(1, A, B)]);
        let (old, new) = (picture(OLD, &[A]), picture(NEW, &[B]));
        assert_eq!(draw(&l, 160 * MS, &old, &new), new);
        assert_eq!(draw(&l, 400 * MS, &old, &new), new, "past the end too");
    }

    #[test]
    fn start_shows_the_capture_background() {
        // pattern D: pure
        let l = leg(vec![mv(1, A, B)]);
        let (old, new) = (picture(OLD, &[A]), picture(NEW, &[B]));
        let out = draw(&l, 0, &old, &new);
        assert_eq!(px(&out, 35, 25), OLD, "the old background");
        assert!(red(&out, 25, 14) <= 0x10, "no card at its new place");
        assert_eq!(px(&out, 4, 4), CARD, "the card at its old place");
    }

    #[test]
    fn move_is_interpolated() {
        // pattern D: pure
        let l = leg(vec![mv(1, A, B)]);
        let half = LayoutLeg::at(&l.items[0], 0.5);
        assert_eq!(half, Rect::new(12, 7, 8, 6));
        assert_eq!(l.progress(80 * MS), 0.5);
        let (old, new) = (picture(OLD, &[A]), picture(NEW, &[B]));
        let out = draw(&l, 80 * MS, &old, &new);
        assert_eq!(px(&out, 15, 9), CARD, "the card halfway");
        assert!(red(&out, 4, 4) <= 0x10, "not at its old place");
        assert!(red(&out, 25, 14) <= 0x10, "nor at its new one");
    }

    #[test]
    fn enter_and_exit_fade() {
        // pattern D: pure
        let enter = Item {
            id: 2,
            kind: ItemKind::Enter,
            from: Rect::default(),
            start: Rect::default(),
            to: B,
        };
        let exit = Item {
            id: 3,
            kind: ItemKind::Exit,
            from: A,
            start: A,
            to: Rect::default(),
        };
        let l = leg(vec![enter, exit]);
        let (old, new) = (picture(OLD, &[A]), picture(NEW, &[B]));
        let first = draw(&l, 0, &old, &new);
        assert_eq!(px(&first, 4, 4), CARD, "the exit, whole");
        assert!(red(&first, 25, 14) <= 0x10, "the enter not yet");
        let gone = draw(&l, 80 * MS, &old, &new);
        assert!(red(&gone, 4, 4) <= 0x10, "the exit is gone by 80 ms");
        let early = draw(&l, 30 * MS, &old, &new);
        assert!(red(&early, 25, 14) <= 0x10, "no enter before 40 ms");
        let late = draw(&l, 130 * MS, &old, &new);
        assert!(red(&late, 25, 14) > 0x80, "the enter fading in");
        assert_eq!(draw(&l, 160 * MS, &old, &new), new);
    }

    #[test]
    fn step_switches_at_the_midpoint() {
        // pattern D: pure
        let mut l = leg(Vec::new());
        l.step = true;
        let (old, new) = (picture(OLD, &[]), picture(NEW, &[]));
        assert_eq!(px(&draw(&l, 70 * MS, &old, &new), 5, 5), OLD);
        assert_eq!(px(&draw(&l, 90 * MS, &old, &new), 5, 5), NEW);
        l.step = false;
        let mid = px(&draw(&l, 80 * MS, &old, &new), 5, 5);
        assert!(mid != OLD && mid != NEW, "a cross-fade: {mid:08x}");
    }

    #[test]
    fn retarget_starts_from_the_sampled_rectangle() {
        // pattern D: pure
        let running = leg(vec![mv(1, A, B)]);
        let mut next = leg(vec![mv(1, B, A), mv(7, B, A)]);
        next.retarget(&running, 80 * MS);
        assert_eq!(next.items[0].start, Rect::new(12, 7, 8, 6), "id 1");
        assert_eq!(next.items[0].from, B, "from stays");
        assert_eq!(next.items[1].start, B, "a new id");
    }

    #[test]
    fn active_tiles_skip_what_did_not_change() {
        // pattern D: pure
        let mut l = leg(Vec::new());
        l.rect = Rect::new(0, 0, 200, 130);
        let stride = 200;
        let old = vec![OLD; 200 * 130];
        let mut new = old.clone();
        new[70 * stride + 150] = NEW;
        let tiles = active_tiles(&l, &old, &new, stride);
        assert_eq!(tiles, vec![Rect::new(128, 64, 64, 64)], "one changed tile");
        l.items
            .push(mv(1, Rect::new(0, 0, 10, 10), Rect::new(0, 100, 10, 10)));
        let tiles = active_tiles(&l, &old, &new, stride);
        assert!(tiles.contains(&Rect::new(0, 0, 10, 110)), "{tiles:?}");
        assert_eq!(tick_px(&tiles), 64 * 64 + 10 * 110);
        let rest = subtract(Rect::new(0, 0, 10, 10), Rect::new(2, 2, 4, 4));
        assert_eq!(rest.iter().map(Rect::area).sum::<u64>(), 84);
    }
}

//! Publishing: the host hands a tree's picture over at a consistency point, and the presenter shows it.
//!
//! `publish` runs on the host's thread.  For each drawable surface of a tree (the root, then its child windows,
//! deepest first) it commits the surface's Glitnir scene (its damage and an immutable snapshot) and copies the damaged
//! pixels of the host's backing into a patch: the whole surface when the presenter has no front of that size.  The
//! packet goes to the mailbox and the call returns; nothing it was handed is read afterwards.
//!
//! The presenter's side (`Stage`) writes each patch into the surface's front image, in arrival order, and merges the
//! packet into what waits for the tree's next commit: damage is unioned per surface, and the earliest key time and
//! draw start are kept until the commit that shows them.  `present_tree` (`present.rs`) composes from the fronts.  A
//! front is dropped when its surface had no publish for the pool's quiet period; the next publish copies the whole
//! surface again.
//!
//! In inline mode the host's thread is the presenter: `publish` applies its packet at once.  In thread mode the
//! presenter thread (`presenter.rs`) owns the stage and drains the mailbox when woken.  The host learns which fronts the
//! presenter dropped through the mailbox, at its next publish: the presenter drops fronts only while the mailbox is
//! empty and no publish is between its copy decision and its push.
//!
//! A window that is its own output (a content layer, `layers.rs`) is left out of its frame's damage.  The patch
//! carries its damage too, and a full-window scroll moves the front's rows as it moved the backing's, so the front
//! stays the whole picture; the output's changes wait, merged, for its layer's next commit.
//!
//! **A child's transitions** (`set_motion`) live on the stage too.  What the host asks reaches it as mail, in order with
//! the packets; the frames are `present_tree`'s (`tick`): a leg starts at a tree commit, and while it runs every frame of
//! the tree composes the child whole, scales it and multiplies it by the sample of the time the frame will show.  A leg
//! is sampled on the tree's clock (`Clock`), so a state is a function of the presented times and never of how many
//! frames were drawn.  A child that goes while it hides stays here as a *ghost*, under a key no window holds, until its
//! leg ends.  On the presenter thread a hide waits `HIDE_HOLD_NS` first: inline, the host's thread is the one that
//! presents, so a hide and a show in one command meet no frame; the thread must not wake between them either.
//!
//! **A child's backdrop** (`set_backdrop`) lives here too.  Its source is the parent's front at the child's native area
//! (`Stage::beneath`): the parent's patches and scrolled rows are what it recomputes from, the child's own drawing never
//! touches it, and what the recompute changed is the child's damage, so it shows in the tree commit that shows the
//! change beneath.  A move, a resize or a new scale recomputes it all.  While a shown child has one the parent keeps its
//! front through the quiet period.  The recompute runs on the presenter's side of the tree's mutex, like a compose.

use crate::api::*;
use crate::wayland::WaylandShell;
use crate::wayland::layers::Pending;
use crate::wayland::present::note_presented;
use crate::wayland::presenter::Counters;
use crate::wayland::scale::SCALE_ONE;
use glitnir::blur::Backdrop;
use glitnir::committed::Committed;
use glitnir::compose::{
    Composer, Extras, SourceView, TargetView, compose_committed, compose_committed_with,
    compose_root,
};
use glitnir::geom::Rect;
use glitnir::kernels::kernels;
use glitnir::motion::{Kind, Motion, MotionSpec, Sample};
use glitnir::pool::IDLE_NS;
use glitnir::stats::StatsEntry;
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

/// Presents whose damage repairs an older buffer; an older buffer is composed whole.
const HIST: usize = 4;
/// Patch buffers kept for the next publish.
const SPARE_PATCHES: usize = 4;
/// How long a hide waits on the presenter thread before its leg starts.  The host's thread may show the child again
/// in the same command, and the presenter would otherwise wake between the two calls.  Long enough for the host to
/// be scheduled, short enough to fit the budget's one period past a leg's duration.
pub const HIDE_HOLD_NS: u64 = 8_000_000;
/// The rectangles of change a backdrop keeps for its next recompute, before they become one.
const MAX_DIRTY: usize = 16;

/// `CLOCK_MONOTONIC` nanoseconds, the host's clock for key times.
pub fn now_ns() -> u64 {
    let mut ts = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: `ts` is writable.
    unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut ts) };
    ts.tv_sec as u64 * 1_000_000_000 + ts.tv_nsec as u64
}

fn locked<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Recycled patch buffers: a publish takes one, the presenter gives it back once the patch is in the front.
#[derive(Default)]
pub struct PatchPool(Mutex<Vec<Vec<u32>>>);

impl PatchPool {
    pub fn take(&self) -> Vec<u32> {
        let mut v = locked(&self.0).pop().unwrap_or_default();
        v.clear();
        v
    }

    pub fn give(&self, v: Vec<u32>) {
        let mut spare = locked(&self.0);
        if spare.len() < SPARE_PATCHES {
            spare.push(v);
        }
    }
}

/// One surface of a packet.
pub struct Part {
    pub window: u64,
    pub glitnir_root: u64,
    /// The committed scene; `None` for a surface without one (the presenter copies its front).
    pub scene: Option<Arc<Committed>>,
    /// Glitnir's damage, clipped to the surface (device pixels).
    pub damage: Vec<Rect>,
    pub w: i32,
    pub h: i32,
    pub src: (i32, i32),
    pub flags: u32,
    pub alpha_cut: u32,
    pub scale: f64,
    /// The copied pixels: each rectangle of `rects`, row by row.  A full patch is one rectangle, the whole surface.
    pub rects: Vec<Rect>,
    pub patch: Vec<u32>,
    pub full: bool,
}

/// A tree's picture at one consistency point.
pub struct Packet {
    pub root: u64,
    /// The mailbox's sequence number: a content ack recorded before it may ride the commit that shows it.
    pub seq: u64,
    pub t_key_ns: u64,
    pub t_publish_ns: u64,
    pub parts: Vec<Part>,
    /// Published for a content ack with nothing drawn: the root's whole picture waits for the commit that carries
    /// the ack.
    pub whole: bool,
}

/// What the host's thread hands the presenter, in order: a packet, a window that is gone (a surface id may be reused
/// for a new window after it), or what it asked of a child's transitions.
pub enum Mail {
    Packet(Packet),
    Forget(u64),
    /// Child `.0` has the transitions `.1`; `.2`: the host has it shown.
    Motion(u64, Option<MotionSpec>, bool),
    /// The host showed (`.1`) or hid child `.0`.
    Mapped(u64, bool),
    /// Child `.0` went during its hide leg: it fades on under key `.1`.
    Ghost(u64, u64),
    /// Child `.0` of window `.1` has a backdrop of σ `.2` logical px (0: none); `.3`: its offset in the parent.
    Backdrop(u64, u64, f32, (i32, i32)),
    /// Child `.0`, which has a backdrop, moved to the offset `.1` in its parent.
    Placed(u64, (i32, i32)),
}

#[derive(Default)]
struct Inbox {
    mail: VecDeque<Mail>,
    /// A publish decided which surfaces it copies whole and has not pushed yet: no front may go meanwhile.
    busy: bool,
    /// Surfaces whose fronts the presenter dropped, until the host's next publish learns it.
    trimmed: Vec<u64>,
}

impl Inbox {
    /// What is on its way: the mail, and a publish in progress.
    fn inflight(&self) -> u64 {
        self.mail.len() as u64 + u64::from(self.busy)
    }
}

/// Packets on their way to the presenter.  The lock is held for a push, a take or a trim only.
#[derive(Default)]
pub struct Mailbox {
    inbox: Mutex<Inbox>,
    /// `Inbox::inflight`, stored under the lock at each change, so that `quiet` takes no lock: the presenter's look
    /// at it never makes the host wait.
    inflight: AtomicU64,
    seq: AtomicU64,
    pub patches: PatchPool,
}

/// Lock `m` from the host's thread; the nanoseconds spent waiting for it.
fn timed<T>(m: &Mutex<T>) -> (MutexGuard<'_, T>, u64) {
    if let Ok(g) = m.try_lock() {
        return (g, 0);
    }
    let t = now_ns();
    let g = locked(m);
    (g, now_ns().saturating_sub(t))
}

impl Mailbox {
    /// The newest request's number (0: none yet): each packet takes the next one, and so does each ack or piece of
    /// surface state the host records for a commit (`bump`).
    pub fn seq(&self) -> u64 {
        self.seq.load(Ordering::Acquire)
    }

    fn next_seq(&self) -> u64 {
        self.seq.fetch_add(1, Ordering::AcqRel) + 1
    }

    /// A request that is no packet (an ack, or surface state recorded for a commit) takes the next number, once it is
    /// recorded and before presenting is woken for it: `presented` passes it only after it has gone out.
    pub(crate) fn bump(&self) {
        self.next_seq();
    }

    /// Nothing is on its way to the stage: no mail, no publish in progress.  No lock: a publish stores its start
    /// before it takes its number (`next_seq`), so a reader that saw the number sees the publish or its mail.
    pub(crate) fn quiet(&self) -> bool {
        self.inflight.load(Ordering::Acquire) == 0
    }

    /// `b` changed: store what is on its way (`inflight`), still under the lock.
    fn note(&self, b: &Inbox) {
        self.inflight.store(b.inflight(), Ordering::Release);
    }

    /// A publish starts deciding what it copies: the fronts the presenter dropped since the last one, and the
    /// nanoseconds spent waiting for the lock.  `push` ends it.
    fn begin_publish(&self) -> (Vec<u64>, u64) {
        let (mut b, waited) = timed(&self.inbox);
        b.busy = true;
        self.note(&b);
        (std::mem::take(&mut b.trimmed), waited)
    }

    /// Queue `p` and end the publish; the nanoseconds spent waiting for the lock.
    pub fn push(&self, p: Packet) -> u64 {
        let (mut b, waited) = timed(&self.inbox);
        b.mail.push_back(Mail::Packet(p));
        b.busy = false;
        self.note(&b);
        waited
    }

    /// Queue `m` outside a publish; the nanoseconds spent waiting for the lock.
    pub fn post(&self, m: Mail) -> u64 {
        let (mut b, waited) = timed(&self.inbox);
        b.mail.push_back(m);
        self.note(&b);
        waited
    }

    pub fn take(&self) -> VecDeque<Mail> {
        let mut b = locked(&self.inbox);
        let mail = std::mem::take(&mut b.mail);
        self.note(&b);
        mail
    }

    /// Run the presenter's front trim `f` when nothing is on its way (no mail, no publish in progress), and tell the
    /// host which fronts went.  `false` when it could not run now.
    pub fn trim(&self, f: impl FnOnce() -> Vec<u64>) -> bool {
        let mut b = locked(&self.inbox);
        if b.busy || !b.mail.is_empty() {
            return false;
        }
        let gone = f();
        b.trimmed.extend(gone);
        true
    }
}

/// A surface's latest picture (the frame layer's pixels at the drawn size), `w` pixels per row.
pub struct Front {
    pub w: i32,
    pub h: i32,
    data: Vec<u32>,
}

impl Front {
    fn new(w: i32, h: i32) -> Front {
        Front {
            w,
            h,
            data: vec![0; w as usize * h as usize],
        }
    }

    /// Write `px` (`r`'s pixels, row by row) at `r`.
    fn write(&mut self, r: &Rect, px: &[u32]) {
        let (fw, rw) = (self.w as usize, r.w as usize);
        for row in 0..r.h as usize {
            let at = (r.y as usize + row) * fw + r.x as usize;
            self.data[at..at + rw].copy_from_slice(&px[row * rw..(row + 1) * rw]);
        }
    }

    /// The layer's pixels as a compose source: from `src` on, where the layer starts.
    fn view(&self, layer: u64, src: (i32, i32), scale: f64) -> SourceView<'_> {
        let (sx, sy) = (src.0.clamp(0, self.w - 1), src.1.clamp(0, self.h - 1));
        SourceView {
            layer,
            data: &self.data[sy as usize * self.w as usize + sx as usize..],
            stride: self.w as usize,
            width: self.w - sx,
            height: self.h - sy,
            scale,
        }
    }

    /// Move the rows of `r` by `dy` (negative: up) within the front, as the host's backing moved them; a row whose
    /// source lies outside the front keeps its pixels (it is damage).
    fn scroll(&mut self, r: &Rect, dy: i32) {
        let r = r.intersect(&Rect::new(0, 0, self.w, self.h));
        if r.is_empty() || dy == 0 {
            return;
        }
        let (fw, x, w) = (self.w as usize, r.x as usize, r.w as usize);
        for i in 0..r.h {
            // rows are moved away from the direction of travel first, so no source is overwritten before it is read
            let y = if dy > 0 { r.y + r.h - 1 - i } else { r.y + i };
            let from = y - dy;
            if (0..self.h).contains(&from) {
                let at = from as usize * fw + x;
                self.data.copy_within(at..at + w, y as usize * fw + x);
            }
        }
    }

    /// The plain copy when there is nothing to compose with: each pixel at its own place, within `clips`.
    fn copy_to(&self, t: &mut TargetView, clips: &[Rect]) {
        let bounds = Rect::new(0, 0, self.w, self.h).intersect(&t.rect);
        let (ox, oy) = (t.rect.x, t.rect.y);
        for r in clips.iter().map(|r| r.intersect(&bounds)) {
            if r.is_empty() {
                continue;
            }
            for y in r.y..r.y + r.h {
                let from = y as usize * self.w as usize + r.x as usize;
                let to = (y - oy) as usize * t.stride + (r.x - ox) as usize;
                t.data[to..to + r.w as usize]
                    .copy_from_slice(&self.data[from..from + r.w as usize]);
            }
        }
    }
}

/// Multiply premultiplied ARGB pixels by `1 - cut / u32::MAX`: what the compositor would do with the alpha modifier.
fn alpha_multiply(t: &mut TargetView, cut: u32) {
    let a = 255 - (u64::from(cut) * 255 / u64::from(u32::MAX)) as u32;
    let (w, h) = (t.rect.w as usize, t.rect.h as usize);
    for y in 0..h {
        for p in &mut t.data[y * t.stride..y * t.stride + w] {
            let v = *p;
            *p = (((v >> 24) * a / 255) << 24)
                | ((((v >> 16) & 0xff) * a / 255) << 16)
                | ((((v >> 8) & 0xff) * a / 255) << 8)
                | ((v & 0xff) * a / 255);
        }
    }
}

/// Each of `p`'s four channels times `a` / 255.
fn scaled(p: u32, a: u32) -> u32 {
    [24, 16, 8, 0]
        .iter()
        .fold(0, |v, sh| v | ((((p >> sh) & 0xff) * a + 127) / 255) << sh)
}

/// Premultiplied `src`, `w` pixels a row, over `dst` (`size`) at `(x, y)` with the compositor's `alpha`: the part
/// inside `dst`.
fn blend(dst: &mut [u32], size: (i32, i32), src: &[u32], w: i32, (x, y): (i32, i32), alpha: u32) {
    let a = (u64::from(alpha) * 255 / u64::from(u32::MAX)) as u32;
    let (x0, x1) = (x.max(0), (x + w).min(size.0));
    for (row, line) in src.chunks_exact(w as usize).enumerate() {
        let to = y + row as i32;
        if !(0..size.1).contains(&to) || x0 >= x1 {
            continue;
        }
        let at = to as usize * size.0 as usize;
        let d = &mut dst[at + x0 as usize..at + x1 as usize];
        for (d, p) in d.iter_mut().zip(&line[(x0 - x) as usize..]) {
            let s = scaled(*p, a);
            *d = s.wrapping_add(scaled(*d, 255 - (s >> 24)));
        }
    }
}

/// Clear `t` within `clips`.
fn clear(t: &mut TargetView, clips: &[Rect]) {
    let bounds = t.rect;
    for r in clips.iter().map(|r| r.intersect(&bounds)) {
        if r.is_empty() {
            continue;
        }
        for y in r.y..r.y + r.h {
            let at = (y - bounds.y) as usize * t.stride + (r.x - bounds.x) as usize;
            t.data[at..at + r.w as usize].fill(0);
        }
    }
}

/// Compose a surface's picture into `t` within `clips`: its scene over `src` with `x`, or its front alone.
fn draw(
    scene: Option<&Committed>,
    composer: &mut Composer,
    front: &Front,
    src: &SourceView,
    t: &mut TargetView,
    clips: &[Rect],
    x: &Extras,
) {
    // a shadow and rounded corners go OVER what the buffer holds, its older picture: nothing lies under a child's layer
    if scene.is_some_and(|c| c.layers.iter().any(|p| p.chrome.is_some())) {
        clear(t, clips);
    }
    let sources = std::slice::from_ref(src);
    let composed =
        scene.is_some_and(|c| compose_committed_with(c, composer, t, clips, sources, x).is_ok());
    if !composed {
        front.copy_to(t, clips);
    }
}

/// What the presenter keeps of a surface: its front, and what waits for its next commit.
#[derive(Default)]
struct Surface {
    root: u64,
    glitnir_root: u64,
    front: Option<Front>,
    scene: Option<Arc<Committed>>,
    damage: Vec<Rect>,
    /// The newest packet of its tree lists it as drawable.
    drawable: bool,
    src: (i32, i32),
    alpha_host: bool,
    alpha_cut: u32,
    scale: f64,
    /// The first publish with damage since the last commit (0: none).
    draw_start_ns: u64,
    damage_px: u64,
    painted_px: u64,
    /// The damage of the latest commits, newest first: what an older buffer misses.
    hist: VecDeque<Vec<Rect>>,
    /// The latest publish that listed it.
    last_ns: u64,
    /// Each own output's changes since its layer's last commit, by layer id.
    outs: BTreeMap<u64, Pending>,
}

impl Surface {
    /// A packet's picture waits for a commit of this surface or of its layers.
    fn waits(&self) -> bool {
        !self.damage.is_empty() || !self.outs.is_empty()
    }
}

/// What a tree's presentation feedback said so far, read under its mutex.
#[derive(Clone, Copy)]
pub(crate) struct Fed {
    /// The refresh period.
    pub(crate) period: u64,
    /// Feedback asked for so far; answers come in that order.
    pub(crate) asked: u64,
    /// The number of the newest answer that reported a frame (0: none), and that frame's time.
    pub(crate) shown: u64,
    pub(crate) shown_ns: u64,
    /// The newest answer said no frame was shown: the compositor is not showing the tree.
    pub(crate) discarded: bool,
}

/// A tree's clock for its transitions, from the first frame of a leg until no leg is left.
///
/// A frame's time is when it will show.  Before anything was reported that is a period from now.  The first frame this
/// clock reported is then tied to the time it was drawn for, and from there on the frame after a reported one shows a
/// period after it: the presented times drive the clock, whichever clock the compositor reports them on, and only
/// where they do not come (no `wp_presentation`, a discarded frame: sway discards the feedback of a root commit that
/// only carried a child) does the real clock take over.
struct Clock {
    /// The feedback asked for before this clock's first frame.
    base: u64,
    /// The time the clock's first frame was drawn for.
    first: u64,
    /// The newest frame's time.
    last: u64,
    /// Reported times plus this are the clock's (0 for a first frame that showed long after it was drawn: the tree
    /// was hidden, and the leg's time passed meanwhile).
    offset: Option<i64>,
}

impl Clock {
    fn new(f: &Fed) -> Clock {
        Clock {
            base: f.asked,
            first: 0,
            last: 0,
            offset: None,
        }
    }

    /// The time the next frame will show, never before the one drawn last.
    fn frame(&mut self, f: &Fed, now: u64) -> u64 {
        let real = now + f.period;
        let t = if f.asked == self.base {
            // none of this clock's frames asked yet: the coming one is its first
            self.first = real;
            real
        } else if f.discarded {
            real
        } else if f.shown > self.base {
            let (ns, first) = (f.shown_ns, self.first);
            let offset = *self.offset.get_or_insert_with(|| {
                if ns.saturating_sub(first) > 2 * f.period {
                    0
                } else {
                    first as i64 - ns as i64
                }
            });
            (ns as i64 + offset) as u64 + f.period
        } else {
            // the answer is on its way
            self.last + f.period
        };
        self.last = self.last.max(t);
        self.last
    }
}

/// `spec`'s motion at rest: shown at identity, or hidden.
fn at_rest(spec: MotionSpec, shown: bool) -> Motion {
    let mut at_once = spec;
    at_once.hide.kind = Kind::None;
    let mut m = Motion::new(at_once);
    if !shown {
        m.request(false);
        m.commit(0);
    }
    m.set_spec(spec);
    m
}

/// A child's transitions.
struct Moving {
    spec: MotionSpec,
    motion: Motion,
    /// What the host asked for last (shown: `true`), and what the tree's commits have settled.
    wanted: bool,
    settled: bool,
    /// The surface has no buffer (never shown, or unmapped): a show waits for a new picture.
    bare: bool,
    /// A leg has frames to go: for a show its last one at identity, for a hide the unmap.
    last: bool,
    /// The surface's window is gone; it lives until its hide ends.
    ghost: bool,
    /// A hide starts no leg before this time (`HIDE_HOLD_NS`; 0: none).
    hold: u64,
    /// The newest frame's time.
    t: u64,
    /// The next commit is a tick; when the one before it went, by the real clock (0: none); the period then.
    ticked: bool,
    tick_ns: u64,
    period: u64,
}

impl Moving {
    /// A child that gets `spec` now: `mapped` by the host, and `drawn` when a buffer of it has shown.  One that never
    /// showed is yet to show: its first picture starts the show leg.
    fn new(spec: MotionSpec, mapped: bool, drawn: bool) -> Moving {
        let shown = mapped && drawn;
        Moving {
            spec,
            motion: at_rest(spec, shown),
            wanted: mapped,
            settled: shown,
            bare: !shown,
            last: false,
            ghost: false,
            hold: 0,
            t: 0,
            ticked: false,
            tick_ns: 0,
            period: 0,
        }
    }

    /// The tree's next frame has something to do for this surface: a ghost with no picture has only to go.
    fn due(&self, s: &Surface) -> bool {
        self.last
            || (self.ghost && self.bare)
            || (self.wanted != self.settled
                && (!self.bare || (s.drawable && s.front.is_some() && !s.damage.is_empty())))
    }

    /// The surface shows a buffer whatever the host's packets say: a hide runs or waits for its first frame.
    fn keeps_drawable(&self) -> bool {
        self.last || (self.settled && !self.wanted)
    }

    /// The surface may be asked to fade at any time, or does: its front stays through the quiet period.
    fn keeps_front(&self) -> bool {
        !self.bare
    }

    /// The leg is over at the frame of time `t`: root `g`'s record learns how far past its length that is.
    fn ended(&self, g: u64, t: u64) {
        if let Some(ns) = self.motion.overrun(t) {
            glitnir::stats::leg_ended(g, ns);
        }
    }
}

/// What a backdrop's image should be made from, in the parent's device pixels.
#[derive(Clone, Copy, PartialEq)]
struct Aim {
    region: Rect,
    sigma_px: f32,
    scale: f64,
    /// The size of what it is read from.
    size: (i32, i32),
    /// The child's layer in its own surface: what takes the region's pixels there.
    layer_at: (i32, i32),
}

/// A child's backdrop blur.
struct Blur {
    parent: u64,
    /// σ in logical pixels, and the surface's offset in its parent's.
    blur_px: f32,
    at: (i32, i32),
    image: Backdrop,
    /// What the image was last brought up to, and how the parent changed since.
    aim: Option<Aim>,
    dirty: Vec<Rect>,
    /// Pixels recomputed that no stats entry has yet, and the nanoseconds that took.
    recomputed: u64,
    took_ns: u64,
}

impl Blur {
    /// What the image should be now, with what lies beneath it; `None` while either picture is missing, or the two
    /// are at different scales.
    fn aim(&self, child: &Surface, surfaces: &HashMap<u64, Surface>) -> Option<Aim> {
        let layer = child
            .scene
            .as_ref()?
            .layers
            .iter()
            .find(|p| p.id == child.glitnir_root)?
            .rect;
        let (front, scale) = Stage::beneath(surfaces, self.parent)?;
        child.front.as_ref()?;
        let device = |logical: i32| (f64::from(logical) * scale).round() as i32;
        (scale == child.scale).then(|| Aim {
            region: Rect::new(
                device(self.at.0) + layer.x,
                device(self.at.1) + layer.y,
                layer.w,
                layer.h,
            ),
            sigma_px: self.blur_px * scale as f32,
            scale,
            size: (front.w, front.h),
            layer_at: (layer.x, layer.y),
        })
    }

    fn due(&self, aim: &Aim) -> bool {
        self.aim.as_ref() != Some(aim) || !self.dirty.is_empty()
    }

    /// The parent changed at `rects`.
    fn soil(&mut self, rects: &[Rect]) {
        self.dirty.extend_from_slice(rects);
        if self.dirty.len() > MAX_DIRTY {
            let all = self.dirty.iter().fold(Rect::default(), |a, r| a.bounds(r));
            self.dirty = vec![all];
        }
    }
}

/// The presenter's side: fronts, merged packets, the display's composer and the counters.
#[derive(Default)]
pub struct Stage {
    surfaces: HashMap<u64, Surface>,
    /// The transitions of the children that have them; empty, none of this costs anything.
    moves: HashMap<u64, Moving>,
    /// The backdrops of the children that have one; empty, none of this costs anything.
    blurs: HashMap<u64, Blur>,
    /// Each moving tree's clock, by root.
    clocks: HashMap<u64, Clock>,
    /// Children whose hide is over: each unmaps in its tree's next commit.
    unmaps: Vec<u64>,
    /// A leg's picture before its scale.
    scratch: Vec<u32>,
    /// How long a hide waits for the host's next call (0: inline, where the host's thread presents anyway).
    pub hide_hold_ns: u64,
    /// Each root's earliest key time that no commit has shown yet.
    keys: HashMap<u64, u64>,
    /// Each root's newest packet applied to the fronts (its sequence number): a content ack recorded before it may
    /// ride a buffer composed now.
    applied: HashMap<u64, u64>,
    composer: Composer,
    /// Commits sent since the last flush, with a buffer (a window's or a layer's) and bare; they count when flushed.
    unflushed: (u64, u64),
    pub stats: Arc<Counters>,
}

impl Stage {
    pub fn new(stats: Arc<Counters>) -> Stage {
        Stage {
            stats,
            ..Stage::default()
        }
    }

    /// Apply everything the mailbox holds, in order.
    pub fn drain(&mut self, mail: &Mailbox) {
        for m in mail.take() {
            match m {
                Mail::Packet(p) => self.apply(p, &mail.patches),
                Mail::Forget(id) => self.forget(id),
                Mail::Motion(id, spec, mapped) => self.set_motion(id, spec, mapped),
                Mail::Mapped(id, on) => {
                    let hold = self.hide_hold_ns;
                    if let Some(m) = self.moves.get_mut(&id) {
                        m.wanted = on;
                        m.hold = if on || hold == 0 { 0 } else { now_ns() + hold };
                        // no leg ran, so none will hide it: the packet that listed it drawable is out of date
                        if !on
                            && m.bare
                            && let Some(s) = self.surfaces.get_mut(&id)
                        {
                            s.drawable = false;
                        }
                    }
                }
                Mail::Ghost(id, key) => self.ghost(id, key),
                Mail::Backdrop(id, parent, blur_px, at) => {
                    self.set_backdrop(id, parent, blur_px, at);
                }
                Mail::Placed(id, at) => {
                    if let Some(b) = self.blurs.get_mut(&id) {
                        b.at = at;
                    }
                }
            }
        }
    }

    /// A surface that may show a buffer has a picture waiting for a commit.
    fn picture_waits(&self) -> bool {
        self.surfaces.values().any(|s| s.drawable && s.waits())
    }

    /// `presented` reaches the newest request when every one has gone out in a flushed commit or needs none any more:
    /// nothing is on its way (`mail`), every commit sent is flushed, no picture waits in the fronts and no tree waits
    /// for a commit (`trees_wait`: their recorded acks and state).  The number is read first, so each request it
    /// counts was queued or recorded before any of that is looked at; one recorded later is behind it.  The stage's
    /// owner calls it: the presenter thread at the end of each turn, the host's thread (inline) when it reads the
    /// counters.  A picture leaves the fronts' waiting damage only with its commit (`presented`, `take_pending`), its
    /// window (`forget`) or a newer packet that no longer lists its surface.  `shown` reaches it too when moreover the
    /// compositor has used every tree's newest commit (`trees_wait`'s second half: a frame callback not done yet).
    pub fn note_presented(&self, mail: &Mailbox, trees_wait: impl FnOnce(&Stage) -> (bool, bool)) {
        let seen = mail.seq();
        if !(mail.quiet() && self.unflushed == (0, 0) && !self.picture_waits()) {
            return;
        }
        let (waits, unused) = trees_wait(self);
        if !waits {
            self.stats.presented.store(seen, Ordering::Release);
            if !unused {
                self.stats.shown.store(seen, Ordering::Release);
            }
        }
    }

    /// Child `id` of window `parent` has a backdrop of σ `blur_px` logical px; 0 takes it away, and its picture is
    /// composed whole without it.
    fn set_backdrop(&mut self, id: u64, parent: u64, blur_px: f32, at: (i32, i32)) {
        if blur_px > 0.0 {
            let b = self.blurs.entry(id).or_insert_with(|| Blur {
                parent,
                blur_px,
                at,
                image: Backdrop::new(),
                aim: None,
                dirty: Vec::new(),
                recomputed: 0,
                took_ns: 0,
            });
            (b.blur_px, b.at) = (blur_px, at);
        } else if self.blurs.remove(&id).is_some()
            && let Some(s) = self.surfaces.get_mut(&id)
            && let Some(f) = &s.front
        {
            s.damage = vec![Rect::new(0, 0, f.w, f.h)];
        }
    }

    /// What a backdrop is read from: the front of window `parent`, and its scale.  (A front holds the backing's pixels:
    /// what Glitnir draws from the scene, a label, is not in it.)
    fn beneath(surfaces: &HashMap<u64, Surface>, parent: u64) -> Option<(&Front, f64)> {
        let s = surfaces.get(&parent)?;
        Some((s.front.as_ref()?, s.scale))
    }

    /// A shown child of root `root`'s tree has a backdrop that is not up to date.
    pub(crate) fn blur_due(&self, root: u64) -> bool {
        self.blurs.iter().any(|(id, b)| {
            self.surfaces
                .get(id)
                .filter(|s| s.root == root && s.drawable)
                .and_then(|s| b.aim(s, &self.surfaces))
                .is_some_and(|aim| b.due(&aim))
        })
    }

    /// Bring root `root`'s backdrops up to date: what a recompute changed waits as the child's damage for the tree's
    /// commit, which shows the change beneath.
    pub(crate) fn blur(&mut self, root: u64) {
        let Stage {
            surfaces, blurs, ..
        } = self;
        for (id, b) in blurs.iter_mut() {
            let Some(aim) = surfaces
                .get(id)
                .filter(|s| s.root == root && s.drawable)
                .and_then(|s| b.aim(s, surfaces))
                .filter(|aim| b.due(aim))
            else {
                continue;
            };
            let Some((front, _)) = Stage::beneath(surfaces, b.parent) else {
                continue;
            };
            let began = now_ns();
            let changed = b.image.update(
                &front.data,
                front.w as usize,
                aim.size,
                aim.region,
                aim.sigma_px,
                aim.scale,
                &b.dirty,
            );
            // an update that kept the cache recomputed nothing: it has no time to report
            if b.image.recomputed_px() != 0 {
                b.took_ns += now_ns() - began;
            }
            b.dirty.clear();
            b.aim = Some(aim);
            b.recomputed += b.image.recomputed_px();
            if let (Some(r), Some(s)) = (changed, surfaces.get_mut(id)) {
                let (x, y) = aim.layer_at;
                let r = r.translate(x - aim.region.x, y - aim.region.y);
                s.damage_px += r.area();
                s.damage.push(r);
                if s.draw_start_ns == 0 {
                    s.draw_start_ns = now_ns();
                }
            }
        }
    }

    /// The windows with a shown child that has a backdrop: their fronts stay through the quiet period.
    fn blur_parents(&self) -> Vec<u64> {
        let shown = |id: &u64| self.surfaces.get(id).is_some_and(|s| s.drawable);
        self.blurs
            .iter()
            .filter(|(id, _)| shown(id))
            .map(|(_, b)| b.parent)
            .collect()
    }

    /// Child `id` has the transitions `spec`, and the host has it shown or not (`mapped`).  With none, a leg that runs
    /// ends: a show's picture is composed once more as it is, a hide (or one that waits) unmaps.
    fn set_motion(&mut self, id: u64, spec: Option<MotionSpec>, mapped: bool) {
        let s = self.surfaces.get_mut(&id);
        match spec {
            Some(spec) => {
                if let Some(m) = self.moves.get_mut(&id) {
                    m.spec = spec;
                    m.motion.set_spec(spec);
                } else {
                    let drawn = s.is_some_and(|s| !s.hist.is_empty());
                    self.moves.insert(id, Moving::new(spec, mapped, drawn));
                }
            }
            None => {
                let (Some(m), Some(s)) = (self.moves.remove(&id), s) else {
                    return;
                };
                if !m.wanted && !m.bare {
                    self.unmaps.push(id);
                } else if let (true, Some(f)) = (m.last, &s.front) {
                    s.damage = vec![Rect::new(0, 0, f.w, f.h)];
                }
            }
        }
    }

    /// Child `id` went during its hide leg: what the stage has of it moves to `key` (the host may give `id` out again).
    fn ghost(&mut self, id: u64, key: u64) {
        if let Some(s) = self.surfaces.remove(&id) {
            self.surfaces.insert(key, s);
        }
        if let Some(mut m) = self.moves.remove(&id) {
            m.ghost = true;
            self.moves.insert(key, m);
        }
        if let Some(b) = self.blurs.remove(&id) {
            self.blurs.insert(key, b);
        }
    }

    /// Some child has transitions, or an unmap to go: `present_tree` looks at its tree's.
    pub(crate) fn moving(&self) -> bool {
        !self.moves.is_empty() || !self.unmaps.is_empty()
    }

    /// The next frame of root `root`'s tree has a transition to serve.
    pub(crate) fn work(&self, root: u64) -> bool {
        self.moves.iter().any(|(id, m)| {
            self.surfaces
                .get(id)
                .is_some_and(|s| s.root == root && m.due(s))
        })
    }

    /// When the next hide stops waiting: the presenter wakes for it, at once for a wait that ended after the tick looked.
    pub(crate) fn next_hold(&self) -> Option<u64> {
        self.moves
            .values()
            .filter(|m| m.wanted != m.settled && m.hold != 0)
            .map(|m| m.hold)
            .min()
    }

    /// Presenting goes inline: nobody would wake for a hide that waits, so none does.
    pub fn end_holds(&mut self) {
        self.hide_hold_ns = 0;
        for m in self.moves.values_mut() {
            m.hold = 0;
        }
    }

    /// The children of root `root` to unmap now, gone from the list.
    pub(crate) fn take_unmaps(&mut self, root: u64) -> Vec<u64> {
        let Stage {
            surfaces, unmaps, ..
        } = self;
        let mut out = Vec::new();
        unmaps.retain(|id| {
            let Some(s) = surfaces.get_mut(id).filter(|s| s.root == root) else {
                return surfaces.contains_key(id);
            };
            s.drawable = false;
            out.push(*id);
            false
        });
        out
    }

    /// A frame of root `root`'s tree is about to go (`fed`: its feedback): each child's leg starts or goes on.  A leg
    /// that runs damages all of its surface, so the frame composes it whole with the sample of the frame's time; the
    /// leg's end gives one last frame at rest, or for a hide the unmap (`take_unmaps`).  Returns the ghosts whose hide
    /// ended.
    pub(crate) fn tick(&mut self, root: u64, fed: &Fed) -> Vec<u64> {
        let now = now_ns();
        let t = self
            .clocks
            .entry(root)
            .or_insert_with(|| Clock::new(fed))
            .frame(fed, now);
        let start = t.saturating_sub(fed.period);
        let Stage {
            surfaces,
            moves,
            unmaps,
            ..
        } = self;
        let mut ended = Vec::new();
        for (&id, m) in moves.iter_mut() {
            let Some(s) = surfaces.get_mut(&id).filter(|s| s.root == root) else {
                continue;
            };
            if !m.due(s) {
                continue;
            }
            // a hide that waits: a leg that runs goes on meanwhile
            let held = now < m.hold;
            if held && !m.last {
                continue;
            }
            if m.wanted != m.settled && !held {
                m.motion.request(m.wanted);
                m.motion.commit(start);
                m.settled = m.wanted;
                m.bare = false;
                m.last = true;
                m.tick_ns = 0;
            }
            m.t = t;
            m.period = fed.period;
            if !m.settled && (s.front.is_none() || m.motion.unmap_due(t)) {
                m.ended(s.glitnir_root, t);
                m.last = false;
                m.bare = true;
                s.drawable = false;
                if m.ghost {
                    ended.push(id);
                } else {
                    unmaps.push(id);
                }
                continue;
            }
            if let Some(f) = &s.front {
                s.damage = vec![Rect::new(0, 0, f.w, f.h)];
            }
            m.ticked = true;
            m.last = m.motion.running(t);
            if !m.last {
                m.ended(s.glitnir_root, t);
            }
        }
        if !self.work(root) {
            self.clocks.remove(&root);
            self.scratch = Vec::new();
        }
        ended
    }

    /// Root `root`'s newest packet in the fronts (0: none).
    pub fn applied(&self, root: u64) -> u64 {
        self.applied.get(&root).copied().unwrap_or(0)
    }

    /// When surface `id`'s waiting damage was first published (0: nothing waits).
    pub fn draw_start(&self, id: u64) -> u64 {
        self.surfaces.get(&id).map_or(0, |s| s.draw_start_ns)
    }

    /// When the next front may be dropped idle: the earliest quiet period's end among fronts with nothing waiting.
    pub fn next_trim(&self) -> Option<u64> {
        let kept = self.blur_parents();
        self.surfaces
            .iter()
            .filter(|(id, s)| {
                s.front.is_some()
                    && s.damage.is_empty()
                    && s.outs.is_empty()
                    && !self.moves.get(id).is_some_and(Moving::keeps_front)
                    && !kept.contains(id)
            })
            .map(|(_, s)| s.last_ns + IDLE_NS)
            .min()
    }

    /// Write `p`'s patches into the fronts and merge it into what waits for its tree's commit.
    fn apply(&mut self, p: Packet, patches: &PatchPool) {
        self.applied.insert(p.root, p.seq);
        let moves = &self.moves;
        let waiting = self
            .surfaces
            .iter_mut()
            .filter(|(_, s)| s.root == p.root)
            .fold(false, |w, (id, s)| {
                // the newest packet says which surfaces may show a buffer, but for a hide that runs (C lists a hidden
                // frame as not drawable at once)
                s.drawable = moves.get(id).is_some_and(Moving::keeps_drawable);
                w || s.waits()
            });
        if waiting {
            Counters::add(&self.stats.superseded, 1);
        }
        let key = self.keys.entry(p.root).or_insert(0);
        if *key == 0 {
            *key = p.t_key_ns;
        }
        for part in p.parts {
            let s = self.surfaces.entry(part.window).or_default();
            let size_ok = s
                .front
                .as_ref()
                .is_some_and(|f| (f.w, f.h) == (part.w, part.h));
            let outputs = part
                .scene
                .as_ref()
                .map_or(&[][..], |c| c.outputs.as_slice());
            if part.full || !size_ok {
                s.front = Some(Front::new(part.w, part.h));
            } else if let Some(front) = &mut s.front {
                // the backing's rows moved before the host drew the ones a scroll exposed
                for o in outputs.iter().filter(|o| !o.full) {
                    front.scroll(&o.rect, o.scroll);
                }
            }
            if let Some(front) = &mut s.front {
                let mut at = 0;
                for r in &part.rects {
                    let n = r.w as usize * r.h as usize;
                    front.write(r, &part.patch[at..at + n]);
                    at += n;
                }
            }
            if !self.blurs.is_empty() {
                let dirt: Vec<Rect> = if part.full || !size_ok {
                    vec![Rect::new(0, 0, part.w, part.h)]
                } else {
                    let scrolled = outputs.iter().filter(|o| !o.full && o.scroll != 0);
                    (part.rects.iter().copied())
                        .chain(scrolled.map(|o| o.rect))
                        .collect()
                };
                let under = self.blurs.values_mut();
                for b in under.filter(|b| b.parent == part.window) {
                    b.soil(&dirt);
                }
            }
            let outs_changed = Pending::merge_all(&mut s.outs, outputs);
            // a window on its own layer: its pixels are in the output's damage, not the frame's
            s.damage_px += outputs
                .iter()
                .flat_map(|o| &o.damage)
                .map(Rect::area)
                .sum::<u64>();
            if part.window != p.root {
                // only a root's windows get layers: a child's own outputs are its damage
                for o in std::mem::take(&mut s.outs).into_values() {
                    s.damage.extend(o.root_rects());
                }
            }
            patches.give(part.patch);
            s.root = p.root;
            s.glitnir_root = part.glitnir_root;
            s.scene = part.scene;
            s.drawable = part.flags & NHSHELL_SF_DRAWABLE != 0;
            s.src = part.src;
            s.alpha_host = part.flags & NHSHELL_SF_ALPHA_HOST != 0;
            s.alpha_cut = part.alpha_cut;
            s.scale = part.scale;
            s.last_ns = p.t_publish_ns;
            if (!part.damage.is_empty() || outs_changed) && s.draw_start_ns == 0 {
                s.draw_start_ns = p.t_publish_ns;
            }
            s.damage_px += part.damage.iter().map(Rect::area).sum::<u64>();
            s.damage.extend(part.damage);
        }
        // its picture waits like a drawn one: a newer packet merges into it, the window's end supersedes it
        if p.whole {
            self.damage_all_if_clean(p.root);
        }
    }

    /// Surface `id` has damage that waits for a commit.
    pub fn damaged(&self, id: u64) -> bool {
        self.surfaces.get(&id).is_some_and(|s| !s.damage.is_empty())
    }

    /// Surface `id` may show a buffer (the newest packet of its tree lists it as drawable).
    pub fn drawable(&self, id: u64) -> bool {
        self.surfaces.get(&id).is_some_and(|s| s.drawable)
    }

    /// The whole of surface `id` waits for its commit when nothing does yet (a content ack with nothing drawn shows the
    /// whole picture).
    pub fn damage_all_if_clean(&mut self, id: u64) {
        if let Some(s) = self.surfaces.get_mut(&id)
            && s.damage.is_empty()
            && let Some(f) = &s.front
        {
            s.damage.push(Rect::new(0, 0, f.w, f.h));
        }
    }

    /// The connection went: the next compositor has none of the pictures, so the whole of every front waits for its
    /// surface's first commit there (composed whole, and damaged whole on the new surface).  Ghosts go, and every leg
    /// ends where it was heading: the new surfaces have no picture to move from.
    pub fn connection_lost(&mut self) {
        let ghosts: Vec<u64> = self
            .moves
            .iter()
            .filter(|(_, m)| m.ghost)
            .map(|(id, _)| *id)
            .collect();
        for id in ghosts {
            self.forget(id);
        }
        self.clocks.clear();
        self.unmaps.clear();
        for (id, m) in &mut self.moves {
            let shown = m.wanted && m.settled;
            m.motion = at_rest(m.spec, shown);
            m.settled = shown;
            m.bare = !shown;
            m.last = false;
            if let (false, Some(s)) = (m.wanted, self.surfaces.get_mut(id)) {
                s.drawable = false;
            }
        }
        for s in self.surfaces.values_mut() {
            if let Some(f) = &s.front {
                s.damage = vec![Rect::new(0, 0, f.w, f.h)];
            }
        }
        if self.picture_waits() {
            // none of them has gone out to the next compositor
            self.stats.presented.store(0, Ordering::Release);
            self.stats.shown.store(0, Ordering::Release);
        }
    }

    /// What surface `id`'s next buffer is: its front's size and the damage that waits.
    pub fn next_frame(&self, id: u64) -> Option<(i32, i32, Vec<Rect>)> {
        let s = self.surfaces.get(&id)?;
        let f = s.front.as_ref()?;
        (!s.damage.is_empty()).then(|| (f.w, f.h, s.damage.clone()))
    }

    /// Compose surface `id`'s buffer `buf` from its front: the waiting damage and what the buffer's age misses.
    pub fn compose(&mut self, id: u64, buf: &NhShellBuffer) {
        let Stage {
            surfaces,
            composer,
            moves,
            blurs,
            scratch,
            ..
        } = self;
        let sample = moves
            .get(&id)
            .map_or(Sample::IDENTITY, |m| m.motion.sample(m.t));
        let Some(s) = surfaces.get_mut(&id) else {
            return;
        };
        let extras = Extras {
            backdrop: blurs.get(&id).map(|b| (&b.image, s.glitnir_root)),
        };
        let Some(front) = &s.front else {
            return;
        };
        if buf.w <= 0 || buf.h <= 0 || buf.stride < 4 * buf.w || buf.pixels.is_null() {
            return;
        }
        let all = Rect::new(0, 0, buf.w, buf.h);
        let age = buf.age as usize;
        let mut clips = Vec::new();
        if s.alpha_host || age == 0 || age - 1 > s.hist.len() {
            // the host multiplies alpha: every pixel is composed, so that each is multiplied once
            clips.push(all);
        } else {
            clips.extend_from_slice(&s.damage);
            for h in s.hist.iter().take(age - 1) {
                clips.extend_from_slice(h);
            }
        }
        s.painted_px += s.damage.iter().map(Rect::area).sum::<u64>();
        let stride = buf.stride as usize / 4;
        let len = stride * (buf.h as usize - 1) + buf.w as usize;
        // SAFETY: a begun buffer is `stride * h` writable bytes, page-aligned, and nothing else touches it before its
        // commit.
        let data = unsafe { std::slice::from_raw_parts_mut(buf.pixels.cast::<u32>(), len) };
        let mut t = TargetView {
            data,
            stride,
            rect: all,
            scale: s.scale,
        };
        let src = front.view(s.glitnir_root, s.src, s.scale);
        let scene = s.scene.as_deref();
        if sample.scale == 1.0 {
            draw(scene, composer, front, &src, &mut t, &clips, &extras);
        } else {
            // a pop: the picture as it will rest, then scaled about its centre, so the shadow's tiles stretch with it
            let (w, h) = (buf.w as usize, buf.h as usize);
            scratch.resize(w * h, 0);
            let mut whole = TargetView {
                data: scratch,
                stride: w,
                rect: all,
                scale: s.scale,
            };
            draw(scene, composer, front, &src, &mut whole, &[all], &extras);
            kernels().scale(t.data, stride, scratch, w, w, h, sample.scale, all);
        }
        let cut = if s.alpha_host { s.alpha_cut } else { 0 };
        if sample.opacity < 1.0 {
            // the leg's opacity is always our multiply (a static alpha stays with the compositor when it can)
            let keep = (1.0 - f64::from(cut) / f64::from(u32::MAX)) * f64::from(sample.opacity);
            alpha_multiply(&mut t, ((1.0 - keep) * f64::from(u32::MAX)) as u32);
        } else if s.alpha_host {
            alpha_multiply(&mut t, cut);
        }
    }

    /// Surface `id`'s buffer was committed: its stats entry (with the tree's key time when it is the root), and its
    /// damage joins the history.  The commit counts at the next `flushed`.
    pub fn presented(&mut self, id: u64) {
        self.record(id);
        if let Some(s) = self.surfaces.get_mut(&id) {
            s.hist.push_front(std::mem::take(&mut s.damage));
            s.hist.truncate(HIST);
        }
        self.unflushed.0 += 1;
    }

    /// Root `root`'s tree showed new pixels on its content layers alone: its stats entry and key time, unless the
    /// root's own damage still waits (its buffer's commit records them).
    pub fn presented_layers(&mut self, root: u64) {
        if !self.damaged(root) {
            self.record(root);
        }
    }

    /// A bare commit was sent; it counts at the next `flushed`.
    pub fn presented_bare(&mut self) {
        self.unflushed.1 += 1;
    }

    /// The connection was flushed: the commits sent before count, and `presented` may follow them
    /// (`note_presented`).  A commit in the counters has left for the compositor (or, with the socket full, waits for
    /// the reader's flush, ahead of any later request), so a settle that reads them and then asks the compositor finds
    /// each one there (`gui-runner--recorder-quiet`).
    pub fn flushed(&mut self) {
        let (commits, bare) = std::mem::take(&mut self.unflushed);
        Counters::add(&self.stats.commits, commits);
        Counters::add(&self.stats.bare_commits, bare);
    }

    /// A tree commit of root `root` showed a child's buffer: the key it answered is spent, whether or not the root
    /// had a buffer of its own in it (a child's entry only reads the key, the root's takes it).
    pub fn kids_shown(&mut self, root: u64) {
        self.keys.remove(&root);
    }

    /// Surface `id`'s stats entry for a commit that showed what waited, with the tree's key time (a child's commit
    /// shows in its tree's), and with the tick it was, if any: late when it came over 1.25 periods after the one
    /// before.  A ghost's root is gone, so its frames go unrecorded.
    fn record(&mut self, id: u64) {
        let now = now_ns();
        let (mut ticks, mut late_ticks, mut tick_interval_ns, mut ghost) = (0, 0, 0, false);
        if let Some(m) = self.moves.get_mut(&id) {
            ghost = m.ghost;
            if std::mem::take(&mut m.ticked) {
                ticks = 1;
                if m.tick_ns != 0 {
                    tick_interval_ns = now - m.tick_ns;
                    late_ticks = u32::from(tick_interval_ns > m.period * 5 / 4);
                }
                m.tick_ns = now;
            }
        }
        let (backdrop_px, blur_ns) = self.blurs.get_mut(&id).map_or((0, 0), |b| {
            let took = std::mem::take(&mut b.took_ns);
            (std::mem::take(&mut b.recomputed), took)
        });
        let Some(s) = self.surfaces.get_mut(&id) else {
            return;
        };
        let key = if s.root == id {
            self.keys.insert(id, 0).unwrap_or(0)
        } else {
            self.keys.get(&s.root).copied().unwrap_or(0)
        };
        let mut e = StatsEntry {
            t_update_ns: key,
            t_commit_ns: s.draw_start_ns,
            t_draw_start_ns: s.draw_start_ns,
            t_draw_end_ns: now,
            presented_us: -1,
            damage_px: s.damage_px,
            painted_px: s.painted_px,
            backdrop_px,
            tick_interval_ns,
            blur_ns,
            backfill: 2,
            ticks,
            late_ticks,
            ..StatsEntry::default()
        };
        self.composer.fill_stats(s.glitnir_root, &mut e);
        if !ghost {
            glitnir::stats::record(s.glitnir_root, e);
        }
        s.draw_start_ns = 0;
        s.damage_px = 0;
        s.painted_px = 0;
    }

    /// The own outputs of root `root`'s newest scene: layer id and rectangle (root device px), in paint order.
    pub fn outputs(&self, root: u64) -> Vec<(u64, Rect)> {
        self.surfaces
            .get(&root)
            .and_then(|s| s.scene.as_ref())
            .map_or_else(Vec::new, |c| {
                c.outputs.iter().map(|o| (o.layer, o.rect)).collect()
            })
    }

    /// Root `root`'s own outputs have changes waiting for their layers.
    pub fn outputs_pending(&self, root: u64) -> bool {
        self.surfaces.get(&root).is_some_and(|s| !s.outs.is_empty())
    }

    /// What waits for layer `layer` of root `root`.
    pub fn pending(&self, root: u64, layer: u64) -> Option<&Pending> {
        self.surfaces.get(&root)?.outs.get(&layer)
    }

    /// Layer `layer` of root `root` committed what waited for it; the commit counts at the next `flushed`.
    pub fn take_pending(&mut self, root: u64, layer: u64) {
        if let Some(s) = self.surfaces.get_mut(&root) {
            s.outs.remove(&layer);
        }
        self.unflushed.0 += 1;
    }

    /// No layer can show root `root`'s own outputs (no subcompositor or viewporter): what waits for them becomes the
    /// frame's damage, composed from the front like the rest.
    pub fn fold_outputs(&mut self, root: u64) {
        if let Some(s) = self.surfaces.get_mut(&root) {
            for p in std::mem::take(&mut s.outs).into_values() {
                s.damage.extend(p.root_rects());
            }
        }
    }

    /// Compose root `root`'s picture within `clips` (root device px) from the root's front into `data` (`stride`
    /// pixels a row): a content layer's view, which lies at `rect` in the root.
    pub fn compose_output(
        &mut self,
        root: u64,
        data: &mut [u32],
        stride: usize,
        rect: Rect,
        clips: &[Rect],
    ) {
        let Stage {
            surfaces, composer, ..
        } = self;
        let Some(s) = surfaces.get_mut(&root) else {
            return;
        };
        s.painted_px += clips.iter().map(Rect::area).sum::<u64>();
        let Some(front) = &s.front else {
            return;
        };
        let mut t = TargetView {
            data,
            stride,
            rect,
            scale: s.scale,
        };
        let src = front.view(s.glitnir_root, s.src, s.scale);
        let composed = s
            .scene
            .as_ref()
            .is_some_and(|c| compose_committed(c, composer, &mut t, clips, &[src]).is_ok());
        if !composed {
            front.copy_to(&mut t, clips);
        }
    }

    /// Drop the fronts of surfaces with no publish for the pool's quiet period and nothing waiting; their windows.
    pub fn trim(&mut self, now: u64) -> Vec<u64> {
        let kept = self.blur_parents();
        let mut out = Vec::new();
        for (id, s) in &mut self.surfaces {
            if s.front.is_some()
                && s.damage.is_empty()
                && s.outs.is_empty()
                && now.saturating_sub(s.last_ns) > IDLE_NS
                && !self.moves.get(id).is_some_and(Moving::keeps_front)
                && !kept.contains(id)
            {
                s.front = None;
                out.push(*id);
            }
        }
        Counters::add(&self.stats.front_trims, out.len() as u64);
        out
    }

    /// Window `id` is gone.
    pub fn forget(&mut self, id: u64) {
        if let Some(s) = self.surfaces.remove(&id) {
            self.composer.forget(s.glitnir_root);
            // the packet this surface's damage waited for commits nothing now, unless the rest of its tree still shows it
            if s.waits()
                && !self
                    .surfaces
                    .values()
                    .any(|o| o.root == s.root && o.waits())
            {
                Counters::add(&self.stats.superseded, 1);
            }
        }
        self.moves.remove(&id);
        self.blurs.remove(&id);
        self.clocks.remove(&id);
        self.unmaps.retain(|u| *u != id);
        self.keys.remove(&id);
        self.applied.remove(&id);
    }
}

/// The publishing side of a display (the host's thread), and the presenter's stage while it runs inline.
pub struct Presenting {
    /// The stage while presenting is inline; the presenter thread owns it while it runs.
    pub stage: Option<Stage>,
    /// The presenter thread; it hands the stage back when it stops (`None` after a panic).
    pub thread: Option<std::thread::JoinHandle<Option<Stage>>>,
    /// The size of each surface's front as the last publish left it (absent: the next publish copies it whole).
    fronts: HashMap<u64, (i32, i32)>,
    pub stats: Arc<Counters>,
    /// Bytes the latest publish copied.
    pub copied: usize,
}

impl Presenting {
    pub fn new(stats: Arc<Counters>) -> Presenting {
        Presenting {
            stage: Some(Stage::new(stats.clone())),
            thread: None,
            fronts: HashMap::new(),
            stats,
            copied: 0,
        }
    }

    /// The presenter thread stopped and handed `stage` back (a fresh one after a panic: every front is then copied
    /// whole again).
    pub fn restage(&mut self, stage: Option<Stage>) {
        if stage.is_none() {
            self.fronts.clear();
        }
        let mut stage = stage.unwrap_or_else(|| Stage::new(self.stats.clone()));
        stage.end_holds();
        self.stage = Some(stage);
    }
}

/// Glitnir's damage of scene `g` clipped to a `w` x `h` surface, and the snapshot (none for a surface without a
/// scene, whose damage alone is committed).
fn commit_scene(g: u64, w: i32, h: i32) -> (Vec<Rect>, Option<Arc<Committed>>) {
    let (rects, scene) = match glitnir::damage::commit_snapshot(g) {
        Some((rects, c)) => (rects, Some(c)),
        None => (glitnir::damage::with(g, |d| d.commit().to_vec()), None),
    };
    let all = Rect::new(0, 0, w, h);
    let rects = rects
        .iter()
        .map(|r| r.intersect(&all))
        .filter(|r| !r.is_empty())
        .collect();
    (rects, scene)
}

/// Append `r`'s pixels from `f`'s backing to `out`.
fn copy_rect(f: &SurfaceFrame, r: &Rect, out: &mut Vec<u32>) {
    for y in r.y..r.y + r.h {
        let at = y as usize * f.stride as usize + r.x as usize * 4;
        let row = &f.pixels[at..at + r.w as usize * 4];
        out.extend(
            row.as_chunks::<4>()
                .0
                .iter()
                .map(|c| u32::from_ne_bytes(*c)),
        );
    }
}

impl WaylandShell {
    pub(super) fn publish_impl(
        &mut self,
        root: WindowId,
        frames: &[SurfaceFrame],
        t_key_ns: u64,
    ) -> ShellResult<bool> {
        let m = &self.main;
        if !m.is_window(root) || m.children.contains_key(&root) {
            return Err(ShellError::State);
        }
        for f in frames {
            let fits = f.w > 0
                && f.h > 0
                && f.stride >= 4 * f.w
                && (f.stride as usize) * (f.h as usize - 1) + 4 * f.w as usize <= f.pixels.len();
            if !fits {
                return Err(ShellError::Arg);
            }
            if !m.is_window(f.window) || m.tree_root(f.window) != root {
                return Err(ShellError::State);
            }
        }
        let start = now_ns();
        // a surface that may not show a buffer keeps its damage in Glitnir
        let committed: Vec<_> = frames
            .iter()
            .filter(|f| f.flags & NHSHELL_SF_DRAWABLE != 0)
            .map(|f| (f, commit_scene(f.glitnir_root, f.w, f.h)))
            .collect();
        // a content layer's scroll or drawing is something new too, though its frame's damage leaves it out
        let damaged = committed.iter().any(|(_, (r, scene))| {
            !r.is_empty()
                || scene
                    .as_ref()
                    .is_some_and(|c| c.outputs.iter().any(Pending::changes))
        });
        if !damaged && !self.main.present_wants_packet(root) {
            self.main.present_nothing_new(root);
            return Ok(false);
        }
        let mail = self.main.mailbox.clone();
        let (gone, waited) = mail.begin_publish();
        for id in gone {
            self.presenting.fronts.remove(&id);
        }
        let mut copied = 0;
        let mut parts = Vec::with_capacity(committed.len());
        for (f, (damage, scene)) in committed {
            let full = self.presenting.fronts.get(&f.window) != Some(&(f.w, f.h));
            let all = Rect::new(0, 0, f.w, f.h);
            let rects = if full {
                vec![all]
            } else {
                // the own outputs' damage goes into the front too: it is the whole picture
                let outputs = scene.as_ref().map_or(&[][..], |c| c.outputs.as_slice());
                let mut v = damage.clone();
                v.extend(
                    outputs
                        .iter()
                        .flat_map(|o| Pending::from_frame(o).root_rects())
                        .map(|r| r.intersect(&all))
                        .filter(|r| !r.is_empty()),
                );
                v
            };
            let mut patch = mail.patches.take();
            for r in &rects {
                copy_rect(f, r, &mut patch);
            }
            copied += patch.len() * 4;
            self.presenting.fronts.insert(f.window, (f.w, f.h));
            let scale = self
                .main
                .scales
                .get(&f.window)
                .map_or(SCALE_ONE, |s| s.scale120());
            parts.push(Part {
                window: f.window,
                glitnir_root: f.glitnir_root,
                scene,
                damage,
                w: f.w,
                h: f.h,
                src: (f.src_x, f.src_y),
                flags: f.flags,
                alpha_cut: f.alpha_cut,
                scale: f64::from(scale.max(1)) / 120.0,
                rects,
                patch,
                full,
            });
        }
        let seq = mail.next_seq();
        let t_publish_ns = now_ns();
        let st = &self.presenting.stats;
        Counters::max(&st.publish_copy_max_ns, t_publish_ns - start);
        let pushed = mail.push(Packet {
            root,
            seq,
            t_key_ns,
            t_publish_ns,
            parts,
            whole: !damaged,
        });
        Counters::max(&st.main_wait_max_ns, waited.max(pushed));
        Counters::add(&st.packets, 1);
        self.presenting.copied = copied;
        match self.presenting.stage.as_mut() {
            // inline: this thread applies what it published
            Some(stage) => stage.drain(&mail),
            // the presenter applies it now, whether or not the tree may present yet
            None => self.link.wake(),
        }
        self.main.present_published(root, seq, damaged);
        Ok(true)
    }

    /// `ShellBackend::tree_snapshot`: each surface composed whole on this thread, as its next buffer would be, from the
    /// host's pixels and the scene committed with them (`Composer`s take no lock), then the surfaces over each other.
    /// Nothing of the presenter's is touched.  `composed` false leaves the scenes out: the host's pixels alone, square.
    pub(super) fn tree_snapshot_impl(
        &self,
        root: WindowId,
        frames: &[SurfaceFrame],
        composed: bool,
    ) -> ShellResult<Vec<u32>> {
        let m = &self.main;
        let first = frames.first().filter(|f| f.window == root);
        let Some(first) = first.filter(|_| m.is_window(root) && !m.children.contains_key(&root))
        else {
            return Err(ShellError::State);
        };
        let size = (first.w, first.h);
        let scale120 = |id| m.scales.get(&id).map_or(SCALE_ONE, |s| s.scale120()).max(1);
        let root_scale = f64::from(scale120(root)) / 120.0;
        let mut out = vec![0; size.0 as usize * size.1 as usize];
        for id in m.painted(root) {
            let Some(f) = frames.iter().find(|f| f.window == id) else {
                continue;
            };
            let all = Rect::new(0, 0, f.w, f.h);
            let mut front = Front::new(f.w, f.h);
            let mut px = Vec::with_capacity(front.data.len());
            copy_rect(f, &all, &mut px);
            front.write(&all, &px);
            // composed into a clear surface, as the presenter's new buffer is: a rounded corner and a shadow go over
            // what lies beneath the surface, not over the host's square pixels
            px.fill(0);
            let scale = f64::from(scale120(id)) / 120.0;
            let mut t = TargetView {
                data: &mut px,
                stride: f.w as usize,
                rect: all,
                scale,
            };
            let src = front.view(f.glitnir_root, (f.src_x, f.src_y), scale);
            if !composed || compose_root(f.glitnir_root, &mut t, &[all], &[src]).is_err() {
                front.copy_to(&mut t, &[all]);
            }
            if f.flags & NHSHELL_SF_ALPHA_HOST != 0 {
                alpha_multiply(&mut t, f.alpha_cut);
            }
            let (x, y) = m.origin(id);
            let at = |v: i32| (f64::from(v) * root_scale).round() as i32;
            let alpha = m.present_alpha(id).unwrap_or(u32::MAX);
            blend(&mut out, size, &px, f.w, (at(x), at(y)), alpha);
        }
        Ok(out)
    }

    /// Test hook: the bytes the latest publish copied.
    #[doc(hidden)]
    pub fn publish_copied(&self) -> usize {
        self.presenting.copied
    }

    /// The counters.  `presented` is current as read: inline this thread owns the stage and looks now; a presenter
    /// thread behind the newest request (one recorded with no wake) gets a turn, which looks at its end.
    pub(super) fn presenter_stats_impl(&self) -> NhShellPresenterStats {
        let (mail, st) = (&self.main.mailbox, &self.presenting.stats);
        match self.presenting.stage.as_ref() {
            Some(stage) => note_presented(stage, mail, &self.link.roots()),
            None if st.shown.load(Ordering::Acquire) < mail.seq() => self.link.wake(),
            None => {}
        }
        st.stats(self.link.on(), mail.seq())
    }

    /// Drop idle fronts (inline; the presenter thread trims its own); the next publish of those surfaces copies them
    /// whole.
    pub(super) fn publish_trim(&mut self, now: u64) {
        if let Some(stage) = self.presenting.stage.as_mut() {
            self.main.mailbox.trim(|| stage.trim(now));
        }
    }

    /// Window `id` is gone: its front and what waited for it go too, after the packets published before.
    pub(super) fn publish_forget(&mut self, id: u64) {
        self.presenting.fronts.remove(&id);
        self.post(Mail::Forget(id));
    }

    /// Hand `m` to the presenter, after the packets published before.
    fn post(&mut self, m: Mail) {
        let mail = &self.main.mailbox;
        let waited = mail.post(m);
        Counters::max(&self.presenting.stats.main_wait_max_ns, waited);
        match self.presenting.stage.as_mut() {
            Some(stage) => stage.drain(mail),
            None => self.link.wake(),
        }
    }

    /// Hand `m` about a child of root `root`'s tree to the presenter, and let the tree's next frame see it: a hide on
    /// an idle tree has no other wake.
    pub(super) fn motion_post(&mut self, root: u64, m: Mail) {
        self.post(m);
        self.main.present_tick(root);
    }

    /// Child `id`, which has a backdrop, moved: what the move owes wakes the presenter, which reads this first.
    pub(super) fn backdrop_placed(&mut self, id: u64, at: (i32, i32)) {
        self.post(Mail::Placed(id, at));
    }

    /// The presenter has a picture of window `id`, or will have with the packets on their way.
    pub(super) fn published_front(&self, id: u64) -> bool {
        self.presenting.fronts.contains_key(&id)
    }

    /// Window `id` went as a ghost under `key` in root `root`'s tree.
    pub(super) fn publish_ghost(&mut self, root: u64, id: u64, key: u64) {
        self.presenting.fronts.remove(&id);
        self.motion_post(root, Mail::Ghost(id, key));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glitnir::ease::Curve;
    use glitnir::motion::Leg;

    /// A hide whose wait ran out between the tick that saw it and the presenter's next sleep still needs its wake.
    #[test]
    fn next_hold_keeps_a_wait_that_just_ended() {
        let leg = Leg {
            kind: Kind::Fade,
            duration_ns: 80_000_000,
            curve: Curve::EASE_IN,
        };
        let spec = MotionSpec {
            show: leg,
            hide: leg,
        };
        let mut stage = Stage::new(Arc::default());
        assert_eq!(stage.next_hold(), None);
        let mut hiding = Moving::new(spec, true, true);
        hiding.wanted = false;
        hiding.hold = 5;
        stage.moves.insert(1, hiding);
        assert_eq!(stage.next_hold(), Some(5));
        // a hide with no wait is due now, and a show has nothing to wait for
        stage.moves.get_mut(&1).unwrap().hold = 0;
        assert_eq!(stage.next_hold(), None);
        let shown = stage.moves.get_mut(&1).unwrap();
        shown.wanted = true;
        shown.hold = 5;
        assert_eq!(stage.next_hold(), None);
    }
}

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

use crate::api::*;
use crate::wayland::WaylandShell;
use crate::wayland::layers::Pending;
use crate::wayland::presenter::Counters;
use crate::wayland::scale::SCALE_ONE;
use glitnir::committed::Committed;
use glitnir::compose::{Composer, SourceView, TargetView, compose_committed};
use glitnir::geom::Rect;
use glitnir::pool::IDLE_NS;
use glitnir::stats::StatsEntry;
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

/// Presents whose damage repairs an older buffer; an older buffer is composed whole.
const HIST: usize = 4;
/// Patch buffers kept for the next publish.
const SPARE_PATCHES: usize = 4;

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
}

/// What the host's thread hands the presenter, in order: a packet, or a window that is gone (a surface id may be
/// reused for a new window after it).
pub enum Mail {
    Packet(Packet),
    Forget(u64),
}

#[derive(Default)]
struct Inbox {
    mail: VecDeque<Mail>,
    /// A publish decided which surfaces it copies whole and has not pushed yet: no front may go meanwhile.
    busy: bool,
    /// Surfaces whose fronts the presenter dropped, until the host's next publish learns it.
    trimmed: Vec<u64>,
}

/// Packets on their way to the presenter.  The lock is held for a push, a take or a trim only.
#[derive(Default)]
pub struct Mailbox {
    inbox: Mutex<Inbox>,
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
    /// The newest packet's sequence number (0: none yet).
    pub fn seq(&self) -> u64 {
        self.seq.load(Ordering::Acquire)
    }

    fn next_seq(&self) -> u64 {
        self.seq.fetch_add(1, Ordering::AcqRel) + 1
    }

    /// A publish starts deciding what it copies: the fronts the presenter dropped since the last one, and the
    /// nanoseconds spent waiting for the lock.  `push` ends it.
    fn begin_publish(&self) -> (Vec<u64>, u64) {
        let (mut b, waited) = timed(&self.inbox);
        b.busy = true;
        (std::mem::take(&mut b.trimmed), waited)
    }

    /// Queue `p` and end the publish; the nanoseconds spent waiting for the lock.
    pub fn push(&self, p: Packet) -> u64 {
        let (mut b, waited) = timed(&self.inbox);
        b.mail.push_back(Mail::Packet(p));
        b.busy = false;
        waited
    }

    /// Queue `m` outside a publish; the nanoseconds spent waiting for the lock.
    pub fn post(&self, m: Mail) -> u64 {
        let (mut b, waited) = timed(&self.inbox);
        b.mail.push_back(m);
        waited
    }

    pub fn take(&self) -> VecDeque<Mail> {
        std::mem::take(&mut locked(&self.inbox).mail)
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

/// The presenter's side: fronts, merged packets, the display's composer and the counters.
#[derive(Default)]
pub struct Stage {
    surfaces: HashMap<u64, Surface>,
    /// Each root's earliest key time that no commit has shown yet.
    keys: HashMap<u64, u64>,
    /// Each root's newest packet applied to the fronts (its sequence number): a content ack recorded before it may
    /// ride a buffer composed now.
    applied: HashMap<u64, u64>,
    composer: Composer,
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
            }
        }
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
        self.surfaces
            .values()
            .filter(|s| s.front.is_some() && s.damage.is_empty() && s.outs.is_empty())
            .map(|s| s.last_ns + IDLE_NS)
            .min()
    }

    /// Write `p`'s patches into the fronts and merge it into what waits for its tree's commit.
    fn apply(&mut self, p: Packet, patches: &PatchPool) {
        self.applied.insert(p.root, p.seq);
        let waiting = self
            .surfaces
            .values_mut()
            .filter(|s| s.root == p.root)
            .fold(false, |w, s| {
                // the newest packet says which surfaces may show a buffer
                s.drawable = false;
                w || !s.damage.is_empty()
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
            let outs_changed = Pending::merge_all(&mut s.outs, outputs);
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

    /// What surface `id`'s next buffer is: its front's size and the damage that waits.
    pub fn next_frame(&self, id: u64) -> Option<(i32, i32, Vec<Rect>)> {
        let s = self.surfaces.get(&id)?;
        let f = s.front.as_ref()?;
        (!s.damage.is_empty()).then(|| (f.w, f.h, s.damage.clone()))
    }

    /// Compose surface `id`'s buffer `buf` from its front: the waiting damage and what the buffer's age misses.
    pub fn compose(&mut self, id: u64, buf: &NhShellBuffer) {
        let Stage {
            surfaces, composer, ..
        } = self;
        let Some(s) = surfaces.get_mut(&id) else {
            return;
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
        let composed = s
            .scene
            .as_ref()
            .is_some_and(|c| compose_committed(c, composer, &mut t, &clips, &[src]).is_ok());
        if !composed {
            front.copy_to(&mut t, &clips);
        }
        if s.alpha_host {
            alpha_multiply(&mut t, s.alpha_cut);
        }
    }

    /// Surface `id`'s buffer was committed: its stats entry (with the tree's key time when it is the root), and its
    /// damage joins the history.
    pub fn presented(&mut self, id: u64) {
        self.record(id);
        if let Some(s) = self.surfaces.get_mut(&id) {
            s.hist.push_front(std::mem::take(&mut s.damage));
            s.hist.truncate(HIST);
        }
        Counters::add(&self.stats.commits, 1);
    }

    /// Root `root`'s tree showed new pixels on its content layers alone: its stats entry and key time, unless the
    /// root's own damage still waits (its buffer's commit records them).
    pub fn presented_layers(&mut self, root: u64) {
        if !self.damaged(root) {
            self.record(root);
        }
    }

    /// Surface `id`'s stats entry for a commit that showed what waited (with the tree's key time when it is the root).
    fn record(&mut self, id: u64) {
        let Some(s) = self.surfaces.get_mut(&id) else {
            return;
        };
        let key = if s.root == id {
            self.keys.insert(id, 0).unwrap_or(0)
        } else {
            0
        };
        let mut e = StatsEntry {
            t_update_ns: key,
            t_commit_ns: s.draw_start_ns,
            t_draw_start_ns: s.draw_start_ns,
            t_draw_end_ns: now_ns(),
            presented_us: -1,
            damage_px: s.damage_px,
            painted_px: s.painted_px,
            backfill: 2,
            ..StatsEntry::default()
        };
        self.composer.fill_stats(s.glitnir_root, &mut e);
        glitnir::stats::record(s.glitnir_root, e);
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

    /// Layer `layer` of root `root` committed what waited for it.
    pub fn take_pending(&mut self, root: u64, layer: u64) {
        if let Some(s) = self.surfaces.get_mut(&root) {
            s.outs.remove(&layer);
        }
        Counters::add(&self.stats.commits, 1);
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
        let Some(s) = surfaces.get(&root) else {
            return;
        };
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
        let mut out = Vec::new();
        for (id, s) in &mut self.surfaces {
            if s.front.is_some()
                && s.damage.is_empty()
                && s.outs.is_empty()
                && now.saturating_sub(s.last_ns) > IDLE_NS
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
        }
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
        self.stage = Some(stage.unwrap_or_else(|| Stage::new(self.stats.clone())));
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

    /// Test hook: the bytes the latest publish copied.
    #[doc(hidden)]
    pub fn publish_copied(&self) -> usize {
        self.presenting.copied
    }

    pub(super) fn presenter_stats_impl(&self) -> NhShellPresenterStats {
        self.presenting.stats.stats(self.link.on())
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
        let mail = &self.main.mailbox;
        let waited = mail.post(Mail::Forget(id));
        Counters::max(&self.presenting.stats.main_wait_max_ns, waited);
        match self.presenting.stage.as_mut() {
            Some(stage) => stage.drain(mail),
            None => self.link.wake(),
        }
    }
}

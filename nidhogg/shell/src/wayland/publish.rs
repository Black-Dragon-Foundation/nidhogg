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
//! In inline mode the host's thread is the presenter: `publish` applies its packet at once.

use crate::api::*;
use crate::wayland::WaylandShell;
use crate::wayland::scale::SCALE_ONE;
use glitnir::committed::Committed;
use glitnir::compose::{Composer, SourceView, TargetView, compose_committed};
use glitnir::geom::Rect;
use glitnir::pool::IDLE_NS;
use glitnir::stats::StatsEntry;
use std::collections::{HashMap, VecDeque};
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

/// Packets on their way to the presenter.  The lock is held for a push or a take only.
#[derive(Default)]
pub struct Mailbox {
    queue: Mutex<VecDeque<Packet>>,
    seq: AtomicU64,
    pub patches: PatchPool,
}

impl Mailbox {
    /// The newest packet's sequence number (0: none yet).
    pub fn seq(&self) -> u64 {
        self.seq.load(Ordering::Acquire)
    }

    fn next_seq(&self) -> u64 {
        self.seq.fetch_add(1, Ordering::AcqRel) + 1
    }

    /// Queue `p`; the nanoseconds spent waiting for the lock.
    pub fn push(&self, p: Packet) -> u64 {
        let t = now_ns();
        let mut q = locked(&self.queue);
        let waited = now_ns().saturating_sub(t);
        q.push_back(p);
        waited
    }

    pub fn take(&self) -> VecDeque<Packet> {
        std::mem::take(&mut *locked(&self.queue))
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

    /// The plain copy when there is nothing to compose with: each pixel at its own place, within `clips`.
    fn copy_to(&self, t: &mut TargetView, clips: &[Rect]) {
        let bounds = Rect::new(0, 0, self.w, self.h).intersect(&t.rect);
        for r in clips.iter().map(|r| r.intersect(&bounds)) {
            if r.is_empty() {
                continue;
            }
            for y in r.y..r.y + r.h {
                let from = y as usize * self.w as usize + r.x as usize;
                let to = y as usize * t.stride + r.x as usize;
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
}

/// The presenter's side: fronts, merged packets, the display's composer and the counters.
#[derive(Default)]
pub struct Stage {
    surfaces: HashMap<u64, Surface>,
    /// Each root's earliest key time that no commit has shown yet.
    keys: HashMap<u64, u64>,
    composer: Composer,
    pub stats: NhShellPresenterStats,
}

impl Stage {
    /// Apply every packet the mailbox holds.
    pub fn drain(&mut self, mail: &Mailbox) {
        for p in mail.take() {
            self.apply(p, &mail.patches);
        }
    }

    /// Write `p`'s patches into the fronts and merge it into what waits for its tree's commit.
    fn apply(&mut self, p: Packet, patches: &PatchPool) {
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
            self.stats.superseded += 1;
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
            if part.full || !size_ok {
                s.front = Some(Front::new(part.w, part.h));
            }
            if let Some(front) = &mut s.front {
                let mut at = 0;
                for r in &part.rects {
                    let n = r.w as usize * r.h as usize;
                    front.write(r, &part.patch[at..at + n]);
                    at += n;
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
            if !part.damage.is_empty() && s.draw_start_ns == 0 {
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
        s.hist.push_front(std::mem::take(&mut s.damage));
        s.hist.truncate(HIST);
        s.draw_start_ns = 0;
        s.damage_px = 0;
        s.painted_px = 0;
        self.stats.commits += 1;
    }

    /// Drop the fronts of surfaces with no publish for the pool's quiet period and nothing waiting; their windows.
    pub fn trim(&mut self, now: u64) -> Vec<u64> {
        let mut out = Vec::new();
        for (id, s) in &mut self.surfaces {
            if s.front.is_some() && s.damage.is_empty() && now.saturating_sub(s.last_ns) > IDLE_NS {
                s.front = None;
                out.push(*id);
            }
        }
        self.stats.front_trims += out.len() as u64;
        out
    }

    /// Window `id` is gone.
    pub fn forget(&mut self, id: u64) {
        if let Some(s) = self.surfaces.remove(&id) {
            self.composer.forget(s.glitnir_root);
        }
        self.keys.remove(&id);
    }
}

/// The publishing side of a display (the host's thread), and the presenter's stage while it runs inline.
#[derive(Default)]
pub struct Presenting {
    pub stage: Stage,
    /// The size of each surface's front as the last publish left it (absent: the next publish copies it whole).
    fronts: HashMap<u64, (i32, i32)>,
    pub thread_on: bool,
    /// Bytes the latest publish copied.
    pub copied: usize,
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
        let damaged = committed.iter().any(|(_, (r, _))| !r.is_empty());
        if !damaged && !self.main.present_wants_packet(root) {
            return Ok(false);
        }
        let mail = self.main.mailbox.clone();
        let mut copied = 0;
        let mut parts = Vec::with_capacity(committed.len());
        for (f, (damage, scene)) in committed {
            let full = self.presenting.fronts.get(&f.window) != Some(&(f.w, f.h));
            let rects = if full {
                vec![Rect::new(0, 0, f.w, f.h)]
            } else {
                damage.clone()
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
        let st = &mut self.presenting.stage.stats;
        st.publish_copy_max_ns = st.publish_copy_max_ns.max(t_publish_ns - start);
        let waited = mail.push(Packet {
            root,
            seq,
            t_key_ns,
            t_publish_ns,
            parts,
        });
        let st = &mut self.presenting.stage.stats;
        st.main_wait_max_ns = st.main_wait_max_ns.max(waited);
        st.packets += 1;
        self.presenting.copied = copied;
        // no presenter thread runs yet: this thread applies what it published
        self.presenting.stage.drain(&mail);
        self.main.present_published(root, seq, damaged);
        Ok(true)
    }

    /// Test hook: the bytes the latest publish copied.
    #[doc(hidden)]
    pub fn publish_copied(&self) -> usize {
        self.presenting.copied
    }

    pub(super) fn presenter_stats_impl(&self) -> NhShellPresenterStats {
        NhShellPresenterStats {
            thread_on: u32::from(self.presenting.thread_on),
            ..self.presenting.stage.stats
        }
    }

    /// Drop idle fronts; the next publish of those surfaces copies them whole.
    pub(super) fn publish_trim(&mut self, now: u64) {
        for id in self.presenting.stage.trim(now) {
            self.presenting.fronts.remove(&id);
        }
    }

    /// Window `id` is gone: its front and what waited for it go too.
    pub(super) fn publish_forget(&mut self, id: u64) {
        self.presenting.stage.forget(id);
        self.presenting.fronts.remove(&id);
    }
}

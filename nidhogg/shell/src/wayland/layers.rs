//! Content layers: a window that scrolls whole shows on a surface of its own, over its frame's.
//!
//! When the host makes a window layer of an opaque root its own output (Glitnir's `set_output`), each commit of the
//! root's scene reports the output apart from the frame (`OutputFrame`): its rectangle, snapped to device rows and
//! columns, how far its content moved, and what changed after the move.  Whoever presents the tree (`present_tree`)
//! keeps one `ContentLayer` per output on the root's `Present`:
//! - a `wl_surface` on the main queue (its events are ignored), a sync `wl_subsurface` of the root placed right above
//!   it (below the root's child windows), an empty input region (the pointer falls through to the root), an opaque
//!   region and a `wp_viewport`;
//! - a pool of ring buffers (`glitnir::pool::Ring`), each `min(2 x view, 8192)` rows tall.  The viewport's source is
//!   the view, `(0, offset, w, view)`; a full-window scroll moves the offset, so rows keep their place in every buffer
//!   and only the exposed ones are drawn and damaged.  When the view would leave the buffer, it goes back to the
//!   middle (a rebase): the newest buffer's view is copied there, and the layer is damaged once in full.
//!
//! A buffer taken for the next commit is first repaired from the newest one, buffer to buffer: the damage of the
//! commits it missed (all of them share the layer's row coordinates), or the whole view when it is too old.  Then the
//! new damage is composed from the root's front, and the pages more than half a view away from the view are given
//! back (`fallocate` punch hole).  That happens before the commit, while the buffer is the presenter's: a buffer the
//! compositor holds is never written, and punched rows lie outside every source rectangle.
//!
//! The layer's commits are part of the tree's: they go in `present_tree`'s descendant pass, before the root's.  An
//! output that is gone (demoted) unmaps (`attach(NULL)`) with the root's next buffer commit, which shows the window's
//! rectangle recomposed in the frame (Glitnir damages it there), so the sync tree applies both at once; the layer's
//! objects go after that commit.

use crate::wayland::MainState;
use crate::wayland::present::{Present, lock};
use crate::wayland::presenter::Wire;
use crate::wayland::publish::Stage;
use crate::wayland::reader::ReaderState;
use crate::wayland::shm::ShmBuf;
use glitnir::committed::OutputFrame;
use glitnir::geom::Rect;
use glitnir::pool::{BufferSlot, GROWN_BUFFERS, Pool, Ring, RingStep, merged};
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use wayland_client::protocol::wl_buffer::{self, WlBuffer};
use wayland_client::protocol::wl_subsurface::WlSubsurface;
use wayland_client::protocol::wl_surface::{self, WlSurface};
use wayland_client::{Connection, Dispatch, QueueHandle};
use wayland_protocols::wp::viewporter::client::wp_viewport::WpViewport;

/// Commits whose damage repairs an older buffer; an older one is repaired whole.
const HIST: usize = 4;

/// An own output's changes since its layer's last commit: Glitnir's `OutputFrame`s merged.
#[derive(Debug, Clone, PartialEq)]
pub struct Pending {
    /// The output's rectangle (root device px).
    pub rect: Rect,
    /// How far the content moved since the layer's last commit (negative: up).
    pub scroll: i32,
    /// What changed after the move, in the output's coordinates.
    pub damage: Vec<Rect>,
    /// Nothing of the layer's last picture can be kept: a new ring, the whole view drawn.
    pub full: bool,
}

impl Pending {
    /// `o` changes what its layer shows.
    pub fn changes(o: &OutputFrame) -> bool {
        o.full || o.scroll != 0 || !o.damage.is_empty()
    }

    pub fn from_frame(o: &OutputFrame) -> Pending {
        let view = Rect::new(0, 0, o.rect.w, o.rect.h);
        Pending {
            rect: o.rect,
            scroll: if o.full { 0 } else { o.scroll },
            damage: if o.full { vec![view] } else { o.damage.clone() },
            full: o.full,
        }
    }

    /// A later commit's `o` on top: the earlier damage moves with the later scroll, and the scrolls add up.
    fn merge(&mut self, o: &OutputFrame) {
        if o.full || self.rect != o.rect {
            *self = Pending::from_frame(o);
            self.full = true;
            self.damage = vec![Rect::new(0, 0, o.rect.w, o.rect.h)];
            return;
        }
        if self.full {
            return;
        }
        let view = Rect::new(0, 0, o.rect.w, o.rect.h);
        let moved = self
            .damage
            .iter()
            .map(|r| r.translate(0, o.scroll).intersect(&view))
            .filter(|r| !r.is_empty());
        self.damage = moved.chain(o.damage.iter().copied()).collect();
        self.scroll = self.scroll.saturating_add(o.scroll);
    }

    /// Merge a scene's `frames` into `outs`: an output the scene no longer has drops what waited for it.  `true` when
    /// some output changed.
    pub fn merge_all(outs: &mut BTreeMap<u64, Pending>, frames: &[OutputFrame]) -> bool {
        outs.retain(|id, _| frames.iter().any(|o| o.layer == *id));
        let mut changed = false;
        for o in frames.iter().filter(|o| Pending::changes(o)) {
            changed = true;
            match outs.get_mut(&o.layer) {
                Some(p) => p.merge(o),
                None => {
                    outs.insert(o.layer, Pending::from_frame(o));
                }
            }
        }
        changed
    }

    /// The damage in the root's device pixels.
    pub fn root_rects(&self) -> Vec<Rect> {
        self.damage
            .iter()
            .map(|r| r.translate(self.rect.x, self.rect.y))
            .collect()
    }
}

/// A content layer's surface: its events (enter, preferred scale) concern nobody.
pub struct LayerMark;

impl Dispatch<WlSurface, LayerMark> for MainState {
    fn event(
        _: &mut Self,
        _: &WlSurface,
        _: wl_surface::Event,
        _: &LayerMark,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

/// The user data of a ring buffer's `wl_buffer`: its root's presenter state, the layer and the slot.
pub type LayerBufData = (Arc<Mutex<Present>>, u64, usize);

/// A ring buffer's release: its layer may take it again; a layer starved for buffers presents the tree again.
impl Dispatch<WlBuffer, LayerBufData> for ReaderState {
    fn event(
        _: &mut Self,
        _: &WlBuffer,
        event: wl_buffer::Event,
        (present, layer, slot): &LayerBufData,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_buffer::Event::Release = event {
            lock(present).layer_released(*layer, *slot);
        }
    }
}

/// Copy `r` of `src` to `dst` at row `dst_y` (same columns).  `src` and `dst` may be one buffer: rows are moved away
/// from the direction of travel first.
fn copy_rows(src: &ShmBuf, dst: &ShmBuf, r: Rect, dst_y: i32) {
    let dy = dst_y - r.y;
    let r = r
        .intersect(&Rect::new(0, 0, src.w.min(dst.w), src.h))
        .intersect(&Rect::new(0, -dy, dst.w, dst.h));
    if r.is_empty() {
        return;
    }
    let (ss, ds) = (src.stride as usize, dst.stride as usize);
    for i in 0..r.h {
        let y = if dy > 0 { r.y + r.h - 1 - i } else { r.y + i };
        let from = y as usize * ss + r.x as usize * 4;
        let to = (y + dy) as usize * ds + r.x as usize * 4;
        // SAFETY: both rows lie inside their mappings (clipped above); `copy` allows them to overlap.
        unsafe {
            std::ptr::copy(
                src.pixels().add(from),
                dst.pixels().add(to),
                r.w as usize * 4,
            );
        }
    }
}

/// One frame of a content layer, between the decision and the send: the buffer it draws into and how.
pub(crate) struct LayerJob {
    pub(crate) layer: u64,
    slot: BufferSlot,
    target: Arc<ShmBuf>,
    /// The newest committed buffer, read for the repair or the rebase.
    source: Option<Arc<ShmBuf>>,
    /// Rectangles of `source` (buffer px) and the row each goes to in `target`.
    copies: Vec<(Rect, i32)>,
    /// What is composed from the front (root device px).
    clips: Vec<Rect>,
    rect: Rect,
    ring: Ring,
    /// The ring was made anew (a new output, a moved or resized one, a new scale).
    reshape: bool,
    /// The commit's damage, in buffer px: what the compositor uploads, and what an older buffer will miss.
    damage: Vec<Rect>,
}

impl LayerJob {
    /// Repair, compose and punch the buffer: on the thread that presents, no lock held.
    pub(crate) fn draw(&self, stage: &mut Stage, root: u64) {
        if let Some(src) = &self.source {
            for (r, y) in &self.copies {
                copy_rows(src, &self.target, *r, *y);
            }
        }
        let t = &self.target;
        let (w, vh) = (self.rect.w, self.rect.h);
        if vh <= 0 || w <= 0 || self.ring.offset + vh > t.h || w > t.w {
            return;
        }
        let stride = t.stride as usize / 4;
        let len = stride * (vh as usize - 1) + w as usize;
        // SAFETY: the view's rows, `offset .. offset + vh`, lie inside the mapping (checked above); the buffer is out of
        // the compositor's hands (the pool handed it out) and only this thread writes it until its commit.
        let data = unsafe {
            let first = t
                .pixels()
                .cast::<u32>()
                .add(self.ring.offset as usize * stride);
            std::slice::from_raw_parts_mut(first, len)
        };
        stage.compose_output(root, data, stride, self.rect, &self.clips);
        for (from, to) in self.ring.punch_rows(vh) {
            t.punch(from, to);
        }
    }
}

/// A window's own layer over its root's surface.
pub(crate) struct ContentLayer {
    /// The Glitnir window layer it shows.
    pub(crate) id: u64,
    surface: WlSurface,
    sub: WlSubsurface,
    viewport: WpViewport,
    /// The output's rectangle the ring was made for (empty before the first commit).
    rect: Rect,
    ring: Ring,
    pool: Pool,
    bufs: HashMap<usize, Arc<ShmBuf>>,
    /// The slot committed last: every other buffer is repaired from it.
    newest: Option<usize>,
    /// The damage of the latest commits, newest first, in buffer px.
    hist: VecDeque<Vec<Rect>>,
    /// Every buffer was busy: the next release presents the tree again.
    starved: bool,
    /// Not in the newest scene: it unmaps with the root's next buffer, then goes.
    pub(crate) leaving: bool,
    /// Its place right above the root has been asked for since the root's children last moved.
    pub(crate) placed: bool,
}

impl ContentLayer {
    /// Whether the compositor offers what a layer needs.
    pub(crate) fn can(wire: &Wire) -> bool {
        wire.compositor.is_some()
            && wire.subcompositor.is_some()
            && wire.viewporter.is_some()
            && wire.shm.is_some()
    }

    /// A layer for window layer `id` over `root`: a sync subsurface taking no input.  Its place above the root is
    /// asked for with the root's commit (`placed`).
    pub(crate) fn new(id: u64, wire: &Wire, root: &WlSurface) -> Option<ContentLayer> {
        let compositor = wire.compositor.as_ref()?;
        let subcompositor = wire.subcompositor.as_ref()?;
        let viewporter = wire.viewporter.as_ref()?;
        let surface = compositor.create_surface(&wire.mqh, LayerMark);
        // sync (the default): the layer shows with its root's commit
        let sub = subcompositor.get_subsurface(&surface, root, &wire.mqh, ());
        let viewport = viewporter.get_viewport(&surface, &wire.mqh, ());
        let empty = compositor.create_region(&wire.mqh, ());
        surface.set_input_region(Some(&empty));
        empty.destroy();
        Some(ContentLayer {
            id,
            surface,
            sub,
            viewport,
            rect: Rect::default(),
            ring: Ring::new(0),
            pool: Pool::new(),
            bufs: HashMap::new(),
            newest: None,
            hist: VecDeque::new(),
            starved: false,
            leaving: false,
            placed: false,
        })
    }

    /// The subsurface, for its place in the root's stacking.
    pub(crate) fn subsurface(&self) -> &WlSubsurface {
        &self.sub
    }

    /// Drop the buffers the pool let go of.
    fn reap(&mut self, now: u64) {
        for s in self.pool.trim(now) {
            self.bufs.remove(&s.id);
            if self.newest == Some(s.id) {
                self.newest = None;
            }
        }
    }

    /// Take a buffer for `p` and plan how it is drawn.  `grow`: the tree's pacer is idle, so a pool whose buffers
    /// are all busy grows by one.  `None` when no buffer is free (the layer starves: its changes keep waiting) or none
    /// can be made.  `me` is the root's presenter state, for the buffers' releases.
    pub(crate) fn begin(
        &mut self,
        me: &Arc<Mutex<Present>>,
        wire: &Wire,
        p: &Pending,
        grow: bool,
        now: u64,
    ) -> Option<LayerJob> {
        let (w, vh) = (p.rect.w, p.rect.h);
        let shm = wire.shm.as_ref()?;
        if w <= 0 || vh <= 0 {
            return None;
        }
        let reshape = p.full || self.rect != p.rect;
        let prev = self.ring.offset;
        let (ring, step) = if reshape {
            (Ring::new(vh), None)
        } else {
            let mut r = self.ring;
            let step = (p.scroll != 0).then(|| r.scroll(p.scroll, vh));
            (r, step)
        };
        self.reap(now);
        let mut slot = self.pool.begin(w, ring.height, now);
        if slot.is_none() && grow {
            slot = self.pool.begin_with(w, ring.height, now, GROWN_BUFFERS);
        }
        let Some(slot) = slot else {
            self.starved = true;
            return None;
        };
        self.reap(now);
        let target = match self.bufs.get(&slot.id) {
            Some(b) => b.clone(),
            None => {
                let data = (me.clone(), self.id, slot.id);
                match ShmBuf::create_ring(shm, &wire.mqh, &wire.rqh, w, ring.height, data) {
                    Ok(b) => {
                        let b = Arc::new(b);
                        self.bufs.insert(slot.id, b.clone());
                        b
                    }
                    Err(_) => {
                        self.pool.abandon(slot);
                        return None;
                    }
                }
            }
        };
        let view = Rect::new(0, ring.offset, w, vh);
        let source = self
            .newest
            .filter(|_| !reshape)
            .and_then(|id| self.bufs.get(&id))
            .filter(|b| (b.w, b.h) == (w, ring.height))
            .cloned();
        let to_root = |rs: &[Rect]| -> Vec<Rect> {
            rs.iter().map(|r| r.translate(p.rect.x, p.rect.y)).collect()
        };
        let mut job = LayerJob {
            layer: self.id,
            slot,
            target,
            source: None,
            copies: Vec::new(),
            clips: vec![p.rect],
            rect: p.rect,
            ring,
            reshape,
            damage: vec![view],
        };
        let Some(source) = source else {
            // nothing to copy from: the whole view is composed
            return Some(job);
        };
        if step == Some(RingStep::Rebased) {
            // the newest view, moved by the scroll, at the new offset; the exposed rows are composed
            let (s, n) = (p.scroll, vh - p.scroll.abs());
            if n > 0 {
                let src = Rect::new(0, prev + (-s).max(0), w, n);
                job.copies.push((src, ring.offset + s.max(0)));
                job.clips = to_root(&p.damage);
            }
        } else {
            // rows keep their place: the commits this buffer missed come from the newest one
            let age = slot.age as usize;
            if self.newest != Some(slot.id) {
                let missed: Vec<Rect> = if age == 0 || age - 1 > self.hist.len() {
                    vec![view]
                } else {
                    self.hist.iter().take(age - 1).flatten().copied().collect()
                };
                job.copies.extend(
                    missed
                        .iter()
                        .map(|r| r.intersect(&view))
                        .filter(|r| !r.is_empty())
                        .map(|r| (r, r.y)),
                );
            }
            job.clips = to_root(&p.damage);
            job.damage = p
                .damage
                .iter()
                .map(|r| ring.to_buffer(*r).intersect(&view))
                .filter(|r| !r.is_empty())
                .collect();
        }
        job.source = Some(source);
        Some(job)
    }

    /// The job's buffer goes back unused.
    pub(crate) fn put_back(&mut self, job: LayerJob) {
        self.pool.abandon(job.slot);
    }

    /// Commit `job` (a sync child's commit: it applies with the root's).  A new ring places the layer and sizes its
    /// viewport and opaque region; every commit sets the viewport's source to the view.  `scale120`: the root's.
    pub(crate) fn commit(&mut self, job: LayerJob, wire: &Wire, scale120: u32) {
        let (w, vh) = (job.rect.w, job.rect.h);
        if job.reshape || self.rect != job.rect {
            // the rectangle is on the device grid: its logical one maps back onto it exactly
            let l = glitnir::snap::outward(job.rect, scale120.max(1));
            self.sub.set_position(l.x, l.y);
            self.viewport.set_destination(l.w.max(1), l.h.max(1));
            self.surface.set_buffer_scale(1);
            if let Some(compositor) = &wire.compositor {
                let opaque = compositor.create_region(&wire.mqh, ());
                opaque.add(0, 0, l.w, l.h);
                self.surface.set_opaque_region(Some(&opaque));
                opaque.destroy();
            }
            self.rect = job.rect;
            self.hist.clear();
        }
        let off = f64::from(job.ring.offset);
        self.viewport
            .set_source(0.0, off, f64::from(w), f64::from(vh));
        self.surface.attach(Some(&job.target.buffer), 0, 0);
        for r in merged(&job.damage) {
            self.surface.damage_buffer(r.x, r.y, r.w, r.h);
        }
        self.surface.commit();
        self.pool.submitted(job.slot);
        self.newest = Some(job.slot.id);
        self.ring = job.ring;
        self.hist.push_front(job.damage);
        self.hist.truncate(HIST);
    }

    /// Show nothing from the root's next commit on (the window is drawn in the frame again).
    pub(crate) fn unmap(&self) {
        self.surface.attach(None, 0, 0);
        self.surface.commit();
    }

    /// Slot `slot`'s buffer is the layer's again.  `true` when the layer was starved.
    pub(crate) fn released(&mut self, slot: usize) -> bool {
        if let Some(b) = self.bufs.get(&slot) {
            let s = BufferSlot {
                id: slot,
                age: 0,
                w: b.w,
                h: b.h,
            };
            self.pool.released(s);
        }
        std::mem::take(&mut self.starved)
    }

    /// Test hook: (surface id, view offset, ring height, each buffer's resident 512-byte blocks).
    pub(crate) fn info(&self) -> (u64, i32, i32, Vec<u64>) {
        use wayland_client::Proxy;
        let blocks = self.bufs.values().map(|b| b.blocks()).collect();
        (
            u64::from(self.surface.id().protocol_id()),
            self.ring.offset,
            self.ring.height,
            blocks,
        )
    }
}

impl Drop for ContentLayer {
    fn drop(&mut self) {
        self.bufs.clear();
        self.viewport.destroy();
        self.sub.destroy();
        self.surface.destroy();
    }
}

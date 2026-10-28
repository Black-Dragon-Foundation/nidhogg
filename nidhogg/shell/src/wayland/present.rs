//! The presenter: `present_begin` hands out a buffer of the window's pool, `present_commit` shows it.
//!
//! A commit attaches the buffer, damages it (buffer coordinates), sets the viewport destination to the logical size and
//! `buffer_scale` 1 (the integer scale without a viewporter), requests a frame callback (and presentation feedback
//! for the statistics) and commits.
//!
//! One pacer per tree decides when the host may draw: a tree is a root (a top-level window or a popup) and its child
//! windows, which are sync subsurfaces committed before their parents, the root last.  Children have buffers but no
//! pacer: their drawing, their owed state and their buffer releases wake the root.  The host calls `window_dirty` and
//! draws when it gets `FRAME_READY` (or `window_ready` says so).  The pacer is `Idle` or `Waiting` for the frame
//! callback of the last commit that carried content; a commit without new content (an ack, empty damage) requests no
//! callback and leaves the state alone.  An ack never waits: it makes the pacer ready at once, in any state but starved.  A pacer
//! is starved when `present_begin` found every buffer busy and leaves that state on a release; when it is idle and
//! the two buffers are busy, the pool grows to three first.  A suspended window only acks.  A pacer that waits
//! re-arms (a callback and an empty commit) when a configure, the end of suspension, `wl_surface.enter` or a scale
//! change shows the surface may be visible again.  Presentation feedback never gates anything.  Callbacks, feedback
//! and releases run on the reader queue.  The buffer policy is `glitnir::pool`.
//!
//! A host that publishes (`publish.rs`) has the shell decide: `present_tree` is the present path's one rule, run at
//! the root's `FRAME_READY`.  For it a content ack counts only once a packet was published after the ack.
//!
//! **The tree owns its commit-time state.**  Once a window's role is set up (`present_register`), every request
//! whose effect waits for `wl_surface.commit` is recorded on its `Present` (position, stacking, input region, unmap,
//! alpha, window geometry, the re-arm) and sent right before the commit it belongs to, through the proxies in its
//! `Commit` record; only presenting commits a registered tree.  The host's thread keeps creating objects, setting
//! roles up, immediate requests and destroying, which happens under the tree's mutex (the root's `Present`) and
//! retires the window.

use crate::api::*;
use crate::event::*;
use crate::wayland::conn::Shared;
use crate::wayland::reader::ReaderState;
use crate::wayland::scale::{SCALE_ONE, buffer_size};
use crate::wayland::shm::ShmBuf;
use crate::wayland::{MainState, WaylandShell};
use glitnir::geom::Rect;
use glitnir::pool::{BufferSlot, GROWN_BUFFERS, Pool, merged};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};
use wayland_client::protocol::wl_buffer::{self, WlBuffer};
use wayland_client::protocol::wl_callback::{self, WlCallback};
use wayland_client::protocol::wl_compositor::WlCompositor;
use wayland_client::protocol::wl_region::WlRegion;
use wayland_client::protocol::wl_shm_pool::WlShmPool;
use wayland_client::protocol::wl_subsurface::WlSubsurface;
use wayland_client::protocol::wl_surface::WlSurface;
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle, delegate_noop};
use wayland_protocols::wp::alpha_modifier::v1::client::wp_alpha_modifier_surface_v1::WpAlphaModifierSurfaceV1;
use wayland_protocols::wp::presentation_time::client::wp_presentation::WpPresentation;
use wayland_protocols::wp::presentation_time::client::wp_presentation_feedback::{
    self, WpPresentationFeedback,
};
use wayland_protocols::wp::viewporter::client::wp_viewport::WpViewport;
use wayland_protocols::xdg::shell::client::xdg_surface::XdgSurface;

/// The proxies a window's commits go through, so that whoever presents needs nothing of the host's state.
pub(crate) struct Commit {
    pub(crate) surface: WlSurface,
    /// A top-level's or popup's (acks, window geometry); a popup made again gets a new one.
    pub(crate) xdg: Option<XdgSurface>,
    /// A child's (its position, and its stacking in its parent's record).
    pub(crate) subsurface: Option<WlSubsurface>,
    pub(crate) viewport: Option<WpViewport>,
    /// Made at the window's first `set_alpha`.
    pub(crate) alpha: Option<WpAlphaModifierSurfaceV1>,
    /// The surface's scale now, times 120 (the viewport destination and the buffer scale follow it).
    pub(crate) scale120: u32,
}

/// Double-buffered state the host asked for, waiting for the commit that carries it.
#[derive(Default)]
pub(crate) struct Recorded {
    /// A child's offset in its parent.  It applies with the parent's commit (Smithay's: the child's own, so a move
    /// makes the child owe one too); it goes right before the child's.
    pub(crate) position: Option<(i32, i32)>,
    /// This surface's children's stacking, in the order asked (child, sibling, above): it applies with this surface's
    /// commit.
    pub(crate) stack: Vec<(WlSubsurface, WlSurface, bool)>,
    /// The input region (logical pixels); empty: the whole surface.
    pub(crate) input: Option<Vec<Rect>>,
    /// Show nothing: the commit attaches no buffer (a commit with one maps the surface again instead).
    pub(crate) unmap: bool,
    pub(crate) alpha: Option<u32>,
    /// A top-level's window geometry: it goes only with a buffer (present path §3.1).
    pub(crate) geometry: Option<Rect>,
    /// A waiting pacer asks again: a frame callback and an empty commit, unless the tree's commit is paced anyway.
    pub(crate) rearm: bool,
}

/// The commits of registered trees, for the test hook `commits_off_presenter`.
#[derive(Default)]
pub struct TreeCommits {
    /// `present_tree` is running.
    in_tree: bool,
    /// Commits made outside it.
    off: u64,
}

impl TreeCommits {
    fn count(&mut self, n: u64) {
        if !self.in_tree {
            self.off += n;
        }
    }
}

/// A window's presenter state, shared with the reader thread's handlers.
pub struct Present {
    window: u64,
    pool: Pool,
    bufs: HashMap<usize, ShmBuf>,
    /// The slot `present_begin` handed out and nothing committed yet.
    begun: Option<BufferSlot>,
    /// The pacer waits for the frame callback of its last commit with content.
    waiting: bool,
    /// A `present_begin` found every buffer busy: the next release makes the pacer ready again.
    starved: bool,
    /// The host drew something since its last frame.
    dirty: bool,
    /// The newest configure serial the host acked, recorded until the commit that carries it.
    ack: Option<u32>,
    /// The recorded ack changes what the window shows (a new size, first configure, a CSD state change): only a
    /// buffer carries it.  Sticky until a buffer does.
    ack_content: bool,
    /// Surface state (a child's position or stacking) that waits for a commit.
    owed: bool,
    /// A child's tree root: the root paces the tree, so a child posts nothing itself (`None` for a root).
    root: Option<Arc<Mutex<Present>>>,
    /// A root: a child committed a buffer since the root's last commit, so the root's next commit is paced (frame
    /// callback and feedback) even without a buffer of its own.
    tree_content: bool,
    /// The root's first configure came (children are configured at creation).
    configured: bool,
    /// `FRAME_READY` was posted and the host has not asked `window_ready` since.
    notified: bool,
    /// The compositor says the window is not shown (xdg_toplevel `suspended`).
    suspended: bool,
    shared: Option<Arc<Shared>>,
    /// Presentation feedback so far (statistics only).
    presented: u32,
    discarded: u32,
    /// Buffers freed by trimming or a resize.
    freed: usize,
    /// The mailbox's sequence number when the ack was recorded: a content ack goes with a packet published after it.
    ack_seq: u64,
    /// A root: the newest packet published for its tree.
    published: u64,
    pub(crate) commit: Commit,
    pub(crate) rec: Recorded,
    /// The window was destroyed (under the tree's mutex): nothing commits it any more.
    retired: bool,
}

/// The user data of a `wl_buffer`: its window's presenter and its slot.
pub type BufData = (Arc<Mutex<Present>>, usize);

pub(crate) fn lock(p: &Mutex<Present>) -> MutexGuard<'_, Present> {
    p.lock().unwrap_or_else(|e| e.into_inner())
}

fn now_ns() -> u64 {
    static BASE: OnceLock<Instant> = OnceLock::new();
    BASE.get_or_init(Instant::now).elapsed().as_nanos() as u64
}

impl Present {
    fn new(
        window: u64,
        commit: Commit,
        shared: Option<Arc<Shared>>,
        root: Option<Arc<Mutex<Present>>>,
    ) -> Present {
        Present {
            commit,
            rec: Recorded::default(),
            retired: false,
            window,
            pool: Pool::new(),
            bufs: HashMap::new(),
            begun: None,
            waiting: false,
            starved: false,
            dirty: false,
            ack: None,
            ack_content: false,
            owed: false,
            // a child needs no configure
            configured: root.is_some(),
            root,
            tree_content: false,
            suspended: false,
            notified: false,
            shared,
            presented: 0,
            discarded: 0,
            freed: 0,
            ack_seq: 0,
            published: 0,
        }
    }

    /// What the host may do now: `CONTENT` (draw and present a buffer: drawn content when idle and shown, or a content
    /// ack, which never waits), with `ACK` when a content ack is what waits, and `OWED` (a commit is owed: a bare ack
    /// or surface state, starved or not).
    fn ready(&self) -> u32 {
        self.ready_with(self.ack_content)
    }

    /// The recorded content ack waits for a packet published after it: until then the tree's picture is not drawn
    /// for that configure (the C gate's `!new_size_p`, moved to the publish).
    fn ack_held(&self) -> bool {
        self.ack_content && self.published <= self.ack_seq
    }

    /// `ready` for a published tree: a content ack counts once a packet came after it, and while it waits no content
    /// goes (the configure's picture is not published yet); owed state may still go bare.
    fn ready_published(&self) -> u32 {
        let held = self.ack_held();
        let bits = self.ready_with(self.ack_content && !held);
        if held {
            bits & NHSHELL_READY_OWED
        } else {
            bits
        }
    }

    /// The ready bits, with `content_ack` saying whether a recorded content ack may go now.
    fn ready_with(&self, content_ack: bool) -> u32 {
        let open = self.configured && !self.starved;
        let content_ack = open && content_ack;
        let content = content_ack || (open && self.dirty && !self.waiting && !self.suspended);
        let owed = self.configured
            && (self.owed || self.rec.rearm || (self.ack.is_some() && !self.ack_content));
        u32::from(content) * NHSHELL_READY_CONTENT
            + u32::from(content_ack) * NHSHELL_READY_ACK
            + u32::from(owed) * NHSHELL_READY_OWED
    }

    /// Post `FRAME_READY` when the pacer is ready, once until the host asks `window_ready`.
    fn notify(&mut self) {
        if let (true, false, Some(shared)) = (self.ready() != 0, self.notified, &self.shared) {
            self.notified = true;
            shared.post(NhShellEvent::new(NHSHELL_EV_FRAME_READY, self.window, 0));
            shared.wake.signal();
        }
    }

    /// Drop the buffers the pool let go of.
    fn reap(&mut self, now: u64) {
        for s in self.pool.trim(now) {
            if self.bufs.remove(&s.id).is_some() {
                self.freed += 1;
            }
        }
    }

    /// Destroy every buffer (the buffers hold this state through their user data).
    pub fn clear(&mut self) {
        self.bufs.clear();
        self.begun = None;
    }

    /// Send the recorded state this surface's next commit carries, right before it.  A commit with a `buffer` also
    /// takes the window geometry, and maps the surface instead of unmapping it; a bare one leaves the geometry
    /// recorded.  Objects destroyed since (a stacked child, its sibling) are skipped.
    fn send_recorded(
        &mut self,
        buffer: bool,
        compositor: Option<&WlCompositor>,
        rqh: &QueueHandle<ReaderState>,
    ) {
        let (c, r) = (&self.commit, &mut self.rec);
        for (sub, sibling, above) in r.stack.drain(..) {
            if sub.is_alive() && sibling.is_alive() {
                if above {
                    sub.place_above(&sibling);
                } else {
                    sub.place_below(&sibling);
                }
            }
        }
        if let (Some((x, y)), Some(sub)) = (r.position.take(), &c.subsurface) {
            sub.set_position(x, y);
        }
        if let Some(rects) = r.input.take() {
            if rects.is_empty() {
                c.surface.set_input_region(None);
            } else if let Some(compositor) = compositor {
                let region = compositor.create_region(rqh, ());
                for r in &rects {
                    region.add(r.x, r.y, r.w, r.h);
                }
                c.surface.set_input_region(Some(&region));
                region.destroy();
            }
        }
        if let (Some(factor), Some(alpha)) = (r.alpha.take(), &c.alpha) {
            alpha.set_multiplier(factor);
        }
        if std::mem::take(&mut r.unmap) && !buffer {
            c.surface.attach(None, 0, 0);
        }
        if buffer
            && let Some(g) = r.geometry.take()
            && let Some(xdg) = &c.xdg
        {
            xdg.set_window_geometry(g.x, g.y, g.w, g.h);
        }
    }

    /// A root's re-arm, after a commit that asked for no frame callback: a callback and an empty commit, carrying
    /// nothing else.  `true` when sent.
    fn send_rearm(&mut self, me: &Arc<Mutex<Present>>, rqh: &QueueHandle<ReaderState>) -> bool {
        if !std::mem::take(&mut self.rec.rearm) || !self.waiting || self.root.is_some() {
            return false;
        }
        self.commit.surface.frame(rqh, me.clone());
        self.commit.surface.commit();
        true
    }

    /// The popup was made again on the same surface with `xdg`: the recorded ack belongs to the destroyed
    /// `xdg_surface`, and the new one has had no configure.
    pub(crate) fn reset(&mut self, xdg: XdgSurface) {
        self.commit.xdg = Some(xdg);
        self.ack = None;
        self.ack_content = false;
        self.configured = false;
    }
}

impl MainState {
    /// Window `id`'s role is set up (`xdg` for a top-level or popup, `subsurface` for a child): its presenter state
    /// takes the commit-time proxies, and from now on only presenting commits it.
    pub(crate) fn present_register(
        &mut self,
        id: u64,
        xdg: Option<XdgSurface>,
        subsurface: Option<WlSubsurface>,
    ) {
        let Some(surface) = self.surfaces.get(&id).cloned() else {
            return;
        };
        let (viewport, scale120) = self
            .scales
            .get(&id)
            .map_or((None, SCALE_ONE), |s| (s.viewport(), s.scale120()));
        let commit = Commit {
            surface,
            xdg,
            subsurface,
            viewport,
            alpha: None,
            scale120,
        };
        // a child (a sync subsurface) is paced by its tree's root, and posts nothing itself
        let p = if self.children.contains_key(&id) {
            let Some(root) = self.presents.get(&self.tree_root(id)).cloned() else {
                return;
            };
            Present::new(id, commit, None, Some(root))
        } else {
            Present::new(id, commit, self.shared.clone(), None)
        };
        self.presents.insert(id, Arc::new(Mutex::new(p)));
    }

    fn present_of(&self, id: u64) -> Option<Arc<Mutex<Present>>> {
        self.presents.get(&id).cloned()
    }

    /// Record `f` on window `id`'s presenter state (nothing when `id` has none).
    pub(crate) fn present_record(&self, id: u64, f: impl FnOnce(&mut Present)) {
        if let Some(p) = self.presents.get(&id) {
            f(&mut lock(p));
        }
    }

    /// The root of window `id`'s tree: the nearest ancestor (or `id` itself) that is not a child.
    pub(crate) fn tree_root(&self, mut id: u64) -> u64 {
        while let Some(c) = self.children.get(&id) {
            id = c.parent;
        }
        id
    }

    /// Child `id`'s state waits for commits (B3: sync subsurfaces): `id`'s own when `own`, and every ancestor's up to
    /// the root, since a sync child's commit applies only with its parent's.  With `wake`, the root's pacer wakes
    /// (a root's own state too: its alpha).
    pub(crate) fn present_owe(&mut self, id: u64, own: bool, wake: bool) {
        let present = if own { self.present_of(id) } else { None };
        if let Some(present) = present {
            let mut p = lock(&present);
            p.owed = true;
            if wake && !self.children.contains_key(&id) {
                p.notify();
            }
        }
        let mut cur = id;
        while let Some(parent) = self.children.get(&cur).map(|c| c.parent) {
            cur = parent;
            let Some(present) = self.present_of(cur) else {
                continue;
            };
            let mut p = lock(&present);
            p.owed = true;
            if wake && !self.children.contains_key(&cur) {
                p.notify();
            }
        }
    }

    /// Record `serial`'s ack for window `id` (sent with the next commit) and wake the pacer.
    pub(crate) fn present_ack(&mut self, id: u64, serial: u32, content: bool) {
        let Some(present) = self.present_of(id) else {
            return;
        };
        let mut p = lock(&present);
        p.configured = true;
        p.ack = Some(serial);
        p.ack_content |= content;
        p.ack_seq = self.mailbox.seq();
        p.notify();
    }

    /// Root `root`'s recorded content ack waits for a packet published after it.
    pub(crate) fn present_wants_packet(&mut self, root: u64) -> bool {
        self.present_of(root).is_some_and(|p| lock(&p).ack_held())
    }

    /// Packet `seq` of root `root`'s tree is published (`damaged`: it shows something new): the pacer wakes.
    pub(crate) fn present_published(&mut self, root: u64, seq: u64, damaged: bool) {
        let Some(present) = self.present_of(root) else {
            return;
        };
        let mut p = lock(&present);
        p.published = seq;
        p.dirty |= damaged;
        p.notify();
    }

    /// The compositor configured window `id`: note whether it is suspended, and re-arm a waiting pacer.
    pub(crate) fn present_configured(&mut self, id: u64, suspended: bool) {
        if !self.windows.contains_key(&id) {
            return;
        }
        let Some(present) = self.present_of(id) else {
            return;
        };
        let mut p = lock(&present);
        p.configured = true;
        let was = std::mem::replace(&mut p.suspended, suspended);
        drop(p);
        self.present_rearm(id);
        if was && !suspended {
            lock(&present).notify();
        }
    }

    /// A waiting pacer asks again (a frame callback and an empty commit, sent with its tree's next commit): the
    /// surface may have become visible, and the callback of the commit that waits may never have been answered.
    pub(crate) fn present_rearm(&mut self, id: u64) {
        if let Some(present) = self.presents.get(&id) {
            let mut p = lock(present);
            if p.waiting {
                p.rec.rearm = true;
                p.notify();
            }
        }
    }

    /// Window `id`'s scale changed: its next buffer's viewport and buffer scale follow.
    pub(crate) fn present_scale(&self, id: u64, scale120: u32) {
        self.present_record(id, |p| p.commit.scale120 = scale120);
    }
}

impl WaylandShell {
    /// The next frame's buffer for window `id`, `dev_w` x `dev_h` device pixels.
    pub(super) fn present_begin_impl(
        &mut self,
        id: WindowId,
        dev_w: i32,
        dev_h: i32,
    ) -> ShellResult<NhShellBuffer> {
        if !self.main.is_window(id) || dev_w <= 0 || dev_h <= 0 {
            return Err(ShellError::State);
        }
        let present = self.main.present_of(id).ok_or(ShellError::State)?;
        let mut p = lock(&present);
        if !p.configured {
            return Err(ShellError::State);
        }
        let now = now_ns();
        p.reap(now);
        let slot = match p.begun.take() {
            // begun twice without a commit: the same buffer
            Some(s) if (s.w, s.h) == (dev_w, dev_h) => s,
            b => {
                if let Some(s) = b {
                    p.pool.abandon(s);
                }
                let mut s = p.pool.begin(dev_w, dev_h, now);
                if s.is_none() && (!p.waiting || p.ack_content) {
                    // idle (or carrying a content ack) with every buffer busy: the compositor holds them, so the
                    // pool grows
                    s = p.pool.begin_with(dev_w, dev_h, now, GROWN_BUFFERS);
                }
                let Some(s) = s else {
                    // the draw is not taken: the release that ends starvation brings it
                    p.starved = true;
                    p.dirty = true;
                    return Err(ShellError::State);
                };
                s
            }
        };
        p.reap(now);
        if !p.bufs.contains_key(&slot.id) {
            let (shm, rqh) = (
                self.main.shm.as_ref().ok_or(ShellError::State)?,
                self.main.reader_qh.as_ref().ok_or(ShellError::State)?,
            );
            let made = ShmBuf::create(
                shm,
                &self.main_queue.handle(),
                rqh,
                dev_w,
                dev_h,
                (present.clone(), slot.id),
            );
            let Ok(buf) = made else {
                p.pool.abandon(slot);
                return Err(ShellError::NoMem);
            };
            p.bufs.insert(slot.id, buf);
        }
        p.begun = Some(slot);
        // what the host drew so far goes into this frame
        p.dirty = false;
        let b = &p.bufs[&slot.id];
        Ok(NhShellBuffer {
            pixels: b.pixels(),
            w: b.w,
            h: b.h,
            stride: b.stride,
            age: slot.age,
        })
    }

    /// Show the begun buffer of window `id`; `damage` is in device pixels.
    pub(super) fn present_commit_impl(&mut self, id: WindowId, damage: &[Rect]) -> ShellResult {
        self.commit_buffer(id, damage, false)
    }

    /// `present_commit`; for a `published` tree a content ack that still waits for its packet stays recorded.  The
    /// surface's recorded state goes right before the commit.
    fn commit_buffer(&mut self, id: WindowId, damage: &[Rect], published: bool) -> ShellResult {
        let present = self.main.present_of(id).ok_or(ShellError::State)?;
        let rqh = self.main.reader_qh.clone().ok_or(ShellError::State)?;
        let mut guard = lock(&present);
        let p = &mut *guard;
        if p.retired {
            return Err(ShellError::State);
        }
        let slot = p.begun.take().ok_or(ShellError::State)?;
        // a buffer that was never written has no age: the whole of it is new
        let all = match p.bufs.get(&slot.id) {
            Some(b) if slot.age == 0 => vec![Rect::new(0, 0, b.w, b.h)],
            _ => merged(damage),
        };
        let send_ack = !(published && p.ack_held());
        let ack = if send_ack { p.ack.take() } else { None };
        // the ack goes before anything the commit carries (a checker, and a compositor, read the order)
        if let Some(serial) = ack
            && let Some(xdg) = &p.commit.xdg
        {
            xdg.ack_configure(serial);
        }
        p.send_recorded(true, self.main.compositor.as_ref(), &rqh);
        let buf = p.bufs.get(&slot.id).ok_or(ShellError::State)?;
        let (surface, scale) = (&p.commit.surface, p.commit.scale120);
        if let Some(viewport) = &p.commit.viewport {
            let s = i64::from(scale.max(1));
            let logical = |d: i32| ((i64::from(d) * 120 + s / 2) / s) as i32;
            viewport.set_destination(logical(buf.w), logical(buf.h));
            surface.set_buffer_scale(1);
        } else {
            surface.set_buffer_scale(((scale + 60) / 120).max(1) as i32);
        }
        surface.attach(Some(&buf.buffer), 0, 0);
        for r in &all {
            surface.damage_buffer(r.x, r.y, r.w, r.h);
        }
        // only a root's commit with content (its own, or a child's it applies) is paced: the frame callback releases
        // the pacer, feedback is statistics
        let child = p.root.is_some();
        let paced = !child && (!all.is_empty() || p.tree_content);
        if paced {
            surface.frame(&rqh, present.clone());
            if let Some(pres) = &self.main.presentation {
                pres.feedback(surface, &rqh, present.clone());
            }
            p.waiting = true;
            p.rec.rearm = false;
        }
        if send_ack {
            p.ack_content = false;
        }
        p.owed = false;
        p.tree_content = false;
        p.commit.surface.commit();
        self.tree_commits.count(1);
        if p.send_rearm(&present, &rqh) {
            self.tree_commits.count(1);
        }
        p.pool.submitted(slot);
        let root = p.root.clone();
        drop(guard);
        if let Some(root) = root {
            // a sync child's buffer shows when every ancestor has committed after it, the root last
            lock(&root).tree_content = true;
            self.main.present_owe(id, false, false);
        }
        self.flush()
    }

    /// The host has drawn into window `id`: `FRAME_READY` comes when the pacer lets it present.  A child's drawing
    /// wakes its tree's root, which presents the tree.
    pub(super) fn window_dirty_impl(&mut self, id: WindowId) -> ShellResult {
        if !self.main.is_window(id) {
            return Err(ShellError::State);
        }
        let root = self.main.tree_root(id);
        let present = self.main.present_of(root).ok_or(ShellError::State)?;
        let mut p = lock(&present);
        p.dirty = true;
        p.notify();
        Ok(())
    }

    /// A commit with no buffer, viewport or scale: a bare ack goes with it (a content ack waits for a buffer), and the
    /// recorded surface state.  No frame callback or feedback either, unless it is a root's commit that applies a
    /// child's buffer: that commit shows content, and paces the tree.  A root's re-arm goes after it, as a commit of
    /// its own, or is the only commit when nothing else is owed.  A child's bare commit makes its ancestors owe one.
    pub(super) fn present_bare_impl(&mut self, id: WindowId) -> ShellResult {
        let present = self.main.present_of(id).ok_or(ShellError::State)?;
        let rqh = self.main.reader_qh.clone().ok_or(ShellError::State)?;
        let mut p = lock(&present);
        if p.retired {
            return Err(ShellError::State);
        }
        let child = p.root.is_some();
        let bare_ack = p.ack.is_some() && !p.ack_content;
        if !child && p.rec.rearm && p.waiting && !(p.owed || bare_ack || p.tree_content) {
            p.send_rearm(&present, &rqh);
            self.tree_commits.count(1);
            drop(p);
            return self.flush();
        }
        if bare_ack
            && let Some(serial) = p.ack.take()
            && let Some(xdg) = &p.commit.xdg
        {
            xdg.ack_configure(serial);
        }
        p.send_recorded(false, self.main.compositor.as_ref(), &rqh);
        if std::mem::take(&mut p.tree_content) {
            let surface = &p.commit.surface;
            surface.frame(&rqh, present.clone());
            if let Some(pres) = &self.main.presentation {
                pres.feedback(surface, &rqh, present.clone());
            }
            p.waiting = true;
            p.rec.rearm = false;
        }
        p.owed = false;
        p.commit.surface.commit();
        self.tree_commits.count(1);
        if p.send_rearm(&present, &rqh) {
            self.tree_commits.count(1);
        }
        drop(p);
        if child {
            self.main.present_owe(id, false, false);
        }
        self.flush()
    }

    /// The host took a `FRAME_READY` record: the pacer may post the next one.
    pub(super) fn frame_ready_taken(&mut self, ev: &NhShellEvent) {
        if ev.kind == NHSHELL_EV_FRAME_READY
            && let Some(p) = self.main.presents.get(&ev.window)
        {
            lock(p).notified = false;
        }
    }

    /// Whether the host may draw and present window `id` now.  A child is presented with its tree: it reports only
    /// `OWED`, its own surface owes a commit (before its parent's).
    pub(super) fn window_ready_impl(&mut self, id: WindowId) -> ShellResult<u32> {
        if !self.main.is_window(id) {
            return Err(ShellError::State);
        }
        Ok(self.main.presents.get(&id).map_or(0, |p| {
            let mut p = lock(p);
            if p.root.is_some() {
                return u32::from(p.owed) * NHSHELL_READY_OWED;
            }
            p.notified = false;
            let bits = p.ready();
            if bits & NHSHELL_READY_CONTENT != 0 {
                p.dirty = false;
            }
            bits
        }))
    }

    /// Present root `root`'s tree by the present path's rule, at its `FRAME_READY` (inline mode).  Buffers when a
    /// published packet left damage on a drawable surface or an eligible content ack waits, and the gate is open
    /// (configured, a buffer free, and idle and shown unless the ack waives that): the child windows first, deepest
    /// first, then the root, which composes its whole picture when the ack came with nothing drawn.  Otherwise bare
    /// commits where one is owed (a bare ack, or surface state), deepest first, the root last.  Otherwise nothing.  A
    /// content ack never rides a bare commit.  `true` when the root committed a buffer.
    pub(super) fn present_tree_impl(&mut self, root: WindowId) -> ShellResult<bool> {
        if self.presenting.thread_on {
            return Err(ShellError::State);
        }
        if !self.main.is_window(root) || self.main.children.contains_key(&root) {
            return Err(ShellError::State);
        }
        self.tree_commits.in_tree = true;
        let shown = self.present_tree_pass(root);
        self.tree_commits.in_tree = false;
        shown
    }

    /// `present_tree`'s decision and commits.
    fn present_tree_pass(&mut self, root: WindowId) -> ShellResult<bool> {
        let mail = self.main.mailbox.clone();
        self.presenting.stage.drain(&mail);
        let Some(present) = self.main.presents.get(&root).cloned() else {
            return Ok(false);
        };
        let bits = {
            let mut p = lock(&present);
            p.notified = false;
            let bits = p.ready_published();
            // reporting content takes the draw
            if bits & NHSHELL_READY_CONTENT != 0 {
                p.dirty = false;
            }
            bits
        };
        if bits == 0 {
            return Ok(false);
        }
        if bits & NHSHELL_READY_ACK != 0 {
            self.presenting.stage.damage_all_if_clean(root);
        }
        let content = bits & NHSHELL_READY_CONTENT != 0 && self.tree_damaged(root);
        self.present_children(root, content);
        let shown = content && self.present_surface(root);
        if !shown && self.owes(root) {
            self.present_bare_counted(root);
        }
        Ok(shown)
    }

    /// The child windows of `id`, in a stable order.
    fn kids(&self, id: WindowId) -> Vec<WindowId> {
        let mut v: Vec<WindowId> = self
            .main
            .children
            .iter()
            .filter(|(_, c)| c.parent == id)
            .map(|(&k, _)| k)
            .collect();
        v.sort_unstable();
        v
    }

    /// `id` or a drawable child window under it has damage waiting.
    fn tree_damaged(&self, id: WindowId) -> bool {
        let stage = &self.presenting.stage;
        stage.damaged(id)
            || self
                .kids(id)
                .into_iter()
                .any(|c| stage.drawable(c) && self.tree_damaged(c))
    }

    /// Commit the child windows under `id`, deepest first (a sync child shows once every ancestor committed after
    /// it): each with a buffer when `content` and it is drawable and damaged, else bare when it owes a commit.
    fn present_children(&mut self, id: WindowId, content: bool) {
        for c in self.kids(id) {
            let draw = content && self.presenting.stage.drawable(c);
            self.present_children(c, draw);
            if !(draw && self.present_surface(c)) && self.owes(c) {
                self.present_bare_counted(c);
            }
        }
    }

    /// Show surface `id`'s waiting damage in a buffer composed from its front; `true` when committed.  A begin that
    /// finds every buffer busy starves the tree (the release brings the draw back); the damage keeps waiting.
    fn present_surface(&mut self, id: WindowId) -> bool {
        let Some((w, h, damage)) = self.presenting.stage.next_frame(id) else {
            return false;
        };
        let Ok(buf) = self.present_begin_impl(id, w, h) else {
            return false;
        };
        self.presenting.stage.compose(id, &buf);
        if self.commit_buffer(id, &damage, true).is_err() {
            let _ = self.window_dirty_impl(id);
            return false;
        }
        self.presenting.stage.presented(id);
        true
    }

    /// Window `id` owes a commit: a root's owed state or bare ack, a child's own surface state.
    fn owes(&self, id: WindowId) -> bool {
        self.main.presents.get(&id).is_some_and(|p| {
            let p = lock(p);
            if p.root.is_some() {
                p.owed
            } else {
                p.ready() & NHSHELL_READY_OWED != 0
            }
        })
    }

    fn present_bare_counted(&mut self, id: WindowId) {
        if self.present_bare_impl(id).is_ok() {
            self.presenting.stage.stats.bare_commits += 1;
        }
    }

    /// Present on a thread of the shell's (`on`) or inline (the host calls `present_tree`).  The mode is stored here;
    /// until the thread exists, publishing applies its packets on the host's thread either way.
    pub(super) fn set_presenter_thread_impl(&mut self, on: bool) -> ShellResult {
        self.presenting.thread_on = on;
        Ok(())
    }

    /// Test hook: (waiting for a frame callback, starved, feedback presented, feedback discarded) of window `id`.
    #[doc(hidden)]
    pub fn present_pacer(&self, id: WindowId) -> Option<(bool, bool, u32, u32)> {
        let p = lock(self.main.presents.get(&id)?);
        Some((p.waiting, p.starved, p.presented, p.discarded))
    }

    /// Housekeeping on the host's thread: free the buffers that sat idle past the pool's quiet period.
    pub(super) fn present_housekeeping(&mut self) {
        let now = now_ns();
        for p in self.main.presents.values() {
            lock(p).reap(now);
        }
        self.publish_trim(crate::wayland::publish::now_ns());
    }

    /// Window `id` goes: `destroy` runs holding its tree's mutex (the root's), so whoever presents the tree is never
    /// between its decision and its commit meanwhile, and finds `id` retired after.  Its alpha object goes first, its
    /// buffers and its front with it.
    pub(super) fn present_retire(&mut self, id: WindowId, destroy: impl FnOnce(&mut MainState)) {
        let root = self.main.tree_root(id);
        let tree = self.main.present_of(root);
        let own = self.main.presents.remove(&id);
        {
            let mut tree = tree.as_ref().map(|t| lock(t));
            let mut own = own.as_ref().filter(|_| root != id).map(|p| lock(p));
            let p = if root == id {
                tree.as_deref_mut()
            } else {
                own.as_deref_mut()
            };
            if let Some(p) = p {
                p.retired = true;
                if let Some(a) = p.commit.alpha.take() {
                    a.destroy();
                }
                p.clear();
            }
            destroy(&mut self.main);
        }
        self.publish_forget(id);
    }

    /// Test hook: commits of registered trees made outside `present_tree` (by `present_commit`, `present_bare`, or
    /// anything that sends on its own).
    #[doc(hidden)]
    pub fn commits_off_presenter(&self) -> u64 {
        self.tree_commits.off
    }

    /// Test hook: hold window `id`'s tree mutex on another thread for `ms`, as a presenter between its decision and
    /// its commit does; returns once it is held, with the thread to join.
    #[doc(hidden)]
    pub fn hold_tree(&self, id: WindowId, ms: u64) -> Option<std::thread::JoinHandle<()>> {
        let tree = self.main.present_of(self.main.tree_root(id))?;
        let (tx, rx) = std::sync::mpsc::channel();
        let held = std::thread::spawn(move || {
            let _held = lock(&tree);
            let _ = tx.send(());
            std::thread::sleep(Duration::from_millis(ms));
        });
        let _ = rx.recv();
        Some(held)
    }

    /// Test hook: (live buffers, buffers freed so far, the size of the newest buffer's memfd) of window `id`.
    #[doc(hidden)]
    pub fn present_stats(&self, id: WindowId) -> Option<(usize, usize, usize)> {
        let p = lock(self.main.presents.get(&id)?);
        Some((p.bufs.len(), p.freed, p.bufs.values().map(|b| b.len).max()?))
    }

    /// The buffer size for a logical size at window `id`'s current scale.
    pub fn present_buffer_size(&self, id: WindowId, logical_w: i32, logical_h: i32) -> (i32, i32) {
        let s = self
            .main
            .scales
            .get(&id)
            .map_or(SCALE_ONE, |s| s.scale120());
        buffer_size(logical_w, logical_h, s)
    }
}

fn frame_done(state: &mut ReaderState, window: u64, time_ns: u64, presented_ns: u64, refresh: u32) {
    let mut ev = NhShellEvent::new(NHSHELL_EV_FRAME_DONE, window, time_ns);
    ev.u.frame_done = FrameDoneEv {
        presented_ns,
        refresh_ns: refresh,
    };
    state.post(ev);
}

/// A frame callback: the commit it was requested with has been shown (or is the compositor's to hold no longer).
impl Dispatch<WlCallback, Arc<Mutex<Present>>> for ReaderState {
    fn event(
        _: &mut Self,
        _: &WlCallback,
        event: wl_callback::Event,
        present: &Arc<Mutex<Present>>,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_callback::Event::Done { .. } = event {
            let mut p = lock(present);
            p.waiting = false;
            // nothing waits any more: a re-arm not sent yet has no callback to replace
            p.rec.rearm = false;
            // starved while waiting: the pacer is idle now, and an idle pacer grows the pool
            p.starved = false;
            p.notify();
        }
    }
}

/// Presentation feedback is statistics: `FRAME_DONE` says a frame was shown, and nothing waits for it.
impl Dispatch<WpPresentationFeedback, Arc<Mutex<Present>>> for ReaderState {
    fn event(
        state: &mut Self,
        _: &WpPresentationFeedback,
        event: wp_presentation_feedback::Event,
        present: &Arc<Mutex<Present>>,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let mut p = lock(present);
        match event {
            wp_presentation_feedback::Event::Presented {
                tv_sec_hi,
                tv_sec_lo,
                tv_nsec,
                refresh,
                ..
            } => {
                p.presented += 1;
                let secs = (u64::from(tv_sec_hi) << 32) | u64::from(tv_sec_lo);
                let t = secs * 1_000_000_000 + u64::from(tv_nsec);
                let window = p.window;
                drop(p);
                frame_done(state, window, t, t, refresh);
            }
            wp_presentation_feedback::Event::Discarded => p.discarded += 1,
            _ => {}
        }
    }
}

impl Dispatch<WlBuffer, BufData> for ReaderState {
    fn event(
        _: &mut Self,
        _: &WlBuffer,
        event: wl_buffer::Event,
        (present, id): &BufData,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_buffer::Event::Release = event {
            let mut p = lock(present);
            if let Some(b) = p.bufs.get(id) {
                let slot = BufferSlot {
                    id: *id,
                    age: 0,
                    w: b.w,
                    h: b.h,
                };
                p.pool.released(slot);
            }
            // a starved pacer leaves its state on a release; a starved child's draw waits in the host, so its root
            // presents the tree again
            if std::mem::take(&mut p.starved) {
                match p.root.clone() {
                    Some(root) => {
                        drop(p);
                        let mut r = lock(&root);
                        r.dirty = true;
                        r.notify();
                    }
                    None => p.notify(),
                }
            }
        }
    }
}

delegate_noop!(MainState: WlShmPool);
delegate_noop!(MainState: ignore WpPresentation);
// input regions made where the recorded state is sent
delegate_noop!(ReaderState: WlRegion);

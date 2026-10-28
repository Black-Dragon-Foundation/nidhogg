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
//! the root's `FRAME_READY` (inline mode) or by the presenter thread (`presenter.rs`) when a pacer becomes ready.  For
//! it a content ack counts only once a packet published after the ack is in the fronts.
//!
//! **The tree owns its commit-time state.**  Once a window's role is set up (`present_register`), every request
//! whose effect waits for `wl_surface.commit` is recorded on its `Present` (position, stacking, input region, unmap,
//! alpha, window geometry, the re-arm) and sent right before the commit it belongs to, through the proxies in its
//! `Commit` record; only presenting commits a registered tree.  The host's thread keeps creating objects, setting
//! roles up, immediate requests and destroying, which happens under the tree's mutex (the root's `Present`) and
//! retires the window.  The root's `Present` also lists the tree's child windows, so that presenting needs nothing of
//! the host's state; the lock order is the root's mutex, then a child's.  It holds the root's content layers too
//! (`layers.rs`), committed with the descendants, before the root.

use crate::api::*;
use crate::event::*;
use crate::wayland::conn::Shared;
use crate::wayland::layers::{ContentLayer, LayerJob};
use crate::wayland::presenter::{Counters, DEFAULT_REFRESH_NS, Link, Wire};
use crate::wayland::publish::Stage;
use crate::wayland::reader::ReaderState;
use crate::wayland::scale::{SCALE_ONE, buffer_size};
use crate::wayland::shm::ShmBuf;
use crate::wayland::{MainState, WaylandShell};
use glitnir::geom::Rect;
use glitnir::pool::{BufferSlot, GROWN_BUFFERS, Pool, merged};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, TryLockError};
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

/// The newest of the recorded state that went out: what a surface made again after a reconnect sends again.
#[derive(Default)]
pub(crate) struct Sent {
    input: Option<Vec<Rect>>,
    alpha: Option<u32>,
    geometry: Option<Rect>,
}

/// Commits of registered trees made outside `present_tree`, for the test hook `commits_off_presenter`.
#[derive(Default)]
pub struct TreeCommits {
    off: u64,
}

/// A child window of a tree, as its root lists it.
#[derive(Clone)]
pub(crate) struct Kid {
    pub(crate) id: u64,
    pub(crate) parent: u64,
    pub(crate) present: Arc<Mutex<Present>>,
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
    /// The display's presenter link: thread mode, and the host's waits for this mutex.
    link: Option<Arc<Link>>,
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
    sent: Sent,
    /// The window was made again after a reconnect: its next configure is a content ack, whatever the host's flag.
    content_next: bool,
    /// The window was destroyed (under the tree's mutex): nothing commits it any more.
    retired: bool,
    /// A root: its child windows, each with its parent.
    kids: Vec<Kid>,
    /// Bumped when the role objects are made again (a popup's): a buffer composed before is not committed after.
    epoch: u64,
    /// A root: when its gate last opened (a frame callback, the end of starvation), `CLOCK_MONOTONIC` ns.
    t_open_ns: u64,
    /// The refresh period presentation feedback reported (0: none yet).
    refresh_ns: u32,
    /// A root: its windows that are their own outputs, each on a content layer (`layers.rs`).
    pub(crate) layers: Vec<ContentLayer>,
}

/// The user data of a `wl_buffer`: its window's presenter and its slot.
pub type BufData = (Arc<Mutex<Present>>, usize);

/// Lock a presenter state.  A host thread's wait for it counts towards `main_wait_max_ns`.
pub(crate) fn lock(p: &Mutex<Present>) -> MutexGuard<'_, Present> {
    match p.try_lock() {
        Ok(g) => g,
        Err(TryLockError::Poisoned(e)) => e.into_inner(),
        Err(TryLockError::WouldBlock) => {
            let t = Instant::now();
            let g = p.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(link) = &g.link {
                link.note_wait(t.elapsed().as_nanos() as u64);
            }
            g
        }
    }
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
        link: Option<Arc<Link>>,
    ) -> Present {
        Present {
            commit,
            rec: Recorded::default(),
            sent: Sent::default(),
            content_next: false,
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
            link,
            presented: 0,
            discarded: 0,
            freed: 0,
            ack_seq: 0,
            published: 0,
            kids: Vec::new(),
            epoch: 0,
            t_open_ns: 0,
            refresh_ns: 0,
            layers: Vec::new(),
        }
    }

    /// What the host may do now: `CONTENT` (draw and present a buffer: drawn content when idle and shown, or a content
    /// ack, which never waits), with `ACK` when a content ack is what waits, and `OWED` (a commit is owed: a bare ack
    /// or surface state, starved or not).
    fn ready(&self) -> u32 {
        self.ready_with(self.ack_content)
    }

    /// The recorded content ack waits for a packet published after it: until then the tree's picture is not drawn
    /// for that configure (the C gate's `!new_size_p`, moved to the publish).  `seq` is the newest packet that counts:
    /// for the host, the newest published; for presenting, the newest in the fronts.
    fn ack_held_at(&self, seq: u64) -> bool {
        self.ack_content && seq <= self.ack_seq
    }

    /// `ready` for a published tree whose newest packet in the fronts is `applied`: a content ack counts once a packet
    /// came after it, and while it waits no content goes (the configure's picture is not there yet); owed state may
    /// still go bare.
    fn ready_published(&self, applied: u64) -> u32 {
        let held = self.ack_held_at(applied);
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

    /// The pacer is ready: in thread mode wake the presenter, else post `FRAME_READY` once until the host asks
    /// `window_ready`.
    fn notify(&mut self) {
        if self.ready() == 0 || self.root.is_some() {
            return;
        }
        if let Some(link) = self.link.as_ref().filter(|l| l.on()) {
            link.wake();
            return;
        }
        if let (false, Some(shared)) = (self.notified, &self.shared) {
            self.notified = true;
            shared.post(NhShellEvent::new(NHSHELL_EV_FRAME_READY, self.window, 0));
            shared.wake.signal();
        }
    }

    /// Presenting turned inline again: a ready root posts `FRAME_READY` afresh.
    pub(crate) fn renotify(&mut self) {
        self.notified = false;
        self.notify();
    }

    /// Drop the buffers the pool let go of.
    fn reap(&mut self, now: u64) {
        for s in self.pool.trim(now) {
            if self.bufs.remove(&s.id).is_some() {
                self.freed += 1;
            }
        }
    }

    /// Destroy every buffer and content layer (the buffers hold this state through their user data).
    pub fn clear(&mut self) {
        self.bufs.clear();
        self.begun = None;
        self.layers.clear();
    }

    /// The connection was made again (`me` is this state's own handle): each buffer gets a new `wl_buffer` from its
    /// own memfd, so what it holds stays, and every slot counts as released, as the old compositor will never say.
    /// The content layers go; presenting promotes them again.
    pub(crate) fn reconnected(&mut self, me: &Arc<Mutex<Present>>, wire: &Wire) {
        self.layers.clear();
        let Some(shm) = &wire.shm else {
            self.clear();
            return;
        };
        for (&id, buf) in &mut self.bufs {
            buf.remake(shm, &wire.mqh, &wire.rqh, (me.clone(), id));
            self.pool.released(BufferSlot {
                id,
                age: 0,
                w: buf.w,
                h: buf.h,
            });
        }
        self.starved = false;
    }

    /// A content layer's buffer came back: a layer starved for buffers presents the tree again.
    pub(crate) fn layer_released(&mut self, layer: u64, slot: usize) {
        let Some(l) = self.layers.iter_mut().find(|l| l.id == layer) else {
            return;
        };
        if l.released(slot) {
            self.dirty = true;
            self.t_open_ns = crate::wayland::publish::now_ns();
            self.notify();
        }
    }

    /// The shell closes: the buffers go (unmapped), and so do the links that a reference cycle through a buffer's or
    /// callback's user data would keep alive (the shared state's descriptors, the root presenter, the tree's
    /// children, the presenter link).
    pub(crate) fn release(&mut self) {
        self.clear();
        self.shared = None;
        self.root = None;
        self.kids.clear();
        self.link = None;
    }

    /// A slot of `w` x `h` for the next frame: a free buffer of the pool, or a new one when the pacer is idle (or
    /// carries a content ack) and every buffer is busy.  `None`: starved (the release that ends it brings the draw
    /// back).
    fn take_slot(&mut self, w: i32, h: i32, now: u64) -> Option<BufferSlot> {
        let mut s = self.pool.begin(w, h, now);
        if s.is_none() && (!self.waiting || self.ack_content) {
            // idle (or carrying a content ack) with every buffer busy: the compositor holds them, so the pool grows
            s = self.pool.begin_with(w, h, now, GROWN_BUFFERS);
        }
        if s.is_none() {
            self.starved = true;
            self.dirty = true;
        }
        s
    }

    /// Make slot `slot`'s buffer if it has none yet.
    fn ensure_buf(
        &mut self,
        me: &Arc<Mutex<Present>>,
        wire: &Wire,
        slot: &BufferSlot,
    ) -> ShellResult {
        if self.bufs.contains_key(&slot.id) {
            return Ok(());
        }
        let shm = wire.shm.as_ref().ok_or(ShellError::State)?;
        match ShmBuf::create(
            shm,
            &wire.mqh,
            &wire.rqh,
            slot.w,
            slot.h,
            (me.clone(), slot.id),
        ) {
            Ok(buf) => {
                self.bufs.insert(slot.id, buf);
                Ok(())
            }
            Err(_) => {
                self.pool.abandon(*slot);
                Err(ShellError::NoMem)
            }
        }
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
        let (c, r, sent) = (&self.commit, &mut self.rec, &mut self.sent);
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
            sent.input = Some(rects);
        }
        if let (Some(factor), Some(alpha)) = (r.alpha.take(), &c.alpha) {
            alpha.set_multiplier(factor);
            sent.alpha = Some(factor);
        }
        if std::mem::take(&mut r.unmap) && !buffer {
            c.surface.attach(None, 0, 0);
        }
        if buffer
            && let Some(g) = r.geometry.take()
            && let Some(xdg) = &c.xdg
        {
            xdg.set_window_geometry(g.x, g.y, g.w, g.h);
            sent.geometry = Some(g);
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

    /// Commit `buf` (pool slot `slot`) with `damage` (device pixels): the recorded ack unless `hold_ack`, the recorded
    /// state, the viewport or buffer scale, attach and damage; for a root a frame callback and feedback when the
    /// commit shows content (its own, or a child's it applies).  A root's re-arm after it.  The commits sent.
    fn commit_buffer(
        &mut self,
        me: &Arc<Mutex<Present>>,
        wire: &Wire,
        slot: BufferSlot,
        buf: &ShmBuf,
        damage: &[Rect],
        hold_ack: bool,
    ) -> u64 {
        // a buffer that was never written has no age: the whole of it is new
        let all = if slot.age == 0 {
            vec![Rect::new(0, 0, buf.w, buf.h)]
        } else {
            merged(damage)
        };
        let ack = if hold_ack { None } else { self.ack.take() };
        // the ack goes before anything the commit carries (a checker, and a compositor, read the order)
        if let Some(serial) = ack
            && let Some(xdg) = &self.commit.xdg
        {
            xdg.ack_configure(serial);
        }
        self.send_recorded(true, wire.compositor.as_ref(), &wire.rqh);
        let (surface, scale) = (&self.commit.surface, self.commit.scale120);
        if let Some(viewport) = &self.commit.viewport {
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
        let paced = self.root.is_none() && (!all.is_empty() || self.tree_content);
        if paced {
            surface.frame(&wire.rqh, me.clone());
            if let Some(pres) = &wire.presentation {
                pres.feedback(surface, &wire.rqh, me.clone());
            }
            self.waiting = true;
            self.rec.rearm = false;
        }
        if !hold_ack {
            self.ack_content = false;
        }
        self.owed = false;
        self.tree_content = false;
        self.commit.surface.commit();
        let rearm = self.send_rearm(me, &wire.rqh);
        self.pool.submitted(slot);
        1 + u64::from(rearm)
    }

    /// A commit with no buffer, viewport or scale: a bare ack goes with it (a content ack waits for a buffer), and the
    /// recorded surface state.  No frame callback or feedback either, unless it is a root's commit that applies a
    /// child's buffer: that commit shows content, and paces the tree.  A root's re-arm goes after it, as a commit of
    /// its own, or is the only commit when nothing else is owed.  The commits sent.
    fn commit_bare(&mut self, me: &Arc<Mutex<Present>>, wire: &Wire) -> u64 {
        let child = self.root.is_some();
        let bare_ack = self.ack.is_some() && !self.ack_content;
        if !child && self.rec.rearm && self.waiting && !(self.owed || bare_ack || self.tree_content)
        {
            return u64::from(self.send_rearm(me, &wire.rqh));
        }
        if bare_ack
            && let Some(serial) = self.ack.take()
            && let Some(xdg) = &self.commit.xdg
        {
            xdg.ack_configure(serial);
        }
        self.send_recorded(false, wire.compositor.as_ref(), &wire.rqh);
        if std::mem::take(&mut self.tree_content) {
            let surface = &self.commit.surface;
            surface.frame(&wire.rqh, me.clone());
            if let Some(pres) = &wire.presentation {
                pres.feedback(surface, &wire.rqh, me.clone());
            }
            self.waiting = true;
            self.rec.rearm = false;
        }
        self.owed = false;
        self.commit.surface.commit();
        1 + u64::from(self.send_rearm(me, &wire.rqh))
    }

    /// Window `id` owes a commit: a root's owed state or bare ack, a child's own surface state.
    fn owes(&self) -> bool {
        if self.root.is_some() {
            self.owed
        } else {
            self.ready() & NHSHELL_READY_OWED != 0
        }
    }

    /// The popup was made again on the same surface with `xdg`: the recorded ack belongs to the destroyed
    /// `xdg_surface`, and the new one has had no configure.
    pub(crate) fn reset(&mut self, xdg: XdgSurface) {
        self.commit.xdg = Some(xdg);
        self.ack = None;
        self.ack_content = false;
        self.configured = false;
        self.epoch += 1;
    }

    /// The window was made again on a new connection with the proxies in `commit` (a reconnect): it is back at its
    /// state at creation (unconfigured, unless a child; no ready bits, frame callback, feedback or ack owed), so
    /// nothing commits it before the new compositor's first configure, which is a content ack.  Its buffers, fronts
    /// and recorded state stay; what the old surface had been sent (input region, alpha, window geometry) is recorded
    /// again unless something newer is, and a child owes the commit that carries it.
    pub(crate) fn remade(&mut self, commit: Commit) {
        self.commit = commit;
        let (r, sent) = (&mut self.rec, &self.sent);
        r.stack.clear();
        r.rearm = false;
        r.input = r.input.take().or_else(|| sent.input.clone());
        r.alpha = r.alpha.or(sent.alpha);
        r.geometry = r.geometry.or(sent.geometry);
        let child = self.root.is_some();
        self.configured = child;
        self.owed = child;
        self.ack = None;
        self.ack_content = false;
        self.content_next = !child;
        self.waiting = false;
        self.starved = false;
        self.notified = false;
        self.suspended = false;
        self.tree_content = false;
        self.epoch += 1;
    }
}

/// A buffer taken out of its window's pool for one frame, between the decision and the send.  Holding the buffer
/// keeps its mapping alive while it is composed, whatever happens to the window meanwhile.
struct Job {
    id: u64,
    /// The window's presenter state the buffer came from (an id can be reused meanwhile).
    present: Arc<Mutex<Present>>,
    epoch: u64,
    slot: BufferSlot,
    buf: ShmBuf,
    damage: Vec<Rect>,
}

impl Job {
    /// The buffer goes back to its window unused.
    fn put_back(self, p: &mut Present) {
        p.pool.abandon(self.slot);
        p.bufs.insert(self.slot.id, self.buf);
    }
}

/// The child windows of `id` in `kids`, in a stable order.
fn kids_of(kids: &[Kid], id: u64) -> Vec<&Kid> {
    let mut v: Vec<&Kid> = kids.iter().filter(|k| k.parent == id).collect();
    v.sort_unstable_by_key(|k| k.id);
    v
}

/// `id` or a drawable child window under it has damage waiting.
fn tree_damaged(stage: &Stage, kids: &[Kid], id: u64) -> bool {
    stage.damaged(id)
        || kids_of(kids, id)
            .into_iter()
            .any(|k| stage.drawable(k.id) && tree_damaged(stage, kids, k.id))
}

/// Take a buffer for surface `id`'s waiting damage out of its pool (`p`, locked).  `None` when it has nothing to
/// show or no buffer is free (that starves the tree; the release brings the draw back and the damage keeps waiting).
fn begin(
    stage: &Stage,
    wire: &Wire,
    id: u64,
    me: &Arc<Mutex<Present>>,
    p: &mut Present,
) -> Option<Job> {
    if p.retired || !p.configured {
        return None;
    }
    let (w, h, damage) = stage.next_frame(id)?;
    let now = now_ns();
    p.reap(now);
    let slot = p.take_slot(w, h, now)?;
    p.reap(now);
    p.ensure_buf(me, wire, &slot).ok()?;
    let buf = p.bufs.remove(&slot.id)?;
    Some(Job {
        id,
        present: me.clone(),
        epoch: p.epoch,
        slot,
        buf,
        damage,
    })
}

/// Take buffers for the child windows under `id`, deepest first, each when `draw` and it is drawable.
fn begin_kids(stage: &Stage, wire: &Wire, kids: &[Kid], id: u64, draw: bool, jobs: &mut Vec<Job>) {
    for k in kids_of(kids, id) {
        let d = draw && stage.drawable(k.id);
        begin_kids(stage, wire, kids, k.id, d, jobs);
        if d {
            let mut p = lock(&k.present);
            jobs.extend(begin(stage, wire, k.id, &k.present, &mut p));
        }
    }
}

fn take_job(jobs: &mut Vec<Job>, me: &Arc<Mutex<Present>>) -> Option<Job> {
    let at = jobs.iter().position(|j| Arc::ptr_eq(&j.present, me))?;
    Some(jobs.swap_remove(at))
}

/// Take buffers for root `root`'s content layers (`p`, the root's state, locked): a layer for each own output of the
/// newest scene that has none, the others marked leaving, then a buffer for each layer whose output has changes
/// waiting.
fn begin_layers(
    stage: &Stage,
    wire: &Wire,
    root: u64,
    me: &Arc<Mutex<Present>>,
    p: &mut Present,
) -> Vec<LayerJob> {
    if p.retired || !p.configured {
        return Vec::new();
    }
    let outs = stage.outputs(root);
    for l in &mut p.layers {
        l.leaving = !outs.iter().any(|(id, _)| *id == l.id);
    }
    for (id, _) in &outs {
        if !p.layers.iter().any(|l| l.id == *id)
            && let Some(l) = ContentLayer::new(*id, wire, &p.commit.surface)
        {
            p.layers.push(l);
        }
    }
    let (grow, now) = (!p.waiting, now_ns());
    let mut jobs = Vec::new();
    for l in p.layers.iter_mut().filter(|l| !l.leaving) {
        if let Some(pending) = stage.pending(root, l.id) {
            jobs.extend(l.begin(me, wire, pending, grow, now));
        }
    }
    jobs
}

/// Commit root `root`'s layer jobs before the root's commit (sync children apply with it), unmap the leaving layers
/// when the root's buffer goes in the same tree commit (`root_shows`: it shows their windows in the frame again), and
/// keep the layers right above the root, below its child windows, whenever the root's children are restacked.
/// `true` when a layer committed a buffer.
fn send_layers(
    stage: &mut Stage,
    wire: &Wire,
    root: u64,
    r: &mut Present,
    jobs: Vec<LayerJob>,
    root_shows: bool,
) -> bool {
    let scale = r.commit.scale120;
    let mut shown = false;
    for j in jobs {
        let Some(l) = r.layers.iter_mut().find(|l| l.id == j.layer) else {
            continue;
        };
        if l.leaving {
            l.put_back(j);
            continue;
        }
        let id = l.id;
        l.commit(j, wire, scale);
        stage.take_pending(root, id);
        shown = true;
    }
    let mut owed = shown;
    if root_shows {
        for l in r.layers.iter().filter(|l| l.leaving) {
            l.unmap();
            owed = true;
        }
    }
    let Present {
        layers,
        rec,
        commit,
        ..
    } = r;
    if !rec.stack.is_empty() || layers.iter().any(|l| !l.placed) {
        // after the children's moves: right above the root is below every child
        for l in layers.iter_mut().filter(|l| !l.leaving) {
            rec.stack
                .push((l.subsurface().clone(), commit.surface.clone(), true));
            l.placed = true;
            owed = true;
        }
    }
    r.tree_content |= shown;
    r.owed |= owed;
    shown
}

/// A commit of root `root`'s tree (`r`) showed published drawing: a frame for the presenter's timing.
fn count_frame(stage: &Stage, root: u64, r: &Present) {
    let start = stage.draw_start(root);
    if start == 0 {
        return;
    }
    let ready = start.max(r.t_open_ns);
    let refresh = match r.refresh_ns {
        0 => DEFAULT_REFRESH_NS,
        n => u64::from(n),
    };
    let now = crate::wayland::publish::now_ns();
    stage.stats.frame(now.saturating_sub(ready), refresh);
}

/// Commit the child windows under `id`, deepest first (a sync child shows once every ancestor committed after it):
/// each with its composed buffer, else bare when it owes a commit.  Each commit makes its ancestors owe one, and a
/// buffer makes the root's commit paced.  `r` is the locked root.
fn send_kids(
    stage: &mut Stage,
    wire: &Wire,
    kids: &[Kid],
    id: u64,
    r: &mut Present,
    jobs: &mut Vec<Job>,
) {
    for k in kids_of(kids, id) {
        send_kids(stage, wire, kids, k.id, r, jobs);
        let mut p = lock(&k.present);
        if p.retired {
            // its job, if any, drops with the others: the buffer goes with the window
            continue;
        }
        let mut buffer = false;
        if let Some(j) = take_job(jobs, &k.present) {
            if j.epoch == p.epoch {
                p.commit_buffer(&k.present, wire, j.slot, &j.buf, &j.damage, false);
                p.bufs.insert(j.slot.id, j.buf);
                stage.presented(k.id);
                buffer = true;
            } else {
                j.put_back(&mut p);
            }
        }
        let bare = !buffer && p.owed;
        if bare {
            p.commit_bare(&k.present, wire);
            Counters::add(&stage.stats.bare_commits, 1);
        }
        drop(p);
        if !(buffer || bare) {
            continue;
        }
        r.tree_content |= buffer;
        // every ancestor up to the root owes a commit now
        let mut cur = k.parent;
        while let Some(a) = kids.iter().find(|a| a.id == cur) {
            lock(&a.present).owed = true;
            cur = a.parent;
        }
        r.owed = true;
    }
}

/// Present root `root`'s tree (`tree`, its `Present`) by the present path's rule; `true` when the root committed a
/// buffer.  Buffers when the fronts hold damage for a drawable surface or an eligible content ack waits, and the gate is
/// open (configured, a buffer free, and idle and shown unless the ack waives that): the child windows first, deepest
/// first, then the root, which composes its whole picture when the ack came with nothing drawn.  Otherwise bare
/// commits where one is owed (a bare ack, or surface state), deepest first, the root last.  Otherwise nothing.  A
/// content ack never rides a bare commit.
///
/// The decision and the sends run under the tree's mutex; composing runs between them without it, so the host's
/// thread never waits for a compose.  A tree retired meanwhile commits nothing; its buffers go.  `delay`: a test hook
/// that makes composing take that long.
pub(crate) fn present_tree(
    stage: &mut Stage,
    wire: &Wire,
    root: u64,
    tree: &Arc<Mutex<Present>>,
    delay: Duration,
) -> bool {
    let applied = stage.applied(root);
    let mut jobs = Vec::new();
    let mut layer_jobs = Vec::new();
    {
        let mut r = lock(tree);
        if r.retired {
            return false;
        }
        r.notified = false;
        let kids = r.kids.clone();
        if !ContentLayer::can(wire) {
            // no layer can show a window apart: the frame shows it
            stage.fold_outputs(root);
        }
        // damage in the fronts, or a content layer's change, is drawn content, whenever its packet arrived
        if tree_damaged(stage, &kids, root) || stage.outputs_pending(root) {
            r.dirty = true;
        }
        let bits = r.ready_published(applied);
        // reporting content takes the draw
        if bits & NHSHELL_READY_CONTENT != 0 {
            r.dirty = false;
        }
        if bits == 0 {
            return false;
        }
        if bits & NHSHELL_READY_ACK != 0 {
            stage.damage_all_if_clean(root);
        }
        if bits & NHSHELL_READY_CONTENT != 0
            && (tree_damaged(stage, &kids, root) || stage.outputs_pending(root))
        {
            begin_kids(stage, wire, &kids, root, true, &mut jobs);
            jobs.extend(begin(stage, wire, root, tree, &mut r));
            layer_jobs = begin_layers(stage, wire, root, tree, &mut r);
        }
        if jobs.is_empty() && layer_jobs.is_empty() {
            return send(stage, wire, root, tree, &mut r, applied, jobs, layer_jobs);
        }
    }
    for j in &jobs {
        let buf = NhShellBuffer {
            pixels: j.buf.pixels(),
            w: j.buf.w,
            h: j.buf.h,
            stride: j.buf.stride,
            age: j.slot.age,
        };
        stage.compose(j.id, &buf);
    }
    for j in &layer_jobs {
        j.draw(stage, root);
    }
    if !delay.is_zero() {
        std::thread::sleep(delay);
    }
    let mut r = lock(tree);
    send(stage, wire, root, tree, &mut r, applied, jobs, layer_jobs)
}

/// `present_tree`'s send phase, under the tree's mutex: the child windows and the content layers, then the root.  A
/// tree retired meanwhile commits nothing; its layers' buffers went with it.
#[allow(clippy::too_many_arguments)]
fn send(
    stage: &mut Stage,
    wire: &Wire,
    root: u64,
    tree: &Arc<Mutex<Present>>,
    r: &mut Present,
    applied: u64,
    mut jobs: Vec<Job>,
    layer_jobs: Vec<LayerJob>,
) -> bool {
    if r.retired {
        return false;
    }
    let kids = r.kids.clone();
    send_kids(stage, wire, &kids, root, r, &mut jobs);
    let root_shows = r.configured
        && jobs
            .iter()
            .any(|j| Arc::ptr_eq(&j.present, tree) && j.epoch == r.epoch);
    let layered = send_layers(stage, wire, root, r, layer_jobs, root_shows);
    let mut shown = false;
    if let Some(j) = take_job(&mut jobs, tree) {
        if j.epoch == r.epoch && r.configured {
            let held = r.ack_held_at(applied);
            r.commit_buffer(tree, wire, j.slot, &j.buf, &j.damage, held);
            r.bufs.insert(j.slot.id, j.buf);
            count_frame(stage, root, r);
            stage.presented(root);
            // the leaving layers' unmaps applied with this commit: their objects go now
            r.layers.retain(|l| !l.leaving);
            shown = true;
        } else {
            j.put_back(r);
        }
    }
    if !shown && layered {
        count_frame(stage, root, r);
        stage.presented_layers(root);
    }
    if !shown && r.owes() {
        r.commit_bare(tree, wire);
        Counters::add(&stage.stats.bare_commits, 1);
    }
    // a child's job left over (the child went meanwhile, or its buffer could not be committed) goes back or drops
    for j in jobs {
        let present = j.present.clone();
        let mut p = lock(&present);
        if !p.retired && p.epoch == j.epoch {
            j.put_back(&mut p);
        }
    }
    shown
}

impl MainState {
    /// Window `id`'s role is set up (`xdg` for a top-level or popup, `subsurface` for a child): its presenter state
    /// takes the commit-time proxies, and from now on only presenting commits it.  A child joins its root's list, a
    /// root the presenter's.
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
        if let Some(parent) = self.children.get(&id).map(|c| c.parent) {
            let Some(root) = self.presents.get(&self.tree_root(id)).cloned() else {
                return;
            };
            let p = Arc::new(Mutex::new(Present::new(
                id,
                commit,
                None,
                Some(root.clone()),
                self.link.clone(),
            )));
            lock(&root).kids.push(Kid {
                id,
                parent,
                present: p.clone(),
            });
            self.presents.insert(id, p);
        } else {
            let p = Arc::new(Mutex::new(Present::new(
                id,
                commit,
                self.shared.clone(),
                None,
                self.link.clone(),
            )));
            if let Some(link) = &self.link {
                link.add_root(id, p.clone());
            }
            self.presents.insert(id, p);
        }
    }

    /// Window `id` was made again on the new connection (`xdg` for a top-level, `subsurface` for a child): its
    /// presenter state takes the new proxies and goes back to its state at creation (`Present::remade`).  Its scale
    /// stays, and an alpha object it had is made again when the compositor still has the protocol.
    pub(crate) fn present_again(
        &mut self,
        id: u64,
        xdg: Option<XdgSurface>,
        subsurface: Option<WlSubsurface>,
        qh: &QueueHandle<MainState>,
    ) {
        let (Some(surface), Some(present)) =
            (self.surfaces.get(&id).cloned(), self.presents.get(&id))
        else {
            return;
        };
        let viewport = self.scales.get(&id).and_then(|s| s.viewport());
        let mut p = lock(present);
        let alpha = match (&p.commit.alpha, &self.alpha_modifier) {
            (Some(_), Some(m)) => Some(m.get_surface(&surface, qh, ())),
            _ => None,
        };
        let commit = Commit {
            surface,
            xdg,
            subsurface,
            viewport,
            alpha,
            scale120: p.commit.scale120,
        };
        p.remade(commit);
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
        // the first configure after a reconnect brings the picture back
        let remade = std::mem::take(&mut p.content_next);
        p.ack_content |= content || remade;
        p.ack_seq = self.mailbox.seq();
        p.notify();
    }

    /// Root `root`'s recorded content ack waits for a packet published after it.
    pub(crate) fn present_wants_packet(&mut self, root: u64) -> bool {
        self.present_of(root).is_some_and(|p| {
            let p = lock(&p);
            p.ack_held_at(p.published)
        })
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
    /// The connection as presenting uses it.
    fn wire(&self) -> ShellResult<Arc<Wire>> {
        self.link.wire().ok_or(ShellError::State)
    }

    /// The next frame's buffer for window `id`, `dev_w` x `dev_h` device pixels.  `State` in thread mode: the
    /// presenter thread is then the only one that commits.
    pub(super) fn present_begin_impl(
        &mut self,
        id: WindowId,
        dev_w: i32,
        dev_h: i32,
    ) -> ShellResult<NhShellBuffer> {
        if self.link.on() || !self.main.is_window(id) || dev_w <= 0 || dev_h <= 0 {
            return Err(ShellError::State);
        }
        let present = self.main.present_of(id).ok_or(ShellError::State)?;
        let wire = self.wire()?;
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
                p.take_slot(dev_w, dev_h, now).ok_or(ShellError::State)?
            }
        };
        p.reap(now);
        p.ensure_buf(&present, &wire, &slot)?;
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

    /// Show the begun buffer of window `id`; `damage` is in device pixels.  The surface's recorded state goes right
    /// before the commit.
    pub(super) fn present_commit_impl(&mut self, id: WindowId, damage: &[Rect]) -> ShellResult {
        if self.link.on() {
            return Err(ShellError::State);
        }
        let present = self.main.present_of(id).ok_or(ShellError::State)?;
        let wire = self.wire()?;
        let mut p = lock(&present);
        if p.retired {
            return Err(ShellError::State);
        }
        let slot = p.begun.take().ok_or(ShellError::State)?;
        let buf = p.bufs.remove(&slot.id).ok_or(ShellError::State)?;
        let n = p.commit_buffer(&present, &wire, slot, &buf, damage, false);
        p.bufs.insert(slot.id, buf);
        self.tree_commits.off += n;
        let root = p.root.clone();
        drop(p);
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

    /// A bare commit of window `id` (`Present::commit_bare`).  A child's makes its ancestors owe one.  `State` in
    /// thread mode.
    pub(super) fn present_bare_impl(&mut self, id: WindowId) -> ShellResult {
        if self.link.on() {
            return Err(ShellError::State);
        }
        let present = self.main.present_of(id).ok_or(ShellError::State)?;
        let wire = self.wire()?;
        let mut p = lock(&present);
        if p.retired {
            return Err(ShellError::State);
        }
        let child = p.root.is_some();
        self.tree_commits.off += p.commit_bare(&present, &wire);
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

    /// Present root `root`'s tree by the present path's rule, at its `FRAME_READY` (inline mode; `State` in thread
    /// mode).  `true` when the root committed a buffer.
    pub(super) fn present_tree_impl(&mut self, root: WindowId) -> ShellResult<bool> {
        if self.link.on() {
            return Err(ShellError::State);
        }
        if !self.main.is_window(root) || self.main.children.contains_key(&root) {
            return Err(ShellError::State);
        }
        let wire = self.wire()?;
        let stage = self.presenting.stage.as_mut().ok_or(ShellError::State)?;
        stage.drain(&self.main.mailbox);
        let Some(tree) = self.main.presents.get(&root).cloned() else {
            return Ok(false);
        };
        let shown = present_tree(stage, &wire, root, &tree, Duration::ZERO);
        self.flush()?;
        Ok(shown)
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
    /// buffers and its front with it; a child leaves its root's list, a root the presenter's.
    pub(super) fn present_retire(&mut self, id: WindowId, destroy: impl FnOnce(&mut MainState)) {
        let root = self.main.tree_root(id);
        let tree = self.main.present_of(root);
        let own = self.main.presents.remove(&id);
        {
            let mut tree = tree.as_ref().map(|t| lock(t));
            let mut own = own.as_ref().filter(|_| root != id).map(|p| lock(p));
            if let Some(t) = tree.as_deref_mut()
                && root != id
            {
                t.kids.retain(|k| k.id != id);
            }
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
        if root == id
            && let Some(t) = &tree
        {
            self.link.remove_root(t);
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

    /// Test hook: root `id`'s content layers, each as (surface id, view offset, ring height, each buffer's resident
    /// 512-byte blocks).
    #[doc(hidden)]
    pub fn content_layers(&self, id: WindowId) -> Vec<(u64, i32, i32, Vec<u64>)> {
        self.main.presents.get(&id).map_or_else(Vec::new, |p| {
            lock(p).layers.iter().map(ContentLayer::info).collect()
        })
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
            p.t_open_ns = crate::wayland::publish::now_ns();
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
                if refresh != 0 {
                    p.refresh_ns = refresh;
                }
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
                let now = crate::wayland::publish::now_ns();
                match p.root.clone() {
                    Some(root) => {
                        drop(p);
                        let mut r = lock(&root);
                        r.dirty = true;
                        r.t_open_ns = now;
                        r.notify();
                    }
                    None => {
                        p.t_open_ns = now;
                        p.notify();
                    }
                }
            }
        }
    }
}

delegate_noop!(MainState: WlShmPool);
delegate_noop!(MainState: ignore WpPresentation);
// input regions made where the recorded state is sent
delegate_noop!(ReaderState: WlRegion);

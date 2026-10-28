//! The presenter: `present_begin` hands out a buffer of the window's pool, `present_commit` shows it.
//!
//! A commit attaches the buffer, damages it (buffer coordinates), sets the viewport destination to the logical size and
//! `buffer_scale` 1 (the integer scale without a viewporter), requests a frame callback (and presentation feedback
//! for the statistics) and commits.
//!
//! One pacer per window decides when the host may draw: the host calls `window_dirty` and draws when it gets
//! `FRAME_READY` (or `window_ready` says so).  The pacer is `Idle` or `Waiting` for the frame callback of the last
//! commit that carried content; a commit without new content (an ack, empty damage) requests no callback and
//! leaves the state alone.  An ack never waits: it makes the pacer ready at once, in any state but starved.  A pacer
//! is starved when `present_begin` found every buffer busy and leaves that state on a release; when it is idle and
//! the two buffers are busy, the pool grows to three first.  A suspended window only acks.  A pacer that waits
//! re-arms (a callback and an empty commit) when a configure, the end of suspension, `wl_surface.enter` or a scale
//! change shows the surface may be visible again.  Presentation feedback never gates anything.  Callbacks, feedback
//! and releases run on the reader queue.  The buffer policy is `glitnir::pool`.

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
use std::time::Instant;
use wayland_client::protocol::wl_buffer::{self, WlBuffer};
use wayland_client::protocol::wl_callback::{self, WlCallback};
use wayland_client::protocol::wl_shm_pool::WlShmPool;
use wayland_client::{Connection, Dispatch, QueueHandle, delegate_noop};
use wayland_protocols::wp::presentation_time::client::wp_presentation::WpPresentation;
use wayland_protocols::wp::presentation_time::client::wp_presentation_feedback::{
    self, WpPresentationFeedback,
};
use wayland_protocols::xdg::shell::client::xdg_surface::XdgSurface;

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
}

/// The user data of a `wl_buffer`: its window's presenter and its slot.
pub type BufData = (Arc<Mutex<Present>>, usize);

fn lock(p: &Mutex<Present>) -> MutexGuard<'_, Present> {
    p.lock().unwrap_or_else(|e| e.into_inner())
}

fn now_ns() -> u64 {
    static BASE: OnceLock<Instant> = OnceLock::new();
    BASE.get_or_init(Instant::now).elapsed().as_nanos() as u64
}

impl Present {
    fn new(window: u64, shared: Option<Arc<Shared>>, configured: bool) -> Present {
        Present {
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
            configured,
            suspended: false,
            notified: false,
            shared,
            presented: 0,
            discarded: 0,
            freed: 0,
        }
    }

    /// What the host may do now: `CONTENT` (draw and present a buffer: drawn content when idle and shown, or a content
    /// ack, which never waits) and `OWED` (a commit is owed: a bare ack or surface state, starved or not).
    fn ready(&self) -> u32 {
        let content = self.configured
            && !self.starved
            && ((self.dirty && !self.waiting && !self.suspended) || self.ack_content);
        let owed = self.configured && (self.owed || (self.ack.is_some() && !self.ack_content));
        u32::from(content) * NHSHELL_READY_CONTENT + u32::from(owed) * NHSHELL_READY_OWED
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
}

impl MainState {
    fn present_of(&mut self, id: u64) -> Arc<Mutex<Present>> {
        let shared = self.shared.clone();
        // a child needs no configure
        let configured = self.children.contains_key(&id);
        self.presents
            .entry(id)
            .or_insert_with(|| Arc::new(Mutex::new(Present::new(id, shared, configured))))
            .clone()
    }

    /// Window `id` owes a commit (surface state changed): wake its pacer.
    pub(crate) fn present_owe(&mut self, id: u64) {
        let present = self.present_of(id);
        let mut p = lock(&present);
        p.owed = true;
        p.notify();
    }

    /// Record `serial`'s ack for window `id` (sent with the next commit) and wake the pacer.
    pub(crate) fn present_ack(&mut self, id: u64, serial: u32, content: bool) {
        let present = self.present_of(id);
        let mut p = lock(&present);
        p.configured = true;
        p.ack = Some(serial);
        p.ack_content |= content;
        p.notify();
    }

    /// Popup `id` was made again on the same surface: the recorded ack belongs to the destroyed `xdg_surface`, and
    /// the new one has had no configure.
    pub(crate) fn present_reset(&mut self, id: u64) {
        let present = self.present_of(id);
        let mut p = lock(&present);
        p.ack = None;
        p.ack_content = false;
        p.configured = false;
    }

    /// The `xdg_surface` that `id`'s acks go to.
    fn xdg_of(&self, id: u64) -> Option<&XdgSurface> {
        self.windows
            .get(&id)
            .map(|w| &w.xdg_surface)
            .or_else(|| self.popups.get(&id).map(|p| p.xdg_surface()))
    }

    /// The compositor configured window `id`: note whether it is suspended, and re-arm a waiting pacer.
    pub(crate) fn present_configured(&mut self, id: u64, suspended: bool, conn: &Connection) {
        if !self.windows.contains_key(&id) {
            return;
        }
        let present = self.present_of(id);
        let mut p = lock(&present);
        p.configured = true;
        let was = std::mem::replace(&mut p.suspended, suspended);
        drop(p);
        self.present_rearm(id, conn);
        if was && !suspended {
            lock(&present).notify();
        }
    }

    /// A waiting pacer asks again: a frame callback and an empty commit.  The surface may have become visible, and
    /// the callback of the commit that waits may never have been answered.
    pub(crate) fn present_rearm(&mut self, id: u64, conn: &Connection) {
        let Some(present) = self.presents.get(&id).cloned() else {
            return;
        };
        if !lock(&present).waiting {
            return;
        }
        if let (Some(surface), Some(rqh)) = (self.surfaces.get(&id), self.reader_qh.as_ref()) {
            surface.frame(rqh, present);
            surface.commit();
            if let Some(shared) = &self.shared {
                let _ = shared.host_flush(conn);
            }
        }
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
        let present = self.main.present_of(id);
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
                    p.starved = true;
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
        let present = self
            .main
            .presents
            .get(&id)
            .ok_or(ShellError::State)?
            .clone();
        let mut p = lock(&present);
        let slot = p.begun.take().ok_or(ShellError::State)?;
        // a buffer that was never written has no age: the whole of it is new
        let all = match p.bufs.get(&slot.id) {
            Some(b) if slot.age == 0 => vec![Rect::new(0, 0, b.w, b.h)],
            _ => merged(damage),
        };
        let surface = self.main.surfaces.get(&id).ok_or(ShellError::State)?;
        let ack = p.ack.take();
        let buf = p.bufs.get(&slot.id).ok_or(ShellError::State)?;
        let scale = self
            .main
            .scales
            .get(&id)
            .map_or(SCALE_ONE, |s| s.scale120());
        if self.main.viewporter.is_some() {
            let s = i64::from(scale.max(1));
            let logical = |d: i32| ((i64::from(d) * 120 + s / 2) / s) as i32;
            self.main
                .scales
                .get(&id)
                .ok_or(ShellError::State)?
                .set_destination(logical(buf.w), logical(buf.h));
            surface.set_buffer_scale(1);
        } else {
            surface.set_buffer_scale(((scale + 60) / 120).max(1) as i32);
        }
        // the ack goes before anything the commit carries (a checker, and a compositor, read the order)
        if let Some(serial) = ack
            && let Some(xdg) = self.main.xdg_of(id)
        {
            xdg.ack_configure(serial);
        }
        surface.attach(Some(&buf.buffer), 0, 0);
        for r in &all {
            surface.damage_buffer(r.x, r.y, r.w, r.h);
        }
        // only a commit with content is paced: the frame callback releases the pacer, feedback is statistics
        let rqh = self.main.reader_qh.as_ref().ok_or(ShellError::State)?;
        if !all.is_empty() {
            surface.frame(rqh, present.clone());
            if let Some(pres) = &self.main.presentation {
                pres.feedback(surface, rqh, present.clone());
            }
            p.waiting = true;
        }
        p.ack_content = false;
        p.owed = false;
        surface.commit();
        p.pool.submitted(slot);
        drop(p);
        self.flush()
    }

    /// The host has drawn into window `id`: `FRAME_READY` comes when the pacer lets it present.
    pub(super) fn window_dirty_impl(&mut self, id: WindowId) -> ShellResult {
        if !self.main.is_window(id) {
            return Err(ShellError::State);
        }
        let present = self.main.present_of(id);
        let mut p = lock(&present);
        p.dirty = true;
        p.notify();
        Ok(())
    }

    /// A commit with no buffer, callback, feedback, viewport or scale: a bare ack goes with it (a content ack waits
    /// for a buffer), and the owed surface state.
    pub(super) fn present_bare_impl(&mut self, id: WindowId) -> ShellResult {
        let present = self
            .main
            .presents
            .get(&id)
            .ok_or(ShellError::State)?
            .clone();
        let mut p = lock(&present);
        let surface = self.main.surfaces.get(&id).ok_or(ShellError::State)?;
        if !p.ack_content
            && let Some(serial) = p.ack.take()
            && let Some(xdg) = self.main.xdg_of(id)
        {
            xdg.ack_configure(serial);
        }
        p.owed = false;
        surface.commit();
        drop(p);
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

    /// Whether the host may draw and present window `id` now.
    pub(super) fn window_ready_impl(&mut self, id: WindowId) -> ShellResult<u32> {
        if !self.main.is_window(id) {
            return Err(ShellError::State);
        }
        Ok(self.main.presents.get(&id).map_or(0, |p| {
            let mut p = lock(p);
            p.notified = false;
            let bits = p.ready();
            if bits & NHSHELL_READY_CONTENT != 0 {
                p.dirty = false;
            }
            bits
        }))
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
    }

    /// Window `id` is going away: its buffers go with it.
    pub(super) fn present_drop(&mut self, id: u64) {
        if let Some(p) = self.main.presents.remove(&id) {
            lock(&p).clear();
        }
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
            // a starved pacer leaves its state on a release
            if std::mem::take(&mut p.starved) {
                p.notify();
            }
        }
    }
}

delegate_noop!(MainState: WlShmPool);
delegate_noop!(MainState: ignore WpPresentation);

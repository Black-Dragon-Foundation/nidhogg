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
    /// An ack went out and the commit that carries its buffer has not.
    ack_owed: bool,
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
    fn new(window: u64, shared: Option<Arc<Shared>>) -> Present {
        Present {
            window,
            pool: Pool::new(),
            bufs: HashMap::new(),
            begun: None,
            waiting: false,
            starved: false,
            dirty: false,
            ack_owed: false,
            suspended: false,
            notified: false,
            shared,
            presented: 0,
            discarded: 0,
            freed: 0,
        }
    }

    /// The host may draw now: an owed ack goes out whatever the state, drawn content only when idle and shown.
    fn ready(&self) -> bool {
        !self.starved && (self.ack_owed || (self.dirty && !self.waiting && !self.suspended))
    }

    /// Post `FRAME_READY` when the pacer is ready, once until the host asks `window_ready`.
    fn notify(&mut self) {
        if let (true, false, Some(shared)) = (self.ready(), self.notified, &self.shared) {
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
        self.presents
            .entry(id)
            .or_insert_with(|| Arc::new(Mutex::new(Present::new(id, shared))))
            .clone()
    }

    /// The compositor configured window `id`: note whether it is suspended, and re-arm a waiting pacer.
    pub(crate) fn present_configured(&mut self, id: u64, suspended: bool, conn: &Connection) {
        if !self.windows.contains_key(&id) {
            return;
        }
        let present = self.present_of(id);
        let mut p = lock(&present);
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
        if !self.main.windows.contains_key(&id) || dev_w <= 0 || dev_h <= 0 {
            return Err(ShellError::State);
        }
        let present = self.main.present_of(id);
        let mut p = lock(&present);
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
                if s.is_none() && !p.waiting {
                    // idle with every buffer busy: the compositor holds them, so the pool grows
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
        p.ack_owed = false;
        surface.commit();
        p.pool.submitted(slot);
        drop(p);
        self.flush()
    }

    /// The host has drawn into window `id`: `FRAME_READY` comes when the pacer lets it present.
    pub(super) fn window_dirty_impl(&mut self, id: WindowId) -> ShellResult {
        if !self.main.windows.contains_key(&id) {
            return Err(ShellError::State);
        }
        let present = self.main.present_of(id);
        let mut p = lock(&present);
        p.dirty = true;
        p.notify();
        Ok(())
    }

    /// An ack went out for window `id`: a commit must follow, and it never waits for the frame callback.
    pub(super) fn ack_sent(&mut self, id: WindowId) {
        let present = self.main.present_of(id);
        let mut p = lock(&present);
        p.ack_owed = true;
        p.notify();
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
    pub(super) fn window_ready_impl(&mut self, id: WindowId) -> ShellResult<bool> {
        if !self.main.windows.contains_key(&id) {
            return Err(ShellError::State);
        }
        Ok(self.main.presents.get(&id).is_some_and(|p| {
            let mut p = lock(p);
            p.notified = false;
            p.ready()
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

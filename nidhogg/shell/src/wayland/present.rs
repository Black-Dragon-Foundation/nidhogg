//! The presenter: `present_begin` hands out a buffer of the window's pool, `present_commit` shows it.
//!
//! A commit attaches the buffer, damages it (buffer coordinates), sets the viewport destination to the logical size and
//! `buffer_scale` 1 (the integer scale without a viewporter), requests a frame callback and presentation feedback and
//! commits.  While a frame callback is pending no commit is made: the begun buffer is kept, its damage is merged, and
//! the next `present_begin` hands the same buffer out again (age 1, it holds the previous frame).  The one exception is
//! a commit right after `ack_configure`, which must reach the compositor.  Frame callbacks, presentation feedback and
//! buffer releases run on the reader queue and post `FRAME_DONE`; the policy is `glitnir::pool`.

use crate::api::*;
use crate::event::*;
use crate::wayland::reader::ReaderState;
use crate::wayland::scale::{SCALE_ONE, buffer_size};
use crate::wayland::shm::ShmBuf;
use crate::wayland::{MainState, WaylandShell};
use glitnir::geom::Rect;
use glitnir::pool::{BufferSlot, Pool, merged};
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
    pool: Pool,
    bufs: HashMap<usize, ShmBuf>,
    /// The slot `present_begin` handed out and nothing committed yet.
    begun: Option<BufferSlot>,
    /// A begun slot whose commit waited for a frame callback, and the damage it has gathered.
    deferred: Option<(BufferSlot, Vec<Rect>)>,
    /// Frame callbacks requested and not yet done.
    cb_pending: u32,
    /// `ack_configure` ran: the next commit must be made.
    force: bool,
    /// Buffers freed by trimming or a resize.
    freed: usize,
}

/// The user data of a `wl_buffer`: its window's presenter and its slot.
pub type BufData = (Arc<Mutex<Present>>, usize);

/// The user data of a frame callback.
pub struct FrameTag {
    window: u64,
    present: Arc<Mutex<Present>>,
    /// Presentation feedback was requested with this commit; it posts `FRAME_DONE`.
    feedback: bool,
}

fn lock(p: &Mutex<Present>) -> MutexGuard<'_, Present> {
    p.lock().unwrap_or_else(|e| e.into_inner())
}

fn now_ns() -> u64 {
    static BASE: OnceLock<Instant> = OnceLock::new();
    BASE.get_or_init(Instant::now).elapsed().as_nanos() as u64
}

impl Present {
    fn new() -> Present {
        Present {
            pool: Pool::new(),
            bufs: HashMap::new(),
            begun: None,
            deferred: None,
            cb_pending: 0,
            force: false,
            freed: 0,
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
        self.deferred = None;
    }
}

impl MainState {
    fn present_of(&mut self, id: u64) -> Arc<Mutex<Present>> {
        self.presents
            .entry(id)
            .or_insert_with(|| Arc::new(Mutex::new(Present::new())))
            .clone()
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
        let slot = match (p.begun, p.deferred.take()) {
            // begun twice without a commit: the same buffer
            (Some(s), d) if (s.w, s.h) == (dev_w, dev_h) => {
                p.deferred = d;
                s
            }
            // a frame whose commit waited: draw over it, it holds the previous frame
            (_, Some((s, d))) if (s.w, s.h) == (dev_w, dev_h) => {
                p.deferred = Some((s, d));
                BufferSlot { age: 1, ..s }
            }
            (b, d) => {
                for s in b.into_iter().chain(d.map(|(s, _)| s)) {
                    p.pool.abandon(s);
                }
                p.begun = None;
                p.deferred = None;
                p.pool.begin(dev_w, dev_h, now).ok_or(ShellError::State)?
            }
        };
        p.reap(now);
        if let std::collections::hash_map::Entry::Vacant(e) = p.bufs.entry(slot.id) {
            let (shm, rqh) = (
                self.main.shm.as_ref().ok_or(ShellError::State)?,
                self.main.reader_qh.as_ref().ok_or(ShellError::State)?,
            );
            let buf = ShmBuf::create(
                shm,
                &self.main_queue.handle(),
                rqh,
                dev_w,
                dev_h,
                (present.clone(), slot.id),
            )
            .map_err(|_| ShellError::NoMem)?;
            e.insert(buf);
        }
        p.begun = Some(slot);
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
        let mut all = p.deferred.take().map(|(_, d)| d).unwrap_or_default();
        all.extend_from_slice(damage);
        let all = merged(&all);
        if p.cb_pending > 0 && !p.force {
            p.deferred = Some((slot, all));
            p.begun = None;
            return Ok(());
        }
        p.force = false;
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
        let rqh = self.main.reader_qh.as_ref().ok_or(ShellError::State)?;
        let feedback = self.main.presentation.is_some();
        surface.frame(
            rqh,
            FrameTag {
                window: id,
                present: present.clone(),
                feedback,
            },
        );
        if let Some(pres) = &self.main.presentation {
            pres.feedback(surface, rqh, id);
        }
        surface.commit();
        p.cb_pending += 1;
        p.pool.submitted(slot, &all);
        drop(p);
        self.flush()
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

    /// `ack_configure` ran: the next commit of the window must be made even while a frame callback is pending.
    pub(super) fn present_force_next(&mut self, id: u64) {
        let p = self.main.present_of(id);
        lock(&p).force = true;
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

impl Dispatch<WlCallback, FrameTag> for ReaderState {
    fn event(
        state: &mut Self,
        _: &WlCallback,
        event: wl_callback::Event,
        tag: &FrameTag,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_callback::Event::Done { callback_data } = event {
            {
                let mut p = lock(&tag.present);
                p.cb_pending = p.cb_pending.saturating_sub(1);
            }
            if !tag.feedback {
                // no presentation-time: the callback's own time (milliseconds)
                let t = u64::from(callback_data) * 1_000_000;
                frame_done(state, tag.window, t, 0, 0);
            }
        }
    }
}

impl Dispatch<WpPresentationFeedback, u64> for ReaderState {
    fn event(
        state: &mut Self,
        _: &WpPresentationFeedback,
        event: wp_presentation_feedback::Event,
        window: &u64,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            wp_presentation_feedback::Event::Presented {
                tv_sec_hi,
                tv_sec_lo,
                tv_nsec,
                refresh,
                ..
            } => {
                let secs = (u64::from(tv_sec_hi) << 32) | u64::from(tv_sec_lo);
                let t = secs * 1_000_000_000 + u64::from(tv_nsec);
                frame_done(state, *window, t, t, refresh);
            }
            wp_presentation_feedback::Event::Discarded => {
                frame_done(state, *window, 0, 0, 0);
            }
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
        }
    }
}

delegate_noop!(MainState: WlShmPool);
delegate_noop!(MainState: ignore WpPresentation);

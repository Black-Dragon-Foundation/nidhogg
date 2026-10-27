//! The presenter: `present_begin` hands out a buffer of the window's pool, `present_commit` shows it.
//!
//! A commit attaches the buffer, damages it (buffer coordinates), sets the viewport destination to the logical size and
//! `buffer_scale` 1 (the integer scale without a viewporter), requests a frame callback and presentation feedback and
//! commits.  Pacing is the host's: it waits for `FRAME_DONE` before it presents again, and every `present_commit` is a
//! commit.  Frame callbacks, presentation feedback and buffer releases run on the reader queue; the first two post
//! `FRAME_DONE`, and a release posts `BUFFER_FREE` when a `present_begin` found every buffer busy.  The policy is
//! `glitnir::pool`.

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
    window: u64,
    pool: Pool,
    bufs: HashMap<usize, ShmBuf>,
    /// The slot `present_begin` handed out and nothing committed yet.
    begun: Option<BufferSlot>,
    /// A `present_begin` found every buffer busy: the next release posts `BUFFER_FREE`.
    starved: bool,
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
    fn new(window: u64) -> Present {
        Present {
            window,
            pool: Pool::new(),
            bufs: HashMap::new(),
            begun: None,
            starved: false,
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
    }
}

impl MainState {
    fn present_of(&mut self, id: u64) -> Arc<Mutex<Present>> {
        self.presents
            .entry(id)
            .or_insert_with(|| Arc::new(Mutex::new(Present::new(id))))
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
        let slot = match p.begun.take() {
            // begun twice without a commit: the same buffer
            Some(s) if (s.w, s.h) == (dev_w, dev_h) => s,
            b => {
                if let Some(s) = b {
                    p.pool.abandon(s);
                }
                let Some(s) = p.pool.begin(dev_w, dev_h, now) else {
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
        let all = merged(damage);
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
        // one source of FRAME_DONE per commit: presentation feedback, else a frame callback
        let rqh = self.main.reader_qh.as_ref().ok_or(ShellError::State)?;
        if let Some(pres) = &self.main.presentation {
            pres.feedback(surface, rqh, id);
        } else {
            surface.frame(rqh, id);
        }
        surface.commit();
        p.pool.submitted(slot);
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

/// A frame callback (requested only without `wp_presentation`); its user data is the window.
impl Dispatch<WlCallback, u64> for ReaderState {
    fn event(
        state: &mut Self,
        _: &WlCallback,
        event: wl_callback::Event,
        window: &u64,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        // no presentation-time: the callback's own time (milliseconds)
        if let wl_callback::Event::Done { callback_data } = event {
            frame_done(state, *window, u64::from(callback_data) * 1_000_000, 0, 0);
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
        state: &mut Self,
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
            // the host owes a frame that found no buffer: tell it one is free
            if std::mem::take(&mut p.starved) {
                let window = p.window;
                drop(p);
                state.post(NhShellEvent::new(NHSHELL_EV_BUFFER_FREE, window, 0));
            }
        }
    }
}

delegate_noop!(MainState: WlShmPool);
delegate_noop!(MainState: ignore WpPresentation);

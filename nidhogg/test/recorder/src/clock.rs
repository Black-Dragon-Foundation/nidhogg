//! The frame clock: frame callbacks and presentation feedback, fired on a real-time tick (`--clock auto`) or by the
//! `present` command (`--clock manual`), and the pacing modes that make a real compositor's habits visible:
//! `damage_only` (feedback of a commit without damage is never answered), `discard` (a later commit discards
//! feedback still waiting), `hold_buffers N` (the N latest replaced buffers stay unreleased until `release`), and
//! `occlude` (callbacks and feedback wait while the output is hidden).  Profiles set the start values, the `mode`
//! command changes them.
use crate::control::err;
use crate::log::HeldWrite;
use crate::state::{Clock, State};
use serde_json::{json, Value};
use smithay::backend::renderer::utils::{Buffer, RendererSurfaceStateUserData};
use smithay::delegate_presentation;
use smithay::reexports::wayland_protocols::wp::presentation_time::server::wp_presentation_feedback::Kind;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::reexports::wayland_server::backend::ObjectId;
use smithay::reexports::wayland_server::Resource;
use smithay::utils::{Clock as SmClock, Monotonic};
use smithay::wayland::compositor::{with_states, SurfaceAttributes};
use smithay::wayland::shm::with_buffer_contents;
use smithay::wayland::presentation::{
    PresentationFeedbackCachedState, PresentationFeedbackCallback, Refresh,
};
use std::collections::{HashMap, VecDeque};
use std::time::Duration;

delegate_presentation!(State);

/// One presentation feedback waiting for its answer, and whether its commit carried damage.
struct Waiting {
    cb: PresentationFeedbackCallback,
    damaged: bool,
}

/// The pacing modes and what they hold back.
#[derive(Default)]
pub struct Pacing {
    pub damage_only: bool,
    pub discard: bool,
    pub hold: usize,
    /// The profile allows `occlude`.
    pub occlusion: bool,
    pub occluded: bool,
    held: VecDeque<Buffer>,
    feedback: HashMap<ObjectId, Vec<Waiting>>,
    /// The refresh rate the output reports (millihertz) and its period: the tick and the feedback's refresh.
    pub refresh_mhz: i32,
    pub frame: Duration,
    /// The buffer each surface shows, checksummed at its attach (see `held_attach`).
    live: HashMap<ObjectId, Attach>,
    /// Replaced buffers still in `held`: checked when they leave it.
    limbo: Vec<Attach>,
}

/// One attach of a buffer: where, when, and the checksum of its bytes inside the viewport source.
struct Attach {
    surface: u32,
    buffer: ObjectId,
    seq: u64,
    src: Option<[f64; 4]>,
    sum: Option<u64>,
    /// Kept only while the buffer is shown, so that dropping it is what releases the buffer.
    buf: Option<Buffer>,
}

/// A fast non-cryptographic hash of the rows of `buf` inside `src` (buffer pixels; null = the whole buffer).
fn checksum(buf: &Buffer, src: Option<[f64; 4]>) -> Option<u64> {
    with_buffer_contents(buf, |ptr, len, d| {
        let (w, h, stride) = (
            d.width.max(0) as usize,
            d.height.max(0) as usize,
            d.stride.max(0) as usize,
        );
        let [x0, y0, x1, y1] = match src {
            Some([x, y, sw, sh]) => [
                (x.floor().max(0.0) as usize).min(w),
                (y.floor().max(0.0) as usize).min(h),
                ((x + sw).ceil().max(0.0) as usize).min(w),
                ((y + sh).ceil().max(0.0) as usize).min(h),
            ],
            None => [0, 0, w, h],
        };
        // SAFETY: Smithay guarantees `ptr` maps `len` bytes of the client's pool for the closure's duration
        let bytes = unsafe { std::slice::from_raw_parts(ptr, len) };
        let mut hash = 0xcbf2_9ce4_8422_2325u64;
        for y in y0..y1 {
            let start = d.offset.max(0) as usize + y * stride + x0 * 4;
            let row = bytes.get(start..start + x1.saturating_sub(x0) * 4)?;
            for chunk in row.chunks(8) {
                let mut word = [0u8; 8];
                word[..chunk.len()].copy_from_slice(chunk);
                hash = (hash ^ u64::from_le_bytes(word)).wrapping_mul(0x0000_0100_0000_01b3);
                hash ^= hash >> 29;
            }
        }
        Some(hash)
    })
    .ok()
    .flatten()
}

impl Pacing {
    pub fn from_profile(p: &crate::state::Profile, refresh_mhz: i32) -> Pacing {
        Pacing {
            damage_only: p.frames == "damage-only",
            discard: p.discard_on_supersede,
            hold: p.hold_buffers,
            occlusion: p.occlusion,
            refresh_mhz,
            frame: Duration::from_nanos(1_000_000_000_000 / refresh_mhz.max(1) as u64),
            ..Pacing::default()
        }
    }

    /// Take the held buffers beyond `hold` (the oldest first); the caller passes them to `buffers_released`.
    fn trim(&mut self) -> Vec<Buffer> {
        let mut out = Vec::new();
        while self.held.len() > self.hold {
            out.extend(self.held.pop_front());
        }
        out
    }
}

/// The buffer a surface shows now (the renderer state keeps it until the next commit replaces it).
fn current_buffer(surface: &WlSurface) -> Option<Buffer> {
    with_states(surface, |s| {
        s.data_map
            .get::<RendererSurfaceStateUserData>()
            .and_then(|d| d.lock().unwrap().buffer().cloned())
    })
}

impl State {
    /// Before a commit is applied: remember the buffer it may replace.
    pub fn pacing_before_commit(&self, surface: &WlSurface) -> Option<Buffer> {
        if self.pacing.hold == 0 {
            return None;
        }
        current_buffer(surface)
    }

    /// After a commit was applied: hold the buffer it replaced, take the commit's feedback into the waiting list,
    /// and discard what a later commit supersedes.
    pub fn pacing_after_commit(&mut self, surface: &WlSurface, old: Option<Buffer>, damaged: bool) {
        if let Some(old) = old {
            let replaced = current_buffer(surface).is_none_or(|new| *new != *old);
            if replaced {
                self.pacing.held.push_back(old);
            }
        }
        let popped = self.pacing.trim();
        self.buffers_released(popped);
        let fresh = with_states(surface, |s| {
            std::mem::take(
                &mut s
                    .cached_state
                    .get::<PresentationFeedbackCachedState>()
                    .current()
                    .callbacks,
            )
        });
        let discard = self.pacing.discard && (damaged || !fresh.is_empty());
        let list = self.pacing.feedback.entry(surface.id()).or_default();
        if discard {
            for w in list.drain(..) {
                w.cb.discarded();
            }
        }
        list.extend(fresh.into_iter().map(|cb| Waiting { cb, damaged }));
    }

    pub fn pacing_surface_gone(&mut self, surface: &WlSurface) {
        self.pacing.feedback.remove(&surface.id());
        self.pacing.live.remove(&surface.id());
    }

    /// A commit of `surface` (the log's next seq is `seq`) was applied: when it attached a buffer (or unmapped the
    /// surface) the buffer shown before is released, so its bytes are compared with those at its attach; a newly
    /// attached buffer is checksummed over the viewport source `src`.
    pub fn held_attach(
        &mut self,
        surface: &WlSurface,
        attached: bool,
        src: Option<[f64; 4]>,
        seq: u64,
    ) {
        let id = surface.id();
        let cur = current_buffer(surface);
        if let Some(old) = self.pacing.live.remove(&id) {
            if attached || cur.is_none() {
                self.held_retire(old);
            } else {
                self.pacing.live.insert(id.clone(), old);
            }
        }
        if let (true, Some(buf)) = (attached, cur) {
            let sum = checksum(&buf, src);
            self.pacing.live.insert(
                id,
                Attach {
                    surface: surface.id().protocol_id(),
                    buffer: buf.id(),
                    seq,
                    src,
                    sum,
                    buf: Some(buf),
                },
            );
        }
    }

    /// The surface no longer shows `old`'s buffer: it is released now, or when it leaves the held queue.
    fn held_retire(&mut self, mut old: Attach) {
        if self.pacing.held.iter().any(|b| b.id() == old.buffer) {
            old.buf = None;
            self.pacing.limbo.push(old);
        } else if let Some(buf) = old.buf.take() {
            self.held_check(&old, &buf);
        }
    }

    /// Buffers left the held queue (the client hears `release`): compare each one's bytes with those at its attach.
    pub fn buffers_released(&mut self, bufs: Vec<Buffer>) {
        for buf in bufs {
            if let Some(i) = self.pacing.limbo.iter().position(|a| a.buffer == buf.id()) {
                let a = self.pacing.limbo.remove(i);
                self.held_check(&a, &buf);
            }
        }
    }

    fn held_check(&mut self, a: &Attach, buf: &Buffer) {
        if a.sum.is_some() && checksum(buf, a.src) != a.sum {
            self.log.push_write(HeldWrite {
                surface: a.surface,
                buffer: a.buffer.protocol_id(),
                attached_seq: a.seq,
                released_seq: self.log.next_seq(),
            });
        }
    }

    /// Fire every pending frame callback and presentation feedback.  In manual mode the clock first moves forward
    /// by `advance_ns`; in auto mode the time is the real monotonic clock.  While occluded nothing is answered.
    pub fn present(&mut self, advance_ns: u64) {
        let now = match self.clock {
            Clock::Manual => {
                self.manual_ns += advance_ns;
                self.mono_base + Duration::from_nanos(self.manual_ns)
            }
            Clock::Auto => Duration::from(SmClock::<Monotonic>::new().now()),
        };
        if self.pacing.occluded {
            return;
        }
        self.present_seq += 1;
        let seq = self.present_seq;
        self.surfaces.retain(|s| s.is_alive());
        for surface in &self.surfaces {
            let frames = with_states(surface, |states| {
                std::mem::take(
                    &mut states
                        .cached_state
                        .get::<SurfaceAttributes>()
                        .current()
                        .frame_callbacks,
                )
            });
            for cb in frames {
                cb.done(now.as_millis() as u32);
            }
            let damage_only = self.pacing.damage_only;
            if let Some(list) = self.pacing.feedback.get_mut(&surface.id()) {
                let (shown, kept): (Vec<_>, Vec<_>) = std::mem::take(list)
                    .into_iter()
                    .partition(|w| w.damaged || !damage_only);
                *list = kept;
                for w in shown {
                    w.cb.presented(
                        &self.output,
                        now,
                        Refresh::fixed(self.pacing.frame),
                        seq,
                        Kind::Vsync,
                    );
                }
            }
        }
    }

    /// The periodic tick: auto clock frames, and ping timeouts.
    pub fn tick(&mut self) {
        if self.clock == Clock::Auto {
            self.present(0);
        }
        self.expire_pings();
    }
}

/// `{"cmd":"present","advance_ns":N}`
pub fn present(state: &mut State, req: &Value) -> Value {
    if state.clock != Clock::Manual {
        return err("present needs --clock manual");
    }
    let advance = req.get("advance_ns").and_then(Value::as_u64).unwrap_or(0);
    state.present(advance);
    json!({"ok": true})
}

/// `{"cmd":"mode","frames":"all"|"damage-only","discard":bool,"hold_buffers":N}`: change the pacing modes
/// (each field optional).  Lowering `hold_buffers` releases at once.
pub fn mode(state: &mut State, req: &Value) -> Value {
    if let Some(f) = req.get("frames").and_then(Value::as_str) {
        match f {
            "all" | "damage-only" => state.pacing.damage_only = f == "damage-only",
            _ => return err("frames is \"all\" or \"damage-only\""),
        }
    }
    if let Some(d) = req.get("discard").and_then(Value::as_bool) {
        state.pacing.discard = d;
    }
    if let Some(n) = req.get("hold_buffers").and_then(Value::as_u64) {
        state.pacing.hold = n as usize;
        let popped = state.pacing.trim();
        state.buffers_released(popped);
    }
    json!({"ok": true})
}

/// `{"cmd":"occlude"}` / `{"cmd":"unocclude"}`: callbacks and feedback wait while occluded.  Needs a profile with
/// `"occlusion": true`.
pub fn occlude(state: &mut State, on: bool) -> Value {
    if !state.pacing.occlusion {
        return err("the profile has no occlusion");
    }
    state.pacing.occluded = on;
    json!({"ok": true})
}

/// `{"cmd":"release"}`: drop every held buffer (the client hears wl_buffer.release).
pub fn release(state: &mut State) -> Value {
    let n = state.pacing.held.len();
    let drained: Vec<Buffer> = state.pacing.held.drain(..).collect();
    state.buffers_released(drained);
    json!({"ok": true, "released": n})
}

/// `{"cmd":"outstanding"}`: what is not yet answered: frame callbacks and presentation feedback committed, and
/// buffers held.
pub fn outstanding(state: &mut State) -> Value {
    state.surfaces.retain(|s| s.is_alive());
    let callbacks: usize = state
        .surfaces
        .iter()
        .map(|s| {
            with_states(s, |st| {
                st.cached_state
                    .get::<SurfaceAttributes>()
                    .current()
                    .frame_callbacks
                    .len()
            })
        })
        .sum();
    let feedback: usize = state.pacing.feedback.values().map(Vec::len).sum();
    json!({"ok": true, "callbacks": callbacks, "feedback": feedback, "buffers": state.pacing.held.len(),
           "occluded": state.pacing.occluded})
}

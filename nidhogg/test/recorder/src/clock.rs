//! The frame clock: frame callbacks and presentation feedback, fired on a real-time tick (`--clock auto`) or by the
//! `present` command (`--clock manual`).
use crate::control::err;
use crate::state::{Clock, State};
use serde_json::{json, Value};
use smithay::delegate_presentation;
use smithay::reexports::wayland_protocols::wp::presentation_time::server::wp_presentation_feedback::Kind;
use smithay::reexports::wayland_server::Resource;
use smithay::utils::{Clock as SmClock, Monotonic};
use smithay::wayland::compositor::{with_states, SurfaceAttributes};
use smithay::wayland::presentation::{PresentationFeedbackCachedState, Refresh};
use std::time::Duration;

/// The refresh period the output reports.
pub const FRAME: Duration = Duration::from_nanos(16_666_667);

delegate_presentation!(State);

impl State {
    /// Fire every pending frame callback and presentation feedback.  In manual mode the clock first moves forward
    /// by `advance_ns`; in auto mode the time is the real monotonic clock.
    pub fn present(&mut self, advance_ns: u64) {
        let now = match self.clock {
            Clock::Manual => {
                self.manual_ns += advance_ns;
                self.mono_base + Duration::from_nanos(self.manual_ns)
            }
            Clock::Auto => Duration::from(SmClock::<Monotonic>::new().now()),
        };
        self.present_seq += 1;
        let seq = self.present_seq;
        self.surfaces.retain(|s| s.is_alive());
        for surface in &self.surfaces {
            let (frames, feedback) = with_states(surface, |states| {
                let frames = std::mem::take(
                    &mut states
                        .cached_state
                        .get::<SurfaceAttributes>()
                        .current()
                        .frame_callbacks,
                );
                let feedback = std::mem::take(
                    &mut states
                        .cached_state
                        .get::<PresentationFeedbackCachedState>()
                        .current()
                        .callbacks,
                );
                (frames, feedback)
            });
            for cb in frames {
                cb.done(now.as_millis() as u32);
            }
            for fb in feedback {
                fb.presented(&self.output, now, Refresh::fixed(FRAME), seq, Kind::Vsync);
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

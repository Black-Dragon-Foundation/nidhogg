//! Reconnecting in place, the glue's part: the frames survive a compositor restart (the shell's `reconnect.rs` makes
//! the connection and the windows again).  C keeps the timer and the Lisp hooks; the glue keeps the schedule and the
//! decisions.
//!
//! - **The loss** (`RECONNECTING`): what the glue held for the old connection goes (scroll layers, the drop target's
//!   drag, touch points, owed acks; every frame counts as unconfigured, so the new compositor's first configure is a
//!   content ack, as the shell makes it), the monitor list and the keyboard's window are noted, and an
//!   `NHG_ACT_RECONNECTING` action asks C to start its timer, the first tick `code` milliseconds later.  The shell has
//!   already posted the releases, an empty preedit, each window's output leave and the drag's end; it reopens the
//!   session and holds the idle inhibitors again itself.
//! - **The backoff** (`nhg_reconnect_step`, C's timer): one step of at most `STEP_MS`, then the next tick 50, 100,
//!   200, 400, 800 ms later and every second after that; past the timeout C's host gives the display up.  A tick
//!   never sleeps beyond its step.
//! - **Back** (`RECONNECTED`): an `NHG_ACT_RECONNECTED` action (C damages every frame whole), and the glue waits for
//!   every shown top-level to present again: configured on the new connection, then published or committed.
//! - **Settled** (`nhg_reconnect_settle`, asked by C at each read): when every frame presented again, or `REPORT_WAIT`
//!   after `RECONNECTED`, C runs Lisp's `reconnected`, and the monitors hook if the list differs from before the loss
//!   (output ids aside: the new compositor names its outputs anew).  The monitor actions in between are dropped.  If no
//!   keyboard enter has come `FOCUS_WAIT` after that, the window that had the keyboard loses it then (the loss itself
//!   gives no focus-out).

use crate::action::{NHG_ACT_MONITORS, NhgAction};
use crate::display::Display;
use crate::frame::{NHG_FACT_VISIBLE, NHG_FRAME_KINDS};
use crate::present::{NHG_PRESENTED_COMMITTED, NHG_PRESENTED_PUBLISHED};
use crate::query::NhgMonitor;
use nhshell::api::ShellError;
use nhshell::event::{Record, ShellEvent};
use std::collections::BTreeSet;
use std::time::{Duration, Instant};

/// The display went and the shell is reconnecting: C starts its timer, the first tick `code` milliseconds later.  No
/// window.
pub const NHG_ACT_RECONNECTING: u32 = 42;
/// The shell is connected again: C damages every frame whole.  No window.
pub const NHG_ACT_RECONNECTED: u32 = 43;

/// `nhg_reconnect_settle` bits: every frame presented again, or `REPORT_WAIT` passed: run `reconnected`.
pub const NHG_SETTLE_REPORT: i32 = 1;
/// With `REPORT`: the monitor list differs from before the loss: run `display-monitors-changed-functions`.
pub const NHG_SETTLE_MONITORS: i32 = 2;
/// No keyboard enter came: the window that had the keyboard loses it.
pub const NHG_SETTLE_FOCUS_OUT: i32 = 4;

/// The longest step: one tick's bound on the main thread.
pub const STEP_MS: u32 = 50;
/// The backoff: the first delay, doubled at each tick up to the last.
const FIRST_MS: u32 = 50;
const LAST_MS: u32 = 1000;
/// How long after `RECONNECTED` `reconnected` waits for the frames to present.
pub const REPORT_WAIT: Duration = Duration::from_secs(2);
/// How long after `reconnected` the keyboard's window waits for an enter before it loses the focus.
pub const FOCUS_WAIT: Duration = Duration::from_secs(1);

#[derive(Debug, Default)]
enum Phase {
    #[default]
    Up,
    /// `RECONNECTING` was taken at `since`; `ticks` steps found no display.
    Reconnecting { since: Instant, ticks: u32 },
    /// The glue gave up (the timeout, or the new display failed): C drops the display.
    Gone,
    /// `RECONNECTED` was taken at `since`; the shown top-levels in `owed` have not presented again.
    Settling { since: Instant, owed: BTreeSet<u64> },
    /// `reconnected` was reported at `at`; the keyboard's window waits for an enter.
    Reported { at: Instant },
}

/// The glue's reconnect state for one display.
#[derive(Debug, Default)]
pub struct Reconnect {
    phase: Phase,
    /// The monitor list before the loss, without the ids.
    before: Vec<Monitor>,
    /// The window that had the keyboard at the loss (0: none of ours).
    focus: u64,
    /// A keyboard enter came since the loss.
    entered: bool,
}

/// An output as Lisp sees it: everything but the id.
type Monitor = (i32, i32, i32, i32, i32, i32, u32, [std::ffi::c_char; 32]);

fn outputs(all: &[NhgMonitor]) -> Vec<Monitor> {
    let mut v: Vec<Monitor> = all
        .iter()
        .map(|m| (m.x, m.y, m.w, m.h, m.mm_w, m.mm_h, m.scale120, m.name))
        .collect();
    v.sort_unstable();
    v
}

impl Reconnect {
    /// The display is given up (`Display::give_up`): every later step says so, and nothing is left to settle.
    pub(crate) fn give_up(&mut self) {
        self.phase = Phase::Gone;
    }
}

/// The next tick's delay after `ticks` steps that found no display.
fn backoff(ticks: u32) -> u32 {
    FIRST_MS
        .checked_shl(ticks)
        .map_or(LAST_MS, |d| d.min(LAST_MS))
}

/// `RECONNECTING`: forget what the old connection had, note what `reconnected` compares, and ask C for the timer.  A
/// loss before an earlier reconnect settled keeps the earlier notes.
pub fn lost(state: &mut Display, ev: &Record) {
    let r = &mut state.reconnect;
    if matches!(r.phase, Phase::Up) {
        r.before = outputs(state.monitors.all());
        r.focus = state.keyboard.window;
    }
    r.entered = false;
    r.phase = Phase::Reconnecting {
        since: Instant::now(),
        ticks: 0,
    };
    crate::layers::drop_all(&mut state.backings);
    state.dnd.lost();
    state.touch.clear();
    state.acks_due.clear();
    for f in state.frames_mut() {
        f.cfg.configured = false;
        f.ack = None;
    }
    let mut a = NhgAction::new(NHG_ACT_RECONNECTING, 0, ev.time_ns);
    a.code = backoff(0) as i32;
    state.actions.push(a);
}

/// `RECONNECTED`: the shown top-levels owe a presentation, and C damages every frame.
pub fn reconnected(state: &mut Display, ev: &Record) {
    let owed = state
        .frames()
        .filter(|(_, f)| f.flags & NHG_FRAME_KINDS == 0 && f.facts.flags & NHG_FACT_VISIBLE != 0)
        .map(|(w, _)| w)
        .collect();
    state.reconnect.phase = Phase::Settling {
        since: Instant::now(),
        owed,
    };
    state
        .actions
        .push(NhgAction::new(NHG_ACT_RECONNECTED, 0, ev.time_ns));
}

/// After each event's actions: a keyboard enter is noted, and the monitor actions wait until `reconnected` (the
/// outputs of the old connection go and the new ones come; `settle` compares the lists once).
pub fn after(state: &mut Display, ev: &Record) {
    let r = &mut state.reconnect;
    if matches!(r.phase, Phase::Up) {
        return;
    }
    let focus_in = matches!(ev.event, ShellEvent::Focus(f) if f.r#in != 0);
    if focus_in {
        r.entered = true;
    }
    let output = matches!(
        ev.event,
        ShellEvent::OutputAdd(_) | ShellEvent::OutputChange(_) | ShellEvent::OutputRemove(_)
    );
    let quiet = matches!(r.phase, Phase::Reconnecting { .. } | Phase::Settling { .. });
    if output
        && quiet
        && state
            .actions
            .as_slice()
            .iter()
            .all(|a| a.kind == NHG_ACT_MONITORS)
    {
        state.actions.clear();
    }
}

/// `nhg_reconnect_step`: one tick, at most `STEP_MS`, given up `timeout_ms` after the loss.  `(NHG_OK, 0)` once
/// connected (`RECONNECTED` comes out of the drain's next steps), `(NHG_E_TIMEOUT, ms)` with the next tick's delay
/// (never past the timeout), `(NHG_E_DISCONNECTED, 0)` when the glue gave up: the timeout passed, or the new display
/// failed (the shell then queues `DISCONNECTED` too).  `NHG_OK` while not reconnecting.
pub fn step(state: &mut Display, timeout_ms: u32) -> (i32, u32) {
    use crate::abi::{NHG_E_DISCONNECTED, NHG_E_TIMEOUT, NHG_OK};
    let (since, ticks) = match state.reconnect.phase {
        Phase::Reconnecting { since, ticks } => (since, ticks),
        Phase::Gone => return (NHG_E_DISCONNECTED, 0),
        _ => return (NHG_OK, 0),
    };
    let timeout = Duration::from_millis(u64::from(timeout_ms));
    let left_ms = || {
        let left = timeout.saturating_sub(since.elapsed());
        u32::try_from(left.as_millis()).unwrap_or(u32::MAX)
    };
    if left_ms() == 0 {
        state.reconnect.phase = Phase::Gone;
        return (NHG_E_DISCONNECTED, 0);
    }
    match state.backend().reconnect(left_ms().clamp(1, STEP_MS)) {
        Ok(()) => (NHG_OK, 0),
        // not there yet (`State`: the shell has not seen its loss taken, which the drain did; try again)
        Err(ShellError::Timeout | ShellError::State) => {
            let ticks = ticks + 1;
            state.reconnect.phase = Phase::Reconnecting { since, ticks };
            (NHG_E_TIMEOUT, backoff(ticks).min(left_ms().max(1)))
        }
        Err(_) => {
            state.reconnect.phase = Phase::Gone;
            (NHG_E_DISCONNECTED, 0)
        }
    }
}

/// `nhg_present` answered `bits` for root `root`: a top-level configured on the new connection that published or
/// committed has presented again.
pub fn presented(state: &mut Display, root: u64, bits: i32) {
    if bits & (NHG_PRESENTED_PUBLISHED | NHG_PRESENTED_COMMITTED) == 0
        || !state.frame(root).is_ok_and(|f| f.cfg.configured)
    {
        return;
    }
    if let Phase::Settling { owed, .. } = &mut state.reconnect.phase {
        owed.remove(&root);
    }
}

/// What C does now (`NHG_SETTLE_*`), the window that loses the keyboard with `FOCUS_OUT`, and in how many
/// milliseconds C asks again (0: nothing is pending, at `now`).
pub fn settle(state: &mut Display, now: Instant) -> (i32, u64, u32) {
    let ms = |d: Duration| u32::try_from(d.as_millis()).unwrap_or(u32::MAX).max(1);
    let mut bits = 0;
    if let Phase::Settling { since, owed } = &state.reconnect.phase {
        let due = *since + REPORT_WAIT;
        // a frame deleted meanwhile owes nothing
        if owed.iter().any(|&w| state.frame(w).is_ok()) && now < due {
            return (0, 0, ms(due - now));
        }
        bits |= NHG_SETTLE_REPORT;
        if outputs(state.monitors.all()) != state.reconnect.before {
            bits |= NHG_SETTLE_MONITORS;
        }
        state.reconnect.phase = Phase::Reported { at: now };
    }
    let Phase::Reported { at } = state.reconnect.phase else {
        return (bits, 0, 0);
    };
    let r = &mut state.reconnect;
    if r.entered || r.focus == 0 {
        r.phase = Phase::Up;
        return (bits, 0, 0);
    }
    if now < at + FOCUS_WAIT {
        return (bits, 0, ms(at + FOCUS_WAIT - now));
    }
    let w = std::mem::take(&mut r.focus);
    r.phase = Phase::Up;
    if state.frame(w).is_err() {
        return (bits, 0, 0);
    }
    if state.keyboard.window == w {
        state.keyboard.window = 0;
    }
    (bits | NHG_SETTLE_FOCUS_OUT, w, 0)
}

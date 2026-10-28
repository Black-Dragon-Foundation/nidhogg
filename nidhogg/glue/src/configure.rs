//! Configures and scale: a toplevel's configure becomes a `RESIZE` action and an ack, and a new preferred scale a
//! `SCALE` action.  Moved from `nhwl_configure` and `nhwl_scale_changed` (`src/nhwlterm.c`); what reads Emacs stays
//! there (the `fullscreen` parameter, the title bar's layout, the frame's new size, the fonts).
//!
//! The ack (present-path S2's rule).  It carries `NHSHELL_ACK_CONTENT` (only a buffer carries it) for the window's
//! first configure, under a client-side title bar for a change of activated, maximized or tiled, and when the
//! configure gives the frame a new size.  Whether it does is C's to say: the size the suggestion gives depends on the
//! title bar's layout, which C recomputes from the new states when it applies `RESIZE` (review B1's rule: a decision
//! that reads state an earlier action changed is made where that action is applied).  C says it by pushing the new
//! size (`nhg_frame_resized`) while it applies `RESIZE`, and the ack is recorded then, as content; otherwise it is
//! recorded at the start of the next `nhg_next`, before any other event is taken.

use crate::action::{NHG_ACT_RESIZE, NHG_ACT_SCALE, NHG_RESIZE_FULLSCREEN, NhgAction};
use crate::display::Display;
use crate::frame::{Ack, Configure, NHG_FACT_CSD};
use crate::guard::GlueError;
use nhshell::api::NHSHELL_ACK_CONTENT;
use nhshell::event::*;

/// The states whose change a client-side title bar shows (its buttons, corners and margins).
const CSD_STATES: u32 = NHSHELL_STATE_ACTIVATED
    | NHSHELL_STATE_MAXIMIZED
    | NHSHELL_STATE_TILED_L
    | NHSHELL_STATE_TILED_R
    | NHSHELL_STATE_TILED_T
    | NHSHELL_STATE_TILED_B;

/// The states the frame's `fullscreen` parameter shows.
const FULLSCREEN_STATES: u32 = NHSHELL_STATE_FULLSCREEN | NHSHELL_STATE_MAXIMIZED;

/// A `CONFIGURE` of a registered window: its state, its ack (due), and a `RESIZE` action.  Nothing for a window the
/// glue does not know.
pub fn configure(state: &mut Display, ev: &NhShellEvent) {
    // SAFETY: a CONFIGURE event carries `configure`.
    let c = unsafe { ev.u.configure };
    let Ok(f) = state.frame_mut(ev.window) else {
        return;
    };
    let old = f.cfg.states;
    let content = !f.cfg.configured
        || (f.facts.flags & NHG_FACT_CSD != 0 && (old ^ c.states) & CSD_STATES != 0);
    f.cfg = Configure {
        configured: true,
        logical_w: c.w,
        logical_h: c.h,
        serial: c.serial,
        states: c.states,
    };
    f.ack = Some(Ack {
        serial: c.serial,
        content,
    });
    let mut a = NhgAction::new(NHG_ACT_RESIZE, ev.window, ev.time_ns);
    a.w = c.w;
    a.h = c.h;
    a.code = c.states as i32;
    if (old ^ c.states) & FULLSCREEN_STATES != 0 {
        a.flags |= NHG_RESIZE_FULLSCREEN;
    }
    state.acks_due.push(ev.window);
    state.actions.push(a);
}

/// A `SCALE` of a registered window: a `SCALE` action when the scale is new (0, or the one C applied last, is none).
pub fn scale(state: &mut Display, ev: &NhShellEvent) {
    // SAFETY: a SCALE event carries `scale`.
    let s = unsafe { ev.u.scale }.scale120;
    let Ok(f) = state.frame_mut(ev.window) else {
        return;
    };
    if s == 0 || s == f.scale120 {
        return;
    }
    f.scale120 = s;
    let mut a = NhgAction::new(NHG_ACT_SCALE, ev.window, ev.time_ns);
    a.code = s as i32;
    a.scale = f64::from(s) / 120.0;
    state.actions.push(a);
}

/// Record `window`'s due ack with the shell, if it has one.  The shell sends it with the commit that carries it and
/// wakes the pacer itself.
fn record(state: &mut Display, window: u64) {
    let Ok(f) = state.frame_mut(window) else {
        return;
    };
    let Some(ack) = f.ack.take() else {
        return;
    };
    let flags = if ack.content { NHSHELL_ACK_CONTENT } else { 0 };
    f.last_ack = Some((ack.serial, flags));
    // as C did, an error is not the drain's: a lost connection reaches C as the next event
    let _ = state.backend().ack_configure(window, ack.serial, flags);
}

/// Record every due ack (the start of `nhg_next`).
pub fn record_due(state: &mut Display) {
    let due = std::mem::take(&mut state.acks_due);
    for &w in &due {
        record(state, w);
    }
    // keep the allocation
    state.acks_due = due;
    state.acks_due.clear();
}

/// C pushed `window`'s native size (it applied a `RESIZE` that changed it, or it resized the frame itself, or it
/// made the window).  A configure being applied gets a content ack, recorded now.
pub fn resized(state: &mut Display, window: u64, w: i32, h: i32) -> Result<(), GlueError> {
    if w <= 0 || h <= 0 {
        return Err(GlueError::Arg);
    }
    let f = state.frame_mut(window)?;
    f.facts.native_w = w;
    f.facts.native_h = h;
    if let Some(ack) = f.ack.as_mut() {
        ack.content = true;
        record(state, window);
    }
    Ok(())
}

/// C pushed the scale it gave `window` (when it made the window).
pub fn set_scale(state: &mut Display, window: u64, scale120: u32) -> Result<(), GlueError> {
    state.frame_mut(window)?.scale120 = scale120;
    Ok(())
}

/// C pushed whether `window`'s frame draws its own title bar.
pub fn set_csd(state: &mut Display, window: u64, on: bool) -> Result<(), GlueError> {
    let f = state.frame_mut(window)?;
    if on {
        f.facts.flags |= NHG_FACT_CSD;
    } else {
        f.facts.flags &= !NHG_FACT_CSD;
    }
    Ok(())
}

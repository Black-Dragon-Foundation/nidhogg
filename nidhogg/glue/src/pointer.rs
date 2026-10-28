//! The pointer's actions: `MOTION` (an enter is a motion), `LEAVE` and `CLICK`, for a window the glue knows.  Moved
//! from the C drain's pointer cases (`src/nhwlterm.c`, adapted from pgtk's `motion_notify_event`,
//! `enter_notify_event`, `leave_notify_event` and `button_event`, emacs-31.1).
//!
//! The title bar sees an event first (`csd::pointer`), and what it does not consume comes here, after the title bar's
//! own action, as the event's last.  The position is the shell's: device pixels in the window's surface; the offset
//! of the frame's native area in it (`nhg_frame_offsets`) and the mouse highlight read live Emacs state and stay C.

use crate::action::{NHG_ACT_CLICK, NHG_ACT_LEAVE, NHG_ACT_MOTION, NhgAction};
use crate::display::Display;
use nhshell::event::*;

/// A pointer event the title bar did not consume (`csd::pointer`, which comes first and may have pushed its own
/// action): the event's own action, which is its last.
pub fn event(state: &mut Display, ev: &NhShellEvent) {
    match ev.kind {
        NHSHELL_EV_POINTER_ENTER | NHSHELL_EV_POINTER_MOTION => motion(state, ev),
        NHSHELL_EV_POINTER_LEAVE => leave(state, ev),
        _ => button(state, ev),
    }
}

/// A `POINTER_ENTER` or `POINTER_MOTION`: a `MOTION` action, `dx` and `dy` the position in device pixels.
fn motion(state: &mut Display, ev: &NhShellEvent) {
    if state.frame(ev.window).is_err() {
        return;
    }
    // SAFETY: a pointer record's valid member is `pointer`.
    let p = unsafe { ev.u.pointer };
    let mut a = NhgAction::new(NHG_ACT_MOTION, ev.window, ev.time_ns);
    a.dx = p.x;
    a.dy = p.y;
    state.actions.push(a);
}

/// A `POINTER_LEAVE`: a `LEAVE` action.
fn leave(state: &mut Display, ev: &NhShellEvent) {
    if state.frame(ev.window).is_ok() {
        state
            .actions
            .push(NhgAction::new(NHG_ACT_LEAVE, ev.window, ev.time_ns));
    }
}

/// A `BUTTON`: a `CLICK` action, `code` the evdev button, `flags` the `NHSHELL_BUTTON_*` state, `dx` and `dy` the
/// position, `mods` the keyboard's.
fn button(state: &mut Display, ev: &NhShellEvent) {
    if state.frame(ev.window).is_err() {
        return;
    }
    // SAFETY: a pointer record's valid member is `pointer`.
    let p = unsafe { ev.u.pointer };
    let mut a = NhgAction::new(NHG_ACT_CLICK, ev.window, ev.time_ns);
    a.code = p.button as i32;
    a.flags = p.state;
    a.dx = p.x;
    a.dy = p.y;
    a.mods = state.keyboard().mods;
    state.actions.push(a);
}

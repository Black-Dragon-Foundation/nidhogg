//! The pointer's actions: `MOTION` (an enter is a motion), `LEAVE` and `CLICK`, for a window the glue knows.  Moved
//! from the C drain's pointer cases (`src/nhwlterm.c`, adapted from pgtk's `motion_notify_event`,
//! `enter_notify_event`, `leave_notify_event` and `button_event`, emacs-31.1).
//!
//! The title bar sees an event first (`csd::pointer`), and what it does not consume comes here, after the title bar's
//! own action, as the event's last.  The position is the shell's: device pixels in the window's surface; the offset
//! of the frame's native area in it (`nhg_frame_offsets`) and the mouse highlight read live Emacs state and stay C.

use crate::action::{NHG_ACT_CLICK, NHG_ACT_LEAVE, NHG_ACT_MOTION, NhgAction};
use crate::display::Display;
use nhshell::event::{PointerEv, Record, ShellEvent};

/// A pointer event the title bar did not consume (`csd::pointer`, which comes first and may have pushed its own
/// action): the event's own action, which is its last.
pub fn event(state: &mut Display, ev: &Record) {
    match ev.event {
        ShellEvent::PointerEnter(p) | ShellEvent::PointerMotion(p) => motion(state, ev, &p),
        ShellEvent::PointerLeave(_) => leave(state, ev),
        ShellEvent::Button(p) => button(state, ev, &p),
        _ => {}
    }
}

/// A `POINTER_ENTER` or `POINTER_MOTION`: a `MOTION` action, `dx` and `dy` the position in device pixels.
fn motion(state: &mut Display, ev: &Record, p: &PointerEv) {
    if state.frame(ev.window).is_err() {
        return;
    }
    let mut a = NhgAction::new(NHG_ACT_MOTION, ev.window, ev.time_ns);
    a.dx = p.x;
    a.dy = p.y;
    state.actions.push(a);
}

/// A `POINTER_LEAVE`: a `LEAVE` action.
fn leave(state: &mut Display, ev: &Record) {
    if state.frame(ev.window).is_ok() {
        state
            .actions
            .push(NhgAction::new(NHG_ACT_LEAVE, ev.window, ev.time_ns));
    }
}

/// The Emacs mouse button of evdev button `button` (0 left, 1 middle, 2 right, 7 back, 8 forward; -1: none).  Moved
/// from `nhwl_button_code` (`src/nhwlterm.c`).
pub fn button_code(button: u32) -> i32 {
    match button {
        0x110 => 0,         // BTN_LEFT
        0x112 => 1,         // BTN_MIDDLE
        0x111 => 2,         // BTN_RIGHT
        0x113 | 0x116 => 7, // BTN_SIDE, BTN_BACK
        0x114 | 0x115 => 8, // BTN_EXTRA, BTN_FORWARD
        _ => -1,
    }
}

/// A `BUTTON`: a `CLICK` action, `code` the Emacs button (`button_code`; -1: none, whose press still focuses), `flags`
/// the `NHSHELL_BUTTON_*` state, `dx` and `dy` the position, `mods` the keyboard's.
fn button(state: &mut Display, ev: &Record, p: &PointerEv) {
    if state.frame(ev.window).is_err() {
        return;
    }
    let mut a = NhgAction::new(NHG_ACT_CLICK, ev.window, ev.time_ns);
    a.code = button_code(p.button);
    a.flags = p.state;
    a.dx = p.x;
    a.dy = p.y;
    a.mods = state.keyboard().mods;
    state.actions.push(a);
}

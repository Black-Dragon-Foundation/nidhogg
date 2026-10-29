//! Tip frames' windows: placing a tip, making its popup, moving it when only its offsets changed, and its end, by
//! `x-hide-tip` or by the compositor.  Moved from `nhwl_tip_anchor`, `nhwl_tip_map` and `nhwl_tip_dismissed`
//! (`src/nhwlfns.c`) and the tip's part of `Fx_show_tip`; the tip frame itself (Lisp from start to end), the frame it
//! hangs on and where the pointer is stay C.
//!
//! A tip is a popup of its parent's window, below and right of a point at the pointer plus the tip's offsets (or at
//! its `left` and `top`), which the compositor flips and slides to keep it on screen; that replaces `compute_tip_xy`'s
//! fitting to the monitor (`pgtkfns.c`, emacs-31.1).

use crate::action::{NHG_ACT_DELETE, NhgAction};
use crate::backing::NhgRect;
use crate::display::Display;
use crate::frame::{NHG_FACT_VISIBLE, NHG_FRAME_TIP};
use crate::guard::GlueError;
use crate::window::{self, NhgAnchor, scale, to_logical};
use nhshell::api::{
    ChildDesc, NHSHELL_ADJUST_FLIP_X, NHSHELL_ADJUST_FLIP_Y, NHSHELL_ADJUST_SLIDE_X,
    NHSHELL_ADJUST_SLIDE_Y, NHSHELL_KIND_POPUP, NHSHELL_WIN_UNDECORATED,
};
use nhshell::event::Record;

/// `DELETE` flags: the compositor dismissed a tip's popup (its window is to go at once, the frame stays to Lisp).
pub const NHG_DEL_TIP: u32 = 1;

/// A tip grows below and right of its anchor point.
const TIP_GRAVITY: u32 = 8;
const TIP_ADJUST: u32 =
    NHSHELL_ADJUST_FLIP_X | NHSHELL_ADJUST_FLIP_Y | NHSHELL_ADJUST_SLIDE_X | NHSHELL_ADJUST_SLIDE_Y;

/// The anchor of a tip at `at` (device pixels in the parent's surface, a point at least 1 x 1) moved by `dx`, `dy`,
/// in the parent's logical pixels at scale `s120`.
fn anchor(s120: u32, at: NhgRect, dx: i32, dy: i32) -> NhgAnchor {
    NhgAnchor {
        x: to_logical(at.x.saturating_add(dx), s120),
        y: to_logical(at.y.saturating_add(dy), s120),
        w: to_logical(at.w, s120).max(1),
        h: to_logical(at.h, s120).max(1),
        gravity: TIP_GRAVITY,
        adjust: TIP_ADJUST,
    }
}

/// Show the tip of window `*window` on `parent`'s window at `at` plus `dx`, `dy`.  `*window` 0: make the tip's popup,
/// `native_w` x `native_h` device pixels at its parent's scale, registered as a visible tip frame; its id goes to
/// `*window`, and the tip is drawn whole (1).  Otherwise only the offsets changed: the popup moves (a bare commit, the
/// shell owes the reposition's ack), and 1 says the whole tip must be drawn again (below `xdg_wm_base` 3 the popup is
/// made again, and only a buffer maps it), 0 that it need not.
pub fn show(
    state: &mut Display,
    window: &mut u64,
    parent: u64,
    at: NhgRect,
    (dx, dy): (i32, i32),
    (native_w, native_h): (i32, i32),
) -> Result<i32, GlueError> {
    let s = scale(state.frame(parent)?);
    let a = anchor(s, at, dx, dy);
    if *window != 0 {
        if state.frame(*window)?.flags & NHG_FRAME_TIP == 0 {
            return Err(GlueError::State);
        }
        return Ok(i32::from(state.backend().set_position(*window, a.x, a.y)?));
    }
    if native_w <= 0 || native_h <= 0 {
        return Err(GlueError::Arg);
    }
    let id = state.backend().child_new(&ChildDesc {
        kind: NHSHELL_KIND_POPUP,
        parent,
        x: 0,
        y: 0,
        w: to_logical(native_w, s).max(1),
        h: to_logical(native_h, s).max(1),
        anchor: a,
        flags: NHSHELL_WIN_UNDECORATED,
    })?;
    // a tip has no configure the host waits for: its first buffer waits in the shell (the pacer is not ready before
    // the first configure)
    let seeded = window::seed(
        state,
        id,
        NHG_FRAME_TIP,
        NHG_FACT_VISIBLE,
        (native_w, native_h),
        s,
    );
    if let Err(e) = seeded {
        let _ = state.backend().window_destroy(id);
        return Err(e);
    }
    *window = id;
    Ok(1)
}

/// The tip's popup goes (the compositor dismissed it, or its frame is going).  `NHG_E_STATE` for a window that is not a
/// tip's.
pub fn hide(state: &mut Display, window: u64) -> Result<(), GlueError> {
    if state.frame(window)?.flags & NHG_FRAME_TIP == 0 {
        return Err(GlueError::State);
    }
    window::destroy(state, window)
}

/// A `POPUP_DONE` of a tip's window: a `DELETE` action flagged `NHG_DEL_TIP`.  `false` for another window's (a popup
/// frame's, still C's).
pub fn popup_done(state: &mut Display, ev: &Record) -> bool {
    match state.frame(ev.window) {
        Ok(f) if f.flags & NHG_FRAME_TIP != 0 => {
            let mut a = NhgAction::new(NHG_ACT_DELETE, ev.window, ev.time_ns);
            a.flags = NHG_DEL_TIP;
            state.actions.push(a);
            true
        }
        _ => false,
    }
}

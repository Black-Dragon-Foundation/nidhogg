//! The input method: the compositor's text-input events as actions, and C's three requests (on or off, the cursor
//! rectangle, the text around point).  Moved from the C drain's input-method cases and `nhwl_im_*`
//! (`src/nhwlterm.c`, adapted from pgtk's `im_context` callbacks, emacs-31.1).
//!
//! The shell's text input already sends a request to the compositor only when it changed, and again when the
//! compositor enters the window's text input, so the requests here do not keep their own copy.  What the text around
//! point is, and what the events do to the buffer, read live Emacs state and stay C.
//!
//! The events are the keyboard's: C sends them to the frame of the window's tree that Emacs focused, or to the focus
//! frame, so the actions name the window as it came, even one the glue forgot.

use crate::action::{NHG_ACT_COMMIT, NHG_ACT_IM_DELETE, NHG_ACT_PREEDIT, NhgAction};
use crate::display::Display;
use crate::guard::GlueError;
use nhshell::event::{Record, ShellEvent};

/// A `PREEDIT`, `COMMIT_TEXT` or `DELETE_SURROUNDING`: a `PREEDIT` or `COMMIT` action with the text (null when the
/// shell could not give it; empty clears a preedit), or an `IM_DELETE` with the bytes to delete before (`x`) and
/// after (`y`) point.
pub fn event(state: &mut Display, ev: &Record) {
    let (kind, t) = match ev.event {
        ShellEvent::Preedit(t) => (NHG_ACT_PREEDIT, t),
        ShellEvent::CommitText(t) => (NHG_ACT_COMMIT, t),
        ShellEvent::DeleteSurrounding(t) => (NHG_ACT_IM_DELETE, t),
        _ => return,
    };
    let mut a = NhgAction::new(kind, ev.window, ev.time_ns);
    if kind == NHG_ACT_IM_DELETE {
        a.x = i32::try_from(t.before).unwrap_or(i32::MAX);
        a.y = i32::try_from(t.after).unwrap_or(i32::MAX);
        state.actions.push(a);
        return;
    }
    let text = match t {
        t if t.len == 0 => Some(Vec::new()),
        t => state.backend().take_text(t.id).ok(),
    };
    state.actions.push_text(a, text);
}

/// Text input for `window` on or off.
pub fn enable(state: &mut Display, window: u64, on: bool) -> Result<(), GlueError> {
    state.backend().text_input_enable(window, on)?;
    Ok(())
}

/// The cursor rectangle `r` of `window`'s frame, device pixels in its native area: the input method of its tree's root
/// gets it in logical pixels of the root's surface at `s120` (C's scale of the root; 0: 120), at least 1 x 1.  The
/// origin and the rounding moved from `nhwl_im_update`.
pub fn cursor(state: &mut Display, window: u64, s120: u32, r: [i32; 4]) -> Result<(), GlueError> {
    let (root, ox, oy) = crate::child::native_origin(state, window)?;
    let [x, y, w, h] = cursor_rect(r, ox, oy, s120);
    state.backend().text_input_cursor(root, x, y, w, h)?;
    Ok(())
}

/// The rectangle `r` (device pixels, from the native area at (`ox`, `oy`) in the root's surface) in logical pixels of
/// the root's surface at `s120` (0: 120), at least 1 x 1, rounded as `nhwl_to_logical` did.
pub fn cursor_rect(r: [i32; 4], ox: i32, oy: i32, s120: u32) -> [i32; 4] {
    let s = if s120 == 0 { 120 } else { s120 };
    let log = |v: i32| crate::window::to_logical(v, s);
    let [x, y, w, h] = r;
    [
        log(x.wrapping_add(ox)),
        log(y.wrapping_add(oy)),
        log(w).max(1),
        log(h).max(1),
    ]
}

/// The text around point in `window`, and where point and the mark are in it (byte offsets).
pub fn surrounding(
    state: &mut Display,
    window: u64,
    text: &str,
    cursor: usize,
    anchor: usize,
) -> Result<(), GlueError> {
    state
        .backend()
        .text_input_surrounding(window, text, cursor, anchor)?;
    Ok(())
}

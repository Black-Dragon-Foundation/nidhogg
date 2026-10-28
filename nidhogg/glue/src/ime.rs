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
use nhshell::event::*;

/// A `PREEDIT`, `COMMIT_TEXT` or `DELETE_SURROUNDING`: a `PREEDIT` or `COMMIT` action with the text (null when the
/// shell could not give it; empty clears a preedit), or an `IM_DELETE` with the bytes to delete before (`x`) and
/// after (`y`) point.
pub fn event(state: &mut Display, ev: &NhShellEvent) {
    let kind = match ev.kind {
        NHSHELL_EV_PREEDIT => NHG_ACT_PREEDIT,
        NHSHELL_EV_COMMIT_TEXT => NHG_ACT_COMMIT,
        _ => NHG_ACT_IM_DELETE,
    };
    let mut a = NhgAction::new(kind, ev.window, ev.time_ns);
    if kind == NHG_ACT_IM_DELETE {
        // SAFETY: a DELETE_SURROUNDING record carries `text`.
        let t = unsafe { ev.u.text };
        a.x = i32::try_from(t.before).unwrap_or(i32::MAX);
        a.y = i32::try_from(t.after).unwrap_or(i32::MAX);
        state.actions.push(a);
        return;
    }
    // SAFETY: PREEDIT and COMMIT_TEXT records carry `text`.
    let text = match unsafe { ev.u.text } {
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

/// The cursor rectangle in `window`, in logical pixels.
pub fn cursor(
    state: &mut Display,
    window: u64,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
) -> Result<(), GlueError> {
    state.backend().text_input_cursor(window, x, y, w, h)?;
    Ok(())
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

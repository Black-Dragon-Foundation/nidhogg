//! Keyboard focus: which window has the keyboard, and the `FOCUS_IN`/`FOCUS_OUT` actions.  Moved from the C drain's
//! `FOCUS` case (`src/nhwlterm.c`).  What the focus means to Emacs stays C, because it reads live Emacs state: the
//! popup frames that keep their top-level's focus, the frame of the tree that takes the keys, `x_focus_frame` and the
//! highlighting (`nhwl_focus_changed`, adapted from pgtk's `pgtk_focus_changed`, emacs-31.1).

use crate::action::{NHG_ACT_FOCUS_IN, NHG_ACT_FOCUS_OUT, NhgAction};
use crate::display::Display;
use nhshell::event::NhShellEvent;

/// A `FOCUS` event: the window with the keyboard follows it, and a registered window gets one `FOCUS_IN` or
/// `FOCUS_OUT` action naming it; a window the glue does not know gets none (C found no frame for it).
pub fn focus(state: &mut Display, ev: &NhShellEvent) {
    // SAFETY: a FOCUS record's valid member is `focus`.
    let on = unsafe { ev.u.focus }.r#in != 0;
    if on {
        state.keyboard.window = ev.window;
    } else if state.keyboard.window == ev.window {
        state.keyboard.window = 0;
    }
    if state.frame(ev.window).is_ok() {
        let kind = if on {
            NHG_ACT_FOCUS_IN
        } else {
            NHG_ACT_FOCUS_OUT
        };
        state
            .actions
            .push(NhgAction::new(kind, ev.window, ev.time_ns));
    }
}

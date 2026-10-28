//! The event step: `nhg_next` takes exactly one shell event and answers with its actions, which C applies before it
//! asks for the next one, so every decision C makes while it applies them sees what the earlier ones changed, as the
//! C drain did (design §4, review B1).  Moved from `nhwl_drain_events` (`src/nhwlterm.c`), whose apply side stays
//! C.
//!
//! An event that names a window the glue forgot gives no action, except the keyboard's (keys and the input method's),
//! which C sends to the focus frame as before; every other kind did nothing in C without a frame.

use crate::action::{
    NHG_ACT_DELETE, NHG_ACT_LOST, NHG_ACT_READY, NHG_ACT_SELECTION_CLEAR, NhgAction, NhgNextIn,
};
use crate::configure;
use crate::csd;
use crate::display::Display;
use crate::focus;
use crate::guard::GlueError;
use crate::ime;
use crate::keys;
use crate::monitors;
use crate::pointer;
use crate::popup;
use crate::scroll;
use crate::select;
use crate::session;
use crate::tip;
use nhshell::event::*;

/// Take one event: `Ok(1)` with its actions in `state.actions` (none, possibly), `Ok(0)` when none is queued.  The
/// last call's actions are dropped first, and the acks of the configures C applied since are recorded.
pub fn next(state: &mut Display, input: NhgNextIn) -> Result<i32, GlueError> {
    state.actions.clear();
    configure::record_due(state);
    let Some(ev) = state.backend().next_event()? else {
        return Ok(0);
    };
    match ev.kind {
        NHSHELL_EV_KEY => keys::key(state, &ev, input),
        NHSHELL_EV_MODIFIERS => keys::modifiers(state, &ev),
        NHSHELL_EV_FOCUS => focus::focus(state, &ev),
        NHSHELL_EV_CONFIGURE => configure::configure(state, &ev),
        NHSHELL_EV_SCALE => configure::scale(state, &ev),
        NHSHELL_EV_CLOSE => window_action(state, &ev, NHG_ACT_DELETE),
        // the present step runs on every FRAME_READY, drawn or not (the shell owes the commit)
        NHSHELL_EV_FRAME_READY => window_action(state, &ev, NHG_ACT_READY),
        NHSHELL_EV_FRAME_DONE => {}
        NHSHELL_EV_SELECTION => selection(state, &ev),
        NHSHELL_EV_SCROLL => scroll::scroll(state, &ev, input),
        NHSHELL_EV_PREEDIT | NHSHELL_EV_COMMIT_TEXT | NHSHELL_EV_DELETE_SURROUNDING => {
            ime::event(state, &ev)
        }
        NHSHELL_EV_OUTPUT_ADD | NHSHELL_EV_OUTPUT_CHANGE => monitors::output(state, &ev),
        NHSHELL_EV_OUTPUT_REMOVE => monitors::removed(state, &ev),
        NHSHELL_EV_ENTER_OUTPUT => monitors::entered(state, &ev),
        NHSHELL_EV_LEAVE_OUTPUT => monitors::left(state, &ev),
        NHSHELL_EV_DISCONNECTED => state
            .actions
            .push(NhgAction::new(NHG_ACT_LOST, 0, ev.time_ns)),
        // through the title bar first: what it does not consume goes on as the pointer's own action, after the title
        // bar's (the event's own action is its last)
        NHSHELL_EV_POINTER_ENTER
        | NHSHELL_EV_POINTER_MOTION
        | NHSHELL_EV_POINTER_LEAVE
        | NHSHELL_EV_BUTTON => {
            if !csd::pointer(state, &ev) {
                pointer::event(state, &ev)
            }
        }
        // a popup frame's dismissal (a tip's is taken in the guard)
        NHSHELL_EV_POPUP_DONE if !tip::popup_done(state, &ev) => {
            popup::popup_done(state, &ev);
        }
        NHSHELL_EV_SESSION => session::event(state, &ev),
        // kinds C did nothing with
        _ => {}
    }
    Ok(1)
}

/// A selection changed: a `SELECTION_CLEAR` (the kind in `code`) when it was ours and another client has it now.
/// From `nhwl_selection_changed`.
fn selection(state: &mut Display, ev: &NhShellEvent) {
    // SAFETY: a SELECTION record's valid member is `selection`.
    let kind = unsafe { ev.u.selection }.kind;
    if select::changed(state, kind) {
        let mut a = NhgAction::new(NHG_ACT_SELECTION_CLEAR, 0, ev.time_ns);
        a.code = kind as i32;
        state.actions.push(a);
    }
}

/// An action of `kind` for a registered window; nothing for another.
fn window_action(state: &mut Display, ev: &NhShellEvent, kind: u32) {
    if state.frame(ev.window).is_ok() {
        state
            .actions
            .push(NhgAction::new(kind, ev.window, ev.time_ns));
    }
}

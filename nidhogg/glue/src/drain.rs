//! The event step: `nhg_next` takes exactly one shell event and answers with its actions, which C applies before it
//! asks for the next one, so every decision C makes while it applies them sees what the earlier ones changed, as the
//! C drain did (design §4, review B1).  Moved from `nhwl_drain_events` (`src/nhwlterm.c`), whose apply side stays
//! C.
//!
//! An event that names a window the glue forgot gives no action, except the keyboard's (keys and the input method's),
//! which C sends to the focus frame as before; every other kind did nothing in C without a frame.

use crate::action::{
    NHG_ACT_DELETE, NHG_ACT_LOST, NHG_ACT_RAW, NHG_ACT_READY, NHG_ACT_SELECTION_CLEAR, NhgAction,
    NhgNextIn,
};
use crate::configure;
use crate::display::Display;
use crate::focus;
use crate::guard::GlueError;
use crate::keys;
use crate::select;
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
        NHSHELL_EV_DISCONNECTED => state
            .actions
            .push(NhgAction::new(NHG_ACT_LOST, 0, ev.time_ns)),
        _ => raw(state, ev),
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

/// Whether `kind` is the keyboard's: C resolves its frame from the focus, not from the event's window.
fn keyboard_kind(kind: u32) -> bool {
    matches!(
        kind,
        NHSHELL_EV_PREEDIT | NHSHELL_EV_COMMIT_TEXT | NHSHELL_EV_DELETE_SURROUNDING
    )
}

/// An event whose kind is not ported yet: a `RAW` action, which C handles on its old path, with the keyboard's
/// modifier mask (a click and a wheel step read it).
fn raw(state: &mut Display, ev: NhShellEvent) {
    if ev.window != 0 && state.is_forgotten(ev.window) && !keyboard_kind(ev.kind) {
        return;
    }
    let mut a = NhgAction::new(NHG_ACT_RAW, ev.window, ev.time_ns);
    a.mods = state.keyboard.mods;
    let text = match ev.kind {
        // SAFETY: these events carry `text`.
        NHSHELL_EV_PREEDIT | NHSHELL_EV_COMMIT_TEXT => match unsafe { ev.u.text } {
            t if t.len == 0 => Some(Vec::new()),
            t => state.backend().take_text(t.id).ok(),
        },
        _ => None,
    };
    state.actions.set_raw(ev);
    state.actions.push_text(a, text);
}

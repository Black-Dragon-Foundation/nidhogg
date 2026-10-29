//! The event step: `nhg_next` takes exactly one shell event and answers with its actions, which C applies before it
//! asks for the next one, so every decision C makes while it applies them sees what the earlier ones changed, as the
//! C drain did.  Moved from `nhwl_drain_events` (`src/nhwlterm.c`), whose apply side stays
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
use crate::dnd;
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
use crate::touch;

use crate::reconnect;
use nhshell::event::{Record, SelectionEv, ShellEvent};

/// Take one event: `Ok(1)` with its actions in `state.actions` (none, possibly), `Ok(0)` when none is queued.  The
/// last call's actions are dropped first, and the acks of the configures C applied since are recorded.
pub fn next(state: &mut Display, input: NhgNextIn) -> Result<i32, GlueError> {
    state.actions.clear();
    configure::record_due(state);
    let Some(ev) = state.backend().next_event()? else {
        return Ok(0);
    };
    // our drag out (`x-begin-drag`) reads the source's records and the input that shows whether it began; the input
    // goes on as usual below
    dnd::drag_input(state, &ev);
    match ev.event {
        ShellEvent::Key(k) => keys::key(state, &ev, &k, input),
        ShellEvent::Modifiers { mods } => keys::modifiers(state, mods),
        ShellEvent::Focus(f) => focus::focus(state, &ev, &f),
        ShellEvent::Configure(c) => configure::configure(state, &ev, &c),
        ShellEvent::Scale(s) => configure::scale(state, &ev, &s),
        ShellEvent::Close => window_action(state, &ev, NHG_ACT_DELETE),
        // the present step runs on every FRAME_READY, drawn or not (the shell owes the commit)
        ShellEvent::FrameReady => window_action(state, &ev, NHG_ACT_READY),
        ShellEvent::FrameDone(_) => {}
        ShellEvent::Selection(s) => selection(state, &ev, &s),
        ShellEvent::Scroll(s) => scroll::scroll(state, &ev, &s, input),
        ShellEvent::Preedit(_) | ShellEvent::CommitText(_) | ShellEvent::DeleteSurrounding(_) => {
            ime::event(state, &ev)
        }
        ShellEvent::OutputAdd(o) | ShellEvent::OutputChange(o) => monitors::output(state, &ev, &o),
        ShellEvent::OutputRemove(o) => monitors::removed(state, &ev, &o),
        ShellEvent::EnterOutput(o) => monitors::entered(state, &ev, &o),
        ShellEvent::LeaveOutput(o) => monitors::left(state, &ev, &o),
        ShellEvent::Disconnected => state
            .actions
            .push(NhgAction::new(NHG_ACT_LOST, 0, ev.time_ns)),
        // through the title bar first: what it does not consume goes on as the pointer's own action, after the title
        // bar's (the event's own action is its last)
        ShellEvent::PointerEnter(_)
        | ShellEvent::PointerMotion(_)
        | ShellEvent::PointerLeave(_)
        | ShellEvent::Button(_) => {
            if !csd::pointer(state, &ev) {
                pointer::event(state, &ev)
            }
        }
        ShellEvent::DndEnter(_)
        | ShellEvent::DndMotion(_)
        | ShellEvent::DndLeave(_)
        | ShellEvent::DndDrop(_) => dnd::event(state, &ev),
        // a popup frame's dismissal (a tip's is taken in the guard)
        ShellEvent::PopupDone if !tip::popup_done(state, &ev) => {
            popup::popup_done(state, &ev);
        }
        ShellEvent::Session(s) => session::event(state, &ev, &s),
        ShellEvent::Touch(t) => touch::touch(state, &ev, &t),
        ShellEvent::Pinch(p) => touch::pinch(state, &ev, &p),
        // a popup closed by the reconnect came as an ordinary dismissal above
        ShellEvent::Reconnecting => reconnect::lost(state, &ev),
        ShellEvent::Reconnected => reconnect::reconnected(state, &ev),
        // kinds C did nothing with
        _ => {}
    }
    reconnect::after(state, &ev);
    Ok(1)
}

/// A selection changed: a `SELECTION_CLEAR` (the kind in `code`) when it was ours and another client has it now.
/// From `nhwl_selection_changed`.
fn selection(state: &mut Display, ev: &Record, s: &SelectionEv) {
    let kind = s.kind;
    if select::changed(state, kind) {
        let mut a = NhgAction::new(NHG_ACT_SELECTION_CLEAR, 0, ev.time_ns);
        a.code = kind as i32;
        state.actions.push(a);
    }
}

/// An action of `kind` for a registered window; nothing for another.
fn window_action(state: &mut Display, ev: &Record, kind: u32) {
    if state.frame(ev.window).is_ok() {
        state
            .actions
            .push(NhgAction::new(kind, ev.window, ev.time_ns));
    }
}

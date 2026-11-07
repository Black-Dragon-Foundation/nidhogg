//! The event step: `nhg_next` takes exactly one shell event and answers with its actions, which C applies before it
//! asks for the next one, so every decision C makes while it applies them sees what the earlier ones changed, as the
//! C drain did.  Moved from `nhwl_drain_events` (`src/nhwlterm.c`), whose apply side stays
//! C.
//!
//! An event that names a window the glue forgot gives no action, except the keyboard's (keys and the input method's),
//! which C sends to the focus frame as before; every other kind did nothing in C without a frame.

use crate::action::{
    NHG_ACT_DELETE, NHG_ACT_LOST, NHG_ACT_MENU, NHG_ACT_READY, NHG_ACT_SELECTION_CLEAR,
    NHG_IN_ACKS_ONLY, NHG_IN_REDISPLAY, NhgAction, NhgNextIn,
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

/// A drain (C's `nhg_next` calls up to its `nhg_dispatch_main`): whether it ran inside redisplay and took events.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Facts {
    pub redisplay: bool,
    pub took: bool,
}

/// Whether the end of drain `d` asks the platform for new events (`dispatch_main`) or only does the host-thread work
/// (`flush_main`), `last_took` being [`carried_take`]'s.  It asks only after a drain that found nothing, outside
/// redisplay, with no take since the last such drain: a drain that took events is called again (gobble
/// input calls read_socket while it stores events), the empty drain right after it closes that read, and redisplay's
/// input checks would only bring an event redisplay does not read.  So a key comes in through one pump, and C-g in a
/// busy loop still reaches the queue through the poll's next empty drain.
pub fn closing_pump(d: Facts, last_took: bool) -> bool {
    !d.redisplay && !d.took && !last_took
}

/// The `last_took` the drain after `d` sees, `last_took` being the one `d` saw: whether a drain took events since
/// the last drain outside redisplay.  Redisplay's drains never pump, so a take in one (read_char redisplays before it
/// waits, and its input check finds the key) still closes at the next drain outside redisplay, past its own empty
/// checks.
pub fn carried_take(d: Facts, last_took: bool) -> bool {
    d.took || (d.redisplay && last_took)
}

/// The end of the drain in progress (`nhg_dispatch_main`): the backend's pump when [`closing_pump`] says, its flush
/// otherwise.  A call outside a drain (a wait, the connection's start) always pumps.
pub fn dispatch_main(state: &mut Display) -> Result<(), GlueError> {
    let pump = match state.drain.take() {
        Some(d) => {
            let pump = closing_pump(d, state.last_drain_took);
            nhshell::trace::pump(|| {
                format!(
                    "drain end {d:?} last_took {}: pump {pump}",
                    state.last_drain_took
                )
            });
            state.last_drain_took = carried_take(d, state.last_drain_took);
            pump
        }
        None => {
            nhshell::trace::pump(|| "outside a drain: pump".into());
            true
        }
    };
    if pump {
        state.backend().dispatch_main()?;
    } else {
        state.backend().flush_main()?;
    }
    Ok(())
}

/// Take one event: `Ok(1)` with its actions in `state.actions` (none, possibly), `Ok(0)` when none is queued.  The
/// last call's actions are dropped first, and the acks of the configures C applied since are recorded; with
/// `NHG_IN_ACKS_ONLY` that is all (`Ok(0)`), so that a drain cut at its cap leaves no configure it applied unanswered.
pub fn next(state: &mut Display, input: NhgNextIn) -> Result<i32, GlueError> {
    state.actions.clear();
    configure::record_due(state);
    let drain = state.drain.get_or_insert_default();
    drain.redisplay |= input.flags & NHG_IN_REDISPLAY != 0;
    if input.flags & NHG_IN_ACKS_ONLY != 0 {
        return Ok(0);
    }
    let Some(ev) = state.backend().next_event()? else {
        return Ok(0);
    };
    state.drain.get_or_insert_default().took = true;
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
        // a choice from the native menu bar: C turns it into the menu-bar events of the item's key path
        ShellEvent::Menu(m) => {
            let mut a = NhgAction::new(NHG_ACT_MENU, 0, ev.time_ns);
            a.code = m.index as i32;
            a.flags = m.generation;
            state.actions.push(a);
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The pumps a run of drains makes, each `(redisplay, took)`, as `dispatch_main` decides them.
    fn pumps(drains: &[(bool, bool)]) -> Vec<bool> {
        let mut last = false;
        drains
            .iter()
            .map(|&(redisplay, took)| {
                let d = Facts { redisplay, took };
                let pump = closing_pump(d, last);
                last = carried_take(d, last);
                pump
            })
            .collect()
    }

    /// A key's cycle: the read that finds nothing pumps (the key comes in), the read that takes it and the empty read
    /// gobble input makes after it do not, nor does redisplay's input check.  One pump.
    #[test]
    fn a_key_comes_in_through_one_pump() {
        let cycle = pumps(&[(false, false), (false, true), (false, false), (true, false)]);
        assert_eq!(cycle, [true, false, false, false]);
    }

    /// The key read_char's redisplay takes (read-event redisplays before it waits): its input checks find the key
    /// and then nothing, and the empty read after them, `redisplay''s own, closes that read.  One pump.
    #[test]
    fn a_key_taken_in_redisplay_comes_in_through_one_pump() {
        let cycle = pumps(&[
            (false, false),
            (true, true),
            (true, false),
            (false, false),
            (true, false),
        ]);
        assert_eq!(cycle, [true, false, false, false, false]);
    }

    /// A busy loop's polls find nothing each time: each one pumps, so C-g reaches the queue; after it is taken the
    /// next empty poll closes that read and the one after pumps again.
    #[test]
    fn polls_pump_until_an_event_and_again_after() {
        assert_eq!(pumps(&[(false, false); 3]), [true; 3]);
        assert_eq!(
            pumps(&[
                (false, false),
                (false, true),
                (false, false),
                (false, false)
            ]),
            [true, false, false, true]
        );
    }
}

//! The drop target: a drag from another client over one of our windows, as `pgtk-dnd.el` sees it.
//!
//! The shell reports the drag (`DND_ENTER`, `DND_MOTION`, `DND_LEAVE`, `DND_DROP`); the glue turns each into an action
//! (`NHG_ACT_DND_*`) that C applies as a `DRAG_N_DROP_EVENT`, and answers Lisp's two calls, `pgtk-update-drop-status`
//! ([`status`]) and `pgtk-drop-finish` ([`finish`]).  The offer's data is read when Lisp asks, through
//! `nhg_selection_read` with `NHSHELL_SEL_DND`, valid from the enter until the leave or the finish.
//!
//! **Acceptance** is Lisp's: the drag is accepted with the type Lisp last read during this drag (else the first the
//! source offers) and the action Lisp chose; no action accepts nothing (`accept null`, actions 0).  The glue refuses on
//! its own only a drag none of whose types the window registered (`pgtk-register-dnd-targets`), or one over a window
//! it does not know.
//!
//! **Stale calls** carry the time of an earlier drag (Lisp answers late): a call older than the current drag's enter
//! does nothing.
//!
//! **The offer is always finished.**  `DND_DROP` leaves the offer alive until Lisp's `pgtk-drop-finish`; if Lisp never
//! calls it (a silent source, an error before the unwind) the next drag's enter finishes it first.  The shell checks
//! that `finish` is legal (an action was set); a finish that would be a protocol error just destroys the offer.

use crate::action::{
    NHG_ACT_DND_DROP, NHG_ACT_DND_ENTER, NHG_ACT_DND_LEAVE, NHG_ACT_DND_MOTION, NhgAction,
};
use crate::display::Display;
use crate::guard::GlueError;
use nhshell::api::{NHSHELL_SEL_DND, ShellBackend, ShellError};
use nhshell::event::{DndEv, NHSHELL_EV_DND_ENTER, NHSHELL_EV_DND_LEAVE, NHSHELL_EV_DND_MOTION};
use nhshell::event::{NHSHELL_EV_DND_DROP, NhShellEvent};
use std::collections::BTreeMap;

/// What the glue holds for the drop target of one display.
#[derive(Debug, Default)]
pub struct Dnd {
    /// Each window's registered types (`pgtk-register-dnd-targets`).
    targets: BTreeMap<u64, Vec<String>>,
    /// A drag is over one of our windows, from its enter until its leave, drop or finish.
    over: bool,
    /// The glue answered the enter with a refusal: no action goes to C for this drag.
    refused: bool,
    /// The drop came and Lisp has not finished it: the offer is alive.
    dropped: bool,
    /// The compositor's time of the current drag's enter, milliseconds.
    enter_ms: u32,
    /// The type Lisp last read during this drag.
    mime: Option<String>,
}

/// Whether `time` is older than `enter` (the compositor's 32-bit milliseconds wrap).
fn older(time: u32, enter: u32) -> bool {
    (time.wrapping_sub(enter) as i32) < 0
}

impl Dnd {
    /// Register the types a drop on `window` takes (replacing the earlier list).
    pub fn register(&mut self, window: u64, mimes: Vec<String>) {
        self.targets.insert(window, mimes);
    }

    /// `window` is forgotten (its id may come back as another window).
    pub fn forget(&mut self, window: u64) {
        self.targets.remove(&window);
    }

    /// Lisp read `mime` from the offer: it is the type this drag is accepted with.
    pub fn note_read(&mut self, mime: &str) {
        if self.over && !self.dropped {
            self.mime = Some(mime.to_owned());
        }
    }

    /// Whether a drag over `window` offering `types` is one the window takes.
    fn takes(&self, window: u64, types: &[String]) -> bool {
        match self.targets.get(&window) {
            Some(t) if !t.is_empty() => types
                .iter()
                .any(|o| t.iter().any(|r| r.eq_ignore_ascii_case(o))),
            _ => true,
        }
    }

    /// Give up the offer an earlier drop left alive.
    fn finish_stale(&mut self, backend: &mut dyn ShellBackend) {
        if self.dropped {
            let _ = backend.dnd_finish(false);
        }
        self.over = false;
        self.refused = false;
        self.dropped = false;
        self.mime = None;
    }
}

/// An action of `kind` for `window` at the record's position; `code` the action (`NHSHELL_DND_*`), `time_ms` the
/// compositor's time (not the event's: Lisp passes it back to `status` and `finish`).
fn action(kind: u32, window: u64, d: &DndEv, code: u32) -> NhgAction {
    let mut a = NhgAction::new(kind, window, 0);
    a.time_ms = u64::from(d.time_ms);
    a.dx = d.x;
    a.dy = d.y;
    a.code = code as i32;
    a
}

/// The shell reported a drag-and-drop event of the target side.
pub fn event(state: &mut Display, ev: &NhShellEvent) {
    // SAFETY: a DND record's valid member is `dnd`.
    let d = unsafe { ev.u.dnd };
    let known = state.frame(ev.window).is_ok();
    let sh = state.shell();
    // SAFETY: the handle lives as long as the display, and `dnd` and `actions` are fields it does not touch.
    let backend = unsafe { &mut *sh }.backend_mut();
    let (dnd, actions) = (&mut state.dnd, &mut state.actions);
    match ev.kind {
        NHSHELL_EV_DND_ENTER => {
            dnd.finish_stale(backend);
            dnd.over = true;
            dnd.enter_ms = d.time_ms;
            let types = backend.selection_types(NHSHELL_SEL_DND).unwrap_or_default();
            if known && dnd.takes(ev.window, &types) {
                actions.push(action(NHG_ACT_DND_ENTER, ev.window, &d, d.preferred));
            } else {
                dnd.refused = true;
                let _ = backend.dnd_status(None, 0, 0);
            }
        }
        NHSHELL_EV_DND_MOTION if dnd.over && !dnd.refused && !dnd.dropped => {
            actions.push(action(NHG_ACT_DND_MOTION, ev.window, &d, d.preferred));
        }
        // the shell sends none after a drop; a leave that comes anyway must not end the offer Lisp will read
        NHSHELL_EV_DND_LEAVE if !dnd.dropped => {
            if dnd.over && !dnd.refused {
                actions.push(action(NHG_ACT_DND_LEAVE, ev.window, &d, 0));
            }
            dnd.over = false;
            dnd.refused = false;
            dnd.mime = None;
        }
        NHSHELL_EV_DND_DROP => {
            if dnd.over && !dnd.refused && !dnd.dropped && known {
                dnd.dropped = true;
                actions.push(action(NHG_ACT_DND_DROP, ev.window, &d, d.action));
            } else {
                // nothing will finish this offer
                let _ = backend.dnd_finish(false);
                dnd.over = false;
                dnd.refused = false;
                dnd.mime = None;
            }
        }
        _ => {}
    }
}

/// `pgtk-register-dnd-targets`: the types a drop on `window` takes.  `State` for a window that is not a frame.
pub fn register(state: &mut Display, window: u64, mimes: Vec<String>) -> Result<(), GlueError> {
    state.frame(window)?;
    state.dnd.register(window, mimes);
    Ok(())
}

/// `pgtk-update-drop-status`: accept the current drag for `action` (`NHSHELL_DND_*`; 0 accepts nothing), stamped
/// `time_ms`.  True when the shell was told, false for a stale call or without a drag to answer.
pub fn status(state: &mut Display, action: u32, time_ms: u32) -> Result<bool, GlueError> {
    let sh = state.shell();
    // SAFETY: the handle lives as long as the display, and `dnd` is a field it does not touch.
    let backend = unsafe { &mut *sh }.backend_mut();
    let dnd = &mut state.dnd;
    if !dnd.over || dnd.refused || dnd.dropped || older(time_ms, dnd.enter_ms) {
        return Ok(false);
    }
    let sent = if action == 0 {
        backend.dnd_status(None, 0, 0)
    } else {
        let mime = match dnd.mime.clone() {
            Some(m) => Some(m),
            None => backend
                .selection_types(NHSHELL_SEL_DND)
                .ok()
                .and_then(|t| t.into_iter().next()),
        };
        backend.dnd_status(mime.as_deref(), action, action)
    };
    match sent {
        Ok(()) => Ok(true),
        // the offer is gone
        Err(ShellError::State) => Ok(false),
        Err(e) => Err(e.into()),
    }
}

/// `pgtk-drop-finish`: the drop is done, `success` when the action was performed.  The offer is destroyed either
/// way; true when the shell finished it with the source (an action was set and `success`), false for a stale call,
/// without a dropped offer, or when there was nothing to finish with the source.
pub fn finish(state: &mut Display, success: bool, time_ms: u32) -> Result<bool, GlueError> {
    let sh = state.shell();
    // SAFETY: as in `status`.
    let backend = unsafe { &mut *sh }.backend_mut();
    let dnd = &mut state.dnd;
    if !dnd.dropped || older(time_ms, dnd.enter_ms) {
        return Ok(false);
    }
    dnd.over = false;
    dnd.dropped = false;
    dnd.mime = None;
    match backend.dnd_finish(success) {
        Ok(()) => Ok(true),
        // the offer is destroyed; the finish was not legal
        Err(ShellError::State) => Ok(false),
        Err(e) => Err(e.into()),
    }
}

/// Whether the glue holds a dropped offer Lisp has not finished (for the tests).
pub fn pending(state: &Display) -> bool {
    state.dnd.dropped
}

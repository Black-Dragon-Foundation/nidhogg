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
//!
//! **The drag source** is `x-begin-drag`'s ([`drag_start`], [`DragOut`]): C runs the call's loop and keeps the time,
//! and the glue turns the shell's source records, and the input that shows whether the compositor took the drag, into
//! the `NHG_ACT_DRAG_*` actions that end it.  Our drag over our own windows (`own` 1) never reaches Lisp as a drag: the
//! glue answers it, and only its drop goes to `pgtk-dnd.el`.

use crate::action::{Actions, NHG_ACT_DRAG_ACTION, NHG_ACT_DRAG_DONE, NHG_ACT_DRAG_DROPPED};
use crate::action::{
    NHG_ACT_DND_DROP, NHG_ACT_DND_ENTER, NHG_ACT_DND_LEAVE, NHG_ACT_DND_MOTION, NhgAction,
};
use crate::display::Display;
use crate::guard::GlueError;
use nhshell::api::{NHSHELL_SEL_DND, ShellBackend, ShellError};
use nhshell::event::{DndEv, Kind, NHSHELL_BUTTON_RELEASED, NHSHELL_TOUCH_UP, Record, ShellEvent};
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
    /// Our own drag out of a frame (`x-begin-drag`).
    out: DragOut,
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

    /// The connection went (a reconnect): the drag and its offer went with it, unfinished; the registered types stay
    /// (window ids survive).
    pub fn lost(&mut self) {
        self.over = false;
        self.refused = false;
        self.dropped = false;
        self.mime = None;
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
pub fn event(state: &mut Display, ev: &Record) {
    let (ShellEvent::DndEnter(d)
    | ShellEvent::DndMotion(d)
    | ShellEvent::DndLeave(d)
    | ShellEvent::DndDrop(d)) = ev.event
    else {
        return;
    };
    if d.own != 0 {
        return own_drag(state, ev, &d);
    }
    let known = state.frame(ev.window).is_ok();
    let sh = state.shell();
    // SAFETY: the handle lives as long as the display, and `dnd` and `actions` are fields it does not touch.
    let backend = unsafe { &mut *sh }.backend_mut();
    let (dnd, actions) = (&mut state.dnd, &mut state.actions);
    match ev.kind() {
        Kind::DndEnter => {
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
        Kind::DndMotion if dnd.over && !dnd.refused && !dnd.dropped => {
            actions.push(action(NHG_ACT_DND_MOTION, ev.window, &d, d.preferred));
        }
        // the shell sends none after a drop; a leave that comes anyway must not end the offer Lisp will read
        Kind::DndLeave if !dnd.dropped => {
            if dnd.over && !dnd.refused {
                actions.push(action(NHG_ACT_DND_LEAVE, ev.window, &d, 0));
            }
            dnd.over = false;
            dnd.refused = false;
            dnd.mime = None;
        }
        Kind::DndDrop => {
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

// ---- The drag source: `x-begin-drag` -------------------------------------------------------------------------------

/// `nhg_drag_start` flags, from `x-begin-drag`'s arguments: drops on the start window are taken (ALLOW-CURRENT-FRAME);
/// the drag is given up for the first of our frames it is over once the pointer has left the start window
/// (RETURN-FRAME), or at once (RETURN-FRAME `now`).
pub const NHG_DRAG_ALLOW_CURRENT: u32 = 1;
pub const NHG_DRAG_RETURN_FRAME: u32 = 2;
pub const NHG_DRAG_RETURN_NOW: u32 = 4;
/// `DRAG_DONE` flags: `window` is the frame to return.
pub const NHG_DRAG_FRAME: u32 = 1;

/// Our drag out of a frame, from [`drag_start`] until the `DRAG_DONE` action that ends it.
#[derive(Debug, Default)]
pub struct DragOut {
    /// The drag runs and its end has not been reported.
    running: bool,
    /// The compositor showed it took the drag (a `DRAG_ACTION` went to C).
    live: bool,
    /// The window it started from.
    window: u64,
    /// The actions offered, and the one our own windows prefer.
    actions: u32,
    preferred: u32,
    /// `NHG_DRAG_*`.
    flags: u32,
    /// The type our own windows accept: the first one offered.
    mime: String,
    /// The pointer has left `window` since the start.
    left: bool,
    /// Our window the drag is over (0: none).
    over: u64,
    /// The action the compositor chose with the target (0: none).
    action: u32,
}

impl DragOut {
    /// The compositor took the drag: say so once.
    fn live(&mut self, actions: &mut Actions) {
        if !self.live {
            self.live = true;
            actions.push(drag_action(
                NHG_ACT_DRAG_ACTION,
                self.window,
                self.action,
                0,
            ));
        }
    }

    /// The drag is over with `action` (0: none); `frame` the window whose frame the call returns.
    fn end(&mut self, actions: &mut Actions, action: u32, frame: Option<u64>) {
        self.running = false;
        let (window, flags) = match frame {
            Some(w) => (w, NHG_DRAG_FRAME),
            None => (self.window, 0),
        };
        actions.push(drag_action(NHG_ACT_DRAG_DONE, window, action, flags));
    }
}

fn drag_action(kind: u32, window: u64, action: u32, flags: u32) -> NhgAction {
    let mut a = NhgAction::new(kind, window, 0);
    a.code = action as i32;
    a.flags = flags;
    a
}

/// `x-begin-drag`: drag `items` (MIME type, bytes) out of `window`, offering `actions` (`NHSHELL_DND_*`), our own
/// windows preferring `preferred` (one of them), with `NHG_DRAG_*` `flags`.  `State` without a held button press or
/// touch down (the call returns nil at once, as X's does) or for a window that is not a frame; `Arg` without items or
/// for actions the shell refuses.  A drag still running is replaced, and its end is never reported.
pub fn drag_start(
    state: &mut Display,
    window: u64,
    items: &[(&str, &[u8])],
    actions: u32,
    preferred: u32,
    flags: u32,
) -> Result<(), GlueError> {
    state.frame(window)?;
    let Some(&(first, _)) = items.first() else {
        return Err(GlueError::Arg);
    };
    if preferred.count_ones() != 1 || actions & preferred == 0 {
        return Err(GlueError::Arg);
    }
    state.dnd.out.running = false;
    state.backend().drag_start(window, items, actions)?;
    state.dnd.out = DragOut {
        running: true,
        window,
        actions,
        preferred,
        flags,
        mime: first.to_owned(),
        ..DragOut::default()
    };
    Ok(())
}

/// Give our drag up (C-g, a timeout, the call unwinding): its source goes, and no `DRAG_DONE` follows.  True when it
/// ran.  The shell's answer is not an error here: a display that is lost or reconnecting has ended the drag already.
pub fn drag_cancel(state: &mut Display) -> bool {
    let ran = std::mem::take(&mut state.dnd.out.running);
    if ran {
        let _ = state.backend().drag_cancel();
    }
    ran
}

/// The records our drag reads while it runs: the source's (`DRAG_*`), and the input that shows whether the compositor
/// took the drag.  A pointer leave says it did (the drag's grab took the pointer).  A button release or a touch up that
/// still reaches us says it did not: the compositor ignores a drag with a stale serial and sends nothing about it, so
/// the drag ends there with no action.  `DRAG_DONE` ends it with the target's action, or none when it was cancelled
/// (dropped on nothing, refused, or the display lost).
pub fn drag_input(state: &mut Display, ev: &Record) {
    if !state.dnd.out.running {
        return;
    }
    let (out, actions) = (&mut state.dnd.out, &mut state.actions);
    let refused = match ev.event {
        ShellEvent::DragAction(d) => {
            out.action = d.action;
            out.live = true;
            actions.push(drag_action(NHG_ACT_DRAG_ACTION, out.window, out.action, 0));
            false
        }
        ShellEvent::DragDropped(d) => {
            out.action = d.action;
            out.live = true;
            if out.over != 0 {
                // on our own window: its DND_DROP gives Lisp the drop, and the call returns now
                let a = out.action;
                out.end(actions, a, None);
            } else {
                actions.push(drag_action(NHG_ACT_DRAG_DROPPED, out.window, out.action, 0));
            }
            false
        }
        ShellEvent::DragDone(d) => {
            out.end(actions, if d.ok != 0 { d.action } else { 0 }, None);
            false
        }
        ShellEvent::PointerLeave(_) => {
            out.live(actions);
            false
        }
        ShellEvent::Button(p) => p.state == NHSHELL_BUTTON_RELEASED,
        ShellEvent::Touch(t) => t.op == NHSHELL_TOUCH_UP,
        _ => false,
    };
    if refused {
        out.end(actions, 0, None);
        let _ = state.backend().drag_cancel();
    }
}

/// A record of our own drag over one of our windows (`own` 1).  Lisp sees none of it but the drop: the glue answers the
/// status itself (the drag's first type, its actions and preferred action; a refusal on the start window without
/// `NHG_DRAG_ALLOW_CURRENT`, on a window that is not a frame, or once the call has returned).  A drop becomes the
/// `DND_ENTER` and `DND_DROP` actions `pgtk-dnd.el` reads the drag's kept bytes from (`NHSHELL_SEL_DND`), and the glue
/// finishes the offer at once, so our source ends; the call returns at the drop.
fn own_drag(state: &mut Display, ev: &Record, d: &DndEv) {
    let known = state.frame(ev.window).is_ok();
    let sh = state.shell();
    // SAFETY: the handle lives as long as the display, and `dnd` and `actions` are fields it does not touch.
    let backend = unsafe { &mut *sh }.backend_mut();
    let (out, actions) = (&mut state.dnd.out, &mut state.actions);
    match ev.kind() {
        Kind::DndEnter => {
            if ev.window != out.window {
                out.left = true;
            }
            out.over = ev.window;
            if out.running {
                out.live(actions);
                let back = out.flags & NHG_DRAG_RETURN_NOW != 0
                    || (out.flags & NHG_DRAG_RETURN_FRAME != 0 && out.left);
                if back && known {
                    out.end(actions, 0, Some(ev.window));
                    let _ = backend.drag_cancel();
                    return;
                }
            }
            let take = out.running
                && known
                && (ev.window != out.window || out.flags & NHG_DRAG_ALLOW_CURRENT != 0);
            let _ = if take {
                backend.dnd_status(Some(&out.mime), out.actions, out.preferred)
            } else {
                backend.dnd_status(None, 0, 0)
            };
        }
        Kind::DndLeave => {
            if out.over == out.window {
                out.left = true;
            }
            out.over = 0;
        }
        Kind::DndDrop => {
            out.over = 0;
            if known && d.action != 0 {
                actions.push(action(NHG_ACT_DND_ENTER, ev.window, d, d.action));
                actions.push(action(NHG_ACT_DND_DROP, ev.window, d, d.action));
            }
            let _ = backend.dnd_finish(d.action != 0);
            if out.running {
                out.end(actions, d.action, None);
            }
        }
        _ => {}
    }
}

/// Whether our drag out runs (for the tests).
pub fn dragging(state: &Display) -> bool {
    state.dnd.out.running
}

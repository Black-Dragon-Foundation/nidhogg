//! Session restore: the compositor's session (`xdg_session_manager_v1`) opened with the id C read from its file, the id
//! it gave, a window taken out of it, and the compositor's answer as an action.
//!
//! A frame's session name travels in its `NhgWindowDesc` (`window.rs`), so the window joins the session, as a restore
//! when the name is known, before its first commit.  The shell ignores names while no session is open and after
//! another client has taken the session over.  What C and Lisp keep: the id file, the names and the switch.

use crate::action::NhgAction;
use crate::display::Display;
use crate::guard::GlueError;
use nhshell::api::ShellError;
use nhshell::event::{NHSHELL_EV_SESSION, NhShellEvent};

/// `nhg_session_open` reasons (the shell's `NHSHELL_SESSION_*`): a fresh launch, a recovery after the compositor
/// restarted, a restore of a session saved earlier.
pub const NHG_SESSION_LAUNCH: u32 = 0;
pub const NHG_SESSION_RECOVER: u32 = 1;
pub const NHG_SESSION_RESTORE: u32 = 2;

/// The compositor's answer, in an `NHG_ACT_SESSION`'s `code`: a new session, the saved one, or taken over by another
/// client (names are ignored from then on).
pub const NHG_SESSION_CREATED: i32 = 0;
pub const NHG_SESSION_RESTORED: i32 = 1;
pub const NHG_SESSION_REPLACED: i32 = 2;

/// The compositor answered `nhg_session_open`, or took the session over: `code` is `NHG_SESSION_*`, no window.
pub const NHG_ACT_SESSION: u32 = 41;

/// Open the session for `reason` (`NHG_SESSION_*`); `id` empty asks for a new one.  The answer comes as an
/// `NHG_ACT_SESSION`.  An open session is dropped first.  `NHG_E_UNSUPPORTED` without the protocol.
pub fn open(state: &mut Display, id: &str, reason: u32) -> Result<(), GlueError> {
    if reason > NHG_SESSION_RESTORE {
        return Err(GlueError::Arg);
    }
    let id = (!id.is_empty()).then_some(id);
    Ok(state.backend().session_open(id, reason)?)
}

/// The open session's id.  `NHG_E_STATE` before the compositor has answered.
pub fn id(state: &mut Display) -> Result<String, GlueError> {
    Ok(state.backend().session_id()?)
}

/// Take `window` out of the session, so the compositor forgets its name.  True when it was named, false when it was
/// not (no session, a replaced one, an unnamed window, or a window already gone).
pub fn remove(state: &mut Display, window: u64) -> Result<bool, GlueError> {
    match state.backend().window_session_remove(window) {
        Ok(()) => Ok(true),
        Err(ShellError::State | ShellError::Unsupported) => Ok(false),
        Err(e) => Err(e.into()),
    }
}

/// The session event: an `NHG_ACT_SESSION` with the answer in `code`.
pub fn event(state: &mut Display, ev: &NhShellEvent) {
    debug_assert_eq!(ev.kind, NHSHELL_EV_SESSION);
    // SAFETY: a SESSION record's valid member is `session`.
    let answer = unsafe { ev.u.session }.state;
    let mut a = NhgAction::new(NHG_ACT_SESSION, 0, ev.time_ns);
    a.code = i32::try_from(answer).unwrap_or(NHG_SESSION_REPLACED);
    state.actions.push(a);
}

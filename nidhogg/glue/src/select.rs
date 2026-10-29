//! The clipboard and the primary selection: which of them this process owns, and the reads that wait for another
//! client's.
//!
//! Moved from `src/nhwlselect.c` (`nhwl_sel_ours`, `nhwl_selection_changed`, the shell calls under the `pgtk-*-selection`
//! DEFUNs), without a change in behaviour.  C still converts Lisp to bytes and back: it passes the MIME types and bytes
//! it offers, and takes a read's bytes as plain data.
//!
//! **Ownership** is what tells "we lost it" from "we gave it up": the shell reports every change of a selection, ours or
//! another client's, and only a change of a selection we own, to a source that is not ours, clears it
//! ([`Select::changed`]; the drain raises `SELECTION_CLEAR` then).
//!
//! **Waits are bounded** (the caller's `timeout_ms`, and the shell's `Disconnected` within 100 ms of the connection's
//! loss), and run with the display's lock held, as every glue call does.

use crate::display::Display;
use crate::guard::GlueError;
use nhshell::api::{NHSHELL_SEL_DND, NHSHELL_SEL_PRIMARY, ShellBackend};

/// The selections the shell knows: `NHSHELL_SEL_CLIPBOARD` and `NHSHELL_SEL_PRIMARY`.
const KINDS: usize = 2;

/// What the glue holds for the selections of one display.
#[derive(Debug, Default)]
pub struct Select {
    /// Which selections this process owns, by kind.
    ours: [bool; KINDS],
    /// The bytes of the last read, lent to C until the next read.
    bytes: Vec<u8>,
    /// The MIME types of the last query, each followed by a NUL, lent to C until the next query.
    types: Vec<u8>,
}

/// `kind` as an index, or `Arg`.
fn index(kind: u32) -> Result<usize, GlueError> {
    match kind {
        k if k <= NHSHELL_SEL_PRIMARY => Ok(k as usize),
        _ => Err(GlueError::Arg),
    }
}

/// `kind` for a call that only reads: also the offer of the drag over our windows, `NHSHELL_SEL_DND` (`dnd.rs`).
fn readable(kind: u32) -> Result<(), GlueError> {
    match kind {
        k if k <= NHSHELL_SEL_DND => Ok(()),
        _ => Err(GlueError::Arg),
    }
}

impl Select {
    /// Offer `items` (MIME type and bytes) as selection `kind`.  `State` without keyboard focus (the compositor
    /// refuses), and then nothing is owned.
    pub fn set(
        &mut self,
        backend: &mut dyn ShellBackend,
        kind: u32,
        items: &[(&str, &[u8])],
    ) -> Result<(), GlueError> {
        let k = index(kind)?;
        backend.selection_own(kind, items)?;
        self.ours[k] = true;
        Ok(())
    }

    /// Give selection `kind` up.  True when we owned it; the shell's answer is not asked for, as `nhwl_selection_disown`
    /// never did.
    pub fn disown(&mut self, backend: &mut dyn ShellBackend, kind: u32) -> Result<bool, GlueError> {
        let k = index(kind)?;
        if !self.ours[k] {
            return Ok(false);
        }
        self.ours[k] = false;
        let _ = backend.selection_disown(kind);
        Ok(true)
    }

    /// Whether our source is the current selection `kind`.
    pub fn owned(backend: &dyn ShellBackend, kind: u32) -> Result<bool, GlueError> {
        readable(kind)?;
        Ok(backend.selection_owned(kind)?)
    }

    /// The current types of selection `kind`: the number of them, and the types, each followed by a NUL, in a buffer
    /// that stays valid until the next call.
    pub fn types(
        &mut self,
        backend: &dyn ShellBackend,
        kind: u32,
    ) -> Result<(usize, &[u8]), GlueError> {
        readable(kind)?;
        let types = backend.selection_types(kind)?;
        self.types.clear();
        for t in &types {
            self.types
                .try_reserve(t.len() + 1)
                .map_err(|_| GlueError::NoMem)?;
            self.types.extend_from_slice(t.as_bytes());
            self.types.push(0);
        }
        Ok((types.len(), &self.types))
    }

    /// The type to read the last query's selection as (`types`' buffer, from its NUL-terminated entry): `name` when
    /// the offer has it, else the first of `prefs` it has; `None` for neither.
    pub fn pick(&self, name: &[u8], prefs: &[&[u8]]) -> Option<&[u8]> {
        pick_at(&self.types, name, prefs).map(|at| &self.types[at..])
    }

    /// Selection `kind` as `mime`, within `timeout_ms` (`Timeout`), in a buffer that stays valid until the next call:
    /// our own bytes at once, another client's after a round trip.  `State` without a selection or without that type.
    pub fn read(
        &mut self,
        backend: &mut dyn ShellBackend,
        kind: u32,
        mime: &str,
        timeout_ms: u32,
    ) -> Result<&[u8], GlueError> {
        readable(kind)?;
        self.bytes = Vec::new();
        self.bytes = backend.selection_read(kind, mime, timeout_ms)?;
        Ok(&self.bytes)
    }

    /// The shell says selection `kind` changed.  True when we owned it and another client has it now: the ownership
    /// is cleared, and the caller reports it (`SELECTION_CLEAR`).  A kind the shell does not know, a selection we do
    /// not own, and one we still own are not clears.
    pub fn changed(&mut self, backend: &dyn ShellBackend, kind: u32) -> bool {
        let Ok(k) = index(kind) else {
            return false;
        };
        if !self.ours[k] || backend.selection_owned(kind).unwrap_or(false) {
            return false;
        }
        self.ours[k] = false;
        true
    }
}

/// Where in `types` (MIME types, each followed by a NUL) the type to read starts: `name` when it is one of them, else
/// the first of `prefs` that is.  Moved from `Fpgtk_get_selection_internal` (`src/nhwlselect.c`), whose `member`
/// compared the strings' bytes.
pub fn pick_at(types: &[u8], name: &[u8], prefs: &[&[u8]]) -> Option<usize> {
    let find = |want: &[u8]| {
        let mut at = 0;
        types.split(|&b| b == 0).find_map(|t| {
            let start = at;
            at += t.len() + 1;
            // the last split is the empty tail after the final NUL
            (start < types.len() && t == want).then_some(start)
        })
    };
    find(name).or_else(|| prefs.iter().find_map(|&p| find(p)))
}

/// The display's backend and its selection state, split so that both can be borrowed at once.
fn split(state: &mut Display) -> (&mut Select, &mut dyn ShellBackend) {
    let sh = state.shell();
    // SAFETY: the backend lives as long as the display, and `select` is a field it does not touch.
    let backend = unsafe { &mut *sh };
    (&mut state.select, backend)
}

/// Offer `items` as selection `kind`.
pub fn set(state: &mut Display, kind: u32, items: &[(&str, &[u8])]) -> Result<(), GlueError> {
    let (select, backend) = split(state);
    select.set(backend, kind, items)
}

/// Give selection `kind` up; true when it was ours.
pub fn disown(state: &mut Display, kind: u32) -> Result<bool, GlueError> {
    let (select, backend) = split(state);
    select.disown(backend, kind)
}

/// Whether our source is the current selection `kind`.
pub fn owned(state: &mut Display, kind: u32) -> Result<bool, GlueError> {
    let (_, backend) = split(state);
    Select::owned(backend, kind)
}

/// The types of selection `kind`: their number, and the NUL-terminated list.
pub fn types(state: &mut Display, kind: u32) -> Result<(usize, &[u8]), GlueError> {
    let sh = state.shell();
    // SAFETY: as in `split`.
    let backend = unsafe { &mut *sh };
    state.select.types(backend, kind)
}

/// The bytes of selection `kind` as `mime`.
pub fn read<'a>(
    state: &'a mut Display,
    kind: u32,
    mime: &str,
    timeout_ms: u32,
) -> Result<&'a [u8], GlueError> {
    if kind == NHSHELL_SEL_DND {
        // the drop target accepts with the type Lisp read
        state.dnd.note_read(mime);
    }
    let sh = state.shell();
    // SAFETY: as in `split`.
    let backend = unsafe { &mut *sh };
    state.select.read(backend, kind, mime, timeout_ms)
}

/// The type to read the last `types` query's selection as: `name`, or the first of `prefs` it offers.
pub fn pick<'a>(state: &'a Display, name: &[u8], prefs: &[&[u8]]) -> Option<&'a [u8]> {
    state.select.pick(name, prefs)
}

/// Whether the shell's change of selection `kind` was a loss of ours.
pub fn changed(state: &mut Display, kind: u32) -> bool {
    let (select, backend) = split(state);
    select.changed(backend, kind)
}

//! The keyboard's state and its `KEY` actions.  Moved from `nhwl_key_event` (`src/nhwlterm.c`, adapted from pgtk's
//! `key_press_event`, emacs-31.1) and from the C drain's `KEY` and `MODIFIERS` cases: the modifier mask, the window
//! with the keyboard, and the repeat drop.  The shell has already run each key through xkb and compose, so a key
//! event carries its keysym and the text it produced.  The cascade (keysym to character or symbol, the keysym table,
//! `x-*-keysym`) and the target frame stay C, which reads live Emacs state for both.

use crate::action::{NHG_ACT_KEY, NHG_IN_INPUT_WAITING, NhgAction, NhgNextIn};
use crate::display::Display;
use nhshell::event::NhShellEvent;

/// `KEY` flags: the event is a key repeat.
pub const NHG_KEY_REPEAT: u32 = 1;

/// The keyboard's state.
#[derive(Clone, Copy, Debug, Default)]
pub struct Keyboard {
    /// The `NHSHELL_MOD_*` mask after the newest key or modifiers event (was `dpyinfo->mods`).
    pub mods: u32,
    /// The window that has the keyboard focus, as the last focus event said; 0 for none of ours (was
    /// `dpyinfo->kbd_window`).
    pub window: u64,
}

/// A `KEY` event: one `KEY` action for a press, none for a release.  A repeat is dropped when the call that takes it
/// says input is waiting (`NHG_IN_INPUT_WAITING`, sampled for each call, so a key stored earlier in the same drain
/// counts): the shell holds a repeat back only while the previous one is in its own queue, and the input poll empties
/// that queue into Emacs's even while Lisp is busy, so without this a stalled Lisp would find one repeat per poll.
///
/// The action names the window the event came for, even one the glue forgot: C sends its keys to the frame of that
/// window's tree that Emacs focused, or to the focus frame when it has none.
pub fn key(state: &mut Display, ev: &NhShellEvent, input: NhgNextIn) {
    // SAFETY: a KEY record's valid member is `key`.
    let k = unsafe { ev.u.key };
    if k.pressed == 0 {
        return;
    }
    let repeat = k.repeat != 0;
    if repeat && input.flags & NHG_IN_INPUT_WAITING != 0 {
        return;
    }
    state.keyboard.mods = k.mods;
    let mut a = NhgAction::new(NHG_ACT_KEY, ev.window, ev.time_ns);
    // keysyms fit in 29 bits
    a.code = k.keysym as i32;
    a.mods = k.mods;
    a.flags = if repeat { NHG_KEY_REPEAT } else { 0 };
    let len = k.utf8.iter().position(|&b| b == 0).unwrap_or(k.utf8.len());
    state.actions.push_text(a, Some(k.utf8[..len].to_vec()));
}

/// A `MODIFIERS` event: the mask changes, and no action.
pub fn modifiers(state: &mut Display, ev: &NhShellEvent) {
    // SAFETY: a MODIFIERS record carries `key` (its `mods`).
    state.keyboard.mods = unsafe { ev.u.key }.mods;
}

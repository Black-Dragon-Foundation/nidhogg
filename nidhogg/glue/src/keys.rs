// Upstream commit: a360712c9d272d950d8d8255ef74570f7e90b7d9 (GNU Emacs emacs-31.1), for the "adapted from" notes below.
//! The keyboard's state and its `KEY` actions.  Moved from `nhwl_key_event` (`src/nhwlterm.c`, adapted from pgtk's
//! `key_press_event`, emacs-31.1) and from the C drain's `KEY` and `MODIFIERS` cases: the modifier mask, the window
//! with the keyboard, and the repeat drop.  The shell has already run each key through xkb and compose, so a key
//! event carries its keysym and the text it produced.  The keysym's classification is here too (`classify`, moved
//! from `nhwl_key_event`, adapted from pgtk's `key_press_event`, emacs-31.1): a character the keysym names, a
//! function key, or the text's character.  The `pgtk-keysym-table` lookup between those cases, the modifier bits
//! (`x-*-keysym`) and the target frame stay C, which reads live Emacs state for them.

use crate::action::{NHG_ACT_KEY, NHG_IN_INPUT_WAITING, NhgAction, NhgNextIn};
use crate::display::Display;
use nhshell::event::{KeyEv, Record};

/// `KEY` flags: the event is a key repeat.
pub const NHG_KEY_REPEAT: u32 = 1;
/// `KEY` flags, the keysym's class: it names the character `x` (Latin-1 32–127, or a Unicode keysym), which C takes
/// before the keysym table.
pub const NHG_KEY_CHAR: u32 = 2;
/// `KEY` flags: when the keysym table has nothing, the keysym is a function key (`make_lispy_event` names it).
pub const NHG_KEY_SYMBOL: u32 = 4;
/// `KEY` flags: when the keysym table has nothing, the key gives the text's first character `x`.  With none of
/// `NHG_KEY_CHAR`, `NHG_KEY_SYMBOL` and `NHG_KEY_TEXT`, only the keysym table gives an event.
pub const NHG_KEY_TEXT: u32 = 8;

fn is_cursor_key(k: u32) -> bool {
    (0xff50..0xff60).contains(&k)
}

fn is_misc_function_key(k: u32) -> bool {
    (0xff60..0xff6c).contains(&k)
}

fn is_keypad_key(k: u32) -> bool {
    (0xff80..0xffbe).contains(&k)
}

fn is_function_key(k: u32) -> bool {
    (0xffbe..0xffe1).contains(&k)
}

fn is_modifier_key(k: u32) -> bool {
    (0xffe1..=0xffee).contains(&k) || (0xfe01..=0xfe13).contains(&k) || k == 0xff7e || k == 0xff7f
}

/// The character at the start of `p`, as Emacs's `string_char_and_length` reads its multibyte form (UTF-8, with
/// Emacs's raw-byte and 5-byte extensions), bytes past `p`'s end read as 0 (C read the text's NUL there).
pub fn string_char(p: &[u8]) -> i32 {
    let b = |i: usize| i32::from(p.get(i).copied().unwrap_or(0));
    let c = b(0);
    if c & 0x80 == 0 {
        return c;
    }
    let mut d = (c << 6) + b(1) - ((0xC0 << 6) + 0x80);
    if c & 0x20 == 0 {
        return d + if c < 0xC2 { 0x3FFF80 } else { 0 };
    }
    d = (d << 6) + b(2) - ((0x20 << 12) + 0x80);
    if c & 0x10 == 0 {
        return d;
    }
    d = (d << 6) + b(3) - ((0x10 << 18) + 0x80);
    if c & 0x08 == 0 {
        return d;
    }
    d.wrapping_shl(6)
        .wrapping_add(b(4))
        .wrapping_sub((0x08 << 24) + 0x80)
}

/// The class of a key press with `keysym` and `text` (its UTF-8, `nbytes` long): `NHG_KEY_*` and the character `x`
/// where the class has one.  Moved from `nhwl_key_event`, its cases in their order, the keysym table's lookup (C's)
/// falling between `NHG_KEY_CHAR` and the others.
pub fn classify(keysym: u32, text: &[u8]) -> (u32, i32) {
    let k = keysym;
    // keysyms with defined translations to characters
    if (32..128).contains(&k) {
        return (NHG_KEY_CHAR, k as i32);
    }
    // keysyms directly mapped to Unicode characters
    if (0x0100_0000..=0x0110_FFFF).contains(&k) {
        return (NHG_KEY_CHAR, (k & 0xFF_FFFF) as i32);
    }
    // random non-modifier sorts of keysyms
    if ((0xff08..=0xff1b).contains(&k)
        || k == 0xffff
        || (0xfe20..=0xfe34).contains(&k)
        || is_cursor_key(k)
        || is_misc_function_key(k)
        || (0xfe50..=0xfe5c).contains(&k)
        || is_keypad_key(k)
        || is_function_key(k)
        // any vendor-specific key
        || k & (1 << 28) != 0
        || (k != 0xff_ffff && text.is_empty()))
        && !(is_modifier_key(k) || (0xfe01..=0xfe13).contains(&k))
    {
        return (NHG_KEY_SYMBOL, 0);
    }
    if text.is_empty() {
        return (0, 0);
    }
    // a key that produced text: one multibyte character
    (NHG_KEY_TEXT, string_char(text))
}

/// The keyboard's state.
#[derive(Clone, Copy, Debug, Default)]
pub struct Keyboard {
    /// The `NHSHELL_MOD_*` mask after the newest key or modifiers event (was `dpyinfo->mods`).
    pub mods: u32,
    /// The window that has the keyboard focus, as the last focus event said; 0 for none of ours (was
    /// `dpyinfo->kbd_window`).
    pub window: u64,
}

/// A `KEY` event: one `KEY` action for a press, none for a release, with the keysym's class (`classify`).  A repeat is dropped when the call that takes it
/// says input is waiting (`NHG_IN_INPUT_WAITING`, sampled for each call, so a key stored earlier in the same drain
/// counts): the shell holds a repeat back only while the previous one is in its own queue, and the input poll empties
/// that queue into Emacs's even while Lisp is busy, so without this a stalled Lisp would find one repeat per poll.
///
/// The action names the window the event came for, even one the glue forgot: C sends its keys to the frame of that
/// window's tree that Emacs focused, or to the focus frame when it has none.
pub fn key(state: &mut Display, ev: &Record, k: &KeyEv, input: NhgNextIn) {
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
    let len = k.utf8.iter().position(|&b| b == 0).unwrap_or(k.utf8.len());
    let (class, x) = classify(k.keysym, &k.utf8[..len]);
    a.flags = class | if repeat { NHG_KEY_REPEAT } else { 0 };
    a.x = x;
    state.actions.push_text(a, Some(k.utf8[..len].to_vec()));
}

/// A `MODIFIERS` event: the mask changes, and no action.
pub fn modifiers(state: &mut Display, mods: u32) {
    state.keyboard.mods = mods;
}

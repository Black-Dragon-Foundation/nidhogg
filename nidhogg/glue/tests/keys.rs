//! T1 tests of the keyboard's actions, against nh-recorder (the shell's harness, `../shell/tests/common`, and the
//! step's, `step/mod.rs`): keys, their repeats and text, and keyboard focus.

#[path = "../../shell/tests/common/mod.rs"]
mod common;
mod step;

use common::{Recorder, readable_within};
use nhglue::action::*;
use nhglue::ffi::*;
use nhglue::keys::{NHG_KEY_CHAR, NHG_KEY_REPEAT, NHG_KEY_TEXT};
use std::ptr;
use std::time::{Duration, Instant};
use step::*;

const KEY_E: u32 = 18;
const KEY_A: u32 = 30;
const KEY_APOSTROPHE: u32 = 40;

fn key(rec: &Recorder, code: u32, press: bool) {
    let state = if press { "press" } else { "release" };
    rec.control_ok(&format!(
        r#"{{"cmd":"key","keycode":{code},"state":"{state}"}}"#
    ));
}

/// A window of the recorder with the keyboard, its first configure applied.
fn focused(rec: &Recorder, d: *mut NhgDisplay) -> u64 {
    let id = window(d);
    configured(d, id);
    rec.control_ok(r#"{"cmd":"focus","surface":"toplevel"}"#);
    settle(d);
    id
}

/// One step with `flags`: its return, and its `KEY` actions with their texts (copied before the next step frees
/// them).
fn step_keys(d: *mut NhgDisplay, flags: u32) -> (i32, Vec<(NhgAction, Vec<u8>)>) {
    let input = NhgNextIn { flags };
    let mut list = NhgActionList {
        v: ptr::null(),
        n: 0,
    };
    // SAFETY: a live display; `input` is readable and `list` writable.
    let rc = unsafe { nhg_next(d, &input, &mut list) };
    assert!(rc >= 0, "nhg_next failed: {rc}");
    let mut keys = Vec::new();
    for i in 0..list.n {
        // SAFETY: `n` actions, valid until the next nhg_next.
        let a = unsafe { *list.v.add(i) };
        if a.kind != NHG_ACT_KEY {
            continue;
        }
        assert!(!a.text.is_null(), "a KEY action always has a text");
        // SAFETY: `len` bytes, valid until the next nhg_next.
        let text = unsafe { std::slice::from_raw_parts(a.text.cast::<u8>(), a.len) }.to_vec();
        keys.push((a, text));
    }
    (rc, keys)
}

/// The window with the keyboard, as the glue keeps it.
fn keyboard_window(d: *mut NhgDisplay) -> u64 {
    // SAFETY: a live display.
    unsafe { &*d }.lock_for_test().keyboard().window
}

/// The plan names these `keys::…`.
#[allow(clippy::module_inception)]
mod keys {
    use super::*;

    /// A key held past the repeat delay: the press and a repeat wait in the shell's queue.  The drain takes the
    /// press with nothing waiting and stores it, so the next call says input is waiting, and the repeat it takes gives
    /// no action; with nothing waiting a repeat comes through, flagged.
    #[test]
    fn repeat_after_a_key_in_the_same_drain_is_dropped() {
        let rec = Recorder::start("sway");
        let d = open(&rec);
        let id = focused(&rec, d);
        key(&rec, KEY_A, true);
        // the recorder's delay is 600 ms; the shell keeps one repeat waiting until it is read
        std::thread::sleep(Duration::from_millis(900));
        let mut keys = Vec::new();
        let mut flags = 0;
        let mut silent = 0;
        loop {
            let (rc, v) = step_keys(d, flags);
            if rc == 0 {
                break;
            }
            if v.is_empty() && !keys.is_empty() {
                silent += 1;
            }
            for (a, _) in v {
                keys.push(a);
                // the press is stored: from now on input is waiting
                flags = NHG_IN_INPUT_WAITING;
            }
        }
        assert_eq!(keys.len(), 1, "only the press: {keys:?}");
        assert_eq!(
            (keys[0].window, keys[0].code, keys[0].flags, keys[0].x),
            (id, i32::from(b'a'), NHG_KEY_CHAR, i32::from(b'a'))
        );
        assert!(silent >= 1, "no repeat was taken after the press");
        // nothing waiting: the next repeat gives an action
        let deadline = Instant::now() + Duration::from_secs(3);
        let repeat = loop {
            assert!(Instant::now() < deadline, "no repeat came");
            let (rc, v) = step_keys(d, 0);
            if let Some((a, text)) = v.into_iter().next() {
                break (a, text);
            }
            if rc == 0 {
                // SAFETY: a live display.
                readable_within(unsafe { nhg_fd(d) }, 50);
            }
        };
        assert_eq!(repeat.0.flags, NHG_KEY_REPEAT | NHG_KEY_CHAR);
        assert_eq!(
            (repeat.0.code, repeat.1.as_slice()),
            (i32::from(b'a'), &b"a"[..])
        );
        key(&rec, KEY_A, false);
        close(d);
    }

    /// A dead key and a letter (us intl: ' then e) give one `KEY` action, with the composed keysym and text; the dead
    /// key gives none.  The shell's compose table follows the locale (the C locale reads en_US.UTF-8's).
    #[test]
    fn compose_sequence_gives_one_text() {
        let rec = Recorder::start("sway");
        let d = open(&rec);
        rec.control_ok(
            r#"{"cmd":"keymap","rmlvo":{"rules":"evdev","model":"pc105","layout":"us","variant":"intl","options":""}}"#,
        );
        let id = focused(&rec, d);
        for (code, press) in [
            (KEY_APOSTROPHE, true),
            (KEY_APOSTROPHE, false),
            (KEY_E, true),
            (KEY_E, false),
        ] {
            key(&rec, code, press);
        }
        queued(d);
        let mut keys = Vec::new();
        loop {
            let (rc, v) = step_keys(d, 0);
            keys.extend(v);
            // SAFETY: a live display.
            if rc == 0 && !readable_within(unsafe { nhg_fd(d) }, 200) {
                break;
            }
        }
        assert_eq!(keys.len(), 1, "{keys:?}");
        let (a, text) = &keys[0];
        // XK_eacute: not a character keysym nor a function key, so (failing the keysym table) the text's é
        assert_eq!(
            (a.window, a.code, a.flags, a.x),
            (id, 0xe9, NHG_KEY_TEXT, 0xe9)
        );
        assert_eq!(text.as_slice(), "é".as_bytes());
        close(d);
    }
}

/// The plan names these `focus::…`.
mod focus {
    use super::*;

    /// The keyboard entering the window gives one `FOCUS_IN` for it, and leaving one `FOCUS_OUT`; the glue keeps the
    /// window with the keyboard.
    #[test]
    fn activation_change_gives_one_focus_action() {
        let rec = Recorder::start("sway");
        let d = open(&rec);
        let id = window(d);
        configured(d, id);
        assert_eq!(keyboard_window(d), 0);
        let is_focus = |a: &&NhgAction| a.kind == NHG_ACT_FOCUS_IN || a.kind == NHG_ACT_FOCUS_OUT;
        rec.control_ok(r#"{"cmd":"focus","surface":"toplevel"}"#);
        let seen = settle(d);
        let focus: Vec<_> = seen.iter().filter(is_focus).collect();
        assert_eq!(focus.len(), 1, "{focus:?}");
        assert_eq!((focus[0].kind, focus[0].window), (NHG_ACT_FOCUS_IN, id));
        assert_eq!(keyboard_window(d), id);
        rec.control_ok(r#"{"cmd":"focus","surface":null}"#);
        let seen = settle(d);
        let focus: Vec<_> = seen.iter().filter(is_focus).collect();
        assert_eq!(focus.len(), 1, "{focus:?}");
        assert_eq!((focus[0].kind, focus[0].window), (NHG_ACT_FOCUS_OUT, id));
        assert_eq!(keyboard_window(d), 0);
        close(d);
    }
}

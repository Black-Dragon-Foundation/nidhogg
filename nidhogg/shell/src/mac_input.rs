//! AppKit's keys and mouse as the shared event records: the pure half of the macOS backend's intake, platform-neutral
//! so its tests run on Linux.
//!
//! A key becomes the `KeyEv` the Wayland backend makes from xkb: an X keysym, the `NHSHELL_MOD_*` mask and the text,
//! which the glue classifies (`keys::classify`) and C turns into an Emacs event.  Function keys map through NS's table
//! (`nsterm.m`, `convert_ns_to_X_keysym`); the control characters AppKit gives for Return, Tab, Delete and Escape map
//! to their keysyms.  Option is meta and Command super, NS's defaults (the `ns-*-modifier` variables come later); a key
//! with Control, Option or Command gives no text, so the keysym (the key's character without them) is what Emacs sees.

use crate::event::{
    KeyEv, NHSHELL_MOD_CAPS, NHSHELL_MOD_CTRL, NHSHELL_MOD_META, NHSHELL_MOD_SHIFT,
    NHSHELL_MOD_SUPER, NHSHELL_SCROLL_FINGER, NHSHELL_SCROLL_WHEEL, ScrollEv,
};

/// `NSEventModifierFlags` bits.
pub const NS_CAPS_LOCK: usize = 1 << 16;
pub const NS_SHIFT: usize = 1 << 17;
pub const NS_CONTROL: usize = 1 << 18;
pub const NS_OPTION: usize = 1 << 19;
pub const NS_COMMAND: usize = 1 << 20;

/// NS's function-key characters (`NSUpArrowFunctionKey` …) and the X keysyms NS gives them.
const FUNCTION_KEYS: &[(u32, u32)] = &[
    (0xF700, 0xff52), // up
    (0xF701, 0xff54), // down
    (0xF702, 0xff51), // left
    (0xF703, 0xff53), // right
    (0xF727, 0xff63), // insert
    (0xF728, 0xffff), // delete (forward)
    (0xF729, 0xff50), // home
    (0xF72A, 0xff58), // begin
    (0xF72B, 0xff57), // end
    (0xF72C, 0xff55), // prior
    (0xF72D, 0xff56), // next
    (0xF72E, 0xff61), // print
    (0xF72F, 0xff14), // scroll lock
    (0xF730, 0xff13), // pause
    (0xF731, 0xff15), // sys req
    (0xF732, 0xff6b), // break
    (0xF735, 0xff67), // menu
    (0xF739, 0xff0b), // clear
    (0xF746, 0xff6a), // help
];

/// The keysym of AppKit's character `c` (from `charactersIgnoringModifiers`).
pub fn keysym(c: char) -> u32 {
    let u = c as u32;
    // F1–F35: NSF1FunctionKey on, XK_F1 on
    if (0xF704..=0xF726).contains(&u) {
        return 0xffbe + (u - 0xF704);
    }
    if let Some(&(_, k)) = FUNCTION_KEYS.iter().find(|&&(n, _)| n == u) {
        return k;
    }
    match u {
        0x0d => 0xff0d,        // Return
        0x03 => 0xff8d,        // keypad Enter
        0x09 => 0xff09,        // Tab
        0x19 => 0xfe20,        // Shift-Tab: ISO_Left_Tab
        0x7f | 0x08 => 0xff08, // Delete: BackSpace
        0x1b => 0xff1b,        // Escape
        0x20..=0x7e => u,
        _ => 0x0100_0000 | u,
    }
}

/// The `NHSHELL_MOD_*` mask of `NSEventModifierFlags` `flags`.
pub fn mods(flags: usize) -> u32 {
    let mut m = 0;
    for (ns, ours) in [
        (NS_SHIFT, NHSHELL_MOD_SHIFT),
        (NS_CONTROL, NHSHELL_MOD_CTRL),
        (NS_OPTION, NHSHELL_MOD_META),
        (NS_COMMAND, NHSHELL_MOD_SUPER),
        (NS_CAPS_LOCK, NHSHELL_MOD_CAPS),
    ] {
        if flags & ns != 0 {
            m |= ours;
        }
    }
    m
}

/// A key press: `ignoring` is `charactersIgnoringModifiers`, `chars` is `characters`, `flags` the modifier flags.
/// `None` for a key with no character (a dead key's first press).
pub fn key(ignoring: &str, chars: &str, flags: usize, repeat: bool) -> Option<KeyEv> {
    let c = ignoring.chars().next().or_else(|| chars.chars().next())?;
    let ks = keysym(c);
    let mut k = KeyEv {
        keycode: 0,
        keysym: ks,
        mods: mods(flags),
        pressed: 1,
        repeat: u8::from(repeat),
        utf8: [0; 16],
    };
    // text only from a plain (or shifted) printable key: chords and named keys go by their keysym
    let printable = !(0xfe00..0x0100_0000).contains(&ks);
    if printable && flags & (NS_CONTROL | NS_OPTION | NS_COMMAND) == 0 {
        let text = chars.as_bytes();
        if text.len() < k.utf8.len() && !text.is_empty() {
            k.utf8[..text.len()].copy_from_slice(text);
        }
    }
    Some(k)
}

/// The evdev button code of `NSEvent.buttonNumber` (0 left, 1 right, 2 middle, 3 back, 4 forward).
pub fn button(number: isize) -> u32 {
    match number {
        0 => 0x110,
        1 => 0x111,
        2 => 0x112,
        3 => 0x116,
        4 => 0x115,
        n => 0x110 + n.clamp(0, 15) as u32,
    }
}

/// A wheel record from `scrollingDeltaX`/`Y` (AppKit: positive is up and left).  A mouse wheel (`precise` false)
/// gives whole notches; a trackpad (`precise`) its delta in points, as a finger record.
pub fn wheel(dx: f64, dy: f64, precise: bool) -> ScrollEv {
    if precise {
        ScrollEv {
            dx: -dx,
            dy: -dy,
            source: NHSHELL_SCROLL_FINGER,
            ..ScrollEv::default()
        }
    } else {
        let notches = |d: f64| (-d * 120.0).round() as i32;
        ScrollEv {
            dx: -dx * 10.0,
            dy: -dy * 10.0,
            v120x: notches(dx),
            v120y: notches(dy),
            source: NHSHELL_SCROLL_WHEEL,
            ..ScrollEv::default()
        }
    }
}

/// A point in the window's content view, flipped (AppKit's origin is bottom-left) and in device pixels.
pub fn device_point(x: f64, y: f64, view_height: f64, scale: f64) -> (f64, f64) {
    (x * scale, (view_height - y) * scale)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(k: &KeyEv) -> &str {
        let n = k.utf8.iter().position(|&b| b == 0).unwrap_or(16);
        std::str::from_utf8(&k.utf8[..n]).unwrap()
    }

    #[test]
    fn named_keys() {
        assert_eq!(keysym('\r'), 0xff0d);
        assert_eq!(keysym('\t'), 0xff09);
        assert_eq!(keysym('\u{7f}'), 0xff08);
        assert_eq!(keysym('\u{1b}'), 0xff1b);
        assert_eq!(keysym('\u{F700}'), 0xff52);
        assert_eq!(keysym('\u{F703}'), 0xff53);
        assert_eq!(keysym('\u{F729}'), 0xff50);
        assert_eq!(keysym('\u{F72B}'), 0xff57);
        assert_eq!(keysym('\u{F72C}'), 0xff55);
        assert_eq!(keysym('\u{F72D}'), 0xff56);
        assert_eq!(keysym('\u{F728}'), 0xffff);
        assert_eq!(keysym('\u{F704}'), 0xffbe, "F1");
        assert_eq!(keysym('\u{F70F}'), 0xffc9, "F12");
    }

    #[test]
    fn characters() {
        assert_eq!(keysym('a'), 'a' as u32);
        assert_eq!(keysym('~'), '~' as u32);
        assert_eq!(keysym('é'), 0x0100_00e9);
        let k = key("a", "a", 0, false).unwrap();
        assert_eq!((k.keysym, k.mods, text(&k)), ('a' as u32, 0, "a"));
        let k = key("A", "A", NS_SHIFT, false).unwrap();
        assert_eq!(
            (k.keysym, k.mods, text(&k)),
            ('A' as u32, NHSHELL_MOD_SHIFT, "A")
        );
        let k = key("é", "é", 0, true).unwrap();
        assert_eq!((text(&k), k.repeat), ("é", 1));
    }

    #[test]
    fn chords_carry_no_text() {
        // C-x: AppKit's characters are the control character, the keysym the key's
        let k = key("x", "\u{18}", NS_CONTROL, false).unwrap();
        assert_eq!(
            (k.keysym, k.mods, text(&k)),
            ('x' as u32, NHSHELL_MOD_CTRL, "")
        );
        // M-x (Option): the composed character is dropped
        let k = key("x", "≈", NS_OPTION, false).unwrap();
        assert_eq!(
            (k.keysym, k.mods, text(&k)),
            ('x' as u32, NHSHELL_MOD_META, "")
        );
        let k = key("s", "s", NS_COMMAND, false).unwrap();
        assert_eq!((k.mods, text(&k)), (NHSHELL_MOD_SUPER, ""));
        // named keys never carry their control character
        let k = key("\r", "\r", 0, false).unwrap();
        assert_eq!((k.keysym, text(&k)), (0xff0d, ""));
        assert!(key("", "", 0, false).is_none());
    }

    #[test]
    fn modifier_bits() {
        assert_eq!(mods(0), 0);
        assert_eq!(
            mods(NS_SHIFT | NS_CONTROL | NS_OPTION | NS_COMMAND | NS_CAPS_LOCK),
            NHSHELL_MOD_SHIFT
                | NHSHELL_MOD_CTRL
                | NHSHELL_MOD_META
                | NHSHELL_MOD_SUPER
                | NHSHELL_MOD_CAPS
        );
        // the device-dependent low bits are ignored
        assert_eq!(mods(0x0100 | 0x0002), 0);
    }

    #[test]
    fn mouse() {
        assert_eq!(button(0), 0x110);
        assert_eq!(button(1), 0x111);
        assert_eq!(button(2), 0x112);
        let w = wheel(0.0, 1.0, false);
        assert_eq!(
            (w.v120y, w.source),
            (-120, NHSHELL_SCROLL_WHEEL),
            "a notch up scrolls up"
        );
        let w = wheel(0.0, -2.0, false);
        assert_eq!(w.v120y, 240);
        let w = wheel(0.0, 3.5, true);
        assert_eq!((w.dy, w.v120y, w.source), (-3.5, 0, NHSHELL_SCROLL_FINGER));
        assert_eq!(device_point(10.0, 30.0, 100.0, 2.0), (20.0, 140.0));
    }
}

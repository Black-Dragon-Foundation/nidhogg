//! The keyboard.  T0 tests drive `Xkb` and `RepeatState` directly (they need xkeyboard-config and the X11 locale
//! data, which the check container installs); the T1 test runs against nh-recorder (harness: `common/mod.rs`).

mod common;

#[allow(clippy::module_inception)]
mod keyboard {
    use super::common::*;
    use nhshell::api::ShellBackend;
    use nhshell::event::*;
    use nhshell::wayland::keyboard::{RepeatState, Xkb};
    use std::ffi::OsStr;
    use std::time::{Duration, Instant};

    const KEY_LEFTCTRL: u32 = 29;
    const KEY_A: u32 = 30;
    const KEY_E: u32 = 18;
    const KEY_APOSTROPHE: u32 = 40;
    const KEY_LEFTALT: u32 = 56;
    const KEY_CAPSLOCK: u32 = 58;
    const KEY_LEFTMETA: u32 = 125;

    fn sym_name(sym: u32) -> String {
        xkbcommon::xkb::keysym_get_name(xkbcommon::xkb::Keysym::new(sym))
    }

    #[test]
    fn user_keymap_swap_option() {
        let mut x = Xkb::from_rmlvo(
            ["evdev", "pc105", "us", "", "ctrl:swap_lalt_lctl_lwin"],
            OsStr::new("C"),
        )
        .expect("keymap");
        let k = x.key(KEY_LEFTALT, true).unwrap();
        assert_eq!(sym_name(k.keysym), "Control_L");
        x.track_key(KEY_LEFTALT, true);
        assert_eq!(x.mods() & NHSHELL_MOD_CTRL, NHSHELL_MOD_CTRL);
        x.track_key(KEY_LEFTALT, false);
        // Left Alt + a is C-a
        x.track_key(KEY_LEFTALT, true);
        let a = x.key(KEY_A, true).unwrap();
        assert_eq!(a.keysym, u32::from(b'a'));
        assert_eq!(x.mods(), NHSHELL_MOD_CTRL);
        x.track_key(KEY_LEFTALT, false);
        // the key where Super was is Alt
        let k = x.key(KEY_LEFTMETA, true).unwrap();
        assert_eq!(sym_name(k.keysym), "Alt_L");
        x.track_key(KEY_LEFTMETA, true);
        assert_eq!(x.mods() & NHSHELL_MOD_ALT, NHSHELL_MOD_ALT);
        x.track_key(KEY_LEFTMETA, false);
        // and the physical Left Ctrl is Super
        let k = x.key(KEY_LEFTCTRL, true).unwrap();
        assert_eq!(sym_name(k.keysym), "Super_L");
        x.track_key(KEY_LEFTCTRL, true);
        assert_eq!(x.mods(), NHSHELL_MOD_SUPER);
    }

    #[test]
    fn hyper_super_meta_bits() {
        // caps:hyper makes Caps Lock a Hyper key on Mod3; the Windows key is Super on Mod4
        let mut x = Xkb::from_rmlvo(["evdev", "pc105", "us", "", "caps:hyper"], OsStr::new("C"))
            .expect("keymap");
        assert_eq!(x.mods(), 0);
        x.track_key(KEY_CAPSLOCK, true);
        assert_eq!(x.mods(), NHSHELL_MOD_HYPER);
        x.track_key(KEY_CAPSLOCK, false);
        assert_eq!(x.mods(), 0);
        x.track_key(KEY_LEFTMETA, true);
        assert_eq!(x.mods(), NHSHELL_MOD_SUPER);
        x.track_key(KEY_CAPSLOCK, true);
        assert_eq!(x.mods(), NHSHELL_MOD_SUPER | NHSHELL_MOD_HYPER);
    }

    #[test]
    fn compose_dead_acute() {
        let mut x = Xkb::from_rmlvo(
            ["evdev", "pc105", "us", "intl", ""],
            OsStr::new("en_US.UTF-8"),
        )
        .expect("keymap");
        assert!(
            x.key(KEY_APOSTROPHE, true).is_none(),
            "dead_acute starts a sequence: no KEY"
        );
        assert!(x.key(KEY_APOSTROPHE, false).is_none());
        let k = x.key(KEY_E, true).expect("the composed key");
        assert_eq!(sym_name(k.keysym), "eacute");
        let end = k.utf8.iter().position(|&b| b == 0).unwrap();
        assert_eq!(std::str::from_utf8(&k.utf8[..end]).unwrap(), "é");
    }

    #[test]
    fn repeat_coalesces_while_unread() {
        let mut r = RepeatState::default();
        r.set_info(25, 400);
        r.press(KEY_A, 0);
        // the fake clock runs 2 s in 10 ms steps and nothing is ever read
        let mut queued = 0;
        for now in (0..=2000).step_by(10) {
            if r.tick(now, queued > 0).is_some() {
                queued += 1;
            }
        }
        assert_eq!(queued, 1, "one repeat waits, not 50");
        // once the host reads it, the next period posts another
        assert_eq!(r.tick(2100, false), Some(KEY_A));
        // a release stops the repeat
        r.release(KEY_A);
        assert_eq!(r.tick(3000, false), None);
        assert_eq!(r.deadline(), None);
    }

    #[test]
    fn repeat_follows_rate_and_delay() {
        let mut r = RepeatState::default();
        r.set_info(25, 400);
        r.press(KEY_A, 1000);
        assert_eq!(r.deadline(), Some(1400));
        assert_eq!(r.tick(1399, false), None);
        assert_eq!(r.tick(1400, false), Some(KEY_A));
        assert_eq!(r.deadline(), Some(1440));
        r.set_info(0, 400);
        r.press(KEY_A, 5000);
        assert_eq!(r.deadline(), None, "rate 0 disables repeat");
    }

    #[test]
    fn keys_arrive_with_focus() {
        let rec = Recorder::start("sway");
        let mut shell = rec.open();
        let surface = shell.surface_new().unwrap();
        shell.flush().unwrap();
        let reply = rec.control_ok(
            r#"{"cmd":"keymap","rmlvo":{"rules":"evdev","model":"pc105","layout":"us","variant":"","options":""}}"#,
        );
        assert!(reply.contains("\"ok\":true"), "{reply}");
        let reply = rec.control_ok(&format!(r#"{{"cmd":"focus","surface":{surface}}}"#));
        assert!(reply.contains("\"ok\":true"), "{reply}");
        let reply = rec.control_ok(r#"{"cmd":"key","text":"a"}"#);
        assert!(reply.contains("\"ok\":true"), "{reply}");
        // FOCUS, then the press and the release of "a"
        let mut keys = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(3);
        while keys.len() < 2 && Instant::now() < deadline {
            readable_within(shell.wake_fd(), 100);
            while let Some(ev) = shell.next_event().unwrap() {
                if let ShellEvent::Key(k) = ev.event {
                    keys.push((ev, k));
                }
            }
        }
        assert_eq!(keys.len(), 2, "a press and a release");
        for ((ev, k), pressed) in keys.iter().zip([1u8, 0]) {
            assert_eq!(ev.window, surface);
            assert_eq!((k.keycode, k.pressed, k.repeat), (KEY_A, pressed, 0));
            assert_eq!(k.keysym, u32::from(b'a'));
            assert_eq!(&k.utf8[..2], b"a\0");
        }
    }
}

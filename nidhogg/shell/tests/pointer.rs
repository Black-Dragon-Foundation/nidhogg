//! T1 tests of the pointer against nh-recorder: motion in device pixels, scroll frames, cursor shapes and press serials.
//! The harness is `common/mod.rs`.

mod common;

use common::*;
use nhshell::api::{NHSHELL_CURSOR_TEXT, ShellBackend, WindowDesc};
use nhshell::event::*;
use nhshell::wayland::WaylandShell;
use std::time::{Duration, Instant};

/// A shell on a fresh recorder with one window that has its first configure and the keyboard focus.
fn setup(profile: &str) -> (Recorder, WaylandShell, u64) {
    let rec = Recorder::start(profile);
    let mut shell = rec.open();
    let id = shell
        .window_new(&WindowDesc {
            w: 200,
            h: 100,
            title: "t".into(),
            app_id: "nidhogg.test".into(),
            flags: 0,
        })
        .unwrap();
    wait_event(&mut shell, NHSHELL_EV_CONFIGURE);
    assert!(
        rec.control(r#"{"cmd":"focus","surface":"toplevel"}"#)
            .contains("\"ok\":true")
    );
    (rec, shell, id)
}

fn pointer_ev(ev: &NhShellEvent) -> PointerEv {
    // SAFETY: pointer records carry `pointer`.
    unsafe { ev.u.pointer }
}

fn scroll_ev(ev: &NhShellEvent) -> ScrollEv {
    // SAFETY: SCROLL records carry `scroll`.
    unsafe { ev.u.scroll }
}

#[allow(clippy::module_inception)]
mod pointer {
    use super::*;

    #[test]
    fn motion_in_device_px() {
        let (rec, mut shell, id) = setup("mutter");
        rec.control(r#"{"cmd":"scale","value120":180}"#);
        wait_for(&mut shell, |e| {
            // SAFETY: SCALE records carry `scale`.
            e.kind == NHSHELL_EV_SCALE && unsafe { e.u.scale.scale120 } == 180
        });
        rec.control(r#"{"cmd":"pointer","pos":[100,50]}"#);
        let enter = wait_event(&mut shell, NHSHELL_EV_POINTER_ENTER);
        assert_eq!(enter.window, id);
        // the compositor reports the first position with the enter itself
        let e = pointer_ev(&enter);
        assert_eq!((e.x, e.y), (150.0, 75.0));
        rec.control(r#"{"cmd":"pointer","pos":[102,50]}"#);
        let m = pointer_ev(&wait_event(&mut shell, NHSHELL_EV_POINTER_MOTION));
        assert_eq!((m.x, m.y), (153.0, 75.0));
    }

    #[test]
    fn value120_with_seat_8() {
        let (rec, mut shell, _) = setup("mutter");
        rec.control(r#"{"cmd":"pointer","pos":[10,10]}"#);
        rec.control(r#"{"cmd":"axis","source":"wheel","v120":120}"#);
        let s = scroll_ev(&wait_event(&mut shell, NHSHELL_EV_SCROLL));
        assert_eq!((s.v120x, s.v120y), (0, 120));
        assert_eq!(s.dy, 15.0);
        assert_eq!((s.source, s.stop), (NHSHELL_SCROLL_WHEEL, 0));
    }

    #[test]
    fn stop_sets_flag() {
        let (rec, mut shell, _) = setup("mutter");
        rec.control(r#"{"cmd":"pointer","pos":[10,10]}"#);
        rec.control(r#"{"cmd":"axis","source":"finger","stop":true}"#);
        let s = scroll_ev(&wait_event(&mut shell, NHSHELL_EV_SCROLL));
        assert_eq!((s.source, s.stop), (NHSHELL_SCROLL_FINGER, 1));
    }

    #[test]
    fn cursor_shape_after_enter() {
        let (rec, mut shell, id) = setup("mutter");
        assert!(shell.caps().cursor_shape > 0);
        rec.control(r#"{"cmd":"pointer","pos":[10,10]}"#);
        wait_event(&mut shell, NHSHELL_EV_POINTER_ENTER);
        shell.set_cursor(id, NHSHELL_CURSOR_TEXT).unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            let log = rec.control(r#"{"cmd":"input_log","since":1}"#);
            if log.contains(r#""req":"set_shape""#) && log.contains(r#""shape":"text""#) {
                break;
            }
            assert!(Instant::now() < deadline, "no set_shape in {log}");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// Move, resize and menus carry the serial of the latest press.
    #[test]
    fn move_carries_press_serial() {
        let (rec, mut shell, id) = setup("mutter");
        rec.control(r#"{"cmd":"pointer","pos":[10,10]}"#);
        let mut seen = Vec::new();
        for _ in 0..2 {
            rec.control(r#"{"cmd":"button","button":272,"state":"press"}"#);
            wait_for(&mut shell, |e| {
                e.kind == NHSHELL_EV_BUTTON && pointer_ev(e).state == NHSHELL_BUTTON_PRESSED
            });
            let press = shell.latest_press_serial();
            assert_ne!(press, 0);
            shell.move_window(id).unwrap();
            let deadline = Instant::now() + Duration::from_secs(3);
            let moved = loop {
                let log = rec.control(r#"{"cmd":"input_log","since":1}"#);
                if let Some(at) = log.rfind(r#""req":"xdg_toplevel.move""#)
                    && let Some(s) = number(&log[at..], "serial")
                    && s == u64::from(press)
                {
                    break s;
                }
                assert!(Instant::now() < deadline, "no move with serial {press}");
                std::thread::sleep(Duration::from_millis(10));
            };
            seen.push(moved);
            rec.control(r#"{"cmd":"button","button":272,"state":"release"}"#);
        }
        assert_ne!(seen[0], seen[1], "each press has its own serial");
    }
}

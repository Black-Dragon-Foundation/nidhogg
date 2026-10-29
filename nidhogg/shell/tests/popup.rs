//! T1 tests of grabbing popups (menus) against nh-recorder: the grab's serial and the rules that withhold it, the
//! destroy order of nested popups, popup resize, and keys on a grabbing popup.  The recorder posts xdg-shell's real
//! errors (`invalid_grab`, `not_the_topmost_popup`) as protocol errors, which end the connection.  The harness is
//! `common/mod.rs`.

mod common;

use common::*;
use nhshell::api::*;
use nhshell::event::*;
use nhshell::wayland::WaylandShell;

const BTN_LEFT: u32 = 0x110;

/// A popup of `parent`, 60x30 logical pixels below and right of (10, 10, 4, 4); `flags` is 0 or `NHSHELL_WIN_GRAB`.
fn popup(shell: &mut WaylandShell, parent: u64, flags: u32) -> u64 {
    shell
        .child_new(&ChildDesc {
            kind: NHSHELL_KIND_POPUP,
            parent,
            x: 0,
            y: 0,
            w: 60,
            h: 30,
            anchor: Anchor {
                x: 10,
                y: 10,
                w: 4,
                h: 4,
                gravity: 8,
                adjust: 0,
            },
            flags,
        })
        .unwrap()
}

/// The recorder presses (and with `release`, releases) the left button over `surface`; the shell has read the press.
fn click(rec: &Recorder, shell: &mut WaylandShell, surface: u64, release: bool) {
    rec.control_ok(&format!(r#"{{"cmd":"focus","surface":{surface}}}"#));
    rec.control_ok(r#"{"cmd":"pointer","pos":[5,5]}"#);
    rec.control_ok(r#"{"cmd":"button","button":272,"state":"press"}"#);
    wait_for(
        shell,
        |e| matches!(e.event, ShellEvent::Button(p) if p.state == NHSHELL_BUTTON_PRESSED),
    );
    if release {
        rec.control_ok(r#"{"cmd":"button","button":272,"state":"release"}"#);
        wait_for(
            shell,
            |e| matches!(e.event, ShellEvent::Button(p) if p.button == BTN_LEFT),
        );
    }
}

fn grab_of(kind: &str) -> String {
    format!("\"grab\":{{\"serial_kind\":\"{kind}\"}}")
}

/// Popup `id` reached the recorder without a grab.
fn no_grab(rec: &Recorder, id: u64) {
    rec.wait_node(id, |n| {
        n.contains("\"role\":\"popup\"") && n.contains("\"grab\":null")
    });
}

/// The connection is still up within `ms`: no protocol error ended it, and no `POPUP_DONE` came.
fn still_up(shell: &mut WaylandShell, ms: i64) {
    for ev in collect(shell, ms) {
        assert_ne!(
            ev.kind(),
            Kind::Disconnected,
            "a protocol error ended the connection"
        );
        assert_ne!(ev.kind(), Kind::PopupDone, "popup {} dismissed", ev.window);
    }
    shell.dispatch_main().expect("the connection is up");
}

#[allow(clippy::module_inception)]
mod popup {
    use super::*;

    /// W2-5: the newest of button press, key press and touch down; here a key press after a button press.
    #[test]
    fn grab_uses_the_newest_press_serial() {
        let rec = Recorder::start("sway");
        let mut shell = rec.open();
        let top = mapped_window(&mut shell, 200, 100);
        click(&rec, &mut shell, top, true);
        rec.control_ok(r#"{"cmd":"key","text":"a"}"#);
        wait_event(&mut shell, Kind::Key);
        let p = popup(&mut shell, top, NHSHELL_WIN_GRAB);
        rec.wait_node(p, |n| n.contains(&grab_of("key")));
        assert!(shell.window_grabbed(p).unwrap());
        assert!(!shell.window_grabbed(top).unwrap());
        still_up(&mut shell, 100);
        // a newer button press: the next grab names it
        shell.window_destroy(p).unwrap();
        click(&rec, &mut shell, top, false);
        let q = popup(&mut shell, top, NHSHELL_WIN_GRAB);
        rec.wait_node(q, |n| n.contains(&grab_of("button")));
    }

    #[test]
    fn no_grab_without_a_serial() {
        let rec = Recorder::start("sway");
        let mut shell = rec.open();
        let top = mapped_window(&mut shell, 200, 100);
        let p = popup(&mut shell, top, NHSHELL_WIN_GRAB);
        no_grab(&rec, p);
        assert!(!shell.window_grabbed(p).unwrap());
        still_up(&mut shell, 150);
    }

    /// A drag holds the pointer: another client's drag over our window, after a press and its release.
    #[test]
    fn no_grab_during_a_drag() {
        let rec = Recorder::start("sway");
        let mut shell = rec.open();
        let top = mapped_window(&mut shell, 200, 100);
        click(&rec, &mut shell, top, true);
        let r = rec.control_ok(
            r#"{"cmd":"dnd_offer","types":{"text/plain;charset=utf-8":"x"},"surface":"toplevel","at":[20,20]}"#,
        );
        assert!(r.contains("\"ok\":true"), "{r}");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        while !shell.dragging() {
            assert!(
                std::time::Instant::now() < deadline,
                "the drag never entered"
            );
            collect(&mut shell, 20);
        }
        let p = popup(&mut shell, top, NHSHELL_WIN_GRAB);
        no_grab(&rec, p);
        assert!(!shell.window_grabbed(p).unwrap());
        // the drag leaves: grabs are allowed again
        rec.control_ok(r#"{"cmd":"dnd_leave"}"#);
        while shell.dragging() {
            assert!(std::time::Instant::now() < deadline, "the drag never left");
            collect(&mut shell, 20);
        }
    }

    /// W2-5: a grabbing popup's parent popup must hold the topmost grab; a plain parent means no grab (the recorder
    /// would answer a grab with `invalid_grab`).
    #[test]
    fn no_grab_under_a_non_grabbing_parent() {
        let rec = Recorder::start("sway");
        let mut shell = rec.open();
        let top = mapped_window(&mut shell, 200, 100);
        click(&rec, &mut shell, top, false);
        let tip = popup(&mut shell, top, 0);
        let p = popup(&mut shell, tip, NHSHELL_WIN_GRAB);
        no_grab(&rec, p);
        assert!(!shell.window_grabbed(p).unwrap());
        // and while a grab chain is open, a second grab beside it (on the top-level) is withheld too
        let a = popup(&mut shell, top, NHSHELL_WIN_GRAB);
        let b = popup(&mut shell, a, NHSHELL_WIN_GRAB);
        let c = popup(&mut shell, a, NHSHELL_WIN_GRAB);
        let d = popup(&mut shell, top, NHSHELL_WIN_GRAB);
        rec.wait_node(b, |n| n.contains(&grab_of("button")));
        no_grab(&rec, c);
        no_grab(&rec, d);
        still_up(&mut shell, 150);
    }

    /// Review Focus 1: three nested grabbing popups; destroying the first destroys them topmost first, so the
    /// recorder raises no `not_the_topmost_popup`, and the tree keeps only the top-level.  The same holds when the
    /// top-level itself goes (the parent frame deleted with a submenu open).
    #[test]
    fn nested_popups_destroyed_topmost_first() {
        let rec = Recorder::start("sway");
        let mut shell = rec.open();
        let top = mapped_window(&mut shell, 200, 100);
        click(&rec, &mut shell, top, false);
        let a = popup(&mut shell, top, NHSHELL_WIN_GRAB);
        let b = popup(&mut shell, a, NHSHELL_WIN_GRAB);
        let c = popup(&mut shell, b, NHSHELL_WIN_GRAB);
        for id in [a, b, c] {
            rec.wait_node(id, |n| n.contains(&grab_of("button")));
        }
        shell.window_destroy(a).unwrap();
        still_up(&mut shell, 200);
        assert_eq!(rec.surfaces("popup"), 0);
        assert_eq!(rec.surfaces("toplevel"), 1, "only the top-level is left");
        assert!(rec.control(r#"{"cmd":"clients"}"#).contains("\"pid\""));

        // a new chain grabs again (no popup holds a grab), then its top-level goes
        let a = popup(&mut shell, top, NHSHELL_WIN_GRAB);
        let b = popup(&mut shell, a, NHSHELL_WIN_GRAB);
        let c = popup(&mut shell, b, NHSHELL_WIN_GRAB);
        rec.wait_node(c, |n| n.contains(&grab_of("button")));
        shell.window_destroy(top).unwrap();
        still_up(&mut shell, 200);
        assert_eq!(rec.surfaces("popup") + rec.surfaces("toplevel"), 0);
    }

    /// After the compositor dismisses a chain (an outside click), the popups stay until the host destroys them, and
    /// that destroy is still topmost first.
    #[test]
    fn dismissed_chain_destroyed_cleanly() {
        let rec = Recorder::start("sway");
        let mut shell = rec.open();
        let top = mapped_window(&mut shell, 200, 100);
        click(&rec, &mut shell, top, false);
        let a = popup(&mut shell, top, NHSHELL_WIN_GRAB);
        let b = popup(&mut shell, a, NHSHELL_WIN_GRAB);
        rec.wait_node(b, |n| n.contains(&grab_of("button")));
        rec.control_ok(r#"{"cmd":"click_outside"}"#);
        let first = wait_event(&mut shell, Kind::PopupDone);
        let second = wait_event(&mut shell, Kind::PopupDone);
        assert_eq!((first.window, second.window), (b, a), "topmost first");
        assert!(!shell.window_grabbed(a).unwrap() && !shell.window_grabbed(b).unwrap());
        shell.window_destroy(a).unwrap();
        for ev in collect(&mut shell, 200) {
            assert_ne!(ev.kind(), Kind::Disconnected);
        }
        shell.dispatch_main().expect("the connection is up");
        assert_eq!(rec.surfaces("popup"), 0);
    }

    /// v3: a new positioner and `reposition`; the next configure is a content ack, and a buffer of the new size
    /// maps it.  `xdg-v2`: the popup is made again, and the call says "redraw all".
    #[test]
    fn set_size_repositions() {
        for (profile, recreated) in [("sway", false), ("xdg-v2", true)] {
            let rec = Recorder::start(profile);
            let mut shell = rec.open();
            let top = mapped_window(&mut shell, 200, 100);
            let p = popup(&mut shell, top, 0);
            wait_for(&mut shell, |e| {
                e.kind() == Kind::FrameReady && e.window == p
            });
            draw(&mut shell, p, 60, 30);
            rec.wait_node(p, |n| n.contains("\"buffer\":{\"h\":30,\"w\":60}"));
            assert!(
                !shell.set_size(p, 60, 30).unwrap(),
                "same size: nothing to do"
            );
            assert_eq!(shell.set_size(p, 80, 40).unwrap(), recreated, "{profile}");
            wait_for(&mut shell, |e| {
                e.kind() == Kind::FrameReady && e.window == p
            });
            let ready = shell.window_ready(p).unwrap();
            assert_ne!(ready & NHSHELL_READY_CONTENT, 0, "{profile}: {ready}");
            assert_ne!(
                ready & NHSHELL_READY_ACK,
                0,
                "{profile}: a content ack waits"
            );
            draw(&mut shell, p, 80, 40);
            let n = rec.wait_node(p, |n| n.contains("\"buffer\":{\"h\":40,\"w\":80}"));
            assert!(n.contains("\"position\":[14,14]"), "{profile}: {n}");
            assert!(shell.set_size(top, 10, 10).is_err(), "not a popup");
            still_up(&mut shell, 100);
        }
    }

    /// Keyboard focus on a grabbing popup: its FOCUS and its keys carry the popup's id.
    #[test]
    fn keys_on_a_grabbing_popup_carry_its_id() {
        let rec = Recorder::start("sway");
        let mut shell = rec.open();
        let top = mapped_window(&mut shell, 200, 100);
        click(&rec, &mut shell, top, false);
        let p = popup(&mut shell, top, NHSHELL_WIN_GRAB);
        wait_for(&mut shell, |e| {
            e.kind() == Kind::FrameReady && e.window == p
        });
        draw(&mut shell, p, 60, 30);
        rec.wait_node(p, |n| {
            n.contains(&grab_of("button")) && n.contains("\"mapped\":true")
        });
        rec.control_ok(&format!(r#"{{"cmd":"focus","surface":{p}}}"#));
        let focus = wait_for(&mut shell, |e| e.kind() == Kind::Focus && e.window == p);
        let ShellEvent::Focus(f) = focus.event else {
            unreachable!()
        };
        assert_eq!(f.r#in, 1);
        rec.control_ok(r#"{"cmd":"key","text":"x"}"#);
        let key = wait_event(&mut shell, Kind::Key);
        assert_eq!(key.window, p);
        let ShellEvent::Key(k) = key.event else {
            unreachable!()
        };
        assert_eq!(k.utf8[0], b'x');
    }
}

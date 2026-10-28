//! T1 tests of reconnecting in place: the recorder restarts (`restart`: the socket kept, bound again, or gone until
//! `restart_up`) and the shell connects again with its windows, pools and input.  The harness is `common/mod.rs`.
//!
//! The descriptor and thread counts of `twenty_reconnects_free_everything` are the process's, so every test here holds
//! `SERIAL`: that one exclusively, the others shared.

mod common;

use common::*;
use nhshell::api::*;
use nhshell::caps::NhShellCaps;
use nhshell::event::*;
use nhshell::wayland::WaylandShell;
use std::sync::RwLock;
use std::time::{Duration, Instant};

static SERIAL: RwLock<()> = RwLock::new(());

/// The `wl_pointer` button code of the left button.
const BTN_LEFT: u32 = 272;
/// The evdev code of "a".
const KEY_A: u32 = 30;

mod reconnect {
    use super::*;

    fn shared() -> std::sync::RwLockReadGuard<'static, ()> {
        SERIAL.read().unwrap_or_else(|e| e.into_inner())
    }

    /// A shell on `rec` with reconnecting on.
    fn open(rec: &Recorder) -> WaylandShell {
        let mut shell = rec.open();
        shell.set_reconnect(true).unwrap();
        shell
    }

    fn restart(rec: &Recorder, req: &str) {
        let reply = rec.control(req);
        assert!(reply.contains("\"ok\":true"), "{reply}");
    }

    /// Every record up to and including `RECONNECTING`, within 3 s; no `DISCONNECTED` on the way.
    fn until_reconnecting(shell: &mut WaylandShell) -> Vec<NhShellEvent> {
        let deadline = Instant::now() + Duration::from_secs(3);
        let mut out = Vec::new();
        loop {
            while let Some(ev) = shell.next_event().unwrap() {
                assert_ne!(ev.kind, NHSHELL_EV_DISCONNECTED, "a hangup is not a loss");
                out.push(ev);
                if ev.kind == NHSHELL_EV_RECONNECTING {
                    return out;
                }
            }
            assert!(Instant::now() < deadline, "no RECONNECTING");
            readable_within(shell.wake_fd(), 50);
        }
    }

    /// Drive `reconnect` as the host's backoff timer would (a 50 ms step every 10 ms) until it succeeds, within
    /// `secs`; then take `RECONNECTED`.
    fn reconnect_within(shell: &mut WaylandShell, secs: u64) {
        reconnect_records(shell, secs);
    }

    /// `reconnect_within`, returning every record up to and including `RECONNECTED`.
    fn reconnect_records(shell: &mut WaylandShell, secs: u64) -> Vec<NhShellEvent> {
        let deadline = Instant::now() + Duration::from_secs(secs);
        loop {
            match shell.reconnect(50) {
                Ok(()) => break,
                Err(ShellError::Timeout) => {
                    assert!(Instant::now() < deadline, "never reconnected");
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(e) => panic!("reconnect: {e:?}"),
            }
        }
        assert_eq!(shell.connection_state(), ConnectionState::Up);
        let mut out = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            while let Some(ev) = shell.next_event().unwrap() {
                out.push(ev);
                if ev.kind == NHSHELL_EV_RECONNECTED {
                    return out;
                }
            }
            assert!(Instant::now() < deadline, "no RECONNECTED");
            readable_within(shell.wake_fd(), 50);
        }
    }

    /// A keep-socket restart, and the reconnect after it: the records up to `RECONNECTED`.
    fn restart_and_reconnect(rec: &Recorder, shell: &mut WaylandShell) -> Vec<NhShellEvent> {
        restart(rec, r#"{"cmd":"restart","mode":"keep-socket"}"#);
        until_reconnecting(shell);
        reconnect_records(shell, 3)
    }

    /// The recorder's next commit number: the commits from it on are the new connection's.
    fn next_seq(rec: &Recorder) -> u64 {
        number(&rec.control(r#"{"cmd":"log","since":1}"#), "next").expect("next")
    }

    /// Surface `pid`'s commits from recorder commit `seq` on, one JSON text each (sorted keys: `ack_serial` opens
    /// each).
    fn commits(rec: &Recorder, seq: u64, pid: u32) -> Vec<String> {
        let log = rec.control(&format!(r#"{{"cmd":"log","since":{seq}}}"#));
        log.split("{\"ack_serial\":")
            .skip(1)
            .map(|c| format!("{{\"ack_serial\":{c}"))
            .filter(|c| c.contains(&format!("\"surface\":{pid},")))
            .collect()
    }

    /// The next `CONFIGURE` of window `w`.
    fn configure_of(shell: &mut WaylandShell, w: u64) -> ConfigureEv {
        let ev = wait_for(shell, |e| e.kind == NHSHELL_EV_CONFIGURE && e.window == w);
        // SAFETY: a CONFIGURE record carries `configure`.
        unsafe { ev.u.configure }
    }

    /// The string after `"data":` in a recorder reply (plain text without escapes).
    fn data_of(reply: &str) -> Option<&str> {
        let start = reply.find("\"data\":\"")? + 8;
        let len = reply[start..].find('"')?;
        Some(&reply[start..start + len])
    }

    /// Give window `w` keyboard focus (its surface now: `pid`) and take the `FOCUS` record, which names `w`.
    fn focus(rec: &Recorder, shell: &mut WaylandShell, w: u64) {
        let pid = shell.surface_protocol_id(w).unwrap();
        rec.control_ok(&format!(r#"{{"cmd":"focus","surface":{pid}}}"#));
        wait_for(shell, |e| {
            // SAFETY: a FOCUS record carries `focus`.
            e.kind == NHSHELL_EV_FOCUS && e.window == w && unsafe { e.u.focus.r#in } == 1
        });
    }

    #[test]
    fn epipe_posts_reconnecting_not_disconnected() {
        let _g = shared();
        let rec = Recorder::start("sway");
        let mut shell = open(&rec);
        let w = mapped_window(&mut shell, 64, 48);
        assert_eq!(shell.connection_state(), ConnectionState::Up);
        restart(&rec, r#"{"cmd":"restart","mode":"gone"}"#);
        let evs = until_reconnecting(&mut shell);
        assert_eq!(evs.last().unwrap().kind, NHSHELL_EV_RECONNECTING);
        assert_eq!(shell.connection_state(), ConnectionState::Reconnecting);
        assert_eq!(shell.last_error(), "", "a hangup is not a protocol error");
        // nothing of the old connection comes after it
        let after = collect(&mut shell, 300);
        assert!(
            after.is_empty(),
            "{} records after RECONNECTING",
            after.len()
        );
        assert_eq!(shell.set_title(w, "x"), Err(ShellError::Reconnecting));
    }

    #[test]
    fn protocol_error_still_disconnects() {
        let _g = shared();
        let rec = Recorder::start("sway");
        let mut shell = open(&rec);
        let w = mapped_window(&mut shell, 64, 48);
        // a configure serial the compositor never sent: xdg_wm_base's invalid_surface_state
        shell.ack_configure(w, 999_999, 0).unwrap();
        let _ = shell.present_bare(w);
        let _ = shell.flush();
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            match shell.next_event() {
                Ok(Some(ev)) => assert_ne!(ev.kind, NHSHELL_EV_RECONNECTING),
                Ok(None) => {
                    assert!(Instant::now() < deadline, "no DISCONNECTED");
                    readable_within(shell.wake_fd(), 50);
                }
                Err(e) => {
                    assert_eq!(e, ShellError::Disconnected);
                    break;
                }
            }
        }
        assert_eq!(shell.connection_state(), ConnectionState::Lost);
        assert!(
            shell.last_error().contains(": error 4: "),
            "{}",
            shell.last_error()
        );
        assert_eq!(shell.reconnect(50), Err(ShellError::Disconnected));
    }

    #[test]
    fn calls_while_reconnecting_return_reconnecting() {
        let _g = shared();
        let rec = Recorder::start("sway");
        let mut shell = open(&rec);
        let w = mapped_window(&mut shell, 64, 48);
        let other = mapped_window(&mut shell, 32, 32);
        restart(&rec, r#"{"cmd":"restart","mode":"gone"}"#);
        until_reconnecting(&mut shell);
        let backing = backing::Backing::new(64, 48);
        let calls: [(&str, ShellResult); 8] = [
            ("set_title", shell.set_title(w, "x")),
            ("set_fullscreen", shell.set_fullscreen(w, true)),
            ("ack_configure", shell.ack_configure(w, 1, 0)),
            (
                "window_new",
                shell
                    .window_new(&WindowDesc {
                        w: 10,
                        h: 10,
                        title: "t".into(),
                        app_id: "nidhogg.test".into(),
                        flags: 0,
                    })
                    .map(drop),
            ),
            (
                "publish",
                shell.publish(w, &[backing.frame(w)], 0).map(drop),
            ),
            ("present_tree", shell.present_tree(w).map(drop)),
            (
                "selection_own",
                shell.selection_own(NHSHELL_SEL_CLIPBOARD, &[("text/plain", b"x")]),
            ),
            ("bell", shell.bell(w)),
        ];
        for (name, r) in calls {
            assert_eq!(r, Err(ShellError::Reconnecting), "{name}");
        }
        // local work still goes: a destroyed window is dropped, the host's own calls answer
        assert_eq!(shell.window_destroy(other), Ok(()));
        assert_eq!(shell.dispatch_main(), Ok(()));
        assert!(shell.wake_fd() >= 0);
        // the socket is gone: a step says not yet, at once
        assert_eq!(shell.reconnect(50), Err(ShellError::Timeout));
        assert_eq!(shell.connection_state(), ConnectionState::Reconnecting);
        restart(&rec, r#"{"cmd":"restart_up"}"#);
        reconnect_within(&mut shell, 3);
        assert!(shell.caps().compositor >= 4);
    }

    #[test]
    fn reconnect_through_a_kept_socket() {
        let _g = shared();
        let rec = Recorder::start("sway");
        let mut shell = open(&rec);
        mapped_window(&mut shell, 64, 48);
        restart(
            &rec,
            r#"{"cmd":"restart","mode":"keep-socket","down_ms":300}"#,
        );
        let lost_at = Instant::now();
        until_reconnecting(&mut shell);
        // the connect succeeds and waits in the backlog: the bounded steps say not yet until it is served
        let mut timeouts = 0;
        loop {
            match shell.reconnect(50) {
                Ok(()) => break,
                Err(ShellError::Timeout) => timeouts += 1,
                Err(e) => panic!("reconnect: {e:?}"),
            }
            assert!(lost_at.elapsed() < Duration::from_secs(3), "never served");
        }
        assert!(
            timeouts >= 2,
            "only {timeouts} steps waited for the backlog"
        );
        assert!(
            lost_at.elapsed() >= Duration::from_millis(250),
            "{:?}",
            lost_at.elapsed()
        );
        wait_event(&mut shell, NHSHELL_EV_RECONNECTED);
        // one connection for the shell, one for its reconnect
        let c = rec.control(r#"{"cmd":"connections"}"#);
        assert_eq!(number(&c, "accepted"), Some(2), "{c}");
        assert_eq!(number(&c, "live"), Some(1), "{c}");
        // the new connection works: the seat and the registry are back
        assert!(shell.caps().seat >= 1);
        assert_eq!(shell.connection_state(), ConnectionState::Up);
    }

    #[test]
    fn reconnect_after_rebind() {
        let _g = shared();
        let rec = Recorder::start("sway");
        let mut shell = open(&rec);
        mapped_window(&mut shell, 64, 48);
        restart(&rec, r#"{"cmd":"restart","mode":"rebind","down_ms":300}"#);
        until_reconnecting(&mut shell);
        // no socket for a while (ENOENT): steps return at once
        let t = Instant::now();
        assert_eq!(shell.reconnect(50), Err(ShellError::Timeout));
        assert!(t.elapsed() < Duration::from_millis(50), "{:?}", t.elapsed());
        reconnect_within(&mut shell, 3);
        let c = rec.control(r#"{"cmd":"connections"}"#);
        assert_eq!(number(&c, "live"), Some(1), "{c}");
    }

    #[test]
    fn reconnect_never_blocks_past_its_timeout() {
        let _g = shared();
        let rec = Recorder::start("sway");
        let mut shell = open(&rec);
        mapped_window(&mut shell, 64, 48);
        restart(
            &rec,
            r#"{"cmd":"restart","mode":"keep-socket","down_ms":1500}"#,
        );
        until_reconnecting(&mut shell);
        let mut steps = 0;
        loop {
            let t = Instant::now();
            let r = shell.reconnect(50);
            let took = t.elapsed();
            if r.is_ok() {
                break;
            }
            assert_eq!(r, Err(ShellError::Timeout));
            assert!(
                took < Duration::from_millis(100),
                "a 50 ms step took {took:?}"
            );
            steps += 1;
            assert!(steps < 200, "never reconnected");
        }
        assert!(steps >= 10, "only {steps} steps across 1.5 s down");
        wait_event(&mut shell, NHSHELL_EV_RECONNECTED);
    }

    #[test]
    fn held_button_released_and_repeat_stopped_at_loss() {
        let _g = shared();
        let rec = Recorder::start("sway");
        let mut shell = open(&rec);
        let w = mapped_window(&mut shell, 200, 100);
        rec.control_ok(r#"{"cmd":"keymap","rmlvo":{"rules":"evdev","model":"pc105","layout":"us","variant":"","options":""}}"#);
        rec.control_ok(&format!(r#"{{"cmd":"focus","surface":{w}}}"#));
        rec.control(r#"{"cmd":"pointer","pos":[10,10]}"#);
        rec.control(&format!(
            r#"{{"cmd":"button","button":{BTN_LEFT},"state":"press"}}"#
        ));
        rec.control(&format!(
            r#"{{"cmd":"key","keycode":{KEY_A},"state":"press"}}"#
        ));
        // the key repeats while held
        wait_for(&mut shell, |e| {
            // SAFETY: a KEY record's valid member is `key`.
            e.kind == NHSHELL_EV_KEY && unsafe { e.u.key.repeat == 1 }
        });
        restart(&rec, r#"{"cmd":"restart","mode":"gone"}"#);
        let evs = until_reconnecting(&mut shell);
        let release = evs
            .iter()
            .position(|e| {
                // SAFETY: a BUTTON record's valid member is `pointer`.
                e.kind == NHSHELL_EV_BUTTON
                    && unsafe {
                        e.u.pointer.button == BTN_LEFT
                            && e.u.pointer.state == NHSHELL_BUTTON_RELEASED
                    }
            })
            .expect("the held button is released at the loss");
        assert_eq!(evs[release].window, w);
        // no repeat after the loss
        let after = collect(&mut shell, 1000);
        assert!(
            after.iter().all(|e| e.kind != NHSHELL_EV_KEY),
            "a key record after RECONNECTING"
        );
    }

    #[test]
    fn pools_remade_from_the_same_memfds() {
        let _g = shared();
        let rec = Recorder::start("sway");
        let mut shell = open(&rec);
        // `mapped_window` draws 0x80 into every byte of its one buffer and commits it: the compositor holds it
        let w = mapped_window(&mut shell, 64, 48);
        restart(&rec, r#"{"cmd":"restart","mode":"keep-socket"}"#);
        until_reconnecting(&mut shell);
        reconnect_within(&mut shell, 3);
        let ev = wait_event(&mut shell, NHSHELL_EV_CONFIGURE);
        // SAFETY: a CONFIGURE record carries `configure`.
        let serial = unsafe { ev.u.configure }.serial;
        shell.ack_configure(w, serial, NHSHELL_ACK_CONTENT).unwrap();
        // every slot counts as released: the buffer drawn before the loss comes back with its pixels, nothing drawn
        let b = shell.present_begin(w, 64, 48).unwrap();
        assert_eq!(b.age, 1, "the buffer of the last frame, released");
        // SAFETY: the begun buffer is `stride * h` readable bytes until the commit.
        assert_eq!(unsafe { *b.pixels }, 0x80);
        shell
            .present_commit(w, &[glitnir::geom::Rect::new(0, 0, 64, 48)])
            .unwrap();
        shell.flush().unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            let snap = rec.snapshot();
            if Recorder::snapshot_pixel(&snap, 10, 10) == 0x8080_8080 {
                break;
            }
            assert!(Instant::now() < deadline, "the old bytes never showed");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    #[test]
    fn caps_recomputed_after_reconnect() {
        let _g = shared();
        let rec = Recorder::start("sway");
        let mut shell = open(&rec);
        let before: NhShellCaps = shell.caps();
        assert!(before.fractional_scale >= 1);
        restart(
            &rec,
            r#"{"cmd":"restart","mode":"keep-socket","profile":"no-fractional"}"#,
        );
        until_reconnecting(&mut shell);
        reconnect_within(&mut shell, 3);
        let after = shell.caps();
        assert_eq!(after.fractional_scale, 0, "the new compositor has none");
        assert_eq!(after.compositor, before.compositor);
        assert!(after.seat >= 1);
    }

    /// A top-level comes back with its title, app id, min size and the fullscreen state the compositor granted, all
    /// on the new surface's role setup; its id stays.  (Top-levels have no parent in the shell's API.)
    #[test]
    fn toplevels_remade_with_their_state() {
        let _g = shared();
        let rec = Recorder::start("sway");
        let mut shell = open(&rec);
        let w = mapped_window(&mut shell, 200, 100);
        shell.set_title(w, "kept title").unwrap();
        shell.set_app_id(w, "nidhogg.kept").unwrap();
        shell.set_min_size(w, 120, 80).unwrap();
        rec.control_ok(&format!(
            r#"{{"cmd":"configure","surface":{w},"size":[300,200],"states":["fullscreen"]}}"#
        ));
        let c = configure_of(&mut shell, w);
        assert_ne!(c.states & NHSHELL_STATE_FULLSCREEN, 0);
        shell
            .ack_configure(w, c.serial, NHSHELL_ACK_CONTENT)
            .unwrap();
        draw(&mut shell, w, 300, 200);
        restart(&rec, r#"{"cmd":"restart","mode":"keep-socket"}"#);
        until_reconnecting(&mut shell);
        let seq = next_seq(&rec);
        reconnect_within(&mut shell, 3);
        let pid = shell.surface_protocol_id(w).expect("the window is kept");
        // the new surface's configure names the same window id
        let c = configure_of(&mut shell, w);
        shell.ack_configure(w, c.serial, 0).unwrap();
        let (cw, ch) = if c.w > 0 { (c.w, c.h) } else { (300, 200) };
        draw(&mut shell, w, cw, ch);
        rec.wait_node(u64::from(pid), |n| {
            n.contains("\"role\":\"toplevel\"") && n.contains("\"min_size\":[120,80]")
        });
        let log = commits(&rec, seq, pid);
        assert!(!log.is_empty(), "no commit on the new surface");
        assert!(
            log.iter().all(|c| c.contains("\"title\":\"kept title\"")
                && c.contains("\"app_id\":\"nidhogg.kept\"")),
            "{log:?}"
        );
        // fullscreen was asked for again on the new connection (its input log starts at the restart)
        let input = rec.control(r#"{"cmd":"input_log"}"#);
        assert!(
            input
                .split('{')
                .any(|e| e.contains("\"req\":\"xdg_toplevel.set_fullscreen\"")
                    && e.contains(&format!("\"surface\":{pid},"))),
            "{input}"
        );
    }

    /// Children come back as sync subsurfaces of their parents' new surfaces, at their offsets and in their stacking
    /// order (a grandchild under its own parent).
    #[test]
    fn children_remade_as_sync_subsurfaces() {
        let _g = shared();
        let rec = Recorder::start("sway");
        let mut shell = open(&rec);
        let top = mapped_window(&mut shell, 200, 100);
        let child = |shell: &mut WaylandShell, parent: u64, x: i32, y: i32| {
            shell
                .child_new(&ChildDesc {
                    kind: NHSHELL_KIND_CHILD,
                    parent,
                    x,
                    y,
                    w: 40,
                    h: 30,
                    anchor: Anchor::default(),
                    flags: 0,
                })
                .unwrap()
        };
        let low = child(&mut shell, top, 0, 0);
        let c2 = child(&mut shell, top, 30, 5);
        let c1 = child(&mut shell, top, 10, 20);
        let c3 = child(&mut shell, c1, 2, 3);
        // c1 below c2 (bottom to top: low, c1, c2), then c1 back above c2, and low below the parent
        shell.stack(c1, c2, false).unwrap();
        shell.stack(c1, c2, true).unwrap();
        shell.stack(low, 0, false).unwrap();
        restart_and_reconnect(&rec, &mut shell);
        let c = configure_of(&mut shell, top);
        shell
            .ack_configure(top, c.serial, NHSHELL_ACK_CONTENT)
            .unwrap();
        // deepest first: a sync child's state applies with its own commit and then its parent's
        for id in [c3, c1, c2, low] {
            shell.present_bare(id).unwrap();
        }
        draw(&mut shell, top, 200, 100);
        let pid = |id: u64| shell.surface_protocol_id(id).expect("kept");
        let (ptop, plow, p1, p2, p3) = (pid(top), pid(low), pid(c1), pid(c2), pid(c3));
        rec.wait_node(u64::from(ptop), |n| {
            n.contains(&format!("\"children\":[{plow},{p2},{p1}]"))
        });
        for (p, parent, pos) in [
            (p1, ptop, [10, 20]),
            (p2, ptop, [30, 5]),
            (plow, ptop, [0, 0]),
            (p3, p1, [2, 3]),
        ] {
            rec.wait_node(u64::from(p), |n| {
                n.contains("\"role\":\"subsurface\"")
                    && n.contains("\"sync\":true")
                    && n.contains(&format!("\"parent\":{parent},"))
                    && n.contains(&format!("\"position\":[{},{}]", pos[0], pos[1]))
            });
        }
        // the ids are the host's as before
        assert_eq!(shell.window_ready(c3), Ok(0));
        assert_eq!(shell.last_error(), "");
    }

    /// Popups close at the reconnect, topmost first, before `RECONNECTED`; their records go, nothing is sent for
    /// them, and a new popup works.
    #[test]
    fn popups_done_topmost_first() {
        let _g = shared();
        let rec = Recorder::start("sway");
        let mut shell = open(&rec);
        let top = mapped_window(&mut shell, 200, 100);
        let popup = |shell: &mut WaylandShell, parent: u64| {
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
                    flags: 0,
                })
                .unwrap()
        };
        let a = popup(&mut shell, top);
        let b = popup(&mut shell, a);
        let c = popup(&mut shell, b);
        let deadline = Instant::now() + Duration::from_secs(3);
        while rec.surfaces("popup") < 3 {
            assert!(Instant::now() < deadline, "the popups never arrived");
            std::thread::sleep(Duration::from_millis(10));
        }
        let evs = restart_and_reconnect(&rec, &mut shell);
        let done: Vec<u64> = evs
            .iter()
            .filter(|e| e.kind == NHSHELL_EV_POPUP_DONE)
            .map(|e| e.window)
            .collect();
        assert_eq!(done, [c, b, a], "topmost first");
        assert_eq!(evs.last().unwrap().kind, NHSHELL_EV_RECONNECTED);
        // the records are gone: destroying one has nothing to do
        assert_eq!(shell.window_destroy(a), Err(ShellError::State));
        let cfg = configure_of(&mut shell, top);
        shell
            .ack_configure(top, cfg.serial, NHSHELL_ACK_CONTENT)
            .unwrap();
        draw(&mut shell, top, 200, 100);
        assert_eq!(rec.surfaces("popup"), 0);
        let again = popup(&mut shell, top);
        let deadline = Instant::now() + Duration::from_secs(3);
        while rec.surfaces("popup") < 1 {
            assert!(Instant::now() < deadline, "the new popup never arrived");
            std::thread::sleep(Duration::from_millis(10));
        }
        for ev in collect(&mut shell, 200) {
            assert_ne!(ev.kind, NHSHELL_EV_DISCONNECTED, "a protocol error");
        }
        assert_eq!(shell.last_error(), "");
        assert_eq!(
            shell.window_grabbed(again),
            Ok(false),
            "the new popup is known"
        );
    }

    /// A top-level published before the loss, with its tree presented; made again and reconnected.  The records up
    /// to `RECONNECTED`, the recorder's first commit of the new connection, and the backing.
    fn published_then_reconnected(
        rec: &Recorder,
        shell: &mut WaylandShell,
    ) -> (u64, u64, backing::Backing) {
        let mut b = backing::Backing::new(200, 100);
        let w = shell
            .window_new(&WindowDesc {
                w: 200,
                h: 100,
                title: "t".into(),
                app_id: "nidhogg.test".into(),
                flags: 0,
            })
            .unwrap();
        let c = configure_of(shell, w);
        shell
            .ack_configure(w, c.serial, NHSHELL_ACK_CONTENT)
            .unwrap();
        b.fill_all(0xff20_4060);
        backing::publish(shell, w, &[(&b, w)], 0);
        assert!(shell.present_tree(w).unwrap(), "the first picture");
        shell.flush().unwrap();
        restart(rec, r#"{"cmd":"restart","mode":"keep-socket"}"#);
        until_reconnecting(shell);
        let seq = next_seq(rec);
        reconnect_within(shell, 3);
        (w, seq, b)
    }

    /// Review Focus 5: a window made again is not presented before the new compositor's configure: the tree's
    /// present and a legacy begin are refused, and on the new connection no buffer is attached before the first
    /// `ack_configure` (the trace rule `unconfigured`).
    #[test]
    fn no_commit_before_the_new_configure() {
        let _g = shared();
        let rec = Recorder::start("sway");
        let mut shell = open(&rec);
        let (w, seq, mut b) = published_then_reconnected(&rec, &mut shell);
        // redisplay drew meanwhile: the packet is taken, and nothing is committed
        b.fill_all(0xff00_ff00);
        backing::publish(&mut shell, w, &[(&b, w)], 0);
        assert!(
            !shell.present_tree(w).unwrap(),
            "presented before the configure"
        );
        assert_eq!(
            shell.present_begin(w, 200, 100).err(),
            Some(ShellError::State)
        );
        assert_eq!(shell.window_ready(w), Ok(0));
        shell.flush().unwrap();
        let c = configure_of(&mut shell, w);
        shell
            .ack_configure(w, c.serial, NHSHELL_ACK_CONTENT)
            .unwrap();
        b.fill_all(0xff00_00ff);
        backing::publish(&mut shell, w, &[(&b, w)], 0);
        assert!(shell.present_tree(w).unwrap(), "the configure's picture");
        shell.flush().unwrap();
        let pid = shell.surface_protocol_id(w).unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        let log = loop {
            let log = commits(&rec, seq, pid);
            if log.iter().any(|c| c.contains("\"buffer\":{")) {
                break log;
            }
            assert!(
                Instant::now() < deadline,
                "no buffer on the new surface: {log:?}"
            );
            std::thread::sleep(Duration::from_millis(10));
        };
        let first_buffer = log.iter().position(|c| c.contains("\"buffer\":{")).unwrap();
        let first_ack = log
            .iter()
            .position(|c| !c.starts_with("{\"ack_serial\":null"))
            .expect("an ack on the new surface");
        assert!(
            first_ack <= first_buffer,
            "a buffer before the first ack: {log:?}"
        );
    }

    /// The first configure after a reconnect is a content ack even when the host acks it bare: its ack goes with
    /// the tree's next buffer, once a packet published after it is there.
    #[test]
    fn first_configure_is_a_content_ack() {
        let _g = shared();
        let rec = Recorder::start("sway");
        let mut shell = open(&rec);
        let (w, seq, mut b) = published_then_reconnected(&rec, &mut shell);
        let c = configure_of(&mut shell, w);
        // the host's flag says bare (the same size as before)
        shell.ack_configure(w, c.serial, 0).unwrap();
        assert!(
            !shell.present_tree(w).unwrap(),
            "the ack waits for a packet after it"
        );
        b.fill_all(0xff00_ff00);
        backing::publish(&mut shell, w, &[(&b, w)], 0);
        assert!(shell.present_tree(w).unwrap(), "the ack goes with a buffer");
        shell.flush().unwrap();
        let pid = shell.surface_protocol_id(w).unwrap();
        let acked = format!("{{\"ack_serial\":{},", c.serial);
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            let log = commits(&rec, seq, pid);
            if let Some(commit) = log.iter().find(|c| c.starts_with(&acked)) {
                assert!(commit.contains("\"buffer\":{"), "a bare commit: {commit}");
                break;
            }
            assert!(Instant::now() < deadline, "the ack never went: {log:?}");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// What we owned is ours across the restart, and is offered again at the first serial on the new connection
    /// (the keyboard enter): another client then pastes it.
    #[test]
    fn owned_clipboard_offered_again_at_the_first_serial() {
        let _g = shared();
        let rec = Recorder::start("sway");
        let mut shell = open(&rec);
        let w = mapped_window(&mut shell, 200, 100);
        focus(&rec, &mut shell, w);
        let text: &[u8] = b"kept across the restart";
        shell
            .selection_own(NHSHELL_SEL_CLIPBOARD, &[("text/plain;charset=utf-8", text)])
            .unwrap();
        restart_and_reconnect(&rec, &mut shell);
        assert!(shell.selection_owned(NHSHELL_SEL_CLIPBOARD).unwrap());
        // our own bytes still read without a round trip
        assert_eq!(
            shell
                .selection_read(NHSHELL_SEL_CLIPBOARD, "text/plain;charset=utf-8", 0)
                .as_deref(),
            Ok(text)
        );
        focus(&rec, &mut shell, w);
        let reply = rec
            .control_ok(r#"{"cmd":"read","kind":"clipboard","type":"text/plain;charset=utf-8"}"#);
        assert_eq!(data_of(&reply), Some("kept across the restart"), "{reply}");
    }

    /// The host's own calls between the reconnect and the first serial win: a selection given up is not offered
    /// again, and one owned after the serial replaces the old one.
    #[test]
    fn newer_selection_wins() {
        let _g = shared();
        let rec = Recorder::start("sway");
        let mut shell = open(&rec);
        let w = mapped_window(&mut shell, 200, 100);
        focus(&rec, &mut shell, w);
        let plain = "text/plain;charset=utf-8";
        shell
            .selection_own(NHSHELL_SEL_CLIPBOARD, &[(plain, b"old")])
            .unwrap();
        shell
            .selection_own(NHSHELL_SEL_PRIMARY, &[(plain, b"given up")])
            .unwrap();
        restart_and_reconnect(&rec, &mut shell);
        shell.selection_disown(NHSHELL_SEL_PRIMARY).unwrap();
        focus(&rec, &mut shell, w);
        shell
            .selection_own(NHSHELL_SEL_CLIPBOARD, &[(plain, b"new")])
            .unwrap();
        let reply = rec.control_ok(&format!(
            r#"{{"cmd":"read","kind":"clipboard","type":"{plain}"}}"#
        ));
        assert_eq!(data_of(&reply), Some("new"), "{reply}");
        assert!(!shell.selection_owned(NHSHELL_SEL_PRIMARY).unwrap());
        std::thread::sleep(Duration::from_millis(100));
        let reply = rec.control(&format!(
            r#"{{"cmd":"read","kind":"primary","type":"{plain}"}}"#
        ));
        assert_eq!(
            data_of(&reply),
            None,
            "the given-up primary came back: {reply}"
        );
    }

    /// With a session, the reconnect opens it again with reason `recover` and its id, and the window is restored
    /// under its name before its role-setup commit.
    #[test]
    fn session_recovered_by_name() {
        let _g = shared();
        let rec = Recorder::start("mutter");
        let mut shell = open(&rec);
        shell.session_open(None, NHSHELL_SESSION_LAUNCH).unwrap();
        let ev = wait_event(&mut shell, NHSHELL_EV_SESSION);
        // SAFETY: a SESSION record carries `session`.
        assert_eq!(unsafe { ev.u.session }.state, NHSHELL_SESSION_CREATED);
        let id = shell.session_id().unwrap();
        let w = shell
            .window_new_named(
                &WindowDesc {
                    w: 300,
                    h: 200,
                    title: "t".into(),
                    app_id: "nidhogg.test".into(),
                    flags: 0,
                },
                Some("initial"),
            )
            .unwrap();
        let c = configure_of(&mut shell, w);
        shell
            .ack_configure(w, c.serial, NHSHELL_ACK_CONTENT)
            .unwrap();
        draw(&mut shell, w, 300, 200);
        restart_and_reconnect(&rec, &mut shell);
        let ev = wait_event(&mut shell, NHSHELL_EV_SESSION);
        // SAFETY: as above.
        assert_eq!(unsafe { ev.u.session }.state, NHSHELL_SESSION_RESTORED);
        assert_eq!(shell.session_id().unwrap(), id);
        let log = rec.control(r#"{"cmd":"session_log"}"#);
        // the reply's keys are sorted: an entry's fields before its "op" and its "reason" after it
        let get = log.find("\"op\":\"get_session\"").expect("get_session");
        let rest = &log[get..];
        assert!(
            rest[..rest.find('}').unwrap()].contains("\"reason\":\"recover\""),
            "{log}"
        );
        assert!(log[..get].contains(&format!("\"id\":\"{id}\"")), "{log}");
        let op = log.rfind("\"op\":\"restore\"").expect("restore");
        let entry = &log[log[..op].rfind('{').unwrap()..op];
        assert!(entry.contains("\"before_first_commit\":true"), "{log}");
        assert!(entry.contains("\"name\":\"initial\""), "{log}");
        assert!(
            op > get,
            "restored after the session was opened again: {log}"
        );
    }

    /// At the loss our drag ends (`DRAG_DONE`, not ok) and the drop it was making on our own window leaves
    /// (`DND_LEAVE`), both before `RECONNECTING`; after the reconnect nothing of either is left.
    #[test]
    fn drag_done_and_leave_posted() {
        let _g = shared();
        let rec = Recorder::start("mutter");
        let mut shell = open(&rec);
        let w = mapped_window(&mut shell, 200, 150);
        focus(&rec, &mut shell, w);
        rec.control_ok(r#"{"cmd":"pointer","pos":[5,5]}"#);
        wait_event(&mut shell, NHSHELL_EV_POINTER_ENTER);
        rec.control_ok(r#"{"cmd":"button","button":272,"state":"press"}"#);
        wait_for(&mut shell, |e| {
            // SAFETY: a BUTTON record carries `pointer`.
            e.kind == NHSHELL_EV_BUTTON && unsafe { e.u.pointer }.state == NHSHELL_BUTTON_PRESSED
        });
        shell
            .drag_start(
                w,
                &[("text/plain;charset=utf-8", b"dragged")],
                NHSHELL_DND_COPY,
            )
            .unwrap();
        rec.control_ok(r#"{"cmd":"pointer","pos":[30,30]}"#);
        let enter = wait_event(&mut shell, NHSHELL_EV_DND_ENTER);
        assert_eq!(enter.window, w);
        restart(&rec, r#"{"cmd":"restart","mode":"keep-socket"}"#);
        let evs = until_reconnecting(&mut shell);
        let at = |kind: u32| evs.iter().position(|e| e.kind == kind);
        let leave = at(NHSHELL_EV_DND_LEAVE).expect("the drop leaves");
        let done = at(NHSHELL_EV_DRAG_DONE).expect("the drag ends");
        assert_eq!(evs[leave].window, w);
        // SAFETY: drag-and-drop records carry `dnd`.
        assert_eq!(unsafe { evs[done].u.dnd }.ok, 0);
        assert!(leave < done && done < evs.len() - 1);
        reconnect_within(&mut shell, 3);
        assert!(!shell.selection_owned(NHSHELL_SEL_DND).unwrap());
        assert!(shell.selection_types(NHSHELL_SEL_DND).unwrap().is_empty());
        assert_eq!(shell.dnd_finish(true), Err(ShellError::State));
        assert!(!shell.dragging());
    }

    fn fds() -> usize {
        std::fs::read_dir("/proc/self/fd").unwrap().count()
    }

    fn tasks() -> usize {
        std::fs::read_dir("/proc/self/task").unwrap().count()
    }

    /// VmRSS in KiB.
    fn rss_kib() -> u64 {
        let status = std::fs::read_to_string("/proc/self/status").unwrap();
        let line = status.lines().find(|l| l.starts_with("VmRSS:")).unwrap();
        line.split_whitespace().nth(1).unwrap().parse().unwrap()
    }

    /// Twenty restarts with the socket kept: descriptors and threads equal cycle 5's, the resident set grows less than
    /// 2 MiB, and each reconnect made one connection.
    #[test]
    fn twenty_reconnects_free_everything() {
        let _g = SERIAL.write().unwrap_or_else(|e| e.into_inner());
        let rec = Recorder::start("sway");
        let mut shell = open(&rec);
        // 470 KiB of drawn buffer, remade every cycle: a kept mapping or memfd would show
        mapped_window(&mut shell, 400, 300);
        let mut at5 = None;
        for cycle in 1..=20 {
            restart(&rec, r#"{"cmd":"restart","mode":"keep-socket"}"#);
            until_reconnecting(&mut shell);
            reconnect_within(&mut shell, 3);
            collect(&mut shell, 20);
            if cycle == 5 {
                at5 = Some((fds(), tasks(), rss_kib()));
            }
        }
        let (fd5, task5, rss5) = at5.unwrap();
        let (fd20, task20, rss20) = (fds(), tasks(), rss_kib());
        assert_eq!(fd20, fd5, "descriptors");
        assert_eq!(task20, task5, "threads");
        assert!(
            rss20 < rss5 + 2048,
            "VmRSS grew {} KiB over 15 cycles",
            rss20.saturating_sub(rss5)
        );
        let c = rec.control(r#"{"cmd":"connections"}"#);
        assert_eq!(number(&c, "accepted"), Some(21), "{c}");
        assert_eq!(number(&c, "live"), Some(1), "{c}");
    }
}

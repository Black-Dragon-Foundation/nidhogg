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
        wait_event(shell, NHSHELL_EV_RECONNECTED);
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

    /// Needs the windows made again on the new connection, which `remake_windows` does not do yet.
    #[test]
    #[ignore = "needs the windows made again after a reconnect"]
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

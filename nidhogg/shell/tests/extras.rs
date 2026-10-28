//! T1 tests of the bell, idle inhibit, xdg-foreign handles, activation tokens and sessions against nh-recorder.
//! `NH_RECORDER` names the recorder binary; each test starts its own with a temporary `XDG_RUNTIME_DIR`.

mod common;

use common::*;
use nhshell::api::*;
use nhshell::event::*;
use nhshell::wayland::WaylandShell;
use rustix::process::Signal;
use std::time::{Duration, Instant};

fn desc(flags: u32) -> WindowDesc {
    WindowDesc {
        w: 300,
        h: 200,
        title: "t".into(),
        app_id: "nidhogg.test".into(),
        flags,
    }
}

/// The control reply of `cmd` once `pred` holds for it, within 3 s.
fn until(rec: &Recorder, cmd: &str, pred: impl Fn(&str) -> bool) -> String {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        let r = rec.control(&format!(r#"{{"cmd":"{cmd}"}}"#));
        if pred(&r) {
            return r;
        }
        assert!(Instant::now() < deadline, "{cmd} never matched; last: {r}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// A window made with `name` in its session, its first configure acked with a buffer.
fn named_window(shell: &mut WaylandShell, flags: u32, name: &str) -> (u64, ConfigureEv) {
    let id = shell.window_new_named(&desc(flags), Some(name)).unwrap();
    let ev = wait_event(shell, NHSHELL_EV_CONFIGURE);
    // SAFETY: a CONFIGURE record carries `configure`.
    let c = unsafe { ev.u.configure };
    shell
        .ack_configure(id, c.serial, NHSHELL_ACK_CONTENT)
        .unwrap();
    let (w, h) = if c.w > 0 { (c.w, c.h) } else { (300, 200) };
    draw(shell, id, w, h);
    (id, c)
}

fn session_state(shell: &mut WaylandShell) -> u32 {
    let ev = wait_event(shell, NHSHELL_EV_SESSION);
    // SAFETY: a SESSION record carries `session`.
    unsafe { ev.u.session }.state
}

mod extras {
    use super::*;

    #[test]
    fn caps_name_the_exporter() {
        let rec = Recorder::start("mutter");
        let shell = rec.open();
        assert_eq!(shell.caps().xdg_foreign, 1);
        assert_eq!(shell.caps().system_bell, 1);
    }

    #[test]
    fn bell_rings_on_the_root_surface() {
        let rec = Recorder::start("mutter");
        let mut shell = rec.open();
        let id = mapped_window(&mut shell, 200, 100);
        shell.bell(id).unwrap();
        until(&rec, "bell_log", |r| {
            r.contains(&format!("\"surface\":{id}"))
        });
        std::thread::sleep(Duration::from_millis(30));
        shell.bell(0).unwrap();
        until(&rec, "bell_log", |r| r.contains("\"surface\":null"));
        assert_eq!(shell.bell(9999), Err(ShellError::State));
    }

    #[test]
    fn bell_rate_limited() {
        let rec = Recorder::start("mutter");
        let mut shell = rec.open();
        let id = mapped_window(&mut shell, 200, 100);
        shell.bell(id).unwrap();
        shell.bell(id).unwrap();
        std::thread::sleep(Duration::from_millis(30));
        shell.bell(id).unwrap();
        let log = until(&rec, "bell_log", |r| r.matches("\"surface\"").count() >= 2);
        std::thread::sleep(Duration::from_millis(50));
        let log2 = rec.control(r#"{"cmd":"bell_log"}"#);
        assert_eq!(log2.matches("\"surface\"").count(), 2, "{log} then {log2}");
    }

    #[test]
    fn bell_unsupported_without_the_protocol() {
        let rec = Recorder::start("sway");
        let mut shell = rec.open();
        let id = mapped_window(&mut shell, 200, 100);
        assert_eq!(shell.bell(id), Err(ShellError::Unsupported));
        assert_eq!(shell.caps().system_bell, 0);
    }

    #[test]
    fn idle_inhibitor_follows_the_flag() {
        let rec = Recorder::start("mutter");
        let mut shell = rec.open();
        let id = mapped_window(&mut shell, 200, 100);
        let alive = |r: &str| r.matches("\"alive\":true").count();
        shell.inhibit_idle(id, true).unwrap();
        shell.inhibit_idle(id, true).unwrap();
        until(&rec, "idle_inhibitors", |r| alive(r) == 1);
        shell.inhibit_idle(id, false).unwrap();
        until(&rec, "idle_inhibitors", |r| alive(r) == 0);
        shell.inhibit_idle(id, true).unwrap();
        until(&rec, "idle_inhibitors", |r| alive(r) == 1);
        shell.window_destroy(id).unwrap();
        until(&rec, "idle_inhibitors", |r| alive(r) == 0);
        assert_eq!(shell.inhibit_idle(id, true), Err(ShellError::State));
    }

    #[test]
    fn export_handle_cached_and_bounded() {
        let rec = Recorder::start("mutter");
        let mut shell = rec.open();
        let id = mapped_window(&mut shell, 200, 100);
        // a compositor that does not answer: the wait ends at the timeout
        rec.signal(Signal::STOP);
        let t = Instant::now();
        assert_eq!(shell.window_export(id, 50), Err(ShellError::Timeout));
        let waited = t.elapsed();
        assert!(waited >= Duration::from_millis(50), "{waited:?}");
        assert!(waited < Duration::from_millis(500), "{waited:?}");
        rec.signal(Signal::CONT);
        let handle = shell.window_export(id, 1000).unwrap();
        assert_eq!(handle, "rec-1");
        assert_eq!(shell.window_export(id, 1000).unwrap(), handle);
        let log = rec.control(r#"{"cmd":"exports"}"#);
        assert_eq!(log.matches("\"handle\"").count(), 1, "{log}");
        shell.window_destroy(id).unwrap();
        assert_eq!(shell.window_export(id, 1000), Err(ShellError::State));
        let id2 = mapped_window(&mut shell, 200, 100);
        assert_eq!(shell.window_export(id2, 1000).unwrap(), "rec-2");
    }

    #[test]
    fn export_unsupported_without_the_exporter() {
        let dir = std::env::temp_dir().join(format!("nhshell-extras-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let text = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../test/recorder/profiles/mutter.json"
        ))
        .unwrap()
        .replace("\"zxdg_exporter_v2\": 1,\n", "");
        std::fs::write(dir.join("no-exporter.json"), text).unwrap();
        let rec = Recorder::start(dir.join("no-exporter").to_str().unwrap());
        let mut shell = rec.open();
        let id = mapped_window(&mut shell, 200, 100);
        assert_eq!(shell.caps().xdg_foreign, 0);
        assert_eq!(shell.window_export(id, 100), Err(ShellError::Unsupported));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn activation_token_needs_a_serial() {
        let rec = Recorder::start("mutter");
        let mut shell = rec.open();
        let id = mapped_window(&mut shell, 200, 100);
        assert_eq!(shell.activation_token(id, 100), Err(ShellError::State));
        rec.control_ok(r#"{"cmd":"focus","surface":"toplevel"}"#);
        rec.control(r#"{"cmd":"pointer","pos":[10,10]}"#);
        rec.control(r#"{"cmd":"button","button":272,"state":"press"}"#);
        // the press is a record in the queue; the token must not consume it
        let deadline = Instant::now() + Duration::from_secs(3);
        while !readable_within(shell.wake_fd(), 20) {
            assert!(Instant::now() < deadline, "no input arrived");
        }
        std::thread::sleep(Duration::from_millis(50));
        let token = shell.activation_token(id, 1000).unwrap();
        assert!(!token.is_empty());
        let again = shell.activation_token(id, 1000).unwrap();
        assert_ne!(again, token, "every call asks for a fresh token");
        wait_event(&mut shell, NHSHELL_EV_BUTTON);
        // nothing answers: bounded
        rec.signal(Signal::STOP);
        let t = Instant::now();
        assert_eq!(shell.activation_token(id, 50), Err(ShellError::Timeout));
        assert!(t.elapsed() < Duration::from_millis(500));
        rec.signal(Signal::CONT);
    }

    #[test]
    fn session_restore_goes_before_the_first_commit() {
        let rec = Recorder::start("mutter");
        let mut a = rec.open();
        a.session_open(None, NHSHELL_SESSION_LAUNCH).unwrap();
        assert_eq!(session_state(&mut a), NHSHELL_SESSION_CREATED);
        let id = a.session_id().unwrap();
        assert!(!id.is_empty());
        named_window(&mut a, 0, "initial");
        let log = rec.control(r#"{"cmd":"session_log"}"#);
        assert!(log.contains("\"op\":\"add\""), "{log}");
        assert!(log.contains("\"before_first_commit\":true"), "{log}");

        // a second client takes the session over and restores the window under its name
        let mut b = rec.open();
        b.session_open(Some(&id), NHSHELL_SESSION_RESTORE).unwrap();
        assert_eq!(session_state(&mut b), NHSHELL_SESSION_RESTORED);
        assert_eq!(b.session_id().unwrap(), id);
        assert_eq!(session_state(&mut a), NHSHELL_SESSION_REPLACED);
        let (_, c) = named_window(&mut b, NHSHELL_WIN_RESTORE, "initial");
        assert_eq!((c.w, c.h), (300, 200), "the stored size comes back");
        let log = rec.control(r#"{"cmd":"session_log"}"#);
        assert!(log.contains("\"op\":\"restore\""), "{log}");
        // the reply's keys are sorted: the entry's fields come before its "op"
        let op = log.rfind("\"op\":\"restore\"").unwrap();
        let entry = &log[log[..op].rfind('{').unwrap()..op];
        assert!(entry.contains("\"before_first_commit\":true"), "{log}");

        // taking the window out of the session, and a name already held
        assert_eq!(
            b.window_new_named(&desc(0), Some("initial")),
            Err(ShellError::State)
        );
        let (w, _) = named_window(&mut b, 0, "second");
        b.window_session_remove(w).unwrap();
        assert_eq!(b.window_session_remove(w), Err(ShellError::State));
    }

    #[test]
    fn session_replaced_ignores_names() {
        let rec = Recorder::start("mutter");
        let mut a = rec.open();
        a.session_open(None, NHSHELL_SESSION_LAUNCH).unwrap();
        session_state(&mut a);
        let id = a.session_id().unwrap();
        let mut b = rec.open();
        b.session_open(Some(&id), NHSHELL_SESSION_RECOVER).unwrap();
        session_state(&mut b);
        assert_eq!(session_state(&mut a), NHSHELL_SESSION_REPLACED);
        let before = rec.control(r#"{"cmd":"session_log"}"#);
        let (w, _) = named_window(&mut a, NHSHELL_WIN_RESTORE, "ignored");
        let after = rec.control(r#"{"cmd":"session_log"}"#);
        assert_eq!(before, after, "no add or restore went out");
        assert_eq!(a.window_session_remove(w), Err(ShellError::State));
    }

    #[test]
    fn no_session_ignores_names() {
        let rec = Recorder::start("sway");
        let mut shell = rec.open();
        assert_eq!(
            shell.session_open(None, NHSHELL_SESSION_LAUNCH),
            Err(ShellError::Unsupported)
        );
        assert_eq!(shell.session_id(), Err(ShellError::State));
        let (w, _) = named_window(&mut shell, 0, "initial");
        assert_eq!(shell.window_session_remove(w), Err(ShellError::State));
    }
}

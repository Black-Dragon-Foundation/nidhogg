//! T1 tests of xdg toplevels against nh-recorder: configure sequencing, states, close and decoration negotiation.
//! The harness is `common/mod.rs`.

mod common;

use common::*;
use nhshell::api::{ShellBackend, WindowDesc};
use nhshell::event::*;
use nhshell::wayland::WaylandShell;
use std::time::{Duration, Instant};

/// The raw token after `"key":` (a string keeps its quotes).
fn token<'a>(json: &'a str, key: &str) -> Option<&'a str> {
    let pat = format!("\"{key}\":");
    let rest = json[json.find(&pat)? + pat.len()..].trim_start();
    let end = rest.find([',', '}']).unwrap_or(rest.len());
    Some(&rest[..end])
}

/// The commits of a `log` reply, one JSON object text each.
fn commits(rec: &Recorder, since: u64) -> Vec<String> {
    let log = rec.control(&format!(r#"{{"cmd":"log","since":{since}}}"#));
    // the reply's objects have sorted keys, so `ack_serial` opens each commit
    log.split("{\"ack_serial\":")
        .skip(1)
        .map(|c| format!("{{\"ack_serial\":{c}"))
        .collect()
}

fn configure_of(ev: &NhShellEvent) -> ConfigureEv {
    assert_eq!(ev.kind, NHSHELL_EV_CONFIGURE);
    // SAFETY: a CONFIGURE record's valid member is `configure`.
    unsafe { ev.u.configure }
}

fn new_window(shell: &mut WaylandShell) -> u64 {
    shell
        .window_new(&WindowDesc {
            w: 200,
            h: 100,
            title: "t".into(),
            app_id: "nidhogg.test".into(),
            flags: 0,
        })
        .unwrap()
}

/// Commits since `since` until one satisfies `pred`, within 3 s.
fn wait_commit(rec: &Recorder, since: u64, pred: impl Fn(&str) -> bool) -> Vec<String> {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        let all = commits(rec, since);
        if all.iter().any(|c| pred(c)) || Instant::now() > deadline {
            return all;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[allow(clippy::module_inception)]
mod toplevel {
    use super::*;

    #[test]
    fn first_configure_maps() {
        let rec = Recorder::start("mutter");
        let mut shell = rec.open();
        let id = new_window(&mut shell);
        let first = configure_of(&wait_event(&mut shell, NHSHELL_EV_CONFIGURE));
        // the initial bare commit carries no ack and no buffer: the window is not mapped yet
        let log = wait_commit(&rec, 1, |c| token(c, "role") == Some("\"toplevel\""));
        let initial = &log[0];
        assert_eq!(token(initial, "ack_serial"), Some("null"), "{initial}");
        assert_eq!(token(initial, "buffer"), Some("null"), "{initial}");
        // the host acks, then commits
        shell.ack_configure(id, first.serial, 0).unwrap();
        shell.present_bare(id).unwrap();
        shell.flush().unwrap();
        let log = wait_commit(&rec, 1, |c| token(c, "ack_serial") != Some("null"));
        let acked = log
            .iter()
            .find(|c| token(c, "ack_serial") != Some("null"))
            .expect("a commit with an ack");
        assert_eq!(number(acked, "ack_serial"), Some(u64::from(first.serial)));
        assert_eq!(token(acked, "ack_stale"), Some("false"), "{acked}");
        assert_eq!(token(acked, "role"), Some("\"toplevel\""));
    }

    /// Two configures reach the host in order, each with its own serial and size.  Which one the host acks is the
    /// glue's policy (the newest), pinned by the GUI rows.
    #[test]
    fn double_configure_in_order() {
        let rec = Recorder::start("mutter");
        let mut shell = rec.open();
        let id = new_window(&mut shell);
        let first = configure_of(&wait_event(&mut shell, NHSHELL_EV_CONFIGURE));
        shell.ack_configure(id, first.serial, 0).unwrap();
        shell.present_bare(id).unwrap();
        shell.flush().unwrap();
        collect(&mut shell, 50);
        // two configures before the client commits
        let r1 = rec.control(r#"{"cmd":"configure","surface":"focused","size":[300,200]}"#);
        let r2 = rec.control(r#"{"cmd":"configure","surface":"focused","size":[400,300]}"#);
        let (s1, s2) = (
            number(&r1, "serial").unwrap(),
            number(&r2, "serial").unwrap(),
        );
        assert!(s1 < s2);
        let got: Vec<ConfigureEv> = {
            let mut v = Vec::new();
            while v.len() < 2 {
                v.push(configure_of(&wait_event(&mut shell, NHSHELL_EV_CONFIGURE)));
            }
            v
        };
        assert_eq!(
            (got[0].serial, got[0].w, got[0].h),
            (s1 as u32, 300, 200),
            "first of the two"
        );
        assert_eq!((got[1].serial, got[1].w, got[1].h), (s2 as u32, 400, 300));
    }

    #[test]
    fn states_bits() {
        let rec = Recorder::start("mutter");
        let mut shell = rec.open();
        new_window(&mut shell);
        wait_event(&mut shell, NHSHELL_EV_CONFIGURE);
        let reply = rec.control(
            r#"{"cmd":"configure","surface":"focused","size":[0,0],"states":["maximized","fullscreen","tiled_left","suspended"]}"#,
        );
        assert!(reply.contains("\"ok\":true"), "{reply}");
        let ev = collect(&mut shell, 100)
            .into_iter()
            .rfind(|e| e.kind == NHSHELL_EV_CONFIGURE)
            .expect("a configure");
        assert_eq!(
            configure_of(&ev).states,
            NHSHELL_STATE_MAXIMIZED
                | NHSHELL_STATE_FULLSCREEN
                | NHSHELL_STATE_TILED_L
                | NHSHELL_STATE_SUSPENDED
        );
        let reply = rec.control(
            r#"{"cmd":"configure","surface":"focused","states":["activated","resizing","tiled_right","tiled_top","tiled_bottom"]}"#,
        );
        assert!(reply.contains("\"ok\":true"), "{reply}");
        let ev = wait_event(&mut shell, NHSHELL_EV_CONFIGURE);
        assert_eq!(
            configure_of(&ev).states,
            NHSHELL_STATE_ACTIVATED
                | NHSHELL_STATE_RESIZING
                | NHSHELL_STATE_TILED_R
                | NHSHELL_STATE_TILED_T
                | NHSHELL_STATE_TILED_B
        );
    }

    /// Commit after the first ack and return the logged decoration of the window's commits.
    fn decoration_logged(rec: &Recorder, shell: &mut WaylandShell, id: u64) -> Option<String> {
        let first = collect(shell, 100)
            .into_iter()
            .rfind(|e| e.kind == NHSHELL_EV_CONFIGURE)
            .expect("a configure");
        shell
            .ack_configure(id, configure_of(&first).serial, 0)
            .unwrap();
        shell.present_bare(id).unwrap();
        shell.flush().unwrap();
        let log = wait_commit(rec, 1, |c| token(c, "ack_serial") != Some("null"));
        let acked = log
            .iter()
            .find(|c| token(c, "ack_serial") != Some("null"))
            .expect("a commit with an ack");
        token(acked, "decoration")
            .filter(|t| *t != "null")
            .map(|t| t.trim_matches('"').to_owned())
    }

    #[test]
    fn ssd_on_sway_profile() {
        let rec = Recorder::start("sway");
        let mut shell = rec.open();
        let id = new_window(&mut shell);
        let logged = decoration_logged(&rec, &mut shell, id);
        assert!(shell.caps().ssd);
        assert!(shell.window_ssd(id).unwrap());
        assert_eq!(logged.as_deref(), Some("server"));
        // sway serves xdg_wm_base 5: the window's capabilities were sent (all four, by the recorder)
        assert_ne!(shell.window_wm_capabilities(id).unwrap(), 0);
    }

    #[test]
    fn csd_on_mutter_profile() {
        let rec = Recorder::start("mutter");
        let mut shell = rec.open();
        let id = new_window(&mut shell);
        let logged = decoration_logged(&rec, &mut shell, id);
        assert!(!shell.caps().ssd);
        assert!(!shell.window_ssd(id).unwrap());
        assert_eq!(logged.as_deref(), None);
    }

    #[test]
    fn force_csd_env() {
        let rec = Recorder::start("sway");
        let mut shell = {
            let _env = ENV.lock().unwrap_or_else(|e| e.into_inner());
            // SAFETY: every open in this binary holds ENV, and only open reads the variable.
            unsafe { std::env::set_var("NIDHOGG_SHELL_FORCE_CSD", "1") };
            let shell = rec.open_locked();
            // SAFETY: as above.
            unsafe { std::env::remove_var("NIDHOGG_SHELL_FORCE_CSD") };
            shell
        };
        assert!(shell.caps().force_csd);
        let id = new_window(&mut shell);
        let logged = decoration_logged(&rec, &mut shell, id);
        assert!(!shell.caps().ssd, "no get_toplevel_decoration was sent");
        assert_eq!(logged, None);
    }

    #[test]
    fn close_event() {
        let rec = Recorder::start("mutter");
        let mut shell = rec.open();
        let id = new_window(&mut shell);
        wait_event(&mut shell, NHSHELL_EV_CONFIGURE);
        let reply = rec.control(r#"{"cmd":"close","surface":"focused"}"#);
        assert!(reply.contains("\"ok\":true"), "{reply}");
        let ev = wait_event(&mut shell, NHSHELL_EV_CLOSE);
        assert_eq!(ev.window, id);
    }
}

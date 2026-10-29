//! T1 tests of the connection, the capabilities and the reader thread, against nh-recorder.
//! The harness is `common/mod.rs`.

mod common;

use common::*;
use glitnir::geom::Rect;
use nhshell::api::*;
use nhshell::event::{Kind, Record, ShellEvent};
use nhshell::wayland::WaylandShell;
use nhshell::wayland::registry::supported;
use rustix::process::Signal;
use std::time::{Duration, Instant};

/// Take every queued record.
fn drain(shell: &mut WaylandShell) -> Vec<Record> {
    let mut out = Vec::new();
    while let Some(ev) = shell.next_event().unwrap() {
        out.push(ev);
    }
    out
}

/// The plan names these `reader::…` (and `check.sh --test reader` selects them).
#[allow(clippy::module_inception)]
mod reader {
    use super::*;

    #[test]
    fn caps_follow_profile() {
        for (profile, wm_base, decoration) in [("mutter", 6, 0), ("sway", 5, 1)] {
            let rec = Recorder::start(profile);
            let shell = rec.open();
            let caps = shell.caps();
            // mutter 49 serves xdg_wm_base 7; the recorder (Smithay 0.7) serves at most 6
            assert_eq!(caps.xdg_wm_base, wm_base, "{profile}: xdg_wm_base");
            assert_eq!(caps.xdg_decoration, decoration, "{profile}: xdg_decoration");
            // every capability is min(what the recorder serves, what the shell supports); absent ones are 0
            let globals = rec.control(r#"{"cmd":"globals"}"#);
            let mut c = caps;
            for (name, _) in nhshell::caps::GLOBAL_FIELDS {
                let want =
                    number(&globals, name).map_or(0, |v| (v as u32).min(supported(name).unwrap()));
                assert_eq!(*c.field_mut(name).unwrap(), want, "{profile}: {name}");
            }
            assert!(!caps.ssd && !caps.force_csd);
        }
    }

    #[test]
    fn ping_answered_while_main_blocks() {
        let rec = Recorder::start("sway");
        let shell = rec.open();
        // the recorder pings from another thread while this one sleeps 3 s without calling the shell
        let reply = std::thread::scope(|s| {
            let pinger = s.spawn(|| {
                std::thread::sleep(Duration::from_millis(500));
                rec.control(r#"{"cmd":"ping"}"#)
            });
            std::thread::sleep(Duration::from_secs(3));
            pinger.join().unwrap()
        });
        let ms = number(&reply, "pong_ms").unwrap_or_else(|| panic!("no pong: {reply}"));
        assert!(ms < 100, "pong after {ms} ms");
        drop(shell);
    }

    #[test]
    fn wake_fd_readable_on_event() {
        let rec = Recorder::start("sway");
        let mut shell = rec.open();
        let surface = shell.surface_new().unwrap();
        shell.flush().unwrap();
        // let the surface's own events (main queue) come and go
        loop {
            drain(&mut shell);
            if !readable_within(shell.wake_fd(), 50) {
                break;
            }
        }
        let reply = rec.control_ok(&format!(r#"{{"cmd":"focus","surface":{surface}}}"#));
        assert!(reply.contains("\"ok\":true"), "{reply}");
        assert!(
            readable_within(shell.wake_fd(), 100),
            "no wake within 100 ms"
        );
        let focus: Vec<Record> = drain(&mut shell)
            .into_iter()
            .filter(|e| e.kind() == Kind::Focus)
            .collect();
        assert_eq!(focus.len(), 1);
        assert_eq!(focus[0].window, surface);
        assert!(matches!(focus[0].event, ShellEvent::Focus(f) if f.r#in == 1));
        // drained to zero: not readable until the next record
        assert!(!readable_within(shell.wake_fd(), 0));
    }

    #[test]
    fn flush_eagain_retried() {
        const BATCH: usize = 200; // 8-byte commits: 1.6 KiB, inside libwayland's 4 KiB buffer
        let rec = Recorder::start("sway");
        let mut shell = rec.open();
        let surface = shell.surface_new().unwrap();
        shell.flush().unwrap();
        let before = number(&rec.control(r#"{"cmd":"log"}"#), "next").unwrap();
        shell.set_send_buffer(4096).unwrap();
        // the recorder stops reading: the socket fills, and a flush meets EAGAIN
        rec.signal(Signal::STOP);
        let mut sent = 0;
        while !shell.flush_pending() {
            assert!(sent < 200 * BATCH, "no EAGAIN after {sent} commits");
            for _ in 0..BATCH {
                shell.surface_commit(surface).unwrap();
            }
            sent += BATCH;
            shell.flush().unwrap();
        }
        std::thread::sleep(Duration::from_millis(100));
        assert!(
            shell.flush_pending(),
            "the retry cannot succeed while the socket is full"
        );
        // the recorder reads again; the reader's POLLOUT retry sends the rest without the host flushing
        rec.signal(Signal::CONT);
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut got = 0;
        while Instant::now() < deadline {
            let log = rec.control(&format!(r#"{{"cmd":"log","since":{before}}}"#));
            got = log.matches("\"seq\":").count();
            if got >= sent {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(got, sent, "commits that reached the recorder");
        assert!(!shell.flush_pending());
    }

    #[test]
    fn disconnect_is_an_event() {
        let rec = Recorder::start("sway");
        let mut shell = rec.open();
        let wake = shell.wake_fd();
        assert!(wake >= 0);
        assert!(rec.control(r#"{"cmd":"quit"}"#).contains("\"ok\":true"));
        assert!(
            readable_within(wake, 2000),
            "no wake after the compositor quit"
        );
        let mut kinds = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(2);
        let last = loop {
            match shell.next_event() {
                Ok(Some(ev)) => kinds.push(ev.kind()),
                Ok(None) => {
                    assert!(Instant::now() < deadline, "no DISCONNECTED: {kinds:?}");
                    readable_within(wake, 100);
                }
                Err(e) => break e,
            }
        };
        assert_eq!(last, ShellError::Disconnected);
        assert_eq!(kinds.last(), Some(&Kind::Disconnected), "{kinds:?}");
        assert_eq!(
            kinds.iter().filter(|&&k| k == Kind::Disconnected).count(),
            1
        );
        // later calls keep reporting the loss, without a panic or a hang
        assert_eq!(shell.next_event().err(), Some(ShellError::Disconnected));
        assert_eq!(shell.dispatch_main(), Err(ShellError::Disconnected));
        drop(shell);
    }

    /// Take records until `DISCONNECTED` has been taken (the next call reports the loss), within 3 s.
    fn wait_lost(shell: &mut WaylandShell) {
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            match shell.next_event() {
                Ok(Some(_)) => {}
                Ok(None) => {
                    assert!(Instant::now() < deadline, "no DISCONNECTED");
                    readable_within(shell.wake_fd(), 50);
                }
                Err(e) => {
                    assert_eq!(e, ShellError::Disconnected);
                    return;
                }
            }
        }
    }

    fn desc() -> WindowDesc {
        WindowDesc {
            w: 64,
            h: 48,
            title: "t".into(),
            app_id: "nidhogg.test".into(),
            flags: 0,
        }
    }

    fn child_desc(kind: u32, parent: u64) -> ChildDesc {
        ChildDesc {
            kind,
            parent,
            x: 0,
            y: 0,
            w: 8,
            h: 8,
            anchor: Anchor {
                w: 1,
                h: 1,
                ..Anchor::default()
            },
            flags: 0,
        }
    }

    fn publish_one(s: &mut WaylandShell, w: u64) -> ShellResult {
        let px = [0u8; 4];
        let frame = SurfaceFrame {
            window: w,
            glitnir_root: 0,
            pixels: &px,
            w: 1,
            h: 1,
            stride: 4,
            src_x: 0,
            src_y: 0,
            flags: NHSHELL_SF_DRAWABLE,
            alpha_cut: 0,
        };
        s.publish(w, &[frame], 0).map(drop)
    }

    type Call = fn(&mut WaylandShell, u64) -> ShellResult;

    /// Every `ShellBackend` call, with plausible arguments on window `w`.  `window_destroy` is last.
    const CALLS: &[(&str, Call)] = &[
        ("next_event", |s, _| s.next_event().map(drop)),
        ("dispatch_main", |s, _| s.dispatch_main()),
        ("window_new", |s, _| s.window_new(&desc()).map(drop)),
        ("window_new_named", |s, _| {
            s.window_new_named(&desc(), Some("n")).map(drop)
        }),
        ("set_title", |s, w| s.set_title(w, "x")),
        ("set_app_id", |s, w| s.set_app_id(w, "x")),
        ("set_min_size", |s, w| s.set_min_size(w, 10, 10)),
        ("set_fullscreen", |s, w| s.set_fullscreen(w, true)),
        ("set_maximized", |s, w| s.set_maximized(w, true)),
        ("minimize", |s, w| s.minimize(w)),
        ("move_window", |s, w| s.move_window(w)),
        ("resize", |s, w| s.resize(w, NHSHELL_EDGE_RIGHT)),
        ("show_menu", |s, w| s.show_menu(w, 1, 1)),
        ("set_geometry", |s, w| s.set_geometry(w, 0, 0, 10, 10)),
        ("ack_configure", |s, w| s.ack_configure(w, 1, 0)),
        ("window_dirty", |s, w| s.window_dirty(w)),
        ("window_ready", |s, w| s.window_ready(w).map(drop)),
        ("present_bare", |s, w| s.present_bare(w)),
        ("set_cursor", |s, w| s.set_cursor(w, NHSHELL_CURSOR_TEXT)),
        ("set_cursor_theme", |s, _| s.set_cursor_theme("default", 24)),
        ("window_activate", |s, w| {
            s.window_activate(w, Some("token"))
        }),
        ("activation_token", |s, w| {
            s.activation_token(w, 100).map(drop)
        }),
        ("bell", |s, w| s.bell(w)),
        ("inhibit_idle", |s, w| s.inhibit_idle(w, true)),
        ("window_export", |s, w| s.window_export(w, 100).map(drop)),
        ("session_open", |s, _| {
            s.session_open(None, NHSHELL_SESSION_LAUNCH)
        }),
        ("session_id", |s, _| s.session_id().map(drop)),
        ("window_session_remove", |s, w| s.window_session_remove(w)),
        ("window_scale", |s, w| s.window_scale(w).map(drop)),
        ("window_ssd", |s, w| s.window_ssd(w).map(drop)),
        ("set_decorated", |s, w| s.set_decorated(w, true)),
        ("set_alpha", |s, w| s.set_alpha(w, u32::MAX / 2)),
        ("child_new", |s, w| {
            s.child_new(&child_desc(NHSHELL_KIND_CHILD, w)).map(drop)
        }),
        ("child_new popup", |s, w| {
            s.child_new(&child_desc(NHSHELL_KIND_POPUP, w)).map(drop)
        }),
        ("set_position", |s, w| s.set_position(w, 1, 1).map(drop)),
        ("stack", |s, w| s.stack(w, 0, true)),
        ("set_input_region", |s, w| {
            s.set_input_region(w, &[Rect::new(0, 0, 1, 1)])
        }),
        ("set_mapped", |s, w| s.set_mapped(w, false)),
        ("window_grabbed", |s, w| s.window_grabbed(w).map(drop)),
        ("set_size", |s, w| s.set_size(w, 10, 10).map(drop)),
        ("present_begin", |s, w| s.present_begin(w, 64, 48).map(drop)),
        ("present_commit", |s, w| {
            s.present_commit(w, &[Rect::new(0, 0, 1, 1)])
        }),
        ("selection_own", |s, _| {
            s.selection_own(NHSHELL_SEL_CLIPBOARD, &[("text/plain", b"x")])
        }),
        ("selection_disown", |s, _| {
            s.selection_disown(NHSHELL_SEL_CLIPBOARD)
        }),
        ("selection_owned", |s, _| {
            s.selection_owned(NHSHELL_SEL_CLIPBOARD).map(drop)
        }),
        ("selection_types", |s, _| {
            s.selection_types(NHSHELL_SEL_CLIPBOARD).map(drop)
        }),
        ("selection_read", |s, _| {
            s.selection_read(NHSHELL_SEL_CLIPBOARD, "text/plain", 100)
                .map(drop)
        }),
        ("text_input_enable", |s, w| s.text_input_enable(w, true)),
        ("text_input_cursor", |s, w| {
            s.text_input_cursor(w, 0, 0, 1, 1)
        }),
        ("text_input_surrounding", |s, w| {
            s.text_input_surrounding(w, "abc", 1, 1)
        }),
        ("text_len", |s, _| s.text_len(1).map(drop)),
        ("take_text", |s, _| s.take_text(1).map(drop)),
        ("publish", publish_one),
        ("present_tree", |s, w| s.present_tree(w).map(drop)),
        ("set_presenter_thread", |s, _| s.set_presenter_thread(false)),
        ("presenter_stats", |s, _| s.presenter_stats().map(drop)),
        ("set_reconnect", |s, _| s.set_reconnect(true)),
        ("reconnect", |s, _| s.reconnect(50)),
        ("window_destroy", |s, w| s.window_destroy(w)),
    ];

    /// Calls that cannot fail: the wake descriptor (the host may still be waiting on it), the capabilities, and the
    /// error text itself.  `abandon` is the glue's teardown handle.
    const NO_ERROR: &[&str] = &[
        "wake_fd",
        "caps",
        "last_error",
        "abandon",
        "connection_state",
    ];

    /// The names of `ShellBackend`'s methods, read from its source.
    fn trait_methods() -> Vec<String> {
        let api = include_str!("../src/api.rs");
        let start = api.find("pub trait ShellBackend").unwrap();
        let body = &api[start..start + api[start..].find("\n}\n").unwrap()];
        body.lines()
            .filter_map(|l| l.strip_prefix("    fn "))
            .map(|l| l[..l.find(['(', '<']).unwrap()].to_string())
            .collect()
    }

    /// After `DISCONNECTED`, every call reports the loss, without a panic or a hang; a plain hangup has no error text.
    #[test]
    fn every_call_after_disconnect_is_an_error() {
        let missing: Vec<String> = trait_methods()
            .into_iter()
            .filter(|m| !NO_ERROR.contains(&m.as_str()) && !CALLS.iter().any(|(n, _)| n == m))
            .collect();
        assert!(
            missing.is_empty(),
            "calls missing from the table: {missing:?}"
        );

        let rec = Recorder::start("mutter");
        let mut shell = rec.open();
        let w = mapped_window(&mut shell, 64, 48);
        let child = shell.child_new(&child_desc(NHSHELL_KIND_CHILD, w)).unwrap();
        assert!(rec.control(r#"{"cmd":"quit"}"#).contains("\"ok\":true"));
        wait_lost(&mut shell);
        assert_eq!(shell.last_error(), "", "a hangup is not a protocol error");
        assert!(shell.wake_fd() >= 0);
        for target in [w, child] {
            for (name, call) in CALLS {
                let started = Instant::now();
                let r = call(&mut shell, target);
                assert_eq!(
                    r,
                    Err(ShellError::Disconnected),
                    "{name} on window {target}"
                );
                assert!(
                    started.elapsed() < Duration::from_millis(50),
                    "{name} took {:?}",
                    started.elapsed()
                );
            }
        }
    }

    /// A selection read waiting on a source that never writes returns `Disconnected` within 100 ms of the connection
    /// dropping (the source lives on: another client).
    #[test]
    fn read_waiting_at_disconnect_returns() {
        let rec = Recorder::start("sway");
        let mut shell = rec.open();
        let surface = shell.surface_new().unwrap();
        shell.flush().unwrap();
        let reply = rec.control_ok(&format!(r#"{{"cmd":"focus","surface":{surface}}}"#));
        assert!(reply.contains("\"ok\":true"), "{reply}");
        wait_event(&mut shell, Kind::Focus);
        let reply = rec.control(
            r#"{"cmd":"offer","kind":"clipboard","types":{"text/plain":"never sent"},"silent":true}"#,
        );
        assert!(reply.contains("\"ok\":true"), "{reply}");
        let deadline = Instant::now() + Duration::from_secs(3);
        while shell.selection_types(NHSHELL_SEL_CLIPBOARD).unwrap() != ["text/plain"] {
            assert!(Instant::now() < deadline, "the offer never came");
            collect(&mut shell, 10);
        }
        let fd = shell.connection_fd();
        let (dropped_at, r, returned_at) = std::thread::scope(|s| {
            let dropper = s.spawn(move || {
                std::thread::sleep(Duration::from_millis(300));
                // SAFETY: the shell's connection stays open until the shell is dropped, after this thread ends.
                let fd = unsafe { rustix::fd::BorrowedFd::borrow_raw(fd) };
                rustix::net::shutdown(fd, rustix::net::Shutdown::Both).unwrap();
                Instant::now()
            });
            let r = shell.selection_read(NHSHELL_SEL_CLIPBOARD, "text/plain", 5000);
            let returned_at = Instant::now();
            (dropper.join().unwrap(), r, returned_at)
        });
        assert_eq!(r, Err(ShellError::Disconnected));
        let after = returned_at.saturating_duration_since(dropped_at);
        assert!(
            after < Duration::from_millis(100),
            "returned {after:?} after the drop"
        );
        wait_lost(&mut shell);
    }

    /// A protocol error ends the connection; its text stays readable after `DISCONNECTED`.
    #[test]
    fn protocol_error_text_kept() {
        let rec = Recorder::start("sway");
        let mut shell = rec.open();
        let w = mapped_window(&mut shell, 64, 48);
        assert_eq!(shell.last_error(), "");
        // a configure serial the compositor never sent: xdg_wm_base's invalid_surface_state (4)
        shell.ack_configure(w, 999_999, 0).unwrap();
        let _ = shell.present_bare(w);
        let _ = shell.flush();
        wait_lost(&mut shell);
        let text = shell.last_error();
        assert!(
            text.starts_with("xdg_wm_base@")
                && text.contains(": error 4: wrong configure serial: 999999"),
            "{text:?}"
        );
        // the call stays valid after the loss
        assert_eq!(shell.last_error(), text);
    }
}

//! T1 tests of drag and drop against nh-recorder: drops onto our window from a foreign source (`dnd_offer`,
//! `dnd_motion`, `dnd_leave`, `dnd_drop`, checked through `dnd_log`), and drags out of it, with the recorder as the
//! foreign target (`drag_target`, `drag_drop`, `drag_cancel`, `drag_log`).  The harness is `common/mod.rs`.

mod common;

use common::*;
use nhshell::api::*;
use nhshell::event::*;
use nhshell::wayland::WaylandShell;
use std::time::{Duration, Instant};

const TEXT: &str = "text/plain;charset=utf-8";

/// A recorder, a shell, and a mapped 200x150 window with keyboard focus and the pointer over it.
fn setup() -> (Recorder, WaylandShell, u64) {
    let rec = Recorder::start("mutter");
    let mut shell = rec.open();
    let id = mapped_window(&mut shell, 200, 150);
    let r = rec.control_ok(&format!(r#"{{"cmd":"focus","surface":{id}}}"#));
    assert!(r.contains("\"ok\":true"), "{r}");
    rec.control_ok(r#"{"cmd":"pointer","pos":[5,5]}"#);
    wait_event(&mut shell, Kind::PointerEnter);
    (rec, shell, id)
}

fn dnd(ev: &Record) -> DndEv {
    match ev.event {
        ShellEvent::DndEnter(d)
        | ShellEvent::DndMotion(d)
        | ShellEvent::DndLeave(d)
        | ShellEvent::DndDrop(d)
        | ShellEvent::DragAction(d)
        | ShellEvent::DragDropped(d)
        | ShellEvent::DragDone(d) => d,
        e => panic!("not a drag-and-drop record: {e:?}"),
    }
}

/// A foreign source enters the window at (10, 12) with copy and move, offering `types` (MIME, text).
fn offer(rec: &Recorder, shell: &mut WaylandShell, types: &[(&str, &str)], silent: bool) -> DndEv {
    let types: Vec<String> = types
        .iter()
        .map(|(m, t)| format!(r#""{m}":"{t}""#))
        .collect();
    let r = rec.control(&format!(
        r#"{{"cmd":"dnd_offer","types":{{{}}},"actions":["copy","move"],"preferred":"copy","silent":{silent},"surface":"toplevel","at":[10,12]}}"#,
        types.join(",")
    ));
    assert!(r.contains("\"ok\":true"), "{r}");
    dnd(&wait_event(shell, Kind::DndEnter))
}

fn plain_offer(rec: &Recorder, shell: &mut WaylandShell) -> DndEv {
    offer(rec, shell, &[("text/plain", "dropped text")], false)
}

/// The recorder's reply to `req` once it contains every one of `want`, within 3 s (the shell's requests travel on
/// another socket than the control commands).
fn wait_reply(rec: &Recorder, req: &str, want: &[&str]) -> String {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        let r = rec.control(req);
        if want.iter().all(|w| r.contains(w)) {
            return r;
        }
        assert!(Instant::now() < deadline, "{req}: want {want:?}, last {r}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn wait_log(rec: &Recorder, want: &[&str]) -> String {
    wait_reply(rec, r#"{"cmd":"dnd_log"}"#, want)
}

/// No record of `kind` within `ms`; the connection is still up.
fn none_of(shell: &mut WaylandShell, kind: Kind, ms: i64) {
    for ev in collect(shell, ms) {
        assert_ne!(ev.kind(), kind, "an unexpected record {kind:?}");
        assert_ne!(ev.kind(), Kind::Disconnected, "the connection ended");
    }
}

/// The recorder presses the left button over the window; the shell has read the press.
fn press(rec: &Recorder, shell: &mut WaylandShell) {
    rec.control_ok(r#"{"cmd":"button","button":272,"state":"press"}"#);
    wait_for(
        shell,
        |e| matches!(e.event, ShellEvent::Button(p) if p.state == NHSHELL_BUTTON_PRESSED),
    );
}

fn release(rec: &Recorder, shell: &mut WaylandShell) {
    rec.control_ok(r#"{"cmd":"button","button":272,"state":"release"}"#);
    wait_for(
        shell,
        |e| matches!(e.event, ShellEvent::Button(p) if p.state == NHSHELL_BUTTON_RELEASED),
    );
}

/// Press the button and drag `items` out of window `id`; the recorder has seen the drag start.
fn start_drag(rec: &Recorder, shell: &mut WaylandShell, id: u64, items: &[(&str, &[u8])]) {
    press(rec, shell);
    shell
        .drag_start(id, items, NHSHELL_DND_COPY | NHSHELL_DND_MOVE)
        .expect("drag_start with the button held");
    wait_reply(rec, r#"{"cmd":"drag_log"}"#, &["\"result\":\"started\""]);
}

/// The records up to and including the first `DRAG_DONE`, within 5 s.
fn until_done(shell: &mut WaylandShell) -> Vec<Record> {
    let mut out = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        while let Some(ev) = shell.next_event().unwrap() {
            out.push(ev);
            if ev.kind() == Kind::DragDone {
                return out;
            }
        }
        assert!(Instant::now() < deadline, "no DRAG_DONE");
        readable_within(shell.wake_fd(), 50);
    }
}

#[allow(clippy::module_inception)]
mod dnd {
    use super::*;

    // --- drops onto our window ----------------------------------------------------------------------------

    #[test]
    fn enter_motion_drop_reported_in_device_pixels() {
        let (rec, mut shell, id) = setup();
        rec.control(r#"{"cmd":"scale","value120":240}"#);
        wait_for(
            &mut shell,
            |e| matches!(e.event, ShellEvent::Scale(s) if s.scale120 == 240),
        );
        let enter = plain_offer(&rec, &mut shell);
        assert_eq!((enter.x, enter.y), (20.0, 24.0));
        assert_eq!(enter.source_actions, NHSHELL_DND_COPY | NHSHELL_DND_MOVE);
        assert_eq!((enter.action, enter.preferred), (0, NHSHELL_DND_COPY));
        assert_eq!(enter.own, 0);
        assert!(shell.dragging(), "a drag over us: popups take no grab");

        rec.control_ok(r#"{"cmd":"dnd_motion","at":[20,21]}"#);
        let ev = wait_event(&mut shell, Kind::DndMotion);
        assert_eq!(ev.window, id);
        let m = dnd(&ev);
        assert_eq!((m.x, m.y), (40.0, 42.0));
        assert_eq!(ev.time_ns, u64::from(m.time_ms) * 1_000_000);

        shell
            .dnd_status(
                Some("text/plain"),
                NHSHELL_DND_COPY | NHSHELL_DND_MOVE,
                NHSHELL_DND_COPY,
            )
            .unwrap();
        wait_log(&rec, &["\"chosen\":\"copy\""]);
        rec.control_ok(r#"{"cmd":"dnd_drop"}"#);
        let ev = wait_event(&mut shell, Kind::DndDrop);
        assert_eq!(ev.window, id);
        let d = dnd(&ev);
        assert_eq!((d.x, d.y), (40.0, 42.0));
        assert_eq!(d.action, NHSHELL_DND_COPY);
        assert!(!shell.dragging());
        // the leave that follows a drop keeps the offer for the host's finish
        none_of(&mut shell, Kind::DndLeave, 200);
        let data = shell.selection_read(NHSHELL_SEL_DND, "text/plain", 1000);
        assert_eq!(data.as_deref(), Ok(&b"dropped text"[..]));
        shell.dnd_finish(true).unwrap();
        wait_log(&rec, &["\"finished\":true", "\"destroyed\":true"]);
        assert_eq!(shell.dnd_finish(true), Err(ShellError::State));
    }

    #[test]
    fn offer_types_and_read() {
        let (rec, mut shell, _) = setup();
        assert!(shell.selection_types(NHSHELL_SEL_DND).unwrap().is_empty());
        offer(
            &rec,
            &mut shell,
            &[
                ("text/plain", "dropped text"),
                ("text/uri-list", "file:///tmp/a"),
            ],
            false,
        );
        let mut types = shell.selection_types(NHSHELL_SEL_DND).unwrap();
        types.sort();
        assert_eq!(types, ["text/plain", "text/uri-list"]);
        let data = shell.selection_read(NHSHELL_SEL_DND, "text/uri-list", 1000);
        assert_eq!(data.as_deref(), Ok(&b"file:///tmp/a"[..]));
        let data = shell.selection_read(NHSHELL_SEL_DND, "text/plain", 1000);
        assert_eq!(data.as_deref(), Ok(&b"dropped text"[..]));
        assert_eq!(
            shell.selection_read(NHSHELL_SEL_DND, "image/png", 1000),
            Err(ShellError::State)
        );
        assert!(!shell.selection_owned(NHSHELL_SEL_DND).unwrap());
        rec.control_ok(r#"{"cmd":"dnd_leave"}"#);
        wait_event(&mut shell, Kind::DndLeave);
        assert!(shell.selection_types(NHSHELL_SEL_DND).unwrap().is_empty());
        assert_eq!(
            shell.selection_read(NHSHELL_SEL_DND, "text/plain", 1000),
            Err(ShellError::State)
        );
        wait_log(&rec, &["\"destroyed\":true"]);
    }

    #[test]
    fn silent_offer_read_times_out() {
        let (rec, mut shell, _) = setup();
        offer(&rec, &mut shell, &[("text/plain", "never sent")], true);
        let t0 = Instant::now();
        assert_eq!(
            shell.selection_read(NHSHELL_SEL_DND, "text/plain", 300),
            Err(ShellError::Timeout)
        );
        let took = t0.elapsed();
        assert!(
            took >= Duration::from_millis(300) && took < Duration::from_millis(1000),
            "{took:?}"
        );
        wait_log(&rec, &["\"received\":[\"text/plain\"]"]);
        // the shell goes on: the drag still moves, leaves, and the offer is gone
        rec.control_ok(r#"{"cmd":"dnd_motion","at":[30,30]}"#);
        wait_event(&mut shell, Kind::DndMotion);
        rec.control_ok(r#"{"cmd":"dnd_leave"}"#);
        wait_event(&mut shell, Kind::DndLeave);
        wait_log(&rec, &["\"destroyed\":true"]);
        // and the next drop works
        offer(&rec, &mut shell, &[("text/plain", "second")], false);
        let data = shell.selection_read(NHSHELL_SEL_DND, "text/plain", 1000);
        assert_eq!(data.as_deref(), Ok(&b"second"[..]));
    }

    #[test]
    fn status_accepts_and_sets_actions() {
        let (rec, mut shell, _) = setup();
        plain_offer(&rec, &mut shell);
        // a preferred action outside the set, unknown bits
        assert_eq!(
            shell.dnd_status(Some("text/plain"), NHSHELL_DND_COPY, NHSHELL_DND_MOVE),
            Err(ShellError::Arg)
        );
        assert_eq!(
            shell.dnd_status(Some("text/plain"), 8, 0),
            Err(ShellError::Arg)
        );
        shell
            .dnd_status(
                Some("text/plain"),
                NHSHELL_DND_COPY | NHSHELL_DND_MOVE,
                NHSHELL_DND_MOVE,
            )
            .unwrap();
        wait_log(
            &rec,
            &[
                "\"accepted\":[\"text/plain\"]",
                "\"actions\":[\"copy\",\"move\"]",
                "\"preferred\":\"move\"",
                "\"chosen\":\"move\"",
            ],
        );
        // the compositor's choice comes with the next record
        rec.control_ok(r#"{"cmd":"dnd_motion","at":[11,12]}"#);
        let m = dnd(&wait_event(&mut shell, Kind::DndMotion));
        assert_eq!(
            (m.action, m.preferred),
            (NHSHELL_DND_MOVE, NHSHELL_DND_MOVE)
        );
    }

    #[test]
    fn finish_without_action_refused() {
        let (rec, mut shell, _) = setup();
        plain_offer(&rec, &mut shell);
        shell.dnd_status(Some("text/plain"), 0, 0).unwrap();
        wait_log(&rec, &["\"accepted\":[\"text/plain\"]"]);
        // `finish` here would be a protocol error: refused, and the offer still goes
        assert_eq!(shell.dnd_finish(true), Err(ShellError::State));
        wait_log(&rec, &["\"finished\":false", "\"destroyed\":true"]);
        none_of(&mut shell, Kind::Disconnected, 200);
        shell.dispatch_main().expect("the connection is up");
        assert_eq!(shell.last_error(), "");
        assert_eq!(shell.dnd_finish(false), Err(ShellError::State));
    }

    #[test]
    fn reject_sends_null_accept() {
        let (rec, mut shell, _) = setup();
        plain_offer(&rec, &mut shell);
        shell.dnd_status(None, 0, 0).unwrap();
        wait_log(&rec, &["\"accepted\":[null]"]);
        // a refused drop is no drop: the drag only leaves
        rec.control_ok(r#"{"cmd":"dnd_drop"}"#);
        let ev = wait_for(&mut shell, |e| {
            e.kind() == Kind::DndDrop || e.kind() == Kind::DndLeave
        });
        assert_eq!(ev.kind(), Kind::DndLeave);
        wait_log(&rec, &["\"finished\":false", "\"destroyed\":true"]);
        assert!(!shell.dragging());
    }

    // --- drags out of our window --------------------------------------------------------------------------

    #[test]
    fn drag_start_needs_a_held_button() {
        let (rec, mut shell, id) = setup();
        let items: &[(&str, &[u8])] = &[(TEXT, b"dragged text")];
        let both = NHSHELL_DND_COPY | NHSHELL_DND_MOVE;
        assert_eq!(
            shell.drag_start(id, items, both),
            Err(ShellError::State),
            "no press"
        );
        press(&rec, &mut shell);
        release(&rec, &mut shell);
        assert_eq!(
            shell.drag_start(id, items, both),
            Err(ShellError::State),
            "the press was released"
        );
        press(&rec, &mut shell);
        assert_eq!(shell.drag_start(id, &[], both), Err(ShellError::Arg));
        assert_eq!(shell.drag_start(id, items, 0), Err(ShellError::Arg));
        assert_eq!(
            shell.drag_start(id + 1000, items, both),
            Err(ShellError::State)
        );
        shell.drag_start(id, items, both).unwrap();
        assert!(shell.dragging());
        assert!(shell.selection_owned(NHSHELL_SEL_DND).unwrap());
        let log = wait_reply(&rec, r#"{"cmd":"drag_log"}"#, &["\"result\":\"started\""]);
        for want in [
            "\"serial_kind\":\"button\"".to_string(),
            format!("\"origin\":{id}"),
            "\"icon\":null".to_string(),
            format!("\"mimes\":[\"{TEXT}\"]"),
            "\"actions\":[\"copy\",\"move\"]".to_string(),
        ] {
            assert!(log.contains(&want), "{want} in {log}");
        }
    }

    #[test]
    fn drag_data_written_by_the_reader() {
        let (rec, mut shell, id) = setup();
        start_drag(&rec, &mut shell, id, &[(TEXT, b"dragged text")]);
        let r = rec.control_ok(&format!(
            r#"{{"cmd":"drag_target","accept":"{TEXT}","action":"copy"}}"#
        ));
        assert!(r.contains("\"ok\":true"), "{r}");
        // this thread waits in the control call and does not touch the shell: the reader writes the bytes
        let r = rec.control(r#"{"cmd":"drag_drop","finish":true}"#);
        assert!(r.contains("\"data\":\"dragged text\""), "{r}");
        assert!(r.contains("\"result\":\"finished\""), "{r}");
        let log = wait_reply(&rec, r#"{"cmd":"drag_log"}"#, &["\"result\":\"finished\""]);
        assert!(log.contains(&format!("\"sends\":[\"{TEXT}\"]")), "{log}");
    }

    #[test]
    fn drag_done_ok_on_finish() {
        let (rec, mut shell, id) = setup();
        start_drag(&rec, &mut shell, id, &[(TEXT, b"dragged text")]);
        rec.control_ok(&format!(
            r#"{{"cmd":"drag_target","accept":"{TEXT}","action":"copy"}}"#
        ));
        let r = rec.control(r#"{"cmd":"drag_drop","finish":true}"#);
        assert!(r.contains("\"result\":\"finished\""), "{r}");
        let evs = until_done(&mut shell);
        let at = |kind: Kind| {
            evs.iter()
                .position(|e| e.kind() == kind)
                .unwrap_or_else(|| panic!("no record {kind:?}"))
        };
        assert!(at(Kind::DragAction) < at(Kind::DragDropped));
        assert!(at(Kind::DragDropped) < at(Kind::DragDone));
        assert_eq!(dnd(&evs[at(Kind::DragAction)]).action, NHSHELL_DND_COPY);
        let done = evs.last().unwrap();
        assert_eq!(done.window, id);
        assert_eq!((dnd(done).ok, dnd(done).action), (1, NHSHELL_DND_COPY));
        assert!(!shell.dragging());
        assert!(!shell.selection_owned(NHSHELL_SEL_DND).unwrap());
        // the recorder ends its grab 150 ms later, when a source still alive would hear `cancelled`
        none_of(&mut shell, Kind::DragDone, 400);
    }

    #[test]
    fn drag_done_cancelled() {
        let (rec, mut shell, id) = setup();
        start_drag(&rec, &mut shell, id, &[(TEXT, b"dragged text")]);
        rec.control_ok(r#"{"cmd":"drag_cancel"}"#);
        let done = *until_done(&mut shell).last().unwrap();
        assert_eq!((dnd(&done).ok, dnd(&done).action), (0, 0));
        assert!(!shell.dragging());
        none_of(&mut shell, Kind::DragDone, 300);
        // our own cancel: the source goes and nothing follows (the pointer comes back over the window first: the
        // drag's grab had taken it)
        rec.control_ok(r#"{"cmd":"pointer","pos":[6,6]}"#);
        press(&rec, &mut shell);
        shell
            .drag_start(id, &[(TEXT, b"again")], NHSHELL_DND_COPY)
            .unwrap();
        shell.drag_cancel().unwrap();
        assert!(!shell.dragging());
        none_of(&mut shell, Kind::DragDone, 300);
        shell.drag_cancel().unwrap();
    }

    #[test]
    fn own_offer_read_without_round_trip() {
        let (rec, mut shell, id) = setup();
        let items: &[(&str, &[u8])] = &[(TEXT, b"own bytes"), ("text/uri-list", b"file:///x")];
        start_drag(&rec, &mut shell, id, items);
        // the drag moves over our own window
        rec.control_ok(r#"{"cmd":"pointer","pos":[30,30]}"#);
        let ev = wait_event(&mut shell, Kind::DndEnter);
        assert_eq!(ev.window, id);
        assert_eq!(dnd(&ev).own, 1);
        let mut types = shell.selection_types(NHSHELL_SEL_DND).unwrap();
        types.sort();
        assert_eq!(types, ["text/plain;charset=utf-8", "text/uri-list"]);
        // a zero timeout: any read through a pipe would time out
        let data = shell.selection_read(NHSHELL_SEL_DND, TEXT, 0);
        assert_eq!(data.as_deref(), Ok(&b"own bytes"[..]));
        // a drop on ourselves: status, drop, finish, done
        shell
            .dnd_status(Some(TEXT), NHSHELL_DND_COPY, NHSHELL_DND_COPY)
            .unwrap();
        // the compositor's answer to the status reaches our source
        wait_for(&mut shell, |e| e.kind() == Kind::DragAction);
        rec.control_ok(r#"{"cmd":"button","button":272,"state":"release"}"#);
        let drop = wait_event(&mut shell, Kind::DndDrop);
        assert_eq!((dnd(&drop).own, dnd(&drop).action), (1, NHSHELL_DND_COPY));
        shell.dnd_finish(true).unwrap();
        let done = *until_done(&mut shell).last().unwrap();
        assert_eq!(dnd(&done).ok, 1);
        // the drag's bytes stay readable after it ended
        let data = shell.selection_read(NHSHELL_SEL_DND, "text/uri-list", 0);
        assert_eq!(data.as_deref(), Ok(&b"file:///x"[..]));
    }

    #[test]
    fn disconnect_during_drag_reports_done_first() {
        let (rec, mut shell, id) = setup();
        start_drag(&rec, &mut shell, id, &[(TEXT, b"dragged text")]);
        assert!(rec.control(r#"{"cmd":"quit"}"#).contains("\"ok\":true"));
        let mut kinds = Vec::new();
        let mut done = None;
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            match shell.next_event() {
                Ok(Some(ev)) => {
                    if ev.kind() == Kind::DragDone {
                        done = Some(dnd(&ev));
                    }
                    kinds.push(ev.kind());
                }
                Ok(None) => {
                    assert!(Instant::now() < deadline, "no DISCONNECTED: {kinds:?}");
                    readable_within(shell.wake_fd(), 50);
                }
                Err(e) => {
                    assert_eq!(e, ShellError::Disconnected);
                    break;
                }
            }
        }
        let n = kinds.len();
        assert!(n >= 2, "{kinds:?}");
        assert_eq!(
            &kinds[n - 2..],
            [Kind::DragDone, Kind::Disconnected],
            "{kinds:?}"
        );
        assert_eq!(kinds.iter().filter(|&&k| k == Kind::DragDone).count(), 1);
        assert_eq!(done.map(|d| d.ok), Some(0));
        assert!(!shell.dragging());
        assert_eq!(shell.drag_cancel(), Err(ShellError::Disconnected));
    }
}

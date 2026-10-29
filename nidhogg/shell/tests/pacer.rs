//! T1 tests of the frame pacer against nh-recorder, one per way a window's frames used to stop (the numbers are the
//! paths of the pacing assessment): a commit without damage whose feedback never comes (1), a surface that is not
//! shown (2), a compositor that holds callbacks and says `suspended` (3), buffers a compositor never releases (4),
//! commits that are not counted (5), an ack in the middle of a wait (6) and readiness without any compositor event (7).
//! The harness is `common/mod.rs`.

mod common;

use common::*;
use glitnir::geom::Rect;
use nhshell::api::{
    NHSHELL_ACK_CONTENT, NHSHELL_READY_CONTENT, NHSHELL_READY_OWED, ShellBackend, WindowDesc,
};
use nhshell::event::*;
use nhshell::wayland::WaylandShell;
use std::time::{Duration, Instant};

fn full() -> Rect {
    Rect::new(0, 0, 200, 100)
}

/// (callbacks, feedback, buffers) the recorder has not answered or released.
fn outstanding(rec: &Recorder) -> (u64, u64, u64) {
    let r = rec.control(r#"{"cmd":"outstanding"}"#);
    let n = |k| number(&r, k).unwrap_or_else(|| panic!("{k} in {r}"));
    (n("callbacks"), n("feedback"), n("buffers"))
}

fn present(rec: &Recorder) {
    let r = rec.control(r#"{"cmd":"present","advance_ns":16000000}"#);
    assert!(r.contains("\"ok\":true"), "{r}");
}

/// Dispatch the shell's records until `f` holds (10 s at most).
fn until(shell: &mut WaylandShell, what: &str, f: impl Fn(&mut WaylandShell) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        while shell.next_event().unwrap().is_some() {}
        if f(shell) {
            return;
        }
        assert!(Instant::now() < deadline, "never: {what}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn waiting(shell: &WaylandShell, id: u64) -> bool {
    shell.present_pacer(id).unwrap().0
}

fn starved(shell: &WaylandShell, id: u64) -> bool {
    shell.present_pacer(id).unwrap().1
}

fn window(shell: &mut WaylandShell) -> u64 {
    let id = shell
        .window_new(&WindowDesc {
            w: 200,
            h: 100,
            title: "t".into(),
            app_id: "nidhogg.test".into(),
            flags: 0,
        })
        .unwrap();
    let ev = wait_for(shell, |e| e.kind() == Kind::Configure);
    shell
        .ack_configure(id, configure_of(&ev).serial, NHSHELL_ACK_CONTENT)
        .unwrap();
    id
}

/// One frame: begin, fill, commit.  The buffer's age.
fn draw(shell: &mut WaylandShell, id: u64, damage: &[Rect]) -> u32 {
    let b = shell.present_begin(id, 200, 100).unwrap();
    // SAFETY: the buffer is `stride * h` writable bytes.
    unsafe { std::ptr::write_bytes(b.pixels, 0x80, (b.stride * b.h) as usize) };
    shell.present_commit(id, damage).unwrap();
    shell.flush().unwrap();
    b.age
}

/// Two shown frames: both pool buffers have been written, and (on a profile that does not hold) the first is released.
fn warm(shell: &mut WaylandShell, rec: &Recorder, id: u64) {
    for _ in 0..2 {
        draw(shell, id, &[full()]);
        present(rec);
        until(shell, "the frame callback", |s| !waiting(s, id));
    }
    std::thread::sleep(Duration::from_millis(100));
}

/// The pacer's ready bits (`NHSHELL_READY_*`) of window `id`.
fn bits(shell: &mut WaylandShell, id: u64) -> u32 {
    shell.window_ready(id).unwrap()
}

fn ready(shell: &mut WaylandShell, id: u64) -> bool {
    bits(shell, id) != 0
}

/// Send a configure of `size` (with `states`), wait for it, record its ack with `flags`; its serial.
fn configure_acked(
    shell: &mut WaylandShell,
    rec: &Recorder,
    id: u64,
    size: (i32, i32),
    states: &str,
    flags: u32,
) -> u32 {
    let r = rec.control(&format!(
        r#"{{"cmd":"configure","surface":"focused","size":[{},{}],"states":[{states}]}}"#,
        size.0, size.1
    ));
    let serial = number(&r, "serial").expect("a serial") as u32;
    wait_for(shell, |e| e.kind() == Kind::Configure);
    shell.ack_configure(id, serial, flags).unwrap();
    serial
}

/// A configure of the same size (a bare ack).
fn reconfigure(shell: &mut WaylandShell, rec: &Recorder, id: u64, states: &str) -> u32 {
    configure_acked(shell, rec, id, (200, 100), states, 0)
}

fn commit_log(rec: &Recorder) -> String {
    rec.control(r#"{"cmd":"log","since":1}"#)
}

/// The recorder's next commit number.
fn next_seq(rec: &Recorder) -> u64 {
    number(&rec.control(r#"{"cmd":"log","since":1}"#), "next").expect("next")
}

/// The log from commit `seq` on.
fn log_since(rec: &Recorder, seq: u64) -> String {
    rec.control(&format!(r#"{{"cmd":"log","since":{seq}}}"#))
}

fn acked(log: &str, serial: u32) -> bool {
    log.contains(&format!("\"ack_serial\":{serial}"))
}

mod pacer {
    use super::*;

    /// Path 1: an ack goes out with a commit that has no damage.  It asks for no callback and no feedback, so the
    /// compositor owes nothing (sway answers feedback only for content) and the pacer does not wait for it.
    #[test]
    fn ack_commit_without_damage_owes_nothing() {
        let rec = Recorder::start_clock("sway", "manual");
        let mut shell = rec.open();
        let id = window(&mut shell);
        warm(&mut shell, &rec, id);
        reconfigure(&mut shell, &rec, id, "");
        assert!(draw(&mut shell, id, &[]) > 0, "a reused buffer");
        std::thread::sleep(Duration::from_millis(200));
        let o = outstanding(&rec);
        assert_eq!(
            (o.0, o.1),
            (0, 0),
            "nothing owed for a commit without content"
        );
        assert!(!waiting(&shell, id));
        shell.window_dirty(id).unwrap();
        assert!(ready(&mut shell, id), "the next frame may go at once");
    }

    /// Path 2: a commit on a surface that is not shown is never answered.  A configure re-arms the wait with a new
    /// callback and an empty commit (sent with the tree's next commit), and when the surface is shown again
    /// everything is answered.
    #[test]
    fn hidden_surface_is_asked_again_on_configure() {
        let rec = Recorder::start_clock("mutter", "manual");
        let mut shell = rec.open();
        let id = window(&mut shell);
        draw(&mut shell, id, &[full()]);
        assert!(rec.control(r#"{"cmd":"occlude"}"#).contains("\"ok\":true"));
        present(&rec);
        assert!(waiting(&shell, id));
        assert_eq!(outstanding(&rec).0, 1);
        reconfigure(&mut shell, &rec, id, "");
        wait_for(&mut shell, |e| e.kind() == Kind::FrameReady);
        assert!(!shell.present_tree(id).unwrap());
        shell.flush().unwrap();
        until(&mut shell, "the re-armed callback", |_| {
            outstanding(&rec).0 == 2
        });
        rec.control(r#"{"cmd":"unocclude"}"#);
        present(&rec);
        until(&mut shell, "the pacer idle again", |s| !waiting(s, id));
        let o = outstanding(&rec);
        assert_eq!((o.0, o.1), (0, 0));
    }

    /// Path 3: mutter holds callbacks of an occluded window and says `suspended` later.  A suspended window only
    /// acks (the ack's commit goes out, content waits), and the end of suspension makes the pacer ready.
    #[test]
    fn suspended_window_only_acks() {
        let rec = Recorder::start_clock("mutter", "manual");
        let mut shell = rec.open();
        let id = window(&mut shell);
        warm(&mut shell, &rec, id);
        let serial = reconfigure(&mut shell, &rec, id, r#""suspended""#);
        assert!(ready(&mut shell, id), "the ack is owed");
        draw(&mut shell, id, &[]);
        let log = commit_log(&rec);
        assert!(log.contains(&format!("\"ack_serial\":{serial}")), "{log}");
        shell.window_dirty(id).unwrap();
        assert!(!ready(&mut shell, id), "suspended: content waits");
        while shell.next_event().unwrap().is_some() {}
        reconfigure(&mut shell, &rec, id, "");
        wait_for(&mut shell, |e| e.kind() == Kind::FrameReady);
        draw(&mut shell, id, &[]);
        shell.window_dirty(id).unwrap();
        assert!(ready(&mut shell, id), "suspension ended");
    }

    /// Path 4: the compositor keeps buffers.  With the pool's two buffers busy and the pacer idle the pool grows to
    /// three; with three busy the pacer is starved and leaves that state on a release.
    #[test]
    fn busy_buffers_grow_the_pool_then_starve() {
        let rec = Recorder::start_clock("kwin", "manual");
        rec.control(r#"{"cmd":"mode","hold_buffers":2}"#);
        let mut shell = rec.open();
        let id = window(&mut shell);
        for _ in 0..2 {
            draw(&mut shell, id, &[full()]);
            present(&rec);
            until(&mut shell, "the frame callback", |s| !waiting(s, id));
        }
        // both buffers are the compositor's: the third frame has a third buffer
        draw(&mut shell, id, &[full()]);
        present(&rec);
        until(&mut shell, "the frame callback", |s| !waiting(s, id));
        assert!(shell.present_begin(id, 200, 100).is_err(), "all three busy");
        assert!(starved(&shell, id));
        while shell.next_event().unwrap().is_some() {}
        shell.window_dirty(id).unwrap();
        assert!(!ready(&mut shell, id), "starved");
        rec.control(r#"{"cmd":"release"}"#);
        wait_for(&mut shell, |e| e.kind() == Kind::FrameReady);
        assert!(!starved(&shell, id));
        assert!(ready(&mut shell, id));
    }

    /// Path 4, waiting: two buffers busy while the pacer waits (an ack's commit went out in the middle of a wait)
    /// starve it, and the frame callback ends that: it is idle, and the pool grows.
    #[test]
    fn frame_callback_ends_starvation_by_growing() {
        let rec = Recorder::start_clock("kwin", "manual");
        rec.control(r#"{"cmd":"mode","hold_buffers":2}"#);
        let mut shell = rec.open();
        let id = window(&mut shell);
        draw(&mut shell, id, &[full()]);
        draw(&mut shell, id, &[full()]);
        assert!(waiting(&shell, id));
        assert!(
            shell.present_begin(id, 200, 100).is_err(),
            "two busy, waiting"
        );
        assert!(starved(&shell, id));
        present(&rec);
        until(&mut shell, "the pacer idle", |s| !waiting(s, id));
        assert!(!starved(&shell, id));
        shell.window_dirty(id).unwrap();
        assert!(ready(&mut shell, id));
        draw(&mut shell, id, &[full()]);
    }

    /// Path 5: there is no count of commits to drift: two commits before the first callback end the wait together.
    #[test]
    fn commits_are_not_counted() {
        let rec = Recorder::start_clock("sway", "manual");
        let mut shell = rec.open();
        let id = window(&mut shell);
        draw(&mut shell, id, &[full()]);
        draw(&mut shell, id, &[full()]);
        present(&rec);
        until(&mut shell, "the frame callbacks", |s| !waiting(s, id));
        shell.window_dirty(id).unwrap();
        assert!(ready(&mut shell, id));
    }

    /// Path 6: an ack never waits: a configure that arrives while a frame is pending is acked, and its commit is
    /// made, with no compositor event in between.
    #[test]
    fn ack_does_not_wait_for_the_frame_callback() {
        let rec = Recorder::start_clock("sway", "manual");
        let mut shell = rec.open();
        let id = window(&mut shell);
        draw(&mut shell, id, &[full()]);
        assert!(waiting(&shell, id));
        let serial = reconfigure(&mut shell, &rec, id, "");
        assert!(ready(&mut shell, id), "waiting, with an ack owed");
        draw(&mut shell, id, &[full()]);
        let log = commit_log(&rec);
        assert!(log.contains(&format!("\"ack_serial\":{serial}")), "{log}");
    }

    /// Path 7: a window that is idle and drawn on is ready with nothing from the compositor, and says so once.
    #[test]
    fn dirty_idle_window_is_ready() {
        let rec = Recorder::start_clock("sway", "manual");
        let mut shell = rec.open();
        let id = window(&mut shell);
        warm(&mut shell, &rec, id);
        while shell.next_event().unwrap().is_some() {}
        shell.window_dirty(id).unwrap();
        shell.window_dirty(id).unwrap();
        let mut posted = 0;
        let deadline = Instant::now() + Duration::from_millis(300);
        while Instant::now() < deadline {
            while let Some(e) = shell.next_event().unwrap() {
                posted += usize::from(e.kind() == Kind::FrameReady && e.window == id);
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(posted, 1);
        assert!(ready(&mut shell, id));
    }

    /// A bare commit has no buffer, frame callback or feedback, and leaves the pacer alone.
    #[test]
    fn bare_commit_has_no_buffer_or_callback() {
        let rec = Recorder::start_clock("sway", "manual");
        let mut shell = rec.open();
        let id = window(&mut shell);
        warm(&mut shell, &rec, id);
        let seq = next_seq(&rec);
        shell.present_bare(id).unwrap();
        until(&mut shell, "the bare commit", |_| {
            log_since(&rec, seq).contains("\"role\":\"toplevel\"")
        });
        let log = log_since(&rec, seq);
        assert!(log.contains("\"buffer\":null"), "{log}");
        assert!(log.contains("\"frame_cb\":false"), "{log}");
        assert!(log.contains("\"presentation\":false"), "{log}");
        assert!(!waiting(&shell, id));
        let o = outstanding(&rec);
        assert_eq!((o.0, o.1), (0, 0));
    }

    /// An ack goes out right before the commit that carries it, and with nothing else.
    #[test]
    fn ack_goes_out_with_its_commit() {
        let rec = Recorder::start_clock("sway", "manual");
        let mut shell = rec.open();
        let id = window(&mut shell);
        warm(&mut shell, &rec, id);
        let serial = reconfigure(&mut shell, &rec, id, "");
        shell.flush().unwrap();
        std::thread::sleep(Duration::from_millis(150));
        assert!(!acked(&commit_log(&rec), serial), "no commit, no ack");
        draw(&mut shell, id, &[full()]);
        std::thread::sleep(Duration::from_millis(100));
        assert!(acked(&commit_log(&rec), serial), "{}", commit_log(&rec));
    }

    /// A waiting pacer re-arms at a configure (a frame callback and an empty commit).  The re-arm waits for the
    /// tree's commit and goes after it, as a commit of its own: it carries no ack, and the bare commit no callback.
    #[test]
    fn rearm_never_carries_an_ack() {
        let rec = Recorder::start_clock("sway", "manual");
        let mut shell = rec.open();
        let id = window(&mut shell);
        draw(&mut shell, id, &[full()]);
        assert!(waiting(&shell, id));
        let serial = reconfigure(&mut shell, &rec, id, "");
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(outstanding(&rec).0, 1, "recorded, not sent");
        let seq = next_seq(&rec);
        shell.present_bare(id).unwrap();
        until(&mut shell, "the re-armed callback", |_| {
            outstanding(&rec).0 == 2
        });
        let log = log_since(&rec, seq);
        let commits: Vec<&str> = log.split("{\"ack_serial\":").skip(1).collect();
        assert_eq!(commits.len(), 2, "{log}");
        assert!(commits[0].starts_with(&serial.to_string()), "{log}");
        assert!(commits[0].contains("\"frame_cb\":false"), "{log}");
        assert!(commits[1].starts_with("null"), "{log}");
        assert!(commits[1].contains("\"frame_cb\":true"), "{log}");
    }

    /// Three buffers busy and a new size acked: nothing is owed that could go bare, a bare commit carries no ack, and
    /// the buffer commit after a release carries it.
    #[test]
    fn resize_ack_never_bare_while_starved() {
        let rec = Recorder::start_clock("kwin", "manual");
        rec.control(r#"{"cmd":"mode","hold_buffers":2}"#);
        let mut shell = rec.open();
        let id = window(&mut shell);
        for _ in 0..3 {
            draw(&mut shell, id, &[full()]);
            present(&rec);
            until(&mut shell, "the frame callback", |s| !waiting(s, id));
        }
        assert!(shell.present_begin(id, 200, 100).is_err(), "all three busy");
        assert!(starved(&shell, id));
        while shell.next_event().unwrap().is_some() {}
        let serial = configure_acked(&mut shell, &rec, id, (300, 150), "", NHSHELL_ACK_CONTENT);
        assert_eq!(bits(&mut shell, id), 0, "a held content ack is not owed");
        let seq = next_seq(&rec);
        shell.present_bare(id).unwrap();
        until(&mut shell, "the bare commit", |_| {
            log_since(&rec, seq).contains("\"role\":\"toplevel\"")
        });
        assert!(!acked(&log_since(&rec, seq), serial), "no bare ack");
        rec.control(r#"{"cmd":"release"}"#);
        wait_for(&mut shell, |e| e.kind() == Kind::FrameReady);
        let b = bits(&mut shell, id);
        assert_ne!(b & NHSHELL_READY_CONTENT, 0);
        assert_ne!(
            b & nhshell::api::NHSHELL_READY_ACK,
            0,
            "a content ack waits"
        );
        draw(&mut shell, id, &[full()]);
        std::thread::sleep(Duration::from_millis(100));
        assert!(
            acked(&log_since(&rec, seq), serial),
            "the buffer carries it"
        );
    }

    /// A begin that finds every buffer busy does not take the draw that `window_ready` reported: the release brings
    /// `FRAME_READY` and `CONTENT` with no new `window_dirty`.
    #[test]
    fn starved_begin_keeps_the_draw() {
        let rec = Recorder::start_clock("kwin", "manual");
        rec.control(r#"{"cmd":"mode","hold_buffers":2}"#);
        let mut shell = rec.open();
        let id = window(&mut shell);
        for _ in 0..3 {
            draw(&mut shell, id, &[full()]);
            present(&rec);
            until(&mut shell, "the frame callback", |s| !waiting(s, id));
        }
        shell.window_dirty(id).unwrap();
        assert_eq!(bits(&mut shell, id), NHSHELL_READY_CONTENT);
        assert!(shell.present_begin(id, 200, 100).is_err(), "all three busy");
        while shell.next_event().unwrap().is_some() {}
        rec.control(r#"{"cmd":"release"}"#);
        wait_for(&mut shell, |e| e.kind() == Kind::FrameReady);
        assert_eq!(bits(&mut shell, id), NHSHELL_READY_CONTENT);
    }

    /// A content ack waives the wait: the pool grows to three buffers where two are busy and the pacer waits.
    #[test]
    fn content_ack_grows_the_pool_while_waiting() {
        let rec = Recorder::start_clock("kwin", "manual");
        rec.control(r#"{"cmd":"mode","hold_buffers":2}"#);
        let mut shell = rec.open();
        let id = window(&mut shell);
        draw(&mut shell, id, &[full()]);
        draw(&mut shell, id, &[full()]);
        assert!(waiting(&shell, id));
        let serial = configure_acked(&mut shell, &rec, id, (300, 150), "", NHSHELL_ACK_CONTENT);
        assert!(shell.present_begin(id, 300, 150).is_ok(), "a third buffer");
        shell.present_commit(id, &[full()]).unwrap();
        shell.flush().unwrap();
        std::thread::sleep(Duration::from_millis(100));
        assert!(acked(&commit_log(&rec), serial));
    }

    /// A bare ack is ready (OWED) while the pacer is starved, and content is not.
    #[test]
    fn bare_ack_is_ready_while_starved() {
        let rec = Recorder::start_clock("kwin", "manual");
        rec.control(r#"{"cmd":"mode","hold_buffers":2}"#);
        let mut shell = rec.open();
        let id = window(&mut shell);
        for _ in 0..3 {
            draw(&mut shell, id, &[full()]);
            present(&rec);
            until(&mut shell, "the frame callback", |s| !waiting(s, id));
        }
        assert!(shell.present_begin(id, 200, 100).is_err());
        while shell.next_event().unwrap().is_some() {}
        let serial = reconfigure(&mut shell, &rec, id, "");
        assert_eq!(bits(&mut shell, id), NHSHELL_READY_OWED);
        shell.present_bare(id).unwrap();
        std::thread::sleep(Duration::from_millis(100));
        assert!(acked(&commit_log(&rec), serial), "{}", commit_log(&rec));
    }

    /// Asking `window_ready` takes the draw: with nothing presented, the next callbacks post no `FRAME_READY`.
    #[test]
    fn ready_clears_dirty() {
        let rec = Recorder::start_clock("sway", "manual");
        let mut shell = rec.open();
        let id = window(&mut shell);
        draw(&mut shell, id, &[full()]);
        while shell.next_event().unwrap().is_some() {}
        shell.window_dirty(id).unwrap();
        present(&rec);
        wait_for(&mut shell, |e| e.kind() == Kind::FrameReady);
        assert_ne!(bits(&mut shell, id) & NHSHELL_READY_CONTENT, 0);
        // the host decided on "nothing": a second ask finds nothing, and no record follows
        assert_eq!(bits(&mut shell, id), 0);
        assert!(
            collect(&mut shell, 300)
                .iter()
                .all(|e| e.kind() != Kind::FrameReady)
        );
    }
}

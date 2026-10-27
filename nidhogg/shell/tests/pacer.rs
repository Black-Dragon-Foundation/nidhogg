//! T1 tests of the frame pacer against nh-recorder, one per way a window's frames used to stop (the numbers are the
//! paths of the pacing assessment): a commit without damage whose feedback never comes (1), a surface that is not
//! shown (2), a compositor that holds callbacks and says `suspended` (3), buffers a compositor never releases (4),
//! commits that are not counted (5), an ack in the middle of a wait (6) and readiness without any compositor event (7).
//! The harness is `common/mod.rs`.

mod common;

use common::*;
use glitnir::geom::Rect;
use nhshell::api::{ShellBackend, WindowDesc};
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
    let ev = wait_for(shell, |e| e.kind == NHSHELL_EV_CONFIGURE);
    // SAFETY: a CONFIGURE record carries `configure`.
    shell
        .ack_configure(id, unsafe { ev.u.configure }.serial)
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

/// Send a configure of the same size (with `states`), wait for it, ack it; its serial.
fn reconfigure(shell: &mut WaylandShell, rec: &Recorder, id: u64, states: &str) -> u32 {
    let r = rec.control(&format!(
        r#"{{"cmd":"configure","surface":"focused","size":[200,100],"states":[{states}]}}"#
    ));
    let serial = number(&r, "serial").expect("a serial") as u32;
    wait_for(shell, |e| e.kind == NHSHELL_EV_CONFIGURE);
    shell.ack_configure(id, serial).unwrap();
    serial
}

fn commit_log(rec: &Recorder) -> String {
    rec.control(r#"{"cmd":"log","since":1}"#)
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
        assert!(
            shell.window_ready(id).unwrap(),
            "the next frame may go at once"
        );
    }

    /// Path 2: a commit on a surface that is not shown is never answered.  A configure re-arms the wait with a new
    /// callback and an empty commit, and when the surface is shown again everything is answered.
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
        assert!(shell.window_ready(id).unwrap(), "the ack is owed");
        draw(&mut shell, id, &[]);
        let log = commit_log(&rec);
        assert!(log.contains(&format!("\"ack_serial\":{serial}")), "{log}");
        shell.window_dirty(id).unwrap();
        assert!(!shell.window_ready(id).unwrap(), "suspended: content waits");
        while shell.next_event().unwrap().is_some() {}
        reconfigure(&mut shell, &rec, id, "");
        wait_for(&mut shell, |e| e.kind == NHSHELL_EV_FRAME_READY);
        draw(&mut shell, id, &[]);
        shell.window_dirty(id).unwrap();
        assert!(shell.window_ready(id).unwrap(), "suspension ended");
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
        assert!(!shell.window_ready(id).unwrap(), "starved");
        rec.control(r#"{"cmd":"release"}"#);
        wait_for(&mut shell, |e| e.kind == NHSHELL_EV_FRAME_READY);
        assert!(!starved(&shell, id));
        assert!(shell.window_ready(id).unwrap());
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
        assert!(shell.present_begin(id, 200, 100).is_err(), "two busy, waiting");
        assert!(starved(&shell, id));
        present(&rec);
        until(&mut shell, "the pacer idle", |s| !waiting(s, id));
        assert!(!starved(&shell, id));
        shell.window_dirty(id).unwrap();
        assert!(shell.window_ready(id).unwrap());
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
        assert!(shell.window_ready(id).unwrap());
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
        assert!(shell.window_ready(id).unwrap(), "waiting, with an ack owed");
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
        let mut ready = 0;
        let deadline = Instant::now() + Duration::from_millis(300);
        while Instant::now() < deadline {
            while let Some(e) = shell.next_event().unwrap() {
                ready += usize::from(e.kind == NHSHELL_EV_FRAME_READY && e.window == id);
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(ready, 1);
        assert!(shell.window_ready(id).unwrap());
    }
}

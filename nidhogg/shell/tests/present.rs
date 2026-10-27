//! T1 tests of the presenter against nh-recorder (manual clock): damage, viewport, frame callbacks, idle trim.
//! The harness is `common/mod.rs`.

mod common;

use common::*;
use glitnir::geom::Rect;
use nhshell::api::{ShellBackend, WindowDesc};
use nhshell::event::*;
use nhshell::wayland::WaylandShell;
use std::time::{Duration, Instant};

/// Whether a matching record arrives within `ms`.
fn arrives(shell: &mut WaylandShell, ms: u64, kind: u32) -> bool {
    let deadline = Instant::now() + Duration::from_millis(ms);
    loop {
        while let Some(ev) = shell.next_event().unwrap() {
            if ev.kind == kind {
                return true;
            }
        }
        if Instant::now() >= deadline {
            return false;
        }
        readable_within(shell.wake_fd(), 20);
    }
}

/// The commits of a `log` reply, one JSON object text each, that carry a buffer.
fn buffer_commits(rec: &Recorder) -> Vec<String> {
    let log = rec.control(r#"{"cmd":"log","since":1}"#);
    // the reply's objects have sorted keys, so `ack_serial` opens each commit
    log.split("{\"ack_serial\":")
        .skip(1)
        .map(|c| format!("{{\"ack_serial\":{c}"))
        .filter(|c| c.contains("\"buffer\":{"))
        .collect()
}

fn wait_buffer_commits(rec: &Recorder, n: usize) -> Vec<String> {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        let all = buffer_commits(rec);
        if all.len() >= n || Instant::now() > deadline {
            return all;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn present(rec: &Recorder) {
    let r = rec.control(r#"{"cmd":"present","advance_ns":16000000}"#);
    assert!(r.contains("\"ok\":true"), "{r}");
}

/// A window of 200x100 logical pixels whose first configure is acked.
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

fn draw(shell: &mut WaylandShell, id: u64, w: i32, h: i32, damage: &[Rect]) -> u32 {
    let b = shell.present_begin(id, w, h).unwrap();
    assert!(!b.pixels.is_null());
    assert_eq!(b.stride % 256, 0);
    // SAFETY: the buffer is `stride * h` writable bytes.
    unsafe { std::ptr::write_bytes(b.pixels, 0x80, (b.stride * b.h) as usize) };
    shell.present_commit(id, damage).unwrap();
    shell.flush().unwrap();
    b.age
}

#[allow(clippy::module_inception)]
mod present {
    use super::*;

    #[test]
    fn damage_is_logged() {
        let rec = Recorder::start_clock("sway", "manual");
        let mut shell = rec.open();
        let id = window(&mut shell);
        assert!(
            rec.control(r#"{"cmd":"scale","value120":180}"#)
                .contains("\"ok\":true")
        );
        wait_for(&mut shell, |e| {
            // SAFETY: SCALE records carry `scale`.
            e.kind == NHSHELL_EV_SCALE && unsafe { e.u.scale.scale120 } == 180
        });
        let (w, h) = shell.present_buffer_size(id, 200, 100);
        assert_eq!((w, h), (300, 150));
        // a buffer that was never written is committed whole; the first reuse carries the host's own damage
        draw(&mut shell, id, w, h, &[Rect::new(5, 6, 100, 20)]);
        let first = wait_buffer_commits(&rec, 1);
        assert!(
            first[0].contains("\"rect\":[0,0,300,150]}]"),
            "{}",
            first[0]
        );
        assert!(first[0].contains("\"dst\":[200,100]"), "{}", first[0]);
        assert!(first[0].contains("\"buffer_scale\":1"), "{}", first[0]);
        let mut n = 1;
        loop {
            present(&rec);
            std::thread::sleep(Duration::from_millis(100));
            n += 1;
            if draw(&mut shell, id, w, h, &[Rect::new(5, 6, 100, 20)]) > 0 {
                break;
            }
            assert!(n < 6, "no buffer was ever reused");
        }
        let log = wait_buffer_commits(&rec, n);
        let c = log.last().unwrap();
        assert!(c.contains("\"rect\":[5,6,100,20]}]"), "{c}");
    }

    #[test]
    fn every_commit_is_made() {
        // a commit while the previous frame is pending is made at once; a new buffer goes out with full damage
        let rec = Recorder::start_clock("sway", "manual");
        let mut shell = rec.open();
        let id = window(&mut shell);
        draw(&mut shell, id, 200, 100, &[Rect::new(0, 0, 10, 10)]);
        draw(&mut shell, id, 200, 100, &[Rect::new(50, 50, 10, 10)]);
        let log = wait_buffer_commits(&rec, 2);
        assert_eq!(log.len(), 2);
        assert!(log[1].contains("\"rect\":[0,0,200,100]}]"), "{}", log[1]);
    }

    #[test]
    fn frame_callback_paces_without_presentation() {
        let rec = Recorder::start_clock("no-presentation", "manual");
        let mut shell = rec.open();
        let id = window(&mut shell);
        draw(&mut shell, id, 200, 100, &[Rect::new(0, 0, 10, 10)]);
        wait_buffer_commits(&rec, 1);
        assert_eq!(shell.present_pacer(id).map(|p| p.0), Some(true), "waits");
        while shell.next_event().unwrap().is_some() {}
        shell.window_dirty(id).unwrap();
        assert!(!shell.window_ready(id).unwrap(), "not before the callback");
        present(&rec);
        wait_for(&mut shell, |e| {
            e.kind == NHSHELL_EV_FRAME_READY && e.window == id
        });
        assert!(
            shell.window_ready(id).unwrap(),
            "{:?}",
            shell.present_pacer(id)
        );
        assert_eq!(shell.present_pacer(id).map(|p| p.0), Some(false));
    }

    #[test]
    fn frame_done_after_present() {
        let rec = Recorder::start_clock("sway", "manual");
        let mut shell = rec.open();
        let id = window(&mut shell);
        draw(&mut shell, id, 200, 100, &[Rect::new(0, 0, 10, 10)]);
        wait_buffer_commits(&rec, 1);
        assert!(
            !arrives(&mut shell, 200, NHSHELL_EV_FRAME_DONE),
            "manual clock: no frame yet"
        );
        present(&rec);
        let ev = wait_for(&mut shell, |e| e.kind == NHSHELL_EV_FRAME_DONE);
        assert_eq!(ev.window, id);
        // SAFETY: FRAME_DONE records carry `frame_done`.
        let fd = unsafe { ev.u.frame_done };
        assert_ne!(fd.presented_ns, 0, "the profile has wp_presentation");
        assert!(fd.refresh_ns > 0);
    }

    #[test]
    fn idle_frees_buffers() {
        // The recorder does not log buffer destruction: the shell's own count stands in for it.
        let rec = Recorder::start_clock("sway", "manual");
        let mut shell = rec.open();
        let id = window(&mut shell);
        for _ in 0..2 {
            draw(&mut shell, id, 200, 100, &[Rect::new(0, 0, 10, 10)]);
            present(&rec);
            wait_for(&mut shell, |e| e.kind == NHSHELL_EV_FRAME_DONE);
        }
        // let the first buffer's release arrive
        std::thread::sleep(Duration::from_millis(200));
        assert_eq!(shell.present_stats(id).map(|s| (s.0, s.1)), Some((2, 0)));
        std::thread::sleep(Duration::from_millis(2100));
        shell.dispatch_main().unwrap();
        assert_eq!(shell.present_stats(id).map(|s| (s.0, s.1)), Some((1, 1)));
    }

    #[test]
    fn memfd_pages_aligned() {
        let rec = Recorder::start_clock("sway", "manual");
        let mut shell = rec.open();
        let id = window(&mut shell);
        draw(&mut shell, id, 201, 101, &[]);
        let (_, _, len) = shell.present_stats(id).unwrap();
        // SAFETY: sysconf has no preconditions.
        let page = unsafe { libc::sysconf(libc::_SC_PAGESIZE) } as usize;
        assert!(len > 0 && len % page == 0, "{len}");
    }
}

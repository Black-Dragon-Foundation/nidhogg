//! T1 test of closing the shell after the display went: nothing it opened stays behind.  Alone in its binary, so that
//! the process's descriptor and thread counts are this test's own.  The harness is `common/mod.rs`.

mod common;

use common::*;
use nhshell::api::{ShellBackend, ShellError};
use std::time::{Duration, Instant};

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

/// One cycle: open, a mapped window (one presented buffer), the compositor drops every client (a restart in place, the
/// socket kept), `DISCONNECTED`, close.
fn cycle(rec: &Recorder) {
    let mut shell = rec.open();
    // 470 KiB of drawn buffer: a kept mapping would show in VmRSS
    let w = mapped_window(&mut shell, 400, 300);
    let reply = rec.control(r#"{"cmd":"restart","mode":"keep-socket"}"#);
    assert!(reply.contains("\"ok\":true"), "{reply}");
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        match shell.next_event() {
            Ok(Some(_)) => {}
            Ok(None) => {
                assert!(Instant::now() < deadline, "no DISCONNECTED");
                // the recorder closes a killed client's socket at its next dispatch of the clients, which a request
                // from any client starts
                let _ = shell.set_title(w, "t");
                let _ = shell.flush();
                readable_within(shell.wake_fd(), 50);
            }
            Err(e) => {
                assert_eq!(e, ShellError::Disconnected);
                break;
            }
        }
    }
    drop(shell);
}

/// The plan names it `reader::…` (and `check.sh --test reader` selects it).
mod reader {
    use super::*;

    /// 50 cycles of open, window, present, the compositor going, close: the descriptors and threads come back to the
    /// baseline, and the resident set grows less than 2 MiB.
    #[test]
    fn close_joins_the_reader_and_frees() {
        let rec = Recorder::start("sway");
        // the first cycles fill what lives as long as the process (libwayland's log handler, allocator arenas)
        for _ in 0..3 {
            cycle(&rec);
        }
        let (fd0, task0, rss0) = (fds(), tasks(), rss_kib());
        for _ in 0..50 {
            cycle(&rec);
        }
        let (fd1, task1, rss1) = (fds(), tasks(), rss_kib());
        assert_eq!(fd1, fd0, "descriptors");
        assert_eq!(task1, task0, "threads");
        assert!(
            rss1 < rss0 + 2048,
            "VmRSS grew {} KiB over 50 cycles",
            rss1.saturating_sub(rss0)
        );
        // the compositor saw every client go
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let c = rec.control(r#"{"cmd":"connections"}"#);
            if number(&c, "live") == Some(0) {
                break;
            }
            assert!(Instant::now() < deadline, "clients left: {c}");
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

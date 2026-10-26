//! T1 tests of the connection, the capabilities and the reader thread, against nh-recorder (S0's recording
//! compositor).  `NH_RECORDER` names the recorder binary; each test starts its own with a temporary
//! `XDG_RUNTIME_DIR` and connects by absolute socket path, never to the environment's display.

use nhshell::api::{NHSHELL_E_DISCONNECTED, ShellBackend};
use nhshell::event::{NHSHELL_EV_DISCONNECTED, NHSHELL_EV_FOCUS, NhShellEvent};
use nhshell::ffi::{
    NhShell, nhshell_close, nhshell_dispatch_main, nhshell_next_event, nhshell_open,
    nhshell_wake_fd,
};
use nhshell::wayland::WaylandShell;
use nhshell::wayland::registry::supported;
use rustix::event::{PollFd, PollFlags, Timespec, poll};
use rustix::fd::BorrowedFd;
use rustix::process::{Pid, Signal, kill_process};
use std::ffi::CString;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

struct Recorder {
    child: Child,
    dir: PathBuf,
}

impl Recorder {
    fn start(profile: &str) -> Recorder {
        let bin = std::env::var_os("NH_RECORDER")
            .expect("NH_RECORDER is not set (the path of S0's nh-recorder binary)");
        static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!("nhshell-reader-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let profile = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../test/recorder/profiles")
            .join(format!("{profile}.json"));
        let mut child = Command::new(bin)
            .args(["--socket", "wl", "--control"])
            .arg(dir.join("control"))
            .arg("--profile")
            .arg(&profile)
            .env("XDG_RUNTIME_DIR", &dir)
            .stdout(Stdio::piped())
            .spawn()
            .expect("spawn nh-recorder");
        let mut line = String::new();
        BufReader::new(child.stdout.take().unwrap())
            .read_line(&mut line)
            .unwrap();
        assert_eq!(line, "READY\n");
        Recorder { child, dir }
    }

    fn display(&self) -> CString {
        CString::new(self.dir.join("wl").as_os_str().as_bytes()).unwrap()
    }

    fn open(&self) -> WaylandShell {
        WaylandShell::open(Some(&self.display())).expect("open the shell on the recorder")
    }

    /// One control command; the reply line.
    fn control(&self, req: &str) -> String {
        let mut s = UnixStream::connect(self.dir.join("control")).unwrap();
        writeln!(s, "{req}").unwrap();
        let mut line = String::new();
        BufReader::new(s).read_line(&mut line).unwrap();
        line
    }

    fn signal(&self, sig: Signal) {
        let pid = Pid::from_raw(self.child.id() as i32).unwrap();
        kill_process(pid, sig).unwrap();
    }
}

impl Drop for Recorder {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// The number after `"key":` in a JSON reply (no JSON crate: nhshell takes no third-party crates).
fn number(json: &str, key: &str) -> Option<u64> {
    let pat = format!("\"{key}\":");
    let rest = json[json.find(&pat)? + pat.len()..].trim_start();
    let end = rest
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(rest.len());
    rest[..end].parse().ok()
}

fn readable_within(fd: i32, ms: i64) -> bool {
    // SAFETY: the shell's wake descriptor stays open while the shell lives.
    let fd = unsafe { BorrowedFd::borrow_raw(fd) };
    let mut fds = [PollFd::new(&fd, PollFlags::IN)];
    let t = Timespec {
        tv_sec: ms / 1000,
        tv_nsec: (ms % 1000) * 1_000_000,
    };
    matches!(poll(&mut fds, Some(&t)), Ok(n) if n > 0)
}

/// Take every queued record.
fn drain(shell: &mut WaylandShell) -> Vec<NhShellEvent> {
    let mut out = Vec::new();
    while let Some(ev) = shell.next_event().unwrap() {
        out.push(ev);
    }
    out
}

/// Retry a control command until it succeeds (a request the shell just flushed may not be processed yet).
fn control_ok(rec: &Recorder, req: &str) -> String {
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let reply = rec.control(req);
        if reply.contains("\"ok\":true") || Instant::now() > deadline {
            return reply;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
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
            // mutter 49 serves xdg_wm_base 7; the recorder (Smithay 0.7) serves at most 6 (S0 ledger)
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
        let reply = control_ok(&rec, &format!(r#"{{"cmd":"focus","surface":{surface}}}"#));
        assert!(reply.contains("\"ok\":true"), "{reply}");
        assert!(
            readable_within(shell.wake_fd(), 100),
            "no wake within 100 ms"
        );
        let focus: Vec<NhShellEvent> = drain(&mut shell)
            .into_iter()
            .filter(|e| e.kind == NHSHELL_EV_FOCUS)
            .collect();
        assert_eq!(focus.len(), 1);
        assert_eq!(focus[0].window, surface);
        // SAFETY: a FOCUS record's valid member is `focus`.
        assert_eq!(unsafe { focus[0].u.focus.r#in }, 1);
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
        let display = rec.display();
        // through the C ABI, as the host sees it
        let mut sh: *mut NhShell = std::ptr::null_mut();
        // SAFETY: `display` is NUL-terminated; `sh` is writable.
        assert_eq!(unsafe { nhshell_open(display.as_ptr(), &mut sh) }, 0);
        // SAFETY: a live handle.
        let wake = unsafe { nhshell_wake_fd(sh) };
        assert!(wake >= 0);
        assert!(rec.control(r#"{"cmd":"quit"}"#).contains("\"ok\":true"));
        assert!(
            readable_within(wake, 2000),
            "no wake after the compositor quit"
        );
        let mut kinds = Vec::new();
        let mut ev = NhShellEvent::new(0, 0, 0);
        let deadline = Instant::now() + Duration::from_secs(2);
        let last = loop {
            // SAFETY: a live handle; `ev` is writable.
            match unsafe { nhshell_next_event(sh, &mut ev) } {
                1 => kinds.push(ev.kind),
                0 => {
                    assert!(Instant::now() < deadline, "no DISCONNECTED: {kinds:?}");
                    readable_within(wake, 100);
                }
                r => break r,
            }
        };
        assert_eq!(last, NHSHELL_E_DISCONNECTED);
        assert_eq!(kinds.last(), Some(&NHSHELL_EV_DISCONNECTED), "{kinds:?}");
        assert_eq!(
            kinds
                .iter()
                .filter(|&&k| k == NHSHELL_EV_DISCONNECTED)
                .count(),
            1
        );
        // later calls keep reporting the loss, without a panic or a hang
        // SAFETY: a live handle, closed last.
        unsafe {
            assert_eq!(nhshell_next_event(sh, &mut ev), NHSHELL_E_DISCONNECTED);
            assert_eq!(nhshell_dispatch_main(sh), NHSHELL_E_DISCONNECTED);
            assert_eq!(nhshell_close(sh), 0);
        }
    }
}

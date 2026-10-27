//! T1 tests of xdg-activation against nh-recorder: a token from the environment, and a token the shell requests.
//! `NH_RECORDER` names the recorder binary; each test starts its own with a temporary `XDG_RUNTIME_DIR`.

use nhshell::api::{ShellBackend, ShellError, WindowDesc};
use nhshell::event::*;
use nhshell::wayland::WaylandShell;
use rustix::event::{PollFd, PollFlags, Timespec, poll};
use rustix::fd::BorrowedFd;
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
        let dir =
            std::env::temp_dir().join(format!("nhshell-activation-{}-{n}", std::process::id()));
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

    fn open(&self) -> WaylandShell {
        let display = CString::new(self.dir.join("wl").as_os_str().as_bytes()).unwrap();
        WaylandShell::open(Some(&display)).expect("open the shell on the recorder")
    }

    fn control(&self, req: &str) -> String {
        let mut s = UnixStream::connect(self.dir.join("control")).unwrap();
        writeln!(s, "{req}").unwrap();
        let mut line = String::new();
        BufReader::new(s).read_line(&mut line).unwrap();
        line
    }

    /// The `activation_log` reply once it contains `want`, within 3 s.
    fn activations_with(&self, shell: &mut WaylandShell, want: &str) -> String {
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            // the requested token arrives on the main queue, so the shell must run
            let _ = shell.next_event();
            let log = self.control(r#"{"cmd":"activation_log"}"#);
            if log.contains(want) {
                return log;
            }
            assert!(Instant::now() < deadline, "no {want} in {log}");
            readable_within(shell.wake_fd(), 20);
        }
    }
}

impl Drop for Recorder {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
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

/// The next record satisfying `want` within 3 s; other records are dropped.
fn wait_for(shell: &mut WaylandShell, want: impl Fn(&NhShellEvent) -> bool) -> NhShellEvent {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        while let Some(ev) = shell.next_event().unwrap() {
            if want(&ev) {
                return ev;
            }
        }
        assert!(Instant::now() < deadline, "the awaited record never came");
        readable_within(shell.wake_fd(), 50);
    }
}

/// A shell on a fresh recorder with one window that has its first configure and the keyboard focus.
fn setup() -> (Recorder, WaylandShell, u64) {
    let rec = Recorder::start("mutter");
    let mut shell = rec.open();
    let id = shell
        .window_new(&WindowDesc {
            w: 200,
            h: 100,
            title: "t".into(),
            app_id: "nidhogg.test".into(),
            flags: 0,
        })
        .unwrap();
    wait_for(&mut shell, |e| e.kind == NHSHELL_EV_CONFIGURE);
    assert!(
        rec.control(r#"{"cmd":"focus","surface":"toplevel"}"#)
            .contains("\"ok\":true")
    );
    (rec, shell, id)
}

/// Press a button and request a token for window `id`; the token the compositor issued.  (The recorder honours only
/// tokens it issued, so a token to hand in has to come from a request.)
fn issued_token(rec: &Recorder, shell: &mut WaylandShell, id: u64) -> String {
    rec.control(r#"{"cmd":"pointer","pos":[10,10]}"#);
    rec.control(r#"{"cmd":"button","button":272,"state":"press"}"#);
    wait_for(shell, |e| e.kind == NHSHELL_EV_BUTTON);
    shell.window_activate(id, None).unwrap();
    let log = rec.activations_with(shell, &format!(r#"{{"surface":{id},"token":""#));
    let at = log.find(r#""token":""#).unwrap() + r#""token":""#.len();
    log[at..at + log[at..].find('"').unwrap()].to_string()
}

#[allow(clippy::module_inception)]
mod activation {
    use super::*;

    #[test]
    fn token_from_env_is_used() {
        let (rec, mut shell, id) = setup();
        let token = issued_token(&rec, &mut shell, id);
        shell.window_activate(id, Some(&token)).unwrap();
        // the request activated once, the handed-in token activates again
        let entry = format!(r#"{{"surface":{id},"token":"{token}"}}"#);
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            let log = rec.activations_with(&mut shell, &entry);
            if log.matches(&entry).count() == 2 {
                break;
            }
            assert!(Instant::now() < deadline, "one activation only in {log}");
            readable_within(shell.wake_fd(), 20);
        }
    }

    #[test]
    fn requested_token_needs_a_serial() {
        let (rec, mut shell, id) = setup();
        assert_eq!(shell.window_activate(id, None), Err(ShellError::State));
        issued_token(&rec, &mut shell, id);
    }
}

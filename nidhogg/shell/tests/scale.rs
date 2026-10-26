//! T1 tests of outputs and scale against nh-recorder (see `reader.rs` for the setup).

use nhshell::api::ShellBackend;
use nhshell::event::{NHSHELL_EV_OUTPUT_ADD, NHSHELL_EV_SCALE, NhShellEvent};
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
        let dir = std::env::temp_dir().join(format!("nhshell-scale-{}-{n}", std::process::id()));
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

/// Dispatch until a record satisfying `want` arrives (returned), or fail after 5 s.
fn wait_for(shell: &mut WaylandShell, want: impl Fn(&NhShellEvent) -> bool) -> NhShellEvent {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        while let Some(ev) = shell.next_event().unwrap() {
            if want(&ev) {
                return ev;
            }
        }
        assert!(Instant::now() < deadline, "the awaited record never came");
        readable_within(shell.wake_fd(), 100);
    }
}

mod scale {
    use super::*;

    fn is_scale(id: u64, v: u32) -> impl Fn(&NhShellEvent) -> bool {
        // SAFETY: SCALE records carry `scale`.
        move |e| e.kind == NHSHELL_EV_SCALE && e.window == id && unsafe { e.u.scale.scale120 } == v
    }

    #[test]
    fn fractional_event_reaches_window() {
        let rec = Recorder::start("sway");
        let mut shell = rec.open();
        let id = shell.surface_new().unwrap();
        shell.scale_attach(id).unwrap();
        shell.flush().unwrap();
        assert_eq!(shell.window_scale(id).unwrap(), 120);
        assert!(
            rec.control(r#"{"cmd":"scale","value120":150}"#)
                .contains("\"ok\":true")
        );
        wait_for(&mut shell, is_scale(id, 150));
        assert_eq!(shell.window_scale(id).unwrap(), 150);
    }

    #[test]
    fn fallback_to_buffer_scale_without_fractional() {
        let rec = Recorder::start("no-fractional");
        let mut shell = rec.open();
        assert_eq!(shell.caps().fractional_scale, 0);
        let id = shell.surface_new().unwrap();
        shell.scale_attach(id).unwrap();
        shell.flush().unwrap();
        assert!(
            rec.control(r#"{"cmd":"scale","value120":240}"#)
                .contains("\"ok\":true")
        );
        wait_for(&mut shell, is_scale(id, 240));
        assert_eq!(shell.window_scale(id).unwrap(), 240);
    }

    #[test]
    fn outputs_listed_with_names() {
        let rec = Recorder::start("sway");
        let mut shell = rec.open();
        let ev = wait_for(&mut shell, |e| e.kind == NHSHELL_EV_OUTPUT_ADD);
        // SAFETY: OUTPUT_ADD records carry `output`.
        let o = unsafe { ev.u.output };
        let name = std::ffi::CStr::from_bytes_until_nul(&o.name).unwrap();
        assert_eq!(name.to_str().unwrap(), "nh-0");
        assert_eq!((o.w, o.h, o.scale120), (1920, 1080, 120));
    }
}

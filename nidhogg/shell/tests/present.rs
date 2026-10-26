//! T1 tests of the presenter against nh-recorder (manual clock): damage, viewport, frame callbacks, idle trim.
//! `NH_RECORDER` names the recorder binary; each test starts its own with a temporary `XDG_RUNTIME_DIR`.

use glitnir::geom::Rect;
use nhshell::api::{ShellBackend, WindowDesc};
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
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// `open` reads `NIDHOGG_SHELL_FORCE_CSD`; the test that sets it holds this lock, and every open takes it.
static ENV: Mutex<()> = Mutex::new(());

struct Recorder {
    child: Child,
    dir: PathBuf,
}

impl Recorder {
    fn start(profile: &str, clock: &str) -> Recorder {
        let bin = std::env::var_os("NH_RECORDER")
            .expect("NH_RECORDER is not set (the path of S0's nh-recorder binary)");
        static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!("nhshell-present-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let profile = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../test/recorder/profiles")
            .join(format!("{profile}.json"));
        let mut child = Command::new(bin)
            .args(["--socket", "wl", "--clock", clock, "--control"])
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
        let _env = ENV.lock().unwrap_or_else(|e| e.into_inner());
        self.open_locked()
    }

    fn open_locked(&self) -> WaylandShell {
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

/// The next record satisfying `pred` within 3 s; other records are dropped.
fn wait_for(shell: &mut WaylandShell, pred: impl Fn(&NhShellEvent) -> bool) -> NhShellEvent {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        while let Some(ev) = shell.next_event().unwrap() {
            if pred(&ev) {
                return ev;
            }
        }
        assert!(Instant::now() < deadline, "no matching record");
        readable_within(shell.wake_fd(), 50);
    }
}

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
        let rec = Recorder::start("sway", "manual");
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
        draw(&mut shell, id, w, h, &[Rect::new(5, 6, 100, 20)]);
        let log = wait_buffer_commits(&rec, 1);
        let c = log.first().expect("a commit with a buffer");
        assert!(c.contains("\"damage\":[[5,6,100,20]]"), "{c}");
        assert!(c.contains("\"dst\":[200,100]"), "{c}");
        assert!(c.contains("\"buffer_scale\":1"), "{c}");
    }

    #[test]
    fn no_commit_while_callback_pending() {
        let rec = Recorder::start("sway", "manual");
        let mut shell = rec.open();
        let id = window(&mut shell);
        assert_eq!(
            draw(&mut shell, id, 200, 100, &[Rect::new(0, 0, 10, 10)]),
            0
        );
        // the callback is pending: the second frame is held back and its damage kept
        draw(&mut shell, id, 200, 100, &[Rect::new(50, 50, 10, 10)]);
        wait_buffer_commits(&rec, 1);
        std::thread::sleep(Duration::from_millis(150));
        assert_eq!(buffer_commits(&rec).len(), 1);
        present(&rec);
        wait_for(&mut shell, |e| e.kind == NHSHELL_EV_FRAME_DONE);
        // the held frame's buffer comes back as the previous frame, and its damage joins the new one
        let age = draw(&mut shell, id, 200, 100, &[Rect::new(60, 60, 5, 5)]);
        assert_eq!(age, 1);
        let log = wait_buffer_commits(&rec, 2);
        assert_eq!(log.len(), 2);
        let c = &log[1];
        assert!(
            c.contains("[50,50,10,10]") && c.contains("[60,60,5,5]"),
            "{c}"
        );
    }

    #[test]
    fn frame_done_after_present() {
        let rec = Recorder::start("sway", "manual");
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
        let rec = Recorder::start("sway", "manual");
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
        let rec = Recorder::start("sway", "manual");
        let mut shell = rec.open();
        let id = window(&mut shell);
        draw(&mut shell, id, 201, 101, &[]);
        let (_, _, len) = shell.present_stats(id).unwrap();
        // SAFETY: sysconf has no preconditions.
        let page = unsafe { libc::sysconf(libc::_SC_PAGESIZE) } as usize;
        assert!(len > 0 && len % page == 0, "{len}");
    }
}

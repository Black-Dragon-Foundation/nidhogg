//! T1 tests of the pointer against nh-recorder: motion in device pixels, scroll frames, cursor shapes and press serials.
//! `NH_RECORDER` names the recorder binary; each test starts its own with a temporary `XDG_RUNTIME_DIR`.

use nhshell::api::{NHSHELL_CURSOR_TEXT, ShellBackend, WindowDesc};
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
    fn start(profile: &str) -> Recorder {
        let bin = std::env::var_os("NH_RECORDER")
            .expect("NH_RECORDER is not set (the path of S0's nh-recorder binary)");
        static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!("nhshell-pointer-{}-{n}", std::process::id()));
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

fn wait_event(shell: &mut WaylandShell, kind: u32) -> NhShellEvent {
    wait_for(shell, |e| e.kind == kind)
}

/// A shell on a fresh recorder with one window that has its first configure and the keyboard focus.
fn setup(profile: &str) -> (Recorder, WaylandShell, u64) {
    let rec = Recorder::start(profile);
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
    wait_event(&mut shell, NHSHELL_EV_CONFIGURE);
    assert!(
        rec.control(r#"{"cmd":"focus","surface":"toplevel"}"#)
            .contains("\"ok\":true")
    );
    (rec, shell, id)
}

fn pointer_ev(ev: &NhShellEvent) -> PointerEv {
    // SAFETY: pointer records carry `pointer`.
    unsafe { ev.u.pointer }
}

fn scroll_ev(ev: &NhShellEvent) -> ScrollEv {
    // SAFETY: SCROLL records carry `scroll`.
    unsafe { ev.u.scroll }
}

#[allow(clippy::module_inception)]
mod pointer {
    use super::*;

    #[test]
    fn motion_in_device_px() {
        let (rec, mut shell, id) = setup("mutter");
        rec.control(r#"{"cmd":"scale","value120":180}"#);
        wait_for(&mut shell, |e| {
            // SAFETY: SCALE records carry `scale`.
            e.kind == NHSHELL_EV_SCALE && unsafe { e.u.scale.scale120 } == 180
        });
        rec.control(r#"{"cmd":"pointer","pos":[100,50]}"#);
        let enter = wait_event(&mut shell, NHSHELL_EV_POINTER_ENTER);
        assert_eq!(enter.window, id);
        // the compositor reports the first position with the enter itself
        let e = pointer_ev(&enter);
        assert_eq!((e.x, e.y), (150.0, 75.0));
        rec.control(r#"{"cmd":"pointer","pos":[102,50]}"#);
        let m = pointer_ev(&wait_event(&mut shell, NHSHELL_EV_POINTER_MOTION));
        assert_eq!((m.x, m.y), (153.0, 75.0));
    }

    #[test]
    fn value120_with_seat_8() {
        let (rec, mut shell, _) = setup("mutter");
        rec.control(r#"{"cmd":"pointer","pos":[10,10]}"#);
        rec.control(r#"{"cmd":"axis","source":"wheel","v120":120}"#);
        let s = scroll_ev(&wait_event(&mut shell, NHSHELL_EV_SCROLL));
        assert_eq!((s.v120x, s.v120y), (0, 120));
        assert_eq!(s.dy, 15.0);
        assert_eq!((s.source, s.stop), (NHSHELL_SCROLL_WHEEL, 0));
    }

    #[test]
    fn stop_sets_flag() {
        let (rec, mut shell, _) = setup("mutter");
        rec.control(r#"{"cmd":"pointer","pos":[10,10]}"#);
        rec.control(r#"{"cmd":"axis","source":"finger","stop":true}"#);
        let s = scroll_ev(&wait_event(&mut shell, NHSHELL_EV_SCROLL));
        assert_eq!((s.source, s.stop), (NHSHELL_SCROLL_FINGER, 1));
    }

    #[test]
    fn cursor_shape_after_enter() {
        let (rec, mut shell, id) = setup("mutter");
        assert!(shell.caps().cursor_shape > 0);
        rec.control(r#"{"cmd":"pointer","pos":[10,10]}"#);
        wait_event(&mut shell, NHSHELL_EV_POINTER_ENTER);
        shell.set_cursor(id, NHSHELL_CURSOR_TEXT).unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            let log = rec.control(r#"{"cmd":"input_log","since":1}"#);
            if log.contains(r#""req":"set_shape""#) && log.contains(r#""shape":"text""#) {
                break;
            }
            assert!(Instant::now() < deadline, "no set_shape in {log}");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// Task 5's open point: move, resize and menus carry the serial of the latest press.
    #[test]
    fn move_carries_press_serial() {
        let (rec, mut shell, id) = setup("mutter");
        rec.control(r#"{"cmd":"pointer","pos":[10,10]}"#);
        let mut seen = Vec::new();
        for _ in 0..2 {
            rec.control(r#"{"cmd":"button","button":272,"state":"press"}"#);
            wait_for(&mut shell, |e| {
                e.kind == NHSHELL_EV_BUTTON && pointer_ev(e).state == NHSHELL_BUTTON_PRESSED
            });
            let press = shell.latest_press_serial();
            assert_ne!(press, 0);
            shell.move_window(id).unwrap();
            let deadline = Instant::now() + Duration::from_secs(3);
            let moved = loop {
                let log = rec.control(r#"{"cmd":"input_log","since":1}"#);
                if let Some(at) = log.rfind(r#""req":"xdg_toplevel.move""#)
                    && let Some(s) = number(&log[at..], "serial")
                    && s == u64::from(press)
                {
                    break s;
                }
                assert!(Instant::now() < deadline, "no move with serial {press}");
                std::thread::sleep(Duration::from_millis(10));
            };
            seen.push(moved);
            rec.control(r#"{"cmd":"button","button":272,"state":"release"}"#);
        }
        assert_ne!(seen[0], seen[1], "each press has its own serial");
    }
}

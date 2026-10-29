//! The T1 tests' harness: each test starts its own nh-recorder (`NH_RECORDER`, a built `nidhogg/test/recorder`) with a
//! temporary `XDG_RUNTIME_DIR` and connects by absolute socket path, never to the environment's display.
#![allow(dead_code)]

pub mod backing;

use nhshell::api::ShellBackend;
use nhshell::event::{ConfigureEv, Kind, OutputEv, PointerEv, Record, ShellEvent};
use nhshell::wayland::WaylandShell;
use rustix::event::{PollFd, PollFlags, Timespec, poll};
use rustix::fd::BorrowedFd;
use rustix::process::{Pid, Signal, kill_process};
use std::ffi::CString;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// `open` reads `NIDHOGG_SHELL_FORCE_CSD`; a test that sets it holds this lock, and every open takes it.
pub static ENV: Mutex<()> = Mutex::new(());

pub struct Recorder {
    child: Child,
    pub dir: PathBuf,
}

impl Recorder {
    /// A recorder imitating `profile` (`nidhogg/test/recorder/profiles/PROFILE.json`) on the real-time clock.
    pub fn start(profile: &str) -> Recorder {
        Recorder::start_clock(profile, "auto")
    }

    /// The same on `clock`: `auto`, or `manual` (frames only on the `present` command).
    pub fn start_clock(profile: &str, clock: &str) -> Recorder {
        let bin = std::env::var_os("NH_RECORDER")
            .expect("NH_RECORDER is not set (the path of a built nh-recorder)");
        static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!("nhshell-t1-{}-{n}", std::process::id()));
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

    pub fn display(&self) -> CString {
        CString::new(self.dir.join("wl").as_os_str().as_bytes()).unwrap()
    }

    pub fn open(&self) -> WaylandShell {
        let _env = ENV.lock().unwrap_or_else(|e| e.into_inner());
        self.open_locked()
    }

    /// `open` for a test that holds `ENV` already.
    pub fn open_locked(&self) -> WaylandShell {
        WaylandShell::open(Some(&self.display())).expect("open the shell on the recorder")
    }

    /// One control command; the reply line.
    pub fn control(&self, req: &str) -> String {
        let mut s = UnixStream::connect(self.dir.join("control")).unwrap();
        writeln!(s, "{req}").unwrap();
        let mut line = String::new();
        BufReader::new(s).read_line(&mut line).unwrap();
        line
    }

    /// Retry a control command until it succeeds (a request the shell just flushed may not be processed yet).
    pub fn control_ok(&self, req: &str) -> String {
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let reply = self.control(req);
            if reply.contains("\"ok\":true") || Instant::now() > deadline {
                return reply;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// The output as the compositor shows it now: (width, its BGRA pixels, 4 bytes each, rows packed).
    pub fn snapshot(&self) -> (usize, Vec<u8>) {
        let path = self.dir.join("snap.bgra");
        let r = self.control(&format!(
            r#"{{"cmd":"snapshot","path":"{}","format":"bgra"}}"#,
            path.display()
        ));
        let w = number(&r, "w").unwrap_or_else(|| panic!("snapshot: {r}")) as usize;
        (w, std::fs::read(&path).unwrap())
    }

    /// The pixel at (`x`, `y`) of a snapshot, as premultiplied ARGB.
    pub fn snapshot_pixel(snap: &(usize, Vec<u8>), x: usize, y: usize) -> u32 {
        let at = (y * snap.0 + x) * 4;
        u32::from_le_bytes(snap.1[at..at + 4].try_into().unwrap())
    }

    /// Surface `id`'s entry in the `tree` reply once `pred` holds for it, within 3 s.  The reply's keys are sorted, so
    /// each surface's object opens with `{"above":`.
    pub fn wait_node(&self, id: u64, pred: impl Fn(&str) -> bool) -> String {
        let key = format!("\"id\":{id},");
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            let r = self.control(r#"{"cmd":"tree"}"#);
            let node = r
                .split("{\"above\":")
                .skip(1)
                .find(|n| n.contains(&key))
                .map(|n| format!("{{\"above\":{n}"));
            if let Some(n) = &node
                && pred(n)
            {
                return n.clone();
            }
            assert!(
                Instant::now() < deadline,
                "surface {id} never matched; last: {node:?}"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// The number of surfaces of `role` (`toplevel`, `popup`, `subsurface`) in the `tree` reply.
    pub fn surfaces(&self, role: &str) -> usize {
        self.control(r#"{"cmd":"tree"}"#)
            .matches(&format!("\"role\":\"{role}\""))
            .count()
    }

    pub fn signal(&self, sig: Signal) {
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
pub fn number(json: &str, key: &str) -> Option<u64> {
    let pat = format!("\"{key}\":");
    let rest = json[json.find(&pat)? + pat.len()..].trim_start();
    let end = rest
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(rest.len());
    rest[..end].parse().ok()
}

pub fn readable_within(fd: i32, ms: i64) -> bool {
    // SAFETY: the shell's wake descriptor stays open while the shell lives.
    let fd = unsafe { BorrowedFd::borrow_raw(fd) };
    let mut fds = [PollFd::new(&fd, PollFlags::IN)];
    let t = Timespec {
        tv_sec: ms / 1000,
        tv_nsec: (ms % 1000) * 1_000_000,
    };
    matches!(poll(&mut fds, Some(&t)), Ok(n) if n > 0)
}

/// The next record satisfying `want` within 5 s; other records are dropped.
pub fn wait_for(shell: &mut WaylandShell, want: impl Fn(&Record) -> bool) -> Record {
    let deadline = Instant::now() + Duration::from_secs(5);
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

pub fn wait_event(shell: &mut WaylandShell, kind: Kind) -> Record {
    wait_for(shell, |e| e.kind() == kind)
}

/// A `CONFIGURE` record's payload.
pub fn configure_of(ev: &Record) -> ConfigureEv {
    match ev.event {
        ShellEvent::Configure(c) => c,
        e => panic!("not a configure: {e:?}"),
    }
}

/// An output record's payload (add, change, remove, enter, leave).
pub fn output_of(ev: &Record) -> OutputEv {
    match ev.event {
        ShellEvent::OutputAdd(o)
        | ShellEvent::OutputChange(o)
        | ShellEvent::OutputRemove(o)
        | ShellEvent::EnterOutput(o)
        | ShellEvent::LeaveOutput(o) => o,
        e => panic!("not an output record: {e:?}"),
    }
}

/// A pointer record's payload (enter, leave, motion, button).
pub fn pointer_of(ev: &Record) -> PointerEv {
    match ev.event {
        ShellEvent::PointerEnter(p)
        | ShellEvent::PointerLeave(p)
        | ShellEvent::PointerMotion(p)
        | ShellEvent::Button(p) => p,
        e => panic!("not a pointer record: {e:?}"),
    }
}

/// A top-level window of `w` x `h` logical pixels, mapped: its first configure acked with the buffer that carries it.
pub fn mapped_window(shell: &mut WaylandShell, w: i32, h: i32) -> u64 {
    use nhshell::api::{NHSHELL_ACK_CONTENT, WindowDesc};
    let id = shell
        .window_new(&WindowDesc {
            w,
            h,
            title: "t".into(),
            app_id: "nidhogg.test".into(),
            flags: 0,
        })
        .unwrap();
    let ev = wait_event(shell, Kind::Configure);
    let serial = configure_of(&ev).serial;
    shell
        .ack_configure(id, serial, NHSHELL_ACK_CONTENT)
        .unwrap();
    draw(shell, id, w, h);
    id
}

/// Present a buffer of `w` x `h` device pixels on window `id`, all of it damaged.
pub fn draw(shell: &mut WaylandShell, id: u64, w: i32, h: i32) {
    let b = shell.present_begin(id, w, h).unwrap();
    // SAFETY: the buffer is `stride * h` writable bytes.
    unsafe { std::ptr::write_bytes(b.pixels, 0x80, (b.stride * b.h) as usize) };
    shell
        .present_commit(id, &[glitnir::geom::Rect::new(0, 0, w, h)])
        .unwrap();
    shell.flush().unwrap();
}

/// Every record that arrives within `ms` of quiet.
pub fn collect(shell: &mut WaylandShell, ms: i64) -> Vec<Record> {
    let mut out = Vec::new();
    loop {
        while let Some(ev) = shell.next_event().unwrap() {
            out.push(ev);
        }
        if !readable_within(shell.wake_fd(), ms) {
            return out;
        }
    }
}

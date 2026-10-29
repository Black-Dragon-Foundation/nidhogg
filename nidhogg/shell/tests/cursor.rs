//! T1 tests of the cursor against nh-recorder: the Xcursor theme without `wp_cursor_shape_manager_v1`, and the hidden
//! pointer.  `NH_RECORDER` names the recorder binary; each test starts its own with a temporary `XDG_RUNTIME_DIR`.

use nhshell::api::{NHSHELL_CURSOR_HIDDEN, NHSHELL_CURSOR_TEXT, ShellBackend, WindowDesc};
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
use std::sync::Once;
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
        let dir = std::env::temp_dir().join(format!("nhshell-cursor-{}-{n}", std::process::id()));
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

/// The next record of `kind` within 3 s; other records are dropped.
fn wait_event(shell: &mut WaylandShell, kind: Kind) -> Record {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        while let Some(ev) = shell.next_event().unwrap() {
            if ev.kind() == kind {
                return ev;
            }
        }
        assert!(Instant::now() < deadline, "the awaited record never came");
        readable_within(shell.wake_fd(), 50);
    }
}

/// A shell on a fresh recorder with one window that has its first configure, the keyboard focus and the pointer.
fn setup(profile: &str) -> (Recorder, WaylandShell, u64) {
    theme_dir();
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
    wait_event(&mut shell, Kind::Configure);
    assert!(
        rec.control(r#"{"cmd":"focus","surface":"toplevel"}"#)
            .contains("\"ok\":true")
    );
    rec.control(r#"{"cmd":"pointer","pos":[10,10]}"#);
    wait_event(&mut shell, Kind::PointerEnter);
    (rec, shell, id)
}

/// One xcursor image: a solid square of `size` pixels with the hotspot in its middle.
fn xcursor_image(size: u32) -> Vec<u8> {
    let mut b = Vec::new();
    for v in [36, 0xfffd_0002, size, 1, size, size, size / 2, size / 2, 0] {
        b.extend(v.to_le_bytes());
    }
    b.extend((0..size * size).flat_map(|_| 0xff80_8080u32.to_le_bytes()));
    b
}

/// A theme `nhtest` with `left_ptr` and `xterm` at 24 and 48 pixels, and `XCURSOR_PATH` pointing at it.  Set once,
/// before any test starts a thread of its own.
fn theme_dir() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let root =
            std::env::temp_dir().join(format!("nhshell-cursor-theme-{}", std::process::id()));
        let cursors = root.join("nhtest/cursors");
        std::fs::create_dir_all(&cursors).unwrap();
        let images = [xcursor_image(24), xcursor_image(48)];
        let mut file = b"Xcur".to_vec();
        for v in [16u32, 0x10000, 2] {
            file.extend(v.to_le_bytes());
        }
        let first = 16 + 2 * 12;
        let mut pos = first;
        for (size, image) in [24u32, 48].iter().zip(&images) {
            for v in [0xfffd_0002u32, *size, pos as u32] {
                file.extend(v.to_le_bytes());
            }
            pos += image.len();
        }
        for (size, image) in [24u32, 48].iter().zip(&images) {
            // the chunk header carries the nominal size as its subtype
            let mut chunk = image.clone();
            chunk[8..12].copy_from_slice(&size.to_le_bytes());
            file.extend(chunk);
        }
        for name in ["left_ptr", "xterm"] {
            std::fs::write(cursors.join(name), &file).unwrap();
        }
        // SAFETY: `Once` runs this before any test touches the environment or starts a thread.
        unsafe { std::env::set_var("XCURSOR_PATH", &root) };
    });
}

/// The (buffer height, buffer scale) of every commit on a cursor surface, oldest first.
fn cursor_commits(rec: &Recorder) -> Vec<(u64, u64)> {
    let log = rec.control(r#"{"cmd":"log","since":1}"#);
    log.split("{\"ack_serial\":")
        .skip(1)
        .filter(|c| c.contains("\"role\":\"cursor\"") && c.contains("\"buffer\":{"))
        .map(|c| {
            let buffer = &c[c.find("\"buffer\":{").unwrap()..];
            (
                number(buffer, "h").unwrap(),
                number(c, "buffer_scale").unwrap(),
            )
        })
        .collect()
}

/// Poll `want` on the recorder's replies for 3 s; the last reply when it never holds.
fn wait_reply(rec: &Recorder, req: &str, want: impl Fn(&str) -> bool) -> String {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        let r = rec.control(req);
        if want(&r) || Instant::now() > deadline {
            return r;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[allow(clippy::module_inception)]
mod cursor {
    use super::*;

    #[test]
    fn theme_fallback_without_cursor_shape() {
        let (rec, mut shell, id) = setup("no-cursor-shape");
        assert_eq!(shell.caps().cursor_shape, 0);
        shell.set_cursor_theme("nhtest", 24).unwrap();
        shell.set_cursor(id, NHSHELL_CURSOR_TEXT).unwrap();
        wait_reply(&rec, r#"{"cmd":"log","since":1}"#, |_| {
            !cursor_commits(&rec).is_empty()
        });
        // the theme size times the scale (1): a 24 px buffer at scale 1
        assert_eq!(
            cursor_commits(&rec),
            [(24, 1)],
            "{}",
            rec.control(r#"{"cmd":"log","since":1}"#)
        );
        let log = rec.control(r#"{"cmd":"input_log","since":1}"#);
        assert!(
            log.contains(r#""req":"set_cursor""#) && !log.contains(r#""surface":null"#),
            "no cursor surface in {log}"
        );

        // a scale change loads the theme again at the new size
        rec.control(r#"{"cmd":"scale","value120":240}"#);
        wait_event(&mut shell, Kind::Scale);
        shell.set_cursor(id, NHSHELL_CURSOR_TEXT).unwrap();
        wait_reply(&rec, r#"{"cmd":"log","since":1}"#, |_| {
            cursor_commits(&rec).len() > 1
        });
        assert_eq!(cursor_commits(&rec), [(24, 1), (48, 2)]);
    }

    #[test]
    fn hidden_pointer_has_no_cursor_surface() {
        let (rec, mut shell, id) = setup("mutter");
        shell.set_cursor(id, NHSHELL_CURSOR_HIDDEN).unwrap();
        let log = wait_reply(&rec, r#"{"cmd":"input_log","since":1}"#, |l| {
            l.contains(r#""surface":null"#)
        });
        assert!(
            log.contains(r#""req":"set_cursor""#) && log.contains(r#""surface":null"#),
            "no hidden cursor in {log}"
        );
        shell.set_cursor(id, NHSHELL_CURSOR_TEXT).unwrap();
        let log = wait_reply(&rec, r#"{"cmd":"input_log","since":1}"#, |l| {
            l.contains(r#""shape":"text""#)
        });
        assert!(log.contains(r#""shape":"text""#), "no shape in {log}");
    }
}

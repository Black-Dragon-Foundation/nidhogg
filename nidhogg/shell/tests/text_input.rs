//! T1 tests of text-input-v3 against nh-recorder (`NH_RECORDER`; it needs the `delete` member of its `text_input`
//! command): enable and the cursor rectangle on enter, preedit and commit in the protocol's order, surrounding text
//! cut to 4000 bytes, and the preedit cleared when focus is lost.  Each test starts its own recorder with a temporary
//! `XDG_RUNTIME_DIR`.  Windows are bare surfaces, as in the keyboard tests: their id is the surface's protocol id.

use nhshell::api::*;
use nhshell::event::*;
use nhshell::wayland::WaylandShell;
use nhshell::wayland::text_input::{SURROUNDING_MAX, truncate_surrounding};
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
    fn start() -> Recorder {
        let bin = std::env::var_os("NH_RECORDER")
            .expect("NH_RECORDER is not set (the path of S0's nh-recorder binary)");
        static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let dir =
            std::env::temp_dir().join(format!("nhshell-textinput-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let profile =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../test/recorder/profiles/sway.json");
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

    /// Send `req`; a command that fails (the text input is not enabled yet) is retried for 2 s.
    fn control(&self, req: &str) -> String {
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let mut s = UnixStream::connect(self.dir.join("control")).unwrap();
            writeln!(s, "{req}").unwrap();
            let mut line = String::new();
            BufReader::new(s).read_line(&mut line).unwrap();
            if line.contains("\"ok\":true") || Instant::now() > deadline {
                return line;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn ok(&self, req: &str) {
        let reply = self.control(req);
        assert!(reply.contains("\"ok\":true"), "{req}: {reply}");
    }

    fn log(&self) -> String {
        self.control(r#"{"cmd":"input_log","since":0}"#)
    }

    /// Wait until the input log holds `n` entries of request `req`; returns the log.
    fn wait_log(&self, req: &str, n: usize) -> String {
        let needle = format!("\"req\":\"{req}\"");
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            let log = self.log();
            if log.matches(&needle).count() >= n || Instant::now() > deadline {
                assert!(log.matches(&needle).count() >= n, "{req} x{n}: {log}");
                return log;
            }
            std::thread::sleep(Duration::from_millis(10));
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

/// What a text-input record came to, with its text fetched.
#[derive(Debug, PartialEq)]
enum Rec {
    Preedit(String, i32, i32),
    Commit(String),
    Delete(u32, u32),
}

/// The next `n` text-input records, with their texts taken.
fn records(shell: &mut WaylandShell, n: usize) -> Vec<Rec> {
    let mut out = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(3);
    while out.len() < n && Instant::now() < deadline {
        readable_within(shell.wake_fd(), 100);
        while let Some(ev) = shell.next_event().unwrap() {
            let mut text = |id: u32, len: u32| {
                if id == 0 {
                    assert_eq!(len, 0, "a record without text id has len 0");
                    return String::new();
                }
                assert_eq!(shell.text_len(id).unwrap(), len as usize);
                String::from_utf8(shell.take_text(id).unwrap()).unwrap()
            };
            match ev.event {
                ShellEvent::Preedit(t) => {
                    let s = text(t.id, t.len);
                    out.push(Rec::Preedit(s, t.cursor_begin, t.cursor_end));
                }
                ShellEvent::CommitText(t) => out.push(Rec::Commit(text(t.id, t.len))),
                ShellEvent::DeleteSurrounding(t) => out.push(Rec::Delete(t.before, t.after)),
                _ => {}
            }
        }
    }
    out
}

/// A shell with a surface that has text input enabled and keyboard focus; the recorder has seen the enable.
fn focused() -> (Recorder, WaylandShell, u64) {
    let rec = Recorder::start();
    let mut shell = rec.open();
    let surface = shell.surface_new().unwrap();
    shell.flush().unwrap();
    shell.text_input_enable(surface, true).unwrap();
    rec.ok(&format!(r#"{{"cmd":"focus","surface":{surface}}}"#));
    rec.wait_log("text_input.commit", 1);
    (rec, shell, surface)
}

#[allow(clippy::module_inception)]
mod text_input {
    use super::*;

    #[test]
    fn enable_on_enter_sends_enable_and_commit() {
        let rec = Recorder::start();
        let mut shell = rec.open();
        let surface = shell.surface_new().unwrap();
        shell.flush().unwrap();
        // asked for before the window has text-input focus: nothing goes out yet
        shell.text_input_enable(surface, true).unwrap();
        shell
            .text_input_cursor(surface, 10, 20, 3, 4)
            .expect("cursor");
        assert!(!rec.log().contains("text_input.enable"));
        rec.ok(&format!(r#"{{"cmd":"focus","surface":{surface}}}"#));
        let log = rec.wait_log("text_input.commit", 1);
        let pos = |needle: &str| {
            log.find(needle)
                .unwrap_or_else(|| panic!("{needle} in {log}"))
        };
        assert!(pos("text_input.enable") < pos("text_input.set_cursor_rectangle"));
        assert!(pos("text_input.set_cursor_rectangle") < pos("text_input.commit"));
        assert!(log.contains(r#""rect":[10,20,3,4]"#), "{log}");
    }

    #[test]
    fn preedit_then_commit_in_done_order() {
        let (rec, mut shell, _) = focused();
        rec.ok(r#"{"cmd":"text_input","preedit":"ab","cursor":[0,1]}"#);
        assert_eq!(records(&mut shell, 1), [Rec::Preedit("ab".into(), 0, 1)]);
        // one `done` with a commit and a new preedit: the old preedit goes, then the commit, then the new preedit
        rec.ok(r#"{"cmd":"text_input","preedit":"cd","cursor":[2,2],"commit":"y"}"#);
        assert_eq!(
            records(&mut shell, 3),
            [
                Rec::Preedit(String::new(), 0, 0),
                Rec::Commit("y".into()),
                Rec::Preedit("cd".into(), 2, 2),
            ]
        );
        // a commit alone ends the preedit
        rec.ok(r#"{"cmd":"text_input","commit":"é"}"#);
        assert_eq!(
            records(&mut shell, 2),
            [Rec::Preedit(String::new(), 0, 0), Rec::Commit("é".into())]
        );
    }

    #[test]
    fn cursor_rectangle_deduplicated() {
        let (rec, mut shell, surface) = focused();
        shell.text_input_cursor(surface, 10, 20, 3, 4).unwrap();
        shell.text_input_cursor(surface, 10, 20, 3, 4).unwrap();
        // a different rectangle still goes out, and nothing came between the first and it
        shell.text_input_cursor(surface, 5, 6, 7, 8).unwrap();
        let log = rec.wait_log("text_input.set_cursor_rectangle", 2);
        assert_eq!(
            log.matches("text_input.set_cursor_rectangle").count(),
            2,
            "{log}"
        );
        let a = log.find(r#""rect":[10,20,3,4]"#).expect("first");
        let b = log.find(r#""rect":[5,6,7,8]"#).expect("second");
        assert!(a < b);
    }

    #[test]
    fn surrounding_text_truncated_on_a_char_boundary() {
        let (rec, mut shell, surface) = focused();
        let text = "é".repeat(2500);
        assert_eq!(text.len(), 5000);
        shell
            .text_input_surrounding(surface, &text, 2500, 2500)
            .unwrap();
        let log = rec.wait_log("text_input.set_surrounding_text", 1);
        let from = log.find(r#""text":""#).expect("text") + 8;
        let sent = &log[from..from + log[from..].find('"').unwrap()];
        assert!(sent.len() <= SURROUNDING_MAX, "{} bytes", sent.len());
        assert!(sent.len() >= SURROUNDING_MAX - 1);
        assert!(sent.chars().all(|c| c == 'é'));
        // the pure function: cut around the cursor, offsets moved into the cut text
        let (cut, cursor, anchor) = truncate_surrounding(&text, 2500, 2400);
        assert_eq!((cursor as usize, anchor as usize), (2000, 1900));
        assert_eq!(cut.len(), SURROUNDING_MAX);
        let (cut, cursor, _) = truncate_surrounding(&text, 5000, 5000);
        assert_eq!((cut.len(), cursor as usize), (SURROUNDING_MAX, 4000));
    }

    #[test]
    fn leave_clears_preedit() {
        let (rec, mut shell, surface) = focused();
        rec.ok(r#"{"cmd":"text_input","preedit":"ab","cursor":[2,2]}"#);
        assert_eq!(records(&mut shell, 1), [Rec::Preedit("ab".into(), 2, 2)]);
        rec.ok(r#"{"cmd":"focus","surface":null}"#);
        assert_eq!(records(&mut shell, 1), [Rec::Preedit(String::new(), 0, 0)]);
        // the enabled state survives: focus again and the compositor hears `enable` a second time
        rec.ok(&format!(r#"{{"cmd":"focus","surface":{surface}}}"#));
        rec.wait_log("text_input.enable", 2);
        rec.wait_log("text_input.commit", 2);
    }

    #[test]
    fn delete_surrounding_reported() {
        let (rec, mut shell, _) = focused();
        rec.ok(r#"{"cmd":"text_input","delete":[2,1]}"#);
        assert_eq!(records(&mut shell, 1), [Rec::Delete(2, 1)]);
    }

    #[test]
    fn text_ids_are_single_use() {
        let (rec, mut shell, _) = focused();
        rec.ok(r#"{"cmd":"text_input","commit":"x"}"#);
        let mut id = 0;
        let deadline = Instant::now() + Duration::from_secs(3);
        while id == 0 && Instant::now() < deadline {
            readable_within(shell.wake_fd(), 100);
            while let Some(ev) = shell.next_event().unwrap() {
                if let ShellEvent::CommitText(t) = ev.event {
                    id = t.id;
                }
            }
        }
        assert_ne!(id, 0);
        assert_eq!(shell.take_text(id).unwrap(), b"x");
        assert_eq!(shell.take_text(id), Err(ShellError::State));
    }
}

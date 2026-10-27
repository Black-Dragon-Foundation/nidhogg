//! T1 tests of child (subsurface) and popup windows against nh-recorder: position, stacking, input region, mapping,
//! anchor placement and dismissal.  `NH_RECORDER` names the recorder binary; each test starts its own with a
//! temporary `XDG_RUNTIME_DIR`.  The recorder's `tree` reply has sorted keys, so each surface's object opens with
//! `{"above":`.

use glitnir::geom::Rect;
use nhshell::api::*;
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
    fn start() -> Recorder {
        Recorder::start_profile("sway")
    }

    fn start_profile(name: &str) -> Recorder {
        let bin = std::env::var_os("NH_RECORDER")
            .expect("NH_RECORDER is not set (the path of S0's nh-recorder binary)");
        static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!("nhshell-child-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let profile = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../test/recorder/profiles")
            .join(format!("{name}.json"));
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

    /// The `tree` reply split into one text per surface.
    fn tree(&self) -> Vec<String> {
        let r = self.control(r#"{"cmd":"tree"}"#);
        r.split("{\"above\":")
            .skip(1)
            .map(|c| format!("{{\"above\":{c}"))
            .collect()
    }

    /// Surface `id`'s tree entry once `pred` holds for it, within 3 s.
    fn wait_node(&self, id: u64, pred: impl Fn(&str) -> bool) -> String {
        let key = format!("\"id\":{id},");
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            let node = self.tree().into_iter().find(|n| n.contains(&key));
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

    fn wait_surfaces(&self, n: usize) {
        let deadline = Instant::now() + Duration::from_secs(3);
        while self.tree().len() != n {
            assert!(Instant::now() < deadline, "never {n} surfaces");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// How many commits of surface `id` the recorder logged that damaged it (a new frame; a bare commit has no damage).
    fn frames_of(&self, id: u64) -> usize {
        let log = self.control(r#"{"cmd":"log","since":1}"#);
        log.split("{\"ack_serial\":")
            .filter(|c| c.contains(&format!("\"surface\":{id},")) && !c.contains("\"damage\":[]"))
            .count()
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

/// A window of 200x100 logical pixels that is mapped: its first configure is acked, with the buffer that carries it.
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
        .ack_configure(id, unsafe { ev.u.configure }.serial, NHSHELL_ACK_CONTENT)
        .unwrap();
    draw(shell, id, 200, 100);
    id
}

fn child_desc(kind: u32, parent: u64, x: i32, y: i32, w: i32, h: i32) -> ChildDesc {
    ChildDesc {
        kind,
        parent,
        x,
        y,
        w,
        h,
        anchor: Anchor::default(),
    }
}

fn child(shell: &mut WaylandShell, parent: u64, x: i32, y: i32) -> u64 {
    shell
        .child_new(&child_desc(NHSHELL_KIND_CHILD, parent, x, y, 100, 60))
        .unwrap()
}

fn popup(shell: &mut WaylandShell, parent: u64, anchor: Anchor) -> u64 {
    let mut d = child_desc(NHSHELL_KIND_POPUP, parent, 0, 0, 60, 30);
    d.anchor = anchor;
    shell.child_new(&d).unwrap()
}

fn draw(shell: &mut WaylandShell, id: u64, w: i32, h: i32) {
    let b = shell.present_begin(id, w, h).unwrap();
    // SAFETY: the buffer is `stride * h` writable bytes.
    unsafe { std::ptr::write_bytes(b.pixels, 0x80, (b.stride * b.h) as usize) };
    shell.present_commit(id, &[Rect::new(0, 0, w, h)]).unwrap();
    shell.flush().unwrap();
}

/// The parent's commit, which applies its children's position and stacking.
fn commit(shell: &mut WaylandShell, id: u64) {
    shell.surface_commit(id).unwrap();
    shell.flush().unwrap();
}

#[allow(clippy::module_inception)]
mod child {
    use super::*;

    #[test]
    fn child_is_a_subsurface_at_its_position() {
        let rec = Recorder::start();
        let mut shell = rec.open();
        let parent = window(&mut shell);
        let c = child(&mut shell, parent, 40, 30);
        draw(&mut shell, c, 100, 60);
        commit(&mut shell, parent);
        let n = rec.wait_node(c, |n| n.contains("\"position\":[40,30]"));
        assert!(n.contains("\"role\":\"subsurface\""), "{n}");
        assert!(n.contains(&format!("\"parent\":{parent},")), "{n}");
    }

    #[test]
    fn child_moves_without_a_new_buffer() {
        let rec = Recorder::start();
        let mut shell = rec.open();
        let parent = window(&mut shell);
        let c = child(&mut shell, parent, 40, 30);
        draw(&mut shell, c, 100, 60);
        commit(&mut shell, parent);
        rec.wait_node(c, |n| n.contains("\"position\":[40,30]"));
        let before = rec.frames_of(c);
        assert_eq!(before, 1);
        shell.set_position(c, 70, 10).unwrap();
        commit(&mut shell, parent);
        rec.wait_node(c, |n| n.contains("\"position\":[70,10]"));
        assert_eq!(rec.frames_of(c), before, "a move is not a new frame");
    }

    #[test]
    fn stacking_follows_raise_and_lower() {
        let rec = Recorder::start();
        let mut shell = rec.open();
        let parent = window(&mut shell);
        let a = child(&mut shell, parent, 0, 0);
        let b = child(&mut shell, parent, 10, 10);
        commit(&mut shell, parent);
        let order = |first: u64, second: u64| format!("\"children\":[{first},{second}]");
        rec.wait_node(parent, |n| n.contains(&order(a, b)));
        shell.stack(a, b, true).unwrap();
        commit(&mut shell, parent);
        rec.wait_node(parent, |n| n.contains(&order(b, a)));
        shell.stack(a, b, false).unwrap();
        commit(&mut shell, parent);
        rec.wait_node(parent, |n| n.contains(&order(a, b)));
        // sibling 0 is the parent: below it is the bottom
        shell.stack(b, 0, false).unwrap();
        commit(&mut shell, parent);
        rec.wait_node(parent, |n| n.contains(&order(b, a)));
    }

    #[test]
    fn input_region_excludes_shadow() {
        let rec = Recorder::start();
        let mut shell = rec.open();
        let parent = window(&mut shell);
        let c = child(&mut shell, parent, 0, 0);
        shell
            .set_input_region(c, &[Rect::new(10, 10, 80, 40)])
            .unwrap();
        shell.flush().unwrap();
        rec.wait_node(c, |n| n.contains("\"input_region\":[[10,10,80,40]]"));
        // none at all: the whole surface again
        shell.set_input_region(c, &[]).unwrap();
        shell.flush().unwrap();
        rec.wait_node(c, |n| n.contains("\"input_region\":null"));
    }

    #[test]
    fn unmapped_child_has_no_buffer() {
        let rec = Recorder::start();
        let mut shell = rec.open();
        let parent = window(&mut shell);
        let c = child(&mut shell, parent, 0, 0);
        draw(&mut shell, c, 100, 60);
        rec.wait_node(c, |n| n.contains("\"mapped\":true"));
        shell.set_mapped(c, false).unwrap();
        shell.flush().unwrap();
        let n = rec.wait_node(c, |n| n.contains("\"mapped\":false"));
        assert!(n.contains("\"buffer\":null"), "{n}");
    }

    #[test]
    fn popup_is_placed_by_its_anchor() {
        let rec = Recorder::start();
        let mut shell = rec.open();
        let parent = window(&mut shell);
        let anchor = Anchor {
            x: 100,
            y: 50,
            w: 10,
            h: 10,
            gravity: 8,
            adjust: 0,
        };
        let p = popup(&mut shell, parent, anchor);
        // the first configure is acked with the popup's first buffer
        wait_for(&mut shell, |e| {
            e.kind == NHSHELL_EV_FRAME_READY && e.window == p
        });
        draw(&mut shell, p, 60, 30);
        // below and right of the rectangle: its bottom-right corner is the popup's top-left
        let n = rec.wait_node(p, |n| n.contains("\"position\":[110,60]"));
        assert!(n.contains("\"role\":\"popup\""), "{n}");
        shell.set_position(p, 20, 25).unwrap();
        wait_for(&mut shell, |e| {
            e.kind == NHSHELL_EV_FRAME_READY && e.window == p
        });
        shell.present_bare(p).unwrap();
        rec.wait_node(p, |n| n.contains("\"position\":[30,35]"));
    }

    /// The tip bug: a reposition takes effect when the client acks and commits.  The ack makes the popup ready
    /// (`OWED`), and a bare commit carries it.
    #[test]
    fn popup_move_applies_with_the_acks_commit() {
        let rec = Recorder::start();
        let mut shell = rec.open();
        let parent = window(&mut shell);
        let anchor = Anchor {
            x: 100,
            y: 50,
            w: 10,
            h: 10,
            gravity: 8,
            adjust: 0,
        };
        let p = popup(&mut shell, parent, anchor);
        wait_for(&mut shell, |e| {
            e.kind == NHSHELL_EV_FRAME_READY && e.window == p
        });
        draw(&mut shell, p, 60, 30);
        rec.wait_node(p, |n| n.contains("\"position\":[110,60]"));
        shell.set_position(p, 20, 25).unwrap();
        wait_for(&mut shell, |e| {
            e.kind == NHSHELL_EV_FRAME_READY && e.window == p
        });
        assert_eq!(shell.window_ready(p).unwrap(), NHSHELL_READY_OWED);
        // acked, not committed: the position stays
        std::thread::sleep(Duration::from_millis(100));
        rec.wait_node(p, |n| n.contains("\"position\":[110,60]"));
        shell.present_bare(p).unwrap();
        rec.wait_node(p, |n| n.contains("\"position\":[30,35]"));
    }

    /// B2: a child's move or stacking is its parent's state, so the parent owes a commit.
    #[test]
    fn child_move_marks_the_parent_owed() {
        let rec = Recorder::start();
        let mut shell = rec.open();
        let parent = window(&mut shell);
        let c = child(&mut shell, parent, 40, 30);
        while shell.next_event().unwrap().is_some() {}
        std::thread::sleep(Duration::from_millis(100));
        while shell.next_event().unwrap().is_some() {}
        assert_eq!(shell.window_ready(parent).unwrap(), 0);
        shell.set_position(c, 70, 10).unwrap();
        wait_for(&mut shell, |e| {
            e.kind == NHSHELL_EV_FRAME_READY && e.window == parent
        });
        assert_ne!(shell.window_ready(parent).unwrap() & NHSHELL_READY_OWED, 0);
        // a bare commit of the parent clears it, and applies the move
        shell.present_bare(parent).unwrap();
        rec.wait_node(c, |n| n.contains("\"position\":[70,10]"));
        assert_eq!(shell.window_ready(parent).unwrap(), 0);
        shell.stack(c, 0, false).unwrap();
        assert_ne!(shell.window_ready(parent).unwrap() & NHSHELL_READY_OWED, 0);
    }

    /// A popup takes no buffer before its first configure.
    #[test]
    fn popup_takes_no_buffer_before_its_first_configure() {
        let rec = Recorder::start();
        let mut shell = rec.open();
        let parent = window(&mut shell);
        let p = popup(&mut shell, parent, Anchor::default());
        shell.window_dirty(p).unwrap();
        assert_eq!(shell.window_ready(p).unwrap(), 0);
        assert!(shell.present_begin(p, 60, 30).is_err());
        // the configure is dispatched: its content ack makes the popup ready
        wait_for(&mut shell, |e| {
            e.kind == NHSHELL_EV_FRAME_READY && e.window == p
        });
        assert_ne!(shell.window_ready(p).unwrap() & NHSHELL_READY_CONTENT, 0);
        assert!(shell.present_begin(p, 60, 30).is_ok());
    }

    /// Below `xdg_popup` v3 a move recreates the popup: the answer is "redraw all", the old `xdg_surface` is acked
    /// no more, and the next buffer maps the new popup.
    #[test]
    fn popup_recreate_asks_for_a_redraw() {
        let rec = Recorder::start_profile("xdg-v2");
        let mut shell = rec.open();
        let parent = window(&mut shell);
        let anchor = Anchor {
            x: 100,
            y: 50,
            w: 10,
            h: 10,
            gravity: 8,
            adjust: 0,
        };
        let p = popup(&mut shell, parent, anchor);
        wait_for(&mut shell, |e| {
            e.kind == NHSHELL_EV_FRAME_READY && e.window == p
        });
        draw(&mut shell, p, 60, 30);
        rec.wait_node(p, |n| n.contains("\"position\":[110,60]"));
        assert!(shell.set_position(p, 20, 25).unwrap(), "redraw all");
        // the new popup is configured again, and that first configure wants a buffer
        wait_for(&mut shell, |e| {
            e.kind == NHSHELL_EV_FRAME_READY && e.window == p
        });
        assert_ne!(shell.window_ready(p).unwrap() & NHSHELL_READY_CONTENT, 0);
        draw(&mut shell, p, 60, 30);
        let n = rec.wait_node(p, |n| n.contains("\"position\":[30,35]"));
        assert!(!n.contains("\"buffer\":null"), "{n}");
        // the connection is still up: the recorder raised no protocol error
        assert!(shell.next_event().is_ok());
    }

    #[test]
    fn popup_done_event_posted() {
        let rec = Recorder::start();
        let mut shell = rec.open();
        let parent = window(&mut shell);
        let p = popup(&mut shell, parent, Anchor::default());
        rec.wait_node(p, |_| true);
        let r = rec.control(&format!(r#"{{"cmd":"popup_done","surface":{p}}}"#));
        assert!(r.contains("\"ok\":true"), "{r}");
        let ev = wait_for(&mut shell, |e| e.kind == NHSHELL_EV_POPUP_DONE);
        assert_eq!(ev.window, p);
    }

    #[test]
    fn destroy_child_leaves_one_surface() {
        let rec = Recorder::start();
        let mut shell = rec.open();
        let parent = window(&mut shell);
        let c = child(&mut shell, parent, 0, 0);
        rec.wait_surfaces(2);
        shell.window_destroy(c).unwrap();
        rec.wait_surfaces(1);
    }
}

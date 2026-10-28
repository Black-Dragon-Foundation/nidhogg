//! T1 tests of the presenter against nh-recorder (manual clock): damage, viewport, frame callbacks, idle trim.
//! The harness is `common/mod.rs`.

mod common;

use common::backing::*;
use common::*;
use glitnir::geom::Rect;
use nhshell::api::{
    Anchor, ChildDesc, NHSHELL_ACK_CONTENT, NHSHELL_KIND_CHILD, NHSHELL_SF_DRAWABLE, ShellBackend,
    WindowDesc,
};
use nhshell::event::*;
use nhshell::wayland::WaylandShell;
use std::time::{Duration, Instant};

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
        .ack_configure(id, unsafe { ev.u.configure }.serial, NHSHELL_ACK_CONTENT)
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
        let rec = Recorder::start_clock("sway", "manual");
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
        // a buffer that was never written is committed whole; the first reuse carries the host's own damage
        draw(&mut shell, id, w, h, &[Rect::new(5, 6, 100, 20)]);
        let first = wait_buffer_commits(&rec, 1);
        assert!(
            first[0].contains("\"rect\":[0,0,300,150]}]"),
            "{}",
            first[0]
        );
        assert!(first[0].contains("\"dst\":[200,100]"), "{}", first[0]);
        assert!(first[0].contains("\"buffer_scale\":1"), "{}", first[0]);
        let mut n = 1;
        loop {
            present(&rec);
            std::thread::sleep(Duration::from_millis(100));
            n += 1;
            if draw(&mut shell, id, w, h, &[Rect::new(5, 6, 100, 20)]) > 0 {
                break;
            }
            assert!(n < 6, "no buffer was ever reused");
        }
        let log = wait_buffer_commits(&rec, n);
        let c = log.last().unwrap();
        assert!(c.contains("\"rect\":[5,6,100,20]}]"), "{c}");
    }

    #[test]
    fn every_commit_is_made() {
        // a commit while the previous frame is pending is made at once; a new buffer goes out with full damage
        let rec = Recorder::start_clock("sway", "manual");
        let mut shell = rec.open();
        let id = window(&mut shell);
        draw(&mut shell, id, 200, 100, &[Rect::new(0, 0, 10, 10)]);
        draw(&mut shell, id, 200, 100, &[Rect::new(50, 50, 10, 10)]);
        let log = wait_buffer_commits(&rec, 2);
        assert_eq!(log.len(), 2);
        assert!(log[1].contains("\"rect\":[0,0,200,100]}]"), "{}", log[1]);
    }

    #[test]
    fn frame_callback_paces_without_presentation() {
        let rec = Recorder::start_clock("no-presentation", "manual");
        let mut shell = rec.open();
        let id = window(&mut shell);
        draw(&mut shell, id, 200, 100, &[Rect::new(0, 0, 10, 10)]);
        wait_buffer_commits(&rec, 1);
        assert_eq!(shell.present_pacer(id).map(|p| p.0), Some(true), "waits");
        while shell.next_event().unwrap().is_some() {}
        shell.window_dirty(id).unwrap();
        assert_eq!(
            shell.window_ready(id).unwrap(),
            0,
            "not before the callback"
        );
        present(&rec);
        wait_for(&mut shell, |e| {
            e.kind == NHSHELL_EV_FRAME_READY && e.window == id
        });
        assert_ne!(
            shell.window_ready(id).unwrap(),
            0,
            "{:?}",
            shell.present_pacer(id)
        );
        assert_eq!(shell.present_pacer(id).map(|p| p.0), Some(false));
    }

    #[test]
    fn frame_done_after_present() {
        let rec = Recorder::start_clock("sway", "manual");
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
        let rec = Recorder::start_clock("sway", "manual");
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
        let rec = Recorder::start_clock("sway", "manual");
        let mut shell = rec.open();
        let id = window(&mut shell);
        draw(&mut shell, id, 201, 101, &[]);
        let (_, _, len) = shell.present_stats(id).unwrap();
        // SAFETY: sysconf has no preconditions.
        let page = unsafe { libc::sysconf(libc::_SC_PAGESIZE) } as usize;
        assert!(len > 0 && len % page == 0, "{len}");
    }

    /// When nothing composes a surface (here: it has no scene) the plain copy puts each pixel at its own place.  It
    /// used to copy from the layer's origin (a child frame's shadow margin) to the buffer's top-left.
    #[test]
    fn fallback_copy_honours_source_origin() {
        let rec = Recorder::start_clock("sway", "manual");
        let mut shell = rec.open();
        let id = window(&mut shell);
        let mut b = Backing::bare(200, 100);
        b.src = (8, 8);
        b.fill_all(0xff20_4060);
        b.fill(Rect::new(20, 20, 10, 10), 0xffff_8000);
        assert!(publish(&mut shell, id, &[(&b, id)], 0));
        assert!(shell.present_tree(id).unwrap());
        shell.flush().unwrap();
        wait_buffer_commits(&rec, 1);
        let snap = rec.snapshot();
        for (x, y) in [(0, 0), (8, 8), (12, 12), (25, 25), (35, 35), (199, 99)] {
            assert_eq!(
                Recorder::snapshot_pixel(&snap, x, y),
                b.pixel(x as i32, y as i32),
                "({x}, {y})"
            );
        }
    }

    #[derive(Clone, Copy, Debug, PartialEq)]
    enum Who {
        Root,
        Kid,
        Grand,
    }

    #[derive(Clone, Copy, Debug, PartialEq)]
    enum Got {
        Buffer,
        BufferAck,
        Bare,
        BareAck,
    }

    /// A root (200x100) with a child (100x60 at 20,20) and a grandchild (40x30 at 5,5), each with its own backing
    /// and scene, all mapped and shown.
    struct Tree {
        rec: Recorder,
        shell: WaylandShell,
        ids: [u64; 3],
        b: [Backing; 3],
    }

    impl Tree {
        fn new() -> Tree {
            let rec = Recorder::start_clock("sway", "manual");
            let mut shell = rec.open();
            let root = window(&mut shell);
            let child = |shell: &mut WaylandShell, parent, x, y, w, h| {
                shell
                    .child_new(&ChildDesc {
                        kind: NHSHELL_KIND_CHILD,
                        parent,
                        x,
                        y,
                        w,
                        h,
                        anchor: Anchor::default(),
                        flags: 0,
                    })
                    .unwrap()
            };
            let kid = child(&mut shell, root, 20, 20, 100, 60);
            let grand = child(&mut shell, kid, 5, 5, 40, 30);
            let mut b = [
                Backing::new(200, 100),
                Backing::new(100, 60),
                Backing::new(40, 30),
            ];
            for (i, b) in b.iter_mut().enumerate() {
                b.fill_all(0xff10_2030 + 0x10 * i as u32);
            }
            let mut t = Tree {
                rec,
                shell,
                ids: [root, kid, grand],
                b,
            };
            assert!(t.publish());
            assert!(t.shell.present_tree(root).unwrap());
            t.shell.flush().unwrap();
            t.settle();
            t
        }

        fn publish(&mut self) -> bool {
            let parts: Vec<(&Backing, u64)> = (0..3).map(|i| (&self.b[i], self.ids[i])).collect();
            publish(&mut self.shell, self.ids[0], &parts, 0)
        }

        /// A configure of the same size, acked with `flags`.
        fn configure(&mut self, flags: u32) {
            let r = self
                .rec
                .control(r#"{"cmd":"configure","surface":"focused","size":[200,100],"states":[]}"#);
            let serial = number(&r, "serial").expect("a serial") as u32;
            wait_for(&mut self.shell, |e| e.kind == NHSHELL_EV_CONFIGURE);
            self.shell
                .ack_configure(self.ids[0], serial, flags)
                .unwrap();
        }

        /// The root's frame is shown and every buffer the compositor let go is back.
        fn settle(&mut self) {
            present(&self.rec);
            let deadline = Instant::now() + Duration::from_secs(5);
            while self.shell.present_pacer(self.ids[0]).unwrap().0 {
                while self.shell.next_event().unwrap().is_some() {}
                assert!(Instant::now() < deadline, "the root never stopped waiting");
                std::thread::sleep(Duration::from_millis(10));
            }
            std::thread::sleep(Duration::from_millis(100));
            while self.shell.next_event().unwrap().is_some() {}
        }

        /// The tree's commits from recorder commit `seq` on, in order.  Every root commit re-applies its sync children
        /// (the recorder logs each, `sync` false); a child's own commits are the held ones (`sync` true).
        fn got(&self, seq: u64) -> Vec<(Who, Got)> {
            let log = self
                .rec
                .control(&format!(r#"{{"cmd":"log","since":{seq}}}"#));
            log.split("{\"ack_serial\":")
                .skip(1)
                .filter_map(|c| {
                    let who = [Who::Root, Who::Kid, Who::Grand]
                        .into_iter()
                        .zip(self.ids)
                        .find(|(_, id)| c.contains(&format!("\"surface\":{id},")))?
                        .0;
                    if who != Who::Root && !c.contains("\"sync\":true") {
                        return None;
                    }
                    let ack = !c.starts_with("null");
                    let got = match (c.contains("\"buffer\":{"), ack) {
                        (true, false) => Got::Buffer,
                        (true, true) => Got::BufferAck,
                        (false, false) => Got::Bare,
                        (false, true) => Got::BareAck,
                    };
                    Some((who, got))
                })
                .collect()
        }
    }

    fn next_seq(rec: &Recorder) -> u64 {
        number(&rec.control(r#"{"cmd":"log","since":1}"#), "next").expect("next")
    }

    /// The present path's rule, one case per row of its table (content, the content ack and its eligibility, owed
    /// state, the gate, the publish condition, and the three branches), on a three-level tree.
    #[test]
    fn rule_matches_present_path_table() {
        use Got::*;
        use Who::*;
        type Case = (&'static str, fn(&mut Tree), &'static [(Who, Got)]);
        let cases: [Case; 12] = [
            (
                "content: damage on the root",
                |t| {
                    t.b[0].fill(Rect::new(0, 0, 10, 10), 0xffc0_2020);
                    assert!(t.publish());
                },
                &[(Root, Buffer)],
            ),
            (
                "content: damage on a child that may not show a buffer waits",
                |t| {
                    t.b[1].flags = 0;
                    t.b[1].fill(Rect::new(0, 0, 10, 10), 0xffc0_2020);
                    assert!(!t.publish(), "nothing drawable was damaged");
                    t.b[1].flags = NHSHELL_SF_DRAWABLE;
                },
                &[],
            ),
            (
                "content: a drawable grandchild, and the child's damage that waited; deepest first",
                |t| {
                    t.b[2].fill(Rect::new(0, 0, 10, 10), 0xffc0_2020);
                    assert!(t.publish());
                },
                &[(Grand, Buffer), (Kid, Buffer), (Root, Bare)],
            ),
            ("else: no damage, nothing owed", |_| {}, &[]),
            ("owed: a bare ack", |t| t.configure(0), &[(Root, BareAck)]),
            (
                "owed: a grandchild's move, deepest first",
                |t| {
                    t.shell.set_position(t.ids[2], 7, 7).unwrap();
                },
                &[(Grand, Bare), (Kid, Bare), (Root, Bare)],
            ),
            (
                "ackc: a content ack with no packet after it is held (the publish condition)",
                |t| t.configure(NHSHELL_ACK_CONTENT),
                &[],
            ),
            (
                "ackc: a packet after it, nothing drawn: the whole picture carries it",
                |t| assert!(t.publish(), "the ack wants a packet"),
                &[(Root, BufferAck)],
            ),
            (
                "gate: a waiting pacer holds content",
                |t| {
                    t.b[0].fill(Rect::new(0, 0, 10, 10), 0xff20_c020);
                    assert!(t.publish());
                    assert!(t.shell.present_tree(t.ids[0]).unwrap());
                    t.b[0].fill(Rect::new(20, 0, 10, 10), 0xff20_c020);
                    assert!(t.publish());
                },
                &[],
            ),
            (
                "gate: a content ack waives the wait",
                |t| {
                    t.b[0].fill(Rect::new(0, 0, 10, 10), 0xff20_20c0);
                    assert!(t.publish());
                    assert!(t.shell.present_tree(t.ids[0]).unwrap());
                    t.configure(NHSHELL_ACK_CONTENT);
                    assert!(t.publish());
                },
                &[(Root, BufferAck)],
            ),
            (
                "buffer branch: every surface damaged, deepest first, the root last",
                |t| {
                    for b in &mut t.b {
                        b.fill(Rect::new(1, 1, 5, 5), 0xff80_8080);
                    }
                    assert!(t.publish());
                },
                &[(Grand, Buffer), (Kid, Buffer), (Root, Buffer)],
            ),
            (
                "bare branch: owed surfaces deepest first, the bare ack on the root only",
                |t| {
                    t.shell.set_position(t.ids[1], 30, 30).unwrap();
                    t.configure(0);
                },
                &[(Kid, Bare), (Root, BareAck)],
            ),
        ];
        let mut t = Tree::new();
        for (name, setup, want) in cases {
            t.settle();
            setup(&mut t);
            // the setup's own commits (a gate case's first picture, a re-arm) are not the case's
            t.shell.flush().unwrap();
            std::thread::sleep(Duration::from_millis(100));
            let seq = next_seq(&t.rec);
            let root_buffer = want.iter().any(|w| matches!(w, (Root, Buffer | BufferAck)));
            assert_eq!(
                t.shell.present_tree(t.ids[0]).unwrap(),
                root_buffer,
                "{name}"
            );
            t.shell.flush().unwrap();
            let deadline = Instant::now() + Duration::from_secs(2);
            while t.got(seq).len() < want.len() && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(10));
            }
            std::thread::sleep(Duration::from_millis(100));
            assert_eq!(t.got(seq), want, "{name}");
        }
        assert!(t.shell.next_event().is_ok(), "no protocol error");
    }
}

//! T1 tests of the presenter thread against nh-recorder: it commits on a frame callback without the host, never
//! writes a buffer the compositor holds, survives a window destroyed mid-compose and a close, keeps the host's waits
//! short, and sleeps when idle.  The host's surfaces are `common/backing.rs`; the harness is `common/mod.rs`.

mod common;

use common::backing::*;
use common::*;
use glitnir::geom::Rect;
use nhshell::api::*;
use nhshell::event::*;
use nhshell::wayland::WaylandShell;
use std::time::{Duration, Instant};

const BG: u32 = 0xff20_4060;

fn present(rec: &Recorder) {
    let r = rec.control(r#"{"cmd":"present","advance_ns":16000000}"#);
    assert!(r.contains("\"ok\":true"), "{r}");
}

/// Dispatch the shell's records until `f` holds (10 s at most).
fn until(shell: &mut WaylandShell, what: &str, f: impl Fn(&mut WaylandShell) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        while shell.next_event().unwrap().is_some() {}
        if f(shell) {
            return;
        }
        assert!(Instant::now() < deadline, "never: {what}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn next_seq(rec: &Recorder) -> u64 {
    number(&rec.control(r#"{"cmd":"log","since":1}"#), "next").expect("next")
}

/// Surface `id`'s commits from recorder commit `seq` on, one JSON text each.
fn commits(rec: &Recorder, seq: u64, id: u64) -> Vec<String> {
    let log = rec.control(&format!(r#"{{"cmd":"log","since":{seq}}}"#));
    // the objects have sorted keys, so `ack_serial` opens each commit
    log.split("{\"ack_serial\":")
        .skip(1)
        .map(|c| format!("{{\"ack_serial\":{c}"))
        .filter(|c| c.contains(&format!("\"surface\":{id},")))
        .collect()
}

fn buffer_commits(rec: &Recorder, seq: u64, id: u64) -> usize {
    commits(rec, seq, id)
        .iter()
        .filter(|c| c.contains("\"buffer\":{"))
        .count()
}

/// The output shows `b` at the top-left.
fn shows(rec: &Recorder, b: &Backing) -> bool {
    let snap = rec.snapshot();
    (0..b.h).all(|y| {
        (0..b.w).all(|x| Recorder::snapshot_pixel(&snap, x as usize, y as usize) == b.pixel(x, y))
    })
}

/// A shell on `rec` presenting on its own thread.
fn threaded(rec: &Recorder) -> WaylandShell {
    let mut shell = rec.open();
    shell.set_presenter_thread(true).unwrap();
    assert_eq!(shell.presenter_stats().unwrap().thread_on, 1);
    shell
}

/// A top-level of `b`'s size whose first configure is acked (a content ack) and whose first picture, `b` filled with
/// `BG`, the presenter has committed.
fn window(shell: &mut WaylandShell, rec: &Recorder, b: &mut Backing) -> u64 {
    let id = shell
        .window_new(&WindowDesc {
            w: b.w,
            h: b.h,
            title: "t".into(),
            app_id: "nidhogg.test".into(),
            flags: 0,
        })
        .unwrap();
    let ev = wait_for(shell, |e| e.kind() == Kind::Configure && e.window == id);
    let seq = next_seq(rec);
    shell
        .ack_configure(id, configure_of(&ev).serial, NHSHELL_ACK_CONTENT)
        .unwrap();
    b.fill_all(BG);
    assert!(publish(shell, id, &[(&*b, id)], 0));
    until(shell, "the first picture", |_| {
        buffer_commits(rec, seq, id) >= 1
    });
    id
}

fn waiting(shell: &WaylandShell, id: u64) -> bool {
    shell.present_pacer(id).unwrap().0
}

/// Under ThreadSanitizer every lock is slow: the time bounds say nothing there (the lane checks races).
fn sanitized() -> bool {
    std::env::var_os("TSAN_OPTIONS").is_some()
}

/// A tiny pseudo-random sequence (no crates in T1).
struct Lcg(u64);

impl Lcg {
    fn next(&mut self, n: u64) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (self.0 >> 33) % n.max(1)
    }
}

/// `tid`'s context switches so far (each wake of a sleeping thread is one).
fn switches(tid: i32) -> u64 {
    let s = std::fs::read_to_string(format!("/proc/self/task/{tid}/status")).unwrap();
    s.lines()
        .filter(|l| l.contains("ctxt_switches:"))
        .map(|l| l.split_whitespace().last().unwrap().parse::<u64>().unwrap())
        .sum()
}

/// A window whose pacer the compositor starves: every replaced buffer is held, then 200 packets.
fn starve(rec: &Recorder) -> (WaylandShell, Backing, u64) {
    let mut shell = threaded(rec);
    let mut b = Backing::new(200, 100);
    let id = window(&mut shell, rec, &mut b);
    rec.control_ok(r#"{"cmd":"mode","hold_buffers":64}"#);
    for i in 0..200u32 {
        b.fill(
            Rect::new((i % 20) as i32 * 10, (i % 10) as i32 * 10, 10, 10),
            0xff00_0000 | (i * 997),
        );
        publish(&mut shell, id, &[(&b, id)], 0);
        // each packet is committed before the next, in a buffer of its own, until none is left
        until(
            &mut shell,
            "the packet committed, or the tree starved",
            |s| {
                let st = s.presenter_stats().unwrap();
                st.presented == st.published || s.present_pacer(id).unwrap().1
            },
        );
    }
    until(&mut shell, "the tree starves", |s| {
        s.present_pacer(id).unwrap().1
    });
    (shell, b, id)
}

/// A popup of `parent`, `b`'s size, beside (10, 10).
fn popup(shell: &mut WaylandShell, parent: u64, b: &Backing) -> u64 {
    shell
        .child_new(&ChildDesc {
            kind: NHSHELL_KIND_POPUP,
            parent,
            x: 0,
            y: 0,
            w: b.w,
            h: b.h,
            anchor: Anchor {
                x: 10,
                y: 10,
                w: 1,
                h: 1,
                gravity: 8,
                adjust: 0,
            },
            flags: 0,
        })
        .unwrap()
}

/// The recorder's next commit `seq` at each `xdg_popup.reposition` of popup `id`.
fn repositions(rec: &Recorder, id: u64) -> Vec<u64> {
    rec.control(r#"{"cmd":"input_log","since":1}"#)
        .split("{\"commit_seq\":")
        .skip(1)
        .filter(|e| e.contains(&format!("\"surface\":{id},")))
        .map(|e| e.split(',').next().unwrap().parse().unwrap())
        .collect()
}

/// The shell and the recorder are level: a presenter turn (thread mode) covered every request so far, the recorder
/// handled them, the shell took every record of what the recorder sent before, and those records raised no request.
/// The connection stays up: no record is a protocol error's.
fn stays_up(shell: &mut WaylandShell) {
    for _ in 0..100 {
        let st = shell.presenter_stats().unwrap();
        if st.thread_on == 1 {
            turn(shell);
        } else {
            sync(shell);
        }
        let mut taken = 0;
        while let Some(ev) = shell.next_event().unwrap() {
            assert_ne!(ev.kind(), Kind::Disconnected, "a protocol error");
            taken += 1;
        }
        shell.dispatch_main().expect("the connection is up");
        if taken == 0 && shell.presenter_stats().unwrap().published == st.published {
            return;
        }
    }
    panic!("the shell and the recorder never came level");
}

/// The configures and other records the recorder sent so far are taken, with the presenter left alone (its compose
/// may be held at the gate).
fn read_records(shell: &mut WaylandShell) {
    sync(shell);
    while let Some(ev) = shell.next_event().unwrap() {
        assert_ne!(ev.kind(), Kind::Disconnected, "a protocol error");
    }
}

/// Thread `tid` is asleep (blocked in a wait), not running or ready to run.
fn asleep(tid: i32) -> bool {
    let s = std::fs::read_to_string(format!("/proc/self/task/{tid}/stat")).unwrap();
    // "TID (COMM) STATE ...": the name may hold spaces, so the state is what follows the last parenthesis
    s[s.rfind(')').unwrap() + 1..].trim_start().starts_with('S')
}

/// A popup chain whose child has its picture first: the child's buffer waits for its parent's, as xdg-shell wants
/// (mutter dismisses a popup mapped under an unmapped parent, and the recorder ends the client for it).
fn popups_map_parent_first(thread: bool) {
    let rec = Recorder::start("sway");
    let mut shell = rec.open();
    if thread {
        shell.set_presenter_thread(true).unwrap();
    }
    let mut tb = Backing::new(200, 100);
    let top = shell
        .window_new(&WindowDesc {
            w: tb.w,
            h: tb.h,
            title: "t".into(),
            app_id: "nidhogg.test".into(),
            flags: 0,
        })
        .unwrap();
    let ev = wait_for(&mut shell, |e| {
        e.kind() == Kind::Configure && e.window == top
    });
    shell
        .ack_configure(top, configure_of(&ev).serial, NHSHELL_ACK_CONTENT)
        .unwrap();
    tb.fill_all(BG);
    publish(&mut shell, top, &[(&tb, top)], 0);
    if !thread {
        shell.present_tree(top).unwrap();
    }
    rec.wait_window(&shell, top, |n| n.contains("\"buffer\":{"));

    let (mut pb, mut cb) = (Backing::new(60, 30), Backing::new(60, 30));
    let p = popup(&mut shell, top, &pb);
    let c = popup(&mut shell, p, &cb);
    // both configures are read
    stays_up(&mut shell);
    cb.fill_all(BG);
    publish(&mut shell, c, &[(&cb, c)], 0);
    if !thread {
        assert!(!shell.present_tree(c).unwrap(), "nothing under its parent");
    }
    stays_up(&mut shell);
    assert_eq!(buffer_commits(&rec, 1, c), 0, "the parent comes first");

    pb.fill_all(BG);
    publish(&mut shell, p, &[(&pb, p)], 0);
    if !thread {
        assert!(shell.present_tree(p).unwrap());
        wait_for(&mut shell, |e| {
            e.kind() == Kind::FrameReady && e.window == c
        });
        assert!(shell.present_tree(c).unwrap());
    }
    until(&mut shell, "the child's buffer", |_| {
        buffer_commits(&rec, 1, c) >= 1
    });
    stays_up(&mut shell);
    assert_eq!(buffer_commits(&rec, 1, p), 1);

    // the child's place was worked out against an unmapped parent: it is worked out again, and that is what it acks
    let first_seq = |id| {
        let c = commits(&rec, 1, id);
        let first = c.iter().find(|c| c.contains("\"buffer\":{")).unwrap();
        assert!(!first.contains("\"ack_stale\":true"), "{first}");
        number(first, "seq").unwrap()
    };
    let (parent, child) = (first_seq(p), first_seq(c));
    let placed = repositions(&rec, c);
    assert_eq!(placed.len(), 1, "{placed:?}");
    assert!(parent < placed[0] && placed[0] <= child, "{placed:?}");
    assert!(repositions(&rec, p).is_empty(), "its parent did not change");
}

mod presenter {
    use super::*;

    #[test]
    fn popups_map_parent_first_on_the_thread() {
        popups_map_parent_first(true);
    }

    #[test]
    fn popups_map_parent_first_inline() {
        popups_map_parent_first(false);
    }

    /// A popup's parent shows a new size: the place worked out against the old one goes out again after that commit,
    /// and the popup acks it, so that the compositor never judges the popup by a stale place.
    #[test]
    fn a_parents_new_size_places_its_popups_again() {
        let rec = Recorder::start("sway");
        let mut shell = threaded(&rec);
        let mut tb = Backing::new(200, 100);
        let top = window(&mut shell, &rec, &mut tb);
        let (mut pb, mut cb) = (Backing::new(60, 30), Backing::new(60, 30));
        let p = popup(&mut shell, top, &pb);
        let c = popup(&mut shell, p, &cb);
        stays_up(&mut shell);
        pb.fill_all(BG);
        publish(&mut shell, p, &[(&pb, p)], 0);
        cb.fill_all(BG);
        publish(&mut shell, c, &[(&cb, c)], 0);
        until(&mut shell, "the child's buffer", |_| {
            buffer_commits(&rec, 1, c) >= 1
        });

        assert!(!shell.set_size(p, 80, 40).unwrap());
        stays_up(&mut shell);
        pb.resize(80, 40);
        pb.fill_all(BG);
        publish(&mut shell, p, &[(&pb, p)], 0);
        until(&mut shell, "a second place", |_| {
            repositions(&rec, c).len() == 2
        });
        let log = commits(&rec, 1, p);
        let resized = log.iter().find(|c| c.contains("\"h\":40")).unwrap();
        assert!(repositions(&rec, c)[1] > number(resized, "seq").unwrap());
        let acked = move |rec: &Recorder| {
            let log = commits(rec, 1, c);
            log.iter().rev().any(|c| !c.contains("\"ack_serial\":null"))
                && !log.last().unwrap().contains("\"ack_stale\":true")
        };
        until(&mut shell, "the new place acked", |_| acked(&rec));
        stays_up(&mut shell);
    }

    /// A popup's new size reaches the compositor while its first picture composes: the configure read meanwhile holds
    /// the only ack there is (it waits for a picture of the new size), and that first buffer may not go without one.
    #[test]
    fn a_first_buffer_never_goes_without_an_ack() {
        let rec = Recorder::start("sway");
        let mut shell = threaded(&rec);
        let mut tb = Backing::new(200, 100);
        let top = window(&mut shell, &rec, &mut tb);
        let mut b = Backing::new(60, 30);
        let p = popup(&mut shell, top, &b);
        stays_up(&mut shell);
        shell.set_compose_gate(true);
        b.fill_all(BG);
        publish(&mut shell, p, &[(&b, p)], 0);
        assert!(
            shell.composes_held(1, Duration::from_secs(10)),
            "the first picture composed"
        );
        // its send waits at the gate while the new size's configure is read
        assert!(!shell.set_size(p, 80, 40).unwrap());
        read_records(&mut shell);
        shell.set_compose_gate(false);
        stays_up(&mut shell);
        assert_eq!(buffer_commits(&rec, 1, p), 0, "no buffer without an ack");
        b.resize(80, 40);
        b.fill_all(BG);
        publish(&mut shell, p, &[(&b, p)], 0);
        until(&mut shell, "the picture of the new size", |_| {
            buffer_commits(&rec, 1, p) >= 1
        });
        stays_up(&mut shell);
        let first = commits(&rec, 1, p)
            .into_iter()
            .find(|c| c.contains("\"buffer\":{"))
            .unwrap();
        assert!(!first.contains("\"ack_serial\":null"), "{first}");
        assert!(first.contains("\"h\":40"), "{first}");
    }

    /// A popup's picture is published before its first configure is read: a popup is made at its size, so that picture
    /// is the configure's, and it shows with no redisplay after it.
    #[test]
    fn a_popups_picture_may_come_before_its_first_configure() {
        let rec = Recorder::start("sway");
        let mut shell = threaded(&rec);
        let mut tb = Backing::new(200, 100);
        let top = window(&mut shell, &rec, &mut tb);
        let mut b = Backing::new(60, 30);
        let p = popup(&mut shell, top, &b);
        b.fill_all(BG);
        publish(&mut shell, p, &[(&b, p)], 0);
        until(&mut shell, "the picture", |_| {
            buffer_commits(&rec, 1, p) >= 1
        });
    }

    /// A popup's new size is drawn before the configure of its reposition is read: that picture is the configure's,
    /// and the ack (the new place) goes with no redisplay after it.
    #[test]
    fn a_resized_popups_picture_may_come_before_its_configure() {
        let rec = Recorder::start("sway");
        let mut shell = threaded(&rec);
        let mut tb = Backing::new(200, 100);
        let top = window(&mut shell, &rec, &mut tb);
        let mut b = Backing::new(60, 30);
        let p = popup(&mut shell, top, &b);
        stays_up(&mut shell);
        b.fill_all(BG);
        publish(&mut shell, p, &[(&b, p)], 0);
        until(&mut shell, "the first picture", |_| {
            buffer_commits(&rec, 1, p) >= 1
        });
        stays_up(&mut shell);
        let seq = next_seq(&rec);
        assert!(!shell.set_size(p, 80, 40).unwrap());
        b.resize(80, 40);
        b.fill_all(BG);
        publish(&mut shell, p, &[(&b, p)], 0);
        until(&mut shell, "the new place acked", |_| {
            commits(&rec, seq, p)
                .iter()
                .any(|c| !c.contains("\"ack_serial\":null"))
        });
        let acked = commits(&rec, seq, p)
            .into_iter()
            .find(|c| !c.contains("\"ack_serial\":null"))
            .unwrap();
        assert!(acked.contains("\"h\":40"), "{acked}");
    }

    /// A second size reaches a window with the first size's picture in the compose: its ack replaces the first's and
    /// waits for its own picture.  The first's picture has no ack left, and may not show that size.
    #[test]
    fn a_picture_whose_ack_was_replaced_does_not_show() {
        let rec = Recorder::start("sway");
        let mut shell = threaded(&rec);
        let mut b = Backing::new(200, 100);
        let id = window(&mut shell, &rec, &mut b);
        let resize = |shell: &mut WaylandShell, w: i32, h: i32| {
            rec.control_ok(&format!(
                r#"{{"cmd":"configure","surface":"focused","size":[{w},{h}],"states":[]}}"#
            ));
            let ev = wait_for(shell, |e| e.kind() == Kind::Configure && e.window == id);
            shell
                .ack_configure(id, configure_of(&ev).serial, NHSHELL_ACK_CONTENT)
                .unwrap();
        };
        let seq = next_seq(&rec);
        shell.set_compose_gate(true);
        resize(&mut shell, 300, 150);
        b.resize(300, 150);
        b.fill_all(BG);
        publish(&mut shell, id, &[(&b, id)], 0);
        assert!(
            shell.composes_held(1, Duration::from_secs(10)),
            "the first size's picture composed"
        );
        // its send waits at the gate while the second size comes and is acked
        resize(&mut shell, 200, 100);
        shell.set_compose_gate(false);
        stays_up(&mut shell);
        assert_eq!(buffer_commits(&rec, seq, id), 0, "a size no ack stands for");
        b.resize(200, 100);
        b.fill_all(BG);
        publish(&mut shell, id, &[(&b, id)], 0);
        until(&mut shell, "the second size's picture", |_| {
            buffer_commits(&rec, seq, id) >= 1
        });
        let shown = commits(&rec, seq, id)
            .into_iter()
            .find(|c| c.contains("\"buffer\":{"))
            .unwrap();
        assert!(!shown.contains("\"ack_serial\":null"), "{shown}");
        assert!(shown.contains("\"h\":100"), "{shown}");
    }

    /// A configure has come and the host, which drew a size of its own, has not answered it.  A compositor reads the
    /// commit of a new size as the client's own resize (sway sizes a floating window by it and drops the one it asked
    /// for), so the picture waits for the answer, which is all it needs.
    #[test]
    fn a_picture_waits_for_the_answer_to_a_configure() {
        let rec = Recorder::start("sway");
        let mut shell = threaded(&rec);
        let mut b = Backing::new(200, 100);
        let id = window(&mut shell, &rec, &mut b);
        let seq = next_seq(&rec);
        rec.control_ok(r#"{"cmd":"configure","surface":"focused","size":[300,150],"states":[]}"#);
        let ev = wait_for(&mut shell, |e| {
            e.kind() == Kind::Configure && e.window == id
        });
        b.resize(240, 100);
        b.fill_all(BG);
        publish(&mut shell, id, &[(&b, id)], 0);
        stays_up(&mut shell);
        assert_eq!(buffer_commits(&rec, seq, id), 0, "over an open configure");
        shell
            .ack_configure(id, configure_of(&ev).serial, 0)
            .unwrap();
        until(&mut shell, "the picture after the answer", |_| {
            buffer_commits(&rec, seq, id) >= 1
        });
    }

    /// Review Focus 3: the host publishes while the pacer waits, then stays away.  The frame callback comes; the
    /// presenter commits the packet before the host is back.
    #[test]
    fn wakes_on_frame_callback_without_the_host() {
        let rec = Recorder::start_clock("sway", "manual");
        let mut shell = threaded(&rec);
        let mut b = Backing::new(200, 100);
        let id = window(&mut shell, &rec, &mut b);
        assert!(
            waiting(&shell, id),
            "the first picture waits for its callback"
        );
        b.fill(Rect::new(10, 10, 20, 20), 0xff12_3456);
        let seq = next_seq(&rec);
        assert!(publish(&mut shell, id, &[(&b, id)], 0));
        // the presenter has had its turn for the packet: the pacer holds it
        turn(&shell);
        assert_eq!(
            buffer_commits(&rec, seq, id),
            0,
            "nothing before the callback"
        );
        // the host is away from here on: no shell call until the commit is in the recorder's log
        present(&rec);
        let deadline = Instant::now() + Duration::from_secs(10);
        while buffer_commits(&rec, seq, id) == 0 {
            assert!(
                Instant::now() < deadline,
                "the presenter never committed on its own"
            );
            // the recorder's log has no wake-up to wait on: ask again shortly
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(shell.next_event().is_ok(), "no protocol error");
    }

    /// The presenter's own commits move `presented`: behind `published` while the pacer holds a picture, at it once
    /// that picture's commit is flushed.
    #[test]
    fn presented_follows_the_presenters_commits() {
        let rec = Recorder::start_clock("sway", "manual");
        let mut shell = threaded(&rec);
        let mut b = Backing::new(200, 100);
        let id = window(&mut shell, &rec, &mut b);
        let presented = |s: &mut WaylandShell| {
            let st = s.presenter_stats().unwrap();
            st.presented == st.published
        };
        until(&mut shell, "the first picture presented", presented);
        b.fill(Rect::new(10, 10, 20, 20), 0xff12_3456);
        let seq = next_seq(&rec);
        assert!(publish(&mut shell, id, &[(&b, id)], 0));
        stays_up(&mut shell);
        assert_eq!(buffer_commits(&rec, seq, id), 0, "the pacer holds it");
        let st = shell.presenter_stats().unwrap();
        assert_eq!(st.presented + 1, st.published, "{st:?}");
        present(&rec);
        until(&mut shell, "the picture presented", presented);
        until(&mut shell, "its commit in the log", |_| {
            buffer_commits(&rec, seq, id) == 1
        });
    }

    /// A configure's bare ack is a request: the presenter moves `presented` past it only once the commit that carries
    /// it is flushed, so a settle that sees `presented` reach `published` has the answer on the wire before its next
    /// request.  Pattern C (handshake): the first picture's callback is answered and the pacer seen idle before the
    /// configure, so the configure re-arms nothing (`a_configure_while_waiting_rearms_bare`) and the ack's is the one
    /// bare commit.
    #[test]
    fn presented_passes_a_bare_ack_after_its_commit() {
        let rec = Recorder::start_clock("sway", "manual");
        let mut shell = threaded(&rec);
        let mut b = Backing::new(200, 100);
        let id = window(&mut shell, &rec, &mut b);
        let presented = |s: &mut WaylandShell| {
            let st = s.presenter_stats().unwrap();
            st.presented == st.published
        };
        until(&mut shell, "the first picture presented", presented);
        present(&rec);
        until(&mut shell, "the first callback", |s| !waiting(s, id));
        let r =
            rec.control(r#"{"cmd":"configure","surface":"focused","size":[200,100],"states":[]}"#);
        let serial = number(&r, "serial").expect("a serial") as u32;
        wait_for(&mut shell, |e| {
            e.kind() == Kind::Configure && e.window == id
        });
        let before = shell.presenter_stats().unwrap();
        shell.ack_configure(id, serial, 0).unwrap();
        until(&mut shell, "the ack presented", presented);
        let st = shell.presenter_stats().unwrap();
        assert_eq!(st.bare_commits - before.bare_commits, 1, "{st:?}");
    }

    /// Pattern C (handshake), the order the bare-ack row once raced on: a configure while the pacer waits for a frame
    /// callback re-arms it, a bare commit of its own that the presenter flushes after the host has read the configure.
    /// Counted from before the configure, the ack's commit makes two.
    #[test]
    fn a_configure_while_waiting_rearms_bare() {
        let rec = Recorder::start_clock("sway", "manual");
        let mut shell = threaded(&rec);
        let mut b = Backing::new(200, 100);
        let id = window(&mut shell, &rec, &mut b);
        let presented = |s: &mut WaylandShell| {
            let st = s.presenter_stats().unwrap();
            st.presented == st.published
        };
        until(&mut shell, "the first picture presented", presented);
        assert!(waiting(&shell, id), "the first callback is not answered");
        let before = shell.presenter_stats().unwrap();
        let r =
            rec.control(r#"{"cmd":"configure","surface":"focused","size":[200,100],"states":[]}"#);
        let serial = number(&r, "serial").expect("a serial") as u32;
        wait_for(&mut shell, |e| {
            e.kind() == Kind::Configure && e.window == id
        });
        until(&mut shell, "the configure's re-arm flushed", |s| {
            s.presenter_stats().unwrap().bare_commits > before.bare_commits
        });
        shell.ack_configure(id, serial, 0).unwrap();
        until(&mut shell, "the ack presented", presented);
        let st = shell.presenter_stats().unwrap();
        assert_eq!(st.bare_commits - before.bare_commits, 2, "{st:?}");
    }

    /// Review Focus 1: the compositor holds every buffer it replaced; 200 packets.  No buffer is written while
    /// held: the tree starves instead.
    #[test]
    fn never_writes_a_held_buffer() {
        let rec = Recorder::start("sway");
        let (mut shell, _b, id) = starve(&rec);
        let w = rec.control(r#"{"cmd":"held_buffer_writes","since":0}"#);
        assert!(w.contains("\"writes\":[]"), "held buffers written: {w}");
        assert!(
            shell.present_pacer(id).unwrap().1,
            "starved, not overwritten"
        );
        assert!(shell.next_event().is_ok(), "no protocol error");
    }

    /// Review Focus 2: a starved tree whose buffers come back commits the newest packet, and the picture equals
    /// the backing.
    #[test]
    fn starved_then_released_commits_latest() {
        let rec = Recorder::start("sway");
        let (mut shell, mut b, id) = starve(&rec);
        b.fill_all(0xff65_4321);
        b.fill(Rect::new(50, 20, 30, 30), 0xff11_2233);
        publish(&mut shell, id, &[(&b, id)], 0);
        rec.control_ok(r#"{"cmd":"mode","hold_buffers":0}"#);
        rec.control_ok(r#"{"cmd":"release"}"#);
        until(&mut shell, "the newest picture", |_| shows(&rec, &b));
        assert!(!shell.present_pacer(id).unwrap().1, "no longer starved");
        let w = rec.control(r#"{"cmd":"held_buffer_writes","since":0}"#);
        assert!(w.contains("\"writes\":[]"), "held buffers written: {w}");
    }

    /// Review Focus 4: a window destroyed while the presenter composes its tree (100 ms): the destroy returns
    /// within 2 ms, and nothing commits the destroyed surface.
    #[test]
    fn retire_during_compose_no_deadlock() {
        let rec = Recorder::start_clock("sway", "manual");
        let mut shell = threaded(&rec);
        let mut b = Backing::new(200, 100);
        let id = window(&mut shell, &rec, &mut b);
        present(&rec);
        until(&mut shell, "the callback", |s| !waiting(s, id));
        shell.set_compose_gate(true);
        b.fill(Rect::new(0, 0, 50, 50), 0xff00_ff00);
        let seq = next_seq(&rec);
        assert!(publish(&mut shell, id, &[(&b, id)], 0));
        assert!(
            shell.composes_held(1, Duration::from_secs(10)),
            "the picture composed"
        );
        assert_eq!(buffer_commits(&rec, seq, id), 0, "still composing");
        let t = Instant::now();
        shell.window_destroy(id).unwrap();
        let took = t.elapsed();
        if !sanitized() {
            assert!(took < Duration::from_millis(2), "destroy waited {took:?}");
        }
        // the compose goes on to its send, which finds the window retired
        shell.set_compose_gate(false);
        stays_up(&mut shell);
        assert!(
            commits(&rec, seq, id).is_empty(),
            "a commit on the destroyed surface"
        );
        assert!(shell.next_event().is_ok(), "no protocol error");
        assert_eq!(
            shell.presenter_stats().unwrap().thread_on,
            1,
            "still presenting"
        );
    }

    /// The structure the 1 ms bound of the GUI storm row (`nhwl-gui/delete-frame-storm-while-presenting`) rests on,
    /// asserted by what completes, not by a clock: with a compose parked at the gate (no tree mutex held), the host's
    /// calls (50 more packets, the window's destroy) all return, so none of them waits for a compose.  A watchdog
    /// aborts the run if one does.
    #[test]
    fn the_host_never_waits_for_a_parked_compose() {
        let rec = Recorder::start_clock("sway", "manual");
        let mut shell = threaded(&rec);
        let mut b = Backing::new(200, 100);
        let id = window(&mut shell, &rec, &mut b);
        present(&rec);
        until(&mut shell, "the callback", |s| !waiting(s, id));
        shell.set_compose_gate(true);
        b.fill(Rect::new(0, 0, 50, 50), 0xff00_ff00);
        assert!(publish(&mut shell, id, &[(&b, id)], 0));
        assert!(
            shell.composes_held(1, Duration::from_secs(10)),
            "the picture composed"
        );
        let done = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let watch = done.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_secs(30));
            if !watch.load(std::sync::atomic::Ordering::SeqCst) {
                eprintln!("a host call is waiting for a parked compose");
                std::process::abort();
            }
        });
        for i in 0..50u32 {
            b.fill(Rect::new(i as i32, 0, 10, 10), 0xff00_0000 | (i * 7919));
            assert!(publish(&mut shell, id, &[(&b, id)], 0));
        }
        shell.window_destroy(id).unwrap();
        done.store(true, std::sync::atomic::Ordering::SeqCst);
        shell.set_compose_gate(false);
        stays_up(&mut shell);
        assert!(shell.next_event().is_ok(), "no protocol error");
    }

    /// Closing joins the presenter within 100 ms, even with a compose under way.
    #[test]
    fn close_joins_within_100ms() {
        let rec = Recorder::start("sway");
        let mut shell = threaded(&rec);
        let mut b = Backing::new(200, 100);
        let id = window(&mut shell, &rec, &mut b);
        shell.set_compose_delay(50);
        for i in 0..5 {
            b.fill(Rect::new(i * 10, 0, 10, 10), 0xff00_00ff);
            publish(&mut shell, id, &[(&b, id)], 0);
        }
        let t = Instant::now();
        drop(shell);
        let took = t.elapsed();
        if !sanitized() {
            assert!(took < Duration::from_millis(100), "close took {took:?}");
        }
    }

    /// 10k packets over three trees while the compositor holds and releases buffers at random: the host's thread
    /// never waits for the presenter's work.  Its waits are for the presenter's decisions and sends, under a tree's
    /// mutex, and no compose or flush began with one held.  How long a wait took is wall-clock time, which load on the
    /// machine stretches (a thread woken late, the holder preempted), so it is not what the row reads.
    #[test]
    fn main_waits_are_bounded() {
        let rec = Recorder::start("kwin");
        let mut shell = threaded(&rec);
        let mut bs: Vec<Backing> = (0..3).map(|_| Backing::new(160, 90)).collect();
        let ids: Vec<u64> = bs.iter_mut().map(|b| window(&mut shell, &rec, b)).collect();
        let mut rng = Lcg(7);
        for i in 0..10_000u64 {
            let k = (i % 3) as usize;
            let (x, y) = (rng.next(150) as i32, rng.next(80) as i32);
            bs[k].fill(Rect::new(x, y, 10, 10), 0xff00_0000 | (i as u32 * 7919));
            publish(&mut shell, ids[k], &[(&bs[k], ids[k])], 0);
            while shell.next_event().unwrap().is_some() {}
            if rng.next(100) == 0 {
                rec.control_ok(r#"{"cmd":"release"}"#);
            }
        }
        let st = shell.presenter_stats().unwrap();
        assert_eq!(st.packets, 10_000 + 3);
        assert!(st.commits > 3, "presenting composed: {st:?}");
        assert_eq!(
            shell.presenter_locked_work(),
            0,
            "work under a tree's mutex"
        );
        assert!(shell.next_event().is_ok(), "no protocol error");
    }

    /// The host, the reader and the presenter at once: random configures (content and bare acks), child windows
    /// destroyed and made again, buffers released, 10k iterations.  No panic, no protocol error, still presenting.
    #[test]
    fn stress_three_threads() {
        let rec = Recorder::start("kwin");
        let mut shell = threaded(&rec);
        let mut root = Backing::new(240, 140);
        let id = window(&mut shell, &rec, &mut root);
        let child_desc = || ChildDesc {
            kind: NHSHELL_KIND_CHILD,
            parent: id,
            x: 20,
            y: 20,
            w: 80,
            h: 60,
            anchor: Anchor::default(),
            flags: 0,
        };
        let mut kid = Backing::new(80, 60);
        let mut child = shell.child_new(&child_desc()).unwrap();
        let mut rng = Lcg(42);
        for i in 0..10_000u64 {
            let on_kid = rng.next(3) == 0;
            let (b, w) = if on_kid {
                (&mut kid, 70)
            } else {
                (&mut root, 230)
            };
            let (x, y) = (rng.next(w) as i32, rng.next(50) as i32);
            b.fill(Rect::new(x, y, 10, 10), 0xff00_0000 | (i as u32 * 104_729));
            publish(&mut shell, id, &[(&root, id), (&kid, child)], 0);
            while let Some(ev) = shell.next_event().unwrap() {
                if let ShellEvent::Configure(c) = ev.event
                    && ev.window == id
                {
                    let serial = c.serial;
                    let flags = if rng.next(2) == 0 {
                        NHSHELL_ACK_CONTENT
                    } else {
                        0
                    };
                    shell.ack_configure(id, serial, flags).unwrap();
                }
            }
            match rng.next(60) {
                0 => {
                    rec.control_ok(&format!(
                        r#"{{"cmd":"configure","surface":{id},"size":[240,140],"states":[]}}"#
                    ));
                }
                1 => {
                    shell.window_destroy(child).unwrap();
                    child = shell.child_new(&child_desc()).unwrap();
                    kid = Backing::new(80, 60);
                    kid.fill_all(BG);
                }
                2 => {
                    rec.control_ok(r#"{"cmd":"release"}"#);
                }
                _ => {}
            }
        }
        assert!(shell.next_event().is_ok(), "no protocol error");
        assert_eq!(
            shell.presenter_stats().unwrap().thread_on,
            1,
            "still presenting"
        );
        assert!(shell.present_pacer(id).is_some());
    }

    /// Once the fronts are trimmed, the presenter sleeps: 3 s without a wake-up.
    #[test]
    fn idle_has_no_wakeups() {
        let rec = Recorder::start_clock("sway", "manual");
        let mut shell = threaded(&rec);
        let mut b = Backing::new(200, 100);
        let id = window(&mut shell, &rec, &mut b);
        present(&rec);
        until(&mut shell, "the callback", |s| !waiting(s, id));
        until(&mut shell, "the front trim", |s| {
            s.presenter_stats().unwrap().front_trims >= 1
        });
        let tid = shell.presenter_tid().expect("the presenter runs");
        // the turn that trimmed has ended in the presenter's wait
        until(&mut shell, "the presenter asleep", |_| asleep(tid));
        let before = switches(tid);
        // an idle span: nothing may wake the thread in it (no event marks the absence of wake-ups)
        std::thread::sleep(Duration::from_secs(3));
        assert_eq!(switches(tid), before, "nh-present woke while idle");
    }
}

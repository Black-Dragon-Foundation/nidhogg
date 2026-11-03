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
        while shell.next_event().unwrap().is_some() {}
        std::thread::sleep(Duration::from_millis(2));
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

/// `ms` of the shell's records: the connection stays up, so no protocol error ended it.
fn stays_up(shell: &mut WaylandShell, ms: i64) {
    for ev in collect(shell, ms) {
        assert_ne!(ev.kind(), Kind::Disconnected, "a protocol error");
    }
    shell.dispatch_main().expect("the connection is up");
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
    rec.wait_node(top, |n| n.contains("\"buffer\":{"));

    let (mut pb, mut cb) = (Backing::new(60, 30), Backing::new(60, 30));
    let p = popup(&mut shell, top, &pb);
    let c = popup(&mut shell, p, &cb);
    // both configures are read in this span
    stays_up(&mut shell, 100);
    cb.fill_all(BG);
    publish(&mut shell, c, &[(&cb, c)], 0);
    if !thread {
        assert!(!shell.present_tree(c).unwrap(), "nothing under its parent");
    }
    stays_up(&mut shell, 100);
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
    stays_up(&mut shell, 50);
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
        stays_up(&mut shell, 100);
        pb.fill_all(BG);
        publish(&mut shell, p, &[(&pb, p)], 0);
        cb.fill_all(BG);
        publish(&mut shell, c, &[(&cb, c)], 0);
        until(&mut shell, "the child's buffer", |_| {
            buffer_commits(&rec, 1, c) >= 1
        });

        assert!(!shell.set_size(p, 80, 40).unwrap());
        stays_up(&mut shell, 50);
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
        stays_up(&mut shell, 50);
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
        stays_up(&mut shell, 100);
        shell.set_compose_delay(300);
        b.fill_all(BG);
        publish(&mut shell, p, &[(&b, p)], 0);
        // in the compose: the delay outlasts these
        std::thread::sleep(Duration::from_millis(50));
        assert!(!shell.set_size(p, 80, 40).unwrap());
        stays_up(&mut shell, 400);
        assert_eq!(buffer_commits(&rec, 1, p), 0, "no buffer without an ack");
        shell.set_compose_delay(0);
        b.resize(80, 40);
        b.fill_all(BG);
        publish(&mut shell, p, &[(&b, p)], 0);
        until(&mut shell, "the picture of the new size", |_| {
            buffer_commits(&rec, 1, p) >= 1
        });
        stays_up(&mut shell, 50);
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
        stays_up(&mut shell, 100);
        b.fill_all(BG);
        publish(&mut shell, p, &[(&b, p)], 0);
        until(&mut shell, "the first picture", |_| {
            buffer_commits(&rec, 1, p) >= 1
        });
        stays_up(&mut shell, 50);
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
        shell.set_compose_delay(300);
        resize(&mut shell, 300, 150);
        b.resize(300, 150);
        b.fill_all(BG);
        publish(&mut shell, id, &[(&b, id)], 0);
        // in the compose: the delay outlasts these
        std::thread::sleep(Duration::from_millis(50));
        resize(&mut shell, 200, 100);
        stays_up(&mut shell, 400);
        assert_eq!(buffer_commits(&rec, seq, id), 0, "a size no ack stands for");
        shell.set_compose_delay(0);
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
        stays_up(&mut shell, 200);
        assert_eq!(buffer_commits(&rec, seq, id), 0, "over an open configure");
        shell
            .ack_configure(id, configure_of(&ev).serial, 0)
            .unwrap();
        until(&mut shell, "the picture after the answer", |_| {
            buffer_commits(&rec, seq, id) >= 1
        });
    }

    /// Review Focus 3: the host publishes while the pacer waits, then stays away for 200 ms.  The frame callback
    /// comes 20 ms in; the presenter commits the packet before the host is back.
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
        std::thread::sleep(Duration::from_millis(5));
        assert_eq!(
            buffer_commits(&rec, seq, id),
            0,
            "nothing before the callback"
        );
        let committed = std::thread::scope(|s| {
            let rec = &rec;
            let seen = s.spawn(move || {
                std::thread::sleep(Duration::from_millis(20));
                present(rec);
                let deadline = Instant::now() + Duration::from_secs(1);
                while Instant::now() < deadline {
                    if buffer_commits(rec, seq, id) >= 1 {
                        return Some(Instant::now());
                    }
                    std::thread::sleep(Duration::from_millis(2));
                }
                None
            });
            // the host is busy: no shell call at all
            std::thread::sleep(Duration::from_millis(200));
            let back = Instant::now();
            (seen.join().unwrap(), back)
        });
        let (seen, back) = committed;
        let seen = seen.expect("the presenter committed on its own");
        assert!(seen < back, "committed before the host came back");
        assert!(shell.next_event().is_ok(), "no protocol error");
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
        shell.set_compose_delay(100);
        b.fill(Rect::new(0, 0, 50, 50), 0xff00_ff00);
        let seq = next_seq(&rec);
        assert!(publish(&mut shell, id, &[(&b, id)], 0));
        std::thread::sleep(Duration::from_millis(30));
        assert_eq!(buffer_commits(&rec, seq, id), 0, "still composing");
        let t = Instant::now();
        shell.window_destroy(id).unwrap();
        let took = t.elapsed();
        if !sanitized() {
            assert!(took < Duration::from_millis(2), "destroy waited {took:?}");
        }
        std::thread::sleep(Duration::from_millis(200));
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
    /// never waits a millisecond for the presenter.
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
        if !sanitized() {
            assert!(
                st.main_wait_max_ns < 1_000_000,
                "the host waited {} ns",
                st.main_wait_max_ns
            );
        }
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
        std::thread::sleep(Duration::from_millis(50));
        let before = switches(tid);
        std::thread::sleep(Duration::from_secs(3));
        assert_eq!(switches(tid), before, "nh-present woke while idle");
    }
}

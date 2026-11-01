//! T1 tests of publishing against nh-recorder (manual clock): patches, packets merged while the pacer waits, key
//! times, content acks and fronts.  The host's surfaces are `common/backing.rs`; the harness is `common/mod.rs`.

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

fn waiting(shell: &WaylandShell, id: u64) -> bool {
    shell.present_pacer(id).unwrap().0
}

/// The frame is shown: its callback came and the buffer before it went back.
fn shown(shell: &mut WaylandShell, rec: &Recorder, id: u64) {
    present(rec);
    until(shell, "the frame callback", |s| !waiting(s, id));
    std::thread::sleep(Duration::from_millis(100));
    while shell.next_event().unwrap().is_some() {}
}

/// A top-level of 200x100 whose first configure is acked (a content ack), showing `b` filled with `BG`.
fn window(shell: &mut WaylandShell, b: &mut Backing) -> u64 {
    let id = shell
        .window_new(&WindowDesc {
            w: 200,
            h: 100,
            title: "t".into(),
            app_id: "nidhogg.test".into(),
            flags: 0,
        })
        .unwrap();
    let ev = wait_for(shell, |e| e.kind() == Kind::Configure);
    shell
        .ack_configure(id, configure_of(&ev).serial, NHSHELL_ACK_CONTENT)
        .unwrap();
    b.fill_all(BG);
    assert!(publish(shell, id, &[(&*b, id)], 0));
    assert!(shell.present_tree(id).unwrap(), "the first picture");
    shell.flush().unwrap();
    id
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

/// Surface `id`'s commits from `seq` on, once `n` of them carry a buffer (3 s at most).
fn buffer_commits(rec: &Recorder, seq: u64, id: u64, n: usize) -> Vec<String> {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        let all: Vec<String> = commits(rec, seq, id)
            .into_iter()
            .filter(|c| c.contains("\"buffer\":{"))
            .collect();
        if all.len() >= n || Instant::now() > deadline {
            return all;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// The `buffer` object of a commit's text.
fn buffer_of(commit: &str) -> &str {
    let b = &commit[commit.find("\"buffer\":{").expect("a buffer")..];
    &b[..b.find('}').unwrap()]
}

/// The output shows `b` at the top-left.
fn shows(rec: &Recorder, b: &Backing) {
    let snap = rec.snapshot();
    for y in 0..b.h {
        for x in 0..b.w {
            let got = Recorder::snapshot_pixel(&snap, x as usize, y as usize);
            assert_eq!(got, b.pixel(x, y), "pixel ({x}, {y})");
        }
    }
}

mod publish {
    use super::*;

    #[test]
    fn patch_copies_only_damage() {
        let rec = Recorder::start_clock("sway", "manual");
        let mut shell = rec.open();
        let mut b = Backing::new(200, 100);
        let id = window(&mut shell, &mut b);
        assert_eq!(shell.publish_copied(), 200 * 100 * 4, "no front yet: all");
        let rects = [Rect::new(0, 0, 10, 10), Rect::new(50, 50, 20, 5)];
        for r in rects {
            b.fill(r, 0xff00_ff00);
        }
        assert!(publish(&mut shell, id, &[(&b, id)], 0));
        let want: u64 = rects.iter().map(|r| r.area() * 4).sum();
        assert_eq!(shell.publish_copied() as u64, want);
        assert!(
            !publish(&mut shell, id, &[(&b, id)], 0),
            "nothing drawn, nothing queued"
        );
        assert_eq!(shell.presenter_stats().unwrap().packets, 2);
    }

    /// A window goes with a packet waiting for the pacer: that packet never commits, and the counters say so.
    #[test]
    fn a_packet_gone_with_its_window_is_superseded() {
        let rec = Recorder::start_clock("sway", "manual");
        let mut shell = rec.open();
        let mut b = Backing::new(200, 100);
        let id = window(&mut shell, &mut b);
        shown(&mut shell, &rec, id);
        b.fill(Rect::new(0, 0, 20, 10), 0xff11_2233);
        assert!(publish(&mut shell, id, &[(&b, id)], 0));
        assert!(shell.present_tree(id).unwrap());
        shell.flush().unwrap();
        b.fill(Rect::new(40, 0, 20, 10), 0xff33_2211);
        assert!(publish(&mut shell, id, &[(&b, id)], 0));
        assert!(!shell.present_tree(id).unwrap(), "the pacer waits");
        let before = shell.presenter_stats().unwrap();
        assert_eq!(before.superseded, 0);
        shell.window_destroy(id).unwrap();
        let after = shell.presenter_stats().unwrap();
        assert_eq!(after.superseded, 1);
        assert_eq!(after.commits, before.commits);
    }

    /// Review Focus 2: three packets while the pacer waits merge; the one commit at the callback damages their union
    /// and shows the last backing.
    #[test]
    fn superseded_packets_keep_their_damage() {
        let rec = Recorder::start_clock("sway", "manual");
        let mut shell = rec.open();
        let mut b = Backing::new(200, 100);
        let id = window(&mut shell, &mut b);
        shown(&mut shell, &rec, id);
        // two more frames: both pool buffers hold a picture, so the next ones are reused and damaged in part
        for c in [0xff40_0000, 0xff00_4000] {
            b.fill_all(c);
            assert!(publish(&mut shell, id, &[(&b, id)], 0));
            assert!(shell.present_tree(id).unwrap());
            shell.flush().unwrap();
            shown(&mut shell, &rec, id);
        }
        b.fill(Rect::new(80, 40, 10, 10), 0xff11_2233);
        assert!(publish(&mut shell, id, &[(&b, id)], 0));
        assert!(shell.present_tree(id).unwrap());
        shell.flush().unwrap();
        let seq = next_seq(&rec);
        let rects = [
            Rect::new(0, 0, 20, 10),
            Rect::new(100, 20, 30, 10),
            Rect::new(150, 80, 40, 15),
        ];
        for (i, r) in rects.iter().enumerate() {
            b.fill(*r, 0xff00_0080 + i as u32);
            assert!(publish(&mut shell, id, &[(&b, id)], 0));
            assert!(!shell.present_tree(id).unwrap(), "the pacer waits");
        }
        assert_eq!(shell.presenter_stats().unwrap().superseded, 2);
        present(&rec);
        wait_for(&mut shell, |e| {
            e.kind() == Kind::FrameReady && e.window == id
        });
        std::thread::sleep(Duration::from_millis(100));
        while shell.next_event().unwrap().is_some() {}
        assert!(shell.present_tree(id).unwrap());
        shell.flush().unwrap();
        let log = buffer_commits(&rec, seq, id, 1);
        assert_eq!(log.len(), 1, "one commit: {log:?}");
        for r in rects {
            let want = format!("\"rect\":[{},{},{},{}]", r.x, r.y, r.w, r.h);
            assert!(log[0].contains(&want), "{want} in {}", log[0]);
        }
        assert_eq!(log[0].matches("\"rect\":").count(), 3, "{}", log[0]);
        shows(&rec, &b);
    }

    #[test]
    fn earliest_key_time_is_kept() {
        let rec = Recorder::start_clock("sway", "manual");
        let mut shell = rec.open();
        let mut b = Backing::new(200, 100);
        let id = window(&mut shell, &mut b);
        let now = nhshell::wayland::publish::now_ns();
        // two keys while the first picture waits for its callback: the earlier one is what the next commit shows
        b.fill(Rect::new(0, 0, 10, 10), 0xff00_00ff);
        assert!(publish(&mut shell, id, &[(&b, id)], now - 10_000_000_000));
        b.fill(Rect::new(20, 0, 10, 10), 0xff00_00ff);
        assert!(publish(&mut shell, id, &[(&b, id)], now));
        // (the first configure's content ack posted a FRAME_READY still queued: wait for the callback itself)
        present(&rec);
        until(&mut shell, "the frame callback", |s| !waiting(s, id));
        assert!(shell.present_tree(id).unwrap());
        let s = glitnir::stats::summary(b.g, false);
        assert!(
            (10_000.0..20_000.0).contains(&s.update_ms.max),
            "key to commit {:?}",
            s.update_ms
        );
        // the key was shown: the next commit carries none
        shown(&mut shell, &rec, id);
        b.fill(Rect::new(40, 0, 10, 10), 0xff00_00ff);
        assert!(publish(&mut shell, id, &[(&b, id)], 0));
        assert!(shell.present_tree(id).unwrap());
        let after = glitnir::stats::summary(b.g, false);
        assert_eq!(after.update_ms.max, s.update_ms.max);
    }

    /// B1 across the publish: a content ack recorded after packet 5 is not sent with packet 5's buffer; it goes with
    /// packet 6, drawn at the configure's size.
    #[test]
    fn content_ack_waits_for_a_later_packet() {
        let rec = Recorder::start_clock("sway", "manual");
        let mut shell = rec.open();
        let mut b = Backing::new(200, 100);
        let id = window(&mut shell, &mut b);
        shown(&mut shell, &rec, id);
        b.fill(Rect::new(0, 0, 50, 50), 0xff80_8000);
        assert!(publish(&mut shell, id, &[(&b, id)], 0));
        let r =
            rec.control(r#"{"cmd":"configure","surface":"focused","size":[300,150],"states":[]}"#);
        let serial = number(&r, "serial").expect("a serial") as u32;
        wait_for(&mut shell, |e| e.kind() == Kind::Configure);
        shell
            .ack_configure(id, serial, NHSHELL_ACK_CONTENT)
            .unwrap();
        let seq = next_seq(&rec);
        let ack = format!("\"ack_serial\":{serial}");
        assert!(
            !shell.present_tree(id).unwrap(),
            "packet 5 does not carry the ack"
        );
        shell.flush().unwrap();
        std::thread::sleep(Duration::from_millis(100));
        assert!(
            commits(&rec, seq, id).iter().all(|c| !c.contains(&ack)),
            "{:?}",
            commits(&rec, seq, id)
        );
        b.resize(300, 150);
        b.fill_all(0xff00_8080);
        assert!(publish(&mut shell, id, &[(&b, id)], 0));
        assert!(shell.present_tree(id).unwrap());
        shell.flush().unwrap();
        let log = buffer_commits(&rec, seq, id, 1);
        let with_ack: Vec<&String> = log.iter().filter(|c| c.contains(&ack)).collect();
        assert_eq!(with_ack.len(), 1, "{log:?}");
        let buf = buffer_of(with_ack[0]);
        assert!(
            buf.contains("\"w\":300") && buf.contains("\"h\":150"),
            "{buf}"
        );
        assert!(shell.next_event().is_ok(), "no protocol error");
    }

    /// After the quiet period the front is dropped; the next publish copies the whole surface, and the picture is
    /// still right.
    #[test]
    fn full_patch_after_front_trim() {
        let rec = Recorder::start_clock("sway", "manual");
        let mut shell = rec.open();
        let mut b = Backing::new(200, 100);
        let id = window(&mut shell, &mut b);
        shown(&mut shell, &rec, id);
        let small = Rect::new(10, 10, 8, 8);
        b.fill(small, 0xff12_3456);
        assert!(publish(&mut shell, id, &[(&b, id)], 0));
        assert_eq!(shell.publish_copied() as u64, small.area() * 4);
        assert!(shell.present_tree(id).unwrap());
        shell.flush().unwrap();
        shown(&mut shell, &rec, id);
        std::thread::sleep(Duration::from_millis(2100));
        shell.dispatch_main().unwrap();
        assert_eq!(shell.presenter_stats().unwrap().front_trims, 1);
        b.fill(Rect::new(30, 30, 8, 8), 0xff65_4321);
        let seq = next_seq(&rec);
        assert!(publish(&mut shell, id, &[(&b, id)], 0));
        assert_eq!(shell.publish_copied(), 200 * 100 * 4, "the whole surface");
        assert!(shell.present_tree(id).unwrap());
        shell.flush().unwrap();
        assert_eq!(buffer_commits(&rec, seq, id, 1).len(), 1);
        shows(&rec, &b);
    }
}

mod tree_snapshot {
    use super::*;

    const GREEN: u32 = 0xff00_ff00;
    /// Half-covering blue, premultiplied.
    const VEIL: u32 = 0x8000_0080;

    fn child(shell: &mut WaylandShell, parent: u64, x: i32, y: i32) -> u64 {
        shell
            .child_new(&ChildDesc {
                kind: NHSHELL_KIND_CHILD,
                parent,
                x,
                y,
                w: 40,
                h: 20,
                anchor: Anchor::default(),
                flags: 0,
            })
            .unwrap()
    }

    fn backing(argb: u32) -> Backing {
        let mut b = Backing::new(40, 20);
        b.fill_all(argb);
        b
    }

    fn at(picture: &[u32], x: usize, y: usize) -> u32 {
        picture[y * 200 + x]
    }

    #[test]
    fn places_stacks_and_blends_a_tree() {
        let rec = Recorder::start_clock("sway", "manual");
        let mut shell = rec.open();
        let mut root = Backing::new(200, 100);
        let id = window(&mut shell, &mut root);
        let (solid, veil) = (backing(GREEN), backing(VEIL));
        let a = child(&mut shell, id, 10, 10);
        let b = child(&mut shell, id, 30, 20);
        let g = child(&mut shell, a, 100, 60);
        let off = child(&mut shell, id, -30, -10);
        let tree = [
            (&root, id),
            (&solid, a),
            (&veil, b),
            (&solid, g),
            (&solid, off),
        ];
        assert!(publish(&mut shell, id, &tree, 0));
        let frames = tree.map(|(b, w)| b.frame(w));
        let shot =
            |shell: &WaylandShell, f: &[SurfaceFrame]| shell.tree_snapshot(id, f, true).unwrap();

        let p = shot(&shell, &frames);
        assert_eq!(p.len(), 200 * 100, "the root's size");
        assert_eq!(at(&p, 199, 99), BG, "only the root");
        assert_eq!(at(&p, 15, 15), GREEN, "a child at its offset");
        assert_eq!(
            at(&p, 35, 25),
            0xff00_7f80,
            "the later child over the earlier"
        );
        assert_eq!(at(&p, 60, 35), 0xff10_20b0, "the veil over the root");
        assert_eq!(at(&p, 115, 75), GREEN, "a grandchild at both offsets");
        assert_eq!(at(&p, 9, 9), GREEN, "what hangs out of the root is cut");

        shell.stack(a, b, true).unwrap();
        assert_eq!(at(&shot(&shell, &frames), 35, 25), GREEN, "raised");

        let no_a = [frames[0], frames[2], frames[4]];
        let p = shot(&shell, &no_a);
        assert_eq!(at(&p, 15, 15), BG, "a surface not listed is hidden");
        assert_eq!(at(&p, 115, 75), BG, "and so is what hangs from it");

        shell.set_alpha(a, u32::MAX / 2).unwrap();
        let p = shot(&shell, &frames);
        assert_eq!(at(&p, 15, 15), 0xff10_9f30, "the compositor's alpha");
        assert_eq!(at(&p, 115, 75), GREEN, "on the window that has it");

        let empty: [SurfaceFrame; 0] = [];
        assert!(shell.tree_snapshot(id, &empty, true).is_err());
        assert!(
            shell.tree_snapshot(a, &frames[1..], true).is_err(),
            "not a root"
        );
    }

    /// A popup goes where the compositor put it, over the tree it hangs from; another tree's is left out.
    #[test]
    fn popups_show_over_their_tree() {
        let rec = Recorder::start("sway");
        let mut shell = rec.open();
        let mut root = Backing::new(200, 100);
        let id = window(&mut shell, &mut root);
        let mut other_root = Backing::new(200, 100);
        let other = window(&mut shell, &mut other_root);
        let (red, veil) = (backing(0xffff_0000), backing(VEIL));
        let anchor = Anchor {
            x: 10,
            y: 10,
            w: 4,
            h: 4,
            gravity: 8,
            adjust: 0,
        };
        let popup = |shell: &mut WaylandShell, parent| {
            shell
                .child_new(&ChildDesc {
                    kind: NHSHELL_KIND_POPUP,
                    parent,
                    x: 0,
                    y: 0,
                    w: 40,
                    h: 20,
                    anchor,
                    flags: 0,
                })
                .unwrap()
        };
        let p = popup(&mut shell, id);
        let q = popup(&mut shell, other);
        wait_for(&mut shell, |e| {
            e.kind() == Kind::FrameReady && e.window == p
        });
        let frames = [root.frame(id), red.frame(p), veil.frame(q)];
        let got = shell.tree_snapshot(id, &frames, true).unwrap();
        assert_eq!(got, shell.tree_snapshot(id, &frames, false).unwrap());
        assert_eq!(at(&got, 13, 13), BG);
        assert_eq!(at(&got, 14, 14), 0xffff_0000);
        assert_eq!(at(&got, 53, 33), 0xffff_0000);
        assert_eq!(at(&got, 54, 34), BG);
    }

    /// Ids are used again, so a popup on a popup may have the lower id, and so may a later sibling: each still shows
    /// over its parent, and siblings stack in the order they were made.
    #[test]
    fn popups_stack_in_the_order_they_were_made() {
        const BLUE: u32 = 0xff00_00ff;
        let rec = Recorder::start("sway");
        let mut shell = rec.open();
        let mut root = Backing::new(200, 100);
        let id = window(&mut shell, &mut root);
        let anchor = Anchor {
            x: 10,
            y: 10,
            w: 4,
            h: 4,
            gravity: 8,
            adjust: 0,
        };
        let popup = |shell: &mut WaylandShell, parent| {
            shell
                .child_new(&ChildDesc {
                    kind: NHSHELL_KIND_POPUP,
                    parent,
                    x: 0,
                    y: 0,
                    w: 40,
                    h: 20,
                    anchor,
                    flags: 0,
                })
                .unwrap()
        };
        let fillers = [popup(&mut shell, id), popup(&mut shell, id)];
        let p = popup(&mut shell, id);
        for f in fillers {
            shell.window_destroy(f).unwrap();
        }
        // the client library frees an id once the compositor has let go of it
        until(&mut shell, "the fillers gone", |_| {
            rec.surfaces("popup") == 1
        });
        std::thread::sleep(Duration::from_millis(100));
        while shell.next_event().unwrap().is_some() {}
        let n = popup(&mut shell, p);
        let s = popup(&mut shell, id);
        assert!(n < p && s < p, "no id used again: {n} and {s}, {p}");
        let (red, green, blue) = (backing(0xffff_0000), backing(GREEN), backing(BLUE));
        let frames = [root.frame(id), red.frame(p), green.frame(n), blue.frame(s)];
        let shot = |shell: &WaylandShell| shell.tree_snapshot(id, &frames, false).unwrap();
        // placed: nothing is left at the root's corner, where an unplaced popup shows
        until(&mut shell, "the popups placed", |sh| {
            let got = shot(sh);
            at(&got, 5, 5) == BG && at(&got, 20, 20) != BG
        });
        std::thread::sleep(Duration::from_millis(100));
        while shell.next_event().unwrap().is_some() {}
        let got = shot(&shell);
        assert_eq!(at(&got, 60, 40), GREEN, "the popup on a popup");
        assert_eq!(at(&got, 20, 20), BLUE, "the later sibling on top");
    }
}

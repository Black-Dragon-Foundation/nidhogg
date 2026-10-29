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

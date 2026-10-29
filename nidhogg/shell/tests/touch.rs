//! T1 tests of touch and pinch against nh-recorder: touch points posted per touch frame, a cancel that ends every
//! point, the touch-down serial a popup grab may use, and the pinch's cumulative scale.  The harness is
//! `common/mod.rs`.

mod common;

use common::*;
use nhshell::api::*;
use nhshell::event::*;
use nhshell::wayland::WaylandShell;

/// (window, id, op, x, y) of a TOUCH record.
fn touch_of(e: &Record) -> (u64, i32, u32, f64, f64) {
    let ShellEvent::Touch(t) = e.event else {
        panic!("not a touch record: {e:?}");
    };
    (e.window, t.id, t.op, t.x, t.y)
}

fn touches(shell: &mut WaylandShell, ms: i64) -> Vec<(u64, i32, u32, f64, f64)> {
    collect(shell, ms)
        .iter()
        .filter(|e| e.kind() == Kind::Touch)
        .map(touch_of)
        .collect()
}

fn touch(rec: &Recorder, op: &str, id: u32, at: (i32, i32), frame: bool) {
    let r = rec.control(&format!(
        r#"{{"cmd":"touch","op":"{op}","id":{id},"at":[{},{}],"frame":{frame}}}"#,
        at.0, at.1
    ));
    assert!(r.contains("\"ok\":true"), "{r}");
}

#[allow(clippy::module_inception)]
mod touch {
    use super::*;

    /// Records wait for the touch frame: two downs without a frame post nothing; the motion's frame posts all three.
    #[test]
    fn points_posted_at_frame() {
        let rec = Recorder::start("sway");
        let mut shell = rec.open();
        let top = mapped_window(&mut shell, 200, 100);
        collect(&mut shell, 50);
        touch(&rec, "down", 0, (5, 6), false);
        touch(&rec, "down", 1, (7, 8), false);
        let early = touches(&mut shell, 150);
        assert!(early.is_empty(), "nothing before the frame: {early:?}");
        touch(&rec, "motion", 0, (9, 10), true);
        let got = touches(&mut shell, 150);
        assert_eq!(
            got,
            [
                (top, 0, NHSHELL_TOUCH_DOWN, 5.0, 6.0),
                (top, 1, NHSHELL_TOUCH_DOWN, 7.0, 8.0),
                (top, 0, NHSHELL_TOUCH_MOTION, 9.0, 10.0),
            ]
        );
        touch(&rec, "up", 0, (0, 0), true);
        let ev = wait_event(&mut shell, Kind::Touch);
        assert_eq!(
            touch_of(&ev),
            (top, 0, NHSHELL_TOUCH_UP, 9.0, 10.0),
            "up keeps the last position"
        );
    }

    #[test]
    fn cancel_ends_every_point() {
        let rec = Recorder::start("sway");
        let mut shell = rec.open();
        let top = mapped_window(&mut shell, 200, 100);
        touch(&rec, "down", 0, (5, 6), true);
        touch(&rec, "down", 1, (7, 8), true);
        wait_for(&mut shell, |e| {
            e.kind() == Kind::Touch && touch_of(e).1 == 1
        });
        // Smithay sends `cancel` only while a point has an event its frame has not closed yet: an unframed motion
        touch(&rec, "motion", 0, (5, 6), false);
        let r = rec.control(r#"{"cmd":"touch","op":"cancel"}"#);
        assert!(r.contains("\"ok\":true"), "{r}");
        let mut got = touches(&mut shell, 200);
        got.sort_by_key(|t| t.1);
        assert_eq!(
            got,
            [
                (top, 0, NHSHELL_TOUCH_CANCEL, 5.0, 6.0),
                (top, 1, NHSHELL_TOUCH_CANCEL, 7.0, 8.0),
            ]
        );
        // the points are gone: a new frame with nothing in it posts nothing
        touch(&rec, "motion", 0, (1, 1), true);
        let late = touches(&mut shell, 150);
        assert!(late.is_empty(), "{late:?}");
    }

    /// A touch down is a press a popup grab may name.
    #[test]
    fn touch_serial_allows_a_grab() {
        let rec = Recorder::start("sway");
        let mut shell = rec.open();
        let top = mapped_window(&mut shell, 200, 100);
        touch(&rec, "down", 0, (5, 6), true);
        wait_event(&mut shell, Kind::Touch);
        let p = shell
            .child_new(&ChildDesc {
                kind: NHSHELL_KIND_POPUP,
                parent: top,
                x: 0,
                y: 0,
                w: 60,
                h: 30,
                anchor: Anchor {
                    x: 10,
                    y: 10,
                    w: 4,
                    h: 4,
                    gravity: 8,
                    adjust: 0,
                },
                flags: NHSHELL_WIN_GRAB,
            })
            .unwrap();
        rec.wait_node(p, |n| n.contains("\"grab\":{\"serial_kind\":\"touch\"}"));
        assert!(shell.window_grabbed(p).unwrap());
    }

    /// The pinch scale is cumulative since begin, as the protocol's is; END repeats the last one.  Positions and
    /// deltas are device pixels.
    #[test]
    fn pinch_scale_is_cumulative() {
        let rec = Recorder::start("sway");
        let mut shell = rec.open();
        let top = mapped_window(&mut shell, 200, 100);
        rec.control_ok(&format!(r#"{{"cmd":"focus","surface":{top}}}"#));
        rec.control_ok(r#"{"cmd":"pointer","pos":[4,4]}"#);
        wait_for(&mut shell, |e| {
            e.kind() == Kind::PointerEnter || e.kind() == Kind::PointerMotion
        });
        for req in [
            r#"{"cmd":"pinch","op":"begin","fingers":2}"#,
            r#"{"cmd":"pinch","op":"update","dx":1,"dy":2,"scale":1.5,"rotation":10}"#,
            r#"{"cmd":"pinch","op":"update","dx":0,"dy":0,"scale":2.0,"rotation":0}"#,
            r#"{"cmd":"pinch","op":"end"}"#,
        ] {
            let r = rec.control(req);
            assert!(r.contains("\"ok\":true"), "{r}");
        }
        let mut got = Vec::new();
        while got.len() < 4 {
            let ev = wait_event(&mut shell, Kind::Pinch);
            assert_eq!(ev.window, top);
            let ShellEvent::Pinch(p) = ev.event else {
                unreachable!()
            };
            assert_eq!((p.fingers, p.x, p.y), (2, 4.0, 4.0));
            got.push((p.phase, p.scale, p.dx, p.dy, p.rotation, p.cancelled));
        }
        assert_eq!(
            got,
            [
                (NHSHELL_PINCH_BEGIN, 1.0, 0.0, 0.0, 0.0, 0),
                (NHSHELL_PINCH_UPDATE, 1.5, 1.0, 2.0, 10.0, 0),
                (NHSHELL_PINCH_UPDATE, 2.0, 0.0, 0.0, 0.0, 0),
                (NHSHELL_PINCH_END, 2.0, 0.0, 0.0, 0.0, 0),
            ]
        );
    }
}

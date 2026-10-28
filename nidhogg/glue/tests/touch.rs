//! T1 tests of touch and pinch through the glue, against nh-recorder (the shell's harness, `../shell/tests/common`, and
//! the step's, `step/mod.rs`).  Each test stands in for C: it makes the window as `nhwl_window_new` does and reads the
//! actions `nhg_next` gives, with their texts.

#[path = "../../shell/tests/common/mod.rs"]
mod common;
mod step;

use common::{Recorder, readable_within};
use nhglue::action::*;
use nhglue::ffi::*;
use nhglue::touch::*;
use std::ptr;
use step::*;

/// Step until nothing comes for 200 ms; every action seen with a copy of its text.
fn settle_texts(d: *mut NhgDisplay) -> Vec<(NhgAction, Option<Vec<u8>>)> {
    let mut seen = Vec::new();
    loop {
        let input = NhgNextIn { flags: 0 };
        let mut list = NhgActionList {
            v: ptr::null(),
            n: 0,
        };
        // SAFETY: a live display; `input` is readable and `list` writable.
        let rc = unsafe { nhg_next(d, &input, &mut list) };
        assert!(rc >= 0, "nhg_next failed: {rc}");
        for i in 0..list.n {
            // SAFETY: `n` actions, valid until the next nhg_next.
            let a = unsafe { *list.v.add(i) };
            let text = (!a.text.is_null())
                // SAFETY: `len` bytes, valid until the next nhg_next.
                .then(|| {
                    unsafe { std::slice::from_raw_parts(a.text.cast::<u8>(), a.len) }.to_vec()
                });
            seen.push((a, text));
        }
        // SAFETY: a live display.
        if rc == 0 && !readable_within(unsafe { nhg_fd(d) }, 200) {
            return seen;
        }
    }
}

/// The touch actions among `v`: (flags, local id, x, y, points of an update as (x, y, id)).
type Touched = (u32, i32, i32, i32, Vec<(i32, i32, i32)>);

fn touches(v: &[(NhgAction, Option<Vec<u8>>)]) -> Vec<Touched> {
    v.iter()
        .filter(|(a, _)| a.kind == NHG_ACT_TOUCH)
        .map(|(a, text)| {
            let points = text
                .iter()
                .flat_map(|t| t.as_chunks::<{ size_of::<NhgTouchPoint>() }>().0.iter())
                .map(|c| {
                    let n = |i: usize| i32::from_ne_bytes(c[i * 4..i * 4 + 4].try_into().unwrap());
                    (n(0), n(1), n(2))
                })
                .collect();
            (a.flags, a.code, a.x, a.y, points)
        })
        .collect()
}

fn ready(d: *mut NhgDisplay) -> u64 {
    let id = window(d);
    // SAFETY: a live display.
    assert_eq!(unsafe { nhg_frame_units(d, id, 10, 10) }, 0);
    configured(d, id);
    id
}

fn touch(rec: &Recorder, op: &str, id: u32, at: (i32, i32), frame: bool) {
    rec.control_ok(&format!(
        r#"{{"cmd":"touch","op":"{op}","id":{id},"at":[{},{}],"frame":{frame}}}"#,
        at.0, at.1
    ));
}

#[allow(clippy::module_inception)]
mod touch {
    use super::*;

    /// A down, a motion and a lift: BEGIN with local id 0 and the position, an UPDATE with the window's points, an
    /// END with the lift's position and no cancel.  A second point gets id 1 and an update lists both, oldest first.
    #[test]
    fn begin_update_end() {
        let rec = Recorder::start("sway");
        let d = open(&rec);
        let id = ready(d);
        touch(&rec, "down", 0, (5, 6), true);
        queued(d);
        let got = touches(&settle_texts(d));
        assert_eq!(got, [(NHG_TOUCH_BEGIN, 0, 5, 6, vec![])]);
        touch(&rec, "down", 1, (20, 30), true);
        touch(&rec, "motion", 0, (9, 10), true);
        queued(d);
        let got = touches(&settle_texts(d));
        assert_eq!(
            got,
            [
                (NHG_TOUCH_BEGIN, 1, 20, 30, vec![]),
                (NHG_TOUCH_UPDATE, 0, 0, 0, vec![(9, 10, 0), (20, 30, 1)]),
            ]
        );
        touch(&rec, "up", 0, (0, 0), true);
        queued(d);
        let seen = settle_texts(d);
        assert_eq!(touches(&seen), [(NHG_TOUCH_END, 0, 9, 10, vec![])]);
        assert!(seen.iter().all(|(a, _)| a.window == id));
        close(d);
    }

    /// pgtk's rule: a motion that rounds to where the point already is gives nothing.
    #[test]
    fn motion_without_a_change_gives_nothing() {
        let rec = Recorder::start("sway");
        let d = open(&rec);
        ready(d);
        touch(&rec, "down", 0, (5, 6), true);
        queued(d);
        settle_texts(d);
        touch(&rec, "motion", 0, (5, 6), true);
        queued(d);
        assert!(touches(&settle_texts(d)).is_empty());
        close(d);
    }

    /// A cancel ends every live point, each marked cancelled; the ids are not reused by the next down.
    #[test]
    fn cancel_ends_all_points() {
        let rec = Recorder::start("sway");
        let d = open(&rec);
        ready(d);
        touch(&rec, "down", 0, (5, 6), true);
        touch(&rec, "down", 1, (7, 8), true);
        queued(d);
        settle_texts(d);
        // the recorder sends a cancel only while a point has an event its frame has not closed
        touch(&rec, "motion", 0, (5, 6), false);
        rec.control_ok(r#"{"cmd":"touch","op":"cancel"}"#);
        queued(d);
        let mut got = touches(&settle_texts(d));
        got.sort_by_key(|t| t.1);
        assert_eq!(
            got,
            [
                (NHG_TOUCH_CANCEL, 0, 5, 6, vec![]),
                (NHG_TOUCH_CANCEL, 1, 7, 8, vec![]),
            ]
        );
        touch(&rec, "down", 0, (1, 1), true);
        queued(d);
        assert_eq!(
            touches(&settle_texts(d)),
            [(NHG_TOUCH_BEGIN, 2, 1, 1, vec![])]
        );
        close(d);
    }

    /// A forgotten window's points go without an action, and its later records give none.
    #[test]
    fn forgotten_window_drops_its_points() {
        let rec = Recorder::start("sway");
        let d = open(&rec);
        let id = ready(d);
        touch(&rec, "down", 0, (5, 6), true);
        queued(d);
        settle_texts(d);
        // SAFETY: a live display.
        assert_eq!(unsafe { nhg_frame_forget(d, id) }, 0);
        touch(&rec, "motion", 0, (9, 9), true);
        touch(&rec, "up", 0, (0, 0), true);
        queued(d);
        assert!(touches(&settle_texts(d)).is_empty());
        close(d);
    }
}

mod pinch {
    use super::*;

    /// Begin and update give a PINCH with the deltas, the cumulative scale, the position and the angle in radians; the
    /// end gives nothing.
    #[test]
    fn begin_and_update_only() {
        let rec = Recorder::start("sway");
        let d = open(&rec);
        let id = ready(d);
        rec.control_ok(r#"{"cmd":"focus","surface":"toplevel"}"#);
        rec.control_ok(r#"{"cmd":"pointer","pos":[4,4]}"#);
        queued(d);
        settle_texts(d);
        for req in [
            r#"{"cmd":"pinch","op":"begin","fingers":2}"#,
            r#"{"cmd":"pinch","op":"update","dx":1,"dy":2,"scale":1.5,"rotation":90}"#,
            r#"{"cmd":"pinch","op":"end"}"#,
        ] {
            rec.control_ok(req);
        }
        queued(d);
        let seen = settle_texts(d);
        let pinches: Vec<_> = seen
            .iter()
            .filter(|(a, _)| a.kind == NHG_ACT_PINCH)
            .collect();
        assert_eq!(pinches.len(), 2, "begin and update, not the end");
        let angle =
            |t: &Option<Vec<u8>>| f64::from_ne_bytes(t.as_ref().unwrap()[..8].try_into().unwrap());
        let (b, bt) = pinches[0];
        assert_eq!((b.window, b.dx, b.dy, b.scale), (id, 0.0, 0.0, 1.0));
        assert_eq!((b.x, b.y, angle(bt)), (4, 4, 0.0));
        let (u, ut) = pinches[1];
        assert_eq!((u.dx, u.dy, u.scale), (1.0, 2.0, 1.5));
        assert!((angle(ut) - std::f64::consts::FRAC_PI_2).abs() < 1e-9);
        close(d);
    }
}

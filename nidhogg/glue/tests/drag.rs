//! T1 tests of the drag source (`x-begin-drag`'s glue side) against nh-recorder as the foreign target (`drag_target`,
//! `drag_drop`, `drag_cancel`, `drag_log`) and over our own window.  Each test stands in for C: it starts the drag
//! with `nhg_drag_start` after a press and reads the `NHG_ACT_DRAG_*` actions `nhg_next` gives.  The harnesses are the
//! shell's (`../shell/tests/common`) and the step's (`step/mod.rs`).

#[path = "../../shell/tests/common/mod.rs"]
mod common;
mod step;

use common::Recorder;
use nhglue::abi::*;
use nhglue::action::*;
use nhglue::dnd::*;
use nhglue::ffi::*;
use nhshell::api::{NHSHELL_DND_COPY, NHSHELL_DND_MOVE, NHSHELL_SEL_DND};
use std::ffi::c_char;
use std::time::{Duration, Instant};
use step::*;

const TEXT: &std::ffi::CStr = c"text/plain;charset=utf-8";

/// A window with the keyboard and the pointer over it, its first configure applied.
fn ready(rec: &Recorder, d: *mut NhgDisplay) -> u64 {
    let id = window(d);
    // SAFETY: a live display.
    assert_eq!(unsafe { nhg_frame_units(d, id, 10, 10) }, NHG_OK);
    configured(d, id);
    rec.control_ok(r#"{"cmd":"focus","surface":"toplevel"}"#);
    rec.control_ok(r#"{"cmd":"pointer","pos":[10,10]}"#);
    settle(d);
    id
}

/// The recorder presses the left button over the window, and the glue has stepped past it.
fn press(rec: &Recorder, d: *mut NhgDisplay) {
    rec.control_ok(r#"{"cmd":"button","button":272,"state":"press"}"#);
    queued(d);
    settle(d);
}

/// `nhg_drag_start` of `bytes` as `TEXT` out of `id`.
fn start(
    d: *mut NhgDisplay,
    id: u64,
    bytes: &[u8],
    actions: u32,
    preferred: u32,
    flags: u32,
) -> i32 {
    let mimes: [*const c_char; 1] = [TEXT.as_ptr()];
    let data: [*const u8; 1] = [bytes.as_ptr()];
    let lens: [usize; 1] = [bytes.len()];
    // SAFETY: a live display; one entry each, alive for the call.
    unsafe {
        nhg_drag_start(
            d,
            id,
            mimes.as_ptr(),
            data.as_ptr(),
            lens.as_ptr(),
            1,
            actions,
            preferred,
            flags,
        )
    }
}

/// Press and start a copy drag of "dragged" out of `id`; the recorder has seen it start.
fn started(rec: &Recorder, d: *mut NhgDisplay, id: u64, flags: u32) {
    press(rec, d);
    assert_eq!(
        start(d, id, b"dragged", NHSHELL_DND_COPY, NHSHELL_DND_COPY, flags),
        NHG_OK
    );
    wait_reply(rec, r#"{"cmd":"drag_log"}"#, &["\"result\":\"started\""]);
}

/// The recorder's reply to `req` once it contains every one of `want`, within 3 s.
fn wait_reply(rec: &Recorder, req: &str, want: &[&str]) -> String {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        let r = rec.control(req);
        if want.iter().all(|w| r.contains(w)) {
            return r;
        }
        assert!(Instant::now() < deadline, "{req}: want {want:?}, last {r}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Step until the drag's DONE (within 5 s); every action seen, DONE last.
fn until_done(d: *mut NhgDisplay) -> Vec<NhgAction> {
    until(d, |a| a.kind == NHG_ACT_DRAG_DONE)
}

fn kinds(v: &[NhgAction]) -> Vec<u32> {
    v.iter().map(|a| a.kind).collect()
}

fn dragging(d: *mut NhgDisplay) -> bool {
    // SAFETY: a live display.
    let d = unsafe { &*d };
    nhglue::dnd::dragging(&d.lock_for_test())
}

fn cancel(d: *mut NhgDisplay) -> i32 {
    // SAFETY: a live display.
    unsafe { nhg_drag_cancel(d) }
}

const ACCEPT_COPY: &str =
    r#"{"cmd":"drag_target","accept":"text/plain;charset=utf-8","action":"copy"}"#;
const ACCEPT_MOVE: &str =
    r#"{"cmd":"drag_target","accept":"text/plain;charset=utf-8","action":"move"}"#;

#[allow(clippy::module_inception)]
mod drag {
    use super::*;

    #[test]
    fn without_a_held_press_nothing_starts() {
        let rec = Recorder::start("mutter");
        let d = open(&rec);
        let id = ready(&rec, d);
        let copy = NHSHELL_DND_COPY;
        assert_eq!(start(d, id, b"x", copy, copy, 0), NHG_E_STATE);
        press(&rec, d);
        rec.control_ok(r#"{"cmd":"button","button":272,"state":"release"}"#);
        queued(d);
        settle(d);
        assert_eq!(start(d, id, b"x", copy, copy, 0), NHG_E_STATE, "released");
        press(&rec, d);
        let move_ = NHSHELL_DND_MOVE;
        assert_eq!(start(d, id, b"x", copy, move_, 0), NHG_E_ARG, "not offered");
        assert_eq!(start(d, id + 1000, b"x", copy, copy, 0), NHG_E_STATE);
        assert!(!dragging(d));
        assert_eq!(cancel(d), 0);
        close(d);
    }

    #[test]
    fn a_target_that_finishes_gives_its_action() {
        let rec = Recorder::start("mutter");
        let d = open(&rec);
        let id = ready(&rec, d);
        press(&rec, d);
        let both = NHSHELL_DND_COPY | NHSHELL_DND_MOVE;
        assert_eq!(start(d, id, b"dragged", both, NHSHELL_DND_MOVE, 0), NHG_OK);
        wait_reply(&rec, r#"{"cmd":"drag_log"}"#, &["\"result\":\"started\""]);
        rec.control_ok(ACCEPT_MOVE);
        let r = rec.control(r#"{"cmd":"drag_drop","finish":true}"#);
        assert!(r.contains("\"data\":\"dragged\""), "{r}");
        let seen = until_done(d);
        let k = kinds(&seen);
        let at = |kind| {
            k.iter()
                .position(|&x| x == kind)
                .unwrap_or_else(|| panic!("no {kind} in {k:?}"))
        };
        assert!(at(NHG_ACT_DRAG_ACTION) < at(NHG_ACT_DRAG_DROPPED), "{k:?}");
        let done = seen.last().unwrap();
        let got = (done.window, done.code as u32, done.flags);
        assert_eq!(got, (id, NHSHELL_DND_MOVE, 0));
        assert!(!dragging(d));
        assert_eq!(cancel(d), 0, "nothing left to give up");
        close(d);
    }

    #[test]
    fn a_target_that_never_finishes_leaves_it_to_c() {
        let rec = Recorder::start("mutter");
        let d = open(&rec);
        let id = ready(&rec, d);
        started(&rec, d, id, 0);
        rec.control_ok(ACCEPT_COPY);
        rec.control(r#"{"cmd":"drag_drop","finish":false}"#);
        let k = kinds(&until(d, |a| a.kind == NHG_ACT_DRAG_DROPPED));
        assert!(!k.contains(&NHG_ACT_DRAG_DONE), "{k:?}");
        // C's timeout gives it up: nothing follows
        assert_eq!(cancel(d), 1);
        assert!(!dragging(d));
        assert!(!kinds(&settle(d)).contains(&NHG_ACT_DRAG_DONE));
        close(d);
    }

    #[test]
    fn a_cancelled_drag_ends_with_no_action() {
        let rec = Recorder::start("mutter");
        let d = open(&rec);
        let id = ready(&rec, d);
        started(&rec, d, id, 0);
        rec.control_ok(r#"{"cmd":"drag_cancel"}"#);
        let done = *until_done(d).last().unwrap();
        assert_eq!((done.code, done.flags), (0, 0));
        assert!(!kinds(&settle(d)).contains(&NHG_ACT_DRAG_DONE), "one DONE");
        close(d);
    }

    #[test]
    fn the_grab_taking_the_pointer_says_the_drag_began() {
        let rec = Recorder::start("mutter");
        let d = open(&rec);
        let id = ready(&rec, d);
        started(&rec, d, id, 0);
        // the drag's grab took the pointer from the window
        let seen = until(d, |a| a.kind == NHG_ACT_DRAG_ACTION);
        assert_eq!(seen.last().unwrap().code, 0);
        assert!(dragging(d));
        assert_eq!(cancel(d), 1);
        close(d);
    }

    #[test]
    fn a_drop_on_our_own_window_goes_to_lisp() {
        let rec = Recorder::start("mutter");
        let d = open(&rec);
        let id = ready(&rec, d);
        started(&rec, d, id, NHG_DRAG_ALLOW_CURRENT);
        // the drag moves over our own window: the glue answers it, Lisp hears nothing yet
        rec.control_ok(r#"{"cmd":"pointer","pos":[30,30]}"#);
        let copy = NHSHELL_DND_COPY as i32;
        let k = kinds(&until(d, |a| {
            a.kind == NHG_ACT_DRAG_ACTION && a.code == copy
        }));
        assert!(!k.contains(&NHG_ACT_DND_ENTER), "{k:?}");
        rec.control_ok(r#"{"cmd":"button","button":272,"state":"release"}"#);
        let mut seen = until_done(d);
        seen.extend(settle(d));
        let k = kinds(&seen);
        let at = |kind| {
            k.iter()
                .position(|&x| x == kind)
                .unwrap_or_else(|| panic!("no {kind} in {k:?}"))
        };
        let drop = at(NHG_ACT_DND_DROP);
        assert!(at(NHG_ACT_DND_ENTER) < drop, "{k:?}");
        assert_eq!((seen[drop].window, seen[drop].code), (id, copy));
        let done = seen[at(NHG_ACT_DRAG_DONE)];
        assert_eq!((done.code, done.flags), (copy, 0));
        let dones = k.iter().filter(|&&x| x == NHG_ACT_DRAG_DONE).count();
        assert_eq!(dones, 1, "{k:?}");
        // pgtk-dnd.el reads the drag's own bytes after the offer was finished
        let (mut out, mut len) = (std::ptr::null(), 0usize);
        let mime = TEXT.as_ptr();
        // SAFETY: a live display; `out` and `len` are writable.
        let rc = unsafe { nhg_selection_read(d, NHSHELL_SEL_DND, mime, 0, &mut out, &mut len) };
        assert_eq!(rc, NHG_OK);
        // SAFETY: the glue lends `len` bytes until its next read.
        assert_eq!(unsafe { std::slice::from_raw_parts(out, len) }, b"dragged");
        close(d);
    }

    #[test]
    fn our_start_window_refuses_without_allow_current() {
        let rec = Recorder::start("mutter");
        let d = open(&rec);
        let id = ready(&rec, d);
        started(&rec, d, id, 0);
        rec.control_ok(r#"{"cmd":"pointer","pos":[30,30]}"#);
        until(d, |a| a.kind == NHG_ACT_DRAG_ACTION);
        settle(d);
        rec.control_ok(r#"{"cmd":"button","button":272,"state":"release"}"#);
        let mut seen = until_done(d);
        seen.extend(settle(d));
        let k = kinds(&seen);
        assert!(!k.contains(&NHG_ACT_DND_DROP), "{k:?}");
        let done = seen.iter().find(|a| a.kind == NHG_ACT_DRAG_DONE);
        assert_eq!(done.map(|a| a.code), Some(0));
        close(d);
    }

    #[test]
    fn return_frame_now_gives_up_for_our_window() {
        let rec = Recorder::start("mutter");
        let d = open(&rec);
        let id = ready(&rec, d);
        started(&rec, d, id, NHG_DRAG_RETURN_NOW);
        rec.control_ok(r#"{"cmd":"pointer","pos":[30,30]}"#);
        let done = *until_done(d).last().unwrap();
        let got = (done.window, done.flags, done.code);
        assert_eq!(got, (id, NHG_DRAG_FRAME, 0));
        assert!(!dragging(d));
        // given up: the source went, and nothing more comes
        assert!(!kinds(&settle(d)).contains(&NHG_ACT_DRAG_DONE));
        wait_reply(&rec, r#"{"cmd":"drag_log"}"#, &["\"result\":\"cancelled\""]);
        close(d);
    }

    #[test]
    fn return_frame_waits_until_the_pointer_left() {
        let rec = Recorder::start("mutter");
        let d = open(&rec);
        let id = ready(&rec, d);
        started(&rec, d, id, NHG_DRAG_RETURN_FRAME);
        // over the start window: still dragging
        rec.control_ok(r#"{"cmd":"pointer","pos":[30,30]}"#);
        until(d, |a| a.kind == NHG_ACT_DRAG_ACTION);
        assert!(!kinds(&settle(d)).contains(&NHG_ACT_DRAG_DONE));
        assert!(dragging(d));
        // out and back in
        rec.control_ok(r#"{"cmd":"pointer","pos":[900,900]}"#);
        settle(d);
        rec.control_ok(r#"{"cmd":"pointer","pos":[40,40]}"#);
        let done = *until_done(d).last().unwrap();
        assert_eq!((done.window, done.flags), (id, NHG_DRAG_FRAME));
        close(d);
    }

    #[test]
    fn a_lost_display_ends_the_drag_first() {
        let rec = Recorder::start("mutter");
        let d = open(&rec);
        let id = ready(&rec, d);
        started(&rec, d, id, 0);
        assert!(rec.control(r#"{"cmd":"quit"}"#).contains("\"ok\":true"));
        let seen = until(d, |a| a.kind == NHG_ACT_LOST);
        let k = kinds(&seen);
        let done = k
            .iter()
            .position(|&x| x == NHG_ACT_DRAG_DONE)
            .expect("DONE before LOST");
        assert!(done < k.len() - 1, "{k:?}");
        assert_eq!(seen[done].code, 0);
        close(d);
    }
}

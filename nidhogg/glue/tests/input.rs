//! T1 tests of the pointer, the wheel, the input method and the outputs, against nh-recorder (the shell's harness,
//! `../shell/tests/common`, and the step's, `step/mod.rs`).  Each test stands in for C: it makes the window as
//! `nhwl_window_new` does, pushes the facts C pushes (`nhg_frame_units`) and reads the actions `nhg_next` gives.

#[path = "../../shell/tests/common/mod.rs"]
mod common;
mod step;

use common::{Recorder, readable_within};
use nhglue::abi::*;
use nhglue::action::*;
use nhglue::ffi::*;
use nhglue::query::NhgMonitor;
use nhglue::scroll::*;
use nhshell::event::NHSHELL_BUTTON_PRESSED;
use std::ptr;
use std::time::{Duration, Instant};
use step::*;

/// Step until nothing comes for 200 ms, each step's `flags` as C samples them; every action seen.
fn drain_with(d: *mut NhgDisplay, flags: u32) -> Vec<NhgAction> {
    let mut seen = Vec::new();
    loop {
        let (rc, v) = step(d, flags);
        assert!(rc >= 0, "nhg_next failed: {rc}");
        seen.extend(v);
        // SAFETY: a live display.
        if rc == 0 && !readable_within(unsafe { nhg_fd(d) }, 200) {
            return seen;
        }
    }
}

/// As `drain_with`, with each action's text (copied before the next step frees it).
fn drain_texts(d: *mut NhgDisplay) -> Vec<(NhgAction, Option<Vec<u8>>)> {
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

/// A window with the keyboard and the pointer over it, its first configure applied, and a font of 10 x 10 pixels.
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

fn axis(rec: &Recorder, source: &str, v120: i32) {
    rec.control_ok(&format!(
        r#"{{"cmd":"axis","source":"{source}","v120":{v120}}}"#
    ));
}

fn wheels(v: &[NhgAction]) -> Vec<NhgAction> {
    v.iter()
        .copied()
        .filter(|a| a.kind == NHG_ACT_WHEEL)
        .collect()
}

mod pointer {
    use super::*;

    /// The position is the shell's, in device pixels of the window's surface: at a scale of 1.5 the compositor's
    /// (100, 50) is (150, 75).  (The plan says "native-area px"; the title bar's offset is C's until the title bar
    /// moves, so the action carries the surface position.)
    #[test]
    fn motion_in_device_px() {
        let rec = Recorder::start("mutter");
        let d = open(&rec);
        let id = ready(&rec, d);
        rec.control_ok(r#"{"cmd":"scale","value120":180}"#);
        until(d, |a| a.kind == NHG_ACT_SCALE && a.window == id);
        rec.control_ok(r#"{"cmd":"pointer","pos":[100,50]}"#);
        queued(d);
        let seen = settle(d);
        let m = seen
            .iter()
            .rev()
            .find(|a| a.kind == NHG_ACT_MOTION)
            .expect("a MOTION action");
        assert_eq!((m.window, m.dx, m.dy), (id, 150.0, 75.0));
        close(d);
    }

    /// A press is a `CLICK` with the evdev button and the state, a leave a `LEAVE`, and neither comes for a window the
    /// glue forgot.
    #[test]
    fn click_and_leave() {
        let rec = Recorder::start("mutter");
        let d = open(&rec);
        let id = ready(&rec, d);
        rec.control_ok(r#"{"cmd":"button","button":272,"state":"press"}"#);
        queued(d);
        let seen = settle(d);
        let c = seen
            .iter()
            .find(|a| a.kind == NHG_ACT_CLICK)
            .expect("a CLICK action");
        assert_eq!(
            (c.window, c.code, c.flags, c.mods),
            (id, 272, NHSHELL_BUTTON_PRESSED, 0)
        );
        // SAFETY: a live display.
        assert_eq!(unsafe { nhg_frame_forget(d, id) }, NHG_OK);
        rec.control_ok(r#"{"cmd":"button","button":272,"state":"release"}"#);
        queued(d);
        assert!(
            settle(d)
                .iter()
                .all(|a| a.window != id || a.kind == NHG_ACT_KEY),
            "an action for a forgotten window"
        );
        close(d);
    }
}

mod scroll {
    use super::*;

    /// Wheel records of 60 units add up: nothing for the first, one step (down, `code` 5) for the second, and the
    /// remainder carries on.
    #[test]
    fn high_resolution_accumulates_to_steps() {
        let rec = Recorder::start("mutter");
        let d = open(&rec);
        let id = ready(&rec, d);
        axis(&rec, "wheel", 60);
        queued(d);
        assert!(wheels(&settle(d)).is_empty(), "half a notch is no step");
        axis(&rec, "wheel", 60);
        queued(d);
        let w = wheels(&settle(d));
        assert_eq!(w.len(), 1);
        assert_eq!(
            (w[0].window, w[0].code, w[0].w, w[0].flags),
            (id, NHG_WHEEL_DOWN, 1, 0)
        );
        // three records of 40: the third completes a notch
        for _ in 0..2 {
            axis(&rec, "wheel", 40);
        }
        queued(d);
        assert!(wheels(&settle(d)).is_empty());
        axis(&rec, "wheel", 40);
        queued(d);
        assert_eq!(wheels(&settle(d)).iter().map(|a| a.w).sum::<i32>(), 1);
        // the other way round: a reversal drops the fraction
        axis(&rec, "wheel", -120);
        queued(d);
        let w = wheels(&settle(d));
        assert_eq!((w.len(), w[0].code, w[0].w), (1, NHG_WHEEL_UP, 1));
        close(d);
    }

    /// A record of another source starts the accumulator over: half a notch of wheel, then ten pixels of finger at a
    /// line height of 10 is one line, not seven.
    #[test]
    fn source_change_resets_the_accumulator() {
        let rec = Recorder::start("mutter");
        let d = open(&rec);
        ready(&rec, d);
        axis(&rec, "wheel", 60);
        queued(d);
        settle(d);
        axis(&rec, "finger", 80);
        queued(d);
        let w = wheels(&drain_with(d, NHG_IN_COALESCE_WHEEL));
        assert_eq!(w.len(), 1);
        assert_eq!((w[0].w, w[0].flags), (1, NHG_WHEEL_COALESCED));
        close(d);
    }

    /// With the flag, finger records add up to whole lines in one event; without it each is its own pixel delta.
    #[test]
    fn coalesce_flag_respected() {
        let rec = Recorder::start("mutter");
        let d = open(&rec);
        ready(&rec, d);
        axis(&rec, "finger", 160);
        queued(d);
        let w = wheels(&drain_with(d, NHG_IN_COALESCE_WHEEL));
        assert_eq!(w.len(), 1);
        assert_eq!(
            (w[0].w, w[0].flags, w[0].dy),
            (2, NHG_WHEEL_COALESCED, 20.0)
        );
        axis(&rec, "finger", 160);
        queued(d);
        let w = wheels(&drain_with(d, 0));
        assert_eq!(w.len(), 1);
        assert_eq!((w[0].flags, w[0].dy), (NHG_WHEEL_HIRES, 20.0));
        // a stop ends the gesture: momentum begins, and the accumulator is clear
        rec.control_ok(r#"{"cmd":"axis","source":"finger","stop":true}"#);
        queued(d);
        let w = wheels(&settle(d));
        assert_eq!((w.len(), w[0].flags), (1, NHG_WHEEL_STOP));
        close(d);
    }

    /// The step of a finger record is the window's line height, as C pushed it.
    #[test]
    fn finger_steps_use_the_pushed_units() {
        let rec = Recorder::start("mutter");
        let d = open(&rec);
        let id = ready(&rec, d);
        // SAFETY: a live display.
        unsafe {
            assert_eq!(nhg_frame_units(d, id, 10, 20), NHG_OK);
            assert_eq!(nhg_frame_units(d, id, 0, 20), NHG_E_ARG);
            assert_eq!(nhg_frame_units(d, id + 1000, 10, 20), NHG_E_STATE);
        }
        axis(&rec, "finger", 160);
        queued(d);
        let w = wheels(&drain_with(d, NHG_IN_COALESCE_WHEEL));
        assert_eq!(
            (w.len(), w[0].w),
            (1, 1),
            "20 pixels at a line height of 20"
        );
        close(d);
    }
}

/// The count of `req` requests in the recorder's input log.
fn logged(rec: &Recorder, req: &str) -> usize {
    rec.control(r#"{"cmd":"input_log","since":0}"#)
        .matches(&format!("\"req\":\"{req}\""))
        .count()
}

/// Wait (3 s at most) until the log holds `n` of `req`, then a little more; the count.
fn logged_after(rec: &Recorder, req: &str, n: usize) -> usize {
    let deadline = Instant::now() + Duration::from_secs(3);
    while logged(rec, req) < n && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    std::thread::sleep(Duration::from_millis(200));
    logged(rec, req)
}

/// A window with text input on and the keyboard; the compositor has entered the text input.
fn typing(rec: &Recorder, d: *mut NhgDisplay) -> u64 {
    let id = window(d);
    configured(d, id);
    // SAFETY: a live display.
    assert_eq!(unsafe { nhg_im_enable(d, id, 1) }, NHG_OK);
    rec.control_ok(r#"{"cmd":"focus","surface":"toplevel"}"#);
    settle(d);
    assert!(
        logged_after(rec, "text_input.commit", 1) >= 1,
        "text input never entered"
    );
    id
}

mod ime {
    use super::*;

    /// The compositor's preedit, commit and the next preedit come in the protocol's order, with their texts; a
    /// preedit of nothing clears.
    #[test]
    fn preedit_then_commit() {
        let rec = Recorder::start("mutter");
        let d = open(&rec);
        let id = typing(&rec, d);
        rec.control_ok(r#"{"cmd":"text_input","preedit":"ab","cursor":[0,1]}"#);
        queued(d);
        let seen = drain_texts(d);
        assert_eq!(seen.len(), 1);
        assert_eq!((seen[0].0.kind, seen[0].0.window), (NHG_ACT_PREEDIT, id));
        assert_eq!(seen[0].1.as_deref(), Some(&b"ab"[..]));
        rec.control_ok(r#"{"cmd":"text_input","preedit":"cd","cursor":[2,2],"commit":"y"}"#);
        queued(d);
        let got: Vec<_> = drain_texts(d)
            .into_iter()
            .map(|(a, t)| (a.kind, t.unwrap_or_default()))
            .collect();
        assert_eq!(
            got,
            [
                (NHG_ACT_PREEDIT, Vec::new()),
                (NHG_ACT_COMMIT, b"y".to_vec()),
                (NHG_ACT_PREEDIT, b"cd".to_vec()),
            ]
        );
        // the bytes around point the compositor asks to delete
        rec.control_ok(r#"{"cmd":"text_input","delete":[3,1]}"#);
        queued(d);
        let del: Vec<_> = drain_texts(d)
            .into_iter()
            .filter(|(a, _)| a.kind == NHG_ACT_IM_DELETE)
            .collect();
        assert_eq!(del.len(), 1);
        assert_eq!((del[0].0.x, del[0].0.y), (3, 1));
        close(d);
    }

    /// The same rectangle twice is one request to the compositor; another rectangle is a second.
    #[test]
    fn cursor_rect_deduplicated() {
        let rec = Recorder::start("mutter");
        let d = open(&rec);
        let id = typing(&rec, d);
        let before = logged(&rec, "text_input.set_cursor_rectangle");
        let r = nhglue::backing::NhgRect {
            x: 10,
            y: 20,
            w: 3,
            h: 4,
        };
        let r2 = nhglue::backing::NhgRect {
            x: 5,
            y: 6,
            w: 7,
            h: 8,
        };
        // SAFETY: a live display.
        unsafe {
            assert_eq!(nhg_im_cursor(d, id, r), NHG_OK);
            assert_eq!(nhg_im_cursor(d, id, r), NHG_OK);
            assert_eq!(nhg_im_cursor(d, id, r2), NHG_OK);
        }
        backend(d).dispatch_main().unwrap();
        assert_eq!(
            logged_after(&rec, "text_input.set_cursor_rectangle", before + 2),
            before + 2
        );
        close(d);
    }

    /// The text around point is sent once for the same text; other text is sent again, and a refused one (not UTF-8)
    /// is `NHG_E_ARG`.
    #[test]
    fn surrounding_request_once_per_enable() {
        let rec = Recorder::start("mutter");
        let d = open(&rec);
        let id = typing(&rec, d);
        let before = logged(&rec, "text_input.set_surrounding_text");
        let text = c"hello";
        // SAFETY: a live display; the text holds 5 readable bytes.
        unsafe {
            for _ in 0..2 {
                assert_eq!(nhg_im_surrounding(d, id, text.as_ptr(), 5, 5, 5), NHG_OK);
            }
            assert_eq!(
                nhg_im_surrounding(d, id, text.as_ptr(), 5, -1, 5),
                NHG_E_ARG
            );
            let bad = [0xffu8, 0xfe];
            assert_eq!(
                nhg_im_surrounding(d, id, bad.as_ptr().cast(), 2, 0, 0),
                NHG_E_ARG
            );
        }
        backend(d).dispatch_main().unwrap();
        assert_eq!(
            logged_after(&rec, "text_input.set_surrounding_text", before + 1),
            before + 1
        );
        // SAFETY: a live display; the text holds 3 readable bytes.
        unsafe {
            assert_eq!(nhg_im_surrounding(d, id, text.as_ptr(), 3, 3, 3), NHG_OK);
        }
        backend(d).dispatch_main().unwrap();
        assert_eq!(
            logged_after(&rec, "text_input.set_surrounding_text", before + 2),
            before + 2
        );
        close(d);
    }
}

fn list_monitors(d: *mut NhgDisplay) -> Vec<NhgMonitor> {
    // SAFETY: an all-zero NhgMonitor is valid (integers and a byte array).
    let mut buf: [NhgMonitor; 16] = unsafe { std::mem::zeroed() };
    let mut n = 0usize;
    // SAFETY: a live display; `buf` holds 16 writable monitors and `n` is writable.
    let rc = unsafe { nhg_monitors(d, buf.as_mut_ptr(), buf.len(), &mut n) };
    assert_eq!(rc as usize, n);
    buf[..n].to_vec()
}

fn name_of(m: &NhgMonitor) -> String {
    let bytes: Vec<u8> = m
        .name
        .iter()
        .take_while(|&&c| c != 0)
        .map(|&c| c as u8)
        .collect();
    String::from_utf8_lossy(&bytes).into_owned()
}

mod monitors {
    use super::*;

    /// The list follows the shell's output records: the compositor's own output at the start, another when it is
    /// added (one `MONITORS` action carrying its scale), and gone when it is removed; and a window's outputs follow
    /// its enter and leave records, and the output's removal.
    #[test]
    fn list_follows_outputs() {
        let rec = Recorder::start("mutter");
        let d = open(&rec);
        assert!(list_monitors(d).is_empty(), "nothing before the first step");
        let id = ready(&rec, d);
        let first = list_monitors(d);
        assert!(!first.is_empty(), "the compositor's output");
        let first_name = name_of(&first[0]);

        rec.control_ok(
            r#"{"cmd":"output","op":"add","name":"nh-extra","size":[800,600],"scale120":240}"#,
        );
        backend(d).dispatch_main().unwrap();
        queued(d);
        let seen = settle(d);
        let changed: Vec<_> = seen.iter().filter(|a| a.kind == NHG_ACT_MONITORS).collect();
        assert!(!changed.is_empty());
        assert!(changed.iter().any(|a| a.code == 240), "{changed:?}");
        let now = list_monitors(d);
        assert_eq!(now.len(), first.len() + 1);
        let extra = now
            .iter()
            .find(|m| name_of(m) == "nh-extra")
            .expect("the new output");
        assert_eq!((extra.w, extra.h, extra.scale120), (800, 600, 240));

        // the window enters the output, then it goes
        rec.control_ok(&format!(
            r#"{{"cmd":"output","op":"enter","name":"{first_name}","surface":{id}}}"#
        ));
        backend(d).dispatch_main().unwrap();
        queued(d);
        settle(d);
        let outputs = |d| {
            let mut ids = [0u64; 16];
            let mut n = 0usize;
            // SAFETY: a live display; `ids` holds 16 writable ids and `n` is writable.
            let rc = unsafe { nhg_frame_outputs(d, id, ids.as_mut_ptr(), ids.len(), &mut n) };
            assert_eq!(rc as usize, n);
            ids[..n].to_vec()
        };
        assert_eq!(outputs(d), [first[0].id]);
        rec.control_ok(&format!(
            r#"{{"cmd":"output","op":"remove","name":"{first_name}"}}"#
        ));
        backend(d).dispatch_main().unwrap();
        queued(d);
        let seen = settle(d);
        assert!(
            seen.iter()
                .any(|a| a.kind == NHG_ACT_MONITORS && a.code == 0)
        );
        assert_eq!(list_monitors(d).len(), first.len(), "one left, one added");
        assert!(outputs(d).is_empty(), "the output the window was on went");
        // SAFETY: a live display.
        assert_eq!(
            unsafe { nhg_frame_outputs(d, id + 1000, ptr::null_mut(), 0, ptr::null_mut()) },
            NHG_E_STATE
        );
        close(d);
    }
}

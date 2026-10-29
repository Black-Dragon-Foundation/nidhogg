//! T1 tests of the drop target against nh-recorder as a foreign source (`dnd_offer`, `dnd_motion`, `dnd_drop`,
//! read back through `dnd_log`).  The tests run one at a time (`SERIAL`).

#[path = "../../shell/tests/common/mod.rs"]
mod common;

use common::{Recorder, readable_within};
use nhglue::abi::*;
use nhglue::action::*;
use nhglue::ffi::*;
use nhshell::api::{NHSHELL_DND_COPY, NHSHELL_SEL_DND, ShellBackend, WindowDesc};
use nhshell::event::{Record, ShellEvent};
use std::ffi::c_char;
use std::ptr;
use std::sync::Mutex;
use std::time::{Duration, Instant};

static SERIAL: Mutex<()> = Mutex::new(());

const TEXT: &str = "text/plain";

/// A DND action as the tests see it.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Act {
    kind: u32,
    window: u64,
    dx: f64,
    dy: f64,
    code: i32,
    time_ms: u64,
}

fn open(rec: &Recorder) -> *mut NhgDisplay {
    let name = rec.display();
    // SAFETY: the name is NUL-terminated; no error buffer.
    let d = unsafe { nhg_open(name.as_ptr(), ptr::null_mut(), 0) };
    assert!(!d.is_null(), "nhg_open failed on the recorder");
    d
}

/// The shell inside `d`, for what the glue does not wrap (windows, records).
fn shell<'a>(d: *mut NhgDisplay) -> &'a mut dyn ShellBackend {
    // SAFETY: a live display's backend, used only between glue calls.
    unsafe { &mut *(*d).backend_ptr() }
}

/// Take the shell's records until one satisfies `want` (3 s at most).
fn wait_record(d: *mut NhgDisplay, mut want: impl FnMut(&Record) -> bool) -> bool {
    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline {
        readable_within(shell(d).wake_fd(), 20);
        while let Ok(Some(ev)) = shell(d).next_event() {
            if want(&ev) {
                return true;
            }
        }
    }
    false
}

/// A registered window with the keyboard focus.
fn window(rec: &Recorder, d: *mut NhgDisplay) -> u64 {
    let id = shell(d)
        .window_new(&WindowDesc {
            w: 200,
            h: 150,
            title: "dnd".into(),
            app_id: "dnd".into(),
            flags: 0,
        })
        .unwrap();
    // SAFETY: a live display.
    assert_eq!(unsafe { nhg_frame_register(d, id, 0) }, NHG_OK);
    let reply = rec.control_ok(&format!(r#"{{"cmd":"focus","surface":{id}}}"#));
    assert!(reply.contains("\"ok\":true"), "{reply}");
    assert!(
        wait_record(
            d,
            |e| matches!(e.event, ShellEvent::Focus(f) if f.r#in == 1)
        ),
        "keyboard focus"
    );
    id
}

/// Step the glue's drain until `n` drop-target actions came or `ms` passed.
fn actions(d: *mut NhgDisplay, n: usize, ms: u64) -> Vec<Act> {
    let mut seen = Vec::new();
    let deadline = Instant::now() + Duration::from_millis(ms);
    while seen.len() < n && Instant::now() < deadline {
        // SAFETY: a live display.
        readable_within(unsafe { nhg_fd(d) }, 20);
        loop {
            let mut list = NhgActionList {
                v: ptr::null(),
                n: 0,
            };
            // SAFETY: a live display; `list` is writable.
            if unsafe { nhg_next(d, &NhgNextIn::default(), &mut list) } <= 0 {
                break;
            }
            for i in 0..list.n {
                // SAFETY: the list holds `n` actions until the next step.
                let a = unsafe { &*list.v.add(i) };
                if (NHG_ACT_DND_ENTER..=NHG_ACT_DND_DROP).contains(&a.kind) {
                    seen.push(Act {
                        kind: a.kind,
                        window: a.window,
                        dx: a.dx,
                        dy: a.dy,
                        code: a.code,
                        time_ms: a.time_ms,
                    });
                }
            }
        }
    }
    seen
}

/// The recorder's `dnd_log` once it contains every one of `want`, within 3 s.
fn wait_log(rec: &Recorder, want: &[&str]) -> String {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        let r = rec.control(r#"{"cmd":"dnd_log"}"#);
        if want.iter().all(|w| r.contains(w)) {
            return r;
        }
        assert!(Instant::now() < deadline, "want {want:?}, last {r}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// A foreign source enters at (10, 12) offering `types`.
fn offer(rec: &Recorder, types: &[(&str, &str)]) {
    let types: Vec<String> = types
        .iter()
        .map(|(m, t)| format!(r#""{m}":"{t}""#))
        .collect();
    let r = rec.control(&format!(
        r#"{{"cmd":"dnd_offer","types":{{{}}},"actions":["copy","move"],"preferred":"copy","surface":"toplevel","at":[10,12]}}"#,
        types.join(",")
    ));
    assert!(r.contains("\"ok\":true"), "{r}");
}

fn status(d: *mut NhgDisplay, action: u32, time: u32) -> i32 {
    // SAFETY: a live display.
    unsafe { nhg_dnd_status(d, action, time) }
}

fn finish(d: *mut NhgDisplay, ok: i32, time: u32) -> i32 {
    // SAFETY: a live display.
    unsafe { nhg_dnd_finish(d, ok, time) }
}

/// The offer's `mime`, read through the glue.
fn read(d: *mut NhgDisplay, mime: &std::ffi::CStr) -> Vec<u8> {
    let (mut out, mut len) = (ptr::null(), 0usize);
    // SAFETY: a live display; `mime` is NUL-terminated; `out` and `len` are writable.
    let rc =
        unsafe { nhg_selection_read(d, NHSHELL_SEL_DND, mime.as_ptr(), 1000, &mut out, &mut len) };
    assert_eq!(rc, NHG_OK);
    // SAFETY: the glue lends `len` bytes until its next read.
    unsafe { std::slice::from_raw_parts(out, len) }.to_vec()
}

fn register(d: *mut NhgDisplay, window: u64, mimes: &[&std::ffi::CStr]) -> i32 {
    let v: Vec<*const c_char> = mimes.iter().map(|m| m.as_ptr()).collect();
    // SAFETY: a live display; `v` holds NUL-terminated strings.
    unsafe { nhg_dnd_register(d, window, v.as_ptr(), v.len()) }
}

#[allow(clippy::module_inception)]
mod dnd {
    use super::*;

    #[test]
    fn enter_motion_and_drop_are_actions() {
        let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let rec = Recorder::start("mutter");
        let d = open(&rec);
        let w = window(&rec, d);
        offer(&rec, &[(TEXT, "dropped text")]);
        let got = actions(d, 1, 3000);
        assert_eq!(got.len(), 1, "{got:?}");
        let enter = got[0];
        assert_eq!(enter.kind, NHG_ACT_DND_ENTER);
        assert_eq!((enter.window, enter.dx, enter.dy), (w, 10.0, 12.0));
        assert_eq!(enter.code as u32, NHSHELL_DND_COPY);

        rec.control_ok(r#"{"cmd":"dnd_motion","at":[20,21]}"#);
        let got = actions(d, 1, 3000);
        assert_eq!(got.len(), 1, "{got:?}");
        assert_eq!(got[0].kind, NHG_ACT_DND_MOTION);
        assert_eq!((got[0].dx, got[0].dy), (20.0, 21.0));
        let time = got[0].time_ms as u32;

        assert_eq!(status(d, NHSHELL_DND_COPY, time), 1);
        wait_log(&rec, &["\"chosen\":\"copy\""]);
        rec.control_ok(r#"{"cmd":"dnd_drop"}"#);
        let got = actions(d, 1, 3000);
        assert_eq!(got.len(), 1, "{got:?}");
        assert_eq!(got[0].kind, NHG_ACT_DND_DROP);
        assert_eq!(got[0].code as u32, NHSHELL_DND_COPY);
        // the data stays readable until the finish
        assert_eq!(read(d, c"text/plain"), b"dropped text");
        assert_eq!(finish(d, 1, time), 1);
        wait_log(&rec, &["\"finished\":true", "\"destroyed\":true"]);
        // nothing left to finish
        assert_eq!(finish(d, 1, time), 0);
        // SAFETY: a live display.
        unsafe { nhg_close(d) };
    }

    #[test]
    fn stale_calls_are_ignored() {
        let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let rec = Recorder::start("mutter");
        let d = open(&rec);
        window(&rec, d);
        offer(&rec, &[(TEXT, "x")]);
        let enter = actions(d, 1, 3000)[0];
        let time = enter.time_ms as u32;
        // a call stamped before this drag's enter answers an earlier drag
        assert_eq!(status(d, NHSHELL_DND_COPY, time.wrapping_sub(5)), 0);
        // no drop yet: nothing to finish
        assert_eq!(finish(d, 1, time), 0);
        assert_eq!(status(d, NHSHELL_DND_COPY, time), 1);
        // SAFETY: a live display.
        unsafe { nhg_close(d) };
    }

    #[test]
    fn no_action_accepts_nothing() {
        let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let rec = Recorder::start("mutter");
        let d = open(&rec);
        window(&rec, d);
        offer(&rec, &[(TEXT, "x")]);
        let time = actions(d, 1, 3000)[0].time_ms as u32;
        assert_eq!(status(d, 0, time), 1);
        // the recorder lists every accept the offer saw
        let log = wait_log(&rec, &["\"accepted\":[null]"]);
        assert!(!log.contains("\"finished\":true"), "{log}");
        // SAFETY: a live display.
        unsafe { nhg_close(d) };
    }

    #[test]
    fn the_type_read_is_the_type_accepted() {
        let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let rec = Recorder::start("mutter");
        let d = open(&rec);
        window(&rec, d);
        offer(&rec, &[(TEXT, "text"), ("text/uri-list", "file:///tmp/a")]);
        let time = actions(d, 1, 3000)[0].time_ms as u32;
        assert_eq!(read(d, c"text/uri-list"), b"file:///tmp/a");
        assert_eq!(status(d, NHSHELL_DND_COPY, time), 1);
        wait_log(&rec, &["\"accepted\":[\"text/uri-list\"]"]);
        // SAFETY: a live display.
        unsafe { nhg_close(d) };
    }

    #[test]
    fn a_drag_none_of_whose_types_a_window_takes_is_refused() {
        let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let rec = Recorder::start("mutter");
        let d = open(&rec);
        let w = window(&rec, d);
        assert_eq!(register(d, w, &[c"text/uri-list", c"TEXT/PLAIN"]), NHG_OK);
        offer(&rec, &[("image/png", "PNG")]);
        assert!(
            actions(d, 1, 500).is_empty(),
            "no action for a refused drag"
        );
        let log = wait_log(&rec, &["\"accepted\":[null]"]);
        assert!(!log.contains("\"finished\":true"), "{log}");
        rec.control_ok(r#"{"cmd":"dnd_leave"}"#);
        // a type in the list (compared without case) goes through
        offer(&rec, &[(TEXT, "x")]);
        let got = actions(d, 1, 3000);
        assert_eq!(got.first().map(|a| a.kind), Some(NHG_ACT_DND_ENTER));
        // a window that is not a frame cannot register
        assert_eq!(register(d, w + 1000, &[c"x"]), NHG_E_STATE);
        // SAFETY: a live display.
        unsafe { nhg_close(d) };
    }

    #[test]
    fn a_drop_nobody_finished_is_finished_by_the_next_enter() {
        let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let rec = Recorder::start("mutter");
        let d = open(&rec);
        window(&rec, d);
        offer(&rec, &[(TEXT, "first")]);
        let time = actions(d, 1, 3000)[0].time_ms as u32;
        assert_eq!(status(d, NHSHELL_DND_COPY, time), 1);
        rec.control_ok(r#"{"cmd":"dnd_drop"}"#);
        assert_eq!(actions(d, 1, 3000)[0].kind, NHG_ACT_DND_DROP);
        // Lisp never finishes (a silent source); the next drag starts
        offer(&rec, &[(TEXT, "second")]);
        let got = actions(d, 1, 3000);
        assert_eq!(
            got.first().map(|a| a.kind),
            Some(NHG_ACT_DND_ENTER),
            "{got:?}"
        );
        wait_log(&rec, &["\"destroyed\":true"]);
        // SAFETY: a live display.
        unsafe { nhg_close(d) };
    }

    #[test]
    fn a_leave_is_an_action_and_ends_the_offer() {
        let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let rec = Recorder::start("mutter");
        let d = open(&rec);
        let w = window(&rec, d);
        offer(&rec, &[(TEXT, "x")]);
        let time = actions(d, 1, 3000)[0].time_ms as u32;
        rec.control_ok(r#"{"cmd":"dnd_leave"}"#);
        let got = actions(d, 1, 3000);
        assert_eq!(got.len(), 1, "{got:?}");
        assert_eq!((got[0].kind, got[0].window), (NHG_ACT_DND_LEAVE, w));
        // the drag is over: answering it does nothing
        assert_eq!(status(d, NHSHELL_DND_COPY, time), 0);
        let mut out: *const c_char = ptr::null();
        let mut len = 0usize;
        // SAFETY: a live display; `out` and `len` are writable.
        assert_eq!(
            unsafe { nhg_selection_types(d, NHSHELL_SEL_DND, &mut out, &mut len) },
            0
        );
        // SAFETY: a live display.
        unsafe { nhg_close(d) };
    }
}

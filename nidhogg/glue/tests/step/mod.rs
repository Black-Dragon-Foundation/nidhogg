//! The event step's T1 harness, shared by the test files that drive `nhg_next` against nh-recorder.  Each test stands
//! in for C: it makes the window as `nhwl_window_new` does (register, then push the scale and the size), applies the
//! actions it is given, and pushes what C would push.  The test file declares `mod common` (the shell's harness).
#![allow(dead_code)]

use crate::common::{Recorder, readable_within};
use nhglue::abi::*;
use nhglue::action::*;
use nhglue::ffi::*;
use nhshell::api::{NHSHELL_ACK_CONTENT, ShellBackend, WindowDesc};
use std::ptr;
use std::time::{Duration, Instant};

pub fn open(rec: &Recorder) -> *mut NhgDisplay {
    let name = rec.display();
    // SAFETY: the name is NUL-terminated; no error buffer.
    let d = unsafe { nhg_open(name.as_ptr(), ptr::null_mut(), 0) };
    assert!(!d.is_null(), "nhg_open failed on the recorder");
    d
}

/// The shell's backend inside `d`, for what the glue does not wrap (windows, records).
pub fn backend<'a>(d: *mut NhgDisplay) -> &'a mut dyn ShellBackend {
    // SAFETY: a live display; its backend lives as long as it does, and no glue call runs meanwhile.
    unsafe { &mut *(*d).backend_ptr() }
}

/// One step: its return and a copy of its actions.
pub fn step(d: *mut NhgDisplay, flags: u32) -> (i32, Vec<NhgAction>) {
    let input = NhgNextIn { flags };
    let mut list = NhgActionList {
        v: ptr::null(),
        n: 0,
    };
    // SAFETY: a live display; `input` is readable and `list` writable.
    let rc = unsafe { nhg_next(d, &input, &mut list) };
    let v = if list.n == 0 {
        Vec::new()
    } else {
        // SAFETY: `n` actions, valid until the next nhg_next.
        unsafe { std::slice::from_raw_parts(list.v, list.n) }.to_vec()
    };
    (rc, v)
}

/// Step until an action satisfies `want` (within 5 s); every action seen, the awaited one last.
pub fn until(d: *mut NhgDisplay, want: impl Fn(&NhgAction) -> bool) -> Vec<NhgAction> {
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut seen = Vec::new();
    loop {
        let (rc, v) = step(d, 0);
        assert!(rc >= 0, "nhg_next failed: {rc}");
        for a in v {
            let done = want(&a);
            seen.push(a);
            if done {
                return seen;
            }
        }
        if rc == 0 {
            assert!(Instant::now() < deadline, "the awaited action never came");
            // SAFETY: a live display.
            readable_within(unsafe { nhg_fd(d) }, 50);
        }
    }
}

/// Step until nothing comes for 200 ms; every action seen.
pub fn settle(d: *mut NhgDisplay) -> Vec<NhgAction> {
    let mut seen = Vec::new();
    loop {
        let (rc, v) = step(d, 0);
        assert!(rc >= 0, "nhg_next failed: {rc}");
        seen.extend(v);
        // SAFETY: a live display.
        if rc == 0 && !readable_within(unsafe { nhg_fd(d) }, 200) {
            return seen;
        }
    }
}

/// Wait until the shell has queued what the recorder sent (its descriptor is readable), and a little more.
pub fn queued(d: *mut NhgDisplay) {
    // SAFETY: a live display.
    assert!(readable_within(unsafe { nhg_fd(d) }, 3000), "nothing came");
    std::thread::sleep(Duration::from_millis(200));
}

/// A top-level window of 200 x 100, made and registered as `nhwl_window_new` makes it, at scale 1.
pub fn window(d: *mut NhgDisplay) -> u64 {
    let id = backend(d)
        .window_new(&WindowDesc {
            w: 200,
            h: 100,
            title: "t".into(),
            app_id: "nidhogg.test".into(),
            flags: 0,
        })
        .unwrap();
    // SAFETY: a live display.
    unsafe {
        assert_eq!(nhg_frame_register(d, id, 0), NHG_OK);
        assert_eq!(nhg_frame_scale(d, id, 120), NHG_OK);
        assert_eq!(nhg_frame_resized(d, id, 200, 100), NHG_OK);
    }
    id
}

/// The window's first configure taken and applied (no resize), its ack recorded.
pub fn configured(d: *mut NhgDisplay, id: u64) {
    until(d, |a| a.kind == NHG_ACT_RESIZE && a.window == id);
    step(d, 0);
    assert_eq!(last_ack(d, id).map(|(_, f)| f), Some(NHSHELL_ACK_CONTENT));
    settle(d);
}

/// The last ack the glue recorded for `id`: serial and flags.
pub fn last_ack(d: *mut NhgDisplay, id: u64) -> Option<(u32, u32)> {
    // SAFETY: a live display.
    let d = unsafe { &*d };
    d.lock_for_test().frame(id).unwrap().last_ack
}

pub fn configure(rec: &Recorder, size: [i32; 2], states: &str) {
    rec.control_ok(&format!(
        r#"{{"cmd":"configure","surface":"focused","size":[{},{}],"states":[{states}]}}"#,
        size[0], size[1]
    ));
}

pub fn close(d: *mut NhgDisplay) {
    // SAFETY: a live display, not used again.
    assert_eq!(unsafe { nhg_close(d) }, NHG_OK);
}

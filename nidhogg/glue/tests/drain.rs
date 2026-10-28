//! T1 tests of the event step and of configures and scale, against nh-recorder (the shell's harness,
//! `../shell/tests/common`).  Each test stands in for C: it makes the window as `nhwl_window_new` does (register, then
//! push the scale and the size), applies the actions it is given, and pushes what C would push.

#[path = "../../shell/tests/common/mod.rs"]
mod common;

use common::{Recorder, readable_within};
use nhglue::abi::*;
use nhglue::action::*;
use nhglue::ffi::*;
use nhshell::api::{NHSHELL_ACK_CONTENT, ShellBackend, WindowDesc};
use nhshell::event::*;
use std::ptr;
use std::time::{Duration, Instant};

fn open(rec: &Recorder) -> *mut NhgDisplay {
    let name = rec.display();
    // SAFETY: the name is NUL-terminated; no error buffer.
    let d = unsafe { nhg_open(name.as_ptr(), ptr::null_mut(), 0) };
    assert!(!d.is_null(), "nhg_open failed on the recorder");
    d
}

/// The shell's backend inside `d`, as C reaches the calls not yet ported.
fn backend<'a>(d: *mut NhgDisplay) -> &'a mut dyn ShellBackend {
    // SAFETY: a live display; its shell handle lives as long as it does, and no glue call runs meanwhile.
    unsafe { &mut *nhg_shell(d) }.backend_mut()
}

/// One step: its return and a copy of its actions.
fn step(d: *mut NhgDisplay, flags: u32) -> (i32, Vec<NhgAction>) {
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
fn until(d: *mut NhgDisplay, want: impl Fn(&NhgAction) -> bool) -> Vec<NhgAction> {
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
fn settle(d: *mut NhgDisplay) -> Vec<NhgAction> {
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
fn queued(d: *mut NhgDisplay) {
    // SAFETY: a live display.
    assert!(readable_within(unsafe { nhg_fd(d) }, 3000), "nothing came");
    std::thread::sleep(Duration::from_millis(200));
}

/// A top-level window of 200 x 100, made and registered as `nhwl_window_new` makes it, at scale 1.
fn window(d: *mut NhgDisplay) -> u64 {
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
fn configured(d: *mut NhgDisplay, id: u64) {
    until(d, |a| a.kind == NHG_ACT_RESIZE && a.window == id);
    step(d, 0);
    assert_eq!(last_ack(d, id).map(|(_, f)| f), Some(NHSHELL_ACK_CONTENT));
    settle(d);
}

/// The last ack the glue recorded for `id`: serial and flags.
fn last_ack(d: *mut NhgDisplay, id: u64) -> Option<(u32, u32)> {
    // SAFETY: a live display.
    let d = unsafe { &*d };
    d.lock_for_test().frame(id).unwrap().last_ack
}

fn configure(rec: &Recorder, size: [i32; 2], states: &str) {
    rec.control_ok(&format!(
        r#"{{"cmd":"configure","surface":"focused","size":[{},{}],"states":[{states}]}}"#,
        size[0], size[1]
    ));
}

fn close(d: *mut NhgDisplay) {
    // SAFETY: a live display, not used again.
    assert_eq!(unsafe { nhg_close(d) }, NHG_OK);
}

/// The plan names these `drain::…`.
#[allow(clippy::module_inception)]
mod drain {
    use super::*;

    /// Three queued configures come one per call, each call taking one event, and a call with nothing queued says 0.
    #[test]
    fn one_event_per_call() {
        let rec = Recorder::start("mutter");
        let d = open(&rec);
        let id = window(d);
        configured(d, id);
        for w in [300, 310, 320] {
            configure(&rec, [w, 150], "");
        }
        queued(d);
        let mut sizes = Vec::new();
        loop {
            let (rc, v) = step(d, 0);
            if rc == 0 {
                assert!(v.is_empty());
                break;
            }
            assert_eq!(rc, 1);
            let resizes: Vec<_> = v.iter().filter(|a| a.kind == NHG_ACT_RESIZE).collect();
            assert!(resizes.len() <= 1, "two configures in one call");
            sizes.extend(resizes.iter().map(|a| a.w));
        }
        assert_eq!(sizes, [300, 310, 320]);
        let (rc, v) = step(d, 0);
        assert_eq!((rc, v.len()), (0, 0));
        close(d);
    }

    /// A `COMMIT_TEXT`'s text stays intact while C makes other calls on the display, until the next `nhg_next`.
    #[test]
    fn text_lives_until_the_next_next() {
        let rec = Recorder::start("mutter");
        let d = open(&rec);
        let id = window(d);
        configured(d, id);
        backend(d).text_input_enable(id, true).unwrap();
        rec.control_ok(r#"{"cmd":"focus","surface":"toplevel"}"#);
        settle(d);
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut list = NhgActionList {
            v: ptr::null(),
            n: 0,
        };
        'found: loop {
            assert!(Instant::now() < deadline, "the commit never came");
            rec.control_ok(r#"{"cmd":"text_input","commit":"é"}"#);
            let until = Instant::now() + Duration::from_millis(300);
            while Instant::now() < until {
                // SAFETY: a live display; `list` is writable.
                let rc = unsafe { nhg_next(d, ptr::null(), &mut list) };
                assert!(rc >= 0);
                let mut ev = NhShellEvent::new(0, 0, 0);
                // SAFETY: `n` actions, valid until the next nhg_next; `ev` is writable.
                let commit = list.n == 1
                    && unsafe { (*list.v).kind } == NHG_ACT_RAW
                    && unsafe { nhg_raw_event(d, &mut ev) } == NHG_OK
                    && ev.kind == NHSHELL_EV_COMMIT_TEXT;
                if commit {
                    break 'found;
                }
                if rc == 0 {
                    // SAFETY: a live display.
                    readable_within(unsafe { nhg_fd(d) }, 50);
                }
            }
        }
        // other calls, as C makes them while it applies the action
        // SAFETY: a live display.
        unsafe {
            assert_eq!(nhg_frame_resized(d, id, 210, 110), NHG_OK);
            assert_eq!(nhg_frame_csd(d, id, 1), NHG_OK);
            assert!(nhg_fd(d) >= 0);
            assert_eq!(nhg_frame_register(d, id + 1000, 0), NHG_OK);
            assert_eq!(nhg_frame_forget(d, id + 1000), NHG_OK);
            let mut ev = NhShellEvent::new(0, 0, 0);
            assert_eq!(nhg_raw_event(d, &mut ev), NHG_OK);
        }
        // SAFETY: the action and its text are valid until the next nhg_next.
        let a = unsafe { *list.v };
        assert!(!a.text.is_null());
        // SAFETY: `len` bytes and a NUL.
        let text = unsafe { std::slice::from_raw_parts(a.text.cast::<u8>(), a.len + 1) };
        assert_eq!(text, "é\0".as_bytes());
        close(d);
    }

    /// A forgotten window's queued configure, close, scale and pointer events give no action; the glue refuses
    /// pushes for it.
    #[test]
    fn events_for_a_forgotten_window_give_no_actions() {
        let rec = Recorder::start("mutter");
        let d = open(&rec);
        let id = window(d);
        configured(d, id);
        configure(&rec, [300, 150], r#""activated""#);
        rec.control_ok(r#"{"cmd":"scale","value120":180}"#);
        rec.control_ok(r#"{"cmd":"close","surface":"focused"}"#);
        queued(d);
        // SAFETY: a live display.
        assert_eq!(unsafe { nhg_frame_forget(d, id) }, NHG_OK);
        // the keyboard's events still come (C sends them to the focus frame); nothing else names the window
        let seen = settle(d);
        assert!(
            seen.iter().all(|a| a.window != id || a.kind == NHG_ACT_RAW),
            "actions for a forgotten window: {seen:?}"
        );
        // SAFETY: a live display.
        unsafe {
            assert_eq!(nhg_frame_resized(d, id, 10, 10), NHG_E_STATE);
            assert_eq!(nhg_frame_scale(d, id, 240), NHG_E_STATE);
        }
        close(d);
    }
}

/// The plan names these `configure::…`.
#[allow(clippy::module_inception)]
mod configure {
    use super::*;

    /// Content acks: the first configure, a state change under a client-side title bar, and a configure C resized the
    /// frame for (recorded at once, on the push); a bare one otherwise.
    #[test]
    fn content_ack_on_new_size_first_and_csd_states() {
        let rec = Recorder::start("mutter");
        let d = open(&rec);
        let id = window(d);
        // the first: content (checked in `configured`)
        configured(d, id);
        // SAFETY: a live display.
        assert_eq!(unsafe { nhg_frame_csd(d, id, 1) }, NHG_OK);
        // activated under CSD, same size: content, recorded at the next step
        configure(&rec, [200, 100], r#""activated""#);
        until(d, |a| a.kind == NHG_ACT_RESIZE && a.window == id);
        step(d, 0);
        assert_eq!(last_ack(d, id).map(|(_, f)| f), Some(NHSHELL_ACK_CONTENT));
        settle(d);
        // a new size: C resizes the frame and pushes the size, which records the ack at once, as content
        configure(&rec, [400, 300], r#""activated""#);
        let seen = until(d, |a| a.kind == NHG_ACT_RESIZE && a.window == id);
        let a = seen.last().unwrap();
        assert_eq!((a.w, a.h), (400, 300));
        assert_eq!(
            a.code as u32 & NHSHELL_STATE_ACTIVATED,
            NHSHELL_STATE_ACTIVATED
        );
        let before = last_ack(d, id);
        // SAFETY: a live display.
        assert_eq!(unsafe { nhg_frame_resized(d, id, 400, 300) }, NHG_OK);
        let after = last_ack(d, id);
        assert_ne!(after, before, "the push did not record the ack");
        assert_eq!(after.map(|(_, f)| f), Some(NHSHELL_ACK_CONTENT));
        settle(d);
        // the same size and states again: bare
        configure(&rec, [400, 300], r#""activated""#);
        until(d, |a| a.kind == NHG_ACT_RESIZE && a.window == id);
        step(d, 0);
        assert_eq!(last_ack(d, id).map(|(_, f)| f), Some(0));
        close(d);
    }

    /// Under server-side decorations a change of activation is no content change: a bare ack.  Maximizing asks C to
    /// store the fullscreen parameter.
    #[test]
    fn bare_ack_on_ssd_activation() {
        let rec = Recorder::start("mutter");
        let d = open(&rec);
        let id = window(d);
        configured(d, id);
        // SAFETY: a live display.
        assert_eq!(unsafe { nhg_frame_csd(d, id, 0) }, NHG_OK);
        configure(&rec, [200, 100], r#""activated""#);
        let seen = until(d, |a| a.kind == NHG_ACT_RESIZE && a.window == id);
        assert_eq!(seen.last().unwrap().flags & NHG_RESIZE_FULLSCREEN, 0);
        step(d, 0);
        assert_eq!(last_ack(d, id).map(|(_, f)| f), Some(0));
        settle(d);
        configure(&rec, [200, 100], r#""activated","maximized""#);
        let seen = until(d, |a| a.kind == NHG_ACT_RESIZE && a.window == id);
        assert_eq!(
            seen.last().unwrap().flags & NHG_RESIZE_FULLSCREEN,
            NHG_RESIZE_FULLSCREEN
        );
        step(d, 0);
        assert_eq!(last_ack(d, id).map(|(_, f)| f), Some(0));
        close(d);
    }

    /// A new preferred scale gives one `SCALE` action, however many times the shell reports it; the same scale again
    /// gives none.
    #[test]
    fn scale_change_gives_one_scale_action() {
        let rec = Recorder::start("mutter");
        let d = open(&rec);
        let id = window(d);
        configured(d, id);
        rec.control_ok(r#"{"cmd":"scale","value120":180}"#);
        let mut seen = until(d, |a| a.kind == NHG_ACT_SCALE);
        seen.extend(settle(d));
        let scales: Vec<_> = seen.iter().filter(|a| a.kind == NHG_ACT_SCALE).collect();
        assert_eq!(scales.len(), 1, "{scales:?}");
        assert_eq!((scales[0].window, scales[0].code), (id, 180));
        assert!((scales[0].scale - 1.5).abs() < 1e-9);
        rec.control_ok(r#"{"cmd":"scale","value120":180}"#);
        std::thread::sleep(Duration::from_millis(200));
        assert!(settle(d).iter().all(|a| a.kind != NHG_ACT_SCALE));
        close(d);
    }
}

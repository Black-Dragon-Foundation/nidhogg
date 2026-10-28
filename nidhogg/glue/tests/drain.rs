//! T1 tests of the event step and of configures and scale, against nh-recorder (the shell's harness,
//! `../shell/tests/common`, and the step's, `step/mod.rs`).

#[path = "../../shell/tests/common/mod.rs"]
mod common;
mod step;

use common::{Recorder, readable_within};
use nhglue::abi::*;
use nhglue::action::*;
use nhglue::ffi::*;
use nhglue::frame::NHG_FRAME_CHILD;
use nhshell::api::{ChildDesc, NHSHELL_ACK_CONTENT};
use nhshell::event::*;
use std::ptr;
use std::time::{Duration, Instant};
use step::*;

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
        // keys and the input method's events still come (C sends them to the focus frame); nothing else names the
        // window
        let seen = settle(d);
        assert!(
            seen.iter()
                .all(|a| a.window != id || a.kind == NHG_ACT_RAW || a.kind == NHG_ACT_KEY),
            "actions for a forgotten window: {seen:?}"
        );
        // SAFETY: a live display.
        unsafe {
            assert_eq!(nhg_frame_resized(d, id, 10, 10), NHG_E_STATE);
            assert_eq!(nhg_frame_scale(d, id, 240), NHG_E_STATE);
        }
        close(d);
    }

    /// A press on a child and a key, queued in one burst, taken in one drain: the key's action names the window with
    /// the keyboard (the root), and the test's apply loop, standing in for C, applied the press's focus before the
    /// next `nhg_next`, so its resolution sends the key to the child (review B1).  The recorder moves the pointer over
    /// the surface with the keyboard, so the burst focuses the child for the press and the root again for the key.
    #[test]
    fn click_on_child_then_key_routes_to_child() {
        let rec = Recorder::start("sway");
        let d = open(&rec);
        let root = window(d);
        configured(d, root);
        let child = backend(d)
            .child_new(&ChildDesc {
                kind: nhshell::api::NHSHELL_KIND_CHILD,
                parent: root,
                x: 40,
                y: 30,
                w: 100,
                h: 60,
                anchor: Default::default(),
                flags: 0,
            })
            .unwrap();
        // SAFETY: a live display.
        unsafe {
            assert_eq!(nhg_frame_register(d, child, NHG_FRAME_CHILD), NHG_OK);
            assert_eq!(nhg_frame_scale(d, child, 120), NHG_OK);
            assert_eq!(nhg_frame_resized(d, child, 100, 60), NHG_OK);
        }
        // sent, as the C drain's dispatch sends it
        backend(d).dispatch_main().unwrap();
        rec.control_ok(r#"{"cmd":"focus","surface":"toplevel"}"#);
        settle(d);
        // the burst, all queued before the drain
        rec.control_ok(&format!(r#"{{"cmd":"focus","surface":{child}}}"#));
        rec.control_ok(r#"{"cmd":"pointer","pos":[10,10]}"#);
        rec.control_ok(r#"{"cmd":"button","button":272,"state":"press"}"#);
        rec.control_ok(r#"{"cmd":"button","button":272,"state":"release"}"#);
        rec.control_ok(r#"{"cmd":"focus","surface":"toplevel"}"#);
        rec.control_ok(r#"{"cmd":"key","text":"x"}"#);
        queued(d);
        // C's state: each root's focus redirect (`nhwl_child_focus`), and the frame each key reached
        let mut redirect: Option<(u64, u64)> = None;
        let mut reached = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(5);
        while reached.is_empty() {
            assert!(Instant::now() < deadline, "the key never came");
            let (rc, v) = step(d, 0);
            assert!(rc >= 0, "nhg_next failed: {rc}");
            for a in &v {
                match a.kind {
                    NHG_ACT_RAW => {
                        let mut ev = NhShellEvent::new(0, 0, 0);
                        // SAFETY: a live display; `ev` is writable.
                        assert_eq!(unsafe { nhg_raw_event(d, &mut ev) }, NHG_OK);
                        // SAFETY: a BUTTON record's valid member is `pointer`.
                        if ev.kind == NHSHELL_EV_BUTTON
                            && unsafe { ev.u.pointer }.state == NHSHELL_BUTTON_PRESSED
                            && ev.window == child
                        {
                            redirect = Some((root, child));
                        }
                    }
                    NHG_ACT_KEY => {
                        assert_eq!(a.window, root, "the key names the window with the keyboard");
                        let to = match redirect {
                            Some((from, to)) if from == a.window => to,
                            _ => a.window,
                        };
                        reached.push(to);
                    }
                    _ => {}
                }
            }
            if rc == 0 {
                // SAFETY: a live display.
                readable_within(unsafe { nhg_fd(d) }, 50);
            }
        }
        assert_eq!(reached, [child]);
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

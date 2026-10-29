//! T1 tests of giving a display up through the glue (the fallback: the compositor went and nothing waits for it to
//! come back), against nh-recorder.  Each test stands in for C's `nhwl_connection_lost`: it takes the loss as the drain
//! does, reads the error text for the exit message, gives the display up, forgets the frames' backings as their
//! deletion does, and closes.

#[path = "../../shell/tests/common/mod.rs"]
mod common;
mod step;

use common::Recorder;
use nhglue::abi::*;
use nhglue::action::*;
use nhglue::backing::{NhgFrameGeom, NhgRect};
use nhglue::ffi::*;
use nhglue::reconnect::NHG_ACT_RECONNECTING;
use std::ffi::c_char;
use std::ptr;
use step::*;

mod lost {
    use super::*;

    const GONE: &str = r#"{"cmd":"restart","mode":"gone"}"#;

    /// The connection's error text through `nhg_last_error`, read as C reads it: the length first, then the text.
    fn last_error(d: *mut NhgDisplay) -> String {
        let mut len = 0usize;
        // SAFETY: a live display; no buffer; `len` is writable.
        assert_eq!(
            unsafe { nhg_last_error(d, ptr::null_mut(), 0, &mut len) },
            NHG_OK
        );
        let mut buf = vec![0u8; len + 1];
        // SAFETY: `buf` holds `len + 1` writable bytes; `len` is writable.
        let rc = unsafe { nhg_last_error(d, buf.as_mut_ptr().cast(), buf.len(), &mut len) };
        assert_eq!(rc, NHG_OK);
        assert_eq!(buf[len], 0, "the text ends in a NUL");
        buf.truncate(len);
        String::from_utf8(buf).unwrap()
    }

    /// A frame's backing as `nhwl_glitnir_ensure` makes it: a new key, with a Glitnir root.
    fn backing(d: *mut NhgDisplay) -> u64 {
        let mut key = 0;
        let mut g = NhgFrameGeom {
            native_w: 200,
            native_h: 100,
            scale120: 120,
            ..Default::default()
        };
        g.layer = NhgRect {
            x: 0,
            y: 0,
            w: 200,
            h: 100,
        };
        // SAFETY: a live display; `key` and `g` are valid for the call.
        assert!(unsafe { nhg_layer_ensure(d, &mut key, &g) } >= 0);
        assert_ne!(key, 0);
        key
    }

    fn root_lives(key: u64) -> bool {
        glitnir::scene::committed(key).is_some()
    }

    #[test]
    fn a_hangup_leaves_no_error_text() {
        let rec = Recorder::start("sway");
        let d = open(&rec);
        let w = window(d);
        configured(d, w);
        assert_eq!(last_error(d), "");
        rec.control_ok(GONE);
        until(d, |a| a.kind == NHG_ACT_LOST);
        assert_eq!(last_error(d), "", "a hangup is not a protocol error");
        // SAFETY: a live display.
        assert_eq!(unsafe { nhg_give_up(d) }, NHG_OK);
        close(d);
    }

    #[test]
    fn a_protocol_error_is_kept_for_the_exit_message() {
        let rec = Recorder::start("sway");
        let d = open(&rec);
        let w = window(d);
        configured(d, w);
        // a configure serial the compositor never sent ends the connection with a protocol error
        let _ = backend(d).ack_configure(w, 999_999, 0);
        // an ack rides the commit that carries it: the window's next present sends it
        until(d, |a| a.kind == NHG_ACT_READY && a.window == w);
        // SAFETY: a live display; no surfaces.
        unsafe { nhg_present(d, w, ptr::null(), 0, nhglue::present::NHG_PRESENT_READY, 0) };
        until(d, |a| a.kind == NHG_ACT_LOST);
        let text = last_error(d);
        assert!(text.contains(": error "), "{text:?}");
        // a short buffer gets the start of the text, ended by a NUL, and the full length
        let mut buf = [0x7f as c_char; 8];
        let mut len = 0usize;
        // SAFETY: `buf` holds 8 writable bytes; `len` is writable.
        let rc = unsafe { nhg_last_error(d, buf.as_mut_ptr(), buf.len(), &mut len) };
        assert_eq!(rc, NHG_OK);
        assert_eq!(len, text.len());
        assert_eq!(buf[7], 0);
        // the text stays after the display is given up
        // SAFETY: a live display.
        assert_eq!(unsafe { nhg_give_up(d) }, NHG_OK);
        assert_eq!(last_error(d), text);
        close(d);
    }

    #[test]
    fn giving_up_ends_reconnecting() {
        let rec = Recorder::start("sway");
        let d = open(&rec);
        // SAFETY: a live display.
        assert_eq!(unsafe { nhg_set_reconnect(d, 1) }, NHG_OK);
        let w = window(d);
        configured(d, w);
        rec.control_ok(GONE);
        until(d, |a| a.kind == NHG_ACT_RECONNECTING);
        // the timeout passed in C, or reconnecting was turned off at the loss: C gives the display up
        // SAFETY: a live display.
        assert_eq!(unsafe { nhg_give_up(d) }, NHG_OK);
        let mut next = 7;
        // SAFETY: a live display; `next` is writable.
        let rc = unsafe { nhg_reconnect_step(d, 60_000, &mut next) };
        assert_eq!(
            (rc, next),
            (NHG_E_DISCONNECTED, 0),
            "no step is due any more"
        );
        let (mut window, mut next) = (1u64, 1u32);
        // SAFETY: a live display; both outputs are writable.
        let bits = unsafe { nhg_reconnect_settle(d, &mut window, &mut next) };
        assert_eq!((bits, window, next), (0, 0, 0), "nothing is left to settle");
        close(d);
    }

    #[test]
    fn every_root_is_freed_by_the_deletions_or_the_close() {
        let rec = Recorder::start("sway");
        let d = open(&rec);
        let w = window(d);
        configured(d, w);
        let deleted = backing(d);
        let missed = backing(d);
        assert!(root_lives(deleted) && root_lives(missed));
        rec.control_ok(GONE);
        until(d, |a| a.kind == NHG_ACT_LOST);
        // SAFETY: a live display.
        assert_eq!(unsafe { nhg_give_up(d) }, NHG_OK);
        // a frame's deletion forgets its backing; one deletion never reaches the glue
        // SAFETY: a live display.
        assert_eq!(unsafe { nhg_backing_forget(d, deleted) }, 0);
        assert!(!root_lives(deleted));
        assert!(root_lives(missed));
        close(d);
        assert!(
            !root_lives(missed),
            "the close frees what the deletions left"
        );
    }

    #[test]
    fn calls_after_giving_up_are_refused_or_harmless() {
        let rec = Recorder::start("sway");
        let d = open(&rec);
        let w = window(d);
        configured(d, w);
        rec.control_ok(GONE);
        until(d, |a| a.kind == NHG_ACT_LOST);
        // SAFETY: a live display.
        unsafe {
            assert_eq!(nhg_give_up(d), NHG_OK);
            // giving up twice changes nothing (a second loss path finds the display already given up)
            assert_eq!(nhg_give_up(d), NHG_OK);
            // the frame's deletion, as C makes it: the input method off, then the window forgotten
            assert!(nhg_im_enable(d, w, 0) <= 0);
            assert!(nhg_window_destroy(d, w) <= 0);
        }
        // nothing more comes out of the drain
        let (rc, v) = step(d, 0);
        assert!(rc <= 0 && v.is_empty(), "{rc} {v:?}");
        close(d);
    }
}

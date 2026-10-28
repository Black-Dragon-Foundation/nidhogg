//! T1 tests of reconnecting in place through the glue, against nh-recorder's `restart` (the socket kept, bound again,
//! or gone).  Each test stands in for C: it turns reconnecting on as `nhwl_term_init` does, takes the actions as the
//! drain does, and calls `nhg_reconnect_step` and `nhg_reconnect_settle` as C's timer and reads do.

#[path = "../../shell/tests/common/mod.rs"]
mod common;
mod step;

use common::{Recorder, readable_within};
use nhglue::abi::*;
use nhglue::action::*;
use nhglue::backing::{NhgBacking, NhgFrameGeom, NhgRect};
use nhglue::ffi::*;
use nhglue::present::{NHG_PRESENT_BACKSTOP, NHG_PRESENT_READY, NHG_SURF_DRAWABLE, NhgSurface};
use nhglue::reconnect::{
    FOCUS_WAIT, NHG_ACT_RECONNECTED, NHG_ACT_RECONNECTING, NHG_SETTLE_FOCUS_OUT,
    NHG_SETTLE_MONITORS, NHG_SETTLE_REPORT, REPORT_WAIT,
};
use std::ptr;
use std::time::{Duration, Instant};
use step::*;

mod reconnect {
    use super::*;

    /// A display on `rec` with reconnecting on, as C opens it.
    fn open_reconnecting(rec: &Recorder) -> *mut NhgDisplay {
        let d = open(rec);
        // SAFETY: a live display.
        assert_eq!(unsafe { nhg_set_reconnect(d, 1) }, NHG_OK);
        d
    }

    fn restart(rec: &Recorder, req: &str) {
        let reply = rec.control(req);
        assert!(reply.contains("\"ok\":true"), "{reply}");
    }

    /// One tick: the code and the next delay.
    fn tick(d: *mut NhgDisplay, timeout_ms: u32) -> (i32, u32) {
        let mut next = 0;
        // SAFETY: a live display; `next` is writable.
        let rc = unsafe { nhg_reconnect_step(d, timeout_ms, &mut next) };
        (rc, next)
    }

    /// What settling says now: bits, window, next delay.
    fn settle_now(d: *mut NhgDisplay) -> (i32, u64, u32) {
        let mut w = 0;
        let mut next = 0;
        // SAFETY: a live display; both outputs are writable.
        let bits = unsafe { nhg_reconnect_settle(d, &mut w, &mut next) };
        (bits, w, next)
    }

    /// Tick as C's timer does (sleeping the delays the glue asks for) until connected, within `secs`.
    fn tick_until_connected(d: *mut NhgDisplay, secs: u64) {
        let deadline = Instant::now() + Duration::from_secs(secs);
        loop {
            let (rc, next) = tick(d, 10_000);
            if rc == NHG_OK {
                return;
            }
            assert_eq!(rc, NHG_E_TIMEOUT, "the step gave up");
            assert!(Instant::now() < deadline, "never reconnected");
            std::thread::sleep(Duration::from_millis(u64::from(next.min(100))));
        }
    }

    /// A keep-socket restart: every action up to `RECONNECTING`, then up to `RECONNECTED` after the ticks.
    fn restart_and_reconnect(
        rec: &Recorder,
        d: *mut NhgDisplay,
    ) -> (Vec<NhgAction>, Vec<NhgAction>) {
        restart(rec, r#"{"cmd":"restart","mode":"keep-socket"}"#);
        let lost = until(d, |a| a.kind == NHG_ACT_RECONNECTING);
        tick_until_connected(d, 5);
        let back = until(d, |a| a.kind == NHG_ACT_RECONNECTED);
        (lost, back)
    }

    #[test]
    fn a_hangup_reconnects_instead_of_losing_the_display() {
        let rec = Recorder::start("sway");
        let d = open_reconnecting(&rec);
        let w = window(d);
        configured(d, w);
        let (lost, back) = restart_and_reconnect(&rec, d);
        assert!(
            lost.iter().chain(&back).all(|a| a.kind != NHG_ACT_LOST),
            "a hangup is not a loss"
        );
        let r = lost.last().unwrap();
        assert_eq!(
            (r.window, r.code),
            (0, 50),
            "the first tick comes 50 ms later"
        );
        assert_eq!(back.last().unwrap().window, 0);
        // the same window gets the new compositor's configure, as a content ack
        configured(d, w);
        close(d);
    }

    #[test]
    fn the_backoff_doubles_to_a_second_and_gives_up_at_the_timeout() {
        let rec = Recorder::start("sway");
        let d = open_reconnecting(&rec);
        let w = window(d);
        configured(d, w);
        restart(&rec, r#"{"cmd":"restart","mode":"gone"}"#);
        until(d, |a| a.kind == NHG_ACT_RECONNECTING);
        let mut delays = Vec::new();
        for _ in 0..6 {
            let t = Instant::now();
            let (rc, next) = tick(d, 60_000);
            assert_eq!(rc, NHG_E_TIMEOUT);
            assert!(
                t.elapsed() < Duration::from_millis(150),
                "a tick blocks at most its step"
            );
            delays.push(next);
        }
        assert_eq!(delays, [100, 200, 400, 800, 1000, 1000]);
        // a timeout already passed gives up, and so does every later tick
        assert_eq!(tick(d, 1), (NHG_E_DISCONNECTED, 0));
        assert_eq!(tick(d, 60_000), (NHG_E_DISCONNECTED, 0));
        close(d);
    }

    #[test]
    fn the_next_tick_never_lands_past_the_timeout() {
        let rec = Recorder::start("sway");
        let d = open_reconnecting(&rec);
        let w = window(d);
        configured(d, w);
        restart(&rec, r#"{"cmd":"restart","mode":"gone"}"#);
        until(d, |a| a.kind == NHG_ACT_RECONNECTING);
        let start = Instant::now();
        loop {
            let (rc, next) = tick(d, 600);
            if rc == NHG_E_DISCONNECTED {
                break;
            }
            assert_eq!(rc, NHG_E_TIMEOUT);
            assert!(next <= 600, "{next}");
            std::thread::sleep(Duration::from_millis(u64::from(next)));
        }
        let took = start.elapsed();
        assert!(took < Duration::from_millis(900), "gave up after {took:?}");
        close(d);
    }

    #[test]
    fn a_rebound_socket_is_found_again() {
        let rec = Recorder::start("sway");
        let d = open_reconnecting(&rec);
        let w = window(d);
        configured(d, w);
        restart(&rec, r#"{"cmd":"restart","mode":"rebind","down_ms":300}"#);
        until(d, |a| a.kind == NHG_ACT_RECONNECTING);
        tick_until_connected(d, 5);
        until(d, |a| a.kind == NHG_ACT_RECONNECTED);
        close(d);
    }

    #[test]
    fn the_loss_forgets_the_old_connections_acks_and_configures() {
        let rec = Recorder::start("sway");
        let d = open_reconnecting(&rec);
        let w = window(d);
        configured(d, w);
        restart(&rec, r#"{"cmd":"restart","mode":"keep-socket"}"#);
        until(d, |a| a.kind == NHG_ACT_RECONNECTING);
        {
            // SAFETY: a live display, no glue call running.
            let g = unsafe { &*d }.lock_for_test();
            let f = g.frame(w).unwrap();
            assert!(
                !f.cfg.configured,
                "the new compositor's configure is a first one"
            );
            assert!(f.ack.is_none());
        }
        tick_until_connected(d, 5);
        until(d, |a| a.kind == NHG_ACT_RECONNECTED);
        close(d);
    }

    #[test]
    fn calls_while_reconnecting_send_nothing_and_fail_softly() {
        let rec = Recorder::start("sway");
        let d = open_reconnecting(&rec);
        let w = window(d);
        configured(d, w);
        restart(&rec, r#"{"cmd":"restart","mode":"gone"}"#);
        until(d, |a| a.kind == NHG_ACT_RECONNECTING);
        // presenting is "not ready", not an error
        // SAFETY: a live display; no surfaces.
        let rc = unsafe { nhg_present(d, w, ptr::null(), 0, NHG_PRESENT_READY, 0) };
        assert_eq!(rc, 0);
        // a sending call says the display is reconnecting, with the glue's own code
        // SAFETY: a live display.
        let rc = unsafe { nhg_inhibit_idle(d, w, 1) };
        assert!(rc == NHG_E_RECONNECTING || rc == NHG_E_UNSUPPORTED, "{rc}");
        // a window deleted in the gap goes locally
        // SAFETY: a live display.
        assert_eq!(unsafe { nhg_window_destroy(d, w) }, NHG_OK);
        close(d);
    }

    #[test]
    fn monitor_changes_wait_for_the_report_and_count_only_real_ones() {
        let rec = Recorder::start("sway");
        let d = open_reconnecting(&rec);
        let w = window(d);
        configured(d, w);
        let (lost, back) = restart_and_reconnect(&rec, d);
        let mut seen = lost;
        seen.extend(back);
        seen.extend(settle(d));
        assert!(
            seen.iter().all(|a| a.kind != NHG_ACT_MONITORS),
            "the outputs going and coming are not reported one by one"
        );
        // nothing presented again: the report comes 2 s after RECONNECTED, and the same outputs are no change
        let deadline = Instant::now() + REPORT_WAIT + Duration::from_secs(1);
        let bits = loop {
            let (bits, _, next) = settle_now(d);
            if bits & NHG_SETTLE_REPORT != 0 {
                break bits;
            }
            assert!(next > 0 && Instant::now() < deadline, "no report");
            std::thread::sleep(Duration::from_millis(u64::from(next.min(200))));
        };
        assert_eq!(bits & NHG_SETTLE_MONITORS, 0);
        close(d);
    }

    #[test]
    fn a_frame_that_presents_again_settles_at_once() {
        let rec = Recorder::start("sway");
        let d = open_reconnecting(&rec);
        let w = window(d);
        configured(d, w);
        // SAFETY: a live display.
        unsafe { assert_eq!(nhg_window_visible(d, w, 1), NHG_OK) };
        let key = backing(d);
        restart_and_reconnect(&rec, d);
        assert_eq!(settle_now(d).0 & NHG_SETTLE_REPORT, 0, "not presented yet");
        // published before the new configure: still owed
        publish_whole(d, w, key);
        assert_eq!(settle_now(d).0 & NHG_SETTLE_REPORT, 0, "not configured yet");
        configured(d, w);
        publish_whole(d, w, key);
        assert_ne!(settle_now(d).0 & NHG_SETTLE_REPORT, 0);
        close(d);
    }

    #[test]
    fn the_focus_goes_when_no_keyboard_enter_comes() {
        let rec = Recorder::start("sway");
        let d = open_reconnecting(&rec);
        let w = window(d);
        configured(d, w);
        rec.control_ok(r#"{"cmd":"focus","surface":"toplevel"}"#);
        until(d, |a| a.kind == NHG_ACT_FOCUS_IN && a.window == w);
        let (lost, back) = restart_and_reconnect(&rec, d);
        assert!(
            lost.iter()
                .chain(&back)
                .all(|a| a.kind != NHG_ACT_FOCUS_OUT),
            "no focus-out at the loss"
        );
        let mut entered = false;
        let deadline = Instant::now() + REPORT_WAIT + FOCUS_WAIT + Duration::from_secs(1);
        let out = loop {
            entered |= settle(d)
                .iter()
                .any(|a| a.kind == NHG_ACT_FOCUS_IN && a.window == w);
            let (bits, win, next) = settle_now(d);
            if bits & NHG_SETTLE_FOCUS_OUT != 0 {
                break Some(win);
            }
            if next == 0 {
                break None;
            }
            assert!(Instant::now() < deadline, "never settled");
            std::thread::sleep(Duration::from_millis(u64::from(next.min(200))));
        };
        // the recorder may give the remade window the keyboard on its own: then there is nothing to take back
        assert_eq!(out, if entered { None } else { Some(w) });
        close(d);
    }

    /// A 200 x 100 backing at scale 1, as `nhwl_backing_resize` makes it; its key.
    fn backing(d: *mut NhgDisplay) -> u64 {
        let g = NhgFrameGeom {
            native_w: 200,
            native_h: 100,
            bg_argb: 0xff20_4060,
            scale120: 120,
            layer: NhgRect {
                x: 0,
                y: 0,
                w: 200,
                h: 100,
            },
            ..Default::default()
        };
        let mut key = 0;
        let mut back = NhgBacking {
            pixels: ptr::null_mut(),
            stride: 0,
            w: 0,
            h: 0,
            generation: 0,
        };
        // SAFETY: a live display; the geometry, key and backing are valid for the calls.
        unsafe {
            assert!(nhg_layer_ensure(d, &mut key, &g) >= 0);
            assert_eq!(nhg_backing(d, key, &g, &mut back), 0);
        }
        key
    }

    /// Damage frame `key` whole and publish window `w` at a consistency point, as C does after a redraw.
    fn publish_whole(d: *mut NhgDisplay, w: u64, key: u64) {
        let s = NhgSurface {
            window: w,
            parent: 0,
            frame: key,
            flags: NHG_SURF_DRAWABLE,
            alpha_cut: 0,
        };
        // SAFETY: a live display; one readable surface.
        unsafe {
            assert!(nhg_damage_all(d, key) >= 0);
            let rc = nhg_present(d, w, &s, 1, NHG_PRESENT_BACKSTOP, 0);
            assert!(rc >= 0, "{rc}");
        }
        // the inline present at the root's FRAME_READY
        // SAFETY: a live display.
        if readable_within(unsafe { nhg_fd(d) }, 500) {
            for a in settle(d) {
                if a.kind == NHG_ACT_READY && a.window == w {
                    // SAFETY: a live display; no surfaces.
                    unsafe { nhg_present(d, w, ptr::null(), 0, NHG_PRESENT_READY, 0) };
                }
            }
        }
    }
}

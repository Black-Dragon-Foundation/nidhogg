//! T1 tests of window operations and tips through the glue, against nh-recorder (the shell's harness,
//! `../shell/tests/common`, and the step's, `step/mod.rs`): a window made through the glue starts from its facts, its
//! alpha falls back to the host, a size C sets itself makes the configure being applied a content ack, fullscreen and
//! back, destroying forgets, the wait for the first configure; a tip's move is a bare commit, and below
//! `xdg_wm_base` 3 it asks for a redraw.

#[path = "../../shell/tests/common/mod.rs"]
mod common;
mod step;

use common::{Recorder, number};
use nhglue::abi::*;
use nhglue::action::*;
use nhglue::backing::{NhgBacking, NhgFrameGeom, NhgRect};
use nhglue::ffi::*;
use nhglue::frame::*;
use nhglue::present::*;
use nhglue::tip::NHG_DEL_TIP;
use nhglue::window::*;
use nhshell::api::{NHSHELL_ACK_CONTENT, NHSHELL_KIND_CHILD, NHSHELL_KIND_TOPLEVEL};
use std::ffi::c_char;
use std::ptr;
use std::time::{Duration, Instant};
use step::*;

const TITLE: &str = "t";
const APP_ID: &str = "nidhogg.test";
const BG: u32 = 0xff20_4060;

/// A top-level's descriptor, 200 x 100 at scale 1, as `nhwl_create_frame_window` makes it.
fn desc() -> NhgWindowDesc {
    NhgWindowDesc {
        w: 200,
        h: 100,
        title: TITLE.as_ptr().cast::<c_char>(),
        title_len: TITLE.len(),
        app_id: APP_ID.as_ptr().cast::<c_char>(),
        app_id_len: APP_ID.len(),
        session_name: ptr::null(),
        session_name_len: 0,
        flags: 0,
        kind: NHSHELL_KIND_TOPLEVEL,
        parent: 0,
        x: 0,
        y: 0,
        anchor: NhgAnchor::default(),
        role: 0,
        facts: 0,
        native_w: 200,
        native_h: 100,
        scale120: 120,
    }
}

/// Make the window `w` describes through the glue: its code and id.
fn make(d: *mut NhgDisplay, w: &NhgWindowDesc) -> (i32, u64) {
    let mut id = 0;
    // SAFETY: a live display; `w` and its texts are readable, `id` writable.
    let rc = unsafe { nhg_window_new(d, w, &mut id) };
    (rc, id)
}

/// A top-level made through the glue, its first configure applied.
fn toplevel(d: *mut NhgDisplay) -> u64 {
    let (rc, id) = make(d, &desc());
    assert_eq!(rc, NHG_OK);
    configured(d, id);
    id
}

/// `f` of the glue's frame of `id`.
fn frame<T>(d: *mut NhgDisplay, id: u64, f: impl FnOnce(&Frame) -> T) -> T {
    // SAFETY: a live display.
    let d = unsafe { &*d };
    let g = d.lock_for_test();
    f(g.frame(id).expect("a registered window"))
}

fn alpha(d: *mut NhgDisplay, id: u64, a: f64) -> (i32, u32) {
    let mut cut = u32::MAX;
    // SAFETY: a live display; `cut` is writable.
    let rc = unsafe { nhg_window_alpha(d, id, a, 1.0, &mut cut) };
    (rc, cut)
}

/// The recorder's log of client requests.
fn requests(rec: &Recorder) -> String {
    rec.control(r#"{"cmd":"input_log","since":1}"#)
}

/// How many `xdg_toplevel.REQ` requests the log holds, and where the last one is.
fn request(log: &str, req: &str) -> (usize, Option<usize>) {
    let pat = format!(r#""req":"xdg_toplevel.{req}""#);
    (log.matches(&pat).count(), log.rfind(&pat))
}

/// Wait (3 s at most) until the log holds `n` `xdg_toplevel.REQ` requests; the log.
fn wait_requests(rec: &Recorder, req: &str, n: usize) -> String {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        let log = requests(rec);
        if request(&log, req).0 >= n {
            return log;
        }
        assert!(Instant::now() < deadline, "no {n} {req} requests: {log}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn geom(w: i32, h: i32) -> NhgFrameGeom {
    NhgFrameGeom {
        native_w: w,
        native_h: h,
        bg_argb: BG,
        scale120: 120,
        layer: NhgRect { x: 0, y: 0, w, h },
        ..Default::default()
    }
}

/// Draw `window`'s frame whole (`w` x `h`, its key in `*key`) and publish it at a consistency point, as C does.
fn draw(d: *mut NhgDisplay, window: u64, key: &mut u64, w: i32, h: i32) {
    let g = geom(w, h);
    let mut b = NhgBacking {
        pixels: ptr::null_mut(),
        stride: 0,
        w: 0,
        h: 0,
        generation: 0,
    };
    // SAFETY: a live display; `g`, the key and the backing are valid for the calls.
    unsafe {
        assert!(nhg_layer_ensure(d, key, &g) >= 0);
        assert_eq!(nhg_backing(d, *key, &g, &mut b), 0);
    }
    for i in 0..(b.w * b.h) as usize {
        // SAFETY: inside the borrowed backing.
        unsafe { *b.pixels.cast::<u32>().add(i) = BG };
    }
    // SAFETY: a live display.
    assert_eq!(unsafe { nhg_damage_all(d, *key) }, 0);
    let s = [NhgSurface {
        window,
        parent: 0,
        frame: *key,
        flags: NHG_SURF_DRAWABLE,
        alpha_cut: 0,
    }];
    // SAFETY: a live display; one readable surface.
    let rc = unsafe { nhg_present(d, window, s.as_ptr(), 1, NHG_PRESENT_BACKSTOP, 0) };
    assert!(rc >= 0, "present: {rc}");
}

/// `window`'s `READY` action, then the inline present it asks for.
fn ready(d: *mut NhgDisplay, window: u64) -> i32 {
    until(d, |a| a.kind == NHG_ACT_READY && a.window == window);
    present_ready(d, window)
}

/// The inline present a `READY` action of `window` asks for.
fn present_ready(d: *mut NhgDisplay, window: u64) -> i32 {
    // SAFETY: a live display; no surfaces.
    unsafe { nhg_present(d, window, ptr::null(), 0, NHG_PRESENT_READY, 0) }
}

fn next_seq(rec: &Recorder) -> u64 {
    number(&rec.control(r#"{"cmd":"log","since":1}"#), "next").expect("next")
}

/// Every commit from recorder commit `seq` on, one JSON text each, in order.
fn log(rec: &Recorder, seq: u64) -> Vec<String> {
    let log = rec.control(&format!(r#"{{"cmd":"log","since":{seq}}}"#));
    // the objects have sorted keys, so `ack_serial` opens each commit
    log.split("{\"ack_serial\":")
        .skip(1)
        .map(|c| format!("{{\"ack_serial\":{c}"))
        .collect()
}

fn of(c: &str, id: u64) -> bool {
    c.contains(&format!("\"surface\":{id},"))
}

fn has_buffer(c: &str) -> bool {
    c.contains("\"buffer\":{")
}

/// The tip's anchor point (device pixels in its parent) for the tests.
fn at(x: i32, y: i32) -> NhgRect {
    NhgRect { x, y, w: 1, h: 1 }
}

fn tip_show(d: *mut NhgDisplay, tip: &mut u64, parent: u64, a: NhgRect, size: (i32, i32)) -> i32 {
    // SAFETY: a live display; `tip` is readable and writable.
    unsafe { nhg_tip_show(d, tip, parent, a, 5, -10, size.0, size.1) }
}

/// A shown top-level on `profile` with a shown 60 x 30 tip at (20, 20) moved by (5, -10): the recorder, the display,
/// the parent, the tip and the tip's key.
fn shown_tip(profile: &str) -> (Recorder, *mut NhgDisplay, u64, u64, u64) {
    let rec = Recorder::start(profile);
    let d = open(&rec);
    let top = toplevel(d);
    let mut key = 0;
    draw(d, top, &mut key, 200, 100);
    ready(d, top);
    rec.wait_node(top, |n| n.contains("\"buffer\":{"));
    let mut tip = 0;
    assert_eq!(
        tip_show(d, &mut tip, top, at(20, 20), (60, 30)),
        1,
        "a new tip is drawn whole"
    );
    assert_ne!(tip, 0);
    frame(d, tip, |f| {
        assert_eq!(f.flags, NHG_FRAME_TIP);
        assert_eq!(f.facts.flags, NHG_FACT_VISIBLE);
        assert_eq!(
            (f.facts.native_w, f.facts.native_h, f.scale120),
            (60, 30, 120)
        );
    });
    // its first configure (the shell's: it acks a popup's, no RESIZE), then its picture: below and right of (25, 10)
    // logical
    until(d, |a| a.kind == NHG_ACT_READY && a.window == tip);
    let mut tkey = 0;
    draw(d, tip, &mut tkey, 60, 30);
    present_ready(d, tip);
    let n = rec.wait_node(tip, |n| n.contains("\"buffer\":{\"h\":30,\"w\":60}"));
    assert!(n.contains("\"position\":[26,11]"), "{n}");
    (rec, d, top, tip, tkey)
}

/// The plan names these `window::…`.
#[allow(clippy::module_inception)]
mod window {
    use super::*;

    /// A window made through the glue is registered with the facts it starts from (its native size, scale,
    /// visibility and title bar), as C pushed them one by one before; a child's role is kept.  Facts it does not
    /// know, an empty native size and an unknown role are refused, and nothing is made.
    #[test]
    fn window_new_seeds_the_facts() {
        let rec = Recorder::start("sway");
        let d = open(&rec);
        let mut w = desc();
        w.facts = NHG_FACT_VISIBLE | NHG_FACT_CSD;
        w.native_w = 300;
        w.native_h = 150;
        w.scale120 = 240;
        let (rc, top) = make(d, &w);
        assert_eq!(rc, NHG_OK);
        frame(d, top, |f| {
            assert_eq!(f.flags, 0);
            assert_eq!((f.facts.native_w, f.facts.native_h), (300, 150));
            assert_eq!(f.facts.flags, NHG_FACT_VISIBLE | NHG_FACT_CSD);
            assert_eq!(f.scale120, 240);
            assert!(!f.cfg.configured);
        });
        let mut c = desc();
        c.kind = NHSHELL_KIND_CHILD;
        c.role = NHG_FRAME_CHILD;
        c.parent = top;
        c.x = 10;
        c.y = 10;
        let (rc, child) = make(d, &c);
        assert_eq!(rc, NHG_OK);
        frame(d, child, |f| assert_eq!(f.flags, NHG_FRAME_CHILD));
        for bad in [
            NhgWindowDesc {
                facts: 1 << 7,
                ..desc()
            },
            NhgWindowDesc {
                native_w: 0,
                ..desc()
            },
            NhgWindowDesc {
                role: 1 << 7,
                ..desc()
            },
        ] {
            assert_eq!(make(d, &bad).0, NHG_E_ARG);
        }
        // SAFETY: a live display.
        assert_eq!(
            unsafe { &*d }.lock_for_test().frame_count(),
            2,
            "nothing registered for the refused ones"
        );
        close(d);
    }

    /// Without `wp_alpha_modifier_v1` (mutter's profile) the shell multiplies: the first alpha damages the frame and
    /// asks for the host; the same alpha again changes nothing; opaque again damages once more and the host is done.
    /// With the protocol (sway's) the compositor multiplies, and nothing is damaged.
    #[test]
    fn alpha_host_when_no_alpha_modifier() {
        let half = u32::MAX - (0.5 * f64::from(u32::MAX)) as u32;
        let rec = Recorder::start("mutter");
        let d = open(&rec);
        let id = toplevel(d);
        assert_eq!(alpha(d, id, 0.5), (NHG_ALPHA_DAMAGE | NHG_ALPHA_HOST, half));
        assert_eq!(alpha(d, id, 0.5), (NHG_ALPHA_HOST, half));
        assert_eq!(alpha(d, id, 1.0), (NHG_ALPHA_DAMAGE, 0));
        frame(d, id, |f| {
            assert_eq!((f.alpha_cut, f.alpha_host), (0, false))
        });
        assert_eq!(alpha(d, id, 1.5).0, NHG_E_ARG);
        close(d);

        let rec = Recorder::start("sway");
        let d = open(&rec);
        let id = toplevel(d);
        assert_eq!(alpha(d, id, 0.5), (0, half));
        frame(d, id, |f| {
            assert_eq!((f.alpha_cut, f.alpha_host), (half, false))
        });
        close(d);
    }

    /// C resizing the frame itself between configures records no ack (the size shows with the next buffer); while it
    /// applies a configure, the size it sets makes that configure's ack a content ack at once.  A configure applied
    /// without a new size is acked later, at the next step, without content.
    #[test]
    fn size_change_is_a_content_ack_later() {
        let rec = Recorder::start("sway");
        let d = open(&rec);
        let id = toplevel(d);
        let first = last_ack(d, id);
        // SAFETY: a live display.
        assert_eq!(unsafe { nhg_window_size(d, id, 300, 150) }, NHG_OK);
        assert_eq!(last_ack(d, id), first, "no configure being applied");
        frame(d, id, |f| {
            assert_eq!((f.facts.native_w, f.facts.native_h), (300, 150))
        });
        // SAFETY: a live display.
        assert_eq!(unsafe { nhg_window_size(d, id, 0, 150) }, NHG_E_ARG);

        configure(&rec, [320, 160], "");
        until(d, |a| a.kind == NHG_ACT_RESIZE && a.window == id);
        assert_eq!(last_ack(d, id), first, "due, not recorded yet");
        // SAFETY: a live display.
        assert_eq!(unsafe { nhg_window_size(d, id, 320, 160) }, NHG_OK);
        let content = last_ack(d, id).expect("recorded as C applied the configure");
        assert_ne!(Some(content), first);
        assert_eq!(content.1, NHSHELL_ACK_CONTENT);

        configure(&rec, [320, 160], "");
        until(d, |a| a.kind == NHG_ACT_RESIZE && a.window == id);
        assert_eq!(last_ack(d, id), Some(content), "due until the next step");
        step(d, 0);
        let later = last_ack(d, id).expect("recorded at the next step");
        assert_ne!(later.0, content.0);
        assert_eq!(later.1, 0, "no new size: no content");
        close(d);
    }

    /// The compositor's size, fitted while C applies its configure: a new size is given and recorded at once (a
    /// content ack, as `nhg_frame_resized`); the size the frame already has once rounded gives nothing.  A clamp with
    /// no border keeps 16384 a side; flags it does not know and an unknown window are refused.
    #[test]
    fn fit_gives_and_records_the_size() {
        use nhglue::csd::{NHG_FIT_CONFIGURE, NHG_FIT_UNDECORATED};
        let rec = Recorder::start("sway");
        let d = open(&rec);
        let id = toplevel(d);
        let first = last_ack(d, id);
        let fit = |lw: i32, lh: i32, native: (i32, i32), flags: u32| {
            let (mut w, mut h) = (-1, -1);
            // SAFETY: a live display; `w` and `h` are writable.
            let rc = unsafe {
                nhg_frame_fit(
                    d, id, 120, lw, lh, 0, native.0, native.1, flags, &mut w, &mut h,
                )
            };
            (rc, w, h)
        };
        configure(&rec, [320, 160], "");
        until(d, |a| a.kind == NHG_ACT_RESIZE && a.window == id);
        assert_eq!(fit(320, 160, (200, 100), NHG_FIT_CONFIGURE), (1, 320, 160));
        frame(d, id, |f| {
            assert_eq!((f.facts.native_w, f.facts.native_h), (320, 160))
        });
        let content = last_ack(d, id).expect("recorded as C applied the configure");
        assert_ne!(Some(content), first);
        assert_eq!(content.1, NHSHELL_ACK_CONTENT);

        assert_eq!(fit(320, 160, (320, 160), NHG_FIT_CONFIGURE), (0, -1, -1));
        assert_eq!(fit(0, 160, (320, 160), NHG_FIT_CONFIGURE), (0, -1, -1));
        assert_eq!(fit(320, 160, (200, 100), 0), (0, -1, -1));
        // not maximized, fullscreen or tiled: an undecorated change keeps the size
        assert_eq!(fit(320, 160, (200, 100), NHG_FIT_UNDECORATED), (0, -1, -1));
        assert_eq!(fit(320, 160, (200, 100), 4).0, NHG_E_ARG);
        let (mut w, mut h) = (0, 0);
        // SAFETY: a live display; `w` and `h` are writable.
        let unknown = unsafe { nhg_frame_fit(d, id + 1000, 120, 1, 1, 0, 2, 2, 1, &mut w, &mut h) };
        assert_eq!(unknown, NHG_E_STATE);

        let (mut w, mut h) = (20_000, 300);
        // SAFETY: a live display; `w` and `h` are readable and writable.
        assert_eq!(unsafe { nhg_frame_clamp(d, id, &mut w, &mut h) }, NHG_OK);
        assert_eq!((w, h), (16384, 300));
        let (mut w, mut h) = (300, 40_000);
        // SAFETY: as above; window 0 has no border.
        assert_eq!(unsafe { nhg_frame_clamp(d, 0, &mut w, &mut h) }, NHG_OK);
        assert_eq!((w, h), (300, 16384));
        close(d);
    }

    /// Fullscreen asks to leave maximized, then for fullscreen; back to none leaves fullscreen, then maximized;
    /// maximized, `fullwidth` and `fullheight` ask for maximized.  An unknown mode is refused.
    #[test]
    fn fullscreen_and_back() {
        let rec = Recorder::start("sway");
        let d = open(&rec);
        let id = toplevel(d);
        let fs = |mode: u32| {
            // SAFETY: a live display.
            unsafe { nhg_window_fullscreen(d, id, mode) }
        };
        let log0 = requests(&rec);
        let n = |log: &str, req: &str| request(log, req).0;

        assert_eq!(fs(NHG_FULLSCREEN_BOTH), NHG_OK);
        let log = wait_requests(&rec, "set_fullscreen", n(&log0, "set_fullscreen") + 1);
        let (unmax, full) = (
            request(&log, "unset_maximized"),
            request(&log, "set_fullscreen"),
        );
        assert_eq!(unmax.0, n(&log0, "unset_maximized") + 1);
        assert!(unmax.1 < full.1, "unset_maximized first: {log}");
        // the recorder answers with a configure
        until(d, |a| a.kind == NHG_ACT_RESIZE && a.window == id);

        assert_eq!(fs(NHG_FULLSCREEN_NONE), NHG_OK);
        let log = wait_requests(&rec, "unset_maximized", unmax.0 + 1);
        let unfull = request(&log, "unset_fullscreen");
        assert_eq!(unfull.0, n(&log0, "unset_fullscreen") + 1);
        assert!(
            unfull.1 < request(&log, "unset_maximized").1,
            "unset_fullscreen first: {log}"
        );

        let max0 = n(&log, "set_maximized");
        for mode in [
            NHG_FULLSCREEN_MAXIMIZED,
            NHG_FULLSCREEN_WIDTH,
            NHG_FULLSCREEN_HEIGHT,
        ] {
            assert_eq!(fs(mode), NHG_OK);
        }
        wait_requests(&rec, "set_maximized", max0 + 3);
        assert_eq!(fs(9), NHG_E_ARG);
        close(d);
    }

    /// Destroying a window forgets its frame: calls naming it are refused, and its surface leaves the compositor.
    #[test]
    fn destroy_forgets_the_frame() {
        let rec = Recorder::start("sway");
        let d = open(&rec);
        let id = toplevel(d);
        // SAFETY: a live display.
        unsafe {
            assert_eq!(nhg_window_destroy(d, id), NHG_OK);
            assert_eq!(
                nhg_window_title(d, id, TITLE.as_ptr().cast(), TITLE.len()),
                NHG_E_STATE
            );
            assert_eq!(nhg_window_visible(d, id, 1), NHG_E_STATE);
            assert_eq!(nhg_window_destroy(d, id), NHG_E_STATE);
        }
        {
            // SAFETY: a live display.
            let g = unsafe { &*d }.lock_for_test();
            assert!(g.is_forgotten(id));
            assert_eq!(g.frame_count(), 0);
        }
        settle(d);
        let deadline = Instant::now() + Duration::from_secs(3);
        while rec.surfaces("toplevel") != 0 {
            assert!(Instant::now() < deadline, "the toplevel stayed");
            std::thread::sleep(Duration::from_millis(10));
        }
        close(d);
    }

    /// The bounded wait: a window not configured yet waits (something queued, or a timeout) until C has drained its
    /// first configure; then it is configured at once.  A child frame's window never waits.
    #[test]
    fn wait_configured_until_the_first_configure() {
        let rec = Recorder::start("sway");
        let d = open(&rec);
        let (rc, id) = make(d, &desc());
        assert_eq!(rc, NHG_OK);
        let mut rounds = 0;
        loop {
            // SAFETY: a live display.
            let rc = unsafe { nhg_window_wait_configured(d, id, 20) };
            if rc == 1 {
                break;
            }
            assert!(rc == 0 || rc == NHG_E_TIMEOUT, "wait: {rc}");
            rounds += 1;
            assert!(rounds < 250, "never configured");
            // C drains what is queued
            while step(d, 0).0 == 1 {}
        }
        assert!(rounds >= 1, "it waited for the configure");
        let mut c = desc();
        c.kind = NHSHELL_KIND_CHILD;
        c.role = NHG_FRAME_CHILD;
        c.parent = id;
        let (rc, child) = make(d, &c);
        assert_eq!(rc, NHG_OK);
        // SAFETY: a live display.
        assert_eq!(unsafe { nhg_window_wait_configured(d, child, 20) }, 1);
        close(d);
    }

    /// The visibility pushes: visible, invisible, iconified (with the minimize request).
    #[test]
    fn visibility_is_pushed() {
        let rec = Recorder::start("sway");
        let d = open(&rec);
        let id = toplevel(d);
        let flags = || {
            frame(d, id, |f| {
                f.facts.flags & (NHG_FACT_VISIBLE | NHG_FACT_ICONIFIED)
            })
        };
        // SAFETY: a live display.
        unsafe {
            assert_eq!(nhg_window_visible(d, id, 1), NHG_OK);
            assert_eq!(flags(), NHG_FACT_VISIBLE);
            assert_eq!(nhg_window_iconify(d, id), NHG_OK);
            assert_eq!(flags(), NHG_FACT_ICONIFIED);
            assert_eq!(nhg_window_visible(d, id, 1), NHG_OK);
            assert_eq!(flags(), NHG_FACT_VISIBLE);
            assert_eq!(nhg_window_visible(d, id, 0), NHG_OK);
            assert_eq!(flags(), 0);
        }
        wait_requests(&rec, "set_minimized", 1);
        close(d);
    }
}

/// The plan names these `tip::…`.
mod tip {
    use super::*;

    /// Only the offsets changed (sway, `xdg_wm_base` 6): the popup moves where it is (`reposition`), the call asks
    /// for no redraw, and the move's commit carries no buffer.
    #[test]
    fn dx_dy_move_is_a_bare_commit() {
        let (rec, d, top, tip, _) = shown_tip("sway");
        let seq = next_seq(&rec);
        let mut w = tip;
        assert_eq!(tip_show(d, &mut w, top, at(40, 40), (0, 0)), 0, "no redraw");
        assert_eq!(w, tip, "the same popup");
        // the reposition's configure is acked with a bare commit, once it is in (no action marks it; the shown tip's
        // frame callbacks are READYs too)
        settle(d);
        present_ready(d, tip);
        rec.wait_node(tip, |n| n.contains("\"position\":[46,31]"));
        let commits: Vec<String> = log(&rec, seq).into_iter().filter(|c| of(c, tip)).collect();
        assert!(!commits.is_empty(), "the move committed");
        assert!(
            commits.iter().all(|c| !has_buffer(c)),
            "a bare commit: {commits:?}"
        );
        close(d);
    }

    /// Below `xdg_wm_base` 3 (`xdg-v2`) the popup is made again under the same id: the call asks for the whole tip
    /// again, and the redraw maps it at its new place.
    #[test]
    fn pre_v3_recreate_asks_for_a_redraw() {
        let (rec, d, top, tip, mut key) = shown_tip("xdg-v2");
        let mut w = tip;
        assert_eq!(
            tip_show(d, &mut w, top, at(40, 40), (0, 0)),
            1,
            "redraw all"
        );
        assert_eq!(w, tip, "the same id");
        frame(d, tip, |f| assert_eq!(f.flags, NHG_FRAME_TIP));
        // the new popup's configure in (no action marks it; the old one's frame callbacks are READYs too)
        settle(d);
        draw(d, tip, &mut key, 60, 30);
        present_ready(d, tip);
        let n = rec.wait_node(tip, |n| n.contains("\"position\":[46,31]"));
        assert!(n.contains("\"buffer\":{\"h\":30,\"w\":60}"), "{n}");
        close(d);
    }

    /// The compositor dismissing a tip gives a `DELETE` action flagged `NHG_DEL_TIP`; hiding it then forgets it, and
    /// a window that is not a tip's is neither moved nor hidden as one.
    #[test]
    fn dismissal_is_a_delete_action() {
        let (rec, d, top, tip, _) = shown_tip("sway");
        rec.control_ok(&format!(r#"{{"cmd":"popup_done","surface":{tip}}}"#));
        let seen = until(d, |a| a.kind == NHG_ACT_DELETE && a.window == tip);
        let del = seen.last().unwrap();
        assert_eq!(del.flags, NHG_DEL_TIP);
        // SAFETY: a live display.
        unsafe {
            assert_eq!(nhg_tip_hide(d, top), NHG_E_STATE, "not a tip");
            assert_eq!(nhg_tip_hide(d, tip), NHG_OK);
            assert_eq!(nhg_tip_hide(d, tip), NHG_E_STATE, "forgotten");
        }
        let mut w = top;
        assert_eq!(tip_show(d, &mut w, top, at(1, 1), (0, 0)), NHG_E_STATE);
        close(d);
    }
}

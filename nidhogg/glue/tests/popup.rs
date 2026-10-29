//! T1 tests of menu popups through the glue, against nh-recorder (the shell's harness, `../shell/tests/common`, and
//! the step's, `step/mod.rs`): the compositor's dismissal is a `DELETE` action, a grabbing popup holds its top-level's
//! focus-out (and hiding it after the focus went elsewhere says so), no press serial means no grab, the parameter's
//! size and gravity apply at the next show, and a popup is the root of its own tree.

#[path = "../../shell/tests/common/mod.rs"]
mod common;
mod step;

use common::{Recorder, number};
use nhglue::abi::*;
use nhglue::action::*;
use nhglue::backing::{NhgBacking, NhgFrameGeom, NhgRect};
use nhglue::ffi::*;
use nhglue::frame::*;
use nhglue::popup::*;
use nhglue::present::*;
use std::ptr;
use step::*;

const BG: u32 = 0xff20_4060;
/// `NhgAnchor` gravities.
const BOTTOM_LEFT: u32 = 6;
const BOTTOM_RIGHT: u32 = 8;

/// `f` of the glue's frame of `id`.
fn frame<T>(d: *mut NhgDisplay, id: u64, f: impl FnOnce(&Frame) -> T) -> T {
    // SAFETY: a live display.
    let d = unsafe { &*d };
    let g = d.lock_for_test();
    f(g.frame(id).expect("a registered window"))
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

/// Draw `window`'s frame whole (`w` x `h`) as the root of its own tree, and publish it at a consistency point, as C
/// does.
fn draw(d: *mut NhgDisplay, window: u64, w: i32, h: i32) {
    let g = geom(w, h);
    let mut key = 0;
    let mut b = NhgBacking {
        pixels: ptr::null_mut(),
        stride: 0,
        w: 0,
        h: 0,
        generation: 0,
    };
    // SAFETY: a live display; `g`, the key and the backing are valid for the calls.
    unsafe {
        assert!(nhg_layer_ensure(d, &mut key, &g) >= 0);
        assert_eq!(nhg_backing(d, key, &g, &mut b), 0);
    }
    for i in 0..(b.w * b.h) as usize {
        // SAFETY: inside the borrowed backing.
        unsafe { *b.pixels.cast::<u32>().add(i) = BG };
    }
    // SAFETY: a live display.
    assert_eq!(unsafe { nhg_damage_all(d, key) }, 0);
    let s = [NhgSurface {
        window,
        parent: 0,
        frame: key,
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

/// A shown 200 x 100 top-level on `profile`, drawn and mapped: the recorder, the display and the window.
fn shown_top(profile: &str) -> (Recorder, *mut NhgDisplay, u64) {
    let rec = Recorder::start(profile);
    let d = open(&rec);
    let top = window(d);
    configured(d, top);
    draw(d, top, 200, 100);
    ready(d, top);
    rec.wait_node(top, |n| n.contains("\"buffer\":{"));
    (rec, d, top)
}

/// `nhg_popup_show` of a popup of `parent` (hanging from top-level `owner`) at `anchor` with `gravity`, no adjust and
/// `flags`, `size` device pixels: its code and window.
fn show(
    d: *mut NhgDisplay,
    (parent, owner): (u64, u64),
    anchor: NhgRect,
    gravity: u32,
    flags: u32,
    size: (i32, i32),
) -> (i32, u64) {
    let mut id = 0;
    // SAFETY: a live display; `id` is writable.
    let rc = unsafe {
        nhg_popup_show(
            d, &mut id, parent, owner, anchor, gravity, 0, flags, size.0, size.1,
        )
    };
    (rc, id)
}

fn rect(x: i32, y: i32, w: i32, h: i32) -> NhgRect {
    NhgRect { x, y, w, h }
}

/// A shown 60 x 30 popup of `top` beside (100, 50, 4, 4), growing bottom-right, with `flags`: its first configure
/// applied and its picture committed.
fn shown_popup(rec: &Recorder, d: *mut NhgDisplay, top: u64, flags: u32) -> u64 {
    let (rc, p) = show(
        d,
        (top, top),
        rect(100, 50, 4, 4),
        BOTTOM_RIGHT,
        flags,
        (60, 30),
    );
    assert_eq!(rc, NHG_OK);
    assert_ne!(p, 0);
    // a popup's configures are the shell's (it acks them; no RESIZE): its first one makes it ready, and it is drawn
    // then
    until(d, |a| a.kind == NHG_ACT_READY && a.window == p);
    draw(d, p, 60, 30);
    present_ready(d, p);
    let n = rec.wait_node(p, |n| n.contains("\"buffer\":{\"h\":30,\"w\":60}"));
    assert!(n.contains("\"position\":[104,54]"), "{n}");
    p
}

/// The recorder gives `surface` the keyboard and the pointer and presses the left button over it; the glue has taken
/// the press.
fn press(rec: &Recorder, d: *mut NhgDisplay, surface: u64) {
    rec.control_ok(&format!(r#"{{"cmd":"focus","surface":{surface}}}"#));
    rec.control_ok(r#"{"cmd":"pointer","pos":[5,5]}"#);
    rec.control_ok(r#"{"cmd":"button","button":272,"state":"press"}"#);
    until(d, |a| a.kind == NHG_ACT_CLICK);
    settle(d);
}

fn is_focus(a: &NhgAction) -> bool {
    a.kind == NHG_ACT_FOCUS_IN || a.kind == NHG_ACT_FOCUS_OUT
}

/// The plan names these `popup::…`.
#[allow(clippy::module_inception)]
mod popup {
    use super::*;

    /// The compositor dismissing a popup frame's popup gives a `DELETE` action flagged `NHG_DEL_POPUP`; the frame
    /// stays until C hides it, which forgets the window; a top-level is not hidden as a popup.
    #[test]
    fn dismissal_is_a_delete_action() {
        let (rec, d, top) = shown_top("sway");
        let p = shown_popup(&rec, d, top, 0);
        frame(d, p, |f| {
            assert_eq!(f.flags, NHG_FRAME_POPUP);
            assert_eq!(f.facts.flags, NHG_FACT_VISIBLE);
            assert_eq!((f.facts.native_w, f.facts.native_h), (60, 30));
        });
        rec.control_ok(&format!(r#"{{"cmd":"popup_done","surface":{p}}}"#));
        let seen = until(d, |a| a.kind == NHG_ACT_DELETE && a.window == p);
        assert_eq!(seen.last().unwrap().flags, NHG_DEL_POPUP);
        // SAFETY: a live display.
        unsafe {
            assert_eq!(nhg_popup_hide(d, top, top), NHG_E_STATE, "not a popup");
            assert_eq!(nhg_popup_hide(d, p, top), 0);
            assert_eq!(nhg_popup_size(d, p, 80, 40), NHG_E_STATE, "forgotten");
            assert_eq!(nhg_popup_hide(d, p, top), 0, "passed over");
        }
        assert_eq!(rec.surfaces("popup"), 0);
        close(d);
    }

    /// Review Focus 2 of the menus: while a popup of its own grabs, the top-level losing the keyboard to it gives no
    /// focus action, the popup's own focus gives none, and keys on the popup name it.  After the compositor dismissed
    /// the popup and the keyboard went to none of our windows, hiding it says the top-level lost the focus it lent.
    #[test]
    fn grab_holds_the_parent_focus_out() {
        let (rec, d, top) = shown_top("sway");
        press(&rec, d, top);
        let p = shown_popup(&rec, d, top, NHG_POPUP_GRAB);
        rec.wait_node(p, |n| n.contains("\"grab\":{\"serial_kind\":\"button\"}"));
        assert!(backend(d).window_grabbed(p).unwrap());
        rec.control_ok(&format!(r#"{{"cmd":"focus","surface":{p}}}"#));
        let seen = settle(d);
        assert!(!seen.iter().any(is_focus), "{seen:?}");
        assert!(frame(d, top, |f| f.focus_lent));
        rec.control_ok(r#"{"cmd":"key","text":"x"}"#);
        let seen = until(d, |a| a.kind == NHG_ACT_KEY);
        assert_eq!(seen.last().unwrap().window, p);
        // dismissed, then the keyboard leaves us: still no focus action until the popup is hidden
        rec.control_ok(r#"{"cmd":"click_outside"}"#);
        until(d, |a| a.kind == NHG_ACT_DELETE && a.window == p);
        assert!(!backend(d).window_grabbed(p).unwrap());
        rec.control_ok(r#"{"cmd":"focus","surface":null}"#);
        let seen = settle(d);
        assert!(!seen.iter().any(is_focus), "{seen:?}");
        // SAFETY: a live display.
        assert_eq!(
            unsafe { nhg_popup_hide(d, p, top) },
            1,
            "the lent focus is lost"
        );
        assert!(!frame(d, top, |f| f.focus_lent));
        // focus back on the top-level is a focus change again
        rec.control_ok(&format!(r#"{{"cmd":"focus","surface":{top}}}"#));
        let seen = until(d, is_focus);
        let a = seen.last().unwrap();
        assert_eq!((a.kind, a.window), (NHG_ACT_FOCUS_IN, top));
        close(d);
    }

    /// No press, no serial: the popup asks for a grab and gets none, and the top-level losing the keyboard is a
    /// focus change as usual.
    #[test]
    fn no_serial_no_grab() {
        let (rec, d, top) = shown_top("sway");
        rec.control_ok(&format!(r#"{{"cmd":"focus","surface":{top}}}"#));
        until(d, |a| a.kind == NHG_ACT_FOCUS_IN && a.window == top);
        let p = shown_popup(&rec, d, top, NHG_POPUP_GRAB);
        rec.wait_node(p, |n| {
            n.contains("\"role\":\"popup\"") && n.contains("\"grab\":null")
        });
        assert!(!backend(d).window_grabbed(p).unwrap());
        rec.control_ok(r#"{"cmd":"focus","surface":null}"#);
        let seen = until(d, is_focus);
        let a = seen.last().unwrap();
        assert_eq!((a.kind, a.window), (NHG_ACT_FOCUS_OUT, top));
        assert!(!frame(d, top, |f| f.focus_lent));
        // SAFETY: a live display.
        assert_eq!(unsafe { nhg_popup_hide(d, p, top) }, 0);
        close(d);
    }

    /// A new parameter moves the shown popup to its anchor's corner and keeps the rest for the comparisons; the
    /// anchor's size and the gravity apply when the popup is shown next.  The frame's own size goes to the popup at
    /// once (same logical size: nothing to do).
    #[test]
    fn size_and_gravity_apply_at_next_show() {
        let (rec, d, top) = shown_top("sway");
        let p = shown_popup(&rec, d, top, 0);
        let same = rect(100, 50, 4, 4);
        let next = rect(100, 50, 20, 20);
        // SAFETY: a live display.
        unsafe {
            assert_eq!(
                nhg_popup_place(d, p, same, BOTTOM_RIGHT, 0, 0),
                0,
                "unchanged"
            );
            assert_eq!(
                nhg_popup_place(d, p, next, BOTTOM_LEFT, 0, 0),
                0,
                "moved, not made again"
            );
            assert_eq!(
                nhg_popup_place(d, p, next, 0, 0, 0),
                NHG_E_ARG,
                "no gravity 0"
            );
            assert_eq!(nhg_popup_size(d, p, 60, 30), 0, "same size");
            assert_eq!(nhg_popup_size(d, p, 80, 40), 0);
        }
        frame(d, p, |f| {
            let pp = f.popup.expect("a popup's state");
            assert_eq!((pp.place.anchor, pp.place.gravity), (next, BOTTOM_LEFT));
            assert_eq!(pp.sent, (80, 40));
        });
        // still at the old anchor's corner, as shown: the reposition keeps the gravity and the anchor's size
        // the reposition's configure in (no action marks it; the shown popup's frame callbacks are READYs too)
        settle(d);
        draw(d, p, 80, 40);
        present_ready(d, p);
        let n = rec.wait_node(p, |n| n.contains("\"buffer\":{\"h\":40,\"w\":80}"));
        assert!(n.contains("\"position\":[104,54]"), "{n}");
        // SAFETY: a live display.
        assert_eq!(unsafe { nhg_popup_hide(d, p, top) }, 0);
        // shown again: below and left of the new anchor
        let (rc, q) = show(d, (top, top), next, BOTTOM_LEFT, 0, (60, 30));
        assert_eq!(rc, NHG_OK);
        until(d, |a| a.kind == NHG_ACT_READY && a.window == q);
        draw(d, q, 60, 30);
        present_ready(d, q);
        let n = rec.wait_node(q, |n| n.contains("\"buffer\":{\"h\":30,\"w\":60}"));
        assert!(n.contains("\"position\":[40,70]"), "{n}");
        close(d);
    }

    /// A popup frame is the root of its own tree: its first configure, its `READY` and its present are its own, and
    /// presenting it commits nothing of the top-level's.
    #[test]
    fn is_its_own_tree_root() {
        let (rec, d, top) = shown_top("sway");
        let seq = next_seq(&rec);
        let p = shown_popup(&rec, d, top, 0);
        frame(d, p, |f| assert_eq!(f.flags & NHG_FRAME_CHILD, 0));
        let commits = log(&rec, seq);
        assert!(commits.iter().any(|c| of(c, p)), "{commits:?}");
        assert!(
            commits.iter().all(|c| !of(c, top)),
            "the popup's present committed the top-level: {commits:?}"
        );
        close(d);
    }
}

//! T1 tests of the title bar (`csd.rs`) against nh-recorder (the shell's harness, `../shell/tests/common`).  The
//! recorder's `mutter` profile offers no server-side decorations, so its windows draw their own title bar; `sway`'s
//! grants them.  Each test stands in for C: it makes the window as `nhwl_window_new` does, lays the border out where C
//! does (after it applies a configure), calls the publish step's `nhg_csd_paint` with the native size, and reads the
//! actions the event step gives.
//!
//! At scale 1, a 200 x 100 native area gets margins of 8 and a strip of 32: the backing is 216 x 148, the strip runs
//! from (8, 8) to (208, 40), and the default buttons (minimize, maximize, close) sit at x 112, 144 and 176.
//!
//! An undecorated window where the compositor offers no server-side decorations keeps the margins of 8 without the
//! strip; with the look's chrome (radius 12, blur 16, offset (0, 6)) the shadow needs ceil(1.5 x 16) + 1 = 25 on the
//! sides, 19 above and 31 below, and the input region is the window and the 8 around it.  Those rows publish for real
//! (the layer, the backing, a present) and read what the recorder applied and shows: the surface sits at the output's
//! top-left.

#[path = "../../shell/tests/common/mod.rs"]
mod common;

use common::{Recorder, readable_within};
use nhglue::abi::*;
use nhglue::action::*;
use nhglue::backing::{NhgBacking, NhgFrameGeom, NhgRect};
use nhglue::child::NhgChrome;
use nhglue::chrome;
use nhglue::csd::*;
use nhglue::ffi::*;
use nhglue::frame::NHG_FACT_CSD;
use nhglue::present::{NHG_PRESENT_BACKSTOP, NHG_PRESENT_READY, NHG_SURF_DRAWABLE, NhgSurface};
use nhshell::api::{
    NHSHELL_ACK_CONTENT, NHSHELL_CURSOR_DEFAULT, NHSHELL_CURSOR_RESIZE_EW,
    NHSHELL_CURSOR_RESIZE_NS, NHSHELL_CURSOR_RESIZE_NWSE, NHSHELL_WIN_UNDECORATED, ShellBackend,
    WindowDesc,
};
use std::ptr;
use std::time::{Duration, Instant};

fn open(rec: &Recorder) -> *mut NhgDisplay {
    let name = rec.display();
    // SAFETY: the name is NUL-terminated; no error buffer.
    let d = unsafe { nhg_open(name.as_ptr(), ptr::null_mut(), 0) };
    assert!(!d.is_null(), "nhg_open failed on the recorder");
    d
}

fn backend<'a>(d: *mut NhgDisplay) -> &'a mut dyn ShellBackend {
    // SAFETY: a live display; its backend lives as long as it does, and no glue call runs meanwhile.
    unsafe { &mut *(*d).backend_ptr() }
}

fn close(d: *mut NhgDisplay) {
    // SAFETY: a live display, not used again.
    assert_eq!(unsafe { nhg_close(d) }, NHG_OK);
}

/// One step: its return and a copy of its actions.
fn step(d: *mut NhgDisplay) -> (i32, Vec<NhgAction>) {
    let mut list = NhgActionList {
        v: ptr::null(),
        n: 0,
    };
    // SAFETY: a live display; `list` is writable.
    let rc = unsafe { nhg_next(d, ptr::null(), &mut list) };
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
        let (rc, v) = step(d);
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

/// Step until nothing comes for 200 ms; every action seen, one list per event.
fn settle(d: *mut NhgDisplay) -> Vec<Vec<NhgAction>> {
    let mut seen = Vec::new();
    loop {
        let (rc, v) = step(d);
        assert!(rc >= 0, "nhg_next failed: {rc}");
        if rc == 1 {
            seen.push(v);
        }
        // SAFETY: a live display.
        if rc == 0 && !readable_within(unsafe { nhg_fd(d) }, 200) {
            return seen;
        }
    }
}

/// A top-level window of 200 x 100, made and registered as `nhwl_window_new` makes it, at scale 1.
fn window(d: *mut NhgDisplay) -> u64 {
    window_with(d, 0)
}

/// `window` with the shell's window `flags` (`NHSHELL_WIN_*`).
fn window_with(d: *mut NhgDisplay, flags: u32) -> u64 {
    let id = backend(d)
        .window_new(&WindowDesc {
            w: 200,
            h: 100,
            title: "t".into(),
            app_id: "nidhogg.test".into(),
            flags,
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

/// Lay the border of `id` out, as C does when it applies a configure or a scale: the return and the layout.
fn lay(d: *mut NhgDisplay, id: u64, s120: u32, flags: u32) -> (i32, NhgCsdLayout) {
    let mut l = NhgCsdLayout::default();
    // SAFETY: a live display; `l` is writable.
    let rc = unsafe { nhg_csd_layout(d, id, s120, flags, &mut l) };
    (rc, l)
}

/// The window's next configure taken and applied (C lays the border out), its ack recorded at the next step.
fn apply_configure(d: *mut NhgDisplay, id: u64) -> NhgAction {
    let a = *until(d, |a| a.kind == NHG_ACT_RESIZE && a.window == id)
        .last()
        .unwrap();
    assert!(lay(d, id, 120, 0).0 >= 0);
    step(d);
    a
}

/// The window's first configure applied, with its title bar laid out at scale 1.
fn configured(d: *mut NhgDisplay, id: u64) {
    apply_configure(d, id);
    assert_eq!(last_ack(d, id).map(|(_, f)| f), Some(NHSHELL_ACK_CONTENT));
    settle(d);
}

fn last_ack(d: *mut NhgDisplay, id: u64) -> Option<(u32, u32)> {
    // SAFETY: a live display.
    let d = unsafe { &*d };
    d.lock_for_test().frame(id).unwrap().last_ack
}

fn facts_flags(d: *mut NhgDisplay, id: u64) -> u32 {
    // SAFETY: a live display.
    let d = unsafe { &*d };
    d.lock_for_test().frame(id).unwrap().facts.flags
}

/// The button under the pointer and the armed one.
fn buttons_state(d: *mut NhgDisplay, id: u64) -> (u32, u32) {
    // SAFETY: a live display.
    let d = unsafe { &*d };
    d.lock_for_test().frame(id).unwrap().csd.buttons_state()
}

fn configure(rec: &Recorder, size: [i32; 2], states: &str) {
    rec.control_ok(&format!(
        r#"{{"cmd":"configure","surface":"focused","size":[{},{}],"states":[{states}]}}"#,
        size[0], size[1]
    ));
}

/// The publish step for a native size of `w` x `h`: the return and the draw list.
fn paint(d: *mut NhgDisplay, id: u64, w: i32, h: i32, look: u64) -> (i32, NhgCsdPaint) {
    let mut p = NhgCsdPaint::default();
    // SAFETY: a live display; `p` is writable.
    let rc = unsafe { nhg_csd_paint(d, id, w, h, look, &mut p) };
    (rc, p)
}

fn title_bar(d: *mut NhgDisplay, id: u64, w: i32, h: i32) -> (i32, NhgCsdPaint) {
    let mut p = NhgCsdPaint::default();
    // SAFETY: a live display; `p` is writable.
    let rc = unsafe { nhg_csd_title_bar(d, id, w, h, &mut p) };
    (rc, p)
}

fn style(d: *mut NhgDisplay, left: &[u32], right: &[u32]) {
    // SAFETY: a live display; both slices are readable.
    let rc = unsafe { nhg_csd_style(d, left.as_ptr(), left.len(), right.as_ptr(), right.len()) };
    assert_eq!(rc, NHG_OK);
}

/// The buttons of a draw list: kind and x.
fn cells(p: &NhgCsdPaint) -> Vec<(u32, i32)> {
    p.button[..p.nbuttons as usize]
        .iter()
        .map(|b| (b.kind, b.r.x))
        .collect()
}

/// The pointer moved to (`x`, `y`) of the surface (logical, so device at scale 1): the events' actions.
fn point(rec: &Recorder, d: *mut NhgDisplay, x: i32, y: i32) -> Vec<Vec<NhgAction>> {
    rec.control_ok(&format!(r#"{{"cmd":"pointer","pos":[{x},{y}]}}"#));
    settle(d)
}

fn button(rec: &Recorder, d: *mut NhgDisplay, press: bool) -> Vec<Vec<NhgAction>> {
    let state = if press { "press" } else { "release" };
    rec.control_ok(&format!(
        r#"{{"cmd":"button","button":272,"state":"{state}"}}"#
    ));
    settle(d)
}

/// The last title-bar action among `events`.
fn last_title_bar(events: &[Vec<NhgAction>]) -> Option<NhgAction> {
    events
        .iter()
        .flatten()
        .rev()
        .find(|a| a.kind == NHG_ACT_TITLE_BAR)
        .copied()
}

/// Whether a pointer event went on past the title bar, as the pointer's own action.
fn any_pointer(events: &[Vec<NhgAction>]) -> bool {
    events
        .iter()
        .flatten()
        .any(|a| [NHG_ACT_MOTION, NHG_ACT_LEAVE, NHG_ACT_CLICK].contains(&a.kind))
}

/// A mapped CSD window on `mutter`, painted at 200 x 100, with the pointer's focus on it.
fn csd_window(rec: &Recorder) -> (*mut NhgDisplay, u64) {
    let d = open(rec);
    let id = window(d);
    configured(d, id);
    assert_eq!(paint(d, id, 200, 100, 1).0, 1);
    rec.control_ok(r#"{"cmd":"focus","surface":"toplevel"}"#);
    settle(d);
    (d, id)
}

const BG: u32 = 0xff20_4060;

/// The look's window chrome: radius 12, the popups' shadow (blur 16, offset (0, 6), 0.35), in logical pixels.
const LOOK: NhgChrome = NhgChrome {
    on: 1,
    radius: 12.0,
    blur: 16.0,
    ox: 0.0,
    oy: 6.0,
    spread: 0.0,
    opacity: 0.35,
    rgb: 0,
};

/// Corners only: no shadow.
const CORNERS: NhgChrome = NhgChrome {
    opacity: 0.0,
    blur: 0.0,
    oy: 0.0,
    ..LOOK
};

/// Push `c` as the chrome vector C keeps for `id` (`nhg_child_chrome`): its return.
fn chrome_of(d: *mut NhgDisplay, id: u64, c: &NhgChrome) -> i32 {
    let v = chrome::encode(c);
    // SAFETY: a live display; `v` is readable.
    unsafe { nhg_child_chrome(d, id, v.as_ptr(), v.len()) }
}

/// An undecorated window of 200 x 100 (it asks for client-side mode), its first configure applied with the border
/// laid out undecorated, as C does.
fn undecorated(d: *mut NhgDisplay) -> u64 {
    let id = window_with(d, NHSHELL_WIN_UNDECORATED);
    apply_undecorated(d, id);
    settle(d);
    id
}

/// The window's next configure taken and applied with the border laid out undecorated.
fn apply_undecorated(d: *mut NhgDisplay, id: u64) {
    until(d, |a| a.kind == NHG_ACT_RESIZE && a.window == id);
    assert!(lay(d, id, 120, NHG_CSD_UNDECORATED).0 >= 0);
    step(d);
}

/// The border as laid out: L, T, R, B.
fn border(d: *mut NhgDisplay, id: u64) -> [i32; 4] {
    let mut off = [0; 4];
    // SAFETY: a live display; `off` holds 4 values.
    assert_eq!(
        unsafe { nhg_frame_offsets(d, id, off.as_mut_ptr()) },
        NHG_OK
    );
    off
}

/// Publish `id` at 200 x 100 as `nhwl_surface_add` does: the title bar's step, the layer and the backing at the
/// frame's border (all of it `BG`, the border too), a present, and the commit its `READY` asks for.  `key` is the
/// frame's key, 0 the first time.
fn show(d: *mut NhgDisplay, id: u64, key: &mut u64) {
    assert!(paint(d, id, 200, 100, 1).0 >= 0);
    let [l, t, r, b] = border(d, id);
    let g = NhgFrameGeom {
        native_w: 200,
        native_h: 100,
        l,
        t,
        r,
        b,
        bg_argb: BG,
        scale120: 120,
        layer: NhgRect {
            x: 0,
            y: 0,
            w: 200 + l + r,
            h: 100 + t + b,
        },
        window: id,
        ..Default::default()
    };
    let mut back = NhgBacking {
        pixels: ptr::null_mut(),
        stride: 0,
        w: 0,
        h: 0,
        generation: 0,
    };
    // SAFETY: a live display; `g`, the key and the backing are valid for the calls.
    unsafe {
        assert!(nhg_layer_ensure(d, key, &g) >= 0);
        assert_eq!(nhg_backing(d, *key, &g, &mut back), 0);
    }
    for i in 0..(back.w * back.h) as usize {
        // SAFETY: inside the borrowed backing.
        unsafe { *back.pixels.cast::<u32>().add(i) = BG };
    }
    let s = [NhgSurface {
        window: id,
        parent: 0,
        frame: *key,
        flags: NHG_SURF_DRAWABLE,
        alpha_cut: 0,
    }];
    // SAFETY: a live display; one readable surface.
    unsafe {
        assert_eq!(nhg_damage_all(d, *key), 0);
        assert!(nhg_present(d, id, s.as_ptr(), 1, NHG_PRESENT_BACKSTOP, 0) >= 0);
    }
    until(d, |a| a.kind == NHG_ACT_READY && a.window == id);
    // SAFETY: a live display; no surfaces.
    assert!(unsafe { nhg_present(d, id, ptr::null(), 0, NHG_PRESENT_READY, 0) } >= 0);
}

/// The pixel at (`x`, `y`) of the output as the recorder shows it now (premultiplied ARGB).
fn shown(rec: &Recorder, x: usize, y: usize) -> u32 {
    Recorder::snapshot_pixel(&rec.snapshot(), x, y)
}

/// The recorder's log of requests contains `req`.
fn logged(rec: &Recorder, req: &str) -> bool {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        if rec
            .control(r#"{"cmd":"input_log","since":1}"#)
            .contains(req)
        {
            return true;
        }
        if Instant::now() > deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// The plan names these `csd::…`.
#[allow(clippy::module_inception)]
mod csd {
    use super::*;

    /// The border follows the compositor's answer and the scale; the buttons follow the style (the default set before Lisp
    /// says), with the title box between them; an undecorated frame has no strip, only the margins.
    #[test]
    fn layout_follows_the_style() {
        let rec = Recorder::start("mutter");
        let d = open(&rec);
        let id = window(d);
        configured(d, id);
        let want = NhgCsdLayout {
            l: 8,
            t: 40,
            r: 8,
            b: 8,
            strip: 32,
            on: 1,
        };
        assert_eq!(lay(d, id, 120, 0), (0, want), "the same layout again");
        assert_ne!(facts_flags(d, id) & NHG_FACT_CSD, 0);
        let mut off = [0; 4];
        // SAFETY: a live display; `off` holds 4 values.
        assert_eq!(
            unsafe { nhg_frame_offsets(d, id, off.as_mut_ptr()) },
            NHG_OK
        );
        assert_eq!(off, [8, 40, 8, 8]);
        // the default buttons
        let (rc, p) = title_bar(d, id, 200, 100);
        assert_eq!(rc, 1);
        let strip = NhgRect {
            x: 8,
            y: 8,
            w: 200,
            h: 32,
        };
        assert_eq!(p.strip, strip);
        assert_eq!(
            cells(&p),
            [(NHG_CSD_MIN, 112), (NHG_CSD_MAX, 144), (NHG_CSD_CLOSE, 176)]
        );
        assert_eq!(
            p.title,
            NhgRect {
                x: 8,
                y: 8,
                w: 104,
                h: 32
            }
        );
        // GNOME's "close:appmenu,minimize"; an unknown code is ignored
        style(d, &[NHG_CSD_CLOSE, 99], &[NHG_CSD_MENU, NHG_CSD_MIN]);
        let (_, p) = title_bar(d, id, 200, 100);
        assert_eq!(
            cells(&p),
            [(NHG_CSD_CLOSE, 8), (NHG_CSD_MENU, 144), (NHG_CSD_MIN, 176)]
        );
        assert_eq!(
            p.title,
            NhgRect {
                x: 40,
                y: 8,
                w: 104,
                h: 32
            }
        );
        // at scale 2 the strip is 64 and the margins 16
        assert_eq!(
            lay(d, id, 240, 0),
            (
                1,
                NhgCsdLayout {
                    l: 16,
                    t: 80,
                    r: 16,
                    b: 16,
                    strip: 64,
                    on: 1
                }
            )
        );
        // at 1.5, round(32 x 1.5) = 48 and round(8 x 1.5) = 12
        assert_eq!(lay(d, id, 180, 0).1.t, 60);
        // undecorated: the margins, no strip
        assert_eq!(
            lay(d, id, 120, NHG_CSD_UNDECORATED),
            (
                1,
                NhgCsdLayout {
                    l: 8,
                    t: 8,
                    r: 8,
                    b: 8,
                    strip: 0,
                    on: 0
                }
            )
        );
        assert_eq!(facts_flags(d, id) & NHG_FACT_CSD, 0);
        assert_eq!(title_bar(d, id, 200, 100).0, 0);
        close(d);
    }

    /// A window made anew is laid out from nothing: once the compositor has said it decorates the window, its layout
    /// is "unchanged" (0), yet the whole layout is written, which C copies over what it kept for the frame's previous
    /// window.
    #[test]
    fn unchanged_layout_is_still_written() {
        let rec = Recorder::start("sway");
        let d = open(&rec);
        let id = window(d);
        until(d, |a| a.kind == NHG_ACT_RESIZE && a.window == id);
        let mut l = NhgCsdLayout {
            l: 99,
            t: 99,
            r: 99,
            b: 99,
            strip: 99,
            on: 1,
        };
        // SAFETY: a live display; `l` is writable.
        let rc = unsafe { nhg_csd_layout(d, id, 120, 0, &mut l) };
        assert_eq!((rc, l), (0, NhgCsdLayout::default()));
        close(d);
    }

    /// A press in the native area keeps the pointer for the frame: over the border and past the surface the motion
    /// goes on (a drag out of the frame starts from it), and the border is back after the release.
    #[test]
    fn a_held_press_keeps_the_motion() {
        let rec = Recorder::start("mutter");
        let (d, _) = csd_window(&rec);
        point(&rec, d, 100, 80);
        button(&rec, d, true);
        for (x, y) in [(213, 80), (100, 20), (260, 80)] {
            let ev = point(&rec, d, x, y);
            assert!(any_pointer(&ev), "at {x}, {y}");
            assert!(last_title_bar(&ev).is_none(), "{ev:?}");
        }
        button(&rec, d, false);
        let ev = point(&rec, d, 100, 3);
        assert!(!any_pointer(&ev), "{ev:?}");
        close(d);
    }

    /// Margins, strip and buttons each hit as `nhwl_csd_hit` did: the border consumes the motion and gives its pointer
    /// shape; a button under the pointer is hovered; back in the native area, the border is left and the motion goes
    /// on as the pointer's own action, after the title bar's.
    #[test]
    fn hit_test_edges_strip_buttons() {
        let rec = Recorder::start("mutter");
        let (d, id) = csd_window(&rec);
        let shape = |ev: &[Vec<NhgAction>]| {
            let a = last_title_bar(ev).expect("no title-bar action");
            assert_ne!(a.flags & NHG_TB_POINTER, 0, "{a:?}");
            assert_eq!(a.window, id);
            a.code as u32
        };
        for (x, y, want) in [
            (2, 60, NHSHELL_CURSOR_RESIZE_EW),
            (213, 60, NHSHELL_CURSOR_RESIZE_EW),
            (2, 2, NHSHELL_CURSOR_RESIZE_NWSE),
            (213, 145, NHSHELL_CURSOR_RESIZE_NWSE),
            (100, 3, NHSHELL_CURSOR_RESIZE_NS),
            (100, 145, NHSHELL_CURSOR_RESIZE_NS),
            (50, 20, NHSHELL_CURSOR_DEFAULT),
        ] {
            let ev = point(&rec, d, x, y);
            assert_eq!(shape(&ev), want, "at {x}, {y}");
            assert!(
                !any_pointer(&ev),
                "the border passed a motion on at {x}, {y}"
            );
        }
        assert_eq!(buttons_state(d, id), (0, 0));
        // the close button: hovered, and the strip is painted again
        let ev = point(&rec, d, 180, 20);
        let a = last_title_bar(&ev).unwrap();
        assert_eq!(a.flags & NHG_TB_REDRAW, NHG_TB_REDRAW);
        assert_eq!(buttons_state(d, id), (NHG_CSD_CLOSE, 0));
        // into the native area: the border is left, the hover goes, and the motion goes on, last
        let ev = point(&rec, d, 100, 80);
        let last = ev.last().expect("no event");
        assert_eq!(last.last().map(|a| a.kind), Some(NHG_ACT_MOTION));
        let a = last_title_bar(&ev).unwrap();
        assert_eq!(a.flags, NHG_TB_LEFT | NHG_TB_REDRAW);
        assert_eq!(buttons_state(d, id), (0, 0));
        // inside the native area nothing of the title bar's
        let ev = point(&rec, d, 110, 90);
        assert!(last_title_bar(&ev).is_none(), "{ev:?}");
        assert!(any_pointer(&ev));
        close(d);
    }

    /// A tiled side has no margin, a maximized window only the strip, and a fullscreen one nothing to paint.
    #[test]
    fn tiled_side_has_no_margin() {
        let rec = Recorder::start("mutter");
        let d = open(&rec);
        let id = window(d);
        configured(d, id);
        let cases: [(&str, [i32; 5]); 4] = [
            (r#""activated","tiled_left""#, [0, 40, 8, 8, 32]),
            (
                r#""activated","tiled_left","tiled_right","tiled_top","tiled_bottom""#,
                [0, 32, 0, 0, 32],
            ),
            (r#""activated","maximized""#, [0, 32, 0, 0, 32]),
            (r#""activated","fullscreen""#, [0, 0, 0, 0, 0]),
        ];
        for (states, [l, t, r, b, strip]) in cases {
            configure(&rec, [200, 100], states);
            apply_configure(d, id);
            let (_, got) = lay(d, id, 120, 0);
            assert_eq!(
                got,
                NhgCsdLayout {
                    l,
                    t,
                    r,
                    b,
                    strip,
                    on: 1
                },
                "{states}"
            );
            settle(d);
        }
        // fullscreen: on, but no border to paint
        assert_eq!(paint(d, id, 200, 100, 1).0, 0);
        assert_eq!(title_bar(d, id, 200, 100).0, 0);
        close(d);
    }

    /// The layout sets the CSD fact the configure step reads: under our own title bar a change of activation is a
    /// content ack; where the compositor decorates (sway), a bare one.
    #[test]
    fn content_ack_on_state_change() {
        for (profile, flags) in [("mutter", NHSHELL_ACK_CONTENT), ("sway", 0)] {
            let rec = Recorder::start(profile);
            let d = open(&rec);
            let id = window(d);
            configured(d, id);
            let csd = facts_flags(d, id) & NHG_FACT_CSD != 0;
            assert_eq!(csd, profile == "mutter", "{profile}");
            // the recorder's first configure activates: inactive, then active again
            for states in ["", r#""activated""#] {
                configure(&rec, [200, 100], states);
                apply_configure(d, id);
                assert_eq!(
                    last_ack(d, id).map(|(_, f)| f),
                    Some(flags),
                    "{profile} {states}"
                );
                settle(d);
            }
            close(d);
        }
    }

    /// The publish step paints only when what the strip shows changed: the look C hashes (title, colours), the
    /// native size, the states, a button's state, the style.
    #[test]
    fn paint_only_when_state_or_title_changes() {
        let rec = Recorder::start("mutter");
        let (d, id) = csd_window(&rec);
        assert_eq!(paint(d, id, 200, 100, 1).0, 0, "nothing changed");
        assert_eq!(paint(d, id, 200, 100, 2).0, 1, "a new title");
        assert_eq!(paint(d, id, 200, 100, 2).0, 0);
        let (rc, p) = paint(d, id, 210, 100, 2);
        assert_eq!((rc, p.strip.w), (1, 210), "a new size");
        assert_eq!(paint(d, id, 210, 100, 2).0, 0);
        // the states (activated since the recorder's first configure): inactive, then active again
        for (states, active) in [("", 0), (r#""activated""#, 1)] {
            configure(&rec, [210, 132], states);
            apply_configure(d, id);
            settle(d);
            let (rc, p) = paint(d, id, 210, 100, 2);
            assert_eq!((rc, p.active), (1, active), "{states}");
            assert_eq!(paint(d, id, 210, 100, 2).0, 0);
        }
        // suspended and back: nothing the strip shows changed
        for states in [r#""suspended","activated""#, r#""activated""#] {
            configure(&rec, [210, 132], states);
            apply_configure(d, id);
            settle(d);
            assert_eq!(paint(d, id, 210, 100, 2).0, 0, "{states}");
        }
        // hovering the close button (226 wide: at 186)
        rec.control_ok(r#"{"cmd":"focus","surface":"toplevel"}"#);
        point(&rec, d, 190, 20);
        let (rc, p) = paint(d, id, 210, 100, 2);
        assert_eq!(rc, 1, "a hovered button");
        assert_eq!(
            p.button[..3].iter().map(|b| b.state).collect::<Vec<_>>(),
            [0, 0, NHG_BTN_HOVER]
        );
        assert_eq!(paint(d, id, 210, 100, 2).0, 0);
        // the style
        style(d, &[], &[NHG_CSD_CLOSE]);
        let (rc, p) = paint(d, id, 210, 100, 2);
        assert_eq!((rc, cells(&p)), (1, vec![(NHG_CSD_CLOSE, 186)]));
        assert_eq!(paint(d, id, 210, 100, 2).0, 0);
        close(d);
    }

    /// Two left presses on the strip within 400 ms: a move, then the window is maximized.  The presses and releases
    /// go no further than the title bar.
    #[test]
    fn double_click_maximizes() {
        let rec = Recorder::start("mutter");
        let (d, _) = csd_window(&rec);
        point(&rec, d, 50, 20);
        let mut ev = Vec::new();
        rec.control_ok(r#"{"cmd":"button","button":272,"state":"press"}"#);
        rec.control_ok(r#"{"cmd":"button","button":272,"state":"release"}"#);
        rec.control_ok(r#"{"cmd":"button","button":272,"state":"press"}"#);
        rec.control_ok(r#"{"cmd":"button","button":272,"state":"release"}"#);
        ev.extend(settle(d));
        assert!(!any_pointer(&ev), "a strip click went on: {ev:?}");
        assert!(logged(&rec, "xdg_toplevel.move"));
        assert!(logged(&rec, "xdg_toplevel.set_maximized"));
        close(d);
    }

    /// A click on close gives `DELETE`; a press on it released elsewhere fires nothing; maximize asks the compositor.
    #[test]
    fn close_click_deletes_and_buttons_fire_on_release() {
        let rec = Recorder::start("mutter");
        let (d, id) = csd_window(&rec);
        point(&rec, d, 180, 20);
        let ev = button(&rec, d, true);
        assert_eq!(buttons_state(d, id), (NHG_CSD_CLOSE, NHG_CSD_CLOSE));
        assert_eq!(last_title_bar(&ev).map(|a| a.flags), Some(NHG_TB_REDRAW));
        assert!(!any_pointer(&ev));
        // released over the strip: disarmed, nothing fires, the release is consumed
        point(&rec, d, 50, 20);
        let ev = button(&rec, d, false);
        assert!(ev.iter().flatten().all(|a| a.kind != NHG_ACT_DELETE));
        assert!(!any_pointer(&ev));
        assert_eq!(buttons_state(d, id).1, 0);
        // pressed and released over close: DELETE
        point(&rec, d, 180, 20);
        button(&rec, d, true);
        let ev = button(&rec, d, false);
        let kinds: Vec<_> = ev.iter().flatten().map(|a| a.kind).collect();
        assert_eq!(kinds, [NHG_ACT_TITLE_BAR, NHG_ACT_DELETE]);
        assert!(ev.iter().flatten().all(|a| a.window == id));
        // maximize
        point(&rec, d, 150, 20);
        button(&rec, d, true);
        button(&rec, d, false);
        assert!(logged(&rec, "xdg_toplevel.set_maximized"));
        // a press in the native area is not the title bar's
        point(&rec, d, 100, 80);
        let ev = button(&rec, d, true);
        assert!(any_pointer(&ev));
        assert!(last_title_bar(&ev).is_none());
        close(d);
    }

    /// The layout of an undecorated window: margins `[l, t, r, b]`, no strip.
    fn bare(m: [i32; 4]) -> NhgCsdLayout {
        NhgCsdLayout {
            l: m[0],
            t: m[1],
            r: m[2],
            b: m[3],
            strip: 0,
            on: 0,
        }
    }

    /// [C] Where the compositor offers no server-side decorations (mutter), an undecorated window keeps its resize
    /// margins without the strip: nothing to paint, the window geometry excludes the margins, which are transparent
    /// (the layer is the native area), and a press in the right margin starts a resize on the right edge.
    #[test]
    fn undecorated_keeps_resize_margins_without_a_strip() {
        let rec = Recorder::start("mutter");
        let d = open(&rec);
        let id = undecorated(d);
        assert_eq!(lay(d, id, 120, NHG_CSD_UNDECORATED).1, bare([8; 4]));
        assert_eq!(facts_flags(d, id) & NHG_FACT_CSD, 0);
        assert_eq!(paint(d, id, 200, 100, 1).0, 0, "no strip to paint");
        assert_eq!(title_bar(d, id, 200, 100).0, 0);
        let mut key = 0;
        show(d, id, &mut key);
        rec.wait_node(id, |n| {
            n.contains("\"geometry\":[8,8,200,100]") && n.contains("\"input_region\":null")
        });
        assert_eq!(shown(&rec, 3, 50), 0, "the left margin");
        assert_eq!(shown(&rec, 100, 50), BG);
        rec.control_ok(r#"{"cmd":"focus","surface":"toplevel"}"#);
        settle(d);
        let ev = point(&rec, d, 212, 50);
        let a = last_title_bar(&ev).expect("no title-bar action");
        assert_ne!(a.flags & NHG_TB_POINTER, 0, "{a:?}");
        assert_eq!(a.code as u32, NHSHELL_CURSOR_RESIZE_EW);
        assert!(!any_pointer(&ev), "the margin passed a motion on");
        button(&rec, d, true);
        assert!(
            logged(&rec, r#""edges":"right""#),
            "no resize on the right edge"
        );
        button(&rec, d, false);
        close(d);
    }

    /// [C] Maximized, an undecorated window has no margin and its chrome goes: the geometry is the whole surface and
    /// the corner is the window's own pixel.
    #[test]
    fn undecorated_maximized_has_no_margin() {
        let rec = Recorder::start("mutter");
        let d = open(&rec);
        let id = undecorated(d);
        assert_eq!(chrome_of(d, id, &LOOK), 1);
        configure(&rec, [200, 100], r#""activated","maximized""#);
        apply_undecorated(d, id);
        assert_eq!(lay(d, id, 120, NHG_CSD_UNDECORATED).1, bare([0; 4]));
        let mut key = 0;
        show(d, id, &mut key);
        rec.wait_node(id, |n| n.contains("\"geometry\":[0,0,200,100]"));
        assert_eq!(shown(&rec, 0, 0), BG, "a square corner");
        close(d);
    }

    /// [C] Fullscreen, or tiled on all four sides: no margin.  Tiled on one side: none there, and only the resize
    /// margins elsewhere, with no chrome (no room for the shadow, a square corner, nothing drawn below).
    #[test]
    fn undecorated_fullscreen_and_tiled_have_none() {
        let rec = Recorder::start("mutter");
        let d = open(&rec);
        let id = undecorated(d);
        assert_eq!(chrome_of(d, id, &LOOK), 1);
        assert_eq!(
            lay(d, id, 120, NHG_CSD_UNDECORATED).1,
            bare([25, 19, 25, 31])
        );
        let cases: [(&str, [i32; 4]); 3] = [
            (r#""activated","fullscreen""#, [0; 4]),
            (
                r#""activated","tiled_left","tiled_right","tiled_top","tiled_bottom""#,
                [0; 4],
            ),
            (r#""activated","tiled_left""#, [0, 8, 8, 8]),
        ];
        for (states, want) in cases {
            configure(&rec, [200, 100], states);
            apply_undecorated(d, id);
            assert_eq!(
                lay(d, id, 120, NHG_CSD_UNDECORATED).1,
                bare(want),
                "{states}"
            );
            settle(d);
        }
        let mut key = 0;
        show(d, id, &mut key);
        rec.wait_node(id, |n| n.contains("\"geometry\":[0,8,200,100]"));
        assert_eq!(shown(&rec, 0, 8), BG, "a square corner by the tiled side");
        assert_eq!(shown(&rec, 100, 8 + 100 + 4), 0, "no shadow below");
        close(d);
    }

    /// [C] Where the compositor offers server-side decorations but grants a window's ask for none (KWin: the sway
    /// profile answering client-side), an undecorated window keeps its invisible resize margins and its chrome, as
    /// where none are offered: the compositor draws nothing around it, so without them the window could not be resized
    /// from its outer edge.
    #[test]
    fn undecorated_with_ssd_offered_keeps_its_margins() {
        let rec = Recorder::start("sway");
        rec.control_ok(r#"{"cmd":"decoration","mode":"client"}"#);
        let d = open(&rec);
        let id = undecorated(d);
        assert_eq!(chrome_of(d, id, &LOOK), 1);
        assert_eq!(
            lay(d, id, 120, NHG_CSD_UNDECORATED),
            (1, bare([25, 19, 25, 31]))
        );
        let mut key = 0;
        show(d, id, &mut key);
        rec.wait_node(id, |n| {
            n.contains("\"geometry\":[25,19,200,100]")
                && n.contains("\"input_region\":[[17,11,216,116]]")
        });
        close(d);
    }

    /// [C] With the look's chrome the margins make room for the shadow: the geometry excludes them, the input region
    /// is the window and the resize margins around it, the shadow lies below the window.  Corners alone keep the
    /// resize margins, and outside the arc the corner is transparent.
    #[test]
    fn undecorated_chrome_rounds_and_shadows() {
        let rec = Recorder::start("mutter");
        let d = open(&rec);
        let id = undecorated(d);
        assert_eq!(chrome_of(d, id, &LOOK), 1);
        assert_eq!(
            lay(d, id, 120, NHG_CSD_UNDECORATED),
            (1, bare([25, 19, 25, 31]))
        );
        let mut key = 0;
        show(d, id, &mut key);
        rec.wait_node(id, |n| {
            n.contains("\"geometry\":[25,19,200,100]")
                && n.contains("\"input_region\":[[17,11,216,116]]")
        });
        let below = shown(&rec, 125, 19 + 100 + 4);
        assert!(
            below >> 24 > 0 && below & 0x00ff_ffff == 0,
            "the shadow below: {below:#x}"
        );
        assert_eq!(shown(&rec, 125, 19 + 50), BG);

        assert_eq!(chrome_of(d, id, &CORNERS), 1);
        assert_eq!(lay(d, id, 120, NHG_CSD_UNDECORATED).1, bare([8; 4]));
        show(d, id, &mut key);
        rec.wait_node(id, |n| {
            n.contains("\"geometry\":[8,8,200,100]") && n.contains("\"input_region\":null")
        });
        assert_eq!(shown(&rec, 8, 8), 0, "outside the arc");
        assert_eq!(shown(&rec, 8 + 12, 8), BG, "past the corner square");
        assert_eq!(shown(&rec, 3, 50), 0, "a margin");
        close(d);
    }
}

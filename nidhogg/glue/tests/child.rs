//! T1 tests of child frames through the glue (`child.rs`) against nh-recorder (manual clock).  Each test stands in for
//! C: it makes the windows as `nhwl_create_frame_window` does (a child at the position `child::new_offset` gives),
//! pushes the child's facts and chrome where C does, lays the border out (`nhg_csd_layout`), runs the publish step
//! (`nhg_csd_paint`) and the layer step (`nhg_layer_ensure`) for each drawable frame, and presents the tree.  The
//! recorder's `tree` reply shows what the compositor applied; a child's state applies with its root's commit.
//!
//! At scale 1 a chrome with blur 4, spread 1, offset (0, 2) and some opacity needs a margin of 8 on the sides, 6 above
//! and 10 below: ceil(1.5 x 4) + 1 + 1 = 8, moved by the offset.

#[path = "../../shell/tests/common/mod.rs"]
mod common;

use common::{Recorder, number, readable_within};
use nhglue::abi::*;
use nhglue::backing::{NhgBacking, NhgFrameGeom, NhgRect};
use nhglue::child::{NHG_CHILD_VISIBLE, NhgChrome};
use nhglue::csd::{NhgCsdLayout, NhgCsdPaint};
use nhglue::ffi::*;
use nhglue::frame::NHG_FRAME_CHILD;
use nhglue::present::*;
use nhshell::api::*;
use nhshell::event::*;
use std::ptr;
use std::time::{Duration, Instant};

const BG: u32 = 0xff20_4060;
const SHADOW: NhgChrome = NhgChrome {
    on: 1,
    radius: 6.0,
    blur: 4.0,
    ox: 0.0,
    oy: 2.0,
    spread: 1.0,
    opacity: 0.5,
    rgb: 0x10_2030,
};

fn open(rec: &Recorder) -> *mut NhgDisplay {
    let name = rec.display();
    // SAFETY: the name is NUL-terminated; no error buffer.
    let d = unsafe { nhg_open(name.as_ptr(), ptr::null_mut(), 0) };
    assert!(!d.is_null(), "nhg_open failed on the recorder");
    d
}

fn close(d: *mut NhgDisplay) {
    // SAFETY: a live display, not used again.
    assert_eq!(unsafe { nhg_close(d) }, NHG_OK);
}

/// The shell inside `d`, for what the glue does not wrap yet (making windows, records).
fn shell<'a>(d: *mut NhgDisplay) -> &'a mut dyn ShellBackend {
    // SAFETY: a live display's backend, used only between glue calls.
    unsafe { &mut *(*d).backend_ptr() }
}

/// The next record satisfying `want` within 5 s; other records are dropped.
fn wait_for(d: *mut NhgDisplay, want: impl Fn(&Record) -> bool) -> Record {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        while let Some(ev) = shell(d).next_event().unwrap() {
            if want(&ev) {
                return ev;
            }
        }
        assert!(Instant::now() < deadline, "the awaited record never came");
        readable_within(shell(d).wake_fd(), 50);
    }
}

/// The records that arrive within `ms` of quiet.
fn collect(d: *mut NhgDisplay, ms: i64) -> Vec<Record> {
    let mut out = Vec::new();
    loop {
        while let Some(ev) = shell(d).next_event().unwrap() {
            out.push(ev);
        }
        if !readable_within(shell(d).wake_fd(), ms) {
            return out;
        }
    }
}

/// A top-level of `w` x `h` whose first configure is acked (a content ack, sent with the first picture).
fn toplevel(d: *mut NhgDisplay, w: i32, h: i32) -> u64 {
    let id = shell(d)
        .window_new(&WindowDesc {
            w,
            h,
            title: "t".into(),
            app_id: "nidhogg.test".into(),
            flags: 0,
        })
        .unwrap();
    // SAFETY: a live display.
    assert_eq!(unsafe { nhg_frame_register(d, id, 0) }, NHG_OK);
    let ev = wait_for(d, |e| e.kind() == Kind::Configure && e.window == id);
    let ShellEvent::Configure(c) = ev.event else {
        unreachable!()
    };
    let serial = c.serial;
    shell(d)
        .ack_configure(id, serial, NHSHELL_ACK_CONTENT)
        .unwrap();
    id
}

/// A child frame of `parent`'s window with its native area at (`x`, `y`), `w` x `h` (scale 1, no margin yet), made
/// and pushed as `nhwl_create_frame_window` and `nhwl_child_created` do.
fn child(d: *mut NhgDisplay, parent: u64, x: i32, y: i32, w: i32, h: i32) -> u64 {
    let id = shell(d)
        .child_new(&ChildDesc {
            kind: NHSHELL_KIND_CHILD,
            parent,
            x,
            y,
            w,
            h,
            anchor: Anchor::default(),
            flags: 0,
        })
        .unwrap();
    // SAFETY: a live display.
    unsafe {
        assert_eq!(nhg_frame_register(d, id, NHG_FRAME_CHILD), NHG_OK);
        assert_eq!(nhg_frame_scale(d, id, 120), NHG_OK);
        assert_eq!(nhg_child_chrome(d, id, &NhgChrome::default()), 0);
    }
    assert_eq!(lay(d, id).0, 0, "no chrome, no margin");
    assert_eq!(set(d, id, parent, x, y, NHG_CHILD_VISIBLE), 0);
    id
}

fn set(d: *mut NhgDisplay, id: u64, parent: u64, x: i32, y: i32, flags: u32) -> i32 {
    // SAFETY: a live display.
    unsafe { nhg_child_set(d, id, parent, x, y, flags) }
}

fn chrome(d: *mut NhgDisplay, id: u64, c: &NhgChrome) -> i32 {
    // SAFETY: a live display; `c` is readable.
    unsafe { nhg_child_chrome(d, id, c) }
}

/// `nhg_csd_layout` at scale 1, as C calls it: its return and the layout.
fn lay(d: *mut NhgDisplay, id: u64) -> (i32, NhgCsdLayout) {
    let mut l = NhgCsdLayout::default();
    // SAFETY: a live display; `l` is writable.
    let rc = unsafe { nhg_csd_layout(d, id, 120, 0, &mut l) };
    (rc, l)
}

fn offsets(d: *mut NhgDisplay, id: u64) -> [i32; 4] {
    let mut o = [0; 4];
    // SAFETY: a live display; 4 writable values.
    assert_eq!(unsafe { nhg_frame_offsets(d, id, o.as_mut_ptr()) }, NHG_OK);
    o
}

fn raise(d: *mut NhgDisplay, id: u64, raise: i32, z_group: i32) -> i32 {
    // SAFETY: a live display.
    unsafe { nhg_child_raise(d, id, raise, z_group) }
}

/// A frame's backing in the glue, as C keeps it: its window, its key, its native size and its border.
struct Surf {
    window: u64,
    key: u64,
    w: i32,
    h: i32,
    border: [i32; 4],
    back: NhgBacking,
}

/// The geometry C hands the glue (`nhwl_frame_size`, `nhwl_frame_geom`): the layer is all of the backing, and the
/// glue moves a child's.
fn geom(s: &Surf) -> NhgFrameGeom {
    let [l, t, r, b] = s.border;
    NhgFrameGeom {
        native_w: s.w,
        native_h: s.h,
        l,
        t,
        r,
        b,
        bg_argb: BG,
        scale120: 120,
        layer: NhgRect {
            x: 0,
            y: 0,
            w: s.w + l + r,
            h: s.h + t + b,
        },
        window: s.window,
        ..Default::default()
    }
}

/// `nhg_layer_ensure`'s return, then the backing at the frame's size and border (`nhwl_backing_resize`).
fn ensure(d: *mut NhgDisplay, s: &mut Surf) -> i32 {
    let g = geom(s);
    // SAFETY: a live display; `g`, the key and the backing are valid for the calls.
    unsafe {
        let rc = nhg_layer_ensure(d, &mut s.key, &g);
        assert!(rc >= 0, "nhg_layer_ensure: {rc}");
        assert_eq!(nhg_backing(d, s.key, &g, &mut s.back), 0);
        rc
    }
}

fn surface(d: *mut NhgDisplay, window: u64, w: i32, h: i32) -> Surf {
    let mut s = Surf {
        window,
        key: 0,
        w,
        h,
        border: offsets(d, window),
        back: NhgBacking {
            pixels: ptr::null_mut(),
            stride: 0,
            w: 0,
            h: 0,
            generation: 0,
        },
    };
    ensure(d, &mut s);
    s
}

/// Paint all of `s` with `argb` and damage it, as the rasteriser and `nhwl_damage_all` do.
fn fill(d: *mut NhgDisplay, s: &Surf, argb: u32) {
    let n = (s.back.w * s.back.h) as usize;
    for i in 0..n {
        // SAFETY: `i` lies inside the borrowed backing.
        unsafe { *s.back.pixels.cast::<u32>().add(i) = argb };
    }
    // SAFETY: a live display.
    assert_eq!(unsafe { nhg_damage_all(d, s.key) }, NHG_OK);
}

/// The publish step of a drawable frame (`nhwl_surface_add`): the strip's and the child's step, then the layer's.
fn prepare(d: *mut NhgDisplay, s: &mut Surf) {
    let mut p = NhgCsdPaint::default();
    // SAFETY: a live display; `p` is writable.
    let rc = unsafe { nhg_csd_paint(d, s.window, s.w, s.h, 0, &mut p) };
    assert!(rc >= 0, "nhg_csd_paint: {rc}");
    s.border = offsets(d, s.window);
    ensure(d, s);
}

fn entry(s: &Surf, parent: u64, flags: u32) -> NhgSurface {
    NhgSurface {
        window: s.window,
        parent,
        frame: s.key,
        flags,
        alpha_cut: 0,
    }
}

fn present(d: *mut NhgDisplay, root: u64, list: &[NhgSurface], flags: u32) -> i32 {
    // SAFETY: a live display; `list` is readable for the call.
    unsafe { nhg_present(d, root, list.as_ptr(), list.len(), flags, 0) }
}

/// The root's `FRAME_READY`, then the inline present it asks for: the tree's owed commits go out.
fn ready(d: *mut NhgDisplay, root: u64) -> i32 {
    wait_for(d, |e| e.kind() == Kind::FrameReady && e.window == root);
    present(d, root, &[], NHG_PRESENT_READY)
}

/// Advance the recorder's clock one frame: frame callbacks and releases go out.
fn tick(rec: &Recorder, d: *mut NhgDisplay) {
    let r = rec.control(r#"{"cmd":"present","advance_ns":16000000}"#);
    assert!(r.contains("\"ok\":true"), "{r}");
    collect(d, 100);
}

fn next_seq(rec: &Recorder) -> u64 {
    number(&rec.control(r#"{"cmd":"log","since":1}"#), "next").expect("next")
}

/// Every commit from recorder commit `seq` on, one JSON text each, in order.
fn log(rec: &Recorder, seq: u64) -> Vec<String> {
    let log = rec.control(&format!(r#"{{"cmd":"log","since":{seq}}}"#));
    log.split("{\"ack_serial\":")
        .skip(1)
        .map(|c| format!("{{\"ack_serial\":{c}"))
        .collect()
}

fn of(c: &str, id: u64) -> bool {
    c.contains(&format!("\"surface\":{id},"))
}

/// How many commits of surface `id` damaged it (a new frame; a bare commit has no damage).
fn frames_of(rec: &Recorder, id: u64) -> usize {
    log(rec, 1)
        .iter()
        .filter(|c| of(c, id) && !c.contains("\"damage\":[]"))
        .count()
}

/// Surface `id`'s tree entry 100 ms on, which must still satisfy `pred`: nothing applied a change meanwhile.
fn still(rec: &Recorder, id: u64, pred: impl Fn(&str) -> bool) {
    std::thread::sleep(Duration::from_millis(100));
    let n = rec.wait_node(id, |_| true);
    assert!(pred(&n), "applied before its root's commit: {n}");
}

/// A shown tree: `frames` (the root first, each parent before its children, as `(surface, parent)`), each drawn whole,
/// prepared and published, the first picture committed and its callbacks answered.
fn show(rec: &Recorder, d: *mut NhgDisplay, frames: &mut [(&mut Surf, u64)]) {
    for (s, _) in frames.iter_mut() {
        prepare(d, s);
        fill(d, s, BG);
    }
    let list: Vec<NhgSurface> = frames
        .iter()
        .map(|(s, p)| entry(s, *p, NHG_SURF_DRAWABLE))
        .collect();
    let root = list[0].window;
    assert_ne!(
        present(d, root, &list, NHG_PRESENT_BACKSTOP) & NHG_PRESENTED_PUBLISHED,
        0
    );
    assert_ne!(ready(d, root) & NHG_PRESENTED_COMMITTED, 0);
    tick(rec, d);
}

/// The tests are named `child::…`, like the other crates' T1 tests.
#[allow(clippy::module_inception)]
mod child {
    use super::*;

    /// A move is recorded at once and reaches the compositor with the root's commit: the child's surface stays where
    /// it was until the root commits, and the move is not a new frame of the child.
    #[test]
    fn position_applies_with_the_root_commit() {
        let rec = Recorder::start_clock("sway", "manual");
        let d = open(&rec);
        let root = toplevel(d, 200, 100);
        let mut r = surface(d, root, 200, 100);
        let c = child(d, root, 30, 20, 60, 40);
        let mut k = surface(d, c, 60, 40);
        show(&rec, d, &mut [(&mut r, 0), (&mut k, root)]);
        rec.wait_node(c, |n| n.contains("\"position\":[30,20]"));
        let frames = frames_of(&rec, c);

        assert_eq!(set(d, c, root, 50, 25, NHG_CHILD_VISIBLE), 0);
        still(&rec, c, |n| n.contains("\"position\":[30,20]"));
        // a bare commit: COMMITTED reports a root buffer only
        assert_eq!(ready(d, root) & NHG_PRESENTED_COMMITTED, 0);
        rec.wait_node(c, |n| n.contains("\"position\":[50,25]"));
        assert_eq!(
            frames_of(&rec, c),
            frames,
            "a move is not a new frame of the child"
        );
        // the same position again sends nothing
        let seq = next_seq(&rec);
        assert_eq!(set(d, c, root, 50, 25, NHG_CHILD_VISIBLE), 0);
        assert!(
            collect(d, 200)
                .iter()
                .all(|e| !(e.kind() == Kind::FrameReady && e.window == root))
        );
        assert!(log(&rec, seq).is_empty());
        close(d);
    }

    /// The chrome gives the child its shadow margin: the frame's offsets grow by it, the surface moves up and left by
    /// it so that the native area stays where the frame is, and the layer takes the chrome at the next layer step.
    /// The same chrome again changes nothing.
    #[test]
    fn chrome_margin_moves_the_layer() {
        let rec = Recorder::start_clock("sway", "manual");
        let d = open(&rec);
        let root = toplevel(d, 200, 100);
        let mut r = surface(d, root, 200, 100);
        let c = child(d, root, 30, 20, 60, 40);
        let mut k = surface(d, c, 60, 40);
        show(&rec, d, &mut [(&mut r, 0), (&mut k, root)]);

        assert_eq!(chrome(d, c, &SHADOW), 1);
        assert_eq!(chrome(d, c, &SHADOW), 0, "the same chrome is no change");
        let (rc, l) = lay(d, c);
        assert_eq!(rc, 1);
        assert_eq!((l.l, l.t, l.r, l.b, l.strip, l.on), (8, 6, 8, 10, 0, 0));
        assert_eq!(offsets(d, c), [8, 6, 8, 10]);
        assert_eq!(lay(d, c).0, 0, "laid out already");

        // the layer step: the layer moves inside the margin and takes the chrome, once
        k.border = offsets(d, c);
        assert_eq!(ensure(d, &mut k), 1);
        assert_eq!((k.back.w, k.back.h), (76, 56));
        assert_eq!(ensure(d, &mut k), 0, "the layer holds it");

        // the publish step: the surface starts the margin before the native area
        fill(d, &k, BG);
        prepare(d, &mut k);
        let list = [
            entry(&r, 0, NHG_SURF_DRAWABLE),
            entry(&k, root, NHG_SURF_DRAWABLE),
        ];
        assert_ne!(
            present(d, root, &list, NHG_PRESENT_BACKSTOP) & NHG_PRESENTED_PUBLISHED,
            0
        );
        ready(d, root);
        rec.wait_node(c, |n| n.contains("\"position\":[22,14]"));

        // no chrome: no margin, and the surface goes back to the frame's place
        assert_eq!(chrome(d, c, &NhgChrome::default()), 1);
        assert_eq!(lay(d, c).0, 1);
        assert_eq!(offsets(d, c), [0; 4]);
        close(d);
    }

    /// The input region is the native area inside the shadow margin, in logical pixels, sent at the publish step and
    /// applied with the root's commit; without a margin it is the whole surface again.
    #[test]
    fn input_region_excludes_shadow() {
        let rec = Recorder::start_clock("sway", "manual");
        let d = open(&rec);
        let root = toplevel(d, 200, 100);
        let mut r = surface(d, root, 200, 100);
        let c = child(d, root, 30, 20, 60, 40);
        assert_eq!(chrome(d, c, &SHADOW), 1);
        assert_eq!(lay(d, c).0, 1);
        let mut k = surface(d, c, 60, 40);
        show(&rec, d, &mut [(&mut r, 0), (&mut k, root)]);
        rec.wait_node(c, |n| n.contains("\"input_region\":[[8,6,60,40]]"));

        assert_eq!(chrome(d, c, &NhgChrome::default()), 1);
        assert_eq!(lay(d, c).0, 1);
        prepare(d, &mut k);
        fill(d, &k, BG);
        let list = [
            entry(&r, 0, NHG_SURF_DRAWABLE),
            entry(&k, root, NHG_SURF_DRAWABLE),
        ];
        assert_ne!(
            present(d, root, &list, NHG_PRESENT_BACKSTOP) & NHG_PRESENTED_PUBLISHED,
            0
        );
        ready(d, root);
        rec.wait_node(c, |n| n.contains("\"input_region\":null"));
        close(d);
    }

    /// A child made goes above its siblings; a raise puts it above the highest, a lower below the lowest, whatever its
    /// z-group (the glue keeps the order); it applies with the root's commit.  A window that is not a child is
    /// refused; a child alone changes nothing.
    #[test]
    fn raise_and_lower_follow_the_glues_order() {
        let rec = Recorder::start_clock("sway", "manual");
        let d = open(&rec);
        let root = toplevel(d, 200, 100);
        let mut r = surface(d, root, 200, 100);
        let a = child(d, root, 0, 0, 40, 30);
        let b = child(d, root, 10, 10, 40, 30);
        show(&rec, d, &mut [(&mut r, 0)]);
        let order = |first: u64, second: u64| format!("\"children\":[{first},{second}]");
        rec.wait_node(root, |n| n.contains(&order(a, b)));

        // a raised (z-group above)
        assert_eq!(raise(d, a, 1, 1), NHG_OK);
        still(&rec, root, |n| n.contains(&order(a, b)));
        ready(d, root);
        rec.wait_node(root, |n| n.contains(&order(b, a)));
        tick(&rec, d);

        // a lowered (z-group below)
        assert_eq!(raise(d, a, 0, 2), NHG_OK);
        ready(d, root);
        rec.wait_node(root, |n| n.contains(&order(a, b)));
        tick(&rec, d);

        // b raised with no z-group: above a
        assert_eq!(raise(d, b, 1, 0), NHG_OK);
        rec.wait_node(root, |n| n.contains(&order(a, b)));

        assert_eq!(raise(d, root, 1, 0), NHG_E_STATE, "not a child");
        assert_eq!(raise(d, 0, 1, 0), NHG_E_STATE, "no window");
        let lone = toplevel(d, 100, 100);
        let c = child(d, lone, 0, 0, 10, 10);
        assert_eq!(raise(d, c, 1, 0), NHG_OK, "alone");
        assert_eq!(raise(d, c, 0, 0), NHG_OK, "alone");
        close(d);
    }

    /// Hiding a child unmaps its surface: the root owes the commit that applies it (its `FRAME_READY` comes with
    /// nothing drawn), and the child shows no buffer.  Showing it again answers 1 (C draws it whole), and its next
    /// buffer maps it.
    #[test]
    fn hidden_child_owes_the_root() {
        let rec = Recorder::start_clock("sway", "manual");
        let d = open(&rec);
        let root = toplevel(d, 200, 100);
        let mut r = surface(d, root, 200, 100);
        let c = child(d, root, 30, 20, 60, 40);
        let mut k = surface(d, c, 60, 40);
        show(&rec, d, &mut [(&mut r, 0), (&mut k, root)]);
        rec.wait_node(c, |n| n.contains("\"mapped\":true"));

        assert_eq!(set(d, c, root, 30, 20, 0), 0);
        assert_eq!(set(d, c, root, 30, 20, 0), 0, "hidden already");
        still(&rec, c, |n| n.contains("\"mapped\":true"));
        // a bare commit: COMMITTED reports a root buffer only
        assert_eq!(ready(d, root) & NHG_PRESENTED_COMMITTED, 0);
        let n = rec.wait_node(c, |n| n.contains("\"mapped\":false"));
        assert!(n.contains("\"buffer\":null"), "{n}");
        tick(&rec, d);

        assert_eq!(set(d, c, root, 30, 20, NHG_CHILD_VISIBLE), 1);
        assert_eq!(
            set(d, c, root, 30, 20, NHG_CHILD_VISIBLE),
            0,
            "shown already"
        );
        fill(d, &k, BG);
        prepare(d, &mut k);
        let list = [
            entry(&r, 0, NHG_SURF_DRAWABLE),
            entry(&k, root, NHG_SURF_DRAWABLE),
        ];
        assert_ne!(
            present(d, root, &list, NHG_PRESENT_BACKSTOP) & NHG_PRESENTED_PUBLISHED,
            0
        );
        ready(d, root);
        rec.wait_node(c, |n| n.contains("\"mapped\":true"));
        close(d);
    }

    /// A grandchild's move is its parent's state, and the parent's is the root's: the child and the root commit
    /// (deepest first) and the move applies, with no new frame of either child.
    #[test]
    fn grandchild_move_owes_parent_and_root() {
        let rec = Recorder::start_clock("sway", "manual");
        let d = open(&rec);
        let root = toplevel(d, 200, 100);
        let mut r = surface(d, root, 200, 100);
        let c = child(d, root, 30, 20, 100, 60);
        let mut k = surface(d, c, 100, 60);
        let g = child(d, c, 10, 10, 40, 30);
        let mut gk = surface(d, g, 40, 30);
        show(&rec, d, &mut [(&mut r, 0), (&mut k, root), (&mut gk, c)]);
        rec.wait_node(g, |n| n.contains("\"position\":[10,10]"));
        let frames = (frames_of(&rec, c), frames_of(&rec, g));

        let seq = next_seq(&rec);
        assert_eq!(set(d, g, c, 25, 15, NHG_CHILD_VISIBLE), 0);
        still(&rec, g, |n| n.contains("\"position\":[10,10]"));
        // a bare commit: COMMITTED reports a root buffer only
        assert_eq!(ready(d, root) & NHG_PRESENTED_COMMITTED, 0);
        rec.wait_node(g, |n| n.contains("\"position\":[25,15]"));
        let commits = log(&rec, seq);
        let first = |id: u64| {
            commits
                .iter()
                .position(|x| of(x, id))
                .unwrap_or_else(|| panic!("no commit of {id}: {commits:?}"))
        };
        assert!(first(c) < first(root), "{commits:?}");
        assert_eq!(
            (frames_of(&rec, c), frames_of(&rec, g)),
            frames,
            "a move is not a new frame"
        );
        close(d);
    }
}

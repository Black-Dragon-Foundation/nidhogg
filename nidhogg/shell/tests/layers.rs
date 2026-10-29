//! T1 tests of content layers against nh-recorder: a window that is its own Glitnir output scrolls by moving its
//! layer's viewport source over a ring buffer, damages only the rows a scroll exposed, rebases once at the ring's end,
//! gives back the pages far from its view, sits above the root and below its child windows, takes no input, unmaps
//! with the root commit that draws it in the frame again, and never writes a buffer the compositor holds.  The host
//! side (a backing whose window rows are moved, then the exposed ones painted) is `common/backing.rs` plus the
//! helpers below.

mod common;

use common::backing::*;
use common::*;
use glitnir::geom::Rect;
use glitnir::scene::LayerDesc;
use nhshell::api::*;
use nhshell::event::*;
use nhshell::wayland::WaylandShell;
use std::time::{Duration, Instant};

const BG: u32 = 0xff20_4060;
/// The window layer's Glitnir id (a window id carries the top bit, as the glue's do).
const WIN: u64 = (1 << 63) | 7;
/// The window's rectangle on a 400 x 300 frame: the text area, with 60 rows of frame (a mode line, an echo area)
/// below it.
const VIEW: Rect = Rect::new(0, 0, 400, 240);

/// Dispatch the shell's records until `f` holds (10 s at most).
fn until(shell: &mut WaylandShell, what: &str, f: impl Fn(&mut WaylandShell) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        while shell.next_event().unwrap().is_some() {}
        if f(shell) {
            return;
        }
        assert!(Instant::now() < deadline, "never: {what}");
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn next_seq(rec: &Recorder) -> u64 {
    number(&rec.control(r#"{"cmd":"log","since":1}"#), "next").expect("next")
}

/// Surface `id`'s commits from recorder commit `seq` on, one JSON text each.
fn commits(rec: &Recorder, seq: u64, id: u64) -> Vec<String> {
    let log = rec.control(&format!(r#"{{"cmd":"log","since":{seq}}}"#));
    // the objects have sorted keys, so `ack_serial` opens each commit
    log.split("{\"ack_serial\":")
        .skip(1)
        .map(|c| format!("{{\"ack_serial\":{c}"))
        .filter(|c| c.contains(&format!("\"surface\":{id},")))
        .collect()
}

fn buffer_commits(rec: &Recorder, seq: u64, id: u64) -> Vec<String> {
    commits(rec, seq, id)
        .into_iter()
        .filter(|c| c.contains("\"buffer\":{"))
        .collect()
}

/// The numbers of the first `[...]` after `key` in `c`.
fn numbers_after(c: &str, key: &str) -> Option<Vec<f64>> {
    let at = c.find(key)? + key.len();
    let rest = &c[at..];
    let open = rest.find('[')?;
    let close = rest[open..].find(']')? + open;
    rest[open + 1..close]
        .split(',')
        .map(|n| n.trim().parse().ok())
        .collect()
}

/// A commit's viewport source `[x, y, w, h]`.
fn source(c: &str) -> [f64; 4] {
    let v = numbers_after(c, "\"src\":").expect("a viewport source");
    [v[0], v[1], v[2], v[3]]
}

/// A commit's damage rectangles.
fn damage(c: &str) -> Vec<Rect> {
    let Some(at) = c.find("\"damage\":[") else {
        return Vec::new();
    };
    // sorted keys: `decoration` follows `damage`
    let end = c[at..].find(",\"decoration\"").map_or(c.len(), |e| at + e);
    c[at..end]
        .split("\"rect\":")
        .skip(1)
        .filter_map(|r| numbers_after(r, ""))
        .map(|v| Rect::new(v[0] as i32, v[1] as i32, v[2] as i32, v[3] as i32))
        .collect()
}

fn area(rs: &[Rect]) -> u64 {
    rs.iter().map(Rect::area).sum()
}

/// The output shows `b` at the top-left.
fn shows(rec: &Recorder, b: &Backing) -> bool {
    let snap = rec.snapshot();
    (0..b.h).all(|y| {
        (0..b.w).all(|x| Recorder::snapshot_pixel(&snap, x as usize, y as usize) == b.pixel(x, y))
    })
}

/// A shell on `rec` presenting on its own thread.
fn threaded(rec: &Recorder) -> WaylandShell {
    let mut shell = rec.open();
    shell.set_presenter_thread(true).unwrap();
    shell
}

/// A top-level of `b`'s size whose first configure is acked (a content ack) and whose first picture, `b` filled with
/// `BG`, the presenter has committed.
fn window(shell: &mut WaylandShell, rec: &Recorder, b: &mut Backing) -> u64 {
    let id = shell
        .window_new(&WindowDesc {
            w: b.w,
            h: b.h,
            title: "t".into(),
            app_id: "nidhogg.test".into(),
            flags: 0,
        })
        .unwrap();
    let ev = wait_for(shell, |e| e.kind() == Kind::Configure && e.window == id);
    let seq = next_seq(rec);
    shell
        .ack_configure(id, configure_of(&ev).serial, NHSHELL_ACK_CONTENT)
        .unwrap();
    b.fill_all(BG);
    assert!(publish(shell, id, &[(&*b, id)], 0));
    until(shell, "the first picture", |_| {
        !buffer_commits(rec, seq, id).is_empty()
    });
    id
}

/// A colour no two rows of one test share.
fn colour(n: &mut u32) -> u32 {
    *n += 1;
    0xff00_0000 | (n.wrapping_mul(2_654_435_761) >> 8)
}

/// The host makes the window its own output (as the glue does on its first full-window scroll) and draws it.
fn promote(b: &mut Backing, n: &mut u32) {
    glitnir::scene::with(b.g, |s| {
        s.upsert(&LayerDesc {
            id: WIN,
            parent: b.g,
            kind: 2,
            rect: VIEW,
            scale: 1.0,
            visible: 1,
        })?;
        s.set_output(WIN, true)
    })
    .unwrap();
    for y in VIEW.y..VIEW.y + VIEW.h {
        b.fill(Rect::new(VIEW.x, y, VIEW.w, 1), colour(n));
    }
}

/// The window goes back into its frame.
fn demote(b: &Backing) {
    glitnir::scene::with(b.g, |s| s.set_output(WIN, false)).unwrap();
}

/// A full-window scroll by `dy` rows (negative: up), as redisplay does it: the backing's rows move, Glitnir hears the
/// run, and the exposed rows are drawn.
fn scroll(b: &mut Backing, dy: i32, n: &mut u32) {
    let w4 = (b.w * 4) as usize;
    for i in 0..VIEW.h {
        let y = if dy > 0 {
            VIEW.y + VIEW.h - 1 - i
        } else {
            VIEW.y + i
        };
        let from = y - dy;
        if (VIEW.y..VIEW.y + VIEW.h).contains(&from) {
            let at = from as usize * w4;
            b.px.copy_within(at..at + w4, y as usize * w4);
        }
    }
    let exposed = if dy < 0 {
        Rect::new(VIEW.x, VIEW.y + VIEW.h + dy, VIEW.w, -dy)
    } else {
        Rect::new(VIEW.x, VIEW.y, VIEW.w, dy)
    };
    glitnir::damage::scroll(b.g, WIN, VIEW, dy, exposed);
    for y in exposed.y..exposed.y + exposed.h {
        b.fill(Rect::new(exposed.x, y, exposed.w, 1), colour(n));
    }
}

/// The single content layer of root `id`: (surface id, view offset, ring height, each buffer's resident blocks).
fn layer(shell: &WaylandShell, id: u64) -> (u64, i32, i32, Vec<u64>) {
    let l = shell.content_layers(id);
    assert_eq!(l.len(), 1, "one content layer");
    l[0].clone()
}

/// A threaded shell with a promoted window whose layer has committed its first picture: (shell, backing, root,
/// layer surface, colour counter).
fn promoted(rec: &Recorder) -> (WaylandShell, Backing, u64, u64, u32) {
    let mut shell = threaded(rec);
    let mut b = Backing::new(400, 300);
    let id = window(&mut shell, rec, &mut b);
    let mut n = 0;
    let seq = next_seq(rec);
    promote(&mut b, &mut n);
    assert!(publish(&mut shell, id, &[(&b, id)], 0));
    until(&mut shell, "the layer's first buffer", |s| {
        s.content_layers(id)
            .first()
            .is_some_and(|l| !buffer_commits(rec, seq, l.0).is_empty())
    });
    let surface = layer(&shell, id).0;
    (shell, b, id, surface, n)
}

/// Scroll by `dy`, publish, and wait for the layer's buffer commit: that commit.
fn scroll_step(
    shell: &mut WaylandShell,
    rec: &Recorder,
    b: &mut Backing,
    id: u64,
    surface: u64,
    dy: i32,
    n: &mut u32,
) -> String {
    let seq = next_seq(rec);
    scroll(b, dy, n);
    assert!(publish(shell, id, &[(&*b, id)], 0));
    until(shell, "the layer's commit", |_| {
        !buffer_commits(rec, seq, surface).is_empty()
    });
    buffer_commits(rec, seq, surface).remove(0)
}

/// The recorder's `tree` node of surface `id`.
fn node(rec: &Recorder, id: u64) -> String {
    let t = rec.control(r#"{"cmd":"tree"}"#);
    t.split("{\"above\":")
        .skip(1)
        .find(|n| n.contains(&format!("\"id\":{id},")))
        .map(|n| format!("{{\"above\":{n}"))
        .unwrap_or_default()
}

/// Surface `id`'s subsurfaces, bottom to top.
fn children(rec: &Recorder, id: u64) -> Vec<u64> {
    let n = node(rec, id);
    numbers_after(&n, "\"children\":")
        .unwrap_or_default()
        .into_iter()
        .map(|v| v as u64)
        .collect()
}

mod layers {
    use super::*;

    /// A full-window scroll of dy moves the layer's viewport source by -dy rows within the ring's buffer.
    #[test]
    fn scroll_is_a_viewport_source_change() {
        let rec = Recorder::start("sway");
        let (mut shell, mut b, id, surface, mut n) = promoted(&rec);
        let first = buffer_commits(&rec, 0, surface).pop().unwrap();
        let s0 = source(&first);
        assert_eq!((s0[2], s0[3]), (400.0, 240.0), "the view: {first}");
        let c = scroll_step(&mut shell, &rec, &mut b, id, surface, -16, &mut n);
        let s1 = source(&c);
        assert_eq!(s1[1], s0[1] + 16.0, "the source moved down 16 rows: {c}");
        let (_, offset, height, _) = layer(&shell, id);
        assert_eq!(f64::from(offset), s1[1]);
        assert_eq!(height, 480, "a ring twice the view");
        assert_eq!(number(&c, "h"), Some(480), "the ring's buffer: {c}");
        assert_eq!(number(&c, "w"), Some(400), "the ring's buffer: {c}");
        until(&mut shell, "the picture", |_| shows(&rec, &b));
    }

    /// The layer's commit damages the exposed rows only (buffer rows of the moved view), and the root's frame is
    /// not damaged inside the window.
    #[test]
    fn damage_covers_only_exposed_rows() {
        let rec = Recorder::start("sway");
        let (mut shell, mut b, id, surface, mut n) = promoted(&rec);
        let seq = next_seq(&rec);
        let c = scroll_step(&mut shell, &rec, &mut b, id, surface, -16, &mut n);
        let off = source(&c)[1] as i32;
        let d = damage(&c);
        let exposed = Rect::new(0, off + VIEW.h - 16, 400, 16);
        assert!(!d.is_empty(), "{c}");
        assert!(
            d.iter().all(|r| r.intersect(&exposed) == *r),
            "damage beyond the exposed rows {exposed:?}: {d:?}"
        );
        assert_eq!(area(&d), exposed.area(), "{d:?}");
        for r in commits(&rec, seq, id) {
            assert!(
                damage(&r).iter().all(|x| x.intersect(&VIEW).is_empty()),
                "the frame damaged inside the window: {r}"
            );
        }
        until(&mut shell, "the picture", |_| shows(&rec, &b));
    }

    /// Scrolling past the ring's end rebases once: the view goes back to the middle and that commit alone damages
    /// the whole view; the picture stays the backing's.
    #[test]
    fn rebase_damages_the_whole_layer_once() {
        let rec = Recorder::start("sway");
        let (mut shell, mut b, id, surface, mut n) = promoted(&rec);
        let (_, middle, height, _) = layer(&shell, id);
        let mut whole = 0;
        // 480 rows, a 240-row view from 120: the eighth 16-row step would leave the buffer
        for i in 0..10 {
            let c = scroll_step(&mut shell, &rec, &mut b, id, surface, -16, &mut n);
            let off = source(&c)[1] as i32;
            assert!(off >= 0 && off + VIEW.h <= height, "{c}");
            if area(&damage(&c)) == VIEW.area() {
                whole += 1;
                assert_eq!(
                    (i, off),
                    (7, middle),
                    "rebased at the end, to the middle: {c}"
                );
            } else {
                assert!(area(&damage(&c)) <= 400 * 16, "{c}");
            }
        }
        assert_eq!(whole, 1, "one whole-layer damage");
        until(&mut shell, "the picture", |_| shows(&rec, &b));
    }

    /// After scrolling both ways, each ring buffer keeps at most the view and one view of margin resident.
    #[test]
    fn rows_outside_the_view_are_punched() {
        let rec = Recorder::start("sway");
        let (mut shell, mut b, id, surface, mut n) = promoted(&rec);
        for dy in [-24, 24] {
            for _ in 0..20 {
                scroll_step(&mut shell, &rec, &mut b, id, surface, dy, &mut n);
            }
        }
        let (_, _, _, blocks) = layer(&shell, id);
        let stride = glitnir::pool::stride_for(VIEW.w) as u64;
        let page = 4096 / 512;
        let bound = 2 * VIEW.h as u64 * stride / 512 + page;
        assert!(!blocks.is_empty());
        for bl in blocks {
            assert!(bl <= bound, "{bl} blocks resident, at most {bound}");
        }
        until(&mut shell, "the picture", |_| shows(&rec, &b));
    }

    /// The layer is a sync subsurface of the root, right above it and below the root's child windows, also after a
    /// child is raised against the root.
    #[test]
    fn layer_sits_above_root_below_children() {
        let rec = Recorder::start("sway");
        let mut shell = threaded(&rec);
        let mut b = Backing::new(400, 300);
        let id = window(&mut shell, &rec, &mut b);
        let kid = shell
            .child_new(&ChildDesc {
                kind: NHSHELL_KIND_CHILD,
                parent: id,
                x: 20,
                y: 20,
                w: 100,
                h: 60,
                anchor: Anchor::default(),
                flags: 0,
            })
            .unwrap();
        let mut n = 0;
        promote(&mut b, &mut n);
        assert!(publish(&mut shell, id, &[(&b, id)], 0));
        until(&mut shell, "the layer", |s| {
            !s.content_layers(id).is_empty()
        });
        let surface = layer(&shell, id).0;
        until(&mut shell, "the layer below the child", |_| {
            children(&rec, id) == vec![surface, kid]
        });
        assert!(node(&rec, surface).contains("\"sync\":true"));
        // the child asks to sit right above the root: the layer goes back under it with the same commit
        shell.stack(kid, 0, true).unwrap();
        scroll_step(&mut shell, &rec, &mut b, id, surface, -16, &mut n);
        until(&mut shell, "still below the child", |_| {
            children(&rec, id) == vec![surface, kid]
        });
    }

    /// The layer's input region is empty: pointer events fall through to the root.
    #[test]
    fn layer_takes_no_input() {
        let rec = Recorder::start("sway");
        let (_shell, _b, _id, surface, _n) = promoted(&rec);
        let first = buffer_commits(&rec, 0, surface).remove(0);
        assert!(first.contains("\"input\":[]"), "{first}");
        let n = node(&rec, surface);
        assert!(n.contains("\"input_region\":[]"), "{n}");
    }

    /// Demotion: the layer's `attach(NULL)` is applied by the one root commit that draws the window in the frame
    /// again, and the layer goes after it; the picture is the backing's throughout.
    #[test]
    fn demote_unmaps_with_the_root_commit() {
        let rec = Recorder::start("sway");
        let (mut shell, mut b, id, surface, mut n) = promoted(&rec);
        scroll_step(&mut shell, &rec, &mut b, id, surface, -16, &mut n);
        until(&mut shell, "the scrolled picture", |_| shows(&rec, &b));
        let seq = next_seq(&rec);
        demote(&b);
        assert!(publish(&mut shell, id, &[(&b, id)], 0));
        until(&mut shell, "the unmap", |_| {
            commits(&rec, seq, surface)
                .iter()
                .any(|c| c.contains("\"buffer\":null"))
        });
        let unmap = commits(&rec, seq, surface)
            .into_iter()
            .find(|c| c.contains("\"buffer\":null"))
            .unwrap();
        let by = number(&unmap, "applied_by").expect("applied by a root commit");
        let root = commits(&rec, seq, id)
            .into_iter()
            .find(|c| number(c, "seq") == Some(by))
            .expect("the applying commit is the root's");
        assert!(root.contains("\"buffer\":{"), "with a buffer: {root}");
        assert!(
            area(
                &damage(&root)
                    .iter()
                    .map(|r| r.intersect(&VIEW))
                    .collect::<Vec<_>>()
            ) >= VIEW.area(),
            "the window recomposed in the frame: {root}"
        );
        until(&mut shell, "the layer gone", |s| {
            s.content_layers(id).is_empty() && node(&rec, surface).is_empty()
        });
        until(&mut shell, "the picture", |_| shows(&rec, &b));
    }

    /// Review Focus 1: the compositor holds every buffer it is done with; 100 scrolls later no held ring buffer was
    /// written, and once the buffers come back the newest picture shows.
    #[test]
    fn never_writes_a_held_ring_buffer() {
        let rec = Recorder::start("sway");
        let (mut shell, mut b, id, surface, mut n) = promoted(&rec);
        rec.control_ok(r#"{"cmd":"mode","hold_buffers":64}"#);
        for i in 0..100 {
            scroll(&mut b, if i % 3 == 2 { 16 } else { -16 }, &mut n);
            publish(&mut shell, id, &[(&b, id)], 0);
            while shell.next_event().unwrap().is_some() {}
            std::thread::sleep(Duration::from_millis(2));
        }
        let w = rec.control(r#"{"cmd":"held_buffer_writes","since":0}"#);
        assert!(w.contains("\"writes\":[]"), "held buffers written: {w}");
        rec.control_ok(r#"{"cmd":"mode","hold_buffers":0}"#);
        rec.control_ok(r#"{"cmd":"release"}"#);
        until(&mut shell, "the newest picture", |_| shows(&rec, &b));
        let w = rec.control(r#"{"cmd":"held_buffer_writes","since":0}"#);
        assert!(w.contains("\"writes\":[]"), "held buffers written: {w}");
        assert!(!buffer_commits(&rec, 0, surface).is_empty());
        assert!(shell.next_event().is_ok(), "no protocol error");
    }
}

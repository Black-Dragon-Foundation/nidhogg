//! T1 tests of presenting through the glue against nh-recorder (manual clock): a draw publishes and wakes the tree,
//! nothing drawn publishes nothing, the key time ends with a picture or with a redisplay that drew nothing, an
//! undrawable surface keeps its last buffer, inline and thread modes commit the same, and the surface list orders the
//! tree.  The harness is the shell's (`../shell/tests/common`).

#[path = "../../shell/tests/common/mod.rs"]
mod common;

use common::{Recorder, number, readable_within};
use nhglue::abi::*;
use nhglue::backing::{NhgBacking, NhgFrameGeom, NhgRect};
use nhglue::ffi::*;
use nhglue::frame::NHG_FRAME_CHILD;
use nhglue::present::*;
use nhshell::api::*;
use nhshell::event::*;
use std::ptr;
use std::time::{Duration, Instant};

const BG: u32 = 0xff20_4060;

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

/// The shell inside `d`, for what the glue does not wrap yet (windows, records).
fn shell<'a>(d: *mut NhgDisplay) -> &'a mut dyn ShellBackend {
    // SAFETY: a live display's shell handle, used only between glue calls.
    unsafe { (*nhg_shell(d)).backend_mut() }
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

/// A child window of `parent` at (10, 10), `w` x `h`.
fn child(d: *mut NhgDisplay, parent: u64, w: i32, h: i32) -> u64 {
    let id = shell(d)
        .child_new(&ChildDesc {
            kind: NHSHELL_KIND_CHILD,
            parent,
            x: 10,
            y: 10,
            w,
            h,
            anchor: Anchor::default(),
            flags: 0,
        })
        .unwrap();
    // SAFETY: a live display.
    assert_eq!(
        unsafe { nhg_frame_register(d, id, NHG_FRAME_CHILD) },
        NHG_OK
    );
    id
}

/// A frame's backing in the glue, as C keeps it.
struct Surf {
    window: u64,
    key: u64,
    back: NhgBacking,
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

/// Make (or resize) `s`'s backing `w` x `h`, as `nhwl_backing_resize` does.
fn size(d: *mut NhgDisplay, s: &mut Surf, w: i32, h: i32) {
    let g = geom(w, h);
    // SAFETY: a live display; `g`, the key and the backing are valid for the calls.
    unsafe {
        assert!(nhg_layer_ensure(d, &mut s.key, &g) >= 0);
        assert_eq!(nhg_backing(d, s.key, &g, &mut s.back), 0);
    }
}

fn surface(d: *mut NhgDisplay, window: u64, w: i32, h: i32) -> Surf {
    let mut s = Surf {
        window,
        key: 0,
        back: NhgBacking {
            pixels: ptr::null_mut(),
            stride: 0,
            w: 0,
            h: 0,
            generation: 0,
        },
    };
    size(d, &mut s, w, h);
    s
}

/// Paint `r` of `s` with `argb` and damage it, as the rasteriser and `nhwl_damage` do.
fn fill(d: *mut NhgDisplay, s: &Surf, r: NhgRect, argb: u32) {
    for y in r.y..r.y + r.h {
        for x in r.x..r.x + r.w {
            // SAFETY: (x, y) lies inside the borrowed backing.
            unsafe { *s.back.pixels.cast::<u32>().add((y * s.back.w + x) as usize) = argb };
        }
    }
    // SAFETY: a live display; one readable rectangle.
    assert_eq!(unsafe { nhg_damage(d, s.key, &r, 1) }, 1);
}

fn all(s: &Surf) -> NhgRect {
    NhgRect {
        x: 0,
        y: 0,
        w: s.back.w,
        h: s.back.h,
    }
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

fn present(d: *mut NhgDisplay, root: u64, list: &[NhgSurface], flags: u32, key: u64) -> i32 {
    // SAFETY: a live display; `list` is readable for the call.
    unsafe { nhg_present(d, root, list.as_ptr(), list.len(), flags, key) }
}

/// The root's `FRAME_READY`, then the inline present it asks for.
fn ready(d: *mut NhgDisplay, root: u64) -> i32 {
    wait_for(d, |e| e.kind() == Kind::FrameReady && e.window == root);
    present(d, root, &[], NHG_PRESENT_READY, 0)
}

fn stats(d: *mut NhgDisplay) -> NhgPresenterStats {
    let mut st = NhgPresenterStats::default();
    // SAFETY: a live display; `st` is writable.
    assert_eq!(unsafe { nhg_presenter_stats(d, &mut st) }, NHG_OK);
    st
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

/// Surface `id`'s buffer commits from `seq` on, once there are `n` (3 s at most).
fn buffers(rec: &Recorder, seq: u64, id: u64, n: usize) -> Vec<String> {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        let all: Vec<String> = log(rec, seq)
            .into_iter()
            .filter(|c| of(c, id) && has_buffer(c))
            .collect();
        if all.len() >= n || Instant::now() > deadline {
            return all;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// A shown top-level of 200 x 100 with its backing, its first picture committed and its callback answered.
fn shown(rec: &Recorder, d: *mut NhgDisplay) -> Surf {
    let root = toplevel(d, 200, 100);
    let s = surface(d, root, 200, 100);
    fill(d, &s, all(&s), BG);
    let list = [entry(&s, 0, NHG_SURF_DRAWABLE)];
    assert_ne!(
        present(d, root, &list, NHG_PRESENT_BACKSTOP, 0) & NHG_PRESENTED_PUBLISHED,
        0
    );
    assert_ne!(
        ready(d, root) & NHG_PRESENTED_COMMITTED,
        0,
        "the first picture"
    );
    tick(rec, d);
    s
}

/// The tests are named `present::…`, like the other crates' T1 tests.
#[allow(clippy::module_inception)]
mod present {
    use super::*;

    /// Drawing then a consistency point publishes; the publish wakes the tree (its `FRAME_READY`), and the inline
    /// present commits a buffer with the damage.
    #[test]
    fn damage_wakes_and_publishes() {
        let rec = Recorder::start_clock("sway", "manual");
        let d = open(&rec);
        let s = shown(&rec, d);
        let seq = next_seq(&rec);
        let r = NhgRect {
            x: 10,
            y: 10,
            w: 20,
            h: 8,
        };
        fill(d, &s, r, 0xff00_ff00);
        let list = [entry(&s, 0, NHG_SURF_DRAWABLE)];
        let before = stats(d).counters.packets;
        assert_eq!(
            present(d, s.window, &list, NHG_PRESENT_BACKSTOP, 0),
            NHG_PRESENTED_PUBLISHED
        );
        assert_eq!(stats(d).counters.packets, before + 1);
        assert_ne!(ready(d, s.window) & NHG_PRESENTED_COMMITTED, 0);
        let b = buffers(&rec, seq, s.window, 1);
        assert_eq!(b.len(), 1, "{b:?}");
        close(d);
    }

    /// A consistency point with nothing drawn publishes nothing and wakes nothing: no packet, no `FRAME_READY`, no
    /// commit.
    #[test]
    fn no_damage_publishes_nothing() {
        let rec = Recorder::start_clock("sway", "manual");
        let d = open(&rec);
        let s = shown(&rec, d);
        let seq = next_seq(&rec);
        let before = stats(d).counters;
        let list = [entry(&s, 0, NHG_SURF_DRAWABLE)];
        for _ in 0..3 {
            assert_eq!(present(d, s.window, &list, NHG_PRESENT_BACKSTOP, 0), 0);
        }
        let evs = collect(d, 200);
        assert!(
            !evs.iter().any(|e| e.kind() == Kind::FrameReady),
            "a FRAME_READY with nothing drawn"
        );
        let after = stats(d).counters;
        assert_eq!(
            (after.packets, after.commits),
            (before.packets, before.commits)
        );
        assert!(
            log(&rec, seq).iter().all(|c| !of(c, s.window)),
            "a commit with nothing drawn"
        );
        close(d);
    }

    /// A key read before a redisplay that drew nothing is done (C clears it); one read during it
    /// waits; a picture carries it; a root that may not show a buffer keeps it.
    #[test]
    fn key_time_cleared_after_a_redisplay_that_drew_nothing() {
        let rec = Recorder::start_clock("sway", "manual");
        let d = open(&rec);
        let s = shown(&rec, d);
        let list = [entry(&s, 0, NHG_SURF_DRAWABLE)];
        let key = 1_000;
        // nothing drawn, the key read during the redisplay: it waits for the next one
        assert_eq!(present(d, s.window, &list, NHG_PRESENT_BACKSTOP, key), 0);
        // nothing drawn, the key read before it: done
        assert_eq!(
            present(
                d,
                s.window,
                &list,
                NHG_PRESENT_BACKSTOP | NHG_PRESENT_KEY_SEEN,
                key
            ),
            NHG_PRESENTED_KEY_DONE
        );
        // a root that may not show a buffer (new_size_p): nothing, and the key waits
        let hidden = [entry(&s, 0, 0)];
        assert_eq!(
            present(
                d,
                s.window,
                &hidden,
                NHG_PRESENT_BACKSTOP | NHG_PRESENT_KEY_SEEN,
                key
            ),
            0
        );
        // drawn: the packet carries the key
        fill(
            d,
            &s,
            NhgRect {
                x: 0,
                y: 0,
                w: 4,
                h: 4,
            },
            0xff11_1111,
        );
        assert_eq!(
            present(d, s.window, &list, NHG_PRESENT_BACKSTOP, key),
            NHG_PRESENTED_PUBLISHED | NHG_PRESENTED_KEY_DONE
        );
        // no key: nothing to clear
        fill(
            d,
            &s,
            NhgRect {
                x: 0,
                y: 0,
                w: 4,
                h: 4,
            },
            0xff22_2222,
        );
        assert_eq!(
            present(
                d,
                s.window,
                &list,
                NHG_PRESENT_BACKSTOP | NHG_PRESENT_KEY_SEEN,
                0
            ),
            NHG_PRESENTED_PUBLISHED
        );
        close(d);
    }

    /// A child with `DRAWABLE` clear (resized, waiting for the redisplay of its new size) while the
    /// root publishes keeps its last buffer: no buffer of either size goes to it; once drawable, its waiting damage
    /// shows in a buffer of the new size.
    #[test]
    fn undrawable_surface_is_not_published() {
        let rec = Recorder::start_clock("sway", "manual");
        let d = open(&rec);
        let root = toplevel(d, 200, 100);
        let r = surface(d, root, 200, 100);
        let c = child(d, root, 100, 60);
        let mut k = surface(d, c, 100, 60);
        let seq = next_seq(&rec);
        fill(d, &r, all(&r), BG);
        fill(d, &k, all(&k), 0xff80_0000);
        let both = |kf: u32, k: &Surf| [entry(&r, 0, NHG_SURF_DRAWABLE), entry(k, root, kf)];
        assert_ne!(
            present(
                d,
                root,
                &both(NHG_SURF_DRAWABLE, &k),
                NHG_PRESENT_BACKSTOP,
                0
            ) & NHG_PRESENTED_PUBLISHED,
            0
        );
        assert_ne!(ready(d, root) & NHG_PRESENTED_COMMITTED, 0);
        assert_eq!(
            buffers(&rec, seq, c, 1).len(),
            1,
            "the child's first buffer"
        );
        tick(&rec, d);

        // the child resizes (its backing follows at once) but waits for its redisplay; the root types
        let seq = next_seq(&rec);
        size(d, &mut k, 120, 70);
        fill(d, &k, all(&k), 0xff00_8000);
        fill(
            d,
            &r,
            NhgRect {
                x: 150,
                y: 80,
                w: 8,
                h: 8,
            },
            0xffff_ffff,
        );
        assert_ne!(
            present(d, root, &both(0, &k), NHG_PRESENT_BACKSTOP, 0) & NHG_PRESENTED_PUBLISHED,
            0
        );
        assert_ne!(
            ready(d, root) & NHG_PRESENTED_COMMITTED,
            0,
            "the root keeps presenting"
        );
        assert_eq!(buffers(&rec, seq, root, 1).len(), 1);
        let kid = buffers(&rec, seq, c, 1);
        assert!(
            kid.is_empty(),
            "a buffer went to the undrawable child: {kid:?}"
        );
        tick(&rec, d);

        // drawn at its new size: the damage that waited shows
        let seq = next_seq(&rec);
        assert_ne!(
            present(
                d,
                root,
                &both(NHG_SURF_DRAWABLE, &k),
                NHG_PRESENT_BACKSTOP,
                0
            ) & NHG_PRESENTED_PUBLISHED,
            0
        );
        ready(d, root);
        let kid = buffers(&rec, seq, c, 1);
        assert_eq!(kid.len(), 1, "{kid:?}");
        assert!(
            kid[0].contains("\"w\":120") && kid[0].contains("\"h\":70"),
            "{}",
            kid[0]
        );
        close(d);
    }

    /// The same scripted session (a first picture, then three draws, each shown at the next frame) commits the same
    /// sequence of buffer and bare commits inline and on the presenter thread.
    #[test]
    fn inline_and_thread_commit_the_same() {
        let kinds = |thread: bool| -> Vec<&'static str> {
            let rec = Recorder::start_clock("sway", "manual");
            let d = open(&rec);
            // SAFETY: a live display.
            assert_eq!(
                unsafe { nhg_presenter_thread(d, i32::from(thread)) },
                NHG_OK
            );
            assert_eq!(stats(d).counters.thread_on, u32::from(thread));
            let seq = next_seq(&rec);
            let root = toplevel(d, 200, 100);
            let s = surface(d, root, 200, 100);
            let list = [entry(&s, 0, NHG_SURF_DRAWABLE)];
            for i in 0..4 {
                let r = if i == 0 {
                    all(&s)
                } else {
                    NhgRect {
                        x: 10 * i,
                        y: 10,
                        w: 8,
                        h: 8,
                    }
                };
                fill(d, &s, r, 0xff00_0000 | i as u32);
                assert_ne!(
                    present(d, root, &list, NHG_PRESENT_BACKSTOP, 0) & NHG_PRESENTED_PUBLISHED,
                    0
                );
                if thread {
                    // FRAME_READY does not come: the presenter commits by itself
                    assert_eq!(present(d, root, &[], NHG_PRESENT_READY, 0), 0);
                } else {
                    ready(d, root);
                }
                assert_eq!(
                    buffers(&rec, seq, root, i as usize + 1).len(),
                    i as usize + 1
                );
                tick(&rec, d);
            }
            let out = log(&rec, seq)
                .iter()
                .filter(|c| of(c, root))
                .map(|c| if has_buffer(c) { "buffer" } else { "bare" })
                .collect();
            close(d);
            out
        };
        let inline = kinds(false);
        assert_eq!(
            inline.iter().filter(|k| **k == "buffer").count(),
            4,
            "{inline:?}"
        );
        assert_eq!(kinds(true), inline);
    }

    /// The surface list is the tree, bottom to top: the root first, each parent before its children.  A surface may
    /// show a buffer only when its ancestors may.  A list that is not the root's tree is refused; a nested tree
    /// commits deepest first, the root last.
    #[test]
    fn tree_order_follows_the_surface_list() {
        let s = |window, parent, flags| NhgSurface {
            window,
            parent,
            frame: window + 100,
            flags,
            alpha_cut: 0,
        };
        let d1 = NHG_SURF_DRAWABLE;
        // the drawable flags follow the ancestors
        assert_eq!(
            drawable(1, &[s(1, 0, d1), s(2, 1, 0), s(3, 2, d1), s(4, 1, d1)]).unwrap(),
            [true, false, false, true]
        );
        assert_eq!(
            drawable(1, &[s(1, 0, 0), s(2, 1, d1)]).unwrap(),
            [false, false]
        );
        // not the root's tree, bottom to top
        for bad in [
            vec![s(2, 1, d1), s(1, 0, d1)],
            vec![s(1, 0, d1), s(3, 2, d1), s(2, 1, d1)],
            vec![s(1, 0, d1), s(2, 1, d1), s(2, 1, d1)],
            vec![s(1, 5, d1)],
            vec![s(1, 0, d1), s(0, 1, d1)],
        ] {
            assert_eq!(
                drawable(1, &bad),
                Err(nhglue::guard::GlueError::Arg),
                "{bad:?}"
            );
        }

        let rec = Recorder::start_clock("sway", "manual");
        let d = open(&rec);
        let root = toplevel(d, 200, 100);
        let r = surface(d, root, 200, 100);
        let c = child(d, root, 100, 60);
        let k = surface(d, c, 100, 60);
        let g = child(d, c, 40, 30);
        let gk = surface(d, g, 40, 30);
        let list = [entry(&r, 0, d1), entry(&k, root, d1), entry(&gk, c, d1)];
        let reversed = [list[2], list[1], list[0]];
        assert_eq!(
            present(d, root, &reversed, NHG_PRESENT_BACKSTOP, 0),
            NHG_E_ARG
        );
        let seq = next_seq(&rec);
        for x in [&r, &k, &gk] {
            fill(d, x, all(x), BG);
        }
        assert_ne!(
            present(d, root, &list, NHG_PRESENT_BACKSTOP, 0) & NHG_PRESENTED_PUBLISHED,
            0
        );
        assert_ne!(ready(d, root) & NHG_PRESENTED_COMMITTED, 0);
        buffers(&rec, seq, root, 1);
        let commits = log(&rec, seq);
        let first = |id: u64| {
            commits
                .iter()
                .position(|x| of(x, id) && has_buffer(x))
                .unwrap_or_else(|| panic!("no buffer for {id}: {commits:?}"))
        };
        assert!(first(g) < first(c) && first(c) < first(root), "{commits:?}");
        close(d);
    }
}

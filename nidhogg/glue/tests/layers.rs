//! Window layers: promotion on a full-window scroll, the opaque-root check, and every demotion trigger, against
//! Glitnir itself (no display needed: the shell follows what Glitnir's commits report).

mod layers {
    use glitnir::committed::Committed;
    use glitnir::damage::commit_snapshot;
    use glitnir::geom::Rect;
    use nhglue::backing::{Backings, NhgBacking, NhgFrameGeom, NhgRect, WINDOW_TAG};
    use nhglue::frame::{NHG_FRAME_CHILD, NHG_FRAME_POPUP, NHG_FRAME_TIP};
    use nhglue::layers::*;
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    const ALL: u32 = NHG_LAYERS_ON | NHG_LAYERS_OPAQUE_BG;
    /// The title strip above the native area.
    const STRIP: i32 = 20;
    const LINE: i32 = 10;

    /// A 200 x 100 native area under a 20-row strip, at `s120`.
    fn geom(s120: u32) -> NhgFrameGeom {
        let mut g = NhgFrameGeom {
            native_w: 200,
            native_h: 100,
            t: STRIP,
            bg_argb: 0xff00_0000,
            scale120: s120,
            ..Default::default()
        };
        let (w, h) = g.size();
        g.layer = NhgRect { x: 0, y: 0, w, h };
        g
    }

    /// A frame with its scene and backing.
    fn frame(b: &mut Backings, s120: u32) -> (u64, NhgBacking) {
        let mut key = 0;
        let g = geom(s120);
        b.ensure(&mut key, &g);
        let v = b.backing(key, &g).unwrap();
        commit(key);
        (key, v)
    }

    fn commit(key: u64) -> (Vec<Rect>, Arc<Committed>) {
        commit_snapshot(key).expect("a scene")
    }

    /// The window: the native area's top 80 rows (a mode line under it), full width.
    fn win() -> NhgRect {
        NhgRect {
            x: 0,
            y: 0,
            w: 200,
            h: 80,
        }
    }

    /// The text moves up one line: rows 10..80 to 0..70, the last line exposed.
    fn up(r: NhgRect) -> (NhgRect, i32) {
        (
            NhgRect {
                x: r.x,
                y: r.y + LINE,
                w: r.w,
                h: r.h - LINE,
            },
            r.y,
        )
    }

    fn rect(r: NhgRect, dy: i32) -> Rect {
        Rect::new(r.x, r.y + dy, r.w, r.h)
    }

    fn px(b: &NhgBacking, x: i32, y: i32) -> u32 {
        // SAFETY: (x, y) lies inside the borrowed backing, which outlives the read.
        unsafe { *b.pixels.cast::<u32>().add((y * b.w + x) as usize) }
    }

    fn put(b: &NhgBacking, x: i32, y: i32, v: u32) {
        // SAFETY: as `px`; nothing else reads the backing meanwhile.
        unsafe { *b.pixels.cast::<u32>().add((y * b.w + x) as usize) = v }
    }

    fn scroll(
        b: &mut Backings,
        key: u64,
        seq: u64,
        r: NhgRect,
        eligible: bool,
        now: Instant,
    ) -> bool {
        let (run, to) = up(r);
        scroll_at(b, key, seq, r, run, to, eligible, now).unwrap()
    }

    /// A window's first full-window scroll on an opaque root makes it its own output: a WINDOW layer with the
    /// tagged sequence number, at the window's rectangle in the backing (the strip below it counted).  The frame's
    /// damage leaves the window out.
    #[test]
    fn full_window_scroll_promotes() {
        let mut b = Backings::default();
        let (key, _) = frame(&mut b, 120);
        let t0 = Instant::now();
        assert!(scroll(&mut b, key, 7, win(), true, t0));
        assert_eq!(count(&mut b, key), 1);
        let (frame_damage, c) = commit(key);
        assert_eq!(c.outputs.len(), 1);
        let o = &c.outputs[0];
        assert_eq!(o.layer, 7 | WINDOW_TAG);
        assert_eq!(o.rect, rect(win(), STRIP));
        assert!(o.full);
        let w = rect(win(), STRIP);
        assert!(
            frame_damage.iter().all(|r| r.intersect(&w).is_empty()),
            "{frame_damage:?}"
        );
    }

    /// Once promoted, a full-window scroll is the output's offset: only the exposed line is damaged, in the output's
    /// coordinates.  The backing still moved its rows (the shell's front relies on it).
    #[test]
    fn promoted_scroll_is_an_offset_and_the_backing_moves() {
        let mut b = Backings::default();
        let (key, v) = frame(&mut b, 120);
        let t0 = Instant::now();
        scroll(&mut b, key, 8, win(), true, t0);
        commit(key);
        put(&v, 5, STRIP + 15, 0xffab_cdef);
        assert!(scroll(&mut b, key, 8, win(), true, t0));
        assert_eq!(
            px(&v, 5, STRIP + 5),
            0xffab_cdef,
            "the backing moved its rows"
        );
        let (frame_damage, c) = commit(key);
        let o = &c.outputs[0];
        assert!(!o.full);
        assert_eq!(o.scroll, -LINE);
        assert_eq!(o.damage, vec![Rect::new(0, 80 - LINE, 200, LINE)]);
        assert!(frame_damage.is_empty(), "{frame_damage:?}");
    }

    /// A scroll inside the window (an `open-line` in the middle) stays damage: no promotion.
    #[test]
    fn partial_scroll_does_not_promote() {
        let mut b = Backings::default();
        let (key, _) = frame(&mut b, 120);
        let run = NhgRect {
            x: 0,
            y: 40,
            w: 200,
            h: 30,
        };
        assert!(!scroll_at(&mut b, key, 9, win(), run, 50, true, Instant::now()).unwrap());
        assert_eq!(count(&mut b, key), 0);
        assert!(commit(key).1.outputs.is_empty());
        // narrower than the window: not full either
        let narrow = NhgRect { w: 100, ..win() };
        assert!(!full_window(win(), up(narrow).0, 0));
        assert!(full_window(win(), up(win()).0, 0));
        // down: rows 0..70 to 10..80
        assert!(full_window(win(), NhgRect { h: 70, ..win() }, LINE));
        assert!(!full_window(win(), NhgRect { h: 70, ..win() }, 0));
        // a partly visible last row is drawn again, not moved: up rows 10..70 to 0..60, down 0..60 to 10..70
        let short = NhgRect { h: 60, ..win() };
        assert!(full_window(win(), NhgRect { y: LINE, ..short }, 0));
        assert!(full_window(win(), short, LINE));
        // reaching past the window is not this window's scroll
        assert!(!full_window(win(), NhgRect { h: 80, ..win() }, LINE));
    }

    /// A frame that is not an opaque root never promotes: a child frame, a tip or a popup; an alpha below 1, or one
    /// the shell multiplies; an `alpha-background` below 1; no viewporter; `nhwl-scroll-layers` nil, or
    /// `integer-scales` at a fractional scale.
    #[test]
    fn only_opaque_roots_promote() {
        let ok = Root {
            viewporter: true,
            ..Root::default()
        };
        assert!(opaque_root(&ok, ALL, 120));
        assert!(opaque_root(&ok, ALL, 150));
        for kind in [NHG_FRAME_CHILD, NHG_FRAME_TIP, NHG_FRAME_POPUP] {
            assert!(!opaque_root(&Root { kind, ..ok }, ALL, 120));
        }
        assert!(!opaque_root(&Root { alpha_cut: 1, ..ok }, ALL, 120));
        assert!(!opaque_root(
            &Root {
                alpha_host: true,
                ..ok
            },
            ALL,
            120
        ));
        assert!(!opaque_root(
            &Root {
                viewporter: false,
                ..ok
            },
            ALL,
            120
        ));
        assert!(
            !opaque_root(&ok, NHG_LAYERS_ON, 120),
            "alpha-background below 1"
        );
        assert!(
            !opaque_root(&ok, NHG_LAYERS_OPAQUE_BG, 120),
            "nhwl-scroll-layers nil"
        );
        let int = ALL | NHG_LAYERS_INTEGER_SCALES;
        assert!(opaque_root(&ok, int, 240));
        assert!(!opaque_root(&ok, int, 150));

        let mut b = Backings::default();
        let (key, _) = frame(&mut b, 120);
        assert!(!scroll(&mut b, key, 10, win(), false, Instant::now()));
        assert_eq!(count(&mut b, key), 0);
    }

    /// At most four windows of a frame are their own outputs.
    #[test]
    fn at_most_four_per_frame() {
        let mut b = Backings::default();
        let (key, _) = frame(&mut b, 120);
        let t0 = Instant::now();
        for seq in 0..5u64 {
            let r = NhgRect {
                x: 40 * seq as i32,
                y: 0,
                w: 40,
                h: 80,
            };
            assert_eq!(
                scroll(&mut b, key, 20 + seq, r, true, t0),
                seq < 4,
                "window {seq}"
            );
        }
        assert_eq!(count(&mut b, key), 4);
        assert_eq!(commit(key).1.outputs.len(), 4);
    }

    /// Two seconds after its last full-window scroll, the window is part of the frame again at the next publish:
    /// Glitnir drops the output and damages its rectangle on the frame.  A scroll before then keeps it.
    #[test]
    fn idle_window_is_demoted_after_two_seconds() {
        let mut b = Backings::default();
        let (key, _) = frame(&mut b, 120);
        let t0 = Instant::now();
        let ws = [NhgLayerWindow {
            seq: 30,
            rect: win(),
        }];
        scroll(&mut b, key, 30, win(), true, t0);
        commit(key);
        let t1 = t0 + Duration::from_millis(1500);
        scroll(&mut b, key, 30, win(), true, t1);
        assert_eq!(
            sync_at(&mut b, key, &ws, true, t0 + Duration::from_millis(3000)).unwrap(),
            1
        );
        commit(key);
        assert_eq!(sync_at(&mut b, key, &ws, true, t1 + IDLE).unwrap(), 0);
        let (frame_damage, c) = commit(key);
        assert!(c.outputs.is_empty());
        assert!(
            c.layers.iter().all(|p| p.id != (30 | WINDOW_TAG)),
            "the layer left the scene"
        );
        let w = rect(win(), STRIP);
        assert!(
            frame_damage.iter().any(|r| r.intersect(&w) == w),
            "{frame_damage:?}"
        );
    }

    /// A window resized, split (its rectangle changes) or deleted (gone from the list) is demoted.
    #[test]
    fn resized_split_or_deleted_window_is_demoted() {
        let mut b = Backings::default();
        let (key, _) = frame(&mut b, 120);
        let t0 = Instant::now();
        let half = NhgRect { h: 40, ..win() };
        scroll(&mut b, key, 40, win(), true, t0);
        scroll(&mut b, key, 41, half, true, t0);
        assert_eq!(count(&mut b, key), 2);
        // 40 split: its upper half stays; 41 deleted
        let ws = [NhgLayerWindow {
            seq: 40,
            rect: half,
        }];
        assert_eq!(sync_at(&mut b, key, &ws, true, t0).unwrap(), 0);
        assert!(commit(key).1.outputs.is_empty());
    }

    /// A scroll that comes with a new rectangle demotes before it moves anything; the next full-window scroll at
    /// that rectangle promotes again.
    #[test]
    fn scroll_with_a_new_rectangle_demotes_first() {
        let mut b = Backings::default();
        let (key, _) = frame(&mut b, 120);
        let t0 = Instant::now();
        scroll(&mut b, key, 50, win(), true, t0);
        commit(key);
        let half = NhgRect { h: 40, ..win() };
        let partial = NhgRect {
            x: 0,
            y: 20,
            w: 200,
            h: 10,
        };
        assert!(!scroll_at(&mut b, key, 50, half, partial, 10, true, t0).unwrap());
        assert!(commit(key).1.outputs.is_empty());
        assert!(scroll(&mut b, key, 50, half, true, t0));
    }

    /// A scale change demotes every window layer of the frame.
    #[test]
    fn scale_change_demotes() {
        let mut b = Backings::default();
        let (mut key, _) = frame(&mut b, 120);
        let t0 = Instant::now();
        let ws = [NhgLayerWindow {
            seq: 60,
            rect: win(),
        }];
        scroll(&mut b, key, 60, win(), true, t0);
        assert_eq!(sync_at(&mut b, key, &ws, true, t0).unwrap(), 1);
        b.ensure(&mut key, &geom(240));
        assert_eq!(sync_at(&mut b, key, &ws, true, t0).unwrap(), 0);
        assert!(commit(key).1.outputs.is_empty());
    }

    /// The title strip going (or coming) moves the window in the backing though its box stays: demoted.
    #[test]
    fn title_strip_change_demotes() {
        let mut b = Backings::default();
        let (mut key, _) = frame(&mut b, 120);
        let t0 = Instant::now();
        let ws = [NhgLayerWindow {
            seq: 65,
            rect: win(),
        }];
        scroll(&mut b, key, 65, win(), true, t0);
        let mut g = geom(120);
        g.t = 0;
        let (w, h) = g.size();
        g.layer = NhgRect { x: 0, y: 0, w, h };
        b.ensure(&mut key, &g);
        assert_eq!(sync_at(&mut b, key, &ws, true, t0).unwrap(), 0);
    }

    /// A frame that stops being an opaque root (translucent, or `nhwl-scroll-layers` turned off) loses all its
    /// window layers at the next publish.
    #[test]
    fn translucent_frame_demotes_all() {
        let mut b = Backings::default();
        let (key, _) = frame(&mut b, 120);
        let t0 = Instant::now();
        let left = NhgRect { w: 100, ..win() };
        let right = NhgRect { x: 100, ..left };
        scroll(&mut b, key, 70, left, true, t0);
        scroll(&mut b, key, 71, right, true, t0);
        let ws = [
            NhgLayerWindow {
                seq: 70,
                rect: left,
            },
            NhgLayerWindow {
                seq: 71,
                rect: right,
            },
        ];
        assert_eq!(sync_at(&mut b, key, &ws, true, t0).unwrap(), 2);
        assert_eq!(sync_at(&mut b, key, &ws, false, t0).unwrap(), 0);
        assert!(commit(key).1.outputs.is_empty());
    }

    /// At a reconnect every frame's window layers go; a forgotten frame takes its own with it.
    #[test]
    fn drop_all_and_forget() {
        let mut b = Backings::default();
        let (k1, _) = frame(&mut b, 120);
        let (k2, _) = frame(&mut b, 120);
        let t0 = Instant::now();
        scroll(&mut b, k1, 80, win(), true, t0);
        scroll(&mut b, k2, 81, win(), true, t0);
        drop_all(&mut b);
        assert_eq!((count(&mut b, k1), count(&mut b, k2)), (0, 0));
        assert!(commit(k1).1.outputs.is_empty());
        scroll(&mut b, k1, 80, win(), true, t0);
        b.forget(k1);
        assert_eq!(count(&mut b, k1), 0);
        b.forget(k2);
    }
}

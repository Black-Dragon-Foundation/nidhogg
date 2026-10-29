//! Backings, damage and scroll, against Glitnir itself (no display needed).

mod backing {
    use glitnir::ffi::{CommitOut, glitnir_commit};
    use nhglue::abi::{NHG_E_ARG, NHG_E_STATE};
    use nhglue::backing::{Backings, NHG_GEOM_FRESH, NhgBacking, NhgFrameGeom, NhgRect};
    use nhglue::ffi::{nhg_backing_forget, nhg_damage, nhg_layer_ensure, nhg_scroll_window};

    /// Glitnir's damage for frame `key` since the last call (publishing commits it the same way).
    fn committed(key: u64) -> Vec<NhgRect> {
        let mut co = CommitOut {
            rects: std::ptr::null(),
            n: 0,
        };
        // SAFETY: `co` is a writable CommitOut for the call.
        unsafe { glitnir_commit(key, &mut co) };
        if co.n == 0 || co.rects.is_null() {
            return Vec::new();
        }
        // SAFETY: Glitnir hands back `co.n` rectangles, valid until the root's next commit.
        let rs = unsafe { std::slice::from_raw_parts(co.rects, co.n) };
        rs.iter().map(|r| NhgRect::from(*r)).collect()
    }

    fn geom(native_w: i32, native_h: i32, l: i32, t: i32, bg: u32) -> NhgFrameGeom {
        let mut g = NhgFrameGeom {
            native_w,
            native_h,
            l,
            t,
            r: l,
            b: 0,
            bg_argb: bg,
            scale120: 120,
            ..Default::default()
        };
        let (w, h) = g.size();
        g.layer = NhgRect { x: 0, y: 0, w, h };
        g
    }

    fn px(b: &NhgBacking, x: i32, y: i32) -> u32 {
        assert!(x < b.w && y < b.h);
        // SAFETY: (x, y) lies inside the borrowed backing, which outlives the read.
        unsafe { *b.pixels.cast::<u32>().add((y * b.w + x) as usize) }
    }

    fn put(b: &NhgBacking, x: i32, y: i32, v: u32) {
        // SAFETY: as `px`; nothing else reads the backing meanwhile.
        unsafe { *b.pixels.cast::<u32>().add((y * b.w + x) as usize) = v }
    }

    fn bbox(rs: &[NhgRect]) -> NhgRect {
        let x0 = rs.iter().map(|r| r.x).min().unwrap();
        let y0 = rs.iter().map(|r| r.y).min().unwrap();
        let x1 = rs.iter().map(|r| r.x + r.w).max().unwrap();
        let y1 = rs.iter().map(|r| r.y + r.h).max().unwrap();
        NhgRect {
            x: x0,
            y: y0,
            w: x1 - x0,
            h: y1 - y0,
        }
    }

    /// A first backing is all background; a resize keeps the overlap and leaves the rest transparent (what Cairo's
    /// SOURCE paint of the old surface left); `NHG_GEOM_FRESH` starts over in the background.
    #[test]
    fn resize_keeps_the_overlap_and_fills_bg() {
        let mut b = Backings::default();
        let mut key = 0;
        let g = geom(4, 3, 0, 0, 0xff11_2233);
        assert!(b.ensure(&mut key, &g));
        assert_ne!(key, 0);
        let v = b.backing(key, &g).unwrap();
        assert_eq!((v.w, v.h, v.stride, v.generation), (4, 3, 16, 1));
        assert!((0..3).all(|y| (0..4).all(|x| px(&v, x, y) == 0xff11_2233)));
        put(&v, 0, 0, 0xdead_beef);
        put(&v, 3, 2, 0xdead_beef);
        // the same size: nothing happens
        let same = b.backing(key, &g).unwrap();
        assert_eq!((same.pixels, same.generation), (v.pixels, 1));

        let g2 = geom(6, 2, 0, 0, 0xff44_5566);
        let v = b.backing(key, &g2).unwrap();
        assert_eq!((v.w, v.h, v.stride, v.generation), (6, 2, 24, 2));
        assert_eq!(px(&v, 0, 0), 0xdead_beef);
        assert_eq!(
            px(&v, 3, 1),
            0xff11_2233,
            "the overlap keeps the old pixels"
        );
        assert_eq!(px(&v, 4, 0), 0, "past the old width");
        assert_eq!(px(&v, 5, 1), 0);

        let mut fresh = g2;
        fresh.flags = NHG_GEOM_FRESH;
        let v = b.backing(key, &fresh).unwrap();
        assert_eq!(v.generation, 3);
        assert!((0..2).all(|y| (0..6).all(|x| px(&v, x, y) == 0xff44_5566)));
        // the layer is the backing's size: the resize changed it
        assert!(b.ensure(&mut key, &g2));
        assert!(!b.ensure(&mut key, &g2));
        b.forget(key);
    }

    /// Damage gets the border's offset; the whole frame is damaged on the layer's creation.
    #[test]
    fn damage_is_offset_by_the_border() {
        let mut b = Backings::default();
        let mut key = 0;
        b.ensure(&mut key, &geom(20, 10, 2, 3, 0));
        committed(key);
        let r = [
            NhgRect {
                x: 1,
                y: 1,
                w: 4,
                h: 2,
            },
            NhgRect {
                x: 9,
                y: 9,
                w: 0,
                h: 3,
            },
        ];
        assert_eq!(b.damage(key, &r), Ok(1));
        assert_eq!(
            committed(key),
            [NhgRect {
                x: 3,
                y: 4,
                w: 4,
                h: 2
            }]
        );
        b.damage_all(key).unwrap();
        assert_eq!(
            bbox(&committed(key)),
            NhgRect {
                x: 0,
                y: 0,
                w: 24,
                h: 13
            }
        );
        assert_eq!(b.damage(key + 1_000_000, &r), Err(NHG_E_STATE));
        b.forget(key);
    }

    /// The run's rows move inside the window's columns only, and the moved area is damaged.
    #[test]
    fn scroll_moves_rows_and_damages_the_area() {
        let mut b = Backings::default();
        let mut key = 0;
        // backing 10 x 8: native 8 x 6 at (1, 2)
        let g = geom(8, 6, 1, 2, 0);
        b.ensure(&mut key, &g);
        let v = b.backing(key, &g).unwrap();
        for y in 0..8 {
            for x in 0..10 {
                put(&v, x, y, (y * 100 + x) as u32);
            }
        }
        committed(key);
        // native rows 3..5 move up to row 1 (backing rows 5..7 to 3..5), columns 0..8 (backing 1..9)
        let run = NhgRect {
            x: 0,
            y: 3,
            w: 8,
            h: 2,
        };
        b.scroll(key, 7, run, 1).unwrap();
        assert_eq!(px(&v, 1, 3), 501);
        assert_eq!(px(&v, 8, 4), 608);
        assert_eq!(px(&v, 0, 3), 300, "left of the window");
        assert_eq!(px(&v, 9, 4), 409, "right of the window");
        assert_eq!(px(&v, 1, 5), 501, "the source rows stay until redrawn");
        assert_eq!(
            bbox(&committed(key)),
            NhgRect {
                x: 1,
                y: 3,
                w: 8,
                h: 4
            }
        );
        // down: rows copied bottom first, so no source row is overwritten before it is read
        b.scroll(
            key,
            7,
            NhgRect {
                x: 0,
                y: 0,
                w: 8,
                h: 3,
            },
            1,
        )
        .unwrap();
        assert_eq!((px(&v, 1, 3), px(&v, 1, 4), px(&v, 1, 5)), (201, 501, 601));
        // past the backing's bottom the copy is clipped
        b.scroll(
            key,
            7,
            NhgRect {
                x: 0,
                y: 4,
                w: 8,
                h: 5,
            },
            5,
        )
        .unwrap();
        assert_eq!(px(&v, 1, 7), 601, "one row fits");
        b.forget(key);
        assert_eq!(b.scroll(key, 7, run, 1), Err(NHG_E_STATE));
    }

    /// The layer's origin in the backing is what publishing names as the source: a child frame's native area inside
    /// its shadow margin, else the backing's top-left.
    #[test]
    fn published_view_starts_at_the_layer() {
        let mut b = Backings::default();
        let mut key = 0;
        let mut g = geom(30, 20, 4, 4, 9);
        g.b = 4;
        g.layer = NhgRect {
            x: 4,
            y: 4,
            w: 30,
            h: 20,
        };
        b.ensure(&mut key, &g);
        assert!(
            b.frame(key).unwrap().published().is_none(),
            "no backing yet"
        );
        let v = b.backing(key, &g).unwrap();
        let p = b.frame(key).unwrap().published().unwrap();
        assert_eq!((p.w, p.h, p.src), (38, 28, (4, 4)));
        assert_eq!(
            (p.pixels.as_ptr(), p.pixels.len()),
            (v.pixels.cast_const(), 38 * 28 * 4)
        );
        b.forget(key);
    }

    /// A snapshot borrows the backing: the same pixels, size and generation, no copy.
    #[test]
    fn snapshot_is_a_borrowed_view() {
        let mut b = Backings::default();
        let mut key = 0;
        let g = geom(3, 2, 0, 0, 7);
        b.ensure(&mut key, &g);
        assert_eq!(b.snapshot(key).unwrap_err(), NHG_E_STATE);
        let v = b.backing(key, &g).unwrap();
        let s = b.snapshot(key).unwrap();
        assert_eq!(
            (s.pixels, s.stride, s.w, s.h, s.generation),
            (v.pixels, v.stride, v.w, v.h, v.generation)
        );
        b.forget(key);
        assert!(b.frame(key).is_none());
        assert_eq!(b.snapshot(key).unwrap_err(), NHG_E_STATE);
    }

    /// The entry points refuse a null display or null out-parameters.
    #[test]
    fn null_arguments_are_refused() {
        let d = std::ptr::null_mut();
        let mut key = 0u64;
        let g = geom(1, 1, 0, 0, 0);
        // SAFETY: null is allowed for each and reported.
        unsafe {
            assert_eq!(nhg_layer_ensure(d, &mut key, &g), NHG_E_ARG);
            assert_eq!(nhg_damage(d, 1, std::ptr::null(), 0), NHG_E_ARG);
            assert_eq!(
                nhg_scroll_window(d, 1, 1, 1, NhgRect::default(), NhgRect::default(), 0, 0),
                NHG_E_ARG
            );
            assert_eq!(nhg_backing_forget(d, 0), NHG_E_ARG);
        }
    }
}

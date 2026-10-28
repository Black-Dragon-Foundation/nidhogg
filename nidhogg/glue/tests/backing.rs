//! Backings, damage and scroll, against Glitnir itself (no display needed).

mod backing {
    use glitnir::geom::Rect;
    use nhglue::abi::{NHG_E_ARG, NHG_E_STATE};
    use nhglue::backing::{
        Backings, NHG_GEOM_FRESH, NhgBacking, NhgFrameGeom, NhgRect, PENDING_MAX, alpha_multiply,
    };
    use nhglue::ffi::{nhg_backing_forget, nhg_commit, nhg_damage, nhg_layer_ensure, nhg_scroll};

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

    /// Committed damage is clipped to the layer; the 33rd rectangle collapses the 32 before it to their bounding box.
    #[test]
    fn pending_rects_collapse_to_a_bounding_box_past_32() {
        let mut b = Backings::default();
        let mut key = 0;
        b.ensure(&mut key, &geom(100, 100, 0, 0, 0));
        let ones: Vec<Rect> = (0..PENDING_MAX as i32)
            .map(|i| Rect {
                x: 2 * i,
                y: 0,
                w: 1,
                h: 1,
            })
            .collect();
        assert_eq!(b.take(key, &ones), Ok(32));
        assert_eq!(b.frame(key).unwrap().pending().len(), 32);
        let more = [
            Rect {
                x: 0,
                y: 50,
                w: 1,
                h: 1,
            },
            Rect {
                x: -5,
                y: -5,
                w: 10,
                h: 10,
            },
            Rect {
                x: 200,
                y: 0,
                w: 5,
                h: 5,
            },
        ];
        assert_eq!(
            b.take(key, &more),
            Ok(2),
            "the one outside the layer is dropped"
        );
        let p = b.frame(key).unwrap().pending().to_vec();
        assert_eq!(
            p,
            [
                NhgRect {
                    x: 0,
                    y: 0,
                    w: 63,
                    h: 1
                },
                NhgRect {
                    x: 0,
                    y: 50,
                    w: 1,
                    h: 1
                },
                NhgRect {
                    x: 0,
                    y: 0,
                    w: 5,
                    h: 5
                },
            ]
        );
        assert_eq!(b.pending(key).damage_px, 32 + 1 + 25);
        b.presented(key).unwrap();
        assert_eq!((b.pending(key).n, b.pending(key).damage_px), (0, 0));
        // a changed layer drops the pending damage
        b.take(key, &ones[..3]).unwrap();
        assert!(b.ensure(&mut key, &geom(90, 100, 0, 0, 0)));
        assert_eq!(b.pending(key).n, 0);
        b.forget(key);
    }

    /// Damage gets the border's offset; the whole frame is damaged on the layer's creation.
    #[test]
    fn damage_is_offset_by_the_border() {
        let mut b = Backings::default();
        let mut key = 0;
        b.ensure(&mut key, &geom(20, 10, 2, 3, 0));
        b.commit(key).unwrap();
        b.presented(key).unwrap();
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
        assert_eq!(b.commit(key), Ok(1));
        let p = b.frame(key).unwrap().pending().to_vec();
        assert_eq!(
            p,
            [NhgRect {
                x: 3,
                y: 4,
                w: 4,
                h: 2
            }]
        );
        b.damage_all(key).unwrap();
        b.commit(key).unwrap();
        assert_eq!(
            bbox(b.frame(key).unwrap().pending()),
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
        b.commit(key).unwrap();
        b.presented(key).unwrap();
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
        b.commit(key).unwrap();
        assert_eq!(
            bbox(b.frame(key).unwrap().pending()),
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

    /// The host's alpha multiplies each channel once, by 1 - cut / u32::MAX in 255ths.
    #[test]
    fn alpha_host_multiplies_once() {
        let mut row = [0xff80_4020, 0x0000_0000, 0xffff_ffff];
        alpha_multiply(&mut row, u32::MAX / 2);
        assert_eq!(row, [0x8040_2010, 0, 0x8080_8080]);
        let mut row = [0xff80_4020];
        alpha_multiply(&mut row, 0);
        assert_eq!(row, [0xff80_4020], "opaque leaves the pixels");
        alpha_multiply(&mut row, u32::MAX);
        assert_eq!(row, [0]);
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
            assert_eq!(nhg_commit(d, 1), NHG_E_ARG);
            assert_eq!(nhg_scroll(d, 1, 1, NhgRect::default(), 0), NHG_E_ARG);
            assert_eq!(nhg_backing_forget(d, 0), NHG_E_ARG);
        }
    }
}

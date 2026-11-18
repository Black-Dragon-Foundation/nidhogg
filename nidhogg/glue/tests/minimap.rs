//! Minimaps: minimap layers from C's list and the text's line model, against Glitnir itself (no display needed).

mod minimap {
    use glitnir::committed::Committed;
    use glitnir::damage::commit_snapshot;
    use glitnir::geom::Rect;
    use glitnir::hlidskjalf::Fold;
    use glitnir::hlidskjalf::abi::{hlid_build, hlid_drop, with_model};
    use glitnir::scene::Placed;
    use nhglue::backing::MINIMAP_TAG;
    use nhglue::backing::{Backings, NhgFrameGeom, NhgRect};
    use nhglue::edges::{NHG_EDGE_POINTS, NhgEdge, NhgPoint, edge_id};
    use nhglue::minimap::{
        CLOSE_DELAY_NS, DWELL_NS, NHG_MINIMAP_CLICK, NHG_MINIMAP_DRAG, NHG_MINIMAP_DWELL,
        NHG_MINIMAP_LEFT, NHG_MINIMAP_NESTING, NHG_MINIMAP_OPEN, NHG_MINIMAP_REDUCED_MOTION,
        NhgMinimap, OPEN_DELAY_NS, REVEAL_NS, Rows, STEP_NS, count, images_at, minimap_id, of,
        place, rows_of, sync_at, sync_at_time,
    };
    use std::sync::Arc;

    const STRIP: i32 = 20;
    const SEQ: u64 = 9;

    fn frame(b: &mut Backings, s120: u32) -> u64 {
        let mut g = NhgFrameGeom {
            native_w: 400,
            native_h: 300,
            t: STRIP,
            bg_argb: 0xff00_0000,
            scale120: s120,
            ..Default::default()
        };
        let (w, h) = g.size();
        g.layer = NhgRect { x: 0, y: 0, w, h };
        let mut key = 0;
        b.ensure(&mut key, &g);
        b.backing(key, &g).unwrap();
        commit(key);
        key
    }

    fn commit(key: u64) -> (Vec<Rect>, Arc<Committed>) {
        commit_snapshot(key).expect("a scene")
    }

    fn placed(c: &Committed, id: u64) -> Option<Placed> {
        c.layers.iter().find(|p| p.id == id).cloned()
    }

    /// A model under KEY of LINES lines, line i indented i % 8 columns and i % 8 + 10 long.
    fn model(key: u64, lines: usize) -> Vec<u8> {
        let text: String = (0..lines)
            .map(|i| format!("{}{}\n", " ".repeat(i % 8), "x".repeat(10)))
            .collect();
        let t = text.into_bytes();
        // SAFETY: T is readable for its length; the second part is empty.
        let rc = unsafe { hlid_build(key, 0, 8, t.as_ptr(), t.len(), std::ptr::null(), 0, 0) };
        assert_eq!(rc, 0);
        t
    }

    const CLIP: NhgRect = NhgRect {
        x: 0,
        y: 10,
        w: 300,
        h: 200,
    };

    fn entry(key: u64, start: u64, end: u64, zv: u64, flags: u32) -> NhgMinimap {
        NhgMinimap {
            seq: SEQ,
            clip: CLIP,
            key,
            start,
            end,
            begv: 1,
            zv,
            flags,
            width: 100,
            scale: 2,
            fill_rgb: 0x10_2030,
            fill_opacity: 1.0,
            text_rgb: 0xff_ffff,
            text_opacity: 1.0,
            band_rgb: 0x80_8080,
            band_opacity: 0.25,
            // the ruler's strip in the fringe, right of the text area
            strip: NhgRect {
                x: 302,
                y: 10,
                w: 12,
                h: 200,
            },
            ..NhgMinimap::default()
        }
    }

    /// On a shell that shows window images on layers of its own (Naglfar), the minimap is pieces: the first sync
    /// gives each with its image, an unchanged one gives nothing, and a scroll moves the pieces, uploading no more
    /// than the tile that came into view (design §6 "Naglfar", review 6).
    #[test]
    fn on_layers_a_scroll_moves_pieces_and_uploads_only_what_came_into_view() {
        let mut b = Backings::default();
        let key = frame(&mut b, 120);
        let text = 0x6d6d_0042;
        let t = model(text, 2000);
        let zv = t.len() as u64 + 1;
        let pos = |l: u64| 1 + (0..l).map(|i| 11 + i % 8).sum::<u64>();
        let open = NHG_MINIMAP_OPEN | NHG_MINIMAP_REDUCED_MOTION;
        let at = |l: u64| entry(text, pos(l), pos(l + 50), zv, open);
        let first = images_at(&mut b, key, &[at(600)], 1)
            .unwrap()
            .expect("pieces");
        // the surface, the tiles in view, the band; each with its image, each a minimap piece of this window
        assert!(first.len() >= 3, "{first:?}");
        assert!(first.iter().all(|c| c.argb.is_some()));
        assert!(
            first
                .iter()
                .all(|c| c.id & MINIMAP_TAG != 0 && c.id & 0xff_ffff_ffff == SEQ)
        );
        let sent: Vec<_> = first
            .iter()
            .map(|c| nhshell::api::CardImage {
                argb: None,
                ..c.clone()
            })
            .collect();
        of(&mut b, key).unwrap().sent = sent;
        assert!(
            images_at(&mut b, key, &[at(600)], 2).unwrap().is_none(),
            "nothing changed"
        );
        // a scroll by 40 lines: the band and tiles move; at most one tile is new
        let second = images_at(&mut b, key, &[at(640)], 3)
            .unwrap()
            .expect("moved");
        let uploads: Vec<_> = second.iter().filter(|c| c.argb.is_some()).collect();
        assert!(uploads.len() <= 1, "{uploads:?}");
        assert!(
            uploads.iter().all(|c| c.rect.3 <= 512),
            "a tile at most, never the whole minimap: {uploads:?}"
        );
        // the pieces kept their ids
        let kept = second
            .iter()
            .filter(|c| first.iter().any(|f| f.id == c.id))
            .count();
        assert!(kept >= second.len() - 1);
        // each piece shows only inside the minimap
        let mm = first[0].rect;
        for c in &second {
            let (x, y, w, h) = c.shown.unwrap_or(c.rect);
            assert!(x >= mm.0 && y >= mm.1, "{c:?} inside {mm:?}");
            assert!(i64::from(x) + i64::from(w) <= i64::from(mm.0) + i64::from(mm.2));
            assert!(i64::from(y) + i64::from(h) <= i64::from(mm.1) + i64::from(mm.3));
        }
        hlid_drop(text);
    }

    /// Each line is a row; the window's start and end give the band's rows; BEGV's line is row 0.
    #[test]
    fn the_rows_and_the_band_come_from_the_line_model() {
        let key = 0x6d6d_0001;
        model(key, 50);
        // each line is 11 + indent characters; line 10 starts after lines 0–9
        let pos = |l: u64| 1 + (0..l).map(|i| 11 + i % 8).sum::<u64>();
        let r = rows_of(key, pos(10), pos(20), 1, pos(50)).expect("built");
        assert_eq!(r.first, 0);
        // 50 lines and the empty one after the last newline
        assert_eq!(r.total, 51);
        assert_eq!(r.shown, (10, 20));
        // narrowed from line 5: rows count from there
        let r = rows_of(key, pos(10), pos(20), pos(5), pos(50)).unwrap();
        assert_eq!((r.first, r.total, r.shown), (5, 46, (5, 15)));
        hlid_drop(key);
        assert!(rows_of(key, 1, 2, 1, 2).is_none());
    }

    /// Short files: no scroll, the band at its rows.  Tall ones: the band keeps the window's fraction, at the top
    /// for the first row and at the bottom for the last.
    #[test]
    fn a_tall_file_scrolls_the_minimap_in_proportion() {
        let clip = Rect::new(0, 0, 300, 200);
        let rows = |total, shown| Rows {
            first: 0,
            total,
            shown,
            generation: 0,
        };
        let p = place(clip, 100, false, 2, &rows(50, (10, 20)));
        assert_eq!(p.rect, Rect::new(200, 0, 100, 200));
        assert_eq!((p.offset, p.band), (0, Rect::new(200, 20, 100, 20)));
        assert_eq!(place(clip, 100, true, 2, &rows(50, (0, 5))).rect.x, 0);
        // 1000 rows of 2 px in 200 px: 1800 px to scroll over 950 starting rows
        let p = place(clip, 100, false, 2, &rows(1000, (0, 50)));
        assert_eq!((p.offset, p.band.y), (0, 0));
        let p = place(clip, 100, false, 2, &rows(1000, (950, 1000)));
        assert_eq!((p.offset, p.band.y + p.band.h), (1800, 200));
        let p = place(clip, 100, false, 2, &rows(1000, (475, 525)));
        assert_eq!(p.offset, 900);
        assert_eq!(p.band.y, 950 - 900);
    }

    /// An open minimap gets a layer at the text area's outer edge, scaled, with its tiles and band, above an edge
    /// layer; closed, it keeps the layer with nothing revealed; gone from the list, the layer goes.
    #[test]
    fn a_minimap_gets_its_layer_with_tiles_and_goes_on_top() {
        let key = 0x6d6d_0002;
        model(key, 400);
        let mut b = Backings::default();
        let f = frame(&mut b, 150);
        let mut p = [NhgPoint::default(); NHG_EDGE_POINTS];
        p[1] = NhgPoint { x: 50, y: 50 };
        let edge = NhgEdge {
            seq: SEQ,
            clip: CLIP,
            points: p,
            npoints: 2,
            rgb: 0xff_0000,
            opacity: 1.0,
            ..Default::default()
        };
        nhglue::edges::sync_at(&mut b, f, &[edge]).unwrap();
        let zv = with_model(key, |m| m.index().unwrap().chars() + 1).unwrap();
        assert_eq!(
            sync_at(&mut b, f, &[entry(key, 1, 200, zv, NHG_MINIMAP_OPEN)]),
            Ok(1)
        );
        let (_, c) = commit(f);
        let l = placed(&c, minimap_id(SEQ)).expect("the minimap layer");
        // 100 logical px at 1.25: 125, at the right edge of the text area (moved down by the title strip)
        assert_eq!(l.rect, Rect::new(300 - 125, 10 + STRIP, 125, 200));
        let m = l.minimap.expect("a minimap");
        assert_eq!(m.rect, l.rect);
        assert_eq!(m.reveal_px, 125);
        // 401 rows of 3 px (2 logical) in 200: two tiles of 256 rows, the first at the top
        assert_eq!(m.tiles.len(), 1);
        assert_eq!((m.tiles[0].y, m.tiles[0].h), (10 + STRIP, 256 * 3));
        // row 1 (indent 1, 11 long): a bar from column 1 to 11, one px a column at this scale
        let px = &m.tiles[0].px;
        let at = |x: usize, y: usize| px[y * 125 + x];
        assert_eq!(at(0, 3), 0);
        assert_eq!(at(1, 3), 0xffff_ffff);
        assert_eq!(at(10, 5), 0xffff_ffff);
        assert_eq!(at(11, 3), 0);
        assert_eq!(m.bands.len(), 1);
        let ids: Vec<u64> = c.layers.iter().map(|p| p.id).collect();
        let at_of = |id| ids.iter().position(|x| *x == id).unwrap();
        assert!(at_of(minimap_id(SEQ)) > at_of(edge_id(SEQ)), "{ids:?}");
        assert_eq!(count(&mut b, f), 1);

        // the same list again: nothing changes
        sync_at(&mut b, f, &[entry(key, 1, 200, zv, NHG_MINIMAP_OPEN)]).unwrap();
        assert!(commit(f).0.is_empty());

        // closed: the layer stays, nothing revealed or rasterised
        sync_at(&mut b, f, &[entry(key, 1, 200, zv, 0)]).unwrap();
        let m = placed(&commit(f).1, minimap_id(SEQ))
            .unwrap()
            .minimap
            .unwrap();
        assert_eq!((m.reveal_px, m.tiles.len()), (0, 0));

        // left: at the text area's left edge
        sync_at(
            &mut b,
            f,
            &[entry(key, 1, 200, zv, NHG_MINIMAP_OPEN | NHG_MINIMAP_LEFT)],
        )
        .unwrap();
        assert_eq!(placed(&commit(f).1, minimap_id(SEQ)).unwrap().rect.x, 0);

        assert_eq!(sync_at(&mut b, f, &[]), Ok(0));
        assert!(placed(&commit(f).1, minimap_id(SEQ)).is_none());
        assert_eq!(count(&mut b, f), 0);
        hlid_drop(key);
    }

    /// A line's class runs are drawn over its bar in their class's colour, a class with none in the bars' colour, and
    /// a tab-indented line's runs shifted to its indent's columns.
    #[test]
    fn class_runs_draw_in_their_classes_colours() {
        use glitnir::hlidskjalf::Span;
        let key = 0x6d6d_0008;
        let t = b"x = 1\n\tif\n".to_vec();
        // SAFETY: T is readable for its length; the second part is empty.
        let rc = unsafe { hlid_build(key, 0, 4, t.as_ptr(), t.len(), std::ptr::null(), 0, 0) };
        assert_eq!(rc, 0);
        // `x` a variable (7, no colour of its own), `1` a constant (6), `if` (after a tab: columns 4–5) a keyword (1)
        let span = |beg, end, class| Span { beg, end, class };
        assert!(
            with_model(key, |m| m.replace_classes(
                1,
                11,
                &[span(1, 2, 7), span(5, 6, 6), span(8, 10, 1)]
            ))
            .unwrap()
        );
        let mut b = Backings::default();
        let f = frame(&mut b, 120);
        let mut e = entry(key, 1, 11, 11, NHG_MINIMAP_OPEN);
        e.class_rgb[1] = 0xff_0000;
        e.class_rgb[6] = 0x00_ff00;
        sync_at(&mut b, f, &[e]).unwrap();
        let m = placed(&commit(f).1, minimap_id(SEQ))
            .unwrap()
            .minimap
            .unwrap();
        let px = &m.tiles[0].px;
        // rows of 2 px, 100 px wide
        let at = |x: usize, y: usize| px[y * 100 + x];
        let row0: Vec<u32> = (0..6).map(|x| at(x, 0)).collect();
        let (white, green, red) = (0xffff_ffff, 0xff00_ff00, 0xffff_0000);
        assert_eq!(row0, [white, white, white, white, green, 0]);
        let row1: Vec<u32> = (0..7).map(|x| at(x, 2)).collect();
        assert_eq!(row1, [0, 0, 0, 0, red, red, 0]);
        hlid_drop(key);
    }

    /// A tile is kept while the model does not change and drawn again after an edit; a window whose model has no
    /// lines has no layer.
    #[test]
    fn tiles_are_kept_until_the_model_changes() {
        let key = 0x6d6d_0003;
        let text = model(key, 30);
        let mut b = Backings::default();
        let f = frame(&mut b, 120);
        let zv = text.len() as u64 + 1;
        let e = [entry(key, 1, 100, zv, NHG_MINIMAP_OPEN)];
        sync_at(&mut b, f, &e).unwrap();
        let tile = |b: &mut Backings| {
            sync_at(b, f, &e).unwrap();
            let m = placed(&commit(f).1, minimap_id(SEQ))
                .unwrap()
                .minimap
                .unwrap();
            Arc::clone(&m.tiles[0].px)
        };
        let t0 = tile(&mut b);
        assert!(Arc::ptr_eq(&t0, &tile(&mut b)));
        // line 0 grows to 30 characters
        with_model(key, |m| {
            let seg = format!("{}\n", "x".repeat(30));
            m.edit(1, 1, 21, &[seg.as_bytes()]).unwrap();
        });
        let t1 = tile(&mut b);
        assert!(!Arc::ptr_eq(&t0, &t1));
        assert_eq!(t1[100 + 25], 0xffff_ffff);
        assert_eq!(t0[100 + 25], 0);
        assert_eq!(sync_at(&mut b, f, &[entry(0x6d6d_00ff, 1, 2, 2, 1)]), Ok(0));
        assert!(placed(&commit(f).1, minimap_id(SEQ)).is_none());
        hlid_drop(key);
    }

    const MS: u64 = 1_000_000;

    /// The pointer resting on the strip opens the minimap after 150 ms, revealed over 120 ms on the frame clock;
    /// inside it the pointer is the minimap's (Emacs gets one leave), a rest of 300 ms is a dwell on the row, a
    /// press a click and its moves drag steps (each the start that centres the row); 600 ms after the pointer left it
    /// closes the same way.
    #[test]
    fn the_pointer_opens_reveals_uses_and_closes_the_minimap() {
        let key = 0x6d6d_0004;
        let text = model(key, 400);
        let zv = text.len() as u64 + 1;
        let mut b = Backings::default();
        let f = frame(&mut b, 120);
        let pos = |line| with_model(key, |m| m.pos_of(line)).flatten().unwrap();
        let e = [entry(key, 1, 200, zv, 0)];
        let reveal = |b: &mut Backings, t: u64| {
            sync_at_time(b, f, &e, t).unwrap();
            placed(&commit(f).1, minimap_id(SEQ))
                .unwrap()
                .minimap
                .unwrap()
                .reveal_px
        };
        let t0 = 1_000 * MS;
        assert_eq!(reveal(&mut b, t0), 0);
        // on the strip (the native area's y is the layer's plus the title strip)
        let y = |v: i32| f64::from(v + STRIP);
        let r = of(&mut b, f).unwrap().motion(Some((305.0, y(50))), t0);
        assert!(!r.swallow && !r.publish);
        assert_eq!(of(&mut b, f).unwrap().deadline(), Some(t0 + OPEN_DELAY_NS));
        assert!(!of(&mut b, f).unwrap().tick(t0 + OPEN_DELAY_NS - 1).publish);
        let t1 = t0 + OPEN_DELAY_NS;
        assert!(of(&mut b, f).unwrap().tick(t1).publish);
        // the reveal: from nothing, part way, then the whole width; while it runs the frame is published again
        assert_eq!(reveal(&mut b, t1), 0);
        assert!(of(&mut b, f).unwrap().animating());
        // its steps come on the shell's timer as well as on frame callbacks (a publish without a redisplay may bring
        // none): due a step later, a publish
        assert_eq!(of(&mut b, f).unwrap().next_at(t1), Some(t1 + STEP_NS));
        assert!(of(&mut b, f).unwrap().tick(t1 + STEP_NS).publish);
        let mid = reveal(&mut b, t1 + REVEAL_NS / 2);
        assert!(0 < mid && mid < 100, "{mid}");
        assert_eq!(reveal(&mut b, t1 + REVEAL_NS), 100);
        assert!(!of(&mut b, f).unwrap().animating());
        assert_eq!(of(&mut b, f).unwrap().next_at(t1 + REVEAL_NS), None);

        // into the minimap: the pointer is its own, with one leave for Emacs
        let t2 = t1 + 200 * MS;
        let r = of(&mut b, f).unwrap().motion(Some((250.0, y(50))), t2);
        assert!(r.swallow && r.leave);
        let r = of(&mut b, f).unwrap().motion(Some((250.0, y(51))), t2 + MS);
        assert!(r.swallow && !r.leave);
        // row (50 - 10) / 2 = 20: a dwell there after 300 ms
        let r = of(&mut b, f).unwrap().tick(t2 + DWELL_NS);
        assert_eq!(r.events, [(NHG_MINIMAP_DWELL, SEQ, pos(20))]);
        // a press: the start that centres row 20 (the window shows rows 0 to 16); a move: a drag step to row 70
        let shown = rows_of(key, 1, 200, 1, zv).unwrap().shown.1;
        let r = of(&mut b, f)
            .unwrap()
            .button(250.0, y(50), true, true, false, t2 + 400 * MS);
        assert!(r.swallow);
        assert_eq!(r.events, [(NHG_MINIMAP_CLICK, SEQ, pos(20 - shown / 2))]);
        let r = of(&mut b, f)
            .unwrap()
            .motion(Some((250.0, y(150))), t2 + 410 * MS);
        assert_eq!(r.events, [(NHG_MINIMAP_DRAG, SEQ, pos(70 - shown / 2))]);
        let r = of(&mut b, f)
            .unwrap()
            .button(250.0, y(150), true, false, false, t2 + 420 * MS);
        assert!(r.swallow && r.events.is_empty());

        // out over the text: Emacs has the pointer again; 600 ms later the minimap closes, hidden over 120 ms
        let t3 = t2 + 500 * MS;
        let r = of(&mut b, f).unwrap().motion(Some((50.0, y(50))), t3);
        assert!(!r.swallow);
        assert!(!of(&mut b, f).unwrap().tick(t3 + CLOSE_DELAY_NS - 1).publish);
        assert!(of(&mut b, f).unwrap().tick(t3 + CLOSE_DELAY_NS).publish);
        let t4 = t3 + CLOSE_DELAY_NS;
        assert_eq!(reveal(&mut b, t4), 100);
        assert_eq!(reveal(&mut b, t4 + REVEAL_NS), 0);
        hlid_drop(key);
    }

    /// With reduced motion the pointer's open is final in its first frame; back before the close, it stays open.
    #[test]
    fn reduced_motion_reveals_at_once_and_a_return_keeps_it_open() {
        let key = 0x6d6d_0005;
        let text = model(key, 100);
        let zv = text.len() as u64 + 1;
        let mut b = Backings::default();
        let f = frame(&mut b, 120);
        let e = [entry(key, 1, 100, zv, NHG_MINIMAP_REDUCED_MOTION)];
        let reveal = |b: &mut Backings, t: u64| {
            sync_at_time(b, f, &e, t).unwrap();
            placed(&commit(f).1, minimap_id(SEQ))
                .unwrap()
                .minimap
                .unwrap()
                .reveal_px
        };
        let t0 = 1_000 * MS;
        reveal(&mut b, t0);
        of(&mut b, f)
            .unwrap()
            .motion(Some((305.0, f64::from(50 + STRIP))), t0);
        assert!(of(&mut b, f).unwrap().tick(t0 + OPEN_DELAY_NS).publish);
        assert_eq!(reveal(&mut b, t0 + OPEN_DELAY_NS), 100);
        assert!(!of(&mut b, f).unwrap().animating());
        // out, and back in before 600 ms: no close
        let t1 = t0 + OPEN_DELAY_NS + MS;
        of(&mut b, f)
            .unwrap()
            .motion(Some((50.0, f64::from(50 + STRIP))), t1);
        of(&mut b, f)
            .unwrap()
            .motion(Some((250.0, f64::from(50 + STRIP))), t1 + 300 * MS);
        assert!(!of(&mut b, f).unwrap().tick(t1 + CLOSE_DELAY_NS).publish);
        assert_eq!(reveal(&mut b, t1 + CLOSE_DELAY_NS), 100);
        hlid_drop(key);
    }

    /// Nesting shades: a fold per depth is a faint band behind the bars from its first row to its last, inset 4 px a
    /// depth (the fifth depth none); a second window on the same text has its band outlined.
    #[test]
    fn nesting_shades_and_the_other_windows_band() {
        use glitnir::hlidskjalf::Fold;
        let key = 0x6d6d_0006;
        let text = model(key, 100);
        let zv = text.len() as u64 + 1;
        let pos = |line| with_model(key, |m| m.pos_of(line)).flatten().unwrap();
        let fold = |a, z, depth| Fold {
            beg: pos(a),
            end: pos(z),
            kind: 0,
            depth,
            source: 0,
        };
        // (the folds' positions first: the model's lock is not reentrant)
        // (the model gives each fold its depth by nesting: the last is the fifth level)
        let folds = [
            fold(10, 30, 0),
            fold(12, 20, 0),
            fold(14, 18, 0),
            fold(15, 17, 0),
            fold(16, 17, 0),
        ];
        with_model(key, |m| m.set_folds(0, &folds, 100));
        let mut b = Backings::default();
        let f = frame(&mut b, 120);
        let mine = entry(key, 1, 200, zv, NHG_MINIMAP_OPEN | NHG_MINIMAP_NESTING);
        let other = NhgMinimap {
            seq: SEQ + 1,
            start: pos(50),
            end: pos(60),
            ..mine
        };
        sync_at(&mut b, f, &[mine, other]).unwrap();
        let m = placed(&commit(f).1, minimap_id(SEQ))
            .unwrap()
            .minimap
            .unwrap();
        // the minimap's top is the text area's (10) under the title strip; rows of 2 px
        let top = 10 + STRIP;
        assert_eq!(m.shades.len(), 4, "{:?}", m.shades);
        assert_eq!(m.shades[0].rect, Rect::new(200, top + 20, 100, 40));
        assert_eq!(m.shades[1].rect, Rect::new(204, top + 24, 96, 16));
        assert_eq!(m.shades[2].rect, Rect::new(208, top + 28, 92, 8));
        assert_eq!(m.shades[3].rect, Rect::new(212, top + 30, 88, 4));
        assert!(
            (10..=12).contains(&(m.shades[0].fill_argb >> 24)),
            "4.5 % of the bars' opacity"
        );
        // its own band filled, the other window's (lines 50 to 59) outlined
        assert_eq!(m.bands.len(), 2);
        assert_eq!(m.bands[1].rect, Rect::new(200, top + 100, 100, 20));
        assert_eq!(m.bands[1].fill_argb, 0);
        assert_ne!(m.bands[1].line_argb, 0);
        hlid_drop(key);
    }

    use nhglue::minimap::{
        NHG_MINIMAP_LABELS_ALL, NHG_MINIMAP_LABELS_MARKS, NhgMinimapHunk, NhgMinimapName,
        nhg_minimap_set_label_render,
    };
    use std::sync::Mutex;

    /// The texts the fake renderer was asked for.
    static RENDERED: Mutex<Vec<String>> = Mutex::new(Vec::new());
    /// Held by each test that sets the (process-wide) label renderer.
    static RENDERER: Mutex<()> = Mutex::new(());

    /// A renderer that fills 5 px a byte (at most the room) by the whole height, in the colour asked.
    unsafe extern "C" fn fake_render(
        _host: *mut std::ffi::c_void,
        _face: i32,
        text: *const u8,
        len: usize,
        _px100: u32,
        rgb: u32,
        buf: *mut u32,
        cap_w: i32,
        cap_h: i32,
        w: *mut i32,
        h: *mut i32,
    ) -> i32 {
        // SAFETY: the glue passes readable text and a writable CAP_W × CAP_H buffer, and writable W and H.
        unsafe {
            let t = std::str::from_utf8(std::slice::from_raw_parts(text, len)).unwrap();
            RENDERED.lock().unwrap().push(t.to_owned());
            let ww = (len as i32 * 5).min(cap_w);
            for y in 0..cap_h {
                for x in 0..ww {
                    *buf.add((y * cap_w + x) as usize) = 0xff00_0000 | rgb;
                }
            }
            *w = ww;
            *h = cap_h;
        }
        0
    }

    /// The hub's fold ranges (Sjá's `folds` layer of the window's file, design §7.4) become the model's source 1 at the
    /// publish, once per layer version; a model its own parser folds (source 0) keeps those alone.
    #[test]
    fn the_hubs_folds_reach_the_model_of_a_buffer_with_no_parser() {
        let key = 0x6d6d_0011;
        let text = "a {\n  b\n}\nc {\n  d\n}\n";
        let t = text.as_bytes();
        // SAFETY: T is readable for its length.
        let rc = unsafe { hlid_build(key, 0, 8, t.as_ptr(), t.len(), std::ptr::null(), 0, 0) };
        assert_eq!(rc, 0);
        let file = "/t/hub-folds-test.xcs";
        let push = |version: u32, items: &str| {
            let line = format!(
                r#"{{"event":"view-model","data":{{"minimap":{{"file":"{file}","layer":"folds","version":{version},"items":{items}}}}}}}"#
            );
            glitnir::sja::minimap::push(glitnir::sja::minimap::read_line(&line).unwrap().unwrap());
        };
        let folds = || {
            with_model(key, |m| {
                m.folds(0, u64::MAX)
                    .map(|f| (f.beg, f.end, f.kind, f.source))
                    .collect::<Vec<_>>()
            })
            .unwrap()
        };
        let mut b = Backings::default();
        let f = frame(&mut b, 120);
        let e = NhgMinimap {
            file: file.as_ptr(),
            file_len: file.len(),
            ..entry(key, 1, 7, t.len() as u64 + 1, NHG_MINIMAP_OPEN)
        };
        push(1, r#"[[1,3,"class"],[4,6,"test"]]"#);
        sync_at(&mut b, f, std::slice::from_ref(&e)).unwrap();
        assert_eq!(folds(), [(1, 10, 2, 1), (11, 20, 5, 1)]);
        // a newer version replaces them; the same one again is not taken twice (a range set meanwhile stays)
        push(2, r#"[[4,6,"function"]]"#);
        sync_at(&mut b, f, std::slice::from_ref(&e)).unwrap();
        assert_eq!(folds(), [(11, 20, 1, 1)]);
        with_model(key, |m| m.set_fold_lines(1, &[(1, 3, 0)]));
        sync_at(&mut b, f, std::slice::from_ref(&e)).unwrap();
        assert_eq!(folds(), [(1, 10, 0, 1)]);
        // the buffer's own parser folds it: a newer hub layer changes nothing
        with_model(key, |m| {
            m.set_folds(
                0,
                &[Fold {
                    beg: 11,
                    end: 20,
                    kind: 1,
                    depth: 0,
                    source: 0,
                }],
                100,
            )
        });
        push(3, r#"[[1,6,"block"]]"#);
        sync_at(&mut b, f, std::slice::from_ref(&e)).unwrap();
        assert_eq!(folds(), [(1, 10, 0, 1), (11, 20, 1, 0)]);
    }

    const MARK_RE: &str = r"^\s*//\s*MARK:\s*(?P<name>.+)$";

    /// Labels (design M1): the hub's definitions from Sjá by the file's name and the model's MARK sections (in
    /// capitals, with a hairline), placed with none overlapping (a deeper one yields), indented by depth, a nested one
    /// in its own colour, the one under the pointer in the accent (the pointer's move onto it publishes); MARK sections
    /// alone with `NHG_MINIMAP_LABELS_MARKS`, none with 0.
    #[test]
    fn labels_come_from_the_hub_and_the_mark_sections() {
        let key = 0x6d6d_0007;
        let text: String = (0..100)
            .map(|i| match i {
                9 => "// MARK: parse\n".to_owned(),
                59 => "// MARK: two\n".to_owned(),
                _ => format!("{}x\n", " ".repeat(i % 4)),
            })
            .collect();
        let t = text.as_bytes();
        let _held = RENDERER.lock().unwrap_or_else(|e| e.into_inner());
        let build = || {
            // SAFETY: T is readable for its length.
            let rc = unsafe { hlid_build(key, 0, 8, t.as_ptr(), t.len(), std::ptr::null(), 0, 0) };
            assert_eq!(rc, 0);
        };
        build();
        with_model(key, |m| m.set_section_regex(Some(MARK_RE)).unwrap());
        build();
        let file = "/t/labels-test.rs";
        let line = format!(
            r#"{{"event":"view-model","data":{{"minimap":{{"file":"{file}","layer":"labels","version":1,"items":[[3,0,"function","main"],[4,1,"method","deep"],[30,1,"method","go"]]}}}}}}"#
        );
        glitnir::sja::minimap::push(glitnir::sja::minimap::read_line(&line).unwrap().unwrap());
        nhg_minimap_set_label_render(Some(fake_render));
        let mut b = Backings::default();
        let f = frame(&mut b, 120);
        let zv = t.len() as u64 + 1;
        let e = NhgMinimap {
            labels: NHG_MINIMAP_LABELS_ALL,
            file: file.as_ptr(),
            file_len: file.len(),
            label_rgb: 0x11_1111,
            sub_rgb: 0x22_2222,
            accent_rgb: 0x33_3333,
            mark_rgb: 0x44_4444,
            ..entry(key, 1, 100, zv, NHG_MINIMAP_OPEN)
        };
        let get = |b: &mut Backings, e: &NhgMinimap| {
            sync_at(b, f, std::slice::from_ref(e)).unwrap();
            placed(&commit(f).1, minimap_id(SEQ))
                .unwrap()
                .minimap
                .unwrap()
        };
        let m = get(&mut b, &e);
        let top = 10 + STRIP;
        let mut got: Vec<(i32, i32, u32)> =
            m.labels.iter().map(|l| (l.y - top, l.x, l.px[0])).collect();
        got.sort();
        // rows of 2 px: main (line 3) at 4, deep (line 4, 2 px under it) yields, PARSE (line 10) at 18, go at 58, TWO
        // at 118; a nested one indented 8 px
        assert_eq!(
            got,
            [
                (4, 212, 0xff11_1111),
                (18, 212, 0xff44_4444),
                (58, 220, 0xff22_2222),
                (118, 212, 0xff44_4444)
            ]
        );
        assert!(
            m.labels
                .iter()
                .all(|l| l.h == 13 && l.back_argb == m.fill_argb)
        );
        let texts = RENDERED.lock().unwrap().clone();
        assert!(
            texts.contains(&"PARSE".to_owned()) && texts.contains(&"TWO".to_owned()),
            "{texts:?}"
        );
        assert!(
            !texts.contains(&"deep".to_owned()),
            "a label not placed is not rendered: {texts:?}"
        );
        // the MARK sections' hairlines
        assert!(
            m.bands
                .iter()
                .any(|b| b.rect == Rect::new(212, top + 18, 78, 1)),
            "{:?}",
            m.bands
        );
        // the pointer onto main: a publish, and main in the accent
        let r = of(&mut b, f)
            .unwrap()
            .motion(Some((250.0, f64::from(top + 6))), 1_000_000_000);
        assert!(r.publish && r.swallow);
        let m = get(&mut b, &e);
        let main = m.labels.iter().find(|l| l.y == top + 4).unwrap();
        assert_eq!(main.px[0], 0xff33_3333);
        // within the same label: no publish
        let r = of(&mut b, f)
            .unwrap()
            .motion(Some((251.0, f64::from(top + 8))), 1_000_000_001);
        assert!(!r.publish);
        // MARK sections only; none
        let m = get(
            &mut b,
            &NhgMinimap {
                labels: NHG_MINIMAP_LABELS_MARKS,
                ..e
            },
        );
        assert_eq!(m.labels.len(), 2);
        let m = get(&mut b, &NhgMinimap { labels: 0, ..e });
        assert!(m.labels.is_empty());
        // Lisp's hunk labels (`hunk-labels-are-placed-as-mark-sections`): at their lines' start in their colour, placed
        // before a definition (the one on line 3 yields; the pointer still rests there, so that one is in the accent),
        // no hairline, a text cut at 256 bytes; none without their colour or with labels off
        let long = "x".repeat(300);
        let hunks = [
            NhgMinimapHunk {
                line: 3,
                text: "hunk 1 · +2 −0".as_ptr(),
                len: "hunk 1 · +2 −0".len(),
            },
            NhgMinimapHunk {
                line: 45,
                text: long.as_ptr(),
                len: long.len(),
            },
        ];
        let h = NhgMinimap {
            hunk_rgb: 0x55_5555,
            hunks: hunks.as_ptr(),
            hunks_len: hunks.len(),
            ..e
        };
        let m = get(&mut b, &h);
        let mut got: Vec<(i32, i32, u32)> =
            m.labels.iter().map(|l| (l.y - top, l.x, l.px[0])).collect();
        got.sort();
        assert_eq!(
            got,
            [
                (4, 212, 0xff33_3333),
                (18, 212, 0xff44_4444),
                (58, 220, 0xff22_2222),
                (88, 212, 0xff55_5555),
                (118, 212, 0xff44_4444)
            ]
        );
        assert!(!m.bands.iter().any(|b| b.rect.y == top + 88));
        let texts = RENDERED.lock().unwrap().clone();
        assert!(
            texts.contains(&"hunk 1 · +2 −0".to_owned()) && texts.contains(&"x".repeat(256)),
            "{texts:?}"
        );
        let m = get(&mut b, &NhgMinimap { hunk_rgb: -1, ..h });
        assert_eq!(m.labels.len(), 4);
        let m = get(&mut b, &NhgMinimap { labels: 0, ..h });
        assert!(m.labels.is_empty());
        nhg_minimap_set_label_render(None);
        glitnir::sja::minimap::forget(file);
        hlid_drop(key);
    }

    /// Fold pills (design M5, `fold-pills-show-the-hidden-lines-or-the-name`): each hidden range gets a dashed pill in
    /// `pill_rgb` after its first line's bar, the lines it hides through the format, a named range its name; with
    /// labels off too; none without the colour.
    #[test]
    fn fold_pills_show_the_hidden_lines_or_the_name() {
        let _held = RENDERER.lock().unwrap_or_else(|e| e.into_inner());
        let key = 0x6d6d_0021;
        let text = model(key, 100);
        let zv = text.len() as u64 + 1;
        let pos = |line| with_model(key, |m| m.pos_of(line)).flatten().unwrap();
        // lines 21–25 hidden after line 20, lines 61–63 after line 60 as the fold "parse"
        let (a, z, c, d) = (pos(21) - 1, pos(26) - 1, pos(61) - 1, pos(64) - 1);
        with_model(key, |m| {
            m.set_hidden_named(&[(a, z, None), (c, d, Some("parse"))])
        });
        nhg_minimap_set_label_render(Some(fake_render));
        let mut b = Backings::default();
        let f = frame(&mut b, 120);
        let e = NhgMinimap {
            pill_rgb: 0x66_6666,
            ..entry(key, 1, 100, zv, NHG_MINIMAP_OPEN)
        };
        let get = |b: &mut Backings, e: &NhgMinimap| {
            sync_at(b, f, std::slice::from_ref(e)).unwrap();
            placed(&commit(f).1, minimap_id(SEQ))
                .unwrap()
                .minimap
                .unwrap()
        };
        RENDERED.lock().unwrap().clear();
        let m = get(&mut b, &e);
        let top = 10 + STRIP;
        let mut got: Vec<(i32, i32, u32)> =
            m.labels.iter().map(|l| (l.y - top, l.x, l.px[0])).collect();
        got.sort();
        // rows of 2 px: line 20 at 40, line 60 five rows up (55) at 110; after the bar (indent 4 + 10 columns) and
        // 4 px; the dashed outline's first pixel in the pill colour
        assert_eq!(got, [(40, 218, 0xff66_6666), (110, 218, 0xff66_6666)]);
        let texts = RENDERED.lock().unwrap().clone();
        assert!(
            texts.contains(&"⋯ 5 folded".to_owned()) && texts.contains(&"parse".to_owned()),
            "{texts:?}"
        );
        // the outline is dashed: the top row has gaps; the text inside, 3 px in
        let l = m.labels.iter().find(|l| l.y == top + 40).unwrap();
        let row0: Vec<u32> = l.px[..l.w as usize].to_vec();
        assert!(row0.contains(&0) && row0.contains(&0xff66_6666));
        assert_eq!(l.px[(6 * l.w + 3) as usize], 0xff66_6666, "the text's fill");
        // a format of Lisp's
        let fmt = "%d hidden";
        let m = get(
            &mut b,
            &NhgMinimap {
                pill_format: fmt.as_ptr(),
                pill_format_len: fmt.len(),
                ..e
            },
        );
        assert_eq!(m.labels.len(), 2);
        assert!(RENDERED.lock().unwrap().contains(&"5 hidden".to_owned()));
        // no colour: no pills
        let m = get(&mut b, &NhgMinimap { pill_rgb: -1, ..e });
        assert!(m.labels.is_empty());
        hlid_drop(key);
    }

    /// Another window on the same text (design M8, `another-windows-band-shows-its-number`): its outlined band shows its
    /// number at the top-right, in the band's colour; none without a number.
    #[test]
    fn another_windows_band_shows_its_number() {
        let _held = RENDERER.lock().unwrap_or_else(|e| e.into_inner());
        let key = 0x6d6d_0022;
        let text = model(key, 100);
        let zv = text.len() as u64 + 1;
        let pos = |line| with_model(key, |m| m.pos_of(line)).flatten().unwrap();
        nhg_minimap_set_label_render(Some(fake_render));
        let mut b = Backings::default();
        let f = frame(&mut b, 120);
        let mine = entry(key, 1, 200, zv, NHG_MINIMAP_OPEN);
        let n = "2";
        let other = NhgMinimap {
            seq: SEQ + 1,
            start: pos(50),
            end: pos(60),
            number: n.as_ptr(),
            number_len: n.len(),
            ..mine
        };
        sync_at(&mut b, f, &[mine, other]).unwrap();
        let m = placed(&commit(f).1, minimap_id(SEQ))
            .unwrap()
            .minimap
            .unwrap();
        let top = 10 + STRIP;
        // the fake renderer draws 5 px a byte: at the right less 10 px of room
        let got: Vec<(i32, i32, u32)> =
            m.labels.iter().map(|l| (l.y - top, l.x, l.px[0])).collect();
        assert_eq!(got, [(100, 285, 0xff80_8080)]);
        // the other window's own minimap: mine has no number
        let o = placed(&commit(f).1, minimap_id(SEQ + 1))
            .map(|p| p.minimap.unwrap().labels.len())
            .unwrap_or(0);
        assert_eq!(o, 0);
        hlid_drop(key);
    }

    /// Fold arrows (design F1, `fold-arrows-mark-the-fold-starts-in-the-fringe`): on each matrix row that starts a fold
    /// range's line, a chevron in the fringe at its inset, down in the open colour, right in the folded colour for a
    /// range whose lines are hidden; in hover mode the open ones only while the pointer is over the fringe (moving on
    /// and off publishes, and is never swallowed); a record for the arrows alone draws no minimap.
    #[test]
    fn fold_arrows_mark_the_fold_starts_in_the_fringe() {
        use nhglue::minimap::{
            NHG_ARROWS_ALWAYS, NHG_ARROWS_HOVER, NHG_MINIMAP_NONE, NhgMinimapRow, arrows_id,
            chevron,
        };
        let key = 0x6d6d_0023;
        let text = model(key, 100);
        let zv = text.len() as u64 + 1;
        let pos = |line| with_model(key, |m| m.pos_of(line)).flatten().unwrap();
        let fold = |a, z| Fold {
            beg: pos(a),
            end: pos(z) - 1,
            kind: 0,
            depth: 0,
            source: 0,
        };
        let folds = [fold(10, 20), fold(30, 40)];
        let hidden = (pos(31) - 1, pos(40) - 1);
        with_model(key, |m| {
            m.set_folds(0, &folds, 100);
            m.set_hidden(&[hidden]);
        });
        let rows = [
            NhgMinimapRow {
                pos: pos(10),
                y: 100,
                h: 16,
            },
            NhgMinimapRow {
                pos: pos(11),
                y: 116,
                h: 16,
            },
            NhgMinimapRow {
                pos: pos(30),
                y: 132,
                h: 16,
            },
        ];
        let mut b = Backings::default();
        let f = frame(&mut b, 120);
        let e = NhgMinimap {
            arrows: NHG_ARROWS_ALWAYS,
            fringe: NhgRect {
                x: 0,
                y: 10,
                w: 16,
                h: 200,
            },
            arrow_size: 10,
            arrow_inset: 2,
            arrow_rgb: 0x11_2233,
            arrow_opacity: 1.0,
            folded_rgb: 0x44_5566,
            folded_opacity: 1.0,
            rows: rows.as_ptr(),
            rows_len: rows.len(),
            ..entry(key, pos(10), pos(31), zv, NHG_MINIMAP_OPEN)
        };
        let get = |b: &mut Backings, e: &NhgMinimap| {
            sync_at(b, f, std::slice::from_ref(e)).unwrap();
            placed(&commit(f).1, arrows_id(SEQ)).map(|p| p.minimap.unwrap())
        };
        let a = get(&mut b, &e).expect("the arrows' layer");
        assert_eq!(a.rect, Rect::new(0, 10 + STRIP, 16, 200));
        let got: Vec<(i32, i32, bool)> = a
            .labels
            .iter()
            .map(|l| {
                let folded = l.px == chevron(10, true, 0xff44_5566);
                assert!(folded || l.px == chevron(10, false, 0xff11_2233));
                (l.x, l.y, folded)
            })
            .collect();
        // inner edge 16 less the inset 2 and the box 10; each centred in its row (moved down by the title strip)
        assert_eq!(got, [(4, 123, false), (4, 155, true)]);
        assert!(chevron(10, true, 1) != chevron(10, false, 1));
        // hover: the folded one alone until the pointer is over the fringe
        let h = NhgMinimap {
            arrows: NHG_ARROWS_HOVER,
            ..e
        };
        let a = get(&mut b, &h).unwrap();
        assert_eq!(a.labels.len(), 1);
        let r = of(&mut b, f).unwrap().motion(Some((8.0, 140.0)), 1);
        assert!(r.publish && !r.swallow, "{r:?}");
        assert_eq!(get(&mut b, &h).unwrap().labels.len(), 2);
        let r = of(&mut b, f).unwrap().motion(Some((100.0, 140.0)), 2);
        assert!(r.publish && !r.swallow);
        assert_eq!(get(&mut b, &h).unwrap().labels.len(), 1);
        // the arrows alone: no minimap layer; gone from the list, the arrows' layer goes
        let alone = NhgMinimap {
            flags: NHG_MINIMAP_NONE,
            ..e
        };
        assert_eq!(get(&mut b, &alone).unwrap().labels.len(), 2);
        assert!(placed(&commit(f).1, minimap_id(SEQ)).is_none());
        sync_at(&mut b, f, &[]).unwrap();
        assert!(placed(&commit(f).1, arrows_id(SEQ)).is_none());
        assert_eq!(count(&mut b, f), 0);
        hlid_drop(key);
    }

    /// Heat (design X3, `a-heat-layer-recolours-the-bars-on-the-ramp`): while Sjá holds the file's `heat:NAME` layer,
    /// each line's bar is one colour mixed between the ramp's ends by its value (items in any order), a line with no
    /// item in the bars' colour; a newer version draws the tiles again; a layer not pushed leaves the bars as they were.
    #[test]
    fn a_heat_layer_recolours_the_bars_on_the_ramp() {
        let key = 0x6d6d_0024;
        let text = model(key, 100);
        let zv = text.len() as u64 + 1;
        let file = "/t/heat-test.rs";
        let push = |version: u32, items: &str| {
            let line = format!(
                r#"{{"event":"view-model","data":{{"minimap":{{"file":"{file}","layer":"heat:blame","version":{version},"items":{items}}}}}}}"#
            );
            glitnir::sja::minimap::push(glitnir::sja::minimap::read_line(&line).unwrap().unwrap());
        };
        push(1, r#"[[30,39,1000,"2 days ago"],[10,19,0],[20,20,500]]"#);
        let mut b = Backings::default();
        let f = frame(&mut b, 120);
        let name = "blame";
        let e = NhgMinimap {
            file: file.as_ptr(),
            file_len: file.len(),
            heat: name.as_ptr(),
            heat_len: name.len(),
            heat_low_rgb: 0x00_00ff,
            heat_low_opacity: 1.0,
            heat_high_rgb: 0xff_0000,
            heat_high_opacity: 1.0,
            ..entry(key, 1, 100, zv, NHG_MINIMAP_OPEN)
        };
        let at = |b: &mut Backings, e: &NhgMinimap, line: usize| {
            sync_at(b, f, std::slice::from_ref(e)).unwrap();
            let m = placed(&commit(f).1, minimap_id(SEQ))
                .unwrap()
                .minimap
                .unwrap();
            // rows of 2 px, 100 px wide; column 8 is inside every line's bar
            m.tiles[0].px[line * 2 * 100 + 8]
        };
        assert_eq!(at(&mut b, &e, 9), 0xff00_00ff, "value 0: the low end");
        assert_eq!(at(&mut b, &e, 19), 0xff80_0080, "500: half way");
        assert_eq!(at(&mut b, &e, 29), 0xffff_0000, "1000: the high end");
        assert_eq!(at(&mut b, &e, 0), 0xffff_ffff, "no item: the bars' colour");
        assert_eq!(at(&mut b, &e, 40), 0xffff_ffff);
        // a newer version: drawn again
        push(2, r#"[[10,10,1000]]"#);
        assert_eq!(at(&mut b, &e, 9), 0xffff_0000);
        assert_eq!(at(&mut b, &e, 29), 0xffff_ffff);
        // another layer, not pushed: the bars as without heat
        let other = "coverage";
        let c = NhgMinimap {
            heat: other.as_ptr(),
            heat_len: other.len(),
            ..e
        };
        assert_eq!(at(&mut b, &c, 9), 0xffff_ffff);
        glitnir::sja::minimap::forget(file);
        hlid_drop(key);
    }

    /// Pinned regions (design X5, `pin-marks-give-a-band-and-a-label`): each mark of the pin kind with an end is a band
    /// over its rows in the pin colour at the band's opacity, the n-th pin by position labelled with the n-th name, with
    /// labels off; marks of another kind or without an end are not pins.
    #[test]
    fn pin_marks_give_a_band_and_a_label() {
        use glitnir::hlidskjalf::Mark;
        let _held = RENDERER.lock().unwrap_or_else(|e| e.into_inner());
        let key = 0x6d6d_0025;
        let text = model(key, 100);
        let zv = text.len() as u64 + 1;
        let pos = |line| with_model(key, |m| m.pos_of(line)).flatten().unwrap();
        let mark = |a: u64, z: u64| Mark {
            pos: a,
            end: z,
            lane: 0,
            class: 0,
            prio: 0,
            kind: 9,
        };
        // (given out of order: the names follow the positions)
        let pins = [mark(pos(60), pos(65) - 1), mark(pos(20), pos(30) - 1)];
        let other = [mark(pos(40), pos(45))];
        let lone = [mark(pos(80), 0)];
        with_model(key, |m| {
            m.set_marks(9, &[pins[0], pins[1], lone[0]]);
            m.set_marks(3, &other);
        });
        nhg_minimap_set_label_render(Some(fake_render));
        let mut b = Backings::default();
        let f = frame(&mut b, 120);
        let names = [
            NhgMinimapName {
                text: "alpha".as_ptr(),
                len: 5,
            },
            NhgMinimapName {
                text: "beta".as_ptr(),
                len: 4,
            },
        ];
        let e = NhgMinimap {
            pin_kind: 9,
            pin_rgb: 0x00_ffff,
            pins: names.as_ptr(),
            pins_len: names.len(),
            ..entry(key, 1, 100, zv, NHG_MINIMAP_OPEN)
        };
        sync_at(&mut b, f, std::slice::from_ref(&e)).unwrap();
        let m = placed(&commit(f).1, minimap_id(SEQ))
            .unwrap()
            .minimap
            .unwrap();
        let top = 10 + STRIP;
        let fill = 0x4000_4040;
        let pinned: Vec<Rect> = m
            .bands
            .iter()
            .filter(|b| b.fill_argb == fill)
            .map(|b| b.rect)
            .collect();
        assert_eq!(
            pinned,
            [
                Rect::new(200, top + 40, 100, 20),
                Rect::new(200, top + 120, 100, 10)
            ]
        );
        let mut got: Vec<(i32, u32)> = m.labels.iter().map(|l| (l.y - top, l.px[0])).collect();
        got.sort();
        assert_eq!(got, [(40, 0xff00_ffff), (120, 0xff00_ffff)]);
        let texts = RENDERED.lock().unwrap().clone();
        assert!(texts.contains(&"alpha".to_owned()) && texts.contains(&"beta".to_owned()));
        // no pin colour: neither
        sync_at(&mut b, f, &[NhgMinimap { pin_rgb: -1, ..e }]).unwrap();
        let m = placed(&commit(f).1, minimap_id(SEQ))
            .unwrap()
            .minimap
            .unwrap();
        assert!(m.labels.is_empty() && m.bands.iter().all(|b| b.fill_argb != fill));
        hlid_drop(key);
    }

    /// The trail (design M4): a 2 px bar at the minimap's left edge on the rows of the newest `trail_length` visits,
    /// the newest at the trail's opacity fading to 15 % for the oldest drawn; no colour, none.
    #[test]
    fn the_trail_draws_fading_bars_at_the_left_edge() {
        let key = 0x6d6d_0026;
        let text = model(key, 100);
        let zv = text.len() as u64 + 1;
        let pos = |line| with_model(key, |m| m.pos_of(line)).flatten().unwrap();
        with_model(key, |m| {
            for l in [5, 10, 30, 50] {
                m.trail_note(pos(l));
            }
        });
        let mut b = Backings::default();
        let f = frame(&mut b, 120);
        let e = NhgMinimap {
            trail_rgb: 0xff_0000,
            trail_opacity: 1.0,
            trail_length: 3,
            ..entry(key, 1, 100, zv, NHG_MINIMAP_OPEN)
        };
        sync_at(&mut b, f, std::slice::from_ref(&e)).unwrap();
        let m = placed(&commit(f).1, minimap_id(SEQ))
            .unwrap()
            .minimap
            .unwrap();
        let top = 10 + STRIP;
        let bars: Vec<(Rect, u32)> = m
            .bands
            .iter()
            .filter(|b| b.rect.w == 2)
            .map(|b| (b.rect, b.fill_argb >> 24))
            .collect();
        let rects: Vec<Rect> = bars.iter().map(|b| b.0).collect();
        assert_eq!(
            rects,
            [
                Rect::new(200, top + 100, 2, 2),
                Rect::new(200, top + 60, 2, 2),
                Rect::new(200, top + 20, 2, 2)
            ]
        );
        let alphas: Vec<u32> = bars.iter().map(|b| b.1).collect();
        assert_eq!(alphas[0], 0xff);
        assert!((146..=147).contains(&alphas[1]), "{alphas:?}");
        assert!((38..=39).contains(&alphas[2]), "{alphas:?}");
        sync_at(&mut b, f, &[NhgMinimap { trail_rgb: -1, ..e }]).unwrap();
        let m = placed(&commit(f).1, minimap_id(SEQ))
            .unwrap()
            .minimap
            .unwrap();
        assert!(m.bands.iter().all(|b| b.rect.w != 2));
        hlid_drop(key);
    }

    /// A jump flash (design M9): the row lit in the accent, fading over 400 ms on the frame clock and gone at its end
    /// (a publish); with reduced motion lit at full until then; none on a minimap not revealed.
    #[test]
    fn a_flash_lights_the_row_and_fades() {
        use nhglue::minimap::FLASH_NS;
        let key = 0x6d6d_0027;
        let text = model(key, 100);
        let zv = text.len() as u64 + 1;
        let pos = |line| with_model(key, |m| m.pos_of(line)).flatten().unwrap();
        let mut b = Backings::default();
        let f = frame(&mut b, 120);
        let top = 10 + STRIP;
        let lit = |b: &mut Backings, e: &NhgMinimap, t: u64| -> Vec<(Rect, u32)> {
            sync_at_time(b, f, std::slice::from_ref(e), t).unwrap();
            placed(&commit(f).1, minimap_id(SEQ))
                .unwrap()
                .minimap
                .unwrap()
                .bands
                .iter()
                .filter(|b| b.fill_argb & 0xff_ffff != 0 && b.fill_argb & 0xff_ff00 == 0)
                .map(|b| (b.rect, b.fill_argb >> 24))
                .collect()
        };
        let t0 = 1_000 * MS;
        let closed = NhgMinimap {
            accent_rgb: 0x00_00ff,
            ..entry(key, 1, 100, zv, 0)
        };
        lit(&mut b, &closed, t0);
        assert!(!of(&mut b, f).unwrap().flash(SEQ, pos(30), t0));
        let e = NhgMinimap {
            accent_rgb: 0x00_00ff,
            ..entry(key, 1, 100, zv, NHG_MINIMAP_OPEN)
        };
        lit(&mut b, &e, t0);
        assert!(of(&mut b, f).unwrap().flash(SEQ, pos(30), t0));
        assert!(of(&mut b, f).unwrap().animating());
        assert_eq!(of(&mut b, f).unwrap().deadline(), Some(t0 + FLASH_NS));
        assert_eq!(
            lit(&mut b, &e, t0),
            [(Rect::new(200, top + 60, 100, 2), 0xff)]
        );
        let half = lit(&mut b, &e, t0 + FLASH_NS / 2);
        assert!(
            half.len() == 1 && (127..=128).contains(&half[0].1),
            "{half:?}"
        );
        assert!(of(&mut b, f).unwrap().tick(t0 + FLASH_NS).publish);
        assert!(lit(&mut b, &e, t0 + FLASH_NS).is_empty());
        assert!(!of(&mut b, f).unwrap().animating());
        // reduced motion: full until the end, no frames asked for
        let r = NhgMinimap {
            flags: NHG_MINIMAP_OPEN | NHG_MINIMAP_REDUCED_MOTION,
            ..e
        };
        let t1 = t0 + 2 * FLASH_NS;
        lit(&mut b, &r, t1);
        assert!(of(&mut b, f).unwrap().flash(SEQ, pos(30), t1));
        assert_eq!(
            lit(&mut b, &r, t1 + FLASH_NS / 2),
            [(Rect::new(200, top + 60, 100, 2), 0xff)]
        );
        assert!(!of(&mut b, f).unwrap().animating());
        assert!(of(&mut b, f).unwrap().tick(t1 + FLASH_NS).publish);
        assert!(lit(&mut b, &r, t1 + FLASH_NS).is_empty());
        hlid_drop(key);
    }

    /// A dwell's part (design M7): on the marks lane (the strip's width at the outer edge) `_DWELL_MARKS` at the row,
    /// on a label `_DWELL_LABEL` at the label's line, elsewhere `_DWELL` at the row.
    #[test]
    fn a_dwell_names_the_marks_lane_or_the_label() {
        use glitnir::hlidskjalf::Mark;
        use nhglue::minimap::{NHG_MINIMAP_DWELL_LABEL, NHG_MINIMAP_DWELL_MARKS};
        let _held = RENDERER.lock().unwrap_or_else(|e| e.into_inner());
        let key = 0x6d6d_0028;
        let text = model(key, 100);
        let zv = text.len() as u64 + 1;
        let pos = |line| with_model(key, |m| m.pos_of(line)).flatten().unwrap();
        with_model(key, |m| {
            m.set_marks(
                9,
                &[Mark {
                    pos: pos(19),
                    end: pos(25) - 1,
                    lane: 0,
                    class: 0,
                    prio: 0,
                    kind: 9,
                }],
            )
        });
        nhg_minimap_set_label_render(Some(fake_render));
        let mut b = Backings::default();
        let f = frame(&mut b, 120);
        let names = [NhgMinimapName {
            text: "pinned".as_ptr(),
            len: 6,
        }];
        let e = NhgMinimap {
            pin_kind: 9,
            pin_rgb: 0x00_ffff,
            pins: names.as_ptr(),
            pins_len: 1,
            ..entry(
                key,
                1,
                100,
                zv,
                NHG_MINIMAP_OPEN | NHG_MINIMAP_REDUCED_MOTION,
            )
        };
        let t0 = 1_000 * MS;
        sync_at_time(&mut b, f, std::slice::from_ref(&e), t0).unwrap();
        let y = |v: i32| f64::from(v + STRIP);
        let mut dwell = |x: f64, v: i32, t: u64| {
            of(&mut b, f).unwrap().motion(Some((x, y(v))), t);
            of(&mut b, f).unwrap().tick(t + DWELL_NS).events
        };
        // the lane: x 288 to 300; row (50 - 10) / 2 = 20
        assert_eq!(
            dwell(295.0, 50, t0),
            [(NHG_MINIMAP_DWELL_MARKS, SEQ, pos(20))]
        );
        // row 21, inside the label drawn at row 19
        assert_eq!(
            dwell(250.0, 52, t0 + 1_000 * MS),
            [(NHG_MINIMAP_DWELL_LABEL, SEQ, pos(19))]
        );
        assert_eq!(
            dwell(250.0, 130, t0 + 2_000 * MS),
            [(NHG_MINIMAP_DWELL, SEQ, pos(60))]
        );
        hlid_drop(key);
    }

    /// The search's hits (design X1): a bar at each hit's column as long as the hit (at least 2 px), the current one
    /// ringed; none without a colour, none for a hidden line.
    #[test]
    fn search_hits_are_bars_at_their_columns_and_the_current_one_is_ringed() {
        let key = 0x6d6d_0029;
        // line i: i % 8 spaces, then ten x
        let text = model(key, 100);
        let zv = text.len() as u64 + 1;
        let pos = |line| with_model(key, |m| m.pos_of(line)).flatten().unwrap();
        with_model(key, |m| {
            assert!(m.search_start(Some(("xxx", true, false))));
            m.search_step(&[text.as_slice()], std::time::Duration::from_secs(5));
            // the first hit of line 3 (3 spaces in) is the current one
            m.search_set_current(Some(pos(3) + 3));
        });
        let mut b = Backings::default();
        let f = frame(&mut b, 120);
        let e = NhgMinimap {
            hit_rgb: 0xff_0000,
            hit_opacity: 1.0,
            ..entry(key, 1, 100, zv, NHG_MINIMAP_OPEN)
        };
        let get = |b: &mut Backings, e: &NhgMinimap| {
            sync_at(b, f, std::slice::from_ref(e)).unwrap();
            placed(&commit(f).1, minimap_id(SEQ))
                .unwrap()
                .minimap
                .unwrap()
                .bands
                .to_vec()
        };
        let top = 10 + STRIP;
        let bands = get(&mut b, &e);
        let bars: Vec<Rect> = bands
            .iter()
            .filter(|b| b.fill_argb == 0xffff_0000)
            .map(|b| b.rect)
            .collect();
        // ten x hold three hits of xxx: columns indent, +3, +6, 3 px long, on every row
        assert_eq!(bars.len(), 300);
        assert_eq!(
            &bars[9..12],
            [
                Rect::new(203, top + 6, 3, 2),
                Rect::new(206, top + 6, 3, 2),
                Rect::new(209, top + 6, 3, 2)
            ]
        );
        let rings: Vec<Rect> = bands
            .iter()
            .filter(|b| b.fill_argb == 0 && b.line_argb == 0xffff_0000)
            .map(|b| b.rect)
            .collect();
        assert_eq!(rings, [Rect::new(202, top + 5, 5, 4)]);
        // a hidden line's hits are not drawn
        with_model(key, |m| m.set_hidden(&[(pos(3), pos(4))]));
        assert_eq!(
            get(&mut b, &e)
                .iter()
                .filter(|b| b.fill_argb == 0xffff_0000)
                .count(),
            297
        );
        let none = NhgMinimap { hit_rgb: -1, ..e };
        assert!(
            get(&mut b, &none)
                .iter()
                .all(|b| b.fill_argb != 0xffff_0000)
        );
        hlid_drop(key);
    }

    /// A Shift-press starts a line selection (design X2): select-start at the row, select-extend at each row change,
    /// select-end at the release, no click or drag; the label under the pointer is told once per change (X7).
    #[test]
    fn a_shift_drag_selects_lines_and_a_label_hover_is_told_once() {
        use glitnir::hlidskjalf::Mark;
        use nhglue::minimap::{
            NHG_MINIMAP_LABEL_HOVER, NHG_MINIMAP_SELECT_END, NHG_MINIMAP_SELECT_EXTEND,
            NHG_MINIMAP_SELECT_START,
        };
        let _held = RENDERER.lock().unwrap_or_else(|e| e.into_inner());
        let key = 0x6d6d_002a;
        let text = model(key, 100);
        let zv = text.len() as u64 + 1;
        let pos = |line| with_model(key, |m| m.pos_of(line)).flatten().unwrap();
        with_model(key, |m| {
            m.set_marks(
                9,
                &[Mark {
                    pos: pos(19),
                    end: pos(25) - 1,
                    lane: 0,
                    class: 0,
                    prio: 0,
                    kind: 9,
                }],
            )
        });
        nhg_minimap_set_label_render(Some(fake_render));
        let mut b = Backings::default();
        let f = frame(&mut b, 120);
        let names = [NhgMinimapName {
            text: "pinned".as_ptr(),
            len: 6,
        }];
        let e = NhgMinimap {
            pin_kind: 9,
            pin_rgb: 0x00_ffff,
            pins: names.as_ptr(),
            pins_len: 1,
            ..entry(
                key,
                1,
                100,
                zv,
                NHG_MINIMAP_OPEN | NHG_MINIMAP_REDUCED_MOTION,
            )
        };
        let t0 = 1_000 * MS;
        sync_at_time(&mut b, f, std::slice::from_ref(&e), t0).unwrap();
        let y = |v: i32| f64::from(v + STRIP);
        let mm = of(&mut b, f).unwrap();
        // row 10, then a shift-press there; rows 15 and 16; the release on row 16
        mm.motion(Some((250.0, y(30))), t0);
        let r = mm.button(250.0, y(30), true, true, true, t0 + MS);
        assert_eq!(r.events, [(NHG_MINIMAP_SELECT_START, SEQ, pos(10))]);
        let r = mm.motion(Some((250.0, y(40))), t0 + 2 * MS);
        assert_eq!(r.events, [(NHG_MINIMAP_SELECT_EXTEND, SEQ, pos(15))]);
        let r = mm.motion(Some((250.0, y(40))), t0 + 3 * MS);
        assert!(r.events.is_empty());
        let r = mm.motion(Some((250.0, y(42))), t0 + 4 * MS);
        assert_eq!(r.events, [(NHG_MINIMAP_SELECT_EXTEND, SEQ, pos(16))]);
        let r = mm.button(250.0, y(42), true, false, false, t0 + 5 * MS);
        assert_eq!(r.events, [(NHG_MINIMAP_SELECT_END, SEQ, pos(16))]);
        // a press without Shift is a click again
        let r = mm.button(250.0, y(42), true, true, false, t0 + 6 * MS);
        assert_eq!(r.events[0].0, NHG_MINIMAP_CLICK);
        mm.button(250.0, y(42), true, false, false, t0 + 7 * MS);
        // onto the label at row 19 (rows 21 and 22 inside it): one hover, then none; off it, a hover of 0
        let r = mm.motion(Some((250.0, y(52))), t0 + 8 * MS);
        assert_eq!(r.events, [(NHG_MINIMAP_LABEL_HOVER, SEQ, pos(19))]);
        let r = mm.motion(Some((250.0, y(54))), t0 + 9 * MS);
        assert!(r.events.iter().all(|e| e.0 != NHG_MINIMAP_LABEL_HOVER));
        let r = mm.motion(Some((250.0, y(130))), t0 + 10 * MS);
        assert_eq!(r.events, [(NHG_MINIMAP_LABEL_HOVER, SEQ, 0)]);
        let r = mm.motion(Some((250.0, y(52))), t0 + 11 * MS);
        assert_eq!(r.events, [(NHG_MINIMAP_LABEL_HOVER, SEQ, pos(19))]);
        let r = mm.motion(Some((50.0, y(52))), t0 + 12 * MS);
        assert_eq!(r.events, [(NHG_MINIMAP_LABEL_HOVER, SEQ, 0)]);
        hlid_drop(key);
    }

    /// A density strip (design M7, state C): no line model; a 1 px row per pixel of the hub's buckets, brightness the
    /// bytes against the fullest row's, an error (else warning) mark at the outer edge, the unbuilt part outlined, the
    /// band at the window's file fraction (a chunk mapped into the file); a dwell names the bucket, a click the start
    /// that centres the fraction.
    #[test]
    fn a_density_strip_draws_the_hubs_buckets() {
        use nhglue::minimap::{NHG_MINIMAP_DENSITY, NHG_MINIMAP_DWELL_BUCKET};
        let file = "/t/density.log";
        // 400 buckets, 300 built: 0..2 full (400 bytes), the rest 100; bucket 100 an error, 150 two warnings
        let buckets: Vec<String> = (0..300)
            .map(|i| {
                let bytes = if i < 3 { 400 } else { 100 };
                let (e, w) = match i {
                    100 => (1, 0),
                    150 => (0, 2),
                    _ => (0, 0),
                };
                format!("[{bytes},{e},{w}]")
            })
            .collect();
        let line = format!(
            r#"{{"event":"view-model","data":{{"minimap":{{"file":"{file}","layer":"density","version":1,"items":{{"buckets":[{}],"total":400}}}}}}}}"#,
            buckets.join(",")
        );
        glitnir::sja::minimap::push(glitnir::sja::minimap::read_line(&line).unwrap().unwrap());
        let mut b = Backings::default();
        let f = frame(&mut b, 120);
        let e = NhgMinimap {
            key: 0,
            width: 26,
            file: file.as_ptr(),
            file_len: file.len(),
            start: 251,
            end: 351,
            begv: 1,
            zv: 1001,
            chunk_start: 0,
            chunk_bytes: 1000,
            file_bytes: 1000,
            error_rgb: 0xff_0000,
            warning_rgb: 0xff_aa00,
            ..entry(
                0,
                251,
                351,
                1001,
                NHG_MINIMAP_OPEN | NHG_MINIMAP_DENSITY | NHG_MINIMAP_REDUCED_MOTION,
            )
        };
        let get = |b: &mut Backings, e: &NhgMinimap| {
            sync_at_time(b, f, std::slice::from_ref(e), 1_000 * MS).unwrap();
            placed(&commit(f).1, minimap_id(SEQ))
                .unwrap()
                .minimap
                .unwrap()
        };
        let m = get(&mut b, &e);
        let top = 10 + STRIP;
        assert_eq!(m.rect, Rect::new(274, top, 26, 200));
        assert!(m.tiles.is_empty());
        let at = |y: i32| -> Vec<(Rect, u32, u32)> {
            m.bands
                .iter()
                .filter(|b| b.rect.y == top + y && b.rect.h == 1)
                .map(|b| (b.rect, b.fill_argb, b.line_argb))
                .collect()
        };
        assert_eq!(at(0), [(Rect::new(274, top, 26, 1), 0xffff_ffff, 0)]);
        let a = at(10)[0].1 >> 24;
        assert!((63..=64).contains(&a), "{a}");
        assert_eq!(at(50)[1], (Rect::new(297, top + 50, 3, 1), 0xffff_0000, 0));
        assert_eq!(at(75)[1], (Rect::new(297, top + 75, 3, 1), 0xffff_aa00, 0));
        assert!(at(160).is_empty());
        let outlined: Vec<Rect> = m
            .bands
            .iter()
            .filter(|b| b.fill_argb == 0 && b.line_argb == 0xffff_ffff)
            .map(|b| b.rect)
            .collect();
        assert_eq!(outlined, [Rect::new(274, top + 150, 26, 50)]);
        assert_eq!(
            m.bands.last().unwrap().rect,
            Rect::new(274, top + 50, 26, 20)
        );
        // a chunk: the buffer is the file's bytes 2000 to 3000 of 4000
        let chunk = NhgMinimap {
            chunk_start: 2000,
            file_bytes: 4000,
            ..e
        };
        assert_eq!(
            get(&mut b, &chunk).bands.last().unwrap().rect,
            Rect::new(274, top + 113, 26, 5)
        );
        get(&mut b, &e);
        // the pointer: a dwell on row 60 is bucket 120; a click on row 100 the start that centres the middle
        let y = |v: i32| f64::from(v + STRIP);
        let t0 = 2_000 * MS;
        let mm = of(&mut b, f).unwrap();
        mm.motion(Some((290.0, y(70))), t0);
        assert_eq!(
            mm.tick(t0 + DWELL_NS).events,
            [(NHG_MINIMAP_DWELL_BUCKET, SEQ, 120)]
        );
        let r = mm.button(290.0, y(110), true, true, false, t0 + DWELL_NS + MS);
        assert_eq!(r.events, [(NHG_MINIMAP_CLICK, SEQ, 451)]);
        glitnir::sja::minimap::forget(file);
    }
}

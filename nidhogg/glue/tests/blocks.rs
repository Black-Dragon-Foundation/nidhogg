//! Window blocks: block layers from C's list of resolved blocks, against Glitnir itself (no display needed).

mod blocks {
    use glitnir::committed::Committed;
    use glitnir::damage::commit_snapshot;
    use glitnir::geom::Rect;
    use glitnir::scene::{BlockKind, Placed};
    use nhglue::backing::{Backings, NhgBacking, NhgFrameGeom, NhgRect};
    use nhglue::blocks::{NHG_BLOCK_OPEN_TOP, NHG_BLOCK_RULE, NhgBlock, block_id, count, sync_at};
    use std::sync::Arc;

    /// The title strip above the native area.
    const STRIP: i32 = 20;
    /// A text row's height.
    const ROW: i32 = 16;
    const SEQ: u64 = 5;

    /// A 200 x 120 native area under a 20-row strip, at `s120`.
    fn geom(s120: u32) -> NhgFrameGeom {
        let mut g = NhgFrameGeom {
            native_w: 200,
            native_h: 120,
            t: STRIP,
            bg_argb: 0xff00_0000,
            scale120: s120,
            ..Default::default()
        };
        let (w, h) = g.size();
        g.layer = NhgRect { x: 0, y: 0, w, h };
        g
    }

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

    fn placed(c: &Committed, id: u64) -> Option<Placed> {
        c.layers.iter().find(|p| p.id == id).cloned()
    }

    /// The window's text area: below a 16-px header line, 100 px of rows.
    const CLIP: NhgRect = NhgRect {
        x: 0,
        y: ROW,
        w: 200,
        h: 100,
    };

    /// Rows FIRST..=LAST of the text area (0 the first), from x0 to x1.
    fn rows(first: i32, last: i32, x0: i32, x1: i32) -> NhgRect {
        NhgRect {
            x: x0,
            y: CLIP.y + first * ROW,
            w: x1 - x0,
            h: (last - first + 1) * ROW,
        }
    }

    fn block(rect: NhgRect, flags: u32) -> NhgBlock {
        NhgBlock {
            seq: SEQ,
            clip: CLIP,
            rect,
            flags,
            radius: 8,
            ring_rgb: 0x40_5060,
            ring_opacity: 1.0,
            beneath_rgb: 0x12_3456,
        }
    }

    /// Root px of a rectangle with the native area's origin.
    fn at(r: NhgRect) -> Rect {
        Rect::new(r.x, r.y + STRIP, r.w, r.h)
    }

    /// Red row 2: the Flow pair (two blocks on the same rows) gives two rectangles with their own x0/x1; a block
    /// scrolled half out has its top open; two rules on adjacent rows and a ring are two bottom lines and the ring.
    #[test]
    fn plan_pairs_open_sides_and_rules() {
        let mut b = Backings::default();
        let (key, _v) = frame(&mut b, 120);
        let pair = [block(rows(1, 3, 10, 95), 0), block(rows(1, 3, 105, 190), 0)];
        assert_eq!(sync_at(&mut b, key, &pair), Ok(1));
        let (_, c) = commit(key);
        let p = placed(&c, block_id(SEQ)).expect("the block layer");
        assert_eq!(p.rect, at(CLIP));
        let rects: Vec<Rect> = p.blocks.iter().map(|x| x.rect).collect();
        assert_eq!(rects, [at(rows(1, 3, 10, 95)), at(rows(1, 3, 105, 190))]);
        assert!(
            p.blocks
                .iter()
                .all(|x| x.kind == BlockKind::Ring && x.ring_px == 1)
        );
        assert!(
            p.blocks
                .iter()
                .all(|x| x.beneath_argb == 0xff12_3456 && x.ring_argb == 0xff40_5060)
        );

        // scrolled half out: C says the top is open; a block reaching above the text area is open there too
        let above = NhgRect {
            y: CLIP.y - 2 * ROW,
            h: 4 * ROW,
            ..rows(0, 0, 10, 190)
        };
        let half = [
            block(rows(0, 2, 10, 190), NHG_BLOCK_OPEN_TOP),
            block(above, 0),
        ];
        sync_at(&mut b, key, &half).unwrap();
        let (_, c) = commit(key);
        let p = placed(&c, block_id(SEQ)).unwrap();
        assert!(
            p.blocks.iter().all(|x| x.open_top && !x.open_bottom),
            "{:?}",
            p.blocks
        );

        // two rules on adjacent rows, a ring under them
        let flow = [
            block(rows(0, 0, 10, 190), NHG_BLOCK_RULE),
            block(rows(1, 1, 10, 190), NHG_BLOCK_RULE),
            block(rows(2, 4, 10, 190), 0),
        ];
        sync_at(&mut b, key, &flow).unwrap();
        let (_, c) = commit(key);
        let p = placed(&c, block_id(SEQ)).unwrap();
        let kinds: Vec<BlockKind> = p.blocks.iter().map(|x| x.kind).collect();
        assert_eq!(kinds, [BlockKind::Rule, BlockKind::Rule, BlockKind::Ring]);
        // a rule's line is the bottom of its row, its full inset width
        assert_eq!(p.blocks[0].rect, at(rows(0, 0, 10, 190)));
        assert_eq!(p.blocks[1].rect, at(rows(1, 1, 10, 190)));
        assert_eq!(p.blocks[2].rect, at(rows(2, 4, 10, 190)));

        // the window's blocks gone: its layer goes
        assert_eq!(sync_at(&mut b, key, &[]), Ok(0));
        let (_, c) = commit(key);
        assert!(placed(&c, block_id(SEQ)).is_none());
        assert_eq!(count(&mut b, key), 0);
    }

    /// For a shell that shows the cards itself (macOS): each window's blocks drawn alone over a clear text area, placed
    /// in the native area, once per change.  The ring's left side is its colour, a corner outside the rounded path the
    /// colour beneath, the rows inside clear; the same list again gives nothing; the blocks gone leave an empty list.
    #[test]
    fn blocks_drawn_alone_for_a_shell_once_per_change() {
        use nhglue::blocks::images_at;
        let mut b = Backings::default();
        let (key, _v) = frame(&mut b, 120);
        let r = rows(1, 3, 10, 190);
        let one = [block(r, 0)];
        let list = images_at(&mut b, key, &one).unwrap().expect("a change");
        assert_eq!(list.len(), 1);
        let c = &list[0];
        assert_eq!(c.id, block_id(SEQ));
        // the text area, with the native area's origin
        assert_eq!(c.rect, (CLIP.x, CLIP.y, CLIP.w as u32, CLIP.h as u32));
        let (x, y, w, _) = c.rect;
        let px = c.argb.as_ref().expect("an image");
        assert_eq!(px.len(), (CLIP.w * CLIP.h) as usize);
        let at = |nx: i32, ny: i32| px[((ny - y) * w as i32 + (nx - x)) as usize];
        // the left side, halfway down: the ring's colour, opaque
        assert_eq!(at(r.x, r.y + r.h / 2), 0xff40_5060);
        // the top-left corner, outside the rounded path: the colour beneath
        assert_eq!(at(r.x, r.y), 0xff12_3456);
        // inside the ring: the rows' own pixels
        assert_eq!(at(r.x + 20, r.y + r.h / 2), 0);
        // outside the block: clear
        assert_eq!(at(r.x - 5, r.y + r.h / 2), 0);
        assert_eq!(count(&mut b, key), 1);
        // nothing changed: nothing to give
        assert!(images_at(&mut b, key, &one).unwrap().is_none());
        // the blocks go
        assert!(
            images_at(&mut b, key, &[])
                .unwrap()
                .expect("a change")
                .is_empty()
        );
        assert!(images_at(&mut b, key, &[]).unwrap().is_none());
        assert_eq!(count(&mut b, key), 0);
    }

    /// At scale 2 the line is two device px; an unchanged list changes nothing in the scene.
    #[test]
    fn scale_and_no_change() {
        let mut b = Backings::default();
        let (key, _v) = frame(&mut b, 240);
        let one = [block(rows(1, 2, 10, 190), 0)];
        sync_at(&mut b, key, &one).unwrap();
        let (_, c) = commit(key);
        assert_eq!(placed(&c, block_id(SEQ)).unwrap().blocks[0].ring_px, 2);
        sync_at(&mut b, key, &one).unwrap();
        let (damage, _) = commit(key);
        assert!(damage.is_empty(), "{damage:?}");
    }
}

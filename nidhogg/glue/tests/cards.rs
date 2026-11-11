//! Window cards: card layers from the window list, against Glitnir itself (no display needed), and how they live
//! beside the windows' scroll layers.

mod cards {
    use glitnir::committed::Committed;
    use glitnir::damage::commit_snapshot;
    use glitnir::geom::Rect;
    use glitnir::scene::Placed;
    use nhglue::backing::{Backings, CARD_TAG, NhgBacking, NhgFrameGeom, NhgRect, WINDOW_TAG};
    use nhglue::cards::{card_id, sync_at};
    use nhglue::chrome::*;
    use nhglue::layers::{IDLE, NhgLayerWindow, scroll_at};
    use std::sync::Arc;
    use std::time::Instant;

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

    /// The left window's box (its text), and its outer edges (the mode line under it).
    fn left() -> (NhgRect, NhgRect) {
        (
            NhgRect {
                x: 0,
                y: 0,
                w: 94,
                h: 80,
            },
            NhgRect {
                x: 0,
                y: 0,
                w: 94,
                h: 90,
            },
        )
    }

    /// The right window, 6 px of gap away.
    fn right() -> (NhgRect, NhgRect) {
        let (r, o) = left();
        (NhgRect { x: 100, ..r }, NhgRect { x: 100, ..o })
    }

    /// The look's card: radius 10, a soft shadow one px down, the ring at 60 %, the corners over 0x123456.
    fn card() -> [f32; SLOTS] {
        let mut v = [0.0; SLOTS];
        v[SLOT_ON] = 1.0;
        v[SLOT_RADIUS] = 10.0;
        v[SLOT_BLUR] = 3.0;
        v[SLOT_OY] = 1.0;
        v[SLOT_OPACITY] = 0.45;
        v[SLOT_BENEATH_RGB] = 0x12_3456 as f32;
        v[SLOT_RING_RGB] = 0x40_5060 as f32;
        v[SLOT_RING_OPACITY] = 0.6;
        v
    }

    fn window(
        seq: u64,
        (rect, outer): (NhgRect, NhgRect),
        card: Option<[f32; SLOTS]>,
    ) -> NhgLayerWindow {
        NhgLayerWindow {
            seq,
            rect,
            outer,
            card: card.unwrap_or_default(),
        }
    }

    /// Root px of a rectangle with the native area's origin (the strip above it).
    fn at(r: NhgRect) -> Rect {
        Rect::new(r.x, r.y + STRIP, r.w, r.h)
    }

    /// The committed layer with id `id`.
    fn placed(c: &Committed, id: u64) -> Option<Placed> {
        c.layers.iter().find(|p| p.id == id).cloned()
    }

    /// Window `seq` at `rect` scrolls one line up: a full-window scroll.
    fn scroll(b: &mut Backings, key: u64, seq: u64, rect: NhgRect, now: Instant) -> bool {
        let run = NhgRect {
            y: rect.y + LINE,
            h: rect.h - LINE,
            ..rect
        };
        scroll_at(b, key, seq, rect, run, rect.y, true, now).unwrap()
    }

    /// Both halves of a publish's sync, as `nhg_layers_sync` runs them.
    fn sync(b: &mut Backings, key: u64, ws: &[NhgLayerWindow], now: Instant) -> u32 {
        nhglue::layers::sync_at(b, key, ws, true, now).unwrap();
        sync_at(b, key, ws).unwrap()
    }

    /// Two windows in one card group that tile a column are one card around both: one layer at their bounding box,
    /// the divider between them its fill.  When they stop tiling a rectangle the next sync makes them two cards.
    #[test]
    fn a_card_group_is_one_card() {
        let mut b = Backings::default();
        let (key, _) = frame(&mut b, 120);
        let t0 = Instant::now();
        let top = (
            NhgRect {
                x: 0,
                y: 0,
                w: 94,
                h: 40,
            },
            NhgRect {
                x: 0,
                y: 0,
                w: 94,
                h: 44,
            },
        );
        let low = (
            NhgRect {
                x: 0,
                y: 50,
                w: 94,
                h: 36,
            },
            NhgRect {
                x: 0,
                y: 50,
                w: 94,
                h: 40,
            },
        );
        let grouped = || {
            let mut v = card();
            v[SLOT_GROUP] = 1.0;
            v[SLOT_FILL_RGB] = 0x18_191e as f32;
            v
        };
        let ws = [
            window(70, top, Some(grouped())),
            window(71, low, Some(grouped())),
            window(72, right(), Some(card())),
        ];
        assert_eq!(sync(&mut b, key, &ws, t0), 2);
        let (_, c) = commit(key);
        let g = placed(&c, card_id(70)).expect("the group's card");
        assert_eq!(
            g.rect,
            at(NhgRect {
                x: 0,
                y: 0,
                w: 94,
                h: 90
            })
        );
        let k = g.card.expect("a card");
        assert_eq!(
            &k.fill[..],
            &[at(NhgRect {
                x: 0,
                y: 44,
                w: 94,
                h: 6
            })]
        );
        assert_eq!(k.fill_argb, 0xff18_191e);
        assert!(placed(&c, card_id(71)).is_none(), "no card of its own");
        // the lower window narrower: no rectangle, two cards, in the same sync
        let narrow = (NhgRect { w: 80, ..low.0 }, NhgRect { w: 80, ..low.1 });
        let ws = [
            window(70, top, Some(grouped())),
            window(71, narrow, Some(grouped())),
            window(72, right(), Some(card())),
        ];
        assert_eq!(sync(&mut b, key, &ws, t0), 3);
        let (_, c) = commit(key);
        let a = placed(&c, card_id(70)).expect("its own card");
        assert_eq!(a.rect, at(top.1));
        assert!(a.card.expect("a card").fill.is_empty());
        assert_eq!(placed(&c, card_id(71)).map(|p| p.rect), Some(at(narrow.1)));
    }

    /// A full-window scroll in a card window promotes it; its card layer stays, at the outer rectangle with its card,
    /// above the output.
    #[test]
    fn card_window_promotes_and_keeps_its_card() {
        let mut b = Backings::default();
        let (key, _) = frame(&mut b, 120);
        let t0 = Instant::now();
        let ws = [window(30, left(), Some(card())), window(31, right(), None)];
        assert_eq!(sync(&mut b, key, &ws, t0), 1);
        let before = placed(&commit(key).1, card_id(30)).expect("the card layer");
        assert!(scroll(&mut b, key, 30, left().0, t0));
        assert_eq!(sync(&mut b, key, &ws, t0), 1);
        let (_, c) = commit(key);
        assert_eq!(c.outputs.len(), 1);
        assert_eq!(c.outputs[0].layer, 30 | WINDOW_TAG);
        assert_eq!(placed(&c, card_id(30)), Some(before.clone()));
        let k = before.card.expect("a card");
        assert_eq!(k.beneath_argb, 0xff12_3456);
        assert_eq!(before.rect, at(left().1));
        // the card goes over the output's labels: above it in paint order
        let pos = |id| c.layers.iter().position(|p| p.id == id).unwrap();
        assert!(pos(card_id(30)) > pos(30 | WINDOW_TAG));
    }

    /// A window already promoted that gains a card stays promoted.
    #[test]
    fn promoted_window_gaining_card_stays_promoted() {
        let mut b = Backings::default();
        let (key, _) = frame(&mut b, 120);
        let t0 = Instant::now();
        assert!(scroll(&mut b, key, 40, left().0, t0));
        assert_eq!(sync(&mut b, key, &[window(40, left(), None)], t0), 0);
        commit(key);
        assert_eq!(
            sync(&mut b, key, &[window(40, left(), Some(card()))], t0),
            1
        );
        let (_, c) = commit(key);
        assert_eq!(c.outputs.len(), 1, "still its own output");
        assert!(
            !c.outputs[0].full,
            "the card changed nothing of the output's picture but its corners"
        );
        assert!(placed(&c, card_id(40)).is_some());
        let pos = |id| c.layers.iter().position(|p| p.id == id).unwrap();
        assert!(
            pos(card_id(40)) > pos(40 | WINDOW_TAG),
            "the card was raised over the output"
        );
    }

    /// After the idle demotion the card layer is as it was.
    #[test]
    fn demoted_card_window_keeps_its_card() {
        let mut b = Backings::default();
        let (key, _) = frame(&mut b, 120);
        let t0 = Instant::now();
        let ws = [window(50, left(), Some(card()))];
        assert!(scroll(&mut b, key, 50, left().0, t0));
        sync(&mut b, key, &ws, t0);
        let before = placed(&commit(key).1, card_id(50));
        assert!(before.is_some());
        assert_eq!(sync(&mut b, key, &ws, t0 + IDLE), 1);
        let (_, c) = commit(key);
        assert!(c.outputs.is_empty(), "demoted");
        assert_eq!(placed(&c, card_id(50)), before);
    }

    /// The card's layer and the window's output are two layers: other ids, other rectangles.
    #[test]
    fn card_and_output_ids_differ() {
        let mut b = Backings::default();
        let (key, _) = frame(&mut b, 120);
        let t0 = Instant::now();
        assert!(scroll(&mut b, key, 60, left().0, t0));
        sync(&mut b, key, &[window(60, left(), Some(card()))], t0);
        let (_, c) = commit(key);
        assert_ne!(card_id(60), 60 | WINDOW_TAG);
        assert_eq!(card_id(60) & CARD_TAG, CARD_TAG);
        assert_eq!(placed(&c, 60 | WINDOW_TAG).unwrap().rect, at(left().0));
        assert_eq!(placed(&c, card_id(60)).unwrap().rect, at(left().1));
        assert!(placed(&c, 60 | WINDOW_TAG).unwrap().card.is_none());
    }

    /// A window that loses its card, or goes, takes its card layer with it.
    #[test]
    fn removed_card_leaves_scene() {
        let mut b = Backings::default();
        let (key, _) = frame(&mut b, 120);
        let t0 = Instant::now();
        let both = [
            window(70, left(), Some(card())),
            window(71, right(), Some(card())),
        ];
        assert_eq!(sync(&mut b, key, &both, t0), 2);
        // C lists the windows while the glue has cards: the publish after the last one went still removes it
        assert_eq!(nhglue::layers::count(&mut b, key), 2);
        commit(key);
        // 70 loses its card, 71 is deleted
        assert_eq!(sync(&mut b, key, &[window(70, left(), None)], t0), 0);
        let (damage, c) = commit(key);
        assert!(placed(&c, card_id(70)).is_none());
        assert!(placed(&c, card_id(71)).is_none());
        assert!(
            !damage.is_empty(),
            "the corners and the gap are drawn again"
        );
        assert_eq!(nhglue::cards::count(&mut b, key), 0);
    }

    /// For a shell that shows the cards itself: each card drawn alone, in the native area, once per change.  The
    /// left window's top-left corner pixel is the colour beneath (opaque), the middle of its text is clear, the gap
    /// carries its shadow; the same list again gives nothing; a card that goes leaves an empty list.
    #[test]
    fn cards_drawn_alone_for_a_shell_once_per_change() {
        use nhglue::cards::images_at;
        let mut b = Backings::default();
        let (key, _) = frame(&mut b, 120);
        let one = [window(70, left(), Some(card())), window(71, right(), None)];
        let list = images_at(&mut b, key, &one).unwrap().expect("a change");
        assert_eq!(list.len(), 1);
        let c = &list[0];
        assert_eq!(c.id, card_id(70));
        let (x, y, w, h) = c.rect;
        let px = c.argb.as_ref().expect("an image");
        assert_eq!(px.len(), w as usize * h as usize);
        let at = |px_x: i32, px_y: i32| px[((px_y - y) * w as i32 + (px_x - x)) as usize];
        // the window's top-left pixel (native 0,0) shows what lies beneath, opaque
        assert_eq!(at(0, 0) >> 24, 0xff, "corner {:08x}", at(0, 0));
        assert_eq!(at(0, 0) & 0xff_ffff, 0x12_3456);
        // the middle of the text is the window's own
        assert_eq!(at(47, 40), 0);
        // the gap beside it, below the top, has the shadow or the ring
        assert!(x + (w as i32) > 94 && at(95, 40) >> 24 > 0);
        // nothing changed: nothing to give
        assert!(images_at(&mut b, key, &one).unwrap().is_none());
        // the card goes
        let none = [window(70, left(), None), window(71, right(), None)];
        assert!(
            images_at(&mut b, key, &none)
                .unwrap()
                .expect("a change")
                .is_empty()
        );
        assert!(images_at(&mut b, key, &none).unwrap().is_none());
    }

    /// A scale change gives the cards again at the new scale: the radius, the shadow and the ring in device px.
    #[test]
    fn scale_change_reapplies() {
        let mut b = Backings::default();
        let (mut key, _) = frame(&mut b, 120);
        let t0 = Instant::now();
        let ws = [window(80, left(), Some(card()))];
        sync(&mut b, key, &ws, t0);
        let one = placed(&commit(key).1, card_id(80)).unwrap();
        assert_eq!(one.chrome.unwrap().radius_px, 10.0);
        assert_eq!(one.card.as_ref().unwrap().ring_px, 1);
        b.ensure(&mut key, &geom(240));
        sync(&mut b, key, &ws, t0);
        let two = placed(&commit(key).1, card_id(80)).unwrap();
        let ch = two.chrome.unwrap();
        assert_eq!((ch.radius_px, ch.blur_px, ch.offset_y), (20.0, 6.0, 2.0));
        assert_eq!(two.card.as_ref().unwrap().ring_px, 2);
        assert_eq!(two.scale, 2.0);
        // a reconnect forgets what was applied; the next sync gives the same again
        nhglue::cards::forget_all(&mut b);
        sync(&mut b, key, &ws, t0);
        assert_eq!(placed(&commit(key).1, card_id(80)), Some(two));
    }

    /// A card keeps its shadow and ring out of every other window's outer rectangle, its own excepted (the bands
    /// around the window area follow them), and the ring comes premultiplied.
    #[test]
    fn keep_out_is_other_windows_outer() {
        let mut b = Backings::default();
        let (key, _) = frame(&mut b, 120);
        let t0 = Instant::now();
        let mini = window(
            99,
            (
                NhgRect {
                    x: 0,
                    y: 92,
                    w: 200,
                    h: 8,
                },
                NhgRect {
                    x: 0,
                    y: 92,
                    w: 200,
                    h: 8,
                },
            ),
            None,
        );
        let ws = [
            window(90, left(), Some(card())),
            window(91, right(), None),
            mini,
        ];
        sync(&mut b, key, &ws, t0);
        let k = placed(&commit(key).1, card_id(90)).unwrap().card.unwrap();
        assert_eq!(&k.keep_out[..2], &[at(right().1), at(mini.outer)]);
        assert_eq!(k.keep_out.len(), 6, "and the bands around");
        assert_eq!(k.ring_argb, 0x9926_303a);
        // the right window moves: the card's list follows
        let moved = (
            NhgRect {
                x: 110,
                w: 90,
                ..right().0
            },
            NhgRect {
                x: 110,
                w: 90,
                ..right().1
            },
        );
        sync(&mut b, key, &[ws[0], window(91, moved, None), mini], t0);
        let k = placed(&commit(key).1, card_id(90)).unwrap().card.unwrap();
        assert_eq!(k.keep_out[0], at(moved.1));
    }

    /// A card's shadow and ring stay inside the window area, the bounding box of the listed windows' outer
    /// rectangles: what lies around it (the title strip here; a frame's tab, tool and menu bars) is kept out too.
    #[test]
    fn chrome_keeps_inside_the_window_area() {
        let mut b = Backings::default();
        let (key, _) = frame(&mut b, 120);
        let t0 = Instant::now();
        let ws = [window(92, left(), Some(card())), window(93, right(), None)];
        sync(&mut b, key, &ws, t0);
        let p = placed(&commit(key).1, card_id(92)).unwrap();
        let k = p.card.expect("a card");
        let area = at(left().1).bounds(&at(right().1));
        // the shadow reaches above the window, into the strip, and past its left edge
        assert!(p.bounds.y < area.y && p.bounds.x < area.x, "{:?}", p.bounds);
        for y in p.bounds.y..p.bounds.y + p.bounds.h {
            for x in p.bounds.x..p.bounds.x + p.bounds.w {
                assert!(
                    area.contains(x, y) || k.keep_out.iter().any(|r| r.contains(x, y)),
                    "{x},{y} is outside the window area and not kept out: {:?}",
                    k.keep_out
                );
            }
        }
    }
}

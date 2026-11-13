//! Window edges: edge layers from C's list of resolved edges, against Glitnir itself (no display needed).

mod edges {
    use glitnir::committed::Committed;
    use glitnir::damage::commit_snapshot;
    use glitnir::geom::Rect;
    use glitnir::scene::{LayerDesc, Placed};
    use nhglue::backing::{Backings, NhgBacking, NhgFrameGeom, NhgRect, WINDOW_TAG};
    use nhglue::blocks::{NhgBlock, block_id};
    use nhglue::edges::{
        NHG_EDGE_ARROW, NHG_EDGE_DASH, NHG_EDGE_POINTS, NhgEdge, NhgPoint, count, edge_id, sync_at,
    };
    use std::sync::Arc;

    /// The title strip above the native area.
    const STRIP: i32 = 20;
    const SEQ: u64 = 5;

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

    fn index(c: &Committed, id: u64) -> usize {
        c.layers.iter().position(|p| p.id == id).expect("the layer")
    }

    /// The window's text area.
    const CLIP: NhgRect = NhgRect {
        x: 0,
        y: 16,
        w: 200,
        h: 100,
    };

    fn edge(points: &[(i32, i32)], flags: u32) -> NhgEdge {
        let mut p = [NhgPoint::default(); NHG_EDGE_POINTS];
        for (i, (x, y)) in points.iter().enumerate() {
            p[i] = NhgPoint { x: *x, y: *y };
        }
        NhgEdge {
            seq: SEQ,
            clip: CLIP,
            points: p,
            npoints: points.len() as u32,
            flags,
            rgb: 0x40_5060,
            opacity: 1.0,
        }
    }

    /// An edge's layer sits at the window's text area and holds its points in root px; the sizes come from the scale
    /// (at 2: a 2-px line, 12-px bends and arrow, 6-px dashes); an edge of one point is left out.
    #[test]
    fn an_edge_gets_its_layer_and_the_scales_sizes() {
        let mut b = Backings::default();
        let (key, _v) = frame(&mut b, 240);
        let e = [
            edge(&[(10, 30), (60, 30), (60, 90)], NHG_EDGE_ARROW),
            edge(&[(100, 30), (150, 30)], NHG_EDGE_DASH),
            edge(&[(5, 5)], 0),
        ];
        assert_eq!(sync_at(&mut b, key, &e), Ok(1));
        let (_, c) = commit(key);
        let p = placed(&c, edge_id(SEQ)).expect("the edge layer");
        assert_eq!(p.rect, Rect::new(CLIP.x, CLIP.y + STRIP, CLIP.w, CLIP.h));
        assert_eq!(p.edges.len(), 2);
        let (a, d) = (&p.edges[0], &p.edges[1]);
        assert_eq!(
            &*a.points,
            &[(10, 30 + STRIP), (60, 30 + STRIP), (60, 90 + STRIP)]
        );
        assert_eq!(
            (a.width_px, a.radius_px, a.arrow_px, a.dash_px),
            (2, 12, 12, 0)
        );
        assert_eq!((d.arrow_px, d.dash_px), (0, 6));
        assert_eq!(a.argb, 0xff40_5060);
        assert_eq!(count(&mut b, key), 1);

        // the edges gone: the layer goes
        assert_eq!(sync_at(&mut b, key, &[]), Ok(0));
        let (_, c) = commit(key);
        assert!(placed(&c, edge_id(SEQ)).is_none());
        assert_eq!(count(&mut b, key), 0);
    }

    /// The edge layer stays above the window's blocks and above an output made after it; an unchanged list changes
    /// nothing in the scene.
    #[test]
    fn edges_stay_on_top_and_an_unchanged_list_is_no_change() {
        let mut b = Backings::default();
        let (key, _v) = frame(&mut b, 120);
        let e = [edge(&[(10, 30), (60, 30), (60, 90)], NHG_EDGE_ARROW)];
        sync_at(&mut b, key, &e).unwrap();
        let block = NhgBlock {
            seq: SEQ,
            clip: CLIP,
            rect: NhgRect {
                x: 10,
                y: 32,
                w: 80,
                h: 32,
            },
            radius: 8,
            ring_rgb: 0x40_5060,
            ring_opacity: 1.0,
            ..Default::default()
        };
        nhglue::blocks::sync_at(&mut b, key, &[block]).unwrap();
        let out = SEQ | WINDOW_TAG;
        glitnir::scene::with(key, |sc| {
            sc.upsert(&LayerDesc {
                id: out,
                parent: key,
                kind: 2,
                rect: Rect::new(0, 16 + STRIP, 200, 100),
                scale: 1.0,
                visible: 1,
            })
        })
        .unwrap();
        sync_at(&mut b, key, &e).unwrap();
        let (_, c) = commit(key);
        let top = index(&c, edge_id(SEQ));
        assert!(top > index(&c, block_id(SEQ)), "{:?}", c.layers);
        assert!(top > index(&c, out), "{:?}", c.layers);

        sync_at(&mut b, key, &e).unwrap();
        let (damage, _) = commit(key);
        assert!(damage.is_empty(), "{damage:?}");
    }
}

//! Layout legs: the vector's check, and what a capture, a leg and a scroll do to the window's layer, against Glitnir
//! itself (no display needed: what goes to the shell is returned).

mod legs {
    use glitnir::damage::commit_snapshot;
    use glitnir::geom::Rect;
    use glitnir::layout::ItemKind;
    use nhglue::backing::{Backings, NhgFrameGeom, NhgRect};
    use nhglue::layers::*;
    use nhglue::legs::*;
    use std::time::Instant;

    /// The title strip above the native area.
    const STRIP: i32 = 20;
    const SEQ: u64 = 7;
    /// The vectors' text area in the backing: the strip lies above the native area.
    const TEXT: Rect = Rect::new(4, 2 + STRIP, 180, 70);

    /// A frame with its scene and backing: a 200 x 100 native area under the strip, at scale 1.
    fn frame(b: &mut Backings) -> u64 {
        let mut key = 0;
        let mut g = NhgFrameGeom {
            native_w: 200,
            native_h: 100,
            t: STRIP,
            bg_argb: 0xff00_0000,
            scale120: 120,
            ..Default::default()
        };
        let (w, h) = g.size();
        g.layer = NhgRect { x: 0, y: 0, w, h };
        b.ensure(&mut key, &g);
        b.backing(key, &g).unwrap();
        commit_snapshot(key).expect("a scene");
        key
    }

    /// The window: the native area's top 80 rows, full width.
    const WIN: NhgRect = NhgRect {
        x: 0,
        y: 0,
        w: 200,
        h: 80,
    };

    /// The text moves up one 10-row line: a full-window scroll.
    fn scroll(b: &mut Backings, key: u64) -> bool {
        let run = NhgRect {
            x: 0,
            y: 10,
            w: 200,
            h: 70,
        };
        let now = Instant::now();
        scroll_at(b, key, SEQ, WIN, run, 0, true, now).unwrap()
    }

    /// A vector: capture `capture` of the text area (4, 2, 180, 70); with `motion`, a leg from that capture moving one
    /// item from (10, 10, 30, 20) to (100, 40, 30, 20).
    fn vector(capture: f32, motion: f32) -> Vec<f32> {
        let mut v = vec![
            capture, 4.0, 2.0, 180.0, 70.0, motion, 0.0, 160.0, 0.0, 0.0, 0.58, 1.0, 0.0,
        ];
        if motion == 0.0 {
            v.push(0.0);
        } else {
            v.push(1.0);
            v.extend([1.0, 5.0, 10.0, 10.0, 30.0, 20.0, 100.0, 40.0, 30.0, 20.0]);
        }
        v
    }

    /// What window `SEQ` asks the shell for with the vector `v`.
    fn ask(b: &mut Backings, key: u64, v: &[f32]) -> Vec<Ask> {
        apply_at(b, key, SEQ, decode(v).unwrap()).unwrap()
    }

    #[test]
    fn vector_out_of_range_is_refused() {
        // pattern D: pure
        assert_eq!(decode(&[]), Ok(None));
        let d = decode(&vector(1.0, 1.0)).unwrap().expect("a capture");
        assert_eq!(d.capture, 1);
        assert_eq!((d.rect.x, d.rect.y, d.rect.w, d.rect.h), (4, 2, 180, 70));
        let leg = d.motion.expect("a leg");
        assert_eq!(leg.items.len(), 1);
        assert_eq!(leg.items[0].kind, ItemKind::Move);
        assert_eq!(leg.duration_ns, 160_000_000);
        let mut bad = vector(1.0, 1.0);
        bad[HEAD] = 9.0;
        assert!(decode(&bad).is_err(), "a kind out of range");
        let mut short = vector(1.0, 1.0);
        short.pop();
        assert!(decode(&short).is_err(), "a length not the count's");
        let mut nan = vector(1.0, 0.0);
        nan[SLOT_X] = f32::NAN;
        assert!(decode(&nan).is_err(), "a slot that is not finite");
        let mut curve = vector(1.0, 1.0);
        curve[SLOT_X1] = 2.0;
        assert!(decode(&curve).is_err(), "a curve x outside 0 to 1");
    }

    #[test]
    fn capture_demotes_the_window() {
        // pattern D: pure (Glitnir's scene, no display)
        let mut b = Backings::default();
        let key = frame(&mut b);
        assert!(scroll(&mut b, key), "promoted");
        assert_eq!(count(&mut b, key), 1);
        let asks = ask(&mut b, key, &vector(1.0, 0.0));
        assert_eq!(asks, vec![Ask::Capture(1, TEXT)]);
        assert_eq!(count(&mut b, key), 0, "part of the frame again");
        let again = ask(&mut b, key, &vector(1.0, 0.0));
        assert!(again.is_empty(), "nothing new: {again:?}");
        let asks = ask(&mut b, key, &vector(1.0, 1.0));
        let [Ask::Leg(leg)] = asks.as_slice() else {
            panic!("a leg: {asks:?}");
        };
        assert_eq!((leg.capture, leg.rect), (1, TEXT));
        let from = Rect::new(14, 12 + STRIP, 30, 20);
        assert_eq!(leg.items[0].from, from, "from the text area");
        assert_eq!(leg.items[0].to, Rect::new(104, 42 + STRIP, 30, 20));
        assert_eq!(ask(&mut b, key, &[]), vec![Ask::End]);
        assert!(ask(&mut b, key, &[]).is_empty(), "ended once");
    }

    #[test]
    fn scroll_ends_the_leg_and_promotes() {
        // pattern D: pure (Glitnir's scene, no display)
        let mut b = Backings::default();
        let key = frame(&mut b);
        ask(&mut b, key, &vector(1.0, 0.0));
        ask(&mut b, key, &vector(1.0, 1.0));
        assert!(take_ended(&mut b, key).is_empty());
        assert!(scroll(&mut b, key), "the scroll promotes it as before");
        assert_eq!(take_ended(&mut b, key), vec![SEQ], "and ends its leg");
        assert!(take_ended(&mut b, key).is_empty(), "once");
        let asks = ask(&mut b, key, &vector(2.0, 0.0));
        assert_eq!(asks.len(), 1, "a new capture: {asks:?}");
        assert_eq!(count(&mut b, key), 0, "which demotes it again");
    }
}

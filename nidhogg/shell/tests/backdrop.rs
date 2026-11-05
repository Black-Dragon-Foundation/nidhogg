//! T1 tests of a child window's backdrop blur in the presenter: it follows the parent's damage and nothing else, in the
//! tree commit that shows the change.  They run on the recorder's manual clock at 120 Hz, stepped by `present`.  A
//! child here is clear, so what the recorder shows at it is the blur itself, and that must equal a blur made afresh
//! from the root's pixels.  The harness is `common/mod.rs`.

mod common;

use common::backing::*;
use common::*;
use glitnir::blur::Backdrop;
use glitnir::ease::Curve;
use glitnir::geom::Rect;
use glitnir::motion::{Kind as Move, Leg, MotionSpec};
use glitnir::scene::{Chrome, LayerDesc};
use glitnir::stats::{Percentiles, summary};
use nhshell::api::*;
use nhshell::event::*;
use nhshell::wayland::WaylandShell;
use std::time::{Duration, Instant};

/// One frame at 120 Hz.
const STEP: u64 = 8_333_333;
const BLACK: u32 = 0xff00_0000;
const WHITE: u32 = 0xffff_ffff;
const SIZE: (i32, i32) = (40, 20);
const ROOT: (i32, i32) = (200, 100);
/// σ, device and logical pixels: scale 1.
const SIGMA: f32 = 4.0;
/// A window of the root that is its own output, and its Glitnir id (a window id carries the top bit, as the glue's do).
const VIEW: Rect = Rect::new(0, 0, 200, 80);
const WIN: u64 = (1 << 63) | 7;

fn stats(g: u64) -> u64 {
    glitnir::stats::summary(g, false).backdrop_px
}

struct Tree {
    rec: Recorder,
    shell: WaylandShell,
    root: u64,
    rb: Backing,
    kid: u64,
    kb: Backing,
    /// The child's surface in the root's, and its layer in its own.
    at: (i32, i32),
    layer: Rect,
}

impl Tree {
    /// A 200x100 top-level, its left half black and its right half white, with its first picture shown (its pacer
    /// idle), on the presenter thread unless `inline`, and a clear child made over the middle, not shown yet.
    fn open(inline: bool) -> Tree {
        let rec = Recorder::start_refresh("kwin", "manual", 120_000);
        let mut shell = rec.open();
        if !inline {
            shell.set_presenter_thread(true).unwrap();
        }
        let root = shell
            .window_new(&WindowDesc {
                w: ROOT.0,
                h: ROOT.1,
                title: "t".into(),
                app_id: "nidhogg.test".into(),
                flags: 0,
            })
            .unwrap();
        let mut t = Tree {
            root,
            shell,
            rb: Backing::new(ROOT.0, ROOT.1),
            kid: 0,
            kb: Backing::new(SIZE.0, SIZE.1),
            at: (80, 40),
            layer: Rect::new(0, 0, SIZE.0, SIZE.1),
            rec,
        };
        let ev = wait_for(&mut t.shell, |e| e.kind() == Kind::Configure);
        t.shell
            .ack_configure(t.root, configure_of(&ev).serial, NHSHELL_ACK_CONTENT)
            .unwrap();
        t.rb.fill_all(BLACK);
        t.rb.fill(Rect::new(100, 0, 100, ROOT.1), WHITE);
        assert!(publish(&mut t.shell, t.root, &[(&t.rb, t.root)], 0));
        t.committed();
        t.step();
        t.idle();
        t
    }

    fn child(&mut self) {
        self.kid = self
            .shell
            .child_new(&ChildDesc {
                kind: NHSHELL_KIND_CHILD,
                parent: self.root,
                x: self.at.0,
                y: self.at.1,
                w: self.kb.w,
                h: self.kb.h,
                anchor: Anchor::default(),
                flags: 0,
            })
            .unwrap();
    }

    /// Shown clear with a backdrop.
    fn blurred(inline: bool) -> Tree {
        let mut t = Tree::open(inline);
        t.child();
        t.shell.set_backdrop(t.kid, SIGMA).unwrap();
        t.kb.fill_all(0);
        t.frame();
        t.shows_the_blur();
        t
    }

    /// The child's surface is 60x40, with its one layer inset by 10 and `chrome` on it.
    fn chromed(&mut self, chrome: Chrome) {
        self.kb = Backing::new(60, 40);
        self.layer = Rect::new(10, 10, SIZE.0, SIZE.1);
        self.at = (70, 30);
        self.kb.src = (10, 10);
        let (g, layer) = (self.kb.g, self.layer);
        glitnir::scene::with(g, |s| {
            s.upsert(&LayerDesc {
                id: g,
                parent: 0,
                kind: 1,
                rect: layer,
                scale: 1.0,
                visible: 1,
            })?;
            s.set_chrome(g, Some(chrome))
        })
        .unwrap();
    }

    fn publish_tree(&mut self) -> bool {
        publish(
            &mut self.shell,
            self.root,
            &[(&self.rb, self.root), (&self.kb, self.kid)],
            0,
        )
    }

    /// Publish the tree and let the frame that shows it go by.
    fn frame(&mut self) {
        assert!(self.publish_tree());
        self.committed();
        self.step();
        self.idle();
    }

    /// Hide the child, with no leg, in the root's next picture: the host publishes the tree without it.
    fn hide(&mut self) {
        self.shell.set_mapped(self.kid, false).unwrap();
        assert!(publish(
            &mut self.shell,
            self.root,
            &[(&self.rb, self.root)],
            0
        ));
        self.committed();
        self.step();
        self.idle();
        self.rec
            .wait_node(self.kid, |c| c.contains("\"mapped\":false"));
    }

    /// In inline mode, what the host does at `FRAME_READY`.
    fn pump(&mut self) {
        while let Some(ev) = self.shell.next_event().unwrap() {
            assert_ne!(ev.kind(), Kind::Disconnected, "a protocol error");
            if ev.kind() == Kind::FrameReady {
                self.shell.present_tree(self.root).unwrap();
                self.shell.flush().unwrap();
            }
        }
    }

    fn callbacks(&self) -> u64 {
        number(&self.rec.control(r#"{"cmd":"outstanding"}"#), "callbacks").expect("callbacks")
    }

    /// The recorder has read what the shell sent: its loop serves the sockets in turns.
    fn sync(&self) {
        self.shell.flush().unwrap();
        self.callbacks();
        self.callbacks();
    }

    /// `f` holds, within 10 s, in no less time than the machine takes.
    fn until(&mut self, what: &str, f: impl Fn(&mut Tree) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            self.pump();
            if f(self) {
                return;
            }
            assert!(Instant::now() < deadline, "never: {what}");
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    /// The tree's next paced commit is in.
    fn committed(&mut self) {
        self.until("a paced commit", |t| t.callbacks() > 0);
        self.sync();
    }

    /// One frame: the recorder's clock moves on and answers what waits; then the tree's next paced commit is in, or its
    /// transitions are over and what they sent last has arrived.
    fn step(&mut self) {
        self.rec.present(STEP);
        self.until("a frame or the end", |t| {
            t.callbacks() > 0 || !t.shell.motion_ticking(t.root)
        });
        self.sync();
    }

    /// The frame callback reached the pacer.
    fn idle(&mut self) {
        self.until("an idle pacer", |t| {
            !t.shell.present_pacer(t.root).unwrap().0
        });
    }

    fn next_seq(&self) -> u64 {
        number(&self.rec.control(r#"{"cmd":"log","since":1}"#), "next").expect("next")
    }

    /// Surface `id`'s commits with a buffer from recorder commit `seq` on.
    fn buffers(&self, seq: u64, id: u64) -> usize {
        let log = self
            .rec
            .control(&format!(r#"{{"cmd":"log","since":{seq}}}"#));
        // the objects have sorted keys, so `ack_serial` opens each commit
        log.split("{\"ack_serial\":")
            .skip(1)
            .filter(|c| c.contains(&format!("\"surface\":{id},")) && c.contains("\"buffer\":{"))
            .count()
    }

    fn pixel(&self, x: i32, y: i32) -> u32 {
        Recorder::snapshot_pixel(&self.rec.snapshot(), x as usize, y as usize)
    }

    /// The child's layer in the root.
    fn region(&self) -> Rect {
        self.layer.translate(self.at.0, self.at.1)
    }

    /// The root's pixels blurred afresh.
    fn fresh(&self) -> Backdrop {
        let (px, _) = self.rb.px.as_chunks::<4>();
        let px: Vec<u32> = px.iter().map(|c| u32::from_ne_bytes(*c)).collect();
        let mut b = Backdrop::new();
        b.update(&px, ROOT.0 as usize, ROOT, self.region(), SIGMA, 1.0, &[]);
        b
    }

    /// What the compositor shows at the child's layer is the root's pixels blurred afresh.
    fn shows_the_blur(&self) {
        let r = self.region();
        let b = self.fresh();
        let snap = self.rec.snapshot();
        let mut row = vec![0; r.w as usize];
        for y in r.y..r.y + r.h {
            b.sample_row(y, r.x, &mut row);
            for x in r.x..r.x + r.w {
                let got = Recorder::snapshot_pixel(&snap, x as usize, y as usize);
                let want = row[(x - r.x) as usize];
                assert_eq!(
                    got, want,
                    "pixel ({x}, {y}): {got:08x}, a fresh blur {want:08x}"
                );
            }
        }
    }

    /// What the compositor shows at the child's layer is the root's own pixels.
    fn shows_the_root(&self) {
        let r = self.region();
        let snap = self.rec.snapshot();
        for y in r.y..r.y + r.h {
            for x in r.x..r.x + r.w {
                let got = Recorder::snapshot_pixel(&snap, x as usize, y as usize);
                assert_eq!(got, self.rb.pixel(x, y), "pixel ({x}, {y})");
            }
        }
    }

    fn no_protocol_error(&mut self) {
        assert_eq!(self.shell.connection_state(), ConnectionState::Up);
        self.pump();
        assert_eq!(self.shell.connection_state(), ConnectionState::Up);
    }
}

mod backdrop {
    use super::*;

    fn follows_parent_damage(inline: bool) {
        let mut t = Tree::blurred(inline);
        let first = stats(t.kb.g);
        assert_eq!(first, t.fresh().recomputed_px(), "all of it, once");
        // a change beneath: the child's new picture goes in the commit of the root's
        let seq = t.next_seq();
        t.rb.fill(Rect::new(84, 44, 12, 12), WHITE);
        t.frame();
        assert_eq!(t.buffers(seq, t.kid), 1);
        assert_eq!(t.buffers(seq, t.root), 1);
        t.shows_the_blur();
        let second = stats(t.kb.g);
        assert!(second > first, "{second} after {first}");
        // a change beyond the blur's reach costs the child nothing
        let seq = t.next_seq();
        t.rb.fill(Rect::new(0, 0, 10, 10), WHITE);
        t.frame();
        assert_eq!(t.buffers(seq, t.kid), 0);
        assert_eq!(stats(t.kb.g), second);
        t.shows_the_blur();
        t.no_protocol_error();
    }

    #[test]
    fn backdrop_follows_parent_damage() {
        follows_parent_damage(false);
    }

    #[test]
    fn backdrop_follows_parent_damage_inline() {
        follows_parent_damage(true);
    }

    /// The blur's time covers recomputes alone: a change beyond its reach and typing in the child add none, and a
    /// change beneath adds one that lies inside its frame.
    #[test]
    fn blur_timer_counts_recomputes_only() {
        let mut t = Tree::blurred(false);
        let g = t.kb.g;
        assert!(summary(g, true).blur_ms.max > 0.0, "the first picture's");
        t.rb.fill(Rect::new(0, 0, 10, 10), WHITE);
        t.frame();
        t.kb.fill(Rect::new(2, 2, 2, 8), 0xff10_2030);
        t.frame();
        assert_eq!(summary(g, false).blur_ms, Percentiles::default());
        let began = Instant::now();
        t.rb.fill(Rect::new(84, 44, 12, 12), WHITE);
        t.frame();
        let span = began.elapsed().as_secs_f64() * 1e3;
        let got = summary(g, false).blur_ms;
        assert!(got.max > 0.0 && got.max <= span, "{got:?}, {span} ms");
        assert_eq!((got.p50, got.p95), (got.max, got.max), "one recompute");
        t.no_protocol_error();
    }

    #[test]
    fn typing_in_child_no_backdrop_px() {
        let mut t = Tree::blurred(false);
        let before = stats(t.kb.g);
        let seq = t.next_seq();
        for i in 0..10 {
            t.kb.fill(Rect::new(2 + 3 * i, 2, 2, 8), 0xff10_2030);
            t.frame();
        }
        assert_eq!(t.buffers(seq, t.kid), 10);
        assert_eq!(stats(t.kb.g), before);
        // the blur still shows beside what was typed
        let r = t.region();
        let snap = t.rec.snapshot();
        assert_eq!(
            Recorder::snapshot_pixel(&snap, r.x as usize + 2, r.y as usize + 2),
            0xff10_2030
        );
        let mid = Recorder::snapshot_pixel(&snap, 100, r.y as usize + 15) & 0xff;
        assert!((1..255).contains(&mid), "{mid}");
    }

    /// A key typed into the child shows in a tree commit of the child's buffer alone (the palette's case): the
    /// child's entry has its key to commit, and the key is spent there, so the root's next commit pairs with none.
    /// Pattern B: the manual clock gates each frame.
    #[test]
    fn typing_in_child_measures_key_to_commit() {
        let mut t = Tree::blurred(false);
        summary(t.kb.g, true);
        summary(t.rb.g, true);
        let key = nhshell::wayland::publish::now_ns() - 5_000_000_000;
        t.kb.fill(Rect::new(2, 2, 2, 8), 0xff10_2030);
        assert!(publish(
            &mut t.shell,
            t.root,
            &[(&t.rb, t.root), (&t.kb, t.kid)],
            key
        ));
        t.committed();
        t.step();
        t.idle();
        let kid = summary(t.kb.g, false).update_ms;
        assert!(
            (5_000.0..10_000.0).contains(&kid.max),
            "the child's key to commit {kid:?}"
        );
        // the root changes later, with no key
        t.rb.fill(Rect::new(0, 0, 10, 10), WHITE);
        t.frame();
        assert_eq!(summary(t.rb.g, false).update_ms, Percentiles::default());
    }

    #[test]
    fn opaque_background_skips_blur() {
        let mut t = Tree::open(false);
        t.child();
        // the glue sends 0 for a frame with an opaque background
        t.shell.set_backdrop(t.kid, 0.0).unwrap();
        t.kb.fill_all(0);
        t.frame();
        t.shows_the_root();
        assert_eq!(stats(t.kb.g), 0);
    }

    #[test]
    fn off_composes_without_it() {
        let mut t = Tree::blurred(false);
        let before = stats(t.kb.g);
        let seq = t.next_seq();
        t.shell.set_backdrop(t.kid, 0.0).unwrap();
        t.committed();
        t.step();
        t.idle();
        assert_eq!(t.buffers(seq, t.kid), 1);
        t.shows_the_root();
        assert_eq!(stats(t.kb.g), before);
    }

    #[test]
    fn child_move_recomputes() {
        let mut t = Tree::blurred(false);
        let before = stats(t.kb.g);
        let seq = t.next_seq();
        t.at = (90, 10);
        t.shell.set_position(t.kid, t.at.0, t.at.1).unwrap();
        t.committed();
        t.step();
        t.idle();
        assert_eq!(t.buffers(seq, t.kid), 1, "the move shows with the new blur");
        t.shows_the_blur();
        assert!(stats(t.kb.g) > before);
    }

    #[test]
    fn fade_of_blurred_child_no_residue() {
        let mut t = Tree::blurred(false);
        let leg = |kind, ms: u64| Leg {
            kind,
            duration_ns: ms * 1_000_000,
            curve: Curve::LINEAR,
        };
        t.shell
            .set_motion(
                t.kid,
                Some(MotionSpec {
                    show: leg(Move::None, 0),
                    hide: leg(Move::Fade, 80),
                }),
            )
            .unwrap();
        // over black, beside the root's edge, the picture is what the leg left of the blur
        let at = (97, 50);
        let mut last = t.pixel(at.0, at.1) & 0xff;
        assert!(last > 0);
        let px = stats(t.kb.g);
        t.shell.set_mapped(t.kid, false).unwrap();
        t.committed();
        let mut n = 0;
        while t.shell.motion_ticking(t.root) {
            let v = t.pixel(at.0, at.1) & 0xff;
            assert!(v <= last, "frame {n}: {v} after {last}");
            last = v;
            t.step();
            n += 1;
            assert!(n < 100, "a leg that does not end");
        }
        assert!(n > 1, "{n} frames");
        t.rec.wait_node(t.kid, |c| c.contains("\"mapped\":false"));
        t.shows_the_root();
        assert_eq!(stats(t.kb.g), px, "blur never animates");
        t.no_protocol_error();
    }

    #[test]
    fn hidden_child_catches_up() {
        let mut t = Tree::blurred(false);
        let before = stats(t.kb.g);
        t.rb.fill(Rect::new(84, 44, 12, 12), WHITE);
        t.hide();
        assert_eq!(stats(t.kb.g), before, "nothing for a child not shown");
        t.shell.set_mapped(t.kid, true).unwrap();
        t.kb.fill_all(0);
        t.frame();
        t.shows_the_blur();
        assert!(stats(t.kb.g) > before);
    }

    /// The rows a window's own output scrolled are the blur's to follow, as much as the ones it drew.
    #[test]
    fn scrolled_output_recomputes() {
        let mut t = Tree::open(false);
        t.child();
        let (rb, root) = (&mut t.rb, t.root);
        glitnir::scene::with(rb.g, |s| {
            s.upsert(&LayerDesc {
                id: WIN,
                parent: rb.g,
                kind: 2,
                rect: VIEW,
                scale: 1.0,
                visible: 1,
            })?;
            s.set_output(WIN, true)
        })
        .unwrap();
        for y in (0..VIEW.h).step_by(4) {
            let v = 0x1_0101 * (y as u32 * 3);
            rb.fill(Rect::new(0, y, VIEW.w, 4), 0xff00_0000 | v);
        }
        assert!(publish(&mut t.shell, root, &[(&t.rb, root)], 0));
        t.committed();
        t.step();
        t.idle();
        t.shell.set_backdrop(t.kid, SIGMA).unwrap();
        t.kb.fill_all(0);
        t.frame();
        t.shows_the_blur();
        for _ in 0..3 {
            let before = stats(t.kb.g);
            let seq = t.next_seq();
            let (rb, dy) = (&mut t.rb, -16);
            let w4 = (rb.w * 4) as usize;
            let rows = (VIEW.h + dy) as usize;
            rb.px.copy_within(16 * w4..(16 + rows) * w4, 0);
            let exposed = Rect::new(0, VIEW.h + dy, VIEW.w, -dy);
            glitnir::damage::scroll(rb.g, WIN, VIEW, dy, exposed);
            rb.fill(exposed, 0xff80_4020);
            t.frame();
            assert_eq!(t.buffers(seq, t.kid), 1);
            t.shows_the_blur();
            assert!(stats(t.kb.g) > before);
        }
    }

    #[test]
    fn corner_shows_what_lies_beneath() {
        let mut t = Tree::open(false);
        t.chromed(Chrome {
            radius_px: 8.0,
            ..Chrome::default()
        });
        t.child();
        t.shell.set_backdrop(t.kid, SIGMA).unwrap();
        t.kb.fill_all(0);
        t.frame();
        let r = t.region();
        assert_eq!(t.pixel(r.x, r.y), t.rb.pixel(r.x, r.y));
        let (x, y) = (r.x + r.w - 1, r.y + r.h - 1);
        assert_eq!(t.pixel(x, y), t.rb.pixel(x, y));
        // inside the corners the blur shows, past where the root's own edge is sharp
        let mid = t.pixel(99, r.y + 10) & 0xff;
        assert!((1..255).contains(&mid), "{mid}");
    }

    /// A shadow goes OVER the buffer's older picture: repainted, and through a leg's frames, it stays as it was.
    #[test]
    fn repaints_leave_the_shadow_as_it_was() {
        let mut t = Tree::open(false);
        t.rb.fill_all(WHITE);
        assert!(publish(&mut t.shell, t.root, &[(&t.rb, t.root)], 0));
        t.committed();
        t.step();
        t.idle();
        t.chromed(Chrome {
            radius_px: 8.0,
            blur_px: 12.0,
            shadow_opacity: 0.5,
            shadow_argb: BLACK,
            ..Chrome::default()
        });
        t.child();
        t.kb.fill_all(0xff20_4060);
        t.frame();
        let at = (t.at.0 + 5, t.at.1 + 20);
        let rested = t.pixel(at.0, at.1);
        assert_ne!(rested, WHITE, "the shadow shows");
        for _ in 0..6 {
            t.kb.fill_all(0xff20_4060);
            t.frame();
            assert_eq!(t.pixel(at.0, at.1), rested);
        }
        let spec = MotionSpec {
            show: Leg {
                kind: Move::Pop,
                duration_ns: 60_000_000,
                curve: Curve::LINEAR,
            },
            hide: Leg {
                kind: Move::None,
                duration_ns: 0,
                curve: Curve::LINEAR,
            },
        };
        t.shell.set_motion(t.kid, Some(spec)).unwrap();
        t.rb.fill(Rect::new(0, 0, 1, 1), WHITE);
        t.hide();
        t.shell.set_mapped(t.kid, true).unwrap();
        t.kb.fill_all(0xff20_4060);
        assert!(t.publish_tree());
        t.committed();
        let mut n = 0;
        while t.shell.motion_ticking(t.root) {
            t.step();
            n += 1;
            assert!(n < 100, "a leg that does not end");
            // on its way in the shadow is fainter than at rest, never built up
            let v = t.pixel(at.0, at.1) & 0xff;
            assert!(v >= rested & 0xff, "frame {n}: {v:02x} over {rested:08x}");
        }
        assert!(n > 1, "{n} frames");
        assert_eq!(t.pixel(at.0, at.1), rested);
    }
}

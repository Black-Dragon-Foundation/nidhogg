//! T1 tests of a window's layout leg in the presenter: the capture taken before the redraw, ticks from the pacer, the
//! end at the new layout.  They run on the recorder's manual clock at 120 Hz, stepped by `present`, so that a test's
//! frames are the test's (as `motion.rs`).  The harness is `common/mod.rs`.

mod common;

use common::backing::*;
use common::*;
use glitnir::ease::Curve;
use glitnir::geom::Rect;
use glitnir::layout::{Item, ItemKind, LayoutLeg};
use glitnir::scene::LayerDesc;
use nhshell::api::*;
use nhshell::event::*;
use nhshell::wayland::WaylandShell;
use std::time::{Duration, Instant};

/// One frame at 120 Hz.
const STEP: u64 = 8_333_333;
const MS: u64 = 1_000_000;
const BLACK: u32 = 0xff00_0000;
/// The old layout's background is blue, the new one's green, a card white.
const OLD: u32 = 0xff00_00ff;
const NEW: u32 = 0xff00_ff00;
const CARD: u32 = 0xffff_ffff;
const BG: u32 = 0xff20_2020;
/// The window's text area in the 200x100 root, the glue's key for it, and two places of a card in it.
const R: Rect = Rect::new(20, 10, 160, 80);
const KEY: u64 = 7;
const A: Rect = Rect::new(30, 20, 40, 20);
const B: Rect = Rect::new(120, 60, 40, 20);
/// A pixel of the background away from both places.
const AWAY: (usize, usize) = (100, 25);

fn mv(id: u32, from: Rect, to: Rect) -> Item {
    Item {
        id,
        kind: ItemKind::Move,
        from,
        start: from,
        to,
    }
}

/// A leg over `R` from capture `capture`, 160 ms linear.
fn leg(capture: u64, items: Vec<Item>) -> LayoutLeg {
    LayoutLeg {
        capture,
        rect: R,
        items,
        background: BG,
        duration_ns: 160 * MS,
        curve: Curve::LINEAR,
        step: false,
    }
}

fn blue(p: u32) -> u32 {
    p & 0xff
}

fn green(p: u32) -> u32 {
    (p >> 8) & 0xff
}

/// A shell and a 200x100 top-level with its first picture shown (its pacer idle).
struct Tree {
    rec: Recorder,
    shell: WaylandShell,
    root: u64,
    rb: Backing,
}

impl Tree {
    /// On a recorder at 120 Hz with `profile`; on the presenter thread unless `inline`.
    fn open(profile: &str, inline: bool) -> Tree {
        let rec = Recorder::start_refresh(profile, "manual", 120_000);
        let mut shell = rec.open();
        if !inline {
            shell.set_presenter_thread(true).unwrap();
        }
        let mut t = Tree {
            root: 0,
            shell,
            rb: Backing::new(200, 100),
            rec,
        };
        t.root = t
            .shell
            .window_new(&WindowDesc {
                w: 200,
                h: 100,
                title: "t".into(),
                app_id: "nidhogg.test".into(),
                flags: 0,
            })
            .unwrap();
        let ev = wait_for(&mut t.shell, |e| e.kind() == Kind::Configure);
        t.shell
            .ack_configure(t.root, configure_of(&ev).serial, NHSHELL_ACK_CONTENT)
            .unwrap();
        t.rb.fill_all(BLACK);
        t.shown();
        t
    }

    /// Publish the root with the window's background `bg` and a card at `card`.
    fn layout(&mut self, bg: u32, card: Rect) {
        self.rb.fill(R, bg);
        self.rb.fill(card, CARD);
        assert!(publish(
            &mut self.shell,
            self.root,
            &[(&self.rb, self.root)],
            0
        ));
    }

    /// Publish what the backing holds and let it show: its commit, its frame, an idle pacer.
    fn shown(&mut self) {
        assert!(publish(
            &mut self.shell,
            self.root,
            &[(&self.rb, self.root)],
            0
        ));
        self.committed();
        self.step();
        self.idle();
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
    }

    /// One frame: the recorder's clock moves on and answers what waits; then the tree's next paced commit is in, or
    /// its legs are over and what they sent last has arrived.
    fn step(&mut self) {
        self.rec.present(STEP);
        self.until("a frame or the end", |t| t.callbacks() > 0 || !t.ticking());
        self.sync();
    }

    /// The frame callback reached the pacer.
    fn idle(&mut self) {
        self.until("an idle pacer", |t| {
            !t.shell.present_pacer(t.root).unwrap().0
        });
    }

    fn ticking(&self) -> bool {
        self.shell.motion_ticking(self.root)
    }

    fn ack(&self) -> LayoutAck {
        self.shell.layout_captured(self.root, KEY).unwrap()
    }

    /// The capture `n` of the window, acknowledged.
    fn capture(&mut self, n: u64) {
        self.shell.layout_capture(self.root, KEY, n, R).unwrap();
        self.until("the capture", |t| t.ack().captured == n);
    }

    fn next_seq(&self) -> u64 {
        number(&self.rec.control(r#"{"cmd":"log","since":1}"#), "next").expect("next")
    }

    /// The root's buffer commits from recorder commit `seq` on.
    fn buffers(&self, seq: u64) -> usize {
        let log = self
            .rec
            .control(&format!(r#"{{"cmd":"log","since":{seq}}}"#));
        let id = format!("\"surface\":{},", self.root);
        // the objects have sorted keys, so `ack_serial` opens each commit
        log.split("{\"ack_serial\":")
            .skip(1)
            .filter(|c| c.contains(&id) && c.contains("\"buffer\":{"))
            .count()
    }

    fn pixel(&self, (x, y): (usize, usize)) -> u32 {
        Recorder::snapshot_pixel(&self.rec.snapshot(), x, y)
    }

    /// Run the legs out, one frame at a time; the frames it took.
    fn finish(&mut self) -> usize {
        let mut n = 0;
        while self.ticking() {
            self.step();
            n += 1;
            assert!(n < 200, "a leg that does not end");
        }
        n
    }

    /// The window's picture is the new layout: background `NEW`, the card at `card`.
    fn shows_new(&self, card: Rect) {
        let snap = self.rec.snapshot();
        for y in R.y..R.y + R.h {
            for x in R.x..R.x + R.w {
                let want = if card.contains(x, y) { CARD } else { NEW };
                let p = Recorder::snapshot_pixel(&snap, x as usize, y as usize);
                assert_eq!(p, want, "pixel ({x}, {y})");
            }
        }
    }

    fn no_protocol_error(&mut self) {
        assert_eq!(self.shell.connection_state(), ConnectionState::Up);
        self.pump();
        assert_eq!(self.shell.connection_state(), ConnectionState::Up);
    }
}

mod layout {
    use super::*;

    #[test]
    fn layout_leg_uses_the_picture_from_before_the_redraw() {
        // pattern C: handshake (the capture's acknowledgement, then the frames the recorder's clock gives)
        for inline in [false, true] {
            let mut t = Tree::open("kwin", inline);
            t.layout(OLD, A);
            t.committed();
            t.step();
            t.idle();
            t.capture(1);
            t.shell
                .layout_leg(t.root, KEY, Some(leg(1, vec![mv(1, A, B)])))
                .unwrap();
            t.layout(NEW, B);
            t.committed();
            let p = t.pixel(AWAY);
            assert!(blue(p) > green(p), "inline {inline}: {p:08x}");
            assert!(t.ack().running);
            t.finish();
            t.shows_new(B);
            assert!(!t.ack().running);
            t.no_protocol_error();
        }
    }

    #[test]
    fn leg_without_its_capture_is_no_leg() {
        // pattern C: handshake
        let mut t = Tree::open("kwin", false);
        t.layout(OLD, A);
        t.committed();
        t.step();
        t.idle();
        t.shell
            .layout_leg(t.root, KEY, Some(leg(5, vec![mv(1, A, B)])))
            .unwrap();
        t.until("the skip", |t| t.ack().skipped == 1);
        t.layout(NEW, B);
        t.committed();
        t.shows_new(B);
        assert!(!t.ticking() && !t.ack().running);
    }

    #[test]
    fn leg_commits_per_present_and_ends_at_identity() {
        // pattern C: handshake
        let mut t = Tree::open("kwin", false);
        t.layout(OLD, A);
        t.committed();
        t.step();
        t.idle();
        t.capture(1);
        let seq = t.next_seq();
        t.shell
            .layout_leg(t.root, KEY, Some(leg(1, vec![mv(1, A, B)])))
            .unwrap();
        t.layout(NEW, B);
        t.committed();
        let mut n = 1;
        let mut last = blue(t.pixel(AWAY));
        while t.ticking() {
            t.step();
            n += 1;
            assert_eq!(t.buffers(seq), n, "one buffer per present");
            let b = blue(t.pixel(AWAY));
            assert!(b <= last, "frame {n}: {b} after {last}");
            last = b;
        }
        // 160 ms is 19.2 frames at 120 Hz: the frame at or past the end is the last
        assert!((19..=21).contains(&n), "{n} frames");
        t.shows_new(B);
        let seq = t.next_seq();
        for _ in 0..3 {
            t.step();
        }
        assert_eq!(t.buffers(seq), 0, "no commit after the end");
        t.no_protocol_error();
    }

    #[test]
    fn retarget_starts_from_the_sampled_rectangle() {
        // pattern C: handshake
        let mut t = Tree::open("kwin", false);
        t.layout(OLD, A);
        t.committed();
        t.step();
        t.idle();
        t.capture(1);
        t.shell
            .layout_leg(t.root, KEY, Some(leg(1, vec![mv(1, A, B)])))
            .unwrap();
        t.layout(NEW, B);
        t.committed();
        // ten frames in, about halfway: the card covers the middle between its places
        for _ in 0..9 {
            t.step();
        }
        let mid = (95, 50);
        assert_eq!(t.pixel(mid), CARD, "halfway");
        // the next layout moves it back to A, from where it is
        t.capture(2);
        t.shell
            .layout_leg(t.root, KEY, Some(leg(2, vec![mv(1, B, A)])))
            .unwrap();
        t.layout(NEW, A);
        t.step();
        assert_eq!(t.pixel(mid), CARD, "from where the card was");
        assert_ne!(t.pixel((140, 70)), CARD, "not from B");
        t.finish();
        t.shows_new(A);
    }

    #[test]
    fn ending_a_leg_jumps_to_the_new_layout() {
        // pattern C: handshake.  Reduced motion, a theme change, a scroll and a nil parameter all send `None`.
        let mut t = Tree::open("kwin", false);
        t.layout(OLD, A);
        t.committed();
        t.step();
        t.idle();
        t.capture(1);
        t.shell
            .layout_leg(t.root, KEY, Some(leg(1, vec![mv(1, A, B)])))
            .unwrap();
        t.layout(NEW, B);
        t.committed();
        t.step();
        t.shell.layout_leg(t.root, KEY, None).unwrap();
        t.step();
        t.until("the end", |t| !t.ticking());
        t.shows_new(B);
        assert!(!t.ack().running);
        let seq = t.next_seq();
        t.step();
        assert_eq!(t.buffers(seq), 0, "no commit after the end");
    }

    #[test]
    fn over_budget_leg_is_skipped() {
        // pattern C: handshake
        let mut t = Tree::open("kwin", false);
        t.shell.layout_budget(16);
        t.layout(OLD, A);
        t.committed();
        t.step();
        t.idle();
        t.capture(1);
        t.shell
            .layout_leg(t.root, KEY, Some(leg(1, vec![mv(1, A, B)])))
            .unwrap();
        t.layout(NEW, B);
        t.committed();
        t.until("the skip", |t| t.ack().skipped == 1);
        t.until("no leg", |t| !t.ticking());
        t.shows_new(B);
    }

    #[test]
    fn card_corners_hold_during_a_leg() {
        // pattern C: handshake
        const CARD_LAYER: u64 = (1 << 62) | 7;
        const BENEATH: u32 = 0xff11_2233;
        let mut t = Tree::open("kwin", false);
        let g = t.rb.g;
        glitnir::scene::with(g, |s| {
            s.upsert(&LayerDesc {
                id: CARD_LAYER,
                parent: g,
                kind: 2,
                rect: R,
                scale: 1.0,
                visible: 1,
            })?;
            s.set_chrome(
                CARD_LAYER,
                Some(glitnir::scene::Chrome {
                    radius_px: 10.0,
                    ..Default::default()
                }),
            )?;
            s.set_card(
                CARD_LAYER,
                Some(glitnir::scene::Card {
                    beneath_argb: BENEATH,
                    ..Default::default()
                }),
            )
        })
        .unwrap();
        t.layout(OLD, A);
        t.committed();
        t.step();
        t.idle();
        let corner = (R.x as usize, R.y as usize);
        assert_eq!(t.pixel(corner), BENEATH, "the card's corner");
        t.capture(1);
        t.shell
            .layout_leg(t.root, KEY, Some(leg(1, vec![mv(1, A, B)])))
            .unwrap();
        t.layout(NEW, B);
        t.committed();
        let mut n = 0;
        loop {
            assert_eq!(t.pixel(corner), BENEATH, "frame {n}: the corner");
            let p = t.pixel(AWAY);
            assert_ne!(p, BENEATH, "frame {n}: the leg draws inside");
            if !t.ticking() {
                break;
            }
            t.step();
            n += 1;
            assert!(n < 200, "a leg that does not end");
        }
        assert!(n > 10, "{n} frames");
    }
}

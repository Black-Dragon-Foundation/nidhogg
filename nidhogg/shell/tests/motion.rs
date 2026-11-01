//! T1 tests of a child window's transitions in the presenter: ticks from the pacer, exit pictures and ghosts.  They run
//! on the recorder's manual clock at 120 Hz, stepped by `present`, so that a test's frames are the test's: a starved
//! machine may take longer, never show a frame more.  The harness is `common/mod.rs`.

mod common;

use common::backing::*;
use common::*;
use glitnir::ease::Curve;
use glitnir::geom::Rect;
use glitnir::motion::{Kind as Move, Leg, MotionSpec};
use nhshell::api::*;
use nhshell::event::*;
use nhshell::wayland::WaylandShell;
use std::time::{Duration, Instant};

/// One frame at 120 Hz.
const STEP: u64 = 8_333_333;
/// The root is black and the child white, so a child pixel's channel is its opacity.
const BLACK: u32 = 0xff00_0000;
const WHITE: u32 = 0xffff_ffff;
/// The child's place in the root, and a pixel in its middle.
const AT: (i32, i32) = (30, 20);
const SIZE: (i32, i32) = (40, 20);
const MID: (usize, usize) = (50, 30);

fn leg(kind: Move, ms: u64, curve: Curve) -> Leg {
    Leg {
        kind,
        duration_ns: ms * 1_000_000,
        curve,
    }
}

/// The defaults: a pop to show, 120 ms `ease-out`; a fade to hide, 80 ms `ease-in`.
fn pop() -> MotionSpec {
    MotionSpec {
        show: leg(Move::Pop, 120, Curve::EASE_OUT),
        hide: leg(Move::Fade, 80, Curve::EASE_IN),
    }
}

/// A shell, a 200x100 top-level with its first picture shown (its pacer idle), and a child not made yet.
struct Tree {
    rec: Recorder,
    shell: WaylandShell,
    root: u64,
    rb: Backing,
    kid: u64,
    kb: Backing,
}

impl Tree {
    /// On a recorder at 120 Hz with `profile`; on the presenter thread unless `inline`.
    fn open(profile: &str, inline: bool) -> Tree {
        Tree::on(Recorder::start_refresh(profile, "manual", 120_000), inline)
    }

    fn on(rec: Recorder, inline: bool) -> Tree {
        let mut shell = rec.open();
        if !inline {
            shell.set_presenter_thread(true).unwrap();
        }
        let mut t = Tree {
            root: 0,
            shell,
            rb: Backing::new(200, 100),
            kid: 0,
            kb: Backing::new(SIZE.0, SIZE.1),
            rec,
        };
        t.mapped();
        t
    }

    /// The top-level, its first configure acked with its first picture, and that frame answered.
    fn mapped(&mut self) {
        self.root = self
            .shell
            .window_new(&WindowDesc {
                w: 200,
                h: 100,
                title: "t".into(),
                app_id: "nidhogg.test".into(),
                flags: 0,
            })
            .unwrap();
        let ev = wait_for(&mut self.shell, |e| e.kind() == Kind::Configure);
        self.shell
            .ack_configure(self.root, configure_of(&ev).serial, NHSHELL_ACK_CONTENT)
            .unwrap();
        self.rb.fill_all(BLACK);
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

    fn child(&mut self) -> u64 {
        self.shell
            .child_new(&ChildDesc {
                kind: NHSHELL_KIND_CHILD,
                parent: self.root,
                x: AT.0,
                y: AT.1,
                w: SIZE.0,
                h: SIZE.1,
                anchor: Anchor::default(),
                flags: 0,
            })
            .unwrap()
    }

    /// Publish the tree with the child white, all of it damaged.
    fn draw_kid(&mut self) {
        self.kb.fill_all(WHITE);
        assert!(publish(
            &mut self.shell,
            self.root,
            &[(&self.rb, self.root), (&self.kb, self.kid)],
            0
        ));
    }

    /// The child made, then given `spec`, and not yet drawn.
    fn kid_with(&mut self, spec: MotionSpec) {
        self.kid = self.child();
        self.shell.set_motion(self.kid, Some(spec)).unwrap();
    }

    /// The child made and shown white with no transition, then given `spec`: it rests, shown.
    fn shown_with(&mut self, spec: MotionSpec) {
        self.kid = self.child();
        self.draw_kid();
        self.committed();
        self.step();
        self.idle();
        self.shell.set_motion(self.kid, Some(spec)).unwrap();
    }

    /// In inline mode, what the host does at `FRAME_READY`.
    fn pump(&mut self) -> u32 {
        let mut ready = 0;
        while let Some(ev) = self.shell.next_event().unwrap() {
            assert_ne!(ev.kind(), Kind::Disconnected, "a protocol error");
            if ev.kind() == Kind::FrameReady {
                ready += 1;
                self.shell.present_tree(self.root).unwrap();
                self.shell.flush().unwrap();
            }
        }
        ready
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
    /// its transitions are over and what they sent last has arrived.
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

    fn next_seq(&self) -> u64 {
        number(&self.rec.control(r#"{"cmd":"log","since":1}"#), "next").expect("next")
    }

    /// Surface `id`'s commits from recorder commit `seq` on, one JSON text each.
    fn commits(&self, seq: u64, id: u64) -> Vec<String> {
        let log = self
            .rec
            .control(&format!(r#"{{"cmd":"log","since":{seq}}}"#));
        // the objects have sorted keys, so `ack_serial` opens each commit
        log.split("{\"ack_serial\":")
            .skip(1)
            .map(|c| format!("{{\"ack_serial\":{c}"))
            .filter(|c| c.contains(&format!("\"surface\":{id},")))
            .collect()
    }

    fn buffers(&self, seq: u64, id: u64) -> usize {
        self.commits(seq, id)
            .iter()
            .filter(|c| c.contains("\"buffer\":{"))
            .count()
    }

    fn mapped_now(&self, id: u64) -> bool {
        let key = format!("\"id\":{id},");
        self.rec
            .control(r#"{"cmd":"tree"}"#)
            .split("{\"above\":")
            .skip(1)
            .find(|n| n.contains(&key))
            .is_some_and(|n| n.contains("\"mapped\":true"))
    }

    /// The channel of the pixel in the child's middle: 255 opaque, 0 not shown.
    fn opacity(&self) -> u32 {
        let snap = self.rec.snapshot();
        Recorder::snapshot_pixel(&snap, MID.0, MID.1) & 0xff
    }

    /// Run the transitions out, one frame at a time; the frames it took.
    fn finish(&mut self) -> usize {
        let mut n = 0;
        while self.ticking() {
            self.step();
            n += 1;
            assert!(n < 200, "a transition that does not end");
        }
        n
    }

    fn no_protocol_error(&mut self) {
        assert_eq!(self.shell.connection_state(), ConnectionState::Up);
        self.pump();
        assert_eq!(self.shell.connection_state(), ConnectionState::Up);
    }
}

mod motion {
    use super::*;

    #[test]
    fn pop_in_commits_each_present_until_identity() {
        let mut t = Tree::open("kwin", false);
        t.kid_with(pop());
        let seq = t.next_seq();
        t.draw_kid();
        t.committed();
        assert_eq!(t.buffers(seq, t.kid), 1, "the leg's first frame");
        // a pop starts small: the corner is less covered than the middle
        let snap = t.rec.snapshot();
        let corner = Recorder::snapshot_pixel(&snap, AT.0 as usize, AT.1 as usize) & 0xff;
        let mut last = Recorder::snapshot_pixel(&snap, MID.0, MID.1) & 0xff;
        assert!(last < 255, "the first frame is not opaque yet: {last}");
        assert!(corner < last, "corner {corner}, middle {last}");
        let mut n = 1;
        while t.ticking() {
            t.step();
            n += 1;
            assert_eq!(t.buffers(seq, t.kid), n, "one buffer per present");
            let a = t.opacity();
            assert!(a >= last, "frame {n}: {a} after {last}");
            last = a;
        }
        assert!((2..=16).contains(&n), "{n} frames");
        // the last frame equals a show without motion
        let snap = t.rec.snapshot();
        for y in 0..SIZE.1 {
            for x in 0..SIZE.0 {
                let p = Recorder::snapshot_pixel(&snap, (AT.0 + x) as usize, (AT.1 + y) as usize);
                assert_eq!(p, WHITE, "pixel ({x}, {y})");
            }
        }
        assert_eq!(glitnir::stats::summary(t.kb.g, false).ticks, n as u64);
        t.no_protocol_error();
    }

    #[test]
    fn fade_out_unmaps_after_duration() {
        let mut t = Tree::open("kwin", false);
        t.shown_with(pop());
        let seq = t.next_seq();
        t.shell.set_mapped(t.kid, false).unwrap();
        t.committed();
        // frame n is drawn for n frames from the leg's start: the one at or past 80 ms (n = 10) is the unmap
        let mut presents = 0;
        let mut last = t.opacity();
        while t.mapped_now(t.kid) {
            t.step();
            presents += 1;
            if t.mapped_now(t.kid) {
                let a = t.opacity();
                assert!(a <= last, "after {presents} presents: {a} after {last}");
                last = a;
            }
            assert!(presents < 30, "never unmapped");
        }
        assert!(
            (9..=11).contains(&presents),
            "unmapped after {presents} presents"
        );
        assert!(!t.ticking());
        let log = t.commits(seq, t.kid);
        let end = log.last().unwrap();
        assert!(!end.contains("\"buffer\":{"), "{end}");
        assert_eq!(t.buffers(seq, t.kid), presents, "{log:?}");
        t.no_protocol_error();
    }

    /// The tree's root commits carry no damage of their own, so sway discards their feedback: the leg runs on the real
    /// clock, and still unmaps.
    #[test]
    fn discarded_feedback_ends_a_leg_by_the_real_clock() {
        let mut t = Tree::open("sway", false);
        t.shown_with(pop());
        t.shell.set_mapped(t.kid, false).unwrap();
        t.committed();
        t.until("the unmap", |t| {
            t.rec.present(STEP);
            !t.mapped_now(t.kid)
        });
        assert!(!t.ticking());
        assert!(t.shell.present_pacer(t.root).unwrap().3 > 0, "discarded");
        t.no_protocol_error();
    }

    /// C's next packet lists a hidden frame as not drawable: the leg goes on.
    #[test]
    fn hide_leg_outlives_a_packet_without_the_child() {
        let mut t = Tree::open("kwin", false);
        t.shown_with(pop());
        t.shell.set_mapped(t.kid, false).unwrap();
        t.committed();
        t.step();
        let seq = t.next_seq();
        t.rb.fill(Rect::new(0, 0, 10, 10), 0xff00_ff00);
        assert!(publish(&mut t.shell, t.root, &[(&t.rb, t.root)], 0));
        t.step();
        assert_eq!(t.buffers(seq, t.kid), 1, "the leg went on");
        t.finish();
        assert!(!t.mapped_now(t.kid));
    }

    #[test]
    fn hide_on_an_idle_tree_starts() {
        let mut t = Tree::open("kwin", false);
        t.shown_with(pop());
        assert!(
            !t.shell.present_pacer(t.root).unwrap().0,
            "the pacer is idle"
        );
        let seq = t.next_seq();
        t.shell.set_mapped(t.kid, false).unwrap();
        t.committed();
        assert_eq!(t.buffers(seq, t.kid), 1, "the first tick");
        assert!(t.ticking());
    }

    #[test]
    fn set_motion_none_ends_a_running_leg() {
        // a show jumps to identity
        let mut t = Tree::open("kwin", false);
        t.kid_with(pop());
        t.draw_kid();
        t.committed();
        t.step();
        assert!(t.opacity() < 255);
        t.shell.set_motion(t.kid, None).unwrap();
        t.rec.present(STEP);
        t.until("the show's end", |t| t.opacity() == 255);
        assert!(!t.ticking());
        t.step();
        t.step();
        let seq = t.next_seq();
        t.step();
        t.step();
        assert!(t.commits(seq, t.kid).is_empty(), "and nothing goes after");
        // a hide unmaps in the next tree commit
        let mut t = Tree::open("kwin", false);
        t.shown_with(pop());
        t.shell.set_mapped(t.kid, false).unwrap();
        t.committed();
        t.step();
        assert!(t.mapped_now(t.kid));
        t.shell.set_motion(t.kid, None).unwrap();
        t.rec.wait_node(t.kid, |n| n.contains("\"mapped\":false"));
        assert!(!t.ticking());
        t.no_protocol_error();
    }

    /// An occluded tree runs no ticks; its leg ends by the clock, with no replay of what it missed.
    #[test]
    fn occluded_leg_ends_when_shown_again() {
        let mut t = Tree::open("mutter", false);
        t.shown_with(pop());
        t.shell.set_mapped(t.kid, false).unwrap();
        t.committed();
        t.step();
        t.step();
        assert!(t.mapped_now(t.kid));
        let r = t.rec.control(r#"{"cmd":"occlude"}"#);
        assert!(r.contains("\"ok\":true"), "{r}");
        for _ in 0..12 {
            t.rec.present(STEP);
        }
        let seq = t.next_seq();
        let r = t.rec.control(r#"{"cmd":"unocclude"}"#);
        assert!(r.contains("\"ok\":true"), "{r}");
        t.step();
        if t.mapped_now(t.kid) {
            t.step();
        }
        assert!(!t.mapped_now(t.kid), "the leg ended");
        assert!(!t.ticking());
        assert!(t.buffers(seq, t.kid) <= 1, "one last frame at most");
    }

    #[test]
    fn hide_show_one_commit_no_frames() {
        let mut t = Tree::open("kwin", true);
        t.shown_with(pop());
        let seq = t.next_seq();
        t.shell.set_mapped(t.kid, false).unwrap();
        t.shell.set_mapped(t.kid, true).unwrap();
        t.pump();
        assert!(!t.ticking());
        t.step();
        assert!(t.commits(seq, t.kid).is_empty());
        assert!(t.commits(seq, t.root).is_empty());
        assert!(t.mapped_now(t.kid));
    }

    /// A hide, then a commit: the presenter has had a turn since the hide.
    fn hide_then_a_turn(t: &mut Tree) -> u64 {
        t.shown_with(pop());
        let seq = t.next_seq();
        t.shell.set_mapped(t.kid, false).unwrap();
        t.rb.fill(Rect::new(0, 0, 10, 10), 0xff00_ff00);
        assert!(publish(&mut t.shell, t.root, &[(&t.rb, t.root)], 0));
        t.committed();
        seq
    }

    /// The presenter thread may wake between a hide and a show of one command: the hide waits, and costs no frame.
    #[test]
    fn hide_show_across_a_presenter_wake_no_frames() {
        // a hold far longer than the test's steps take on a starved machine
        let mut t = Tree::open("kwin", false);
        t.shell.set_hide_hold(5000);
        let seq = hide_then_a_turn(&mut t);
        assert_eq!(t.buffers(seq, t.root), 1, "the turn");
        assert_eq!(t.buffers(seq, t.kid), 0, "the hide waits");
        t.shell.set_mapped(t.kid, true).unwrap();
        t.step();
        t.until("no leg to tick", |t| !t.ticking());
        assert_eq!(t.buffers(seq, t.kid), 0, "no frame");
        assert_eq!(glitnir::stats::summary(t.kb.g, false).ticks, 0);
        assert!(t.mapped_now(t.kid));
        t.no_protocol_error();
        // without the hold the same calls run a leg
        let mut t = Tree::open("kwin", false);
        t.shell.set_hide_hold(0);
        let seq = hide_then_a_turn(&mut t);
        assert_eq!(t.buffers(seq, t.kid), 1, "the leg's first frame");
        t.shell.set_mapped(t.kid, true).unwrap();
        t.finish();
        assert!(glitnir::stats::summary(t.kb.g, false).ticks >= 1);
    }

    /// A hide that waits starts by itself, and a running show's frames do not wait for it.
    #[test]
    fn held_hide_lets_a_running_show_go_on() {
        let mut t = Tree::open("kwin", false);
        t.shell.set_hide_hold(100);
        t.kid_with(pop());
        t.draw_kid();
        t.committed();
        t.step();
        let seq = t.next_seq();
        t.shell.set_mapped(t.kid, false).unwrap();
        t.sync();
        t.rec.present(STEP);
        t.until("the show's next frame", |t| t.buffers(seq, t.kid) > 0);
        t.until("the unmap", |t| {
            t.rec.present(STEP);
            !t.mapped_now(t.kid)
        });
        assert!(!t.ticking());
        t.no_protocol_error();
    }

    #[test]
    fn no_spec_maps_and_unmaps_at_once() {
        let mut t = Tree::open("kwin", false);
        t.kid = t.child();
        let seq = t.next_seq();
        t.draw_kid();
        t.committed();
        assert_eq!(t.buffers(seq, t.kid), 1);
        assert_eq!(t.opacity(), 255, "shown whole in its first commit");
        t.step();
        let seq = t.next_seq();
        t.shell.set_mapped(t.kid, false).unwrap();
        t.rec.wait_node(t.kid, |n| n.contains("\"mapped\":false"));
        assert_eq!(t.buffers(seq, t.kid), 0, "no leg");
        assert!(!t.ticking());
        assert_eq!(glitnir::stats::summary(t.kb.g, false).ticks, 0);
    }

    #[test]
    fn idle_after_motion_no_commit() {
        let mut t = Tree::open("kwin", false);
        t.kid_with(pop());
        t.draw_kid();
        t.committed();
        t.finish();
        t.step();
        let seq = t.next_seq();
        let before = t.shell.presenter_stats().unwrap().commits;
        for _ in 0..20 {
            t.rec.present(STEP);
        }
        std::thread::sleep(Duration::from_millis(50));
        assert!(t.commits(seq, t.kid).is_empty());
        assert!(t.commits(seq, t.root).is_empty());
        assert_eq!(t.shell.presenter_stats().unwrap().commits, before);
    }

    /// Inline, the host's thread gets `FRAME_READY` once per period while a leg runs, and never after.
    #[test]
    fn inline_mode_wakes_per_period() {
        let mut t = Tree::open("kwin", true);
        t.shown_with(pop());
        let seq = t.next_seq();
        t.shell.set_mapped(t.kid, false).unwrap();
        assert_eq!(t.pump(), 1, "the first tick");
        for n in 2..=4 {
            t.sync();
            t.rec.present(STEP);
            let deadline = Instant::now() + Duration::from_secs(10);
            let mut ready = t.pump();
            while ready == 0 {
                assert!(Instant::now() < deadline, "no FRAME_READY");
                readable_within(t.shell.wake_fd(), 10);
                ready = t.pump();
            }
            assert_eq!(ready, 1, "one per period");
            t.sync();
            assert_eq!(t.buffers(seq, t.kid), n);
        }
        t.finish();
        t.rec.present(STEP);
        std::thread::sleep(Duration::from_millis(50));
        assert_eq!(t.pump(), 0, "none after the leg");
    }

    /// A picture at the content ack's old size must not go with the ack: while a content ack waits for its packet no
    /// tick is presented, and the packet takes the tick along in the tree's commit that carries the ack.
    #[test]
    fn tick_waits_for_a_held_content_ack() {
        let mut t = Tree::open("kwin", false);
        t.shown_with(pop());
        t.shell.set_mapped(t.kid, false).unwrap();
        t.committed();
        let r = t
            .rec
            .control(r#"{"cmd":"configure","surface":"focused","size":[300,150],"states":[]}"#);
        let serial = number(&r, "serial").expect("a serial") as u32;
        wait_for(&mut t.shell, |e| e.kind() == Kind::Configure);
        t.shell
            .ack_configure(t.root, serial, NHSHELL_ACK_CONTENT)
            .unwrap();
        let seq = t.next_seq();
        t.rec.present(STEP);
        let deadline = Instant::now() + Duration::from_secs(10);
        while t.shell.present_pacer(t.root).unwrap().0 {
            assert!(Instant::now() < deadline, "the frame callback never came");
            std::thread::sleep(Duration::from_millis(1));
        }
        std::thread::sleep(Duration::from_millis(50));
        assert_eq!(t.buffers(seq, t.kid), 0, "no tick while the ack is held");
        assert!(t.commits(seq, t.root).is_empty());
        t.rb.resize(300, 150);
        t.rb.fill_all(BLACK);
        assert!(publish(&mut t.shell, t.root, &[(&t.rb, t.root)], 0));
        t.committed();
        assert_eq!(t.buffers(seq, t.kid), 1, "the tick rides the ack's commit");
        let ack = format!("\"ack_serial\":{serial}");
        let root = t.commits(seq, t.root);
        assert!(root.iter().any(|c| c.contains(&ack)), "{root:?}");
        t.finish();
        assert!(!t.mapped_now(t.kid));
    }

    /// The child drawn and hidden again while the tree waits for its frame callback, so no frame has seen the child.
    fn hidden_before_its_first_frame(t: &mut Tree) {
        t.rb.fill(Rect::new(0, 0, 10, 10), 0xff00_ff00);
        assert!(publish(&mut t.shell, t.root, &[(&t.rb, t.root)], 0));
        t.committed();
        t.kid_with(pop());
        t.draw_kid();
        t.shell.set_mapped(t.kid, false).unwrap();
    }

    /// A hide that came before the first frame leaves nothing to show, and a later show starts as a first one does.
    #[test]
    fn hide_before_the_first_frame_shows_nothing() {
        let mut t = Tree::open("kwin", false);
        let seq = t.next_seq();
        hidden_before_its_first_frame(&mut t);
        t.rec.present(STEP);
        t.idle();
        std::thread::sleep(Duration::from_millis(50));
        t.sync();
        assert_eq!(t.buffers(seq, t.kid), 0, "a hidden child's picture");
        assert!(!t.ticking());
        t.shell.set_mapped(t.kid, true).unwrap();
        t.draw_kid();
        t.committed();
        assert_eq!(t.buffers(seq, t.kid), 1, "the show's first frame");
        t.finish();
        assert!(t.mapped_now(t.kid));
        t.no_protocol_error();
    }

    /// A frame that sat quiet past the pool's idle time still has its picture to fade.
    #[test]
    fn hide_after_a_quiet_period_fades() {
        let mut t = Tree::open("kwin", false);
        t.shown_with(pop());
        std::thread::sleep(Duration::from_millis(2300));
        assert!(
            t.shell.presenter_stats().unwrap().front_trims >= 1,
            "the root's"
        );
        let seq = t.next_seq();
        t.shell.set_mapped(t.kid, false).unwrap();
        t.committed();
        assert_eq!(t.buffers(seq, t.kid), 1);
        assert!(t.opacity() > 0, "the picture is there");
    }

    #[test]
    fn ghost_finishes_fade_then_destroyed() {
        let mut t = Tree::open("kwin", false);
        t.shown_with(pop());
        t.shell.set_mapped(t.kid, false).unwrap();
        t.committed();
        t.step();
        let seq = t.next_seq();
        t.shell.window_destroy(t.kid).unwrap();
        assert!(
            t.shell.window_ready(t.kid).is_err(),
            "the host's id is gone"
        );
        assert_eq!(t.rec.surfaces("subsurface"), 1, "the ghost");
        t.step();
        assert_eq!(t.rec.surfaces("subsurface"), 1);
        assert_eq!(t.buffers(seq, t.kid), 1, "the fade goes on");
        t.finish();
        let deadline = Instant::now() + Duration::from_secs(3);
        while t.rec.surfaces("subsurface") != 0 {
            assert!(Instant::now() < deadline, "a stuck ghost");
            std::thread::sleep(Duration::from_millis(5));
        }
        t.no_protocol_error();
        let seq = t.next_seq();
        t.step();
        assert!(t.commits(seq, t.root).is_empty(), "the tree is idle again");
    }

    #[test]
    fn ghost_dies_with_parent() {
        let mut t = Tree::open("kwin", false);
        t.shown_with(pop());
        t.shell.set_mapped(t.kid, false).unwrap();
        t.committed();
        t.shell.window_destroy(t.kid).unwrap();
        t.step();
        assert_eq!(t.rec.surfaces("subsurface"), 1, "the ghost");
        t.shell.window_destroy(t.root).unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        while t.rec.surfaces("subsurface") + t.rec.surfaces("toplevel") != 0 {
            assert!(Instant::now() < deadline, "a ghost outlived its parent");
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(t.shell.connection_state(), ConnectionState::Up);
        assert_eq!(t.shell.presenter_stats().unwrap().thread_on, 1);
    }

    /// The transport's half of a reconnect: the old connection is gone and the shell is back on the recorder.
    fn reconnected(t: &mut Tree) {
        let r = t.rec.control(r#"{"cmd":"restart","mode":"keep-socket"}"#);
        assert!(r.contains("\"ok\":true"), "{r}");
        wait_for(&mut t.shell, |e| e.kind() == Kind::Reconnecting);
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            match t.shell.reconnect(50) {
                Ok(()) => break,
                Err(ShellError::Timeout | ShellError::Reconnecting) => {}
                Err(e) => panic!("reconnect: {e:?}"),
            }
            assert!(Instant::now() < deadline, "no reconnect");
            std::thread::sleep(Duration::from_millis(10));
        }
        let ev = wait_for(&mut t.shell, |e| {
            e.kind() == Kind::Configure && e.window == t.root
        });
        t.shell
            .ack_configure(t.root, configure_of(&ev).serial, NHSHELL_ACK_CONTENT)
            .unwrap();
        t.rb.fill_all(BLACK);
        assert!(publish(&mut t.shell, t.root, &[(&t.rb, t.root)], 0));
        t.committed();
        t.step();
    }

    #[test]
    fn reconnect_drops_ghosts() {
        let rec = Recorder::start_refresh("sway", "manual", 120_000);
        let mut t = Tree::on(rec, true);
        t.shell.set_reconnect(true).unwrap();
        t.shown_with(pop());
        t.shell.set_mapped(t.kid, false).unwrap();
        t.pump();
        t.shell.window_destroy(t.kid).unwrap();
        t.rec.present(STEP);
        t.pump();
        reconnected(&mut t);
        assert_eq!(
            t.rec.surfaces("subsurface"),
            0,
            "no ghost on the new connection"
        );
        assert!(!t.ticking());
        t.no_protocol_error();
    }

    /// After a reconnect a window keeps its id on a surface with another protocol id, so its old id is free once it
    /// goes: a new window takes it while the ghost fades, and each keeps its own picture.
    #[test]
    fn ghost_id_reuse_is_safe() {
        let rec = Recorder::start_refresh("sway", "manual", 120_000);
        let mut t = Tree::on(rec, false);
        t.shell.set_reconnect(true).unwrap();
        // a high id, which the new connection's first objects stay below
        for _ in 0..60 {
            t.shell.surface_new().unwrap();
        }
        t.shown_with(pop());
        let old = t.kid;
        reconnected(&mut t);
        t.kb.fill_all(WHITE);
        assert!(publish(
            &mut t.shell,
            t.root,
            &[(&t.rb, t.root), (&t.kb, old)],
            0
        ));
        t.committed();
        t.step();
        t.shell.set_mapped(old, false).unwrap();
        t.committed();
        t.shell.window_destroy(old).unwrap();
        loop {
            let s = t.shell.surface_new().unwrap();
            assert!(s < old, "the new connection passed the old id: {s}, {old}");
            if s == old - 1 {
                break;
            }
        }
        t.kid = t.child();
        assert_eq!(t.kid, old, "the new window took the freed id");
        t.kb = Backing::new(SIZE.0, SIZE.1);
        t.kb.fill_all(0xff00_ff00);
        assert!(publish(
            &mut t.shell,
            t.root,
            &[(&t.rb, t.root), (&t.kb, t.kid)],
            0
        ));
        t.step();
        t.finish();
        let deadline = Instant::now() + Duration::from_secs(3);
        while t.rec.surfaces("subsurface") != 1 {
            assert!(Instant::now() < deadline, "a stuck ghost");
            std::thread::sleep(Duration::from_millis(5));
        }
        let snap = t.rec.snapshot();
        assert_eq!(
            Recorder::snapshot_pixel(&snap, MID.0, MID.1),
            0xff00_ff00,
            "the new window shows"
        );
        t.no_protocol_error();
    }

    /// The modifier stays where it was; the motion multiplies the pixels on top.
    #[test]
    fn static_alpha_kept_with_motion() {
        let mut t = Tree::open("kwin", false);
        t.shown_with(pop());
        let seq = t.next_seq();
        t.shell
            .set_alpha(t.kid, (u32::MAX as f64 * 0.8) as u32)
            .unwrap();
        t.until("the multiplier", |t| !t.commits(seq, t.kid).is_empty());
        let seq = t.next_seq();
        t.shell.set_mapped(t.kid, false).unwrap();
        t.committed();
        t.step();
        t.step();
        let log = t.commits(seq, t.kid);
        assert!(log.len() >= 3, "{log:?}");
        for c in &log {
            let at = c.find("\"alpha\":").expect("an alpha") + "\"alpha\":".len();
            let a: f64 = c[at..]
                .split(|ch: char| !ch.is_ascii_digit() && ch != '.')
                .next()
                .unwrap()
                .parse()
                .unwrap();
            assert!((a - 0.8).abs() < 0.01, "{c}");
        }
        // the buffer holds the leg's multiply alone
        let snap = t.rec.snapshot();
        let a = Recorder::snapshot_pixel(&snap, MID.0, MID.1) & 0xff;
        assert!(a < 255, "the pixels fade: {a}");
        t.finish();
        assert!(!t.mapped_now(t.kid));
    }
}

//! Show and hide transitions of a child frame's surface (fade, pop), as a state that is a function of time.
//!
//! The owner asks for a state ([`Motion::request`]) and the frame's commit settles it ([`Motion::commit`]): a request
//! that equals the committed state starts nothing, so a hide and a show between two commits cost nothing.  A leg
//! starts at the commit's time and moves the sample from where it is to its end state along the curve, so a show
//! during a hide (or the reverse) continues from the current opacity and takes the share of the duration that is left.
//! [`Motion::sample`] depends only on the time asked for, never on how often it was asked.

use crate::ease::Curve;

/// The scale a pop starts from and ends at.
pub const POP_SCALE: f32 = 0.96;

/// What a leg changes: nothing (immediate), the opacity, or the opacity and the scale.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    None,
    Fade,
    Pop,
}

/// One direction's transition.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Leg {
    pub kind: Kind,
    pub duration_ns: u64,
    pub curve: Curve,
}

/// The show and hide legs.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MotionSpec {
    pub show: Leg,
    pub hide: Leg,
}

/// The opacity (0 to 1) and the scale about the surface's centre at one moment.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Sample {
    pub opacity: f32,
    pub scale: f32,
}

impl Sample {
    pub const IDENTITY: Sample = Sample {
        opacity: 1.0,
        scale: 1.0,
    };

    fn lerp(a: Sample, b: Sample, y: f32) -> Sample {
        Sample {
            opacity: a.opacity + (b.opacity - a.opacity) * y,
            scale: a.scale + (b.scale - a.scale) * y,
        }
    }
}

/// A running leg: from `from` to `to` over `[start_ns, start_ns + duration_ns]`.
#[derive(Clone, Copy, Debug)]
struct Active {
    start_ns: u64,
    duration_ns: u64,
    curve: Curve,
    from: Sample,
    to: Sample,
}

/// A frame's show/hide state.
#[derive(Clone, Debug)]
pub struct Motion {
    spec: MotionSpec,
    /// The committed state: shown (or showing), or hidden (or hiding).
    shown: bool,
    /// Where the sample rests when no leg runs.
    rest: Sample,
    leg: Option<Active>,
    request: Option<bool>,
}

impl Motion {
    /// A frame that is shown, at identity.
    pub fn new(spec: MotionSpec) -> Motion {
        Motion {
            spec,
            shown: true,
            rest: Sample::IDENTITY,
            leg: None,
            request: None,
        }
    }

    /// The legs for the next transition; one that runs keeps its own.
    pub fn set_spec(&mut self, spec: MotionSpec) {
        self.spec = spec;
    }

    /// Ask for the frame to be mapped or unmapped, at the next [`commit`](Motion::commit).
    pub fn request(&mut self, mapped: bool) {
        self.request = Some(mapped);
    }

    /// The tree's commit at `now_ns`: start the leg the request calls for, or nothing when the request equals the
    /// committed state.
    pub fn commit(&mut self, now_ns: u64) {
        let Some(want) = self.request.take() else {
            return;
        };
        if want == self.shown {
            return;
        }
        let now = self.sample(now_ns);
        let mid = self.running(now_ns);
        let leg = if want { self.spec.show } else { self.spec.hide };
        // a hidden frame with a pop rests at the pop's small scale
        let hidden = Sample {
            opacity: 0.0,
            scale: if leg.kind == Kind::Pop {
                POP_SCALE
            } else {
                1.0
            },
        };
        let to = if want { Sample::IDENTITY } else { hidden };
        self.shown = want;
        self.rest = to;
        self.leg = None;
        if leg.kind == Kind::None || leg.duration_ns == 0 {
            return;
        }
        // mid-leg (or a hide of a shown frame) goes on from the sample; a show of a hidden frame starts from the
        // leg's own small end
        let from = if mid || !want { now } else { hidden };
        // the share of the duration that is left: from full to the target takes all of it
        let span = (to.opacity - from.opacity).abs().clamp(0.0, 1.0);
        let duration_ns = (leg.duration_ns as f64 * f64::from(span)).round() as u64;
        if duration_ns == 0 {
            return;
        }
        self.leg = Some(Active {
            start_ns: now_ns,
            duration_ns,
            curve: leg.curve,
            from,
            to,
        });
    }

    /// The opacity and scale at `t_ns`.  A function of `t_ns` and the committed legs only.
    pub fn sample(&self, t_ns: u64) -> Sample {
        let Some(a) = self.leg else {
            return self.rest;
        };
        if t_ns <= a.start_ns {
            return a.from;
        }
        let p = (t_ns - a.start_ns) as f64 / a.duration_ns as f64;
        if p >= 1.0 {
            return a.to;
        }
        Sample::lerp(a.from, a.to, a.curve.y(p as f32))
    }

    /// Whether a leg has not ended at `t_ns`.
    pub fn running(&self, t_ns: u64) -> bool {
        self.leg
            .is_some_and(|a| t_ns < a.start_ns.saturating_add(a.duration_ns))
    }

    /// How far `t_ns` lies past the end of the newest leg; `None` when none ran.
    pub fn overrun(&self, t_ns: u64) -> Option<u64> {
        self.leg
            .map(|a| t_ns.saturating_sub(a.start_ns.saturating_add(a.duration_ns)))
    }

    /// A hide has ended (or was immediate): the surface can be unmapped.
    pub fn unmap_due(&self, t_ns: u64) -> bool {
        !self.shown && !self.running(t_ns)
    }

    /// Forget a leg that has ended at `t_ns` (its `overrun` read): the frame rests where it ended, and samples, the
    /// next commit and `running` read the same as with the leg kept.
    pub fn settle(&mut self, t_ns: u64) {
        if !self.running(t_ns) {
            self.leg = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MS: u64 = 1_000_000;

    fn leg(kind: Kind, ms: u64) -> Leg {
        Leg {
            kind,
            duration_ns: ms * MS,
            curve: Curve::LINEAR,
        }
    }

    fn spec(show: Kind, hide: Kind) -> MotionSpec {
        MotionSpec {
            show: leg(show, 120),
            hide: leg(hide, 80),
        }
    }

    /// A hidden frame (hide leg done), ready for a show.
    fn hidden(s: MotionSpec) -> Motion {
        let mut m = Motion::new(s);
        m.request(false);
        m.commit(0);
        assert!(m.unmap_due(1_000 * MS));
        m
    }

    #[test]
    fn pop_in_reaches_identity_at_duration() {
        let mut m = hidden(spec(Kind::Pop, Kind::Fade));
        m.request(true);
        m.commit(1_000 * MS);
        let a = m.sample(1_000 * MS);
        assert_eq!((a.opacity, a.scale), (0.0, POP_SCALE));
        assert!(m.running(1_000 * MS + 119 * MS));
        let mid = m.sample(1_000 * MS + 60 * MS);
        assert!((mid.opacity - 0.5).abs() < 1e-5);
        assert!((mid.scale - (POP_SCALE + (1.0 - POP_SCALE) * 0.5)).abs() < 1e-5);
        let end = 1_000 * MS + 120 * MS;
        assert!(!m.running(end));
        assert_eq!(m.sample(end), Sample::IDENTITY);
        assert_eq!(m.sample(end + 5_000 * MS), Sample::IDENTITY);
        assert!(!m.unmap_due(end));
    }

    #[test]
    fn fade_out_reaches_zero_and_unmap_due() {
        let mut m = Motion::new(spec(Kind::Pop, Kind::Fade));
        m.request(false);
        m.commit(10 * MS);
        assert!(!m.unmap_due(10 * MS + 79 * MS));
        assert!(m.running(10 * MS + 79 * MS));
        let end = 10 * MS + 80 * MS;
        assert!(m.unmap_due(end));
        let s = m.sample(end);
        assert_eq!((s.opacity, s.scale), (0.0, 1.0));
    }

    #[test]
    fn hide_show_one_commit_starts_nothing() {
        let mut m = Motion::new(spec(Kind::Pop, Kind::Fade));
        m.request(false);
        m.request(true);
        m.commit(5 * MS);
        assert!(!m.running(5 * MS));
        assert!(!m.unmap_due(5 * MS));
        assert_eq!(m.sample(5 * MS), Sample::IDENTITY);
        // the same from hidden
        let mut h = hidden(spec(Kind::Pop, Kind::Fade));
        h.request(true);
        h.request(false);
        h.commit(2_000 * MS);
        assert!(!h.running(2_000 * MS));
        assert!(h.unmap_due(2_000 * MS));
    }

    #[test]
    fn show_during_hide_continues_from_current() {
        let mut m = Motion::new(spec(Kind::Pop, Kind::Pop));
        m.request(false);
        m.commit(0);
        let t = 40 * MS;
        let before = m.sample(t);
        assert!(before.opacity > 0.0 && before.opacity < 1.0);
        m.request(true);
        m.commit(t);
        let after = m.sample(t);
        assert!((after.opacity - before.opacity).abs() < 1e-5);
        assert!((after.scale - before.scale).abs() < 1e-5);
        // the way back takes the share that is left: (1 - 0.5) of 120 ms
        assert!(m.running(t + 59 * MS));
        assert!(!m.running(t + 60 * MS));
        assert_eq!(m.sample(t + 60 * MS), Sample::IDENTITY);
        // and the mirror: a hide during a show
        let mut n = hidden(spec(Kind::Fade, Kind::Fade));
        n.request(true);
        n.commit(0);
        let u = 30 * MS;
        let b = n.sample(u);
        n.request(false);
        n.commit(u);
        assert!((n.sample(u).opacity - b.opacity).abs() < 1e-5);
        let left = (80.0 * f64::from(b.opacity)).ceil() as u64;
        assert!(!n.unmap_due(u + (left - 1) * MS));
        assert!(n.unmap_due(u + (left + 1) * MS));
    }

    #[test]
    fn state_is_function_of_time() {
        let mut m = hidden(spec(Kind::Pop, Kind::Fade));
        m.request(true);
        m.commit(100 * MS);
        let t = 100 * MS + 37 * MS;
        let once = m.sample(t);
        let mut last = once;
        for i in 0..1000u64 {
            last = m.sample(100 * MS + i * 120 * MS / 1000);
        }
        let _ = last;
        assert_eq!(m.sample(t), once);
    }

    #[test]
    fn none_kind_is_immediate() {
        let mut m = Motion::new(spec(Kind::None, Kind::None));
        m.request(false);
        m.commit(7 * MS);
        assert!(!m.running(7 * MS));
        assert!(m.unmap_due(7 * MS));
        assert_eq!(m.sample(7 * MS).opacity, 0.0);
        m.request(true);
        m.commit(9 * MS);
        assert!(!m.running(9 * MS));
        assert!(!m.unmap_due(9 * MS));
        assert_eq!(m.sample(9 * MS), Sample::IDENTITY);
    }

    #[test]
    fn overrun_is_the_time_past_the_newest_leg() {
        let mut m = Motion::new(spec(Kind::Fade, Kind::None));
        assert_eq!(m.overrun(5 * MS), None);
        m.request(false);
        m.commit(0);
        assert_eq!(m.overrun(5 * MS), None, "an immediate hide runs no leg");
        m.request(true);
        m.commit(100 * MS);
        assert_eq!(m.overrun(150 * MS), Some(0));
        assert_eq!(m.overrun(223 * MS), Some(3 * MS));
    }

    #[test]
    fn set_spec_applies_to_the_next_leg_only() {
        let mut m = Motion::new(spec(Kind::Fade, Kind::Fade));
        m.request(false);
        m.commit(0);
        m.set_spec(spec(Kind::Fade, Kind::None));
        assert!(m.running(79 * MS));
        assert!(!m.running(80 * MS));
    }

    /// Settling forgets a leg once it has ended, not before, and the frame reads the same: at rest where the leg ended,
    /// and the next leg starts as it would have.
    #[test]
    fn settle_forgets_an_ended_leg_only() {
        let mut m = Motion::new(spec(Kind::Fade, Kind::Fade));
        m.request(false);
        m.commit(0);
        m.settle(40 * MS);
        assert!(m.running(40 * MS), "a leg that runs stays");
        let mut kept = m.clone();
        m.settle(80 * MS);
        assert_eq!(m.overrun(90 * MS), None, "forgotten");
        assert_eq!(kept.overrun(90 * MS), Some(10 * MS));
        assert_eq!(m.sample(90 * MS), kept.sample(90 * MS));
        assert!(!m.running(90 * MS) && m.unmap_due(90 * MS));
        for x in [&mut m, &mut kept] {
            x.request(true);
            x.commit(200 * MS);
        }
        assert_eq!(m.sample(260 * MS), kept.sample(260 * MS));
    }
}

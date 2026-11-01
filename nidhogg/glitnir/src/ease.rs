//! CSS easing curves (CSS Easing Level 1): a cubic Bézier through (0,0) and (1,1) with two control points, solved for
//! y at a given x.
//!
//! Adapted-from: https://github.com/WebKit/WebKit@029da7d3d074a75a24c2413e98b183a8bbfcfb62 Source/WebCore/platform/graphics/UnitBezier.h
//! (the solver: an 11-sample first guess, Newton's method, then bisection; rewritten in Rust, in f64 inside).
//! Copyright (C) 2008 Apple Inc. All rights reserved.  BSD 2-clause; the notice is kept with the project's licence texts.

const SAMPLES: usize = 11;
const NEWTON_EPSILON: f64 = 1e-7;
const BISECTION_EPSILON: f64 = 1e-10;
const NEWTON_STEPS: usize = 4;

/// A cubic Bézier easing curve, `cubic-bezier(x1, y1, x2, y2)`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Curve {
    x1: f32,
    y1: f32,
    x2: f32,
    y2: f32,
}

impl Curve {
    pub const LINEAR: Curve = Curve::raw(0.0, 0.0, 1.0, 1.0);
    pub const EASE: Curve = Curve::raw(0.25, 0.1, 0.25, 1.0);
    pub const EASE_IN: Curve = Curve::raw(0.42, 0.0, 1.0, 1.0);
    pub const EASE_OUT: Curve = Curve::raw(0.0, 0.0, 0.58, 1.0);
    pub const EASE_IN_OUT: Curve = Curve::raw(0.42, 0.0, 0.58, 1.0);

    const fn raw(x1: f32, y1: f32, x2: f32, y2: f32) -> Curve {
        Curve { x1, y1, x2, y2 }
    }

    /// The curve, or `None` unless x1 and x2 are in 0 to 1 (CSS), and the y values are finite.
    pub fn new(x1: f32, y1: f32, x2: f32, y2: f32) -> Option<Curve> {
        let ok = (0.0..=1.0).contains(&x1)
            && (0.0..=1.0).contains(&x2)
            && y1.is_finite()
            && y2.is_finite();
        ok.then_some(Curve::raw(x1, y1, x2, y2))
    }

    /// The curve's y at `x`; `x` is clamped to 0 to 1.  The error is at most 1e-4 (1e-7 in x before the f32 result).
    pub fn y(&self, x: f32) -> f32 {
        let x = f64::from(x).clamp(0.0, 1.0);
        let (x1, y1, x2, y2) = (
            f64::from(self.x1),
            f64::from(self.y1),
            f64::from(self.x2),
            f64::from(self.y2),
        );
        let cx = 3.0 * x1;
        let bx = 3.0 * (x2 - x1) - cx;
        let ax = 1.0 - cx - bx;
        let cy = 3.0 * y1;
        let by = 3.0 * (y2 - y1) - cy;
        let ay = 1.0 - cy - by;
        let sx = |t: f64| ((ax * t + bx) * t + cx) * t;
        let dx = |t: f64| (3.0 * ax * t + 2.0 * bx) * t + cx;

        // the first guess: linear interpolation inside the sample interval that holds x
        let step = 1.0 / (SAMPLES - 1) as f64;
        let (mut t0, mut t1, mut t2) = (0.0, 1.0, x);
        let mut prev = 0.0;
        for i in 1..SAMPLES {
            let s = sx(step * i as f64);
            if x <= s {
                t1 = step * i as f64;
                t0 = t1 - step;
                t2 = if s > prev {
                    t0 + step * (x - prev) / (s - prev)
                } else {
                    t0
                };
                break;
            }
            prev = s;
        }
        let t = 'solve: {
            // Newton's method, normally very fast
            for _ in 0..NEWTON_STEPS {
                let e = sx(t2) - x;
                if e.abs() < NEWTON_EPSILON {
                    break 'solve t2;
                }
                let d = dx(t2);
                if d.abs() < NEWTON_EPSILON {
                    break;
                }
                t2 -= e / d;
            }
            // bisection, for reliability
            for _ in 0..64 {
                let e = sx(t2);
                if (e - x).abs() < BISECTION_EPSILON {
                    break;
                }
                if x > e {
                    t0 = t2;
                } else {
                    t1 = t2;
                }
                t2 = 0.5 * (t0 + t1);
            }
            t2
        };
        (((ay * t + by) * t + cy) * t) as f32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SET: [Curve; 5] = [
        Curve::LINEAR,
        Curve::EASE,
        Curve::EASE_IN,
        Curve::EASE_OUT,
        Curve::EASE_IN_OUT,
    ];

    #[test]
    fn css_curves_at_control_points() {
        for c in SET {
            assert!(c.y(0.0).abs() < 1e-6, "{c:?}");
            assert!((c.y(1.0) - 1.0).abs() < 1e-6, "{c:?}");
        }
        assert!((Curve::EASE_OUT.y(0.5) - 0.6846).abs() < 1e-3);
        assert!((Curve::LINEAR.y(0.3) - 0.3).abs() < 1e-5);
    }

    #[test]
    fn y_is_monotonic_for_css_set() {
        for c in SET {
            let mut last = -1.0;
            for i in 0..=1000 {
                let y = c.y(i as f32 / 1000.0);
                assert!(y >= last - 1e-6, "{c:?} at {i}");
                last = y;
            }
        }
    }

    #[test]
    fn rejects_x_outside_unit() {
        assert!(Curve::new(-0.1, 0.0, 0.5, 1.0).is_none());
        assert!(Curve::new(0.1, 0.0, 1.1, 1.0).is_none());
        assert!(Curve::new(f32::NAN, 0.0, 0.5, 1.0).is_none());
        assert!(Curve::new(0.0, 0.0, 0.58, 1.0).is_some());
        // y may overshoot
        assert!(Curve::new(0.3, -0.5, 0.7, 1.5).is_some());
    }

    #[test]
    fn solver_matches_bisection_reference() {
        // an independent check: find t by plain bisection on x(t), then take y(t)
        for c in SET {
            let b = |p1: f64, p2: f64, t: f64| {
                3.0 * (1.0 - t) * (1.0 - t) * t * p1 + 3.0 * (1.0 - t) * t * t * p2 + t * t * t
            };
            for i in 0..=100 {
                let x = i as f64 / 100.0;
                let (mut lo, mut hi) = (0.0, 1.0);
                for _ in 0..60 {
                    let m = 0.5 * (lo + hi);
                    if b(f64::from(c.x1), f64::from(c.x2), m) < x {
                        lo = m;
                    } else {
                        hi = m;
                    }
                }
                let want = b(f64::from(c.y1), f64::from(c.y2), 0.5 * (lo + hi));
                assert!((f64::from(c.y(x as f32)) - want).abs() < 1e-4, "{c:?} {x}");
            }
        }
    }
}

//! A child's show and hide transitions (design §5.7) as Core Animation runs them: the container's model values are set
//! to the transition's end first (shown, or hidden; opacity and transform at rest), and `CABasicAnimation`s on
//! `opacity` and `transform` carry the picture there, so a leg needs no redisplay, no commit and no timer of ours.
//! Glitnir's [`Motion`] decides each leg (where it starts, how long it runs), as it does for the Wayland presenter.
//!
//! A hide sets the model hidden at once and adds an animation that holds `hidden` off for the leg, so the layer is
//! hidden the moment its fade ends and the hit-test (which reads the host's mapping) ignores it from the start.  A show
//! from hidden unhides the layer in the transaction that adds its animations, so its first composited frame is the
//! leg's start (rule L2).  When the system asks for reduced motion no leg runs.

use glitnir::motion::{Motion, MotionSpec, Sample};
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_app_kit::NSWorkspace;
use objc2_foundation::{NSNumber, NSString, NSValue};
use objc2_quartz_core::{
    CABasicAnimation, CACurrentMediaTime, CALayer, CAMediaTiming, CAMediaTimingFunction,
    CATransform3D, NSValueCATransform3DAdditions,
};
use std::sync::atomic::{AtomicI32, Ordering};

/// Test hook: -1 reads the system's reduced-motion setting, 0 and 1 stand for it.
pub static REDUCED_OVERRIDE: AtomicI32 = AtomicI32::new(-1);

/// Whether the system asks for reduced motion (`NSWorkspace`; it keeps the value current from its own notification).
pub fn reduced() -> bool {
    match REDUCED_OVERRIDE.load(Ordering::Relaxed) {
        -1 => NSWorkspace::sharedWorkspace().accessibilityDisplayShouldReduceMotion(),
        v => v != 0,
    }
}

/// The last leg a child ran, as its info query reports it.
#[derive(Clone, Copy, Debug)]
#[cfg_attr(not(feature = "test-hooks"), allow(dead_code))]
pub struct Ran {
    pub show: bool,
    pub from: Sample,
    pub to: Sample,
    pub seconds: f64,
}

/// A child's transitions: its spec's state (`None`: no transitions) and the last leg.
#[derive(Debug, Default)]
pub struct State {
    motion: Option<Motion>,
    pub last: Option<Ran>,
}

impl State {
    /// The host's spec (`None`: none; a running leg is left to end).  `hidden` is what the container shows now.
    pub fn set_spec(&mut self, spec: Option<MotionSpec>, hidden: bool) {
        match (spec, self.motion.as_mut()) {
            (None, _) => self.motion = None,
            (Some(s), Some(m)) => m.set_spec(s),
            (Some(s), None) => {
                let mut m = Motion::new(s);
                if hidden {
                    // at rest, hidden: a hide whose leg ended long ago
                    m.request(false);
                    m.commit(0);
                    m.settle(u64::MAX);
                }
                self.motion = Some(m);
            }
        }
    }

    /// Show or hide `container` (`w` x `h` points); the model values are the end state.  Whether a leg runs.
    pub fn transition(&mut self, container: &CALayer, hidden: bool, w: f64, h: f64) -> bool {
        let now = CACurrentMediaTime();
        let now_ns = (now * 1e9) as u64;
        let leg = self.motion.as_mut().and_then(|m| {
            m.request(!hidden);
            m.commit(now_ns);
            m.leg().filter(|l| l.0 == now_ns && l.1 > 0)
        });
        container.setHidden(hidden);
        container.setTransform(scale_about_centre(1.0, w, h));
        let leg = match leg {
            Some(l) if !reduced() => l,
            _ => {
                remove(container);
                return false;
            }
        };
        let (_, dur_ns, curve, from, to) = leg;
        let seconds = dur_ns as f64 / 1e9;
        let [x1, y1, x2, y2] = curve.points();
        let timing = CAMediaTimingFunction::functionWithControlPoints(x1, y1, x2, y2);
        let alpha = container.opacity();
        // the leg starts now in the layer's time, so a reader can tell a running animation from one Core Animation has
        // ended but not yet taken off (it does so at a later transaction)
        let begin = container.convertTime_fromLayer(now, None);
        let anim = |key: &str, a: &AnyObject, b: &AnyObject| {
            let an = CABasicAnimation::animationWithKeyPath(Some(&NSString::from_str(key)));
            // SAFETY: values of the key path's type (NSNumber for opacity and hidden, NSValue of CATransform3D).
            unsafe {
                an.setFromValue(Some(a));
                an.setToValue(Some(b));
            }
            an.setBeginTime(begin);
            an.setDuration(seconds);
            an.setTimingFunction(Some(&timing));
            container.addAnimation_forKey(&an, Some(&NSString::from_str(key)));
        };
        let num = |v: f32| NSNumber::new_f32(v);
        anim(
            "opacity",
            &num(from.opacity * alpha),
            &num(to.opacity * alpha),
        );
        if from.scale != 1.0 || to.scale != 1.0 {
            let t = |s: f32| -> Retained<NSValue> {
                // SAFETY: a plain struct boxed in an NSValue.
                unsafe { NSValue::valueWithCATransform3D(scale_about_centre(f64::from(s), w, h)) }
            };
            anim("transform", &t(from.scale), &t(to.scale));
        } else {
            container.removeAnimationForKey(&NSString::from_str("transform"));
        }
        if hidden {
            let shown = NSNumber::new_bool(false);
            anim("hidden", &shown, &shown);
        } else {
            container.removeAnimationForKey(&NSString::from_str("hidden"));
        }
        self.last = Some(Ran {
            show: !hidden,
            from,
            to,
            seconds,
        });
        super::super::layers::count(&super::super::layers::PROPS.legs);
        true
    }
}

/// Take the transitions' animations off `container` (a change with no leg jumps to its end).
fn remove(container: &CALayer) {
    for k in ["opacity", "transform", "hidden"] {
        container.removeAnimationForKey(&NSString::from_str(k));
    }
}

/// Scale `s` about the centre of a `w` x `h` layer whose anchor is its bottom-left corner.
fn scale_about_centre(s: f64, w: f64, h: f64) -> CATransform3D {
    CATransform3D {
        m11: s,
        m12: 0.0,
        m13: 0.0,
        m14: 0.0,
        m21: 0.0,
        m22: s,
        m23: 0.0,
        m24: 0.0,
        m31: 0.0,
        m32: 0.0,
        m33: 1.0,
        m34: 0.0,
        m41: (1.0 - s) * w / 2.0,
        m42: (1.0 - s) * h / 2.0,
        m43: 0.0,
        m44: 1.0,
    }
}

#[cfg_attr(not(feature = "test-hooks"), allow(dead_code))]
/// The keys of `container`'s animations still running (begun, not past their end, in the layer's time).
pub fn running(container: &CALayer) -> Vec<String> {
    let now = container.convertTime_fromLayer(CACurrentMediaTime(), None);
    container.animationKeys().map_or_else(Vec::new, |keys| {
        keys.iter()
            .filter(|k| {
                // SAFETY: a key the layer just listed; the animation is only read.
                unsafe { container.animationForKey(k) }
                    .is_some_and(|a| a.beginTime() + a.duration() > now)
            })
            .map(|k| k.to_string())
            .collect()
    })
}

//! A root's layout legs (`layout_capture`, `layout_leg`, `layout_captured`) on Core Animation: the capture is a copy
//! of the text area's pixels in the newest surface, taken when the host asks (before it draws the new layout); the leg
//! starts in the transaction of the root's first commit that shows a new picture after it came, as layers over the
//! root's contents (under its cards and children), and Core Animation runs it, so it needs no redisplay, commit or
//! timer of ours (as a child's show and hide legs, `motion.rs`).
//!
//! The layers draw what `glitnir::layout::compose_leg` draws for the Wayland presenter, bottom to top over the new
//! picture: the new places of the moves and enters filled with the background; the capture with the old places of the
//! moves and exits filled the same way, its opacity going from 1 to 0 on the leg's curve (`step`: at the midpoint), so
//! the background cross-fades; each exit's old picture fading out over [`EXIT_NS`]; each enter's new picture fading in
//! over the last [`ENTER_NS`]; each move's new picture going from its start to its new place on the curve.  The leg's
//! layer is hidden at its end by an animation that holds it shown until then, so the last frame is the plain picture;
//! the layer goes at the next commit or call after that, or at once when the host ends the leg.

use super::chrome::{CGImageRelease, argb_image};
use crate::units::Scale;
use glitnir::geom::Rect;
use glitnir::layout::{ENTER_NS, EXIT_NS, ItemKind, LayoutLeg};
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_foundation::{NSNumber, NSPoint, NSRect, NSSize, NSString, NSValue};
use objc2_quartz_core::{
    CABasicAnimation, CACurrentMediaTime, CALayer, CAMediaTiming, CAMediaTimingFunction,
    kCAFillModeBackwards, kCAFilterNearest,
};
use std::collections::BTreeMap;

/// One window's capture and leg.
#[derive(Default)]
struct Window {
    /// The newest capture's number (0: none), its rectangle (root device pixels) and pixels, row by row.
    captured: u64,
    old: Option<(Rect, Vec<u32>)>,
    /// Legs whose capture was not the newest held.
    skipped: u64,
    /// The leg waiting for the root's next picture.
    pending: Option<LayoutLeg>,
    /// The running leg's layer and its end (`CACurrentMediaTime`).
    running: Option<(Retained<CALayer>, f64)>,
}

/// A root's windows' legs, by window key.
#[derive(Default)]
pub struct Legs {
    windows: BTreeMap<u64, Window>,
    /// What happened to the legs, newest last (test hook `test-legs`).
    log: Vec<String>,
}

/// The pixels of `r` (inside `src`'s rectangle `at`) as a buffer of their own.
fn crop(src: &[u32], at: Rect, r: Rect) -> Vec<u32> {
    let mut out = Vec::with_capacity(r.area() as usize);
    for y in r.y..r.y + r.h {
        let o = ((y - at.y) * at.w + (r.x - at.x)) as usize;
        out.extend_from_slice(&src[o..o + r.w as usize]);
    }
    out
}

/// A layer showing `px` (`w` x `h`) at `r`, device pixels in the leg's layer of height `h_pt` points; `None` when the
/// image cannot be made.
fn image_layer(
    px: &[u32],
    w: i32,
    h: i32,
    r: Rect,
    h_pt: f64,
    scale: Scale,
) -> Option<Retained<CALayer>> {
    let i = argb_image(px, w as u32, h as u32);
    if i.is_null() {
        return None;
    }
    let l = CALayer::new();
    l.setAnchorPoint(NSPoint::new(0.0, 0.0));
    // SAFETY: a CGImage is an object `contents` takes; released once the layer retained it.  Core Animation's filter
    // names.
    unsafe {
        l.setContents(Some(&*i.cast::<AnyObject>()));
        CGImageRelease(i);
        l.setMagnificationFilter(kCAFilterNearest);
        l.setMinificationFilter(kCAFilterNearest);
    }
    l.setContentsScale(scale.factor());
    place(&l, r, h_pt, scale);
    Some(l)
}

/// `l`'s model place: `r` in device pixels from the top left of a layer `h_pt` points tall.
fn place(l: &CALayer, r: Rect, h_pt: f64, scale: Scale) {
    let (x, y, w, h) = points(r, h_pt, scale);
    l.setPosition(NSPoint::new(x, y));
    l.setBounds(NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(w, h)));
}

fn points(r: Rect, h_pt: f64, scale: Scale) -> (f64, f64, f64, f64) {
    scale.view_rect(
        (
            f64::from(r.x),
            f64::from(r.y),
            f64::from(r.w),
            f64::from(r.h),
        ),
        h_pt,
    )
}

/// Animate `key` of `l` from `a` to `b` (the model holds `b`), from `begin` (the layer's time) for `secs`; before its
/// begin the layer shows `a`.
fn animate(
    l: &CALayer,
    key: &str,
    (a, b): (&AnyObject, &AnyObject),
    begin: f64,
    secs: f64,
    timing: Option<&CAMediaTimingFunction>,
) {
    let an = CABasicAnimation::animationWithKeyPath(Some(&NSString::from_str(key)));
    // SAFETY: values of the key path's type (NSNumber for opacity and hidden, NSValue of a point or a rectangle);
    // Core Animation's fill mode name.
    unsafe {
        an.setFromValue(Some(a));
        an.setToValue(Some(b));
        an.setFillMode(kCAFillModeBackwards);
    }
    an.setBeginTime(begin);
    an.setDuration(secs.max(1e-3));
    an.setTimingFunction(timing);
    l.addAnimation_forKey(&an, Some(&NSString::from_str(key)));
}

fn num(v: f32) -> Retained<NSNumber> {
    NSNumber::new_f32(v)
}

impl Legs {
    fn note(&mut self, line: String) {
        if self.log.len() >= 32 {
            self.log.remove(0);
        }
        self.log.push(line);
    }

    /// Capture `n` of window `key`: `rect`'s pixels in `old` (the newest surface's, `None` when it has none there).
    pub fn capture(&mut self, key: u64, n: u64, rect: Rect, old: Option<Vec<u32>>) {
        let ok = old.is_some();
        let w = self.windows.entry(key).or_default();
        if let Some(px) = old {
            w.captured = n;
            w.old = Some((rect, px));
        }
        self.note(format!(
            "capture {key} {n} {rect:?} {}",
            if ok { "taken" } else { "no-surface" }
        ));
    }

    /// Window `key`'s leg (`None`: end it).  A leg that is not from the newest capture held is skipped.  Whether a
    /// running leg's layer must go now.
    pub fn leg(&mut self, key: u64, leg: Option<LayoutLeg>) -> Option<Retained<CALayer>> {
        let w = self.windows.entry(key).or_default();
        let Some(leg) = leg else {
            w.pending = None;
            let gone = w.running.take().map(|(l, _)| l);
            self.note(format!(
                "end {key}{}",
                if gone.is_some() { " running" } else { "" }
            ));
            return gone;
        };
        let held = w.old.as_ref().is_some_and(|(r, _)| *r == leg.rect);
        let line = if leg.capture == w.captured && held {
            let items = leg.items.len();
            w.pending = Some(leg);
            format!("leg {key} waits for the next picture ({items} items)")
        } else {
            w.skipped += 1;
            format!(
                "leg {key} skipped: from capture {} with {} held{}",
                leg.capture,
                w.captured,
                if held {
                    ""
                } else {
                    " (another rectangle, or none)"
                }
            )
        };
        self.note(line);
        None
    }

    /// What the presenter did with window `key`'s capture and leg.
    pub fn ack(&self, key: u64) -> crate::api::LayoutAck {
        let w = self.windows.get(&key);
        crate::api::LayoutAck {
            captured: w.map_or(0, |w| w.captured),
            skipped: w.map_or(0, |w| w.skipped),
            running: w.is_some_and(|w| {
                w.pending.is_some()
                    || w.running
                        .as_ref()
                        .is_some_and(|(_, end)| CACurrentMediaTime() < *end)
            }),
        }
    }

    /// Inside the root tree's commit: ended legs' layers go; with a new picture (`drawn`), the waiting legs start on
    /// `root` (the root's layer, `h_pt` points tall), their new pixels read by `new` (a rectangle of the picture).
    pub fn commit(
        &mut self,
        root: &CALayer,
        drawn: bool,
        h_pt: f64,
        scale: Scale,
        new: impl Fn(Rect) -> Option<Vec<u32>>,
    ) {
        let now = CACurrentMediaTime();
        let mut lines = Vec::new();
        for (key, w) in &mut self.windows {
            if w.running.as_ref().is_some_and(|(_, end)| now >= *end) {
                if let Some((l, _)) = w.running.take() {
                    l.removeFromSuperlayer();
                }
                lines.push(format!("ended {key}"));
            }
            if !drawn {
                continue;
            }
            let Some(leg) = w.pending.take() else {
                continue;
            };
            let Some((at, old)) = w.old.as_ref() else {
                continue;
            };
            let Some(px) = new(leg.rect) else {
                lines.push(format!(
                    "leg {key} not started: no new picture of {:?}",
                    leg.rect
                ));
                continue;
            };
            if let Some((l, _)) = w.running.take() {
                l.removeFromSuperlayer();
            }
            match start(root, &leg, (*at, old), &px, h_pt, scale) {
                Some(l) => {
                    let secs = leg.duration_ns as f64 / 1e9;
                    lines.push(format!(
                        "started {key}: {:?}, {} items, {secs} s{}",
                        leg.rect,
                        leg.items.len(),
                        if leg.step { ", step" } else { "" }
                    ));
                    w.running = Some((l, now + secs));
                }
                None => lines.push(format!("leg {key} not started: no image")),
            }
        }
        for l in lines {
            self.note(l);
        }
    }

    /// The legs' state and log (test hook): `((KEY CAPTURED SKIPPED PENDING RUNNING) ...)` and the log's lines.
    #[cfg_attr(not(feature = "test-hooks"), allow(dead_code))]
    pub fn text(&self) -> String {
        let now = CACurrentMediaTime();
        let ws: Vec<String> = self
            .windows
            .iter()
            .map(|(k, w)| {
                format!(
                    "({k} {} {} {} {})",
                    w.captured,
                    w.skipped,
                    if w.pending.is_some() { "t" } else { "nil" },
                    if w.running.as_ref().is_some_and(|(_, e)| now < *e) {
                        "t"
                    } else {
                        "nil"
                    }
                )
            })
            .collect();
        let log: Vec<String> = self.log.iter().map(|l| format!("{l:?}")).collect();
        format!("(:windows ({}) :log ({}))", ws.join(" "), log.join(" "))
    }
}

/// Start `leg` on `root` from the capture `old` (its rectangle and pixels) to the new picture's pixels `new` of the
/// leg's rectangle: the leg's layer, added under the root's cards and children, or `None`.
fn start(
    root: &CALayer,
    leg: &LayoutLeg,
    (at, old): (Rect, &[u32]),
    new: &[u32],
    h_pt: f64,
    scale: Scale,
) -> Option<Retained<CALayer>> {
    let r = leg.rect;
    let local = |x: Rect| x.intersect(&r).translate(-r.x, -r.y);
    let lh = scale.pt(f64::from(r.h));
    let bg = [leg.background];
    let secs = leg.duration_ns as f64 / 1e9;
    let [x1, y1, x2, y2] = leg.curve.points();
    let curve = CAMediaTimingFunction::functionWithControlPoints(x1, y1, x2, y2);

    let clip = CALayer::new();
    clip.setAnchorPoint(NSPoint::new(0.0, 0.0));
    clip.setMasksToBounds(true);
    place(&clip, r, h_pt, scale);
    let begin = clip.convertTime_fromLayer(CACurrentMediaTime(), None);
    let fill = |x: Rect| {
        let x = local(x);
        (!x.is_empty())
            .then(|| image_layer(&bg, 1, 1, x, lh, scale))
            .flatten()
    };
    let rect0 = Rect::new(0, 0, r.w, r.h);

    // the new side of the background: the new places filled
    for it in leg.items.iter().filter(|i| i.kind != ItemKind::Exit) {
        if let Some(l) = fill(it.to) {
            clip.addSublayer(&l);
        }
    }
    // the old side: the capture with the old places filled, fading to the new on the curve
    let mut back = crop(old, at, r);
    for it in leg.items.iter().filter(|i| i.kind != ItemKind::Enter) {
        let f = local(it.from);
        for y in f.y..f.y + f.h {
            let o = (y * r.w + f.x) as usize;
            back[o..o + f.w as usize].fill(leg.background);
        }
    }
    let back = image_layer(&back, r.w, r.h, rect0, lh, scale)?;
    back.setOpacity(0.0);
    if leg.step {
        // the cross-fade switches at the midpoint
        animate(
            &back,
            "opacity",
            (&num(1.0), &num(1.0)),
            begin,
            secs / 2.0,
            None,
        );
    } else {
        animate(
            &back,
            "opacity",
            (&num(1.0), &num(0.0)),
            begin,
            secs,
            Some(&curve),
        );
    }
    clip.addSublayer(&back);
    let d_ns = leg.duration_ns;
    let rect_of = |x: Rect| -> Retained<NSValue> {
        let (_, _, w, h) = points(x, lh, scale);
        NSValue::new(NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(w, h)))
    };
    let point_of = |x: Rect| -> Retained<NSValue> {
        let (px, py, _, _) = points(x, lh, scale);
        NSValue::new(NSPoint::new(px, py))
    };
    for kind in [ItemKind::Exit, ItemKind::Enter, ItemKind::Move] {
        for it in leg.items.iter().filter(|i| i.kind == kind) {
            let (src, at_src, from) = match kind {
                ItemKind::Exit => (old, at, it.from),
                _ => (new, r, it.to),
            };
            let s = from.intersect(&at_src).intersect(&r);
            if s.is_empty() {
                continue;
            }
            let px = crop(src, at_src, s);
            let Some(l) = image_layer(&px, s.w, s.h, local(s), lh, scale) else {
                continue;
            };
            match kind {
                ItemKind::Exit => {
                    l.setOpacity(0.0);
                    let t = EXIT_NS.min(d_ns) as f64 / 1e9;
                    animate(&l, "opacity", (&num(1.0), &num(0.0)), begin, t, None);
                }
                ItemKind::Enter => {
                    let t = ENTER_NS.min(d_ns) as f64 / 1e9;
                    animate(
                        &l,
                        "opacity",
                        (&num(0.0), &num(1.0)),
                        begin + secs - t,
                        t,
                        None,
                    );
                }
                ItemKind::Move => {
                    let to = local(it.to);
                    let st = it.start.translate(-r.x, -r.y);
                    animate(
                        &l,
                        "position",
                        (&point_of(st), &point_of(to)),
                        begin,
                        secs,
                        Some(&curve),
                    );
                    animate(
                        &l,
                        "bounds",
                        (&rect_of(st), &rect_of(to)),
                        begin,
                        secs,
                        Some(&curve),
                    );
                }
            }
            clip.addSublayer(&l);
        }
    }
    // shown for the leg, hidden at its end without a commit
    clip.setHidden(true);
    let shown = NSNumber::new_bool(false);
    animate(&clip, "hidden", (&shown, &shown), begin, secs, None);
    // over the root's contents, under its cards and every child's container
    root.insertSublayer_atIndex(&clip, 0);
    super::layers::count(&super::layers::PROPS.legs);
    Some(clip)
}

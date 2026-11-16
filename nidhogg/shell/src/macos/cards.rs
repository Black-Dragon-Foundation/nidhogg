//! A root's window cards on layers of their own: each card is an image the glue drew alone (its shadow and ring in the
//! gap, a group's fill, its corners showing the gap), put over the root's content and under its children's
//! containers.  `set_cards` keeps the list; the root tree's next commit applies it, in the transaction that shows the
//! root's picture, so a card and the text it frames change together.  A card that keeps its image is not touched.

use super::chrome::{CGImageRelease, argb_image};
use crate::api::{CardImage, ShellError, ShellResult};
use crate::units::Scale;
use objc2::Message;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_foundation::{NSPoint, NSRect, NSSize};
use objc2_quartz_core::{CAEdgeAntialiasingMask, CALayer, kCAFilterNearest};
use std::collections::BTreeMap;

/// A card's place in its root's surface: x, y, width, height, device pixels.
type Place = (i32, i32, u32, u32);

/// The part `shown` of a card at `rect` (device px from the top-left) as a unit rectangle of its image from the
/// bottom-left (a layer's `contentsRect`); all of it without `shown`.
fn contents_rect(rect: Place, shown: Option<Place>) -> NSRect {
    let Some((sx, sy, sw, sh)) = shown else {
        return NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(1.0, 1.0));
    };
    let (x, y, w, h) = rect;
    let (w, h) = (f64::from(w.max(1)), f64::from(h.max(1)));
    let bottom = f64::from(y) + h - (f64::from(sy) + f64::from(sh));
    NSRect::new(
        NSPoint::new(f64::from(sx - x) / w, bottom / h),
        NSSize::new(f64::from(sw) / w, f64::from(sh) / h),
    )
}

/// A window's cards (a root's window cards, and its windows' blocks; a child's blocks): the layers shown, by card id,
/// the layer they are in, and the list waiting for the next commit.
#[derive(Default)]
pub struct Cards {
    /// Each card's layer, place and the part of the place that shows (`None`: all).
    shown: BTreeMap<u64, (Retained<CALayer>, Place, Option<Place>)>,
    host: Option<Retained<CALayer>>,
    pending: Option<Vec<CardImage>>,
    /// Images set on a layer since the window opened (test hook).
    #[cfg_attr(not(feature = "test-hooks"), allow(dead_code))]
    pub images: u64,
    /// Each shown card's last image, by id (test hook: `test-card-row`).
    #[cfg(feature = "test-hooks")]
    pub last: BTreeMap<u64, std::sync::Arc<[u32]>>,
}

impl Cards {
    /// The host's list, checked and kept for the next commit.
    pub fn set(&mut self, list: &[CardImage]) -> ShellResult {
        for c in list {
            // the part that shows lies inside the card
            if let Some((sx, sy, sw, sh)) = c.shown {
                let (x, y, w, h) = c.rect;
                let end = |a: i32, n: u32| i64::from(a) + i64::from(n);
                if sx < x || sy < y || end(sx, sw) > end(x, w) || end(sy, sh) > end(y, h) {
                    return Err(ShellError::Arg);
                }
            }
            match &c.argb {
                Some(px) if px.len() != c.rect.2 as usize * c.rect.3 as usize => {
                    return Err(ShellError::Arg);
                }
                None if !self.shown.contains_key(&c.id)
                    && !self
                        .pending
                        .iter()
                        .flatten()
                        .any(|p| p.id == c.id && p.argb.is_some()) =>
                {
                    return Err(ShellError::Arg);
                }
                _ => {}
            }
        }
        // an image kept (None) that the list before had not shown yet is still to be shown: it carries over
        let before = self.pending.take().unwrap_or_default();
        self.pending = Some(
            list.iter()
                .map(|c| match &c.argb {
                    None => CardImage {
                        argb: before
                            .iter()
                            .find(|p| p.id == c.id)
                            .and_then(|p| p.argb.clone()),
                        ..c.clone()
                    },
                    Some(_) => c.clone(),
                })
                .collect(),
        );
        Ok(())
    }

    /// Apply the waiting list to `root` (the root's layer, `h` points tall) at `scale`: inside the root tree's
    /// transaction.
    pub fn apply(&mut self, root: &CALayer, h: f64, scale: Scale) {
        // a window that changed hosts (a child made top-level, or the reverse): its layers move to the new one
        if !self
            .host
            .as_ref()
            .is_some_and(|l| std::ptr::eq(Retained::as_ptr(l), root))
        {
            for (l, ..) in self.shown.values() {
                l.removeFromSuperlayer();
                root.insertSublayer_atIndex(l, 0);
            }
            self.host = Some(root.retain());
        }
        let Some(list) = self.pending.take() else {
            return;
        };
        let keep: Vec<u64> = list.iter().map(|c| c.id).collect();
        self.shown.retain(|id, (l, ..)| {
            let kept = keep.contains(id);
            if !kept {
                l.removeFromSuperlayer();
            }
            kept
        });
        #[cfg(feature = "test-hooks")]
        {
            let shown = &self.shown;
            self.last.retain(|id, _| shown.contains_key(id));
        }
        for c in &list {
            let (l, at, cut) = self.shown.entry(c.id).or_insert_with(|| {
                let l = CALayer::new();
                l.setAnchorPoint(NSPoint::new(0.0, 0.0));
                // the image is the card's device pixels at a device-pixel place: shown as they are, never filtered
                // or blended at the layer's edge with what is beneath (its corners hold the gap's exact colour)
                l.setAllowsEdgeAntialiasing(false);
                l.setEdgeAntialiasingMask(CAEdgeAntialiasingMask::empty());
                // SAFETY: Core Animation's filter name constants.
                unsafe {
                    l.setMagnificationFilter(kCAFilterNearest);
                    l.setMinificationFilter(kCAFilterNearest);
                }
                // over the root's contents, under every child's container
                root.insertSublayer_atIndex(&l, 0);
                (l, (0, 0, 0, 0), None)
            });
            if let Some(px) = &c.argb {
                let i = argb_image(px, c.rect.2, c.rect.3);
                if !i.is_null() {
                    // SAFETY: a CGImage is an object `contents` takes; released once the layer retained it.
                    unsafe {
                        l.setContents(Some(&*i.cast::<AnyObject>()));
                        CGImageRelease(i);
                    }
                    l.setContentsScale(scale.factor());
                    self.images += 1;
                    #[cfg(feature = "test-hooks")]
                    self.last.insert(c.id, px.clone());
                }
            }
            if *at != c.rect || *cut != c.shown || c.argb.is_some() {
                // the layer is the part that shows; its contents the same part of the image (`contentsRect`, unit
                // space from the bottom-left), so a card cut at an edge needs no new image
                let (x, y, w, hh) = c.shown.unwrap_or(c.rect);
                let (px, py, pw, ph) =
                    scale.view_rect((f64::from(x), f64::from(y), f64::from(w), f64::from(hh)), h);
                l.setPosition(NSPoint::new(px, py));
                l.setBounds(NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(pw, ph)));
                l.setContentsRect(contents_rect(c.rect, c.shown));
                *at = c.rect;
                *cut = c.shown;
            }
        }
        // stacked in the list's order, each just above the one before (the cards, the blocks, then the edges, which
        // overlap the blocks): a layer made later went in beneath the others
        let mut below: Option<&Retained<CALayer>> = None;
        for c in &list {
            let Some((l, ..)) = self.shown.get(&c.id) else {
                continue;
            };
            if let Some(b) = below {
                root.insertSublayer_above(l, Some(b));
            }
            below = Some(l);
        }
    }

    /// The cards shown: id, place and the part of it that shows (device pixels), in id order (test hook).
    #[cfg_attr(not(feature = "test-hooks"), allow(dead_code))]
    pub fn shown(&self) -> Vec<(u64, Place, Option<Place>)> {
        self.shown
            .iter()
            .map(|(id, (_, at, cut))| (*id, *at, *cut))
            .collect()
    }

    /// Card `id`'s last image's row `y` from column `x0`, `n` pixels, as #xAARRGGBB words (test hook).
    #[cfg(feature = "test-hooks")]
    pub fn row(&self, id: u64, y: u32, x0: u32, n: u32) -> Option<Vec<u32>> {
        let (_, (_, _, w, h), _) = self.shown.get(&id)?;
        let px = self.last.get(&id)?;
        (y < *h && x0 < *w).then(|| {
            let start = (y * w + x0) as usize;
            px[start..start + n.min(w - x0) as usize].to_vec()
        })
    }
}

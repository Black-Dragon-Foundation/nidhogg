//! A root's window cards on layers of their own: each card is an image the glue drew alone (its shadow and ring in the
//! gap, a group's fill, its corners showing the gap), put over the root's content and under its children's
//! containers.  `set_cards` keeps the list; the root tree's next commit applies it, in the transaction that shows the
//! root's picture, so a card and the text it frames change together.  A card that keeps its image is not touched.

use super::chrome::{CGImageRelease, argb_image};
use crate::api::{CardImage, ShellError, ShellResult};
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_foundation::{NSPoint, NSRect, NSSize};
use objc2_quartz_core::CALayer;
use std::collections::BTreeMap;

/// A card's place in its root's surface: x, y, width, height, device pixels.
type Place = (i32, i32, u32, u32);

/// A root's cards: the layers shown, by card id, and the list waiting for the next commit.
#[derive(Default)]
pub struct Cards {
    shown: BTreeMap<u64, (Retained<CALayer>, Place)>,
    pending: Option<Vec<CardImage>>,
    /// Images set on a layer since the window opened (test hook).
    #[cfg_attr(not(feature = "test-hooks"), allow(dead_code))]
    pub images: u64,
}

impl Cards {
    /// The host's list, checked and kept for the next commit.
    pub fn set(&mut self, list: &[CardImage]) -> ShellResult {
        for c in list {
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
        self.pending = Some(list.to_vec());
        Ok(())
    }

    /// Apply the waiting list to `root` (the root's layer, `h` points tall) at `scale`: inside the root tree's
    /// transaction.
    pub fn apply(&mut self, root: &CALayer, h: f64, scale: f64) {
        let Some(list) = self.pending.take() else {
            return;
        };
        let keep: Vec<u64> = list.iter().map(|c| c.id).collect();
        self.shown.retain(|id, (l, _)| {
            let kept = keep.contains(id);
            if !kept {
                l.removeFromSuperlayer();
            }
            kept
        });
        for c in &list {
            let (l, at) = self.shown.entry(c.id).or_insert_with(|| {
                let l = CALayer::new();
                l.setAnchorPoint(NSPoint::new(0.0, 0.0));
                // over the root's contents, under every child's container
                root.insertSublayer_atIndex(&l, 0);
                (l, (0, 0, 0, 0))
            });
            if let Some(px) = &c.argb {
                let i = argb_image(px, c.rect.2, c.rect.3);
                if !i.is_null() {
                    // SAFETY: a CGImage is an object `contents` takes; released once the layer retained it.
                    unsafe {
                        l.setContents(Some(&*i.cast::<AnyObject>()));
                        CGImageRelease(i);
                    }
                    l.setContentsScale(scale);
                    self.images += 1;
                }
            }
            if *at != c.rect || c.argb.is_some() {
                let (x, y, w, hh) = c.rect;
                let (pw, ph) = (f64::from(w) / scale, f64::from(hh) / scale);
                // device pixels from the top-left to points from the bottom-left
                l.setPosition(NSPoint::new(
                    f64::from(x) / scale,
                    h - f64::from(y) / scale - ph,
                ));
                l.setBounds(NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(pw, ph)));
                *at = c.rect;
            }
        }
    }

    /// The cards shown: id and place (device pixels), in id order (test hook).
    #[cfg_attr(not(feature = "test-hooks"), allow(dead_code))]
    pub fn shown(&self) -> Vec<(u64, Place)> {
        self.shown.iter().map(|(id, (_, at))| (*id, *at)).collect()
    }
}

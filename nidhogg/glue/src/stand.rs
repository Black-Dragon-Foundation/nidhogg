//! The image worker's stand-ins on Naglfar.  While an image's decode runs on the worker, `image.c` may draw the
//! picture of a ready entry in its place, scaled into the pending entry's box (the worker's stand-in rule).  On
//! Cairo the stand-in is a pattern sharing the ready entry's surface; here it is a copy of the ready entry's pixels
//! as Core Graphics draws them, kept by the pending entry until it lands or goes, so it outlives the ready entry as
//! the shared surface does.  It is drawn through the matrix the Cairo branch composes: the box's scale, then the ready
//! entry's own matrix.  Platform-free; drawn on macOS.

use crate::image::{Matrix, NhgPix};
use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard};

/// A stand-in: premultiplied words (the ready container's size), the matrix from the pending box to them, and
/// whether it draws smoothed (the ready entry's choice).
#[derive(Debug, Clone, PartialEq)]
pub struct Stand {
    pub words: Vec<u32>,
    pub width: i32,
    pub height: i32,
    pub matrix: Matrix,
    pub smooth: bool,
}

/// The stand-ins, by the pending entry they are drawn for (`image.c`'s `struct image` address).
static STANDS: Mutex<Option<HashMap<usize, Stand>>> = Mutex::new(None);

fn stands() -> MutexGuard<'static, Option<HashMap<usize, Stand>>> {
    STANDS.lock().unwrap_or_else(|e| e.into_inner())
}

/// The Cairo branch's stand-in matrix: a point of the pending box (`img_w` x `img_h`) scaled into the ready entry's
/// drawn box (`ready_w` x `ready_h`), then through the ready entry's matrix `ready`.  `None` for an empty box.
pub fn matrix(
    ready: &Matrix,
    ready_w: i32,
    ready_h: i32,
    img_w: i32,
    img_h: i32,
) -> Option<Matrix> {
    if ready_w <= 0 || ready_h <= 0 || img_w <= 0 || img_h <= 0 {
        return None;
    }
    let (sx, sy) = (
        f64::from(ready_w) / f64::from(img_w),
        f64::from(ready_h) / f64::from(img_h),
    );
    let [xx, yx, xy, yy, x0, y0] = *ready;
    Some([xx * sx, yx * sx, xy * sy, yy * sy, x0, y0])
}

/// Make `key`'s stand-in from the ready container `pix` (its bytes `data` and `mask`; `fg` an 8-bit container's
/// colour), drawn `ready_w` x `ready_h`, for a pending box `img_w` x `img_h`; it replaces the one `key` had.  The
/// words of the one it replaces are returned, for the shell to forget.  `None`, keeping no stand-in, when the
/// container or a box is refused.
#[allow(clippy::too_many_arguments)]
pub fn set(
    key: usize,
    pix: &NhgPix,
    data: &[u8],
    mask: Option<&[u8]>,
    fg: u32,
    ready: (i32, i32),
    img: (i32, i32),
) -> (Option<()>, Option<Stand>) {
    let old = take(key);
    let Some(m) = matrix(&pix.matrix, ready.0, ready.1, img.0, img.1) else {
        return (None, old);
    };
    let Some(words) = crate::image::combine(pix, data, mask, fg) else {
        return (None, old);
    };
    let stand = Stand {
        words,
        width: pix.width,
        height: pix.height,
        matrix: m,
        smooth: pix.smooth != 0,
    };
    stands().get_or_insert_with(HashMap::new).insert(key, stand);
    (Some(()), old)
}

/// Take `key`'s stand-in away (it landed, is prepared again, or is freed); the stand-in it had, if any.
pub fn take(key: usize) -> Option<Stand> {
    stands().as_mut()?.remove(&key)
}

/// `f` on `key`'s stand-in, if it has one.
pub fn with<R>(key: usize, f: impl FnOnce(&Stand) -> R) -> Option<R> {
    stands().as_ref()?.get(&key).map(f)
}

#[cfg(test)]
mod tests {
    use super::*;

    const IDENTITY: Matrix = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];

    fn pix(data: &[u8], w: i32, h: i32, matrix: Matrix) -> NhgPix {
        NhgPix {
            data: data.as_ptr(),
            mask: std::ptr::null(),
            width: w,
            height: h,
            stride: w * 4,
            mask_stride: 0,
            bpp: 32,
            smooth: 1,
            matrix,
        }
    }

    #[test]
    fn the_box_scales_first_then_the_ready_matrix() {
        // twice the size: half a container pixel a box pixel
        assert_eq!(
            matrix(&IDENTITY, 10, 20, 20, 40),
            Some([0.5, 0.0, 0.0, 0.5, 0.0, 0.0])
        );
        // a 10x20 container turned a quarter, drawn 20x10 (x' = y, y' = 20 - x): the box's scale comes before the turn
        let turned = [0.0, -1.0, 1.0, 0.0, 0.0, 20.0];
        let m = matrix(&turned, 20, 10, 40, 20).unwrap();
        let at = |x: f64, y: f64| (m[0] * x + m[2] * y + m[4], m[1] * x + m[3] * y + m[5]);
        // the box's corners land on the container's, as the ready box's do
        assert_eq!(at(40.0, 20.0), (10.0, 0.0));
        assert_eq!(at(0.0, 0.0), (0.0, 20.0));
        assert_eq!(matrix(&IDENTITY, 0, 1, 1, 1), None);
        assert_eq!(matrix(&IDENTITY, 1, 1, 1, 0), None);
    }

    #[test]
    fn a_stand_in_is_kept_until_it_is_dropped_or_replaced() {
        let key = 0x5a5a_0001;
        let words = [0x00ff_0000u32, 0x0000_ff00];
        let data: Vec<u8> = words.iter().flat_map(|w| w.to_ne_bytes()).collect();
        let p = pix(&data, 2, 1, IDENTITY);
        let (made, old) = set(key, &p, &data, None, 0, (2, 1), (4, 2));
        assert_eq!((made, old), (Some(()), None));
        // opaque without a mask, as the container draws
        let s = with(key, Clone::clone).unwrap();
        assert_eq!(s.words, vec![0xffff_0000, 0xff00_ff00]);
        assert_eq!((s.width, s.height, s.smooth), (2, 1, true));
        assert_eq!(s.matrix, [0.5, 0.0, 0.0, 0.5, 0.0, 0.0]);
        // its own copy: the ready container may go
        drop(data);
        assert_eq!(with(key, |s| s.words.len()), Some(2));
        // a second stand-in replaces the first and hands it back
        let data2 = [0u8; 4];
        let (made, old) = set(
            key,
            &pix(&data2, 1, 1, IDENTITY),
            &data2,
            None,
            0,
            (1, 1),
            (1, 1),
        );
        assert_eq!(made, Some(()));
        assert_eq!(old.unwrap().words.len(), 2);
        assert_eq!(take(key).unwrap().words, vec![0xff00_0000]);
        assert_eq!(take(key), None);
        assert_eq!(with(key, |_| ()), None);
    }

    #[test]
    fn a_refused_box_keeps_no_stand_in() {
        let key = 0x5a5a_0002;
        let data = [0u8; 4];
        let p = pix(&data, 1, 1, IDENTITY);
        assert_eq!(set(key, &p, &data, None, 0, (1, 1), (1, 1)).0, Some(()));
        // an empty pending box: the old one goes and none is kept
        let (made, old) = set(key, &p, &data, None, 0, (1, 1), (0, 1));
        assert_eq!(made, None);
        assert!(old.is_some());
        assert_eq!(with(key, |_| ()), None);
        // bytes too short for the container
        assert_eq!(
            set(
                key,
                &pix(&data, 2, 1, IDENTITY),
                &data,
                None,
                0,
                (2, 1),
                (2, 1)
            )
            .0,
            None
        );
    }
}

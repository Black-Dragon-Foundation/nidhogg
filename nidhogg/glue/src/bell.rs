//! The bell, idle inhibition and the handles other programs name a window by.
//!
//! - **The system bell** rings on the root surface of the frame's tree: a child frame has no surface of its own to
//!   ring on, so the walk up its parents ends at the top-level.
//! - **The visible bell** is `pgtk_flash`'s (emacs-31.1): the middle quarter is not flashed, the top and bottom line
//!   are (the whole frame when it is short).  It inverts those rows in the backing and damages them
//!   (`Backings::flash`); C puts them back 50 ms later (`Backings::unflash`) with an atimer of its own.  A draw that
//!   lands on the rows between the two takes its pixels from the restore, so the bell never undoes new drawing.
//! - **Idle inhibition** holds an inhibitor on the window's own surface while its frame parameter is set.
//! - **The export handle and the activation token** are those of the tree's root: only a top-level can be exported
//!   or activated.

use crate::abi::{NHG_E_TOO_SMALL, NHG_OK};
use crate::backing::NhgRect;
use crate::display::Display;
use crate::frame::NHG_FRAME_CHILD;
use crate::guard::GlueError;
use std::ffi::c_char;

/// How many parents a walk to the root follows (a loop in the facts ends it).
const MAX_DEPTH: usize = 32;

/// The frame facts the visible bell's rows come from, as C reads them.  Layout equals `NhgBellGeom`.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct NhgBellGeom {
    /// `FRAME_PIXEL_WIDTH`, `FRAME_PIXEL_HEIGHT`: the native area.
    pub width: i32,
    pub height: i32,
    /// `FRAME_LINE_HEIGHT`, `FRAME_INTERNAL_BORDER_WIDTH`, `FRAME_TOP_MARGIN_HEIGHT`, `FRAME_BOTTOM_MARGIN_HEIGHT`.
    pub line_h: i32,
    pub border: i32,
    pub top_margin: i32,
    pub bottom_margin: i32,
}

/// The rows to invert: the top and bottom line of a tall frame, the whole of a short one (`height <= 3 * line_h`).
pub fn flash_rects(g: &NhgBellGeom) -> Vec<NhgRect> {
    let left = g.border;
    let width = g.width - 2 * g.border;
    if g.height > 3 * g.line_h {
        vec![
            NhgRect {
                x: left,
                y: g.border + g.top_margin,
                w: width,
                h: g.line_h,
            },
            NhgRect {
                x: left,
                y: g.height - g.line_h - g.border - g.bottom_margin,
                w: width,
                h: g.line_h,
            },
        ]
    } else {
        vec![NhgRect {
            x: left,
            y: g.border,
            w: width,
            h: g.height - 2 * g.border,
        }]
    }
}

/// The top-level of `window`'s tree: `window` itself unless it is a child frame, then its parent's root.
pub fn root_of(state: &Display, window: u64) -> Result<u64, GlueError> {
    let mut cur = window;
    for _ in 0..MAX_DEPTH {
        let f = state.frame(cur)?;
        let parent = f.child.parent();
        if f.flags & NHG_FRAME_CHILD == 0 || parent == 0 || state.frame(parent).is_err() {
            return Ok(cur);
        }
        cur = parent;
    }
    Err(GlueError::State)
}

/// Ring the system bell on the root surface of `window`'s tree (0: no surface).
pub fn ring(state: &mut Display, window: u64) -> Result<(), GlueError> {
    let root = if window == 0 {
        0
    } else {
        root_of(state, window)?
    };
    Ok(state.backend().bell(root)?)
}

/// Invert the visible bell's rows in the backing of the frame with key `key`.  Returns the rows inverted.
pub fn flash(state: &mut Display, key: u64, g: &NhgBellGeom) -> Result<i32, GlueError> {
    if g.width <= 0 || g.height <= 0 || g.line_h <= 0 || g.border < 0 {
        return Err(GlueError::Arg);
    }
    Ok(state.backings.flash(key, &flash_rects(g))?)
}

/// Hold (`on`) or release the idle inhibitor of `window`.  True when that changed it.
pub fn inhibit_idle(state: &mut Display, window: u64, on: bool) -> Result<bool, GlueError> {
    if state.frame(window)?.idle_inhibited == on {
        return Ok(false);
    }
    state.backend().inhibit_idle(window, on)?;
    state.frame_mut(window)?.idle_inhibited = on;
    Ok(true)
}

/// The export handle of `window`'s root (no scheme prefix), waiting at most `timeout_ms`.
pub fn export(state: &mut Display, window: u64, timeout_ms: u32) -> Result<String, GlueError> {
    let root = root_of(state, window)?;
    Ok(state.backend().window_export(root, timeout_ms)?)
}

/// A fresh activation token made on `window`'s root, within `timeout_ms`.
pub fn token(state: &mut Display, window: u64, timeout_ms: u32) -> Result<String, GlueError> {
    let root = root_of(state, window)?;
    Ok(state.backend().activation_token(root, timeout_ms)?)
}

/// Copy `text` and a NUL into `buf` (`cap` bytes) and its length into `*len`.  `NHG_E_TOO_SMALL` (with the length
/// still written) when it does not fit.
///
/// # Safety
/// `buf` is null or holds `cap` writable bytes; `len` is null or writable.
pub unsafe fn put(
    text: &str,
    buf: *mut c_char,
    cap: usize,
    len: *mut usize,
) -> Result<i32, GlueError> {
    if !len.is_null() {
        // SAFETY: non-null and writable (the caller's promise).
        unsafe { len.write(text.len()) };
    }
    if buf.is_null() || text.len() >= cap {
        return Err(GlueError::Code(NHG_E_TOO_SMALL));
    }
    // SAFETY: `text.len() + 1 <= cap` bytes are writable, and `text` does not overlap them.
    unsafe {
        std::ptr::copy_nonoverlapping(text.as_ptr().cast::<c_char>(), buf, text.len());
        buf.add(text.len()).write(0);
    }
    Ok(NHG_OK)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn geom(height: i32) -> NhgBellGeom {
        NhgBellGeom {
            width: 400,
            height,
            line_h: 20,
            border: 4,
            top_margin: 6,
            bottom_margin: 2,
        }
    }

    #[test]
    fn a_tall_frame_flashes_its_top_and_bottom_line() {
        let r = flash_rects(&geom(200));
        assert_eq!(
            r,
            vec![
                NhgRect {
                    x: 4,
                    y: 10,
                    w: 392,
                    h: 20
                },
                NhgRect {
                    x: 4,
                    y: 200 - 20 - 4 - 2,
                    w: 392,
                    h: 20
                },
            ]
        );
    }

    #[test]
    fn a_short_frame_flashes_all_of_it() {
        assert_eq!(
            flash_rects(&geom(60)),
            vec![NhgRect {
                x: 4,
                y: 4,
                w: 392,
                h: 52
            }]
        );
    }
}

//! Menu popups: a popup frame's window, an xdg popup of its parent frame's window placed by the frame's `nhwl-popup`
//! parameter; its size and place while it is shown; its end, by Lisp or by the compositor; and the keyboard focus its
//! top-level lends it while it grabs.  Moved from `src/nhwlmenu.c` (`nhwl_popup_show`, `popup_anchor`,
//! `nhwl_popup_set_size`, `nhwl_popup_params_changed`, `nhwl_popup_grabbing`, `nhwl_popup_focus_kept` and the window
//! and focus half of `nhwl_popup_hide`) and from the C drain's `POPUP_DONE` case.  What reads Emacs stays C: the
//! parameter's decoding, the frame's size, which frame is a popup frame, the top-level a popup hangs from, hiding the
//! popup frames that hang from a hidden one first, and what losing the focus means to Emacs (`nhwl_focus_lost`).
//!
//! A popup frame's window exists while the frame is shown, and the frame is the root of its own tree (its pacer and
//! acks are its own).  The parameter's size, gravity, adjust and grab apply at the next show; a new parameter moves
//! the shown popup.  Keyboard focus moving between a top-level and its own popups is no focus change for Emacs: a
//! popup's focus events give no action, and a top-level that loses the keyboard while a popup of its own grabs has
//! lent it, and keeps Emacs's focus until it comes back or the popup goes.

use crate::action::{NHG_ACT_DELETE, NhgAction};
use crate::backing::NhgRect;
use crate::csd;
use crate::display::Display;
use crate::frame::{NHG_FACT_VISIBLE, NHG_FRAME_POPUP};
use crate::guard::GlueError;
use crate::window::{self, NhgAnchor, scale, to_logical};
use nhshell::api::{
    ChildDesc, NHSHELL_ADJUST_FLIP_X, NHSHELL_ADJUST_FLIP_Y, NHSHELL_ADJUST_SLIDE_X,
    NHSHELL_ADJUST_SLIDE_Y, NHSHELL_KIND_POPUP, NHSHELL_WIN_GRAB,
};
use nhshell::event::Record;

/// `nhg_popup_show` and `nhg_popup_place` flags: the popup takes the keyboard and pointer, and a click elsewhere
/// dismisses it.
pub const NHG_POPUP_GRAB: u32 = 1;

/// `DELETE` flags: the compositor dismissed a popup frame's popup (the frame stays until Lisp deletes or hides it, on
/// the `delete-frame` event).
pub const NHG_DEL_POPUP: u32 = 2;

const ADJUST_ALL: u32 =
    NHSHELL_ADJUST_FLIP_X | NHSHELL_ADJUST_FLIP_Y | NHSHELL_ADJUST_SLIDE_X | NHSHELL_ADJUST_SLIDE_Y;

/// Where a popup goes, as C decoded the frame's `nhwl-popup` parameter: the rectangle it sits beside (device pixels of
/// the parent's native area), the side or corner it grows toward (`NhgAnchor`'s gravity, 1 to 8), what the compositor
/// may adjust (`NHSHELL_ADJUST_*`) and `NHG_POPUP_*`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Place {
    pub anchor: NhgRect,
    pub gravity: u32,
    pub adjust: u32,
    pub flags: u32,
}

/// A shown popup frame's state (kept with its window's frame).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Popup {
    /// The parent frame's window.
    pub parent: u64,
    /// The window of the top-level it hangs from (through popup and child frames).
    pub owner: u64,
    /// The parameter as last given: the next comparison's base.
    pub place: Place,
    /// The window's size in logical pixels as last sent.
    pub sent: (i32, i32),
}

/// `place` in the parent's surface at scale `s120`, in logical pixels: the parent's native area starts after its
/// border (`offsets`, its title strip and margins).
fn anchor(s120: u32, offsets: [i32; 4], place: &Place) -> Result<NhgAnchor, GlueError> {
    if !(1..=8).contains(&place.gravity) || place.adjust & !ADJUST_ALL != 0 {
        return Err(GlueError::Arg);
    }
    let a = place.anchor;
    let x = offsets[0].saturating_add(a.x);
    let y = offsets[1].saturating_add(a.y);
    let (x0, y0) = (to_logical(x, s120), to_logical(y, s120));
    Ok(NhgAnchor {
        x: x0,
        y: y0,
        w: (to_logical(x.saturating_add(a.w.max(0)), s120) - x0).max(1),
        h: (to_logical(y.saturating_add(a.h.max(0)), s120) - y0).max(1),
        gravity: place.gravity,
        adjust: place.adjust,
    })
}

/// `place` in `parent`'s surface, at the parent's scale (as it is now).
fn anchor_in(state: &Display, parent: u64, place: &Place) -> Result<NhgAnchor, GlueError> {
    let s = scale(state.frame(parent)?);
    anchor(s, csd::offsets(state, parent)?, place)
}

/// Show a popup frame: a new popup of `parent`'s window at `place`, `native` device pixels at the parent's scale,
/// registered as a visible popup frame hanging from top-level `owner`; its id.  A popup asks for a grab with
/// `NHG_POPUP_GRAB`, which the shell sends only with a recent press's serial (no serial: no grab).
pub fn show(
    state: &mut Display,
    parent: u64,
    owner: u64,
    place: &Place,
    (native_w, native_h): (i32, i32),
) -> Result<u64, GlueError> {
    if native_w <= 0 || native_h <= 0 {
        return Err(GlueError::Arg);
    }
    let s = scale(state.frame(parent)?);
    let a = anchor_in(state, parent, place)?;
    let sent = (
        to_logical(native_w, s).max(1),
        to_logical(native_h, s).max(1),
    );
    let id = state.backend().child_new(&ChildDesc {
        kind: NHSHELL_KIND_POPUP,
        parent,
        x: 0,
        y: 0,
        w: sent.0,
        h: sent.1,
        anchor: a,
        flags: if place.flags & NHG_POPUP_GRAB != 0 {
            NHSHELL_WIN_GRAB
        } else {
            0
        },
    })?;
    let seeded = window::seed(
        state,
        id,
        NHG_FRAME_POPUP,
        NHG_FACT_VISIBLE,
        (native_w, native_h),
        s,
    );
    if let Err(e) = seeded {
        let _ = state.backend().window_destroy(id);
        return Err(e);
    }
    state.frame_mut(id)?.popup = Some(Popup {
        parent,
        owner,
        place: *place,
        sent,
    });
    Ok(id)
}

/// The frame's parameter is now `place`: unchanged, nothing (0); otherwise it is kept for the comparisons, and the
/// shown popup moves to its anchor (`xdg_popup.reposition`), 1 when the whole frame must be drawn again (below
/// `xdg_wm_base` 3 the popup is made again).  Size, gravity, adjust and grab apply at the next show.
pub fn place(state: &mut Display, window: u64, place: &Place) -> Result<i32, GlueError> {
    let p = state.frame(window)?.popup.ok_or(GlueError::State)?;
    if p.place == *place {
        return Ok(0);
    }
    let a = anchor_in(state, p.parent, place)?;
    if let Some(p) = state.frame_mut(window)?.popup.as_mut() {
        p.place = *place;
    }
    Ok(i32::from(state.backend().set_position(window, a.x, a.y)?))
}

/// The popup frame's size is now `w` x `h` device pixels: the popup takes it, placed again by its anchor; 1 when the
/// whole frame must be drawn again (the popup was made again, below `xdg_wm_base` 3), 0 when not or when the size in
/// logical pixels did not change.
pub fn size(state: &mut Display, window: u64, w: i32, h: i32) -> Result<i32, GlueError> {
    let f = state.frame(window)?;
    let p = f.popup.ok_or(GlueError::State)?;
    let s = scale(f);
    let sent = (to_logical(w, s).max(1), to_logical(h, s).max(1));
    if sent == p.sent {
        return Ok(0);
    }
    let remade = state.backend().set_size(window, sent.0, sent.1)?;
    if let Some(p) = state.frame_mut(window)?.popup.as_mut() {
        p.sent = sent;
    }
    Ok(i32::from(remade))
}

/// Hide a popup frame: its window (0: none) goes and is forgotten, as a window goes even when the shell refuses.  1
/// when top-level `owner` had lent its keyboard focus to a grabbing popup, no popup of its own grabs now, and the
/// keyboard is on none of our windows: the top-level has lost the focus (the compositor dismissed the menu while the
/// focus went elsewhere), and C tells Emacs.  `NHG_E_STATE` for a registered window that is not a popup frame's; a
/// window the glue does not know is passed over, as C passed over a refused destroy.
pub fn hide(state: &mut Display, window: u64, owner: u64) -> Result<i32, GlueError> {
    match state.frame(window) {
        Ok(f) if f.flags & NHG_FRAME_POPUP == 0 => return Err(GlueError::State),
        Ok(_) => {
            let _ = window::destroy(state, window);
        }
        Err(_) => {}
    }
    let lent = state.frame(owner).is_ok_and(|f| f.focus_lent);
    if !lent || state.keyboard.window != 0 || grabbing(state, owner) {
        return Ok(0);
    }
    state.frame_mut(owner)?.focus_lent = false;
    Ok(1)
}

/// Whether a shown popup frame hanging from top-level `owner` holds a grab (asked for, granted, not dismissed).
pub fn grabbing(state: &mut Display, owner: u64) -> bool {
    let shown: Vec<u64> = state
        .frames()
        .filter(|(_, f)| {
            f.facts.flags & NHG_FACT_VISIBLE != 0
                && f.popup
                    .is_some_and(|p| p.owner == owner && p.place.flags & NHG_POPUP_GRAB != 0)
        })
        .map(|(w, _)| w)
        .collect();
    shown
        .into_iter()
        .any(|w| matches!(state.backend().window_grabbed(w), Ok(true)))
}

/// A keyboard focus event (`on`: in) for `window`: true when Emacs sees no focus change.  A popup frame's focus is its
/// top-level's; a top-level that loses the keyboard while a popup frame of its own grabs has lent it to that popup,
/// and keeps Emacs's focus.  Focus coming back ends the loan.
pub fn focus_kept(state: &mut Display, window: u64, on: bool) -> bool {
    let Ok(f) = state.frame(window) else {
        return false;
    };
    if f.flags & NHG_FRAME_POPUP != 0 {
        return true;
    }
    let kept = !on && grabbing(state, window);
    if let Ok(f) = state.frame_mut(window) {
        if on {
            f.focus_lent = false;
        } else if kept {
            f.focus_lent = true;
        }
    }
    kept
}

/// A `POPUP_DONE` of a popup frame's window: a `DELETE` action flagged `NHG_DEL_POPUP`.  `false` for another window's.
pub fn popup_done(state: &mut Display, ev: &Record) -> bool {
    match state.frame(ev.window) {
        Ok(f) if f.flags & NHG_FRAME_POPUP != 0 => {
            let mut a = NhgAction::new(NHG_ACT_DELETE, ev.window, ev.time_ns);
            a.flags = NHG_DEL_POPUP;
            state.actions.push(a);
            true
        }
        _ => false,
    }
}

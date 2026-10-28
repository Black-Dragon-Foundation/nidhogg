//! The wheel: a `SCROLL` record becomes a `WHEEL` action, with the accumulator that turns small records into whole
//! steps.  Moved from `nhwl_scroll_record` (`src/nhwlterm.c`, adapted from pgtk's `scroll_event`, emacs-31.1): wheel
//! records add up to whole 120-unit notches (the fraction stays in the accumulator, which a new window, source or
//! direction resets); finger and continuous records carry their pixel delta in device pixels, for
//! `pixel-scroll-precision-mode`; with `mwheel-coalesce-scroll-events` they add up to whole lines or columns instead;
//! a stop record is a `TOUCH_END_EVENT` (momentum).  What the steps mean to Emacs (the event, its position, `arg`)
//! stays C.
//!
//! The size of a step in a finger record is the window's column width or line height, which C pushes with
//! `nhg_frame_units` when the font changes; the pixel delta is the record's in device pixels (the record's logical
//! pixels times the window's scale).

use crate::action::{NHG_ACT_WHEEL, NHG_IN_COALESCE_WHEEL, NhgAction, NhgNextIn};
use crate::display::Display;
use crate::guard::GlueError;
use nhshell::event::NhShellEvent;

/// `WHEEL` flags.  None of them: `w` whole steps, one event each.
/// A finger or continuous record's own pixel delta (`dx`, `dy`): one event, no steps.
pub const NHG_WHEEL_HIRES: u32 = 1;
/// The stop of a gesture: momentum begins.
pub const NHG_WHEEL_STOP: u32 = 2;
/// `w` steps in one event (`mwheel-coalesce-scroll-events`).
pub const NHG_WHEEL_COALESCED: u32 = 4;

/// `WHEEL` `code`s, as pgtk's buttons: up, down, left, right.
pub const NHG_WHEEL_UP: i32 = 4;
pub const NHG_WHEEL_DOWN: i32 = 5;
pub const NHG_WHEEL_LEFT: i32 = 6;
pub const NHG_WHEEL_RIGHT: i32 = 7;

/// The wheel's accumulator: the fractions of a step not yet taken, and whose they are.
#[derive(Clone, Copy, Debug, Default)]
pub struct Scroll {
    acc_x: f64,
    acc_y: f64,
    /// The window the fractions are of (compared, never used).
    window: u64,
    source: u32,
}

/// A `SCROLL` record: nothing for a window the glue does not know, else a `WHEEL` action when it makes an event.  The
/// action's `code` is the direction, `w` the number of steps, `dx` and `dy` the pixel delta in device pixels, `mods`
/// the keyboard's.
pub fn scroll(state: &mut Display, ev: &NhShellEvent, input: NhgNextIn) {
    let Ok(f) = state.frame(ev.window) else {
        return;
    };
    let scale = if f.scale120 == 0 {
        1.0
    } else {
        f64::from(f.scale120) / 120.0
    };
    let (col_w, line_h) = (f.facts.col_w, f.facts.line_h);
    // SAFETY: a SCROLL record's valid member is `scroll`.
    let s = unsafe { ev.u.scroll };
    let (px, py) = (s.dx * scale, s.dy * scale);
    let wheel = s.v120x != 0 || s.v120y != 0;
    let horizontal = if wheel {
        s.v120x != 0
    } else {
        px.abs() > py.abs()
    };
    let mut a = NhgAction::new(NHG_ACT_WHEEL, ev.window, ev.time_ns);
    a.mods = state.keyboard.mods;
    a.dx = px;
    a.dy = py;
    let acc = &mut state.scroll;
    if s.stop != 0 {
        acc.acc_x = 0.0;
        acc.acc_y = 0.0;
        a.flags = NHG_WHEEL_STOP;
        state.actions.push(a);
        return;
    }
    let delta = if wheel {
        f64::from(if horizontal { s.v120x } else { s.v120y })
    } else if horizontal {
        px
    } else {
        py
    };
    if delta == 0.0 {
        return;
    }
    let (mine, other) = if horizontal {
        (&mut acc.acc_x, &mut acc.acc_y)
    } else {
        (&mut acc.acc_y, &mut acc.acc_x)
    };
    if acc.window != ev.window
        || acc.source != s.source
        || (*mine != 0.0 && (*mine < 0.0) != (delta < 0.0))
    {
        *mine = 0.0;
        *other = 0.0;
    }
    acc.window = ev.window;
    acc.source = s.source;
    a.code = match (horizontal, delta < 0.0) {
        (false, true) => NHG_WHEEL_UP,
        (false, false) => NHG_WHEEL_DOWN,
        (true, true) => NHG_WHEEL_LEFT,
        (true, false) => NHG_WHEEL_RIGHT,
    };
    let coalesce = input.flags & NHG_IN_COALESCE_WHEEL != 0;
    if !wheel && !coalesce {
        a.flags = NHG_WHEEL_HIRES;
        state.actions.push(a);
        return;
    }
    let unit = if wheel {
        120.0
    } else {
        f64::from(if horizontal { col_w } else { line_h }).max(1.0)
    };
    *mine += delta;
    let n = (mine.abs() / unit) as i32;
    if n == 0 {
        return;
    }
    *mine -= (if delta < 0.0 { -n } else { n }) as f64 * unit;
    a.w = n;
    a.flags = if coalesce { NHG_WHEEL_COALESCED } else { 0 };
    state.actions.push(a);
}

/// C pushed `window`'s column width and line height in device pixels (the font changed, or the window was made).
pub fn set_units(
    state: &mut Display,
    window: u64,
    col_w: i32,
    line_h: i32,
) -> Result<(), GlueError> {
    if col_w <= 0 || line_h <= 0 {
        return Err(GlueError::Arg);
    }
    let f = state.frame_mut(window)?;
    f.facts.col_w = col_w;
    f.facts.line_h = line_h;
    Ok(())
}

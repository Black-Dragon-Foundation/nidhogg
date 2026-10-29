//! Touch screens and touchpad pinch: the shell's `TOUCH` and `PINCH` records as the actions C turns into Emacs's
//! `TOUCHSCREEN_*_EVENT` and `PINCH_EVENT`.  Moved from pgtk's `touch_event_cb`, `pgtk_link_touch_point` and
//! `pgtk_handle_event` (emacs-31.1), whose local detail ids, "nothing changed" rule and per-frame point lists it keeps.
//!
//! The points of a display are tracked here (the shell's ids are the compositor's, unique while down; Lisp gets a
//! counter's local ids).  A window's points go with it when the glue forgets the window, as pgtk unlinks a deleted
//! frame's points without an event.  Positions are the shell's, device pixels in the window's surface, rounded the way
//! `lrint` rounds; C moves them to the native area (`nhwl_csd_native_pos`).

use crate::action::NhgAction;
use crate::display::Display;
use nhshell::event::*;

/// A touch action (`TOUCH`): `code` the point's local id, `flags` `NHG_TOUCH_*`, `x`, `y` the point's position.  An
/// `UPDATE` carries the window's live points, oldest first, in `text` as `len / 12` `NhgTouchPoint`s, and no position.
pub const NHG_ACT_TOUCH: u32 = 39;
/// A pinch action (`PINCH`, begin and update; the end gives none): `dx`, `dy` this update's motion of the fingers'
/// centre (device pixels), `scale` the cumulative scale, `x`, `y` the pointer's position, `mods` the keyboard's, and
/// `text` the update's angle in radians, clockwise, as a native `double` (8 bytes).
pub const NHG_ACT_PINCH: u32 = 40;

/// `TOUCH` flags: the point went down, moved, lifted, or the compositor cancelled it.
pub const NHG_TOUCH_BEGIN: u32 = 0;
pub const NHG_TOUCH_UPDATE: u32 = 1;
pub const NHG_TOUCH_END: u32 = 2;
pub const NHG_TOUCH_CANCEL: u32 = 3;

/// One live point in an `UPDATE`'s `text` (the header's `NhgTouchPoint`).
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NhgTouchPoint {
    pub x: i32,
    pub y: i32,
    pub id: i32,
}

/// A point that is down.
#[derive(Debug)]
struct Point {
    /// The shell's id.
    shell_id: i32,
    window: u64,
    x: i32,
    y: i32,
    /// The id Lisp sees.
    local: i32,
}

/// The display's live touch points, in the order they went down.
#[derive(Debug, Default)]
pub struct Touch {
    points: Vec<Point>,
    next_local: i32,
}

impl Touch {
    /// `window` is gone: its points go with it, silently.
    pub fn forget(&mut self, window: u64) {
        self.points.retain(|p| p.window != window);
    }

    /// The display's connection is lost: every point goes, silently (the ids Lisp holds are never reused).
    pub fn clear(&mut self) {
        self.points.clear();
    }
}

/// `lrint`: round to nearest, ties to even.
fn lrint(v: f64) -> i32 {
    v.round_ties_even() as i32
}

fn push(state: &mut Display, ev: &Record, flags: u32, local: i32, x: i32, y: i32) {
    let mut a = NhgAction::new(NHG_ACT_TOUCH, ev.window, ev.time_ns);
    a.flags = flags;
    a.code = local;
    a.x = x;
    a.y = y;
    state.actions.push(a);
}

/// A `TOUCH` record.  A point already down is not begun again; a motion of an unknown point, or one that rounds to
/// where the point already is, gives nothing; a lift or cancel of an unknown point gives nothing.  A window the glue
/// does not know gets no points.
pub fn touch(state: &mut Display, ev: &Record, t: &TouchEv) {
    let (x, y) = (lrint(t.x), lrint(t.y));
    let known = state.frame(ev.window).is_ok();
    let at = state.touch.points.iter().position(|p| p.shell_id == t.id);
    match (t.op, at) {
        (NHSHELL_TOUCH_DOWN, None) if known => {
            if state.touch.next_local == i32::MAX {
                state.touch.next_local = 0;
            }
            let local = state.touch.next_local;
            state.touch.next_local += 1;
            state.touch.points.push(Point {
                shell_id: t.id,
                window: ev.window,
                x,
                y,
                local,
            });
            push(state, ev, NHG_TOUCH_BEGIN, local, x, y);
        }
        (NHSHELL_TOUCH_MOTION, Some(i)) => {
            let p = &mut state.touch.points[i];
            if (p.x, p.y) == (x, y) {
                return;
            }
            (p.x, p.y) = (x, y);
            let window = p.window;
            let mut list = Vec::new();
            for q in state.touch.points.iter().filter(|q| q.window == window) {
                list.extend_from_slice(&q.x.to_ne_bytes());
                list.extend_from_slice(&q.y.to_ne_bytes());
                list.extend_from_slice(&q.local.to_ne_bytes());
            }
            let mut a = NhgAction::new(NHG_ACT_TOUCH, window, ev.time_ns);
            a.flags = NHG_TOUCH_UPDATE;
            a.code = state.touch.points[i].local;
            state.actions.push_text(a, Some(list));
        }
        (NHSHELL_TOUCH_UP | NHSHELL_TOUCH_CANCEL, Some(i)) => {
            let p = state.touch.points.remove(i);
            let flags = if t.op == NHSHELL_TOUCH_UP {
                NHG_TOUCH_END
            } else {
                NHG_TOUCH_CANCEL
            };
            let mut a = NhgAction::new(NHG_ACT_TOUCH, p.window, ev.time_ns);
            a.flags = flags;
            a.code = p.local;
            // the event's position (the shell repeats the point's last for a lift)
            (a.x, a.y) = (x, y);
            state.actions.push(a);
        }
        _ => {}
    }
}

/// A `PINCH` record: begin and update give a `PINCH` action for a window the glue knows; the end gives none.
pub fn pinch(state: &mut Display, ev: &Record, p: &PinchEv) {
    if p.phase == NHSHELL_PINCH_END || state.frame(ev.window).is_err() {
        return;
    }
    let mut a = NhgAction::new(NHG_ACT_PINCH, ev.window, ev.time_ns);
    a.dx = p.dx;
    a.dy = p.dy;
    a.scale = p.scale;
    a.x = lrint(p.x);
    a.y = lrint(p.y);
    a.mods = state.keyboard().mods;
    let angle = p.rotation.to_radians();
    state
        .actions
        .push_text(a, Some(angle.to_ne_bytes().to_vec()));
}

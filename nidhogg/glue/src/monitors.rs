//! The outputs: the monitor list the shell's `OUTPUT_*` records keep up to date, and the outputs each window is on.
//! Moved from the C drain's output cases and `nhwl_output_left` (`src/nhwlterm.c`) and from `struct nhwl_monitor` and
//! `struct nhwl_output`'s `entered` list (`src/nhwlterm.h`).  C makes `display-monitors-changed-functions` run when a
//! `MONITORS` action comes, and reads the list and each window's outputs through `nhg_monitors` and
//! `nhg_frame_outputs`.

use crate::action::{NHG_ACT_MONITORS, NhgAction};
use crate::display::Display;
use crate::guard::GlueError;
use crate::query::{MAX_OUTPUTS, NhgMonitor};
use nhshell::event::{OutputEv, Record};
use std::ffi::c_char;

/// An `OUTPUT_ADD` or `OUTPUT_CHANGE`: the output joins the list or replaces the one with its id, and a `MONITORS`
/// action says so, with the output's scale times 120 in `code` (C takes the display's scale from the first nonzero
/// one).  A window alone on the output that was given a fraction of the new ceiling gives it back to the output (its
/// scale may have come first).  A new output past the sixteenth gives nothing.
pub fn output(state: &mut Display, ev: &Record, o: &OutputEv) {
    let mut m = NhgMonitor {
        id: o.id,
        x: o.x,
        y: o.y,
        w: o.w,
        h: o.h,
        mm_w: o.mm_w,
        mm_h: o.mm_h,
        scale120: o.scale120,
        name: [0; 32],
    };
    for (dst, src) in m.name.iter_mut().zip(o.name) {
        *dst = src as c_char;
    }
    if !state.monitors.set(&m) {
        return;
    }
    let alone = state
        .frames()
        .find(|(_, f)| f.outputs == [o.id] && f.scale120 != 0)
        .map(|(_, f)| f.scale120);
    if let Some(s120) = alone {
        state.monitors.learn(o.id, s120);
    }
    let mut a = NhgAction::new(NHG_ACT_MONITORS, 0, ev.time_ns);
    a.code = o.scale120 as i32;
    state.actions.push(a);
}

/// An `OUTPUT_REMOVE`: the output leaves the list and every window's outputs, and a `MONITORS` action says so.  An
/// output the glue did not know gives nothing.
pub fn removed(state: &mut Display, ev: &Record, o: &OutputEv) {
    let id = o.id;
    if !state.monitors.remove(id) {
        return;
    }
    for f in state.frames_mut() {
        f.outputs.retain(|&o| o != id);
    }
    state
        .actions
        .push(NhgAction::new(NHG_ACT_MONITORS, 0, ev.time_ns));
}

/// An `ENTER_OUTPUT`: the window is on the output now (at most `MAX_OUTPUTS` are kept per window).
pub fn entered(state: &mut Display, ev: &Record, o: &OutputEv) {
    let id = o.id;
    let Ok(f) = state.frame_mut(ev.window) else {
        return;
    };
    let first = f.outputs.first().copied();
    if f.outputs.len() < MAX_OUTPUTS && !f.outputs.contains(&id) {
        f.outputs.push(id);
    }
    moved(state, ev, first);
}

/// A `LEAVE_OUTPUT`: the window is off the output.
pub fn left(state: &mut Display, ev: &Record, o: &OutputEv) {
    let id = o.id;
    let Ok(f) = state.frame_mut(ev.window) else {
        return;
    };
    let first = f.outputs.first().copied();
    f.outputs.retain(|&o| o != id);
    moved(state, ev, first);
}

/// After an enter or leave: a `MONITORS` action for the window when it is on an output and its first one is no longer
/// `first` (it came to or moved to another monitor; C runs `move-frame-functions`), and the output's scale learnt.
fn moved(state: &mut Display, ev: &Record, first: Option<u64>) {
    if let Ok(f) = state.frame(ev.window)
        && f.outputs.first().is_some_and(|&o| Some(o) != first)
    {
        state
            .actions
            .push(NhgAction::new(NHG_ACT_MONITORS, ev.window, ev.time_ns));
    }
    learn(state, ev);
}

/// When `ev.window` is on one output and has a scale, that scale is the output's (the compositor gives a window
/// its monitor's fractional scale; `wl_output` only its ceiling); a `MONITORS` action says when that changed it.
pub fn learn(state: &mut Display, ev: &Record) {
    let Ok(f) = state.frame(ev.window) else {
        return;
    };
    let (&[id], s120) = (f.outputs.as_slice(), f.scale120) else {
        return;
    };
    if s120 != 0 && state.monitors.learn(id, s120) {
        state
            .actions
            .push(NhgAction::new(NHG_ACT_MONITORS, 0, ev.time_ns));
    }
}

/// Copy the monitors into `out` (as many as fit); the number written.
pub fn list(state: &Display, out: &mut [NhgMonitor]) -> usize {
    let all = state.monitors().all();
    let n = all.len().min(out.len());
    out[..n].copy_from_slice(&all[..n]);
    n
}

/// The monitors `window` is on, as a mask of their places in the list (bit J: the Jth monitor; the first when the
/// compositor named none of them), for `display-monitor-attributes-list`'s frames (moved from
/// `Fpgtk_display_monitor_attributes_list`).
pub fn frame_monitors(state: &Display, window: u64) -> Result<u32, GlueError> {
    let ids = &state.frame(window)?.outputs;
    let mask = state
        .monitors()
        .all()
        .iter()
        .take(MAX_OUTPUTS.min(32))
        .enumerate()
        .filter(|(_, m)| ids.contains(&m.id))
        .fold(0u32, |acc, (j, _)| acc | 1 << j);
    Ok(if mask == 0 { 1 } else { mask })
}

/// A monitor's position in device pixels from its logical `x`, `y` and `s120`, as `display-monitor-attributes-list`
/// gives it (moved from `Fpgtk_display_monitor_attributes_list`).  The product is signed, so a monitor left of or
/// above the primary has a negative origin, as pgtk and GDK report it.
pub fn device_origin(x: i32, y: i32, s120: u32) -> (i32, i32) {
    let dev = |v: i32| ((i64::from(v) * i64::from(s120)) as f32 / 120.0f32).round() as i32;
    (dev(x), dev(y))
}

/// Copy the ids of the outputs `window` is on into `out` (as many as fit); the number written.
pub fn frame_outputs(state: &Display, window: u64, out: &mut [u64]) -> Result<usize, GlueError> {
    let ids = &state.frame(window)?.outputs;
    let n = ids.len().min(out.len());
    out[..n].copy_from_slice(&ids[..n]);
    Ok(n)
}

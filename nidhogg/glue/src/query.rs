//! What `x-display-*` asks of the display: its size in pixels and millimetres, the scale and what the shell can do.
//!
//! Moved from `src/nhwlfns.c` (`x-display-pixel-width`, `-height`, `nhwl_display_mm`) and `src/nhwlterm.c`
//! (`nhwl_display_pixel_width`, `-height`).  The arithmetic is pgtk's (`pgtk_display_pixel_*`, `pgtk_display_mm`,
//! emacs-31.1): the outputs at the origin count the largest in millimetres and the others add up.
//!
//! The outputs come from the drain, from the shell's `OUTPUT_ADD`, `OUTPUT_CHANGE` and `OUTPUT_REMOVE` records
//! (`monitors.rs`).

use crate::display::Display;
use nhshell::api::ShellBackend;
use nhshell::caps::NhShellCaps;
use std::ffi::c_char;

/// Outputs kept (the shell's records past this many are dropped, as C dropped them).
pub const MAX_OUTPUTS: usize = 16;

/// The size reported when no output is known.
const FALLBACK_W: i32 = 1920;
const FALLBACK_H: i32 = 1080;

/// One output.  Layout equals `NhgMonitor`.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NhgMonitor {
    pub id: u64,
    /// The output's position in logical pixels, and its mode in device pixels.
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
    pub mm_w: i32,
    pub mm_h: i32,
    pub scale120: u32,
    /// NUL-terminated.
    pub name: [c_char; 32],
}

/// The display as `x-display-*` reads it.  Layout equals `NhgDisplayInfo`.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct NhgDisplayInfo {
    /// The extent of the outputs in device pixels (1920 x 1080 with none).
    pub w: i32,
    pub h: i32,
    /// The extent in millimetres.
    pub mm_w: i32,
    pub mm_h: i32,
    /// The first output's scale times 120 (0: no output has been seen).
    pub scale120: u32,
    pub caps: NhShellCaps,
}

/// The outputs the shell has reported.
#[derive(Debug, Default)]
pub struct Monitors {
    list: Vec<NhgMonitor>,
    /// The first scale an output reported, kept after that output goes (as `dpyinfo->scale120` was).
    scale120: u32,
}

impl Monitors {
    /// Add the output `m.id`, or replace it.  False when the list is full and `m` is new.
    pub fn set(&mut self, m: &NhgMonitor) -> bool {
        let mut m = *m;
        m.name[m.name.len() - 1] = 0;
        match self.list.iter().position(|o| o.id == m.id) {
            Some(i) => self.list[i] = m,
            None if self.list.len() == MAX_OUTPUTS => return false,
            None => self.list.push(m),
        }
        if self.scale120 == 0 {
            self.scale120 = m.scale120;
        }
        true
    }

    /// Drop the output `id`.  False when it was not there.
    pub fn remove(&mut self, id: u64) -> bool {
        match self.list.iter().position(|o| o.id == id) {
            Some(i) => {
                self.list.swap_remove(i);
                true
            }
            None => false,
        }
    }

    /// The outputs, in the order the list keeps them.
    pub fn all(&self) -> &[NhgMonitor] {
        &self.list
    }

    /// The extent of the outputs in device pixels: each output's far corner, its position scaled to device pixels.
    pub fn pixels(&self) -> (i32, i32) {
        let far = |pos: i32, m: &NhgMonitor, len: i32| {
            (pos as f32 * m.scale120 as f32 / 120.0).round() as i32 + len
        };
        let w = self
            .list
            .iter()
            .map(|m| far(m.x, m, m.w))
            .max()
            .unwrap_or(0);
        let h = self
            .list
            .iter()
            .map(|m| far(m.y, m, m.h))
            .max()
            .unwrap_or(0);
        // an extent of 0 falls back to the widest or tallest mode, and that to the default
        let widest = self.list.iter().map(|m| m.w).max().unwrap_or(0);
        let tallest = self.list.iter().map(|m| m.h).max().unwrap_or(0);
        let pick = |extent: i32, fallback: i32, default: i32| {
            if extent != 0 {
                extent
            } else if fallback != 0 {
                fallback
            } else {
                default
            }
        };
        (pick(w, widest, FALLBACK_W), pick(h, tallest, FALLBACK_H))
    }

    /// The extent in millimetres: the largest of the outputs at the origin, plus every other output.
    pub fn mm(&self) -> (i32, i32) {
        let sum = |at: fn(&NhgMonitor) -> (i32, i32)| {
            let (mut at_0, mut other) = (0, 0);
            for m in &self.list {
                let (pos, mm) = at(m);
                if pos == 0 {
                    at_0 = at_0.max(mm);
                } else {
                    other += mm;
                }
            }
            at_0 + other
        };
        (sum(|m| (m.x, m.mm_w)), sum(|m| (m.y, m.mm_h)))
    }

    /// The display's size and scale, with the shell's `caps`.
    pub fn info(&self, caps: NhShellCaps) -> NhgDisplayInfo {
        let (w, h) = self.pixels();
        let (mm_w, mm_h) = self.mm();
        NhgDisplayInfo {
            w,
            h,
            mm_w,
            mm_h,
            scale120: self.scale120,
            caps,
        }
    }
}

/// The display's backend, for the calls that need no state of ours.
///
/// # Safety
/// `sh` is the display's live handle (`Display::shell`), and nothing else uses the backend while the result lives.
pub(crate) unsafe fn backend<'a>(sh: *mut nhshell::ffi::NhShell) -> &'a mut dyn ShellBackend {
    // SAFETY: a live handle (the caller's promise).
    unsafe { &mut *sh }.backend_mut()
}

/// The display as `x-display-*` reads it now.
pub fn info(state: &mut Display) -> NhgDisplayInfo {
    let sh = state.shell();
    // SAFETY: the display's handle, used for this call only.
    let caps = unsafe { backend(sh) }.caps();
    state.monitors.info(caps)
}

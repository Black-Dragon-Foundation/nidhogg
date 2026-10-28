//! Window layers: an Emacs window that scrolls whole gets a surface of its own.
//!
//! The glue decides which windows of a frame are their own outputs in the frame's Glitnir scene
//! (`glitnir_layer_set_output`); the shell follows the outputs each commit reports, and shows each on a content layer
//! whose viewport source moves with the scroll instead of the whole window being drawn and uploaded again.
//!
//! - **Which frames:** an opaque root only.  The window is a top-level (not a child frame, a tip or a popup), its alpha
//!   is 1 and the compositor applies it (no alpha the shell multiplies), its `alpha-background` is 1 (C says so in its
//!   flags), the compositor has `wp_viewporter` and `wl_subcompositor`, and `nhwl-scroll-layers` allows the frame's
//!   scale (C passes the variable as flags: off, on, or integer scales only).
//! - **Promotion:** a window's first full-window scroll (its run moves every row of the window except the ones the
//!   scroll exposes) makes it a WINDOW layer right under the frame's layer, with id `w->sequence_number | WINDOW_TAG`
//!   (the scroll's id too) and the window's exact rectangle in the backing (Glitnir snaps it), then its own output.  At
//!   most four per frame.
//! - **Demotion:** two seconds after the window's last full-window scroll, when the window is resized, split or
//!   deleted, at a scale change, when the frame stops being an opaque root, and at a reconnect.  The layer goes back to
//!   being part of the frame's (`set_output (…, 0)`) and leaves the scene; Glitnir damages its rectangle on the frame,
//!   and the shell unmaps the layer with the root's next buffer, in the same tree commit.  The idle, window-tree and
//!   frame checks run when C publishes the tree ([`sync`]), which is when a demotion reaches the screen anyway.
//! - **The backing still moves its rows** (`Backings::scroll`): it is Emacs's retained picture, and the shell's front
//!   of the frame relies on it equalling the moved rows plus the damage.

use crate::backing::{Backings, NhgRect, WINDOW_TAG};
use crate::display::Display;
use crate::frame::NHG_FRAME_KINDS;
use crate::guard::GlueError;
use glitnir::ffi::{glitnir_layer_remove, glitnir_layer_set_output, glitnir_layer_upsert};
use glitnir::geom::Rect;
use glitnir::scene::{LayerDesc, MAX_OUTPUTS};
use std::collections::BTreeMap;
use std::time::{Duration, Instant};

/// Flags C passes: `nhwl-scroll-layers` is not nil.
pub const NHG_LAYERS_ON: u32 = 1;
/// Flags C passes: `nhwl-scroll-layers` is `integer-scales`: only at a whole scale.
pub const NHG_LAYERS_INTEGER_SCALES: u32 = 2;
/// Flags C passes: the frame's `alpha-background` is 1.
pub const NHG_LAYERS_OPAQUE_BG: u32 = 4;

/// How long a window stays its own output after its last full-window scroll.
pub const IDLE: Duration = Duration::from_secs(2);

/// Glitnir's `GLITNIR_LAYER_WINDOW`.
const LAYER_WINDOW: u32 = 2;

/// One window of a frame as C lists it when it publishes.  Layout equals `NhgLayerWindow`.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct NhgLayerWindow {
    /// The window's `sequence_number`.
    pub seq: u64,
    /// `window_box (w, ANY_AREA)`: device pixels, with the native area's origin.
    pub rect: NhgRect,
}

/// A window that is its own output.
#[derive(Debug, Clone, Copy)]
struct Promoted {
    /// Its rectangle as C passed it (with the native area's origin).
    rect: NhgRect,
    /// The frame's scale times 120 and the native area's offset in the backing when it was promoted.
    s120: u32,
    origin: (i32, i32),
    /// Its last full-window scroll.
    last: Instant,
}

/// A frame's windows that are their own outputs, by `sequence_number`.
#[derive(Debug, Default)]
pub struct Layers {
    windows: BTreeMap<u64, Promoted>,
}

impl Layers {
    pub fn len(&self) -> usize {
        self.windows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.windows.is_empty()
    }

    /// Whether window `seq` is its own output.
    pub fn contains(&self, seq: u64) -> bool {
        self.windows.contains_key(&seq)
    }
}

/// What the opaque-root check reads of a frame.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Root {
    /// The frame's kind (`NHG_FRAME_*`; none of them: a top-level).
    pub kind: u32,
    /// What its alpha takes off opaque, and whether the shell multiplies it.
    pub alpha_cut: u32,
    pub alpha_host: bool,
    /// The compositor has `wp_viewporter` and `wl_subcompositor`.
    pub viewporter: bool,
}

/// Whether a frame may have window layers: an opaque top-level on a compositor that can place them, at a scale
/// `nhwl-scroll-layers` (`flags`) allows.  `s120` is the frame's scale times 120.
pub fn opaque_root(r: &Root, flags: u32, s120: u32) -> bool {
    flags & NHG_LAYERS_ON != 0
        && flags & NHG_LAYERS_OPAQUE_BG != 0
        && (flags & NHG_LAYERS_INTEGER_SCALES == 0 || s120.is_multiple_of(120))
        && r.kind & NHG_FRAME_KINDS == 0
        && r.alpha_cut == 0
        && !r.alpha_host
        && r.viewporter
}

/// Whether the rows `run` moving to `to_y` move every row of the window at `rect` except the ones the move exposes:
/// the window's columns, from its first row to its last.
pub fn full_window(rect: NhgRect, run: NhgRect, to_y: i32) -> bool {
    let top = run.y.min(to_y);
    let bottom = run.y.max(to_y).saturating_add(run.h);
    rect.w > 0
        && rect.h > 0
        && run.h > 0
        && to_y != run.y
        && run.x == rect.x
        && run.w == rect.w
        && top == rect.y
        && bottom == rect.y.saturating_add(rect.h)
}

/// The frame's facts for `window` (its shell window), and the display's compositor.
fn root(state: &mut Display, window: u64) -> Result<Root, GlueError> {
    let caps = state.backend().caps();
    let f = state.frame(window)?;
    Ok(Root {
        kind: f.flags,
        alpha_cut: f.alpha_cut,
        alpha_host: f.alpha_host,
        viewporter: caps.viewporter != 0 && caps.subcompositor != 0,
    })
}

/// Window `seq`'s layer is part of the frame `key`'s again, and leaves its scene.
fn demote(key: u64, layers: &mut Layers, seq: u64) {
    if layers.windows.remove(&seq).is_some() {
        let id = seq | WINDOW_TAG;
        glitnir_layer_set_output(key, id, 0);
        glitnir_layer_remove(key, id);
    }
}

/// Make window `seq` at `rect` (with the native area's origin) frame `key`'s own output.  False when Glitnir refused
/// (the scene has no frame layer, or four outputs already).
fn promote(key: u64, b: &mut Backings, seq: u64, rect: NhgRect, now: Instant) -> bool {
    let Ok(f) = b.frame_mut(key) else {
        return false;
    };
    let ((l, t), s120) = (f.origin(), f.scale120());
    if s120 == 0 || f.layers.len() >= MAX_OUTPUTS {
        return false;
    }
    let id = seq | WINDOW_TAG;
    let d = LayerDesc {
        id,
        parent: key,
        kind: LAYER_WINDOW,
        rect: Rect {
            x: rect.x.saturating_add(l),
            y: rect.y.saturating_add(t),
            w: rect.w,
            h: rect.h,
        },
        scale: f64::from(s120) / 120.0,
        visible: 1,
    };
    // SAFETY: `d` is a readable LayerDesc for the call.
    if unsafe { glitnir_layer_upsert(key, &d) } != 0 {
        return false;
    }
    if glitnir_layer_set_output(key, id, 1) != 0 {
        glitnir_layer_remove(key, id);
        return false;
    }
    f.layers.windows.insert(
        seq,
        Promoted {
            rect,
            s120,
            origin: (l, t),
            last: now,
        },
    );
    true
}

/// Window `seq` (its rectangle `rect`) of frame `key` scrolls: its rows `run` move to `to_y`.  A full-window scroll on
/// an opaque root (`eligible`) promotes the window, or keeps it promoted; a changed rectangle or scale, or a frame no
/// longer eligible, demotes it first.  Then the backing moves the rows and Glitnir learns the move.  True when the
/// window is its own output now.  Writes pixels.
#[allow(clippy::too_many_arguments)]
pub fn scroll_at(
    b: &mut Backings,
    key: u64,
    seq: u64,
    rect: NhgRect,
    run: NhgRect,
    to_y: i32,
    eligible: bool,
    now: Instant,
) -> Result<bool, i32> {
    let f = b.frame_mut(key)?;
    let (s120, origin) = (f.scale120(), f.origin());
    if let Some(p) = f.layers.windows.get(&seq).copied()
        && (!eligible || p.rect != rect || p.s120 != s120 || p.origin != origin)
    {
        demote(key, &mut f.layers, seq);
    }
    let mut own = f.layers.contains(seq);
    if eligible && full_window(rect, run, to_y) {
        if let Some(p) = f.layers.windows.get_mut(&seq) {
            p.last = now;
        } else {
            own = promote(key, b, seq, rect, now);
        }
    }
    b.scroll(key, seq, run, to_y)?;
    Ok(own)
}

/// Frame `key` is published, its windows now being `windows`: each promoted window that is gone or has another
/// rectangle, whose last full-window scroll is `IDLE` old, or every one when the frame is not `eligible` or its scale
/// changed, goes back to being part of the frame.  Returns how many stay own outputs.
pub fn sync_at(
    b: &mut Backings,
    key: u64,
    windows: &[NhgLayerWindow],
    eligible: bool,
    now: Instant,
) -> Result<usize, i32> {
    let f = b.frame_mut(key)?;
    let (s120, origin) = (f.scale120(), f.origin());
    let gone: Vec<u64> = f
        .layers
        .windows
        .iter()
        .filter(|(seq, p)| {
            !eligible
                || p.s120 != s120
                || p.origin != origin
                || now.saturating_duration_since(p.last) >= IDLE
                || !windows.iter().any(|w| w.seq == **seq && w.rect == p.rect)
        })
        .map(|(seq, _)| *seq)
        .collect();
    for seq in gone {
        demote(key, &mut f.layers, seq);
    }
    Ok(f.layers.len())
}

/// Every window layer of every frame goes back to being part of its frame (the connection is lost: the shell drops
/// its content layers, and the next full-window scroll promotes again).
pub fn drop_all(b: &mut Backings) {
    for (key, f) in b.frames_mut() {
        let seqs: Vec<u64> = f.layers.windows.keys().copied().collect();
        for seq in seqs {
            demote(key, &mut f.layers, seq);
        }
    }
}

/// `nhg_scroll_window`: [`scroll_at`] now, with the opaque-root check on `window`, the frame's shell window.
#[allow(clippy::too_many_arguments)]
pub fn scroll(
    state: &mut Display,
    window: u64,
    key: u64,
    seq: u64,
    rect: NhgRect,
    run: NhgRect,
    to_y: i32,
    flags: u32,
) -> Result<bool, GlueError> {
    let s120 = state.backings.frame_mut(key)?.scale120();
    // a frame without a registered window still scrolls; it has no layers
    let eligible = root(state, window).is_ok_and(|r| opaque_root(&r, flags, s120));
    Ok(scroll_at(
        &mut state.backings,
        key,
        seq,
        rect,
        run,
        to_y,
        eligible,
        Instant::now(),
    )?)
}

/// `nhg_layers_sync`: [`sync_at`] now, with the opaque-root check on `window`.  A frame without a backing yet has no
/// layers (0).
pub fn sync(
    state: &mut Display,
    window: u64,
    key: u64,
    windows: &[NhgLayerWindow],
    flags: u32,
) -> Result<usize, GlueError> {
    let Ok(s120) = state.backings.frame_mut(key).map(|f| f.scale120()) else {
        return Ok(0);
    };
    let eligible = root(state, window).is_ok_and(|r| opaque_root(&r, flags, s120));
    Ok(sync_at(
        &mut state.backings,
        key,
        windows,
        eligible,
        Instant::now(),
    )?)
}

/// How many windows of frame `key` are their own outputs (0 for a frame without a backing).
pub fn count(b: &mut Backings, key: u64) -> usize {
    b.frame_mut(key).map_or(0, |f| f.layers.len())
}

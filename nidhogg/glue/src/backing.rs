//! Backings, damage and scroll: each frame's pixels (C draws on them with Cairo, through
//! `cairo_image_surface_create_for_data`), its Glitnir scene and layer.  Publishing (`present.rs`) hands the pixels to
//! the shell, which commits the scene's damage itself.
//!
//! Moved from `src/nhwldraw.c` (`nhwl_backing_resize`, `nhwl_glitnir_ensure`, the damage and scroll calls,
//! `nhwl_snapshot`'s copy) without a change in behaviour.  C still decides when to call: it reads the frame's geometry
//! and passes it as [`NhgFrameGeom`].
//!
//! **A frame's key** is its Glitnir root id, which [`Backings::ensure`] gives it (C keeps it in `glitnir_id`).  A shell
//! window id is not stable for a frame: popup, tip and child frames get a new window each time they are shown.

use glitnir::ffi::{
    glitnir_damage_add_n, glitnir_damage_all, glitnir_damage_forget, glitnir_layer_upsert,
    glitnir_root_drop, glitnir_root_new, glitnir_scroll, glitnir_stats_forget,
};
use glitnir::geom::Rect;
use glitnir::scene::LayerDesc;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::abi::{NHG_E_NOMEM, NHG_E_STATE};

/// [`NhgFrameGeom::flags`]: the layer's rectangle or chrome changed (a child frame's, as `child::layer` says).
pub const NHG_GEOM_LAYER_CHANGED: u32 = 1;
/// [`NhgFrameGeom::flags`]: the backing starts over, in the background colour, keeping nothing (C has no surface).
pub const NHG_GEOM_FRESH: u32 = 2;
/// Glitnir's `GLITNIR_LAYER_FRAME`.
const LAYER_FRAME: u32 = 1;
/// The scroll's window id goes to Glitnir with this bit set: an Emacs window's `sequence_number` and a frame's layer id
/// are both small counters, and Glitnir damages the layer whose id equals a scrolled window's.
pub const WINDOW_TAG: u64 = 1 << 63;

/// The last key given (C's `static last_id`: one counter for every display, as Glitnir's ids are global).
static LAST_ID: AtomicU64 = AtomicU64::new(0);

/// A rectangle in device pixels.  Layout equals `NhgRect` (and `GlitnirRect`).
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct NhgRect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl From<Rect> for NhgRect {
    fn from(r: Rect) -> Self {
        NhgRect {
            x: r.x,
            y: r.y,
            w: r.w,
            h: r.h,
        }
    }
}

impl From<NhgRect> for Rect {
    fn from(r: NhgRect) -> Self {
        Rect {
            x: r.x,
            y: r.y,
            w: r.w,
            h: r.h,
        }
    }
}

/// A frame's geometry as C reads it before a call.  Layout equals `NhgFrameGeom`.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct NhgFrameGeom {
    /// The native area (Emacs's frame pixel size).
    pub native_w: i32,
    pub native_h: i32,
    /// The border around the native area in the backing: the title strip and the margins.
    pub l: i32,
    pub t: i32,
    pub r: i32,
    pub b: i32,
    /// A fresh backing's colour, premultiplied ARGB as Cairo stores it.
    pub bg_argb: u32,
    /// The window's scale times 120 (0: 120).
    pub scale120: u32,
    /// `NHG_GEOM_*`.
    pub flags: u32,
    /// The Glitnir layer in the backing: all of it, or a child frame's native area inside its shadow margin.
    pub layer: NhgRect,
    /// The frame's shell window (0: none): `nhg_layer_ensure` lays a child frame's layer out from it (`child.rs`).
    pub window: u64,
}

impl NhgFrameGeom {
    /// The backing's size: the native area and its border, at least 1 x 1.
    pub fn size(&self) -> (i32, i32) {
        (
            1.max(self.native_w.saturating_add(self.l).saturating_add(self.r)),
            1.max(self.native_h.saturating_add(self.t).saturating_add(self.b)),
        )
    }

    fn s120(&self) -> u32 {
        if self.scale120 == 0 {
            120
        } else {
            self.scale120
        }
    }
}

/// A backing's pixels, borrowed: ARGB32, `stride` = 4 * `w`.  Layout equals `NhgBacking`.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct NhgBacking {
    pub pixels: *mut u8,
    pub stride: i32,
    pub w: i32,
    pub h: i32,
    /// Counts the backing's creations: C wraps the pixels again when it changes (`gen` in C; a keyword in Rust 2024).
    pub generation: u64,
}

/// One frame's backing and layer.
#[derive(Debug, Default)]
pub struct Frame {
    /// The layer's size and scale as Glitnir was last told (0: not yet).
    layer_w: i32,
    layer_h: i32,
    layer_s120: u32,
    /// The native area's offset in the backing, as of the last ensure.
    l: i32,
    t: i32,
    /// Where the layer starts in the backing, as of the last ensure (a child frame's shadow margin): the source
    /// publishing names.
    src: (i32, i32),
    pixels: Vec<u32>,
    w: i32,
    h: i32,
    generation: u64,
}

impl Frame {
    fn view(&mut self) -> NhgBacking {
        NhgBacking {
            pixels: self.pixels.as_mut_ptr().cast(),
            stride: self.w * 4,
            w: self.w,
            h: self.h,
            generation: self.generation,
        }
    }

    /// The backing for publishing.  None before the first `backing`.
    pub fn published(&self) -> Option<Published<'_>> {
        if self.pixels.is_empty() {
            return None;
        }
        // SAFETY: the bytes of the `u32`s, read-only, for as long as `self` is borrowed (u8 needs no alignment).
        let pixels = unsafe {
            std::slice::from_raw_parts(self.pixels.as_ptr().cast::<u8>(), self.pixels.len() * 4)
        };
        Some(Published {
            pixels,
            w: self.w,
            h: self.h,
            src: self.src,
        })
    }
}

/// A frame's backing as publishing reads it.
#[derive(Debug, Clone, Copy)]
pub struct Published<'a> {
    /// Premultiplied ARGB32, `4 * w` bytes per row.
    pub pixels: &'a [u8],
    pub w: i32,
    pub h: i32,
    /// Where the layer starts in the backing.
    pub src: (i32, i32),
}

/// Every frame of one display, by key.
#[derive(Debug, Default)]
pub struct Backings {
    frames: BTreeMap<u64, Box<Frame>>,
}

impl Backings {
    /// The frame with key `key`, if it has one.
    pub fn frame(&self, key: u64) -> Option<&Frame> {
        self.frames.get(&key).map(|f| &**f)
    }

    fn get(&mut self, key: u64) -> Result<&mut Frame, i32> {
        self.frames
            .get_mut(&key)
            .map(|f| &mut **f)
            .ok_or(NHG_E_STATE)
    }

    /// Make the frame's Glitnir scene match `g`: one frame layer, the backing's size, at the window's scale.  A frame
    /// without a key (`*key` 0) gets one and a scene.  A new or changed layer damages the whole frame; true then (C
    /// hands a child's chrome over).
    ///
    /// From `nhwl_glitnir_ensure`.
    pub fn ensure(&mut self, key: &mut u64, g: &NhgFrameGeom) -> bool {
        if *key == 0 {
            *key = LAST_ID.fetch_add(1, Ordering::Relaxed) + 1;
            glitnir_root_new(*key);
        }
        let id = *key;
        let f = self.frames.entry(id).or_default();
        let (w, h) = g.size();
        let s120 = g.s120();
        f.l = g.l;
        f.t = g.t;
        f.src = (g.layer.x, g.layer.y);
        if f.layer_w == w
            && f.layer_h == h
            && f.layer_s120 == s120
            && g.flags & NHG_GEOM_LAYER_CHANGED == 0
        {
            return false;
        }
        let d = LayerDesc {
            id,
            parent: 0,
            kind: LAYER_FRAME,
            rect: g.layer.into(),
            scale: f64::from(s120) / 120.0,
            visible: 1,
        };
        // SAFETY: `d` is a readable LayerDesc for the call.
        unsafe { glitnir_layer_upsert(id, &d) };
        glitnir_damage_all(id);
        f.layer_w = w;
        f.layer_h = h;
        f.layer_s120 = s120;
        true
    }

    /// Make the backing `g`'s size.  A resize keeps the old pixels where both sizes overlap and leaves the rest
    /// transparent (as Cairo's SOURCE paint of the old surface did); a fresh backing (`NHG_GEOM_FRESH`, or none yet)
    /// is all `bg_argb`.  The same size and not fresh: the backing as it is.  Reads and writes pixels.
    ///
    /// Adapted in role from `pgtk_cr_update_surface_desired_size` (emacs-31.1), through `nhwl_backing_resize`.
    pub fn backing(&mut self, key: u64, g: &NhgFrameGeom) -> Result<NhgBacking, i32> {
        let f = self.get(key)?;
        let (w, h) = g.size();
        let fresh = g.flags & NHG_GEOM_FRESH != 0 || f.pixels.is_empty();
        if !fresh && f.w == w && f.h == h {
            return Ok(f.view());
        }
        let n = (w as usize).checked_mul(h as usize).ok_or(NHG_E_NOMEM)?;
        let mut px: Vec<u32> = Vec::new();
        px.try_reserve_exact(n).map_err(|_| NHG_E_NOMEM)?;
        if fresh {
            px.resize(n, g.bg_argb);
        } else {
            px.resize(n, 0);
            let (cw, ch) = (w.min(f.w) as usize, h.min(f.h) as usize);
            for y in 0..ch {
                let src = y * f.w as usize;
                px[y * w as usize..][..cw].copy_from_slice(&f.pixels[src..src + cw]);
            }
        }
        f.pixels = px;
        f.w = w;
        f.h = h;
        f.generation += 1;
        Ok(f.view())
    }

    /// Damage `rects` (device pixels, with the native area's origin) of the frame.  Empty rectangles are skipped.
    /// Returns how many were added.
    ///
    /// From `nhwl_damage` (C clips to the native area).
    pub fn damage(&mut self, key: u64, rects: &[NhgRect]) -> Result<i32, i32> {
        let f = self.get(key)?;
        let mut added = 0;
        for r in rects.iter().filter(|r| r.w > 0 && r.h > 0) {
            let moved = Rect {
                x: r.x.saturating_add(f.l),
                y: r.y.saturating_add(f.t),
                w: r.w,
                h: r.h,
            };
            // SAFETY: `moved` is one readable rectangle for the call.
            unsafe { glitnir_damage_add_n(key, &moved, 1) };
            added += 1;
        }
        Ok(added)
    }

    /// Damage the whole frame, its margins and title strip included.  From `nhwl_damage_all`.
    pub fn damage_all(&mut self, key: u64) -> Result<(), i32> {
        self.get(key)?;
        glitnir_damage_all(key);
        Ok(())
    }

    /// The rows `run` (device pixels, with the native area's origin: the window's columns, and the run's first row
    /// and height) move to `to_y`, inside the backing, one row at a time in the direction that overwrites no source
    /// row before it is copied.  Glitnir learns the move and the rows it left stale: both are damaged.  Writes pixels.
    ///
    /// Adapted from `pgtk_scroll_run` (emacs-31.1) and `glitnir_pgtk_scroll`, through `nhwl_scroll_run` and
    /// `nhwl_glitnir_scroll`.
    pub fn scroll(&mut self, key: u64, w_seq: u64, run: NhgRect, to_y: i32) -> Result<(), i32> {
        let f = self.get(key)?;
        if f.pixels.is_empty() {
            return Err(NHG_E_STATE);
        }
        let (x, from_y, width, height) = (run.x, run.y, run.w, run.h);
        let (ox, oy) = (f.l, f.t);
        let x0 = x.max(0) + ox;
        let x1 = f.w.min(x + width + ox);
        let (mut rows, mut sy, mut dy) = (height, from_y + oy, to_y + oy);
        // Clip the copy to the backing, both ends.
        if sy.min(dy) < 0 {
            let cut = -sy.min(dy);
            sy += cut;
            dy += cut;
            rows -= cut;
        }
        if sy.max(dy) + rows > f.h {
            rows = f.h - sy.max(dy);
        }
        if x1 > x0 {
            let stride = f.w as usize;
            let (c0, cn) = (x0 as usize, (x1 - x0) as usize);
            for i in 0..rows.max(0) {
                let r = if dy < sy { i } else { rows - 1 - i };
                let src = (sy + r) as usize * stride + c0;
                let dst = (dy + r) as usize * stride + c0;
                f.pixels.copy_within(src..src + cn, dst);
            }
        }
        let top = from_y.min(to_y);
        let bottom = from_y.max(to_y) + height;
        let area = Rect {
            x: x + ox,
            y: top + oy,
            w: width,
            h: bottom - top,
        };
        let exposed = if to_y < from_y {
            Rect {
                x: x + ox,
                y: to_y + height + oy,
                w: width,
                h: from_y - to_y,
            }
        } else {
            Rect {
                x: x + ox,
                y: from_y + oy,
                w: width,
                h: to_y - from_y,
            }
        };
        glitnir_scroll(key, w_seq | WINDOW_TAG, area, to_y - from_y, exposed);
        Ok(())
    }

    /// The backing as it is, borrowed (C copies it).  Reads pixels.  From `nhwl_snapshot`.
    pub fn snapshot(&mut self, key: u64) -> Result<NhgBacking, i32> {
        let f = self.get(key)?;
        if f.pixels.is_empty() {
            return Err(NHG_E_STATE);
        }
        Ok(f.view())
    }

    /// The frame is gone: its scene, damage and statistics go, and its backing is freed.  From
    /// `nhwl_glitnir_forget`.
    pub fn forget(&mut self, key: u64) {
        if key == 0 {
            return;
        }
        glitnir_root_drop(key);
        glitnir_damage_forget(key);
        glitnir_stats_forget(key);
        self.frames.remove(&key);
    }
}

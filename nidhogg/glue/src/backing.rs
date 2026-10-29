// Upstream commit: a360712c9d272d950d8d8255ef74570f7e90b7d9 (GNU Emacs emacs-31.1), for the "adapted from" notes below.
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

use crate::abi::{NHG_E_ARG, NHG_E_NOMEM, NHG_E_STATE};

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
    /// The frame's windows that are their own outputs (`layers.rs`).
    pub(crate) layers: crate::layers::Layers,
    /// The rows the visible bell inverted, until they are put back (`bell.rs`).
    flash: Vec<Saved>,
    /// What changed since the frame's last publish (backing coordinates).
    since: Since,
    /// The backing as of the frame's last publish (empty: not known).
    shown: Vec<u32>,
}

/// What a frame's damage has been since its last publish: the plain damage (drawing), and the rows any damage
/// touched, for keeping the shown copy.
#[derive(Debug, Default)]
struct Since {
    /// The drawing's rectangles ([`Backings::damage`]).
    plain: Vec<Rect>,
    /// Damage other than the drawing's came (the whole frame, a scroll, the bell): nothing is filtered.
    other: bool,
    /// The rectangles any damage touched, unless `all`.
    touched: Vec<Rect>,
    /// The whole backing may have changed.
    all: bool,
}

/// Past this many rectangles the shown copy is taken whole.
const TOUCHED_MAX: usize = 64;

impl Since {
    fn touch(&mut self, r: Rect) {
        if self.touched.len() >= TOUCHED_MAX {
            self.all = true;
        }
        if !self.all {
            self.touched.push(r);
        }
    }

    fn other(&mut self, rects: &[Rect]) {
        self.other = true;
        rects.iter().for_each(|r| self.touch(*r));
    }

    fn everything(&mut self) {
        self.other = true;
        self.all = true;
    }
}

/// Copy `h` rows of `w` pixels from `src` (rows `w` apart) into `dst` (rows `stride` bytes apart), each pixel's bytes
/// as they lie in memory.  `NHG_E_ARG` when either side is too short.
pub fn copy_rows(
    src: &[u32],
    w: usize,
    h: usize,
    dst: &mut [u8],
    stride: usize,
) -> Result<(), i32> {
    let row = w.checked_mul(4).ok_or(NHG_E_ARG)?;
    let need = match h {
        0 => 0,
        _ => (h - 1)
            .checked_mul(stride)
            .and_then(|n| n.checked_add(row))
            .ok_or(NHG_E_ARG)?,
    };
    if stride < row || dst.len() < need || src.len() < w.saturating_mul(h) {
        return Err(NHG_E_ARG);
    }
    for (i, s) in src.chunks_exact(w.max(1)).take(h).enumerate() {
        let d = &mut dst[i * stride..i * stride + row];
        for (px, out) in s.iter().zip(d.as_chunks_mut::<4>().0) {
            out.copy_from_slice(&px.to_ne_bytes());
        }
    }
    Ok(())
}

/// The frame's background as the backing stores it: `rgb` (0xRRGGBB) at `alpha`, premultiplied ARGB, exactly as
/// Cairo 1.18 paints a solid source with `CAIRO_OPERATOR_SOURCE` on an ARGB32 image (moved from
/// `nhwl_background_argb`'s 1x1 paint): `cairo_pattern_create_rgba` clamps to [0, 1], `_cairo_color_compute_shorts`
/// premultiplies and rounds each channel to 16 bits (`d * 65535.0 + 0.5`, truncated), and the image compositor's
/// `color_to_uint32` keeps each short's high byte.  The colour's 16-bit channels are `nhwl_query_color`'s (8 bits
/// times 257).
pub fn premultiply(rgb: u32, alpha: f64) -> u32 {
    // Cairo's `_cairo_restrict_value`: a NaN stays NaN
    let clamp = |v: f64| v.clamp(0.0, 1.0);
    // a NaN gives 0, as the C conversion did on x86-64
    let short = |d: f64| u32::from((d * 65535.0 + 0.5) as u16);
    let a = clamp(alpha);
    let channel = |s: u32| short(clamp(f64::from(((rgb >> s) & 0xff) * 257) / 65535.0) * a);
    (short(a) >> 8 << 24) | (channel(16) >> 8 << 16) | (channel(8) & 0xff00) | (channel(0) >> 8)
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
        f.since.everything();
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
        // the rows the bell inverted are not where they were
        f.flash.clear();
        f.shown = Vec::new();
        f.since.everything();
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
            f.release(moved);
            // SAFETY: `moved` is one readable rectangle for the call.
            unsafe { glitnir_damage_add_n(key, &moved, 1) };
            f.since.plain.push(moved);
            f.since.touch(moved);
            added += 1;
        }
        Ok(added)
    }

    /// Damage the whole frame, its margins and title strip included.  From `nhwl_damage_all`.
    pub fn damage_all(&mut self, key: u64) -> Result<(), i32> {
        // all of it is drawn again: nothing of the bell's rows is left to put back
        let f = self.get(key)?;
        f.flash.clear();
        f.since.everything();
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
        f.release(area);
        f.since.other(&[area]);
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

    /// Copy the frame's `w` x `h` backing into `dst`, rows `stride` bytes apart (`glitnir-snapshot`'s copy, moved
    /// from `nhwl_snapshot`).  `NHG_E_STATE` without a backing of that size.
    pub fn snapshot_into(
        &mut self,
        key: u64,
        dst: &mut [u8],
        stride: usize,
        w: i32,
        h: i32,
    ) -> Result<(), i32> {
        let f = self.get(key)?;
        if f.pixels.is_empty() || (f.w, f.h) != (w, h) {
            return Err(NHG_E_STATE);
        }
        copy_rows(&f.pixels, w as usize, h as usize, dst, stride)
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

// ---- Window layers (`layers.rs`) -----------------------------------------------------------------------------------

impl Frame {
    /// The native area's offset in the backing, as of the last ensure.
    pub fn origin(&self) -> (i32, i32) {
        (self.l, self.t)
    }

    /// The layer's scale times 120 as Glitnir was last told (0: not yet).
    pub fn scale120(&self) -> u32 {
        self.layer_s120
    }
}

impl Backings {
    /// The frame with key `key`.  `NHG_E_STATE` when it has none.
    pub fn frame_mut(&mut self, key: u64) -> Result<&mut Frame, i32> {
        self.get(key)
    }

    /// Every frame, with its key.
    pub fn frames_mut(&mut self) -> impl Iterator<Item = (u64, &mut Frame)> {
        self.frames.iter_mut().map(|(k, f)| (*k, &mut **f))
    }
}

// ---- The visible bell (`bell.rs`) ----------------------------------------------------------------------------------

/// Rows the visible bell inverted: where they are in the backing, what they held, and which pixels of them are still
/// the bell's (a draw since has made the others its own).
#[derive(Debug)]
struct Saved {
    rect: Rect,
    px: Vec<u32>,
    keep: Vec<bool>,
}

impl Saved {
    /// A draw damaged `r`: the pixels inside it are not the bell's to put back any more.
    fn release(&mut self, r: Rect) {
        let (x0, y0) = (r.x.max(self.rect.x), r.y.max(self.rect.y));
        let x1 = r.x.saturating_add(r.w).min(self.rect.x + self.rect.w);
        let y1 = r.y.saturating_add(r.h).min(self.rect.y + self.rect.h);
        for y in y0..y1 {
            for x in x0..x1 {
                let i = ((y - self.rect.y) * self.rect.w + (x - self.rect.x)) as usize;
                self.keep[i] = false;
            }
        }
    }

    fn any(&self) -> bool {
        self.keep.iter().any(|k| *k)
    }
}

impl Frame {
    /// A draw damaged `r` (backing coordinates).
    fn release(&mut self, r: Rect) {
        if self.flash.is_empty() {
            return;
        }
        for s in &mut self.flash {
            s.release(r);
        }
        self.flash.retain(Saved::any);
    }

    /// Put the inverted rows back, last first, except what a draw has replaced.  The rectangles written, in backing
    /// coordinates.
    fn unflash(&mut self) -> Vec<Rect> {
        let w = self.w as usize;
        let mut out = Vec::new();
        for s in self.flash.drain(..).rev() {
            let rw = s.rect.w as usize;
            let mut any = false;
            for (i, (p, keep)) in s.px.iter().zip(&s.keep).enumerate() {
                if *keep {
                    let (x, y) = (s.rect.x as usize + i % rw, s.rect.y as usize + i / rw);
                    self.pixels[y * w + x] = *p;
                    any = true;
                }
            }
            if any {
                out.push(s.rect);
            }
        }
        out
    }
}

/// Glitnir learns that `rects` (backing coordinates) changed.
fn damage_backing(key: u64, rects: &[Rect]) {
    for r in rects {
        // SAFETY: `r` is one readable rectangle for the call.
        unsafe { glitnir_damage_add_n(key, r, 1) };
    }
}

impl Backings {
    /// Invert `rects` (device pixels, with the native area's origin, clipped to the backing) the way Cairo's
    /// DIFFERENCE with opaque white does on premultiplied pixels (each channel `255 - c`, alpha 255), keep what they
    /// held and damage them.  A flash still showing is put back first.  Returns how many rectangles were inverted.
    /// Writes pixels.
    ///
    /// The rows of `pgtk_flash` (emacs-31.1), inverted in the backing instead of in a copy of the surface.
    pub fn flash(&mut self, key: u64, rects: &[NhgRect]) -> Result<i32, i32> {
        let f = self.get(key)?;
        if f.pixels.is_empty() {
            return Err(NHG_E_STATE);
        }
        let mut changed = f.unflash();
        let (bw, bh) = (f.w, f.h);
        let mut n = 0;
        for r in rects {
            let x0 = r.x.saturating_add(f.l).max(0);
            let y0 = r.y.saturating_add(f.t).max(0);
            let x1 = r.x.saturating_add(f.l).saturating_add(r.w).min(bw);
            let y1 = r.y.saturating_add(f.t).saturating_add(r.h).min(bh);
            if x1 <= x0 || y1 <= y0 {
                continue;
            }
            let rect = Rect {
                x: x0,
                y: y0,
                w: x1 - x0,
                h: y1 - y0,
            };
            let mut px = Vec::with_capacity((rect.w * rect.h) as usize);
            for y in y0..y1 {
                let row = &mut f.pixels[(y * bw + x0) as usize..(y * bw + x1) as usize];
                px.extend_from_slice(row);
                for p in row {
                    *p = 0xff00_0000 | (!*p & 0x00ff_ffff);
                }
            }
            let keep = vec![true; px.len()];
            f.flash.push(Saved { rect, px, keep });
            changed.push(rect);
            n += 1;
        }
        f.since.other(&changed);
        damage_backing(key, &changed);
        Ok(n)
    }

    /// Put the rows the bell inverted back, except pixels a draw has replaced since, and damage them.  Returns how
    /// many rectangles were written (0: nothing to put back).  Writes pixels.
    pub fn unflash(&mut self, key: u64) -> Result<i32, i32> {
        let f = self.get(key)?;
        let rects = f.unflash();
        if !rects.is_empty() {
            f.since.other(&rects);
        }
        damage_backing(key, &rects);
        Ok(rects.len() as i32)
    }
}

// ---- Damage that changed nothing -----------------------------------------------------------------------------------

impl Frame {
    /// The pixels of `r` (backing coordinates) are what the frame's last publish showed.  False without a shown copy.
    fn unchanged(&self, r: &Rect) -> bool {
        if self.shown.len() != self.pixels.len() {
            return false;
        }
        let x0 = r.x.clamp(0, self.w) as usize;
        let x1 = r.x.saturating_add(r.w).clamp(0, self.w) as usize;
        let (y0, y1) = (
            r.y.clamp(0, self.h),
            r.y.saturating_add(r.h).clamp(0, self.h),
        );
        let w = self.w as usize;
        (y0..y1).all(|y| {
            let row = y as usize * w;
            self.pixels[row + x0..row + x1] == self.shown[row + x0..row + x1]
        })
    }
}

/// `inner` lies inside `outer`.
fn inside(outer: &Rect, inner: &Rect) -> bool {
    inner.x >= outer.x
        && inner.y >= outer.y
        && inner.x as i64 + inner.w as i64 <= outer.x as i64 + outer.w as i64
        && inner.y as i64 + inner.h as i64 <= outer.y as i64 + outer.h as i64
}

impl Backings {
    /// Before frame `key` is published: the drawing's damage whose pixels are what the last publish showed goes (a
    /// redisplay that drew the same pixels again, such as the cursor in its place).  Only when the frame had no damage
    /// but the drawing's since: anything else (a scroll, the whole frame, the bell) is published as it came.  Reads
    /// pixels.
    pub fn drop_unchanged(&mut self, key: u64) {
        let Ok(f) = self.get(key) else { return };
        if f.since.other || f.since.plain.is_empty() || f.shown.len() != f.pixels.len() {
            return;
        }
        let changed: Vec<Rect> = f
            .since
            .plain
            .iter()
            .filter(|r| !f.unchanged(r))
            .copied()
            .collect();
        if changed.len() == f.since.plain.len() {
            return;
        }
        // the pending damage goes again without what lies inside an unchanged rectangle; damage C gave Glitnir itself
        // (the title strip) stays
        let dropped: Vec<Rect> = f
            .since
            .plain
            .iter()
            .filter(|r| !changed.contains(r))
            .copied()
            .collect();
        glitnir::damage::with(key, |d| {
            let (rects, _, _) = d.drain();
            for r in rects {
                if !dropped.iter().any(|x| inside(x, &r)) {
                    d.add(r);
                }
            }
        });
        damage_backing(key, &changed);
        f.since.plain = changed;
    }

    /// Frame `key` was published: what it showed is the backing now.  Reads pixels.
    pub fn published(&mut self, key: u64) {
        let Ok(f) = self.get(key) else { return };
        let since = std::mem::take(&mut f.since);
        if f.pixels.is_empty() {
            f.shown = Vec::new();
            return;
        }
        if since.all || f.shown.len() != f.pixels.len() {
            f.shown.clone_from(&f.pixels);
            return;
        }
        let w = f.w as usize;
        for r in &since.touched {
            let x0 = r.x.clamp(0, f.w) as usize;
            let x1 = r.x.saturating_add(r.w).clamp(0, f.w) as usize;
            for y in r.y.clamp(0, f.h)..r.y.saturating_add(r.h).clamp(0, f.h) {
                let row = y as usize * w;
                f.shown[row + x0..row + x1].copy_from_slice(&f.pixels[row + x0..row + x1]);
            }
        }
    }
}

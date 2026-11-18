//! The minimap (state B of the scroll bar): an Emacs window with the parameter `glitnir-minimap` (Lisp resolves it
//! into `glitnir--minimap`) gets its buffer's lines drawn as bars over the outer edge of its text, from the line model
//! (`hlidskjalf`, the text's model by key), with no reflow: the text keeps its width and the minimap covers it.
//!
//! - **The layer:** a Glitnir WINDOW layer per window, id `w->sequence_number | MINIMAP_TAG`, at the minimap's
//!   rectangle, holding a [`glitnir::minimap::Minimap`]: tiles of [`TILE_ROWS`] rows, the band over the rows the
//!   window shows, and the surface under them.  It goes above the window's blocks and edges.
//! - **Rows:** one per visible line (the hidden lines left out), `scale` logical px high, a column one logical px wide.
//!   Only the accessible part (BEGV..ZV) is drawn.  A file taller than the minimap scrolls it in proportion, keeping
//!   the band at the fraction the window is at (VS Code's proportional mode).
//! - **Tiles** are rasterised when first shown and kept by index while the model's generation, the first row, the sizes
//!   and the colour stay; a scroll that shows the same tiles only moves them.
//! - **What C gives ([`NhgMinimap`]):** the text area, the model's key, the window's start and end and the accessible
//!   part's bounds (positions), the flags, the width and rows per line (logical px), and the colours.
//! - **Labels** (design M1): the hub's definitions of the window's file (Sjá's `labels` layer, by the file's name)
//!   and the model's MARK sections, each a sprite C renders (Cairo, the label face) through the renderer set with
//!   [`nhg_minimap_set_label_render`], cached here by text, colour, size and room.  Placed in the crate
//!   ([`glitnir::minimap::place_labels`]): none overlaps another, a deeper one yields; the one under the pointer is in
//!   the accent colour.  A MARK section is in capitals in its colour, with a hairline above.
//! - **When:** at each publish while the frame has blocks, edges, minimaps or their layers ([`sync_at`]); a reconnect
//!   gives them all again ([`forget_all`]).  A shell that shows the cards on layers of its own (macOS) gets its
//!   minimap layers from Naglfar, not from here.

use crate::backing::{Backings, MINIMAP_TAG, NhgRect};
use crate::cards::premultiplied;
use crate::display::Display;
use crate::guard::GlueError;
use glitnir::geom::Rect;
use glitnir::hlidskjalf::{BLANK, Line, RUNS, abi::with_model};
use glitnir::minimap::{
    Band, CLASSES, Candidate, LabelAt, Minimap, Row, RowRun, TILE_ROWS, TileAt, TileSpec,
    place_labels, raster,
};
use glitnir::scene::LayerDesc;
use nhshell::api::CardImage;
use std::collections::{BTreeMap, HashMap};
use std::ffi::c_void;
use std::sync::{Arc, Mutex};

/// Glitnir's `GLITNIR_LAYER_WINDOW`.
const LAYER_WINDOW: u32 = 2;

/// [`NhgMinimap::flags`]: shown (the whole width revealed).
pub const NHG_MINIMAP_OPEN: u32 = 1;
/// [`NhgMinimap::flags`]: on the text's left edge instead of its right.
pub const NHG_MINIMAP_LEFT: u32 = 2;
/// [`NhgMinimap::flags`]: reduced motion: the pointer's open and close are final in their first frame.
pub const NHG_MINIMAP_REDUCED_MOTION: u32 = 4;
/// [`NhgMinimap::flags`]: nesting shades: each fold range of the model a faint band behind its bars, inset 4 logical
/// px per depth, the first [`NESTING_DEPTHS`] depths (design M6).
pub const NHG_MINIMAP_NESTING: u32 = 8;
/// The depths nesting shades are drawn for.
pub const NESTING_DEPTHS: u8 = 4;
/// A nesting shade's opacity: of the bars' colour.
pub const NESTING_OPACITY: f32 = 0.045;

/// The pointer rests on the strip this long before the minimap opens.
pub const OPEN_DELAY_NS: u64 = 150_000_000;
/// The minimap closes this long after the pointer left it.
pub const CLOSE_DELAY_NS: u64 = 600_000_000;
/// The pointer rests on a row this long before a dwell is reported.
pub const DWELL_NS: u64 = 300_000_000;
/// The reveal's length, on the frame clock.
pub const REVEAL_NS: u64 = 120_000_000;
/// A reveal leg's step on the shell's timer (about 120 Hz), beside the frame callbacks'.
pub const STEP_NS: u64 = 8_000_000;

/// [`NhgMinimap::labels`]: the hub's definitions and the MARK sections.
pub const NHG_MINIMAP_LABELS_ALL: u32 = 1;
/// [`NhgMinimap::labels`]: the MARK sections alone.
pub const NHG_MINIMAP_LABELS_MARKS: u32 = 2;

/// A label's height, its font sizes (a definition's, a MARK section's), the room left of it and right of it, and the
/// indent per depth: logical px (the mockup's `.mmap .lab`).
pub const LABEL_H: f64 = 13.0;
pub const LABEL_PX: f64 = 10.5;
pub const MARK_PX: f64 = 9.5;
pub const LABEL_LEFT: f64 = 12.0;
pub const LABEL_RIGHT: f64 = 10.0;
pub const LABEL_INDENT: f64 = 8.0;
/// The deepest indent drawn.
pub const LABEL_DEPTHS: u32 = 4;
/// A MARK section's hairline: its colour's opacity.
pub const MARK_LINE_OPACITY: f32 = 0.45;

/// One window's minimap as C gives it at publish.  Layout equals `NhgMinimap`.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NhgMinimap {
    /// The window's `sequence_number`.
    pub seq: u64,
    /// The window's text area, device px with the native area's origin.
    pub clip: NhgRect,
    /// The buffer text's line model (`nh_lines_model`).
    pub key: u64,
    /// The window's start and end, and the accessible part's bounds: positions.
    pub start: u64,
    pub end: u64,
    pub begv: u64,
    pub zv: u64,
    /// `NHG_MINIMAP_*`.
    pub flags: u32,
    /// The width, logical px.
    pub width: u32,
    /// Rows per line: a line's height, logical px (1 to 3).
    pub scale: u32,
    /// The surface's, the bars' and the band's colours, 0xRRGGBB (-1: none), and their opacities (0 to 1).
    pub fill_rgb: i32,
    pub fill_opacity: f32,
    pub text_rgb: i32,
    pub text_opacity: f32,
    pub band_rgb: i32,
    pub band_opacity: f32,
    /// The window's scroll bar strip (the overview ruler, `glitnir--ruler`), same coordinates as `clip`; empty for
    /// none.  The pointer resting on it opens the minimap.
    pub strip: NhgRect,
    /// `NHG_MINIMAP_LABELS_*`, 0 for no labels.
    pub labels: u32,
    /// The face the labels are rendered in (an Emacs face id) and the frame (C's, passed back to the renderer).
    pub label_face: i32,
    pub host: *mut c_void,
    /// The file's name, `file_len` bytes of UTF-8 (null: none): the key of the hub's labels in Sjá.
    pub file: *const u8,
    pub file_len: usize,
    /// A definition's, a nested definition's, the one under the pointer's and a MARK section's 0xRRGGBB (-1: none).
    pub label_rgb: i32,
    pub sub_rgb: i32,
    pub accent_rgb: i32,
    pub mark_rgb: i32,
    /// Each display class's bar 0xRRGGBB (-1: `text_rgb`'s), at `text_opacity`: the line model's class runs.
    pub class_rgb: [i32; CLASSES],
    /// The hunk labels' 0xRRGGBB (-1: none), and the labels, `hunks_len` of them sorted by line (null: none): drawn
    /// with `labels` on, each at its line's start, placed as a MARK section is.
    pub hunk_rgb: i32,
    pub hunks: *const NhgMinimapHunk,
    pub hunks_len: usize,
    /// The fold pills' 0xRRGGBB (-1: none) and their text's format, `pill_format_len` bytes of UTF-8 with one `%d`
    /// (the lines hidden; null: [`PILL_FORMAT`]): each hidden range of the model gets a dashed pill after its first
    /// line's bar, its name instead when it has one, placed as a MARK section is, with labels off too (design M5).
    pub pill_rgb: i32,
    pub pill_format: *const u8,
    pub pill_format_len: usize,
    /// This window's number, `number_len` bytes of UTF-8 (null: none): another window's minimap on the same text shows
    /// it at the top-right of this window's outlined band (design M8).
    pub number: *const u8,
    pub number_len: usize,
    /// Fold arrows (design F1): `NHG_ARROWS_*` (0: none); the left fringe (same coordinates as `clip`); the chevron's
    /// box and its inset from the fringe's inner edge (device px); an open fold's and a folded one's 0xRRGGBB (-1:
    /// none) at their opacities; and the rows of the window's matrix that start a line, `rows_len` of them.
    pub arrows: u32,
    pub fringe: NhgRect,
    pub arrow_size: u32,
    pub arrow_inset: u32,
    pub arrow_rgb: i32,
    pub arrow_opacity: f32,
    pub folded_rgb: i32,
    pub folded_opacity: f32,
    pub rows: *const NhgMinimapRow,
    pub rows_len: usize,
    /// The heat layer (design X3): its name, `heat_len` bytes of UTF-8 (null: no heat), Sjá's layer `heat:NAME` of
    /// `file`; the ramp's colours at 0 and at 1000, 0xRRGGBB (-1: none) at their opacities.  While the layer exists each
    /// line's bar is one colour mixed between them by its value, a line with no item in the bars' colour.
    pub heat: *const u8,
    pub heat_len: usize,
    pub heat_low_rgb: i32,
    pub heat_low_opacity: f32,
    pub heat_high_rgb: i32,
    pub heat_high_opacity: f32,
    /// Pinned regions (design X5): the line model's mark kind of pins (-1: none), their 0xRRGGBB (-1: none), and their
    /// names in buffer order, `pins_len` of them: each pin mark with an end a band over its rows at `band_opacity`, its
    /// name at its first row, placed as a MARK section is, with labels off too.
    pub pin_kind: i32,
    pub pin_rgb: i32,
    pub pins: *const NhgMinimapName,
    pub pins_len: usize,
    /// The trail (design M4): its 0xRRGGBB (-1: none) at its opacity, and how many of the model's visits are drawn
    /// (1 to 64): a 2 logical px bar at the minimap's left edge on each visit's row, the newest at the opacity, fading to
    /// [`TRAIL_FADE`] of it for the oldest drawn.
    pub trail_rgb: i32,
    pub trail_opacity: f32,
    pub trail_length: u32,
}

/// The oldest trail bar's share of the trail's opacity.
pub const TRAIL_FADE: f32 = 0.15;
/// A trail bar's width, logical px.
pub const TRAIL_W: f64 = 2.0;
/// A jump flash's length (design M9): the row lit in the accent, fading over it (with reduced motion lit, then gone).
pub const FLASH_NS: u64 = 400_000_000;

/// A name as C gives it: `len` bytes of UTF-8.  Layout equals `NhgMinimapName`.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NhgMinimapName {
    pub text: *const u8,
    pub len: usize,
}

/// A row of a window's current matrix that is a line's first screen row: the line's start position, and the row's
/// top and height (same coordinates as `clip`).  Layout equals `NhgMinimapRow`.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct NhgMinimapRow {
    pub pos: u64,
    pub y: i32,
    pub h: i32,
}

/// [`NhgMinimap::arrows`]: shown while the pointer is over the window's left fringe (folded lines always).
pub const NHG_ARROWS_HOVER: u32 = 1;
/// [`NhgMinimap::arrows`]: always shown.
pub const NHG_ARROWS_ALWAYS: u32 = 2;
/// [`NhgMinimap::flags`]: the record is for the fold arrows alone (no minimap is drawn).
pub const NHG_MINIMAP_NONE: u32 = 16;

/// A fold pill's text when C gives no format.
pub const PILL_FORMAT: &str = "⋯ %d folded";
/// A fold pill's room around its text (logical px), its gap after the bar, and its dash and gap.
pub const PILL_PAD: f64 = 3.0;
pub const PILL_GAP: f64 = 4.0;
pub const PILL_DASH: f64 = 2.0;

/// One hunk label as C gives it.  Layout equals `NhgMinimapHunk`.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NhgMinimapHunk {
    /// Its line, from 1.
    pub line: u64,
    /// Its text, `len` bytes of UTF-8; at most [`HUNK_TEXT_MAX`] are drawn.
    pub text: *const u8,
    pub len: usize,
}

/// The longest hunk label text drawn, in bytes (cut at a character boundary).
pub const HUNK_TEXT_MAX: usize = 256;

impl Default for NhgMinimap {
    fn default() -> Self {
        NhgMinimap {
            seq: 0,
            clip: NhgRect::default(),
            key: 0,
            start: 0,
            end: 0,
            begv: 0,
            zv: 0,
            flags: 0,
            width: 0,
            scale: 0,
            fill_rgb: -1,
            fill_opacity: 0.0,
            text_rgb: -1,
            text_opacity: 0.0,
            band_rgb: -1,
            band_opacity: 0.0,
            strip: NhgRect::default(),
            labels: 0,
            label_face: 0,
            host: std::ptr::null_mut(),
            file: std::ptr::null(),
            file_len: 0,
            label_rgb: -1,
            sub_rgb: -1,
            accent_rgb: -1,
            mark_rgb: -1,
            class_rgb: [-1; CLASSES],
            hunk_rgb: -1,
            hunks: std::ptr::null(),
            hunks_len: 0,
            pill_rgb: -1,
            pill_format: std::ptr::null(),
            pill_format_len: 0,
            number: std::ptr::null(),
            number_len: 0,
            arrows: 0,
            fringe: NhgRect::default(),
            arrow_size: 0,
            arrow_inset: 0,
            arrow_rgb: -1,
            arrow_opacity: 0.0,
            folded_rgb: -1,
            folded_opacity: 0.0,
            rows: std::ptr::null(),
            rows_len: 0,
            heat: std::ptr::null(),
            heat_len: 0,
            heat_low_rgb: -1,
            heat_low_opacity: 0.0,
            heat_high_rgb: -1,
            heat_high_opacity: 0.0,
            pin_kind: -1,
            pin_rgb: -1,
            pins: std::ptr::null(),
            pins_len: 0,
            trail_rgb: -1,
            trail_opacity: 0.0,
            trail_length: 0,
        }
    }
}

impl NhgMinimap {
    /// The file's name, when it has one in UTF-8.
    fn file_name(&self) -> Option<&str> {
        if self.file.is_null() || self.file_len == 0 {
            return None;
        }
        // SAFETY: C passes `file_len` readable bytes, valid for the publish this record is part of.
        std::str::from_utf8(unsafe { std::slice::from_raw_parts(self.file, self.file_len) }).ok()
    }

    /// UTF-8 text C gives at `p` for `len` bytes, when it is some.
    fn text_at(p: *const u8, len: usize) -> Option<&'static str> {
        if p.is_null() || len == 0 {
            return None;
        }
        // SAFETY: C passes `len` readable bytes, valid for the publish the record is part of (the lifetime is the
        // caller's to keep within it).
        std::str::from_utf8(unsafe { std::slice::from_raw_parts(p, len) }).ok()
    }

    /// The fold pills' format.
    fn pill_format(&self) -> &str {
        Self::text_at(self.pill_format, self.pill_format_len).unwrap_or(PILL_FORMAT)
    }

    /// The window's number.
    fn number(&self) -> Option<&str> {
        Self::text_at(self.number, self.number_len)
    }

    /// Sjá's heat layer name (`heat:NAME`), when there is heat and a file.
    fn heat_layer(&self) -> Option<String> {
        self.file_name()?;
        Self::text_at(self.heat, self.heat_len).map(|h| format!("heat:{h}"))
    }

    /// The pins' names, in buffer order (a name not in UTF-8 is empty).
    fn pin_names(&self) -> Vec<&str> {
        if self.pins.is_null() || self.pins_len == 0 {
            return Vec::new();
        }
        // SAFETY: C passes `pins_len` records, each with `len` readable bytes, valid for the publish.
        let ps = unsafe { std::slice::from_raw_parts(self.pins, self.pins_len) };
        ps.iter()
            .map(|p| Self::text_at(p.text, p.len).unwrap_or(""))
            .collect()
    }

    /// The matrix rows that start a line.
    fn line_rows(&self) -> &[NhgMinimapRow] {
        if self.rows.is_null() || self.rows_len == 0 {
            return &[];
        }
        // SAFETY: C passes `rows_len` records, valid for the publish this record is part of.
        unsafe { std::slice::from_raw_parts(self.rows, self.rows_len) }
    }

    /// The hunk labels, (line from 1, text) in the order C gives them (by line); a text not in UTF-8 is left out.
    fn hunk_labels(&self) -> Vec<(u64, &str)> {
        if self.hunks.is_null() || self.hunks_len == 0 {
            return Vec::new();
        }
        // SAFETY: C passes `hunks_len` records, each with `len` readable bytes, valid for the publish this record is
        // part of.
        let hs = unsafe { std::slice::from_raw_parts(self.hunks, self.hunks_len) };
        hs.iter()
            .filter(|h| !h.text.is_null())
            .filter_map(|h| {
                // SAFETY: as above, `len` readable bytes at `text`.
                let t = unsafe { std::slice::from_raw_parts(h.text, h.len) };
                let t = std::str::from_utf8(t).ok()?;
                let mut end = t.len().min(HUNK_TEXT_MAX);
                while !t.is_char_boundary(end) {
                    end -= 1;
                }
                Some((h.line, &t[..end]))
            })
            .collect()
    }
}

/// C's label renderer: TEXT (LEN bytes of UTF-8) in face FACE of frame HOST at PX100 / 100 device px, in 0xRRGGBB
/// RGB, drawn into BUF (premultiplied ARGB32, CAP_W × CAP_H, rows of CAP_W pixels, cleared) from its top-left, the
/// text's top at the top; the box it used into *W and *H.  0 when it drew.
pub type NhgLabelRender = unsafe extern "C" fn(
    host: *mut c_void,
    face: i32,
    text: *const u8,
    len: usize,
    px100: u32,
    rgb: u32,
    buf: *mut u32,
    cap_w: i32,
    cap_h: i32,
    w: *mut i32,
    h: *mut i32,
) -> i32;

static RENDER: Mutex<Option<NhgLabelRender>> = Mutex::new(None);

/// A label's sprite key: (text, rgb, px100, room, face).
pub type SpriteKey = (Box<str>, u32, u32, i32, i32);
/// A rendered sprite: width, height, pixels.
pub type Sprite = (u32, u32, Arc<[u32]>);
/// The most sprites a frame keeps.
pub const SPRITES_MAX: usize = 2048;

/// A frame's label sprites (it goes with the frame's minimaps, so with the frame): at most [`SPRITES_MAX`]; when full,
/// the least recently used eighth goes.
#[derive(Debug, Default)]
pub struct SpriteCache {
    map: HashMap<SpriteKey, (Sprite, u64)>,
    tick: u64,
}

impl SpriteCache {
    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    /// Whether KEY is held (not a use).
    pub fn contains(&self, key: &SpriteKey) -> bool {
        self.map.contains_key(key)
    }

    /// KEY's sprite, a use; else RENDER's, kept.
    pub fn get_or_render(
        &mut self,
        key: SpriteKey,
        render: impl FnOnce() -> Option<Sprite>,
    ) -> Option<Sprite> {
        self.tick += 1;
        if let Some((s, t)) = self.map.get_mut(&key) {
            *t = self.tick;
            return Some(s.clone());
        }
        let s = render()?;
        if self.map.len() >= SPRITES_MAX {
            let mut ticks: Vec<u64> = self.map.values().map(|(_, t)| *t).collect();
            ticks.sort_unstable();
            let cut = ticks[SPRITES_MAX / 8 - 1];
            self.map.retain(|_, (_, t)| *t > cut);
        }
        self.map.insert(key, (s.clone(), self.tick));
        Some(s)
    }
}

/// Set the label renderer (null: none).
#[unsafe(no_mangle)]
pub extern "C" fn nhg_minimap_set_label_render(f: Option<NhgLabelRender>) {
    *RENDER.lock().unwrap_or_else(|e| e.into_inner()) = f;
}

/// Naglfar's label raster, called by C's renderer there (`nhmac_minimap_label_render`, which knows the label face's
/// font): TEXT (LEN bytes of UTF-8) in the open font FONT (`nhg_font_open`'s handle) at PX100 / 100 device px in RGB,
/// drawn by Core Text into BUF as [`NhgLabelRender`] draws; 0 when it drew, -1 otherwise (always without Core Text).
///
/// # Safety
/// TEXT holds LEN readable bytes, BUF CAP_W × CAP_H writable words, and W and H are writable (each may be null: -1).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn nhg_label_raster(
    font: u64,
    px100: u32,
    text: *const u8,
    len: usize,
    rgb: u32,
    buf: *mut u32,
    cap_w: i32,
    cap_h: i32,
    w: *mut i32,
    h: *mut i32,
) -> i32 {
    if text.is_null() || buf.is_null() || w.is_null() || h.is_null() || cap_w <= 0 || cap_h <= 0 {
        return -1;
    }
    // SAFETY: the caller's promise for TEXT.
    let Ok(name) = std::str::from_utf8(unsafe { std::slice::from_raw_parts(text, len) }) else {
        return -1;
    };
    // SAFETY: the caller's promise for BUF.
    let px = unsafe { std::slice::from_raw_parts_mut(buf, (cap_w as usize) * (cap_h as usize)) };
    let drawn = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        label_raster(font, f64::from(px100) / 100.0, name, rgb, px, cap_w, cap_h)
    }))
    .ok()
    .flatten();
    let Some((uw, uh)) = drawn else {
        return -1;
    };
    // SAFETY: W and H are writable (the caller's promise).
    unsafe {
        w.write(uw);
        h.write(uh);
    }
    0
}

/// Core Text's label (`nhshell::macos::labels`); none elsewhere.
fn label_raster(
    font: u64,
    px: f64,
    name: &str,
    rgb: u32,
    buf: &mut [u32],
    cap_w: i32,
    cap_h: i32,
) -> Option<(i32, i32)> {
    #[cfg(all(feature = "macos", target_os = "macos"))]
    return nhshell::macos::labels::render(font, px, name, rgb, buf, cap_w, cap_h);
    #[cfg(not(all(feature = "macos", target_os = "macos")))]
    {
        let _ = (font, px, name, rgb, buf, cap_w, cap_h);
        None
    }
}

/// TEXT's sprite for window E in RGB at PX100, at most CAP_W × CAP_H: from CACHE, else rendered; `None` without a
/// renderer or when it did not draw.
#[allow(clippy::too_many_arguments)]
fn sprite(
    cache: &mut SpriteCache,
    e: &NhgMinimap,
    text: &str,
    rgb: u32,
    px100: u32,
    cap_w: i32,
    cap_h: i32,
) -> Option<Sprite> {
    if cap_w <= 0 || cap_h <= 0 {
        return None;
    }
    let key: SpriteKey = (text.into(), rgb, px100, cap_w, e.label_face);
    cache.get_or_render(key, || render_sprite(e, text, rgb, px100, cap_w, cap_h))
}

/// A fold pill's sprite: TEXT rendered in RGB at PX100, `pad` device px of room each side, in a dashed outline one px
/// wide in RGB (dashes of `dash` px), at most CAP_W × H; from CACHE, else built.
#[allow(clippy::too_many_arguments)]
fn pill(
    cache: &mut SpriteCache,
    e: &NhgMinimap,
    text: &str,
    rgb: u32,
    px100: u32,
    cap_w: i32,
    h: i32,
    pad: i32,
    dash: i32,
) -> Option<Sprite> {
    let inner_w = cap_w - 2 * pad;
    if inner_w <= 0 || h <= 2 {
        return None;
    }
    let (tw, th, tpx) = sprite(cache, e, text, rgb, px100, inner_w, h - 2)?;
    let key: SpriteKey = (
        format!("\u{1}pill\u{1}{text}").into(),
        rgb,
        px100,
        cap_w,
        e.label_face,
    );
    cache.get_or_render(key, || {
        let (w, hh) = (tw as i32 + 2 * pad, h);
        let mut buf = vec![0u32; (w * hh) as usize];
        let line = 0xff00_0000 | rgb;
        let on = |i: i32| (i / dash.max(1)) % 2 == 0;
        for x in 0..w {
            if on(x) {
                buf[x as usize] = line;
                buf[((hh - 1) * w + x) as usize] = line;
            }
        }
        for y in 0..hh {
            if on(y) {
                buf[(y * w) as usize] = line;
                buf[(y * w + w - 1) as usize] = line;
            }
        }
        let top = ((hh - th as i32) / 2).max(1);
        for y in 0..(th as i32).min(hh - 1 - top) {
            for x in 0..tw as i32 {
                let v = tpx[(y as u32 * tw + x as u32) as usize];
                if v != 0 {
                    buf[((top + y) * w + pad + x) as usize] = v;
                }
            }
        }
        Some((w as u32, hh as u32, buf.into()))
    })
}

/// Ask C's renderer for TEXT's sprite.
fn render_sprite(
    e: &NhgMinimap,
    text: &str,
    rgb: u32,
    px100: u32,
    cap_w: i32,
    cap_h: i32,
) -> Option<Sprite> {
    let render = (*RENDER.lock().unwrap_or_else(|p| p.into_inner()))?;
    let mut buf = vec![0u32; (cap_w * cap_h) as usize];
    let (mut w, mut h) = (0i32, 0i32);
    // SAFETY: TEXT and BUF are readable and writable for the lengths given; W and H are writable.
    let rc = unsafe {
        render(
            e.host,
            e.label_face,
            text.as_ptr(),
            text.len(),
            px100,
            rgb,
            buf.as_mut_ptr(),
            cap_w,
            cap_h,
            &mut w,
            &mut h,
        )
    };
    if rc != 0 || w <= 0 || h <= 0 {
        return None;
    }
    let (w, h) = (w.min(cap_w), h.min(cap_h));
    let mut px = Vec::with_capacity((w * h) as usize);
    for y in 0..h {
        px.extend_from_slice(&buf[(y * cap_w) as usize..(y * cap_w + w) as usize]);
    }
    Some((w as u32, h as u32, px.into()))
}

/// The rows a window's minimap stands for, as the line model counts them (visible lines of the accessible part).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rows {
    /// The visible line BEGV is on: the minimap's row 0.
    pub first: u64,
    /// How many rows.
    pub total: u64,
    /// The rows the window shows, [from, to).
    pub shown: (u64, u64),
    /// The model's generation.
    pub generation: u64,
}

/// Where a minimap goes and what it shows of its rows, all device px.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Placement {
    /// Its rectangle: `width_px` at the text area's outer edge, the text area's height.
    pub rect: Rect,
    /// How far its rows are scrolled: the minimap's top shows row px `offset`.
    pub offset: u64,
    /// The band over the rows the window shows.
    pub band: Rect,
}

/// Minimap of `width_px` over text area `clip` (root px), on its left edge when `left`, for `rows` of `row_px` each:
/// scrolled in proportion when they are taller than it.
pub fn place(clip: Rect, width_px: i32, left: bool, row_px: u32, rows: &Rows) -> Placement {
    let w = width_px.clamp(0, clip.w.max(0));
    let x = if left { clip.x } else { clip.x + clip.w - w };
    let rect = Rect::new(x, clip.y, w, clip.h);
    let row = u64::from(row_px.max(1));
    let h = clip.h.max(0) as u64;
    let content = rows.total * row;
    let (from, to) = (rows.shown.0.min(rows.total), rows.shown.1.min(rows.total));
    let shown = to.saturating_sub(from);
    let offset = if content <= h {
        0
    } else {
        // the band keeps the window's fraction: at the first row the top, at the last the bottom
        let span = rows.total.saturating_sub(shown).max(1);
        ((content - h) * from.min(span) / span).min(content - h)
    };
    let top = clip.y as i64 + (from * row) as i64 - offset as i64;
    let band = Rect::new(
        x,
        top as i32,
        w,
        ((shown.max(1) * row) as i64).min(i64::from(i32::MAX)) as i32,
    );
    Placement { rect, offset, band }
}

/// The minimap row of model line `l`: a bar from its indent to its end (a blank line none), its class runs over it.
fn row(l: &Line) -> Row {
    if l.indent == BLANK {
        return Row::default();
    }
    let indent = u32::from(l.indent);
    // a run's characters as columns: past the indent's characters, shifted to its columns (a tab is wider)
    let at = |col: u32| {
        let lead = u32::from(l.lead);
        if col >= lead {
            col - lead + indent
        } else {
            col
        }
    };
    let mut runs = [RowRun::default(); RUNS];
    for (o, r) in runs.iter_mut().zip(l.runs.iter().filter(|r| r.len > 0)) {
        *o = RowRun {
            from: at(u32::from(r.col)),
            to: at(u32::from(r.col)) + u32::from(r.len),
            class: r.class,
        };
    }
    // its characters, the newline left out, as columns
    Row {
        indent,
        end: at(l.len.saturating_sub(1)).max(indent + 1),
        class: 0,
        runs,
        heat: None,
    }
}

/// What a window's tiles were drawn for: they hold while it does not change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Stamp {
    key: u64,
    generation: u64,
    first: u64,
    heat: Option<Heat>,
    spec: TileSpec,
}

/// One window's minimap: its tiles by index, what the layer was last given, where it was, and the pointer's state.
#[derive(Debug, Default)]
struct Window {
    stamp: Option<Stamp>,
    tiles: BTreeMap<u64, Arc<[u32]>>,
    applied: Option<(Rect, u32, Arc<Minimap>)>,
    geom: Option<Geom>,
    hover: Hover,
    /// The labels drawn at the last publish: their boxes' top and bottom (root px), and which one was in the accent.
    boxes: Vec<(i32, i32)>,
    accent: Option<usize>,
    /// The pieces last given to a shell that shows the minimap on layers of its own (`minimap_layers`).
    given: crate::minimap_layers::Given,
    /// The fold arrows' fringe (root px) and mode at the last publish, and their layer as last given.
    arrows: Option<(Rect, u32)>,
    arrows_applied: Option<(u32, Arc<Minimap>)>,
    /// A jump flash: the position lit and when it started; whether the window's minimap has reduced motion.
    flash: Option<(u64, u64)>,
    reduced: bool,
    /// Each label box's line start (as `boxes`).
    box_pos: Vec<u64>,
}

/// Where a window's minimap was at its last publish (root device px), for the pointer.
#[derive(Debug, Clone, Copy)]
struct Geom {
    key: u64,
    /// On the text's left edge.
    left: bool,
    rect: Rect,
    /// The part revealed.
    shown: Rect,
    strip: Rect,
    offset: u64,
    row_px: u32,
    rows: Rows,
}

/// The pointer over one window's minimap: opened by it, the reveal's leg, the deadlines, inside or dragging.
#[derive(Debug, Default, Clone, Copy)]
struct Hover {
    /// Opened by the pointer (resting on the strip).
    open: bool,
    /// The revealed fraction at the last publish, and the leg running: start, from, to.
    reveal: f64,
    leg: Option<(u64, f64, f64)>,
    /// When the pointer came to rest on the strip, the minimap closed.
    strip_since: Option<u64>,
    close_at: Option<u64>,
    dwell_at: Option<u64>,
    /// The pointer is over the revealed minimap (Emacs was sent a `LEAVE`).
    inside: bool,
    /// The left button went down on the minimap and is held.
    drag: bool,
    /// The row under the pointer (from the minimap's row 0), and the pointer's y (root px).
    row: Option<u64>,
    y: Option<f64>,
    /// The pointer is over the window's left fringe (the fold arrows show in `NHG_ARROWS_HOVER`).
    fringe: bool,
    /// The pointer's x (root px) over the minimap.
    x: Option<f64>,
}

impl Hover {
    /// The revealed fraction at `now` for the pointer's target, starting a leg when the target changed: with reduced
    /// motion the target at once.
    fn reveal_at(&mut self, now: u64, reduced: bool) -> f64 {
        let to = if self.open { 1.0 } else { 0.0 };
        if reduced {
            self.leg = None;
            self.reveal = to;
            return to;
        }
        match self.leg {
            Some((_, _, t)) if t == to => {}
            _ if self.leg.is_none() && self.reveal == to => return to,
            _ => self.leg = Some((now, self.reveal, to)),
        }
        let (t0, from, to) = self.leg.expect("a leg");
        let k = (now.saturating_sub(t0) as f64 / REVEAL_NS as f64).clamp(0.0, 1.0);
        // ease out (cubic)
        let e = 1.0 - (1.0 - k).powi(3);
        self.reveal = from + (to - from) * e;
        if k >= 1.0 {
            self.leg = None;
            self.reveal = to;
        }
        self.reveal
    }

    fn deadline(&self) -> Option<u64> {
        [
            self.strip_since
                .filter(|_| !self.open)
                .map(|t| t + OPEN_DELAY_NS),
            self.close_at,
            self.dwell_at,
        ]
        .into_iter()
        .flatten()
        .min()
    }
}

/// [`NhgAction::code`] of an `NHG_ACT_MINIMAP` (`crate::action`): publish the frame (no redisplay: the minimap's
/// reveal or state changed); a click, a drag step or a dwell, with the position in `dx` and the Emacs window's
/// `sequence_number` in `dy`.
pub const NHG_MINIMAP_PUBLISH: i32 = 0;
pub const NHG_MINIMAP_CLICK: i32 = 1;
pub const NHG_MINIMAP_DRAG: i32 = 2;
pub const NHG_MINIMAP_DWELL: i32 = 3;
/// A dwell on the marks lane (the ruler strip's width at the minimap's outer edge), at the row's position.
pub const NHG_MINIMAP_DWELL_MARKS: i32 = 4;
/// A dwell on a label, at the label's line start.
pub const NHG_MINIMAP_DWELL_LABEL: i32 = 5;

/// What the pointer did to a frame's minimaps: whether the event is the minimap's (Emacs never sees it), whether
/// Emacs is sent a `LEAVE` first, whether the frame is to be published, and the events for Lisp: (code, window, pos).
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Reply {
    pub swallow: bool,
    pub leave: bool,
    pub publish: bool,
    pub events: Vec<(i32, u64, u64)>,
}

/// A frame's minimap layers, by window `sequence_number`.
#[derive(Debug, Default)]
pub struct Minimaps {
    windows: BTreeMap<u64, Window>,
    /// The labels' sprites, kept while the frame is.
    pub sprites: SpriteCache,
    /// The shell window the frame is in (its tree's root's), as the last sync said: pointer events come by it.
    pub(crate) shell_window: u64,
    /// Where the pointer last moved (root px): a button record carries no position of its own.
    last: Option<(f64, f64)>,
    /// The pieces last given to a shell that shows the minimaps on layers of its own (`cards_on_layers`), without
    /// their images: the other window images' syncs give them again as they are.
    pub sent: Vec<CardImage>,
}

/// Root px point (`x`, `y`) in `r`.
fn within(r: &Rect, x: f64, y: f64) -> bool {
    !r.is_empty()
        && x >= f64::from(r.x)
        && y >= f64::from(r.y)
        && x < f64::from(r.x + r.w)
        && y < f64::from(r.y + r.h)
}

/// The position visible row `row` (from row 0 of the model's `first`) starts at; `None` past the text.
fn pos_of_row(key: u64, first: u64, row: u64) -> Option<u64> {
    with_model(key, |m| {
        let l = m.visible_line(first + row)?;
        m.pos_of(l)
    })
    .flatten()
}

impl Minimaps {
    pub fn len(&self) -> usize {
        self.windows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.windows.is_empty()
    }

    /// A pointer leg runs: the frame is published again at its next frame.
    pub fn animating(&self) -> bool {
        self.windows
            .values()
            .any(|w| w.hover.leg.is_some() || (w.flash.is_some() && !w.reduced))
    }

    /// The earliest deadline of the frame's minimaps (a flash's end among them).
    pub fn deadline(&self) -> Option<u64> {
        self.windows
            .values()
            .flat_map(|w| [w.hover.deadline(), w.flash.map(|(_, t)| t + FLASH_NS)])
            .flatten()
            .min()
    }

    /// Light window `seq`'s row of `pos` for [`FLASH_NS`] from `now` (design M9); false when the window has no minimap
    /// revealed.
    pub fn flash(&mut self, seq: u64, pos: u64, now: u64) -> bool {
        match self.windows.get_mut(&seq) {
            Some(w) if w.applied.as_ref().is_some_and(|(_, _, m)| m.reveal_px > 0) => {
                w.flash = Some((pos, now));
                true
            }
            _ => false,
        }
    }

    /// When the shell's timer is due for the frame's minimaps, at `now`: the earliest deadline, or the next step of a
    /// leg running.  A leg steps on the timer too: a publish without a redisplay need not bring a frame callback.
    pub fn next_at(&self, now: u64) -> Option<u64> {
        let step = self.animating().then_some(now + STEP_NS);
        [self.deadline(), step].into_iter().flatten().min()
    }

    /// The pointer moved to root px (`x`, `y`) at `now` (`None`: it left the window).
    pub fn motion(&mut self, at: Option<(f64, f64)>, now: u64) -> Reply {
        self.last = at;
        let mut r = Reply::default();
        for (seq, w) in &mut self.windows {
            // the fold arrows' fringe: never swallowed (Emacs's fringe clicks fold), a publish on the way in and out
            if let Some((fringe, mode)) = w.arrows {
                let on = at.is_some_and(|(x, y)| within(&fringe, x, y));
                if on != w.hover.fringe {
                    w.hover.fringe = on;
                    r.publish |= mode == NHG_ARROWS_HOVER;
                }
            }
            let Some(g) = w.geom else {
                continue;
            };
            let h = &mut w.hover;
            let over = at.is_some_and(|(x, y)| within(&g.shown, x, y));
            let on_strip = at.is_some_and(|(x, y)| within(&g.strip, x, y));
            if over || h.drag {
                if !h.inside {
                    r.leave = true;
                }
                h.inside = true;
                h.close_at = None;
                r.swallow = true;
                let row = at.map(|(_, y)| {
                    ((y - f64::from(g.rect.y)).max(0.0) as u64 + g.offset)
                        / u64::from(g.row_px.max(1))
                });
                let row = row.map(|v| v.min(g.rows.total.saturating_sub(1)));
                h.y = at.map(|(_, y)| y);
                h.x = at.map(|(x, _)| x);
                // the label under the pointer changed: drawn again in the accent
                if accent_of(&w.boxes, h.y) != w.accent {
                    r.publish = true;
                }
                if row != h.row {
                    h.row = row;
                    h.dwell_at = Some(now + DWELL_NS);
                    if h.drag
                        && let Some(p) = row.and_then(|v| start_pos(&g, v))
                    {
                        r.events.push((NHG_MINIMAP_DRAG, *seq, p));
                    }
                }
                continue;
            }
            if h.inside {
                h.inside = false;
                h.row = None;
                h.y = None;
                h.dwell_at = None;
                if w.accent.is_some() {
                    r.publish = true;
                }
            }
            if on_strip {
                h.strip_since.get_or_insert(now);
                h.close_at = None;
            } else {
                h.strip_since = None;
                if h.open && h.close_at.is_none() {
                    h.close_at = Some(now + CLOSE_DELAY_NS);
                }
            }
        }
        r
    }

    /// A button at root px (`x`, `y`): the left one's press on a revealed minimap is a click (the window scrolls there)
    /// and starts a drag, its release ends it; every button on the minimap is the minimap's.
    pub fn button(&mut self, x: f64, y: f64, left: bool, press: bool, now: u64) -> Reply {
        let mut r = self.motion(Some((x, y)), now);
        for (seq, w) in &mut self.windows {
            let (Some(g), h) = (w.geom, &mut w.hover) else {
                continue;
            };
            if !h.inside {
                continue;
            }
            r.swallow = true;
            if left && press {
                h.drag = true;
                if let Some(p) = h.row.and_then(|v| start_pos(&g, v)) {
                    r.events.push((NHG_MINIMAP_CLICK, *seq, p));
                }
            } else if left {
                h.drag = false;
            }
        }
        r
    }

    /// The deadlines due at `now`: the strip's open, the close, a dwell; and a leg's next step.
    pub fn tick(&mut self, now: u64) -> Reply {
        let mut r = Reply {
            publish: self.animating(),
            ..Reply::default()
        };
        for (seq, w) in &mut self.windows {
            if w.flash.is_some_and(|(_, t)| t + FLASH_NS <= now) {
                w.flash = None;
                r.publish = true;
            }
            let h = &mut w.hover;
            if !h.open && h.strip_since.is_some_and(|t| t + OPEN_DELAY_NS <= now) {
                h.open = true;
                h.strip_since = None;
                r.publish = true;
            }
            if h.close_at.is_some_and(|t| t <= now) {
                h.close_at = None;
                if !h.drag {
                    h.open = false;
                    r.publish = true;
                }
            }
            if h.dwell_at.is_some_and(|t| t <= now) {
                h.dwell_at = None;
                if let (Some(g), Some(row), true) = (w.geom, h.row, h.inside)
                    && let Some(p) = pos_of_row(g.key, g.rows.first, row)
                {
                    // the marks lane at the outer edge, else a label, else the row
                    let lane = g.strip.w.max(0);
                    let outer = if g.left {
                        g.rect.x
                    } else {
                        g.rect.x + g.rect.w - lane
                    };
                    let on_lane =
                        h.x.is_some_and(|x| x >= f64::from(outer) && x < f64::from(outer + lane));
                    let ev = if on_lane {
                        (NHG_MINIMAP_DWELL_MARKS, p)
                    } else if let Some(k) = accent_of(&w.boxes, h.y)
                        && let Some(&lp) = w.box_pos.get(k)
                    {
                        (NHG_MINIMAP_DWELL_LABEL, lp)
                    } else {
                        (NHG_MINIMAP_DWELL, p)
                    };
                    r.events.push((ev.0, *seq, ev.1));
                }
            }
        }
        r
    }
}

/// The label box (of `boxes`, root px top and bottom) the pointer at `y` is in.
fn accent_of(boxes: &[(i32, i32)], y: Option<f64>) -> Option<usize> {
    let y = y?;
    boxes
        .iter()
        .position(|&(a, b)| y >= f64::from(a) && y < f64::from(b))
}

/// Where a label comes from: the hub's definitions, the model's MARK sections or Lisp's hunk labels (the last two
/// placed before any definition).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Def,
    Mark,
    Hunk,
    /// A fold pill (design M5): after its line's bar, at its column (logical columns from the minimap's left).
    Pill(u32),
    /// A pinned region's name (design X5).
    Pin,
}

/// What [`labels`] gives: the sprites placed, the hairlines, each label's rows (top, bottom) and the accented one.
type Labels = (
    Vec<LabelAt>,
    Vec<Band>,
    Vec<(i32, i32)>,
    Option<usize>,
    Vec<u64>,
);

/// The labels of window `e`'s minimap `p` over `rows` (each `row_px` tall) at scale `s`: the sprites placed, and the
/// MARK sections' hairlines; `hover_y` the pointer's y (root px), whose label is in the accent; `back` the backing.
#[allow(clippy::too_many_arguments)]
fn labels(
    e: &NhgMinimap,
    rows: &Rows,
    p: &Placement,
    row_px: u32,
    s: f64,
    hover_y: Option<f64>,
    back: u32,
    cache: &mut SpriteCache,
) -> Labels {
    let none = (Vec::new(), Vec::new(), Vec::new(), None, Vec::new());
    if e.labels == 0 && e.pill_rgb < 0 && (e.pin_kind < 0 || e.pin_rgb < 0) {
        return none;
    }
    let row = u64::from(row_px.max(1));
    let r0 = rows.first + p.offset / row;
    let r1 = rows.first + (p.offset + p.rect.h.max(0) as u64) / row + 1;
    // the lines the minimap shows, and their positions
    let Some((la, lb, pa, pb)) = with_model(e.key, |m| {
        let la = m.visible_line(r0)?;
        let lb = m.visible_line(r1).unwrap_or(u64::MAX);
        let pa = m.pos_of(la)?;
        let pb = if lb == u64::MAX {
            u64::MAX
        } else {
            m.pos_of(lb).unwrap_or(u64::MAX)
        };
        Some((la, lb, pa, pb))
    })
    .flatten() else {
        return none;
    };
    // (line, depth, kind, text)
    let mut found: Vec<(u64, u32, Kind, String)> = Vec::new();
    if e.labels == NHG_MINIMAP_LABELS_ALL
        && let Some(file) = e.file_name()
    {
        glitnir::sja::minimap::with_layer(file, "labels", |_, d| {
            if let glitnir::sja::minimap::Data::Labels(v) = d {
                let a = v.partition_point(|x| u64::from(x.0) < la + 1);
                for x in v[a..]
                    .iter()
                    .take_while(|x| u64::from(x.0) <= lb.saturating_add(1))
                    .take(4096)
                {
                    found.push((u64::from(x.0) - 1, x.1, Kind::Def, x.3.to_string()));
                }
            }
        });
    }
    let placed_rows = with_model(e.key, |m| {
        let marks: Vec<(u64, String)> = if e.labels == 0 {
            Vec::new()
        } else {
            m.sections(pa, pb)
                .iter()
                .take(4096)
                .map(|x| (x.pos, x.name.to_uppercase()))
                .collect()
        };
        for (pos, name) in marks {
            if let Some(l) = m.line_of(pos) {
                found.push((l, 0, Kind::Mark, name));
            }
        }
        // Lisp's hunk labels, the lines shown (none without their colour)
        // the pins' names: the n-th pin by position gets the n-th name, at its first line
        if e.pin_kind >= 0 && e.pin_rgb >= 0 {
            let names = e.pin_names();
            for (pos, name) in pins(m, e.pin_kind).into_iter().zip(names) {
                if !name.is_empty()
                    && pos.0 >= pa
                    && pos.0 < pb
                    && let Some(l) = m.line_of(pos.0)
                {
                    found.push((l, 0, Kind::Pin, name.to_owned()));
                }
            }
        }
        // the fold pills: each hidden range starting on a line shown, its name or the lines it hides
        if e.pill_rgb >= 0 {
            let lo = m.pos_of(la).unwrap_or(pa);
            for (l, n, name) in m.hidden_in(lo, pb).into_iter().take(4096) {
                let text = match name {
                    Some(t) => t.to_string(),
                    None => e.pill_format().replacen("%d", &n.to_string(), 1),
                };
                let end = m.index().map_or(0, |x| self::row(&x.line(l)).end);
                found.push((l, 0, Kind::Pill(end), text));
            }
        }
        for (line, text) in e
            .hunk_labels()
            .into_iter()
            .filter(|_| e.hunk_rgb >= 0 && e.labels > 0)
        {
            let l = line.saturating_sub(1);
            if line > 0 && l >= la && l <= lb {
                found.push((l, 0, Kind::Hunk, text.to_owned()));
            }
        }
        let mut out = Vec::new();
        for (l, depth, mark, text) in found.drain(..) {
            match m.visible_before(l) {
                Some((v, false)) if v >= rows.first => {
                    out.push((v - rows.first, depth, mark, text))
                }
                _ => {}
            }
        }
        out
    })
    .unwrap_or_default();
    let px = |logical: f64| (logical * s).round() as i32;
    let h = px(LABEL_H).max(1);
    let cands: Vec<Candidate> = placed_rows
        .iter()
        .map(|(r, depth, kind, _)| Candidate {
            y: (i64::from(p.rect.y) + (*r * row) as i64 - p.offset as i64)
                .clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32,
            h: h as u32,
            depth: *depth,
            mark: *kind != Kind::Def,
        })
        .collect();
    let chosen = place_labels(&cands, p.rect.y, p.rect.y + p.rect.h, px(1.0).max(1));
    let boxes: Vec<(i32, i32)> = chosen
        .iter()
        .map(|&i| (cands[i].y, cands[i].y + h))
        .collect();
    let accent = accent_of(&boxes, hover_y);
    let box_pos: Vec<u64> = with_model(e.key, |m| {
        chosen
            .iter()
            .map(|&i| {
                m.visible_line(rows.first + placed_rows[i].0)
                    .and_then(|l| m.pos_of(l))
                    .unwrap_or(0)
            })
            .collect()
    })
    .unwrap_or_default();
    let mut out = Vec::new();
    let mut lines = Vec::new();
    for (k, &i) in chosen.iter().enumerate() {
        let (_, depth, kind, text) = &placed_rows[i];
        if let Kind::Pill(end) = *kind {
            let Ok(rgb) = u32::try_from(e.pill_rgb) else {
                continue;
            };
            let x = p.rect.x + end as i32 * px(1.0).max(1) + px(PILL_GAP);
            let cap_w = p.rect.x + p.rect.w - px(LABEL_RIGHT) - x;
            let px100 = (MARK_PX * s * 100.0).round() as u32;
            let dash = px(PILL_DASH).max(1);
            if let Some((w, sh, spx)) = pill(
                cache,
                e,
                text,
                rgb & 0xff_ffff,
                px100,
                cap_w,
                h,
                px(PILL_PAD),
                dash,
            ) {
                out.push(LabelAt {
                    x,
                    y: cands[i].y,
                    w,
                    h: sh,
                    px: spx,
                    back_argb: back,
                });
            }
            continue;
        }
        let mark = *kind == Kind::Mark;
        let rgb = if *kind == Kind::Pin {
            e.pin_rgb
        } else if accent == Some(k) {
            e.accent_rgb
        } else if *kind == Kind::Hunk {
            e.hunk_rgb
        } else if mark {
            e.mark_rgb
        } else if *depth > 0 {
            e.sub_rgb
        } else {
            e.label_rgb
        };
        let Ok(rgb) = u32::try_from(rgb) else {
            continue;
        };
        let indent = if *kind != Kind::Def {
            0
        } else {
            (*depth).min(LABEL_DEPTHS) as i32 * px(LABEL_INDENT)
        };
        let x = p.rect.x + px(LABEL_LEFT) + indent;
        let cap_w = p.rect.x + p.rect.w - px(LABEL_RIGHT) - x;
        let size = if mark { MARK_PX } else { LABEL_PX };
        let px100 = (size * s * 100.0).round() as u32;
        if let Some((w, sh, spx)) = sprite(cache, e, text, rgb & 0xff_ffff, px100, cap_w, h) {
            out.push(LabelAt {
                x,
                y: cands[i].y,
                w,
                h: sh,
                px: spx,
                back_argb: back,
            });
        }
        if mark && let Ok(m) = u32::try_from(e.mark_rgb) {
            lines.push(Band {
                rect: Rect::new(
                    p.rect.x + px(LABEL_LEFT),
                    cands[i].y,
                    (p.rect.w - px(LABEL_LEFT) - px(LABEL_RIGHT)).max(0),
                    px(1.0).max(1),
                ),
                fill_argb: premultiplied(m & 0xff_ffff, MARK_LINE_OPACITY),
                line_argb: 0,
            });
        }
    }
    (out, lines, boxes, accent, box_pos)
}

/// Model `m`'s pins: the marks of kind `kind` with an end, (start, end) by position.
fn pins(m: &mut glitnir::hlidskjalf::Model, kind: i32) -> Vec<(u64, u64)> {
    let mut v: Vec<(u64, u64)> = m
        .marks()
        .iter()
        .filter(|x| i32::from(x.kind) == kind && x.end > x.pos)
        .map(|x| (x.pos, x.end))
        .collect();
    v.sort_unstable();
    v
}

/// The pin bands of window `e`'s minimap `p` over `rows`: each pin a band over its visible rows in the pin colour at the
/// band's opacity.
fn pin_bands(e: &NhgMinimap, rows: &Rows, p: &Placement, row_px: u32) -> Vec<Band> {
    let Ok(rgb) = u32::try_from(e.pin_rgb) else {
        return Vec::new();
    };
    if e.pin_kind < 0 {
        return Vec::new();
    }
    let row = u64::from(row_px.max(1));
    let fill = premultiplied(rgb & 0xff_ffff, e.band_opacity);
    with_model(e.key, |m| {
        let vis = |m: &mut glitnir::hlidskjalf::Model, pos: u64| -> Option<u64> {
            let l = m.line_of(pos)?;
            Some(m.visible_before(l)?.0)
        };
        let mut out = Vec::new();
        for (a, z) in pins(m, e.pin_kind).into_iter().take(4096) {
            let (Some(a), Some(z)) = (vis(m, a), vis(m, z.saturating_sub(1).max(a))) else {
                continue;
            };
            let (a, z) = (
                a.saturating_sub(rows.first),
                z.saturating_sub(rows.first) + 1,
            );
            let y = i64::from(p.rect.y) + (a * row) as i64 - p.offset as i64;
            out.push(Band {
                rect: Rect::new(
                    p.rect.x,
                    y.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32,
                    p.rect.w,
                    ((z.max(a + 1) - a) * row).min(i32::MAX as u64) as i32,
                ),
                fill_argb: fill,
                line_argb: 0,
            });
        }
        out
    })
    .unwrap_or_default()
}

/// The minimap row (from `rows.first`) of position `pos`, when its line is visible.
fn row_of_pos(key: u64, rows: &Rows, pos: u64) -> Option<u64> {
    with_model(key, |m| {
        let l = m.line_of(pos)?;
        match m.visible_before(l)? {
            (v, false) if v >= rows.first => Some(v - rows.first),
            _ => None,
        }
    })
    .flatten()
}

/// Minimap `p`'s rectangle of row `r` (`row_px` tall), across its width.
fn row_rect(p: &Placement, r: u64, row_px: u32) -> Rect {
    let row = u64::from(row_px.max(1));
    let y = i64::from(p.rect.y) + (r * row) as i64 - p.offset as i64;
    Rect::new(
        p.rect.x,
        y.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32,
        p.rect.w,
        row_px.max(1) as i32,
    )
}

/// The trail's bars (design M4): `w_px` wide at minimap `p`'s left edge on the rows of the model's newest
/// `trail_length` visits, the newest at the trail's opacity fading to [`TRAIL_FADE`] of it for the oldest drawn.
fn trail_bars(e: &NhgMinimap, rows: &Rows, p: &Placement, row_px: u32, w_px: i32) -> Vec<Band> {
    if e.trail_rgb < 0 || e.trail_length == 0 {
        return Vec::new();
    }
    let n = e.trail_length.min(64) as usize;
    let visits: Vec<u64> =
        with_model(e.key, |m| m.trail().take(n).copied().collect()).unwrap_or_default();
    let last = visits.len().saturating_sub(1).max(1) as f32;
    visits
        .iter()
        .enumerate()
        .filter_map(|(i, &pos)| {
            let r = row_of_pos(e.key, rows, pos)?;
            let k = 1.0 - (1.0 - TRAIL_FADE) * i as f32 / last;
            let mut rect = row_rect(p, r, row_px);
            rect.w = w_px.min(p.rect.w);
            Some(Band {
                rect,
                fill_argb: colour(e.trail_rgb, e.trail_opacity * k),
                line_argb: 0,
            })
        })
        .collect()
}

/// A heat layer as a window draws it: the layer's version and the ramp's ends and the plain bars' colour (premultiplied).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Heat {
    version: u64,
    low: u32,
    high: u32,
    plain: u32,
}

/// Premultiplied `low` and `high` mixed at `value` / 1000, channel by channel.
pub fn heat_mix(low: u32, high: u32, value: u16) -> u32 {
    let v = u32::from(value.min(1000));
    let ch = |sh: u32| {
        let (a, b) = ((low >> sh) & 0xff, (high >> sh) & 0xff);
        ((a * (1000 - v) + b * v + 500) / 1000) << sh
    };
    ch(24) | ch(16) | ch(8) | ch(0)
}

/// The nesting shades of model `key` for the rows minimap `p` shows: a band per fold range of depth below
/// [`NESTING_DEPTHS`], from its first row to its last, inset `inset_px` per depth, in `argb`.
fn shades(
    key: u64,
    rows: &Rows,
    p: &Placement,
    row_px: u32,
    inset_px: i32,
    argb: u32,
) -> Vec<Band> {
    let row = u64::from(row_px.max(1));
    let r0 = rows.first + p.offset / row;
    let r1 = rows.first + (p.offset + p.rect.h.max(0) as u64) / row + 1;
    with_model(key, |m| {
        let pos = |m: &mut glitnir::hlidskjalf::Model, r: u64| -> Option<u64> {
            let l = m.visible_line(r)?;
            m.pos_of(l)
        };
        let beg = pos(m, r0)?;
        let end = pos(m, r1).unwrap_or(u64::MAX);
        let folds: Vec<_> = m
            .folds(beg, end)
            .filter(|f| f.depth < NESTING_DEPTHS)
            .take(4096)
            .copied()
            .collect();
        let vis = |m: &mut glitnir::hlidskjalf::Model, pos: u64| -> Option<u64> {
            let l = m.line_of(pos)?;
            Some(m.visible_before(l)?.0)
        };
        let mut out = Vec::new();
        for f in folds {
            let (Some(a), Some(z)) = (vis(m, f.beg), vis(m, f.end.saturating_sub(1).max(f.beg)))
            else {
                continue;
            };
            let (a, z) = (
                a.saturating_sub(rows.first),
                z.saturating_sub(rows.first) + 1,
            );
            let inset = inset_px * i32::from(f.depth);
            let y = i64::from(p.rect.y) + (a * row) as i64 - p.offset as i64;
            out.push(Band {
                rect: Rect::new(
                    p.rect.x + inset,
                    y.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32,
                    (p.rect.w - inset).max(0),
                    ((z - a) * row).min(i32::MAX as u64) as i32,
                ),
                fill_argb: argb,
                line_argb: 0,
            });
        }
        Some(out)
    })
    .flatten()
    .unwrap_or_default()
}

/// The window start that puts row `row` in the middle of the window (a click or a drag step there).
fn start_pos(g: &Geom, row: u64) -> Option<u64> {
    let shown = g.rows.shown.1.saturating_sub(g.rows.shown.0);
    pos_of_row(g.key, g.rows.first, row.saturating_sub(shown / 2))
}

/// The layer id of window `seq`'s minimap.
pub fn minimap_id(seq: u64) -> u64 {
    seq | MINIMAP_TAG
}

/// The fold arrows' layer bit, beside [`MINIMAP_TAG`].
pub const ARROWS_TAG: u64 = 1 << 57;

/// The layer id of window `seq`'s fold arrows.
pub fn arrows_id(seq: u64) -> u64 {
    seq | MINIMAP_TAG | ARROWS_TAG
}

/// A chevron in a `size` px square box in `argb` (premultiplied): pointing down (an open fold), or right when `folded`.
pub fn chevron(size: u32, folded: bool, argb: u32) -> Arc<[u32]> {
    let n = size as usize;
    let s = f64::from(size);
    let pts = if folded {
        [(0.35, 0.2), (0.65, 0.5), (0.35, 0.8)]
    } else {
        [(0.2, 0.35), (0.5, 0.65), (0.8, 0.35)]
    };
    let pts = pts.map(|(x, y)| (x * s, y * s));
    let half = (s / 14.0).max(0.5) + 0.25;
    let near = |px: f64, py: f64, a: (f64, f64), b: (f64, f64)| {
        let (dx, dy) = (b.0 - a.0, b.1 - a.1);
        let t = (((px - a.0) * dx + (py - a.1) * dy) / (dx * dx + dy * dy)).clamp(0.0, 1.0);
        let (cx, cy) = (a.0 + t * dx - px, a.1 + t * dy - py);
        (cx * cx + cy * cy).sqrt() <= half
    };
    let mut buf = vec![0u32; n * n];
    for y in 0..n {
        for x in 0..n {
            let (cx, cy) = (x as f64 + 0.5, y as f64 + 0.5);
            if near(cx, cy, pts[0], pts[1]) || near(cx, cy, pts[1], pts[2]) {
                buf[y * n + x] = argb;
            }
        }
    }
    buf.into()
}

/// Window `e`'s fold arrows (design F1) with the layer origin `origin`: a chevron in its fringe on each matrix row that
/// starts a line where a fold range starts, right-pointing in the folded colour when the range's lines are hidden; the
/// open ones only while the pointer is over the fringe in `NHG_ARROWS_HOVER`.  `None` without arrows or a fringe.
fn arrows(e: &NhgMinimap, origin: (i32, i32), win: &mut Window) -> Option<Minimap> {
    let fringe = Rect::new(
        e.fringe.x.saturating_add(origin.0),
        e.fringe.y.saturating_add(origin.1),
        e.fringe.w,
        e.fringe.h,
    );
    if e.arrows == 0 || fringe.is_empty() || e.arrow_size == 0 {
        win.arrows = None;
        return None;
    }
    win.arrows = Some((fringe, e.arrows));
    let shown_open = e.arrows == NHG_ARROWS_ALWAYS || win.hover.fringe;
    let size = e.arrow_size.min(fringe.h.max(0) as u32);
    let x = fringe.x + fringe.w - e.arrow_inset as i32 - size as i32;
    let rows = e.line_rows();
    let marks: Vec<(usize, bool)> = with_model(e.key, |m| {
        let mut out = Vec::new();
        for (i, r) in rows.iter().enumerate().take(4096) {
            let Some(l) = m.line_of(r.pos) else {
                continue;
            };
            let (Some(a), b) = (m.pos_of(l), m.pos_of(l + 1).unwrap_or(u64::MAX)) else {
                continue;
            };
            if !m.folds(a, b).any(|f| f.beg >= a && f.beg < b) {
                continue;
            }
            out.push((i, !m.hidden_in(a, b).is_empty()));
        }
        out
    })
    .unwrap_or_default();
    let mut labels = Vec::new();
    for (i, folded) in marks {
        if !folded && !shown_open {
            continue;
        }
        let (rgb, op) = if folded {
            (e.folded_rgb, e.folded_opacity)
        } else {
            (e.arrow_rgb, e.arrow_opacity)
        };
        if rgb < 0 {
            continue;
        }
        let r = rows[i];
        labels.push(LabelAt {
            x,
            y: r.y.saturating_add(origin.1) + (r.h - size as i32) / 2,
            w: size,
            h: size,
            px: chevron(size, folded, colour(rgb, op)),
            back_argb: 0,
        });
    }
    Some(Minimap {
        rect: fringe,
        reveal_px: fringe.w.max(0) as u32,
        left: true,
        fill_argb: 0,
        tiles: Arc::from([]),
        bands: Arc::from([]),
        shades: Arc::from([]),
        labels: labels.into(),
    })
}

/// Model `key`'s rows for a window from `start` to `end` of the accessible part `begv`..`zv`; `None` while it has
/// no lines (no model, or being built).
pub fn rows_of(key: u64, start: u64, end: u64, begv: u64, zv: u64) -> Option<Rows> {
    with_model(key, |m| {
        let vis = |m: &mut glitnir::hlidskjalf::Model, pos: u64| -> Option<u64> {
            let l = m.line_of(pos)?;
            Some(m.visible_before(l)?.0)
        };
        let first = vis(m, begv)?;
        let last = vis(m, zv)?;
        let s = vis(m, start.clamp(begv, zv))?;
        // the window's end is past its last line's end: that line is the one before it
        let e = vis(m, end.saturating_sub(1).clamp(start.clamp(begv, zv), zv))?;
        Some(Rows {
            first,
            total: last.saturating_sub(first) + 1,
            shown: (s.saturating_sub(first), e.saturating_sub(first) + 1),
            generation: m.generation(),
        })
    })
    .flatten()
}

/// Model `key`'s tile `t` for rows from `first`: [`TILE_ROWS`] visible rows rasterised.
fn tile(
    key: u64,
    first: u64,
    t: u64,
    spec: &TileSpec,
    heat: Option<(&str, &str, Heat)>,
) -> Arc<[u32]> {
    let r0 = first + t * TILE_ROWS as u64;
    let (lines, nums) = with_model(key, |m| {
        let lines = m.visible_rows(r0, TILE_ROWS)?;
        let nums: Vec<u64> = if heat.is_some() {
            (0..lines.len() as u64)
                .map(|i| m.visible_line(r0 + i).unwrap_or(u64::MAX))
                .collect()
        } else {
            Vec::new()
        };
        Some((lines, nums))
    })
    .flatten()
    .unwrap_or_default();
    let mut rows: Vec<Row> = lines.iter().map(row).collect();
    if let Some((file, layer, h)) = heat {
        glitnir::sja::minimap::with_layer(file, layer, |_, d| {
            if let glitnir::sja::minimap::Data::Heat(v) = d {
                for (r, &l) in rows.iter_mut().zip(&nums) {
                    // lines count from 1 in Sjá: the last item starting at or before the line, when it reaches it
                    let line = l.saturating_add(1);
                    let i = v.partition_point(|x| u64::from(x.0) <= line);
                    r.heat = Some(match i.checked_sub(1).map(|i| &v[i]) {
                        Some(x) if u64::from(x.1) >= line => heat_mix(h.low, h.high, x.2),
                        _ => h.plain,
                    });
                }
            }
        });
    }
    raster(spec, &rows).into()
}

/// A colour from C: 0xRRGGBB (negative: none) at an opacity, premultiplied.
fn colour(rgb: i32, opacity: f32) -> u32 {
    u32::try_from(rgb).map_or(0, |c| premultiplied(c & 0xff_ffff, opacity))
}

/// Window `e`'s minimap at scale `s120` with the layer origin `origin`, its tiles from (and kept in) `win`; `None`
/// while its model has no lines.
fn minimap(
    e: &NhgMinimap,
    s120: u32,
    origin: (i32, i32),
    win: &mut Window,
    now: u64,
    sprites: &mut SpriteCache,
) -> Option<Minimap> {
    let s = f64::from(s120) / 120.0;
    let px = |logical: f64| (logical * s).round().max(1.0) as u32;
    let clip = Rect::new(
        e.clip.x.saturating_add(origin.0),
        e.clip.y.saturating_add(origin.1),
        e.clip.w,
        e.clip.h,
    );
    let rows = rows_of(e.key, e.start, e.end, e.begv, e.zv)?;
    let row_px = px(f64::from(e.scale.clamp(1, 3)));
    let left = e.flags & NHG_MINIMAP_LEFT != 0;
    let p = place(clip, px(f64::from(e.width)) as i32, left, row_px, &rows);
    let spec = TileSpec {
        width: p.rect.w.max(0) as u32,
        row_px,
        col_px: px(1.0),
        argb: std::array::from_fn(|c| {
            let rgb = if c > 0 && e.class_rgb[c] >= 0 {
                e.class_rgb[c]
            } else {
                e.text_rgb
            };
            colour(rgb, e.text_opacity)
        }),
    };
    // the heat layer, while it exists: its version is part of what the tiles were drawn for
    let layer = e.heat_layer();
    let file = e.file_name().unwrap_or("");
    let heat = layer.as_deref().and_then(|l| {
        let version = glitnir::sja::minimap::with_layer(file, l, |v, d| {
            matches!(d, glitnir::sja::minimap::Data::Heat(_)).then_some(v)
        })
        .flatten()?;
        Some(Heat {
            version,
            low: colour(e.heat_low_rgb, e.heat_low_opacity),
            high: colour(e.heat_high_rgb, e.heat_high_opacity),
            plain: colour(e.text_rgb, e.text_opacity),
        })
    });
    let stamp = Stamp {
        key: e.key,
        generation: rows.generation,
        first: rows.first,
        heat,
        spec,
    };
    if win.stamp != Some(stamp) {
        win.stamp = Some(stamp);
        win.tiles.clear();
    }
    // Lisp's open is a setting, at once; the pointer's reveals on the frame clock
    let reduced = e.flags & NHG_MINIMAP_REDUCED_MOTION != 0;
    let hover = win.hover.reveal_at(now, reduced);
    let frac = if e.flags & NHG_MINIMAP_OPEN != 0 {
        1.0
    } else {
        hover
    };
    let reveal_px = (f64::from(p.rect.w.max(0)) * frac).round() as u32;
    let mut tiles = Vec::new();
    if reveal_px > 0 && rows.total > 0 {
        let tile_px = TILE_ROWS as u64 * u64::from(row_px);
        let last_row_px = rows.total * u64::from(row_px);
        let bottom = (p.offset + p.rect.h as u64).min(last_row_px);
        let (t0, t1) = (p.offset / tile_px, bottom.saturating_sub(1) / tile_px);
        for t in t0..=t1 {
            let px = Arc::clone(win.tiles.entry(t).or_insert_with(|| {
                let h = heat.and_then(|h| Some((file, layer.as_deref()?, h)));
                tile(e.key, rows.first, t, &spec, h)
            }));
            let h = (px.len() / spec.width.max(1) as usize) as u32;
            tiles.push(TileAt {
                y: (i64::from(p.rect.y) + (t * tile_px) as i64 - p.offset as i64) as i32,
                h,
                px,
            });
        }
        // tiles scrolled out are dropped: a file of many screens keeps only what is near
        win.tiles.retain(|t, _| *t + 2 >= t0 && *t <= t1 + 2);
    }
    let band = colour(e.band_rgb, e.band_opacity);
    let shades = if e.flags & NHG_MINIMAP_NESTING != 0 && reveal_px > 0 {
        shades(
            e.key,
            &rows,
            &p,
            row_px,
            px(4.0) as i32,
            colour(e.text_rgb, e.text_opacity * NESTING_OPACITY),
        )
    } else {
        Vec::new()
    };
    let fill = colour(e.fill_rgb, e.fill_opacity);
    let (labels, hairlines, boxes, accent, box_pos) = if reveal_px > 0 {
        labels(e, &rows, &p, row_px, s, win.hover.y, fill, sprites)
    } else {
        (Vec::new(), Vec::new(), Vec::new(), None, Vec::new())
    };
    win.boxes = boxes;
    win.accent = accent;
    win.box_pos = box_pos;
    win.reduced = reduced;
    let mut bands = hairlines;
    if reveal_px > 0 {
        bands.extend(pin_bands(e, &rows, &p, row_px));
        bands.extend(trail_bars(e, &rows, &p, row_px, px(TRAIL_W) as i32));
    }
    // a jump flash: its row in the accent, fading on the frame clock (with reduced motion lit, then gone)
    if let Some((pos, t0)) = win.flash {
        let age = now.saturating_sub(t0);
        if age >= FLASH_NS {
            win.flash = None;
        } else if reveal_px > 0
            && let Some(r) = row_of_pos(e.key, &rows, pos)
        {
            let k = if reduced {
                1.0
            } else {
                1.0 - age as f32 / FLASH_NS as f32
            };
            bands.push(Band {
                rect: row_rect(&p, r, row_px),
                fill_argb: colour(e.accent_rgb, k),
                line_argb: 0,
            });
        }
    }
    bands.push(Band {
        rect: p.band,
        fill_argb: band,
        line_argb: colour(e.band_rgb, (e.band_opacity * 2.0).min(1.0)),
    });
    let m = Minimap {
        rect: p.rect,
        reveal_px,
        left,
        fill_argb: fill,
        tiles: tiles.into(),
        bands: bands.into(),
        shades: shades.into(),
        labels: labels.into(),
    };
    win.geom = Some(Geom {
        key: e.key,
        left,
        rect: p.rect,
        shown: m.shown(),
        strip: Rect::new(
            e.strip.x.saturating_add(origin.0),
            e.strip.y.saturating_add(origin.1),
            e.strip.w,
            e.strip.h,
        ),
        offset: p.offset,
        row_px,
        rows,
    });
    Some(m)
}

/// Frame `key` is published with the minimaps `list`: each window with one gets (or keeps) its minimap layer, those
/// of windows without one (or whose model has no lines yet) leave the scene, and the minimap layers go on top of the
/// frame's other layers.  Returns the number of minimap layers.  A frame without a scale yet has none.
pub fn sync_at(b: &mut Backings, key: u64, list: &[NhgMinimap]) -> Result<u32, i32> {
    sync_at_time(b, key, list, now_ns())
}

/// `CLOCK_MONOTONIC` nanoseconds (the shell's clock for event times and the timer).
pub fn now_ns() -> u64 {
    let mut ts = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: `ts` is writable.
    unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut ts) };
    ts.tv_sec as u64 * 1_000_000_000 + ts.tv_nsec as u64
}

/// The frame's minimaps planned by [`plan`]: each window's by its `sequence_number`, its fold arrows' likewise, the scale
/// (×120) and the origin.
type Planned = (
    BTreeMap<u64, Minimap>,
    BTreeMap<u64, Minimap>,
    u32,
    (i32, i32),
);

/// Frame `key`'s minimaps from C's `list` at `now`: each window's [`Minimap`] (the other windows on its text add
/// their bands); `None` before the frame has a scale.
fn plan(b: &mut Backings, key: u64, list: &[NhgMinimap], now: u64) -> Result<Option<Planned>, i32> {
    let f = b.frame_mut(key)?;
    let (s120, origin) = (f.scale120(), f.origin());
    if s120 == 0 {
        return Ok(None);
    }
    let mm = &mut f.minimaps;
    let mut planned: BTreeMap<u64, Minimap> = BTreeMap::new();
    let mut arrows_planned: BTreeMap<u64, Minimap> = BTreeMap::new();
    for e in list {
        if let Some(file) = e.file_name() {
            glitnir::minimap::take_hub_folds(e.key, file);
        }
        let win = mm.windows.entry(e.seq).or_default();
        if let Some(a) = arrows(e, origin, win) {
            arrows_planned.insert(e.seq, a);
        }
        if e.flags & NHG_MINIMAP_NONE != 0 {
            win.geom = None;
        } else if let Some(m) = minimap(e, s120, origin, win, now, &mut mm.sprites) {
            planned.insert(e.seq, m);
        }
    }
    // every other window on the same text: its band outlined (design M8)
    for e in list {
        let Some(g) = mm.windows.get(&e.seq).and_then(|w| w.geom) else {
            continue;
        };
        let Some(m) = planned.get_mut(&e.seq) else {
            continue;
        };
        let line = colour(e.band_rgb, (e.band_opacity * 2.0).min(1.0));
        let mut bands = m.bands.to_vec();
        let mut labels = m.labels.to_vec();
        let s = f64::from(s120) / 120.0;
        let px = |logical: f64| (logical * s).round() as i32;
        for o in list
            .iter()
            .filter(|o| o.key == e.key && o.seq != e.seq && o.flags & NHG_MINIMAP_NONE == 0)
        {
            let Some(r) = rows_of(e.key, o.start, o.end, e.begv, e.zv) else {
                continue;
            };
            let row = u64::from(g.row_px.max(1));
            let y = i64::from(g.rect.y) + (r.shown.0 * row) as i64 - g.offset as i64;
            // its number at the band's top-right, in the label face and the band's colour (design M8)
            if let (Some(n), Ok(rgb)) = (o.number(), u32::try_from(e.band_rgb))
                && m.reveal_px > 0
            {
                let cap_w = (g.rect.w - px(LABEL_LEFT) - px(LABEL_RIGHT)).max(0);
                let h = px(LABEL_H).max(1);
                let px100 = (LABEL_PX * s * 100.0).round() as u32;
                if let Some((w, sh, spx)) =
                    sprite(&mut mm.sprites, e, n, rgb & 0xff_ffff, px100, cap_w, h)
                {
                    labels.push(LabelAt {
                        x: g.rect.x + g.rect.w - px(LABEL_RIGHT) - w as i32,
                        y: y.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32,
                        w,
                        h: sh,
                        px: spx,
                        back_argb: m.fill_argb,
                    });
                }
            }
            bands.push(Band {
                rect: Rect::new(
                    g.rect.x,
                    y.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32,
                    g.rect.w,
                    ((r.shown.1.saturating_sub(r.shown.0)).max(1) * row).min(i32::MAX as u64)
                        as i32,
                ),
                fill_argb: 0,
                line_argb: line,
            });
        }
        m.bands = bands.into();
        m.labels = labels.into();
    }
    Ok(Some((planned, arrows_planned, s120, origin)))
}

/// [`sync_at`] at `now` (the pointer's legs are read at it).
pub fn sync_at_time(b: &mut Backings, key: u64, list: &[NhgMinimap], now: u64) -> Result<u32, i32> {
    let Some((planned, arrows_planned, s120, _)) = plan(b, key, list, now)? else {
        return Ok(0);
    };
    let s = f64::from(s120) / 120.0;
    let mm = &mut b.frame_mut(key)?.minimaps;
    glitnir::scene::with(key, |sc| {
        for (seq, m) in &planned {
            let win = mm.windows.get_mut(seq).expect("planned from it");
            if let Some((r, q, old)) = &win.applied
                && *r == m.rect
                && *q == s120
                && **old == *m
            {
                continue;
            }
            let id = minimap_id(*seq);
            sc.upsert(&LayerDesc {
                id,
                parent: key,
                kind: LAYER_WINDOW,
                rect: m.rect,
                scale: s,
                visible: 1,
            })?;
            let m = Arc::new(m.clone());
            sc.set_minimap(id, Some(Arc::clone(&m)))?;
            win.applied = Some((m.rect, s120, m));
        }
        for (seq, a) in &arrows_planned {
            let win = mm.windows.get_mut(seq).expect("planned from it");
            if let Some((q, old)) = &win.arrows_applied
                && *q == s120
                && **old == *a
            {
                continue;
            }
            let id = arrows_id(*seq);
            sc.upsert(&LayerDesc {
                id,
                parent: key,
                kind: LAYER_WINDOW,
                rect: a.rect,
                scale: s,
                visible: 1,
            })?;
            let a = Arc::new(a.clone());
            sc.set_minimap(id, Some(Arc::clone(&a)))?;
            win.arrows_applied = Some((s120, a));
        }
        let seqs: Vec<u64> = mm.windows.keys().copied().collect();
        for seq in seqs {
            let (m, a) = (
                planned.contains_key(&seq),
                arrows_planned.contains_key(&seq),
            );
            let win = mm.windows.get_mut(&seq).expect("listed");
            if !m && win.applied.take().is_some() {
                sc.remove(minimap_id(seq));
            }
            if !a && win.arrows_applied.take().is_some() {
                sc.remove(arrows_id(seq));
            }
            if !m && !a {
                mm.windows.remove(&seq);
            }
        }
        // the minimap layers on top of the frame's children, edges included
        let kids = sc.children(key).to_vec();
        let is_mm = |id: &u64| id & MINIMAP_TAG != 0;
        if let (Some(first), Some(last)) = (
            kids.iter().position(is_mm),
            kids.iter().rposition(|id| !is_mm(id)),
        ) && first < last
        {
            let order: Vec<u64> = kids.iter().filter(|id| is_mm(id)).copied().collect();
            for id in order {
                sc.raise(id)?;
            }
        }
        Ok(())
    })?;
    Ok(u32::try_from(mm.windows.len()).unwrap_or(u32::MAX))
}

/// [`sync_at`] for a shell that shows window images on layers of its own (`cards_on_layers`, Naglfar): each window's
/// minimap as pieces (`minimap_layers`), placed in the frame's native area, at `now`; `None` when no piece came, went,
/// moved or changed (or the frame has no scale yet).  Windows without a minimap are forgotten.
pub fn images_at(
    b: &mut Backings,
    key: u64,
    list: &[NhgMinimap],
    now: u64,
) -> Result<Option<Vec<CardImage>>, i32> {
    let Some((planned, _, _, origin)) = plan(b, key, list, now)? else {
        return Ok(None);
    };
    let mm = &mut b.frame_mut(key)?.minimaps;
    mm.windows.retain(|q, _| planned.contains_key(q));
    let mut out = Vec::new();
    for (seq, m) in &planned {
        let win = mm.windows.get_mut(seq).expect("planned from it");
        out.extend(crate::minimap_layers::cards(
            *seq,
            m,
            origin,
            &mut win.given,
        ));
    }
    let same = out.len() == mm.sent.len()
        && out.iter().zip(&mm.sent).all(|(a, b)| {
            a.argb.is_none() && a.id == b.id && a.rect == b.rect && a.shown == b.shown
        });
    Ok((!same).then_some(out))
}

/// The pieces last given, as they are (no image): what the other window images' syncs put after their own.
pub(crate) fn kept(b: &mut Backings, key: u64) -> Vec<CardImage> {
    b.frame_mut(key)
        .map(|f| f.minimaps.sent.clone())
        .unwrap_or_default()
}

/// [`sync_at`] for frame `frame` of shell window `window`, or on a shell that shows window images on layers of its
/// own, [`images_at`] given with the frame's cards, blocks and edges as they are, the minimaps last (on top).
pub fn sync(
    state: &mut Display,
    window: u64,
    frame: u64,
    list: &[NhgMinimap],
) -> Result<u32, GlueError> {
    if matches!(state.csd_parts(window), Ok((backend, ..)) if backend.cards_on_layers()) {
        if let Some(pieces) = images_at(&mut state.backings, frame, list, now_ns())? {
            let (cards, blocks, edges) = state
                .backings
                .frame_mut(frame)
                .map(|f| {
                    (
                        f.cards.sent.clone(),
                        f.blocks.sent.clone(),
                        f.edges.sent.clone(),
                    )
                })
                .unwrap_or_default();
            let mut full: Vec<CardImage> = cards
                .iter()
                .chain(blocks.iter())
                .chain(edges.iter())
                .map(|&(id, rect)| CardImage {
                    id,
                    rect,
                    argb: None,
                    shown: None,
                })
                .collect();
            full.extend(pieces.iter().cloned());
            let given = match state.csd_parts(window) {
                Ok((backend, ..)) => backend.set_cards(window, &full).is_ok(),
                Err(_) => false,
            };
            let mm = &mut state.backings.frame_mut(frame)?.minimaps;
            if given {
                mm.sent = pieces
                    .into_iter()
                    .map(|c| CardImage { argb: None, ..c })
                    .collect();
            } else {
                // refused: every piece given again, with its image, at the next publish
                mm.sent.clear();
                for w in mm.windows.values_mut() {
                    w.given = Default::default();
                }
            }
        }
        if let Ok(f) = state.backings.frame_mut(frame) {
            f.minimaps.shell_window = window;
        }
        rearm(state);
        return Ok(u32::try_from(count(&mut state.backings, frame)).unwrap_or(u32::MAX));
    }
    let n = sync_at(&mut state.backings, frame, list)?;
    if let Ok(f) = state.backings.frame_mut(frame) {
        f.minimaps.shell_window = window;
    }
    rearm(state);
    Ok(n)
}

/// The frame key whose minimaps are in shell window `window`.
fn frame_of(state: &mut Display, window: u64) -> Option<u64> {
    state
        .backings
        .frames_mut()
        .find(|(_, f)| f.minimaps.shell_window == window && !f.minimaps.is_empty())
        .map(|(k, _)| k)
}

/// Point the shell's timer at the earliest deadline of every frame's minimaps.
fn rearm(state: &mut Display) {
    let at = state
        .backings
        .frames_mut()
        .filter_map(|(_, f)| f.minimaps.next_at(now_ns()))
        .min();
    // a shell without a timer has minimaps opened by Lisp only
    let _ = state.backend().set_timer(at);
}

/// Turn `reply` for shell window `window` into actions: a `LEAVE` first, the events, then a publish.
fn apply(state: &mut Display, window: u64, time_ns: u64, reply: &Reply) {
    use crate::action::{NHG_ACT_LEAVE, NHG_ACT_MINIMAP, NhgAction};
    if reply.leave {
        state
            .actions
            .push(NhgAction::new(NHG_ACT_LEAVE, window, time_ns));
    }
    for &(code, seq, pos) in &reply.events {
        let mut a = NhgAction::new(NHG_ACT_MINIMAP, window, time_ns);
        a.code = code;
        a.dx = pos as f64;
        a.dy = seq as f64;
        state.actions.push(a);
    }
    if reply.publish {
        let mut a = NhgAction::new(NHG_ACT_MINIMAP, window, time_ns);
        a.code = NHG_MINIMAP_PUBLISH;
        state.actions.push(a);
    }
}

/// A pointer event for shell window `ev.window`: true when it was the minimap's (Emacs gets none of it but a
/// `LEAVE` on the way in).
pub fn pointer(state: &mut Display, ev: &nhshell::event::Record) -> bool {
    use nhshell::event::ShellEvent;
    let Some(key) = frame_of(state, ev.window) else {
        return false;
    };
    let now = now_ns();
    let Ok(f) = state.backings.frame_mut(key) else {
        return false;
    };
    let mm = &mut f.minimaps;
    let reply = match ev.event {
        ShellEvent::PointerEnter(p) | ShellEvent::PointerMotion(p) => {
            mm.motion(Some((p.x, p.y)), now)
        }
        ShellEvent::PointerLeave(_) => mm.motion(None, now),
        // (the shell's button records carry no position: where the pointer last moved)
        ShellEvent::Button(p) => match mm.last {
            Some((x, y)) => mm.button(
                x,
                y,
                crate::pointer::button_code(p.button) == 0,
                p.state & nhshell::event::NHSHELL_BUTTON_PRESSED != 0,
                now,
            ),
            None => return false,
        },
        _ => return false,
    };
    apply(state, ev.window, ev.time_ns, &reply);
    rearm(state);
    reply.swallow
}

/// Light Emacs window `seq`'s row of `pos` in frame `frame`'s minimap (`glitnir-minimap-flash`), published at once and
/// faded on the frame clock; false when the window has no minimap revealed.
pub fn flash(state: &mut Display, frame: u64, seq: u64, pos: u64) -> bool {
    let now = now_ns();
    let Ok(f) = state.backings.frame_mut(frame) else {
        return false;
    };
    if !f.minimaps.flash(seq, pos, now) {
        return false;
    }
    let window = f.minimaps.shell_window;
    apply(
        state,
        window,
        now,
        &Reply {
            publish: true,
            ..Reply::default()
        },
    );
    rearm(state);
    true
}

/// The shell's timer: every frame's due deadlines.
pub fn timer(state: &mut Display, time_ns: u64) {
    let now = now_ns();
    let replies: Vec<(u64, Reply)> = state
        .backings
        .frames_mut()
        .filter(|(_, f)| !f.minimaps.is_empty())
        .map(|(_, f)| (f.minimaps.shell_window, f.minimaps.tick(now)))
        .collect();
    for (window, reply) in replies {
        apply(state, window, time_ns, &reply);
    }
    rearm(state);
}

/// Shell window `window` showed a frame: while a minimap of it reveals or hides, it is published again.
pub fn frame_ready(state: &mut Display, window: u64, time_ns: u64) {
    if let Some(key) = frame_of(state, window)
        && state
            .backings
            .frame_mut(key)
            .is_ok_and(|f| f.minimaps.animating())
    {
        apply(
            state,
            window,
            time_ns,
            &Reply {
                publish: true,
                ..Reply::default()
            },
        );
    }
}

/// The next sync gives every frame's minimaps again (a reconnect: the shell's surfaces start over).
pub fn forget_all(b: &mut Backings) {
    for (_, f) in b.frames_mut() {
        for w in f.minimaps.windows.values_mut() {
            w.applied = None;
            w.arrows_applied = None;
            w.given = Default::default();
        }
        f.minimaps.sent.clear();
    }
}

/// Frame `key`'s minimaps (for tests and the pointer).
pub fn of(b: &mut Backings, key: u64) -> Result<&mut Minimaps, i32> {
    Ok(&mut b.frame_mut(key)?.minimaps)
}

/// How many of frame `key`'s windows have a minimap layer (0 for a frame without a backing): while it is not 0, C
/// lists the frame's minimaps at each publish, so the publish after the last one went still removes its layer.
pub fn count(b: &mut Backings, key: u64) -> usize {
    b.frame_mut(key).map_or(0, |f| f.minimaps.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn one(n: u32) -> Option<Sprite> {
        Some((n, 1, Arc::from([n])))
    }

    fn key(i: usize) -> SpriteKey {
        (format!("l{i}").into(), 0, 1050, 100, 0)
    }

    /// The cache holds at most SPRITES_MAX; when full, the least recently used eighth goes and a sprite used since
    /// stays.
    #[test]
    fn the_sprite_cache_is_bounded_and_drops_the_least_recently_used() {
        let mut c = SpriteCache::default();
        for i in 0..SPRITES_MAX {
            c.get_or_render(key(i), || one(i as u32));
        }
        assert_eq!(c.len(), SPRITES_MAX);
        // key 0 used again: the most recent
        assert_eq!(c.get_or_render(key(0), || panic!("held")).unwrap().0, 0);
        c.get_or_render(key(SPRITES_MAX), || one(1));
        assert_eq!(c.len(), SPRITES_MAX - SPRITES_MAX / 8 + 1);
        assert!(c.contains(&key(0)), "used since: kept");
        assert!(
            !c.contains(&key(1)) && !c.contains(&key(SPRITES_MAX / 8)),
            "the oldest went"
        );
        assert!(c.contains(&key(SPRITES_MAX / 8 + 1)));
        for i in 0..10 * SPRITES_MAX {
            c.get_or_render(key(SPRITES_MAX + 1 + i), || one(2));
            assert!(c.len() <= SPRITES_MAX);
        }
    }
}

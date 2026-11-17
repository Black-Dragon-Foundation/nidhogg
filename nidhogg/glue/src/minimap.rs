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
}

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
    }
}

/// What a window's tiles were drawn for: they hold while it does not change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Stamp {
    key: u64,
    generation: u64,
    first: u64,
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
}

/// Where a window's minimap was at its last publish (root device px), for the pointer.
#[derive(Debug, Clone, Copy)]
struct Geom {
    key: u64,
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
        self.windows.values().any(|w| w.hover.leg.is_some())
    }

    /// The earliest deadline of the frame's minimaps.
    pub fn deadline(&self) -> Option<u64> {
        self.windows
            .values()
            .filter_map(|w| w.hover.deadline())
            .min()
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
                    r.events.push((NHG_MINIMAP_DWELL, *seq, p));
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

/// What [`labels`] gives: the sprites placed, the hairlines, each label's rows (top, bottom) and the accented one.
type Labels = (Vec<LabelAt>, Vec<Band>, Vec<(i32, i32)>, Option<usize>);

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
    let none = (Vec::new(), Vec::new(), Vec::new(), None);
    if e.labels == 0 {
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
    // (line, depth, mark, text)
    let mut found: Vec<(u64, u32, bool, String)> = Vec::new();
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
                    found.push((u64::from(x.0) - 1, x.1, false, x.3.to_string()));
                }
            }
        });
    }
    let placed_rows = with_model(e.key, |m| {
        let marks: Vec<(u64, String)> = m
            .sections(pa, pb)
            .iter()
            .take(4096)
            .map(|x| (x.pos, x.name.to_uppercase()))
            .collect();
        for (pos, name) in marks {
            if let Some(l) = m.line_of(pos) {
                found.push((l, 0, true, name));
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
        .map(|(r, depth, mark, _)| Candidate {
            y: (i64::from(p.rect.y) + (*r * row) as i64 - p.offset as i64)
                .clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32,
            h: h as u32,
            depth: *depth,
            mark: *mark,
        })
        .collect();
    let chosen = place_labels(&cands, p.rect.y, p.rect.y + p.rect.h, px(1.0).max(1));
    let boxes: Vec<(i32, i32)> = chosen
        .iter()
        .map(|&i| (cands[i].y, cands[i].y + h))
        .collect();
    let accent = accent_of(&boxes, hover_y);
    let mut out = Vec::new();
    let mut lines = Vec::new();
    for (k, &i) in chosen.iter().enumerate() {
        let (_, depth, mark, text) = &placed_rows[i];
        let rgb = if accent == Some(k) {
            e.accent_rgb
        } else if *mark {
            e.mark_rgb
        } else if *depth > 0 {
            e.sub_rgb
        } else {
            e.label_rgb
        };
        let Ok(rgb) = u32::try_from(rgb) else {
            continue;
        };
        let indent = if *mark {
            0
        } else {
            (*depth).min(LABEL_DEPTHS) as i32 * px(LABEL_INDENT)
        };
        let x = p.rect.x + px(LABEL_LEFT) + indent;
        let cap_w = p.rect.x + p.rect.w - px(LABEL_RIGHT) - x;
        let size = if *mark { MARK_PX } else { LABEL_PX };
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
        if *mark && let Ok(m) = u32::try_from(e.mark_rgb) {
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
    (out, lines, boxes, accent)
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
fn tile(key: u64, first: u64, t: u64, spec: &TileSpec) -> Arc<[u32]> {
    let lines = with_model(key, |m| {
        m.visible_rows(first + t * TILE_ROWS as u64, TILE_ROWS)
    })
    .flatten()
    .unwrap_or_default();
    let rows: Vec<Row> = lines.iter().map(row).collect();
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
    let stamp = Stamp {
        key: e.key,
        generation: rows.generation,
        first: rows.first,
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
            let px = Arc::clone(
                win.tiles
                    .entry(t)
                    .or_insert_with(|| tile(e.key, rows.first, t, &spec)),
            );
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
    let (labels, hairlines, boxes, accent) = if reveal_px > 0 {
        labels(e, &rows, &p, row_px, s, win.hover.y, fill, sprites)
    } else {
        (Vec::new(), Vec::new(), Vec::new(), None)
    };
    win.boxes = boxes;
    win.accent = accent;
    let mut bands = hairlines;
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

/// The line model's source of the hub's fold ranges (design §7.4: 0 the editor's parser, 1 the hub).
const SOURCE_HUB: u8 = 1;

/// The fold kinds by name, at their numbers in the line model (the hub's `folds` layer names them).
const FOLD_KINDS: [&str; 9] = [
    "block", "function", "class", "comment", "import", "test", "region", "cell", "heading",
];

/// The hub's fold ranges of FILE (Sjá's `folds` layer: a buffer with no parser of its own) into model KEY as its hub
/// source, once per layer version, unless its own parser folds it; a layer that comes while the lines are built is
/// taken at a later publish.
fn take_hub_folds(key: u64, file: Option<&str>) {
    static TAKEN: std::sync::Mutex<Vec<(u64, u64)>> = std::sync::Mutex::new(Vec::new());
    let Some(file) = file else {
        return;
    };
    let Some((version, lines)) = glitnir::sja::minimap::with_layer(file, "folds", |v, d| match d {
        glitnir::sja::minimap::Data::Folds(x) => Some((
            v,
            x.iter()
                .filter_map(|(a, b, k)| {
                    Some((*a, *b, FOLD_KINDS.iter().position(|n| *n == &**k)? as u8))
                })
                .collect::<Vec<_>>(),
        )),
        _ => None,
    })
    .flatten() else {
        return;
    };
    let mut taken = TAKEN
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if taken.iter().any(|&(k, v)| k == key && v == version) {
        return;
    }
    // a buffer its own parser folds (source 0) keeps those: the hub's would be the same ranges again
    if with_model(key, |m| {
        !m.has_folds_of(0) && m.set_fold_lines(SOURCE_HUB, &lines)
    }) == Some(true)
    {
        taken.retain(|&(k, _)| k != key);
        // a model per text shown: the oldest go first past a bound no session reaches
        if taken.len() >= 1024 {
            taken.remove(0);
        }
        taken.push((key, version));
    }
}

/// The frame's minimaps planned by [`plan`]: each window's by its `sequence_number`, the scale (×120) and the origin.
type Planned = (BTreeMap<u64, Minimap>, u32, (i32, i32));

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
    for e in list {
        take_hub_folds(e.key, e.file_name());
        let win = mm.windows.entry(e.seq).or_default();
        if let Some(m) = minimap(e, s120, origin, win, now, &mut mm.sprites) {
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
        for o in list.iter().filter(|o| o.key == e.key && o.seq != e.seq) {
            let Some(r) = rows_of(e.key, o.start, o.end, e.begv, e.zv) else {
                continue;
            };
            let row = u64::from(g.row_px.max(1));
            let y = i64::from(g.rect.y) + (r.shown.0 * row) as i64 - g.offset as i64;
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
    }
    Ok(Some((planned, s120, origin)))
}

/// [`sync_at`] at `now` (the pointer's legs are read at it).
pub fn sync_at_time(b: &mut Backings, key: u64, list: &[NhgMinimap], now: u64) -> Result<u32, i32> {
    let Some((planned, s120, _)) = plan(b, key, list, now)? else {
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
        let gone: Vec<u64> = mm
            .windows
            .keys()
            .filter(|q| !planned.contains_key(q))
            .copied()
            .collect();
        for seq in gone {
            sc.remove(minimap_id(seq));
            mm.windows.remove(&seq);
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
    let Some((planned, _, origin)) = plan(b, key, list, now)? else {
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

//! The faces AppKit lists and the fonts Core Text opens, for the glue's font driver (`glue/src/font.rs`).
//!
//! The faces come from `NSFontManager` once, on the main thread, and are kept: a face's id is its index plus one.  An
//! open font is an `NSFont` (toll-free bridged to `CTFont`) at the pixel size Emacs asks for, so its metrics are already
//! device pixels; glyphs and their bounds come from Core Text.

use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_app_kit::{NSFont, NSFontManager};
use objc2_foundation::{NSArray, NSNumber, NSString};
use std::collections::HashMap;
use std::ffi::c_void;
use std::sync::{Mutex, OnceLock};

/// One face: its PostScript name, family and traits as AppKit gives them.
#[derive(Clone, Debug)]
pub struct MacFace {
    pub id: u64,
    pub postscript: String,
    pub family: String,
    /// `NSFontManager`'s weight, 0–15 (5 normal, 9 bold).
    pub weight: i64,
    /// `NSFontTraitMask` bits.
    pub traits: u64,
}

/// `NSFontTraitMask` bits.
pub const NS_ITALIC: u64 = 0x1;
pub const NS_BOLD: u64 = 0x2;
pub const NS_EXPANDED: u64 = 0x20;
pub const NS_CONDENSED: u64 = 0x40;
pub const NS_FIXED_PITCH: u64 = 0x400;

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct CGPoint {
    x: f64,
    y: f64,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct CGSize {
    width: f64,
    height: f64,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct CGRect {
    origin: CGPoint,
    size: CGSize,
}

#[link(name = "CoreText", kind = "framework")]
unsafe extern "C" {
    fn CTFontGetGlyphsForCharacters(
        font: *const c_void,
        chars: *const u16,
        glyphs: *mut u16,
        count: isize,
    ) -> bool;
    fn CTFontGetAdvancesForGlyphs(
        font: *const c_void,
        orientation: u32,
        glyphs: *const u16,
        advances: *mut CGSize,
        count: isize,
    ) -> f64;
    fn CTFontGetBoundingRectsForGlyphs(
        font: *const c_void,
        orientation: u32,
        glyphs: *const u16,
        rects: *mut CGRect,
        count: isize,
    ) -> CGRect;
}

/// `kCTFontOrientationDefault`.
const CT_DEFAULT: u32 = 0;

fn faces_cell() -> &'static Mutex<Option<Vec<MacFace>>> {
    static FACES: OnceLock<Mutex<Option<Vec<MacFace>>>> = OnceLock::new();
    FACES.get_or_init(Mutex::default)
}

fn number(o: &AnyObject) -> Option<i64> {
    o.downcast_ref::<NSNumber>()
        .map(|n| n.integerValue() as i64)
}

/// Every face, in AppKit's family order; listed on the main thread's first call and kept (another thread before that
/// gets none).
pub fn faces() -> Vec<MacFace> {
    let mut cell = faces_cell().lock().unwrap_or_else(|p| p.into_inner());
    if let Some(f) = cell.as_ref() {
        return f.clone();
    }
    let Some(mtm) = MainThreadMarker::new() else {
        return Vec::new();
    };
    let fm = NSFontManager::sharedFontManager(mtm);
    let mut out = Vec::new();
    for family in fm.availableFontFamilies().iter() {
        let Some(members) = fm.availableMembersOfFontFamily(&family) else {
            continue;
        };
        for m in members.iter() {
            // [PostScript name, style name, weight, traits]
            let Some(m) = m.downcast_ref::<NSArray>() else {
                continue;
            };
            if m.count() < 4 {
                continue;
            }
            let Some(ps) = m
                .objectAtIndex(0)
                .downcast_ref::<NSString>()
                .map(|s| s.to_string())
            else {
                continue;
            };
            out.push(MacFace {
                id: out.len() as u64 + 1,
                postscript: ps,
                family: family.to_string(),
                weight: number(&m.objectAtIndex(2)).unwrap_or(5),
                traits: number(&m.objectAtIndex(3)).unwrap_or(0) as u64,
            });
        }
    }
    *cell = Some(out.clone());
    out
}

/// An open font.
struct OpenFont {
    font: Retained<NSFont>,
}

// SAFETY: Core Text fonts are immutable and documented thread-safe; the `NSFont` is only used as a `CTFont` here.
unsafe impl Send for OpenFont {}

fn open_fonts() -> &'static Mutex<(u64, HashMap<u64, OpenFont>)> {
    static OPEN: OnceLock<Mutex<(u64, HashMap<u64, OpenFont>)>> = OnceLock::new();
    OPEN.get_or_init(Mutex::default)
}

/// A font's raw metrics, in device pixels (unrounded).
#[derive(Clone, Copy, Debug, Default)]
pub struct RawMetrics {
    pub ascent: f64,
    pub descent: f64,
    pub leading: f64,
    pub underline_position: f64,
    pub underline_thickness: f64,
    pub mono: bool,
    /// Courier, Helvetica or Times: AppKit's taller ascent (`macfont_open`).
    pub tall_family: bool,
}

/// Open face `id` at `size` pixels: a handle and its metrics.
pub fn open(id: u64, size: f64) -> Option<(u64, RawMetrics)> {
    let face = faces().into_iter().find(|f| f.id == id)?;
    let font = NSFont::fontWithName_size(&NSString::from_str(&face.postscript), size)?;
    let family = font.familyName().map(|s| s.to_string()).unwrap_or_default();
    let m = RawMetrics {
        ascent: font.ascender(),
        descent: -font.descender(),
        leading: font.leading(),
        underline_position: font.underlinePosition(),
        underline_thickness: font.underlineThickness(),
        mono: face.traits & NS_FIXED_PITCH != 0 || font.isFixedPitch(),
        tall_family: matches!(family.as_str(), "Courier" | "Helvetica" | "Times"),
    };
    let mut open = open_fonts().lock().unwrap_or_else(|p| p.into_inner());
    open.0 += 1;
    let handle = open.0;
    open.1.insert(handle, OpenFont { font });
    Some((handle, m))
}

pub fn close(handle: u64) {
    open_fonts()
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .1
        .remove(&handle);
}

fn with_font<R>(handle: u64, f: impl FnOnce(*const c_void) -> R) -> Option<R> {
    let open = open_fonts().lock().unwrap_or_else(|p| p.into_inner());
    open.1
        .get(&handle)
        .map(|o| f(Retained::as_ptr(&o.font).cast()))
}

/// The glyph of character `c` (`None`: the font has none; only the BMP for now).
pub fn glyph(handle: u64, c: u32) -> Option<u32> {
    let unit = u16::try_from(c).ok()?;
    with_font(handle, |ct| {
        let mut g = 0u16;
        // SAFETY: one character and one glyph slot, on a live font.
        let ok = unsafe { CTFontGetGlyphsForCharacters(ct, &unit, &mut g, 1) };
        (ok && g != 0).then_some(u32::from(g))
    })
    .flatten()
}

/// Glyph `g`'s advance and bounds: (advance, left, right, top, bottom), in pixels, y up.
pub fn glyph_bounds(handle: u64, g: u32) -> Option<(f64, f64, f64, f64, f64)> {
    let g = u16::try_from(g).ok()?;
    with_font(handle, |ct| {
        let mut adv = CGSize::default();
        let mut r = CGRect::default();
        // SAFETY: one glyph and one output slot each, on a live font.
        unsafe {
            CTFontGetAdvancesForGlyphs(ct, CT_DEFAULT, &g, &mut adv, 1);
            CTFontGetBoundingRectsForGlyphs(ct, CT_DEFAULT, &g, &mut r, 1);
        }
        (
            adv.width,
            r.origin.x,
            r.origin.x + r.size.width,
            r.origin.y + r.size.height,
            r.origin.y,
        )
    })
}

//! The fonts Core Text opens, for the glue's font driver (`glue/src/font.rs`); the faces are `text.rs`'s.
//!
//! An open font is a `CTFont` made from a face's PostScript name at the pixel size Emacs asks for
//! (`macfont_open`), so its metrics are already device pixels; glyphs and their bounds come from Core Text a page at a
//! time, for the glue's glyph tables.

use super::text::{Cf, string};
use objc2::rc::Retained;
use objc2_foundation::NSString;
use std::collections::HashMap;
use std::ffi::c_void;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

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
    fn CTFontGetSize(font: *const c_void) -> f64;
    fn CTFontCreateWithName(name: *const c_void, size: f64, matrix: *const c_void)
    -> *const c_void;
    fn CTFontGetAscent(font: *const c_void) -> f64;
    fn CTFontGetDescent(font: *const c_void) -> f64;
    fn CTFontGetLeading(font: *const c_void) -> f64;
    fn CTFontGetUnderlinePosition(font: *const c_void) -> f64;
    fn CTFontGetUnderlineThickness(font: *const c_void) -> f64;
    fn CTFontGetSymbolicTraits(font: *const c_void) -> u32;
    fn CTFontCopyFamilyName(font: *const c_void) -> *const c_void;
    fn CTFontCopyTable(font: *const c_void, tag: u32, options: u32) -> *const c_void;
    fn CTFontCopyAttribute(font: *const c_void, attribute: *const c_void) -> *const c_void;
    static kCTFontURLAttribute: *const c_void;
    #[cfg(feature = "test-hooks")]
    fn CTFontManagerRegisterFontsForURL(
        url: *const c_void,
        scope: u32,
        error: *mut *const c_void,
    ) -> bool;
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    static kCFPreferencesCurrentApplication: *const c_void;
    fn CFPreferencesGetAppIntegerValue(
        key: *const c_void,
        app: *const c_void,
        valid: *mut u8,
    ) -> isize;
    fn CFDataGetLength(d: *const c_void) -> isize;
    fn CFURLGetFileSystemRepresentation(
        url: *const c_void,
        resolve: u8,
        buf: *mut u8,
        cap: isize,
    ) -> u8;
    fn CFRelease(cf: *const c_void);
    fn CFDataGetBytePtr(d: *const c_void) -> *const u8;
}

/// `AppleAntiAliasingThreshold` (`macfont_update_antialias_threshold`): text this size or smaller is drawn without
/// antialiasing; 0 when unset.  Read once.
fn antialias_threshold() -> f64 {
    static T: OnceLock<f64> = OnceLock::new();
    *T.get_or_init(|| {
        let key = NSString::from_str("AppleAntiAliasingThreshold");
        let mut valid = 0u8;
        // SAFETY: a live CFString (toll-free bridged) and the current-application constant.
        let v = unsafe {
            CFPreferencesGetAppIntegerValue(
                Retained::as_ptr(&key).cast(),
                kCFPreferencesCurrentApplication,
                &mut valid,
            )
        };
        if valid != 0 { v as f64 } else { 0.0 }
    })
}

/// Whether the font `handle`'s text is drawn without antialiasing (`macfont_draw`'s `no_antialias_p`):
/// `ns-antialias-text` is nil, or the size is at most the threshold.  `None`: not open.
pub fn integral(handle: u64) -> Option<bool> {
    let off = !super::draw::ANTIALIAS.load(std::sync::atomic::Ordering::Relaxed);
    // SAFETY: a live font.
    let size = with_font(handle, |ct| unsafe { CTFontGetSize(ct) })?;
    Some(off || size <= antialias_threshold())
}

/// `kCTFontOrientationDefault`.
const CT_DEFAULT: u32 = 0;

/// An open font, and what macfont draws synthesised for it.
struct OpenFont {
    font: Cf,
    synthetic_italic: bool,
    synthetic_bold: bool,
    /// The family and size, for the test hooks.
    family: String,
    size: f64,
    /// The font's `cmap` table, read the first time a variation sequence is asked of it.
    cmap: OnceLock<Option<Vec<u8>>>,
}

fn open_fonts() -> &'static Mutex<(u64, HashMap<u64, OpenFont>)> {
    static OPEN: OnceLock<Mutex<(u64, HashMap<u64, OpenFont>)>> = OnceLock::new();
    OPEN.get_or_init(Mutex::default)
}

/// A font's raw metrics, in device pixels (unrounded), as `macfont_open` reads them.
#[derive(Clone, Debug, Default)]
pub struct RawMetrics {
    pub ascent: f64,
    pub descent: f64,
    pub leading: f64,
    pub underline_position: f64,
    pub underline_thickness: f64,
    /// `CTFontGetSize`.
    pub size: f64,
    /// `CTFontGetSymbolicTraits`.
    pub symbolic: u32,
    pub family: String,
}

/// Open the face named `postscript` at `size` pixels (`CTFontCreateWithName`), drawn with the synthesised traits
/// given: a handle and its metrics.
pub fn open(
    postscript: &str,
    size: f64,
    synthetic_italic: bool,
    synthetic_bold: bool,
) -> Option<(u64, RawMetrics)> {
    let name = NSString::from_str(postscript);
    // SAFETY: a live CFString (toll-free bridged); the font is +1, owned by `Cf`.
    let font = Cf::new(unsafe {
        CTFontCreateWithName(Retained::as_ptr(&name).cast(), size, std::ptr::null())
    })?;
    let f = font.get();
    // SAFETY: a live font; the family name's copy is +1.
    let m = unsafe {
        RawMetrics {
            ascent: CTFontGetAscent(f),
            descent: CTFontGetDescent(f),
            leading: CTFontGetLeading(f),
            underline_position: CTFontGetUnderlinePosition(f),
            underline_thickness: CTFontGetUnderlineThickness(f),
            size: CTFontGetSize(f),
            symbolic: CTFontGetSymbolicTraits(f),
            family: Cf::new(CTFontCopyFamilyName(f)).map_or_else(String::new, |s| string(s.get())),
        }
    };
    let mut open = open_fonts().lock().unwrap_or_else(|p| p.into_inner());
    open.0 += 1;
    let handle = open.0;
    open.1.insert(
        handle,
        OpenFont {
            font,
            synthetic_italic,
            synthetic_bold,
            family: m.family.clone(),
            size: m.size,
            cmap: OnceLock::new(),
        },
    );
    Some((handle, m))
}

/// The open font `handle`'s raw metrics, read from Core Text again (`None`: not open): the test hooks' oracle.
pub fn raw(handle: u64) -> Option<RawMetrics> {
    with_font(handle, |f| {
        // SAFETY: a live font; the family name's copy is +1.
        unsafe {
            RawMetrics {
                ascent: CTFontGetAscent(f),
                descent: CTFontGetDescent(f),
                leading: CTFontGetLeading(f),
                underline_position: CTFontGetUnderlinePosition(f),
                underline_thickness: CTFontGetUnderlineThickness(f),
                size: CTFontGetSize(f),
                symbolic: CTFontGetSymbolicTraits(f),
                family: Cf::new(CTFontCopyFamilyName(f))
                    .map_or_else(String::new, |s| string(s.get())),
            }
        }
    })
}

/// The open font `handle`'s synthesised italic and bold, and its size (`None`: not open).
pub fn synthetic(handle: u64) -> Option<(bool, bool, f64)> {
    let open = open_fonts().lock().unwrap_or_else(|p| p.into_inner());
    open.1
        .get(&handle)
        .map(|o| (o.synthetic_italic, o.synthetic_bold, o.size))
}

/// The newest open font of `family` at `size` pixels (the test hooks' way to name the font a frame uses).
pub fn find(family: &str, size: f64) -> Option<u64> {
    let open = open_fonts().lock().unwrap_or_else(|p| p.into_inner());
    open.1
        .iter()
        .filter(|(_, o)| o.family.eq_ignore_ascii_case(family) && o.size == size)
        .map(|(h, _)| *h)
        .max()
}

pub fn close(handle: u64) {
    open_fonts()
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .1
        .remove(&handle);
}

/// `f` on the open font `handle`'s `CTFont` (`None`: not open); the font stays open for the call.
pub fn with_font<R>(handle: u64, f: impl FnOnce(*const c_void) -> R) -> Option<R> {
    let open = open_fonts().lock().unwrap_or_else(|p| p.into_inner());
    open.1.get(&handle).map(|o| f(o.font.get()))
}

/// The file the open font `handle` is read from (its `kCTFontURLAttribute`, as a path), for `font-info`'s FILENAME:
/// drgnx's hub reads its advances from it rather than asking the editor per character.  `None` when the font is not
/// open, Core Text names no file, or the file is a collection (the face's index in it is not known).
pub fn file(handle: u64) -> Option<String> {
    with_font(handle, |ct| {
        // SAFETY: a live font and the framework's constant; the URL (+1) is released once read.
        unsafe {
            let url = CTFontCopyAttribute(ct, kCTFontURLAttribute);
            if url.is_null() {
                return None;
            }
            let mut buf = [0u8; 4096];
            let ok = CFURLGetFileSystemRepresentation(url, 1, buf.as_mut_ptr(), buf.len() as isize);
            CFRelease(url);
            let n = buf.iter().position(|&b| b == 0)?;
            (ok != 0 && n > 0).then(|| String::from_utf8_lossy(&buf[..n]).into_owned())
        }
    })
    .flatten()
    .filter(|path| !collection(path))
}

/// Whether the font file at `path` is a collection (`ttcf`): its face's index is not known here, and a reader taking
/// the first face would read another face's advances, so no file is named for it.
fn collection(path: &str) -> bool {
    use std::io::Read;
    let mut tag = [0u8; 4];
    std::fs::File::open(path)
        .and_then(|mut f| f.read_exact(&mut tag))
        .map_or(true, |()| &tag == b"ttcf")
}

/// `f` on the open font `handle`'s `cmap` table (`CTFontCopyTable`, read once; empty when it has none); `None`: not
/// open.
pub fn with_cmap<R>(handle: u64, f: impl FnOnce(&[u8]) -> R) -> Option<R> {
    let open = open_fonts().lock().unwrap_or_else(|p| p.into_inner());
    let o = open.1.get(&handle)?;
    let cmap = o.cmap.get_or_init(|| {
        // SAFETY: a live font; the table's copy is +1, owned by `Cf`.
        let t = Cf::new(unsafe { CTFontCopyTable(o.font.get(), u32::from_be_bytes(*b"cmap"), 0) })?;
        // SAFETY: a live CFData.
        let (p, n) = unsafe { (CFDataGetBytePtr(t.get()), CFDataGetLength(t.get())) };
        // SAFETY: its `n` bytes, copied before `t` is released.
        (!p.is_null() && n > 0)
            .then(|| unsafe { std::slice::from_raw_parts(p, n as usize) }.to_vec())
    });
    Some(f(cmap.as_deref().unwrap_or(&[])))
}

/// Test hook: register the font file at `path` for this process (`CTFontManagerRegisterFontsForURL`, process scope),
/// as an app installing a font while Emacs runs.  Core Text then posts its registered-fonts notification.
#[cfg(feature = "test-hooks")]
pub fn register(path: &str) -> bool {
    let url = objc2_foundation::NSURL::fileURLWithPath(&NSString::from_str(path));
    // SAFETY: a live file URL (toll-free bridged to CFURL); kCTFontManagerScopeProcess is 1; no error asked for.
    unsafe {
        CTFontManagerRegisterFontsForURL(Retained::as_ptr(&url).cast(), 1, std::ptr::null_mut())
    }
}

/// The glyph of each UTF-16 unit of `units` into `out` (0: none; a surrogate pair's glyph at its first unit), one
/// Core Text call.  False: the font is not open.
pub fn glyphs_for(handle: u64, units: &[u16], out: &mut [u16]) -> bool {
    let n = units.len().min(out.len());
    with_font(handle, |ct| {
        // SAFETY: `n` characters and `n` glyph slots, on a live font.
        unsafe { CTFontGetGlyphsForCharacters(ct, units.as_ptr(), out.as_mut_ptr(), n as isize) };
    })
    .is_some()
}

/// Each glyph's advance and bounds into `out`: (advance, left, right, top, bottom), in pixels, y up; one Core Text call
/// for each.  False: the font is not open.
pub fn bounds_for(handle: u64, glyphs: &[u16], out: &mut [[f64; 5]]) -> bool {
    let n = glyphs.len().min(out.len());
    let mut adv = vec![CGSize::default(); n];
    let mut r = vec![CGRect::default(); n];
    with_font(handle, |ct| {
        // SAFETY: `n` glyphs and `n` output slots each, on a live font.
        unsafe {
            CTFontGetAdvancesForGlyphs(
                ct,
                CT_DEFAULT,
                glyphs.as_ptr(),
                adv.as_mut_ptr(),
                n as isize,
            );
            CTFontGetBoundingRectsForGlyphs(
                ct,
                CT_DEFAULT,
                glyphs.as_ptr(),
                r.as_mut_ptr(),
                n as isize,
            );
        }
    })
    .map(|()| {
        for ((o, a), r) in out.iter_mut().zip(&adv).zip(&r) {
            *o = [
                a.width,
                r.origin.x,
                r.origin.x + r.size.width,
                r.origin.y + r.size.height,
                r.origin.y,
            ];
        }
    })
    .is_some()
}

/// The font driver's counters, in `(nhmac-stats)`: the glue's `nhg_font_*` calls, and the glyph-table pages C asked
/// to have filled.
pub struct FontStats {
    pub calls: AtomicU64,
    pub fills: AtomicU64,
}

pub static STATS: FontStats = FontStats {
    calls: AtomicU64::new(0),
    fills: AtomicU64::new(0),
};

impl FontStats {
    /// The counters as `(nhmac-stats)` lists them.
    pub fn text(&self) -> String {
        format!(
            ":font-calls {} :font-fills {}",
            self.calls.load(Ordering::Relaxed),
            self.fills.load(Ordering::Relaxed)
        )
    }
}

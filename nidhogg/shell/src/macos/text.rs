//! Core Text's faces, the family it draws a script's characters in, and the layout of a line in one font, for the
//! glue's font driver (`glue/src/font.rs`).
//!
//! - **Faces** (`macfont_list`): the families of `CTFontManagerCopyAvailableFontFamilyNames` but the hidden ones (a
//!   name starting with ".") and LastResort, each family's faces from its matching descriptors, with their symbolic
//!   and numeric traits.  Listed once, off the main thread too; a face's character set and languages are read the
//!   first time a script or language is asked of it.
//! - **The preferred family** (`mac_font_create_preferred_family_for_attributes`): the family of the one font a line of
//!   the characters is laid out in, with no font asked for.
//! - **A line** (`mac_font_shape`): the text in one font with kerning off, each glyph run's glyphs, string indices,
//!   positions and typographic widths, and each unit's composed-character range; none when Core Text falls back to
//!   another font for some of it.

use objc2_foundation::NSString;
use std::ffi::c_void;
use std::sync::OnceLock;

type CFRef = *const c_void;

/// `kCTFontTraitItalic`, `kCTFontTraitBold`, `kCTFontTraitMonoSpace` and `kCTFontTraitColorGlyphs`.
pub const TRAIT_ITALIC: u32 = 1 << 0;
pub const TRAIT_BOLD: u32 = 1 << 1;
pub const TRAIT_MONO: u32 = 1 << 10;
pub const TRAIT_COLOR: u32 = 1 << 13;

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct CFRange {
    location: isize,
    length: isize,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct CGPoint {
    x: f64,
    y: f64,
}

/// `CFDictionaryKeyCallBacks` and `CFDictionaryValueCallBacks`, whose addresses are all that is used.
#[repr(C)]
struct KeyCallBacks([usize; 6]);
#[repr(C)]
struct ValueCallBacks([usize; 5]);

const NUMBER_SINT64: isize = 4;
const NUMBER_FLOAT32: isize = 5;
const NUMBER_FLOAT64: isize = 13;
/// `kCTRunStatusRightToLeft`.
const RUN_RTL: u32 = 1;

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    static kCFTypeDictionaryKeyCallBacks: KeyCallBacks;
    static kCFTypeDictionaryValueCallBacks: ValueCallBacks;
    fn CFRetain(r: CFRef) -> CFRef;
    fn CFRelease(r: CFRef);
    fn CFEqual(a: CFRef, b: CFRef) -> u8;
    fn CFArrayGetCount(a: CFRef) -> isize;
    fn CFArrayGetValueAtIndex(a: CFRef, i: isize) -> CFRef;
    fn CFDictionaryCreate(
        alloc: CFRef,
        keys: *const CFRef,
        values: *const CFRef,
        n: isize,
        kcb: *const c_void,
        vcb: *const c_void,
    ) -> CFRef;
    fn CFDictionaryGetValue(d: CFRef, k: CFRef) -> CFRef;
    fn CFNumberCreate(alloc: CFRef, ty: isize, v: *const c_void) -> CFRef;
    fn CFNumberGetValue(n: CFRef, ty: isize, out: *mut c_void) -> bool;
    fn CFCharacterSetIsLongCharacterMember(cs: CFRef, c: u32) -> bool;
    fn CFStringCreateWithCharacters(alloc: CFRef, chars: *const u16, n: isize) -> CFRef;
    fn CFStringGetRangeOfComposedCharactersAtIndex(s: CFRef, i: isize) -> CFRange;
    fn CFAttributedStringCreate(alloc: CFRef, s: CFRef, attrs: CFRef) -> CFRef;
}

#[link(name = "CoreText", kind = "framework")]
unsafe extern "C" {
    static kCTFontFamilyNameAttribute: CFRef;
    static kCTFontNameAttribute: CFRef;
    static kCTFontTraitsAttribute: CFRef;
    static kCTFontSymbolicTrait: CFRef;
    static kCTFontWeightTrait: CFRef;
    static kCTFontWidthTrait: CFRef;
    static kCTFontSlantTrait: CFRef;
    static kCTFontCharacterSetAttribute: CFRef;
    static kCTFontLanguagesAttribute: CFRef;
    static kCTFontAttributeName: CFRef;
    static kCTLanguageAttributeName: CFRef;
    static kCTKernAttributeName: CFRef;
    fn CTFontManagerCopyAvailableFontFamilyNames() -> CFRef;
    fn CTFontManagerCompareFontFamilyNames(a: CFRef, b: CFRef, ctx: *mut c_void) -> isize;
    fn CTFontDescriptorCreateWithAttributes(attrs: CFRef) -> CFRef;
    fn CTFontDescriptorCreateMatchingFontDescriptors(d: CFRef, mandatory: CFRef) -> CFRef;
    fn CTFontDescriptorCopyAttribute(d: CFRef, attr: CFRef) -> CFRef;
    fn CTFontCopyPostScriptName(f: CFRef) -> CFRef;
    fn CTFontCopyFamilyName(f: CFRef) -> CFRef;
    fn CTLineCreateWithAttributedString(s: CFRef) -> CFRef;
    fn CTLineGetGlyphRuns(l: CFRef) -> CFRef;
    fn CTRunGetGlyphCount(r: CFRef) -> isize;
    fn CTRunGetStatus(r: CFRef) -> u32;
    fn CTRunGetStringRange(r: CFRef) -> CFRange;
    fn CTRunGetStringIndices(r: CFRef, range: CFRange, out: *mut isize);
    fn CTRunGetGlyphs(r: CFRef, range: CFRange, out: *mut u16);
    fn CTRunGetPositions(r: CFRef, range: CFRange, out: *mut CGPoint);
    fn CTRunGetTypographicBounds(
        r: CFRef,
        range: CFRange,
        a: *mut f64,
        d: *mut f64,
        l: *mut f64,
    ) -> f64;
    fn CTRunGetAttributes(r: CFRef) -> CFRef;
}

/// An owned Core Foundation object (released on drop); a null one is `None` at `Cf::new`.
pub(crate) struct Cf(CFRef);

// SAFETY: the objects held here (strings, arrays, descriptors, character sets, fonts, lines) are immutable, and Core
// Foundation's immutable objects are thread-safe.
unsafe impl Send for Cf {}
unsafe impl Sync for Cf {}

impl Cf {
    /// Own `r`, a +1 reference from a Create or Copy call.
    pub(crate) fn new(r: CFRef) -> Option<Cf> {
        (!r.is_null()).then_some(Cf(r))
    }

    pub(crate) fn get(&self) -> CFRef {
        self.0
    }
}

impl Drop for Cf {
    fn drop(&mut self) {
        // SAFETY: a +1 reference owned here, released once.
        unsafe { CFRelease(self.0) };
    }
}

/// The text of the CFString `s` (borrowed).
pub(crate) fn string(s: CFRef) -> String {
    // SAFETY: a live CFString, toll-free bridged to NSString.
    unsafe { &*s.cast::<NSString>() }.to_string()
}

fn cf_string(s: &str) -> Option<Cf> {
    let units: Vec<u16> = s.encode_utf16().collect();
    cf_units(&units)
}

fn cf_units(units: &[u16]) -> Option<Cf> {
    // SAFETY: `units.len()` UTF-16 units, copied.
    Cf::new(unsafe {
        CFStringCreateWithCharacters(std::ptr::null(), units.as_ptr(), units.len() as isize)
    })
}

fn dictionary(keys: &[CFRef], values: &[CFRef]) -> Option<Cf> {
    // SAFETY: as many live keys as values; the CF type callbacks retain both.
    Cf::new(unsafe {
        CFDictionaryCreate(
            std::ptr::null(),
            keys.as_ptr(),
            values.as_ptr(),
            keys.len() as isize,
            (&raw const kCFTypeDictionaryKeyCallBacks).cast(),
            (&raw const kCFTypeDictionaryValueCallBacks).cast(),
        )
    })
}

fn number_f64(n: CFRef) -> Option<f64> {
    let mut v = 0.0f64;
    // SAFETY: `n` is null or a live CFNumber; `v` is the double asked for.
    (!n.is_null() && unsafe { CFNumberGetValue(n, NUMBER_FLOAT64, (&raw mut v).cast()) })
        .then_some(v)
}

/// One face.
pub struct Face {
    pub postscript: String,
    pub family: String,
    /// `kCTFontSymbolicTrait`.
    pub symbolic: u32,
    /// `kCTFontWeightTrait`, `kCTFontWidthTrait` and `kCTFontSlantTrait` (-1–1).
    pub weight: f64,
    pub width: f64,
    pub slant: f64,
    desc: Cf,
    charset: OnceLock<Option<Cf>>,
    languages: OnceLock<Vec<String>>,
}

impl Face {
    /// Whether the face has a glyph for `c` (its `kCTFontCharacterSetAttribute`; none: no character).
    pub fn has(&self, c: u32) -> bool {
        let cs = self.charset.get_or_init(|| {
            // SAFETY: a live descriptor and attribute name; the copy is +1.
            Cf::new(unsafe {
                CTFontDescriptorCopyAttribute(self.desc.get(), kCTFontCharacterSetAttribute)
            })
        });
        // SAFETY: a live character set.
        cs.as_ref()
            .is_some_and(|cs| unsafe { CFCharacterSetIsLongCharacterMember(cs.get(), c) })
    }

    /// The languages the face covers (`kCTFontLanguagesAttribute`).
    pub fn languages(&self) -> &[String] {
        self.languages.get_or_init(|| {
            // SAFETY: as in `has`.
            let Some(a) = Cf::new(unsafe {
                CTFontDescriptorCopyAttribute(self.desc.get(), kCTFontLanguagesAttribute)
            }) else {
                return Vec::new();
            };
            // SAFETY: a live CFArray of CFStrings.
            (0..unsafe { CFArrayGetCount(a.get()) })
                .map(|i| string(unsafe { CFArrayGetValueAtIndex(a.get(), i) }))
                .collect()
        })
    }
}

/// The face of descriptor `d` (borrowed), or none without a name or traits.
fn face(d: CFRef) -> Option<Face> {
    // SAFETY: a live descriptor; each copy is +1 and owned by a `Cf`.
    let (ps, family, traits) = unsafe {
        (
            Cf::new(CTFontDescriptorCopyAttribute(d, kCTFontNameAttribute))?,
            Cf::new(CTFontDescriptorCopyAttribute(d, kCTFontFamilyNameAttribute))?,
            Cf::new(CTFontDescriptorCopyAttribute(d, kCTFontTraitsAttribute))?,
        )
    };
    // SAFETY: a live dictionary; the values are borrowed.
    let get = |k: CFRef| unsafe { CFDictionaryGetValue(traits.get(), k) };
    let mut sym = 0i64;
    // SAFETY: a CFNumber or null; `sym` is the 64-bit integer asked for.
    let n = unsafe { get(kCTFontSymbolicTrait) };
    if !n.is_null() {
        unsafe { CFNumberGetValue(n, NUMBER_SINT64, (&raw mut sym).cast()) };
    }
    Some(Face {
        postscript: string(ps.get()),
        family: string(family.get()),
        symbolic: sym as u32,
        // SAFETY: the trait keys are Core Text's constants.
        weight: number_f64(unsafe { get(kCTFontWeightTrait) }).unwrap_or(0.0),
        width: number_f64(unsafe { get(kCTFontWidthTrait) }).unwrap_or(0.0),
        slant: number_f64(unsafe { get(kCTFontSlantTrait) }).unwrap_or(0.0),
        // SAFETY: a borrowed descriptor, retained by taking a fresh +1 reference.
        desc: Cf::new(unsafe { CFRetain(d) })?,
        charset: OnceLock::new(),
        languages: OnceLock::new(),
    })
}

/// Every face, in Core Text's family order (`mac_font_create_available_families`); listed on the first call and kept.
pub fn faces() -> &'static [Face] {
    static FACES: OnceLock<Vec<Face>> = OnceLock::new();
    FACES.get_or_init(|| {
        let mut out = Vec::new();
        // SAFETY: a +1 array of CFStrings.
        let Some(families) = Cf::new(unsafe { CTFontManagerCopyAvailableFontFamilyNames() }) else {
            return out;
        };
        let last_resort = cf_string("LastResort");
        // SAFETY: a live array.
        for i in 0..unsafe { CFArrayGetCount(families.get()) } {
            // SAFETY: an index in range; the string is borrowed.
            let family = unsafe { CFArrayGetValueAtIndex(families.get(), i) };
            let name = string(family);
            let hidden = name.starts_with('.')
                || last_resort
                    .as_ref()
                    // SAFETY: two live strings (kCFCompareEqualTo is 0).
                    .is_some_and(|l| unsafe { CTFontManagerCompareFontFamilyNames(family, l.get(), std::ptr::null_mut()) } == 0);
            if hidden {
                continue;
            }
            // SAFETY: the attribute key is Core Text's; the family name is live.
            let Some(attrs) = dictionary(&[unsafe { kCTFontFamilyNameAttribute }], &[family]) else {
                continue;
            };
            // SAFETY: live attributes; each result is +1.
            let Some(pat) = Cf::new(unsafe { CTFontDescriptorCreateWithAttributes(attrs.get()) }) else {
                continue;
            };
            let Some(descs) =
                Cf::new(unsafe { CTFontDescriptorCreateMatchingFontDescriptors(pat.get(), std::ptr::null()) })
            else {
                continue;
            };
            // SAFETY: a live array of descriptors, borrowed.
            for j in 0..unsafe { CFArrayGetCount(descs.get()) } {
                if let Some(f) = face(unsafe { CFArrayGetValueAtIndex(descs.get(), j) }) {
                    out.push(f);
                }
            }
        }
        out
    })
}

/// The PostScript name of the font `f` (borrowed).
fn postscript(f: CFRef) -> Option<Cf> {
    // SAFETY: a live font; the copy is +1.
    Cf::new(unsafe { CTFontCopyPostScriptName(f) })
}

/// The one font of every glyph run of `line`, or none when the runs use more than one (by PostScript name).
fn single_font(line: CFRef) -> Option<CFRef> {
    // SAFETY: a live line; its runs and their attributes are borrowed from it.
    unsafe {
        let runs = CTLineGetGlyphRuns(line);
        let mut font: Option<(CFRef, Cf)> = None;
        for i in 0..CFArrayGetCount(runs) {
            let attrs = CTRunGetAttributes(CFArrayGetValueAtIndex(runs, i));
            if attrs.is_null() {
                return None;
            }
            let f = CFDictionaryGetValue(attrs, kCTFontAttributeName);
            if f.is_null() {
                return None;
            }
            match &font {
                None => font = Some((f, postscript(f)?)),
                Some((g, name)) => {
                    if *g != f && !postscript(f).is_some_and(|n| CFEqual(n.get(), name.get()) != 0)
                    {
                        return None;
                    }
                }
            }
        }
        font.map(|(f, _)| f)
    }
}

/// A line of `units` with `attrs`.
fn line(units: &[u16], attrs: &Cf) -> Option<Cf> {
    let s = cf_units(units)?;
    // SAFETY: a live string and dictionary; each result is +1.
    let a = Cf::new(unsafe { CFAttributedStringCreate(std::ptr::null(), s.get(), attrs.get()) })?;
    Cf::new(unsafe { CTLineCreateWithAttributedString(a.get()) })
}

/// The family Core Text lays `chars` out in when no font is asked for, if it is one font.
pub fn preferred_family(chars: &[u32]) -> Option<String> {
    preferred_family_in(chars, "")
}

/// As `preferred_family`, with the text in `language` (empty: none), as macfont asks for a registry's characters.
pub fn preferred_family_in(chars: &[u32], language: &str) -> Option<String> {
    let mut units = Vec::new();
    for &c in chars {
        let mut b = [0u16; 2];
        units.extend_from_slice(char::from_u32(c)?.encode_utf16(&mut b));
    }
    if units.is_empty() {
        return None;
    }
    let lang = if language.is_empty() {
        None
    } else {
        Some(cf_string(language)?)
    };
    // SAFETY: a CoreText constant, live for the process.
    let attrs = match &lang {
        Some(l) => dictionary(&[unsafe { kCTLanguageAttributeName }], &[l.get()])?,
        None => dictionary(&[], &[])?,
    };
    let l = line(&units, &attrs)?;
    let f = single_font(l.get())?;
    // SAFETY: a live font; the copy is +1.
    let family = Cf::new(unsafe { CTFontCopyFamilyName(f) })?;
    Some(string(family.get()))
}

/// One glyph run of a line, in Core Text's storage order.
pub struct Run {
    pub rtl: bool,
    /// The run's string range (UTF-16): location, length.
    pub range: (usize, usize),
    pub indices: Vec<usize>,
    pub glyphs: Vec<u16>,
    /// Each glyph's position from the line's origin, y up.
    pub positions: Vec<(f64, f64)>,
    /// Each glyph's typographic width.
    pub widths: Vec<f64>,
}

/// A shaped line.
pub struct Line {
    pub runs: Vec<Run>,
    /// Each UTF-16 unit's composed-character range: location, length.
    pub composed: Vec<(usize, usize)>,
}

/// `units` laid out in the open font `handle` (`mac_font_create_line_with_string_and_font`): kerning off, every run in
/// that font.  `None`: not open, or Core Text needed another font.
pub fn shape(handle: u64, units: &[u16]) -> Option<Line> {
    super::fonts::with_font(handle, |font| shape_in(font, units))?
}

fn shape_in(font: CFRef, units: &[u16]) -> Option<Line> {
    let zero = 0.0f32;
    // SAFETY: a float32 CFNumber, +1.
    let kern = Cf::new(unsafe {
        CFNumberCreate(std::ptr::null(), NUMBER_FLOAT32, (&raw const zero).cast())
    })?;
    // SAFETY: the attribute keys are Core Text's; the font and number are live.
    let attrs = dictionary(
        &[unsafe { kCTFontAttributeName }, unsafe {
            kCTKernAttributeName
        }],
        &[font, kern.get()],
    )?;
    let l = line(units, &attrs)?;
    let want = postscript(font)?;
    let used = single_font(l.get());
    // a line with no runs (empty text) is in no other font
    if let Some(f) = used
        && f != font
        && !postscript(f).is_some_and(|n| unsafe { CFEqual(n.get(), want.get()) != 0 })
    {
        return None;
    }
    // SAFETY: a live line; each run is borrowed from it, and each buffer holds the counts asked for.
    let runs = unsafe {
        let runs = CTLineGetGlyphRuns(l.get());
        if used.is_none() && CFArrayGetCount(runs) > 0 {
            return None;
        }
        (0..CFArrayGetCount(runs))
            .map(|k| {
                let r = CFArrayGetValueAtIndex(runs, k);
                let n = CTRunGetGlyphCount(r).max(0) as usize;
                let all = CFRange {
                    location: 0,
                    length: 0,
                };
                let mut indices = vec![0isize; n];
                let mut glyphs = vec![0u16; n];
                let mut pos = vec![CGPoint::default(); n];
                CTRunGetStringIndices(r, all, indices.as_mut_ptr());
                CTRunGetGlyphs(r, all, glyphs.as_mut_ptr());
                CTRunGetPositions(r, all, pos.as_mut_ptr());
                let widths = (0..n)
                    .map(|i| {
                        let one = CFRange {
                            location: i as isize,
                            length: 1,
                        };
                        CTRunGetTypographicBounds(
                            r,
                            one,
                            std::ptr::null_mut(),
                            std::ptr::null_mut(),
                            std::ptr::null_mut(),
                        )
                    })
                    .collect();
                let sr = CTRunGetStringRange(r);
                Run {
                    rtl: CTRunGetStatus(r) & RUN_RTL != 0,
                    range: (sr.location.max(0) as usize, sr.length.max(0) as usize),
                    indices: indices.into_iter().map(|i| i.max(0) as usize).collect(),
                    glyphs,
                    positions: pos.into_iter().map(|p| (p.x, p.y)).collect(),
                    widths,
                }
            })
            .collect()
    };
    let s = cf_units(units)?;
    let composed = (0..units.len())
        .map(|i| {
            // SAFETY: a live string and an index in it.
            let r = unsafe { CFStringGetRangeOfComposedCharactersAtIndex(s.get(), i as isize) };
            (r.location.max(0) as usize, r.length.max(0) as usize)
        })
        .collect();
    Some(Line { runs, composed })
}

//! The native font driver's Rust half on macOS (`src/nhmacfont.c` is the C half): the Core Text font source the
//! matching in `fontspec.rs` lists from, a font's metrics and its glyphs' extents rounded as `macfont_open` and
//! `macfont_glyph_extents` round them, the layout of a run as `macfont_draw` places it, and shaping as `macfont_shape`
//! does it.  C reads glyphs and their metrics from each font's glyph table (`glyphs.rs`).
//!
//! macfont's rules are copied rule by rule (`macfont.m`, cited per function); they are platform-neutral and tested on
//! Linux.  The Core Text calls are the shell's (`nhshell::macos::text`, `nhshell::macos::fonts`).

use crate::fontspec::{NHG_FS_SPACING_MONO, NHG_FS_SPACING_PROPORTIONAL};

/// A font's metrics as C stores them in `struct font`.  Layout equals `NhgFontMetrics`.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct NhgFontMetrics {
    pub handle: u64,
    pub pixel_size: i32,
    pub ascent: i32,
    pub descent: i32,
    pub height: i32,
    pub space_width: i32,
    pub average_width: i32,
    pub underline_position: i32,
    pub underline_thickness: i32,
    /// 1 for a monospaced font.
    pub mono: u32,
}

/// One glyph's metrics, as `struct font_metrics`.  Layout equals `NhgGlyphMetrics`.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct NhgGlyphMetrics {
    pub lbearing: i32,
    pub rbearing: i32,
    pub width: i32,
    pub ascent: i32,
    pub descent: i32,
}

/// One shaped glyph, as an LGLYPH takes it (`macfont_shape`).  `from` and `to` are the first and last character of
/// its cluster; `c` its character, 0 when the glyph is not that character's own; `adjust` is 1 when `xoff`, `yoff` and
/// `wadjust` are the glyph's adjustment.  Layout equals `NhgShaped`.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct NhgShaped {
    pub from: i32,
    pub to: i32,
    pub c: u32,
    pub code: u32,
    pub m: NhgGlyphMetrics,
    pub xoff: i32,
    pub yoff: i32,
    pub wadjust: i32,
    pub adjust: u32,
}

/// `nhg_font_open`'s flags: the entity's synthesised traits (`NHG_FS_SYNTH_*`) and `:minspace`.
pub const NHG_FONT_SYNTH_ITALIC: u32 = 1;
pub const NHG_FONT_SYNTH_BOLD: u32 = 2;
pub const NHG_FONT_SYNTH_MONO: u32 = 4;
pub const NHG_FONT_MINSPACE: u32 = 8;

/// `macfont_info->spacing`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Spacing {
    #[default]
    Proportional,
    Mono,
    /// A proportional face laid out on a grid of `pixel_size` (a CJK face asked for as monospaced).
    SyntheticMono,
}

/// What a font's glyphs are rounded on: the fields of `macfont_info` and `struct font` that
/// `macfont_glyph_extents` reads.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Grid {
    pub spacing: Spacing,
    pub space_width: i32,
    pub pixel_size: i32,
    /// The font's size in points (`CTFontGetSize`): the synthetic bold's stroke is a fraction of it.
    pub size: f64,
    pub synthetic_italic: bool,
    pub synthetic_bold: bool,
}

/// `WIDTH_FRAC_SCALE`: `macfont.m` keeps a glyph's rounding in fourteenths.
const FRAC_SCALE: f64 = 14.0;
/// `LCD_FONT_SMOOTHING_LEFT_MARGIN` and `_RIGHT_MARGIN` (floats in macfont).
const SMOOTHING_MARGIN: f64 = 0.396f32 as f64;
/// `synthetic_bold_factor`.
pub const SYNTHETIC_BOLD_FACTOR: f64 = 0.024;
/// `synthetic_italic_atfm`'s c: x grows by a quarter of y.
pub const SYNTHETIC_ITALIC_SKEW: f64 = 0.25;

/// `macfont_monospace_width_multiplier`: how many cells a monospaced glyph `advance` wide covers, 0 for one cell or a
/// width off the grid.
pub fn multiplier(grid: &Grid, advance: f64) -> i32 {
    if grid.spacing != Spacing::Mono || grid.space_width == 0 {
        return 0;
    }
    let m = (advance / f64::from(grid.space_width)).round() as i32;
    if m == 1 || m == 0 || (advance / f64::from(m)).round() as i32 != grid.space_width {
        return 0;
    }
    m
}

/// The cache's `width_int` and `width_frac` of a glyph `advance` wide (`macfont_glyph_extents`).
fn parts(grid: &Grid, advance: f64) -> (i32, i32) {
    match grid.spacing {
        Spacing::Mono => match multiplier(grid, advance) {
            0 => (advance.round() as i32, 0),
            m => (
                grid.space_width * m,
                ((advance / f64::from(m) - f64::from(grid.space_width)) * FRAC_SCALE).round()
                    as i32,
            ),
        },
        Spacing::Proportional | Spacing::SyntheticMono => {
            let w = if grid.spacing == Spacing::SyntheticMono {
                (f64::from(grid.pixel_size) - advance) / 2.0
            } else {
                advance
            };
            let int = w.round() as i32;
            (int, ((w - f64::from(int)) * FRAC_SCALE).round() as i32)
        }
    }
}

/// A glyph's width (`macfont_glyph_extents`' return value).
pub fn width(grid: &Grid, advance: f64) -> i32 {
    match grid.spacing {
        Spacing::SyntheticMono => grid.pixel_size,
        _ => parts(grid, advance).0,
    }
}

/// A glyph's metrics from its `advance` and Core Text's bounds (left, right, top, bottom; y up), as
/// `macfont_glyph_extents` makes them: synthetic italic and bold widen the bounds, the rounding of the width moves them,
/// LCD smoothing's margins pad them, and `CGRectIntegral` rounds them outwards.
pub fn extents(
    grid: &Grid,
    advance: f64,
    (left, right, top, bottom): (f64, f64, f64, f64),
) -> NhgGlyphMetrics {
    let (mut x, mut y, mut w, mut h) = (left, bottom, right - left, top - bottom);
    if grid.synthetic_italic {
        x += SYNTHETIC_ITALIC_SKEW * y;
        w += SYNTHETIC_ITALIC_SKEW * h;
    }
    if grid.synthetic_bold {
        let d = -SYNTHETIC_BOLD_FACTOR * grid.size / 2.0;
        x += d;
        y += d;
        w -= 2.0 * d;
        h -= 2.0 * d;
    }
    let (int, frac) = parts(grid, advance);
    match grid.spacing {
        Spacing::Proportional => x -= f64::from(frac) / (FRAC_SCALE * 2.0),
        Spacing::Mono => {
            if frac != 0 && grid.space_width != 0 {
                x -= f64::from(frac) / (FRAC_SCALE * 2.0) * f64::from(int / grid.space_width);
            }
        }
        Spacing::SyntheticMono => x += f64::from(int) + f64::from(frac) / FRAC_SCALE,
    }
    if w > 0.0 {
        x -= SMOOTHING_MARGIN;
        w += 2.0 * SMOOTHING_MARGIN;
    }
    NhgGlyphMetrics {
        lbearing: x.floor() as i32,
        rbearing: (x + w).ceil() as i32,
        width: width(grid, advance),
        ascent: (y + h).ceil() as i32,
        descent: -(y.floor() as i32),
    }
}

/// How far a glyph is drawn from its pen position (`macfont_glyph_extents`' `advance_delta`); `integral` is
/// `force_integral_p` (text drawn without antialiasing).
pub fn advance_delta(grid: &Grid, advance: f64, integral: bool) -> f64 {
    let (int, frac) = parts(grid, advance);
    match grid.spacing {
        Spacing::Proportional if integral => 0.0,
        Spacing::Proportional => -f64::from(frac) / (FRAC_SCALE * 2.0),
        // macfont's delta for a monospaced glyph is 0 either way
        Spacing::Mono => 0.0,
        Spacing::SyntheticMono if integral => f64::from(int),
        Spacing::SyntheticMono => f64::from(int) + f64::from(frac) / FRAC_SCALE,
    }
}

/// Where each glyph of a run is drawn, from the run's x (`macfont_draw`): the sum of the widths before it (one pixel
/// each for a padding run) plus the glyph's advance delta.
pub fn run_offsets(advances: &[f64], grid: &Grid, padding: bool, integral: bool) -> Vec<f64> {
    let mut total = 0;
    advances
        .iter()
        .map(|&adv| {
            if padding {
                total += 1;
                return f64::from(total - 1);
            }
            let x = f64::from(total) + advance_delta(grid, adv, integral);
            total += width(grid, adv);
            x
        })
        .collect()
}

/// A font's metrics before rounding, in pixels (descent positive).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct RawFont {
    pub ascent: f64,
    pub descent: f64,
    pub leading: f64,
    pub underline_position: f64,
    pub underline_thickness: f64,
    pub mono: bool,
    /// Courier, Helvetica or Times, whose ascent AppKit makes taller.
    pub tall_family: bool,
    /// A Hiragino family, whose leading macfont quarters and moves into the ascent.
    pub hiragino: bool,
}

/// `macfont_open`'s metrics: `space` is the space's advance (`None`: no space glyph), `printable` the advances of
/// U+0021–U+007E (`None` where the font has no glyph); `minspace` the entity's `:minspace`.
pub fn font_metrics(
    raw: &RawFont,
    grid: &Grid,
    space: Option<f64>,
    printable: &[Option<f64>],
    minspace: bool,
) -> NhgFontMetrics {
    let space_width = space.map_or(grid.pixel_size, |a| {
        width(
            &Grid {
                space_width: 0,
                ..*grid
            },
            a,
        )
    });
    let g = Grid {
        space_width,
        ..*grid
    };
    let mut total = space_width;
    let mut all = true;
    for a in printable {
        match a {
            Some(a) => total += width(&g, *a),
            None => {
                all = false;
                break;
            }
        }
    }
    let average_width = if all && printable.len() == 94 {
        total / 95
    } else {
        space_width
    };
    // macfont's CGFloat arithmetic (its `.15f` and `.25f` are exact enough widened), truncated as C converts
    let (mut ascent, descent, mut leading) = (raw.ascent, raw.descent, raw.leading);
    if raw.tall_family {
        ascent += (ascent + descent) * f64::from(0.15f32);
    } else if raw.hiragino {
        leading *= 0.25;
        ascent += leading;
    }
    let ascent = (ascent + 0.5) as i32;
    let descent = if minspace {
        (descent + 0.5) as i32
    } else {
        (descent + leading + 0.5) as i32
    };
    NhgFontMetrics {
        handle: 0,
        pixel_size: grid.pixel_size,
        ascent,
        descent,
        height: ascent + descent,
        space_width,
        average_width,
        underline_position: (-raw.underline_position + 0.5) as i32,
        underline_thickness: (raw.underline_thickness + 0.5) as i32,
        mono: u32::from(grid.spacing == Spacing::Mono),
    }
}

/// The spacing of a face whose traits have fixed pitch or not.
pub fn spacing(fixed_pitch: bool) -> i32 {
    if fixed_pitch {
        NHG_FS_SPACING_MONO
    } else {
        NHG_FS_SPACING_PROPORTIONAL
    }
}

// ---- Shaping (macfont_shape, mac_font_shape) ------------------------------------------------------------------------

/// One Core Text glyph run of a line, as `mac_font_shape` reads it: whether it runs right to left, its string range
/// (UTF-16), and per glyph its string index, glyph id, position and typographic width.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ShapeRun {
    pub rtl: bool,
    pub range: (usize, usize),
    pub indices: Vec<usize>,
    pub glyphs: Vec<u16>,
    pub positions: Vec<(f64, f64)>,
    pub widths: Vec<f64>,
}

/// One glyph's layout (`struct mac_glyph_layout`).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Layout {
    pub string_index: usize,
    /// The composed characters the glyph belongs to: (location, length), UTF-16.
    pub comp: (usize, usize),
    pub glyph: u16,
    pub advance_delta: f64,
    pub baseline_delta: f64,
    pub advance: f64,
}

/// `mac_font_shape`'s layouts of a line's `runs`, in display order within each run.  `composed(i)` is
/// `CFStringGetRangeOfComposedCharactersAtIndex` of the line's string at UTF-16 index `i`.
pub fn layouts(runs: &[ShapeRun], composed: &dyn Fn(usize) -> (usize, usize)) -> Vec<Layout> {
    let mut out: Vec<Layout> = Vec::new();
    let mut total_advance = 0.0f64;
    for run in runs {
        let n = run.glyphs.len();
        let base = out.len();
        out.resize(base + n, Layout::default());
        let gl = &mut out[base..];
        let mut perm = vec![0usize; if run.rtl { n } else { 0 }];
        // comp holds, for now: .0 the least location of the composed ranges after this glyph, .1 its range's end
        let mut min_location = run.range.0 + run.range.1;
        for i in 0..n {
            let g = &mut gl[n - i - 1];
            let glyph_index = if run.rtl { i } else { n - i - 1 };
            g.string_index = run.indices[glyph_index];
            let (loc, len) = composed(g.string_index);
            g.comp = (min_location, loc + len);
            min_location = min_location.min(loc);
        }
        if n > 0 {
            let (mut cloc, mut clen) = (run.range.0, 0usize);
            let (mut rloc, mut rlen) = (0usize, 0usize);
            loop {
                let g = gl[rloc + rlen];
                if g.comp.1 > cloc + clen {
                    clen = g.comp.1 - cloc;
                }
                let min_location = g.comp.0;
                rlen += 1;
                // the last glyph always closes its group (macfont relies on Core Text's ranges for that)
                let last = rloc + rlen == n;
                if min_location >= cloc + clen || last {
                    clen = min_location.max(cloc + clen) - cloc;
                    for i in 0..rlen {
                        gl[rloc + i].comp = (cloc, clen);
                        if run.rtl {
                            perm[rloc + i] = rloc + rlen - i - 1;
                        }
                    }
                    cloc = min_location;
                    clen = 0;
                    rloc += rlen;
                    rlen = 0;
                    if rloc == n {
                        break;
                    }
                }
            }
        }
        for loc in 0..n {
            let at = if run.rtl {
                let src = n - 1 - loc;
                let dest = perm[src];
                if src < dest {
                    let t = gl[dest].string_index;
                    gl[dest].string_index = gl[src].string_index;
                    gl[src].string_index = t;
                }
                dest
            } else {
                loc
            };
            let g = &mut gl[at];
            g.glyph = run.glyphs[loc];
            let (px, py) = run.positions[loc];
            let max_x = (px + run.widths[loc]).max(total_advance);
            g.advance_delta = px - total_advance;
            g.baseline_delta = py;
            g.advance = max_x - total_advance;
            total_advance = max_x;
        }
    }
    out
}

/// `macfont_shape`'s glyphs from `layouts` of the UTF-16 `units` (of `chars`, one each but two for a character beyond
/// the BMP): clusters as character indices, each glyph's character when the glyph is that character's own (`glyph_of`,
/// the font's table), its metrics (`metrics_of`), and the adjustment Core Text's position and advance make.
pub fn shaped(
    layouts: &[Layout],
    units: &[u16],
    grid: &Grid,
    glyph_of: &mut dyn FnMut(u32) -> Option<u32>,
    metrics_of: &mut dyn FnMut(u32) -> NhgGlyphMetrics,
) -> Vec<NhgShaped> {
    // the UTF-16 index of each character beyond the BMP (its high surrogate)
    let nonbmp: Vec<usize> = (0..units.len())
        .filter(|&i| (0xD800..0xDC00).contains(&units[i]) && i + 1 < units.len())
        .collect();
    let to_char = |u: usize| u - nonbmp.iter().take_while(|&&i| i < u).count();
    layouts
        .iter()
        .map(|gl| {
            let from = to_char(gl.comp.0);
            let to = to_char(gl.comp.0 + gl.comp.1);
            let s = gl.string_index;
            let c = match units.get(s) {
                Some(&hi) if (0xD800..0xDC00).contains(&hi) && s + 1 < units.len() => {
                    0x10000 + ((u32::from(hi) - 0xD800) << 10) + (u32::from(units[s + 1]) - 0xDC00)
                }
                Some(&u) => u32::from(u),
                None => 0,
            };
            let code = u32::from(gl.glyph);
            let c = if glyph_of(c) == Some(code) { c } else { 0 };
            let m = metrics_of(code);
            let xoff = gl.advance_delta.round() as i32;
            let yoff = (-gl.baseline_delta).round() as i32;
            let wadjust = match multiplier(grid, gl.advance) {
                0 => gl.advance.round() as i32,
                k => grid.space_width * k,
            };
            NhgShaped {
                from: from as i32,
                to: to as i32 - 1,
                c,
                code,
                m,
                xoff,
                yoff,
                wadjust,
                adjust: u32::from(xoff != 0 || yoff != 0 || wadjust != m.width),
            }
        })
        .collect()
}

/// The UTF-16 units of `chars` (invalid characters as U+FFFD).
pub fn utf16(chars: &[u32]) -> Vec<u16> {
    let mut out = Vec::with_capacity(chars.len());
    for &c in chars {
        let mut b = [0u16; 2];
        out.extend_from_slice(char::from_u32(c).unwrap_or('\u{fffd}').encode_utf16(&mut b));
    }
    out
}

// ---- Scripts: Emacs's script-representative-chars ---------------------------------------------------------------

/// `script-representative-chars` as C hands it to `nhg_font_start`: per script its name id, a count, then that many
/// characters.  A face has the script when it has every one of them (a list in Lisp), or, with `NHG_FONT_ANY_CHAR` in
/// the count, any one of them (a vector; `macfont_supports_charset_and_languages_p`).
pub const NHG_FONT_ANY_CHAR: u32 = 0x8000_0000;

/// A script's representative characters.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ScriptChars {
    pub chars: Vec<u32>,
    /// Any one of `chars` is enough (else all of them).
    pub any: bool,
}

impl ScriptChars {
    /// Whether a face whose character set is `has` has the script.
    pub fn covered(&self, has: &dyn Fn(u32) -> bool) -> bool {
        if self.any {
            self.chars.iter().any(|&c| has(c))
        } else {
            self.chars.iter().all(|&c| has(c))
        }
    }
}

/// The scripts of the flat table `flat`: (name id, characters).
pub fn parse_scripts(flat: &[u32]) -> Vec<(u32, ScriptChars)> {
    let mut out = Vec::new();
    let mut i = 0;
    while i + 1 < flat.len() {
        let (id, n) = (flat[i], flat[i + 1]);
        i += 2;
        let any = n & NHG_FONT_ANY_CHAR != 0;
        let n = (n & !NHG_FONT_ANY_CHAR) as usize;
        if i + n > flat.len() {
            break;
        }
        out.push((
            id,
            ScriptChars {
                chars: flat[i..i + n].to_vec(),
                any,
            },
        ));
        i += n;
    }
    out
}

#[cfg(all(feature = "macos", target_os = "macos"))]
mod coretext {
    use super::*;
    use crate::fontspec::{FontFace, FontSource};
    use crate::glyphs::{GlyphSource, NHG_FONT_FILL_GLYPHS, NHG_FONT_PAGE, NhgFontTable, Table};
    use nhshell::macos::fonts as ct;
    use nhshell::macos::text;
    use std::collections::HashMap;
    use std::sync::atomic::Ordering::Relaxed;
    use std::sync::{Mutex, MutexGuard, OnceLock};

    /// The scripts' characters (`nhg_font_start`), and each script's preferred family once asked.
    #[derive(Default)]
    struct Scripts {
        chars: HashMap<String, ScriptChars>,
        preferred: HashMap<String, Option<String>>,
    }

    fn scripts() -> MutexGuard<'static, Scripts> {
        static S: OnceLock<Mutex<Scripts>> = OnceLock::new();
        S.get_or_init(Mutex::default)
            .lock()
            .unwrap_or_else(|p| p.into_inner())
    }

    pub fn set_scripts(named: Vec<(String, ScriptChars)>) {
        let mut s = scripts();
        s.chars = named.into_iter().collect();
        s.preferred.clear();
    }

    /// The faces Core Text lists (`macfont_list`'s families, in Core Text's order); a face's id is its index plus one.
    pub struct CoreText;

    impl FontSource for CoreText {
        fn faces(&self) -> Vec<FontFace> {
            text::faces()
                .iter()
                .enumerate()
                .map(|(i, f)| FontFace {
                    id: i as u64 + 1,
                    family: f.family.clone(),
                    foundry: "apple".into(),
                    weight: crate::fontspec::weight_from_trait(f.weight),
                    slant: crate::fontspec::slant_from_trait(f.slant),
                    width: crate::fontspec::width_from_trait(f.width),
                    spacing: spacing(f.symbolic & text::TRAIT_MONO != 0),
                    // coverage is read lazily, in `fits`
                    scripts: Vec::new(),
                    languages: Vec::new(),
                    registries: Vec::new(),
                    color: f.symbolic & text::TRAIT_COLOR != 0,
                    bitmap_only: false,
                })
                .collect()
        }

        fn aliases(&self, family: &str) -> Vec<String> {
            let names: &[&str] = match family.to_ascii_lowercase().as_str() {
                "monospace" | "mono" => &["Menlo", "Monaco", "Courier"],
                "sans-serif" | "sans" => &["Helvetica", "Helvetica Neue"],
                "serif" => &["Times", "Times New Roman"],
                _ => &[],
            };
            names.iter().map(|s| (*s).to_string()).collect()
        }

        fn preferred(&self, script: &str) -> Option<String> {
            let mut s = scripts();
            if let Some(p) = s.preferred.get(script) {
                return p.clone();
            }
            // macfont asks only for a list's characters, not a vector's
            let chars = s
                .chars
                .get(script)
                .filter(|c| !c.any && !c.chars.is_empty())?
                .chars
                .clone();
            let p = text::preferred_family(&chars);
            s.preferred.insert(script.to_owned(), p.clone());
            p
        }

        /// A registry's preferred family: its characters laid out in its language (macfont's charset string).
        fn preferred_chars(&self, chars: &[u32], language: &str) -> Option<String> {
            text::preferred_family_in(chars, language)
        }

        /// `macfont_supports_charset_and_languages_p`: the face's character set has the script's characters, and its
        /// languages the one asked for (`zh-Hans` is had by a face that lists `zh`).  A script without characters is
        /// had by every face.
        fn fits(&self, face: &FontFace, script: &str, language: &str) -> bool {
            let faces = text::faces();
            let Some(f) = usize::try_from(face.id)
                .ok()
                .and_then(|i| i.checked_sub(1))
                .and_then(|i| faces.get(i))
            else {
                return false;
            };
            if !script.is_empty() {
                let chars = scripts().chars.get(script).cloned();
                if let Some(c) = chars
                    && !c.covered(&|ch| f.has(ch))
                {
                    return false;
                }
            }
            language.is_empty() || {
                let l = f.languages();
                l.iter().any(|x| x == language)
                    || (language == "zh-Hans" && l.iter().any(|x| x == "zh"))
            }
        }

        /// The face's character set (`macfont_has_char` on an entity).
        fn has_char(&self, id: u64, c: u32) -> Option<bool> {
            let faces = text::faces();
            let f = usize::try_from(id)
                .ok()
                .and_then(|i| i.checked_sub(1))
                .and_then(|i| faces.get(i))?;
            Some(f.has(c))
        }
    }

    /// A font's glyphs and bounds from Core Text, one batched call per table page.
    struct Source(u64);

    impl GlyphSource for Source {
        fn glyphs(&self, units: &[u16], out: &mut [u16]) {
            ct::glyphs_for(self.0, units, out);
        }

        fn bounds(&self, glyphs: &[u16], out: &mut [[f64; 5]]) {
            ct::bounds_for(self.0, glyphs, out);
        }
    }

    /// Each open font's glyph table.
    fn tables() -> MutexGuard<'static, HashMap<u64, Table>> {
        static TABLES: OnceLock<Mutex<HashMap<u64, Table>>> = OnceLock::new();
        TABLES
            .get_or_init(Mutex::default)
            .lock()
            .unwrap_or_else(|p| p.into_inner())
    }

    /// `macfont_open`: the face's font at `size` pixels, its spacing and synthesised traits from `flags`.
    pub fn open(id: u64, size: i32, flags: u32) -> Option<NhgFontMetrics> {
        let faces = text::faces();
        let face = faces.get(usize::try_from(id).ok()?.checked_sub(1)?)?;
        let synthetic_italic =
            flags & NHG_FONT_SYNTH_ITALIC != 0 && face.symbolic & text::TRAIT_ITALIC == 0;
        let synthetic_bold =
            flags & NHG_FONT_SYNTH_BOLD != 0 && face.symbolic & text::TRAIT_BOLD == 0;
        let (handle, m) = ct::open(
            &face.postscript,
            f64::from(size),
            synthetic_italic,
            synthetic_bold,
        )?;
        let spacing = if m.symbolic & text::TRAIT_MONO != 0 {
            Spacing::Mono
        } else if flags & NHG_FONT_SYNTH_MONO != 0 {
            Spacing::SyntheticMono
        } else {
            Spacing::Proportional
        };
        let raw = RawFont {
            ascent: m.ascent,
            descent: m.descent,
            leading: m.leading,
            underline_position: m.underline_position,
            underline_thickness: m.underline_thickness,
            mono: spacing == Spacing::Mono,
            tall_family: matches!(m.family.as_str(), "Courier" | "Helvetica" | "Times"),
            hiragino: m.family.starts_with("Hiragino"),
        };
        let mut grid = Grid {
            spacing,
            space_width: 0,
            // `(int) pointSize`
            pixel_size: m.size as i32,
            size: m.size,
            synthetic_italic,
            synthetic_bold,
        };
        let src = Source(handle);
        // the first page answers the space and the printable ASCII macfont_open measures, in one call each
        let mut table = Table::new(grid);
        table.fill(0, &src);
        let gl: Vec<Option<u32>> = (0x20..0x7f)
            .map(|c| table.char_glyph(c).flatten())
            .collect();
        let ids: Vec<u16> = gl.iter().map(|g| g.unwrap_or(0) as u16).collect();
        let mut b = vec![[0.0f64; 5]; ids.len()];
        ct::bounds_for(handle, &ids, &mut b);
        let advance = |i: usize| gl[i].map(|_| b[i][0]);
        let printable: Vec<Option<f64>> = (1..gl.len()).map(advance).collect();
        let mut out = font_metrics(
            &raw,
            &grid,
            advance(0),
            &printable,
            flags & NHG_FONT_MINSPACE != 0,
        );
        out.handle = handle;
        grid.space_width = out.space_width;
        table.set_grid(grid);
        tables().insert(handle, table);
        Some(out)
    }

    pub fn table(handle: u64) -> Option<*const NhgFontTable> {
        tables().get(&handle).map(Table::as_ptr)
    }

    pub fn fill(handle: u64, page: u32) -> Option<bool> {
        let mut tables = tables();
        let t = tables.get_mut(&handle)?;
        ct::STATS.fills.fetch_add(1, Relaxed);
        Some(t.fill(page, &Source(handle)))
    }

    pub fn run(handle: u64, glyphs: &[u16], padding: bool) -> Option<(Vec<f64>, bool)> {
        let integral = ct::integral(handle)?;
        let mut tables = tables();
        let t = tables.get_mut(&handle)?;
        let advances = t.advances(glyphs, &Source(handle));
        Some((
            run_offsets(&advances, &t.grid(), padding, integral),
            integral,
        ))
    }

    /// `macfont_shape`: `None` when the font is not open or Core Text lays the text out in another font.
    pub fn shape(handle: u64, chars: &[u32]) -> Option<Vec<NhgShaped>> {
        let units = utf16(chars);
        let line = text::shape(handle, &units)?;
        let runs: Vec<ShapeRun> = line
            .runs
            .into_iter()
            .map(|r| ShapeRun {
                rtl: r.rtl,
                range: r.range,
                indices: r.indices,
                glyphs: r.glyphs,
                positions: r.positions,
                widths: r.widths,
            })
            .collect();
        let composed = line.composed;
        let lay = layouts(&runs, &|i| composed.get(i).copied().unwrap_or((i, 1)));
        let mut tables = tables();
        let t = tables.get_mut(&handle)?;
        let src = Source(handle);
        let grid = t.grid();
        // `shaped` asks for glyphs and metrics through two closures; both read the one table
        let t = std::cell::RefCell::new(t);
        let mut glyph_of = |c: u32| {
            let mut t = t.borrow_mut();
            t.fill(c / NHG_FONT_PAGE as u32, &src);
            t.char_glyph(c).flatten()
        };
        let mut metrics_of = |g: u32| {
            let mut t = t.borrow_mut();
            t.fill(NHG_FONT_FILL_GLYPHS | (g / NHG_FONT_PAGE as u32), &src);
            t.metrics(g).unwrap_or_default()
        };
        Some(shaped(&lay, &units, &grid, &mut glyph_of, &mut metrics_of))
    }

    pub fn note_call() {
        ct::STATS.calls.fetch_add(1, Relaxed);
    }

    pub fn close(handle: u64) {
        tables().remove(&handle);
        ct::close(handle);
    }
}

/// Install the Core Text font source (macOS) with the scripts of `scripts` (`nhg_font_start`'s flat table, `parse_scripts`);
/// false elsewhere.
pub fn start(scripts: &[u32]) -> bool {
    #[cfg(all(feature = "macos", target_os = "macos"))]
    {
        let named = parse_scripts(scripts)
            .into_iter()
            .map(|(id, chars)| (crate::fontspec::name_of(id), chars))
            .collect();
        coretext::set_scripts(named);
        crate::fontspec::set_source(Box::new(coretext::CoreText));
        true
    }
    #[cfg(not(all(feature = "macos", target_os = "macos")))]
    {
        let _ = scripts;
        false
    }
}

/// Open face `id` at `size` pixels with `flags` (`NHG_FONT_*`); `None`: no such face, or no Core Text.
pub fn open(id: u64, size: i32, flags: u32) -> Option<NhgFontMetrics> {
    #[cfg(all(feature = "macos", target_os = "macos"))]
    return coretext::open(id, size, flags);
    #[cfg(not(all(feature = "macos", target_os = "macos")))]
    {
        let _ = (id, size, flags);
        None
    }
}

pub fn close(handle: u64) {
    #[cfg(all(feature = "macos", target_os = "macos"))]
    coretext::close(handle);
    #[cfg(not(all(feature = "macos", target_os = "macos")))]
    let _ = handle;
}

/// The glyph table of the open font `handle` (`glyphs.rs`), whose address C keeps while the font is open.
pub fn table(handle: u64) -> Option<*const crate::glyphs::NhgFontTable> {
    #[cfg(all(feature = "macos", target_os = "macos"))]
    return coretext::table(handle);
    #[cfg(not(all(feature = "macos", target_os = "macos")))]
    {
        let _ = handle;
        None
    }
}

/// Fill page `page` of the open font `handle`'s table (`Table::fill`): `None` for a font that is not open, false for
/// a page past the table.
pub fn fill(handle: u64, page: u32) -> Option<bool> {
    #[cfg(all(feature = "macos", target_os = "macos"))]
    return coretext::fill(handle, page);
    #[cfg(not(all(feature = "macos", target_os = "macos")))]
    {
        let _ = (handle, page);
        None
    }
}

/// Shape `chars` in the open font `handle` (`macfont_shape`): `None` without Core Text or for a font that is not open,
/// an empty list when Core Text lays the text out in another font (macfont gives up then too).
pub fn shape(handle: u64, chars: &[u32]) -> Option<Vec<NhgShaped>> {
    #[cfg(all(feature = "macos", target_os = "macos"))]
    return coretext::shape(handle, chars)
        .or(Some(Vec::new()))
        .filter(|_| table(handle).is_some());
    #[cfg(not(all(feature = "macos", target_os = "macos")))]
    {
        let _ = (handle, chars);
        None
    }
}

/// Count one `nhg_font_*` call (`(nhmac-stats)`'s `:font-calls`; macOS).
pub fn note_call() {
    #[cfg(all(feature = "macos", target_os = "macos"))]
    coretext::note_call();
}

/// A run of `glyphs` in the font `handle`: each glyph's offset from the run's x (`run_offsets`), and whether it is drawn
/// without antialiasing.  `None` without Core Text or for a font that is not open.
pub fn run(handle: u64, glyphs: &[u16], padding: bool) -> Option<(Vec<f64>, bool)> {
    #[cfg(all(feature = "macos", target_os = "macos"))]
    return coretext::run(handle, glyphs, padding);
    #[cfg(not(all(feature = "macos", target_os = "macos")))]
    {
        let _ = (handle, glyphs, padding);
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn menlo_like() -> RawFont {
        RawFont {
            ascent: 11.138,
            descent: 2.830,
            leading: 0.0,
            underline_position: -1.6,
            underline_thickness: 0.72,
            mono: true,
            tall_family: false,
            hiragino: false,
        }
    }

    fn grid(spacing: Spacing, space_width: i32) -> Grid {
        Grid {
            spacing,
            space_width,
            pixel_size: 12,
            size: 12.0,
            synthetic_italic: false,
            synthetic_bold: false,
        }
    }

    #[test]
    fn widths_round_and_stay_on_the_grid() {
        let p = grid(Spacing::Proportional, 7);
        let m = grid(Spacing::Mono, 7);
        assert_eq!(width(&p, 7.22), 7);
        assert_eq!(width(&p, 7.6), 8);
        // a monospaced ligature of two cells stays two cells
        assert_eq!(width(&m, 14.9), 14);
        assert_eq!(width(&m, 7.22), 7);
        // no space width: the nearest integer
        assert_eq!(width(&grid(Spacing::Mono, 0), 7.6), 8);
        // a single cell is no multiple, and the advance must divide back to the space
        assert_eq!(width(&m, 7.6), 8);
        assert_eq!(width(&m, 16.0), 16);
        // a synthetic monospaced glyph is pixel_size wide
        assert_eq!(width(&grid(Spacing::SyntheticMono, 7), 7.6), 12);
    }

    #[test]
    fn runs_sit_on_rounded_widths_moved_by_half_the_rounding() {
        let p = grid(Spacing::Proportional, 7);
        // proportional: 6.6 rounds to 7 (0.4 short: 6/14, half of it left), 7.2 to 7 (0.2 over: 3/14)
        let xs = run_offsets(&[6.6, 7.2, 7.0], &p, false, false);
        assert_eq!(xs, vec![6.0 / 28.0, 7.0 - 3.0 / 28.0, 14.0]);
        // without antialiasing, or monospaced, on the whole pixels
        assert_eq!(run_offsets(&[6.6, 7.2], &p, false, true), vec![0.0, 7.0]);
        assert_eq!(
            run_offsets(&[7.22, 14.45, 7.22], &grid(Spacing::Mono, 7), false, false),
            vec![0.0, 7.0, 21.0]
        );
        // a padding run is one pixel per glyph
        assert_eq!(
            run_offsets(&[6.6, 7.2, 7.0], &p, true, false),
            vec![0.0, 1.0, 2.0]
        );
        // synthetic mono: centred in its pixel_size cell, (12 - 7.6) / 2 = 2.2: 2 and 3/14
        let s = grid(Spacing::SyntheticMono, 7);
        assert_eq!(
            run_offsets(&[7.6, 7.6], &s, false, false),
            vec![2.0 + 3.0 / 14.0, 12.0 + (2.0 + 3.0 / 14.0)]
        );
        assert_eq!(run_offsets(&[7.6], &s, false, true), vec![2.0]);
    }

    #[test]
    fn metrics_round_as_macfont() {
        let printable = vec![Some(7.22); 94];
        let m = font_metrics(
            &menlo_like(),
            &grid(Spacing::Mono, 0),
            Some(7.22),
            &printable,
            false,
        );
        assert_eq!((m.ascent, m.descent, m.height), (11, 3, 14));
        assert_eq!((m.space_width, m.average_width), (7, 7));
        assert_eq!((m.underline_position, m.underline_thickness), (2, 1));
        assert_eq!((m.pixel_size, m.mono), (12, 1));
    }

    #[test]
    fn missing_glyphs_fall_back_to_the_space() {
        let mut printable = vec![Some(9.0); 94];
        printable[10] = None;
        let g = grid(Spacing::Mono, 0);
        let m = font_metrics(&menlo_like(), &g, Some(7.22), &printable, false);
        assert_eq!(m.average_width, 7);
        let m = font_metrics(&menlo_like(), &g, None, &[], false);
        assert_eq!((m.space_width, m.average_width), (12, 12));
    }

    #[test]
    fn tall_families_hiragino_and_minspace() {
        let g = grid(Spacing::Proportional, 0);
        let tall = RawFont {
            tall_family: true,
            mono: false,
            ..menlo_like()
        };
        // 11.138 + (11.138 + 2.830) * 0.15 = 13.23
        assert_eq!(font_metrics(&tall, &g, Some(7.0), &[], false).ascent, 13);
        // Hiragino: a quarter of the leading, moved into the ascent: 11.138 + 1 = 12.138, descent 2.830 + 1
        let hira = RawFont {
            hiragino: true,
            leading: 4.0,
            ..menlo_like()
        };
        let m = font_metrics(&hira, &g, Some(7.0), &[], false);
        assert_eq!((m.ascent, m.descent), (12, 4));
        // :minspace leaves the leading out of the descent
        let lead = RawFont {
            leading: 2.0,
            ..menlo_like()
        };
        assert_eq!(font_metrics(&lead, &g, Some(7.0), &[], false).descent, 5);
        assert_eq!(font_metrics(&lead, &g, Some(7.0), &[], true).descent, 3);
    }

    #[test]
    fn extents_round_as_macfont() {
        // proportional, advance 7 exactly: no shift; the smoothing margins pad 0.396 each side
        let p = grid(Spacing::Proportional, 7);
        let e = extents(&p, 7.0, (0.4, 6.2, 8.1, -2.2));
        assert_eq!(
            e,
            NhgGlyphMetrics {
                lbearing: 0,
                rbearing: 7,
                width: 7,
                ascent: 9,
                descent: 3
            }
        );
        // advance 6.6: frac -6/14 moves the bounds right by 6/28 (0.214): 0.6 - 0.396 + 0.214 floors to 0, 0.6 + 0.214
        // + 0.396 = 1.21 ceils to 2
        let e = extents(&p, 6.6, (0.6, 0.6 + 0.0, 1.0, 0.0));
        assert_eq!((e.lbearing, e.rbearing, e.width), (0, 1, 7));
        let e = extents(&p, 6.6, (0.6, 1.0, 1.0, 0.0));
        assert_eq!((e.lbearing, e.rbearing), (0, 2));
        // an empty glyph (a space) gets no margins
        let e = extents(&p, 7.0, (0.0, 0.0, 0.0, 0.0));
        assert_eq!((e.lbearing, e.rbearing, e.ascent, e.descent), (0, 0, 0, 0));
        // synthetic italic leans the box right by a quarter of its height; synthetic bold grows it by 0.024 * size / 2
        let it = Grid {
            synthetic_italic: true,
            ..p
        };
        let e = extents(&it, 7.0, (1.0, 5.0, 8.0, 0.0));
        assert_eq!((e.lbearing, e.rbearing), (0, 8));
        let bold = Grid {
            synthetic_bold: true,
            size: 100.0,
            ..p
        };
        let e = extents(&bold, 7.0, (1.0, 5.0, 8.0, 0.0));
        assert_eq!(
            (e.lbearing, e.rbearing, e.ascent, e.descent),
            (-1, 7, 10, 2)
        );
        // synthetic mono: the box moves right by the centring delta (12 - 7.6) / 2 = 2.2
        let s = grid(Spacing::SyntheticMono, 7);
        let e = extents(&s, 7.6, (1.0, 5.0, 8.0, 0.0));
        assert_eq!((e.lbearing, e.rbearing, e.width), (2, 8, 12));
    }

    /// A run over UTF-16 units `start..start + len`.
    fn run(
        rtl: bool,
        (start, len): (usize, usize),
        glyphs: &[u16],
        indices: &[usize],
        xs: &[f64],
        ws: &[f64],
    ) -> ShapeRun {
        ShapeRun {
            rtl,
            range: (start, len),
            indices: indices.to_vec(),
            glyphs: glyphs.to_vec(),
            positions: xs.iter().map(|&x| (x, 0.0)).collect(),
            widths: ws.to_vec(),
        }
    }

    #[test]
    fn a_left_to_right_run_lays_out_glyph_by_glyph() {
        let r = run(
            false,
            (0, 3),
            &[10, 11, 12],
            &[0, 1, 2],
            &[0.0, 7.2, 14.4],
            &[7.2, 7.2, 7.2],
        );
        let l = layouts(&[r], &|i| (i, 1));
        assert_eq!(
            l.iter().map(|g| g.comp).collect::<Vec<_>>(),
            vec![(0, 1), (1, 1), (2, 1)]
        );
        assert_eq!(
            l.iter().map(|g| g.glyph).collect::<Vec<_>>(),
            vec![10, 11, 12]
        );
        assert!(
            l.iter()
                .all(|g| g.advance_delta.abs() < 1e-9 && (g.advance - 7.2).abs() < 1e-9)
        );
    }

    #[test]
    fn a_combining_mark_joins_its_base() {
        // "e" + U+0301: two glyphs, the mark at the base's x, one composed range (0, 2)
        let r = run(false, (0, 2), &[20, 21], &[0, 1], &[0.0, 0.5], &[7.0, 0.0]);
        let l = layouts(&[r], &|_| (0, 2));
        assert_eq!(l[0].comp, (0, 2));
        assert_eq!(l[1].comp, (0, 2));
        // the mark: 7 back from the pen, no advance of its own
        assert!((l[1].advance_delta - (0.5 - 7.0)).abs() < 1e-9);
        assert!(l[1].advance.abs() < 1e-9);
        let mut glyph_of = |c: u32| Some(if c == u32::from('e') { 20 } else { 21 });
        let mut metrics_of = |_g: u32| NhgGlyphMetrics {
            width: 7,
            ..Default::default()
        };
        let s = shaped(
            &l,
            &[u16::from(b'e'), 0x301],
            &grid(Spacing::Proportional, 7),
            &mut glyph_of,
            &mut metrics_of,
        );
        assert_eq!((s[0].from, s[0].to, s[0].c), (0, 1, u32::from('e')));
        assert_eq!(
            (
                s[1].from,
                s[1].to,
                s[1].c,
                s[1].xoff,
                s[1].wadjust,
                s[1].adjust
            ),
            (0, 1, 0x301, -7, 0, 1)
        );
        assert_eq!(s[0].adjust, 0);
    }

    #[test]
    fn a_right_to_left_run_keeps_logical_clusters() {
        // three Arabic letters, Core Text's glyphs in visual order (the last character's first), one cluster each:
        // macfont puts them back in logical order, each with its own advance
        let r = run(
            true,
            (0, 3),
            &[32, 31, 30],
            &[2, 1, 0],
            &[0.0, 5.0, 11.0],
            &[5.0, 6.0, 4.0],
        );
        let l = layouts(&[r], &|i| (i, 1));
        assert_eq!(
            l.iter().map(|g| g.glyph).collect::<Vec<_>>(),
            vec![30, 31, 32]
        );
        assert_eq!(
            l.iter().map(|g| g.comp).collect::<Vec<_>>(),
            vec![(0, 1), (1, 1), (2, 1)]
        );
        assert_eq!(
            l.iter().map(|g| g.string_index).collect::<Vec<_>>(),
            vec![0, 1, 2]
        );
        assert_eq!(
            l.iter().map(|g| g.advance).collect::<Vec<_>>(),
            vec![4.0, 6.0, 5.0]
        );
        // a two-glyph cluster in a right-to-left run: its glyphs keep their visual order, swapped back into place
        let r = run(true, (0, 2), &[41, 40], &[1, 0], &[0.0, 3.0], &[3.0, 4.0]);
        let l = layouts(&[r], &|_| (0, 2));
        assert_eq!(
            l.iter().map(|g| g.comp).collect::<Vec<_>>(),
            vec![(0, 2), (0, 2)]
        );
        assert_eq!(l.iter().map(|g| g.glyph).collect::<Vec<_>>(), vec![41, 40]);
    }

    #[test]
    fn characters_beyond_the_bmp_count_once() {
        // U+1F600 then "a": three UTF-16 units, two characters
        let units = utf16(&[0x1F600, u32::from('a')]);
        assert_eq!(units.len(), 3);
        let r = run(
            false,
            (0, 3),
            &[40, 41],
            &[0, 2],
            &[0.0, 16.0],
            &[16.0, 7.0],
        );
        let l = layouts(&[r], &|i| if i < 2 { (0, 2) } else { (2, 1) });
        let mut glyph_of = |c: u32| Some(if c == 0x1F600 { 40 } else { 41 });
        let mut metrics_of = |g: u32| NhgGlyphMetrics {
            width: if g == 40 { 16 } else { 7 },
            ..Default::default()
        };
        let s = shaped(
            &l,
            &units,
            &grid(Spacing::Proportional, 7),
            &mut glyph_of,
            &mut metrics_of,
        );
        assert_eq!((s[0].from, s[0].to, s[0].c), (0, 0, 0x1F600));
        assert_eq!((s[1].from, s[1].to, s[1].c), (1, 1, u32::from('a')));
        assert!(s.iter().all(|g| g.adjust == 0));
    }

    #[test]
    fn a_monospaced_ligature_adjusts_to_whole_cells() {
        let r = run(false, (0, 2), &[50], &[0], &[0.0], &[14.9]);
        let l = layouts(&[r], &|_| (0, 2));
        let mut glyph_of = |_c: u32| None;
        let mut metrics_of = |_g: u32| NhgGlyphMetrics {
            width: 14,
            ..Default::default()
        };
        let s = shaped(
            &l,
            &utf16(&[u32::from('-'), u32::from('>')]),
            &grid(Spacing::Mono, 7),
            &mut glyph_of,
            &mut metrics_of,
        );
        assert_eq!(
            (s[0].from, s[0].to, s[0].c, s[0].wadjust, s[0].adjust),
            (0, 1, 0, 14, 0)
        );
    }

    #[test]
    fn scripts_parse_lists_and_vectors() {
        let flat = [
            7,
            2,
            0x4E2D,
            0x6587,
            9,
            NHG_FONT_ANY_CHAR | 2,
            0x3B1,
            0x3B2,
            11,
            0,
        ];
        let got = parse_scripts(&flat);
        let list = ScriptChars {
            chars: vec![0x4E2D, 0x6587],
            any: false,
        };
        let vector = ScriptChars {
            chars: vec![0x3B1, 0x3B2],
            any: true,
        };
        assert_eq!(
            got,
            vec![
                (7, list.clone()),
                (9, vector.clone()),
                (11, ScriptChars::default())
            ]
        );
        // a list needs every character, a vector any one, no characters nothing
        let has = |c: u32| c == 0x4E2D || c == 0x3B2;
        assert!(!list.covered(&has));
        assert!(vector.covered(&has));
        assert!(ScriptChars::default().covered(&has));
        // a count past the end stops the table
        assert_eq!(parse_scripts(&[7, 5, 1]), vec![]);
    }
}

//! The native font driver's Rust half on macOS (`src/nhmacfont.c` is the C half): the Core Text font source the
//! matching in `fontspec.rs` lists from, and a font's metrics and glyphs, rounded as `macfont_open` and
//! `macfont_glyph_extents` round them.  One family's worth for now: no shaping; C reads glyphs and their metrics from
//! each font's glyph table (`glyphs.rs`), and a run's glyphs are placed as `macfont_draw` places them.
//!
//! The rounding is platform-neutral and tested on Linux; the Core Text calls are the shell's (`nhshell::macos::fonts`).

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
}

/// A glyph's advance width, rounded as `macfont_glyph_extents` rounds it: a monospaced font's glyph that is a whole
/// multiple (two or more) of the space's stays on the grid (wide ligatures), any other the nearest integer.
pub fn glyph_width(advance: f64, mono: bool, space_width: i32) -> i32 {
    if mono && space_width > 0 {
        let multiplier = (advance / f64::from(space_width)).round() as i32;
        if multiplier > 1 && (advance / f64::from(multiplier)).round() as i32 == space_width {
            return space_width * multiplier;
        }
    }
    advance.round() as i32
}

/// `WIDTH_FRAC_SCALE`: `macfont.m` keeps a glyph's rounding in fourteenths.
const FRAC_SCALE: f64 = 14.0;

/// Where each glyph of a run is drawn, from the run's x (`macfont_draw`): the sum of the rounded widths before it (one
/// pixel each for a padding run), moved left by half a proportional glyph's rounding, in fourteenths, unless the text
/// is drawn without antialiasing (`integral`).  A monospaced glyph is not moved (macfont's delta for one is 0).
pub fn run_offsets(
    advances: &[f64],
    mono: bool,
    space_width: i32,
    padding: bool,
    integral: bool,
) -> Vec<f64> {
    let mut total = 0;
    advances
        .iter()
        .map(|&adv| {
            if padding {
                total += 1;
                return f64::from(total - 1);
            }
            let delta = if mono || integral {
                0.0
            } else {
                -((adv - adv.round()) * FRAC_SCALE).round() / (FRAC_SCALE * 2.0)
            };
            let x = f64::from(total) + delta;
            total += glyph_width(adv, mono, space_width);
            x
        })
        .collect()
}

/// A glyph's metrics from its rounded `width` and its bounds (left, right, top, bottom; y up).
pub fn glyph_metrics(
    width: i32,
    (left, right, top, bottom): (f64, f64, f64, f64),
) -> NhgGlyphMetrics {
    NhgGlyphMetrics {
        lbearing: left.floor() as i32,
        rbearing: right.ceil() as i32,
        width,
        ascent: top.ceil() as i32,
        descent: (-bottom).ceil() as i32,
    }
}

/// `macfont_open`'s metrics: `space` is the space's advance (`None`: no space glyph), `printable` the advances of
/// U+0021–U+007E (`None` where the font has no glyph).
pub fn font_metrics(
    raw: &RawFont,
    size: i32,
    space: Option<f64>,
    printable: &[Option<f64>],
) -> NhgFontMetrics {
    let space_width = space.map_or(size, |a| a.round() as i32);
    let mut total = space_width;
    let mut all = true;
    for a in printable {
        match a {
            Some(a) => total += glyph_width(*a, raw.mono, space_width),
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
    let mut ascent = raw.ascent;
    if raw.tall_family {
        ascent += (raw.ascent + raw.descent) * 0.15;
    }
    let ascent = (ascent + 0.5) as i32;
    let descent = (raw.descent + raw.leading + 0.5) as i32;
    NhgFontMetrics {
        handle: 0,
        pixel_size: size,
        ascent,
        descent,
        height: ascent + descent,
        space_width,
        average_width,
        underline_position: (-raw.underline_position + 0.5) as i32,
        underline_thickness: (raw.underline_thickness + 0.5) as i32,
        mono: u32::from(raw.mono),
    }
}

/// `NSFontManager`'s weight (0–15) as Emacs's numeric weight.
pub fn weight_from_appkit(w: i64) -> i32 {
    match w {
        ..=1 => 0,      // thin
        2 => 40,        // ultra-light
        3 => 50,        // light
        4 => 55,        // semi-light
        5 => 80,        // normal
        6 => 100,       // medium
        7 | 8 => 180,   // semi-bold
        9 => 200,       // bold
        10 | 11 => 205, // extra-bold
        _ => 210,       // black
    }
}

/// The spacing of a face whose `NSFontTraitMask` has fixed pitch or not.
pub fn spacing(fixed_pitch: bool) -> i32 {
    if fixed_pitch {
        NHG_FS_SPACING_MONO
    } else {
        NHG_FS_SPACING_PROPORTIONAL
    }
}

#[cfg(all(feature = "macos", target_os = "macos"))]
mod coretext {
    use super::*;
    use crate::fontspec::{FontFace, FontSource};
    use crate::glyphs::{GlyphSource, NhgFontTable, Table};
    use nhshell::macos::fonts as ct;
    use std::collections::HashMap;
    use std::sync::atomic::Ordering::Relaxed;
    use std::sync::{Mutex, OnceLock};

    /// The faces AppKit lists, as the matching wants them.
    pub struct CoreText;

    impl FontSource for CoreText {
        fn faces(&self) -> Vec<FontFace> {
            ct::faces()
                .into_iter()
                .map(|f| FontFace {
                    id: f.id,
                    family: f.family,
                    foundry: "apple".into(),
                    weight: weight_from_appkit(f.weight),
                    slant: if f.traits & ct::NS_ITALIC != 0 {
                        200
                    } else {
                        100
                    },
                    width: if f.traits & ct::NS_CONDENSED != 0 {
                        75
                    } else if f.traits & ct::NS_EXPANDED != 0 {
                        125
                    } else {
                        100
                    },
                    spacing: spacing(f.traits & ct::NS_FIXED_PITCH != 0),
                    scripts: vec!["latin".into()],
                    languages: Vec::new(),
                    registries: Vec::new(),
                    color: false,
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
    fn tables() -> &'static Mutex<HashMap<u64, Table>> {
        static TABLES: OnceLock<Mutex<HashMap<u64, Table>>> = OnceLock::new();
        TABLES.get_or_init(Mutex::default)
    }

    pub fn open(id: u64, size: i32) -> Option<NhgFontMetrics> {
        let (handle, m) = ct::open(id, f64::from(size))?;
        let raw = RawFont {
            ascent: m.ascent,
            descent: m.descent,
            leading: m.leading,
            underline_position: m.underline_position,
            underline_thickness: m.underline_thickness,
            mono: m.mono,
            tall_family: m.tall_family,
        };
        let src = Source(handle);
        // the first page answers the space and the printable ASCII macfont_open measures, in one call each
        let mut table = Table::new(raw.mono, 0);
        table.fill(0, &src);
        let gl: Vec<Option<u32>> = (0x20..0x7f)
            .map(|c| table.char_glyph(c).flatten())
            .collect();
        let ids: Vec<u16> = gl.iter().map(|g| g.unwrap_or(0) as u16).collect();
        let mut b = vec![[0.0f64; 5]; ids.len()];
        ct::bounds_for(handle, &ids, &mut b);
        let advance = |i: usize| gl[i].map(|_| b[i][0]);
        let printable: Vec<Option<f64>> = (1..gl.len()).map(advance).collect();
        let mut out = font_metrics(&raw, size, advance(0), &printable);
        out.handle = handle;
        table.set_space_width(out.space_width);
        tables()
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(handle, table);
        Some(out)
    }

    pub fn table(handle: u64) -> Option<*const NhgFontTable> {
        let tables = tables().lock().unwrap_or_else(|p| p.into_inner());
        tables.get(&handle).map(Table::as_ptr)
    }

    pub fn fill(handle: u64, page: u32) -> Option<bool> {
        let mut tables = tables().lock().unwrap_or_else(|p| p.into_inner());
        let t = tables.get_mut(&handle)?;
        ct::STATS.fills.fetch_add(1, Relaxed);
        Some(t.fill(page, &Source(handle)))
    }

    pub fn run(handle: u64, glyphs: &[u16], padding: bool) -> Option<(Vec<f64>, bool)> {
        let integral = ct::integral(handle)?;
        let mut tables = tables().lock().unwrap_or_else(|p| p.into_inner());
        let t = tables.get_mut(&handle)?;
        let advances = t.advances(glyphs, &Source(handle));
        Some((
            run_offsets(&advances, t.mono(), t.space_width(), padding, integral),
            integral,
        ))
    }

    pub fn note_call() {
        ct::STATS.calls.fetch_add(1, Relaxed);
    }

    pub fn close(handle: u64) {
        tables()
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(&handle);
        ct::close(handle);
    }
}

/// Install the Core Text font source (macOS); false elsewhere.
pub fn start() -> bool {
    #[cfg(all(feature = "macos", target_os = "macos"))]
    {
        crate::fontspec::set_source(Box::new(coretext::CoreText));
        true
    }
    #[cfg(not(all(feature = "macos", target_os = "macos")))]
    false
}

/// Open face `id` at `size` pixels (`None`: no such face, or no Core Text).
pub fn open(id: u64, size: i32) -> Option<NhgFontMetrics> {
    #[cfg(all(feature = "macos", target_os = "macos"))]
    return coretext::open(id, size);
    #[cfg(not(all(feature = "macos", target_os = "macos")))]
    {
        let _ = (id, size);
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

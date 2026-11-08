//! Glyph tables: per open font, the answers C's font driver needs per glyph (`src/nhmacfont.c`'s `encode_char` and
//! `text_extents`), kept where C reads them without a call.
//!
//! A table is a fixed directory of page pointers: character pages (character → glyph id) and glyph pages (glyph id →
//! the rounded metrics of `struct font_metrics`).  C gets the table's address once, at `open`, and reads a page through
//! the directory; a null page is a miss, and `nhg_font_fill` fills that page with one call to the font's source (Core
//! Text, one batched call per page).
//!
//! **The invariant.**  Pages are written only inside `nhg_*` calls, which C makes holding Emacs's global lock; C reads
//! them on a Lisp thread holding that lock.  So a reader never sees a page being written.  Nothing moves while the font
//! is open: the directory is allocated once at its fixed size, and each page is its own allocation, written in full
//! before its pointer is stored and freed only when the table is dropped (at `close`).
//!
//! **Page size: 256 code points.**  One page is one Unicode block's worth (Basic Latin with Latin-1, Greek, a slice of
//! CJK), so one Core Text call answers what a line of one script needs; the directory stays 4,352 + 256 pointers
//! (36 KiB) per open font; a page costs 1 KiB (characters) or 5 KiB (glyph metrics).  Smaller pages mean a bigger
//! directory and more misses on the same text, larger ones fill thousands of CJK glyphs a buffer never shows.
//!
//! Platform-neutral and tested on Linux; the source on macOS is `font.rs`'s Core Text one.

use crate::font::{Grid, NhgGlyphMetrics, extents};
use std::ptr::NonNull;

/// Code points or glyph ids per page.
pub const NHG_FONT_PAGE: usize = 256;
/// Character pages: all of Unicode.
pub const NHG_FONT_CHAR_PAGES: usize = 0x11_0000 / NHG_FONT_PAGE;
/// Glyph pages: Core Text's glyph ids are 16 bits.
pub const NHG_FONT_GLYPH_PAGES: usize = 0x1_0000 / NHG_FONT_PAGE;
/// `nhg_font_fill`'s page names a glyph page when this bit is set, a character page when not.
pub const NHG_FONT_FILL_GLYPHS: u32 = 1 << 31;
/// A character page's entry for a character the font has no glyph for.
pub const NHG_GLYPH_NONE: u32 = u32::MAX;

/// Character → glyph id for one page (`NHG_GLYPH_NONE`: none).  Layout equals `NhgCharPage`.
#[repr(C)]
pub struct NhgCharPage {
    pub glyph: [u32; NHG_FONT_PAGE],
}

/// Glyph id → metrics for one page, the width rounded as `macfont_glyph_extents` rounds it.  Layout equals
/// `NhgGlyphPage`.
#[repr(C)]
pub struct NhgGlyphPage {
    pub metrics: [NhgGlyphMetrics; NHG_FONT_PAGE],
}

/// The directory C reads.  Layout equals `NhgFontTable`.
#[repr(C)]
pub struct NhgFontTable {
    pub chars: [*const NhgCharPage; NHG_FONT_CHAR_PAGES],
    pub glyphs: [*const NhgGlyphPage; NHG_FONT_GLYPH_PAGES],
}

/// What a fill asks the font for, a page at a time (Core Text on macOS; a fake in the tests).
pub trait GlyphSource {
    /// The glyph of each UTF-16 unit of `units` into `out` (0: none); a surrogate pair's glyph goes at its first unit.
    fn glyphs(&self, units: &[u16], out: &mut [u16]);
    /// Each glyph's advance and bounds into `out`: advance, left, right, top, bottom (pixels, y up).
    fn bounds(&self, glyphs: &[u16], out: &mut [[f64; 5]]);
}

/// The UTF-16 units of character page `page` and, for each of its code points, where its units start (`None` for a
/// surrogate code point, which has none).  Every page is all BMP or all beyond it, so a page is 256 or 512 units.
pub fn page_units(page: usize) -> (Vec<u16>, Vec<Option<usize>>) {
    let mut units = Vec::with_capacity(2 * NHG_FONT_PAGE);
    let mut at = Vec::with_capacity(NHG_FONT_PAGE);
    for i in 0..NHG_FONT_PAGE {
        match char::from_u32((page * NHG_FONT_PAGE + i) as u32) {
            Some(c) => {
                at.push(Some(units.len()));
                let mut b = [0u16; 2];
                units.extend_from_slice(c.encode_utf16(&mut b));
            }
            None => at.push(None),
        }
    }
    (units, at)
}

/// One open font's table.
pub struct Table {
    dir: NonNull<NhgFontTable>,
    /// Each glyph page's unrounded advances, for laying out runs (Rust only).
    advances: Vec<Option<Box<[f64; NHG_FONT_PAGE]>>>,
    /// What the glyphs' metrics are rounded on.
    grid: Grid,
}

// SAFETY: the table owns its directory and pages; they are reached only through `&self`/`&mut self` (and by C under
// the global lock, per the invariant above).
unsafe impl Send for Table {}

impl Table {
    /// An empty table of a font whose glyphs are rounded on `grid`.
    pub fn new(grid: Grid) -> Table {
        // SAFETY: all-zero is a valid `NhgFontTable`: every entry a null pointer.
        let dir: Box<NhgFontTable> = unsafe { Box::<NhgFontTable>::new_zeroed().assume_init() };
        Table {
            dir: NonNull::from(Box::leak(dir)),
            advances: (0..NHG_FONT_GLYPH_PAGES).map(|_| None).collect(),
            grid,
        }
    }

    /// Set the grid, whose space width is known only once the space's glyph is read; before any glyph page is filled.
    pub fn set_grid(&mut self, grid: Grid) {
        debug_assert!(self.advances.iter().all(Option::is_none));
        self.grid = grid;
    }

    pub fn grid(&self) -> Grid {
        self.grid
    }

    /// The directory's address, fixed while the table lives.
    pub fn as_ptr(&self) -> *const NhgFontTable {
        self.dir.as_ptr()
    }

    fn dir(&self) -> &NhgFontTable {
        // SAFETY: the directory is live while `self` is, and written only through `&mut self`.
        unsafe { self.dir.as_ref() }
    }

    /// Fill page `page` (`NHG_FONT_FILL_GLYPHS` set: a glyph page) from `src`, unless it is filled already.  False for
    /// a page past the directory.
    pub fn fill(&mut self, page: u32, src: &dyn GlyphSource) -> bool {
        if page & NHG_FONT_FILL_GLYPHS != 0 {
            let p = (page & !NHG_FONT_FILL_GLYPHS) as usize;
            if p >= NHG_FONT_GLYPH_PAGES {
                return false;
            }
            if self.dir().glyphs[p].is_null() {
                self.fill_glyphs(p, src);
            }
        } else {
            let p = page as usize;
            if p >= NHG_FONT_CHAR_PAGES {
                return false;
            }
            if self.dir().chars[p].is_null() {
                self.fill_chars(p, src);
            }
        }
        true
    }

    fn fill_chars(&mut self, p: usize, src: &dyn GlyphSource) {
        let mut page = Box::new(NhgCharPage {
            glyph: [NHG_GLYPH_NONE; NHG_FONT_PAGE],
        });
        let (units, at) = page_units(p);
        if !units.is_empty() {
            let mut out = vec![0u16; units.len()];
            src.glyphs(&units, &mut out);
            for (slot, at) in page.glyph.iter_mut().zip(at) {
                if let Some(g) = at.map(|i| out[i]).filter(|&g| g != 0) {
                    *slot = u32::from(g);
                }
            }
        }
        // SAFETY: the directory is live and only `&mut self` writes it; the page is complete before C can see it.
        unsafe { (*self.dir.as_ptr()).chars[p] = Box::into_raw(page) };
    }

    fn fill_glyphs(&mut self, p: usize, src: &dyn GlyphSource) {
        let glyphs: Vec<u16> = (0..NHG_FONT_PAGE)
            .map(|i| (p * NHG_FONT_PAGE + i) as u16)
            .collect();
        let mut b = vec![[0.0f64; 5]; NHG_FONT_PAGE];
        src.bounds(&glyphs, &mut b);
        let mut page = Box::new(NhgGlyphPage {
            metrics: [NhgGlyphMetrics::default(); NHG_FONT_PAGE],
        });
        let mut adv = Box::new([0.0f64; NHG_FONT_PAGE]);
        for (i, [a, l, r, t, bot]) in b.into_iter().enumerate() {
            page.metrics[i] = extents(&self.grid, a, (l, r, t, bot));
            adv[i] = a;
        }
        self.advances[p] = Some(adv);
        // SAFETY: as in `fill_chars`.
        unsafe { (*self.dir.as_ptr()).glyphs[p] = Box::into_raw(page) };
    }

    /// Character `c`'s glyph as C reads it: `None` on a miss (its page not filled), `Some(None)` when the font has no
    /// glyph for it.
    pub fn char_glyph(&self, c: u32) -> Option<Option<u32>> {
        let p = *self.dir().chars.get(c as usize / NHG_FONT_PAGE)?;
        // SAFETY: a non-null page pointer is a live, complete page (`fill_chars`).
        let g = unsafe { p.as_ref() }?.glyph[c as usize % NHG_FONT_PAGE];
        Some((g != NHG_GLYPH_NONE).then_some(g))
    }

    /// Glyph `g`'s metrics as C reads them: `None` on a miss.
    pub fn metrics(&self, g: u32) -> Option<NhgGlyphMetrics> {
        let p = *self.dir().glyphs.get(g as usize / NHG_FONT_PAGE)?;
        // SAFETY: as in `char_glyph`.
        Some(unsafe { p.as_ref() }?.metrics[g as usize % NHG_FONT_PAGE])
    }

    /// The unrounded advances of `glyphs`, their pages filled from `src` where missing.
    pub fn advances(&mut self, glyphs: &[u16], src: &dyn GlyphSource) -> Vec<f64> {
        glyphs
            .iter()
            .map(|&g| {
                let p = usize::from(g) / NHG_FONT_PAGE;
                if self.advances[p].is_none() {
                    self.fill(NHG_FONT_FILL_GLYPHS | p as u32, src);
                }
                self.advances[p]
                    .as_ref()
                    .map_or(0.0, |a| a[usize::from(g) % NHG_FONT_PAGE])
            })
            .collect()
    }
}

impl Drop for Table {
    fn drop(&mut self) {
        // SAFETY: the directory and every non-null page were made by `Box` here and are freed once, now.
        unsafe {
            let dir = Box::from_raw(self.dir.as_ptr());
            for &p in dir.chars.iter().filter(|p| !p.is_null()) {
                drop(Box::from_raw(p.cast_mut()));
            }
            for &p in dir.glyphs.iter().filter(|p| !p.is_null()) {
                drop(Box::from_raw(p.cast_mut()));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::font::Spacing;
    use std::cell::RefCell;

    fn grid(spacing: Spacing, space_width: i32) -> Grid {
        Grid {
            spacing,
            space_width,
            pixel_size: 12,
            size: 12.0,
            ..Grid::default()
        }
    }

    /// A font with a glyph for every character but U+0000 and U+0378 (unassigned): glyph `c % 60000 + 1`.  Glyph `g`'s
    /// advance is 7.2 for an odd `g`, 14.9 for an even one.  It records every call.
    #[derive(Default)]
    struct Fake {
        glyph_calls: RefCell<Vec<Vec<u16>>>,
        bound_calls: RefCell<usize>,
    }

    fn fake_glyph(c: u32) -> u16 {
        if c == 0 || c == 0x378 {
            0
        } else {
            (c % 60000 + 1) as u16
        }
    }

    impl GlyphSource for Fake {
        fn glyphs(&self, units: &[u16], out: &mut [u16]) {
            self.glyph_calls.borrow_mut().push(units.to_vec());
            let mut i = 0;
            while i < units.len() {
                let u = u32::from(units[i]);
                if (0xD800..0xDC00).contains(&u) && i + 1 < units.len() {
                    let c = 0x10000 + ((u - 0xD800) << 10) + (u32::from(units[i + 1]) - 0xDC00);
                    out[i] = fake_glyph(c);
                    out[i + 1] = 0;
                    i += 2;
                } else {
                    out[i] = fake_glyph(u);
                    i += 1;
                }
            }
        }

        fn bounds(&self, glyphs: &[u16], out: &mut [[f64; 5]]) {
            *self.bound_calls.borrow_mut() += 1;
            for (g, o) in glyphs.iter().zip(out) {
                let a = if g % 2 == 1 { 7.2 } else { 14.9 };
                *o = [a, 0.4, a - 0.8, 8.1, -2.2];
            }
        }
    }

    #[test]
    fn a_page_fill_answers_the_whole_page_in_one_call() {
        let src = Fake::default();
        let mut t = Table::new(grid(Spacing::Proportional, 7));
        assert!(t.fill(0, &src));
        assert_eq!(t.char_glyph(u32::from('A')), Some(Some(66)));
        assert_eq!(t.char_glyph(0xff), Some(Some(256)));
        assert_eq!(t.char_glyph(0), Some(None));
        assert_eq!(src.glyph_calls.borrow().len(), 1);
        assert_eq!(src.glyph_calls.borrow()[0].len(), 256);
        // filled already: no second call
        assert!(t.fill(0, &src));
        assert_eq!(src.glyph_calls.borrow().len(), 1);
    }

    #[test]
    fn a_miss_is_a_page_not_filled() {
        let src = Fake::default();
        let mut t = Table::new(grid(Spacing::Proportional, 7));
        assert_eq!(t.char_glyph(0x3b1), None);
        assert_eq!(t.metrics(66), None);
        t.fill(3, &src);
        assert_eq!(t.char_glyph(0x3b1), Some(Some(0x3b2)));
        assert_eq!(t.char_glyph(0x378), Some(None));
        // the next page is still a miss
        assert_eq!(t.char_glyph(0x400), None);
        t.fill(NHG_FONT_FILL_GLYPHS, &src);
        assert_eq!(*src.bound_calls.borrow(), 1);
        assert_eq!(
            t.metrics(66),
            Some(NhgGlyphMetrics {
                lbearing: 0,
                rbearing: 15,
                width: 15,
                ascent: 9,
                descent: 3
            })
        );
        assert_eq!(t.metrics(256), None);
    }

    #[test]
    fn the_directory_and_its_pages_never_move() {
        let src = Fake::default();
        let mut t = Table::new(grid(Spacing::Proportional, 7));
        let dir = t.as_ptr();
        t.fill(0, &src);
        t.fill(NHG_FONT_FILL_GLYPHS, &src);
        // SAFETY: the table is live.
        let (c0, g0) = unsafe { ((*dir).chars[0], (*dir).glyphs[0]) };
        for p in (0..NHG_FONT_CHAR_PAGES as u32).step_by(7) {
            assert!(t.fill(p, &src));
        }
        for p in 0..NHG_FONT_GLYPH_PAGES as u32 {
            assert!(t.fill(NHG_FONT_FILL_GLYPHS | p, &src));
        }
        assert_eq!(t.as_ptr(), dir);
        // SAFETY: as above.
        unsafe {
            assert_eq!((*dir).chars[0], c0);
            assert_eq!((*dir).glyphs[0], g0);
            assert_eq!((*(*dir).chars[0]).glyph[usize::from(b'A')], 66);
        }
    }

    #[test]
    fn surrogate_and_plane_boundary_pages() {
        let src = Fake::default();
        let mut t = Table::new(grid(Spacing::Proportional, 7));
        // surrogates have no characters: answered without asking the font
        assert!(t.fill(0xD8, &src));
        assert!(t.fill(0xDF, &src));
        assert_eq!(src.glyph_calls.borrow().len(), 0);
        assert_eq!(t.char_glyph(0xD800), Some(None));
        assert_eq!(t.char_glyph(0xDFFF), Some(None));
        // the BMP's last page and the next plane's first: one unit per character, then a surrogate pair each
        t.fill(0xFF, &src);
        t.fill(0x100, &src);
        assert_eq!(src.glyph_calls.borrow()[0].len(), 256);
        assert_eq!(src.glyph_calls.borrow()[1].len(), 512);
        assert_eq!(t.char_glyph(0xFFFD), Some(Some(fake_glyph(0xFFFD).into())));
        assert_eq!(
            t.char_glyph(0x10000),
            Some(Some(fake_glyph(0x10000).into()))
        );
        assert_eq!(
            t.char_glyph(0x100FF),
            Some(Some(fake_glyph(0x100FF).into()))
        );
        // the last page of Unicode, then nothing past it
        assert!(t.fill(0x10FF, &src));
        assert_eq!(
            t.char_glyph(0x10FFFF),
            Some(Some(fake_glyph(0x10FFFF).into()))
        );
        assert!(!t.fill(NHG_FONT_CHAR_PAGES as u32, &src));
        assert_eq!(t.char_glyph(0x110000), None);
        assert!(!t.fill(NHG_FONT_FILL_GLYPHS | NHG_FONT_GLYPH_PAGES as u32, &src));
    }

    #[test]
    fn glyph_pages_round_on_the_fonts_grid() {
        let src = Fake::default();
        // monospaced, 7 wide: a 14.9 advance is two cells (14), 7.2 one (7)
        let mut t = Table::new(grid(Spacing::Mono, 0));
        t.set_grid(grid(Spacing::Mono, 7));
        t.fill(NHG_FONT_FILL_GLYPHS, &src);
        assert_eq!(t.metrics(2).map(|m| m.width), Some(14));
        assert_eq!(t.metrics(3).map(|m| m.width), Some(7));
        // the runs' advances stay unrounded, and a missing page is filled for them
        assert_eq!(t.advances(&[2, 3, 300], &src), vec![14.9, 7.2, 14.9]);
        assert_eq!(*src.bound_calls.borrow(), 2);
    }
}

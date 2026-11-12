//! Unicode variation sequences from a font's `cmap` table, as macfont reads them (`mac_font_copy_uvs_table` and
//! `mac_font_get_glyphs_for_variants`): the Unicode-platform subtable of encoding 5, format 14.  For a base character
//! and each of the 256 selectors Emacs asks about (VS1–VS16, then VS17–VS256), the sequence is in the font's Default
//! UVS table (drawn with the base character's own glyph), maps to a glyph in its Non-Default UVS table, or is not in
//! the font.  A subtable whose counts or offsets run past it is refused whole, as macfont refuses it.  Platform-free;
//! tested on Linux.

/// The selector Emacs's `variations[i]` stands for (`macfont_variation_glyphs`).
pub fn selector(i: usize) -> u32 {
    if i < 16 {
        0xFE00 + i as u32
    } else {
        0xE0100 + (i as u32 - 16)
    }
}

/// One selector's answer for a base character.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Variant {
    /// In the Default UVS table: the base character's own glyph.
    Default,
    /// In the Non-Default UVS table: this glyph.
    Glyph(u16),
}

fn u16_at(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from(u16::from_be_bytes(
        b.get(at..at + 2)?.try_into().ok()?,
    )))
}

fn u24_at(b: &[u8], at: usize) -> Option<u32> {
    let s = b.get(at..at + 3)?;
    Some(u32::from(s[0]) << 16 | u32::from(s[1]) << 8 | u32::from(s[2]))
}

fn u32_at(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

const UVS_HEADER: usize = 10;
const RECORD: usize = 11;
const TABLE_HEADER: usize = 4;
const RANGE: usize = 4;
const MAPPING: usize = 5;

/// The format-14 subtable of the `cmap` table `cmap`, checked whole (`mac_font_copy_uvs_table`); `None` when there is
/// none or it does not hold together.
pub fn subtable(cmap: &[u8]) -> Option<&[u8]> {
    let ntables = u16_at(cmap, 2)? as usize;
    if ntables > cmap.len().checked_sub(4)? / 8 {
        return None;
    }
    let offset = (0..ntables).find_map(|i| {
        let rec = 4 + i * 8;
        (u16_at(cmap, rec)? == 0 && u16_at(cmap, rec + 2)? == 5)
            .then(|| u32_at(cmap, rec + 4))
            .flatten()
    })? as usize;
    if offset > cmap.len() || UVS_HEADER > cmap.len() - offset {
        return None;
    }
    let uvs = &cmap[offset..];
    let len = u32_at(uvs, 2)? as usize;
    if len > uvs.len() || UVS_HEADER > len || u16_at(uvs, 0)? != 14 {
        return None;
    }
    let uvs = &uvs[..len];
    let nrecords = u32_at(uvs, 6)? as usize;
    if nrecords > (len - UVS_HEADER) / RECORD {
        return None;
    }
    for r in 0..nrecords {
        let rec = UVS_HEADER + r * RECORD;
        for (field, item) in [(3, RANGE), (7, MAPPING)] {
            let off = u32_at(uvs, rec + field)? as usize;
            if off != 0 {
                if off > len || TABLE_HEADER > len - off {
                    return None;
                }
                if u32_at(uvs, off)? as usize > (len - off - TABLE_HEADER) / item {
                    return None;
                }
            }
        }
    }
    Some(uvs)
}

/// Each of the 256 selectors' answer for base character `c` in the checked subtable `uvs`
/// (`mac_font_get_glyphs_for_variants`, the selectors in `selector`'s order).
pub fn variants(uvs: &[u8], c: u32) -> [Option<Variant>; 256] {
    let mut out = [None; 256];
    let nrecords = u32_at(uvs, 6).unwrap_or(0) as usize;
    let record = |r: usize| {
        let rec = UVS_HEADER + r * RECORD;
        Some((
            u24_at(uvs, rec)?,
            u32_at(uvs, rec + 3)? as usize,
            u32_at(uvs, rec + 7)? as usize,
        ))
    };
    let (mut i, mut r) = (0, 0);
    while i < 256 && r < nrecords {
        let Some((vs, default, non_default)) = record(r) else {
            break;
        };
        let want = selector(i);
        if want < vs {
            i += 1;
            continue;
        }
        if want > vs {
            r += 1;
            continue;
        }
        if default != 0 && in_default(uvs, default, c) {
            out[i] = Some(Variant::Default);
        } else if non_default != 0 {
            out[i] = mapped(uvs, non_default, c).map(Variant::Glyph);
        }
        i += 1;
        r += 1;
    }
    out
}

/// Whether `c` is in a Default UVS table's ranges (binary search on the starts, as macfont does).
fn in_default(uvs: &[u8], at: usize, c: u32) -> bool {
    let n = u32_at(uvs, at).unwrap_or(0) as usize;
    let start = |k: usize| u24_at(uvs, at + TABLE_HEADER + k * RANGE).unwrap_or(u32::MAX);
    let (mut lo, mut hi) = (0, n);
    while lo < hi {
        let mid = (lo + hi) / 2;
        if c < start(mid) {
            hi = mid;
        } else {
            lo = mid + 1;
        }
    }
    hi > 0 && {
        let k = at + TABLE_HEADER + (hi - 1) * RANGE;
        let extra = uvs.get(k + 3).copied().unwrap_or(0);
        c <= start(hi - 1) + u32::from(extra)
    }
}

/// The glyph a Non-Default UVS table maps `c` to.
fn mapped(uvs: &[u8], at: usize, c: u32) -> Option<u16> {
    let n = u32_at(uvs, at)? as usize;
    let value = |k: usize| u24_at(uvs, at + TABLE_HEADER + k * MAPPING).unwrap_or(u32::MAX);
    let (mut lo, mut hi) = (0, n);
    while lo < hi {
        let mid = (lo + hi) / 2;
        if c < value(mid) {
            hi = mid;
        } else {
            lo = mid + 1;
        }
    }
    (hi > 0 && value(hi - 1) == c)
        .then(|| u16_at(uvs, at + TABLE_HEADER + (hi - 1) * MAPPING + 3))
        .flatten()
        .map(|g| g as u16)
}

/// `macfont_variation_glyphs`: into `out`, each selector's glyph for `c` (0: not in the font), a default sequence (or
/// one mapped to glyph 0) as `base`, the base character's glyph; how many are in the font.
pub fn glyphs(uvs: &[u8], c: u32, base: u32, out: &mut [u32; 256]) -> u32 {
    let mut n = 0;
    for (o, v) in out.iter_mut().zip(variants(uvs, c)) {
        *o = match v {
            None => 0,
            // glyph 0 reads as the default too, as macfont reads it
            Some(Variant::Default | Variant::Glyph(0)) => base,
            Some(Variant::Glyph(g)) => u32::from(g),
        };
        n += u32::from(v.is_some());
    }
    n
}

#[cfg(test)]
mod tests {
    use super::*;

    fn be24(v: u32) -> [u8; 3] {
        [(v >> 16) as u8, (v >> 8) as u8, v as u8]
    }

    /// A `cmap` with a format-4 stub (platform 3) and a format-14 subtable: VS16 (FE0F) has U+2764 by default;
    /// VS15 (FE0E) maps U+2764 to glyph 7; VS17 (E0100) maps U+845B to 300 and U+8FBB to 301, and has U+9089 to U+908B
    /// by default.
    fn cmap() -> Vec<u8> {
        let mut uvs = Vec::new();
        let records: [(u32, bool, bool); 3] = [
            (0xFE0E, false, true),
            (0xFE0F, true, false),
            (0xE0100, true, true),
        ];
        let tables_at = UVS_HEADER + records.len() * RECORD;
        // the tables, after the records
        let mut tables = Vec::new();
        let mut offs = Vec::new();
        for &(vs, d, nd) in &records {
            let mut d_off = 0;
            let mut nd_off = 0;
            if d {
                d_off = tables_at + tables.len();
                let ranges: &[(u32, u8)] = if vs == 0xFE0F {
                    &[(0x2764, 0)]
                } else {
                    &[(0x9089, 2)]
                };
                tables.extend_from_slice(&(ranges.len() as u32).to_be_bytes());
                for &(s, n) in ranges {
                    tables.extend_from_slice(&be24(s));
                    tables.push(n);
                }
            }
            if nd {
                nd_off = tables_at + tables.len();
                let maps: &[(u32, u16)] = if vs == 0xFE0E {
                    &[(0x2764, 7)]
                } else {
                    &[(0x845B, 300), (0x8FBB, 301)]
                };
                tables.extend_from_slice(&(maps.len() as u32).to_be_bytes());
                for &(u, g) in maps {
                    tables.extend_from_slice(&be24(u));
                    tables.extend_from_slice(&g.to_be_bytes());
                }
            }
            offs.push((vs, d_off as u32, nd_off as u32));
        }
        uvs.extend_from_slice(&14u16.to_be_bytes());
        uvs.extend_from_slice(&((tables_at + tables.len()) as u32).to_be_bytes());
        uvs.extend_from_slice(&(records.len() as u32).to_be_bytes());
        for (vs, d, nd) in offs {
            uvs.extend_from_slice(&be24(vs));
            uvs.extend_from_slice(&d.to_be_bytes());
            uvs.extend_from_slice(&nd.to_be_bytes());
        }
        uvs.extend_from_slice(&tables);
        let mut cmap = vec![0, 0, 0, 2];
        let stub = [0u8, 4, 0, 6, 0, 0];
        let stub_at = 4 + 2 * 8;
        let uvs_at = stub_at + stub.len();
        for (p, e, o) in [(3u16, 1u16, stub_at), (0, 5, uvs_at)] {
            cmap.extend_from_slice(&p.to_be_bytes());
            cmap.extend_from_slice(&e.to_be_bytes());
            cmap.extend_from_slice(&(o as u32).to_be_bytes());
        }
        cmap.extend_from_slice(&stub);
        cmap.extend_from_slice(&uvs);
        cmap
    }

    #[test]
    fn selectors_in_emacs_order() {
        assert_eq!(selector(0), 0xFE00);
        assert_eq!(selector(15), 0xFE0F);
        assert_eq!(selector(16), 0xE0100);
        assert_eq!(selector(255), 0xE01EF);
    }

    #[test]
    fn emoji_and_ideographic_sequences() {
        let c = cmap();
        let uvs = subtable(&c).expect("a format-14 subtable");
        let heart = variants(uvs, 0x2764);
        assert_eq!(heart[14], Some(Variant::Glyph(7)), "VS15");
        assert_eq!(heart[15], Some(Variant::Default), "VS16");
        assert_eq!(heart.iter().filter(|v| v.is_some()).count(), 2);
        let kuzu = variants(uvs, 0x845B);
        assert_eq!(kuzu[16], Some(Variant::Glyph(300)), "VS17");
        assert_eq!(variants(uvs, 0x8FBB)[16], Some(Variant::Glyph(301)));
        // a default range: start and two more
        for c in 0x9089..=0x908B {
            assert_eq!(variants(uvs, c)[16], Some(Variant::Default), "{c:x}");
        }
        assert_eq!(variants(uvs, 0x908C)[16], None);
        assert!(variants(uvs, 0x41).iter().all(Option::is_none));
        let mut out = [0u32; 256];
        assert_eq!(glyphs(uvs, 0x2764, 42, &mut out), 2);
        assert_eq!((out[14], out[15], out[0]), (7, 42, 0));
    }

    #[test]
    fn a_broken_or_missing_subtable_is_refused() {
        let c = cmap();
        // no Unicode encoding 5
        let mut other = c.clone();
        other[4 + 8 + 3] = 4;
        assert!(subtable(&other).is_none());
        // the subtable's length past the table
        let uvs_at = c.len() - subtable(&c).unwrap().len();
        let mut long = c.clone();
        long[uvs_at + 2..uvs_at + 6].copy_from_slice(&(10_000u32).to_be_bytes());
        assert!(subtable(&long).is_none());
        // a mapping count past the subtable
        let mut count = c.clone();
        let first_nd = u32_at(subtable(&c).unwrap(), UVS_HEADER + 7).unwrap() as usize;
        count[uvs_at + first_nd..uvs_at + first_nd + 4].copy_from_slice(&(1000u32).to_be_bytes());
        assert!(subtable(&count).is_none());
        // too short to hold its header
        assert!(subtable(&c[..6]).is_none());
        assert!(subtable(&[]).is_none());
    }
}

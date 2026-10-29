//! PNG through the `png` crate, with libpng's read transformations as a host built on libpng asks for them:
//! `png_set_strip_16`, `png_set_expand`, `png_set_gray_to_rgb`, and `png_set_background` at gamma 1.0 over the
//! given background unless the image's transparency is all-or-nothing.
//!
//! The crate decodes the samples untransformed; everything libpng would do to them is done here, sample by sample,
//! so that the pixels are libpng's: palette and low-depth gray expansion, `tRNS` keys compared as libpng compares
//! them, composition at the file's bit depth with `png_composite`'s rounding, then the low byte of 16-bit samples
//! dropped. The chunks whose meaning libpng decides (`tRNS`, `bKGD`) are read from the file with libpng's rules.
//!
//! One case is not decoded here: an image whose alpha would be composited (not all-or-nothing) and that carries
//! `gAMA`, `sRGB`, `iCCP` or `cICP` before its image data. libpng composites such an image in linear light, through
//! gamma tables whose details depend on its version, so the decoder answers [`Error::Host`] and the host decodes it.

use super::composite::{over8, over16};
use super::{CBuf, Error, Image, Params};
use png::{ColorType, DecodeOptions, Transformations};
use std::io::Cursor;

const SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];

/// The chunks libpng reads before the image data that decide the pixels, by libpng's rules.
#[derive(Debug, Default)]
struct Chunks {
    /// `tRNS` as libpng keeps it: palette alphas, or one 16-bit key per channel (gray: 1, RGB: 3).
    trns: Option<Vec<u16>>,
    /// `bKGD` as `png_get_bKGD` returns it: red, green, blue.
    bkgd: Option<[u16; 3]>,
    /// A chunk that may set a gamma other than 1.0.
    gamma: bool,
}

fn crc32(bytes: &[u8]) -> u32 {
    let mut c = !0u32;
    for &b in bytes {
        c ^= u32::from(b);
        for _ in 0..8 {
            c = if c & 1 != 0 {
                0xedb8_8320 ^ (c >> 1)
            } else {
                c >> 1
            };
        }
    }
    !c
}

fn be16(b: &[u8]) -> u16 {
    u16::from_be_bytes([b[0], b[1]])
}

/// Walk the chunks before the first `IDAT`. A malformed chunk ends the walk: the decoder reports it.
fn scan(data: &[u8], color: ColorType, depth: u8, palette_len: usize) -> Chunks {
    let mut c = Chunks::default();
    let mut have_plte = false;
    let mut pos = 8usize;
    while pos + 8 <= data.len() {
        let len =
            u32::from_be_bytes([data[pos], data[pos + 1], data[pos + 2], data[pos + 3]]) as usize;
        let kind: [u8; 4] = [data[pos + 4], data[pos + 5], data[pos + 6], data[pos + 7]];
        if len > 0x7fff_ffff || data.len() - pos - 8 < len || data.len() - pos - 8 - len < 4 {
            break;
        }
        let body = &data[pos + 8..pos + 8 + len];
        let crc_at = pos + 8 + len;
        let crc_ok = crc32(&data[pos + 4..crc_at])
            == u32::from_be_bytes([
                data[crc_at],
                data[crc_at + 1],
                data[crc_at + 2],
                data[crc_at + 3],
            ]);
        pos = crc_at + 4;
        match &kind {
            b"IDAT" => break,
            b"PLTE" => have_plte = true,
            b"gAMA" | b"sRGB" | b"iCCP" | b"cICP" => c.gamma = true,
            // png_handle_tRNS: the first valid one; an ancillary chunk with a bad CRC is dropped
            b"tRNS" if crc_ok && c.trns.is_none() => {
                c.trns = match color {
                    ColorType::Grayscale if len == 2 => Some(vec![be16(body)]),
                    ColorType::Rgb if len == 6 => {
                        Some(vec![be16(body), be16(&body[2..]), be16(&body[4..])])
                    }
                    ColorType::Indexed if have_plte && len > 0 && len <= palette_len.min(256) => {
                        Some(body.iter().map(|&a| u16::from(a)).collect())
                    }
                    _ => None,
                };
            }
            // png_handle_bKGD
            b"bKGD" if crc_ok && c.bkgd.is_none() => {
                c.bkgd = match color {
                    // the index, which the caller resolves through the palette
                    ColorType::Indexed if have_plte && len == 1 => {
                        (usize::from(body[0]) < palette_len).then_some([u16::from(body[0]); 3])
                    }
                    ColorType::Grayscale | ColorType::GrayscaleAlpha if len == 2 => {
                        let g = be16(body);
                        (depth > 8 || (body[0] == 0 && u32::from(body[1]) < (1u32 << depth)))
                            .then_some([g; 3])
                    }
                    ColorType::Rgb | ColorType::Rgba if len == 6 => (depth > 8
                        || (body[0] == 0 && body[2] == 0 && body[4] == 0))
                        .then(|| [be16(body), be16(&body[2..]), be16(&body[4..])]),
                    _ => None,
                };
            }
            _ => {}
        }
    }
    c
}

/// The Adam7 passes: (x offset, x step, y offset, y step).
const ADAM7: [(u32, u32, u32, u32); 7] = [
    (0, 8, 0, 8),
    (4, 8, 0, 8),
    (0, 4, 4, 8),
    (2, 4, 0, 4),
    (0, 2, 2, 4),
    (1, 2, 0, 2),
    (0, 1, 1, 2),
];

/// The rows the crate hands out, in order: (y, first x, x step, samples) of each.
fn rows(width: u32, height: u32, interlaced: bool) -> impl Iterator<Item = (u32, u32, u32, u32)> {
    let passes: &[(u32, u32, u32, u32)] = if interlaced { &ADAM7 } else { &[(0, 1, 0, 1)] };
    passes.iter().flat_map(move |&(x0, dx, y0, dy)| {
        let samples = width.saturating_sub(x0).div_ceil(dx);
        let lines = if samples == 0 {
            0
        } else {
            height.saturating_sub(y0).div_ceil(dy)
        };
        (0..lines).map(move |l| (y0 + l * dy, x0, dx, samples))
    })
}

/// How one sample of the file becomes a pixel.
struct Convert<'a> {
    color: ColorType,
    depth: u8,
    palette: &'a [u8],
    trns: Option<&'a [u16]>,
    /// Composite over this background (16 bits per channel for 16-bit files, else 8).
    compose: Option<[u16; 3]>,
}

impl Convert<'_> {
    fn sample(&self, row: &[u8], i: usize) -> u16 {
        match self.depth {
            16 => be16(&row[2 * i..]),
            8 => u16::from(row[i]),
            d => {
                let bit = i * usize::from(d);
                let shift = 8 - u32::from(d) - (bit % 8) as u32;
                u16::from((row[bit / 8] >> shift) & ((1u8 << d) - 1))
            }
        }
    }

    /// The pixel `j` of `row`: 0x00RRGGBB and its alpha's high byte (255 where the image has none).
    fn pixel(&self, row: &[u8], j: usize) -> (u32, u8) {
        let s = |k: usize| self.sample(row, k);
        let wide = self.depth == 16;
        let hi = |v: u16| if wide { (v >> 8) as u8 } else { v as u8 };
        let rgb = |r: u8, g: u8, b: u8| (u32::from(r) << 16) | (u32::from(g) << 8) | u32::from(b);
        let max: u16 = if wide { 0xffff } else { 0xff };
        let comp = |v: u16, a: u16, k: usize| -> u8 {
            match self.compose {
                Some(bg) if wide => hi(over16(v, a, bg[k])),
                Some(bg) => over8(v as u8, a as u8, bg[k] as u8),
                None => hi(v),
            }
        };
        match self.color {
            ColorType::Grayscale => {
                let v = s(j);
                let (g, a) = match self.depth {
                    16 | 8 => {
                        let key = self.trns.map(|t| if wide { t[0] } else { t[0] & 0xff });
                        (hi(v), if key == Some(v) { 0 } else { 255 })
                    }
                    d => {
                        // png_do_expand: low-depth gray and its key scale to 8 bits
                        let scale = 255 / ((1u16 << d) - 1);
                        let key = self.trns.map(|t| (t[0] & ((1 << d) - 1)) * scale);
                        let v8 = v * scale;
                        (v8 as u8, if key == Some(v8) { 0 } else { 255 })
                    }
                };
                (rgb(g, g, g), a)
            }
            ColorType::GrayscaleAlpha => {
                let (v, a) = (s(2 * j), s(2 * j + 1));
                (rgb(comp(v, a, 0), comp(v, a, 1), comp(v, a, 2)), hi(a))
            }
            ColorType::Rgb => {
                let (r, g, b) = (s(3 * j), s(3 * j + 1), s(3 * j + 2));
                let a = match self.trns {
                    Some(t) if wide && t[0] == r && t[1] == g && t[2] == b => 0,
                    Some(t)
                        if !wide
                            && (t[0] & 0xff) == r
                            && (t[1] & 0xff) == g
                            && (t[2] & 0xff) == b =>
                    {
                        0
                    }
                    _ => 255,
                };
                (rgb(hi(r), hi(g), hi(b)), a)
            }
            ColorType::Rgba => {
                let (r, g, b, a) = (s(4 * j), s(4 * j + 1), s(4 * j + 2), s(4 * j + 3));
                (rgb(comp(r, a, 0), comp(g, a, 1), comp(b, a, 2)), hi(a))
            }
            ColorType::Indexed => {
                let i = usize::from(s(j));
                // libpng's palette has 256 entries, black past the file's
                let c = |k: usize| self.palette.get(3 * i + k).copied().map_or(0, u16::from);
                let a = self.trns.and_then(|t| t.get(i)).copied().unwrap_or(max);
                (
                    rgb(comp(c(0), a, 0), comp(c(1), a, 1), comp(c(2), a, 2)),
                    a as u8,
                )
            }
        }
    }
}

fn lib(e: png::DecodingError) -> Error {
    Error::Decode(e.to_string())
}

pub fn decode(data: &[u8], p: &Params) -> Result<Image, Error> {
    if data.len() < 8 || data[..8] != SIGNATURE {
        return Err(Error::NotImage);
    }
    let mut opts = DecodeOptions::default();
    // the decoder reads neither: text is metadata, and the profile only decides the gamma (see the module)
    opts.set_ignore_text_chunk(true);
    opts.set_ignore_iccp_chunk(true);
    let mut dec = png::Decoder::new_with_options(Cursor::new(data), opts);
    dec.set_transformations(Transformations::IDENTITY);
    let mut reader = dec.read_info().map_err(lib)?;
    let info = reader.info();
    let (w, h) = (info.width, info.height);
    super::size_ok(p, i64::from(w), i64::from(h))?;
    let (color, depth, interlaced) = (info.color_type, info.bit_depth as u8, info.interlaced);
    let palette: Vec<u8> = info.palette.as_deref().unwrap_or(&[]).to_vec();
    let chunks = scan(data, color, depth, palette.len() / 3);

    let trns = chunks.trns.as_deref();
    let has_alpha = matches!(color, ColorType::GrayscaleAlpha | ColorType::Rgba) || trns.is_some();
    // all-or-nothing transparency: palette alphas of 0 and 255 only, or a key
    let transparent = match trns {
        None => false,
        Some(t) if color == ColorType::Indexed => t.iter().all(|&a| a == 0 || a == 255),
        Some(_) => true,
    };
    let compose = has_alpha && !transparent && p.bg_set != 0;
    if compose && chunks.gamma {
        return Err(Error::Host);
    }
    // The C loader passes the 16-bit colour unshifted for a 16-bit file; libpng, which strips after composing,
    // then widens the background itself (`png_init_read_transformations`: `background.red * 257`, kept in 16 bits,
    // so it wraps) before composing at 16 bits.
    let bg = |v: u16| {
        if depth == 16 {
            v.wrapping_mul(257)
        } else {
            v >> 8
        }
    };
    let conv = Convert {
        color,
        depth,
        palette: &palette,
        trns,
        compose: compose.then_some([bg(p.bg_r), bg(p.bg_g), bg(p.bg_b)]),
    };

    let n = w as usize * h as usize;
    let mut pixels = CBuf::<u32>::zeroed(n)?;
    let mut mask = if has_alpha && transparent {
        Some(CBuf::<u8>::zeroed(n)?)
    } else {
        None
    };
    {
        let px = pixels.as_mut_slice();
        let mut mk = mask.as_mut().map(|m| m.as_mut_slice());
        let bits = usize::from(depth) * color.samples();
        for (y, x0, dx, samples) in rows(w, h, interlaced) {
            let row = reader
                .next_row()
                .map_err(lib)?
                .ok_or_else(|| Error::Decode("Not enough image data".into()))?;
            let data = row.data();
            if data.len() < (samples as usize * bits).div_ceil(8) {
                return Err(Error::Decode("Row length mismatch".into()));
            }
            let base = y as usize * w as usize;
            for j in 0..samples as usize {
                let at = base + (x0 + j as u32 * dx) as usize;
                let (v, a) = conv.pixel(data, j);
                px[at] = v;
                if let Some(m) = mk.as_deref_mut() {
                    m[at] = if a > 0 { 255 } else { 0 };
                }
            }
        }
    }
    // png_read_end: the chunks after the image data must read too
    reader.finish().map_err(lib)?;

    let bkgd = chunks.bkgd.map(|b| {
        if color == ColorType::Indexed {
            let i = usize::from(b[0]);
            let c = |k: usize| palette.get(3 * i + k).copied().map_or(0, u16::from);
            [c(0), c(1), c(2)]
        } else {
            b
        }
    });
    Ok(Image {
        mask,
        bkgd,
        ..Image::still(pixels, w as i32, h as i32)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use png::{BitDepth, Encoder};

    fn params(bg: Option<[u16; 3]>) -> Params {
        let mut p = Params::default();
        if let Some(bg) = bg {
            p.bg_set = 1;
            [p.bg_r, p.bg_g, p.bg_b] = bg;
        }
        p
    }

    /// Encode a W×H image of COLOR and DEPTH; EXTRA sets chunks through the encoder.
    fn encode(
        w: u32,
        h: u32,
        color: ColorType,
        depth: BitDepth,
        data: &[u8],
        extra: impl FnOnce(&mut Encoder<'_, &mut Vec<u8>>),
    ) -> Vec<u8> {
        encode_with(w, h, color, depth, data, extra, &[])
    }

    /// The same, with raw CHUNKS written before the image data.
    fn encode_with(
        w: u32,
        h: u32,
        color: ColorType,
        depth: BitDepth,
        data: &[u8],
        extra: impl FnOnce(&mut Encoder<'_, &mut Vec<u8>>),
        chunks: &[(png::chunk::ChunkType, &[u8])],
    ) -> Vec<u8> {
        let mut out = Vec::new();
        {
            let mut e = Encoder::new(&mut out, w, h);
            e.set_color(color);
            e.set_depth(depth);
            extra(&mut e);
            let mut wr = e.write_header().unwrap();
            for (kind, body) in chunks {
                wr.write_chunk(*kind, body).unwrap();
            }
            wr.write_image_data(data).unwrap();
        }
        out
    }

    fn pixels(img: &Image) -> Vec<u32> {
        img.pixels.as_slice().to_vec()
    }

    #[test]
    fn rgba_composites_with_libpng_rounding() {
        let data = [10, 20, 30, 0, 200, 100, 50, 128, 1, 2, 3, 255];
        let f = encode(3, 1, ColorType::Rgba, BitDepth::Eight, &data, |_| {});
        let img = decode(&f, &params(Some([0xffff, 0x8080, 0]))).unwrap();
        let want = |r: u8, g: u8, b: u8| (u32::from(r) << 16) | (u32::from(g) << 8) | u32::from(b);
        assert_eq!(
            pixels(&img),
            vec![
                want(255, 128, 0),
                want(
                    over8(200, 128, 255),
                    over8(100, 128, 128),
                    over8(50, 128, 0)
                ),
                want(1, 2, 3),
            ]
        );
        assert!(img.mask.is_none());
        // no background: the alpha is ignored, no mask
        let img = decode(&f, &params(None)).unwrap();
        assert_eq!(pixels(&img)[0], want(10, 20, 30));
        assert!(img.mask.is_none());
    }

    #[test]
    fn sixteen_bit_composites_before_the_strip() {
        // one RGBA16 pixel: composed at 16 bits, then the low byte dropped
        let (r, a) = (0x12ffu16, 0x8000u16);
        let mut data = Vec::new();
        for v in [r, r, r, a] {
            data.extend_from_slice(&v.to_be_bytes());
        }
        let f = encode(1, 1, ColorType::Rgba, BitDepth::Sixteen, &data, |_| {});
        let bg = 0xff01u16;
        let img = decode(&f, &params(Some([bg; 3]))).unwrap();
        let c = u32::from(over16(r, a, bg.wrapping_mul(257)) >> 8);
        assert_eq!(pixels(&img), vec![(c << 16) | (c << 8) | c]);
    }

    #[test]
    fn sixteen_bit_background_is_widened_like_libpng() {
        // the C core gives 0x5a for this pixel: its 0x3333 background acts as 0x6633
        let mut data = Vec::new();
        for v in [0x5555u16, 0x5555, 0x5555, 0xaaaa] {
            data.extend_from_slice(&v.to_be_bytes());
        }
        let f = encode(1, 1, ColorType::Rgba, BitDepth::Sixteen, &data, |_| {});
        let img = decode(&f, &params(Some([0x3333; 3]))).unwrap();
        assert_eq!(pixels(&img), vec![0x005a_5a5a]);
    }

    #[test]
    fn a_key_makes_a_mask_and_no_composition() {
        let data = [1, 2, 3, 4, 5, 6];
        let f = encode(2, 1, ColorType::Rgb, BitDepth::Eight, &data, |e| {
            e.set_trns(vec![0, 4, 0, 5, 0, 6]);
        });
        let img = decode(&f, &params(Some([0xffff; 3]))).unwrap();
        assert_eq!(pixels(&img), vec![0x010203, 0x040506]);
        assert_eq!(img.mask.as_ref().unwrap().as_slice(), &[255, 0]);
    }

    #[test]
    fn low_depth_gray_expands_and_keys_as_libpng() {
        // 2-bit gray 0..3 → 0, 85, 170, 255; key 2 (masked to the depth) is transparent
        let f = encode(
            4,
            1,
            ColorType::Grayscale,
            BitDepth::Two,
            &[0b0001_1011],
            |e| {
                e.set_trns(vec![0, 2]);
            },
        );
        let img = decode(&f, &params(None)).unwrap();
        let g = |v: u32| (v << 16) | (v << 8) | v;
        assert_eq!(pixels(&img), vec![g(0), g(85), g(170), g(255)]);
        assert_eq!(img.mask.as_ref().unwrap().as_slice(), &[255, 255, 0, 255]);
    }

    #[test]
    fn palette_alpha_composites_or_masks() {
        let pal = vec![255, 0, 0, 0, 255, 0, 0, 0, 255];
        let data = [0u8, 1, 2];
        // partial alpha: composited over the background (8-bit)
        let f = encode(3, 1, ColorType::Indexed, BitDepth::Eight, &data, |e| {
            e.set_palette(pal.clone());
            e.set_trns(vec![0, 100]);
        });
        let img = decode(&f, &params(Some([0x1000, 0x2000, 0x3000]))).unwrap();
        assert_eq!(
            pixels(&img),
            vec![
                0x102030,
                (u32::from(over8(0, 100, 0x10)) << 16)
                    | (u32::from(over8(255, 100, 0x20)) << 8)
                    | u32::from(over8(0, 100, 0x30)),
                0x0000ff,
            ]
        );
        assert!(img.mask.is_none());
        // all-or-nothing: a mask
        let f = encode(3, 1, ColorType::Indexed, BitDepth::Eight, &data, |e| {
            e.set_palette(pal.clone());
            e.set_trns(vec![0, 255]);
        });
        let img = decode(&f, &params(Some([0; 3]))).unwrap();
        assert_eq!(pixels(&img), vec![0xff0000, 0x00ff00, 0x0000ff]);
        assert_eq!(img.mask.as_ref().unwrap().as_slice(), &[0, 255, 255]);
    }

    #[test]
    fn adam7_rows_cover_every_pixel_once() {
        for (w, h) in [(1, 1), (2, 3), (8, 8), (13, 11), (1, 9), (9, 1)] {
            let mut seen = vec![0u8; (w * h) as usize];
            for (y, x0, dx, samples) in rows(w, h, true) {
                for j in 0..samples {
                    seen[(y * w + x0 + j * dx) as usize] += 1;
                }
            }
            assert!(seen.iter().all(|&n| n == 1), "{w}×{h}");
        }
    }

    #[test]
    fn gamma_with_composition_goes_to_the_host() {
        let data = [10, 20, 30, 128];
        let f = encode(1, 1, ColorType::Rgba, BitDepth::Eight, &data, |e| {
            e.set_source_gamma(png::ScaledFloat::from_scaled(45455));
        });
        assert!(matches!(
            decode(&f, &params(Some([0; 3]))),
            Err(Error::Host)
        ));
        // nothing to composite: decoded here
        assert!(decode(&f, &params(None)).is_ok());
    }

    #[test]
    fn bkgd_follows_libpng() {
        let rgb = |bkgd: &[u8]| {
            let f = encode_with(
                1,
                1,
                ColorType::Rgb,
                BitDepth::Eight,
                &[1, 2, 3],
                |_| {},
                &[(png::chunk::bKGD, bkgd)],
            );
            decode(&f, &params(None)).unwrap().bkgd
        };
        assert_eq!(rgb(&[0, 7, 0, 8, 0, 9]), Some([7, 8, 9]));
        // an 8-bit file's background must fit in 8 bits; a wrong length is ignored
        assert_eq!(rgb(&[1, 7, 0, 8, 0, 9]), None);
        assert_eq!(rgb(&[0, 7]), None);
        // a palette index resolves through the palette; one past it is ignored
        let pal = |bkgd: &[u8]| {
            let f = encode_with(
                1,
                1,
                ColorType::Indexed,
                BitDepth::Eight,
                &[0],
                |e| e.set_palette(vec![1, 2, 3, 4, 5, 6]),
                &[(png::chunk::bKGD, bkgd)],
            );
            decode(&f, &params(None)).unwrap().bkgd
        };
        assert_eq!(pal(&[1]), Some([4, 5, 6]));
        assert_eq!(pal(&[2]), None);
    }

    #[test]
    fn limits_and_garbage_fail_without_panicking() {
        let f = encode(4, 4, ColorType::Rgb, BitDepth::Eight, &[0; 48], |_| {});
        let mut p = params(None);
        p.limit_w = 3;
        assert!(matches!(decode(&f, &p), Err(Error::Size)));
        assert!(matches!(
            decode(b"GIF89a", &params(None)),
            Err(Error::NotImage)
        ));
        for cut in [9, 20, 33, f.len() - 13, f.len() - 1] {
            assert!(decode(&f[..cut], &params(None)).is_err(), "cut {cut}");
        }
    }

    /// The fuzz smoke of the C ABI: mutated PNGs of every colour type (bit flips, byte sets, cuts, splices; a fixed
    /// random seed, so a failure repeats) never panic, never break the ABI's contract and always free cleanly.
    #[test]
    fn the_abi_survives_mutated_input() {
        use super::super::{GLITNIR_IMG_PNG, Out, glitnir_img_decode, glitnir_img_free};
        let seeds = [
            encode(5, 3, ColorType::Rgba, BitDepth::Eight, &[0x7f; 60], |_| {}),
            encode(3, 2, ColorType::Rgb, BitDepth::Sixteen, &[0x31; 36], |_| {}),
            encode(9, 2, ColorType::Grayscale, BitDepth::Two, &[0x1b; 6], |e| {
                e.set_trns(vec![0, 2])
            }),
            encode(4, 4, ColorType::Indexed, BitDepth::Four, &[0x12; 8], |e| {
                e.set_palette(vec![9, 8, 7, 6, 5, 4, 3, 2, 1]);
                e.set_trns(vec![0, 128]);
            }),
            encode(
                2,
                2,
                ColorType::GrayscaleAlpha,
                BitDepth::Sixteen,
                &[0xa5; 16],
                |_| {},
            ),
        ];
        let mut rng = 0x9e37_79b9_7f4a_7c15u64;
        let mut next = move || {
            rng ^= rng << 13;
            rng ^= rng >> 7;
            rng ^= rng << 17;
            rng
        };
        let mut p = params(Some([0x1234, 0x8000, 0xfedc]));
        p.limit_w = 64;
        p.limit_h = 64;
        let mut ok = 0;
        for run in 0..20_000 {
            let s = &seeds[(next() % seeds.len() as u64) as usize];
            let mut b = s.clone();
            for _ in 0..1 + next() % 6 {
                let n = b.len() as u64;
                if n <= 9 {
                    break;
                }
                match next() % 5 {
                    0 => b[(next() % n) as usize] ^= 1 << (next() % 8),
                    1 => b[(next() % n) as usize] = next() as u8,
                    2 => b.truncate(8 + (next() % (n - 8)) as usize),
                    3 => {
                        let (x, y) = ((next() % n) as usize, (next() % n) as usize);
                        b[x] = b[y];
                    }
                    _ => {
                        let at = 8 + (next() % (n - 8)) as usize;
                        b[at] = 0xff;
                    }
                }
            }
            if next() % 4 != 0 {
                // most runs get valid CRCs again, so the mutation reaches past the chunk checks
                let mut at = 8;
                while at + 12 <= b.len() {
                    let len = u32::from_be_bytes(b[at..at + 4].try_into().unwrap()) as usize;
                    if len > b.len() - at - 12 {
                        break;
                    }
                    let crc = crc32(&b[at + 4..at + 8 + len]);
                    b[at + 8 + len..at + 12 + len].copy_from_slice(&crc.to_be_bytes());
                    at += 12 + len;
                }
            }
            p.bg_set = (run & 1) as u8;
            let mut out = Out::empty();
            let r =
                unsafe { glitnir_img_decode(GLITNIR_IMG_PNG, b.as_ptr(), b.len(), &p, &mut out) };
            assert_ne!(r, crate::ffi::GLITNIR_E_PANIC, "run {run}: {b:?}");
            assert_eq!(r, out.err);
            if r == 0 {
                ok += 1;
                assert!(!out.pixels.is_null() && out.width > 0 && out.height > 0);
                assert!(out.width <= 64 && out.height <= 64);
            } else {
                assert!(out.pixels.is_null() && out.mask.is_null());
            }
            unsafe { glitnir_img_free(&mut out) };
        }
        assert!(ok > 0, "no mutation decoded");
    }
}

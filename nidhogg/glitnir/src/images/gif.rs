//! GIF as a host built on giflib decodes it: the file read as `DGifSlurp` reads it, then frames composed with the
//! host's disposal and transparency rules.
//!
//! The record structure (screen, colour maps, images, extension blocks) is walked here, so that every frame's
//! extension blocks are exactly the ones giflib keeps (`SavedImage.ExtensionBlocks`: the first sub-block under the
//! extension's function, each further one as a continuation, function 0) and the same files fail. The `gif` crate
//! decodes each image's LZW data into colour indices (de-interlaced), from a one-image GIF made of the image's own
//! bytes (its `FrameDecoder` loops forever on data that ends early). As `DGifSlurp` does, every image is decoded
//! once when the file is first read, so a broken later frame fails the file; the frames are decoded again while
//! composing, from the compressed data the state keeps.
//!
//! Composition: the area outside the first image is the frame background; frame 0 is drawn whole (disposal
//! "background"), and a later frame keeps the canvas's pixel wherever it has its transparent index, unless its
//! disposal is something other than "none" ("unspecified" and "previous" count as "none"). The transparent index
//! draws the GIF background when the host gives one, else the frame background. A request for the frame after the
//! state's draws only that frame; any other starts from frame 0 on the kept canvas.

use super::anim::Anim;
use super::{CBuf, Error, Image, MAX_FRAMES, Params, size_ok, try_vec};
use gif::{ColorOutput, DecodeOptions, MemoryLimit};
use std::io::Cursor;
use std::num::NonZeroU64;

/// giflib's texts for the errors that have one.
const READ_FAILED: &str = "Failed to read from given file";
const NOT_GIF: &str = "Data is not in GIF format";
const WRONG_RECORD: &str = "Wrong record type detected";

/// `DISPOSE_DO_NOT`, `DISPOSE_BACKGROUND`, `DISPOSE_PREVIOUS`; 0 is "unspecified".
const DO_NOT: u8 = 1;
const BACKGROUND: u8 = 2;
const PREVIOUS: u8 = 3;

/// One image of the file.
struct GifFrame {
    left: i32,
    top: i32,
    width: i32,
    height: i32,
    interlaced: bool,
    /// The local colour map (RGB triples), if any.
    palette: Option<Vec<u8>>,
    /// The image data as the file has it: the minimum code size, the sub-blocks, the terminator.
    lzw: Vec<u8>,
    /// The extension blocks before it, in `Out::ext`'s form.
    ext: Vec<u8>,
}

impl GifFrame {
    /// `DGifSavedExtensionToGCB`: the first graphic control block's disposal and transparent index, if it has
    /// giflib's 4 bytes; else "unspecified" and none.
    fn control(&self) -> (u8, i32) {
        for (function, bytes) in blocks(&self.ext) {
            if function == 0xf9 {
                return if bytes.len() == 4 {
                    let t = if bytes[0] & 1 != 0 {
                        i32::from(bytes[3])
                    } else {
                        -1
                    };
                    ((bytes[0] >> 2) & 7, t)
                } else {
                    (0, -1)
                };
            }
        }
        (0, -1)
    }
}

/// The records of an `Out::ext` blob: (function, bytes).
fn blocks(ext: &[u8]) -> impl Iterator<Item = (u8, &[u8])> {
    let mut at = 0;
    std::iter::from_fn(move || {
        let (&f, rest) = ext.get(at..)?.split_first()?;
        let (&n, rest) = rest.split_first()?;
        let bytes = rest.get(..usize::from(n))?;
        at += 2 + usize::from(n);
        Some((f, bytes))
    })
}

/// What a GIF animation keeps between decodes.
pub struct GifAnim {
    width: i32,
    height: i32,
    global: Option<Vec<u8>>,
    frames: Vec<GifFrame>,
    /// The last composed frame (empty until the first composition).
    canvas: Vec<u32>,
    /// The frame on the canvas; -1: none to continue from.
    pos: i32,
    frame_bg: u32,
    gif_bg_set: bool,
    gif_bg: u32,
    data_bytes: usize,
}

impl GifAnim {
    pub fn bytes(&self) -> usize {
        self.canvas.len() * 4 + self.data_bytes
    }
}

struct Reader<'a> {
    d: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], Error> {
        let s = self
            .d
            .get(self.at..)
            .and_then(|r| r.get(..n))
            .ok_or_else(|| Error::Decode(READ_FAILED.into()))?;
        self.at += n;
        Ok(s)
    }

    fn byte(&mut self) -> Result<u8, Error> {
        Ok(self.take(1)?[0])
    }

    /// `DGifGetExtensionNext`/`DGifGetCodeNext`: the next sub-block, or None at a zero length.
    fn sub_block(&mut self) -> Result<Option<&'a [u8]>, Error> {
        let n = self.byte()?;
        if n == 0 {
            Ok(None)
        } else {
            self.take(usize::from(n)).map(Some)
        }
    }

    /// A colour map of `1 << ((flags & 7) + 1)` entries when FLAGS has its bit.
    fn color_map(&mut self, flags: u8) -> Result<Option<Vec<u8>>, Error> {
        if flags & 0x80 == 0 {
            return Ok(None);
        }
        let n = 3usize << ((flags & 7) + 1);
        Ok(Some(self.take(n)?.to_vec()))
    }
}

fn le16(b: &[u8]) -> i32 {
    i32::from(u16::from_le_bytes([b[0], b[1]]))
}

/// One image's colour indices, `width * height`, de-interlaced: the image alone, at the origin of a screen its size
/// (with a 2-entry colour map, which the crate wants), decoded by the `gif` crate.
fn raster(f: &GifFrame) -> Result<Vec<u8>, Error> {
    let n = (f.width as usize) * (f.height as usize);
    let (w, h) = (
        (f.width as u16).to_le_bytes(),
        (f.height as u16).to_le_bytes(),
    );
    let mut one = Vec::new();
    one.try_reserve(32 + f.lzw.len())
        .map_err(|_| Error::NoMem)?;
    one.extend_from_slice(b"GIF89a");
    one.extend_from_slice(&[w[0], w[1], h[0], h[1], 0x80, 0, 0, 0, 0, 0, 0, 0, 0]);
    one.extend_from_slice(&[b',', 0, 0, 0, 0, w[0], w[1], h[0], h[1]]);
    one.push(if f.interlaced { 0x40 } else { 0 });
    one.extend_from_slice(&f.lzw);
    one.push(b';');
    let mut opts = DecodeOptions::new();
    opts.set_color_output(ColorOutput::Indexed);
    opts.set_memory_limit(MemoryLimit::Bytes(
        NonZeroU64::new(n as u64).ok_or(Error::Size)?,
    ));
    let lib = |e: gif::DecodingError| Error::Decode(e.to_string());
    let mut dec = opts.read_info(Cursor::new(one)).map_err(lib)?;
    let frame = dec
        .read_next_frame()
        .map_err(lib)?
        .ok_or_else(|| Error::Decode(READ_FAILED.into()))?;
    if frame.buffer.len() != n {
        return Err(Error::Decode(READ_FAILED.into()));
    }
    let mut buf = try_vec(n, 0u8)?;
    buf.copy_from_slice(&frame.buffer);
    Ok(buf)
}

/// `DGifOpen` and `DGifSlurp`, and the host's checks after them.
fn read(data: &[u8], p: &Params) -> Result<GifAnim, Error> {
    let mut r = Reader { d: data, at: 0 };
    if data.len() < 6 || &data[..3] != b"GIF" {
        return Err(Error::NotFormat(NOT_GIF));
    }
    r.at = 6;
    let screen = r.take(7).map_err(|_| Error::NotFormat(READ_FAILED))?;
    let (width, height) = (le16(&screen[0..]), le16(&screen[2..]));
    let global = r
        .color_map(screen[4])
        .map_err(|_| Error::NotFormat(READ_FAILED))?;
    size_ok(p, i64::from(width), i64::from(height))?;

    let mut frames: Vec<GifFrame> = Vec::new();
    let mut ext: Vec<u8> = Vec::new();
    let mut outside = false;
    loop {
        match r.byte()? {
            b',' => {
                let d = r.take(9)?;
                let (left, top, w, h, flags) =
                    (le16(d), le16(&d[2..]), le16(&d[4..]), le16(&d[6..]), d[8]);
                let palette = r.color_map(flags)?;
                let min_code = r.byte()?;
                if min_code > 8 {
                    return Err(Error::Decode(READ_FAILED.into()));
                }
                if w <= 0 || h <= 0 {
                    return Err(Error::Decode(String::new()));
                }
                let from = r.at - 1;
                while r.sub_block()?.is_some() {}
                let mut lzw = Vec::new();
                lzw.try_reserve_exact(r.at - from)
                    .map_err(|_| Error::NoMem)?;
                lzw.extend_from_slice(&data[from..r.at]);
                let f = GifFrame {
                    left,
                    top,
                    width: w,
                    height: h,
                    interlaced: flags & 0x40 != 0,
                    palette,
                    lzw,
                    ext: std::mem::take(&mut ext),
                };
                // the host refuses a frame outside the screen after reading the file; such a frame is not
                // decoded (its size is the file's to choose)
                if top + h > height || left + w > width {
                    outside = true;
                } else {
                    raster(&f)?;
                }
                if frames.len() == MAX_FRAMES {
                    return Err(Error::NoMem);
                }
                frames.try_reserve(1).map_err(|_| Error::NoMem)?;
                frames.push(f);
            }
            b'!' => {
                let function = r.byte()?;
                // as DGifSlurp: the first sub-block under the function, then continuations until a zero length
                // (after an empty first one too)
                let mut add = |f: u8, b: &[u8]| -> Result<(), Error> {
                    ext.try_reserve(2 + b.len()).map_err(|_| Error::NoMem)?;
                    ext.push(f);
                    ext.push(b.len() as u8);
                    ext.extend_from_slice(b);
                    Ok(())
                };
                if let Some(b) = r.sub_block()? {
                    add(function, b)?;
                }
                while let Some(b) = r.sub_block()? {
                    add(0, b)?;
                }
            }
            b';' => break,
            _ => return Err(Error::Decode(WRONG_RECORD.into())),
        }
    }
    if frames.is_empty() {
        return Err(Error::Decode(String::new()));
    }
    if outside {
        return Err(Error::Subimage);
    }
    Ok(GifAnim {
        width,
        height,
        global,
        frames,
        canvas: Vec::new(),
        pos: -1,
        frame_bg: 0,
        gif_bg_set: false,
        gif_bg: 0,
        data_bytes: data.len(),
    })
}

pub fn decode(data: &[u8], p: &Params, state: Option<Box<Anim>>) -> Result<Image, Error> {
    let gif_bg_set = p.gif_bg_set != 0;
    let (mut g, cached) = match state.map(|b| *b) {
        Some(Anim::Gif(mut g)) => {
            if g.frame_bg != p.frame_bg
                || g.gif_bg_set != gif_bg_set
                || (gif_bg_set && g.gif_bg != p.gif_bg)
            {
                g.pos = -1;
            }
            (g, true)
        }
        _ => (read(data, p)?, false),
    };
    let idx = p.frame_index.max(0);
    let (width, height) = (g.width, g.height);
    if idx as usize >= g.frames.len() {
        return Err(Error::Index(idx));
    }
    size_ok(p, i64::from(width), i64::from(height))?;
    let n = width as usize * height as usize;
    if g.canvas.is_empty() {
        g.canvas = try_vec(n, 0u32)?;
    }

    let f0 = &g.frames[0];
    let (top, left, bot, right) = (f0.top, f0.left, f0.top + f0.height, f0.left + f0.width);
    let frame_bg = p.frame_bg;
    for y in 0..height {
        for x in 0..width {
            if y < top || y >= bot || x < left || x >= right {
                g.canvas[(y * width + x) as usize] = frame_bg;
            }
        }
    }

    let start = if cached && g.pos >= 0 && g.pos == idx - 1 {
        idx
    } else {
        0
    };
    for j in start..=idx {
        let f = &g.frames[j as usize];
        let raster = raster(f)?;
        let (mut disposal, transparency) = f.control();
        if j == 0 {
            disposal = BACKGROUND;
        }
        if disposal == 0 || disposal == PREVIOUS {
            disposal = DO_NOT;
        }
        let mut colors = [0u32; 256];
        if let Some(map) = f.palette.as_deref().or(g.global.as_deref()) {
            for (i, c) in map.as_chunks::<3>().0.iter().take(256).enumerate() {
                colors[i] = if transparency == i as i32 {
                    if gif_bg_set { p.gif_bg } else { frame_bg }
                } else {
                    (u32::from(c[0]) << 16) | (u32::from(c[1]) << 8) | u32::from(c[2])
                };
            }
        }
        for y in 0..f.height {
            for x in 0..f.width {
                let c = raster[(y * f.width + x) as usize];
                if transparency != i32::from(c) || disposal != DO_NOT {
                    g.canvas[((y + f.top) * width + x + f.left) as usize] = colors[usize::from(c)];
                }
            }
        }
    }

    let mut pixels = CBuf::<u32>::zeroed(n)?;
    pixels.as_mut_slice().copy_from_slice(&g.canvas);
    let cur = &g.frames[idx as usize];
    let mut delay = None;
    for (function, bytes) in blocks(&cur.ext) {
        let d = delay.get_or_insert(0);
        if function == 0xf9 && bytes.len() == 4 {
            *d = (i32::from(bytes[2]) << 8) | i32::from(bytes[1]);
        }
    }
    let ext = if cur.ext.is_empty() {
        None
    } else {
        let mut e = CBuf::<u8>::zeroed(cur.ext.len())?;
        e.as_mut_slice().copy_from_slice(&cur.ext);
        Some(e)
    };
    let frame_count = g.frames.len() as i32;
    g.pos = idx;
    g.frame_bg = frame_bg;
    g.gif_bg_set = gif_bg_set;
    g.gif_bg = p.gif_bg;
    let mut img = Image::still(pixels, width, height);
    img.frame_count = frame_count;
    img.delay = delay;
    img.corners = Some([top, left, bot, right]);
    img.ext = ext;
    img.anim = Some(Box::new(Anim::Gif(g)));
    Ok(img)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A GIF of a W×H screen with a 4-colour global map (black, red, green, blue) and FRAMES, each
    /// (left, top, w, h, indices, graphic control block bytes or None).
    /// A frame for `build`: left, top, width, height, palette indices, graphics control block.
    type F<'a> = (u16, u16, u16, u16, &'a [u8], Option<[u8; 4]>);

    pub(crate) fn build(w: u16, h: u16, frames: &[F<'_>]) -> Vec<u8> {
        let mut g = b"GIF89a".to_vec();
        g.extend_from_slice(&w.to_le_bytes());
        g.extend_from_slice(&h.to_le_bytes());
        g.extend_from_slice(&[0x81, 0, 0]);
        g.extend_from_slice(&[0, 0, 0, 255, 0, 0, 0, 255, 0, 0, 0, 255]);
        for &(l, t, fw, fh, idx, gcb) in frames {
            if let Some(c) = gcb {
                g.extend_from_slice(&[0x21, 0xf9, 4]);
                g.extend_from_slice(&c);
                g.push(0);
            }
            g.push(b',');
            for v in [l, t, fw, fh] {
                g.extend_from_slice(&v.to_le_bytes());
            }
            g.push(0);
            g.push(2);
            let lzw = lzw_literal(2, idx);
            for chunk in lzw.chunks(255) {
                g.push(chunk.len() as u8);
                g.extend_from_slice(chunk);
            }
            g.push(0);
        }
        g.push(b';');
        g
    }

    /// LZW data for INDICES with MIN code size that only ever emits literals (a clear code before the table
    /// would grow a code width), so it is a valid stream any decoder reads.
    pub(crate) fn lzw_literal(min: u8, indices: &[u8]) -> Vec<u8> {
        let clear = 1u32 << min;
        let width = u32::from(min) + 1;
        let (mut acc, mut bits, mut out) = (0u64, 0u32, Vec::new());
        let put = |code: u32, acc: &mut u64, bits: &mut u32, out: &mut Vec<u8>| {
            *acc |= u64::from(code) << *bits;
            *bits += width;
            while *bits >= 8 {
                out.push(*acc as u8);
                *acc >>= 8;
                *bits -= 8;
            }
        };
        put(clear, &mut acc, &mut bits, &mut out);
        for (i, &c) in indices.iter().enumerate() {
            put(u32::from(c), &mut acc, &mut bits, &mut out);
            // each literal after the first adds a table entry; clear before the width would grow
            if i % 2 == 1 {
                put(clear, &mut acc, &mut bits, &mut out);
            }
        }
        put(clear + 1, &mut acc, &mut bits, &mut out);
        if bits > 0 {
            out.push(acc as u8);
        }
        out
    }

    fn params(index: i32) -> Params {
        Params {
            frame_index: index,
            frame_bg: 0x00ab_cdef,
            keep: 1,
            ..Params::default()
        }
    }

    const R: u32 = 0xff0000;
    const G: u32 = 0x00ff00;
    const B: u32 = 0x0000ff;
    const K: u32 = 0;

    #[test]
    fn frames_compose_with_the_hosts_disposal_and_transparency() {
        // frame 0 covers the left half; frame 1 (transparent index 3, disposal none) draws over all of it
        let f = build(
            2,
            2,
            &[
                (0, 0, 1, 2, &[1, 3], Some([0x01, 0, 0, 3])),
                (0, 0, 2, 2, &[3, 2, 2, 3], Some([0x04 | 0x01, 10, 0, 3])),
            ],
        );
        let img = decode(&f, &params(0), None).unwrap();
        // frame 0: drawn whole (its transparent index too, as the frame background), outside: frame background
        assert_eq!(img.pixels.as_slice(), &[R, 0xabcdef, 0xabcdef, 0xabcdef]);
        assert_eq!(img.corners, Some([0, 0, 2, 1]));
        assert_eq!(img.frame_count, 2);
        let img1 = decode(&f, &params(1), img.anim).unwrap();
        // frame 1 from frame 0's canvas: transparent pixels keep it
        assert_eq!(img1.pixels.as_slice(), &[R, G, G, 0xabcdef]);
        assert_eq!(img1.delay, Some(10));
        // disposal "background" (2) draws the transparent index too, over the GIF background
        let f = build(
            2,
            1,
            &[
                (0, 0, 2, 1, &[1, 1], None),
                (0, 0, 2, 1, &[3, 2], Some([0x08 | 0x01, 0, 0, 3])),
            ],
        );
        let mut p = params(1);
        p.gif_bg_set = 1;
        p.gif_bg = 0x123456;
        assert_eq!(
            decode(&f, &p, None).unwrap().pixels.as_slice(),
            &[0x123456, G]
        );
        // no graphic control block: unspecified, drawn as "none" with no transparency
        let f = build(1, 1, &[(0, 0, 1, 1, &[2], None), (0, 0, 1, 1, &[3], None)]);
        assert_eq!(
            decode(&f, &params(1), None).unwrap().pixels.as_slice(),
            &[B]
        );
    }

    #[test]
    fn the_next_frame_continues_and_others_restart() {
        let f = build(
            1,
            1,
            &[
                (0, 0, 1, 1, &[1], None),
                (0, 0, 1, 1, &[3], Some([0x01, 0, 0, 3])),
                (0, 0, 1, 1, &[2], None),
            ],
        );
        let a = decode(&f, &params(0), None).unwrap();
        let b = decode(&[], &params(1), a.anim).unwrap();
        assert_eq!(b.pixels.as_slice(), &[R]);
        // a changed frame background restarts from frame 0
        let mut p = params(2);
        p.frame_bg = 1;
        let c = decode(&[], &p, b.anim).unwrap();
        assert_eq!(c.pixels.as_slice(), &[G]);
        // past the last frame
        assert!(matches!(
            decode(&[], &params(3), c.anim),
            Err(Error::Index(3))
        ));
    }

    #[test]
    fn extension_blocks_are_giflibs() {
        let mut f = build(1, 1, &[(0, 0, 1, 1, &[1], Some([0, 7, 1, 0]))]);
        // a comment with two sub-blocks before the image, and an application block after it
        let at = 6 + 7 + 12;
        let comment = [0x21, 0xfe, 2, b'h', b'i', 1, b'!', 0];
        f.splice(at..at, comment);
        let img = decode(&f, &params(0), None).unwrap();
        let ext = img.ext.as_ref().unwrap().as_slice();
        assert_eq!(ext, &[0xfe, 2, b'h', b'i', 0, 1, b'!', 0xf9, 4, 0, 7, 1, 0]);
        assert_eq!(img.delay, Some(263));
    }

    #[test]
    fn errors_are_the_hosts() {
        let good = build(2, 2, &[(0, 0, 2, 2, &[0, 1, 2, 3], None)]);
        assert!(matches!(
            decode(b"PNG", &params(0), None),
            Err(Error::NotFormat(_))
        ));
        assert!(matches!(
            decode(&good[..8], &params(0), None),
            Err(Error::NotFormat(_))
        ));
        // no trailer: giflib's read fails
        assert!(matches!(
            decode(&good[..good.len() - 1], &params(0), None),
            Err(Error::Decode(_))
        ));
        // a frame outside the screen
        let out = build(2, 2, &[(1, 0, 2, 2, &[0, 1, 2, 3], None)]);
        assert!(matches!(
            decode(&out, &params(0), None),
            Err(Error::Subimage)
        ));
        // too few pixels in the data
        let short = build(2, 2, &[(0, 0, 2, 2, &[0, 1], None)]);
        assert!(matches!(
            decode(&short, &params(0), None),
            Err(Error::Decode(_))
        ));
        // a broken later frame fails the first too, as DGifSlurp
        let mut two = build(1, 1, &[(0, 0, 1, 1, &[1], None), (0, 0, 1, 1, &[2], None)]);
        let n = two.len();
        two[n - 4] = 0xff;
        assert!(decode(&two, &params(0), None).is_err());
        let mut p = params(0);
        p.limit_w = 1;
        assert!(matches!(decode(&good, &p, None), Err(Error::Size)));
        let unknown = [&good[..good.len() - 1], b"x;"].concat();
        assert!(matches!(
            decode(&unknown, &params(0), None),
            Err(Error::Decode(_))
        ));
        // more frames than the cap (frames off the screen: not decoded, still kept)
        let one = build(1, 1, &[(1, 0, 1, 1, &[0], None)]);
        let two = build(1, 1, &[(1, 0, 1, 1, &[0], None), (1, 0, 1, 1, &[0], None)]);
        let frame = &two[one.len() - 1..two.len() - 1];
        let many = |n: usize| [&one[..one.len() - 1], &frame.repeat(n)[..], b";"].concat();
        assert!(matches!(
            decode(&many(MAX_FRAMES - 1), &params(0), None),
            Err(Error::Subimage)
        ));
        assert!(matches!(
            decode(&many(MAX_FRAMES), &params(0), None),
            Err(Error::NoMem)
        ));
    }

    #[test]
    fn the_abi_survives_mutated_input() {
        let seeds = [
            build(
                3,
                2,
                &[
                    (0, 0, 2, 2, &[0, 1, 2, 3], Some([0x05, 3, 0, 1])),
                    (1, 0, 2, 1, &[3, 3], Some([0x09, 0, 0, 3])),
                    (0, 1, 3, 1, &[1, 2, 3], None),
                ],
            ),
            build(1, 4, &[(0, 0, 1, 4, &[0, 1, 2, 3], None)]),
        ];
        super::super::fuzz_abi(super::super::GLITNIR_IMG_GIF, &seeds, 20_000);
    }

    #[test]
    fn interlaced_frames_deinterlace() {
        let mut f = build(1, 4, &[(0, 0, 1, 4, &[0, 1, 2, 3], None)]);
        // the flags byte of the image descriptor: interlaced; the data order is rows 0, 2, 1, 3
        let at = 6 + 7 + 12 + 1 + 8;
        f[at] = 0x40;
        let img = decode(&f, &params(0), None).unwrap();
        assert_eq!(img.pixels.as_slice(), &[K, G, R, B]);
    }
}

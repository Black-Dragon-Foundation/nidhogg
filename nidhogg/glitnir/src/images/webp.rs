//! WebP as a host built on libwebp decodes it.
//!
//! A still image is decoded by `image-webp` (RGBA when it has alpha, else RGB; lossy images with its "fancy"
//! upsampling, libwebp's default) and its alpha composited over the background with the host's float formula.
//!
//! An animation is read as libwebp's demuxer reads it (ANIM before the frames; each ANMF an optional ALPH and a VP8
//! or VP8L image whose own header gives the frame's size, inside the canvas) and composed as `WebPAnimDecoder`
//! composes it (`anim_decode.c`): a key frame starts from a transparent canvas, any other from the previous frame
//! after its disposal; the frame's pixels replace the canvas's, and then, when it blends, every pixel that is not
//! opaque is blended over the previous canvas with `BlendPixelNonPremult` (except where the previous frame was
//! disposed to the background). Each frame's image is decoded by `image-webp` as a still image made of its own
//! chunks. The result goes over the background as a still image's alpha does. The state the host keeps holds the
//! file, the two canvases and the position, so frame N continues from frame N-1.

use super::anim::{Anim, blend_row};
use super::{CBuf, Error, Image, MAX_FRAMES, Params, size_ok, try_vec};
use image_webp::WebPDecoder;
use std::io::Cursor;
use std::ops::Range;

fn le24(b: &[u8]) -> u32 {
    u32::from(b[0]) | (u32::from(b[1]) << 8) | (u32::from(b[2]) << 16)
}

fn le32(b: &[u8]) -> u32 {
    u32::from_le_bytes([b[0], b[1], b[2], b[3]])
}

/// One frame of an animation, as the demuxer's iterator gives it.
#[derive(Clone, Default)]
struct WebpFrame {
    x: u32,
    y: u32,
    width: u32,
    height: u32,
    duration: i32,
    dispose_background: bool,
    blend: bool,
    has_alpha: bool,
    lossless: bool,
    /// The ALPH and image chunks' payloads, in the file.
    alph: Option<Range<usize>>,
    image: Range<usize>,
}

/// What an animation keeps between decodes (`WebPAnimDecoder`'s state, and the host's position on it).
pub struct WebpAnim {
    data: Vec<u8>,
    width: u32,
    height: u32,
    frames: Vec<WebpFrame>,
    curr: Vec<[u8; 4]>,
    prev_disposed: Vec<[u8; 4]>,
    /// The previous frame (`prev_iter_`; zeroed: dispose none, empty) and whether it was a key frame.
    prev: WebpFrame,
    prev_was_key: bool,
    /// The next frame (0-based) and the previous one's timestamp.
    next: usize,
    prev_timestamp: i32,
    /// The host's position: the frame `curr` holds (-1: none), whether it is kept, and its duration.
    pos: i32,
    has_last: bool,
    duration: i32,
}

impl WebpAnim {
    pub fn bytes(&self) -> usize {
        self.data.len() + (self.width as usize) * (self.height as usize) * 8
    }

    /// `WebPAnimDecoderReset`.
    fn reset(&mut self) {
        self.prev_timestamp = 0;
        self.prev = WebpFrame::default();
        self.prev_was_key = false;
        self.next = 0;
    }

    /// `WebPAnimDecoderGetNext`: compose the next frame into `curr`; its timestamp.
    fn get_next(&mut self) -> Result<i32, String> {
        let (cw, ch) = (self.width as usize, self.height as usize);
        let iter = self.frames[self.next].clone();
        let timestamp = self.prev_timestamp.wrapping_add(iter.duration);
        let full = |f: &WebpFrame| f.width == self.width && f.height == self.height;
        let is_key = self.next == 0
            || ((!iter.has_alpha || !iter.blend) && full(&iter))
            || (self.prev.dispose_background && (full(&self.prev) || self.prev_was_key));
        if is_key {
            self.curr.fill([0; 4]);
        } else {
            self.curr.copy_from_slice(&self.prev_disposed);
        }
        let rgba = frame_rgba(&self.data, &iter)?;
        let (fx, fy, fw) = (iter.x as usize, iter.y as usize, iter.width as usize);
        for (row, src) in rgba.chunks_exact(fw).enumerate() {
            let at = (fy + row) * cw + fx;
            self.curr[at..at + fw].copy_from_slice(src);
        }
        if self.next > 0 && iter.blend && !is_key {
            for row in 0..iter.height as usize {
                let y = fy + row;
                let ranges = if !self.prev.dispose_background {
                    [(fx, fw), (0, 0)]
                } else {
                    blend_ranges(&iter, &self.prev, y)
                };
                for (left, width) in ranges {
                    if width > 0 {
                        let at = y * cw + left;
                        blend_row(
                            &mut self.curr[at..at + width],
                            &self.prev_disposed[at..at + width],
                        );
                    }
                }
            }
        }
        self.prev_timestamp = timestamp;
        self.prev_was_key = is_key;
        self.prev_disposed.copy_from_slice(&self.curr);
        if iter.dispose_background {
            let (px, py, pw) = (iter.x as usize, iter.y as usize, iter.width as usize);
            for row in 0..iter.height as usize {
                let at = (py + row) * cw + px;
                self.prev_disposed[at..at + pw].fill([0; 4]);
            }
        }
        self.prev = iter;
        self.next += 1;
        debug_assert!(self.curr.len() == cw * ch);
        Ok(timestamp)
    }
}

/// `FindBlendRangeAtRow`: the parts of row Y of SRC outside DST's rectangle, as (left, width) pairs.
fn blend_ranges(src: &WebpFrame, dst: &WebpFrame, y: usize) -> [(usize, usize); 2] {
    let (sx, sw) = (src.x as usize, src.width as usize);
    let (dx, dw, dy, dh) = (
        dst.x as usize,
        dst.width as usize,
        dst.y as usize,
        dst.height as usize,
    );
    let (src_max_x, dst_max_x, dst_max_y) = (sx + sw, dx + dw, dy + dh);
    if y < dy || y >= dst_max_y || sx >= dst_max_x || src_max_x <= dx {
        return [(sx, sw), (0, 0)];
    }
    let mut r = [(0, 0), (0, 0)];
    if sx < dx {
        r[0] = (sx, dx - sx);
    }
    if src_max_x > dst_max_x {
        r[1] = (dst_max_x, src_max_x - dst_max_x);
    }
    r
}

/// A frame's pixels, `width * height` RGBA (alpha 255 where the frame has none): its chunks decoded as a still
/// image, with a VP8X header that declares the alpha libwebp would write (an ALPH chunk, or any lossless image).
fn frame_rgba(data: &[u8], f: &WebpFrame) -> Result<Vec<[u8; 4]>, String> {
    let alpha = f.alph.is_some() || f.lossless;
    let chunk = |fourcc: &[u8; 4], r: &Range<usize>, out: &mut Vec<u8>| {
        out.extend_from_slice(fourcc);
        out.extend_from_slice(&(r.len() as u32).to_le_bytes());
        out.extend_from_slice(&data[r.clone()]);
        if r.len() % 2 == 1 {
            out.push(0);
        }
    };
    let mut file = Vec::new();
    file.try_reserve(64 + f.image.len() + f.alph.as_ref().map_or(0, |a| a.len()))
        .map_err(|_| "out of memory".to_string())?;
    file.extend_from_slice(b"RIFF\0\0\0\0WEBPVP8X");
    file.extend_from_slice(&10u32.to_le_bytes());
    file.extend_from_slice(&[if alpha { 0x10 } else { 0 }, 0, 0, 0]);
    file.extend_from_slice(&(f.width - 1).to_le_bytes()[..3]);
    file.extend_from_slice(&(f.height - 1).to_le_bytes()[..3]);
    if let Some(a) = &f.alph {
        chunk(b"ALPH", a, &mut file);
    }
    chunk(
        if f.lossless { b"VP8L" } else { b"VP8 " },
        &f.image,
        &mut file,
    );
    let riff = (file.len() - 8) as u32;
    file[4..8].copy_from_slice(&riff.to_le_bytes());

    let mut dec = WebPDecoder::new(Cursor::new(file)).map_err(|e| e.to_string())?;
    if dec.dimensions() != (f.width, f.height) {
        return Err("frame size".into());
    }
    let n = f.width as usize * f.height as usize;
    let size = dec.output_buffer_size().ok_or("frame size")?;
    let mut buf = try_vec(size, 0u8).map_err(|_| "out of memory".to_string())?;
    dec.read_image(&mut buf).map_err(|e| e.to_string())?;
    let mut out = try_vec(n, [0u8, 0, 0, 255]).map_err(|_| "out of memory".to_string())?;
    if dec.has_alpha() {
        for (o, c) in out.iter_mut().zip(buf.as_chunks::<4>().0) {
            *o = *c;
        }
    } else {
        for (o, c) in out.iter_mut().zip(buf.as_chunks::<3>().0) {
            o[..3].copy_from_slice(c);
        }
    }
    Ok(out)
}

/// `WebPGetFeatures` of an image chunk (`VP8GetInfo`, `VP8LGetInfo`): width, height, alpha.
fn image_features(fourcc: &[u8], p: &[u8]) -> Option<(u32, u32, bool)> {
    match fourcc {
        b"VP8 " => {
            if p.len() < 10 {
                return None;
            }
            let bits = le24(p);
            let key_frame = bits & 1 == 0;
            if !key_frame
                || (bits >> 1) & 7 > 3
                || (bits >> 4) & 1 == 0
                || (bits >> 5) as usize >= p.len()
            {
                return None;
            }
            if p[3..6] != [0x9d, 0x01, 0x2a] {
                return None;
            }
            let w = u32::from(u16::from_le_bytes([p[6], p[7]]) & 0x3fff);
            let h = u32::from(u16::from_le_bytes([p[8], p[9]]) & 0x3fff);
            (w > 0 && h > 0).then_some((w, h, false))
        }
        b"VP8L" => {
            if p.len() < 5 || p[0] != 0x2f {
                return None;
            }
            let bits = le32(&p[1..]);
            if bits >> 29 != 0 {
                return None;
            }
            Some((
                (bits & 0x3fff) + 1,
                ((bits >> 14) & 0x3fff) + 1,
                (bits >> 28) & 1 != 0,
            ))
        }
        _ => None,
    }
}

/// The animation's frames, as `WebPDemux` reads them for `WebPAnimDecoderNew`; its texts are ours.
fn demux(data: &[u8], cw: u32, ch: u32) -> Result<Vec<WebpFrame>, Error> {
    let bad = |m: &str| Error::Parse(m.to_string());
    let riff_end = (le32(&data[4..]) as usize)
        .checked_add(8)
        .filter(|&e| e <= data.len() && e >= 12)
        .ok_or_else(|| bad("truncated RIFF"))?;
    let mut at = 12;
    let mut anim_chunks = 0;
    let mut frames: Vec<WebpFrame> = Vec::new();
    let mut first = true;
    while at < riff_end {
        let head = data
            .get(at..at + 8)
            .filter(|_| at + 8 <= riff_end)
            .ok_or_else(|| bad("truncated chunk"))?;
        let (fourcc, size) = (&head[..4], le32(&head[4..]) as usize);
        let start = at + 8;
        let end = start
            .checked_add(size)
            .filter(|&e| e <= riff_end)
            .ok_or_else(|| bad("truncated chunk"))?;
        let payload = &data[start..end];
        if first && fourcc != b"VP8X" {
            return Err(bad("no VP8X"));
        }
        first = false;
        match fourcc {
            b"ANIM" => {
                if size < 6 {
                    return Err(bad("ANIM"));
                }
                anim_chunks += 1;
            }
            b"ANMF" => {
                if anim_chunks == 0 {
                    return Err(bad("ANMF before ANIM"));
                }
                if size < 16 {
                    return Err(bad("ANMF"));
                }
                let mut f = WebpFrame {
                    x: 2 * le24(payload),
                    y: 2 * le24(&payload[3..]),
                    duration: le24(&payload[12..]) as i32,
                    dispose_background: payload[15] & 1 != 0,
                    blend: payload[15] & 2 == 0,
                    ..WebpFrame::default()
                };
                // [ALPH] then VP8 or VP8L; anything else ends the frame
                let mut sub = start + 16;
                let mut image = false;
                while sub + 8 <= end && !image {
                    let (sf, ss) = (&data[sub..sub + 4], le32(&data[sub + 4..]) as usize);
                    let (s0, s1) = (
                        sub + 8,
                        (sub + 8)
                            .checked_add(ss)
                            .filter(|&e| e <= end)
                            .ok_or_else(|| bad("ANMF"))?,
                    );
                    match sf {
                        b"ALPH" if f.alph.is_none() => {
                            f.alph = Some(s0..s1);
                            f.has_alpha = true;
                        }
                        b"VP8L" if f.alph.is_some() => return Err(bad("ALPH before VP8L")),
                        b"VP8 " | b"VP8L" => {
                            let (w, h, a) = image_features(sf, &data[s0..s1])
                                .ok_or_else(|| bad("frame header"))?;
                            f.width = w;
                            f.height = h;
                            f.has_alpha |= a;
                            f.lossless = sf == b"VP8L";
                            f.image = s0..s1;
                            image = true;
                        }
                        _ => break,
                    }
                    sub = s1 + (ss & 1);
                }
                if !image {
                    return Err(bad("frame without an image"));
                }
                if f.x + f.width > cw || f.y + f.height > ch {
                    return Err(bad("frame outside the canvas"));
                }
                if frames.len() == MAX_FRAMES {
                    return Err(Error::NoMem);
                }
                frames.try_reserve(1).map_err(|_| Error::NoMem)?;
                frames.push(f);
            }
            b"ALPH" | b"VP8 " | b"VP8L" => return Err(bad("image outside a frame")),
            _ => {}
        }
        at = end + (size & 1);
    }
    if frames.is_empty() {
        return Err(bad("no frames"));
    }
    Ok(frames)
}

/// The host's composition over the background: `a * c + (1 - a) * bg` in single-precision floats, truncated.
fn over_bg(c: u8, a: f32, bg: i32) -> u32 {
    (a * f32::from(c) + (1.0 - a) * bg as f32) as i32 as u32
}

pub fn decode(data: &[u8], p: &Params, state: Option<Box<Anim>>) -> Result<Image, Error> {
    let keep = p.keep != 0;
    let mut anim = match state.map(|b| *b) {
        Some(Anim::Webp(w)) => Some(w),
        _ => None,
    };
    let mut still: Option<(Vec<u8>, bool, u32, u32)> = None;
    if anim.is_none() {
        // WebPGetFeatures: the RIFF header and the first chunk
        let vp8x = data.len() >= 30
            && &data[..4] == b"RIFF"
            && &data[8..12] == b"WEBP"
            && &data[12..16] == b"VP8X";
        if vp8x && data[20] & 0x02 != 0 {
            let (cw, ch) = (le24(&data[24..]) + 1, le24(&data[27..]) + 1);
            size_ok(p, i64::from(cw), i64::from(ch))?;
            let frames = demux(data, cw, ch)?;
            let n = cw as usize * ch as usize;
            let mut copy = Vec::new();
            copy.try_reserve_exact(data.len())
                .map_err(|_| Error::NoMem)?;
            copy.extend_from_slice(data);
            anim = Some(WebpAnim {
                data: copy,
                width: cw,
                height: ch,
                frames,
                curr: try_vec(n, [0; 4])?,
                prev_disposed: try_vec(n, [0; 4])?,
                prev: WebpFrame::default(),
                prev_was_key: false,
                next: 0,
                prev_timestamp: 0,
                pos: -1,
                has_last: false,
                duration: 0,
            });
        } else {
            let mut dec = WebPDecoder::new(Cursor::new(data)).map_err(|_| Error::NotImage)?;
            let (w, h) = dec.dimensions();
            size_ok(p, i64::from(w), i64::from(h))?;
            let size = dec.output_buffer_size().ok_or(Error::Size)?;
            let mut buf = try_vec(size, 0u8)?;
            dec.read_image(&mut buf)
                .map_err(|e| Error::Decode(e.to_string()))?;
            still = Some((buf, dec.has_alpha(), w, h));
        }
    }

    let (width, height, frame_count, duration);
    let decoded: &[u8];
    let alpha_blend;
    if let Some(e) = anim.as_mut() {
        let frames = e.frames.len() as i32;
        let idx = if p.frame_index >= 0 {
            p.frame_index.min(frames)
        } else {
            0
        };
        let mut dur = 0;
        let mut got = false;
        if e.pos == idx && e.has_last {
            got = true;
            dur = e.duration;
        } else {
            if e.pos >= idx {
                e.reset();
                e.pos = -1;
            }
            while e.pos < idx && e.next < e.frames.len() {
                // the host's own timestamp runs with the decoder's (both restart at 0)
                let before = e.prev_timestamp;
                let ts = e.get_next().map_err(|m| Error::Frame(e.pos + 1, m))?;
                dur = ts.wrapping_sub(before);
                e.pos += 1;
                got = true;
            }
            if got && keep {
                e.has_last = true;
                e.duration = dur;
            }
        }
        if !got {
            return Err(Error::Decode(String::new()));
        }
        width = e.width;
        height = e.height;
        frame_count = frames;
        duration = dur;
        decoded = e.curr.as_flattened();
        alpha_blend = true;
    } else {
        let (buf, has_alpha, w, h) = still.as_ref().expect("a still image");
        width = *w;
        height = *h;
        frame_count = 0;
        duration = 0;
        decoded = buf;
        alpha_blend = *has_alpha;
    }
    size_ok(p, i64::from(width), i64::from(height))?;
    let n = width as usize * height as usize;
    let mut pixels = CBuf::<u32>::zeroed(n)?;
    let (bgr, bgg, bgb) = if p.bg_set != 0 {
        (
            i32::from(p.bg_r >> 8),
            i32::from(p.bg_g >> 8),
            i32::from(p.bg_b >> 8),
        )
    } else {
        (0, 0, 0)
    };
    let step = if alpha_blend { 4 } else { 3 };
    for (px, q) in pixels
        .as_mut_slice()
        .iter_mut()
        .zip(decoded.chunks_exact(step))
    {
        *px = if alpha_blend {
            let a = f32::from(q[3]) / 255.0;
            (over_bg(q[0], a, bgr) << 16) | (over_bg(q[1], a, bgg) << 8) | over_bg(q[2], a, bgb)
        } else {
            (u32::from(q[0]) << 16) | (u32::from(q[1]) << 8) | u32::from(q[2])
        };
    }
    let mut img = Image::still(pixels, width as i32, height as i32);
    img.corners = Some([0, 0, height as i32, width as i32]);
    if let Some(e) = anim {
        img.animated = true;
        img.frame_count = frame_count;
        img.delay = Some(duration);
        img.anim = Some(Box::new(Anim::Webp(e)));
    }
    Ok(img)
}

#[cfg(test)]
mod tests {
    use super::super::anim::blend_non_premult;
    use super::*;
    use image_webp::{ColorType, WebPEncoder};

    /// A lossless still image of W×H RGBA pixels (the simple format: RIFF, VP8L).
    fn still(w: u32, h: u32, rgba: &[[u8; 4]]) -> Vec<u8> {
        let mut out = Vec::new();
        WebPEncoder::new(&mut out)
            .encode(rgba.as_flattened(), w, h, ColorType::Rgba8)
            .unwrap();
        out
    }

    /// The VP8L chunk's payload of a `still`.
    fn vp8l(w: u32, h: u32, rgba: &[[u8; 4]]) -> Vec<u8> {
        let f = still(w, h, rgba);
        assert_eq!(&f[12..16], b"VP8L");
        let n = le32(&f[16..]) as usize;
        f[20..20 + n].to_vec()
    }

    /// One frame of `animation`: x, y (even), w, h, duration, dispose to background, blend, pixels.
    type F<'a> = (u32, u32, u32, u32, u32, bool, bool, &'a [[u8; 4]]);

    /// An animated WebP of a CW×CH canvas, lossless frames.
    fn animation(cw: u32, ch: u32, frames: &[F<'_>]) -> Vec<u8> {
        let mut body = b"WEBP".to_vec();
        let chunk = |fourcc: &[u8], p: &[u8], out: &mut Vec<u8>| {
            out.extend_from_slice(fourcc);
            out.extend_from_slice(&(p.len() as u32).to_le_bytes());
            out.extend_from_slice(p);
            if p.len() % 2 == 1 {
                out.push(0);
            }
        };
        let mut vp8x = vec![0x10 | 0x02, 0, 0, 0];
        vp8x.extend_from_slice(&(cw - 1).to_le_bytes()[..3]);
        vp8x.extend_from_slice(&(ch - 1).to_le_bytes()[..3]);
        chunk(b"VP8X", &vp8x, &mut body);
        chunk(b"ANIM", &[0, 0, 0, 0, 0, 0], &mut body);
        for &(x, y, w, h, d, dispose, blend, px) in frames {
            let mut anmf = Vec::new();
            for v in [x / 2, y / 2, w - 1, h - 1, d] {
                anmf.extend_from_slice(&v.to_le_bytes()[..3]);
            }
            anmf.push(u8::from(dispose) | if blend { 0 } else { 2 });
            chunk(b"VP8L", &vp8l(w, h, px), &mut anmf);
            chunk(b"ANMF", &anmf, &mut body);
        }
        let mut f = b"RIFF".to_vec();
        f.extend_from_slice(&(body.len() as u32).to_le_bytes());
        f.extend_from_slice(&body);
        f
    }

    fn params(index: i32, bg: Option<[u16; 3]>) -> Params {
        let [r, g, b] = bg.unwrap_or_default();
        Params {
            frame_index: index,
            keep: 1,
            bg_set: u8::from(bg.is_some()),
            bg_r: r,
            bg_g: g,
            bg_b: b,
            ..Params::default()
        }
    }

    /// The host's formula, written out.
    fn host(c: [u8; 4], bg: [i32; 3]) -> u32 {
        let a = c[3] as f32 / 255.0;
        let ch = |k: usize| (a * c[k] as f32 + (1.0 - a) * bg[k] as f32) as i32 as u32;
        (ch(0) << 16) | (ch(1) << 8) | ch(2)
    }

    #[test]
    fn a_still_image_composites_over_the_background() {
        let px = [[200, 100, 50, 128], [1, 2, 3, 255], [9, 9, 9, 0]];
        let f = still(3, 1, &px);
        let img = decode(&f, &params(-1, Some([0xffff, 0x8000, 0x0100])), None).unwrap();
        let want: Vec<u32> = px.iter().map(|&c| host(c, [255, 128, 1])).collect();
        assert_eq!(img.pixels.as_slice(), &want[..]);
        assert_eq!(img.corners, Some([0, 0, 1, 3]));
        assert!(!img.animated && img.anim.is_none());
        // no background: over black
        let img = decode(&f, &params(-1, None), None).unwrap();
        assert_eq!(img.pixels.as_slice()[0], host(px[0], [0, 0, 0]));
    }

    #[test]
    fn frames_blend_and_dispose_as_webp_anim_decoder() {
        let (red, green, blue_half) = ([255, 0, 0, 255], [0, 255, 0, 255], [0, 0, 255, 128]);
        let clear = [7, 7, 7, 0];
        let f = animation(
            4,
            1,
            &[
                // key frame, whole canvas
                (0, 0, 4, 1, 100, false, false, &[red, red, green, green]),
                // blends over frame 0 (disposed: none); then disposed to the background
                (0, 0, 2, 1, 50, true, true, &[blue_half, clear]),
                // blends over the canvas with frame 1's rectangle cleared
                (
                    0,
                    0,
                    4,
                    1,
                    70,
                    false,
                    true,
                    &[blue_half, blue_half, blue_half, clear],
                ),
            ],
        );
        let p = |i| params(i, None);
        let f0 = decode(&f, &p(0), None).unwrap();
        assert!(f0.animated);
        assert_eq!((f0.frame_count, f0.delay), (3, Some(100)));
        let f1 = decode(&[], &p(1), f0.anim).unwrap();
        let c1 = [
            blend_non_premult(blue_half, red),
            blend_non_premult(clear, red),
            green,
            green,
        ];
        let want1: Vec<u32> = c1.iter().map(|&c| host(c, [0, 0, 0])).collect();
        assert_eq!(f1.pixels.as_slice(), &want1[..]);
        assert_eq!(f1.delay, Some(50));
        let f2 = decode(&[], &p(2), f1.anim).unwrap();
        // pixels 0-1: frame 1 disposed to transparent, not blended (FindBlendRangeAtRow); 2: blended; 3: kept
        let c2 = [
            blue_half,
            blue_half,
            blend_non_premult(blue_half, green),
            green,
        ];
        let want2: Vec<u32> = c2.iter().map(|&c| host(c, [0, 0, 0])).collect();
        assert_eq!(f2.pixels.as_slice(), &want2[..]);
        // a fresh decode of frame 2 agrees with the continued one
        let fresh = decode(&f, &p(2), None).unwrap();
        assert_eq!(fresh.pixels.as_slice(), f2.pixels.as_slice());
    }

    #[test]
    fn the_hosts_frame_index_rules() {
        let px: &[[u8; 4]] = &[[1, 2, 3, 255]];
        let f = animation(
            2,
            2,
            &[
                (0, 0, 1, 1, 10, false, true, px),
                (0, 0, 2, 2, 20, false, true, &[[4, 5, 6, 255]; 4]),
            ],
        );
        // past the last frame: the last one
        let last = decode(&f, &params(9, None), None).unwrap();
        assert_eq!(last.pixels.as_slice(), &[0x040506; 4]);
        // asked again from the state, the host's decoder has nothing left to give
        assert!(matches!(
            decode(&[], &params(9, None), last.anim),
            Err(Error::Decode(_))
        ));
        // the same frame again from the state: its kept canvas
        let a = decode(&f, &params(1, None), None).unwrap();
        let b = decode(&[], &params(1, None), a.anim).unwrap();
        assert_eq!(b.pixels.as_slice(), &[0x040506; 4]);
        // an earlier frame: restarted
        let c = decode(&[], &params(0, None), b.anim).unwrap();
        assert_eq!(c.pixels.as_slice()[0], 0x010203);
        assert_eq!(c.pixels.as_slice()[3], 0);
    }

    #[test]
    fn the_abi_survives_mutated_input() {
        let px: &[[u8; 4]] = &[[1, 2, 3, 255], [9, 8, 7, 100]];
        let seeds = [
            still(2, 1, px),
            animation(
                4,
                2,
                &[
                    (0, 0, 2, 1, 10, true, true, px),
                    (2, 0, 1, 2, 20, false, true, px),
                ],
            ),
        ];
        super::super::fuzz_abi(super::super::GLITNIR_IMG_WEBP, &seeds, 10_000);
    }

    #[test]
    fn broken_files_fail_without_panicking() {
        let px: &[[u8; 4]] = &[[1, 2, 3, 255]];
        let f = animation(2, 1, &[(0, 0, 1, 1, 10, false, true, px)]);
        assert!(matches!(
            decode(b"RIFF\0\0\0\0WEBPjunk", &params(0, None), None),
            Err(Error::NotImage)
        ));
        assert!(matches!(
            decode(&f[..f.len() - 3], &params(0, None), None),
            Err(Error::Parse(_))
        ));
        // a frame outside the canvas
        let out = animation(2, 1, &[(2, 0, 1, 1, 10, false, true, px)]);
        assert!(matches!(
            decode(&out, &params(0, None), None),
            Err(Error::Parse(_))
        ));
        // a frame whose image does not decode
        let mut bad = f.clone();
        let n = bad.len();
        bad[n - 2] ^= 0xff;
        bad[n - 3] ^= 0xff;
        // whatever it gives, it gives without panicking
        let _ = decode(&bad, &params(0, None), None);
        let mut p = params(0, None);
        p.limit_w = 1;
        assert!(matches!(decode(&f, &p, None), Err(Error::Size)));
        let s = still(2, 1, &[[0; 4]; 2]);
        for cut in [13, 20, s.len() - 1] {
            assert!(
                decode(&s[..cut], &params(-1, None), None).is_err(),
                "cut {cut}"
            );
        }
    }
}

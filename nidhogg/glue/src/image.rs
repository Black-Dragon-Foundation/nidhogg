//! Images on macOS (Naglfar, design §5.5): `image.c`'s native route decodes through ImageIO in the shell
//! (`nhshell::macos::images`) into the plain container `image.c` keeps (`dispextern.h`): 32-bit pixels whose colour is
//! not premultiplied, and an 8-bit mask holding the alpha when the image has any.  Drawing turns a container (and its
//! mask) into premultiplied words as Cairo's `cr_create_surface_from_pix_containers` does, wrapped once as a
//! `CGImage`.  The conversions are platform-free and tested on Linux.

/// A decoded image as C takes it (`NhgImage`): `nhg_image_take` copies the pixels into the containers and frees them.
#[repr(C)]
#[derive(Debug)]
pub struct NhgImage {
    pub width: i32,
    pub height: i32,
    pub frames: u32,
    /// 1 when some pixel is not opaque: the image gets a mask.
    pub has_alpha: u32,
    /// Seconds between frames; 0 for a still image.
    pub delay: f64,
    /// The glue's: the pixels and their count.
    pub argb: *mut u32,
    pub len: usize,
}

/// A container as drawing reads it (`NhgPix`): `data` rows `stride` bytes apart, `bpp` 32 or 8; `mask` (null: none)
/// 8-bit rows `mask_stride` bytes apart.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct NhgPix {
    pub data: *const u8,
    pub mask: *const u8,
    pub width: i32,
    pub height: i32,
    pub stride: i32,
    pub mask_stride: i32,
    pub bpp: i32,
}

/// The image types the native route takes.
pub fn supported(name: &str) -> bool {
    matches!(
        name,
        "native-image" | "png" | "jpeg" | "gif" | "tiff" | "webp" | "heic"
    )
}

/// Premultiplied ARGB `argb` made straight (each colour divided by its alpha), in place; whether any pixel is not
/// opaque.
pub fn unpremultiply(argb: &mut [u32]) -> bool {
    let mut any = false;
    for p in argb.iter_mut() {
        let a = *p >> 24;
        if a == 255 {
            continue;
        }
        any = true;
        if a == 0 {
            *p = 0;
            continue;
        }
        let ch = |s: u32| ((((*p >> s) & 0xff) * 255 + a / 2) / a).min(255);
        *p = a << 24 | ch(16) << 16 | ch(8) << 8 | ch(0);
    }
    any
}

/// `c` times `alpha`, rounded as Cairo's container conversion rounds.
fn times(c: u32, alpha: u32) -> u32 {
    (c * alpha + 0x7f) / 0xff
}

/// The premultiplied words Core Graphics draws for the container `pix` (`data` and `mask` are its bytes): with a mask,
/// each colour times the mask's alpha; without one, opaque (Cairo's RGB24); an 8-bit container is `fg` through it.
/// `None` when the bytes are too short for the sizes.
pub fn combine(pix: &NhgPix, data: &[u8], mask: Option<&[u8]>, fg: u32) -> Option<Vec<u32>> {
    let (w, h, stride) = (pix.width as usize, pix.height as usize, pix.stride as usize);
    let bytes = if pix.bpp == 8 { 1 } else { 4 };
    if pix.width < 0
        || pix.height < 0
        || stride < w * bytes
        || data.len() < stride * h.saturating_sub(1) + w * bytes
    {
        return None;
    }
    let ms = pix.mask_stride as usize;
    if let Some(m) = mask
        && (ms < w || m.len() < ms * h.saturating_sub(1) + w)
    {
        return None;
    }
    let mut out = Vec::with_capacity(w * h);
    for y in 0..h {
        for x in 0..w {
            let px = if bytes == 1 {
                let a = u32::from(data[y * stride + x]);
                a << 24
                    | times((fg >> 16) & 0xff, a) << 16
                    | times((fg >> 8) & 0xff, a) << 8
                    | times(fg & 0xff, a)
            } else {
                let o = y * stride + x * 4;
                let c = u32::from_ne_bytes([data[o], data[o + 1], data[o + 2], data[o + 3]]);
                match mask {
                    Some(m) => {
                        let a = u32::from(m[y * ms + x]);
                        a << 24
                            | times((c >> 16) & 0xff, a) << 16
                            | times((c >> 8) & 0xff, a) << 8
                            | times(c & 0xff, a)
                    }
                    None => 0xff00_0000 | (c & 0xff_ffff),
                }
            };
            out.push(px);
        }
    }
    Some(out)
}

/// Frame `index` of the image in `bytes` (`None`: the route cannot read it, or there is no such frame).
pub fn decode(bytes: &[u8], index: u32) -> Option<NhgImage> {
    #[cfg(all(feature = "macos", target_os = "macos"))]
    {
        let d = nhshell::macos::images::decode(bytes, index)?;
        let mut argb = d.argb.into_boxed_slice();
        let has_alpha = unpremultiply(&mut argb);
        let len = argb.len();
        Some(NhgImage {
            width: d.width,
            height: d.height,
            frames: d.frames,
            has_alpha: u32::from(has_alpha),
            delay: d.delay.unwrap_or(0.0),
            argb: Box::into_raw(argb).cast(),
            len,
        })
    }
    #[cfg(not(all(feature = "macos", target_os = "macos")))]
    {
        let _ = (bytes, index);
        None
    }
}

/// Free what `decode` made.
///
/// # Safety
/// `img` came from `decode` and is freed once.
pub unsafe fn free(img: &mut NhgImage) {
    if !img.argb.is_null() {
        // SAFETY: the box `decode` leaked, with its length.
        drop(unsafe { Box::from_raw(std::ptr::slice_from_raw_parts_mut(img.argb, img.len)) });
        img.argb = std::ptr::null_mut();
        img.len = 0;
    }
}

/// `img`'s pixels into the containers' bytes: `data` (32-bit, `width * 4` bytes a row) and, when it has alpha, `mask`
/// (8-bit, `width` bytes a row).
pub fn store(argb: &[u32], data: &mut [u8], mask: Option<&mut [u8]>) {
    for (i, p) in argb.iter().enumerate() {
        data[i * 4..i * 4 + 4].copy_from_slice(&p.to_ne_bytes());
    }
    if let Some(m) = mask {
        for (i, p) in argb.iter().enumerate() {
            m[i] = (p >> 24) as u8;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pix(w: i32, h: i32, bpp: i32, mask: bool) -> NhgPix {
        NhgPix {
            data: std::ptr::null(),
            mask: std::ptr::null(),
            width: w,
            height: h,
            stride: w * bpp / 8,
            mask_stride: if mask { w } else { 0 },
            bpp,
        }
    }

    #[test]
    fn decoded_pixels_come_straight_with_their_alpha_kept() {
        let mut px = [0xff10_2030, 0x8040_2010, 0, 0x0101_0101];
        assert!(unpremultiply(&mut px));
        assert_eq!(px, [0xff10_2030, 0x8080_4020, 0, 0x01ff_ffff]);
        let mut opaque = [0xff00_0000, 0xffff_ffff];
        assert!(!unpremultiply(&mut opaque));
    }

    #[test]
    fn a_container_without_a_mask_is_opaque() {
        let words: Vec<u8> = [0x0012_3456u32, 0x7f65_4321]
            .iter()
            .flat_map(|w| w.to_ne_bytes())
            .collect();
        let got = combine(&pix(2, 1, 32, false), &words, None, 0).unwrap();
        assert_eq!(got, vec![0xff12_3456, 0xff65_4321]);
    }

    #[test]
    fn a_mask_premultiplies_as_cairo_does() {
        let words: Vec<u8> = [0x00ff_8000u32, 0x00ff_ffff]
            .iter()
            .flat_map(|w| w.to_ne_bytes())
            .collect();
        let got = combine(&pix(2, 1, 32, true), &words, Some(&[0x80, 0]), 0).unwrap();
        // 255 * 128 + 127 = 32767, / 255 = 128; 128 * 128 + 127 = 16511, / 255 = 64
        assert_eq!(got, vec![0x8080_4000, 0]);
    }

    #[test]
    fn a_bitmap_is_the_foreground_through_it() {
        let got = combine(&pix(2, 1, 8, false), &[0xff, 0x80], None, 0x00ff_0000).unwrap();
        assert_eq!(got, vec![0xffff_0000, 0x8080_0000]);
    }

    #[test]
    fn short_buffers_and_strides_are_refused() {
        assert!(combine(&pix(2, 2, 32, false), &[0; 12], None, 0).is_none());
        let mut p = pix(2, 1, 32, false);
        p.stride = 4;
        assert!(combine(&p, &[0; 8], None, 0).is_none());
        assert!(combine(&pix(2, 1, 32, true), &[0; 8], Some(&[0]), 0).is_none());
    }

    #[test]
    fn store_round_trips_through_combine() {
        let mut argb = [0x8040_2010u32, 0xff11_2233];
        let alpha = unpremultiply(&mut argb);
        let mut data = [0u8; 8];
        let mut mask = [0u8; 2];
        store(&argb, &mut data, alpha.then_some(&mut mask[..]));
        let back = combine(&pix(2, 1, 32, true), &data, Some(&mask), 0).unwrap();
        assert_eq!(back, vec![0x8040_2010, 0xff11_2233]);
    }

    #[test]
    fn the_route_takes_imageios_types() {
        for t in ["png", "jpeg", "gif", "tiff", "webp", "heic", "native-image"] {
            assert!(supported(t), "{t}");
        }
        assert!(!supported("xpm"));
        assert!(!supported("svg"));
    }
}

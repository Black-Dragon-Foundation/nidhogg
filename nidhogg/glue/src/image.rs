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
/// 8-bit rows `mask_stride` bytes apart; the image's `matrix` (`image_set_transform`'s, from a drawn pixel to the
/// container's; all zero: none) and whether drawing it `smooth`s.
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
    pub smooth: i32,
    pub matrix: Matrix,
}

/// The colour a name gives (`0xRRGGBB`), `image.c`'s: called with its context and a NUL-terminated name.
pub type NhgColorFn =
    Option<unsafe extern "C" fn(*mut std::ffi::c_void, *const std::ffi::c_char) -> u32>;

/// How `image.c` asks for an SVG (`NhgSvgStyle`): its `:css`, the face's font family, `:base-uri` (each `len` bytes;
/// null or empty: none), the face's font size and the display's resolution, and the colours (`0xRRGGBB`).
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct NhgSvgStyle {
    pub css: *const u8,
    pub css_len: usize,
    pub family: *const u8,
    pub family_len: usize,
    pub base: *const u8,
    pub base_len: usize,
    pub font_size: f64,
    pub dpi: f64,
    pub foreground: u32,
    pub background: u32,
}

/// The `len` bytes at `p` as text (lossy), empty for null.
///
/// # Safety
/// `p` is null or holds `len` readable bytes.
#[cfg(feature = "svg")]
unsafe fn text(p: *const u8, len: usize) -> String {
    if p.is_null() {
        return String::new();
    }
    // SAFETY: the caller's promise.
    String::from_utf8_lossy(unsafe { std::slice::from_raw_parts(p, len) }).into_owned()
}

/// The shell's style for `s`; references resolve against `:base-uri`, else the directory of `file`.
///
/// # Safety
/// `s`'s strings hold their lengths.
#[cfg(feature = "svg")]
pub unsafe fn svg_style(s: &NhgSvgStyle, file: Option<&std::path::Path>) -> nhshell::svg::Style {
    // SAFETY: the caller's promise.
    let (css, font_family, base) = unsafe {
        (
            text(s.css, s.css_len),
            text(s.family, s.family_len),
            text(s.base, s.base_len),
        )
    };
    let base = if base.is_empty() {
        file.and_then(|f| f.parent())
            .map(std::path::Path::to_path_buf)
    } else {
        // a file: URI or a path; the directory of what it names
        let p = std::path::PathBuf::from(base.strip_prefix("file://").unwrap_or(&base));
        Some(if p.is_dir() {
            p
        } else {
            p.parent()
                .map(std::path::Path::to_path_buf)
                .unwrap_or_default()
        })
    };
    nhshell::svg::Style {
        css,
        font_family,
        font_size: s.font_size as f32,
        dpi: s.dpi as f32,
        foreground: s.foreground,
        base,
    }
}

/// An opaque rendered image as C takes it (no alpha, one frame).
pub fn opaque(width: u32, height: u32, argb: Vec<u32>) -> NhgImage {
    boxed(width as i32, height as i32, 1, false, 0.0, argb)
}

/// The image types the native route takes.
pub fn supported(name: &str) -> bool {
    matches!(
        name,
        "native-image" | "png" | "jpeg" | "gif" | "tiff" | "webp" | "heic" | "xpm"
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

/// Cairo's pattern matrix `(xx, yx, xy, yy, x0, y0)`.
pub type Matrix = [f64; 6];
/// A matrix and the drawn width and height.
pub type Transformed = (Matrix, f64, f64);

/// `a` times `b`, Emacs's `matrix3x3_mult` (row vectors: a point is `[x y 1]`, translations in row 2).
fn mult(a: &[[f64; 3]; 3], b: &[[f64; 3]; 3]) -> [[f64; 3]; 3] {
    let mut r = [[0.0; 3]; 3];
    for (i, row) in r.iter_mut().enumerate() {
        for (j, v) in row.iter_mut().enumerate() {
            *v = (0..3).map(|k| a[i][k] * b[k][j]).sum();
        }
    }
    r
}

/// The matrix `image_set_transform` makes on Cairo, as Cairo's pattern matrix `(xx, yx, xy, yy, x0, y0)` from a drawn
/// pixel to the container's: an image `w` x `h` shown at `width` x `height`, turned by `rotation` degrees and flipped;
/// and the drawn size.  `Err` holds the scale alone when the rotation is not a multiple of 90 degrees (Emacs reports
/// it).
pub fn transform(
    w: f64,
    h: f64,
    width: f64,
    height: f64,
    rotation: f64,
    flip: bool,
) -> Result<Transformed, Matrix> {
    let mut m = [
        [w / width, 0.0, 0.0],
        [0.0, h / height, 0.0],
        [0.0, 0.0, 1.0],
    ];
    let six = |m: &[[f64; 3]; 3]| [m[0][0], m[0][1], m[1][0], m[1][1], m[2][0], m[2][1]];
    if rotation == 0.0 && !flip {
        return Ok((six(&m), width, height));
    }
    let (cos, sin, dw, dh) = if rotation == 0.0 {
        (1.0, 0.0, width, height)
    } else if rotation == 90.0 {
        (0.0, 1.0, height, width)
    } else if rotation == 180.0 {
        (-1.0, 0.0, width, height)
    } else if rotation == 270.0 {
        (0.0, -1.0, height, width)
    } else {
        return Err(six(&m));
    };
    // 1. (0, 0) to the centre of the image; 2. the turn; 3. back, flipped horizontally if asked
    let mut t = [
        [1.0, 0.0, 0.0],
        [0.0, 1.0, 0.0],
        [width * 0.5, height * 0.5, 1.0],
    ];
    let u = mult(&t, &m);
    let rot = [[cos, -sin, 0.0], [sin, cos, 0.0], [0.0, 0.0, 1.0]];
    let v = mult(&rot, &u);
    t[2] = [dw * -0.5, dh * -0.5, 1.0];
    if flip {
        t[0][0] = -t[0][0];
        t[2][0] = -t[2][0];
    }
    m = mult(&t, &v);
    Ok((six(&m), dw, dh))
}

/// The map from a container pixel to the drawn image's, `matrix` inverted (`Shape::Image`'s `to`); the identity for an
/// image with none (all zero) or one that does not invert.
pub fn drawn(matrix: &Matrix) -> Matrix {
    let [xx, yx, xy, yy, x0, y0] = *matrix;
    let det = xx * yy - xy * yx;
    if det == 0.0 || !det.is_finite() {
        return [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];
    }
    let (a, b, c, d) = (yy / det, -yx / det, -xy / det, xx / det);
    [a, b, c, d, -(a * x0 + c * y0), -(b * x0 + d * y0)]
}

/// Straight ARGB words handed to C.
fn boxed(
    width: i32,
    height: i32,
    frames: u32,
    has_alpha: bool,
    delay: f64,
    argb: Vec<u32>,
) -> NhgImage {
    let argb = argb.into_boxed_slice();
    let len = argb.len();
    NhgImage {
        width,
        height,
        frames,
        has_alpha: u32::from(has_alpha),
        delay,
        argb: Box::into_raw(argb).cast(),
        len,
    }
}

/// Frame `index` of the image in `bytes` (`None`: the route cannot read it, or there is no such frame).  An XPM's
/// colour names go through `resolve` (`0xRRGGBB`).
pub fn decode(bytes: &[u8], index: u32, resolve: &mut dyn FnMut(&str) -> u32) -> Option<NhgImage> {
    if crate::xpm::is_xpm(bytes) {
        let (w, h, argb, alpha) = crate::xpm::decode(bytes, resolve)?;
        return (index == 0).then(|| boxed(w as i32, h as i32, 1, alpha, 0.0, argb));
    }
    #[cfg(all(feature = "macos", target_os = "macos"))]
    {
        let d = nhshell::macos::images::decode(bytes, index)?;
        let mut argb = d.argb;
        let has_alpha = unpremultiply(&mut argb);
        Some(boxed(
            d.width,
            d.height,
            d.frames,
            has_alpha,
            d.delay.unwrap_or(0.0),
            argb,
        ))
    }
    #[cfg(not(all(feature = "macos", target_os = "macos")))]
    {
        let _ = index;
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
            smooth: 0,
            matrix: [0.0; 6],
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
        for t in [
            "png",
            "jpeg",
            "gif",
            "tiff",
            "webp",
            "heic",
            "xpm",
            "native-image",
        ] {
            assert!(supported(t), "{t}");
        }
        // SVG has a type of its own (image.c's keywords); XBM and PBM are Emacs's
        for t in ["svg", "xbm", "pbm"] {
            assert!(!supported(t), "{t}");
        }
    }

    #[test]
    fn an_xpm_decodes_on_every_platform_through_the_resolver() {
        let xpm = br#"/* XPM */ static char *x[] = { "2 1 2 1", "a c None", "b c red", "ab" };"#;
        let mut asked = Vec::new();
        let mut img = decode(xpm, 0, &mut |n| {
            asked.push(n.to_string());
            0x00ff_0000
        })
        .unwrap();
        assert_eq!(
            (img.width, img.height, img.frames, img.has_alpha),
            (2, 1, 1, 1)
        );
        let (mut data, mut mask) = ([0u8; 8], [0u8; 2]);
        // SAFETY: the decode's pixels, read once.
        store(
            unsafe { std::slice::from_raw_parts(img.argb, img.len) },
            &mut data,
            Some(&mut mask),
        );
        // SAFETY: freed once.
        unsafe { free(&mut img) };
        assert_eq!(asked, vec!["red"]);
        assert_eq!(mask, [0, 255]);
        assert_eq!(
            u32::from_ne_bytes(data[4..].try_into().unwrap()),
            0xffff_0000
        );
        assert!(decode(xpm, 1, &mut |_| 0).is_none());
    }

    /// Where the matrix takes a drawn pixel in the container.
    fn at(m: &Matrix, x: f64, y: f64) -> (f64, f64) {
        (m[0] * x + m[2] * y + m[4], m[1] * x + m[3] * y + m[5])
    }

    #[test]
    fn a_scale_maps_the_drawn_size_onto_the_container() {
        let (m, w, h) = transform(10.0, 20.0, 20.0, 40.0, 0.0, false).unwrap();
        assert_eq!((w, h), (20.0, 40.0));
        assert_eq!(at(&m, 20.0, 40.0), (10.0, 20.0));
        assert_eq!(at(&m, 0.0, 0.0), (0.0, 0.0));
    }

    #[test]
    fn quarter_turns_and_flips_turn_the_corners_as_cairo_does() {
        // 90 degrees clockwise: the drawn top left is the container's bottom left, and the size swaps
        let (m, w, h) = transform(10.0, 20.0, 10.0, 20.0, 90.0, false).unwrap();
        assert_eq!((w, h), (20.0, 10.0));
        assert_eq!(at(&m, 0.0, 0.0), (0.0, 20.0));
        assert_eq!(at(&m, 20.0, 10.0), (10.0, 0.0));
        // 180: corners swap diagonally
        let (m, _, _) = transform(10.0, 20.0, 10.0, 20.0, 180.0, false).unwrap();
        assert_eq!(at(&m, 0.0, 0.0), (10.0, 20.0));
        // a flip alone mirrors x
        let (m, w, h) = transform(10.0, 20.0, 10.0, 20.0, 0.0, true).unwrap();
        assert_eq!((w, h), (10.0, 20.0));
        assert_eq!(at(&m, 0.0, 0.0), (10.0, 0.0));
        assert_eq!(at(&m, 10.0, 20.0), (0.0, 20.0));
        // scaled and turned 270: the drawn top left is the container's top right
        let (m, w, h) = transform(10.0, 20.0, 5.0, 10.0, 270.0, false).unwrap();
        assert_eq!((w, h), (10.0, 5.0));
        assert_eq!(at(&m, 0.0, 0.0), (10.0, 0.0));
    }

    #[test]
    fn drawing_inverts_the_matrix_and_draws_an_untransformed_image_as_it_is() {
        assert_eq!(drawn(&[0.0; 6]), [1.0, 0.0, 0.0, 1.0, 0.0, 0.0]);
        let (m, _, _) = transform(10.0, 20.0, 10.0, 20.0, 90.0, false).unwrap();
        let to = drawn(&m);
        // the container's bottom left is the drawn top left, and its top right the drawn bottom right
        let p = |x: f64, y: f64| (to[0] * x + to[2] * y + to[4], to[1] * x + to[3] * y + to[5]);
        assert_eq!(p(0.0, 20.0), (0.0, 0.0));
        assert_eq!(p(10.0, 0.0), (20.0, 10.0));
        let (m, _, _) = transform(10.0, 20.0, 20.0, 40.0, 0.0, false).unwrap();
        assert_eq!(drawn(&m), [2.0, 0.0, 0.0, 2.0, 0.0, 0.0]);
    }

    #[test]
    fn another_angle_keeps_the_scale_and_is_refused() {
        assert_eq!(
            transform(10.0, 10.0, 5.0, 5.0, 45.0, false),
            Err([2.0, 0.0, 0.0, 2.0, 0.0, 0.0])
        );
    }
}

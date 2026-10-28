//! libpng's alpha composition over a background, with its rounding.
//!
//! `over8` is `png_composite` as libpng's default configuration builds it (`READ_COMPOSITE_NODIV`), `over16` is
//! `png_composite_16`. Both compose a straight (not premultiplied) sample with its alpha over an opaque background
//! sample, at gamma 1.0.

/// `png_composite (fg, alpha, bg)`: round((fg·a + bg·(255 − a)) / 255) without a division.
pub fn over8(fg: u8, alpha: u8, bg: u8) -> u8 {
    let t = u32::from(fg) * u32::from(alpha) + u32::from(bg) * (255 - u32::from(alpha)) + 128;
    ((t + (t >> 8)) >> 8) as u8
}

/// `png_composite_16 (fg, alpha, bg)`: (fg·a + bg·(65535 − a) + 32767) / 65535.
pub fn over16(fg: u16, alpha: u16, bg: u16) -> u16 {
    let t = u64::from(fg) * u64::from(alpha) + u64::from(bg) * (65535 - u64::from(alpha)) + 32767;
    (t / 65535) as u16
}

#[cfg(test)]
mod tests {
    use super::*;

    /// libpng's two spellings of `png_composite`, transcribed: with `READ_COMPOSITE_NODIV` (the default
    /// configuration) and without it.
    fn libpng_nodiv(fg: u8, alpha: u8, bg: u8) -> u8 {
        let temp: u16 = (fg as u16) * (alpha as u16) + (bg as u16) * (255 - alpha as u16) + 128;
        (((temp as u32 + ((temp as u32) >> 8)) >> 8) & 0xff) as u8
    }

    fn libpng_div(fg: u8, alpha: u8, bg: u8) -> u8 {
        (0xff & (((fg as u16) * (alpha as u16) + (bg as u16) * (255 - alpha as u16) + 127) / 255))
            as u8
    }

    #[test]
    fn png_composite_matches_libpng() {
        // every foreground, alpha and background byte: 2^24 cases
        for a in 0..=255u8 {
            for bg in 0..=255u8 {
                for fg in 0..=255u8 {
                    let want = libpng_nodiv(fg, a, bg);
                    assert_eq!(over8(fg, a, bg), want, "fg {fg} a {a} bg {bg}");
                    assert_eq!(libpng_div(fg, a, bg), want, "fg {fg} a {a} bg {bg}");
                }
            }
        }
    }

    #[test]
    fn png_composite_16_matches_libpng() {
        fn libpng(fg: u16, alpha: u16, bg: u16) -> u16 {
            (0xffff
                & ((fg as u64 * alpha as u64 + bg as u64 * (65535 - alpha as u64) + 32767) / 65535))
                as u16
        }
        let samples = [
            0u16, 1, 2, 127, 128, 255, 256, 257, 32767, 32768, 65279, 65534, 65535,
        ];
        let mut steps: Vec<u16> = (0..=65535u32).step_by(251).map(|v| v as u16).collect();
        steps.extend_from_slice(&samples);
        for &a in &steps {
            for &bg in &samples {
                for &fg in &steps {
                    assert_eq!(
                        over16(fg, a, bg),
                        libpng(fg, a, bg),
                        "fg {fg} a {a} bg {bg}"
                    );
                }
            }
        }
        assert_eq!(over16(1234, 0, 999), 999);
        assert_eq!(over16(1234, 65535, 999), 1234);
    }
}

//! The glue's pure functions that were C, pinned to the results the C gave (its arithmetic and rounding, worked by
//! hand from the C that was moved).  No display, no recorder.

mod keys {
    use nhglue::keys::{NHG_KEY_CHAR, NHG_KEY_SYMBOL, NHG_KEY_TEXT, classify, string_char};

    /// Latin-1 32–127 and Unicode keysyms name their character; the rest of Latin-1 does not.
    #[test]
    fn character_keysyms() {
        assert_eq!(classify(0x20, b" "), (NHG_KEY_CHAR, 0x20));
        assert_eq!(classify(0x7f, b""), (NHG_KEY_CHAR, 0x7f));
        assert_eq!(classify(u32::from(b'a'), b"a"), (NHG_KEY_CHAR, 0x61));
        // XK_eacute: the table's, else the text's
        assert_eq!(classify(0xe9, "é".as_bytes()), (NHG_KEY_TEXT, 0xe9));
        // U+0041 and U+20AC as Unicode keysyms; the ASCII boundary is C's 0x01000080
        assert_eq!(classify(0x0100_0041, b"A"), (NHG_KEY_CHAR, 0x41));
        assert_eq!(classify(0x0100_007f, b""), (NHG_KEY_CHAR, 0x7f));
        assert_eq!(
            classify(0x0100_20ac, "€".as_bytes()),
            (NHG_KEY_CHAR, 0x20ac)
        );
        assert_eq!(classify(0x0110_ffff, b""), (NHG_KEY_CHAR, 0x10_ffff));
        // just past the Unicode range: a key without text is a symbol
        assert_eq!(classify(0x0111_0000, b""), (NHG_KEY_SYMBOL, 0));
        assert_eq!(classify(0x1f, b""), (NHG_KEY_SYMBOL, 0));
    }

    /// Function keys and keysyms without text are symbols; modifiers never are; `0xffffff` without text is nothing.
    #[test]
    fn symbols_and_modifiers() {
        for k in [
            0xff08,
            0xff1b,
            0xffff,
            0xfe20,
            0xfe34,
            0xff50,
            0xff5f,
            0xff60,
            0xff6b,
            0xfe50,
            0xfe5c,
            0xff80,
            0xffbd,
            0xffbe,
            0xffe0,
            0x1000_ff00,
        ] {
            assert_eq!(classify(k, b"x"), (NHG_KEY_SYMBOL, 0), "{k:#x}");
        }
        // outside the ranges, with text: the text's character
        for k in [
            0xff07, 0xff1c, 0xfe1f, 0xfe35, 0xff6c, 0xfe4f, 0xfe5d, 0xff7d,
        ] {
            assert_eq!(classify(k, b"x"), (NHG_KEY_TEXT, 0x78), "{k:#x}");
        }
        // modifiers (and ISO_Lock .. ISO_Last_Group_Lock, Mode_switch, Num_Lock): nothing, even vendor-flagged
        for k in [0xffe1, 0xffee, 0xfe01, 0xfe13, 0xff7e, 0xff7f] {
            assert_eq!(classify(k, b""), (0, 0), "{k:#x}");
            assert_eq!(
                classify(k | (1 << 28), b""),
                (NHG_KEY_SYMBOL, 0),
                "{k:#x} vendor"
            );
        }
        assert_eq!(classify(0xffe1, b"x"), (NHG_KEY_TEXT, 0x78));
        assert_eq!(classify(0xff_ffff, b""), (0, 0));
        assert_eq!(classify(0xff_ffff, b"x"), (NHG_KEY_TEXT, 0x78));
        assert_eq!(classify(0x0, b""), (NHG_KEY_SYMBOL, 0));
    }

    /// `string_char_and_length` on UTF-8, and on Emacs's raw bytes (an overlong two-byte lead).
    #[test]
    fn text_characters() {
        assert_eq!(string_char(b"a"), 0x61);
        assert_eq!(string_char("é".as_bytes()), 0xe9);
        assert_eq!(string_char("€x".as_bytes()), 0x20ac);
        assert_eq!(string_char("😀".as_bytes()), 0x1f600);
        // C0 80: the raw byte 0x80 (0x3FFF80 + 0x00)
        assert_eq!(string_char(&[0xc0, 0x80]), 0x3f_ff80);
        assert_eq!(string_char(&[0xc1, 0xbf]), 0x3f_ffff);
        // Emacs's five-byte form: F8 88 80 80 80 is 0x200000
        assert_eq!(string_char(&[0xf8, 0x88, 0x80, 0x80, 0x80]), 0x20_0000);
        // a cut sequence reads the NUL after it
        assert_eq!(string_char(&[0xc3]), (0xc3 << 6) - ((0xc0 << 6) + 0x80));
    }
}

mod pointer {
    use nhglue::pointer::button_code;

    #[test]
    fn evdev_buttons() {
        let want = [
            (0x110, 0),
            (0x112, 1),
            (0x111, 2),
            (0x113, 7),
            (0x116, 7),
            (0x114, 8),
            (0x115, 8),
            (0x117, -1),
            (0x10f, -1),
            (0, -1),
        ];
        for (b, code) in want {
            assert_eq!(button_code(b), code, "{b:#x}");
        }
    }
}

mod select {
    use nhglue::select::pick_at;

    const TYPES: &[u8] = b"text/plain\0UTF8_STRING\0image/png\0";
    const TEXT: [&[u8]; 5] = [
        b"text/plain;charset=utf-8",
        b"UTF8_STRING",
        b"text/plain",
        b"STRING",
        b"TEXT",
    ];

    /// The target's own name first, wherever the offer has it; then the text types in their order of preference,
    /// not the offer's; whole names only.
    #[test]
    fn mime_choice() {
        assert_eq!(pick_at(TYPES, b"image/png", &TEXT), Some(23));
        assert_eq!(pick_at(TYPES, b"text/plain", &[]), Some(0));
        assert_eq!(pick_at(TYPES, b"STRING", &TEXT), Some(11));
        assert_eq!(pick_at(b"TEXT\0text/plain\0", b"STRING", &TEXT), Some(5));
        assert_eq!(pick_at(TYPES, b"STRING", &[]), None);
        assert_eq!(pick_at(TYPES, b"text", &[]), None);
        assert_eq!(pick_at(TYPES, b"", &[]), None);
        assert_eq!(pick_at(b"", b"TEXT", &TEXT), None);
        // an offered empty type is a type
        assert_eq!(pick_at(b"a\0\0", b"", &[]), Some(2));
    }
}

mod colours {
    use nhglue::backing::premultiply;
    use nhglue::csd::title_bar_mix;

    /// W0's mix: each channel in double, truncated; (inactive foreground, background).
    #[test]
    fn title_bar_mix_truncates() {
        assert_eq!(title_bar_mix(0xffffff, 0x000000), (0x999999, 0x1e1e1e));
        assert_eq!(title_bar_mix(0x000000, 0xffffff), (0x646464, 0xe0e0e0));
        // 255 * 0.88 + 255 * 0.12 is 255 in double, not 254
        assert_eq!(title_bar_mix(0xffffff, 0xffffff), (0xffffff, 0xffffff));
        assert_eq!(title_bar_mix(0x336699, 0xf0e0d0), (0x7d96ae, 0xd9d1c9));
        assert_eq!(title_bar_mix(0xc8c8c8, 0x646464), (0xa0a0a0, 0x707070));
        // only the low 24 bits are colour
        assert_eq!(
            title_bar_mix(0xff00_0000 | 0x336699, 0xff00_0000 | 0xf0e0d0),
            (0x7d96ae, 0xd9d1c9)
        );
    }

    /// Cairo 1.18's solid SOURCE paint on ARGB32: clamp, premultiply, `d * 65535 + 0.5` truncated to 16 bits, the high
    /// byte kept.
    #[test]
    fn premultiply_rounds_as_cairo() {
        assert_eq!(premultiply(0xffffff, 1.0), 0xffff_ffff);
        assert_eq!(premultiply(0x000000, 1.0), 0xff00_0000);
        assert_eq!(premultiply(0x336699, 1.0), 0xff33_6699);
        assert_eq!(premultiply(0x808080, 0.5), 0x8040_4040);
        assert_eq!(premultiply(0xff0000, 0.25), 0x4040_0000);
        assert_eq!(premultiply(0x336699, 0.8), 0xcc28_517a);
        assert_eq!(premultiply(0x7f7f7f, 1.0 / 3.0), 0x552a_2a2a);
        // alpha 0.004 is 262 / 65535: a byte of 1; 0.003 is 197, which Cairo clears
        assert_eq!(premultiply(0xffffff, 0.004), 0x0101_0101);
        assert_eq!(premultiply(0xffffff, 0.003), 0);
        assert_eq!(premultiply(0x123456, 0.0), 0);
        // clamped to [0, 1]; NaN is none
        assert_eq!(premultiply(0xffffff, 2.0), 0xffff_ffff);
        assert_eq!(premultiply(0xffffff, -1.0), 0);
        assert_eq!(premultiply(0xffffff, f64::NAN), 0);
    }
}

mod monitors {
    use nhglue::monitors::device_origin;

    /// `roundf (x * scale120 / 120.0f)` with C's types: the int position times the unsigned scale is an unsigned
    /// product, so a negative position wraps (kept as the C had it).
    #[test]
    fn device_origin_as_c() {
        assert_eq!(device_origin(1920, 0, 180), (2880, 0));
        assert_eq!(device_origin(1, 3, 180), (2, 5));
        assert_eq!(device_origin(3, 1, 140), (4, 1));
        assert_eq!(device_origin(100, 100, 0), (0, 0));
        assert_eq!(device_origin(-1920, 0, 120), (35_789_476, 0));
        assert_eq!(device_origin(0, -1, 120), (0, 35_791_396));
    }
}

mod snapshot {
    use nhglue::abi::NHG_E_ARG;
    use nhglue::backing::copy_rows;

    /// Rows of `w` pixels into a wider stride; the padding is left alone.
    #[test]
    fn copy_rows_keeps_padding() {
        let src = [0x0102_0304u32, 5, 6, 7, 8, 9];
        let mut dst = [0xaau8; 16 + 12];
        assert_eq!(copy_rows(&src, 3, 2, &mut dst, 16), Ok(()));
        assert_eq!(&dst[..4], &0x0102_0304u32.to_ne_bytes());
        assert_eq!(&dst[8..12], &6u32.to_ne_bytes());
        assert_eq!(&dst[12..16], &[0xaa; 4]);
        assert_eq!(&dst[16..20], &7u32.to_ne_bytes());
        assert_eq!(&dst[24..28], &9u32.to_ne_bytes());
        // too short a destination, a stride below a row, or too few pixels
        assert_eq!(copy_rows(&src, 3, 2, &mut dst[..27], 16), Err(NHG_E_ARG));
        assert_eq!(copy_rows(&src, 3, 2, &mut dst, 8), Err(NHG_E_ARG));
        assert_eq!(copy_rows(&src[..5], 3, 2, &mut dst, 16), Err(NHG_E_ARG));
        assert_eq!(copy_rows(&[], 0, 0, &mut [], 0), Ok(()));
    }
}

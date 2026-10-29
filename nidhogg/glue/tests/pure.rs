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

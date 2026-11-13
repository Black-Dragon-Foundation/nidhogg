//! Test hooks for the font driver's own rows (`test-font-probe`): Core Text's and AppKit's answers, asked directly, that
//! the rows judge the driver against.  Each answer is a Lisp form.
//!
//! - `(register "FILE")`: register a font file for the process (`CTFontManagerRegisterFontsForURL`); `t` or `nil`.
//! - `(relist)`: list the faces again at the next listing, as Core Text's registered-fonts notification does; `t`.
//! - `(font-changes)`: `(NOTIFICATIONS GENERATION STALE)`: the registered-fonts notifications so far, the listing's
//!   generation, and whether a new listing is due.
//! - `(preferred "LANG" C...)`: the family Core Text lays the characters out in, in language LANG ("": none).
//! - `(ultralight)`: every listed face whose weight trait is -0.40, as `(ID "FAMILY" "POSTSCRIPT" APPKIT)`: ID the one
//!   its entities carry (`:font-entity`), APPKIT `weightOfFont:` (nil when it does not open).
//! - `(variation FAMILY SIZE C VS)`: the glyph Core Text lays the sequence C VS out with in the open font FAMILY at
//!   SIZE pixels (one line, that font); nil when the font is not open or Core Text needs another font.  FAMILY comes
//!   unquoted, as the call prints a string: every word before the three numbers.
//! - `(glyph-rows FAMILY SIZE C FG BG)`: C's glyph painted alone as the surface paints a run, `FG` on `BG`: `(BASE
//!   ROW...)`, each row's pixel furthest from BG, top down, the baseline BASE rows down.

use super::{fonts, text};

/// The words of `arg`, a printed Lisp list of symbols, numbers and strings without escapes.
fn words(arg: &str) -> Vec<String> {
    let s = arg.trim().trim_start_matches('(').trim_end_matches(')');
    let mut out = Vec::new();
    let mut chars = s.chars().peekable();
    while let Some(&c) = chars.peek() {
        if c.is_whitespace() {
            chars.next();
        } else if c == '"' {
            chars.next();
            out.push(chars.by_ref().take_while(|&c| c != '"').collect());
        } else {
            let mut w = String::new();
            while let Some(&c) = chars.peek() {
                if c.is_whitespace() {
                    break;
                }
                w.push(c);
                chars.next();
            }
            out.push(w);
        }
    }
    out
}

fn lisp_string(s: &str) -> String {
    format!("{s:?}")
}

/// The answer to `arg`; an unknown probe answers `(unknown-probe "ARG")`, and `None` is an empty or unreadable `arg`.
pub fn query(arg: &str) -> Option<String> {
    let w = words(arg);
    let num = |i: usize| w.get(i).and_then(|x| x.parse::<f64>().ok());
    Some(match w.first()?.as_str() {
        "register" => (if fonts::register(w.get(1)?) {
            "t"
        } else {
            "nil"
        })
        .into(),
        "relist" => {
            text::relist();
            "t".into()
        }
        "font-changes" => {
            let (notes, generation, stale) = text::font_changes();
            format!("({notes} {generation} {})", if stale { "t" } else { "nil" })
        }
        "preferred" => {
            let chars: Vec<u32> = (2..w.len())
                .filter_map(|i| num(i).map(|c| c as u32))
                .collect();
            text::preferred_family_in(&chars, w.get(1)?)
                .map_or_else(|| "nil".into(), |f| lisp_string(&f))
        }
        "ultralight" => {
            let (generation, listed) = text::listing();
            let rows: Vec<String> = listed
                .iter()
                .enumerate()
                .filter(|(_, f)| (f.weight * 100.0).round() as i64 == -40)
                .map(|(i, f)| {
                    format!(
                        "({} {} {} {})",
                        text::key(generation, i) + 1,
                        lisp_string(&f.family),
                        lisp_string(&f.postscript),
                        f.appkit_weight()
                            .map_or_else(|| "nil".into(), |a| a.to_string())
                    )
                })
                .collect();
            format!("({})", rows.join(" "))
        }
        "variation" => {
            // an argument that does not read is said, as the hook got it and as words (round 4 had only the hook's -2)
            let bad = || {
                Some(format!(
                    "(bad-arg {} {})",
                    lisp_string(arg),
                    lisp_string(&format!("{w:?}"))
                ))
            };
            // the call's text prints a string without its quotes (`nhmac--call'), so a family of several words
            // arrives as several (round 6: "Hiragino Mincho ProN"): the family is every word before the 3 numbers
            let n = w.len();
            if n < 5 {
                return bad();
            }
            let family = w[1..n - 3].join(" ");
            let (Some(size), Some(c), Some(vs)) = (
                num(n - 3),
                num(n - 2).and_then(|c| char::from_u32(c as u32)),
                num(n - 1).and_then(|c| char::from_u32(c as u32)),
            ) else {
                return bad();
            };
            let Some(handle) = fonts::find(&family, size) else {
                return Some("nil".into());
            };
            let mut units = Vec::new();
            for ch in [c, vs] {
                let mut b = [0u16; 2];
                units.extend_from_slice(ch.encode_utf16(&mut b));
            }
            text::shape(handle, &units)
                .and_then(|l| l.runs.first().and_then(|r| r.glyphs.first().copied()))
                .map_or_else(|| "nil".into(), |g| g.to_string())
        }
        "glyph-rows" => {
            // (glyph-rows FAMILY SIZE C FG BG): C's glyph painted alone as a run is, its baseline 2 SIZE rows down a
            // 3 SIZE tall column: (BASE ROW...), each row's pixel furthest from BG (round 6: ink a row above the
            // glyph's bounds at scale 1)
            let n = w.len();
            if n < 6 {
                return Some(format!("(bad-arg {})", lisp_string(arg)));
            }
            let family = w[1..n - 4].join(" ");
            let (Some(size), Some(c), Some(fg), Some(bg)) = (
                num(n - 4),
                num(n - 3).and_then(|c| char::from_u32(c as u32)),
                num(n - 2),
                num(n - 1),
            ) else {
                return Some(format!("(bad-arg {})", lisp_string(arg)));
            };
            let Some(handle) = fonts::find(&family, size) else {
                return Some("nil".into());
            };
            let mut b = [0u16; 2];
            let units = c.encode_utf16(&mut b);
            let mut g = [0u16; 2];
            if !fonts::glyphs_for(handle, units, &mut g[..units.len()]) {
                return Some("nil".into());
            }
            let s = size.ceil() as i32;
            let base = 2 * s;
            super::draw::glyph_rows(handle, g[0], fg as u32, bg as u32, 4 * s, 3 * s, base)
                .map_or_else(
                    || "nil".into(),
                    |rows| {
                        let rows: Vec<String> =
                            rows.iter().map(|p| format!("\"{p:06x}\"")).collect();
                        format!("({base} {})", rows.join(" "))
                    },
                )
        }
        // an unknown probe is said with the text the hook got (a "variation" that arrived as something else)
        _ => format!("(unknown-probe {})", lisp_string(arg)),
    })
}

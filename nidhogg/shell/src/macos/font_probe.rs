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
//! - `(variation "FAMILY" SIZE C VS)`: the glyph Core Text lays the sequence C VS out with in the open font FAMILY at
//!   SIZE pixels (one line, that font); nil when the font is not open or Core Text needs another font.

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

/// The answer to `arg`; `None` when it is not a probe this knows.
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
            let (size, c, vs) = (num(2)?, num(3)? as u32, num(4)? as u32);
            let Some(handle) = fonts::find(w.get(1)?, size) else {
                return Some("nil".into());
            };
            let mut units = Vec::new();
            for ch in [c, vs] {
                let mut b = [0u16; 2];
                units.extend_from_slice(char::from_u32(ch)?.encode_utf16(&mut b));
            }
            text::shape(handle, &units)
                .and_then(|l| l.runs.first().and_then(|r| r.glyphs.first().copied()))
                .map_or_else(|| "nil".into(), |g| g.to_string())
        }
        _ => return None,
    })
}

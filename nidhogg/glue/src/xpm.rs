//! XPM on Naglfar (design §5.5): ImageIO has no XPM, and `image.c`'s own decoder is compiled only for the ports it
//! names, so the native route reads XPM 3 here, as `xpm_load_image` does.  A colour line's keys are taken
//! `c` > `g` > `g4` > `m` (a colour display's order; `s` names count only through `:color-symbols`, which the native
//! route does not take); `None` is transparent; every other value is resolved by the caller (Emacs's colour names), and
//! one it cannot resolve is the frame's foreground there.  Platform-free; tested on Linux.

use std::collections::HashMap;

/// Whether `bytes` are an XPM 3 file (they start with its comment).
pub fn is_xpm(bytes: &[u8]) -> bool {
    bytes.starts_with(b"/* XPM */")
}

/// The string literals of the C source in `bytes`, in order, comments skipped (`xpm_scan`'s tokens).
fn strings(bytes: &[u8]) -> Vec<&[u8]> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i..].starts_with(b"/*") {
            i = bytes[i + 2..]
                .windows(2)
                .position(|w| w == b"*/")
                .map_or(bytes.len(), |p| i + 2 + p + 2);
        } else if bytes[i] == b'"' {
            let end = bytes[i + 1..]
                .iter()
                .position(|&b| b == b'"')
                .map_or(bytes.len(), |p| i + 1 + p);
            out.push(&bytes[i + 1..end]);
            i = end + 1;
        } else {
            i += 1;
        }
    }
    out
}

/// The colour a colour line gives (after its characters): the best key's value, `None` for transparent.
fn colour(spec: &str, resolve: &mut dyn FnMut(&str) -> u32) -> Option<Option<u32>> {
    const KEYS: [&str; 5] = ["s", "m", "g4", "g", "c"];
    let rank = |k: &str| KEYS.iter().position(|&x| x == k);
    let mut words = spec.split_whitespace().peekable();
    let (mut best, mut best_rank) = (None, 0);
    while let Some(key) = words.next() {
        let r = rank(key)?;
        let mut value = Vec::new();
        while let Some(&w) = words.peek() {
            if rank(w).is_some() {
                break;
            }
            value.push(w);
            words.next();
        }
        if value.is_empty() {
            return None;
        }
        if r > best_rank {
            (best, best_rank) = (Some(value.join(" ")), r);
        }
    }
    let v = best?;
    Some((!v.eq_ignore_ascii_case("none")).then(|| resolve(&v)))
}

/// An XPM 3 image: its size and straight ARGB words (alpha 0 for `None`, else 255), and whether any is transparent.
/// `None` when it does not parse.
pub fn decode(
    bytes: &[u8],
    resolve: &mut dyn FnMut(&str) -> u32,
) -> Option<(u32, u32, Vec<u32>, bool)> {
    if !is_xpm(bytes) {
        return None;
    }
    let s = strings(bytes);
    let head = std::str::from_utf8(s.first()?).ok()?;
    let n: Vec<usize> = head
        .split_whitespace()
        .take(4)
        .map(|v| v.parse().ok())
        .collect::<Option<_>>()?;
    let [w, h, ncolors, cpp] = n[..] else {
        return None;
    };
    if w == 0 || h == 0 || ncolors == 0 || cpp == 0 || s.len() < 1 + ncolors + h {
        return None;
    }
    let mut table: HashMap<&[u8], u32> = HashMap::with_capacity(ncolors);
    for line in &s[1..=ncolors] {
        if line.len() <= cpp {
            return None;
        }
        let c = colour(std::str::from_utf8(&line[cpp..]).ok()?, resolve)?;
        table.insert(
            &line[..cpp],
            c.map_or(0, |rgb| 0xff00_0000 | (rgb & 0xff_ffff)),
        );
    }
    let mut argb = Vec::with_capacity(w * h);
    for row in &s[1 + ncolors..1 + ncolors + h] {
        if row.len() < w * cpp {
            return None;
        }
        for px in row[..w * cpp].chunks(cpp) {
            argb.push(*table.get(px)?);
        }
    }
    let alpha = argb.iter().any(|p| p >> 24 == 0);
    Some((w as u32, h as u32, argb, alpha))
}

#[cfg(test)]
mod tests {
    use super::*;

    const XPM: &[u8] = br#"/* XPM */
static char *test[] = {
/* columns rows colours chars-per-pixel */
"3 2 3 1",
"  c None",
". m black c #ff0000",
"X c light gray s mask",
/* pixels */
" .X",
"X. "
};"#;

    fn names(name: &str) -> u32 {
        match name {
            "#ff0000" => 0xff0000,
            "light gray" => 0xd3d3d3,
            _ => 0x123456,
        }
    }

    #[test]
    fn colours_by_the_best_key_none_transparent() {
        let (w, h, px, alpha) = decode(XPM, &mut names).unwrap();
        assert_eq!((w, h, alpha), (3, 2, true));
        assert_eq!(
            px,
            vec![0, 0xffff_0000, 0xffd3_d3d3, 0xffd3_d3d3, 0xffff_0000, 0]
        );
    }

    #[test]
    fn two_characters_a_pixel_and_no_transparency() {
        let xpm =
            br#"/* XPM */ static char *x[] = { "2 1 2 2", "aa c #ff0000", "bb c blue", "aabb" };"#;
        let (_, _, px, alpha) = decode(xpm, &mut names).unwrap();
        assert!(!alpha);
        assert_eq!(px, vec![0xffff_0000, 0xff12_3456]);
    }

    #[test]
    fn malformed_files_are_refused() {
        assert!(decode(b"static char *x[]", &mut names).is_none());
        // a pixel no colour names, a short row, a bad key, too few rows
        for bad in [
            &br#"/* XPM */ { "1 1 1 1", "a c red", "b" }"#[..],
            br#"/* XPM */ { "2 1 1 1", "a c red", "a" }"#,
            br#"/* XPM */ { "1 1 1 1", "a q red", "a" }"#,
            br#"/* XPM */ { "1 2 1 1", "a c red", "a" }"#,
        ] {
            assert!(
                decode(bad, &mut names).is_none(),
                "{}",
                String::from_utf8_lossy(bad)
            );
        }
    }
}

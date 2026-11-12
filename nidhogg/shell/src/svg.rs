//! SVG (design §5.5): resvg renders an image the way `image.c`'s librsvg route (`svg_load_image`) does.  The style
//! sheet names the face's font and adds the image's `:css`.  `currentColor` is the foreground, inherited as librsvg's
//! wrapper makes it (an attribute on the root, which a `color` of the image's own overrides).  The image is stretched to the size Emacs
//! computed, without keeping its aspect ratio, over an opaque background.  Pure Rust, so it is tested on Linux too.

use resvg::{tiny_skia, usvg};
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

/// How Emacs reads an image: the face's font, the image's `:css`, the resolution, the foreground (`0xRRGGBB`), and
/// where relative references resolve (the file's directory or `:base-uri`).
#[derive(Debug, Clone, Default)]
pub struct Style {
    pub css: String,
    pub font_family: String,
    pub font_size: f32,
    pub dpi: f32,
    pub foreground: u32,
    pub base: Option<PathBuf>,
}

/// The system's fonts, read once.
fn fonts() -> Arc<usvg::fontdb::Database> {
    static DB: OnceLock<Arc<usvg::fontdb::Database>> = OnceLock::new();
    DB.get_or_init(|| {
        let mut db = usvg::fontdb::Database::new();
        db.load_system_fonts();
        Arc::new(db)
    })
    .clone()
}

/// The style sheet `svg_load_image` gives librsvg (the face's font, then the image's `:css`).
fn style_sheet(style: &Style) -> String {
    let mut css = String::from("svg{");
    if !style.font_family.is_empty() {
        css += &format!(
            "font-family:\"{}\";",
            style.font_family.replace(['"', '\\'], "")
        );
    }
    if style.font_size > 0.0 {
        css += &format!("font-size:{}px;", style.font_size);
    }
    css + "}" + &style.css
}

/// Where the root `svg` element's start tag is in `d`, after the prolog, comments and a document type.
fn root_start(d: &[u8]) -> Option<usize> {
    let find = |h: &[u8], n: &[u8]| h.windows(n.len()).position(|w| w == n);
    let mut i = 0;
    loop {
        i += d[i..].iter().position(|&b| b == b'<')?;
        let r = &d[i..];
        if r.starts_with(b"<?") {
            i += find(r, b"?>")? + 2;
        } else if r.starts_with(b"<!--") {
            i += find(r, b"-->")? + 3;
        } else if r.starts_with(b"<!") {
            // a document type, its internal subset in brackets
            let mut depth = 0;
            let mut j = 2;
            loop {
                match r.get(j)? {
                    b'[' => depth += 1,
                    b']' => depth -= 1,
                    b'>' if depth <= 0 => break,
                    _ => {}
                }
                j += 1;
            }
            i += j + 1;
        } else {
            let after = r.get(4).copied().unwrap_or(b'>');
            return (r.starts_with(b"<svg")
                && (after.is_ascii_whitespace() || after == b'>' || after == b'/'))
                .then_some(i);
        }
    }
}

/// Whether the attributes `t` (a start tag's, after its name) name `name`; the scan stops at the tag's end.
fn has_attribute(mut t: &[u8], name: &[u8]) -> bool {
    loop {
        t = t.trim_ascii_start();
        let n = t
            .iter()
            .position(|&b| b == b'=' || b == b'>' || b == b'/' || b.is_ascii_whitespace())
            .unwrap_or(t.len());
        if n == 0 {
            return false;
        }
        let (key, rest) = t.split_at(n);
        let Some(rest) = rest.trim_ascii_start().strip_prefix(b"=") else {
            return false;
        };
        let rest = rest.trim_ascii_start();
        let Some(&q) = rest.first().filter(|&&q| q == b'"' || q == b'\'') else {
            return false;
        };
        let Some(close) = rest[1..].iter().position(|&b| b == q) else {
            return false;
        };
        if key == name {
            return true;
        }
        t = &rest[close + 2..];
    }
}

/// `data` (decompressed when it is SVGZ) with the foreground as the root `svg` element's `color` attribute, as
/// librsvg's wrapper gives it (`style="color: …"` on an outer element): inherited, so a `color` the image sets itself
/// (the root's attribute or style, any element's, a rule of its own) wins.  `None`: use `data` as it is (the root
/// names a `color`, or no root `svg` start tag is found).
fn with_foreground(data: &[u8], foreground: u32) -> Option<Vec<u8>> {
    let plain;
    let data = if data.starts_with(&[0x1f, 0x8b]) {
        plain = usvg::decompress_svgz(data).ok()?;
        &plain[..]
    } else {
        data
    };
    let at = root_start(data)? + 4;
    if has_attribute(&data[at..], b"color") {
        return None;
    }
    let attr = format!(" color=\"#{:06x}\"", foreground & 0xff_ffff);
    let mut out = Vec::with_capacity(data.len() + attr.len());
    out.extend_from_slice(&data[..at]);
    out.extend_from_slice(attr.as_bytes());
    out.extend_from_slice(&data[at..]);
    Some(out)
}

fn tree(data: &[u8], style: &Style) -> Option<usvg::Tree> {
    let coloured = with_foreground(data, style.foreground);
    let data = coloured.as_deref().unwrap_or(data);
    let mut opts = usvg::Options {
        resources_dir: style.base.clone(),
        fontdb: fonts(),
        style_sheet: Some(style_sheet(style)),
        ..Default::default()
    };
    if style.dpi > 0.0 {
        opts.dpi = style.dpi;
    }
    if !style.font_family.is_empty() {
        opts.font_family = style.font_family.clone();
    }
    if style.font_size > 0.0 {
        opts.font_size = style.font_size;
    }
    usvg::Tree::from_data(data, &opts).ok()
}

/// The image's own size in pixels (its width and height, or its view box); `None` when it does not parse.
pub fn size(data: &[u8], style: &Style) -> Option<(f64, f64)> {
    let s = tree(data, style)?.size();
    Some((f64::from(s.width()), f64::from(s.height())))
}

/// The image drawn at `w` x `h` over `background` (`0xRRGGBB`): opaque `0xffRRGGBB` words, rows top down.  `None` when
/// it does not parse or the size is empty.
pub fn render(data: &[u8], style: &Style, background: u32, w: u32, h: u32) -> Option<Vec<u32>> {
    let tree = tree(data, style)?;
    let size = tree.size();
    let mut pix = tiny_skia::Pixmap::new(w, h)?;
    let [_, r, g, b] = background.to_be_bytes();
    pix.fill(tiny_skia::Color::from_rgba8(r, g, b, 255));
    let t = tiny_skia::Transform::from_scale(w as f32 / size.width(), h as f32 / size.height());
    resvg::render(&tree, t, &mut pix.as_mut());
    Some(
        pix.pixels()
            .iter()
            .map(|p| {
                0xff00_0000
                    | u32::from(p.red()) << 16
                    | u32::from(p.green()) << 8
                    | u32::from(p.blue())
            })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const HALF: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" width="4" height="2">
        <rect width="2" height="2" fill="currentColor"/></svg>"#;

    #[test]
    fn the_size_is_the_images_own_or_its_view_box() {
        let s = Style::default();
        assert_eq!(size(HALF, &s), Some((4.0, 2.0)));
        let vb = br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 30 15"/>"#;
        assert_eq!(size(vb, &s), Some((30.0, 15.0)));
        // physical units at the display's resolution
        let mm = br#"<svg xmlns="http://www.w3.org/2000/svg" width="25.4mm" height="1in"/>"#;
        let s72 = Style {
            dpi: 72.0,
            ..Style::default()
        };
        assert_eq!(size(mm, &s72), Some((72.0, 72.0)));
        assert_eq!(size(b"not svg", &s), None);
    }

    #[test]
    fn current_colour_is_the_foreground_over_the_background_stretched_to_the_size() {
        let s = Style {
            foreground: 0xff0000,
            ..Style::default()
        };
        // 4x2 drawn at 8x2: the left half (the rect) red, the right half the background
        let px = render(HALF, &s, 0x0000ff, 8, 2).unwrap();
        assert_eq!(px.len(), 16);
        assert_eq!(px[0], 0xffff_0000);
        assert_eq!(px[3], 0xffff_0000);
        assert_eq!(px[4], 0xff00_00ff);
        assert_eq!(px[15], 0xff00_00ff);
    }

    #[test]
    fn the_images_css_is_applied_after_emacss() {
        let s = Style {
            foreground: 0xff0000,
            css: "rect{fill:#00ff00}".into(),
            ..Style::default()
        };
        let px = render(HALF, &s, 0x000000, 4, 2).unwrap();
        assert_eq!(px[0], 0xff00_ff00);
        assert_eq!(px[3], 0xff00_0000);
    }

    /// librsvg's wrapper only makes the foreground inherited: a `color` the image sets wins, at the root or below.
    #[test]
    fn the_images_own_colour_wins_over_the_foreground() {
        let s = Style {
            foreground: 0xff0000,
            ..Style::default()
        };
        let rect = r#"<rect width="2" height="2" fill="currentColor"/>"#;
        let attr = format!(
            r##"<svg xmlns="http://www.w3.org/2000/svg" color="#00ff00" width="2" height="2">{rect}</svg>"##
        );
        assert_eq!(
            render(attr.as_bytes(), &s, 0, 2, 2).unwrap()[0],
            0xff00_ff00
        );
        let style = format!(
            r##"<svg xmlns="http://www.w3.org/2000/svg" style="color:#0000ff" width="2" height="2">{rect}</svg>"##
        );
        assert_eq!(
            render(style.as_bytes(), &s, 0, 2, 2).unwrap()[0],
            0xff00_00ff
        );
        let group = format!(
            r##"<svg xmlns="http://www.w3.org/2000/svg" width="2" height="2"><g color="#00ff00">{rect}</g></svg>"##
        );
        assert_eq!(
            render(group.as_bytes(), &s, 0, 2, 2).unwrap()[0],
            0xff00_ff00
        );
        // a nested svg inherits the foreground: no rule sets it on every svg
        let nested = format!(
            r##"<svg xmlns="http://www.w3.org/2000/svg" width="2" height="2" color="#00ff00"><svg width="2" height="2">{rect}</svg></svg>"##
        );
        assert_eq!(
            render(nested.as_bytes(), &s, 0, 2, 2).unwrap()[0],
            0xff00_ff00
        );
    }

    #[test]
    fn the_foreground_reaches_the_root_past_a_prolog_comments_and_a_doctype() {
        let s = Style {
            foreground: 0xff0000,
            ..Style::default()
        };
        let doc = br#"<?xml version="1.0"?>
<!-- a comment, <svg> in it -->
<!DOCTYPE svg [ <!ENTITY e "x"> ]>
<svg xmlns="http://www.w3.org/2000/svg" width="2" height="2"><rect width="2" height="2" fill="currentColor"/></svg>"#;
        assert_eq!(render(doc, &s, 0, 2, 2).unwrap()[0], 0xffff_0000);
        assert_eq!(root_start(b"<svgx/>"), None);
        assert!(has_attribute(br#" a="1" color='red'>"#, b"color"));
        assert!(!has_attribute(
            br#" style="color:red" data-color="x">"#,
            b"color"
        ));
    }

    #[test]
    fn an_empty_size_or_bad_data_renders_nothing() {
        let s = Style::default();
        assert_eq!(render(HALF, &s, 0, 0, 2), None);
        assert_eq!(render(b"<svg", &s, 0, 4, 2), None);
    }
}

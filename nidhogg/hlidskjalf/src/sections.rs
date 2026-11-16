//! MARK sections (design M1): the lines a regular expression (Rust regex syntax, `drgnx-minimap-mark-regexp`) picks
//! as section headers, each with its name: the capture group `name`, else the first group, else the whole match,
//! trimmed.  Found from the text the model already reads (the first build and each edit's whole lines), kept sorted by
//! the position of the line's start, and shifted with the edits; with no expression set, none are looked for.

use regex_lite::Regex;

/// The longest name kept, in bytes (cut at a character boundary).
pub const NAME_MAX: usize = 256;
/// The most sections a text keeps; the rest are dropped.
pub const MAX: usize = 1 << 16;

/// One section: its line's start (a position) and its name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Section {
    pub pos: u64,
    pub name: Box<str>,
}

/// The characters in BYTES: every byte for a unibyte text, else every byte that does not continue a character.
pub fn chars(bytes: &[u8], multibyte: bool) -> u64 {
    if multibyte {
        bytes.iter().filter(|&&b| b & 0xc0 != 0x80).count() as u64
    } else {
        bytes.len() as u64
    }
}

fn name_of(re: &Regex, line: &str) -> Option<Box<str>> {
    let c = re.captures(line)?;
    let m = c.name("name").or_else(|| c.get(1)).or_else(|| c.get(0))?;
    let s = m.as_str().trim();
    if s.is_empty() {
        return None;
    }
    let mut end = s.len().min(NAME_MAX);
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    Some(s[..end].into())
}

/// The sections of TEXT (whole lines), whose first character is at position POS.
pub fn scan(re: &Regex, text: &[u8], pos: u64, multibyte: bool) -> Vec<Section> {
    let mut out = Vec::new();
    let mut at = pos;
    for line in text.split_inclusive(|&b| b == b'\n') {
        let body = line.strip_suffix(b"\n").unwrap_or(line);
        let s = String::from_utf8_lossy(body);
        if let Some(name) = name_of(re, &s) {
            if out.len() >= MAX {
                break;
            }
            out.push(Section { pos: at, name });
        }
        at += chars(line, multibyte);
    }
    out
}

/// The whole lines from position LS, OLD characters of them, became NEW (their new text): the sections in them go,
/// those after shift, and NEW's are found.
pub fn edit(list: &mut Vec<Section>, re: &Regex, ls: u64, old: u64, new: &[u8], multibyte: bool) {
    let new_chars = chars(new, multibyte);
    let a = list.partition_point(|s| s.pos < ls);
    let b = list.partition_point(|s| s.pos < ls + old);
    for s in &mut list[b..] {
        s.pos = s.pos - old + new_chars;
    }
    let found = scan(re, new, ls, multibyte);
    list.splice(a..b, found);
    list.truncate(MAX);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// drgnx's default (`drgnx-minimap-mark-regexp`).
    const DEFAULT: &str = r"^\s*(?:;;;;+|(?://+|#+|;+|--|/\*+)\s*(?:MARK:\s*-?|pragma\s+mark\s+-?|region\b|%%))\s*(?P<name>\S.*?)\s*(?:\*/)?$";

    fn names(v: &[Section]) -> Vec<(u64, &str)> {
        v.iter().map(|s| (s.pos, &*s.name)).collect()
    }

    #[test]
    fn the_default_finds_each_kind_of_section_header() {
        let re = Regex::new(DEFAULT).unwrap();
        let text = b"// MARK: - Parsing\nfn a() {}\n#pragma mark Drawing\n  // region Helpers\n#region Io\n;;;; Commands\n# %% Load the data\n/* MARK: Old */\nlet mark = 1; // not one\n;; a comment\n";
        let v = scan(&re, text, 1, true);
        let n: Vec<&str> = v.iter().map(|s| &*s.name).collect();
        assert_eq!(
            n,
            [
                "Parsing",
                "Drawing",
                "Helpers",
                "Io",
                "Commands",
                "Load the data",
                "Old"
            ]
        );
        assert_eq!(v[0].pos, 1);
        assert_eq!(v[1].pos, 1 + 19 + 10, "the line's start");
    }

    #[test]
    fn positions_count_characters_not_bytes() {
        let re = Regex::new(DEFAULT).unwrap();
        let v = scan(&re, "é\n// MARK: Ü\n".as_bytes(), 10, true);
        assert_eq!(names(&v), [(12, "Ü")]);
        // unibyte: every byte a character
        let v = scan(&re, "é\n// MARK: x\n".as_bytes(), 10, false);
        assert_eq!(names(&v), [(13, "x")]);
    }

    #[test]
    fn an_edit_drops_the_lines_it_replaced_shifts_the_rest_and_finds_the_new() {
        let re = Regex::new(DEFAULT).unwrap();
        let text = b"// MARK: A\nx\n// MARK: B\ny\n// MARK: C\n";
        let mut v = scan(&re, text, 1, true);
        assert_eq!(names(&v), [(1, "A"), (14, "B"), (27, "C")]);
        // line 3 ("// MARK: B\n", 11 chars at 14) becomes two lines, one a section
        edit(&mut v, &re, 14, 11, b"// MARK: B2\nz\n", true);
        assert_eq!(names(&v), [(1, "A"), (14, "B2"), (30, "C")]);
        // a section header typed into a plain line
        edit(&mut v, &re, 12, 2, b"#region R\n", true);
        assert_eq!(names(&v), [(1, "A"), (12, "R"), (22, "B2"), (38, "C")]);
        // deleted
        edit(&mut v, &re, 1, 11, b"", true);
        assert_eq!(names(&v), [(1, "R"), (11, "B2"), (27, "C")]);
    }

    #[test]
    fn the_model_keeps_its_sections_through_builds_and_edits() {
        let text: &[u8] = b"a\n// MARK: One\nb\n";
        let mut m = crate::Model::build(&[text], true, 8, false);
        assert!(
            m.sections(0, u64::MAX).is_empty(),
            "no expression: none looked for"
        );
        assert!(m.set_section_regex(Some("(")).is_err());
        m.set_section_regex(Some(DEFAULT)).unwrap();
        m.rebuild(&[text], false);
        assert_eq!(names(m.sections(0, u64::MAX)), [(3, "One")]);
        // a line typed above shifts it
        m.edit(1, 1, 3, &[b"x\na\n"]).unwrap();
        assert_eq!(names(m.sections(0, u64::MAX)), [(5, "One")]);
        // a section typed into the last line
        m.edit(18, 19, 29, &[b"#region Two\n"]).unwrap();
        assert_eq!(names(m.sections(0, u64::MAX)), [(5, "One"), (18, "Two")]);
        assert_eq!(names(m.sections(6, 19)), [(18, "Two")]);
        // a background build finds them too, with the edits made meanwhile
        let mut b = crate::Model::build(&[b"x\n"], true, 8, false);
        b.set_section_regex(Some(DEFAULT)).unwrap();
        b.rebuild(&[text], true);
        b.edit(1, 1, 3, &[b"x\na\n"]).unwrap();
        b.poll(true);
        assert_eq!(names(b.sections(0, u64::MAX)), [(5, "One")]);
    }
}

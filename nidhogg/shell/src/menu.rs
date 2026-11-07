//! A menu as the Lisp side sends it, platform-neutral so its tests run on Linux.
//!
//! `nhmac-menu-encode` (`lisp/term/nhmac-win.el`) writes the flat records of the flattener: 7 fields per record, the
//! fields separated by US (31) and the records by RS (30).  The fields are the depth, the kind, the label, the key
//! text, enabled, state and the item's place in `menu_items` (-1 for a title).  This module reads them back into a
//! tree; the AppKit half (`macos::menus`) builds the `NSMenu` from it.

/// Fields per record, as `nhmac-menu-width`.
pub const WIDTH: usize = 7;
const US: char = '\u{1f}';
const RS: char = '\u{1e}';

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Item,
    Submenu,
    Separator,
    Toggle,
    Radio,
}

/// An entry with the entries one level below it under it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Node {
    pub kind: Kind,
    pub label: String,
    pub key: String,
    pub enabled: bool,
    /// A toggle or radio that is on.
    pub state: bool,
    /// The place in `menu_items`; `None` for a title.
    pub index: Option<u32>,
    pub children: Vec<Node>,
}

/// The tree of the records in `text`; `None` for text that is not what the flattener makes (a field count that is
/// not [`WIDTH`], a number or kind that does not read, a level that skips one, a separator or a leaf with children).
pub fn decode(text: &str) -> Option<Vec<Node>> {
    let mut roots: Vec<Node> = Vec::new();
    // the open path: the last entry of each level, from the top
    let mut open: Vec<Node> = Vec::new();
    let finish = |open: &mut Vec<Node>, roots: &mut Vec<Node>, keep: usize| {
        while open.len() > keep {
            let done = open.pop()?;
            match open.last_mut() {
                Some(parent) => parent.children.push(done),
                None => roots.push(done),
            }
        }
        Some(())
    };
    for rec in text.split(RS).filter(|r| !r.is_empty()) {
        let f: Vec<&str> = rec.split(US).collect();
        if f.len() != WIDTH {
            return None;
        }
        let depth: usize = f[0].parse().ok()?;
        let kind = match f[1].parse::<i32>().ok()? {
            0 => Kind::Item,
            1 => Kind::Submenu,
            2 => Kind::Separator,
            3 => Kind::Toggle,
            4 => Kind::Radio,
            _ => return None,
        };
        let index: i64 = f[6].parse().ok()?;
        if depth > open.len() {
            return None;
        }
        finish(&mut open, &mut roots, depth)?;
        if depth > 0 && open.last()?.kind != Kind::Submenu {
            return None;
        }
        open.push(Node {
            kind,
            label: f[2].to_string(),
            key: f[3].to_string(),
            enabled: f[4] == "1",
            state: f[5] == "1",
            index: u32::try_from(index).ok(),
            children: Vec::new(),
        });
    }
    finish(&mut open, &mut roots, 0)?;
    Some(roots)
}

/// The leading integers of a call's argument text ("(3 40 50 records…)"), and what follows them.
pub fn split_numbers(arg: &str, n: usize) -> Option<(Vec<i64>, &str)> {
    let mut rest = arg.trim().trim_start_matches('(');
    let mut out = Vec::new();
    for _ in 0..n {
        rest = rest.trim_start();
        let end = rest.find(|c: char| c.is_whitespace()).unwrap_or(rest.len());
        out.push(rest[..end].trim_end_matches(')').parse().ok()?);
        rest = &rest[end..];
    }
    Some((out, rest.trim_start()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(depth: i32, kind: i32, label: &str, key: &str, en: i32, st: i32, idx: i32) -> String {
        format!("{depth}{US}{kind}{US}{label}{US}{key}{US}{en}{US}{st}{US}{idx}")
    }

    fn join(recs: &[String]) -> String {
        recs.join(&RS.to_string())
    }

    #[test]
    fn a_bar_of_two_titles() {
        let text = join(&[
            rec(0, 1, "File", "", 1, 0, -1),
            rec(1, 0, "Open", "C-x C-f", 1, 0, 3),
            rec(1, 2, "", "", 0, 0, 11),
            rec(1, 3, "Toggle", "", 1, 1, 19),
            rec(1, 1, "Sub", "", 1, 0, 27),
            rec(2, 0, "Z", "", 0, 0, 39),
            rec(0, 1, "Edit", "", 1, 0, -1),
            rec(1, 4, "R", "", 1, 1, 51),
        ]);
        let bar = decode(&text).unwrap();
        assert_eq!(bar.len(), 2);
        assert_eq!(bar[0].label, "File");
        assert_eq!(bar[0].index, None);
        assert_eq!(bar[0].children.len(), 4);
        assert_eq!(bar[0].children[0].key, "C-x C-f");
        assert_eq!(bar[0].children[0].index, Some(3));
        assert_eq!(bar[0].children[1].kind, Kind::Separator);
        assert!(bar[0].children[2].state);
        assert_eq!(bar[0].children[3].children[0].label, "Z");
        assert!(!bar[0].children[3].children[0].enabled);
        assert_eq!(bar[1].children[0].kind, Kind::Radio);
    }

    #[test]
    fn empty_text_is_an_empty_bar() {
        assert_eq!(decode("").unwrap(), Vec::new());
    }

    #[test]
    fn bad_records_are_refused() {
        // wrong field count
        assert!(decode("0\u{1f}1\u{1f}File").is_none());
        // a level that skips one
        assert!(
            decode(&join(&[
                rec(0, 1, "A", "", 1, 0, -1),
                rec(2, 0, "B", "", 1, 0, 3)
            ]))
            .is_none()
        );
        // children under an item that is not a submenu
        assert!(
            decode(&join(&[
                rec(0, 0, "A", "", 1, 0, 1),
                rec(1, 0, "B", "", 1, 0, 3)
            ]))
            .is_none()
        );
        // an unknown kind
        assert!(decode(&join(&[rec(0, 9, "A", "", 1, 0, 1)])).is_none());
        // a number that is not
        assert!(decode("0\u{1f}0\u{1f}A\u{1f}\u{1f}1\u{1f}0\u{1f}x").is_none());
    }

    #[test]
    fn leading_numbers_of_a_call() {
        let (n, rest) = split_numbers("(7 -3 20 rest of it)", 3).unwrap();
        assert_eq!(n, vec![7, -3, 20]);
        assert_eq!(rest, "rest of it)");
        assert!(split_numbers("(a 2)", 2).is_none());
    }
}

//! The clipboard's and the file dialogs' request texts, platform-neutral so their tests run on Linux.
//!
//! Lisp reaches the macOS pasteboard and panels through `nhmac--call` and `nhmac--query`, whose arguments arrive as
//! the printed list, strings without quotes ("(CLIPBOARD some text)"), and whose answers are read as Lisp.  This module
//! reads such arguments and quotes the answers; the AppKit half is `macos::pasteboard` and `macos::dialogs`.

/// The unit separator `nhmac-win.el` puts between the fields of a dialog request.
pub const US: char = '\u{1f}';

/// The first word of `arg` and the text after the one space that follows it, without the list's closing parenthesis
/// (exactly one is the list's own; a text may end in more).  `None` for an argument without the word.
pub fn word_and_text(arg: &str) -> Option<(&str, &str)> {
    let inner = arg.strip_prefix('(').unwrap_or(arg);
    let inner = inner.strip_suffix(')').unwrap_or(inner);
    let (word, rest) = match inner.split_once(' ') {
        Some((w, r)) => (w, r),
        None => (inner, ""),
    };
    (!word.is_empty()).then_some((word, rest))
}

/// `text` as a Lisp string literal (backslash and double quote escaped; Lisp reads line breaks as they are).
pub fn lisp_string(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for c in text.chars() {
        if c == '\\' || c == '"' {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('"');
    out
}

/// The pasteboard type a MIME type is written as (`NSPasteboardTypeString` for text, `NSPasteboardTypeFileURL` for a
/// file's URL); any other type keeps its name.
pub fn uti_for_target(target: &str) -> &str {
    match target {
        "STRING"
        | "TEXT"
        | "UTF8_STRING"
        | "COMPOUND_TEXT"
        | "NSStringPboardType"
        | "text/plain"
        | "text/plain;charset=utf-8" => "public.utf8-plain-text",
        "text/uri-list" | "FILE_NAME" | "NSFilenamesPboardType" => "public.file-url",
        other => other,
    }
}

/// The fields of a dialog request: prompt, directory, default file name, must-match and directory-only flags.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DialogReq {
    pub prompt: String,
    pub dir: String,
    pub default: String,
    /// An open panel (the file must exist); otherwise a save panel.
    pub must_match: bool,
    pub only_dir: bool,
}

/// Read the request `nhmac-win.el`'s `x-file-dialog` sends: five fields joined by [`US`], the flags `1` or `0`.
pub fn dialog_req(arg: &str) -> Option<DialogReq> {
    let inner = arg.strip_prefix('(').unwrap_or(arg);
    let inner = inner.strip_suffix(')').unwrap_or(inner);
    let f: Vec<&str> = inner.split(US).collect();
    let [prompt, dir, default, must, only] = f[..] else {
        return None;
    };
    Some(DialogReq {
        prompt: prompt.to_string(),
        dir: dir.to_string(),
        default: default.to_string(),
        must_match: must == "1",
        only_dir: only == "1",
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn word_and_text_keeps_the_texts_own_parentheses() {
        assert_eq!(
            word_and_text("(CLIPBOARD f(x))"),
            Some(("CLIPBOARD", "f(x)"))
        );
        assert_eq!(
            word_and_text("(CLIPBOARD a b\nc)"),
            Some(("CLIPBOARD", "a b\nc"))
        );
        assert_eq!(word_and_text("(CLIPBOARD )"), Some(("CLIPBOARD", "")));
        assert_eq!(word_and_text("(CLIPBOARD)"), Some(("CLIPBOARD", "")));
        assert_eq!(word_and_text("()"), None);
    }

    #[test]
    fn lisp_strings_are_quoted() {
        assert_eq!(lisp_string("a\"b\\c\nd é"), "\"a\\\"b\\\\c\nd é\"");
        assert_eq!(lisp_string(""), "\"\"");
    }

    #[test]
    fn targets_map_to_pasteboard_types() {
        assert_eq!(uti_for_target("STRING"), "public.utf8-plain-text");
        assert_eq!(
            uti_for_target("text/plain;charset=utf-8"),
            "public.utf8-plain-text"
        );
        assert_eq!(uti_for_target("text/uri-list"), "public.file-url");
        assert_eq!(uti_for_target("image/png"), "image/png");
    }

    #[test]
    fn a_dialog_request() {
        let r = dialog_req("(Pick\u{1f}/tmp/\u{1f}x.txt\u{1f}1\u{1f}0)").unwrap();
        assert_eq!(
            r,
            DialogReq {
                prompt: "Pick".into(),
                dir: "/tmp/".into(),
                default: "x.txt".into(),
                must_match: true,
                only_dir: false,
            }
        );
        assert!(dialog_req("(Pick\u{1f}/tmp/)").is_none());
    }
}

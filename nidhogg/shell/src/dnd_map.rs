//! Drag and drop between the shell's terms (MIME types, `NHSHELL_DND_*` actions) and AppKit's (pasteboard types,
//! `NSDragOperation` bits), platform-neutral so its tests run on Linux.

use crate::api::{NHSHELL_DND_ASK, NHSHELL_DND_COPY, NHSHELL_DND_MOVE};

/// `NSDragOperation` bits.
pub const OP_COPY: usize = 1;
pub const OP_GENERIC: usize = 4;
pub const OP_MOVE: usize = 16;

/// The `NHSHELL_DND_*` set of the operations a source allows (generic is ask).
pub fn actions_of(ops: usize) -> u32 {
    let mut a = 0;
    if ops & OP_COPY != 0 {
        a |= NHSHELL_DND_COPY;
    }
    if ops & OP_MOVE != 0 {
        a |= NHSHELL_DND_MOVE;
    }
    if ops & OP_GENERIC != 0 {
        a |= NHSHELL_DND_ASK;
    }
    a
}

/// The operation of one action (0 for none or a set).
pub fn op_of(action: u32) -> usize {
    match action {
        NHSHELL_DND_COPY => OP_COPY,
        NHSHELL_DND_MOVE => OP_MOVE,
        NHSHELL_DND_ASK => OP_GENERIC,
        _ => 0,
    }
}

/// The operations of a set of actions.
pub fn ops_of(actions: u32) -> usize {
    [NHSHELL_DND_COPY, NHSHELL_DND_MOVE, NHSHELL_DND_ASK]
        .into_iter()
        .filter(|a| actions & a != 0)
        .fold(0, |ops, a| ops | op_of(a))
}

/// The first action of `actions` in copy, move, ask order (0 for none).
pub fn first_action(actions: u32) -> u32 {
    [NHSHELL_DND_COPY, NHSHELL_DND_MOVE, NHSHELL_DND_ASK]
        .into_iter()
        .find(|a| actions & a != 0)
        .unwrap_or(0)
}

/// What the drop target answers AppKit: nothing when the drag is not accepted, else the operation of the preferred
/// action when the source allows it, else of the first action both sides have.  `target_actions` is Lisp's set (0:
/// not answered yet, so every action the source offers is taken, the first in copy, move, ask order).
pub fn answer(accepted: bool, source_ops: usize, target_actions: u32, preferred: u32) -> usize {
    if !accepted {
        return 0;
    }
    let src = actions_of(source_ops);
    let common = if target_actions == 0 {
        src
    } else {
        src & target_actions
    };
    if preferred != 0 && common & preferred != 0 {
        return op_of(preferred);
    }
    op_of(first_action(common))
}

pub const TEXT_UTI: &str = "public.utf8-plain-text";
pub const FILE_UTI: &str = "public.file-url";

/// The MIME type of a pasteboard type.
pub fn mime_of(uti: &str) -> String {
    match uti {
        TEXT_UTI => "text/plain;charset=utf-8".into(),
        FILE_UTI => "text/uri-list".into(),
        other => other.into(),
    }
}

/// The pasteboard type of a MIME type.
pub fn uti_of(mime: &str) -> String {
    match mime {
        "text/plain;charset=utf-8" | "text/plain" | "UTF8_STRING" | "STRING" => TEXT_UTI.into(),
        "text/uri-list" => FILE_UTI.into(),
        other => other.into(),
    }
}

/// A `text/uri-list` of file URLs (CRLF-terminated lines), from the URLs of a drop.
pub fn uri_list(urls: &[String]) -> String {
    urls.iter().map(|u| format!("{u}\r\n")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn operations_and_actions_map_both_ways() {
        assert_eq!(
            actions_of(OP_COPY | OP_MOVE),
            NHSHELL_DND_COPY | NHSHELL_DND_MOVE
        );
        assert_eq!(actions_of(OP_GENERIC), NHSHELL_DND_ASK);
        assert_eq!(
            ops_of(NHSHELL_DND_COPY | NHSHELL_DND_ASK),
            OP_COPY | OP_GENERIC
        );
        assert_eq!(op_of(NHSHELL_DND_MOVE), OP_MOVE);
        assert_eq!(op_of(0), 0);
        assert_eq!(
            first_action(NHSHELL_DND_MOVE | NHSHELL_DND_ASK),
            NHSHELL_DND_MOVE
        );
    }

    #[test]
    fn the_answer_follows_lisps_acceptance() {
        // refused
        assert_eq!(
            answer(false, OP_COPY, NHSHELL_DND_COPY, NHSHELL_DND_COPY),
            0
        );
        // not answered yet: the source's first
        assert_eq!(answer(true, OP_COPY | OP_MOVE, 0, 0), OP_COPY);
        // Lisp takes move only
        assert_eq!(
            answer(true, OP_COPY | OP_MOVE, NHSHELL_DND_MOVE, 0),
            OP_MOVE
        );
        // the preferred one when both have it
        assert_eq!(
            answer(
                true,
                OP_COPY | OP_MOVE,
                NHSHELL_DND_COPY | NHSHELL_DND_MOVE,
                NHSHELL_DND_MOVE
            ),
            OP_MOVE
        );
        // nothing in common
        assert_eq!(answer(true, OP_COPY, NHSHELL_DND_MOVE, 0), 0);
    }

    #[test]
    fn types_map_both_ways() {
        assert_eq!(
            mime_of("public.utf8-plain-text"),
            "text/plain;charset=utf-8"
        );
        assert_eq!(uti_of("text/uri-list"), "public.file-url");
        assert_eq!(uti_of(&mime_of("image/png")), "image/png");
    }

    #[test]
    fn a_uri_list_is_crlf_lines() {
        assert_eq!(
            uri_list(&["file:///a".into(), "file:///b c".into()]),
            "file:///a\r\nfile:///b c\r\n"
        );
    }
}

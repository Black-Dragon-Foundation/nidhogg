//! A local list (`sja::pick`): the palette's filter, score, order and groups, and the kept lists (pure).

use sja::pick::{LISTS_MAX, List, close, kept, load, top};

/// A palette: commands, a recent one, a file, an agent command.
fn palette() -> List {
    List::read(concat!(
        "0\t-1\tAgents: New Session\n",
        "0\t-1\tView: Toggle Sidebar\n",
        "0\t-1\tFile: Save All\n",
        "0\t0\tWindow: Split Right\n",
        "0\t-1\tSearch: Find in Files\n",
        "1\t-1\tFile: src/split.rs\n",
        "0\t-1\tView: Split Editor Down\n",
    ))
    .unwrap()
}

/// Pattern D. With nothing typed: the groups (Commands, Files, Agents), the recent run first, the rest as they came.
#[test]
fn nothing_typed_is_recent_first_by_group() {
    assert_eq!(palette().top("", 100), [3, 1, 4, 6, 2, 5, 0]);
}

/// Pattern D. Every word must be in the label (any case, the category too); a word at a word's start beats one inside,
/// the words in order beat them apart, and a recent run breaks near-ties only.
#[test]
fn typed_words_filter_and_score() {
    let l = palette();
    // "split": Window (recent, 2+1) and View: Split Editor Down (2), the file a file's
    assert_eq!(l.top("split", 100), [3, 6, 5]);
    // "split down": only View's label holds both words (Window's has no "down": filtered)
    assert_eq!(l.top("SPLIT down", 100), [6]);
    // the category holds "view": a match, scored 0 for it
    assert_eq!(l.top("view", 100), [1, 6]);
    // "ile" inside a word: still a match
    assert_eq!(l.top("ile", 100), [4, 2, 5]);
    assert!(l.top("nothing", 100).is_empty());
    // the words in order: "split editor" starts the word right after "split"'s (2 + 2+1), "split down" does not (2+2)
    let l = List::read("0\t-1\tA: Split Down Editor\n0\t-1\tB: Split Editor Down\n").unwrap();
    assert_eq!(l.top("split editor", 10), [1, 0]);
}

/// Pattern D. Only the first N come back, after the groups.
#[test]
fn only_the_first_n() {
    assert_eq!(palette().top("", 2), [3, 1]);
}

/// Pattern D. A line that is not `KIND RECENT LABEL` refuses the list.
#[test]
fn a_bad_line_refuses_the_list() {
    assert!(List::read("2\t-1\tX: y\n").is_none());
    assert!(List::read("0\tx\tX: y\n").is_none());
    assert!(List::read("0\t-1\n").is_none());
    assert_eq!(List::read("").unwrap().len(), 0);
}

/// Pattern D. Lists are kept by handle until closed, at most `LISTS_MAX`; the oldest goes first.
#[test]
fn lists_are_kept_by_handle_and_bounded() {
    let first = load(palette());
    assert_eq!(top(first, "split down", 10), Some(vec![6]));
    let more: Vec<u32> = (0..LISTS_MAX).map(|_| load(palette())).collect();
    assert_eq!(top(first, "", 1), None, "the oldest went");
    assert!(kept() <= LISTS_MAX);
    for h in more {
        assert!(close(h));
    }
    assert!(!close(first));
}

# hlidskjalf

Hlidskjalf ("the high seat", Odin's seat that sees every world) is the line model behind the minimap, the scroll
bar's marks and folding. It keeps, for each buffer text a window shows with the ruler or the minimap on:

- **Lines:** each line's length and indent, in chunks of 1–4k lines with a Fenwick tree over the chunks' character
  and line totals. Position to line and line to position are O(log n); an edit rewrites only the chunk it touched.
- **Marks:** the positions an owner (drgnx's marks) sends per kind (changes, problems, hits, breakpoints…), kept
  sorted and shifted on every edit, so next/previous mark and counts in a range never touch Lisp.
- **Folds:** fold ranges per source (tree-sitter, the hub, headings, indentation), with their nesting depth, shifted on
  edits and capped (the smallest dropped first).
- **Hidden lines:** the ranges drgnx's folds hide, so the scroll bar and minimap measure visible lines.
- **The trail:** the last 64 visited lines (`nidhogg-lines-trail-clear` forgets them).
- **MARK sections:** with an expression (`nidhogg-lines-set-section-regexp`, Rust regex syntax), the lines that match
  it and their names (the group `name`, else the first group, else the match), found at each build and in each edit's
  lines, kept sorted and shifted like the marks; the minimap draws them as labels (`sections.rs`).

Positions are Emacs's: 1-based characters over the whole text, narrowing ignored. Emacs feeds the model from
`src/nidhogg-lines.c`: the change hook at every buffer-text change site (beside the edit history's), a rebuild when the
text is replaced as a whole, and the `nidhogg-lines-*` functions.

## Crate rules

- Standard library, and `regex-lite` (exact pin, no default features) for the MARK sections' expression, which
  drgnx documents in Rust regex syntax; nothing else.
- The first build of a text of 1 MB or more runs on a thread of its own, from a copy of the text; the edits made
  meanwhile are kept and replayed when it is installed. Everything else runs on the thread that holds Emacs's global
  lock.
- The C ABI is `include/hlidskjalf.h` (`src/abi.rs`); every entry point catches a panic.
- Builds on Rust 1.93 (the glue's floor). Carried by the glitnir crate, so libglitnir.a and the glue's libnhglue.a
  hold it.

## Checks

`check.sh --container fedora-45`: fmt, clippy (`-D warnings`), the tests (`tests/model.rs` replays random edits against
a plain text), the licence policy.

//! The model against a plain text: every edit the host's change hook would send, checked line by line.

use hlidskjalf::{BLANK, Fold, Mark, Model, Run, Span};

/// A text and its model, edited together as the host does.
struct Pair {
    text: Vec<u8>,
    m: Model,
}

impl Pair {
    fn new(text: &str, background: bool) -> Pair {
        let text = text.as_bytes().to_vec();
        let m = Model::build(&[&text], false, 8, background);
        Pair { text, m }
    }

    /// Replace the OLD characters at 1-based START with NEW, sending the model the edit's whole lines.
    fn edit(&mut self, start: usize, old: usize, new: &str) {
        let s = start - 1;
        self.text.splice(s..s + old, new.bytes());
        let new_end = s + new.len();
        let ls = self.text[..s]
            .iter()
            .rposition(|&b| b == b'\n')
            .map_or(0, |i| i + 1);
        let le = self.text[new_end..]
            .iter()
            .position(|&b| b == b'\n')
            .map_or(self.text.len(), |i| new_end + i + 1);
        let seg = self.text[ls..le].to_vec();
        self.m
            .edit(
                start as u64,
                (start + old) as u64,
                (start + new.len()) as u64,
                &[&seg],
            )
            .expect("edit applies");
    }

    fn check(&mut self) {
        self.m.poll(true);
        let lines: Vec<&[u8]> = self.text.split(|&b| b == b'\n').collect();
        let idx = self.m.index().expect("built");
        assert_eq!(idx.lines(), lines.len() as u64, "line count");
        assert_eq!(idx.chars(), self.text.len() as u64);
        let mut off = 0u64;
        for (i, l) in lines.iter().enumerate() {
            assert_eq!(idx.start_of(i as u64), off, "start of line {i}");
            let nl = u64::from(i + 1 < lines.len());
            assert_eq!(
                u64::from(idx.line(i as u64).len),
                l.len() as u64 + nl,
                "length of line {i}"
            );
            let blank = l.iter().all(|&b| b == b' ' || b == b'\t');
            assert_eq!(idx.line(i as u64).indent == BLANK, blank, "blank line {i}");
            off += l.len() as u64 + nl;
        }
    }
}

/// A small deterministic generator (xorshift).
struct Rng(u64);
impl Rng {
    fn next(&mut self, n: usize) -> usize {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 % n as u64) as usize
    }
}

fn big_text(lines: usize) -> String {
    (0..lines)
        .map(|i| format!("{}line {i}\n", " ".repeat(i % 5)))
        .collect()
}

#[test]
fn random_edits_match_the_text() {
    let mut p = Pair::new(&big_text(9000), false);
    let mut r = Rng(0x9e37_79b9_7f4a_7c15);
    let pieces = [
        "",
        "x",
        "\n",
        "a\nb",
        "\n\n\n",
        "  indented\n",
        &"z\n".repeat(3000),
    ];
    for step in 0..400 {
        let len = p.text.len();
        let start = 1 + r.next(len + 1);
        let old = r.next((len + 1 - (start - 1)).min(if step % 50 == 0 { 40_000 } else { 30 }) + 1);
        let old = old.min(len + 1 - start);
        let new = pieces[r.next(pieces.len())];
        p.edit(start, old, new);
        if step % 20 == 0 {
            p.check();
        }
    }
    p.check();
}

#[test]
fn sum_tree_edit_is_local() {
    let mut p = Pair::new(&big_text(250_000), false);
    let chunks = p.m.index().unwrap().chunks();
    assert!(chunks > 50, "{chunks} chunks");
    // a newline typed in the middle rewrites one chunk only
    let mid = p.text.len() / 2;
    p.edit(mid, 0, "\n");
    let after = p.m.index().unwrap();
    assert_eq!(after.chunks(), chunks);
    assert_eq!(after.touched(), 1, "one chunk rewritten");
    p.check();
}

#[test]
fn marks_shift_on_edit() {
    let mut p = Pair::new("aaa\nbbb\nccc\nddd\n", false);
    let mk = |pos, end| Mark {
        pos,
        end,
        lane: 0,
        class: 0,
        prio: 0,
        kind: 0,
    };
    p.m.set_marks(1, &[mk(5, 8), mk(13, 0)]);
    p.m.set_marks(2, &[mk(9, 0)]);
    // insert two lines at the start: every mark moves by their length
    p.edit(1, 0, "x\ny\n");
    let pos: Vec<(u64, u64, u8)> = p.m.marks().iter().map(|m| (m.pos, m.end, m.kind)).collect();
    assert_eq!(pos, vec![(9, 12, 1), (13, 0, 2), (17, 0, 1)]);
    // delete over a mark: it collapses to the deletion's start
    p.edit(12, 4, "");
    let pos: Vec<u64> = p.m.marks().iter().map(|m| m.pos).collect();
    assert_eq!(pos, vec![9, 12, 13]);
    assert_eq!(p.m.next_mark(9, 1 << 1, 1), Some(13));
    assert_eq!(p.m.next_mark(13, 1 << 1, -1), Some(9));
    assert_eq!(p.m.next_mark(13, 1 << 2, 1), None);
    let c = p.m.counts(1, 13);
    assert_eq!((c[1], c[2]), (1, 1));
    p.check();
}

#[test]
fn fold_table_shift() {
    let mut p = Pair::new("fn a() {\n  x\n  y\n}\nfn b() {\n  z\n}\n", false);
    let f = |beg, end| Fold {
        beg,
        end,
        kind: 1,
        depth: 0,
        source: 0,
    };
    p.m.set_folds(3, &[f(1, 19), f(20, 33), f(10, 15)], 100);
    let got: Vec<(u64, u64, u8)> =
        p.m.folds(0, u64::MAX)
            .map(|f| (f.beg, f.end, f.depth))
            .collect();
    assert_eq!(got, vec![(1, 19, 0), (10, 15, 1), (20, 33, 0)]);
    // a line typed inside the first function moves the second, and widens the first
    p.edit(13, 0, "  w\n");
    let got: Vec<(u64, u64)> = p.m.folds(0, u64::MAX).map(|f| (f.beg, f.end)).collect();
    assert_eq!(got, vec![(1, 23), (10, 19), (24, 37)]);
    // the fold to toggle on "fn b" is the one starting on that line; inside "x" the innermost holding it
    assert_eq!(p.m.fold_at(26).map(|f| f.beg), Some(24));
    assert_eq!(p.m.fold_at(11).map(|f| f.beg), Some(10));
    // the cap keeps the largest
    p.m.set_folds(3, &[f(1, 23), f(10, 19), f(24, 37)], 2);
    assert_eq!(p.m.fold_count(), 2);
    assert!(p.m.folds(0, u64::MAX).all(|f| f.end - f.beg > 9));
    // a deletion over a whole fold drops it
    p.edit(24, 13, "");
    assert_eq!(p.m.fold_count(), 1);
    p.check();
}

#[test]
fn hidden_lines_leave_the_fraction() {
    // ten lines of "nn\n": 30 characters
    let text: String = (0..10).map(|i| format!("{i:02}\n")).collect();
    let mut p = Pair::new(&text, false);
    let zv = 31;
    assert_eq!(p.m.fraction(1, 1, zv), Some(0.0));
    assert_eq!(p.m.fraction(16, 1, zv), Some(0.5));
    // hide lines 2–6 (a fold from the end of line 1 to the end of line 6): 5 lines left
    p.m.set_hidden(&[(3, 18)]);
    let f7 = p.m.fraction(19, 1, zv).unwrap();
    assert!(
        (f7 - 1.0 / 5.0).abs() < 1e-9,
        "line 7 after one visible line: {f7}"
    );
    // a hidden line takes its fold's place
    assert_eq!(p.m.fraction(10, 1, zv), p.m.fraction(4, 1, zv));
    assert_eq!(p.m.fraction(zv, 1, zv), Some(1.0));
}

#[test]
fn a_background_build_replays_the_edits_made_meanwhile() {
    let mut p = Pair::new(&big_text(200_000), true);
    p.edit(5, 0, "new\nlines\n");
    p.edit(1, 3, "");
    p.check();
    assert!(!p.m.building());
}

#[test]
fn the_hubs_ranges_come_as_lines() {
    // no newline at the end: a range past the text is cut to its last line, not to one past its end
    let mut p = Pair::new("a {\n  b\n}\nc {\n  d\n}", false);
    // lines from 1: 1–3 and 4–6 (a block, then a function), one past the text cut to it, one of a single line none,
    // and of two starting on line 4 the larger
    assert!(p.m.set_fold_lines(1, &[(1, 3, 0), (4, 5, 0), (4, 99, 1), (2, 2, 0), (0, 3, 0)]));
    let got: Vec<(u64, u64, u8, u8)> =
        p.m.folds(0, u64::MAX)
            .map(|f| (f.beg, f.end, f.kind, f.source))
            .collect();
    // the text ends at 20, after the last line's `}`
    assert_eq!(got, vec![(1, 10, 0, 1), (11, 20, 1, 1)]);
    assert!(p.m.has_folds_of(1));
    assert!(!p.m.has_folds_of(0));
    // while the lines are built: nothing changes, false
    let mut b = Pair::new(&big_text(200_000), true);
    assert!(b.m.building());
    assert!(!b.m.set_fold_lines(1, &[(1, 3, 0)]));
    p.check();
}

#[test]
fn a_parsers_ranges_sent_while_the_lines_are_built_are_kept() {
    let mut p = Pair::new(&big_text(200_000), true);
    assert!(p.m.building());
    // a node from line 0's third character to line 1's third, sent before the lines exist…
    p.m.replace_folds(
        0,
        1,
        u64::MAX,
        &[Fold {
            beg: 3,
            end: 10,
            kind: 1,
            depth: 0,
            source: 0,
        }],
    );
    // …moves with an edit made meanwhile, and is lines 0–1 once they are built
    p.edit(1, 0, "xy");
    p.m.poll(true);
    let got: Vec<(u64, u64, u8)> =
        p.m.folds(0, u64::MAX)
            .map(|f| (f.beg, f.end, f.kind))
            .collect();
    assert_eq!(got, vec![(1, 17, 1)]);
    p.check();
}

#[test]
fn a_bad_edit_makes_the_model_stale() {
    let mut p = Pair::new("abc\ndef\n", false);
    // the segment says 2 characters where the edit says 3
    assert!(p.m.edit(1, 1, 2, &[b"x\n"]).is_err());
    assert!(p.m.stale());
    p.m.rebuild(&[&p.text], false);
    assert!(!p.m.stale());
}

#[test]
fn the_rows_leave_out_the_hidden_lines_and_the_generation_counts_changes() {
    // line i: i spaces, a digit and a newline (indent i, i + 2 characters)
    let text: String = (0..10).map(|i| format!("{}{i}\n", " ".repeat(i))).collect();
    let mut p = Pair::new(&text, false);
    let total = p.m.index().unwrap().lines();
    assert_eq!(p.m.visible_lines(), Some(total));
    let indents = |v: Vec<hlidskjalf::Line>| v.iter().map(|l| l.indent).collect::<Vec<_>>();
    assert_eq!(indents(p.m.visible_rows(0, 3).unwrap()), [0, 1, 2]);
    let g = p.m.generation();
    // hide lines 2–6: (the end of line 1, the start of line 6]
    p.m.set_hidden(&[(5, 28)]);
    assert!(p.m.generation() > g);
    assert_eq!(p.m.visible_lines(), Some(total - 5));
    assert_eq!(indents(p.m.visible_rows(0, 4).unwrap()), [0, 1, 7, 8]);
    assert_eq!(indents(p.m.visible_rows(2, 1).unwrap()), [7]);
    assert_eq!(p.m.visible_line(1), Some(1));
    assert_eq!(p.m.visible_line(2), Some(7));
    assert_eq!(p.m.visible_line(total - 5), None);
    // the rows end with the text
    assert_eq!(p.m.visible_rows(total - 6, 9).unwrap().len(), 1);
    let g = p.m.generation();
    p.edit(1, 0, "x");
    assert!(p.m.generation() > g);
    let g = p.m.generation();
    p.m.rebuild(&[&p.text], false);
    assert!(p.m.generation() > g);
}

fn ranges(m: &mut Model) -> Vec<(u64, u64, u8, u8)> {
    m.folds(0, u64::MAX)
        .map(|f| (f.beg, f.end, f.depth, f.source))
        .collect()
}

#[test]
fn a_parsers_ranges_are_replaced_only_where_the_text_changed() {
    let mut p = Pair::new(
        "fn a() {\n  x\n}\nfn b() {\n  y\n  z\n}\nfn c() {}\n",
        false,
    );
    // the same lines as the indentation ranges: their positions are what a node's whole lines give
    p.m.set_derived_folds(true, None).unwrap();
    let ind = ranges(&mut p.m);
    let ((a0, a1), (b0, b1)) = ((ind[0].0, ind[0].1), (ind[1].0, ind[1].1));
    p.m.set_derived_folds(false, None).unwrap();
    let node = |beg: u64, end: u64, kind: u8| Fold {
        beg,
        end,
        kind,
        depth: 0,
        source: 9,
    };
    let c = p.m.pos_of(7).unwrap();
    // a node from mid-line to just after its closing brace covers its whole lines; a node of one line is no range
    p.m.replace_folds(
        0,
        1,
        u64::MAX,
        &[
            node(a0 + 3, a1 + 1, 1),
            node(b0 + 3, b1 + 1, 1),
            node(c, c + 9, 1),
        ],
    );
    assert_eq!(ranges(&mut p.m), vec![(a0, a1, 0, 0), (b0, b1, 0, 0)]);
    // a change inside b: b's range replaced (found again, of another kind), a's left alone, nothing doubled
    p.m.replace_folds(
        0,
        b0 + 10,
        b0 + 11,
        &[node(b0, b1, 2), node(b0 + 2, b1 - 1, 2)],
    );
    let got: Vec<(u64, u64, u8)> =
        p.m.folds(0, u64::MAX)
            .map(|f| (f.beg, f.end, f.kind))
            .collect();
    assert_eq!(got, vec![(a0, a1, 1), (b0, b1, 2)]);
    // the node went: so does its range, and only its
    p.m.replace_folds(0, b0 + 10, b0 + 11, &[]);
    assert_eq!(ranges(&mut p.m), vec![(a0, a1, 0, 0)]);
    // two nodes of one range: the more specific kind stays, whichever came first
    p.m.replace_folds(0, b0, b1, &[node(b0, b1, 5), node(b0, b1, 0)]);
    p.m.replace_folds(0, a0, a1, &[node(a0, a1, 0), node(a0, a1, 5)]);
    let kinds: Vec<u8> = p.m.folds(0, u64::MAX).map(|f| f.kind).collect();
    assert_eq!(kinds, vec![5, 5]);
    p.m.replace_folds(0, b0, b1, &[]);
    // another source's ranges are never touched
    p.m.set_folds(5, &[node(b0, b1, 0)], 100);
    p.m.replace_folds(0, 1, u64::MAX, &[]);
    assert_eq!(ranges(&mut p.m), vec![(b0, b1, 0, 5)]);
    p.check();
}

#[test]
fn indentation_ranges_follow_the_lines() {
    let mut p = Pair::new("fn a() {\n  x\n\n  y\n}\nfn b() {\n  z\n}\n", false);
    assert!(ranges(&mut p.m).is_empty(), "none until asked for");
    p.m.set_derived_folds(true, None).unwrap();
    // each from its first line's start to the last deeper line's end (the blank line inside attached), its newline out
    assert_eq!(ranges(&mut p.m), vec![(1, 18, 0, 4), (21, 33, 0, 4)]);
    assert_eq!(p.m.fold_at(25).map(|f| f.beg), Some(21));
    // a deeper line typed into fn b widens its range; fn a's stays
    p.edit(33, 0, "\n    w");
    assert_eq!(
        ranges(&mut p.m),
        vec![(1, 18, 0, 4), (21, 39, 0, 4), (30, 39, 1, 4)]
    );
    // the owner's ranges stay beside them
    p.m.set_folds(
        5,
        &[Fold {
            beg: 1,
            end: 10,
            kind: 0,
            depth: 0,
            source: 0,
        }],
        100,
    );
    assert_eq!(ranges(&mut p.m).len(), 4);
    p.m.set_derived_folds(false, None).unwrap();
    assert_eq!(ranges(&mut p.m), vec![(1, 10, 0, 5)]);
    p.check();
}

#[test]
fn heading_ranges_end_before_the_next_heading_of_their_level() {
    for background in [false, true] {
        let text = "# A\ntext\n## B\nmore\n# C\nlast\n";
        let mut p = Pair::new(text, false);
        p.m.set_derived_folds(false, Some(r"^(#+)\s")).unwrap();
        // the headings are found when the lines are built again, as the host does after setting the expression
        p.m.rebuild(&[text.as_bytes()], background);
        p.m.poll(true);
        assert_eq!(
            ranges(&mut p.m),
            vec![(1, 19, 0, 2), (10, 19, 1, 2), (20, 28, 0, 2)],
            "background {background}"
        );
        assert!(p.m.folds(0, u64::MAX).all(|f| f.kind == 8));
        // a line typed under A moves every range after it
        p.edit(5, 0, "x\n");
        assert_eq!(
            ranges(&mut p.m),
            vec![(1, 21, 0, 2), (12, 21, 1, 2), (22, 30, 0, 2)]
        );
        // B becomes a top heading: A ends before it
        p.edit(12, 1, "");
        assert_eq!(
            ranges(&mut p.m),
            vec![(1, 11, 0, 2), (12, 20, 0, 2), (21, 29, 0, 2)]
        );
        assert!(p.m.set_derived_folds(false, Some("(")).is_err());
        p.check();
    }
}

/// Line L's runs in use, as (col, len, class).
fn runs(p: &mut Pair, l: u64) -> Vec<(u16, u8, u8)> {
    p.m.index()
        .expect("built")
        .line(l)
        .runs
        .iter()
        .filter(|r| r.len > 0)
        .map(|&Run { col, len, class }| (col, len, class))
        .collect()
}

#[test]
fn class_spans_become_runs_on_their_lines() {
    // "fn a() {" / "    s = \"x" / "y\";" / "}": positions from 1
    let mut p = Pair::new("fn a() {\n    s = \"x\ny\";\n}\n", false);
    let span = |beg, end, class| Span { beg, end, class };
    assert!(p.m.replace_classes(
        1,
        26,
        &[
            // a block over everything (class 9) under the rest: shorter spans paint over it
            span(1, 26, 9),
            span(1, 3, 1),   // fn
            span(4, 5, 2),   // a
            span(14, 15, 7), // s, then text (0) over it
            span(14, 15, 0),
            span(18, 23, 4), // the string, over two lines
        ]
    ));
    // line 0: fn, then the block, a, the block (its newline left out)
    assert_eq!(
        runs(&mut p, 0),
        vec![(0, 2, 1), (2, 1, 9), (3, 1, 2), (4, 4, 9)]
    );
    // line 1: the block, `s` (the text span over it paints nothing), the block, the string from column 8
    assert_eq!(
        runs(&mut p, 1),
        vec![(0, 4, 9), (4, 1, 7), (5, 3, 9), (8, 2, 4)]
    );
    assert_eq!(runs(&mut p, 2), vec![(0, 2, 4), (2, 1, 9)]);
    assert_eq!(runs(&mut p, 3), vec![(0, 1, 9)]);
    // a range replaces the runs of the lines it touches only
    assert!(p.m.replace_classes(14, 15, &[span(14, 15, 3)]));
    assert_eq!(runs(&mut p, 1), vec![(4, 1, 3)]);
    assert_eq!(runs(&mut p, 0).len(), 4);
    // an edit that rewrites a line clears its runs; the others keep theirs
    p.edit(1, 2, "pub fn");
    assert!(runs(&mut p, 0).is_empty());
    assert_eq!(runs(&mut p, 1), vec![(4, 1, 3)]);
    // at most six runs a line, the leftmost
    let mut q = Pair::new(&"a ".repeat(20), false);
    let many: Vec<Span> = (0..10)
        .map(|i| span(1 + 2 * i, 2 + 2 * i, 1 + i as u8))
        .collect();
    assert!(q.m.replace_classes(1, 40, &many));
    assert_eq!(
        runs(&mut q, 0),
        (0..6u8)
            .map(|i| (u16::from(2 * i), 1, 1 + i))
            .collect::<Vec<_>>()
    );
    // while the lines are built: nothing changes, false
    let mut b = Pair::new(&big_text(200_000), true);
    assert!(b.m.building());
    assert!(!b.m.replace_classes(1, 3, &many));
    b.m.poll(true);
    assert!(
        b.m.take_classes_missed(),
        "the producer runs again once they are built"
    );
    assert!(!b.m.take_classes_missed());
    assert!(!p.m.take_classes_missed());
    p.check();
}

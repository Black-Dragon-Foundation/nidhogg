//! The model against a plain text: every edit the host's change hook would send, checked line by line.

use hlidskjalf::{BLANK, Fold, Mark, Model};

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

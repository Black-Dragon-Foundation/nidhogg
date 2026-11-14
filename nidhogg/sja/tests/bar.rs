//! A bar's layout (`sja::bar`): the fit's steps in order, a terminal's columns, and the spec line (pure).

use sja::bar::{Cut, Cutting, Fit, Seg, Spec, fit};

fn seg(drop: Option<i64>, width: i64) -> Seg {
    Seg {
        drop,
        width,
        cut: None,
    }
}

/// A status bar in pixels: the branch (cuttable: 12 px of room, eleven 8 px characters) and problems on the left;
/// lsp (dropped third), the position (never in the drop order) and eol (dropped first) on the right.  270 px whole.
fn pixels(width: i64) -> Spec {
    Spec {
        width,
        pad: 8,
        space: 4,
        left_sep: 2,
        right_sep: 2,
        ellipsis: 6,
        cut_min: 56,
        columns: false,
        left: vec![
            Seg {
                drop: None,
                width: 100,
                cut: Some(Cut {
                    extra: 12,
                    chars: vec![8; 11],
                }),
            },
            seg(None, 30),
        ],
        right: vec![seg(Some(3), 40), seg(None, 50), seg(Some(0), 20)],
    }
}

fn cut(keep: usize, ellipsis: bool, width: i64) -> Option<Cutting> {
    Some(Cutting {
        index: 0,
        keep,
        ellipsis,
        width,
    })
}

#[test]
fn a_bar_that_fits_keeps_everything() {
    assert_eq!(
        fit(&pixels(270)),
        Fit {
            left: vec![0, 1],
            right: vec![0, 1, 2],
            cut: None,
            right_width: 114
        }
    );
}

#[test]
fn the_drop_order_goes_first_and_stops_once_it_fits() {
    // eol (rank 0) goes and the bar fits: lsp (rank 3) stays
    let f = fit(&pixels(250));
    assert_eq!(
        (f.left, f.right, f.cut, f.right_width),
        (vec![0, 1], vec![0, 1], None, 92)
    );
}

#[test]
fn the_branch_is_cut_before_another_segment_goes_for_it() {
    // past the drop order 6 px too wide: the branch is cut to 94 px (9 characters and the ellipsis), nothing else goes
    let f = fit(&pixels(200));
    assert_eq!(
        (f.left, f.right, f.cut),
        (vec![0, 1], vec![1], cut(9, true, 90))
    );
}

#[test]
fn past_the_short_branch_the_right_run_goes_last_first() {
    // the branch stops at its minimum (56 px: 4 characters), then the position goes, then it fits
    let f = fit(&pixels(120));
    assert_eq!(
        (f.left, f.right, f.cut, f.right_width),
        (vec![0, 1], vec![], cut(4, true, 50), 0)
    );
}

#[test]
fn then_the_left_run_goes_but_the_branch_and_last_the_branch_is_cut_to_fit() {
    let f = fit(&pixels(60));
    assert_eq!(
        (f.left, f.right, f.cut),
        (vec![0], vec![], cut(3, true, 42))
    );
}

/// A terminal's bar: a 20-column branch alone, the ellipsis ELLIPSIS columns wide.
fn columns(width: i64, ellipsis: i64) -> Spec {
    Spec {
        width,
        pad: 1,
        space: 1,
        left_sep: 3,
        right_sep: 2,
        ellipsis,
        cut_min: 14,
        columns: true,
        left: vec![Seg {
            drop: None,
            width: 20,
            cut: Some(Cut {
                extra: 0,
                chars: vec![1; 20],
            }),
        }],
        right: vec![],
    }
}

#[test]
fn columns_cut_with_the_ellipsis_while_it_fits_and_without_it_after() {
    assert_eq!(fit(&columns(10, 1)).cut, cut(7, true, 8));
    // "..." is wider than the 2 columns left: the text is cut to them, as `truncate-string-to-width' does
    assert_eq!(fit(&columns(4, 3)).cut, cut(2, false, 2));
}

#[test]
fn a_pixel_cut_that_keeps_no_character_is_empty() {
    let spec = Spec {
        width: 10,
        pad: 0,
        space: 4,
        left_sep: 2,
        right_sep: 2,
        ellipsis: 6,
        cut_min: 56,
        columns: false,
        left: vec![Seg {
            drop: None,
            width: 30,
            cut: Some(Cut {
                extra: 12,
                chars: vec![9, 9],
            }),
        }],
        right: vec![],
    };
    let f = fit(&spec);
    assert_eq!((f.left, f.cut), (vec![0], cut(0, false, 0)));
}

#[test]
fn the_spec_line_reads_back_and_a_wrong_one_is_none() {
    let line = "1 200 8 4 2 2 6 56 0 2 3  -1 100 1 12 11 8 8 8 8 8 8 8 8 8 8 8  -1 30 0  3 40 0  -1 50 0  0 20 0";
    assert_eq!(Spec::read(line), Some(pixels(200)));
    assert_eq!(
        Spec::read(&line.replacen('1', "2", 1)),
        None,
        "another version"
    );
    assert_eq!(Spec::read(&format!("{line} 7")), None, "a token left over");
    assert_eq!(
        Spec::read("1 200 8 4 2 2 6 56 0 1 0 -1 30"),
        None,
        "a segment cut short"
    );
    assert_eq!(
        Spec::read("1 200 8 4 2 2 6 56 0 1 0 -1 30 2"),
        None,
        "an unknown cut flag"
    );
    assert_eq!(Spec::read("1 x"), None);
}

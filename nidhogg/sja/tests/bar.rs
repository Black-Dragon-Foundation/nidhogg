//! A bar's layout (`sja::bar`): the fit's steps in order, a terminal's columns, and the spec line (pure).

use sja::bar::{Center, Cut, Cutting, Fit, Placed, Seg, Spec, fit};

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
        center: None,
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
            right_width: 114,
            center: None
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
        center: None,
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
        center: None,
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

/// A title row: the mark and the branch on the left (60 + 40), the controls on the right (90), a search box of 200
/// centred as if the right held ROOM 150 (the chips it keeps room for), 10 apart from either side at least.
fn title_row(width: i64) -> Spec {
    Spec {
        width,
        pad: 0,
        space: 0,
        left_sep: 0,
        right_sep: 0,
        ellipsis: 0,
        cut_min: 0,
        columns: false,
        left: vec![seg(None, 60), seg(None, 40)],
        right: vec![seg(None, 90)],
        center: Some(Center {
            width: 200,
            gap: 10,
            room: 150,
        }),
    }
}

#[test]
fn a_centred_segment_sits_between_the_runs_or_goes() {
    // 800: start = 100 + 10 + (800 - 100 - 10 - 200 - 10 - 150) / 2 = 275; gaps 175 and 800 - 275 - 200 - 90 = 235
    assert_eq!(
        fit(&title_row(800)).center,
        Some(Placed {
            kept: true,
            before: 175,
            after: 235
        })
    );
    // 400: start = 110 + (400 - 470) / 2 = 75 < 110: it goes, one gap fills the row
    assert_eq!(
        fit(&title_row(400)).center,
        Some(Placed {
            kept: false,
            before: 210,
            after: 0
        })
    );
    // no centre: none
    assert_eq!(fit(&pixels(270)).center, None);
}

#[test]
fn a_spec_with_a_centre_reads_back() {
    assert_eq!(
        Spec::read("2 800 0 0 0 0 0 0 0 2 1  -1 60 0  -1 40 0  -1 90 0  200 10 150"),
        Some(title_row(800))
    );
    assert_eq!(
        Spec::read("2 800 0 0 0 0 0 0 0 2 1  -1 60 0  -1 40 0  -1 90 0"),
        None,
        "version 2 without its centre"
    );
}

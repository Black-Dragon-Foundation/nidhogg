use super::*;

fn at(text: &str, line_start: usize, point: usize, mark: usize) -> Surrounding {
    Surrounding {
        text: text.into(),
        line_start,
        point,
        mark,
        marked: 0,
    }
}

/// The selection is the region, whichever side point is on; without an active mark (C pushes point twice) it is
/// point, empty.  Characters, not bytes.
#[test]
fn selected_range_is_the_region() {
    assert_eq!(at("héllo", 10, 13, 13).selected(), (13, 0));
    assert_eq!(at("héllo", 10, 11, 14).selected(), (11, 3));
    assert_eq!(at("héllo", 10, 14, 11).selected(), (11, 3));
}

/// Marked text sits at point.
#[test]
fn marked_range_starts_at_point() {
    let mut s = at("ab", 0, 1, 1);
    assert_eq!(s.marked_range(), None);
    s.marked = 2;
    assert_eq!(s.marked_range(), Some((1, 2)));
}

/// A substring is clipped to the line, and its actual range said; outside the line there is none.
#[test]
fn substring_is_clipped_to_the_line() {
    let s = at("héllo", 10, 10, 10);
    assert_eq!(s.substring(11, 2), Some(("él".into(), (11, 2))));
    assert_eq!(s.substring(13, 10), Some(("lo".into(), (13, 2))));
    assert_eq!(s.substring(5, 7), Some(("hé".into(), (10, 2))));
    assert_eq!(s.substring(15, 1), None);
    assert_eq!(s.substring(3, 4), None);
    assert_eq!(s.substring(12, 0), None);
}

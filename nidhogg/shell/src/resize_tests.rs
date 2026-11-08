use super::*;

const F: Rect = Rect {
    x: 100.0,
    y: 100.0,
    w: 400.0,
    h: 300.0,
};
const MIN: (f64, f64) = (50.0, 40.0);

#[test]
fn hit_finds_edges_and_corners() {
    assert_eq!(hit(F, (98.0, 250.0), BAND), Some(LEFT));
    assert_eq!(hit(F, (502.0, 250.0), BAND), Some(RIGHT));
    assert_eq!(hit(F, (300.0, 101.0), BAND), Some(BOTTOM));
    assert_eq!(hit(F, (300.0, 403.0), BAND), Some(TOP));
    assert_eq!(hit(F, (99.0, 99.0), BAND), Some(LEFT | BOTTOM));
    // along the right edge, near the top: the corner
    assert_eq!(hit(F, (501.0, 395.0), BAND), Some(RIGHT | TOP));
    // the middle, and well outside, are not the band
    assert_eq!(hit(F, (300.0, 250.0), BAND), None);
    assert_eq!(hit(F, (80.0, 250.0), BAND), None);
}

#[test]
fn drag_moves_the_grabbed_edges() {
    let none = Mods::default();
    assert_eq!(
        drag(F, RIGHT, 30.0, 99.0, none, MIN),
        Rect {
            x: 100.0,
            y: 100.0,
            w: 430.0,
            h: 300.0
        }
    );
    // the left edge: the right one stays
    assert_eq!(
        drag(F, LEFT, 30.0, 0.0, none, MIN),
        Rect {
            x: 130.0,
            y: 100.0,
            w: 370.0,
            h: 300.0
        }
    );
    // bottom-left corner, outward
    assert_eq!(
        drag(F, LEFT | BOTTOM, -10.0, -20.0, none, MIN),
        Rect {
            x: 90.0,
            y: 80.0,
            w: 410.0,
            h: 320.0
        }
    );
}

#[test]
fn option_resizes_about_the_centre() {
    let m = Mods {
        symmetric: true,
        aspect: false,
    };
    assert_eq!(
        drag(F, RIGHT, 20.0, 0.0, m, MIN),
        Rect {
            x: 80.0,
            y: 100.0,
            w: 440.0,
            h: 300.0
        }
    );
}

#[test]
fn shift_keeps_the_aspect_ratio() {
    let m = Mods {
        symmetric: false,
        aspect: true,
    };
    // the right edge drives both: 400x300 -> 480x360, the bottom-left stays
    assert_eq!(
        drag(F, RIGHT, 80.0, 0.0, m, MIN),
        Rect {
            x: 100.0,
            y: 100.0,
            w: 480.0,
            h: 360.0
        }
    );
    // a corner takes the larger scale
    assert_eq!(
        drag(F, RIGHT | TOP, 40.0, 60.0, m, MIN),
        Rect {
            x: 100.0,
            y: 100.0,
            w: 480.0,
            h: 360.0
        }
    );
}

#[test]
fn option_and_shift_together() {
    let m = Mods {
        symmetric: true,
        aspect: true,
    };
    // the right edge by 40 grows the width by 80 about the centre, the height with it
    assert_eq!(
        drag(F, RIGHT, 40.0, 0.0, m, MIN),
        Rect {
            x: 60.0,
            y: 70.0,
            w: 480.0,
            h: 360.0
        }
    );
}

#[test]
fn the_minimum_size_holds() {
    let none = Mods::default();
    // the left edge past the right one: the right edge stays, the width stops at the minimum
    assert_eq!(
        drag(F, LEFT, 1000.0, 0.0, none, MIN),
        Rect {
            x: 450.0,
            y: 100.0,
            w: 50.0,
            h: 300.0
        }
    );
    let m = Mods {
        symmetric: false,
        aspect: true,
    };
    // with the aspect held, the scale stops where either side reaches its minimum
    let r = drag(F, RIGHT, -1000.0, 0.0, m, MIN);
    assert!(
        (r.h - 40.0).abs() < 1e-9 && (r.w - 160.0 / 3.0).abs() < 1e-9,
        "{r:?}"
    );
}

#[test]
fn steps_coalesce_to_the_newest() {
    let mut c = Coalesce::default();
    assert_eq!(c.take(), None::<u32>);
    c.push(1);
    c.push(2);
    c.push(3);
    assert_eq!(c.take(), Some(3));
    assert_eq!(c.take(), None);
}

//! A child frame's chrome as a slot vector.  Lisp resolves it (`glitnir--chrome-vector`), C copies the numbers into a
//! float array without decoding them (`nhwl_chrome_vector`), and this module checks and decodes it.  The slot numbers
//! are the same in `lisp/glitnir.el` (`glitnir--chrome-slots`), `include/nhglue.h` (`NHG_CHROME_*`) and here.

use crate::child::NhgChrome;
use crate::guard::GlueError;
use glitnir::ease::Curve;
use glitnir::motion::{Kind, Leg, MotionSpec};

pub const SLOT_ON: usize = 0;
pub const SLOT_RADIUS: usize = 1;
pub const SLOT_BLUR: usize = 2;
pub const SLOT_OX: usize = 3;
pub const SLOT_OY: usize = 4;
pub const SLOT_SPREAD: usize = 5;
pub const SLOT_OPACITY: usize = 6;
pub const SLOT_RGB: usize = 7;
pub const SLOT_SHOW_KIND: usize = 8;
pub const SLOT_SHOW_MS: usize = 9;
pub const SLOT_SHOW_X1: usize = 10;
pub const SLOT_SHOW_Y1: usize = 11;
pub const SLOT_SHOW_X2: usize = 12;
pub const SLOT_SHOW_Y2: usize = 13;
pub const SLOT_HIDE_KIND: usize = 14;
pub const SLOT_HIDE_MS: usize = 15;
pub const SLOT_HIDE_X1: usize = 16;
pub const SLOT_HIDE_Y1: usize = 17;
pub const SLOT_HIDE_X2: usize = 18;
pub const SLOT_HIDE_Y2: usize = 19;
pub const SLOT_BACKDROP: usize = 20;
pub const SLOT_BENEATH_RGB: usize = 21;
pub const SLOT_RING_RGB: usize = 22;
pub const SLOT_RING_OPACITY: usize = 23;
pub const SLOTS: usize = 24;

/// The vector, checked and split.  `chrome` is `None` when slot 0 is 0.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Decoded {
    pub chrome: Option<NhgChrome>,
    /// Slots 8 to 19: the show and hide legs (kind, ms, curve).
    pub motion_slots: [f32; 12],
    pub backdrop_px: f32,
    pub beneath_rgb: Option<u32>,
    /// A window card's edge ring: its colour (0xRRGGBB) and opacity; `None` for no ring or opacity 0.
    pub ring: Option<(u32, f32)>,
}

fn rgb_of(v: f32, none_ok: bool) -> Result<Option<u32>, GlueError> {
    if none_ok && v == -1.0 {
        return Ok(None);
    }
    if v.fract() == 0.0 && (0.0..=16_777_215.0).contains(&v) {
        Ok(Some(v as u32))
    } else {
        Err(GlueError::Arg)
    }
}

/// Check `v` and decode it.  `GlueError::Arg` for a wrong length, a non-finite slot, or a slot out of range: a
/// negative radius, blur, duration or backdrop; an opacity outside 0 to 1; a kind above 2; a curve x outside 0 to 1; a
/// colour that is not a 24-bit integer.
pub fn decode(v: &[f32]) -> Result<Decoded, GlueError> {
    if v.len() != SLOTS || v.iter().any(|x| !x.is_finite()) {
        return Err(GlueError::Arg);
    }
    let unit = |i: usize| (0.0..=1.0).contains(&v[i]);
    let kind = |i: usize| v[i].fract() == 0.0 && (0.0..=2.0).contains(&v[i]);
    let ok = v[SLOT_RADIUS] >= 0.0
        && v[SLOT_BLUR] >= 0.0
        && unit(SLOT_OPACITY)
        && kind(SLOT_SHOW_KIND)
        && kind(SLOT_HIDE_KIND)
        && v[SLOT_SHOW_MS] >= 0.0
        && v[SLOT_HIDE_MS] >= 0.0
        && [SLOT_SHOW_X1, SLOT_SHOW_X2, SLOT_HIDE_X1, SLOT_HIDE_X2]
            .into_iter()
            .all(unit)
        && v[SLOT_BACKDROP] >= 0.0
        && unit(SLOT_RING_OPACITY);
    if !ok {
        return Err(GlueError::Arg);
    }
    let rgb = rgb_of(v[SLOT_RGB], false)?.unwrap_or(0);
    let beneath_rgb = rgb_of(v[SLOT_BENEATH_RGB], true)?;
    let ring = rgb_of(v[SLOT_RING_RGB], true)?
        .filter(|_| v[SLOT_RING_OPACITY] > 0.0)
        .map(|c| (c, v[SLOT_RING_OPACITY]));
    let chrome = (v[SLOT_ON] != 0.0).then(|| NhgChrome {
        on: 1,
        radius: v[SLOT_RADIUS],
        blur: v[SLOT_BLUR],
        ox: v[SLOT_OX],
        oy: v[SLOT_OY],
        spread: v[SLOT_SPREAD],
        opacity: v[SLOT_OPACITY],
        rgb,
    });
    let mut motion_slots = [0.0; 12];
    motion_slots.copy_from_slice(&v[SLOT_SHOW_KIND..=SLOT_HIDE_Y2]);
    Ok(Decoded {
        chrome,
        motion_slots,
        backdrop_px: v[SLOT_BACKDROP],
        beneath_rgb,
        ring,
    })
}

/// One leg's slots (kind, ms, x1, y1, x2, y2), as `decode` checked them.
fn leg(s: &[f32]) -> Leg {
    Leg {
        kind: match s[0] as u32 {
            1 => Kind::Fade,
            2 => Kind::Pop,
            _ => Kind::None,
        },
        duration_ns: (f64::from(s[1]) * 1e6).round() as u64,
        curve: Curve::new(s[2], s[3], s[4], s[5]).unwrap_or(Curve::LINEAR),
    }
}

/// The transitions `d` asks for: `None` when both kinds are 0.
pub fn motion(d: &Decoded) -> Option<MotionSpec> {
    let (show, hide) = d.motion_slots.split_at(6);
    (show[0] != 0.0 || hide[0] != 0.0).then(|| MotionSpec {
        show: leg(show),
        hide: leg(hide),
    })
}

/// The vector of a struct chrome with no motion, backdrop or beneath colour: what Lisp gives today's chrome.
pub fn encode(c: &NhgChrome) -> [f32; SLOTS] {
    let mut v = [0.0; SLOTS];
    v[SLOT_ON] = c.on as f32;
    v[SLOT_RADIUS] = c.radius;
    v[SLOT_BLUR] = c.blur;
    v[SLOT_OX] = c.ox;
    v[SLOT_OY] = c.oy;
    v[SLOT_SPREAD] = c.spread;
    v[SLOT_OPACITY] = c.opacity;
    v[SLOT_RGB] = c.rgb as f32;
    v[SLOT_BENEATH_RGB] = -1.0;
    v[SLOT_RING_RGB] = -1.0;
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    const TODAY: NhgChrome = NhgChrome {
        on: 1,
        radius: 6.0,
        blur: 4.0,
        ox: -1.0,
        oy: 2.0,
        spread: -1.0,
        opacity: 0.5,
        rgb: 0x10_2030,
    };

    #[test]
    fn vector_of_today_equals_struct_of_today() {
        let d = decode(&encode(&TODAY)).unwrap();
        assert_eq!(d.chrome, Some(TODAY));
        assert_eq!(d.motion_slots, [0.0; 12]);
        assert_eq!(d.backdrop_px, 0.0);
        assert_eq!(d.beneath_rgb, None);
        let off = decode(&encode(&NhgChrome::default())).unwrap();
        assert_eq!(off.chrome, None);
    }

    #[test]
    fn wrong_length_is_e_arg() {
        let v = encode(&TODAY);
        assert_eq!(decode(&v[..SLOTS - 1]), Err(GlueError::Arg));
        assert_eq!(decode(&[]), Err(GlueError::Arg));
        let mut long = v.to_vec();
        long.push(0.0);
        assert_eq!(decode(&long), Err(GlueError::Arg));
    }

    #[test]
    fn out_of_range_slots_are_e_arg() {
        let cases: [(usize, f32); 13] = [
            (SLOT_RADIUS, -1.0),
            (SLOT_BLUR, -0.5),
            (SLOT_OPACITY, 1.5),
            (SLOT_OPACITY, -0.1),
            (SLOT_SHOW_KIND, 3.0),
            (SLOT_HIDE_KIND, 3.0),
            (SLOT_SHOW_MS, -1.0),
            (SLOT_HIDE_MS, -1.0),
            (SLOT_SHOW_X1, 1.5),
            (SLOT_HIDE_X2, -0.5),
            (SLOT_BACKDROP, -2.0),
            (SLOT_RGB, 16_777_216.0),
            (SLOT_BLUR, f32::NAN),
        ];
        for (i, x) in cases {
            let mut v = encode(&TODAY);
            v[i] = x;
            assert_eq!(decode(&v), Err(GlueError::Arg), "slot {i} = {x}");
        }
        // y of a curve may leave 0 to 1 (an overshoot)
        let mut v = encode(&TODAY);
        v[SLOT_SHOW_Y1] = 1.5;
        v[SLOT_HIDE_Y2] = -0.5;
        assert!(decode(&v).is_ok());
    }

    #[test]
    fn no_kind_is_no_motion() {
        let d = decode(&encode(&TODAY)).unwrap();
        assert_eq!(motion(&d), None);
        // reduced motion zeroes the kinds and leaves the rest
        let mut v = encode(&TODAY);
        v[SLOT_SHOW_MS] = 120.0;
        v[SLOT_HIDE_MS] = 80.0;
        assert_eq!(motion(&decode(&v).unwrap()), None);
    }

    #[test]
    fn slots_become_the_legs() {
        let mut v = encode(&TODAY);
        v[SLOT_SHOW_KIND..=SLOT_HIDE_Y2].copy_from_slice(&[
            2.0, 120.0, 0.0, 0.0, 0.58, 1.0, 1.0, 80.0, 0.42, 0.0, 1.0, 1.0,
        ]);
        let m = motion(&decode(&v).unwrap()).unwrap();
        assert_eq!(
            m.show,
            Leg {
                kind: Kind::Pop,
                duration_ns: 120_000_000,
                curve: Curve::EASE_OUT,
            }
        );
        assert_eq!(
            m.hide,
            Leg {
                kind: Kind::Fade,
                duration_ns: 80_000_000,
                curve: Curve::EASE_IN,
            }
        );
        // one leg alone: the other, all zeros, moves nothing
        v[SLOT_HIDE_KIND..=SLOT_HIDE_Y2].copy_from_slice(&[0.0; 6]);
        let m = motion(&decode(&v).unwrap()).unwrap();
        assert_eq!(m.show.kind, Kind::Pop);
        assert_eq!((m.hide.kind, m.hide.duration_ns), (Kind::None, 0));
    }

    #[test]
    fn rgb_round_trips_exactly() {
        for rgb in [0xff_ffff_u32, 0x01_0203, 0] {
            let mut c = TODAY;
            c.rgb = rgb;
            assert_eq!(decode(&encode(&c)).unwrap().chrome.unwrap().rgb, rgb);
            let mut v = encode(&TODAY);
            v[SLOT_BENEATH_RGB] = rgb as f32;
            assert_eq!(decode(&v).unwrap().beneath_rgb, Some(rgb));
        }
    }

    #[test]
    fn ring_slots_decode() {
        let mut v = encode(&TODAY);
        assert_eq!(decode(&v).unwrap().ring, None);
        v[SLOT_RING_RGB] = 0x40_5060 as f32;
        v[SLOT_RING_OPACITY] = 0.6;
        assert_eq!(decode(&v).unwrap().ring, Some((0x40_5060, 0.6)));
        // opacity 0 is no ring
        v[SLOT_RING_OPACITY] = 0.0;
        assert_eq!(decode(&v).unwrap().ring, None);
        for (i, x) in [
            (SLOT_RING_OPACITY, 1.5),
            (SLOT_RING_OPACITY, -0.1),
            (SLOT_RING_RGB, 16_777_216.0),
            (SLOT_RING_RGB, 0.5),
        ] {
            let mut v = encode(&TODAY);
            v[SLOT_RING_OPACITY] = 0.5;
            v[i] = x;
            assert_eq!(decode(&v), Err(GlueError::Arg), "slot {i} = {x}");
        }
    }
}

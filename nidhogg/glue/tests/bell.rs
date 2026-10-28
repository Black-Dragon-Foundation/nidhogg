//! The visible bell in the backing (no display needed): the rows are inverted and damaged, put back, and a draw that
//! lands on them keeps its own pixels.

mod flash {
    use nhglue::abi::NHG_E_STATE;
    use nhglue::backing::{Backings, NhgBacking, NhgFrameGeom, NhgRect};

    const BG: u32 = 0xff11_2233;
    const INV: u32 = 0xffee_ddcc;

    fn setup() -> (Backings, u64, NhgBacking) {
        let mut b = Backings::default();
        let mut key = 0;
        let mut g = NhgFrameGeom {
            native_w: 8,
            native_h: 6,
            bg_argb: BG,
            scale120: 120,
            ..Default::default()
        };
        g.layer = NhgRect {
            x: 0,
            y: 0,
            w: 8,
            h: 6,
        };
        b.ensure(&mut key, &g);
        let v = b.backing(key, &g).unwrap();
        (b, key, v)
    }

    fn px(b: &NhgBacking, x: i32, y: i32) -> u32 {
        // SAFETY: (x, y) lies inside the borrowed backing.
        unsafe { *b.pixels.cast::<u32>().add((y * b.w + x) as usize) }
    }

    fn put(b: &NhgBacking, x: i32, y: i32, v: u32) {
        // SAFETY: as `px`; nothing else reads meanwhile.
        unsafe { *b.pixels.cast::<u32>().add((y * b.w + x) as usize) = v }
    }

    fn row(x: i32, y: i32, w: i32) -> NhgRect {
        NhgRect { x, y, w, h: 1 }
    }

    #[test]
    fn inverts_opaque_pixels_and_restores_them() {
        let (mut b, key, v) = setup();
        assert_eq!(b.flash(key, &[row(1, 0, 6), row(1, 5, 6)]), Ok(2));
        assert_eq!(px(&v, 1, 0), INV);
        assert_eq!(px(&v, 6, 5), INV);
        assert_eq!(px(&v, 0, 0), BG);
        assert_eq!(px(&v, 1, 1), BG);
        assert_eq!(b.unflash(key), Ok(2));
        assert!((0..6).all(|y| (0..8).all(|x| px(&v, x, y) == BG)));
        assert_eq!(b.unflash(key), Ok(0));
    }

    #[test]
    fn a_draw_on_the_rows_keeps_its_pixels() {
        let (mut b, key, v) = setup();
        b.flash(key, &[row(0, 0, 8)]).unwrap();
        // a draw covers columns 2..4 of the row and damages them
        put(&v, 2, 0, 0xff00_ff00);
        put(&v, 3, 0, 0xff00_ff00);
        b.damage(
            key,
            &[NhgRect {
                x: 2,
                y: 0,
                w: 2,
                h: 1,
            }],
        )
        .unwrap();
        assert_eq!(b.unflash(key), Ok(1));
        assert_eq!(px(&v, 2, 0), 0xff00_ff00);
        assert_eq!(px(&v, 3, 0), 0xff00_ff00);
        assert_eq!(px(&v, 1, 0), BG);
        assert_eq!(px(&v, 4, 0), BG);
    }

    #[test]
    fn a_draw_elsewhere_does_not_stop_the_restore() {
        let (mut b, key, v) = setup();
        b.flash(key, &[row(0, 0, 8)]).unwrap();
        b.damage(
            key,
            &[NhgRect {
                x: 0,
                y: 3,
                w: 8,
                h: 1,
            }],
        )
        .unwrap();
        assert_eq!(b.unflash(key), Ok(1));
        assert_eq!(px(&v, 4, 0), BG);
    }

    #[test]
    fn damaging_everything_leaves_nothing_to_restore() {
        let (mut b, key, v) = setup();
        b.flash(key, &[row(0, 0, 8)]).unwrap();
        put(&v, 4, 0, 0xff01_0203);
        b.damage_all(key).unwrap();
        assert_eq!(b.unflash(key), Ok(0));
        assert_eq!(px(&v, 4, 0), 0xff01_0203);
    }

    #[test]
    fn a_second_flash_puts_the_first_back_and_inverts_once() {
        let (mut b, key, v) = setup();
        b.flash(key, &[row(0, 0, 8)]).unwrap();
        b.flash(key, &[row(0, 0, 8)]).unwrap();
        assert_eq!(px(&v, 0, 0), INV);
        b.unflash(key).unwrap();
        assert_eq!(px(&v, 0, 0), BG);
    }

    #[test]
    fn rows_outside_the_backing_are_clipped() {
        let (mut b, key, v) = setup();
        assert_eq!(
            b.flash(
                key,
                &[
                    NhgRect {
                        x: 6,
                        y: 5,
                        w: 10,
                        h: 10
                    },
                    row(0, 9, 4)
                ]
            ),
            Ok(1)
        );
        assert_eq!(px(&v, 7, 5), INV);
        assert_eq!(b.unflash(key), Ok(1));
    }

    #[test]
    fn a_frame_without_a_backing_is_refused() {
        let mut b = Backings::default();
        let mut key = 0;
        let g = NhgFrameGeom {
            native_w: 4,
            native_h: 4,
            ..Default::default()
        };
        b.ensure(&mut key, &g);
        assert_eq!(b.flash(key, &[row(0, 0, 4)]), Err(NHG_E_STATE));
        assert_eq!(b.flash(key + 1, &[row(0, 0, 4)]), Err(NHG_E_STATE));
    }
}

#[path = "../../shell/tests/common/mod.rs"]
mod common;
mod step;

/// The bell, idle inhibition and handles through the glue, against nh-recorder (mutter profile: system bell,
/// idle inhibit and xdg-foreign).  Each test stands in for C.
mod shell {
    use super::common::Recorder;
    use super::step::*;
    use nhglue::abi::*;
    use nhglue::child::NHG_CHILD_VISIBLE;
    use nhglue::ffi::*;
    use nhglue::frame::NHG_FRAME_CHILD;
    use nhshell::api::{Anchor, ChildDesc, NHSHELL_KIND_CHILD};
    use std::ffi::c_char;
    use std::time::{Duration, Instant};

    fn until_log(rec: &Recorder, cmd: &str, pred: impl Fn(&str) -> bool) -> String {
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            let r = rec.control(&format!(r#"{{"cmd":"{cmd}"}}"#));
            if pred(&r) {
                return r;
            }
            assert!(Instant::now() < deadline, "{cmd} never matched; last: {r}");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn child_of(d: *mut NhgDisplay, parent: u64) -> u64 {
        let id = backend(d)
            .child_new(&ChildDesc {
                kind: NHSHELL_KIND_CHILD,
                parent,
                x: 10,
                y: 10,
                w: 50,
                h: 40,
                anchor: Anchor::default(),
                flags: 0,
            })
            .unwrap();
        // SAFETY: a live display.
        unsafe {
            assert_eq!(nhg_frame_register(d, id, NHG_FRAME_CHILD), NHG_OK);
            assert_eq!(nhg_frame_scale(d, id, 120), NHG_OK);
            nhg_child_set(d, id, parent, 10, 10, NHG_CHILD_VISIBLE);
        }
        id
    }

    #[test]
    fn a_child_frame_rings_on_its_top_level() {
        let rec = Recorder::start("mutter");
        let d = open(&rec);
        let top = window(d);
        configured(d, top);
        let kid = child_of(d, top);
        // SAFETY: a live display.
        unsafe {
            assert_eq!(nhg_bell(d, kid), NHG_OK);
        }
        until_log(&rec, "bell_log", |r| {
            r.contains(&format!("\"surface\":{top}"))
        });
        std::thread::sleep(Duration::from_millis(30));
        // SAFETY: a live display.
        unsafe {
            assert_eq!(nhg_bell(d, 0), NHG_OK);
            assert_eq!(nhg_bell(d, 9999), NHG_E_STATE);
        }
        close(d);
    }

    #[test]
    fn the_idle_inhibitor_follows_the_parameter() {
        let rec = Recorder::start("mutter");
        let d = open(&rec);
        let top = window(d);
        configured(d, top);
        let alive = |r: &str| r.matches("\"alive\":true").count();
        // SAFETY: a live display.
        unsafe {
            assert_eq!(nhg_inhibit_idle(d, top, 1), 1);
            assert_eq!(nhg_inhibit_idle(d, top, 1), 0, "already on");
        }
        until_log(&rec, "idle_inhibitors", |r| alive(r) == 1);
        // SAFETY: a live display.
        unsafe {
            assert_eq!(nhg_inhibit_idle(d, top, 0), 1);
            assert_eq!(nhg_inhibit_idle(d, 9999, 1), NHG_E_STATE);
        }
        until_log(&rec, "idle_inhibitors", |r| alive(r) == 0);
        close(d);
    }

    #[test]
    fn the_export_handle_is_the_roots_and_fits_or_says_so() {
        let rec = Recorder::start("mutter");
        let d = open(&rec);
        let top = window(d);
        configured(d, top);
        let kid = child_of(d, top);
        let mut buf = [0 as c_char; 32];
        let mut len = 0usize;
        // SAFETY: a live display; `buf` holds 32 writable bytes, `len` is writable.
        unsafe {
            assert_eq!(
                nhg_window_export(d, kid, 1000, buf.as_mut_ptr(), buf.len(), &mut len),
                NHG_OK
            );
            assert_eq!((len, buf[len]), (5, 0));
            let mut tiny = [0 as c_char; 3];
            assert_eq!(
                nhg_window_export(d, top, 1000, tiny.as_mut_ptr(), tiny.len(), &mut len),
                NHG_E_TOO_SMALL
            );
            assert_eq!(len, 5, "the length that would fit");
        }
        let log = rec.control(r#"{"cmd":"exports"}"#);
        assert_eq!(log.matches("\"handle\"").count(), 1, "{log}");
        close(d);
    }

    #[test]
    fn an_activation_token_needs_input() {
        let rec = Recorder::start("mutter");
        let d = open(&rec);
        let top = window(d);
        configured(d, top);
        let mut buf = [0 as c_char; 64];
        // SAFETY: a live display; `buf` holds 64 writable bytes.
        let rc = unsafe {
            nhg_activation_token(
                d,
                top,
                100,
                buf.as_mut_ptr(),
                buf.len(),
                std::ptr::null_mut(),
            )
        };
        assert_eq!(rc, NHG_E_STATE);
        close(d);
    }
}

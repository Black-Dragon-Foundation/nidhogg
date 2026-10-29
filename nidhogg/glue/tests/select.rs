//! T1 tests of selections, display queries and the bench, against nh-recorder (the shell's harness,
//! `../shell/tests/common`).  The tests run one at a time (`SERIAL`).

#[path = "../../shell/tests/common/mod.rs"]
mod common;

use common::{Recorder, readable_within};
use nhglue::abi::*;
use nhglue::action::{NHG_ACT_SELECTION_CLEAR, NhgActionList, NhgNextIn};
use nhglue::backing::{NhgBacking, NhgFrameGeom, NhgRect};
use nhglue::bench::{NhgBenchOut, Redraw, percentile, run};
use nhglue::ffi::*;
use nhglue::guard::GlueError;
use nhglue::present::{NHG_SURF_DRAWABLE, NhgSurface};
use nhglue::query::{Monitors, NhgMonitor};
use nhshell::api::{NHSHELL_ACK_CONTENT, NHSHELL_SEL_CLIPBOARD, ShellBackend, WindowDesc};
use nhshell::caps::NhShellCaps;
use nhshell::event::{Record, ShellEvent};
use rustix::process::Signal;
use std::ptr;
use std::sync::Mutex;
use std::time::{Duration, Instant};

static SERIAL: Mutex<()> = Mutex::new(());

fn open(rec: &Recorder) -> *mut NhgDisplay {
    let name = rec.display();
    // SAFETY: the name is NUL-terminated; no error buffer.
    let d = unsafe { nhg_open(name.as_ptr(), ptr::null_mut(), 0) };
    assert!(!d.is_null(), "nhg_open failed on the recorder");
    d
}

fn d_ref<'a>(d: *mut NhgDisplay) -> &'a NhgDisplay {
    // SAFETY: the tests pass live displays only.
    unsafe { &*d }
}

/// The shell inside `d`, for what the glue does not wrap (windows, records).
fn shell<'a>(d: *mut NhgDisplay) -> &'a mut dyn ShellBackend {
    // SAFETY: a live display's backend, used only between glue calls.
    unsafe { &mut *(*d).backend_ptr() }
}

/// Take the shell's records until one satisfies `want` (3 s at most); false when none did.
fn wait_record(d: *mut NhgDisplay, mut want: impl FnMut(&Record) -> bool) -> bool {
    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline {
        readable_within(shell(d).wake_fd(), 20);
        while let Ok(Some(ev)) = shell(d).next_event() {
            if want(&ev) {
                return true;
            }
        }
    }
    false
}

/// Step the glue's event drain for up to `ms` (until the shell's queue is empty and nothing more comes): the `code`s of
/// the `SELECTION_CLEAR` actions seen.
fn clears(d: *mut NhgDisplay, ms: u64) -> Vec<i32> {
    let mut seen = Vec::new();
    let deadline = Instant::now() + Duration::from_millis(ms);
    loop {
        // SAFETY: a live display.
        readable_within(unsafe { nhg_fd(d) }, 20);
        loop {
            let mut list = NhgActionList {
                v: ptr::null(),
                n: 0,
            };
            // SAFETY: a live display; `list` is writable.
            if unsafe { nhg_next(d, &NhgNextIn::default(), &mut list) } <= 0 {
                break;
            }
            for i in 0..list.n {
                // SAFETY: the list holds `n` actions until the next step.
                let a = unsafe { &*list.v.add(i) };
                if a.kind == NHG_ACT_SELECTION_CLEAR {
                    seen.push(a.code);
                }
            }
        }
        if !seen.is_empty() || Instant::now() >= deadline {
            return seen;
        }
    }
}

/// A window of the display, registered, and (when `focus`) holding the keyboard focus.
fn window(rec: &Recorder, d: *mut NhgDisplay, focus: bool) -> u64 {
    let id = shell(d)
        .window_new(&WindowDesc {
            w: 200,
            h: 100,
            title: "select".into(),
            app_id: "select".into(),
            flags: 0,
        })
        .unwrap();
    // SAFETY: a live display.
    assert_eq!(unsafe { nhg_frame_register(d, id, 0) }, NHG_OK);
    if focus {
        let reply = rec.control_ok(&format!(r#"{{"cmd":"focus","surface":{id}}}"#));
        assert!(reply.contains("\"ok\":true"), "{reply}");
        assert!(
            wait_record(
                d,
                |e| matches!(e.event, ShellEvent::Focus(f) if f.r#in == 1)
            ),
            "keyboard focus"
        );
    }
    id
}

fn monitor(id: u64, x: i32, y: i32, w: i32, h: i32, mm: (i32, i32), scale120: u32) -> NhgMonitor {
    let mut m = NhgMonitor {
        id,
        x,
        y,
        w,
        h,
        mm_w: mm.0,
        mm_h: mm.1,
        scale120,
        name: [0; 32],
    };
    m.name[0] = b'M' as _;
    m
}

#[allow(clippy::module_inception)]
mod select {
    use super::*;

    /// Our own selection is read at once: with the compositor stopped, a round trip would not come back.
    #[test]
    fn own_selection_read_without_round_trip() {
        let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let rec = Recorder::start("sway");
        let d = open(&rec);
        let _ = window(&rec, d, true);
        let mimes = [c"text/plain".as_ptr(), c"UTF8_STRING".as_ptr()];
        let (a, b): (&[u8], &[u8]) = (b"plain bytes", b"utf8 bytes");
        let data = [a.as_ptr(), b.as_ptr()];
        let lens = [a.len(), b.len()];
        // SAFETY: a live display; the arrays hold two entries each and the strings are NUL-terminated.
        unsafe {
            assert_eq!(nhg_selection_owned(d, 0), 0);
            assert_eq!(
                nhg_selection_set(d, 0, mimes.as_ptr(), data.as_ptr(), lens.as_ptr(), 2),
                NHG_OK
            );
            assert_eq!(nhg_selection_owned(d, 0), 1);
            let (mut buf, mut len) = (ptr::null(), 0usize);
            assert_eq!(nhg_selection_types(d, 0, &mut buf, &mut len), 2);
            let types = std::slice::from_raw_parts(buf.cast::<u8>(), len);
            assert_eq!(types, b"text/plain\0UTF8_STRING\0");
            rec.signal(Signal::STOP);
            let started = Instant::now();
            let (mut out, mut n) = (ptr::null(), 0usize);
            let rc = nhg_selection_read(d, 0, c"UTF8_STRING".as_ptr(), 1000, &mut out, &mut n);
            let took = started.elapsed();
            rec.signal(Signal::CONT);
            assert_eq!(rc, NHG_OK);
            assert!(took < Duration::from_millis(100), "{took:?}: a round trip");
            assert_eq!(std::slice::from_raw_parts(out, n), b"utf8 bytes");
            assert_eq!(
                nhg_selection_read(d, 0, c"image/png".as_ptr(), 1000, &mut out, &mut n),
                NHG_E_STATE
            );
            assert_eq!(nhg_selection_disown(d, 0), 1);
            assert_eq!(nhg_selection_disown(d, 0), 0);
            assert_eq!(nhg_selection_owned(d, 0), 0);
            // 2 is the drag's offer (readable), 3 no selection
            assert_eq!(nhg_selection_owned(d, 3), NHG_E_ARG);
            assert_eq!(nhg_close(d), NHG_OK);
        }
    }

    /// Another client taking a selection we own clears it once, and only that one.  Without focus nothing is owned.
    #[test]
    fn clear_on_foreign_owner() {
        let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let rec = Recorder::start("sway");
        let d = open(&rec);
        let mimes = [c"text/plain".as_ptr()];
        let text: &[u8] = b"ours";
        let data = [text.as_ptr()];
        let lens = [text.len()];
        // SAFETY: a live display; the arrays hold one entry each.
        unsafe {
            assert_eq!(
                nhg_selection_set(d, 0, mimes.as_ptr(), data.as_ptr(), lens.as_ptr(), 1),
                NHG_E_STATE,
                "no focus yet"
            );
        }
        assert!(clears(d, 100).is_empty(), "nothing owned, nothing lost");
        let _ = window(&rec, d, true);
        // SAFETY: as above.
        unsafe {
            assert_eq!(
                nhg_selection_set(d, 0, mimes.as_ptr(), data.as_ptr(), lens.as_ptr(), 1),
                NHG_OK
            );
        }
        assert!(clears(d, 200).is_empty(), "still ours");
        let reply =
            rec.control(r#"{"cmd":"offer","kind":"clipboard","types":{"text/plain":"theirs"}}"#);
        assert!(reply.contains("\"ok\":true"), "{reply}");
        // the clipboard (kind 0), once
        assert_eq!(clears(d, 3000), [NHSHELL_SEL_CLIPBOARD as i32]);
        assert!(clears(d, 200).is_empty(), "cleared once");
        // SAFETY: a live display.
        unsafe {
            assert_eq!(nhg_selection_disown(d, 0), 0, "no longer ours");
            assert_eq!(nhg_close(d), NHG_OK);
        }
    }
}

mod query {
    use super::*;

    /// The extent follows the outputs: millimetres at the origin count the largest, the others add up, and pixels
    /// reach the far corner of the output farthest out, in device pixels.  Without outputs: 1920 x 1080.
    #[test]
    fn display_info_follows_monitors() {
        // the list the drain keeps (`input.rs` feeds it from the shell's records); a display is not needed
        let mut list = Monitors::default();
        let get = |m: &Monitors| {
            let i = m.info(NhShellCaps::default());
            (i.w, i.h, i.mm_w, i.mm_h, i.scale120)
        };
        assert_eq!(get(&list), (1920, 1080, 0, 0, 0));
        let a = monitor(1, 0, 0, 2560, 1440, (600, 340), 240);
        let b = monitor(2, 1280, 0, 1920, 1080, (500, 280), 120);
        assert!(list.set(&a));
        assert_eq!(get(&list), (2560, 1440, 600, 340, 240));
        assert!(list.set(&b));
        // b starts at 1280 logical pixels, which is 1280 device pixels at its scale of 1
        assert_eq!(get(&list), (3200, 1440, 1100, 340, 240));
        let a2 = monitor(1, 0, 0, 3840, 2160, (600, 340), 240);
        assert!(list.set(&a2), "a change replaces");
        assert!(list.remove(2));
        assert!(!list.remove(2));
        assert_eq!(get(&list), (3840, 2160, 600, 340, 240));
        assert!(list.remove(1));
        assert_eq!(get(&list), (1920, 1080, 0, 0, 240), "the scale is kept");
    }

    /// `nhg_display_info` refuses a null `out`.
    #[test]
    fn display_info_refuses_null() {
        let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let rec = Recorder::start("sway");
        let d = open(&rec);
        // SAFETY: a live display; null is allowed and reported.
        unsafe {
            assert_eq!(nhg_display_info(d, ptr::null_mut()), NHG_E_ARG);
            assert_eq!(nhg_close(d), NHG_OK);
        }
    }

    /// The list holds sixteen outputs, and a name that fills its field is cut and terminated.
    #[test]
    fn list_is_bounded() {
        let mut m = Monitors::default();
        for id in 1..=16 {
            assert!(m.set(&monitor(id, 0, 0, 100, 100, (10, 10), 120)));
        }
        let mut extra = monitor(17, 0, 0, 100, 100, (10, 10), 120);
        assert!(!m.set(&extra));
        extra.id = 3;
        extra.name = [b'x' as _; 32];
        assert!(m.set(&extra), "an output already known is replaced");
        assert_eq!(m.info(NhShellCaps::default()).w, 100);
    }
}

mod bench {
    use super::*;

    /// A present that commits after a few tries, with a clock the test does not need.
    struct Fake {
        tries_per_frame: u32,
        tries: u32,
        damaged: u32,
    }

    impl Redraw for Fake {
        fn damage(&mut self) -> Result<(), GlueError> {
            self.damaged += 1;
            self.tries = 0;
            Ok(())
        }
        fn attempt(&mut self) -> Result<Option<Duration>, GlueError> {
            self.tries += 1;
            Ok((self.tries >= self.tries_per_frame).then(|| Duration::from_millis(2)))
        }
    }

    /// The loop damages each frame once, retries until it commits, and counts the frames; percentiles are nearest rank.
    #[test]
    fn loop_counts_frames_and_ranks() {
        let mut f = Fake {
            tries_per_frame: 3,
            tries: 0,
            damaged: 0,
        };
        let out = run(&mut f, 10).unwrap();
        assert_eq!((out.frames, f.damaged), (10, 10));
        assert_eq!(out.compose_p50, 2.0);
        assert!(out.interval_p95 >= 2.0, "two retries sleep 1 ms each");
        assert_eq!(run(&mut f, 0).unwrap_err(), GlueError::Arg);
        assert_eq!(run(&mut f, 100_001).unwrap_err(), GlueError::Arg);
        let v: Vec<f64> = (1..=20).map(f64::from).collect();
        assert_eq!(percentile(&v, 0.5), 10.0);
        assert_eq!(percentile(&v, 0.95), 19.0);
        assert_eq!(percentile(&v[..1], 0.95), 1.0);
    }

    /// N frames of a mapped window present and are counted; a window that is not registered is refused.
    #[test]
    fn full_redraw_counts_frames() {
        let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let rec = Recorder::start("sway");
        let d = open(&rec);
        let win = window(&rec, d, false);
        let mut key = 0u64;
        let mut g = NhgFrameGeom {
            native_w: 200,
            native_h: 100,
            scale120: 120,
            bg_argb: 0xff20_2020,
            ..Default::default()
        };
        g.layer = NhgRect {
            x: 0,
            y: 0,
            w: 200,
            h: 100,
        };
        let mut back = std::mem::MaybeUninit::<NhgBacking>::zeroed();
        let mut out = NhgBenchOut::default();
        let tree = |window: u64, frame: u64| {
            [NhgSurface {
                window,
                parent: 0,
                frame,
                flags: NHG_SURF_DRAWABLE,
                alpha_cut: 0,
            }]
        };
        // SAFETY: a live display; the records are readable and writable.
        unsafe {
            assert_eq!(
                nhg_bench_full_redraw(d, tree(9999, 0).as_ptr(), 1, 5, &mut out),
                NHG_E_STATE
            );
            assert_eq!(nhg_layer_ensure(d, &mut key, &g), 1);
            assert_eq!(nhg_backing(d, key, &g, back.as_mut_ptr()), NHG_OK);
        }
        // map the window: its first configure acked with the buffer that carries it
        let mut serial = None;
        assert!(wait_record(d, |e| {
            if let ShellEvent::Configure(c) = e.event {
                serial = Some(c.serial);
            }
            serial.is_some()
        }));
        {
            let mut state = d_ref(d).lock_for_test();
            let b = state.backend();
            b.ack_configure(win, serial.unwrap(), NHSHELL_ACK_CONTENT)
                .unwrap();
        }
        // SAFETY: a live display; `out` is writable.
        unsafe {
            let t = tree(win, key);
            assert_eq!(
                nhg_bench_full_redraw(d, t.as_ptr(), 1, 0, &mut out),
                NHG_E_ARG
            );
            assert_eq!(nhg_bench_full_redraw(d, t.as_ptr(), 1, 5, &mut out), NHG_OK);
            assert_eq!(out.frames, 5);
            assert!(out.compose_p50 >= 0.0 && out.compose_p95 >= out.compose_p50);
            assert!(out.interval_p95 >= 0.0);
            assert_eq!(nhg_close(d), NHG_OK);
        }
    }
}

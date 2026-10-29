//! T1 tests of the guard and the entry points, against nh-recorder (the shell's harness, `../shell/tests/common`).

#[path = "../../shell/tests/common/mod.rs"]
mod common;

use common::Recorder;
use nhglue::abi::*;
use nhglue::ffi::*;
use std::ffi::c_char;
use std::ptr;
use std::time::{Duration, Instant};

/// A display on `rec`.
fn open(rec: &Recorder) -> *mut NhgDisplay {
    let name = rec.display();
    // SAFETY: the name is NUL-terminated; no error buffer.
    let d = unsafe { nhg_open(name.as_ptr(), ptr::null_mut(), 0) };
    assert!(!d.is_null(), "nhg_open failed on the recorder");
    d
}

/// The first panic's report on `d` (empty: none).
fn panic_message(d: *mut NhgDisplay) -> String {
    let mut len = 0usize;
    // SAFETY: a live display; no buffer, `len` writable.
    assert_eq!(
        unsafe { nhg_panic_message(d, ptr::null_mut(), 0, &mut len) },
        NHG_OK
    );
    let mut buf = vec![0 as c_char; len + 1];
    // SAFETY: `buf` holds `len + 1` writable bytes.
    assert_eq!(
        unsafe { nhg_panic_message(d, buf.as_mut_ptr(), buf.len(), &mut len) },
        NHG_OK
    );
    let bytes: Vec<u8> = buf[..len].iter().map(|&c| c as u8).collect();
    String::from_utf8(bytes).unwrap()
}

/// The display behind `d`.
fn display<'a>(d: *mut NhgDisplay) -> &'a NhgDisplay {
    // SAFETY: the tests pass live displays only.
    unsafe { &*d }
}

/// A display pointer the scoped threads share; the outer lock serialises its use, as Emacs's lock does.
struct Shared(*mut NhgDisplay);
// SAFETY: every use is under the test's outer mutex.
unsafe impl Sync for Shared {}

/// The plan names these `ffi::…`.
#[allow(clippy::module_inception)]
mod ffi {
    use super::*;
    use std::sync::Mutex;

    #[test]
    fn panic_returns_e_panic_and_poisons() {
        let rec = Recorder::start("mutter");
        let d = open(&rec);
        assert_eq!(panic_message(d), "");
        // SAFETY: a live display.
        unsafe {
            assert_eq!(nhg_frame_register(d, 7, 0), NHG_OK);
            assert_eq!(nhg_test_panic(d), NHG_E_PANIC);
            assert!(display(d).is_poisoned());
            // every later call answers NHG_E_PANIC without touching the state
            assert_eq!(nhg_frame_forget(d, 7), NHG_E_PANIC);
            assert_eq!(nhg_fd(d), NHG_E_PANIC);
            assert_eq!(nhg_dispatch_main(d), NHG_E_PANIC);
        }
        let msg = panic_message(d);
        assert!(msg.contains("nhg_test_panic: a test panic"), "{msg}");
        assert!(msg.contains("src/ffi.rs"), "no location: {msg}");
        assert!(msg.contains("backtrace:"), "no backtrace: {msg}");
        // SAFETY: a live display, not used again.
        assert_eq!(unsafe { nhg_close(d) }, NHG_OK);
    }

    /// Every `extern "C"` function of the glue comes from `nhg_export!`, so each runs in the guard, and each has a
    /// `# Safety` section; every function of the header is one of them.
    #[test]
    fn every_extern_comes_from_nhg_export() {
        let root = env!("CARGO_MANIFEST_DIR");
        let ffi = include_str!("../src/ffi.rs");
        // the invocations' spans, by brace matching from each `nhg_export! {`
        let mut spans = Vec::new();
        for (at, _) in ffi.match_indices("nhg_export! {") {
            let open = at + "nhg_export! ".len();
            let mut depth = 0usize;
            let end = ffi[open..]
                .char_indices()
                .find_map(|(i, c)| {
                    match c {
                        '{' => depth += 1,
                        '}' => depth -= 1,
                        _ => {}
                    }
                    (depth == 0).then_some(open + i)
                })
                .expect("an unclosed nhg_export! invocation");
            let body = &ffi[open..end];
            assert!(body.contains("/// # Safety"), "no # Safety in:\n{body}");
            spans.push((open, end));
        }
        let inside = |at: usize| spans.iter().any(|&(a, b)| a < at && at < b);
        for (at, _) in ffi.match_indices("fn nhg_") {
            assert!(
                inside(at),
                "an entry point outside nhg_export!: {}",
                &ffi[at..at + 40]
            );
        }
        // nothing in the crate writes an export by hand (comments aside): only the macro in guard.rs does
        for entry in std::fs::read_dir(format!("{root}/src")).unwrap() {
            let path = entry.unwrap().path();
            let text = std::fs::read_to_string(&path).unwrap();
            let code = text.lines().filter(|l| !l.trim_start().starts_with("//"));
            let mut in_macro = false;
            for line in code {
                in_macro |=
                    path.ends_with("guard.rs") && line.starts_with("macro_rules! nhg_export");
                for needle in ["no_mangle", "extern \"C\""] {
                    assert!(
                        in_macro || !line.contains(needle),
                        "{needle} outside nhg_export! in {}: {line}",
                        path.display()
                    );
                }
            }
        }
        // each function of nhglue.h is an entry point written with the macro
        let header = std::fs::read_to_string(format!("{root}/include/nhglue.h")).unwrap();
        let mut n = 0;
        for line in header.lines() {
            let Some(at) = line.find(" nhg_").or_else(|| line.find("*nhg_")) else {
                continue;
            };
            let name: String = line[at + 1..]
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                .collect();
            if !line[at + 1 + name.len()..].starts_with(" (") {
                continue;
            }
            let def = ffi
                .find(&format!("fn {name}("))
                .unwrap_or_else(|| panic!("{name} has no entry point"));
            assert!(inside(def), "{name} is not written with nhg_export!");
            n += 1;
        }
        assert!(n >= 10, "only {n} functions found in nhglue.h");
    }

    /// Two threads call in turn under an outer lock (Emacs's global lock stands in): the guard's `try_lock` never sees
    /// the other's call, nothing is poisoned and the registry adds up.
    #[test]
    fn calls_from_two_threads_serialise() {
        let rec = Recorder::start("mutter");
        let d = Shared(open(&rec));
        let emacs = Mutex::new(());
        std::thread::scope(|s| {
            for t in 1u64..=2 {
                let (d, emacs) = (&d, &emacs);
                s.spawn(move || {
                    for i in 0..5000u64 {
                        let id = (t << 32) | (i + 1);
                        {
                            let _lock = emacs.lock().unwrap();
                            // SAFETY: a live display, used under the outer lock.
                            assert_eq!(unsafe { nhg_frame_register(d.0, id, 0) }, NHG_OK);
                        }
                        if i % 2 == 0 {
                            let _lock = emacs.lock().unwrap();
                            // SAFETY: as above.
                            assert_eq!(unsafe { nhg_frame_forget(d.0, id) }, NHG_OK);
                        }
                    }
                });
            }
        });
        let disp = display(d.0);
        assert!(!disp.is_poisoned());
        {
            let state = disp.lock_for_test();
            assert_eq!(state.frame_count(), 5000);
            let id = |t: u64, n: u64| (t << 32) | n;
            assert!(state.is_forgotten(id(1, 1)) && state.is_forgotten(id(2, 5)));
            assert!(state.frame(id(1, 2)).is_ok() && state.frame(id(2, 5000)).is_ok());
        }
        // SAFETY: a live display, not used again.
        assert_eq!(unsafe { nhg_close(d.0) }, NHG_OK);
    }

    /// In a build with debug assertions (the tests', and `--enable-checking` Emacs's), facts that drifted from Emacs's
    /// poison the display.
    #[test]
    fn debug_check_frame_compares_pushed_facts() {
        let rec = Recorder::start("mutter");
        let d = open(&rec);
        let same = nhglue::frame::NhgFrameFacts::default();
        let other = nhglue::frame::NhgFrameFacts {
            native_w: 800,
            ..same
        };
        // SAFETY: a live display; the facts are readable.
        unsafe {
            assert_eq!(nhg_debug_check_frame(d, 9, &same), NHG_E_STATE);
            assert_eq!(nhg_frame_register(d, 9, 0), NHG_OK);
            assert_eq!(nhg_debug_check_frame(d, 9, ptr::null()), NHG_E_ARG);
            assert_eq!(nhg_debug_check_frame(d, 9, &same), NHG_OK);
            let drift = nhg_debug_check_frame(d, 9, &other);
            if cfg!(debug_assertions) {
                assert_eq!(drift, NHG_E_PANIC);
                assert!(panic_message(d).contains("drifted"));
            } else {
                assert_eq!(drift, NHG_OK);
            }
            assert_eq!(nhg_close(d), NHG_OK);
        }
    }
}

/// The plan names these `guard::…`.
mod guard {
    use super::*;

    /// A held lock (an abandoned call, or a re-entry) poisons the display at once instead of blocking.
    #[test]
    fn held_lock_poisons_instead_of_blocking() {
        let rec = Recorder::start("mutter");
        let d = open(&rec);
        let held = display(d).lock_for_test();
        let t0 = Instant::now();
        // SAFETY: a live display.
        let rc = unsafe { nhg_frame_register(d, 1, 0) };
        let took = t0.elapsed();
        drop(held);
        assert_eq!(rc, NHG_E_PANIC);
        assert!(took < Duration::from_millis(10), "took {took:?}");
        assert!(display(d).is_poisoned());
        // SAFETY: a live display.
        assert_eq!(unsafe { nhg_frame_register(d, 1, 0) }, NHG_E_PANIC);
        assert!(panic_message(d).contains("locked on entry"));
        // SAFETY: a live display, not used again.
        assert_eq!(unsafe { nhg_close(d) }, NHG_OK);
    }

    /// With less than 256 KiB of stack left below the caller, a call is refused with `NHG_E_NOMEM` and nothing is
    /// poisoned.  The stack's bound is injected: the caller's address minus 100 KiB, then minus 1 MiB.
    #[test]
    fn low_stack_returns_nomem() {
        let rec = Recorder::start("mutter");
        let d = open(&rec);
        let here = 0u8;
        let sp = std::hint::black_box(&here) as *const u8 as usize;
        nhglue::guard::set_stack_low_for_test(Some(sp - 100 * 1024));
        // SAFETY: a live display.
        let (low, low_open) = unsafe {
            (
                nhg_frame_register(d, 3, 0),
                nhg_open(ptr::null(), ptr::null_mut(), 0),
            )
        };
        nhglue::guard::set_stack_low_for_test(Some(sp - 1024 * 1024));
        // SAFETY: a live display.
        let enough = unsafe { nhg_frame_register(d, 3, 0) };
        nhglue::guard::set_stack_low_for_test(None);
        assert_eq!(low, NHG_E_NOMEM);
        assert!(low_open.is_null());
        assert_eq!(enough, NHG_OK);
        assert!(!display(d).is_poisoned());
        // SAFETY: a live display, not used again.
        assert_eq!(unsafe { nhg_close(d) }, NHG_OK);
    }
}

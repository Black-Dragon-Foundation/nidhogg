//! T1 tests of the guard and the entry points, against nh-recorder (the shell's harness, `../shell/tests/common`).

#[path = "../../shell/tests/common/mod.rs"]
mod common;

use common::Recorder;
use nhglue::abi::*;
use nhglue::ffi::*;
use std::ffi::c_char;
use std::ptr;
use std::time::Duration;

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

    /// Sjá (nidhogg/sja), which this archive carries through Glitnir, writes every `extern "C"` function with
    /// `sja_export!` (so each runs in its guard) with a `# Safety` section, every function of `sja.h` is one, and the
    /// archive exports it (pattern D: the sources and the linked symbol).
    #[test]
    fn sja_entry_points_come_from_sja_export() {
        let sja = format!("{}/../sja", env!("CARGO_MANIFEST_DIR"));
        let abi = std::fs::read_to_string(format!("{sja}/src/abi.rs")).unwrap();
        let mut spans = Vec::new();
        for (at, _) in abi.match_indices("sja_export! {") {
            let open = at + "sja_export! ".len();
            let mut depth = 0usize;
            let end = abi[open..]
                .char_indices()
                .find_map(|(i, c)| {
                    match c {
                        '{' => depth += 1,
                        '}' => depth -= 1,
                        _ => {}
                    }
                    (depth == 0).then_some(open + i)
                })
                .expect("an unclosed sja_export! invocation");
            assert!(
                abi[open..end].contains("/// # Safety"),
                "no # Safety in:\n{}",
                &abi[open..end]
            );
            spans.push((open, end));
        }
        assert!(
            !spans.is_empty(),
            "no sja_export! invocation in sja/src/abi.rs"
        );
        let inside = |at: usize| spans.iter().any(|&(a, b)| a < at && at < b);
        for entry in std::fs::read_dir(format!("{sja}/src")).unwrap() {
            let path = entry.unwrap().path();
            let text = std::fs::read_to_string(&path).unwrap();
            let mut in_macro = false;
            for line in text.lines().filter(|l| !l.trim_start().starts_with("//")) {
                in_macro |= path.ends_with("abi.rs") && line.starts_with("macro_rules! sja_export");
                in_macro &= !(path.ends_with("abi.rs") && line.starts_with("sja_export! {"));
                for needle in ["no_mangle", "extern \"C\""] {
                    assert!(
                        in_macro || !line.contains(needle),
                        "{needle} outside sja_export! in {}: {line}",
                        path.display()
                    );
                }
            }
        }
        let header = std::fs::read_to_string(format!("{sja}/include/sja.h")).unwrap();
        let mut n = 0;
        for line in header.lines() {
            let Some(at) = line.find(" sja_") else {
                continue;
            };
            let name: String = line[at + 1..]
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                .collect();
            if !line[at + 1 + name.len()..].starts_with(" (") {
                continue;
            }
            let def = abi
                .find(&format!("fn {name}("))
                .unwrap_or_else(|| panic!("{name} has no entry point"));
            assert!(inside(def), "{name} is not written with sja_export!");
            n += 1;
        }
        assert_eq!(n, 1, "sja.h declares one function");
        // the archive carries it (through glitnir's re-export and this crate's `pub use glitnir::*`)
        let _: unsafe extern "C" fn(
            i32,
            *const nhglue::sja::abi::SjaArg,
            usize,
            *mut *const nhglue::sja::abi::SjaTok,
            *mut usize,
        ) -> i32 = nhglue::sja_call;
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

    /// A held lock (an abandoned call, or a re-entry) poisons the display at once instead of blocking: the call
    /// returns while the lock is still held.  Not timed: the 30 s only stops a call that waits for the release.
    #[test]
    fn held_lock_poisons_instead_of_blocking() {
        let rec = Recorder::start("mutter");
        let d = open(&rec);
        let held = display(d).lock_for_test();
        let (tx, rx) = std::sync::mpsc::channel();
        let addr = d as usize;
        let caller = std::thread::spawn(move || {
            // SAFETY: a live display (the test keeps it open until this thread is joined).
            let rc = unsafe { nhg_frame_register(addr as *mut NhgDisplay, 1, 0) };
            let _ = tx.send(rc);
        });
        let rc = rx.recv_timeout(Duration::from_secs(30));
        // the lock is released only now: a call that returned above did not wait for it
        drop(held);
        caller.join().expect("the caller thread");
        assert_eq!(
            rc,
            Ok(NHG_E_PANIC),
            "the call waited for the lock's release"
        );
        assert!(display(d).is_poisoned());
        // SAFETY: a live display.
        assert_eq!(unsafe { nhg_frame_register(d, 1, 0) }, NHG_E_PANIC);
        assert!(panic_message(d).contains("locked on entry"));
        // it names the holder's thread and the guards this thread is inside, with the backtrace
        let msg = panic_message(d);
        assert!(msg.contains("guards on this thread"), "{msg}");
        assert!(msg.contains("backtrace:"), "{msg}");
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

/// An XPM's colours cross in two calls and no function pointer: the names out, then their colours in.
#[test]
fn xpm_names_then_colours() {
    let xpm = br#"/* XPM */ static char *x[] = { "2 1 2 1", "a c None", "b c red", "ab" };"#;
    let (mut need, mut count) = (0usize, 0u32);
    // SAFETY: `xpm` is readable; no buffer; `need` and `count` writable.
    let rc = unsafe {
        nhg_xpm_names(
            xpm.as_ptr(),
            xpm.len(),
            0,
            ptr::null_mut(),
            0,
            &mut need,
            &mut count,
        )
    };
    assert_eq!((rc, need, count), (NHG_E_TOO_SMALL, 4, 1));
    let mut buf = [0xaau8; 4];
    // SAFETY: `buf` holds `need` writable bytes.
    let rc = unsafe {
        nhg_xpm_names(
            xpm.as_ptr(),
            xpm.len(),
            0,
            buf.as_mut_ptr(),
            buf.len(),
            &mut need,
            &mut count,
        )
    };
    assert_eq!((rc, &buf), (NHG_OK, b"red\0"));
    let mut img = std::mem::MaybeUninit::uninit();
    // SAFETY: one colour for the one name; `img` writable.
    let rc = unsafe {
        nhg_image_decode(
            xpm.as_ptr(),
            xpm.len(),
            0,
            0,
            [0xff_0000u32].as_ptr(),
            1,
            img.as_mut_ptr(),
        )
    };
    assert_eq!(rc, NHG_OK);
    // SAFETY: decoded above, freed once.
    assert_eq!(unsafe { nhg_image_free(img.as_mut_ptr()) }, NHG_OK);
    // the wrong number of colours decodes nothing
    // SAFETY: as above, no colours.
    let rc = unsafe {
        nhg_image_decode(
            xpm.as_ptr(),
            xpm.len(),
            0,
            0,
            ptr::null(),
            0,
            img.as_mut_ptr(),
        )
    };
    assert_eq!(rc, NHG_E_STATE);
    // not an XPM: nothing to resolve
    let png = b"\x89PNG\r\n\x1a\n";
    // SAFETY: as above.
    let rc = unsafe {
        nhg_xpm_names(
            png.as_ptr(),
            png.len(),
            0,
            ptr::null_mut(),
            0,
            &mut need,
            &mut count,
        )
    };
    assert_eq!((rc, need, count), (NHG_OK, 0, 0));
}

//! The C ABI: status codes, the panic guard and the exported functions.

use crate::geom::Rect;
use crate::stats::{StatsEntry, Summary};
use std::panic::{UnwindSafe, catch_unwind};

pub const GLITNIR_OK: i32 = 0;
pub const GLITNIR_E_PANIC: i32 = -1;
pub const GLITNIR_E_ARG: i32 = -2;
// Returned once the global state machine exists (a call in the wrong state); nothing uses it yet.
#[allow(dead_code)]
pub const GLITNIR_E_STATE: i32 = -3;

/// The ABI version the header describes.
const ABI_VERSION: u32 = 1;

/// Every exported C symbol; a test checks this list against `include/glitnir.h`.
pub const EXPORTS: &[&str] = &[
    "glitnir_abi_version",
    "glitnir_commit",
    "glitnir_damage_add_n",
    "glitnir_damage_all",
    "glitnir_damage_forget",
    "glitnir_damage_pending",
    "glitnir_debug_panic",
    "glitnir_scroll",
    "glitnir_stats_forget",
    "glitnir_stats_record",
    "glitnir_stats_summary",
];

/// Run an export's body; a panic becomes `GLITNIR_E_PANIC` instead of unwinding into C.
pub(crate) fn ffi_guard<F: FnOnce() -> i32 + UnwindSafe>(f: F) -> i32 {
    catch_unwind(f).unwrap_or(GLITNIR_E_PANIC)
}

/// Write the ABI version to `out`.
///
/// # Safety
/// `out` must be null or point to a writable `u32`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn glitnir_abi_version(out: *mut u32) -> i32 {
    ffi_guard(|| {
        if out.is_null() {
            return GLITNIR_E_ARG;
        }
        // SAFETY: non-null, and the caller promises it is writable.
        unsafe { out.write(ABI_VERSION) };
        GLITNIR_OK
    })
}

/// Panic inside the guard (tests, and the C side's fallback check); returns `GLITNIR_E_PANIC`.
#[unsafe(no_mangle)]
pub extern "C" fn glitnir_debug_panic() -> i32 {
    ffi_guard(|| panic!("glitnir_debug_panic"))
}

/// Append one paint's record to ROOT's ring.
///
/// # Safety
/// `entry` must be null or point to a readable `StatsEntry`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn glitnir_stats_record(root: u64, entry: *const StatsEntry) -> i32 {
    ffi_guard(|| {
        if entry.is_null() {
            return GLITNIR_E_ARG;
        }
        // SAFETY: non-null, and the caller promises it is readable.
        crate::stats::record(root, unsafe { entry.read() });
        GLITNIR_OK
    })
}

/// Write ROOT's summary to `out`, then empty its ring when `reset` is non-zero.
///
/// # Safety
/// `out` must be null or point to a writable `Summary`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn glitnir_stats_summary(root: u64, reset: i32, out: *mut Summary) -> i32 {
    ffi_guard(|| {
        if out.is_null() {
            return GLITNIR_E_ARG;
        }
        // SAFETY: non-null, and the caller promises it is writable.
        unsafe { out.write(crate::stats::summary(root, reset != 0)) };
        GLITNIR_OK
    })
}

/// Drop ROOT's ring (the root is gone).
#[unsafe(no_mangle)]
pub extern "C" fn glitnir_stats_forget(root: u64) -> i32 {
    ffi_guard(|| {
        crate::stats::forget(root);
        GLITNIR_OK
    })
}

/// Add N rectangles (device pixels) to FRAME's pending damage.
///
/// # Safety
/// `rects` must point to `n` readable rectangles, or be null when `n` is 0.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn glitnir_damage_add_n(frame: u64, rects: *const Rect, n: usize) -> i32 {
    ffi_guard(|| {
        if n == 0 {
            return GLITNIR_OK;
        }
        if rects.is_null() {
            return GLITNIR_E_ARG;
        }
        // SAFETY: non-null, and the caller promises N readable rectangles.
        let rs = unsafe { std::slice::from_raw_parts(rects, n) };
        crate::damage::with(frame, |d| rs.iter().for_each(|r| d.add(*r)));
        GLITNIR_OK
    })
}

/// Damage the whole of FRAME.
#[unsafe(no_mangle)]
pub extern "C" fn glitnir_damage_all(frame: u64) -> i32 {
    ffi_guard(|| {
        crate::damage::with(frame, |d| d.add(crate::damage::ALL));
        GLITNIR_OK
    })
}

/// WINDOW's pixels in AREA moved by DY on FRAME; EXPOSED is what the move left stale.
#[unsafe(no_mangle)]
pub extern "C" fn glitnir_scroll(
    frame: u64,
    window: u64,
    area: Rect,
    dy: i32,
    exposed: Rect,
) -> i32 {
    // The window and the distance are for the scene's scroll operation; damage needs only the rectangles.
    let _ = (window, dy);
    ffi_guard(|| {
        crate::damage::with(frame, |d| d.scroll(area, exposed));
        GLITNIR_OK
    })
}

/// Take FRAME's pending damage: `*out` points to `*n` rectangles, valid until FRAME's next take or forget.
///
/// # Safety
/// `out` and `n` must be null or writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn glitnir_damage_pending(
    frame: u64,
    out: *mut *const Rect,
    n: *mut usize,
) -> i32 {
    ffi_guard(|| {
        if out.is_null() || n.is_null() {
            return GLITNIR_E_ARG;
        }
        let (p, len) = crate::damage::take(frame);
        // SAFETY: both non-null, and the caller promises they are writable.
        unsafe {
            out.write(p);
            n.write(len);
        }
        GLITNIR_OK
    })
}

/// What a commit hands back: ROOT's damage, valid until ROOT's next commit.  Mirrors `GlitnirCommitOut`.
#[repr(C)]
#[derive(Debug)]
pub struct CommitOut {
    pub rects: *const Rect,
    pub n: usize,
}

/// Commit ROOT at a consistency point: `out` gets its damage, valid until ROOT's next commit or forget.
///
/// # Safety
/// `out` must be null or point to a writable `CommitOut`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn glitnir_commit(root: u64, out: *mut CommitOut) -> i32 {
    ffi_guard(|| {
        if out.is_null() {
            return GLITNIR_E_ARG;
        }
        let (rects, n) = crate::damage::commit(root);
        // SAFETY: non-null, and the caller promises it is writable.
        unsafe { out.write(CommitOut { rects, n }) };
        GLITNIR_OK
    })
}

/// Drop FRAME's damage (the frame is gone).
#[unsafe(no_mangle)]
pub extern "C" fn glitnir_damage_forget(frame: u64) -> i32 {
    ffi_guard(|| {
        crate::damage::forget(frame);
        GLITNIR_OK
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
        for e in std::fs::read_dir(dir).unwrap() {
            let p = e.unwrap().path();
            if p.is_dir() {
                rust_files(&p, out);
            } else if p.extension().is_some_and(|x| x == "rs") {
                out.push(p);
            }
        }
    }

    fn crate_file(rel: &str) -> String {
        std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join(rel)).unwrap()
    }

    #[test]
    fn a_panicking_export_returns_the_error_code() {
        assert_eq!(glitnir_debug_panic(), GLITNIR_E_PANIC);
    }

    #[test]
    fn abi_version_is_written() {
        let mut v = 0;
        assert_eq!(unsafe { glitnir_abi_version(&mut v) }, GLITNIR_OK);
        assert_eq!(v, 1);
        assert_eq!(
            unsafe { glitnir_abi_version(std::ptr::null_mut()) },
            GLITNIR_E_ARG
        );
    }

    fn pending(frame: u64) -> Vec<Rect> {
        let (mut p, mut n) = (std::ptr::null::<Rect>(), 0usize);
        assert_eq!(
            unsafe { glitnir_damage_pending(frame, &mut p, &mut n) },
            GLITNIR_OK
        );
        if n == 0 {
            return Vec::new();
        }
        // SAFETY: the library keeps N rectangles at P until FRAME's next take.
        unsafe { std::slice::from_raw_parts(p, n) }.to_vec()
    }

    #[test]
    fn damage_round_trips_through_the_abi() {
        let f = 8001;
        assert!(pending(f).is_empty());
        let rects = [
            Rect::new(0, 0, 10, 2),
            Rect::new(0, 0, 4, 1),
            Rect::new(50, 50, 0, 3),
        ];
        assert_eq!(
            unsafe { glitnir_damage_add_n(f, rects.as_ptr(), rects.len()) },
            GLITNIR_OK
        );
        assert_eq!(pending(f), vec![Rect::new(0, 0, 10, 2)]);
        assert!(pending(f).is_empty());
        let area = Rect::new(0, 100, 300, 200);
        assert_eq!(
            glitnir_scroll(f, 3, area, -40, Rect::new(0, 260, 300, 40)),
            GLITNIR_OK
        );
        assert_eq!(pending(f), vec![area]);
        assert_eq!(glitnir_damage_all(f), GLITNIR_OK);
        assert_eq!(
            unsafe { glitnir_damage_add_n(f, rects.as_ptr(), 1) },
            GLITNIR_OK
        );
        assert_eq!(pending(f), vec![Rect::new(0, 0, i32::MAX, i32::MAX)]);
        assert_eq!(glitnir_damage_forget(f), GLITNIR_OK);
        assert!(pending(f).is_empty());
    }

    #[test]
    fn commit_round_trips_through_the_abi() {
        let f = 8002;
        let mut out = CommitOut {
            rects: std::ptr::null(),
            n: 7,
        };
        assert_eq!(unsafe { glitnir_commit(f, &mut out) }, GLITNIR_OK);
        assert_eq!(out.n, 0);
        let rects = [Rect::new(3, 4, 5, 6), Rect::new(100, 0, 2, 2)];
        assert_eq!(
            unsafe { glitnir_damage_add_n(f, rects.as_ptr(), rects.len()) },
            GLITNIR_OK
        );
        assert_eq!(unsafe { glitnir_commit(f, &mut out) }, GLITNIR_OK);
        // SAFETY: the library keeps OUT.N rectangles at OUT.RECTS until F's next commit.
        let mut got = unsafe { std::slice::from_raw_parts(out.rects, out.n) }.to_vec();
        got.sort_by_key(|r| (r.x, r.y));
        assert_eq!(got, rects.to_vec());
        assert!(pending(f).is_empty());
        assert_eq!(
            unsafe { glitnir_commit(f, std::ptr::null_mut()) },
            GLITNIR_E_ARG
        );
        assert_eq!(glitnir_damage_forget(f), GLITNIR_OK);
    }

    #[test]
    fn damage_rejects_null_arguments() {
        let (mut p, mut n) = (std::ptr::null::<Rect>(), 0usize);
        assert_eq!(
            unsafe { glitnir_damage_add_n(1, std::ptr::null(), 2) },
            GLITNIR_E_ARG
        );
        assert_eq!(
            unsafe { glitnir_damage_add_n(1, std::ptr::null(), 0) },
            GLITNIR_OK
        );
        assert_eq!(
            unsafe { glitnir_damage_pending(1, std::ptr::null_mut(), &mut n) },
            GLITNIR_E_ARG
        );
        assert_eq!(
            unsafe { glitnir_damage_pending(1, &mut p, std::ptr::null_mut()) },
            GLITNIR_E_ARG
        );
    }

    #[test]
    fn rect_has_the_layout_of_glitnir_rect() {
        assert_eq!(std::mem::size_of::<Rect>(), 16);
        assert_eq!(std::mem::align_of::<Rect>(), 4);
    }

    #[test]
    fn the_header_lists_every_export() {
        let header = crate_file("include/glitnir.h");
        let mut found: Vec<String> = Vec::new();
        let b = header.as_bytes();
        let mut i = 0;
        while let Some(off) = header[i..].find("glitnir_") {
            let start = i + off;
            let mut end = start;
            while end < b.len()
                && (b[end].is_ascii_lowercase() || b[end].is_ascii_digit() || b[end] == b'_')
            {
                end += 1;
            }
            // a function declaration: the name is followed by " ("
            if header[end..].starts_with(" (") {
                found.push(header[start..end].to_string());
            }
            i = end.max(start + 1);
        }
        found.sort();
        found.dedup();
        let mut want: Vec<String> = EXPORTS.iter().map(|s| s.to_string()).collect();
        want.sort();
        assert_eq!(found, want);
    }

    #[test]
    fn the_crate_names_no_host() {
        // The needles are assembled so that this file does not contain them.
        let needles = [
            ["nid", "hogg"].concat(),
            ["drgn", "x"].concat(),
            ["ema", "cs"].concat(),
        ];
        let mut files = Vec::new();
        rust_files(
            &Path::new(env!("CARGO_MANIFEST_DIR")).join("src"),
            &mut files,
        );
        let mut texts: Vec<(String, String)> = files
            .iter()
            .map(|p| (p.display().to_string(), std::fs::read_to_string(p).unwrap()))
            .collect();
        texts.push(("include/glitnir.h".into(), crate_file("include/glitnir.h")));
        for (name, text) in texts {
            let lower = text.to_lowercase();
            for n in &needles {
                assert!(!lower.contains(n.as_str()), "{name} mentions {n}");
            }
        }
    }
}

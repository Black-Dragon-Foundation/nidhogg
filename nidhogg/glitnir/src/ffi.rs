//! The C ABI: status codes, the panic guard and the exported functions.

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
    "glitnir_debug_panic",
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

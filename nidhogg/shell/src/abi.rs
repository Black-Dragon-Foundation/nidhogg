//! The ABI version, and what keeps Glitnir's exports in `libnhshell.a`.

use glitnir::ffi::{GLITNIR_E_ARG, GLITNIR_E_PANIC, GLITNIR_OK};
use std::panic::catch_unwind;

pub const NHSHELL_OK: i32 = GLITNIR_OK;
pub const NHSHELL_E_PANIC: i32 = GLITNIR_E_PANIC;
pub const NHSHELL_E_ARG: i32 = GLITNIR_E_ARG;

/// The ABI version the header describes.
const ABI_VERSION: u32 = 1;

/// Write the ABI version to `out`.
///
/// # Safety
/// `out` must be null or point to a writable `u32`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn nhshell_abi_version(out: *mut u32) -> i32 {
    catch_unwind(|| {
        if out.is_null() {
            return NHSHELL_E_ARG;
        }
        // SAFETY: non-null, and the caller promises it is writable.
        unsafe { out.write(ABI_VERSION) };
        NHSHELL_OK
    })
    .unwrap_or(NHSHELL_E_PANIC)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_is_one() {
        let mut v = 0u32;
        // SAFETY: `v` is a writable u32.
        assert_eq!(unsafe { nhshell_abi_version(&mut v) }, NHSHELL_OK);
        assert_eq!(v, 1);
        // SAFETY: null is allowed and reported.
        assert_eq!(
            unsafe { nhshell_abi_version(std::ptr::null_mut()) },
            NHSHELL_E_ARG
        );
    }
}

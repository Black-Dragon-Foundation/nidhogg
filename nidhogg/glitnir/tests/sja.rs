//! Glitnir carries Sjá (nidhogg/sja): its entry point is re-exported, so `libglitnir.a` (the NS build) and, through
//! the glue's `pub use glitnir::*`, `libnhglue.a` hold `sja_call` (pattern D).

use glitnir::sja::abi::{Ans, SJA_OK, SJA_STATS, SjaArg, SjaTok};

type Entry = unsafe extern "C" fn(i32, *const SjaArg, usize, *mut *const SjaTok, *mut usize) -> i32;

#[test]
fn reexports_sja_call() {
    let f: Entry = glitnir::sja_call;
    let mut out: *const SjaTok = std::ptr::null();
    let mut n = 0usize;
    // SAFETY: no arguments; `out` and `n` are writable locals.
    assert_eq!(
        unsafe { f(SJA_STATS, std::ptr::null(), 0, &mut out, &mut n) },
        SJA_OK
    );
    assert!(matches!(
        glitnir::sja::abi::call_for_test(SJA_STATS, &[]),
        Ok(Ans::List(_))
    ));
}

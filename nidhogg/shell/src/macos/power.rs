//! Idle sleep: `ShellBackend::inhibit_idle` holds a power assertion (`IOPMAssertion`) while a client wants the
//! display to stay awake (`nhg_inhibit_idle`).

use objc2_foundation::NSString;
use std::ffi::c_void;

#[link(name = "IOKit", kind = "framework")]
unsafe extern "C" {
    fn IOPMAssertionCreateWithName(
        assertion_type: *const c_void,
        level: u32,
        name: *const c_void,
        id: *mut u32,
    ) -> i32;
    fn IOPMAssertionRelease(id: u32) -> i32;
}

/// `kIOPMAssertionLevelOn`.
const LEVEL_ON: u32 = 255;

/// Take an assertion that keeps the system from sleeping for being idle; its id, or `None` when IOKit refuses.
pub fn hold(reason: &str) -> Option<u32> {
    let kind = NSString::from_str("PreventUserIdleSystemSleep");
    let name = NSString::from_str(reason);
    let mut id = 0u32;
    // SAFETY: NSString is toll-free bridged to CFString; both outlive the call, and `id` is writable.
    let rc = unsafe {
        IOPMAssertionCreateWithName(
            (&*kind as *const NSString).cast(),
            LEVEL_ON,
            (&*name as *const NSString).cast(),
            &mut id,
        )
    };
    (rc == 0).then_some(id)
}

/// Release an assertion `hold` made.
pub fn release(id: u32) {
    // SAFETY: an id `hold` returned; releasing twice is harmless to IOKit.
    unsafe { IOPMAssertionRelease(id) };
}

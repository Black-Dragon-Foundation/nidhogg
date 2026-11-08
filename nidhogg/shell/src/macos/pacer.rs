//! The commit pacer's run-loop timer (`crate::ring::Pacer`).  While a window holds a commit, the timer looks every
//! [`PERIOD`] seconds for a freed slot (or the hold's bound) and asks the host for the held publish; it is disarmed as
//! soon as no window holds one, so an idle Emacs runs no timer.  It runs in the common modes, so live resize and menu
//! tracking see it too.  It reports through `runloop::deliver`, as the window delegates do: the main thread's run
//! loop runs only while the backend is lent, and a fire outside a lend waits for the next.

use super::runloop::{MacEv, deliver};
use super::surface::STATS;
use std::ffi::c_void;
use std::sync::atomic::Ordering;

type CFRef = *mut c_void;

/// `CFRunLoopTimerContext`.
#[repr(C)]
struct Context {
    version: isize,
    info: *mut c_void,
    retain: *const c_void,
    release: *const c_void,
    copy_description: *const c_void,
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    static kCFRunLoopCommonModes: CFRef;
    fn CFRunLoopGetMain() -> CFRef;
    fn CFAbsoluteTimeGetCurrent() -> f64;
    fn CFRunLoopTimerCreate(
        alloc: CFRef,
        fire_date: f64,
        interval: f64,
        flags: usize,
        order: isize,
        callout: extern "C" fn(CFRef, *mut c_void),
        context: *mut Context,
    ) -> CFRef;
    fn CFRunLoopAddTimer(rl: CFRef, timer: CFRef, mode: CFRef);
    fn CFRunLoopTimerInvalidate(timer: CFRef);
    fn CFRelease(cf: *const c_void);
}

/// Seconds between the timer's looks: a quarter of a 60 Hz frame.
pub const PERIOD: f64 = 0.004;

extern "C" fn fired(_timer: CFRef, _info: *mut c_void) {
    STATS.pacer_fires.fetch_add(1, Ordering::Relaxed);
    deliver(MacEv::Pace);
}

/// The timer, armed or not.  Main thread only.
#[derive(Debug, Default)]
pub struct Timer(Option<CFRef>);

impl Timer {
    /// Arm it (nothing when it is armed).
    pub fn arm(&mut self) {
        if self.0.is_some() {
            return;
        }
        let mut ctx = Context {
            version: 0,
            info: std::ptr::null_mut(),
            retain: std::ptr::null(),
            release: std::ptr::null(),
            copy_description: std::ptr::null(),
        };
        // SAFETY: CF calls on the main run loop; the context is copied by the call; the timer is ours (+1) until
        // `disarm` releases it.
        unsafe {
            let t = CFRunLoopTimerCreate(
                std::ptr::null_mut(),
                CFAbsoluteTimeGetCurrent() + PERIOD,
                PERIOD,
                0,
                0,
                fired,
                &raw mut ctx,
            );
            if t.is_null() {
                return;
            }
            CFRunLoopAddTimer(CFRunLoopGetMain(), t, kCFRunLoopCommonModes);
            self.0 = Some(t);
            STATS.pacer_arms.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Disarm it (nothing when it is not armed).
    pub fn disarm(&mut self) {
        if let Some(t) = self.0.take() {
            // SAFETY: our timer, invalidated (removed from the run loop) and released once.
            unsafe {
                CFRunLoopTimerInvalidate(t);
                CFRelease(t);
            }
        }
    }
}

impl Drop for Timer {
    fn drop(&mut self) {
        self.disarm();
    }
}

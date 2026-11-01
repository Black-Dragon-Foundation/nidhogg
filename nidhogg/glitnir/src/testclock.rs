//! Virtual time and a deadline queue for test runs.
//!
//! The host keeps what touches its own state (the idle check, the hooks); this holds the clock, which sees none of it,
//! so any thread may call it.  One elapsed time drives a wall reading and a monotonic one, which therefore move in step.
//! An entry of the queue is a thread's timed wait: its condition variable and mutex come back through
//! [`glitnir_tc_take_due`] once virtual time reaches its deadline, and only the host ever touches them.  They must
//! outlive the wait.

use std::ffi::c_void;
use std::sync::{Mutex, MutexGuard};

/// `glitnir_tc_register`: the clock is off, the caller waits on real time.
const OFF: u64 = 0;
/// `glitnir_tc_register`: the deadline has passed, there is nothing to wait for.
const DUE: u64 = 1;
const FIRST_ID: u64 = 2;

struct Entry {
    deadline: u64,
    id: u64,
    cv: *mut c_void,
    m: *mut c_void,
}

struct Clock {
    on: bool,
    wall0: i64,
    mono0: u64,
    elapsed: u64,
    next_id: u64,
    /// Sorted by deadline; equal deadlines in the order they registered.
    queue: Vec<Entry>,
}

// SAFETY: the two pointers are opaque here; only the C side, which owns them, reads them.
unsafe impl Send for Clock {}

static CLOCK: Mutex<Clock> = Mutex::new(Clock {
    on: false,
    wall0: 0,
    mono0: 0,
    elapsed: 0,
    next_id: FIRST_ID,
    queue: Vec::new(),
});

fn clock() -> MutexGuard<'static, Clock> {
    CLOCK.lock().unwrap_or_else(|p| p.into_inner())
}

/// Start at the real readings WALL_NS and MONO_NS, with nothing elapsed.
#[unsafe(no_mangle)]
pub extern "C" fn glitnir_tc_start(wall_ns: i64, mono_ns: u64) {
    let mut c = clock();
    c.on = true;
    c.wall0 = wall_ns;
    c.mono0 = mono_ns;
    c.elapsed = 0;
}

/// Stop.  Entries still queued come back through `glitnir_tc_take_due`, every one of them.
#[unsafe(no_mangle)]
pub extern "C" fn glitnir_tc_stop() {
    clock().on = false;
}

#[unsafe(no_mangle)]
pub extern "C" fn glitnir_tc_wall_ns() -> i64 {
    let c = clock();
    c.wall0 + c.elapsed as i64
}

#[unsafe(no_mangle)]
pub extern "C" fn glitnir_tc_mono_ns() -> u64 {
    let c = clock();
    c.mono0 + c.elapsed
}

/// Move the monotonic reading to MONO_NS, and the wall reading with it.  Time never goes back: an earlier MONO_NS is a
/// no-op.
#[unsafe(no_mangle)]
pub extern "C" fn glitnir_tc_advance_to(mono_ns: u64) {
    let mut c = clock();
    let to = mono_ns.saturating_sub(c.mono0);
    if to > c.elapsed {
        c.elapsed = to;
    }
}

/// The earliest deadline queued, 0 if none.
#[unsafe(no_mangle)]
pub extern "C" fn glitnir_tc_next_deadline() -> u64 {
    clock().queue.first().map_or(0, |e| e.deadline)
}

/// How many waits are queued.
#[unsafe(no_mangle)]
pub extern "C" fn glitnir_tc_waiting() -> u64 {
    clock().queue.len() as u64
}

/// Queue a wait on CV and M until the monotonic reading DEADLINE.  Return its id, or 0 if the clock is off, or 1 if
/// DEADLINE has passed (neither queues anything).
#[unsafe(no_mangle)]
pub extern "C" fn glitnir_tc_register(deadline: u64, cv: *mut c_void, m: *mut c_void) -> u64 {
    let mut c = clock();
    if !c.on {
        return OFF;
    }
    if deadline <= c.mono0 + c.elapsed {
        return DUE;
    }
    let id = c.next_id;
    c.next_id += 1;
    let at = c.queue.partition_point(|e| e.deadline <= deadline);
    c.queue.insert(
        at,
        Entry {
            deadline,
            id,
            cv,
            m,
        },
    );
    id
}

/// Take the wait ID off the queue.  Return 1 if it was there, 0 if `glitnir_tc_take_due` took it first.
#[unsafe(no_mangle)]
pub extern "C" fn glitnir_tc_unregister(id: u64) -> i32 {
    let mut c = clock();
    match c.queue.iter().position(|e| e.id == id) {
        Some(i) => {
            c.queue.remove(i);
            1
        }
        None => 0,
    }
}

/// Take the earliest wait whose deadline has passed (with the clock off, the earliest of all), its condition variable
/// in *CV and its mutex in *M.  Return 1, or 0 if there is none.
///
/// # Safety
///
/// CV and M must be valid for writes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn glitnir_tc_take_due(cv: *mut *mut c_void, m: *mut *mut c_void) -> i32 {
    let mut c = clock();
    let now = c.mono0 + c.elapsed;
    let due = c.queue.first().is_some_and(|e| !c.on || e.deadline <= now);
    if !due {
        return 0;
    }
    let e = c.queue.remove(0);
    // SAFETY: the caller's promise.
    unsafe {
        *cv = e.cv;
        *m = e.m;
    }
    1
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ptr::null_mut;

    /// The clock is one global: the tests take turns.
    static TURN: Mutex<()> = Mutex::new(());

    fn take() -> Option<(usize, usize)> {
        let (mut cv, mut m) = (null_mut(), null_mut());
        // SAFETY: both point at locals.
        (unsafe { glitnir_tc_take_due(&mut cv, &mut m) } == 1).then_some((cv as usize, m as usize))
    }

    fn drain() {
        glitnir_tc_stop();
        while take().is_some() {}
    }

    fn fake(n: usize) -> *mut c_void {
        n as *mut c_void
    }

    #[test]
    fn wall_and_monotonic_move_in_step() {
        let _t = TURN.lock().unwrap();
        glitnir_tc_start(5_000, 100);
        assert_eq!((glitnir_tc_wall_ns(), glitnir_tc_mono_ns()), (5_000, 100));
        glitnir_tc_advance_to(350);
        assert_eq!((glitnir_tc_wall_ns(), glitnir_tc_mono_ns()), (5_250, 350));
        drain();
    }

    #[test]
    fn time_never_goes_back() {
        let _t = TURN.lock().unwrap();
        glitnir_tc_start(0, 1_000);
        glitnir_tc_advance_to(2_000);
        glitnir_tc_advance_to(1_500);
        glitnir_tc_advance_to(1);
        assert_eq!(glitnir_tc_mono_ns(), 2_000);
        drain();
    }

    #[test]
    fn a_wait_comes_due_at_its_deadline_and_not_before() {
        let _t = TURN.lock().unwrap();
        glitnir_tc_start(0, 0);
        assert!(glitnir_tc_register(10, fake(1), fake(2)) >= FIRST_ID);
        assert_eq!(glitnir_tc_next_deadline(), 10);
        glitnir_tc_advance_to(9);
        assert_eq!(take(), None);
        glitnir_tc_advance_to(10);
        assert_eq!(take(), Some((1, 2)));
        assert_eq!(take(), None);
        assert_eq!(glitnir_tc_next_deadline(), 0);
        drain();
    }

    #[test]
    fn waits_come_due_in_deadline_order_then_in_order_of_arrival() {
        let _t = TURN.lock().unwrap();
        glitnir_tc_start(0, 0);
        glitnir_tc_register(20, fake(1), fake(1));
        glitnir_tc_register(10, fake(2), fake(2));
        glitnir_tc_register(20, fake(3), fake(3));
        glitnir_tc_advance_to(30);
        assert_eq!(take(), Some((2, 2)));
        assert_eq!(take(), Some((1, 1)));
        assert_eq!(take(), Some((3, 3)));
        drain();
    }

    #[test]
    fn unregister_says_who_took_it() {
        let _t = TURN.lock().unwrap();
        glitnir_tc_start(0, 0);
        let a = glitnir_tc_register(10, fake(1), fake(1));
        let b = glitnir_tc_register(10, fake(2), fake(2));
        assert_eq!(glitnir_tc_unregister(a), 1);
        assert_eq!(glitnir_tc_unregister(a), 0);
        assert_eq!(glitnir_tc_waiting(), 1);
        glitnir_tc_advance_to(10);
        assert_eq!(take(), Some((2, 2)));
        assert_eq!(glitnir_tc_unregister(b), 0);
        drain();
    }

    #[test]
    fn registering_off_or_past_the_deadline_queues_nothing() {
        let _t = TURN.lock().unwrap();
        assert_eq!(glitnir_tc_register(10, fake(1), fake(1)), OFF);
        glitnir_tc_start(0, 100);
        assert_eq!(glitnir_tc_register(100, fake(1), fake(1)), DUE);
        assert_eq!(glitnir_tc_waiting(), 0);
        drain();
    }

    #[test]
    fn stopping_hands_back_every_wait() {
        let _t = TURN.lock().unwrap();
        glitnir_tc_start(0, 0);
        glitnir_tc_register(1_000, fake(1), fake(1));
        glitnir_tc_stop();
        assert_eq!(take(), Some((1, 1)));
        assert_eq!(take(), None);
    }
}

//! `nhg_wait`'s fd sets: what C's `fd_set`s ask for, as the shell's registrations (`nhshell::wait`), and the ready
//! registrations written back into them.  The wait itself, its kqueue and its wake order are the shell's.

use nhshell::wait::{FILT_READ, FILT_WRITE, Reg};
use std::collections::BTreeSet;

/// The registrations `rfds` and `wfds` (either null) ask for, below `nfds`.
///
/// # Safety
/// Each set is null or a valid `fd_set`.
pub unsafe fn want(
    nfds: i32,
    rfds: *const libc::fd_set,
    wfds: *const libc::fd_set,
) -> BTreeSet<Reg> {
    let mut out = BTreeSet::new();
    let n = nfds.clamp(0, libc::FD_SETSIZE as i32);
    for (set, filt) in [(rfds, FILT_READ), (wfds, FILT_WRITE)] {
        if set.is_null() {
            continue;
        }
        for fd in 0..n {
            // SAFETY: a valid set (the caller's promise), and `fd` below FD_SETSIZE.
            if unsafe { libc::FD_ISSET(fd, set) } {
                out.insert((fd, filt));
            }
        }
    }
    out
}

/// Write `ready` into `rfds` and `wfds` (either null), cleared first, as `select` leaves its sets; the count of set
/// bits, as `select` returns it.
///
/// # Safety
/// Each set is null or a valid, writable `fd_set`.
pub unsafe fn fill(ready: &[Reg], rfds: *mut libc::fd_set, wfds: *mut libc::fd_set) -> i32 {
    for set in [rfds, wfds] {
        if !set.is_null() {
            // SAFETY: a valid, writable set.
            unsafe { libc::FD_ZERO(set) };
        }
    }
    let mut n = 0;
    for &(fd, filt) in ready {
        let set = if filt == FILT_WRITE { wfds } else { rfds };
        if set.is_null() || !(0..libc::FD_SETSIZE as i32).contains(&fd) {
            continue;
        }
        // SAFETY: as above; `fd` is below FD_SETSIZE.
        unsafe {
            if !libc::FD_ISSET(fd, set) {
                libc::FD_SET(fd, set);
                n += 1;
            }
        }
    }
    n
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty() -> libc::fd_set {
        // SAFETY: plain data, then cleared.
        unsafe {
            let mut s: libc::fd_set = std::mem::zeroed();
            libc::FD_ZERO(&mut s);
            s
        }
    }

    #[test]
    fn sets_round_trip() {
        let (mut r, mut w) = (empty(), empty());
        // SAFETY: our sets; fds below FD_SETSIZE.
        unsafe {
            libc::FD_SET(3, &mut r);
            libc::FD_SET(7, &mut r);
            libc::FD_SET(7, &mut w);
            libc::FD_SET(40, &mut r);
        }
        // SAFETY: valid sets.
        let got = unsafe { want(10, &r, &w) };
        assert_eq!(
            got.into_iter().collect::<Vec<_>>(),
            vec![(3, FILT_READ), (7, FILT_READ), (7, FILT_WRITE)],
            "fd 40 is past nfds"
        );
        // SAFETY: valid, writable sets.
        let n = unsafe {
            fill(
                &[(7, FILT_WRITE), (3, FILT_READ), (3, FILT_READ)],
                &mut r,
                &mut w,
            )
        };
        assert_eq!(n, 2);
        // SAFETY: valid sets.
        unsafe {
            assert!(libc::FD_ISSET(3, &r) && !libc::FD_ISSET(7, &r) && !libc::FD_ISSET(40, &r));
            assert!(libc::FD_ISSET(7, &w));
        }
    }

    #[test]
    fn a_null_write_set_is_skipped() {
        let mut r = empty();
        // SAFETY: our set.
        unsafe { libc::FD_SET(5, &mut r) };
        // SAFETY: a valid set and a null one.
        let got = unsafe { want(8, &r, std::ptr::null()) };
        assert_eq!(got.len(), 1);
        // SAFETY: as above.
        let n = unsafe { fill(&[(5, FILT_WRITE)], &mut r, std::ptr::null_mut()) };
        assert_eq!(n, 0);
    }
}

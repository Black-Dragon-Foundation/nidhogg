//! The change rings of an edit history: one per tracked text buffer of the host, keyed by an opaque id.
//!
//! The host calls [`glitnir_history_push`] after each change to a tracked text, and drains the ring later with
//! [`glitnir_history_peek`] and [`glitnir_history_pop`] (oldest first).  A ring that would grow past
//! [`MAX_CHANGES`] or [`MAX_BYTES`] drops what it holds and is marked for a resync: the host then reads the whole text
//! again, and changes pushed until it does are dropped too (the text it reads holds them).  The text of a change is the
//! host's bytes, kept as they are.  Called from the host's main thread; the lock only makes the static sound.

use crate::ffi::{GLITNIR_E_ARG, GLITNIR_E_STATE, GLITNIR_OK, ffi_guard};
use std::collections::{HashMap, VecDeque};
use std::sync::{Mutex, MutexGuard};

/// The most changes a ring holds before it resyncs.
pub const MAX_CHANGES: usize = 1 << 16;
/// The most text bytes a ring holds before it resyncs.
pub const MAX_BYTES: usize = 64 << 20;

/// `glitnir_history_peek` and `glitnir_history_push`: the ring is empty.
pub const GLITNIR_HISTORY_EMPTY: i32 = 1;
/// `glitnir_history_peek` and `glitnir_history_push`: the ring is marked for a resync.
pub const GLITNIR_HISTORY_RESYNC: i32 = 2;

/// One change as the host sees it: the text from character START, OLD_LEN characters long, was replaced by
/// NCHARS characters (NBYTES bytes) at TIME, under ORIGIN (the host's index of who made it).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct HistoryHead {
    pub start: i64,
    pub old_len: i64,
    pub nchars: i64,
    pub nbytes: u64,
    pub time: f64,
    pub origin: u32,
}

#[derive(Debug)]
struct Change {
    head: HistoryHead,
    text: Vec<u8>,
}

#[derive(Debug, Default)]
struct Ring {
    changes: VecDeque<Change>,
    bytes: usize,
    resync: bool,
}

#[derive(Debug, Default)]
struct Rings {
    by_key: HashMap<u64, Ring>,
}

static RINGS: Mutex<Option<Rings>> = Mutex::new(None);

fn rings() -> MutexGuard<'static, Option<Rings>> {
    RINGS.lock().unwrap_or_else(|p| p.into_inner())
}

fn with<R>(f: impl FnOnce(&mut Rings) -> R) -> R {
    f(rings().get_or_insert_with(Rings::default))
}

/// Start (ON) or stop tracking KEY.  Starting a tracked key keeps its ring; stopping drops it.
pub fn track(key: u64, on: bool) {
    with(|r| {
        if on {
            r.by_key.entry(key).or_default();
        } else {
            r.by_key.remove(&key);
        }
    });
}

/// Record a change of KEY whose text is the concatenation of PARTS.  `GLITNIR_OK` when kept,
/// `GLITNIR_HISTORY_RESYNC` when the ring is (now) marked for a resync, `GLITNIR_E_STATE` when KEY is not tracked.
pub fn push(key: u64, mut head: HistoryHead, parts: &[&[u8]]) -> i32 {
    with(|r| {
        let Some(ring) = r.by_key.get_mut(&key) else {
            return GLITNIR_E_STATE;
        };
        if ring.resync {
            return GLITNIR_HISTORY_RESYNC;
        }
        let n: usize = parts.iter().map(|p| p.len()).sum();
        if ring.changes.len() >= MAX_CHANGES || ring.bytes + n > MAX_BYTES {
            mark(ring);
            return GLITNIR_HISTORY_RESYNC;
        }
        let mut text = Vec::with_capacity(n);
        for p in parts {
            text.extend_from_slice(p);
        }
        head.nbytes = n as u64;
        ring.bytes += n;
        ring.changes.push_back(Change { head, text });
        GLITNIR_OK
    })
}

fn mark(ring: &mut Ring) {
    ring.changes.clear();
    ring.bytes = 0;
    ring.resync = true;
}

/// Mark KEY's ring for a resync (its text was replaced as a whole).
pub fn resync(key: u64) -> i32 {
    with(|r| match r.by_key.get_mut(&key) {
        Some(ring) => {
            mark(ring);
            GLITNIR_OK
        }
        None => GLITNIR_E_STATE,
    })
}

/// KEY's oldest change, or why there is none.
pub fn peek(key: u64) -> Result<HistoryHead, i32> {
    with(|r| {
        let ring = r.by_key.get(&key).ok_or(GLITNIR_E_STATE)?;
        if ring.resync {
            return Err(GLITNIR_HISTORY_RESYNC);
        }
        ring.changes
            .front()
            .map(|c| c.head)
            .ok_or(GLITNIR_HISTORY_EMPTY)
    })
}

/// Remove KEY's oldest change, its text copied into OUT (exactly its length).  `GLITNIR_E_ARG` when OUT has another
/// length, `GLITNIR_HISTORY_EMPTY` or `GLITNIR_HISTORY_RESYNC` when there is none.
pub fn pop(key: u64, out: &mut [u8]) -> i32 {
    with(|r| {
        let Some(ring) = r.by_key.get_mut(&key) else {
            return GLITNIR_E_STATE;
        };
        if ring.resync {
            return GLITNIR_HISTORY_RESYNC;
        }
        match ring.changes.front() {
            None => GLITNIR_HISTORY_EMPTY,
            Some(c) if c.text.len() != out.len() => GLITNIR_E_ARG,
            Some(_) => {
                let c = ring.changes.pop_front().expect("a front");
                out.copy_from_slice(&c.text);
                ring.bytes -= c.text.len();
                GLITNIR_OK
            }
        }
    })
}

/// Clear KEY's ring and its resync mark (the host read the whole text).
pub fn clear(key: u64) -> i32 {
    with(|r| match r.by_key.get_mut(&key) {
        Some(ring) => {
            *ring = Ring::default();
            GLITNIR_OK
        }
        None => GLITNIR_E_STATE,
    })
}

/// How many rings hold a change or a resync mark.
pub fn pending() -> u64 {
    with(|r| {
        r.by_key
            .values()
            .filter(|g| g.resync || !g.changes.is_empty())
            .count() as u64
    })
}

/// Start or stop tracking KEY (ON non-zero: start).
#[unsafe(no_mangle)]
pub extern "C" fn glitnir_history_track(key: u64, on: u32) -> i32 {
    ffi_guard(|| {
        track(key, on != 0);
        GLITNIR_OK
    })
}

/// Record a change of KEY: HEAD (its `nbytes` is set here), its text the N1 bytes at P1 then the N2 at P2 (either may
/// be NULL when its length is 0).
///
/// # Safety
/// `head` must point to a readable `HistoryHead`; `p1` and `p2` to N1 and N2 readable bytes unless their length is 0.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn glitnir_history_push(
    key: u64,
    head: *const HistoryHead,
    p1: *const u8,
    n1: usize,
    p2: *const u8,
    n2: usize,
) -> i32 {
    ffi_guard(|| {
        if head.is_null() || (p1.is_null() && n1 > 0) || (p2.is_null() && n2 > 0) {
            return GLITNIR_E_ARG;
        }
        // SAFETY: the caller promises these are readable for their lengths; a zero length is never dereferenced.
        let (h, a, b) = unsafe {
            (
                head.read(),
                if n1 == 0 {
                    &[][..]
                } else {
                    std::slice::from_raw_parts(p1, n1)
                },
                if n2 == 0 {
                    &[][..]
                } else {
                    std::slice::from_raw_parts(p2, n2)
                },
            )
        };
        push(key, h, &[a, b])
    })
}

/// Mark KEY's ring for a resync.
#[unsafe(no_mangle)]
pub extern "C" fn glitnir_history_resync(key: u64) -> i32 {
    ffi_guard(|| resync(key))
}

/// Write KEY's oldest change to *OUT: `GLITNIR_OK`, or `GLITNIR_HISTORY_EMPTY`, `GLITNIR_HISTORY_RESYNC`,
/// `GLITNIR_E_STATE` (not tracked).
///
/// # Safety
/// `out` must be null or point to a writable `HistoryHead`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn glitnir_history_peek(key: u64, out: *mut HistoryHead) -> i32 {
    ffi_guard(|| {
        if out.is_null() {
            return GLITNIR_E_ARG;
        }
        match peek(key) {
            Ok(h) => {
                // SAFETY: non-null, and the caller promises it is writable.
                unsafe { out.write(h) };
                GLITNIR_OK
            }
            Err(e) => e,
        }
    })
}

/// Remove KEY's oldest change, its text copied to BUF (LEN bytes: the change's `nbytes`).
///
/// # Safety
/// `buf` must point to LEN writable bytes unless LEN is 0.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn glitnir_history_pop(key: u64, buf: *mut u8, len: usize) -> i32 {
    ffi_guard(|| {
        if buf.is_null() && len > 0 {
            return GLITNIR_E_ARG;
        }
        let out = if len == 0 {
            &mut [][..]
        } else {
            // SAFETY: the caller promises LEN writable bytes.
            unsafe { std::slice::from_raw_parts_mut(buf, len) }
        };
        pop(key, out)
    })
}

/// Clear KEY's ring and its resync mark.
#[unsafe(no_mangle)]
pub extern "C" fn glitnir_history_clear(key: u64) -> i32 {
    ffi_guard(|| clear(key))
}

/// Write to *OUT how many rings hold a change or a resync mark.
///
/// # Safety
/// `out` must be null or point to a writable `u64`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn glitnir_history_pending(out: *mut u64) -> i32 {
    ffi_guard(|| {
        if out.is_null() {
            return GLITNIR_E_ARG;
        }
        // SAFETY: non-null, and the caller promises it is writable.
        unsafe { out.write(pending()) };
        GLITNIR_OK
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // Every test takes its own keys: the rings are one static, and the tests run in parallel.
    fn head(start: i64, old_len: i64, nchars: i64) -> HistoryHead {
        HistoryHead {
            start,
            old_len,
            nchars,
            nbytes: 0,
            time: 1.5,
            origin: 3,
        }
    }

    fn drain(key: u64) -> Result<Vec<(HistoryHead, Vec<u8>)>, i32> {
        let mut out = Vec::new();
        loop {
            match peek(key) {
                Ok(h) => {
                    let mut buf = vec![0; h.nbytes as usize];
                    assert_eq!(pop(key, &mut buf), GLITNIR_OK);
                    out.push((h, buf));
                }
                Err(GLITNIR_HISTORY_EMPTY) => return Ok(out),
                Err(e) => return Err(e),
            }
        }
    }

    /// Pattern D: changes come back oldest first, with the two parts of a text joined; an untracked key is refused.
    #[test]
    fn changes_come_back_in_order() {
        let k = 0x1001;
        assert_eq!(push(k, head(1, 0, 1), &[b"a"]), GLITNIR_E_STATE);
        track(k, true);
        assert_eq!(push(k, head(1, 0, 2), &[b"ab", b""]), GLITNIR_OK);
        assert_eq!(push(k, head(2, 1, 3), &[b"x", b"yz"]), GLITNIR_OK);
        assert_eq!(push(k, head(4, 2, 0), &[]), GLITNIR_OK);
        let got = drain(k).unwrap();
        assert_eq!(got.len(), 3);
        assert_eq!(
            (got[0].0.start, got[0].0.nbytes, &got[0].1[..]),
            (1, 2, &b"ab"[..])
        );
        assert_eq!((got[1].0.old_len, &got[1].1[..]), (1, &b"xyz"[..]));
        assert_eq!(
            (got[2].0.old_len, got[2].0.nbytes, got[2].0.origin),
            (2, 0, 3)
        );
        assert_eq!(peek(k), Err(GLITNIR_HISTORY_EMPTY));
        track(k, false);
        assert_eq!(peek(k), Err(GLITNIR_E_STATE));
    }

    /// Pattern D: a pop into a buffer of the wrong length takes nothing.
    #[test]
    fn a_pop_takes_exactly_the_text() {
        let k = 0x1002;
        track(k, true);
        push(k, head(1, 0, 3), &[b"abc"]);
        assert_eq!(pop(k, &mut [0; 2]), GLITNIR_E_ARG);
        let mut buf = [0; 3];
        assert_eq!(pop(k, &mut buf), GLITNIR_OK);
        assert_eq!(&buf, b"abc");
        assert_eq!(pop(k, &mut []), GLITNIR_HISTORY_EMPTY);
        track(k, false);
    }

    /// Pattern D: past the count cap the ring resyncs, drops what it had and every later push, until cleared.
    #[test]
    fn the_count_cap_resyncs() {
        let k = 0x1003;
        track(k, true);
        for i in 0..MAX_CHANGES {
            assert_eq!(push(k, head(i as i64 + 1, 0, 1), &[b"x"]), GLITNIR_OK);
        }
        assert_eq!(push(k, head(1, 0, 1), &[b"x"]), GLITNIR_HISTORY_RESYNC);
        assert_eq!(push(k, head(1, 0, 1), &[b"x"]), GLITNIR_HISTORY_RESYNC);
        assert_eq!(peek(k), Err(GLITNIR_HISTORY_RESYNC));
        assert_eq!(clear(k), GLITNIR_OK);
        assert_eq!(peek(k), Err(GLITNIR_HISTORY_EMPTY));
        assert_eq!(push(k, head(1, 0, 1), &[b"y"]), GLITNIR_OK);
        assert_eq!(drain(k).unwrap().len(), 1);
        track(k, false);
    }

    /// Pattern D: one change as large as the byte cap is kept; a byte more resyncs.
    #[test]
    fn the_byte_cap_resyncs() {
        let k = 0x1004;
        track(k, true);
        let big = vec![b'z'; MAX_BYTES];
        assert_eq!(push(k, head(1, 0, MAX_BYTES as i64), &[&big]), GLITNIR_OK);
        assert_eq!(push(k, head(1, 0, 1), &[b"!"]), GLITNIR_HISTORY_RESYNC);
        assert_eq!(drain(k), Err(GLITNIR_HISTORY_RESYNC));
        clear(k);
        assert_eq!(push(k, head(1, 0, 1), &[b"!"]), GLITNIR_OK);
        track(k, false);
    }

    /// Pattern D: a resync asked for drops the ring; tracking a tracked key again keeps it.
    #[test]
    fn a_resync_drops_the_ring() {
        let k = 0x1005;
        track(k, true);
        push(k, head(1, 0, 1), &[b"a"]);
        track(k, true);
        assert_eq!(peek(k).map(|h| h.nbytes), Ok(1));
        assert_eq!(resync(k), GLITNIR_OK);
        assert_eq!(peek(k), Err(GLITNIR_HISTORY_RESYNC));
        assert_eq!(resync(0x1fff), GLITNIR_E_STATE);
        track(k, false);
    }

    /// Pattern D: the C entry points check their pointers.
    #[test]
    fn the_entry_points_check_their_arguments() {
        let k = 0x1006;
        assert_eq!(glitnir_history_track(k, 1), GLITNIR_OK);
        let h = head(1, 0, 2);
        // SAFETY: valid pointers and lengths, or null with length 0.
        unsafe {
            let none = std::ptr::null();
            assert_eq!(
                glitnir_history_push(k, none, none.cast(), 0, none.cast(), 0),
                GLITNIR_E_ARG
            );
            assert_eq!(
                glitnir_history_push(k, &h, none.cast(), 1, none.cast(), 0),
                GLITNIR_E_ARG
            );
            assert_eq!(
                glitnir_history_push(k, &h, b"a".as_ptr(), 1, b"b".as_ptr(), 1),
                GLITNIR_OK
            );
            let mut out = HistoryHead::default();
            assert_eq!(glitnir_history_peek(k, std::ptr::null_mut()), GLITNIR_E_ARG);
            assert_eq!(glitnir_history_peek(k, &mut out), GLITNIR_OK);
            assert_eq!(out.nbytes, 2);
            let mut n = 0u64;
            assert_eq!(glitnir_history_pending(&mut n), GLITNIR_OK);
            assert!(n >= 1);
            let mut buf = [0u8; 2];
            assert_eq!(glitnir_history_pop(k, buf.as_mut_ptr(), 2), GLITNIR_OK);
            assert_eq!(&buf, b"ab");
            assert_eq!(
                glitnir_history_pop(k, std::ptr::null_mut(), 0),
                GLITNIR_HISTORY_EMPTY
            );
        }
        assert_eq!(glitnir_history_clear(k), GLITNIR_OK);
        assert_eq!(glitnir_history_track(k, 0), GLITNIR_OK);
        assert_eq!(glitnir_history_clear(k), GLITNIR_E_STATE);
    }
}

//! The C ABI (`include/hlidskjalf.h`): the models by key, behind one lock.  Every entry point catches a panic and
//! answers `HLID_E_PANIC`; the host calls them from its main thread only.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{LazyLock, Mutex};

use crate::{Fold, Mark, Model, Stale};

pub const HLID_OK: i32 = 0;
pub const HLID_E_PANIC: i32 = -1;
pub const HLID_E_ARG: i32 = -2;
/// No model for the key.
pub const HLID_E_NONE: i32 = -3;
/// The lines are being built (line answers are not there yet).
pub const HLID_BUILDING: i32 = 1;
/// The lines no longer match the text: build them again.
pub const HLID_STALE: i32 = 2;
/// Nothing found.
pub const HLID_NOTHING: i32 = 3;

/// The model's counters (`hlid_describe`).
#[repr(C)]
#[derive(Default)]
pub struct HlidStats {
    pub lines: u64,
    pub chars: u64,
    pub chunks: u64,
    pub marks: u64,
    pub folds: u64,
    pub hidden: u64,
    pub heap_bytes: u64,
    pub edits: u64,
    /// 0 ready, 1 building, 2 stale.
    pub state: i32,
}

static MODELS: LazyLock<Mutex<Vec<(u64, Model)>>> = LazyLock::new(|| Mutex::new(Vec::new()));

fn guard(f: impl FnOnce() -> i32) -> i32 {
    catch_unwind(AssertUnwindSafe(f)).unwrap_or(HLID_E_PANIC)
}

/// KEY's model under the lock, for a Rust caller in the same process (the glue's minimap reads its rows), on the
/// host's main thread as the C ABI is; `None` when KEY has no model.
pub fn with_model<R>(key: u64, f: impl FnOnce(&mut Model) -> R) -> Option<R> {
    let mut ms = MODELS.lock().unwrap_or_else(|e| e.into_inner());
    ms.iter_mut().find(|(k, _)| *k == key).map(|(_, m)| f(m))
}

fn with(key: u64, f: impl FnOnce(&mut Model) -> i32) -> i32 {
    guard(|| {
        let mut ms = MODELS.lock().unwrap_or_else(|e| e.into_inner());
        match ms.iter_mut().find(|(k, _)| *k == key) {
            Some((_, m)) => f(m),
            None => HLID_E_NONE,
        }
    })
}

/// # Safety
/// P1 and P2 must be readable for N1 and N2 bytes unless the length is 0.
unsafe fn parts<'a>(p1: *const u8, n1: usize, p2: *const u8, n2: usize) -> Option<[&'a [u8]; 2]> {
    if (p1.is_null() && n1 > 0) || (p2.is_null() && n2 > 0) {
        return None;
    }
    // SAFETY: the caller promises these are readable for their lengths; a zero length is never dereferenced.
    let a = if n1 == 0 {
        &[][..]
    } else {
        unsafe { std::slice::from_raw_parts(p1, n1) }
    };
    let b = if n2 == 0 {
        &[][..]
    } else {
        unsafe { std::slice::from_raw_parts(p2, n2) }
    };
    Some([a, b])
}

fn state(m: &mut Model) -> i32 {
    m.poll(false);
    if m.stale() {
        HLID_STALE
    } else if m.building() {
        HLID_BUILDING
    } else {
        HLID_OK
    }
}

/// Build KEY's model (made when new) from its text, the N1 bytes at P1 then the N2 at P2; on a thread of its own
/// when BACKGROUND.  Marks, folds and the trail of an existing model stay.
///
/// # Safety
/// As `parts`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hlid_build(
    key: u64,
    multibyte: u32,
    tab_width: u32,
    p1: *const u8,
    n1: usize,
    p2: *const u8,
    n2: usize,
    background: u32,
) -> i32 {
    guard(|| {
        // SAFETY: passed through from the caller's promise.
        let Some(t) = (unsafe { parts(p1, n1, p2, n2) }) else {
            return HLID_E_ARG;
        };
        let tw = tab_width.clamp(1, 1000) as u16;
        let mut ms = MODELS.lock().unwrap_or_else(|e| e.into_inner());
        match ms.iter_mut().find(|(k, _)| *k == key) {
            Some((_, m)) => {
                m.set_text_kind(multibyte != 0, tw);
                m.rebuild(&t, background != 0);
            }
            None => ms.push((key, Model::build(&t, multibyte != 0, tw, background != 0))),
        }
        HLID_OK
    })
}

/// Forget KEY's model.
#[unsafe(no_mangle)]
pub extern "C" fn hlid_drop(key: u64) -> i32 {
    guard(|| {
        let mut ms = MODELS.lock().unwrap_or_else(|e| e.into_inner());
        ms.retain(|(k, _)| *k != key);
        HLID_OK
    })
}

/// Wait for KEY's build, if one runs; answers the model's state.
#[unsafe(no_mangle)]
pub extern "C" fn hlid_wait(key: u64) -> i32 {
    with(key, |m| {
        m.poll(true);
        state(m)
    })
}

/// KEY's text changed (`Model::edit`): HLID_STALE when its lines must be built again.
///
/// # Safety
/// As `parts`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hlid_edit(
    key: u64,
    start: u64,
    old_end: u64,
    new_end: u64,
    p1: *const u8,
    n1: usize,
    p2: *const u8,
    n2: usize,
) -> i32 {
    // SAFETY: passed through from the caller's promise.
    let Some(t) = (unsafe { parts(p1, n1, p2, n2) }) else {
        return HLID_E_ARG;
    };
    with(key, |m| match m.edit(start, old_end, new_end, &t) {
        Ok(()) => HLID_OK,
        Err(Stale) => HLID_STALE,
    })
}

/// The 0-based line of POS into *OUT.
///
/// # Safety
/// OUT must be writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hlid_line_of(key: u64, pos: u64, out: *mut u64) -> i32 {
    if out.is_null() {
        return HLID_E_ARG;
    }
    with(key, |m| match m.line_of(pos) {
        // SAFETY: checked non-null; the caller promises it is writable.
        Some(l) => unsafe {
            *out = l;
            HLID_OK
        },
        None => state(m),
    })
}

/// The position 0-based LINE starts at into *OUT.
///
/// # Safety
/// OUT must be writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hlid_pos_of(key: u64, line: u64, out: *mut u64) -> i32 {
    if out.is_null() {
        return HLID_E_ARG;
    }
    with(key, |m| match m.pos_of(line) {
        // SAFETY: checked non-null; the caller promises it is writable.
        Some(p) => unsafe {
            *out = p;
            HLID_OK
        },
        None => state(m),
    })
}

/// The visible-line fraction of POS in BEGV..ZV into *OUT (`Model::fraction`).
///
/// # Safety
/// OUT must be writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hlid_fraction(
    key: u64,
    pos: u64,
    begv: u64,
    zv: u64,
    out: *mut f64,
) -> i32 {
    if out.is_null() {
        return HLID_E_ARG;
    }
    with(key, |m| match m.fraction(pos, begv, zv) {
        // SAFETY: checked non-null; the caller promises it is writable.
        Some(f) => unsafe {
            *out = f;
            HLID_OK
        },
        None => state(m),
    })
}

/// Replace KEY's marks of KIND with the N at MARKS.
///
/// # Safety
/// MARKS must be readable for N marks unless N is 0.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hlid_set_marks(key: u64, kind: u32, marks: *const Mark, n: usize) -> i32 {
    if marks.is_null() && n > 0 {
        return HLID_E_ARG;
    }
    // SAFETY: checked; the caller promises N readable marks.
    let ms = if n == 0 {
        &[][..]
    } else {
        unsafe { std::slice::from_raw_parts(marks, n) }
    };
    with(key, |m| {
        m.set_marks((kind & 63) as u8, ms);
        HLID_OK
    })
}

/// The next (DIR > 0) or previous mark from POS whose kind is in the bit set KINDS, into *OUT; HLID_NOTHING when
/// none.
///
/// # Safety
/// OUT must be writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hlid_next_mark(
    key: u64,
    pos: u64,
    kinds: u64,
    dir: i32,
    out: *mut u64,
) -> i32 {
    if out.is_null() {
        return HLID_E_ARG;
    }
    with(key, |m| match m.next_mark(pos, kinds, dir) {
        // SAFETY: checked non-null; the caller promises it is writable.
        Some(p) => unsafe {
            *out = p;
            HLID_OK
        },
        None => HLID_NOTHING,
    })
}

/// The marks of each kind in [BEG, END) into OUT[0..64].
///
/// # Safety
/// OUT must be writable for 64 counts.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hlid_counts(key: u64, beg: u64, end: u64, out: *mut u32) -> i32 {
    if out.is_null() {
        return HLID_E_ARG;
    }
    with(key, |m| {
        let c = m.counts(beg, end);
        // SAFETY: checked non-null; the caller promises 64 writable counts.
        unsafe { std::ptr::copy_nonoverlapping(c.as_ptr(), out, 64) };
        HLID_OK
    })
}

/// Replace SOURCE's folds with the N at FOLDS, keeping at most MAX in all.
///
/// # Safety
/// FOLDS must be readable for N folds unless N is 0.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hlid_set_folds(
    key: u64,
    source: u32,
    folds: *const Fold,
    n: usize,
    max: u64,
) -> i32 {
    if folds.is_null() && n > 0 {
        return HLID_E_ARG;
    }
    // SAFETY: checked; the caller promises N readable folds.
    let fs = if n == 0 {
        &[][..]
    } else {
        unsafe { std::slice::from_raw_parts(folds, n) }
    };
    with(key, |m| {
        m.set_folds(source as u8, fs, max as usize);
        HLID_OK
    })
}

/// The folds overlapping [BEG, END) into OUT (at most CAP), their number into *N (all of them, even past CAP).
///
/// # Safety
/// OUT must be writable for CAP folds unless CAP is 0; N must be writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hlid_folds(
    key: u64,
    beg: u64,
    end: u64,
    out: *mut Fold,
    cap: usize,
    n: *mut usize,
) -> i32 {
    if n.is_null() || (out.is_null() && cap > 0) {
        return HLID_E_ARG;
    }
    with(key, |m| {
        let mut k = 0usize;
        for f in m.folds(beg, end) {
            if k < cap {
                // SAFETY: k < CAP, which the caller promises writable.
                unsafe { *out.add(k) = *f };
            }
            k += 1;
        }
        // SAFETY: checked non-null.
        unsafe { *n = k };
        HLID_OK
    })
}

/// The fold to toggle at POS into *OUT (`Model::fold_at`); HLID_NOTHING when none.
///
/// # Safety
/// OUT must be writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hlid_fold_at(key: u64, pos: u64, out: *mut Fold) -> i32 {
    if out.is_null() {
        return HLID_E_ARG;
    }
    with(key, |m| match m.fold_at(pos) {
        // SAFETY: checked non-null; the caller promises it is writable.
        Some(f) => unsafe {
            *out = f;
            HLID_OK
        },
        None => HLID_NOTHING,
    })
}

/// KEY's hidden ranges: N (BEG, END) pairs at RANGES (2N positions).
///
/// # Safety
/// RANGES must be readable for 2N positions unless N is 0.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hlid_set_hidden(key: u64, ranges: *const u64, n: usize) -> i32 {
    if ranges.is_null() && n > 0 {
        return HLID_E_ARG;
    }
    // SAFETY: checked; the caller promises 2N readable positions.
    let r = if n == 0 {
        &[][..]
    } else {
        unsafe { std::slice::from_raw_parts(ranges, 2 * n) }
    };
    let pairs: Vec<(u64, u64)> = r.as_chunks::<2>().0.iter().map(|p| (p[0], p[1])).collect();
    with(key, |m| {
        m.set_hidden(&pairs);
        HLID_OK
    })
}

/// Note a visit to POS on KEY's trail.
#[unsafe(no_mangle)]
pub extern "C" fn hlid_trail_note(key: u64, pos: u64) -> i32 {
    with(key, |m| {
        m.trail_note(pos);
        HLID_OK
    })
}

/// KEY's trail, newest first, into OUT (at most CAP), its length into *N.
///
/// # Safety
/// OUT must be writable for CAP positions unless CAP is 0; N must be writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hlid_trail(key: u64, out: *mut u64, cap: usize, n: *mut usize) -> i32 {
    if n.is_null() || (out.is_null() && cap > 0) {
        return HLID_E_ARG;
    }
    with(key, |m| {
        let mut k = 0usize;
        for &p in m.trail() {
            if k < cap {
                // SAFETY: k < CAP, which the caller promises writable.
                unsafe { *out.add(k) = p };
            }
            k += 1;
        }
        // SAFETY: checked non-null.
        unsafe { *n = k };
        HLID_OK
    })
}

/// KEY's counters into *OUT.
///
/// # Safety
/// OUT must be writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hlid_describe(key: u64, out: *mut HlidStats) -> i32 {
    if out.is_null() {
        return HLID_E_ARG;
    }
    with(key, |m| {
        let st = state(m);
        let mut s = HlidStats {
            marks: m.marks().len() as u64,
            folds: m.fold_count() as u64,
            hidden: m.hidden_count() as u64,
            heap_bytes: m.heap_bytes() as u64,
            edits: m.edits,
            state: st,
            ..HlidStats::default()
        };
        if let Some(idx) = m.index() {
            s.lines = idx.lines();
            s.chars = idx.chars();
            s.chunks = idx.chunks() as u64;
        }
        // SAFETY: checked non-null; the caller promises it is writable.
        unsafe { *out = s };
        HLID_OK
    })
}

/// KEY's MARK sections' expression: the N bytes at RE (UTF-8, Rust regex syntax), or none for a null RE.  HLID_E_ARG
/// when it does not compile (nothing changes).  The sections are found at the next build, which the caller asks for.
///
/// # Safety
/// RE must be readable for N bytes unless null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hlid_set_sections(key: u64, re: *const u8, n: usize) -> i32 {
    let text = if re.is_null() {
        None
    } else {
        // SAFETY: the caller promises N readable bytes.
        match std::str::from_utf8(unsafe { std::slice::from_raw_parts(re, n) }) {
            Ok(t) => Some(t.to_owned()),
            Err(_) => return HLID_E_ARG,
        }
    };
    with(key, |m| match m.set_section_regex(text.as_deref()) {
        Ok(()) => HLID_OK,
        Err(_) => HLID_E_ARG,
    })
}

/// The fold ranges KEY's model finds itself: by indentation when INDENT is non-zero, and by headings with the N bytes
/// at RE (UTF-8, Rust regex syntax, its first group the heading's marks), none for a null RE.  HLID_E_ARG when RE does
/// not compile (nothing changes).  The headings are found at the next build, which the caller asks for.
///
/// # Safety
/// RE must be readable for N bytes unless null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hlid_set_derived_folds(
    key: u64,
    indent: i32,
    re: *const u8,
    n: usize,
) -> i32 {
    let text = if re.is_null() {
        None
    } else {
        // SAFETY: the caller promises N readable bytes.
        match std::str::from_utf8(unsafe { std::slice::from_raw_parts(re, n) }) {
            Ok(t) => Some(t.to_owned()),
            Err(_) => return HLID_E_ARG,
        }
    };
    with(key, |m| {
        match m.set_derived_folds(indent != 0, text.as_deref()) {
            Ok(()) => HLID_OK,
            Err(_) => HLID_E_ARG,
        }
    })
}

/// KEY's Ith MARK section whose line starts in BEG..END: its line's start into *POS, its name into *NAME (UTF-8, valid
/// until KEY's model next changes) and *LEN.  HLID_NOTHING past the last; the model's state while it is built.
///
/// # Safety
/// POS, NAME and LEN must be writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hlid_section(
    key: u64,
    beg: u64,
    end: u64,
    i: usize,
    pos: *mut u64,
    name: *mut *const u8,
    len: *mut usize,
) -> i32 {
    if pos.is_null() || name.is_null() || len.is_null() {
        return HLID_E_ARG;
    }
    with(key, |m| {
        let st = state(m);
        if st != HLID_OK {
            return st;
        }
        match m.sections(beg, end).get(i) {
            // SAFETY: checked non-null; the caller promises them writable.  The name lives in the model.
            Some(s) => unsafe {
                *pos = s.pos;
                *name = s.name.as_ptr();
                *len = s.name.len();
                HLID_OK
            },
            None => HLID_NOTHING,
        }
    })
}

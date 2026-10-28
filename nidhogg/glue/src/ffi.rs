//! The entry points of `include/nhglue.h`.
//!
//! Every entry point catches panics at the boundary: a panic that unwinds out of `extern "C"` aborts the process, and
//! the glue never aborts Emacs.

use crate::abi::{
    NHG_ABI_VERSION, NHG_E_ARG, NHG_E_DISCONNECTED, NHG_E_NOMEM, NHG_E_PANIC, NHG_E_STATE,
    NHG_E_UNSUPPORTED, NHG_OK,
};
use crate::backing::{Backings, NhgBacking, NhgFrameGeom, NhgPending, NhgRect, alpha_multiply};
use nhshell::ffi::{NhShell, nhshell_close, nhshell_open, nhshell_wake_fd};
use std::ffi::c_char;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::ptr::{self, NonNull};

/// One display: the shell connection and the glue's state for it.
pub struct NhgDisplay {
    /// From `nhshell_open`; closed when the display is dropped.
    shell: NonNull<NhShell>,
    /// Each frame's backing, layer and pending damage (`backing.rs`).
    backings: Backings,
}

impl Drop for NhgDisplay {
    fn drop(&mut self) {
        // SAFETY: the handle came from `nhshell_open` and is released only here.
        unsafe { nhshell_close(self.shell.as_ptr()) };
    }
}

/// Run `body`, turning a panic into `NHG_E_PANIC`.
fn catch(body: impl FnOnce() -> i32) -> i32 {
    catch_unwind(AssertUnwindSafe(body)).unwrap_or(NHG_E_PANIC)
}

/// Why opening failed, for the caller's message.
fn reason(rc: i32) -> String {
    match rc {
        NHG_E_PANIC => "internal error in the shell glue".into(),
        NHG_E_UNSUPPORTED => "no shell backend could connect".into(),
        NHG_E_DISCONNECTED => "the display refused the connection".into(),
        NHG_E_NOMEM => "out of memory".into(),
        NHG_E_ARG => "bad display name".into(),
        _ => format!("error {rc}"),
    }
}

/// Write `text` into `buf` as a NUL-terminated string, cut to `cap` bytes.
///
/// # Safety
/// `buf` is null or holds `cap` writable bytes.
unsafe fn put_text(buf: *mut c_char, cap: usize, text: &str) {
    if buf.is_null() || cap == 0 {
        return;
    }
    let n = text.len().min(cap - 1);
    // SAFETY: `n < cap` bytes plus the NUL fit in `buf`, and `text` does not overlap it.
    unsafe {
        ptr::copy_nonoverlapping(text.as_ptr().cast::<c_char>(), buf, n);
        buf.add(n).write(0);
    }
}

/// Write the ABI version to `out`.
///
/// # Safety
/// `out` is null or points to a writable `u32`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn nhg_abi_version(out: *mut u32) -> i32 {
    catch(|| {
        if out.is_null() {
            return NHG_E_ARG;
        }
        // SAFETY: non-null, and the caller promises it is writable.
        unsafe { out.write(NHG_ABI_VERSION) };
        NHG_OK
    })
}

/// Connect to `display` (null: the environment's).  Null on failure, with the reason in `err`.
///
/// # Safety
/// `display` is null or NUL-terminated; `err` is null or holds `cap` writable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn nhg_open(
    display: *const c_char,
    err: *mut c_char,
    cap: usize,
) -> *mut NhgDisplay {
    let opened = catch_unwind(AssertUnwindSafe(|| {
        let mut sh: *mut NhShell = ptr::null_mut();
        // SAFETY: `display` is null or NUL-terminated (the caller's promise); `sh` is a writable local.
        let rc = unsafe { nhshell_open(display, &mut sh) };
        match NonNull::new(sh) {
            Some(shell) if rc >= 0 => Ok(Box::into_raw(Box::new(NhgDisplay {
                shell,
                backings: Backings::default(),
            }))),
            Some(shell) => {
                // SAFETY: a handle the failed open left behind is released here (none is expected).
                unsafe { nhshell_close(shell.as_ptr()) };
                Err(rc)
            }
            None => Err(if rc < 0 { rc } else { NHG_E_STATE }),
        }
    }));
    let rc = match opened {
        Ok(Ok(d)) => return d,
        Ok(Err(rc)) => rc,
        Err(_) => NHG_E_PANIC,
    };
    // SAFETY: `err` is null or holds `cap` writable bytes (the caller's promise).
    unsafe { put_text(err, cap, &reason(rc)) };
    ptr::null_mut()
}

/// Close the connection and free `d`.  Null is accepted.
///
/// # Safety
/// `d` is null or a display from `nhg_open`, not used again.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn nhg_close(d: *mut NhgDisplay) -> i32 {
    catch(|| {
        if !d.is_null() {
            // SAFETY: a live display from `nhg_open`, released here.
            drop(unsafe { Box::from_raw(d) });
        }
        NHG_OK
    })
}

/// The descriptor that is readable when events are queued (>= 0), or a negative code.
///
/// # Safety
/// `d` is null or a live display.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn nhg_fd(d: *mut NhgDisplay) -> i32 {
    catch(|| {
        // SAFETY: null or live (the caller's promise).
        match unsafe { d.as_ref() } {
            None => NHG_E_ARG,
            // SAFETY: the shell handle lives as long as the display.
            Some(d) => unsafe { nhshell_wake_fd(d.shell.as_ptr()) },
        }
    })
}

/// The shell handle inside `d`, for the `nhshell_*` calls not yet ported; null if `d` is null.
///
/// # Safety
/// `d` is null or a live display.  The handle is valid until `nhg_close (d)`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn nhg_shell(d: *mut NhgDisplay) -> *mut NhShell {
    catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: null or live (the caller's promise).
        unsafe { d.as_ref() }.map_or(ptr::null_mut(), |d| d.shell.as_ptr())
    }))
    .unwrap_or(ptr::null_mut())
}

// ---- Backings, damage and scroll (`backing.rs`) --------------------------------------------------------------------
//
// FRAME is a frame's key, its Glitnir root id (`nhg_layer_ensure` gives it).  An error is the code; `Ok` the count.

/// Run `body` on `d`'s backings, turning a panic into `NHG_E_PANIC`; `NHG_E_ARG` if `d` is null.
///
/// # Safety
/// `d` is null or a live display.
unsafe fn with_backings(
    d: *mut NhgDisplay,
    body: impl FnOnce(&mut Backings) -> Result<i32, i32>,
) -> i32 {
    catch(|| {
        // SAFETY: null or live (the caller's promise); calls come from the one thread holding Emacs's lock.
        match unsafe { d.as_mut() } {
            None => NHG_E_ARG,
            Some(d) => body(&mut d.backings).unwrap_or_else(|e| e),
        }
    })
}

/// Make the frame's Glitnir layer match `g`; a frame without a key (`*frame` 0) gets one.  1 when the layer was made
/// or changed (the frame is damaged whole and its pending damage dropped), 0 when it already matched.  Neither reads
/// nor writes pixels.
///
/// # Safety
/// `d` is null or a live display; `frame` and `g` are null or valid for the call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn nhg_layer_ensure(
    d: *mut NhgDisplay,
    frame: *mut u64,
    g: *const NhgFrameGeom,
) -> i32 {
    // SAFETY: the caller's promises, passed on.
    unsafe {
        with_backings(d, |b| {
            // SAFETY: null or valid for the call (the caller's promise).
            match (frame.as_mut(), g.as_ref()) {
                (Some(frame), Some(g)) => Ok(i32::from(b.ensure(frame, g))),
                _ => Err(NHG_E_ARG),
            }
        })
    }
}

/// Make the frame's backing `g`'s size (or start it over with `NHG_GEOM_FRESH`) and describe it in `out`.  Reads and
/// writes pixels.
///
/// # Safety
/// `d` is null or a live display; `g` and `out` are null or valid for the call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn nhg_backing(
    d: *mut NhgDisplay,
    frame: u64,
    g: *const NhgFrameGeom,
    out: *mut NhgBacking,
) -> i32 {
    // SAFETY: the caller's promises, passed on.
    unsafe {
        with_backings(d, |b| {
            // SAFETY: null or valid for the call (the caller's promise).
            let (Some(g), false) = (g.as_ref(), out.is_null()) else {
                return Err(NHG_E_ARG);
            };
            // SAFETY: non-null and writable (the caller's promise).
            out.write(b.backing(frame, g)?);
            Ok(0)
        })
    }
}

/// Damage `n` rectangles of the frame (device pixels, with the native area's origin).  Returns how many were not
/// empty.  Neither reads nor writes pixels.
///
/// # Safety
/// `d` is null or a live display; `r` holds `n` readable rectangles, or is null when `n` is 0.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn nhg_damage(
    d: *mut NhgDisplay,
    frame: u64,
    r: *const NhgRect,
    n: usize,
) -> i32 {
    // SAFETY: the caller's promises, passed on.
    unsafe {
        with_backings(d, |b| {
            let rects = match (n, r.is_null()) {
                (0, _) => &[][..],
                (_, true) => return Err(NHG_E_ARG),
                // SAFETY: non-null, with `n` readable rectangles (the caller's promise).
                _ => std::slice::from_raw_parts(r, n),
            };
            b.damage(frame, rects)
        })
    }
}

/// Damage the whole frame, its border included.  Neither reads nor writes pixels.
///
/// # Safety
/// `d` is null or a live display.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn nhg_damage_all(d: *mut NhgDisplay, frame: u64) -> i32 {
    // SAFETY: the caller's promise, passed on.
    unsafe { with_backings(d, |b| b.damage_all(frame).map(|()| 0)) }
}

/// Emacs window `w_seq`'s rows `run` (device pixels, with the native area's origin: its columns, the run's first row
/// and its height) move to `to_y` in the backing, and Glitnir damages the move.  Writes pixels.
///
/// # Safety
/// `d` is null or a live display.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn nhg_scroll(
    d: *mut NhgDisplay,
    frame: u64,
    w_seq: u64,
    run: NhgRect,
    to_y: i32,
) -> i32 {
    // SAFETY: the caller's promise, passed on.
    unsafe { with_backings(d, |b| b.scroll(frame, w_seq, run, to_y).map(|()| 0)) }
}

/// Take Glitnir's committed damage for the frame into its pending list.  Returns how many rectangles it took.
/// Neither reads nor writes pixels.
///
/// # Safety
/// `d` is null or a live display.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn nhg_commit(d: *mut NhgDisplay, frame: u64) -> i32 {
    // SAFETY: the caller's promise, passed on.
    unsafe { with_backings(d, |b| b.commit(frame)) }
}

/// The frame's pending damage in `out`, valid until the next call that changes it (none for an unknown frame).
/// Neither reads nor writes pixels.
///
/// # Safety
/// `d` is null or a live display; `out` is null or writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn nhg_pending(d: *mut NhgDisplay, frame: u64, out: *mut NhgPending) -> i32 {
    // SAFETY: the caller's promises, passed on.
    unsafe {
        with_backings(d, |b| {
            if out.is_null() {
                return Err(NHG_E_ARG);
            }
            // SAFETY: non-null and writable (the caller's promise).
            out.write(b.pending(frame));
            Ok(0)
        })
    }
}

/// The frame's pending damage was presented: the list and its area start again.  Neither reads nor writes pixels.
///
/// # Safety
/// `d` is null or a live display.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn nhg_presented(d: *mut NhgDisplay, frame: u64) -> i32 {
    // SAFETY: the caller's promise, passed on.
    unsafe { with_backings(d, |b| b.presented(frame).map(|()| 0)) }
}

/// The frame's backing in `out`, borrowed until the next call on the frame.  Reads pixels.
///
/// # Safety
/// `d` is null or a live display; `out` is null or writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn nhg_snapshot(d: *mut NhgDisplay, frame: u64, out: *mut NhgBacking) -> i32 {
    // SAFETY: the caller's promises, passed on.
    unsafe {
        with_backings(d, |b| {
            if out.is_null() {
                return Err(NHG_E_ARG);
            }
            // SAFETY: non-null and writable (the caller's promise).
            out.write(b.snapshot(frame)?);
            Ok(0)
        })
    }
}

/// The frame is gone: its Glitnir scene, damage and statistics, and its backing.  Frame 0 is accepted.  Neither reads
/// nor writes pixels.
///
/// # Safety
/// `d` is null or a live display.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn nhg_backing_forget(d: *mut NhgDisplay, frame: u64) -> i32 {
    // SAFETY: the caller's promise, passed on.
    unsafe {
        with_backings(d, |b| {
            b.forget(frame);
            Ok(0)
        })
    }
}

/// Multiply every pixel of a `w` x `h` buffer (premultiplied ARGB32, rows `stride` bytes apart, 4-byte aligned) by
/// the alpha 1 - `cut` / `UINT32_MAX`.  Writes the buffer's pixels (a shell buffer, not a backing).
///
/// # Safety
/// `d` is null or a live display; `pixels` holds `h` rows of `stride` writable bytes, or is null when `w` or `h` is 0.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn nhg_alpha_multiply(
    d: *mut NhgDisplay,
    pixels: *mut u8,
    stride: i32,
    w: i32,
    h: i32,
    cut: u32,
) -> i32 {
    // SAFETY: the caller's promises, passed on.
    unsafe {
        with_backings(d, |_| {
            if w <= 0 || h <= 0 {
                return Ok(0);
            }
            let (Ok(wu), Ok(su)) = (usize::try_from(w), usize::try_from(stride)) else {
                return Err(NHG_E_ARG);
            };
            if pixels.is_null()
                || su < wu.saturating_mul(4)
                || !su.is_multiple_of(4)
                || !pixels.cast::<u32>().is_aligned()
            {
                return Err(NHG_E_ARG);
            }
            for y in 0..h as usize {
                // SAFETY: row `y` lies inside the buffer and holds `w` aligned pixels (the caller's promise, checked
                // for alignment above); rows do not overlap.
                let row = std::slice::from_raw_parts_mut(pixels.add(y * su).cast::<u32>(), wu);
                alpha_multiply(row, cut);
            }
            Ok(0)
        })
    }
}

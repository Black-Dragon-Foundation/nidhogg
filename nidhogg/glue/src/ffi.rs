//! The entry points of `include/nhglue.h`.
//!
//! Every entry point catches panics at the boundary: a panic that unwinds out of `extern "C"` aborts the process, and
//! the glue never aborts Emacs.

use crate::abi::{
    NHG_ABI_VERSION, NHG_E_ARG, NHG_E_DISCONNECTED, NHG_E_NOMEM, NHG_E_PANIC, NHG_E_STATE,
    NHG_E_UNSUPPORTED, NHG_OK,
};
use nhshell::ffi::{NhShell, nhshell_close, nhshell_open, nhshell_wake_fd};
use std::ffi::c_char;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::ptr::{self, NonNull};

/// One display: the shell connection and the glue's state for it.
pub struct NhgDisplay {
    /// From `nhshell_open`; closed when the display is dropped.
    shell: NonNull<NhShell>,
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
            Some(shell) if rc >= 0 => Ok(Box::into_raw(Box::new(NhgDisplay { shell }))),
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

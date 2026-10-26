//! The C ABI of the shell (`include/nhshell.h`): thin `extern "C"` functions over `Box<dyn ShellBackend>`.
//!
//! Every body runs in `catch_unwind` and reports a panic as `NHSHELL_E_PANIC`.  A call returns 0 or a negative
//! `NHSHELL_E_*` code.  The functions are not thread-safe: the host calls them from one thread.

use crate::api::*;
use crate::caps::NhShellCaps;
use crate::event::NhShellEvent;
use glitnir::geom::Rect;
use std::ffi::{CStr, c_char};
use std::panic::{AssertUnwindSafe, catch_unwind};

/// The handle behind `NhShell *`.
pub struct NhShell {
    backend: Box<dyn ShellBackend>,
}

/// The window descriptor of `nhshell_window_new`.
#[repr(C)]
pub struct NhShellWindowDesc {
    pub w: i32,
    pub h: i32,
    pub title: *const c_char,
    pub app_id: *const c_char,
    pub flags: u32,
}

/// Connect to `display` (`None`: the environment's).  The backends are added here as they are written.
fn open_backend(display: Option<&CStr>) -> ShellResult<Box<dyn ShellBackend>> {
    let shell = crate::wayland::WaylandShell::open(display)?;
    Ok(Box::new(shell))
}

/// # Safety
/// `p` is null or a NUL-terminated string valid for `'a`.
unsafe fn cstr<'a>(p: *const c_char) -> ShellResult<&'a str> {
    if p.is_null() {
        return Err(ShellError::Arg);
    }
    // SAFETY: non-null and NUL-terminated by the caller's promise.
    unsafe { CStr::from_ptr(p) }
        .to_str()
        .map_err(|_| ShellError::Arg)
}

/// Run `f` on the backend behind `sh` (null, or a live handle by the exported functions' contracts) and turn the
/// outcome into a return code.
fn guard<T>(
    sh: *mut NhShell,
    f: impl FnOnce(&mut dyn ShellBackend) -> ShellResult<T>,
    ok: impl FnOnce(T) -> i32,
) -> i32 {
    catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: null or a live handle, by the caller's promise.
        let Some(sh) = (unsafe { sh.as_mut() }) else {
            return NHSHELL_E_ARG;
        };
        match f(&mut *sh.backend) {
            Ok(v) => ok(v),
            Err(e) => e.code(),
        }
    }))
    .unwrap_or(NHSHELL_E_PANIC)
}

/// Write `v` to `out` unless it is null.
///
/// # Safety
/// `out` is null or writable.
unsafe fn put<T>(out: *mut T, v: T) -> ShellResult {
    if out.is_null() {
        return Err(ShellError::Arg);
    }
    // SAFETY: non-null and writable by the caller's promise.
    unsafe { out.write(v) };
    Ok(())
}

/// One window request: `fn name(sh, args...) |backend| expr`.
macro_rules! ffi {
    ($(#[$doc:meta])* fn $name:ident($($a:ident : $t:ty),*) |$b:ident| $body:expr) => {
        $(#[$doc])*
        ///
        /// # Safety
        /// `sh` is a live handle from `nhshell_open`; pointer arguments are null or valid.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn $name(sh: *mut NhShell $(, $a: $t)*) -> i32 {
            guard(sh, |$b| $body, |()| NHSHELL_OK)
        }
    };
}

/// Open a connection to `display` (NULL: the environment's).  `NHSHELL_E_UNSUPPORTED` if no backend can.
///
/// # Safety
/// `display` is null or a NUL-terminated string; `out` is null or writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn nhshell_open(display: *const c_char, out: *mut *mut NhShell) -> i32 {
    catch_unwind(AssertUnwindSafe(|| {
        let display = if display.is_null() {
            None
        } else {
            // SAFETY: non-null, NUL-terminated by the caller's promise.
            Some(unsafe { CStr::from_ptr(display) })
        };
        let r = open_backend(display).and_then(|backend| {
            let p = Box::into_raw(Box::new(NhShell { backend }));
            // SAFETY: `out` is null or writable.
            unsafe { put(out, p) }.inspect_err(|_| {
                // SAFETY: `p` was just made by `Box::into_raw`.
                drop(unsafe { Box::from_raw(p) });
            })
        });
        r.map_or_else(ShellError::code, |()| NHSHELL_OK)
    }))
    .unwrap_or(NHSHELL_E_PANIC)
}

/// Close the connection and free the handle.  NULL is accepted.
///
/// # Safety
/// `sh` is null or a handle from `nhshell_open`, not used again.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn nhshell_close(sh: *mut NhShell) -> i32 {
    catch_unwind(AssertUnwindSafe(|| {
        if !sh.is_null() {
            // SAFETY: a live handle, released here.
            drop(unsafe { Box::from_raw(sh) });
        }
        NHSHELL_OK
    }))
    .unwrap_or(NHSHELL_E_PANIC)
}

/// The descriptor that is readable when events are queued.  Returns it (>= 0), or a negative code.
///
/// # Safety
/// `sh` is a live handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn nhshell_wake_fd(sh: *mut NhShell) -> i32 {
    guard(sh, |b| Ok(b.wake_fd()), |fd| fd)
}

/// Copy the capabilities to `*out`.
///
/// # Safety
/// `sh` is a live handle; `out` is null or writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn nhshell_caps(sh: *mut NhShell, out: *mut NhShellCaps) -> i32 {
    guard(
        sh,
        // SAFETY: `out` is null or writable.
        |b| unsafe { put(out, b.caps()) },
        |()| NHSHELL_OK,
    )
}

/// Take the next event into `*out`: 1 got one, 0 the queue is empty, negative on error.
///
/// # Safety
/// `sh` is a live handle; `out` is null or writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn nhshell_next_event(sh: *mut NhShell, out: *mut NhShellEvent) -> i32 {
    guard(
        sh,
        |b| match b.next_event()? {
            // SAFETY: `out` is null or writable.
            Some(ev) => unsafe { put(out, ev) }.map(|()| 1),
            None => Ok(0),
        },
        |n| n,
    )
}

ffi!(
    /// Run the work that must happen on the host's thread.
    fn nhshell_dispatch_main() |b| b.dispatch_main()
);

/// Create a window; its id goes to `*out`.
///
/// # Safety
/// `sh` is a live handle; `desc` points to a valid descriptor with NUL-terminated strings (or NULL for none); `out`
/// is null or writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn nhshell_window_new(
    sh: *mut NhShell,
    desc: *const NhShellWindowDesc,
    out: *mut u64,
) -> i32 {
    guard(
        sh,
        |b| {
            // SAFETY: null or valid, by the contract.
            let d = unsafe { desc.as_ref() }.ok_or(ShellError::Arg)?;
            let text = |p: *const c_char| -> ShellResult<String> {
                if p.is_null() {
                    Ok(String::new())
                } else {
                    // SAFETY: non-null, NUL-terminated by the contract.
                    unsafe { cstr(p) }.map(str::to_owned)
                }
            };
            let id = b.window_new(&WindowDesc {
                w: d.w,
                h: d.h,
                title: text(d.title)?,
                app_id: text(d.app_id)?,
                flags: d.flags,
            })?;
            // SAFETY: `out` is null or writable.
            unsafe { put(out, id) }
        },
        |()| NHSHELL_OK,
    )
}

ffi!(fn nhshell_window_destroy(window: u64) |b| b.window_destroy(window));
ffi!(fn nhshell_window_set_title(window: u64, title: *const c_char) |b| {
    // SAFETY: NUL-terminated by the contract.
    b.set_title(window, unsafe { cstr(title) }?)
});
ffi!(fn nhshell_window_set_app_id(window: u64, app_id: *const c_char) |b| {
    // SAFETY: NUL-terminated by the contract.
    b.set_app_id(window, unsafe { cstr(app_id) }?)
});
ffi!(fn nhshell_window_set_min_size(window: u64, w: i32, h: i32) |b| b.set_min_size(window, w, h));
ffi!(fn nhshell_window_set_fullscreen(window: u64, on: i32) |b| b.set_fullscreen(window, on != 0));
ffi!(fn nhshell_window_set_maximized(window: u64, on: i32) |b| b.set_maximized(window, on != 0));
ffi!(fn nhshell_window_minimize(window: u64) |b| b.minimize(window));
ffi!(fn nhshell_window_move(window: u64) |b| b.move_window(window));
ffi!(fn nhshell_window_resize(window: u64, edges: u32) |b| b.resize(window, edges));
ffi!(fn nhshell_window_show_menu(window: u64, x: i32, y: i32) |b| b.show_menu(window, x, y));
ffi!(fn nhshell_window_ack_configure(window: u64, serial: u32) |b| b.ack_configure(window, serial));
ffi!(fn nhshell_window_set_cursor(window: u64, shape: u32) |b| b.set_cursor(window, shape));

/// The window's preferred scale times 120 to `*scale120`.
///
/// # Safety
/// `sh` is a live handle; `scale120` is null or writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn nhshell_window_scale(
    sh: *mut NhShell,
    window: u64,
    scale120: *mut u32,
) -> i32 {
    guard(
        sh,
        |b| {
            let s = b.window_scale(window)?;
            // SAFETY: null or writable, by the contract.
            unsafe { put(scale120, s) }
        },
        |()| NHSHELL_OK,
    )
}

/// The window's logical size to `*w` and `*h`.
///
/// # Safety
/// `sh` is a live handle; `w` and `h` are null or writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn nhshell_window_logical_size(
    sh: *mut NhShell,
    window: u64,
    w: *mut i32,
    h: *mut i32,
) -> i32 {
    guard(
        sh,
        |b| {
            let (lw, lh) = b.window_logical_size(window)?;
            // SAFETY: null or writable, by the contract.
            unsafe { put(w, lw)? };
            // SAFETY: as above.
            unsafe { put(h, lh) }
        },
        |()| NHSHELL_OK,
    )
}

/// Begin a frame of `dev_w` x `dev_h` device pixels; the buffer to draw into goes to `*out`.
///
/// # Safety
/// `sh` is a live handle; `out` is null or writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn nhshell_present_begin(
    sh: *mut NhShell,
    window: u64,
    dev_w: i32,
    dev_h: i32,
    out: *mut NhShellBuffer,
) -> i32 {
    guard(
        sh,
        |b| {
            let buf = b.present_begin(window, dev_w, dev_h)?;
            // SAFETY: null or writable, by the contract.
            unsafe { put(out, buf) }
        },
        |()| NHSHELL_OK,
    )
}

/// Show the begun buffer; `damage` points to `n` rectangles in device pixels (NULL only when `n` is 0, which damages
/// nothing and still presents).
///
/// # Safety
/// `sh` is a live handle; `damage` is null or points to `n` rectangles.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn nhshell_present_commit(
    sh: *mut NhShell,
    window: u64,
    damage: *const Rect,
    n: usize,
) -> i32 {
    guard(
        sh,
        |b| {
            let rects: &[Rect] = if n == 0 {
                &[]
            } else if damage.is_null() {
                return Err(ShellError::Arg);
            } else {
                // SAFETY: `n` rectangles by the contract.
                unsafe { std::slice::from_raw_parts(damage, n) }
            };
            b.present_commit(window, rects)
        },
        |()| NHSHELL_OK,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_without_a_compositor_is_unsupported() {
        let mut sh: *mut NhShell = std::ptr::null_mut();
        // an explicit path that does not exist (never the environment's display: tests stay off the user's session)
        let display = c"/nonexistent/nhshell-test-display";
        // SAFETY: `display` is NUL-terminated; `sh` is writable.
        assert_eq!(
            unsafe { nhshell_open(display.as_ptr(), &mut sh) },
            NHSHELL_E_UNSUPPORTED
        );
        assert!(sh.is_null());
        // SAFETY: null handles are rejected, and close accepts null.
        unsafe {
            assert_eq!(nhshell_wake_fd(sh), NHSHELL_E_ARG);
            assert_eq!(nhshell_close(sh), NHSHELL_OK);
        }
    }
}

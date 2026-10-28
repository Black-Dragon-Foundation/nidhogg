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

impl NhShell {
    /// The backend behind the handle, for the glue, which owns the handle until this C ABI retires.
    pub fn backend_mut(&mut self) -> &mut dyn ShellBackend {
        &mut *self.backend
    }
}

/// The window descriptor of `nhshell_window_new`.
#[repr(C)]
pub struct NhShellWindowDesc {
    pub w: i32,
    pub h: i32,
    pub title: *const c_char,
    pub app_id: *const c_char,
    pub flags: u32,
    /// `NHSHELL_KIND_*`; 0 for a top-level window.
    pub kind: u32,
    /// Children and popups: the parent window.
    pub parent: u64,
    /// Children: the offset in the parent (logical pixels).
    pub x: i32,
    pub y: i32,
    /// Popups: where to put it.
    pub anchor: Anchor,
    /// Top-level windows: the session name (NULL: none).
    pub session_name: *const c_char,
}

/// Connect to `display` (`None`: the environment's).  The backends are added here as they are written.
#[cfg(feature = "wayland")]
fn open_backend(display: Option<&CStr>) -> ShellResult<Box<dyn ShellBackend>> {
    let shell = crate::wayland::WaylandShell::open(display)?;
    Ok(Box::new(shell))
}

/// Without a backend (the Android check builds without `wayland`), nothing can connect.
#[cfg(not(feature = "wayland"))]
fn open_backend(_display: Option<&CStr>) -> ShellResult<Box<dyn ShellBackend>> {
    Err(ShellError::Unsupported)
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
            let id = if d.kind == NHSHELL_KIND_TOPLEVEL {
                let session_name = text(d.session_name)?;
                b.window_new_named(
                    &WindowDesc {
                        w: d.w,
                        h: d.h,
                        title: text(d.title)?,
                        app_id: text(d.app_id)?,
                        flags: d.flags,
                    },
                    Some(session_name.as_str()),
                )?
            } else {
                b.child_new(&ChildDesc {
                    kind: d.kind,
                    parent: d.parent,
                    x: d.x,
                    y: d.y,
                    w: d.w,
                    h: d.h,
                    anchor: d.anchor,
                    flags: d.flags,
                })?
            };
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
ffi!(fn nhshell_window_set_decorated(window: u64, on: i32) |b| b.set_decorated(window, on != 0));
ffi!(fn nhshell_window_set_alpha(window: u64, factor: u32) |b| b.set_alpha(window, factor));
ffi!(fn nhshell_window_move(window: u64) |b| b.move_window(window));
ffi!(fn nhshell_window_resize(window: u64, edges: u32) |b| b.resize(window, edges));
ffi!(fn nhshell_window_show_menu(window: u64, x: i32, y: i32) |b| b.show_menu(window, x, y));
ffi!(fn nhshell_window_set_geometry(window: u64, x: i32, y: i32, w: i32, h: i32) |b| b.set_geometry(window, x, y, w, h));
ffi!(fn nhshell_window_ack_configure(window: u64, serial: u32, flags: u32) |b| b.ack_configure(window, serial, flags));
ffi!(fn nhshell_present_bare(window: u64) |b| b.present_bare(window));
ffi!(fn nhshell_window_dirty(window: u64) |b| b.window_dirty(window));
ffi!(
    /// What the window's pacer lets the host do now: `NHSHELL_READY_*` bits (0: nothing) to `*ready`.
    fn nhshell_window_ready(window: u64, ready: *mut i32) |b| {
        let r = b.window_ready(window)?;
        // SAFETY: null or writable, by the contract.
        unsafe { put(ready, r as i32) }
    }
);
ffi!(fn nhshell_window_set_cursor(window: u64, shape: u32) |b| b.set_cursor(window, shape));
ffi!(fn nhshell_set_cursor_theme(name: *const c_char, size: u32) |b| {
    // SAFETY: NUL-terminated by the contract.
    b.set_cursor_theme(unsafe { cstr(name) }?, size)
});
ffi!(fn nhshell_window_activate(window: u64, token: *const c_char) |b| {
    // SAFETY: null or NUL-terminated by the contract.
    let token = if token.is_null() { None } else { Some(unsafe { cstr(token) }?) };
    b.window_activate(window, token)
});

ffi!(fn nhshell_bell(window: u64) |b| b.bell(window));
ffi!(fn nhshell_window_inhibit_idle(window: u64, on: i32) |b| b.inhibit_idle(window, on != 0));
ffi!(fn nhshell_window_session_remove(window: u64) |b| b.window_session_remove(window));
ffi!(fn nhshell_session_open(id: *const c_char, reason: u32) |b| {
    // SAFETY: null or NUL-terminated by the contract.
    let id = if id.is_null() { None } else { Some(unsafe { cstr(id) }?) };
    b.session_open(id, reason)
});

/// Copy `text` into `buf` (`cap` bytes, not NUL-terminated) and put its length in `*len`; `TooSmall` (with `*len` set)
/// when it does not fit.
///
/// # Safety
/// `buf` is writable for `cap` bytes (null only with `cap` 0); `len` is null or writable.
unsafe fn put_text(buf: *mut c_char, cap: usize, len: *mut usize, text: &str) -> ShellResult {
    // SAFETY: null or writable, by the contract.
    unsafe { put(len, text.len()) }?;
    if text.len() > cap || (!text.is_empty() && buf.is_null()) {
        return Err(ShellError::TooSmall);
    }
    // SAFETY: `cap >= text.len()` bytes are writable at `buf`.
    unsafe { std::ptr::copy_nonoverlapping(text.as_ptr(), buf.cast::<u8>(), text.len()) };
    Ok(())
}

/// The window's xdg-foreign handle (no `wayland:` prefix) into `buf`; waits at most `timeout_ms` for it
/// (`NHSHELL_E_TIMEOUT`).  `NHSHELL_E_TOO_SMALL` leaves `*len` set and the handle cached.
///
/// # Safety
/// `sh` is a live handle; `buf` is writable for `cap` bytes; `len` is writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn nhshell_window_export(
    sh: *mut NhShell,
    window: u64,
    timeout_ms: u32,
    buf: *mut c_char,
    cap: usize,
    len: *mut usize,
) -> i32 {
    guard(
        sh,
        |b| {
            let handle = b.window_export(window, timeout_ms)?;
            // SAFETY: by the contract.
            unsafe { put_text(buf, cap, len, &handle) }
        },
        |()| NHSHELL_OK,
    )
}

/// A fresh activation token for `window` into `buf`, with the same wait and size rules as `nhshell_window_export`.
///
/// # Safety
/// `sh` is a live handle; `buf` is writable for `cap` bytes; `len` is writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn nhshell_activation_token(
    sh: *mut NhShell,
    window: u64,
    timeout_ms: u32,
    buf: *mut c_char,
    cap: usize,
    len: *mut usize,
) -> i32 {
    guard(
        sh,
        |b| {
            let token = b.activation_token(window, timeout_ms)?;
            // SAFETY: by the contract.
            unsafe { put_text(buf, cap, len, &token) }
        },
        |()| NHSHELL_OK,
    )
}

/// The open session's id into `buf`.  `NHSHELL_E_STATE` before the compositor has answered.
///
/// # Safety
/// `sh` is a live handle; `buf` is writable for `cap` bytes; `len` is writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn nhshell_session_id(
    sh: *mut NhShell,
    buf: *mut c_char,
    cap: usize,
    len: *mut usize,
) -> i32 {
    guard(
        sh,
        |b| {
            let id = b.session_id()?;
            // SAFETY: by the contract.
            unsafe { put_text(buf, cap, len, &id) }
        },
        |()| NHSHELL_OK,
    )
}

/// Move a child (with its parent's next commit) or a popup (its anchor rectangle).  Returns 1 when the whole window
/// must be redrawn (a popup that was made again), 0 otherwise, or a negative `NHSHELL_E_*` code.
///
/// # Safety
/// `sh` is a live handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn nhshell_window_set_position(
    sh: *mut NhShell,
    window: u64,
    x: i32,
    y: i32,
) -> i32 {
    guard(sh, |b| b.set_position(window, x, y), i32::from)
}
ffi!(fn nhshell_window_stack(window: u64, sibling: u64, above: i32) |b| b.stack(window, sibling, above != 0));
ffi!(fn nhshell_window_set_mapped(window: u64, on: i32) |b| b.set_mapped(window, on != 0));

/// Whether popup `window` holds a grab: 1 or 0, or a negative `NHSHELL_E_*` code.
///
/// # Safety
/// `sh` is a live handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn nhshell_window_grabbed(sh: *mut NhShell, window: u64) -> i32 {
    guard(sh, |b| b.window_grabbed(window), i32::from)
}

/// A popup's new size (logical pixels).  Returns 1 when the whole window must be redrawn (a popup that was made
/// again), 0 otherwise, or a negative `NHSHELL_E_*` code.
///
/// # Safety
/// `sh` is a live handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn nhshell_window_set_size(
    sh: *mut NhShell,
    window: u64,
    w: i32,
    h: i32,
) -> i32 {
    guard(sh, |b| b.set_size(window, w, h), i32::from)
}

/// Set the part of the window that takes input: `n` rectangles in logical pixels (`n` 0: the whole surface).
///
/// # Safety
/// `sh` is a live handle; `rects` is null (only with `n` 0) or points to `n` rectangles.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn nhshell_window_set_input_region(
    sh: *mut NhShell,
    window: u64,
    rects: *const Rect,
    n: usize,
) -> i32 {
    guard(
        sh,
        |b| {
            let rects: &[Rect] = if n == 0 {
                &[]
            } else if rects.is_null() {
                return Err(ShellError::Arg);
            } else {
                // SAFETY: `n` rectangles by the contract.
                unsafe { std::slice::from_raw_parts(rects, n) }
            };
            b.set_input_region(window, rects)
        },
        |()| NHSHELL_OK,
    )
}

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

ffi!(
    /// Whether the compositor decorates the window: 1 or 0 to `*ssd`.
    fn nhshell_window_ssd(window: u64, ssd: *mut i32) |b| {
        let on = b.window_ssd(window)?;
        // SAFETY: null or writable, by the contract.
        unsafe { put(ssd, i32::from(on)) }
    }
);

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

/// One surface of `nhshell_publish` (`SurfaceFrame` with a raw pointer).
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct NhShellSurfaceFrame {
    pub window: u64,
    pub glitnir_root: u64,
    pub pixels: *const u8,
    pub w: i32,
    pub h: i32,
    pub stride: i32,
    pub src_x: i32,
    pub src_y: i32,
    pub flags: u32,
    pub alpha_cut: u32,
}

/// Publish the tree of `root`: `n` surfaces at `s`.  1 when a packet was queued, 0 when there was nothing.
///
/// # Safety
/// `sh` is a live handle; `s` is null only with `n` 0, else it points to `n` frames, each with `h` rows of `stride`
/// readable bytes at `pixels` (the last row `4 * w`), read during the call only.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn nhshell_publish(
    sh: *mut NhShell,
    root: u64,
    s: *const NhShellSurfaceFrame,
    n: usize,
    t_key_ns: u64,
) -> i32 {
    guard(
        sh,
        |b| {
            let raw: &[NhShellSurfaceFrame] = if n == 0 {
                &[]
            } else if s.is_null() {
                return Err(ShellError::Arg);
            } else {
                // SAFETY: `n` frames by the contract.
                unsafe { std::slice::from_raw_parts(s, n) }
            };
            let mut frames = Vec::with_capacity(n);
            for f in raw {
                if f.pixels.is_null() || f.w <= 0 || f.h <= 0 || f.stride < 4 * f.w {
                    return Err(ShellError::Arg);
                }
                let len = f.stride as usize * (f.h as usize - 1) + 4 * f.w as usize;
                frames.push(SurfaceFrame {
                    window: f.window,
                    glitnir_root: f.glitnir_root,
                    // SAFETY: `len` readable bytes by the contract, for the call.
                    pixels: unsafe { std::slice::from_raw_parts(f.pixels, len) },
                    w: f.w,
                    h: f.h,
                    stride: f.stride,
                    src_x: f.src_x,
                    src_y: f.src_y,
                    flags: f.flags,
                    alpha_cut: f.alpha_cut,
                });
            }
            b.publish(root, &frames, t_key_ns)
        },
        i32::from,
    )
}

/// Inline mode: present `root`'s tree.  1 when the root committed a buffer, 0 otherwise; `NHSHELL_E_STATE` in thread
/// mode.
///
/// # Safety
/// `sh` is a live handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn nhshell_present_tree(sh: *mut NhShell, root: u64) -> i32 {
    guard(sh, |b| b.present_tree(root), i32::from)
}

ffi!(fn nhshell_set_presenter_thread(on: i32) |b| b.set_presenter_thread(on != 0));
ffi!(
    /// The presenter's counters to `*out`.
    fn nhshell_presenter_stats(out: *mut NhShellPresenterStats) |b| {
        let st = b.presenter_stats()?;
        // SAFETY: null or writable, by the contract.
        unsafe { put(out, st) }
    }
);

/// Own selection `kind` (`NHSHELL_SEL_*`) with `n` MIME types: `mimes[i]` with the `lens[i]` bytes at `data[i]`,
/// copied here.  `NHSHELL_E_STATE` without keyboard focus.
///
/// # Safety
/// `sh` is a live handle; `mimes`, `data` and `lens` point to `n` entries each (any may be NULL when `n` is 0); each
/// `mimes[i]` is NUL-terminated and each `data[i]` points to `lens[i]` bytes (NULL allowed when that is 0).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn nhshell_selection_own(
    sh: *mut NhShell,
    kind: u32,
    mimes: *const *const c_char,
    data: *const *const u8,
    lens: *const usize,
    n: usize,
) -> i32 {
    guard(
        sh,
        |b| {
            if n > 0 && (mimes.is_null() || data.is_null() || lens.is_null()) {
                return Err(ShellError::Arg);
            }
            let mut items = Vec::with_capacity(n);
            for i in 0..n {
                // SAFETY: `n` entries each, by the contract.
                let (m, d, len) = unsafe { (*mimes.add(i), *data.add(i), *lens.add(i)) };
                // SAFETY: NUL-terminated by the contract.
                let mime = unsafe { cstr(m) }?;
                let bytes: &[u8] = match (len, d.is_null()) {
                    (0, _) => &[],
                    (_, true) => return Err(ShellError::Arg),
                    // SAFETY: `len` bytes by the contract.
                    (_, false) => unsafe { std::slice::from_raw_parts(d, len) },
                };
                items.push((mime, bytes));
            }
            b.selection_own(kind, &items)
        },
        |()| NHSHELL_OK,
    )
}

ffi!(fn nhshell_selection_disown(kind: u32) |b| b.selection_disown(kind));

/// 1 while our source is the current selection `kind`, else 0.
///
/// # Safety
/// `sh` is a live handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn nhshell_selection_owned(sh: *mut NhShell, kind: u32) -> i32 {
    guard(sh, |b| b.selection_owned(kind), i32::from)
}

/// The current MIME types of selection `kind`, each followed by a NUL, into `buf` when `cap` holds them all; `*len`
/// gets the bytes needed.  Returns the number of types.
///
/// # Safety
/// `sh` is a live handle; `buf` is null or holds `cap` bytes; `len` is null or writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn nhshell_selection_types(
    sh: *mut NhShell,
    kind: u32,
    buf: *mut c_char,
    cap: usize,
    len: *mut usize,
) -> i32 {
    guard(
        sh,
        |b| {
            let types = b.selection_types(kind)?;
            let mut all = Vec::new();
            for t in &types {
                all.extend_from_slice(t.as_bytes());
                all.push(0u8);
            }
            // SAFETY: `len` is null or writable.
            unsafe { put(len, all.len()) }?;
            if !all.is_empty() && all.len() <= cap {
                if buf.is_null() {
                    return Err(ShellError::Arg);
                }
                // SAFETY: `buf` holds `cap` >= `all.len()` bytes.
                unsafe { std::ptr::copy_nonoverlapping(all.as_ptr(), buf.cast(), all.len()) };
            }
            Ok(types.len() as i32)
        },
        |n| n,
    )
}

/// Selection `kind` as `mime` in a new buffer (`*out`, `*len`; free it with `nhshell_free_bytes`): our own bytes at
/// once, another client's read until it finishes or `timeout_ms` passes (`NHSHELL_E_TIMEOUT`).
///
/// # Safety
/// `sh` is a live handle; `mime` is NUL-terminated; `out` and `len` are null or writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn nhshell_selection_read(
    sh: *mut NhShell,
    kind: u32,
    mime: *const c_char,
    timeout_ms: u32,
    out: *mut *mut u8,
    len: *mut usize,
) -> i32 {
    guard(
        sh,
        |b| {
            if out.is_null() || len.is_null() {
                return Err(ShellError::Arg);
            }
            // SAFETY: NUL-terminated by the contract.
            let bytes = b.selection_read(kind, unsafe { cstr(mime) }?, timeout_ms)?;
            // SAFETY: a plain allocation of at least one byte.
            let p = unsafe { libc::malloc(bytes.len().max(1)) }.cast::<u8>();
            if p.is_null() {
                return Err(ShellError::NoMem);
            }
            // SAFETY: `p` holds `bytes.len()` bytes; `out` and `len` are writable (checked above).
            unsafe {
                std::ptr::copy_nonoverlapping(bytes.as_ptr(), p, bytes.len());
                out.write(p);
                len.write(bytes.len());
            }
            Ok(())
        },
        |()| NHSHELL_OK,
    )
}

/// Free a buffer from `nhshell_selection_read`.  NULL is accepted.
///
/// # Safety
/// `p` is null or such a buffer, not used again.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn nhshell_free_bytes(p: *mut u8) {
    // SAFETY: null or from `libc::malloc`, by the contract.
    unsafe { libc::free(p.cast()) };
}

ffi!(
    /// Text input for `window` on (`on` non-zero) or off.  `NHSHELL_E_UNSUPPORTED` without text-input-v3.
    fn nhshell_text_input_enable(window: u64, on: i32) |b| b.text_input_enable(window, on != 0)
);
ffi!(
    /// The cursor rectangle in the window (logical pixels); sent to the input method only when it changed.
    fn nhshell_text_input_cursor(window: u64, x: i32, y: i32, w: i32, h: i32)
        |b| b.text_input_cursor(window, x, y, w, h)
);

/// The text around the cursor: `len` bytes of UTF-8 at `utf8`, `cursor` and `anchor` byte offsets in it.  At most 4000
/// bytes go to the input method, cut around the cursor on a character boundary.
///
/// # Safety
/// `sh` is a live handle; `utf8` points to `len` bytes (or is null with `len` 0).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn nhshell_text_input_surrounding(
    sh: *mut NhShell,
    window: u64,
    utf8: *const c_char,
    len: usize,
    cursor: i32,
    anchor: i32,
) -> i32 {
    guard(
        sh,
        |b| {
            let bytes: &[u8] = if len == 0 {
                &[]
            } else if utf8.is_null() {
                return Err(ShellError::Arg);
            } else {
                // SAFETY: `len` bytes by the contract.
                unsafe { std::slice::from_raw_parts(utf8.cast::<u8>(), len) }
            };
            let text = std::str::from_utf8(bytes).map_err(|_| ShellError::Arg)?;
            let (cursor, anchor) = (
                usize::try_from(cursor).map_err(|_| ShellError::Arg)?,
                usize::try_from(anchor).map_err(|_| ShellError::Arg)?,
            );
            b.text_input_surrounding(window, text, cursor, anchor)
        },
        |()| NHSHELL_OK,
    )
}

/// Copy the text of record `id` into `buf` (`cap` bytes, not NUL-terminated) and put its length in `*len`.  The id
/// is single-use: a success forgets it.  `NHSHELL_E_TOO_SMALL` (with `*len` set, the text kept) if `cap` is short;
/// `NHSHELL_E_STATE` for an unknown id.
///
/// # Safety
/// `sh` is a live handle; `buf` is writable for `cap` bytes (null only with `cap` 0); `len` is writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn nhshell_take_text(
    sh: *mut NhShell,
    id: u32,
    buf: *mut c_char,
    cap: usize,
    len: *mut usize,
) -> i32 {
    guard(
        sh,
        |b| {
            let n = b.text_len(id)?;
            // SAFETY: null or writable, by the contract.
            unsafe { put(len, n) }?;
            if n > cap || (n > 0 && buf.is_null()) {
                return Err(ShellError::TooSmall);
            }
            let text = b.take_text(id)?;
            if n > 0 {
                // SAFETY: `cap >= n` bytes are writable at `buf`.
                unsafe { std::ptr::copy_nonoverlapping(text.as_ptr(), buf.cast::<u8>(), n) };
            }
            Ok(())
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

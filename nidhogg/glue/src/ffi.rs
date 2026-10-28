//! The entry points of `include/nhglue.h`.
//!
//! Every one is written with `nhg_export!`, so each runs in the guard (`guard.rs`): stack headroom, `try_lock` on the
//! display, `catch_unwind`.  A panic poisons its display and returns `NHG_E_PANIC`; the glue never aborts Emacs.

use crate::abi::{
    NHG_ABI_VERSION, NHG_E_ARG, NHG_E_DISCONNECTED, NHG_E_NOMEM, NHG_E_PANIC, NHG_E_STATE,
    NHG_E_UNSUPPORTED, NHG_OK,
};
use crate::action::{NhgActionList, NhgNextIn};
use crate::backing::{NhgBacking, NhgFrameGeom, NhgRect};
pub use crate::display::NhgDisplay;
use crate::guard::GlueError;
use crate::present::{self, NhgPresenterStats, NhgSurface};
use crate::{configure, drain};
use crate::{tip, window};
use nhshell::ffi::{NhShell, nhshell_close, nhshell_open, nhshell_wake_fd};
use std::ffi::c_char;
use std::ptr::{self, NonNull};

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

/// Connect to `display` and wrap the shell; `Err` holds the code.
///
/// # Safety
/// `display` is null or NUL-terminated.
unsafe fn open(display: *const c_char) -> Result<*mut NhgDisplay, i32> {
    let mut sh: *mut NhShell = ptr::null_mut();
    // SAFETY: `display` is null or NUL-terminated (the caller's promise); `sh` is a writable local.
    let rc = unsafe { nhshell_open(display, &mut sh) };
    match NonNull::new(sh) {
        Some(shell) if rc >= 0 => {
            // SAFETY: the open succeeded, and the handle goes nowhere else.
            let d = unsafe { NhgDisplay::new(shell) };
            Ok(Box::into_raw(Box::new(d)))
        }
        Some(shell) => {
            // SAFETY: a handle the failed open left behind is released here (none is expected).
            unsafe { nhshell_close(shell.as_ptr()) };
            Err(rc)
        }
        None => Err(if rc < 0 { rc } else { NHG_E_STATE }),
    }
}

nhg_export! {
    /// Write the ABI version to `out`.
    ///
    /// # Safety
    /// `out` is null or points to a writable `u32`.
    unlocked fn nhg_abi_version(out: *mut u32) -> i32 {
        if out.is_null() {
            return Err(GlueError::Arg);
        }
        // SAFETY: non-null, and the caller promises it is writable.
        unsafe { out.write(NHG_ABI_VERSION) };
        Ok(NHG_OK)
    }
}

nhg_export! {
    /// Connect to `display` (null: the environment's).  Null on failure, with the reason in `err`.
    ///
    /// # Safety
    /// `display` is null or NUL-terminated; `err` is null or holds `cap` writable bytes.
    unlocked fn nhg_open(display: *const c_char, err: *mut c_char, cap: usize) -> *mut NhgDisplay {
        // SAFETY: `display` is null or NUL-terminated (the caller's promise).
        let opened = unsafe { open(display) };
        opened.or_else(|rc| {
            // SAFETY: `err` is null or holds `cap` writable bytes (the caller's promise).
            unsafe { put_text(err, cap, &reason(rc)) };
            Ok(ptr::null_mut())
        })
    }
}

nhg_export! {
    /// Close the connection and free `d`.  Null is accepted.  On a poisoned display (or one whose lock is held) it
    /// touches nothing behind the lock: it shuts the connection down, waits at most 100 ms for each of the shell's
    /// threads and leaks the rest (design §5).
    ///
    /// # Safety
    /// `d` is null or a display from `nhg_open`, not used again.
    unlocked fn nhg_close(d: *mut NhgDisplay) -> i32 {
        if d.is_null() {
            return Ok(NHG_OK);
        }
        // SAFETY: a live display from `nhg_open`, released here.
        let d = unsafe { Box::from_raw(d) };
        let healthy = !d.is_poisoned() && d.state.try_lock().is_ok();
        if healthy {
            drop(d);
        } else {
            d.abandon();
        }
        Ok(NHG_OK)
    }
}

nhg_export! {
    /// The descriptor that is readable when events are queued (>= 0), or a negative code.
    ///
    /// # Safety
    /// `d` is null or a live display.
    fn nhg_fd(d: *mut NhgDisplay) -> i32 |state| {
        // SAFETY: the shell handle lives as long as the display.
        Ok(unsafe { nhshell_wake_fd(state.shell()) })
    }
}

nhg_export! {
    /// The shell handle inside `d`, for the `nhshell_*` calls not yet ported; null if `d` is null or poisoned.
    ///
    /// # Safety
    /// `d` is null or a live display.  The handle is valid until `nhg_close (d)`.
    fn nhg_shell(d: *mut NhgDisplay) -> *mut NhShell |state| {
        Ok(state.shell())
    }
}

nhg_export! {
    /// Register shell window `window` as a frame (`NHG_FRAME_*` in `flags`).  `NHG_E_STATE` if it is registered.
    ///
    /// # Safety
    /// `d` is null or a live display.
    fn nhg_frame_register(d: *mut NhgDisplay, window: u64, flags: u32) -> i32 |state| {
        state.register(window, flags)?;
        Ok(NHG_OK)
    }
}

nhg_export! {
    /// Forget the frame of `window`; calls naming it are refused from now on.  `NHG_E_STATE` if it is not registered.
    ///
    /// # Safety
    /// `d` is null or a live display.
    fn nhg_frame_forget(d: *mut NhgDisplay, window: u64) -> i32 |state| {
        state.forget(window)?;
        Ok(NHG_OK)
    }
}

nhg_export! {
    /// Push: `window`'s frame has the native size `native_w` x `native_h` (device pixels) as Emacs has it.  C calls it
    /// when it makes the window, when it applies a `RESIZE` that changes the frame's size, and when it resizes the
    /// frame itself; a configure being applied then gets a content ack.  `NHG_E_STATE` for an unknown window,
    /// `NHG_E_ARG` for an empty size.  Neither reads nor writes pixels.
    ///
    /// # Safety
    /// `d` is null or a live display.
    fn nhg_frame_resized(d: *mut NhgDisplay, window: u64, native_w: i32, native_h: i32) -> i32 |state| {
        configure::resized(state, window, native_w, native_h)?;
        Ok(NHG_OK)
    }
}

nhg_export! {
    /// Push: the scale (times 120) C gave `window` when it made it.  Neither reads nor writes pixels.
    ///
    /// # Safety
    /// `d` is null or a live display.
    fn nhg_frame_scale(d: *mut NhgDisplay, window: u64, scale120: u32) -> i32 |state| {
        configure::set_scale(state, window, scale120)?;
        Ok(NHG_OK)
    }
}

nhg_export! {
    /// Set whether `window`'s frame draws its own title bar (`on` non-zero).  C no longer calls it: the title bar's
    /// layout (`nhg_csd_layout`) sets the fact itself; the tests use it for a window they lay out no title bar for.
    /// Neither reads nor writes pixels.
    ///
    /// # Safety
    /// `d` is null or a live display.
    fn nhg_frame_csd(d: *mut NhgDisplay, window: u64, on: i32) -> i32 |state| {
        configure::set_csd(state, window, on != 0)?;
        Ok(NHG_OK)
    }
}

// ---- The event step (`drain.rs`, `action.rs`) ----------------------------------------------------------------------

nhg_export! {
    /// Take exactly one shell event: 1 when one was taken, with its zero or more actions in `*out`; 0 when none is
    /// queued (`*out` empty).  The actions and their texts stay valid until the next `nhg_next` on `d`; no other call
    /// frees them.  The acks of the configures C applied since the last call are recorded first.  Neither reads nor
    /// writes pixels.
    ///
    /// # Safety
    /// `d` is null or a live display; `input` is null (no flags) or readable; `out` is writable.
    fn nhg_next(d: *mut NhgDisplay, input: *const NhgNextIn, out: *mut NhgActionList) -> i32 |state| {
        if out.is_null() {
            return Err(GlueError::Arg);
        }
        // SAFETY: null or readable (the caller's promise).
        let input = unsafe { input.as_ref() }.copied().unwrap_or_default();
        let taken = drain::next(state, input);
        // SAFETY: non-null and writable (the caller's promise); an error leaves the list empty.
        unsafe { out.write(state.actions().as_list()) };
        taken
    }
}

nhg_export! {
    /// The first panic's report (message, location, backtrace) on `d`: up to `cap - 1` bytes and a NUL go to `buf`,
    /// its full length in bytes to `*len` (0: no panic).  Works on a poisoned display.
    ///
    /// # Safety
    /// `d` is null or a live display; `buf` is null or holds `cap` writable bytes; `len` is null or writable.
    unlocked fn nhg_panic_message(d: *mut NhgDisplay, buf: *mut c_char, cap: usize, len: *mut usize) -> i32 {
        // SAFETY: null or live (the caller's promise); only the report, outside the lock, is read.
        let d = unsafe { d.as_ref() }.ok_or(GlueError::Arg)?;
        let report = d.panic_report().unwrap_or("");
        // SAFETY: `buf` is null or holds `cap` writable bytes.
        unsafe { put_text(buf, cap, report) };
        if !len.is_null() {
            // SAFETY: non-null and writable (the caller's promise).
            unsafe { len.write(report.len()) };
        }
        Ok(NHG_OK)
    }
}

nhg_export! {
    /// Checking builds: compare the facts C pushed for `window` with Emacs's values `f`; a mismatch fails a debug
    /// assertion, which poisons the display.  Without debug assertions it only checks its arguments.
    ///
    /// # Safety
    /// `d` is null or a live display; `f` is null or points to a readable `NhgFrameFacts`.
    #[cfg(feature = "test-hooks")]
    fn nhg_debug_check_frame(d: *mut NhgDisplay, window: u64, f: *const crate::frame::NhgFrameFacts) -> i32 |state| {
        // SAFETY: null or readable (the caller's promise).
        let emacs = unsafe { f.as_ref() }.ok_or(GlueError::Arg)?;
        let pushed = state.frame(window)?.facts;
        // the facts C pushes so far: the native size it asked for, whether it draws its own title bar, whether it is
        // visible or iconified, and its wheel units (the rest join as the tasks that read them push them)
        let seen = |f: &crate::frame::NhgFrameFacts| {
            let flags = f.flags & crate::window::NHG_WINDOW_FACTS;
            (f.native_w, f.native_h, flags, f.col_w, f.line_h)
        };
        debug_assert_eq!(seen(&pushed), seen(emacs), "the facts pushed for window {window} drifted from Emacs's");
        // a child frame's parent (`nhg_child_set`)
        if state.frame(window)?.flags & crate::frame::NHG_FRAME_CHILD != 0 {
            debug_assert_eq!(pushed.parent, emacs.parent, "the parent pushed for window {window} drifted from Emacs's");
        }
        Ok(NHG_OK)
    }
}

nhg_export! {
    /// Test hook: panic inside the glue, which poisons `d`.
    ///
    /// # Safety
    /// `d` is null or a live display.
    #[cfg(feature = "test-hooks")]
    fn nhg_test_panic(d: *mut NhgDisplay) -> i32 |_state| {
        panic!("nhg_test_panic: a test panic inside the glue")
    }
}

// ---- Backings, damage and scroll (`backing.rs`) --------------------------------------------------------------------
//
// FRAME is a frame's key, its Glitnir root id (`nhg_layer_ensure` gives it).  An error is the code; `Ok` the count.

nhg_export! {
    /// Make the frame's Glitnir layer match `g`; a frame without a key (`*frame` 0) gets one.  1 when the layer was
    /// made or changed (the frame is damaged whole and its pending damage dropped), 0 when it already matched.  A
    /// child frame's layer (`g.window`) sits inside its shadow margin and takes its chrome (`child.rs`).  Neither
    /// reads nor writes pixels.
    ///
    /// # Safety
    /// `d` is null or a live display; `frame` and `g` are null or valid for the call.
    fn nhg_layer_ensure(d: *mut NhgDisplay, frame: *mut u64, g: *const NhgFrameGeom) -> i32 |state| {
        // SAFETY: null or valid for the call (the caller's promise).
        match unsafe { (frame.as_mut(), g.as_ref()) } {
            (Some(frame), Some(g)) => {
                let mut g = *g;
                let child = crate::child::layer(state, &mut g);
                let changed = state.backings.ensure(frame, &g);
                if let (true, Some(window)) = (changed, child) {
                    crate::child::apply_chrome(state, window, *frame, g.scale120);
                }
                Ok(i32::from(changed))
            }
            _ => Err(GlueError::Arg),
        }
    }
}

nhg_export! {
    /// Make the frame's backing `g`'s size (or start it over with `NHG_GEOM_FRESH`) and describe it in `out`.  Reads
    /// and writes pixels.
    ///
    /// # Safety
    /// `d` is null or a live display; `g` and `out` are null or valid for the call.
    fn nhg_backing(d: *mut NhgDisplay, frame: u64, g: *const NhgFrameGeom, out: *mut NhgBacking) -> i32 |state| {
        // SAFETY: null or valid for the call (the caller's promise).
        let (Some(g), false) = (unsafe { g.as_ref() }, out.is_null()) else {
            return Err(GlueError::Arg);
        };
        let b = state.backings.backing(frame, g)?;
        // SAFETY: non-null and writable (the caller's promise).
        unsafe { out.write(b) };
        Ok(0)
    }
}

nhg_export! {
    /// Damage `n` rectangles of the frame (device pixels, with the native area's origin).  Returns how many were not
    /// empty.  Neither reads nor writes pixels.
    ///
    /// # Safety
    /// `d` is null or a live display; `r` holds `n` readable rectangles, or is null when `n` is 0.
    fn nhg_damage(d: *mut NhgDisplay, frame: u64, r: *const NhgRect, n: usize) -> i32 |state| {
        let rects = match (n, r.is_null()) {
            (0, _) => &[][..],
            (_, true) => return Err(GlueError::Arg),
            // SAFETY: non-null, with `n` readable rectangles (the caller's promise).
            _ => unsafe { std::slice::from_raw_parts(r, n) },
        };
        Ok(state.backings.damage(frame, rects)?)
    }
}

nhg_export! {
    /// Damage the whole frame, its border included.  Neither reads nor writes pixels.
    ///
    /// # Safety
    /// `d` is null or a live display.
    fn nhg_damage_all(d: *mut NhgDisplay, frame: u64) -> i32 |state| {
        state.backings.damage_all(frame)?;
        Ok(0)
    }
}

nhg_export! {
    /// Emacs window `w_seq`'s rows `run` (device pixels, with the native area's origin: its columns, the run's first
    /// row and its height) move to `to_y` in the backing, and Glitnir damages the move.  Writes pixels.
    ///
    /// # Safety
    /// `d` is null or a live display.
    fn nhg_scroll(d: *mut NhgDisplay, frame: u64, w_seq: u64, run: NhgRect, to_y: i32) -> i32 |state| {
        state.backings.scroll(frame, w_seq, run, to_y)?;
        Ok(0)
    }
}

nhg_export! {
    /// The frame's backing in `out`, borrowed until the next call on the frame.  Reads pixels.
    ///
    /// # Safety
    /// `d` is null or a live display; `out` is null or writable.
    fn nhg_snapshot(d: *mut NhgDisplay, frame: u64, out: *mut NhgBacking) -> i32 |state| {
        if out.is_null() {
            return Err(GlueError::Arg);
        }
        let b = state.backings.snapshot(frame)?;
        // SAFETY: non-null and writable (the caller's promise).
        unsafe { out.write(b) };
        Ok(0)
    }
}

nhg_export! {
    /// The frame is gone: its Glitnir scene, damage and statistics, and its backing.  Frame 0 is accepted.  Neither
    /// reads nor writes pixels.
    ///
    /// # Safety
    /// `d` is null or a live display.
    fn nhg_backing_forget(d: *mut NhgDisplay, frame: u64) -> i32 |state| {
        state.backings.forget(frame);
        Ok(0)
    }
}

// ---- Presenting (`present.rs`) -------------------------------------------------------------------------------------
//
// ROOT is a tree's root window (a shell window id); the surface list names frames by their keys.

nhg_export! {
    /// Publish and present the tree of root window `root`.  `s` holds its `n` surfaces bottom to top, the root first
    /// and each parent before its children (null only when `n` is 0, which `NHG_PRESENT_READY` alone allows).  With
    /// `NHG_PRESENT_BACKSTOP` (a consistency point) the tree is published when its root may show a buffer, with the
    /// root's key time `t_key_ns` (0: none); with `NHG_PRESENT_READY` (its `FRAME_READY`) the shell presents it, in
    /// inline mode.  Returns `NHG_PRESENTED_*` bits.  Reads pixels.
    ///
    /// # Safety
    /// `d` is null or a live display; `s` holds `n` readable surfaces, or is null when `n` is 0.
    fn nhg_present(d: *mut NhgDisplay, root: u64, s: *const NhgSurface, n: usize, flags: u32, t_key_ns: u64) -> i32 |state| {
        let list = match (n, s.is_null()) {
            (0, _) => &[][..],
            (_, true) => return Err(GlueError::Arg),
            // SAFETY: non-null, with `n` readable surfaces (the caller's promise).
            _ => unsafe { std::slice::from_raw_parts(s, n) },
        };
        let (backend, backings, presenting) = state.presenting();
        let out = present::present(backend, backings, presenting, root, list, flags, t_key_ns)?;
        crate::reconnect::presented(state, root, out);
        Ok(out)
    }
}

nhg_export! {
    /// Present on the shell's own thread (`on` nonzero) or inline, at each root's `FRAME_READY`.  A thread that cannot
    /// start leaves the display inline (`NHG_E_NOMEM`).  Neither reads nor writes pixels.
    ///
    /// # Safety
    /// `d` is null or a live display.
    fn nhg_presenter_thread(d: *mut NhgDisplay, on: i32) -> i32 |state| {
        let (backend, _, presenting) = state.presenting();
        presenting.set_thread(backend, on != 0)?;
        Ok(NHG_OK)
    }
}

nhg_export! {
    /// The presenter's counters and timing since open in `out`.  Neither reads nor writes pixels.
    ///
    /// # Safety
    /// `d` is null or a live display; `out` is null or writable.
    fn nhg_presenter_stats(d: *mut NhgDisplay, out: *mut NhgPresenterStats) -> i32 |state| {
        if out.is_null() {
            return Err(GlueError::Arg);
        }
        let (backend, _, _) = state.presenting();
        let st = present::stats(backend)?;
        // SAFETY: non-null and writable (the caller's promise).
        unsafe { out.write(st) };
        Ok(NHG_OK)
    }
}

// ---- Selections, display queries, the bench, the bell and the pointer (`select.rs`, `query.rs`, `bench.rs`) --------
//
// KIND is `NHSHELL_SEL_CLIPBOARD` 0 or `NHSHELL_SEL_PRIMARY` 1.  A buffer a call lends (`nhg_selection_types`, `_read`)
// stays valid until the next call of the same entry point on the display.

use crate::bench::{NhgBenchOut, full_redraw};
use crate::query::{self, NhgDisplayInfo, NhgMonitor};
use crate::select;
use nhshell::api::NHSHELL_CURSOR_HIDDEN;
use std::ffi::CStr;

nhg_export! {
    /// Offer `n` types as selection `kind`: `mimes[i]` with the `lens[i]` bytes at `data[i]` (copied).  `NHG_E_STATE`
    /// without keyboard focus.  Neither reads nor writes pixels.
    ///
    /// # Safety
    /// `d` is null or a live display; `mimes`, `data` and `lens` hold `n` entries each (any may be null when `n` is
    /// 0), each `mimes[i]` is NUL-terminated and each `data[i]` holds `lens[i]` bytes (null allowed when that is 0).
    fn nhg_selection_set(d: *mut NhgDisplay, kind: u32, mimes: *const *const c_char, data: *const *const u8,
                         lens: *const usize, n: usize) -> i32 |state| {
        // SAFETY: the caller's promise, as this entry point's.
        let items = unsafe { typed_items(mimes, data, lens, n) }?;
        select::set(state, kind, &items)?;
        Ok(0)
    }
}

/// The `n` (MIME type, bytes) pairs C passes as `mimes[i]` with the `lens[i]` bytes at `data[i]`, borrowed.
///
/// # Safety
/// `mimes`, `data` and `lens` hold `n` entries each (any may be null when `n` is 0), each `mimes[i]` is NUL-terminated
/// and each `data[i]` holds `lens[i]` bytes (null allowed when that is 0), all alive for `'a`.
unsafe fn typed_items<'a>(
    mimes: *const *const c_char,
    data: *const *const u8,
    lens: *const usize,
    n: usize,
) -> Result<Vec<(&'a str, &'a [u8])>, GlueError> {
    if n > 0 && (mimes.is_null() || data.is_null() || lens.is_null()) {
        return Err(GlueError::Arg);
    }
    let mut items: Vec<(&str, &[u8])> = Vec::new();
    items.try_reserve(n).map_err(|_| GlueError::NoMem)?;
    for i in 0..n {
        // SAFETY: `n` entries each (the caller's promise).
        let (m, p, len) = unsafe { (*mimes.add(i), *data.add(i), *lens.add(i)) };
        if m.is_null() {
            return Err(GlueError::Arg);
        }
        // SAFETY: NUL-terminated (the caller's promise).
        let mime = unsafe { CStr::from_ptr(m) }
            .to_str()
            .map_err(|_| GlueError::Arg)?;
        let bytes: &[u8] = match (len, p.is_null()) {
            (0, _) => &[],
            (_, true) => return Err(GlueError::Arg),
            // SAFETY: `len` bytes (the caller's promise).
            (_, false) => unsafe { std::slice::from_raw_parts(p, len) },
        };
        items.push((mime, bytes));
    }
    Ok(items)
}

nhg_export! {
    /// Give selection `kind` up.  1 when this process owned it, 0 when it did not.  Neither reads nor writes pixels.
    ///
    /// # Safety
    /// `d` is null or a live display.
    fn nhg_selection_disown(d: *mut NhgDisplay, kind: u32) -> i32 |state| {
        Ok(i32::from(select::disown(state, kind)?))
    }
}

nhg_export! {
    /// 1 while this process's source is the current selection `kind`, else 0.  Neither reads nor writes pixels.
    ///
    /// # Safety
    /// `d` is null or a live display.
    fn nhg_selection_owned(d: *mut NhgDisplay, kind: u32) -> i32 |state| {
        Ok(i32::from(select::owned(state, kind)?))
    }
}

nhg_export! {
    /// The current MIME types of selection `kind`: their number is returned (0: no selection), and `*out`/`*len` get
    /// the types, each followed by a NUL, and their byte count.  Either may be null to ask for the number alone.
    /// Neither reads nor writes pixels.
    ///
    /// # Safety
    /// `d` is null or a live display; `out` and `len` are null or writable.
    fn nhg_selection_types(d: *mut NhgDisplay, kind: u32, out: *mut *const c_char, len: *mut usize) -> i32 |state| {
        let (n, bytes) = select::types(state, kind)?;
        if !out.is_null() {
            // SAFETY: non-null and writable (the caller's promise).
            unsafe { out.write(bytes.as_ptr().cast()) };
        }
        if !len.is_null() {
            // SAFETY: non-null and writable (the caller's promise).
            unsafe { len.write(bytes.len()) };
        }
        i32::try_from(n).map_err(|_| GlueError::NoMem)
    }
}

nhg_export! {
    /// Selection `kind` as `mime`: our own bytes at once, another client's within `timeout_ms` (`NHG_E_TIMEOUT`;
    /// `NHG_E_DISCONNECTED` within 100 ms of the connection's loss).  `NHG_E_STATE` without a selection or without
    /// that type.  The bytes go to `*out` and `*len`.  Neither reads nor writes pixels.
    ///
    /// # Safety
    /// `d` is null or a live display; `mime` is NUL-terminated; `out` and `len` are null or writable.
    fn nhg_selection_read(d: *mut NhgDisplay, kind: u32, mime: *const c_char, timeout_ms: u32, out: *mut *const u8,
                          len: *mut usize) -> i32 |state| {
        if mime.is_null() || out.is_null() || len.is_null() {
            return Err(GlueError::Arg);
        }
        // SAFETY: non-null and NUL-terminated (the caller's promise).
        let mime = unsafe { CStr::from_ptr(mime) }.to_str().map_err(|_| GlueError::Arg)?;
        let bytes = select::read(state, kind, mime, timeout_ms)?;
        // SAFETY: non-null and writable (checked above and the caller's promise).
        unsafe {
            out.write(bytes.as_ptr());
            len.write(bytes.len());
        }
        Ok(0)
    }
}

nhg_export! {
    /// The display as `x-display-*` reads it, in `out`.  Neither reads nor writes pixels.
    ///
    /// # Safety
    /// `d` is null or a live display; `out` is null or writable.
    fn nhg_display_info(d: *mut NhgDisplay, out: *mut NhgDisplayInfo) -> i32 |state| {
        if out.is_null() {
            return Err(GlueError::Arg);
        }
        // SAFETY: non-null and writable (the caller's promise).
        unsafe { out.write(query::info(state)) };
        Ok(0)
    }
}

nhg_export! {
    /// Present `frames` frames of the tree `s` (`n` surfaces as `nhg_present` takes them, bottom to top, the root
    /// first), each damaged whole and presented through the presenter, and time them (`out`).  `NHG_E_ARG` for
    /// `frames` outside 1 to 100000 or no surface; `NHG_E_STATE` when the root is not registered, has no backing, or
    /// fewer than two frames committed (`out->frames` says how many did).  Waits at most 5 s for each frame.  Reads
    /// pixels.
    ///
    /// # Safety
    /// `d` is null or a live display; `s` holds `n` readable surfaces; `out` is null or writable.
    fn nhg_bench_full_redraw(d: *mut NhgDisplay, s: *const NhgSurface, n: usize, frames: u32, out: *mut NhgBenchOut) -> i32 |state| {
        if out.is_null() || s.is_null() {
            return Err(GlueError::Arg);
        }
        // SAFETY: non-null, with `n` readable surfaces (the caller's promise).
        let list = unsafe { std::slice::from_raw_parts(s, n) };
        let r = full_redraw(state, list, frames)?;
        // SAFETY: non-null and writable (the caller's promise).
        unsafe { out.write(r) };
        if r.frames < 2 {
            return Err(GlueError::State);
        }
        Ok(0)
    }
}

nhg_export! {
    /// Ring the system bell on the root surface of `window`'s tree (a child frame rings on its top-level; 0: no
    /// surface), at most once per 20 ms.  `NHG_E_UNSUPPORTED` without the protocol.  Neither reads nor writes pixels.
    ///
    /// # Safety
    /// `d` is null or a live display.
    fn nhg_bell(d: *mut NhgDisplay, window: u64) -> i32 |state| {
        crate::bell::ring(state, window)?;
        Ok(0)
    }
}

nhg_export! {
    /// Show (`on`) the pointer over `window` as `shape` (`NHSHELL_CURSOR_*`), or hide it.  Neither reads nor writes
    /// pixels.
    ///
    /// # Safety
    /// `d` is null or a live display.
    fn nhg_pointer_visible(d: *mut NhgDisplay, window: u64, on: i32, shape: u32) -> i32 |state| {
        state.frame(window)?;
        let sh = state.shell();
        // SAFETY: the display's handle, used by this call only.
        let backend = unsafe { query::backend(sh) };
        backend.set_cursor(window, if on != 0 { shape } else { NHSHELL_CURSOR_HIDDEN })?;
        Ok(0)
    }
}

// ---- The title bar (`csd.rs`) --------------------------------------------------------------------------------------
//
// C lays a frame's border out through the glue when the states, the scale or `undecorated` change, keeps a copy of
// the offsets for its drawing, and at each publish asks whether to paint the strip; the painting is C's.

/// `n` codes at `p` (null only when `n` is 0).
///
/// # Safety
/// `p` holds `n` readable codes, or `n` is 0.
unsafe fn codes<'a>(p: *const u32, n: usize) -> Result<&'a [u32], GlueError> {
    match (n, p.is_null()) {
        (0, _) => Ok(&[]),
        (_, true) => Err(GlueError::Arg),
        // SAFETY: non-null, with `n` readable codes (the caller's promise).
        _ => Ok(unsafe { std::slice::from_raw_parts(p, n) }),
    }
}

nhg_export! {
    /// The title bars' style from Lisp: the buttons of the left and right sides in order (`NHG_CSD_*`; other codes
    /// are ignored).  Every title bar is painted again.  Neither reads nor writes pixels.
    ///
    /// # Safety
    /// `d` is null or a live display; `left` and `right` hold `nleft` and `nright` readable codes (null when 0).
    fn nhg_csd_style(d: *mut NhgDisplay, left: *const u32, nleft: usize, right: *const u32, nright: usize) -> i32 |state| {
        // SAFETY: as the caller promises.
        let (left, right) = unsafe { (codes(left, nleft)?, codes(right, nright)?) };
        crate::csd::style(state, left, right);
        Ok(NHG_OK)
    }
}

nhg_export! {
    /// Lay `window`'s border out again (C calls it when it applies a configure or a scale, and when `undecorated`
    /// changes): from the compositor's decoration answer, the toplevel's states, `scale120` (0: 120) and `flags`
    /// (`NHG_CSD_UNDECORATED`).  1 when the offsets or the strip changed (C garbages the frame), 0 when not; the
    /// layout in `*out`.  Sets the frame's CSD fact.  A child frame's border is its shadow margin (`child.rs`).
    /// Neither reads nor writes pixels.
    ///
    /// # Safety
    /// `d` is null or a live display; `out` is null or writable.
    fn nhg_csd_layout(d: *mut NhgDisplay, window: u64, scale120: u32, flags: u32, out: *mut crate::csd::NhgCsdLayout) -> i32 |state| {
        let (changed, lay) = crate::csd::layout(state, window, scale120, flags)?;
        if !out.is_null() {
            // SAFETY: non-null and writable (the caller's promise).
            unsafe { out.write(lay) };
        }
        Ok(i32::from(changed))
    }
}

nhg_export! {
    /// `window`'s backing around its native area, `out[0..4]` = L, T, R, B (device pixels; T includes the strip), as
    /// the title bar lays it out, or a child frame's shadow margin.  Neither reads nor writes pixels.
    ///
    /// # Safety
    /// `d` is null or a live display; `out` is null or holds 4 writable values.
    fn nhg_frame_offsets(d: *mut NhgDisplay, window: u64, out: *mut i32) -> i32 |state| {
        if out.is_null() {
            return Err(GlueError::Arg);
        }
        let o = crate::csd::offsets(state, window)?;
        // SAFETY: 4 writable values (the caller's promise), not overlapping `o`.
        unsafe { ptr::copy_nonoverlapping(o.as_ptr(), out, 4) };
        Ok(NHG_OK)
    }
}

nhg_export! {
    /// The publish step of `window`, before `nhg_present`, with its native size: sends a child frame's position and
    /// input region and the window geometry if they changed, then 1 with the strip's draw list in `*out` when C must
    /// paint the strip (and damage the border), 0 when the backing already shows it.  `look` hashes what C paints from that the glue does not see: the title,
    /// the colours, the font and the backing's generation.  Neither reads nor writes pixels.
    ///
    /// # Safety
    /// `d` is null or a live display; `out` is writable.
    fn nhg_csd_paint(d: *mut NhgDisplay, window: u64, native_w: i32, native_h: i32, look: u64, out: *mut crate::csd::NhgCsdPaint) -> i32 |state| {
        if out.is_null() {
            return Err(GlueError::Arg);
        }
        match crate::csd::paint(state, window, native_w, native_h, look)? {
            Some(p) => {
                // SAFETY: non-null and writable (the caller's promise).
                unsafe { out.write(p) };
                Ok(1)
            }
            None => Ok(0),
        }
    }
}

nhg_export! {
    /// `window`'s strip as laid out for the native size `native_w` x `native_h` (`nhwl--title-bar`): 1 with it in
    /// `*out`, 0 when the frame draws none.  Changes nothing.  Neither reads nor writes pixels.
    ///
    /// # Safety
    /// `d` is null or a live display; `out` is writable.
    fn nhg_csd_title_bar(d: *mut NhgDisplay, window: u64, native_w: i32, native_h: i32, out: *mut crate::csd::NhgCsdPaint) -> i32 |state| {
        if out.is_null() {
            return Err(GlueError::Arg);
        }
        match crate::csd::title_bar(state, window, native_w, native_h)? {
            Some(p) => {
                // SAFETY: non-null and writable (the caller's promise).
                unsafe { out.write(p) };
                Ok(1)
            }
            None => Ok(0),
        }
    }
}

// ---- Wheel units, the input method and outputs (`scroll.rs`, `ime.rs`, `monitors.rs`) ---------------------------------
//
// None of these reads or writes pixels.

nhg_export! {
    /// Push: `window`'s column width and line height in device pixels (`FRAME_COLUMN_WIDTH`, `FRAME_LINE_HEIGHT`),
    /// the size of a wheel step in a finger record.  C calls it when it makes the window and when the font changes;
    /// a checking build compares it with Emacs's values.  `NHG_E_ARG` for a size under 1, `NHG_E_STATE` for an
    /// unknown window.
    ///
    /// # Safety
    /// `d` is null or a live display.
    fn nhg_frame_units(d: *mut NhgDisplay, window: u64, col_w: i32, line_h: i32) -> i32 |state| {
        crate::scroll::set_units(state, window, col_w, line_h)?;
        Ok(NHG_OK)
    }
}

nhg_export! {
    /// Text input for `window` on (`on` non-zero) or off.  `NHG_E_UNSUPPORTED` without text-input-v3.
    ///
    /// # Safety
    /// `d` is null or a live display.
    fn nhg_im_enable(d: *mut NhgDisplay, window: u64, on: i32) -> i32 |state| {
        crate::ime::enable(state, window, on != 0)?;
        Ok(NHG_OK)
    }
}

nhg_export! {
    /// The cursor rectangle `r` in `window`, in logical pixels of its surface.  The compositor is told only when it
    /// changed.
    ///
    /// # Safety
    /// `d` is null or a live display.
    fn nhg_im_cursor(d: *mut NhgDisplay, window: u64, r: NhgRect) -> i32 |state| {
        crate::ime::cursor(state, window, r.x, r.y, r.w, r.h)?;
        Ok(NHG_OK)
    }
}

nhg_export! {
    /// The `len` bytes of UTF-8 at `text` around point in `window`, with point at byte `cursor` and the mark at byte
    /// `anchor` of them.  The compositor is told only when they changed (at most 4000 bytes go to it, cut around
    /// point).  `NHG_E_ARG` for text that is not UTF-8 or offsets under 0.
    ///
    /// # Safety
    /// `d` is null or a live display; `text` holds `len` readable bytes (null allowed when `len` is 0).
    fn nhg_im_surrounding(d: *mut NhgDisplay, window: u64, text: *const c_char, len: usize, cursor: i32, anchor: i32) -> i32 |state| {
        let bytes: &[u8] = match (len, text.is_null()) {
            (0, _) => &[],
            (_, true) => return Err(GlueError::Arg),
            // SAFETY: `len` bytes (the caller's promise).
            (_, false) => unsafe { std::slice::from_raw_parts(text.cast::<u8>(), len) },
        };
        let text = std::str::from_utf8(bytes).map_err(|_| GlueError::Arg)?;
        let cursor = usize::try_from(cursor).map_err(|_| GlueError::Arg)?;
        let anchor = usize::try_from(anchor).map_err(|_| GlueError::Arg)?;
        crate::ime::surrounding(state, window, text, cursor, anchor)?;
        Ok(NHG_OK)
    }
}

nhg_export! {
    /// Copy up to `cap` outputs into `out`; their number goes to `*n` (when `n` is not null) and is returned.
    ///
    /// # Safety
    /// `d` is null or a live display; `out` holds `cap` writable monitors (null allowed when `cap` is 0); `n` is null
    /// or writable.
    fn nhg_monitors(d: *mut NhgDisplay, out: *mut NhgMonitor, cap: usize, n: *mut usize) -> i32 |state| {
        let buf: &mut [NhgMonitor] = if cap == 0 {
            &mut []
        } else if out.is_null() {
            return Err(GlueError::Arg);
        } else {
            // SAFETY: `cap` writable monitors (the caller's promise).
            unsafe { std::slice::from_raw_parts_mut(out, cap) }
        };
        let got = crate::monitors::list(state, buf);
        if !n.is_null() {
            // SAFETY: non-null and writable (the caller's promise).
            unsafe { n.write(got) };
        }
        i32::try_from(got).map_err(|_| GlueError::NoMem)
    }
}

nhg_export! {
    /// Copy the ids of the outputs `window` is on, up to `cap`, into `out`; their number goes to `*n` (when `n` is not
    /// null) and is returned.  `NHG_E_STATE` for an unknown window.
    ///
    /// # Safety
    /// `d` is null or a live display; `out` holds `cap` writable ids (null allowed when `cap` is 0); `n` is null or
    /// writable.
    fn nhg_frame_outputs(d: *mut NhgDisplay, window: u64, out: *mut u64, cap: usize, n: *mut usize) -> i32 |state| {
        let buf: &mut [u64] = if cap == 0 {
            &mut []
        } else if out.is_null() {
            return Err(GlueError::Arg);
        } else {
            // SAFETY: `cap` writable ids (the caller's promise).
            unsafe { std::slice::from_raw_parts_mut(out, cap) }
        };
        let got = crate::monitors::frame_outputs(state, window, buf)?;
        if !n.is_null() {
            // SAFETY: non-null and writable (the caller's promise).
            unsafe { n.write(got) };
        }
        i32::try_from(got).map_err(|_| GlueError::NoMem)
    }
}

// ---- Window operations and tips (`window.rs`, `tip.rs`) ------------------------------------------------------------
//
// WINDOW is a shell window id; a window the glue does not know (or forgot) gives NHG_E_STATE.  None of these reads or
// writes pixels.

nhg_export! {
    /// Make the window `w` describes and register it as a frame of kind `w->role`, with the facts it starts from (its
    /// native size, scale, visibility and title bar); its id goes to `*out`.  A shell error is its code.
    ///
    /// # Safety
    /// `d` is null or a live display; `w` is null or readable, its texts null or holding their lengths' bytes; `out`
    /// is null or writable.
    fn nhg_window_new(d: *mut NhgDisplay, w: *const window::NhgWindowDesc, out: *mut u64) -> i32 |state| {
        // SAFETY: null or readable (the caller's promise).
        let desc = unsafe { w.as_ref() }.ok_or(GlueError::Arg)?;
        if out.is_null() {
            return Err(GlueError::Arg);
        }
        // SAFETY: each text is null or holds its length's bytes, valid for the call (the caller's promise).
        let (title, app_id, session) = unsafe {
            (
                window::text(desc.title, desc.title_len)?,
                window::text(desc.app_id, desc.app_id_len)?,
                window::text(desc.session_name, desc.session_name_len)?,
            )
        };
        let id = window::new(state, desc, title, app_id, session)?;
        // SAFETY: non-null and writable (the caller's promise).
        unsafe { out.write(id) };
        Ok(NHG_OK)
    }
}

nhg_export! {
    /// The window's title, `len` bytes of UTF-8 at `title` (up to a NUL).
    ///
    /// # Safety
    /// `d` is null or a live display; `title` is null (with `len` 0) or holds `len` readable bytes.
    fn nhg_window_title(d: *mut NhgDisplay, window: u64, title: *const c_char, len: usize) -> i32 |state| {
        // SAFETY: null or `len` readable bytes, valid for the call (the caller's promise).
        let title = unsafe { window::text(title, len) }?;
        window::title(state, window, title)?;
        Ok(NHG_OK)
    }
}

nhg_export! {
    /// The frame's `undecorated` changed at run time (`on` non-zero: undecorated): a top-level asks the compositor
    /// again; a child frame's window has nothing to ask.
    ///
    /// # Safety
    /// `d` is null or a live display.
    fn nhg_window_undecorated(d: *mut NhgDisplay, window: u64, on: i32) -> i32 |state| {
        window::undecorated(state, window, on != 0)?;
        Ok(NHG_OK)
    }
}

nhg_export! {
    /// Apply the frame's effective alpha `alpha` (0 to 1; C applied the highlight and `frame-alpha-lower-limit`) with
    /// its `alpha-background` `alpha_bg`.  Returns `NHG_ALPHA_*` bits: `DAMAGE` (C damages the whole frame) and `HOST`
    /// (the shell multiplies the pixels); the alpha's cut goes to `*cut` for C's surface list.
    ///
    /// # Safety
    /// `d` is null or a live display; `cut` is null or writable.
    fn nhg_window_alpha(d: *mut NhgDisplay, window: u64, alpha: f64, alpha_bg: f64, cut: *mut u32) -> i32 |state| {
        let (bits, c) = window::alpha(state, window, alpha, alpha_bg)?;
        if !cut.is_null() {
            // SAFETY: non-null and writable (the caller's promise).
            unsafe { cut.write(c) };
        }
        Ok(bits)
    }
}

nhg_export! {
    /// Push: C resized the frame itself to `w` x `h` device pixels; a configure being applied gets a content ack.
    /// `NHG_E_ARG` for an empty size.
    ///
    /// # Safety
    /// `d` is null or a live display.
    fn nhg_window_size(d: *mut NhgDisplay, window: u64, w: i32, h: i32) -> i32 |state| {
        window::size(state, window, w, h)?;
        Ok(NHG_OK)
    }
}

nhg_export! {
    /// The frame's smallest size, `w` x `h` device pixels, for the compositor (a child frame's window has none).
    ///
    /// # Safety
    /// `d` is null or a live display.
    fn nhg_window_min_size(d: *mut NhgDisplay, window: u64, w: i32, h: i32) -> i32 |state| {
        window::min_size(state, window, w, h)?;
        Ok(NHG_OK)
    }
}

nhg_export! {
    /// Push: Emacs made the frame visible (`on` non-zero) or invisible, not iconified.  Checked in checking builds.
    ///
    /// # Safety
    /// `d` is null or a live display.
    fn nhg_window_visible(d: *mut NhgDisplay, window: u64, on: i32) -> i32 |state| {
        window::visible(state, window, on != 0)?;
        Ok(NHG_OK)
    }
}

nhg_export! {
    /// Iconify the frame: the compositor minimizes the window (a push too: iconified, not visible).
    ///
    /// # Safety
    /// `d` is null or a live display.
    fn nhg_window_iconify(d: *mut NhgDisplay, window: u64) -> i32 |state| {
        window::iconify(state, window)?;
        Ok(NHG_OK)
    }
}

nhg_export! {
    /// The frame's `fullscreen`, `mode` an `NHG_FULLSCREEN_*` (`fullwidth` and `fullheight` ask for maximized).
    ///
    /// # Safety
    /// `d` is null or a live display.
    fn nhg_window_fullscreen(d: *mut NhgDisplay, window: u64, mode: u32) -> i32 |state| {
        window::fullscreen(state, window, mode)?;
        Ok(NHG_OK)
    }
}

nhg_export! {
    /// Destroy the window and forget its frame; calls naming it are refused from now on.  The frame is forgotten even
    /// when the shell refuses (a lost connection): C ignores the code while it tears down.
    ///
    /// # Safety
    /// `d` is null or a live display.
    fn nhg_window_destroy(d: *mut NhgDisplay, window: u64) -> i32 |state| {
        window::destroy(state, window)?;
        Ok(NHG_OK)
    }
}

nhg_export! {
    /// One round of the bounded wait for a top-level's first configure: 1 when it is configured; else the work due on
    /// this thread is done and the shell's descriptor waited on for at most `timeout_ms`: 0 when something is queued
    /// (C drains, then calls again), `NHG_E_TIMEOUT` when nothing came.
    ///
    /// # Safety
    /// `d` is null or a live display.
    fn nhg_window_wait_configured(d: *mut NhgDisplay, window: u64, timeout_ms: u32) -> i32 |state| {
        window::wait_configured(state, window, timeout_ms)
    }
}

nhg_export! {
    /// Show a tip on `parent`'s window at `anchor_dev` (device pixels in the parent's surface: the pointer, or the
    /// tip's `left` and `top`) moved by `dx`, `dy`.  `*window` 0: make the tip's popup, `native_w` x `native_h` device
    /// pixels, its id into `*window`, and return 1 (draw it whole).  Otherwise move it: 1 when the whole tip must be
    /// drawn again (the popup was made again, below `xdg_wm_base` 3), 0 when not.
    ///
    /// # Safety
    /// `d` is null or a live display; `window` is null or readable and writable.
    fn nhg_tip_show(d: *mut NhgDisplay, window: *mut u64, parent: u64, anchor_dev: NhgRect, dx: i32, dy: i32, native_w: i32, native_h: i32) -> i32 |state| {
        // SAFETY: null or readable and writable (the caller's promise).
        let w = unsafe { window.as_mut() }.ok_or(GlueError::Arg)?;
        tip::show(state, w, parent, anchor_dev, (dx, dy), (native_w, native_h))
    }
}

nhg_export! {
    /// The tip's popup goes (the compositor dismissed it: a `DELETE` action with `NHG_DEL_TIP`), and its frame is
    /// forgotten.  `NHG_E_STATE` for a window that is not a tip's.
    ///
    /// # Safety
    /// `d` is null or a live display.
    fn nhg_tip_hide(d: *mut NhgDisplay, window: u64) -> i32 |state| {
        tip::hide(state, window)?;
        Ok(NHG_OK)
    }
}

// ---- Child frames (`child.rs`) -------------------------------------------------------------------------------------
//
// C pushes a child frame's facts when its window is made, moved, shown or hidden, and its chrome when its parameters
// change; it hands the siblings' order over on raise and lower.  The child's publish step runs in `nhg_csd_paint`, its
// margin is laid out by `nhg_csd_layout`, and its layer and chrome go to Glitnir in `nhg_layer_ensure`.

nhg_export! {
    /// Push: `window` is a child of `parent`'s window, its native area at (`x`, `y`) in the parent's native area
    /// (device pixels), with `flags` (`NHG_CHILD_*`).  1 when its surface was mapped again (C damages the frame
    /// whole), else 0.  Neither reads nor writes pixels.
    ///
    /// # Safety
    /// `d` is null or a live display.
    fn nhg_child_set(d: *mut NhgDisplay, window: u64, parent: u64, x: i32, y: i32, flags: u32) -> i32 |state| {
        Ok(i32::from(crate::child::set(state, window, parent, x, y, flags)?))
    }
}

nhg_export! {
    /// `window`'s chrome as `glitnir--resolve` gave it now: 1 when it changed (C lays the frame out again and
    /// garbages it), 0 when not.  Neither reads nor writes pixels.
    ///
    /// # Safety
    /// `d` is null or a live display; `c` is null or points to a readable `NhgChrome`.
    fn nhg_child_chrome(d: *mut NhgDisplay, window: u64, c: *const crate::child::NhgChrome) -> i32 |state| {
        // SAFETY: null or readable (the caller's promise).
        let c = unsafe { c.as_ref() }.ok_or(GlueError::Arg)?;
        Ok(i32::from(crate::child::chrome(state, window, c)?))
    }
}

nhg_export! {
    /// `window` was raised or lowered: `siblings` holds its parent's `n` child windows bottom to top as C now stacks
    /// them, `window` among them; `z_group` is the frame's z-group.  Neither reads nor writes pixels.
    ///
    /// # Safety
    /// `d` is null or a live display; `siblings` holds `n` readable window ids.
    fn nhg_child_stack(d: *mut NhgDisplay, window: u64, siblings: *const u64, n: usize, z_group: i32) -> i32 |state| {
        if siblings.is_null() || n == 0 {
            return Err(GlueError::Arg);
        }
        // SAFETY: non-null, with `n` readable ids (the caller's promise).
        let s = unsafe { std::slice::from_raw_parts(siblings, n) };
        crate::child::stack(state, window, s, z_group)?;
        Ok(NHG_OK)
    }
}

// ---- Menu popups (`popup.rs`) ---------------------------------------------------------------------------------------
//
// A popup frame's window exists while the frame is shown.  ANCHOR_DEV is the `nhwl-popup` parameter's rectangle, in
// device pixels of the parent's native area; GRAVITY is 1 to 8 (`NhgAnchor`'s), ADJUST `NHSHELL_ADJUST_*`, FLAGS
// `NHG_POPUP_*`.  None of these reads or writes pixels.

nhg_export! {
    /// Show a popup frame: a new popup of `parent`'s window at `anchor_dev`, growing toward `gravity`, `native_w` x
    /// `native_h` device pixels at the parent's scale, hanging from top-level `owner` (its keyboard focus is kept
    /// while the popup grabs); registered as a visible popup frame, its id into `*window`.
    ///
    /// # Safety
    /// `d` is null or a live display; `window` is null or writable.
    fn nhg_popup_show(d: *mut NhgDisplay, window: *mut u64, parent: u64, owner: u64, anchor_dev: NhgRect, gravity: u32, adjust: u32, flags: u32, native_w: i32, native_h: i32) -> i32 |state| {
        if window.is_null() {
            return Err(GlueError::Arg);
        }
        let place = crate::popup::Place { anchor: anchor_dev, gravity, adjust, flags };
        let id = crate::popup::show(state, parent, owner, &place, (native_w, native_h))?;
        // SAFETY: non-null and writable (the caller's promise).
        unsafe { window.write(id) };
        Ok(NHG_OK)
    }
}

nhg_export! {
    /// The popup frame's parameter changed: unchanged, 0; otherwise the shown popup moves to the new anchor, and 1
    /// when the whole frame must be drawn again (the popup was made again, below `xdg_wm_base` 3).  Size, gravity,
    /// adjust and grab apply at the next show.
    ///
    /// # Safety
    /// `d` is null or a live display.
    fn nhg_popup_place(d: *mut NhgDisplay, window: u64, anchor_dev: NhgRect, gravity: u32, adjust: u32, flags: u32) -> i32 |state| {
        let place = crate::popup::Place { anchor: anchor_dev, gravity, adjust, flags };
        crate::popup::place(state, window, &place)
    }
}

nhg_export! {
    /// The popup frame's size is now `w` x `h` device pixels: the popup takes it; 1 when the whole frame must be drawn
    /// again (below `xdg_wm_base` 3), 0 when not.
    ///
    /// # Safety
    /// `d` is null or a live display.
    fn nhg_popup_size(d: *mut NhgDisplay, window: u64, w: i32, h: i32) -> i32 |state| {
        crate::popup::size(state, window, w, h)
    }
}

nhg_export! {
    /// Hide a popup frame: its window (0: none) goes and is forgotten.  1 when top-level `owner` has lost the keyboard
    /// focus it had lent to a grabbing popup (C sends Emacs the focus-out), else 0.
    ///
    /// # Safety
    /// `d` is null or a live display.
    fn nhg_popup_hide(d: *mut NhgDisplay, window: u64, owner: u64) -> i32 |state| {
        crate::popup::hide(state, window, owner)
    }
}

// ---- Window layers (`layers.rs`) -----------------------------------------------------------------------------------
//
// WINDOW is the frame's shell window (the opaque-root check reads it), FRAME its key.  FLAGS are `NHG_LAYERS_*`:
// `nhwl-scroll-layers` and the frame's `alpha-background`.

nhg_export! {
    /// `nhg_scroll` for Emacs window `w_seq` at `rect` (`window_box (w, ANY_AREA)`, with the native area's origin): a
    /// full-window scroll on an opaque root makes the window its own output first, a changed rectangle or a frame no
    /// longer eligible makes it part of the frame again.  1 when the window is its own output now, else 0.  Writes
    /// pixels.
    ///
    /// # Safety
    /// `d` is null or a live display.
    fn nhg_scroll_window(d: *mut NhgDisplay, window: u64, frame: u64, w_seq: u64, rect: NhgRect, run: NhgRect, to_y: i32, flags: u32) -> i32 |state| {
        let own = crate::layers::scroll(state, window, frame, w_seq, rect, run, to_y, flags)?;
        Ok(i32::from(own))
    }
}

nhg_export! {
    /// The frame is about to be published and its Emacs windows are the `n` at `windows`: own outputs that are gone,
    /// resized or idle, or all of them when the frame is no longer an opaque root, become part of the frame again.
    /// Returns how many stay own outputs.  Neither reads nor writes pixels.
    ///
    /// # Safety
    /// `d` is null or a live display; `windows` holds `n` readable records, or is null when `n` is 0.
    fn nhg_layers_sync(d: *mut NhgDisplay, window: u64, frame: u64, windows: *const crate::layers::NhgLayerWindow, n: usize, flags: u32) -> i32 |state| {
        let w = match (n, windows.is_null()) {
            (0, _) => &[][..],
            (_, true) => return Err(GlueError::Arg),
            // SAFETY: non-null, with `n` readable records (the caller's promise).
            _ => unsafe { std::slice::from_raw_parts(windows, n) },
        };
        let left = crate::layers::sync(state, window, frame, w, flags)?;
        Ok(i32::try_from(left).unwrap_or(i32::MAX))
    }
}

nhg_export! {
    /// How many of the frame's Emacs windows are their own outputs (0 for a frame without a backing).
    ///
    /// # Safety
    /// `d` is null or a live display.
    fn nhg_layers_count(d: *mut NhgDisplay, frame: u64) -> i32 |state| {
        let n = crate::layers::count(&mut state.backings, frame);
        Ok(i32::try_from(n).unwrap_or(i32::MAX))
    }
}

// ---- The visible bell, idle inhibition, export handles and activation tokens (`bell.rs`) ---------------------------

nhg_export! {
    /// The visible bell: invert the rows `g` names (the top and bottom line, or all of a short frame) in the backing
    /// of the frame with key `key`, and damage them.  Returns how many rows were inverted.  C puts them back with
    /// `nhg_bell_restore` 50 ms later.  Writes pixels: C flushes the surface before and marks it dirty after.
    ///
    /// # Safety
    /// `d` is null or a live display; `g` is null or points to a readable `NhgBellGeom`.
    fn nhg_bell_flash(d: *mut NhgDisplay, key: u64, g: *const crate::bell::NhgBellGeom) -> i32 |state| {
        // SAFETY: null or readable (the caller's promise).
        let g = unsafe { g.as_ref() }.ok_or(GlueError::Arg)?;
        crate::bell::flash(state, key, g)
    }
}

nhg_export! {
    /// Put back what `nhg_bell_flash` inverted, except the pixels a draw has replaced since, and damage the rows.
    /// Returns how many rows were written (0: nothing was left to put back).  Writes pixels: C flushes the surface
    /// before and marks it dirty after.
    ///
    /// # Safety
    /// `d` is null or a live display.
    fn nhg_bell_restore(d: *mut NhgDisplay, key: u64) -> i32 |state| {
        Ok(state.backings.unflash(key)?)
    }
}

nhg_export! {
    /// Hold (`on` non-zero) or release an idle inhibitor on `window`'s surface.  Returns 1 when that changed it, 0
    /// when it was so already.  `NHG_E_UNSUPPORTED` without the protocol.  Neither reads nor writes pixels.
    ///
    /// # Safety
    /// `d` is null or a live display.
    fn nhg_inhibit_idle(d: *mut NhgDisplay, window: u64, on: i32) -> i32 |state| {
        Ok(i32::from(crate::bell::inhibit_idle(state, window, on != 0)?))
    }
}

nhg_export! {
    /// The xdg-foreign handle of `window`'s top-level (no scheme prefix), exported at the first call and kept until
    /// the window goes, waiting at most `timeout_ms`: NUL-terminated into `buf` (`cap` bytes), its length into `*len`.
    /// `NHG_E_TIMEOUT`, `NHG_E_UNSUPPORTED` without the exporter, `NHG_E_TOO_SMALL`.  Neither reads nor writes pixels.
    ///
    /// # Safety
    /// `d` is null or a live display; `buf` is null or holds `cap` writable bytes; `len` is null or writable.
    fn nhg_window_export(d: *mut NhgDisplay, window: u64, timeout_ms: u32, buf: *mut c_char, cap: usize, len: *mut usize) -> i32 |state| {
        let handle = crate::bell::export(state, window, timeout_ms)?;
        // SAFETY: `buf` and `len` are null or valid (the caller's promise).
        unsafe { crate::bell::put(&handle, buf, cap, len) }
    }
}

nhg_export! {
    /// A fresh activation token made on `window`'s top-level with the newest input serial, within `timeout_ms`:
    /// NUL-terminated into `buf` (`cap` bytes), its length into `*len`.  `NHG_E_STATE` before any input,
    /// `NHG_E_TIMEOUT`, `NHG_E_UNSUPPORTED` without the protocol, `NHG_E_TOO_SMALL`.  Neither reads nor writes pixels.
    ///
    /// # Safety
    /// `d` is null or a live display; `buf` is null or holds `cap` writable bytes; `len` is null or writable.
    fn nhg_activation_token(d: *mut NhgDisplay, window: u64, timeout_ms: u32, buf: *mut c_char, cap: usize, len: *mut usize) -> i32 |state| {
        let token = crate::bell::token(state, window, timeout_ms)?;
        // SAFETY: `buf` and `len` are null or valid (the caller's promise).
        unsafe { crate::bell::put(&token, buf, cap, len) }
    }
}

// ---- Session restore (`session.rs`) ---------------------------------------------------------------------------------

nhg_export! {
    /// Open the compositor's session for `reason` (`NHG_SESSION_LAUNCH`, `_RECOVER` or `_RESTORE`): `id` (`id_len`
    /// bytes, empty or null for none) is the saved session's id, else a new session is asked for.  An open session is
    /// dropped first.  The answer comes as an `NHG_ACT_SESSION`.  `NHG_E_UNSUPPORTED` without the protocol.  Neither
    /// reads nor writes pixels.
    ///
    /// # Safety
    /// `d` is null or a live display; `id` is null (with `id_len` 0) or holds `id_len` readable bytes.
    fn nhg_session_open(d: *mut NhgDisplay, id: *const c_char, id_len: usize, reason: u32) -> i32 |state| {
        // SAFETY: null or `id_len` readable bytes, valid for the call (the caller's promise).
        let id = unsafe { window::text(id, id_len) }?;
        crate::session::open(state, id, reason)?;
        Ok(NHG_OK)
    }
}

nhg_export! {
    /// The open session's id: NUL-terminated into `buf` (`cap` bytes), its length into `*len`.  `NHG_E_STATE` before
    /// the compositor has answered, `NHG_E_TOO_SMALL`.
    ///
    /// # Safety
    /// `d` is null or a live display; `buf` is null or holds `cap` writable bytes; `len` is null or writable.
    fn nhg_session_id(d: *mut NhgDisplay, buf: *mut c_char, cap: usize, len: *mut usize) -> i32 |state| {
        let id = crate::session::id(state)?;
        // SAFETY: `buf` and `len` are null or valid (the caller's promise).
        unsafe { crate::bell::put(&id, buf, cap, len) }
    }
}

nhg_export! {
    /// Take `window` out of the session, so the compositor forgets its name.  1 when it was named, else 0.
    ///
    /// # Safety
    /// `d` is null or a live display.
    fn nhg_session_remove(d: *mut NhgDisplay, window: u64) -> i32 |state| {
        Ok(i32::from(crate::session::remove(state, window)?))
    }
}

// ---- The drop target (`dnd.rs`) ------------------------------------------------------------------------------------

nhg_export! {
    /// Register the types a drop on `window` takes: `mimes[i]` for `i < n`, replacing the window's earlier list (an
    /// empty list takes any type).  `NHG_E_STATE` for a window that is not a registered frame.  Neither reads nor
    /// writes pixels.
    ///
    /// # Safety
    /// `d` is null or a live display; `mimes` holds `n` NUL-terminated strings (null allowed when `n` is 0).
    fn nhg_dnd_register(d: *mut NhgDisplay, window: u64, mimes: *const *const c_char, n: usize) -> i32 |state| {
        if n > 0 && mimes.is_null() {
            return Err(GlueError::Arg);
        }
        let mut list = Vec::new();
        list.try_reserve(n).map_err(|_| GlueError::NoMem)?;
        for i in 0..n {
            // SAFETY: `n` entries (the caller's promise).
            let m = unsafe { *mimes.add(i) };
            if m.is_null() {
                return Err(GlueError::Arg);
            }
            // SAFETY: NUL-terminated (the caller's promise).
            let m = unsafe { CStr::from_ptr(m) }.to_str().map_err(|_| GlueError::Arg)?;
            list.push(m.to_owned());
        }
        crate::dnd::register(state, window, list)?;
        Ok(0)
    }
}

nhg_export! {
    /// Answer the drag over our window: accept it for `action` (`NHSHELL_DND_*`, one bit; 0 accepts nothing), with
    /// the type last read from the offer or else its first.  `time_ms` is the time of the `DND_*` action Lisp answers.
    /// 1 when the shell was told, 0 for a stale call or without a drag.  Neither reads nor writes pixels.
    ///
    /// # Safety
    /// `d` is null or a live display.
    fn nhg_dnd_status(d: *mut NhgDisplay, action: u32, time_ms: u32) -> i32 |state| {
        Ok(i32::from(crate::dnd::status(state, action, time_ms)?))
    }
}

nhg_export! {
    /// Finish the drop at `time_ms`: `success` non-zero when its action was performed.  The offer is destroyed in
    /// every case.  1 when the source was told it finished, 0 for a stale call, without a dropped offer, or when there
    /// was nothing to tell the source.  Neither reads nor writes pixels.
    ///
    /// # Safety
    /// `d` is null or a live display.
    fn nhg_dnd_finish(d: *mut NhgDisplay, success: i32, time_ms: u32) -> i32 |state| {
        Ok(i32::from(crate::dnd::finish(state, success != 0, time_ms)?))
    }
}

// ---- Reconnecting in place (`reconnect.rs`) -------------------------------------------------------------------------

nhg_export! {
    /// Reconnect in place when the display goes (`on` non-zero; off at open): a hangup then gives
    /// `NHG_ACT_RECONNECTING`, not `NHG_ACT_LOST`.  `NHG_E_UNSUPPORTED` when the connection cannot be made again (an
    /// inherited socket).  Neither reads nor writes pixels.
    ///
    /// # Safety
    /// `d` is null or a live display.
    fn nhg_set_reconnect(d: *mut NhgDisplay, on: i32) -> i32 |state| {
        state.backend().set_reconnect(on != 0)?;
        Ok(NHG_OK)
    }
}

nhg_export! {
    /// One tick of C's reconnect timer: a step of at most 50 ms, given up `timeout_ms` after the loss.  `NHG_OK` once
    /// connected (`NHG_ACT_RECONNECTED` comes out of `nhg_next`) or when not reconnecting; `NHG_E_TIMEOUT` with the
    /// next tick's delay in `*next_ms`; `NHG_E_DISCONNECTED` when the glue gave up (C drops the display).  Neither
    /// reads nor writes pixels.
    ///
    /// # Safety
    /// `d` is null or a live display; `next_ms` is null or writable.
    fn nhg_reconnect_step(d: *mut NhgDisplay, timeout_ms: u32, next_ms: *mut u32) -> i32 |state| {
        let (rc, next) = crate::reconnect::step(state, timeout_ms);
        if !next_ms.is_null() {
            // SAFETY: non-null and writable (the caller's promise).
            unsafe { *next_ms = next };
        }
        Ok(rc)
    }
}

nhg_export! {
    /// After `NHG_ACT_RECONNECTED`, what is due now: `NHG_SETTLE_*` bits, with `*window` the window that loses the
    /// keyboard for `NHG_SETTLE_FOCUS_OUT`, and `*next_ms` when to ask again (0: nothing is pending).  Neither reads
    /// nor writes pixels.
    ///
    /// # Safety
    /// `d` is null or a live display; `window` and `next_ms` are null or writable.
    fn nhg_reconnect_settle(d: *mut NhgDisplay, window: *mut u64, next_ms: *mut u32) -> i32 |state| {
        let (bits, w, next) = crate::reconnect::settle(state, std::time::Instant::now());
        // SAFETY: each is null or writable (the caller's promise).
        unsafe {
            if !window.is_null() {
                *window = w;
            }
            if !next_ms.is_null() {
                *next_ms = next;
            }
        }
        Ok(bits)
    }
}

// ---- The drag source, `x-begin-drag` (`dnd.rs`) --------------------------------------------------------------------

nhg_export! {
    /// Drag `n` types out of `window`: `mimes[i]` with the `lens[i]` bytes at `data[i]` (copied), offering `actions`
    /// (`NHSHELL_DND_*`), our own windows preferring `preferred` (one of them), with `NHG_DRAG_*` `flags`.
    /// `NHG_E_STATE` without a held button press or touch down, or for a window that is not a frame; `NHG_E_ARG`
    /// without types or for bad actions.  The `NHG_ACT_DRAG_*` actions follow.  Neither reads nor writes pixels.
    ///
    /// # Safety
    /// `d` is null or a live display; `mimes`, `data` and `lens` as for `nhg_selection_set`.
    fn nhg_drag_start(d: *mut NhgDisplay, window: u64, mimes: *const *const c_char, data: *const *const u8,
                      lens: *const usize, n: usize, actions: u32, preferred: u32, flags: u32) -> i32 |state| {
        // SAFETY: the caller's promise, as this entry point's.
        let items = unsafe { typed_items(mimes, data, lens, n) }?;
        crate::dnd::drag_start(state, window, &items, actions, preferred, flags)?;
        Ok(0)
    }
}

nhg_export! {
    /// Give our drag up: its source goes, and no `NHG_ACT_DRAG_DONE` follows.  1 when it ran, else 0.  Neither reads
    /// nor writes pixels.
    ///
    /// # Safety
    /// `d` is null or a live display.
    fn nhg_drag_cancel(d: *mut NhgDisplay) -> i32 |state| {
        Ok(i32::from(crate::dnd::drag_cancel(state)))
    }
}

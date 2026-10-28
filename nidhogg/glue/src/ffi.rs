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
use nhshell::event::NhShellEvent;
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
    /// Push: whether `window`'s frame draws its own title bar (`on` non-zero).  Neither reads nor writes pixels.
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
    /// The shell event behind the last `nhg_next`'s `NHG_ACT_RAW` action, into `*out`.  `NHG_E_STATE` when the last
    /// event gave none.  Transitional, until every event kind is ported.  Neither reads nor writes pixels.
    ///
    /// # Safety
    /// `d` is null or a live display; `out` is writable.
    fn nhg_raw_event(d: *mut NhgDisplay, out: *mut NhShellEvent) -> i32 |state| {
        if out.is_null() {
            return Err(GlueError::Arg);
        }
        let ev = *state.actions().raw().ok_or(GlueError::State)?;
        // SAFETY: non-null and writable (the caller's promise).
        unsafe { out.write(ev) };
        Ok(NHG_OK)
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
        // the facts C pushes so far: the native size it asked for, and whether it draws its own title bar (the rest
        // join as the tasks that read them push them)
        let seen = |f: &crate::frame::NhgFrameFacts| (f.native_w, f.native_h, f.flags & crate::frame::NHG_FACT_CSD);
        debug_assert_eq!(seen(&pushed), seen(emacs), "the facts pushed for window {window} drifted from Emacs's");
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
    /// made or changed (the frame is damaged whole and its pending damage dropped), 0 when it already matched.
    /// Neither reads nor writes pixels.
    ///
    /// # Safety
    /// `d` is null or a live display; `frame` and `g` are null or valid for the call.
    fn nhg_layer_ensure(d: *mut NhgDisplay, frame: *mut u64, g: *const NhgFrameGeom) -> i32 |state| {
        // SAFETY: null or valid for the call (the caller's promise).
        match unsafe { (frame.as_mut(), g.as_ref()) } {
            (Some(frame), Some(g)) => Ok(i32::from(state.backings.ensure(frame, g))),
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
        present::present(backend, backings, presenting, root, list, flags, t_key_ns)
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

// ---- The keyboard (`keys.rs`, `focus.rs`) --------------------------------------------------------------------------

nhg_export! {
    /// The window that has the keyboard focus, as the last focus event said, into `*out` (0: none of ours).  Neither
    /// reads nor writes pixels.
    ///
    /// # Safety
    /// `d` is null or a live display; `out` is writable.
    fn nhg_keyboard_window(d: *mut NhgDisplay, out: *mut u64) -> i32 |state| {
        if out.is_null() {
            return Err(GlueError::Arg);
        }
        // SAFETY: non-null and writable (the caller's promise).
        unsafe { out.write(state.keyboard().window) };
        Ok(NHG_OK)
    }
}

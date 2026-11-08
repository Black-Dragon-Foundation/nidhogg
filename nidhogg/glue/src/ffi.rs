//! The entry points of `include/nhglue.h`.
//!
//! Every one is written with `nhg_export!`, so each runs in the guard (`guard.rs`): stack headroom, `try_lock` on the
//! display, `catch_unwind`.  A panic poisons its display and returns `NHG_E_PANIC`; the glue never aborts Emacs.

use crate::abi::{
    NHG_ABI_VERSION, NHG_E_ARG, NHG_E_DISCONNECTED, NHG_E_NOMEM, NHG_E_PANIC, NHG_E_UNSUPPORTED,
    NHG_OK,
};
use crate::action::{NhgActionList, NhgNextIn};
use crate::backing::{NhgBacking, NhgFrameGeom, NhgRect};
pub use crate::display::NhgDisplay;
use crate::guard::GlueError;
use crate::present::{self, NhgPresenterStats, NhgSurface};
use crate::{configure, drain};
use crate::{tip, window};
use nhshell::caps::NhShellCaps;
use std::ffi::c_char;
use std::ptr;

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
    // SAFETY: `display` is null or NUL-terminated (the caller's promise).
    let display = (!display.is_null()).then(|| unsafe { CStr::from_ptr(display) });
    // a panic in the backend's open is the shell's (NHG_E_PANIC), and poisons nothing: there is no display yet.  Its
    // report goes to stderr (inside the guard the panic hook keeps it instead of printing it).
    let shell = crate::guard::catching(|| nhshell::open(display))
        .map_err(|report| {
            use std::io::Write as _;
            let _ = writeln!(std::io::stderr(), "nhglue: {}", report.text());
            NHG_E_PANIC
        })?
        .map_err(|e| e.code())?;
    Ok(Box::into_raw(Box::new(NhgDisplay::new(shell))))
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
    /// threads and leaks the rest.
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
        Ok(state.backend().wake_fd())
    }
}

nhg_export! {
    /// Run the shell's work that must happen on the host's thread (configure and surface events) and flush.
    ///
    /// # Safety
    /// `d` is null or a live display.
    fn nhg_dispatch_main(d: *mut NhgDisplay) -> i32 |state| {
        state.backend().dispatch_main()?;
        Ok(NHG_OK)
    }
}

nhg_export! {
    /// Copy the shell's capabilities to `*out` (`NhgCaps`).
    ///
    /// # Safety
    /// `d` is null or a live display; `out` is null or writable.
    fn nhg_caps(d: *mut NhgDisplay, out: *mut NhShellCaps) -> i32 |state| {
        if out.is_null() {
            return Err(GlueError::Arg);
        }
        // SAFETY: non-null and writable (the caller's promise).
        unsafe { out.write(state.backend().caps()) };
        Ok(NHG_OK)
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
    /// child frame's layer (`g.window`), or a framed undecorated top-level's, sits inside its margin and takes its
    /// chrome (`child.rs`).  Neither reads nor writes pixels.
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
        // C lists child frames unsorted: a list to publish is checked as given, then put in stacking order
        let list = if flags & present::NHG_PRESENT_BACKSTOP != 0 {
            present::drawable(root, list)?;
            crate::child::stack_order(state, list)
        } else {
            list.to_vec()
        };
        let (backend, backings, presenting) = state.presenting();
        let out = present::present(backend, backings, presenting, root, &list, flags, t_key_ns)?;
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

nhg_export! {
    /// Up to `cap` of root `root`'s child windows in `out`, bottom to top as the compositor was told to stack them; the
    /// number copied.  Neither reads nor writes pixels.
    ///
    /// # Safety
    /// `d` is null or a live display; `out` holds `cap` writable windows.
    fn nhg_stacking(d: *mut NhgDisplay, root: u64, out: *mut u64, cap: usize) -> i32 |state| {
        if out.is_null() {
            return Err(GlueError::Arg);
        }
        let (backend, _, _) = state.presenting();
        let list = backend.window_stacking(root)?;
        let n = list.len().min(cap);
        // SAFETY: `out` holds `cap` writable windows, and `n <= cap` (the caller's promise).
        unsafe { ptr::copy_nonoverlapping(list.as_ptr(), out, n) };
        Ok(n as i32)
    }
}

nhg_export! {
    /// Root `root`'s commits that showed something, in `out`.  Neither reads nor writes pixels.
    ///
    /// # Safety
    /// `d` is null or a live display; `out` is null or writable.
    fn nhg_draw_count(d: *mut NhgDisplay, root: u64, out: *mut u64) -> i32 |state| {
        if out.is_null() {
            return Err(GlueError::Arg);
        }
        let (backend, _, _) = state.presenting();
        let n = backend.draw_count(root)?;
        // SAFETY: non-null and writable (the caller's promise).
        unsafe { out.write(n) };
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
    /// The type to read the selection of the last `nhg_selection_types` call as: `name` (`len` bytes) when the offer
    /// has it, else the first of the `nprefs` NUL-terminated `prefs` it has.  1 and the type in `*out`
    /// (NUL-terminated, valid until the next `nhg_selection_types`), or 0 for none.  Neither reads nor writes pixels.
    ///
    /// # Safety
    /// `d` is null or a live display; `name` holds `len` readable bytes (null with 0); `prefs` holds `nprefs`
    /// NUL-terminated strings (null with 0); `out` is null or writable.
    fn nhg_selection_pick(d: *mut NhgDisplay, name: *const c_char, len: usize, prefs: *const *const c_char,
                          nprefs: usize, out: *mut *const c_char) -> i32 |state| {
        if out.is_null() || (name.is_null() && len != 0) || (prefs.is_null() && nprefs != 0) {
            return Err(GlueError::Arg);
        }
        let name: &[u8] = if len == 0 {
            &[]
        } else {
            // SAFETY: non-null with `len` readable bytes (the caller's promise).
            unsafe { std::slice::from_raw_parts(name.cast::<u8>(), len) }
        };
        let mut want = Vec::with_capacity(nprefs);
        for i in 0..nprefs {
            // SAFETY: `prefs` holds `nprefs` pointers (the caller's promise), and `i < nprefs`.
            let p = unsafe { prefs.add(i).read() };
            if p.is_null() {
                return Err(GlueError::Arg);
            }
            // SAFETY: non-null and NUL-terminated (the caller's promise).
            want.push(unsafe { CStr::from_ptr(p) }.to_bytes());
        }
        let Some(t) = select::pick(state, name, &want) else {
            return Ok(0);
        };
        // SAFETY: non-null and writable (checked above and the caller's promise).
        unsafe { out.write(t.as_ptr().cast()) };
        Ok(1)
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
        // SAFETY: non-null and NUL-terminated (the caller's promise).  Copied: it may be nhg_selection_pick's
        // answer, which lies in the glue's own buffer.
        let mime = unsafe { CStr::from_ptr(mime) }.to_str().map_err(|_| GlueError::Arg)?.to_owned();
        let bytes = select::read(state, kind, &mime, timeout_ms)?;
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
    /// layout in `*out`.  Sets the frame's CSD fact.  An undecorated top-level where the compositor offers no server
    /// decorations keeps its margins without the strip.  A child frame's border is its shadow margin (`child.rs`).
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
    /// The cursor rectangle `r` of `window`'s frame, device pixels in its native area, for its tree's root at
    /// `scale120` (`ime::cursor`).  The compositor is told only when it changed.
    ///
    /// # Safety
    /// `d` is null or a live display.
    fn nhg_im_cursor(d: *mut NhgDisplay, window: u64, scale120: u32, r: NhgRect) -> i32 |state| {
        crate::ime::cursor(state, window, scale120, [r.x, r.y, r.w, r.h])?;
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
    /// Where the line `nhg_im_surrounding` pushed starts, point and the selection's other end (the mark when it is
    /// active, else point), in characters from the buffer's accessible start, for `window`'s input method
    /// (`ime::offsets`; macOS, as NS counts its ranges).  `NHG_E_ARG` for an offset under 0.
    ///
    /// # Safety
    /// `d` is null or a live display.
    fn nhg_im_offsets(d: *mut NhgDisplay, window: u64, line_start: i64, point: i64, mark: i64) -> i32 |state| {
        let off = |v: i64| usize::try_from(v).map_err(|_| GlueError::Arg);
        crate::ime::offsets(state, window, off(line_start)?, off(point)?, off(mark)?)?;
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

/// `p` as UTF-8, or `Arg` (also for null).
///
/// # Safety
/// `p` is null or NUL-terminated, valid for `'a`.
unsafe fn c_text<'a>(p: *const c_char) -> Result<&'a str, GlueError> {
    if p.is_null() {
        return Err(GlueError::Arg);
    }
    // SAFETY: non-null and NUL-terminated (the caller's promise).
    unsafe { CStr::from_ptr(p) }
        .to_str()
        .map_err(|_| GlueError::Arg)
}

nhg_export! {
    /// Start an interactive move of `window` with the latest input serial.
    ///
    /// # Safety
    /// `d` is null or a live display.
    fn nhg_window_move(d: *mut NhgDisplay, window: u64) -> i32 |state| {
        state.backend().move_window(window)?;
        Ok(NHG_OK)
    }
}

nhg_export! {
    /// Start an interactive resize of `window` from `edges` (`NHSHELL_EDGE_*`) with the latest input serial.
    ///
    /// # Safety
    /// `d` is null or a live display.
    fn nhg_window_resize(d: *mut NhgDisplay, window: u64, edges: u32) -> i32 |state| {
        state.backend().resize(window, edges)?;
        Ok(NHG_OK)
    }
}

nhg_export! {
    /// Show the window menu of `window` at `x`, `y` (logical pixels in its surface) with the latest input serial.
    ///
    /// # Safety
    /// `d` is null or a live display.
    fn nhg_window_show_menu(d: *mut NhgDisplay, window: u64, x: i32, y: i32) -> i32 |state| {
        state.backend().show_menu(window, x, y)?;
        Ok(NHG_OK)
    }
}

nhg_export! {
    /// The pointer shape over `window` (`NHSHELL_CURSOR_*`).
    ///
    /// # Safety
    /// `d` is null or a live display.
    fn nhg_window_set_cursor(d: *mut NhgDisplay, window: u64, shape: u32) -> i32 |state| {
        state.backend().set_cursor(window, shape)?;
        Ok(NHG_OK)
    }
}

nhg_export! {
    /// Ask the compositor to activate `window` with `token` (null: one the shell requests).
    ///
    /// # Safety
    /// `d` is null or a live display; `token` is null or NUL-terminated.
    fn nhg_window_activate(d: *mut NhgDisplay, window: u64, token: *const c_char) -> i32 |state| {
        // SAFETY: null or NUL-terminated, valid for the call (the caller's promise).
        let token = if token.is_null() { None } else { Some(unsafe { c_text(token) }?) };
        state.backend().window_activate(window, token)?;
        Ok(NHG_OK)
    }
}

nhg_export! {
    /// The Xcursor theme `name` and logical `size`, for a compositor without cursor shapes.
    ///
    /// # Safety
    /// `d` is null or a live display; `name` is NUL-terminated.
    fn nhg_set_cursor_theme(d: *mut NhgDisplay, name: *const c_char, size: u32) -> i32 |state| {
        // SAFETY: null or NUL-terminated, valid for the call (the caller's promise).
        let name = unsafe { c_text(name) }?;
        state.backend().set_cursor_theme(name, size)?;
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
    /// The native size `*w` x `*h` the frame of `window` asks for, clamped in place so that its surface stays within
    /// 16384 device pixels a side (`csd::clamp_native`; an unknown window, or 0, has no border).  Neither reads nor
    /// writes pixels.
    ///
    /// # Safety
    /// `d` is null or a live display; `w` and `h` are null or readable and writable.
    fn nhg_frame_clamp(d: *mut NhgDisplay, window: u64, w: *mut i32, h: *mut i32) -> i32 |state| {
        if w.is_null() || h.is_null() {
            return Err(GlueError::Arg);
        }
        // SAFETY: non-null, readable and writable (the caller's promise).
        unsafe {
            let (cw, ch) = crate::csd::clamp_native(state, window, w.read(), h.read());
            w.write(cw);
            h.write(ch);
        }
        Ok(NHG_OK)
    }
}

nhg_export! {
    /// The compositor's size `lw` x `lh` (logical) for `window`, whose frame has the native size `native_w` x
    /// `native_h` and C's scale `scale120` and `states`, with `flags` (`NHG_FIT_*`, `csd::fit`): 1 with the native
    /// size the frame takes in `*w`, `*h` (recorded as `nhg_frame_resized` records it; C changes the frame's size), 0
    /// when it keeps its size.  Neither reads nor writes pixels.
    ///
    /// # Safety
    /// `d` is null or a live display; `w` and `h` are null or writable.
    fn nhg_frame_fit(d: *mut NhgDisplay, window: u64, scale120: u32, lw: i32, lh: i32, states: u32, native_w: i32, native_h: i32, flags: u32, w: *mut i32, h: *mut i32) -> i32 |state| {
        if w.is_null() || h.is_null() {
            return Err(GlueError::Arg);
        }
        let Some((fw, fh)) = crate::csd::fit(state, window, scale120, lw, lh, states, (native_w, native_h), flags)? else {
            return Ok(0);
        };
        // SAFETY: non-null and writable (the caller's promise).
        unsafe {
            w.write(fw);
            h.write(fh);
        }
        Ok(1)
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
    /// `window`'s chrome slot vector as `glitnir--chrome-vector` gave it now: `NHG_E_ARG` for a wrong length or a slot
    /// out of range; 1 when the corners or shadow changed (C lays the frame out again and garbages it), 0 when not.
    /// New transitions go to the shell at once.  A top-level's vector (`toplevel-chrome`) is kept for its margins and
    /// layer (1 only while it is framed); a tip's or a popup's is ignored.  Neither reads nor writes pixels.
    ///
    /// # Safety
    /// `d` is null or a live display; `v` is null or points to `n` readable floats.
    fn nhg_child_chrome(d: *mut NhgDisplay, window: u64, v: *const f32, n: usize) -> i32 |state| {
        if v.is_null() {
            return Err(GlueError::Arg);
        }
        // SAFETY: `n` readable floats (the caller's promise).
        let v = unsafe { std::slice::from_raw_parts(v, n) };
        Ok(i32::from(crate::child::chrome(state, window, v)?))
    }
}

nhg_export! {
    /// `window`'s border as C painted it now, `width` device pixels (0: none) in premultiplied `argb`: 1 when it
    /// changed (the chrome's corners keep it as a ring from the next `nhg_layer_ensure`), 0 when not.  Neither reads
    /// nor writes pixels.
    ///
    /// # Safety
    /// `d` is null or a live display.
    fn nhg_child_border(d: *mut NhgDisplay, window: u64, width: i32, argb: u32) -> i32 |state| {
        Ok(i32::from(crate::child::border(state, window, width, argb)?))
    }
}

nhg_export! {
    /// `window` was raised (`raise` nonzero) or lowered among its siblings, with `z_group`, the frame's z-group
    /// (`child::raise`).  Neither reads nor writes pixels.
    ///
    /// # Safety
    /// `d` is null or a live display.
    fn nhg_child_raise(d: *mut NhgDisplay, window: u64, raise: i32, z_group: i32) -> i32 |state| {
        crate::child::raise(state, window, raise != 0, z_group)?;
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
    /// Emacs window `w_seq`'s rows `run` (device pixels, with the native area's origin: its columns, the run's first
    /// row and its height) move to `to_y` in the backing, and Glitnir damages the move.  `rect` is the window
    /// (`window_box (w, ANY_AREA)`, with the native area's origin): a full-window scroll on an opaque root makes the window its own output first, a changed rectangle or a frame no
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
    /// Paint the `n` draw records at `recs` (Naglfar's `nhmacdraw.c`) into frame `frame`'s backing, each as it comes
    /// (device pixels with the native area's origin).  A record the glue refuses is skipped and the call returns
    /// `NHG_E_ARG` after the rest are painted.  `NHG_E_UNSUPPORTED` where the shell has no painter.  Writes pixels.
    ///
    /// # Safety
    /// `d` is null or a live display; `recs` holds `n` readable records, each `NHG_DRAW_BITMAP` one's `bits` its
    /// `r.h` readable rows.
    fn nhg_draw(d: *mut NhgDisplay, frame: u64, recs: *const crate::draw::NhgDrawRec, n: usize) -> i32 |state| {
        if n == 0 {
            return Ok(NHG_OK);
        }
        if recs.is_null() {
            return Err(GlueError::Arg);
        }
        // SAFETY: non-null, with `n` readable records (the caller's promise).
        let recs = unsafe { std::slice::from_raw_parts(recs, n) };
        let f = state.backings.frame_mut(frame)?;
        let mut bad = false;
        for r in recs {
            let bits: &[u16] = if r.kind == crate::draw::NHG_DRAW_BITMAP && !r.bits.is_null() && r.r.h > 0 {
                // SAFETY: `r.h` readable rows (the caller's promise).
                unsafe { std::slice::from_raw_parts(r.bits, r.r.h as usize) }
            } else {
                &[]
            };
            match crate::draw::decode(r, bits) {
                Ok(mut p) => f.paint(&mut p)?,
                Err(_) => bad = true,
            }
        }
        if bad { Err(GlueError::Arg) } else { Ok(NHG_OK) }
    }
}

nhg_export! {
    /// Paint the glyph run `rec` (`NHG_DRAW_GLYPHS`; its `bits` holds `n` glyphs) of the open font `font` into frame
    /// `frame`'s backing, each glyph placed as the font lays it out.  `NHG_E_ARG` for a record the glue refuses,
    /// `NHG_E_STATE` for a font that is not open, `NHG_E_UNSUPPORTED` where the shell has no painter.  Writes pixels.
    ///
    /// # Safety
    /// `d` is null or a live display; `rec` is readable and its `bits` holds `n` readable glyphs (null when `n` is 0).
    fn nhg_draw_glyphs(d: *mut NhgDisplay, frame: u64, font: u64, rec: *const crate::draw::NhgDrawRec, n: usize) -> i32 |state| {
        if rec.is_null() {
            return Err(GlueError::Arg);
        }
        // SAFETY: non-null and readable (the caller's promise).
        let rec = unsafe { &*rec };
        let glyphs: &[u16] = match (n, rec.bits.is_null()) {
            (0, _) => &[],
            (_, true) => return Err(GlueError::Arg),
            // SAFETY: `n` readable glyphs (the caller's promise).
            _ => unsafe { std::slice::from_raw_parts(rec.bits, n) },
        };
        let padding = rec.flags & crate::draw::NHG_DRAW_PADDING != 0;
        let (xs, integral) = crate::font::run(font, glyphs, padding).ok_or(GlueError::State)?;
        let mut p = crate::draw::decode_glyphs(rec, font, glyphs, xs, integral).map_err(|_| GlueError::Arg)?;
        state.backings.frame_mut(frame)?.paint(&mut p)?;
        Ok(NHG_OK)
    }
}

nhg_export! {
    /// Paint the image record `rec` (`NHG_DRAW_IMAGE`) of the container `pix` into frame `frame`'s backing.  The
    /// container is wrapped as a `CGImage` the first time it is drawn and kept until `nhg_image_forget` names its pixels
    /// or its mask.  `NHG_E_ARG` for a record or container the glue refuses, `NHG_E_UNSUPPORTED` where the shell has no
    /// painter.  Writes pixels.
    ///
    /// # Safety
    /// `d` is null or a live display; `rec` and `pix` are readable, and `pix`'s `data` (and `mask` when not null) hold
    /// its rows.
    fn nhg_draw_image(d: *mut NhgDisplay, frame: u64, rec: *const crate::draw::NhgDrawRec, pix: *const crate::image::NhgPix) -> i32 |state| {
        if rec.is_null() || pix.is_null() {
            return Err(GlueError::Arg);
        }
        // SAFETY: non-null and readable (the caller's promise).
        let (rec, pix) = unsafe { (&*rec, *pix) };
        let bytes = if pix.bpp == 8 { 1 } else { 4 };
        if pix.data.is_null() || pix.width <= 0 || pix.height <= 0 || pix.stride < pix.width * bytes {
            return Err(GlueError::Arg);
        }
        let (w, h) = (pix.width as usize, pix.height as usize);
        // SAFETY: the container's rows (the caller's promise).
        let data = unsafe { std::slice::from_raw_parts(pix.data, pix.stride as usize * (h - 1) + w * bytes as usize) };
        let mask = (bytes == 4 && !pix.mask.is_null() && pix.mask_stride >= pix.width)
            // SAFETY: the mask's rows (the caller's promise).
            .then(|| unsafe { std::slice::from_raw_parts(pix.mask, pix.mask_stride as usize * (h - 1) + w) });
        let key = (pix.data as usize, pix.width, pix.height);
        // what the picture is made with: the mask, or an 8-bit container's colour
        let made_with = if bytes == 1 { !(rec.fg as usize) } else { mask.map_or(0, |m| m.as_ptr() as usize) };
        #[cfg(all(feature = "macos", target_os = "macos"))]
        nhshell::macos::images::with_picture(
            key,
            made_with,
            || crate::image::combine(&pix, data, mask, rec.fg).unwrap_or_default(),
            |_| (),
        )
        .ok_or(GlueError::Arg)?;
        #[cfg(not(all(feature = "macos", target_os = "macos")))]
        let _ = (data, mask);
        let mut p = crate::draw::decode_image(rec, key, made_with, &pix.matrix, pix.smooth != 0).map_err(|_| GlueError::Arg)?;
        state.backings.frame_mut(frame)?.paint(&mut p)?;
        Ok(NHG_OK)
    }
}

// ---- Images (`image.rs`; macOS) ---------------------------------------------------------------------------------

/// An image's bytes: the `len` bytes at `src`, or the file they name when `is_file`.  `State` when it cannot be read.
///
/// # Safety
/// `src` holds `len` readable bytes for `'a`.
unsafe fn image_source<'a>(
    src: *const u8,
    len: usize,
    is_file: u32,
) -> Result<std::borrow::Cow<'a, [u8]>, GlueError> {
    // SAFETY: the caller's promise.
    let src = unsafe { std::slice::from_raw_parts(src, len) };
    if is_file == 0 {
        return Ok(std::borrow::Cow::Borrowed(src));
    }
    use std::os::unix::ffi::OsStrExt;
    std::fs::read(std::ffi::OsStr::from_bytes(src))
        .map(std::borrow::Cow::Owned)
        .map_err(|_| GlueError::State)
}

/// The file an SVG was read from (for its relative references), when `is_file`.
///
/// # Safety
/// `src` holds `len` readable bytes.
#[cfg(feature = "svg")]
unsafe fn image_file(src: *const u8, len: usize, is_file: u32) -> Option<std::path::PathBuf> {
    use std::os::unix::ffi::OsStrExt;
    // SAFETY: the caller's promise.
    (is_file != 0).then(|| {
        std::ffi::OsStr::from_bytes(unsafe { std::slice::from_raw_parts(src, len) }).into()
    })
}

nhg_export! {
    /// The SVG in the `len` bytes at `src` (a file name when `is_file`), read with `style`: its own size in pixels into
    /// `*w`, `*h`.  `NHG_E_STATE` when it cannot be read or parsed, `NHG_E_UNSUPPORTED` in a build without SVG.
    ///
    /// # Safety
    /// `src` holds `len` readable bytes; `style` is readable and its strings hold their lengths; `w` and `h` are
    /// writable.
    unlocked fn nhg_svg_size(src: *const u8, len: usize, is_file: u32, style: *const crate::image::NhgSvgStyle, w: *mut f64, h: *mut f64) -> i32 {
        if src.is_null() || style.is_null() || w.is_null() || h.is_null() {
            return Err(GlueError::Arg);
        }
        #[cfg(feature = "svg")]
        {
            // SAFETY: the caller's promises.
            let (bytes, file) = unsafe { (image_source(src, len, is_file)?, image_file(src, len, is_file)) };
            // SAFETY: as above.
            let style = unsafe { crate::image::svg_style(&*style, file.as_deref()) };
            let (sw, sh) = nhshell::svg::size(&bytes, &style).ok_or(GlueError::State)?;
            // SAFETY: writable (the caller's promise).
            unsafe { (w.write(sw), h.write(sh)) };
            Ok(NHG_OK)
        }
        #[cfg(not(feature = "svg"))]
        {
            let _ = (len, is_file);
            Err(GlueError::Code(crate::abi::NHG_E_UNSUPPORTED))
        }
    }
}

nhg_export! {
    /// The SVG in the `len` bytes at `src` (a file name when `is_file`) drawn with `style` at `width` x `height` over
    /// its background into `*out` (opaque; `nhg_image_take` or `nhg_image_free` frees it).  `NHG_E_STATE` when it
    /// cannot be read, parsed or drawn, `NHG_E_UNSUPPORTED` in a build without SVG.
    ///
    /// # Safety
    /// As `nhg_svg_size`; `out` is writable.
    unlocked fn nhg_svg_render(src: *const u8, len: usize, is_file: u32, style: *const crate::image::NhgSvgStyle, width: i32, height: i32, out: *mut crate::image::NhgImage) -> i32 {
        if src.is_null() || style.is_null() || out.is_null() || width <= 0 || height <= 0 {
            return Err(GlueError::Arg);
        }
        #[cfg(feature = "svg")]
        {
            // SAFETY: the caller's promises.
            let (bytes, file) = unsafe { (image_source(src, len, is_file)?, image_file(src, len, is_file)) };
            // SAFETY: as above.
            let (s, bg) = unsafe { (crate::image::svg_style(&*style, file.as_deref()), (*style).background) };
            let (w, h) = (width as u32, height as u32);
            let argb = nhshell::svg::render(&bytes, &s, bg, w, h).ok_or(GlueError::State)?;
            // SAFETY: writable (the caller's promise).
            unsafe { out.write(crate::image::opaque(w, h, argb)) };
            Ok(NHG_OK)
        }
        #[cfg(not(feature = "svg"))]
        {
            let _ = (len, is_file);
            Err(GlueError::Code(crate::abi::NHG_E_UNSUPPORTED))
        }
    }
}

nhg_export! {
    /// `NHG_OK` when `image.c`'s native route takes the image type named by the `len` bytes at `name`, else
    /// `NHG_E_UNSUPPORTED`.  Neither reads nor writes pixels.
    ///
    /// # Safety
    /// `name` is null or holds `len` readable bytes.
    unlocked fn nhg_image_type(name: *const c_char, len: usize) -> i32 {
        if name.is_null() {
            return Err(GlueError::Arg);
        }
        // SAFETY: `len` readable bytes (the caller's promise).
        let name = unsafe { std::slice::from_raw_parts(name.cast::<u8>(), len) };
        match std::str::from_utf8(name) {
            Ok(n) if cfg!(all(feature = "macos", target_os = "macos")) && crate::image::supported(n) => Ok(NHG_OK),
            _ => Err(GlueError::Code(crate::abi::NHG_E_UNSUPPORTED)),
        }
    }
}

nhg_export! {
    /// Decode frame `index` of the image in the `len` bytes at `src` (a file name when `is_file`) into `*out`, whose
    /// pixels `nhg_image_take` or `nhg_image_free` frees.  An XPM's colour names go to `color` (with `ctx`), which
    /// gives `0xRRGGBB`.  `NHG_E_STATE` when the file cannot be read or decoded or has no such frame.
    ///
    /// # Safety
    /// `src` holds `len` readable bytes; `out` is writable; `color` is safe to call with `ctx` and a NUL-terminated
    /// name, on this thread.
    unlocked fn nhg_image_decode(src: *const u8, len: usize, is_file: u32, index: u32, color: crate::image::NhgColorFn, ctx: *mut std::ffi::c_void, out: *mut crate::image::NhgImage) -> i32 {
        if src.is_null() || out.is_null() {
            return Err(GlueError::Arg);
        }
        // SAFETY: `len` readable bytes (the caller's promise).
        let bytes = unsafe { image_source(src, len, is_file) }?;
        let mut resolve = |name: &str| {
            let name = std::ffi::CString::new(name).unwrap_or_default();
            // SAFETY: the caller's promise; the name lives across the call.
            color.map_or(0, |f| unsafe { f(ctx, name.as_ptr()) })
        };
        let img = crate::image::decode(&bytes, index, &mut resolve).ok_or(GlueError::State)?;
        // SAFETY: writable (the caller's promise).
        unsafe { out.write(img) };
        Ok(NHG_OK)
    }
}

nhg_export! {
    /// Copy `img`'s pixels into the container bytes at `data` (`width * 4` a row) and, when it has alpha, its alpha
    /// into `mask` (`width` a row; may be null otherwise), then free them.
    ///
    /// # Safety
    /// `img` came from `nhg_image_decode` and is not used after; `data` (and `mask`) hold the rows.
    unlocked fn nhg_image_take(img: *mut crate::image::NhgImage, data: *mut u8, mask: *mut u8) -> i32 {
        if img.is_null() || data.is_null() {
            return Err(GlueError::Arg);
        }
        // SAFETY: a live decode (the caller's promise).
        let img = unsafe { &mut *img };
        if !img.argb.is_null() {
            // SAFETY: the decode's pixels, and containers of its size (the caller's promise).
            let (argb, out, m) = unsafe {
                (
                    std::slice::from_raw_parts(img.argb, img.len),
                    std::slice::from_raw_parts_mut(data, img.len * 4),
                    (img.has_alpha != 0 && !mask.is_null()).then(|| std::slice::from_raw_parts_mut(mask, img.len)),
                )
            };
            crate::image::store(argb, out, m);
        }
        // SAFETY: freed once, here.
        unsafe { crate::image::free(img) };
        Ok(NHG_OK)
    }
}

nhg_export! {
    /// Free a decode's pixels without taking them.
    ///
    /// # Safety
    /// `img` is null or came from `nhg_image_decode` and is not used after.
    unlocked fn nhg_image_free(img: *mut crate::image::NhgImage) -> i32 {
        if !img.is_null() {
            // SAFETY: a live decode, freed once (the caller's promise).
            unsafe { crate::image::free(&mut *img) };
        }
        Ok(NHG_OK)
    }
}

nhg_export! {
    /// `image_set_transform`'s matrix for an image `w` x `h` shown at `width` x `height`, turned by `rotation`
    /// degrees and flipped: the six numbers into `out` (Cairo's pattern matrix, from a drawn pixel to the container's)
    /// and the drawn size into `*out_w`, `*out_h`.  `NHG_E_UNSUPPORTED` for a rotation that is not a multiple of 90
    /// degrees: `out` then holds the scale alone and the size is left as it was.
    ///
    /// # Safety
    /// `out` holds six writable doubles; `out_w` and `out_h` are writable.
    unlocked fn nhg_image_transform(w: i32, h: i32, width: i32, height: i32, rotation: f64, flip: bool, out: *mut f64, out_w: *mut i32, out_h: *mut i32) -> i32 {
        if out.is_null() || out_w.is_null() || out_h.is_null() || width <= 0 || height <= 0 {
            return Err(GlueError::Arg);
        }
        let t = crate::image::transform(f64::from(w), f64::from(h), f64::from(width), f64::from(height), rotation, flip);
        let m = match t {
            Ok((m, ..)) | Err(m) => m,
        };
        // SAFETY: six writable doubles and two ints (the caller's promise).
        unsafe {
            std::ptr::copy_nonoverlapping(m.as_ptr(), out, 6);
            if let Ok((_, dw, dh)) = t {
                *out_w = dw.round() as i32;
                *out_h = dh.round() as i32;
            }
        }
        t.map(|_| NHG_OK).map_err(|_| GlueError::Code(crate::abi::NHG_E_UNSUPPORTED))
    }
}

nhg_export! {
    /// The container bytes at `addr` are being freed: forget the pictures made from them (as pixels or as a mask).
    ///
    /// # Safety
    /// None beyond the C ABI: the address is only compared.
    unlocked fn nhg_image_forget(addr: *const u8) -> i32 {
        #[cfg(all(feature = "macos", target_os = "macos"))]
        nhshell::macos::images::forget(addr as usize);
        #[cfg(not(all(feature = "macos", target_os = "macos")))]
        let _ = addr;
        Ok(NHG_OK)
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
    /// How many of the frame's Emacs windows are their own outputs, plus how many have a card layer (0 for a frame
    /// without a backing).
    ///
    /// # Safety
    /// `d` is null or a live display.
    fn nhg_layers_count(d: *mut NhgDisplay, frame: u64) -> i32 |state| {
        let n = crate::layers::count(&mut state.backings, frame);
        Ok(i32::try_from(n).unwrap_or(i32::MAX))
    }
}

nhg_export! {
    /// Emacs window `w_seq` of the frame (shell window `window`, a top-level; key `frame`) has the layout leg vector
    /// `v` (`n` floats, `glitnir--leg-vector`; 0 for none): the glue asks the shell for its capture and its leg
    /// (`legs.rs`), and writes the number of the newest capture the presenter took of the window to `*captured`.
    ///
    /// # Safety
    /// `d` is null or a live display; `v` holds `n` readable floats, or is null when `n` is 0; `captured` is null or
    /// writable.
    fn nhg_layout_leg(d: *mut NhgDisplay, window: u64, frame: u64, w_seq: u64, v: *const f32, n: usize, captured: *mut u64) -> i32 |state| {
        let v = match (n, v.is_null()) {
            (0, _) => &[][..],
            (_, true) => return Err(GlueError::Arg),
            // SAFETY: non-null, with `n` readable floats (the caller's promise).
            _ => unsafe { std::slice::from_raw_parts(v, n) },
        };
        let took = crate::legs::apply(state, window, frame, w_seq, v)?;
        if !captured.is_null() {
            // SAFETY: non-null and writable (the caller's promise).
            unsafe { *captured = took };
        }
        Ok(0)
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

nhg_export! {
    /// Where our drag stands: the phase (`NHG_DRAG_PHASE_*`), and once done `*action` the action performed (0: none)
    /// and `*frame` the window whose frame the call returns (0: none).  Neither reads nor writes pixels.
    ///
    /// # Safety
    /// `d` is null or a live display; `action` and `frame` are null or writable.
    fn nhg_drag_state(d: *mut NhgDisplay, action: *mut u32, frame: *mut u64) -> i32 |state| {
        let (phase, act, win) = crate::dnd::drag_state(state);
        // SAFETY: each is null or writable (the caller's promise).
        unsafe {
            if !action.is_null() {
                *action = act;
            }
            if !frame.is_null() {
                *frame = win;
            }
        }
        Ok(phase)
    }
}

// ---- Giving the display up -----------------------------------------------------------------------------------------

nhg_export! {
    /// The display is given up (the compositor went and is not waited for): what the glue held for the connection goes
    /// (`Display::give_up`), before C deletes the display's frames.  Neither reads nor writes pixels.
    ///
    /// # Safety
    /// `d` is null or a live display.
    fn nhg_give_up(d: *mut NhgDisplay) -> i32 |state| {
        state.give_up();
        Ok(NHG_OK)
    }
}

nhg_export! {
    /// Why the connection ended: a protocol error's text (`INTERFACE@ID: error CODE: MESSAGE`), up to `cap` - 1 bytes
    /// and a NUL into `buf`, its full length into `*len` (0: the connection lives, or ended with a plain hangup).
    ///
    /// # Safety
    /// `d` is null or a live display; `buf` is null or holds `cap` writable bytes; `len` is null or writable.
    fn nhg_last_error(d: *mut NhgDisplay, buf: *mut c_char, cap: usize, len: *mut usize) -> i32 |state| {
        let text = state.backend().last_error();
        // SAFETY: `buf` is null or holds `cap` writable bytes.
        unsafe { put_text(buf, cap, &text) };
        if !len.is_null() {
            // SAFETY: non-null and writable (the caller's promise).
            unsafe { len.write(text.len()) };
        }
        Ok(NHG_OK)
    }
}

/// A NUL-terminated UTF-8 argument (`Arg` for null or invalid UTF-8).
///
/// # Safety
/// `p` is null or NUL-terminated.
unsafe fn str_arg<'a>(p: *const c_char) -> Result<&'a str, GlueError> {
    if p.is_null() {
        return Err(GlueError::Arg);
    }
    // SAFETY: non-null and NUL-terminated (the caller's promise).
    unsafe { std::ffi::CStr::from_ptr(p) }
        .to_str()
        .map_err(|_| GlueError::Arg)
}

nhg_export! {
    /// A platform call by name (`nhmac--call`): `kind` and `arg` are NUL-terminated UTF-8.  `NHG_E_ARG` for a name the
    /// shell does not know, `NHG_E_UNSUPPORTED` for a shell with no platform calls.
    ///
    /// # Safety
    /// `d` is null or a live display; `kind` and `arg` are null or NUL-terminated.
    fn nhg_app_call(d: *mut NhgDisplay, kind: *const c_char, arg: *const c_char) -> i32 |state| {
        // SAFETY: null or NUL-terminated (the caller's promise).
        let (kind, arg) = unsafe { (str_arg(kind)?, str_arg(arg)?) };
        state.backend().app_call(kind, arg)?;
        Ok(NHG_OK)
    }
}

nhg_export! {
    /// A platform query by name (`nhmac--query`): its answer, up to `cap` - 1 bytes and a NUL, into `buf`, its full
    /// length into `*len`.  It may run a modal session.
    ///
    /// # Safety
    /// `d` is null or a live display; `kind` and `arg` are null or NUL-terminated; `buf` is null or holds `cap` writable
    /// bytes; `len` is null or writable.
    fn nhg_app_query(d: *mut NhgDisplay, kind: *const c_char, arg: *const c_char, buf: *mut c_char, cap: usize,
                     len: *mut usize) -> i32 |state| {
        // SAFETY: null or NUL-terminated (the caller's promise).
        let (kind, arg) = unsafe { (str_arg(kind)?, str_arg(arg)?) };
        let text = state.backend().app_query(kind, arg)?;
        // SAFETY: `buf` is null or holds `cap` writable bytes.
        unsafe { put_text(buf, cap, &text) };
        if !len.is_null() {
            // SAFETY: non-null and writable (the caller's promise).
            unsafe { len.write(text.len()) };
        }
        Ok(NHG_OK)
    }
}

// ---- Small pure functions moved from C: colours, a monitor's place, the snapshot's copy ----------------------------

nhg_export! {
    /// The title bar's mixed colours (`csd::title_bar_mix`) from the frame's foreground `fg` and background `bg`
    /// (0xRRGGBB): the inactive foreground into `*out_fg`, the background into `*out_bg`.  Neither reads nor writes
    /// pixels.
    ///
    /// # Safety
    /// `out_fg` and `out_bg` are null or writable.
    unlocked fn nhg_csd_mix(fg: u32, bg: u32, out_fg: *mut u32, out_bg: *mut u32) -> i32 {
        if out_fg.is_null() || out_bg.is_null() {
            return Err(GlueError::Arg);
        }
        let (f, b) = crate::csd::title_bar_mix(fg, bg);
        // SAFETY: non-null and writable (the caller's promise).
        unsafe {
            out_fg.write(f);
            out_bg.write(b);
        }
        Ok(NHG_OK)
    }
}

nhg_export! {
    /// `rgb` (0xRRGGBB) at `alpha` as a backing stores it: premultiplied ARGB, as Cairo paints it
    /// (`backing::premultiply`), into `*out`.  Neither reads nor writes pixels.
    ///
    /// # Safety
    /// `out` is null or writable.
    unlocked fn nhg_premultiply(rgb: u32, alpha: f64, out: *mut u32) -> i32 {
        if out.is_null() {
            return Err(GlueError::Arg);
        }
        // SAFETY: non-null and writable (the caller's promise).
        unsafe { out.write(crate::backing::premultiply(rgb, alpha)) };
        Ok(NHG_OK)
    }
}

nhg_export! {
    /// The tree of `s`'s first surface as the compositor was given it, its popups on top, into `dst`, rows `stride`
    /// bytes apart: `s` holds trees as `nhg_present` takes them, one after another, `composed` false leaves Glitnir's
    /// chrome and scenes out, and `w` x `h` is the size `nhg_snapshot` gave for the root.  `NHG_E_STATE` without a root
    /// of that size that may show a buffer.  Reads
    /// pixels.
    ///
    /// # Safety
    /// `d` is null or a live display; `s` holds `n` readable surfaces; `dst` is null or holds `stride` x (`h` - 1) +
    /// 4 x `w` writable bytes.
    fn nhg_tree_snapshot(d: *mut NhgDisplay, s: *const NhgSurface, n: usize, composed: bool, dst: *mut u8, stride: i32, w: i32, h: i32) -> i32 |state| {
        let (Ok(su), Ok(wu), Ok(hu)) = (usize::try_from(stride), usize::try_from(w), usize::try_from(h)) else {
            return Err(GlueError::Arg);
        };
        if dst.is_null() || s.is_null() || n == 0 {
            return Err(GlueError::Arg);
        }
        let bytes = match hu {
            0 => 0,
            _ => (hu - 1)
                .checked_mul(su)
                .and_then(|n| n.checked_add(wu.checked_mul(4)?))
                .ok_or(GlueError::Arg)?,
        };
        // SAFETY: non-null, with `n` readable surfaces (the caller's promise).
        let list = unsafe { std::slice::from_raw_parts(s, n) };
        let (backend, backings, _) = state.presenting();
        let picture = present::tree_snapshot(backend, backings, list, composed, w, h)?;
        // SAFETY: `dst` holds `bytes` writable bytes (the caller's promise), not the backing's.
        let out = unsafe { std::slice::from_raw_parts_mut(dst, bytes) };
        crate::backing::copy_rows(&picture, wu, hu, out, su)?;
        Ok(NHG_OK)
    }
}

nhg_export! {
    /// The monitors `window` is on as a mask of their places in `nhg_monitors`' list (bit J for the Jth; the first
    /// when the compositor named none), into `*out`.  `NHG_E_STATE` for an unknown window.  Neither reads nor writes
    /// pixels.
    ///
    /// # Safety
    /// `d` is null or a live display; `out` is null or writable.
    fn nhg_frame_monitors(d: *mut NhgDisplay, window: u64, out: *mut u32) -> i32 |state| {
        if out.is_null() {
            return Err(GlueError::Arg);
        }
        let mask = crate::monitors::frame_monitors(state, window)?;
        // SAFETY: non-null and writable (the caller's promise).
        unsafe { out.write(mask) };
        Ok(NHG_OK)
    }
}

nhg_export! {
    /// A monitor's position in device pixels (`monitors::device_origin`) from its logical `x`, `y` and `scale120`,
    /// into `*dx`, `*dy`.  Neither reads nor writes pixels.
    ///
    /// # Safety
    /// `dx` and `dy` are null or writable.
    unlocked fn nhg_monitor_origin(x: i32, y: i32, scale120: u32, dx: *mut i32, dy: *mut i32) -> i32 {
        if dx.is_null() || dy.is_null() {
            return Err(GlueError::Arg);
        }
        let (ox, oy) = crate::monitors::device_origin(x, y, scale120);
        // SAFETY: non-null and writable (the caller's promise).
        unsafe {
            dx.write(ox);
            dy.write(oy);
        }
        Ok(NHG_OK)
    }
}

nhg_export! {
    /// What changing frame parameter `key` (`NHG_PK_*`) to the `n` values `v` does on a frame with the facts `f`
    /// (`params::decide`), into `*out`.  A status in `out` is a signal for C to raise.  Neither reads nor writes pixels.
    ///
    /// # Safety
    /// `f` and `out` are null or valid; `v` is null or holds `n` values.
    unlocked fn nhg_frame_param(
        key: u32,
        f: *const crate::params::NhgParamFrame,
        v: *const crate::params::NhgParamValue,
        n: usize,
        out: *mut crate::params::NhgParamAction,
    ) -> i32 {
        if f.is_null() || out.is_null() || (v.is_null() && n > 0) {
            return Err(GlueError::Arg);
        }
        // SAFETY: non-null, and `v` holds `n` values (the caller's promise); `n` is 0 only when `v` may be null.
        let (f, v) = unsafe { (&*f, if n == 0 { &[][..] } else { std::slice::from_raw_parts(v, n) }) };
        // SAFETY: non-null and writable (the caller's promise).
        unsafe { out.write(crate::params::decide(key, f, v)) };
        Ok(NHG_OK)
    }
}

nhg_export! {
    /// The id of the name at `s` (`len` bytes, not NUL-terminated) into `*id`, made on the first call and kept for the
    /// life of the process, for the font records.  0 for an empty name or bytes that are not UTF-8.
    ///
    /// # Safety
    /// `s` is null or holds `len` readable bytes; `id` is null or writable.
    unlocked fn nhg_font_id(s: *const c_char, len: usize, id: *mut u32) -> i32 {
        if s.is_null() || id.is_null() {
            return Err(GlueError::Arg);
        }
        // SAFETY: non-null and writable (the caller's promise).
        unsafe { id.write(crate::fontspec::intern_raw(s, len)) };
        Ok(NHG_OK)
    }
}

nhg_export! {
    /// The name of font record id `id` into `buf` (`cap` bytes, NUL-terminated); its length into `*len` (may be NULL).
    /// `NHG_E_TOO_SMALL` when `cap` is not above the length.
    ///
    /// # Safety
    /// `buf` is null or holds `cap` writable bytes; `len` is null or writable.
    unlocked fn nhg_font_name(id: u32, buf: *mut c_char, cap: usize, len: *mut usize) -> i32 {
        if crate::fontspec::name_raw(id, buf, cap, len) { Ok(NHG_OK) } else { Err(GlueError::Code(crate::abi::NHG_E_TOO_SMALL)) }
    }
}

nhg_export! {
    /// Every font that fits `spec` into `out` (`cap` records), the number written into `*n` (`macfont_list`'s
    /// matching, `fontspec.rs`).  `NHG_E_UNSUPPORTED` when no font source is installed (the Linux shell has none: it
    /// lists fonts with fontconfig).  Neither reads nor writes pixels.
    ///
    /// # Safety
    /// `spec` is null or valid; `out` is null or holds `cap` writable records; `n` is null or writable.
    unlocked fn nhg_font_list(spec: *const crate::fontspec::NhgFontSpec, out: *mut crate::fontspec::NhgFontEntity, cap: usize, n: *mut usize) -> i32 {
        font_query(spec, out, cap, n, true)
    }
}

nhg_export! {
    /// The font nearest to `spec` into `out[0]`, 1 or 0 into `*n` (`macfont_match`).  Same errors as `nhg_font_list`.
    ///
    /// # Safety
    /// As `nhg_font_list`, with `cap` at least 1.
    unlocked fn nhg_font_match(spec: *const crate::fontspec::NhgFontSpec, out: *mut crate::fontspec::NhgFontEntity, cap: usize, n: *mut usize) -> i32 {
        font_query(spec, out, cap, n, false)
    }
}

nhg_export! {
    /// Install the platform's font source for `nhg_font_list` and `nhg_font_match` (Core Text on macOS).
    /// `NHG_E_UNSUPPORTED` where there is none (Linux lists its fonts with fontconfig).
    ///
    /// # Safety
    /// None beyond the C ABI: it takes no pointers.
    unlocked fn nhg_font_start() -> i32 {
        if crate::font::start() { Ok(NHG_OK) } else { Err(GlueError::Shell(crate::abi::NHG_E_UNSUPPORTED)) }
    }
}

nhg_export! {
    /// Startup, before Lisp is initialised and without a display: `LANG` from the system's locale when the
    /// environment has none (macOS; `NHG_E_UNSUPPORTED` elsewhere).  Calls nothing in Emacs.  It keeps no pool: every
    /// entry point drains its own (`guard`), so a pool made here would be gone when this call returns.
    ///
    /// # Safety
    /// None beyond the C ABI: it takes no pointers.  Call it once, from Emacs's main thread, before any other thread.
    unlocked fn nhg_startup() -> i32 {
        if platform_startup() { Ok(NHG_OK) } else { Err(GlueError::Shell(crate::abi::NHG_E_UNSUPPORTED)) }
    }
}

nhg_export! {
    /// The configured path `epath` placed in the app bundle this executable is in (`ns_relocate`): when it is
    /// relative and exists there, the new path into `out` (`cap` bytes, NUL-terminated) and its length into `*len`;
    /// otherwise `*len` is 0 and `out` is untouched.  `NHG_E_TOO_SMALL` when `cap` is not above the length.
    ///
    /// # Safety
    /// `epath` is null or NUL-terminated; `out` is null or holds `cap` writable bytes; `len` is writable.
    unlocked fn nhg_relocate(epath: *const c_char, out: *mut c_char, cap: usize, len: *mut usize) -> i32 {
        if epath.is_null() || out.is_null() || len.is_null() {
            return Err(GlueError::Arg);
        }
        // SAFETY: non-null and NUL-terminated (the caller's promise).
        let path = unsafe { std::ffi::CStr::from_ptr(epath) }.to_str().map_err(|_| GlueError::Arg)?;
        let Some(moved) = relocate_path(path) else {
            // SAFETY: non-null and writable (the caller's promise).
            unsafe { len.write(0) };
            return Ok(NHG_OK);
        };
        if moved.len() >= cap {
            return Err(GlueError::Code(crate::abi::NHG_E_TOO_SMALL));
        }
        // SAFETY: `cap` bytes are writable, and `moved` plus its NUL fits.
        unsafe {
            ptr::copy_nonoverlapping(moved.as_ptr(), out.cast::<u8>(), moved.len());
            out.add(moved.len()).write(0);
            len.write(moved.len());
        }
        Ok(NHG_OK)
    }
}

/// The platform's startup; false where there is none.
fn platform_startup() -> bool {
    #[cfg(all(feature = "macos", target_os = "macos"))]
    {
        nhshell::macos::startup::init();
        true
    }
    #[cfg(not(all(feature = "macos", target_os = "macos")))]
    {
        false
    }
}

/// `epath` in the app bundle (macOS), `None` when it stays.
fn relocate_path(epath: &str) -> Option<String> {
    #[cfg(all(feature = "macos", target_os = "macos"))]
    return nhshell::macos::startup::relocate(epath);
    #[cfg(not(all(feature = "macos", target_os = "macos")))]
    {
        let _ = epath;
        None
    }
}

nhg_export! {
    /// Open the face `font_id` (an entity's) at `size` device pixels: its handle and metrics into `*out`, rounded as
    /// `macfont_open` rounds them.  `NHG_E_STATE` when the face does not open, `NHG_E_UNSUPPORTED` without Core Text.
    ///
    /// # Safety
    /// `out` is null or writable.
    unlocked fn nhg_font_open(font_id: u64, size: i32, out: *mut crate::font::NhgFontMetrics) -> i32 {
        if out.is_null() || size <= 0 {
            return Err(GlueError::Arg);
        }
        if !cfg!(all(feature = "macos", target_os = "macos")) {
            return Err(GlueError::Shell(crate::abi::NHG_E_UNSUPPORTED));
        }
        let m = crate::font::open(font_id, size).ok_or(GlueError::State)?;
        // SAFETY: non-null and writable (the caller's promise).
        unsafe { out.write(m) };
        Ok(NHG_OK)
    }
}

nhg_export! {
    /// Close a font `nhg_font_open` opened.
    ///
    /// # Safety
    /// None beyond the C ABI: it takes no pointers.
    unlocked fn nhg_font_close(handle: u64) -> i32 {
        crate::font::close(handle);
        Ok(NHG_OK)
    }
}

nhg_export! {
    /// Character `c`'s glyph in font `handle` into `*glyph`; `NHG_E_STATE` when the font has none.
    ///
    /// # Safety
    /// `glyph` is null or writable.
    unlocked fn nhg_font_glyph(handle: u64, c: u32, glyph: *mut u32) -> i32 {
        if glyph.is_null() {
            return Err(GlueError::Arg);
        }
        let g = crate::font::glyph(handle, c).ok_or(GlueError::State)?;
        // SAFETY: non-null and writable (the caller's promise).
        unsafe { glyph.write(g) };
        Ok(NHG_OK)
    }
}

nhg_export! {
    /// Glyph `glyph`'s metrics in font `handle` into `*out`, its width rounded as `macfont_glyph_extents` rounds it:
    /// `mono` and `space_width` are the font's, from `nhg_font_open`.  `NHG_E_STATE` for an unknown font or glyph.
    ///
    /// # Safety
    /// `out` is null or writable.
    unlocked fn nhg_font_glyph_metrics(handle: u64, glyph: u32, mono: u32, space_width: i32, out: *mut crate::font::NhgGlyphMetrics) -> i32 {
        if out.is_null() {
            return Err(GlueError::Arg);
        }
        let m = crate::font::glyph_extents(handle, glyph, mono != 0, space_width).ok_or(GlueError::State)?;
        // SAFETY: non-null and writable (the caller's promise).
        unsafe { out.write(m) };
        Ok(NHG_OK)
    }
}

/// The body of `nhg_font_list` and `nhg_font_match`.
fn font_query(
    spec: *const crate::fontspec::NhgFontSpec,
    out: *mut crate::fontspec::NhgFontEntity,
    cap: usize,
    n: *mut usize,
    all: bool,
) -> Result<i32, GlueError> {
    if spec.is_null() || n.is_null() || (out.is_null() && cap > 0) {
        return Err(GlueError::Arg);
    }
    // SAFETY: non-null and valid, and `out` holds `cap` records (the caller's promise).
    let (spec, out) = unsafe {
        (
            &*spec,
            if cap == 0 {
                &mut [][..]
            } else {
                std::slice::from_raw_parts_mut(out, cap)
            },
        )
    };
    let mut found = 0;
    let have = crate::fontspec::run(spec, all, out, &mut found);
    // SAFETY: non-null and writable (the caller's promise).
    unsafe { n.write(found) };
    if have {
        Ok(NHG_OK)
    } else {
        Err(GlueError::Shell(crate::abi::NHG_E_UNSUPPORTED))
    }
}

nhg_export! {
    /// The main wait (Naglfar): register the fds `rfds` and `wfds` ask for below `nfds`, run the backend's event loop
    /// until one is ready, a wake (`nhg_wake`), or `timeout_ns` (negative: none), and leave the ready ones in the sets
    /// as `select` does.  The count, as `select` returns it (0 after a wake or the timeout); `NHG_E_UNSUPPORTED` for a
    /// backend whose host waits in its own select.  The caller holds Emacs's global lock and is the main thread.
    ///
    /// # Safety
    /// `d` is null or a live display; each set is null or a valid, writable `fd_set`.
    fn nhg_wait(
        d: *mut NhgDisplay,
        nfds: i32,
        rfds: *mut libc::fd_set,
        wfds: *mut libc::fd_set,
        timeout_ns: i64,
    ) -> i32 |state| {
        // SAFETY: valid sets or null (the caller's promise).
        let want = unsafe { crate::wait::want(nfds, rfds, wfds) };
        let timeout = u64::try_from(timeout_ns).ok().map(std::time::Duration::from_nanos);
        let mut ready = Vec::new();
        state.backend().wait(&want, timeout, &mut ready)?;
        // SAFETY: as above.
        Ok(unsafe { crate::wait::fill(&ready, rfds, wfds) })
    }
}

nhg_export! {
    /// Any thread, no lock: end the main wait if it is parked, or make the next one return before it sleeps.
    ///
    /// # Safety
    /// None beyond the C ABI: it takes no pointers.
    unlocked fn nhg_wake() -> i32 {
        nhshell::wait::wake();
        Ok(NHG_OK)
    }
}

nhg_export! {
    /// Hand the wait the host's lock-waiter count, an `_Atomic int` that lives as long as the process: while it is
    /// above 0 the main wait does not park.  Once, at the first wait.
    ///
    /// # Safety
    /// `count` is null or points at a 4-byte-aligned `_Atomic int` (an `AtomicI32`) that is never freed.
    unlocked fn nhg_set_lock_waiters(count: *const std::ffi::c_void) -> i32 {
        if count.is_null() {
            return Err(GlueError::Arg);
        }
        // SAFETY: a live, never-freed atomic int (the caller's promise), which has AtomicI32's layout.
        nhshell::wait::PARK.set_waiters(unsafe { &*count.cast::<std::sync::atomic::AtomicI32>() });
        Ok(NHG_OK)
    }
}

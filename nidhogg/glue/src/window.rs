// Upstream commit: a360712c9d272d950d8d8255ef74570f7e90b7d9 (GNU Emacs emacs-31.1), for the "adapted from" notes below.
//! Window operations: a frame's shell window made and registered with the facts it starts from, its title,
//! decorations, alpha, size and smallest size, its visibility, iconifying, fullscreen, destroying it, and the bounded
//! wait for a top-level's first configure.  Moved from `src/nhwlterm.c` (`nhwl_window_new`, `nhwl_set_title_hint`,
//! `nhwl_set_undecorated_hint`, `nhwl_set_frame_alpha`, `nhwl_set_window_size`, `nhwl_set_min_size`,
//! `nhwl_wait_for_configure`, `nhwl_iconify_frame`, `nhwl_set_fullscreen_state`, `nhwl_window_destroy`).  What reads
//! Emacs stays there: the title's encoding, the `alpha` parameter with the highlight frame and
//! `frame-alpha-lower-limit`, the sizes from the frame's fonts and bars, the `fullscreen` parameter, `change_frame_size`
//! and the draining of events while it waits.
//!
//! A window's state is keyed by its shell window id (`frame.rs`).  Tip, popup and child frames get a new window each
//! time they are shown, so C gives every new window the facts it starts from (`NhgWindowDesc`), and its alpha starts
//! over with the window.

use crate::abi::NHG_E_TIMEOUT;
use crate::configure;
use crate::display::Display;
use crate::frame::{Frame, NHG_FACT_CSD, NHG_FACT_ICONIFIED, NHG_FACT_VISIBLE, NHG_FRAME_CHILD};
use crate::guard::GlueError;
use nhshell::api::{
    Anchor, ChildDesc, NHSHELL_KIND_CHILD, NHSHELL_KIND_TOPLEVEL, NHSHELL_WIN_PLACED, NHSHELL_WIN_RESTORE, Place,
    ShellError, WindowDesc,
};
use std::ffi::c_char;

/// Where a popup goes (the shell's `Anchor`: a rectangle in the parent's surface in logical pixels, a gravity and
/// what the compositor may adjust).
pub type NhgAnchor = Anchor;

/// The facts a new window may start from: Emacs's visibility and title bar.
pub const NHG_WINDOW_FACTS: u32 = NHG_FACT_VISIBLE | NHG_FACT_ICONIFIED | NHG_FACT_CSD;

/// `nhg_window_fullscreen` modes (Emacs's `fullscreen_type` values).
pub const NHG_FULLSCREEN_NONE: u32 = 0;
pub const NHG_FULLSCREEN_WIDTH: u32 = 1;
pub const NHG_FULLSCREEN_HEIGHT: u32 = 2;
pub const NHG_FULLSCREEN_BOTH: u32 = 3;
pub const NHG_FULLSCREEN_MAXIMIZED: u32 = 4;

/// `nhg_window_alpha` result bits: C damages the whole frame (the host multiplies, or did); the compositor cannot
/// apply the alpha, so the shell multiplies the pixels (`NHG_SURF_ALPHA_HOST`).
pub const NHG_ALPHA_DAMAGE: i32 = 1;
pub const NHG_ALPHA_HOST: i32 = 2;

/// A window to make (`nhg_window_new`): the shell's window descriptor, the window's role among the frames, and the facts
/// its frame starts from.  Texts are UTF-8 with their lengths (null with length 0: none).
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct NhgWindowDesc {
    /// The size in logical pixels.
    pub w: i32,
    pub h: i32,
    pub title: *const c_char,
    pub title_len: usize,
    pub app_id: *const c_char,
    pub app_id_len: usize,
    /// Top-level windows: the session name (none: empty).
    pub session_name: *const c_char,
    pub session_name_len: usize,
    /// `NHSHELL_WIN_*`.
    pub flags: u32,
    /// `NHSHELL_KIND_*`.
    pub kind: u32,
    /// Children and popups: the parent window.  A top-level: the window it opens beside (0: where the shell puts it).
    pub parent: u64,
    /// Children: where the surface starts in the parent's native area (device pixels: the frame's left and top less
    /// its own margin); the glue adds the parent's border and gives the shell logical pixels at `scale120`
    /// (`child::new_offset`).  A top-level with `NHSHELL_WIN_PLACED`: its top-left in device pixels at `scale120`
    /// from the main screen's top-left.
    pub x: i32,
    pub y: i32,
    /// Popups: where to put it.
    pub anchor: NhgAnchor,
    /// `NHG_FRAME_*`: what kind of frame the window has (none of them: a top-level).
    pub role: u32,
    /// `NHG_FACT_VISIBLE`, `NHG_FACT_ICONIFIED`, `NHG_FACT_CSD` as Emacs has them when the window is made.
    pub facts: u32,
    /// The frame's native size as Emacs has it (device pixels).
    pub native_w: i32,
    pub native_h: i32,
    /// The scale (times 120) C gives the window.
    pub scale120: u32,
}

/// The UTF-8 text of `len` bytes at `p` (null: empty), up to its first NUL (a C string stopped there).
///
/// # Safety
/// `p` is null or holds `len` readable bytes, valid for `'a`.
pub unsafe fn text<'a>(p: *const c_char, len: usize) -> Result<&'a str, GlueError> {
    if p.is_null() {
        return if len == 0 {
            Ok("")
        } else {
            Err(GlueError::Arg)
        };
    }
    // SAFETY: non-null with `len` readable bytes (the caller's promise).
    let b = unsafe { std::slice::from_raw_parts(p.cast::<u8>(), len) };
    let b = b.split(|&c| c == 0).next().unwrap_or_default();
    std::str::from_utf8(b).map_err(|_| GlueError::Arg)
}

/// Logical size of `dev` device pixels at scale `s120`/120, rounded as C's `nhwl_to_logical` (and the presenter's
/// viewport) rounds.
pub fn to_logical(dev: i32, s120: u32) -> i32 {
    let s = i64::from(s120.max(1));
    ((i64::from(dev) * 120 + s / 2) / s) as i32
}

/// The frame's scale times 120 (120 until C pushed one).
pub fn scale(f: &Frame) -> u32 {
    if f.scale120 == 0 { 120 } else { f.scale120 }
}

/// Register `window` as a frame of kind `role` with the facts it starts from (C pushed them one by one before).
pub fn seed(
    state: &mut Display,
    window: u64,
    role: u32,
    facts: u32,
    native: (i32, i32),
    scale120: u32,
) -> Result<(), GlueError> {
    if native.0 <= 0 || native.1 <= 0 || facts & !NHG_WINDOW_FACTS != 0 {
        return Err(GlueError::Arg);
    }
    state.register(window, role)?;
    let f = state.frame_mut(window)?;
    f.scale120 = scale120;
    f.facts.native_w = native.0;
    f.facts.native_h = native.1;
    f.facts.flags = facts;
    Ok(())
}

/// A named top-level of a session opened with a saved id is restored: `NHSHELL_WIN_RESTORE` is added to `flags`.
fn restore_flag(flags: u32, session_saved: bool, session_name: &str) -> u32 {
    if session_saved && !session_name.is_empty() {
        flags | NHSHELL_WIN_RESTORE
    } else {
        flags
    }
}

/// Make the window `d` describes and register it, seeded.  A window the glue cannot register is destroyed again.
pub fn new(
    state: &mut Display,
    d: &NhgWindowDesc,
    title: &str,
    app_id: &str,
    session_name: &str,
) -> Result<u64, GlueError> {
    if d.native_w <= 0 || d.native_h <= 0 || d.facts & !NHG_WINDOW_FACTS != 0 {
        return Err(GlueError::Arg);
    }
    let id = if d.kind == NHSHELL_KIND_TOPLEVEL {
        let desc = WindowDesc {
            w: d.w,
            h: d.h,
            title: title.into(),
            app_id: app_id.into(),
            flags: restore_flag(d.flags & !NHSHELL_WIN_PLACED, state.session_saved, session_name),
        };
        // where it opens: at its frame's position (device pixels at the scale C made it for), or beside the frame
        // it was made from, so it shows first on the screen of that scale
        let place = if d.flags & NHSHELL_WIN_PLACED != 0 {
            Place::At(to_logical(d.x, d.scale120), to_logical(d.y, d.scale120))
        } else if d.parent != 0 {
            Place::Near(d.parent)
        } else {
            Place::Shell
        };
        // an empty name is none
        state
            .backend()
            .window_new_placed(&desc, Some(session_name), place)?
    } else {
        let (x, y) = if d.kind == NHSHELL_KIND_CHILD {
            crate::child::new_window_offset(state, d)
        } else {
            (d.x, d.y)
        };
        state.backend().child_new(&ChildDesc {
            kind: d.kind,
            parent: d.parent,
            x,
            y,
            w: d.w,
            h: d.h,
            anchor: d.anchor,
            flags: d.flags,
        })?
    };
    let seeded = seed(
        state,
        id,
        d.role,
        d.facts,
        (d.native_w, d.native_h),
        d.scale120,
    );
    if let Err(e) = seeded {
        let _ = state.backend().window_destroy(id);
        return Err(e);
    }
    nhshell::trace::note(|| format!("window registered: window {id} kind {}", d.kind));
    // the frame was made for this scale (a reparented one shows at it already): its buffers start there, not at 1;
    // a refusal leaves them at 1 until the compositor says, as before
    if d.scale120 > 0 {
        let _ = state.backend().scale_start(id, d.scale120);
    }
    Ok(id)
}

/// `parent-frame` changed: `window` takes the place `d` describes (a top-level, or a child of `d.parent`) in the
/// shell, keeping its id and its picture, where the shell can (`NHG_E_UNSUPPORTED` where a window keeps its role: C
/// makes it again).  The frame takes `d`'s kind and facts; its child state starts over but for the chrome, which
/// the frame keeps, so C's pushes after it send the new place and an unchanged chrome changes nothing.
pub fn reparent(
    state: &mut Display,
    window: u64,
    d: &NhgWindowDesc,
    title: &str,
) -> Result<(), GlueError> {
    if d.native_w <= 0
        || d.native_h <= 0
        || d.facts & !NHG_WINDOW_FACTS != 0
        || d.role & !crate::frame::NHG_FRAME_KINDS != 0
    {
        return Err(GlueError::Arg);
    }
    state.frame(window)?;
    let (x, y) = if d.kind == NHSHELL_KIND_CHILD {
        crate::child::new_window_offset(state, d)
    } else {
        (d.x, d.y)
    };
    let desc = ChildDesc {
        kind: d.kind,
        parent: d.parent,
        x,
        y,
        w: d.w,
        h: d.h,
        anchor: d.anchor,
        flags: d.flags,
    };
    state.backend().reparent(window, &desc, title)?;
    let f = state.frame_mut(window)?;
    f.flags = d.role;
    f.scale120 = d.scale120;
    f.facts.native_w = d.native_w;
    f.facts.native_h = d.native_h;
    f.facts.flags = d.facts;
    f.child.reparented();
    f.floating = None;
    nhshell::trace::note(|| format!("window reparented: window {window} kind {}", d.kind));
    Ok(())
}

pub fn title(state: &mut Display, window: u64, title: &str) -> Result<(), GlueError> {
    state.frame(window)?;
    Ok(state.backend().set_title(window, title)?)
}

/// `undecorated` changed at run time: a top-level asks the compositor again (server decorations or none); a child
/// frame has none to ask for.
pub fn undecorated(state: &mut Display, window: u64, on: bool) -> Result<(), GlueError> {
    if state.frame(window)?.flags & NHG_FRAME_CHILD != 0 {
        return Ok(());
    }
    Ok(state.backend().set_decorated(window, !on)?)
}

/// Apply the frame's effective alpha `alpha` (C's choice between the active and inactive values, never below
/// `frame-alpha-lower-limit`; `alpha_bg`, its `alpha-background`, is painted into the backing by C and decides nothing
/// here).  The compositor multiplies when it can (`wp_alpha_modifier_v1`: surface state the window then owes a
/// commit for, the shell wakes its pacer); otherwise the shell multiplies the pixels at each present, and the whole
/// frame is damaged for it.  `NHG_ALPHA_*` bits, and the alpha's cut (what it takes off opaque) for C's surface list.
pub fn alpha(
    state: &mut Display,
    window: u64,
    alpha: f64,
    alpha_bg: f64,
) -> Result<(i32, u32), GlueError> {
    if !(0.0..=1.0).contains(&alpha) || alpha_bg.is_nan() {
        return Err(GlueError::Arg);
    }
    let cut = if alpha >= 1.0 {
        0
    } else {
        u32::MAX - (alpha * f64::from(u32::MAX)) as u32
    };
    let f = state.frame(window)?;
    let bits = |host: bool| if host { NHG_ALPHA_HOST } else { 0 };
    if cut == f.alpha_cut {
        return Ok((bits(f.alpha_host), cut));
    }
    let host = matches!(
        state.backend().set_alpha(window, u32::MAX - cut),
        Err(ShellError::Unsupported)
    );
    let f = state.frame_mut(window)?;
    let damage = if host || f.alpha_host {
        NHG_ALPHA_DAMAGE
    } else {
        0
    };
    f.alpha_cut = cut;
    f.alpha_host = host && cut != 0;
    Ok((damage | bits(f.alpha_host), cut))
}

/// C resized the frame itself to `w` x `h` device pixels (it takes the size it asks for at once: the compositor only
/// suggests).  As a push of the native size, a configure being applied gets a content ack.
pub fn size(state: &mut Display, window: u64, w: i32, h: i32) -> Result<(), GlueError> {
    configure::resized(state, window, w, h)
}

/// The frame's smallest size, `w` x `h` device pixels, as the compositor's minimum (with the next commit), in the
/// window's logical pixels.  A child frame has none.
pub fn min_size(state: &mut Display, window: u64, w: i32, h: i32) -> Result<(), GlueError> {
    let f = state.frame(window)?;
    if f.flags & NHG_FRAME_CHILD != 0 {
        return Ok(());
    }
    let s = scale(f);
    Ok(state
        .backend()
        .set_min_size(window, to_logical(w, s), to_logical(h, s))?)
}

/// Place the top-level frame with its top-left at (`x`, `y`) device pixels from the main screen's top-left, in the
/// window's logical pixels.  A child frame is placed by its parent (`NHG_E_ARG`); `NHG_E_UNSUPPORTED` where clients
/// have no position.
pub fn place(state: &mut Display, window: u64, x: i32, y: i32) -> Result<(), GlueError> {
    let f = state.frame(window)?;
    if f.flags & NHG_FRAME_CHILD != 0 {
        return Err(GlueError::Arg);
    }
    let s = scale(f);
    Ok(state
        .backend()
        .move_to(window, to_logical(x, s), to_logical(y, s))?)
}

/// Push: Emacs made the frame visible (`on`) or invisible, not iconified.
pub fn visible(state: &mut Display, window: u64, on: bool) -> Result<(), GlueError> {
    let f = state.frame_mut(window)?;
    nhshell::trace::note(|| {
        format!(
            "visible pushed: window {window} {on}, configured {}",
            f.cfg.configured
        )
    });
    f.facts.flags &= !(NHG_FACT_VISIBLE | NHG_FACT_ICONIFIED);
    if on {
        f.facts.flags |= NHG_FACT_VISIBLE;
    }
    Ok(())
}

/// Iconify: the compositor minimizes the window, and the frame is iconified, not visible.
pub fn iconify(state: &mut Display, window: u64) -> Result<(), GlueError> {
    let f = state.frame_mut(window)?;
    f.facts.flags = (f.facts.flags & !NHG_FACT_VISIBLE) | NHG_FACT_ICONIFIED;
    Ok(state.backend().minimize(window)?)
}

/// The frame's `fullscreen` (adapted from `set_fullscreen_state`, `pgtkterm.c`, emacs-31.1).  Wayland fills one
/// direction only by tiling, which only the compositor does: the nearest a client can ask for `fullwidth` and
/// `fullheight` is maximized (the configure then stores `maximized`).  Both requests are made; the first error is the
/// result.
pub fn fullscreen(state: &mut Display, window: u64, mode: u32) -> Result<(), GlueError> {
    state.frame(window)?;
    let b = state.backend();
    let (a, c) = match mode {
        NHG_FULLSCREEN_NONE => (
            b.set_fullscreen(window, false),
            b.set_maximized(window, false),
        ),
        NHG_FULLSCREEN_BOTH => (
            b.set_maximized(window, false),
            b.set_fullscreen(window, true),
        ),
        NHG_FULLSCREEN_MAXIMIZED | NHG_FULLSCREEN_WIDTH | NHG_FULLSCREEN_HEIGHT => (
            b.set_fullscreen(window, false),
            b.set_maximized(window, true),
        ),
        _ => return Err(GlueError::Arg),
    };
    Ok(a.and(c)?)
}

/// Destroy the window and forget its frame.  The frame is forgotten even when the shell refuses (after the
/// connection is lost every call is refused, and the window's local state is freed all the same).
pub fn destroy(state: &mut Display, window: u64) -> Result<(), GlueError> {
    state.frame(window)?;
    let r = state.backend().window_destroy(window);
    state.forget(window)?;
    Ok(r?)
}

/// One round of the wait for the window's first configure (adapted in role from `pgtk_wait_for_map_event`,
/// `pgtkterm.c`, emacs-31.1): 1 when the window is configured (a child frame needs no configure); otherwise the work
/// due on this thread is done and the shell's descriptor is waited on for at most `timeout_ms`: 0 when something is
/// queued (C drains it, then asks again), `NHG_E_TIMEOUT` when nothing came.
pub fn wait_configured(
    state: &mut Display,
    window: u64,
    timeout_ms: u32,
) -> Result<i32, GlueError> {
    let f = state.frame(window)?;
    if f.cfg.configured || f.flags & NHG_FRAME_CHILD != 0 {
        return Ok(1);
    }
    state.backend().dispatch_main()?;
    let mut p = libc::pollfd {
        fd: state.backend().wake_fd(),
        events: libc::POLLIN,
        revents: 0,
    };
    let ms = i32::try_from(timeout_ms).unwrap_or(i32::MAX);
    // SAFETY: one valid pollfd, for the call.
    let woke = unsafe { libc::poll(&mut p, 1, ms) };
    nhshell::trace::wait_round(window, woke == 0);
    match woke {
        0 => Err(GlueError::Code(NHG_E_TIMEOUT)),
        // something queued, or an interrupted wait: C drains and asks again
        _ => Ok(0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_named_window_of_a_saved_session_is_restored() {
        assert_eq!(restore_flag(0, true, "w"), NHSHELL_WIN_RESTORE);
        assert_eq!(restore_flag(0, true, ""), 0, "unnamed");
        assert_eq!(restore_flag(0, false, "w"), 0, "new session");
        assert_eq!(
            restore_flag(2, true, "w"),
            2 | NHSHELL_WIN_RESTORE,
            "other flags kept"
        );
    }
}

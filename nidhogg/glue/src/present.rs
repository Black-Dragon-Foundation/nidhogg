//! Presenting: the glue publishes a tree's picture to the shell at a consistency point, and has the shell present the
//! tree at its root's `FRAME_READY` (inline mode).
//!
//! Moved from `src/nhwlterm.c` (`nhwl_backstop`, `nhwl_present_tree`, `nhwl_presentable`, `nhwl_child_drawable`),
//! where C decided and committed.  The present path's rule is the shell's now (`ShellBackend::present_tree`); the
//! glue decides when a tree's picture is complete and which of its surfaces may show a buffer, from what C passes:
//!
//! - **The surface list.**  C walks the tree (it reads the frames) and lists each surface bottom to top, the root
//!   first and every parent before its children: the window, its parent's window, the frame's key, whether the frame
//!   may show a buffer by itself (shown, and not waiting for the redisplay of a new size; for a child frame, mapped
//!   too) and the alpha the shell must multiply when the compositor cannot.  A surface may show a buffer only when
//!   every ancestor may.
//! - **The consistency point** (`NHG_PRESENT_BACKSTOP`): Emacs is not redisplaying and buffer flips are not blocked,
//!   so every frame of the tree is drawn completely.  The glue publishes the tree there if its root may show a
//!   buffer: the drawable surfaces' backings go to the shell, which commits their scenes and copies what changed.  An
//!   undrawable surface is left out, so its damage waits in Glitnir and its last buffer stays on screen.
//! - **`FRAME_READY`** (`NHG_PRESENT_READY`): the shell presents the tree from what was published, whatever Emacs is
//!   doing (a packet is a complete picture by construction).  A tree that drew nothing but owes a commit (a bare
//!   ack, surface state, a re-arm of a waiting pacer) gets that commit here too.
//! - **The key time:** C passes the root's key time with the publish.  It is done (C clears it)
//!   when a packet carried it, or when the redisplay that just ended came after the key and drew nothing.

use crate::backing::Backings;
use crate::guard::GlueError;
use nhshell::api::{
    NHSHELL_SF_ALPHA_HOST, NHSHELL_SF_DRAWABLE, NhShellPresenterStats, ShellBackend, ShellError,
    SurfaceFrame,
};

/// [`NhgSurface::flags`]: the frame may show a buffer by itself (`FRAME_VISIBLE_P`, not `new_size_p`, and a child
/// frame's window is mapped).
pub const NHG_SURF_DRAWABLE: u32 = 1;
/// [`NhgSurface::flags`]: the compositor cannot apply the frame's alpha: the shell multiplies its pixels by
/// `1 - alpha_cut / u32::MAX`.
pub const NHG_SURF_ALPHA_HOST: u32 = 2;

/// `nhg_present` flags: a consistency point (not redisplaying, buffer flips not blocked): publish.
pub const NHG_PRESENT_BACKSTOP: u32 = 1;
/// `nhg_present` flags: the root's `FRAME_READY`: present inline.
pub const NHG_PRESENT_READY: u32 = 2;
/// `nhg_present` flags: the key time passed was read before the redisplay that just ended.
pub const NHG_PRESENT_KEY_SEEN: u32 = 4;

/// `nhg_present` result bits: a packet was published.
pub const NHG_PRESENTED_PUBLISHED: i32 = 1;
/// `nhg_present` result bits: the root committed a buffer (inline).
pub const NHG_PRESENTED_COMMITTED: i32 = 2;
/// `nhg_present` result bits: the key time is done; C clears it.
pub const NHG_PRESENTED_KEY_DONE: i32 = 4;

/// One surface of a tree, as C lists it.  Layout equals `NhgSurface`.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct NhgSurface {
    /// The surface's shell window.
    pub window: u64,
    /// Its parent's window (0 for the root).
    pub parent: u64,
    /// The frame's key (`backing.rs`): its Glitnir root id.
    pub frame: u64,
    /// `NHG_SURF_*`.
    pub flags: u32,
    /// With `NHG_SURF_ALPHA_HOST`: what the frame's alpha takes off opaque (`u32::MAX` minus the multiplier).
    pub alpha_cut: u32,
}

/// The display's presenting mode.
#[derive(Debug, Default)]
pub struct Presenting {
    /// The shell presents on its own thread: `FRAME_READY` does not come, and `present_tree` is not called.
    pub thread_on: bool,
}

impl Presenting {
    /// Present on the shell's thread (`on`) or inline, at the root's `FRAME_READY` (the host reads
    /// `nhwl-presenter-thread` and `NHWL_PRESENTER` at open).  A thread that cannot start leaves the display inline.
    pub fn set_thread(
        &mut self,
        backend: &mut dyn ShellBackend,
        on: bool,
    ) -> Result<(), GlueError> {
        let r = backend.set_presenter_thread(on);
        self.thread_on = on && r.is_ok();
        Ok(r?)
    }
}

/// Which surfaces of `s` may show a buffer: their own flag and every ancestor's.  `Arg` when `s` is not `root`'s
/// tree listed bottom to top (the root first with no parent, each parent before its children, no window twice).
pub fn drawable(root: u64, s: &[NhgSurface]) -> Result<Vec<bool>, GlueError> {
    let mut out: Vec<bool> = Vec::with_capacity(s.len());
    for (i, x) in s.iter().enumerate() {
        let own = x.flags & NHG_SURF_DRAWABLE != 0;
        if i == 0 {
            if root == 0 || x.window != root || x.parent != 0 {
                return Err(GlueError::Arg);
            }
            out.push(own);
            continue;
        }
        let before = &s[..i];
        if x.window == 0 || before.iter().any(|q| q.window == x.window) {
            return Err(GlueError::Arg);
        }
        let p = before
            .iter()
            .position(|q| q.window == x.parent)
            .ok_or(GlueError::Arg)?;
        out.push(own && out[p]);
    }
    Ok(out)
}

/// The frames to publish: each drawable surface with a backing, in the list's order.
fn frames<'a>(
    backings: &'a Backings,
    s: &[NhgSurface],
    drawable: &[bool],
) -> Vec<SurfaceFrame<'a>> {
    s.iter()
        .zip(drawable)
        .filter(|(_, d)| **d)
        .filter_map(|(x, _)| {
            let p = backings.frame(x.frame)?.published()?;
            let alpha_host = x.flags & NHG_SURF_ALPHA_HOST != 0;
            Some(SurfaceFrame {
                window: x.window,
                glitnir_root: x.frame,
                pixels: p.pixels,
                w: p.w,
                h: p.h,
                stride: p.w * 4,
                src_x: p.src.0,
                src_y: p.src.1,
                flags: NHSHELL_SF_DRAWABLE | if alpha_host { NHSHELL_SF_ALPHA_HOST } else { 0 },
                alpha_cut: if alpha_host { x.alpha_cut } else { 0 },
            })
        })
        .collect()
}

/// `nhg_present`: publish `root`'s tree at a consistency point, present it at its `FRAME_READY`, or both (the bench).
/// Returns `NHG_PRESENTED_*` bits.  Reads pixels.
pub fn present(
    backend: &mut dyn ShellBackend,
    backings: &Backings,
    presenting: &Presenting,
    root: u64,
    s: &[NhgSurface],
    flags: u32,
    t_key_ns: u64,
) -> Result<i32, GlueError> {
    let mut out = 0;
    if flags & NHG_PRESENT_BACKSTOP != 0 {
        let drawable = drawable(root, s)?;
        // a root that may not show a buffer publishes nothing, and its key waits for the redisplay that draws it
        if drawable.first() == Some(&true) {
            let frames = frames(backings, s, &drawable);
            let published = match frames.first() {
                Some(f) if f.window == root => backend.publish(root, &frames, t_key_ns)?,
                _ => false,
            };
            if published {
                out |= NHG_PRESENTED_PUBLISHED;
            }
            if t_key_ns != 0 && (published || flags & NHG_PRESENT_KEY_SEEN != 0) {
                out |= NHG_PRESENTED_KEY_DONE;
            }
        }
    }
    if flags & NHG_PRESENT_READY != 0 && !presenting.thread_on {
        match backend.present_tree(root) {
            Ok(true) => out |= NHG_PRESENTED_COMMITTED,
            // nothing to present, or not a tree's root (any more)
            Ok(false) | Err(ShellError::State) => {}
            Err(e) => return Err(e.into()),
        }
    }
    Ok(out)
}

/// The presenter's counters and timing since open.  Layout equals `NhgPresenterStats`.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct NhgPresenterStats {
    /// The shell's counters (`NhShellPresenterStats`).
    pub counters: NhShellPresenterStats,
    /// Root buffers that showed published drawing, and those committed more than a refresh after they were ready.
    pub frames: u64,
    pub late: u64,
    pub ready_to_commit_max_ns: u64,
}

/// The shell presenter's counters and timing.
pub fn stats(backend: &dyn ShellBackend) -> Result<NhgPresenterStats, GlueError> {
    let t = backend.presenter_timing()?;
    Ok(NhgPresenterStats {
        counters: backend.presenter_stats()?,
        frames: t.frames,
        late: t.late,
        ready_to_commit_max_ns: t.ready_to_commit_max_ns,
    })
}

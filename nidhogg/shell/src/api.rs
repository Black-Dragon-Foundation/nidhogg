//! The shell API: `ShellBackend`, the trait every windowing backend implements, and the plain types around it.
//!
//! The glue holds a `Box<dyn ShellBackend>` (`crate::open`) and knows nothing of the backend behind it.  All calls
//! come from the host's main thread.  Sizes are logical pixels unless a name says `dev` (device pixels).

use crate::caps::NhShellCaps;
use crate::event::Record;
use glitnir::geom::Rect;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::Thread;
use std::time::Instant;

pub type WindowId = u64;

/// `ack_configure` flag: the ack changes what the window shows, so only a buffer carries it.
pub const NHSHELL_ACK_CONTENT: u32 = 1;
/// `window_ready` bits: the host may draw and present a buffer; the window owes a commit (bare or with a buffer); with
/// `CONTENT`, a content ack waits (present a buffer even with nothing drawn).
pub const NHSHELL_READY_CONTENT: u32 = 1;
pub const NHSHELL_READY_OWED: u32 = 2;
pub const NHSHELL_READY_ACK: u32 = 4;

pub const NHSHELL_OK: i32 = 0;
pub const NHSHELL_E_PANIC: i32 = -1;
pub const NHSHELL_E_ARG: i32 = -2;
/// The call does not apply now (no such window, no buffer begun).
pub const NHSHELL_E_STATE: i32 = -3;
/// The backend cannot do this (a protocol it needs is absent, or no backend is built).
pub const NHSHELL_E_UNSUPPORTED: i32 = -4;
/// The connection to the display is gone.
pub const NHSHELL_E_DISCONNECTED: i32 = -5;
/// Out of memory or descriptors.
pub const NHSHELL_E_NOMEM: i32 = -6;
/// The other side did not answer in time (a selection read).
pub const NHSHELL_E_TIMEOUT: i32 = -7;
/// The buffer is smaller than the text (`take_text`); the text stays and `*len` says how big it is.
pub const NHSHELL_E_TOO_SMALL: i32 = -8;
/// The display is reconnecting (`ShellBackend::reconnect`): the call sent nothing.
pub const NHSHELL_E_RECONNECTING: i32 = -9;

/// Why a call failed; `code` is the glue's `NHG_E_*` value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShellError {
    Arg,
    State,
    Unsupported,
    Disconnected,
    NoMem,
    Timeout,
    TooSmall,
    Reconnecting,
}

impl ShellError {
    pub fn code(self) -> i32 {
        match self {
            ShellError::Arg => NHSHELL_E_ARG,
            ShellError::State => NHSHELL_E_STATE,
            ShellError::Unsupported => NHSHELL_E_UNSUPPORTED,
            ShellError::Disconnected => NHSHELL_E_DISCONNECTED,
            ShellError::NoMem => NHSHELL_E_NOMEM,
            ShellError::Timeout => NHSHELL_E_TIMEOUT,
            ShellError::TooSmall => NHSHELL_E_TOO_SMALL,
            ShellError::Reconnecting => NHSHELL_E_RECONNECTING,
        }
    }
}

pub type ShellResult<T = ()> = Result<T, ShellError>;

/// The connection's state (`ShellBackend::connection_state`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConnectionState {
    Up,
    /// The host took `RECONNECTING`; `reconnect` has not succeeded yet.
    Reconnecting,
    /// The host took `DISCONNECTED`.
    Lost,
}

impl ConnectionState {
    /// `NHSHELL_CONN_UP` 0, `_RECONNECTING` 1, `_LOST` 2.
    pub fn code(self) -> u32 {
        match self {
            ConnectionState::Up => 0,
            ConnectionState::Reconnecting => 1,
            ConnectionState::Lost => 2,
        }
    }
}

pub const NHSHELL_WIN_UNDECORATED: u32 = 1 << 0;
pub const NHSHELL_WIN_FULLSCREEN: u32 = 1 << 1;
pub const NHSHELL_WIN_MAXIMIZED: u32 = 1 << 2;
/// A popup that takes the keyboard and pointer (a menu): it grabs with the newest press, key or touch-down serial,
/// unless a drag is in progress, there is no serial, or it would not become the topmost grab (its parent must be a
/// top-level window while no popup holds a grab, or the popup holding the topmost grab).
/// Without the grab it still works by keys through its parent's focus, but an outside click does not dismiss it.
pub const NHSHELL_WIN_GRAB: u32 = 1 << 3;
/// With a session name: ask the session to restore the window (before its first commit) instead of just adding it.
pub const NHSHELL_WIN_RESTORE: u32 = 1 << 4;

/// Why a session is opened (`session_open`): a fresh launch, a recovery after the compositor restarted, a restore of
/// a saved session.
pub const NHSHELL_SESSION_LAUNCH: u32 = 0;
pub const NHSHELL_SESSION_RECOVER: u32 = 1;
pub const NHSHELL_SESSION_RESTORE: u32 = 2;

/// Edges for `resize`, OR-ed for a corner.
pub const NHSHELL_EDGE_TOP: u32 = 1;
pub const NHSHELL_EDGE_BOTTOM: u32 = 2;
pub const NHSHELL_EDGE_LEFT: u32 = 4;
pub const NHSHELL_EDGE_RIGHT: u32 = 8;

/// Pointer shapes for `set_cursor`.
pub const NHSHELL_CURSOR_DEFAULT: u32 = 0;
pub const NHSHELL_CURSOR_TEXT: u32 = 1;
pub const NHSHELL_CURSOR_POINTER: u32 = 2;
pub const NHSHELL_CURSOR_CROSSHAIR: u32 = 3;
pub const NHSHELL_CURSOR_WAIT: u32 = 4;
pub const NHSHELL_CURSOR_HELP: u32 = 5;
pub const NHSHELL_CURSOR_MOVE: u32 = 6;
pub const NHSHELL_CURSOR_NOT_ALLOWED: u32 = 7;
pub const NHSHELL_CURSOR_RESIZE_NS: u32 = 8;
pub const NHSHELL_CURSOR_RESIZE_EW: u32 = 9;
pub const NHSHELL_CURSOR_RESIZE_NWSE: u32 = 10;
pub const NHSHELL_CURSOR_RESIZE_NESW: u32 = 11;
pub const NHSHELL_CURSOR_HIDDEN: u32 = 12;

/// The selections, for the `selection_*` calls and `ShellEvent::Selection`.
pub const NHSHELL_SEL_CLIPBOARD: u32 = 0;
pub const NHSHELL_SEL_PRIMARY: u32 = 1;
/// The offer of the drag over one of our windows, for `selection_types` and `selection_read` only: valid from
/// `DND_ENTER` until `DND_LEAVE` or `dnd_finish`.  When it is our own drag's (`own` 1), or after our own drag's offer
/// is gone, the reads answer from our latest drag's bytes without a round trip.  `selection_owned` says whether our
/// drag runs.
pub const NHSHELL_SEL_DND: u32 = 2;

/// Drag-and-drop actions, OR-ed into a set (`DndEv`, `dnd_status`, `drag_start`).
pub const NHSHELL_DND_COPY: u32 = 1;
pub const NHSHELL_DND_MOVE: u32 = 2;
pub const NHSHELL_DND_ASK: u32 = 4;

/// What to open a window with.  Strings are copied by the callee.
#[derive(Clone, Debug)]
pub struct WindowDesc {
    pub w: i32,
    pub h: i32,
    pub title: String,
    pub app_id: String,
    pub flags: u32,
}

/// `ChildDesc::kind`: a top-level window (made by `window_new`), a child (a subsurface) or a popup.
pub const NHSHELL_KIND_TOPLEVEL: u32 = 0;
pub const NHSHELL_KIND_CHILD: u32 = 1;
pub const NHSHELL_KIND_POPUP: u32 = 2;

/// `Anchor::adjust`: what the compositor may do when a popup would leave the screen.
pub const NHSHELL_ADJUST_SLIDE_X: u32 = 1 << 0;
pub const NHSHELL_ADJUST_SLIDE_Y: u32 = 1 << 1;
pub const NHSHELL_ADJUST_FLIP_X: u32 = 1 << 2;
pub const NHSHELL_ADJUST_FLIP_Y: u32 = 1 << 3;

/// Where a popup goes: the rectangle `x`, `y`, `w`, `h` (logical pixels in the parent's surface) it sits beside.
/// `gravity` is the side (and corner) it grows toward: 0 none, 1 top, 2 bottom, 3 left, 4 right, 5 top-left,
/// 6 bottom-left, 7 top-right, 8 bottom-right.  The popup's matching corner touches the same corner of the rectangle,
/// so 8 puts it below and right of the rectangle.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct Anchor {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
    pub gravity: u32,
    pub adjust: u32,
}

/// What to open a child or popup with: `w` x `h` logical pixels at (`x`, `y`) in `parent`.  A popup is placed by
/// `anchor` and ignores `x`, `y`.
#[derive(Clone, Copy, Debug)]
pub struct ChildDesc {
    pub kind: u32,
    pub parent: WindowId,
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
    pub anchor: Anchor,
    /// `NHSHELL_WIN_GRAB` for a popup; 0 otherwise.
    pub flags: u32,
}

/// A pixel buffer to draw into until the next `present_commit`.  `age` is how many presents ago these pixels were
/// last shown (0: contents undefined, draw everything).  Valid until `present_commit` or the next `present_begin`.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct NhShellBuffer {
    pub pixels: *mut u8,
    pub w: i32,
    pub h: i32,
    pub stride: i32,
    pub age: u32,
}

/// A windowing backend: connection, windows, events and the presenter.
///
/// **Threads.** The glue (`nidhogg/glue`) calls a backend on the Lisp thread that holds Emacs's global lock, which is
/// usually the main thread but may be another Lisp thread.  A backend may require the main thread (AppKit is main-thread
/// only); the glue then defers or refuses other Lisp threads' calls with `NHG_E_THREAD`, as `nsterm.m`'s `ns_select`
/// falls back off the main thread.  The Wayland backend does not require it.
pub trait ShellBackend {
    /// A descriptor that becomes readable when `next_event` has something; the host adds it to its wait set.
    fn wake_fd(&self) -> i32;
    fn caps(&self) -> NhShellCaps;

    /// The next queued event, or `None` when the queue is empty.  After the connection is lost, one `DISCONNECTED`
    /// event comes out, then `ShellError::Disconnected`.  Once `DISCONNECTED` has been taken, every call but
    /// `wake_fd`, `caps` and `last_error` returns `Disconnected`, at once; `window_destroy` still frees the window's
    /// local state.  A bounded wait in progress (a selection read) gives up with `Disconnected` when the loss comes.
    fn next_event(&mut self) -> ShellResult<Option<Record>>;
    /// Run the work that must happen on the host's thread (configure and surface events).
    fn dispatch_main(&mut self) -> ShellResult;
    /// Why the connection ended: a protocol error's text, `INTERFACE@ID: error CODE: MESSAGE`; empty while it lives
    /// or after a plain hangup.
    fn last_error(&self) -> String {
        String::new()
    }
    /// Reconnect in place when the display goes (`on`; off at open).  With it on, a hangup posts `RECONNECTING`
    /// instead of `DISCONNECTED`; a protocol error still posts `DISCONNECTED`.  `Unsupported` when the connection
    /// cannot be made again (an inherited socket).
    fn set_reconnect(&mut self, on: bool) -> ShellResult {
        let _ = on;
        Err(ShellError::Unsupported)
    }
    /// Up; reconnecting from the moment the host takes `RECONNECTING` until `reconnect` succeeds; lost once the host
    /// takes `DISCONNECTED`.  While reconnecting, every call that would send returns `Reconnecting`; `window_destroy`
    /// frees the window's local state and succeeds.
    fn connection_state(&self) -> ConnectionState {
        ConnectionState::Up
    }
    /// One bounded step of reconnecting, at most `timeout_ms`: `Ok` once connected again (the globals, input and
    /// buffers are back, and `RECONNECTED` is posted), `Timeout` while the display is not there yet (call again
    /// later), `Disconnected` when the new display failed (a protocol error, a required global missing).  `Ok` when
    /// the connection is up; `State` while `RECONNECTING` waits to be taken.
    fn reconnect(&mut self, timeout_ms: u32) -> ShellResult {
        let _ = timeout_ms;
        Err(ShellError::Unsupported)
    }

    fn window_new(&mut self, desc: &WindowDesc) -> ShellResult<WindowId> {
        self.window_new_named(desc, None)
    }
    /// `window_new` with a session name: sent to the session (`NHSHELL_WIN_RESTORE`: as a restore) before the window's
    /// first commit.  The name is ignored without a session, or when it was replaced.
    fn window_new_named(
        &mut self,
        desc: &WindowDesc,
        session_name: Option<&str>,
    ) -> ShellResult<WindowId>;
    fn window_destroy(&mut self, window: WindowId) -> ShellResult;
    fn set_title(&mut self, window: WindowId, title: &str) -> ShellResult;
    fn set_app_id(&mut self, window: WindowId, app_id: &str) -> ShellResult;
    fn set_min_size(&mut self, window: WindowId, w: i32, h: i32) -> ShellResult;
    fn set_fullscreen(&mut self, window: WindowId, on: bool) -> ShellResult;
    fn set_maximized(&mut self, window: WindowId, on: bool) -> ShellResult;
    fn minimize(&mut self, window: WindowId) -> ShellResult;
    /// Start an interactive move; the backend uses the latest input serial.
    fn move_window(&mut self, window: WindowId) -> ShellResult;
    /// Start an interactive resize from `edges` (`NHSHELL_EDGE_*`); the backend uses the latest input serial.
    fn resize(&mut self, window: WindowId, edges: u32) -> ShellResult;
    /// Show the window menu at (`x`, `y`) in the window; the backend uses the latest input serial.
    fn show_menu(&mut self, window: WindowId, x: i32, y: i32) -> ShellResult;
    /// Set the window geometry (`xdg_surface.set_window_geometry`, surface-local logical pixels), which takes effect
    /// with the next commit that carries a buffer.
    fn set_geometry(&mut self, window: WindowId, x: i32, y: i32, w: i32, h: i32) -> ShellResult;
    /// Record configure `serial`'s ack; it goes out right before the commit that carries it.  `flags`
    /// `NHSHELL_ACK_CONTENT` says the ack changes what the window shows (a new size, the first configure, a client
    /// side state change): it is sent only with a buffer.  The flag is sticky until a buffer carries the ack.
    fn ack_configure(&mut self, window: WindowId, serial: u32, flags: u32) -> ShellResult;
    fn set_cursor(&mut self, window: WindowId, shape: u32) -> ShellResult;
    /// The Xcursor theme (`name`, logical `size`) used where the compositor has no cursor-shape protocol.
    fn set_cursor_theme(&mut self, name: &str, size: u32) -> ShellResult;
    /// Ask the compositor to activate the window with `token`; without one, request a token with the latest input
    /// serial and the window's surface, and activate with it when it arrives (`State` before any input).
    fn window_activate(&mut self, window: WindowId, token: Option<&str>) -> ShellResult;
    /// A fresh activation token made with the newest input serial and the window's surface, within `timeout_ms`
    /// (`Timeout`; `State` before any input; `Unsupported` without the protocol).
    fn activation_token(&mut self, window: WindowId, timeout_ms: u32) -> ShellResult<String>;

    /// Ring the system bell on the window's root surface (window 0: no surface), at most once per 20 ms.
    /// `Unsupported` without the protocol.  The visible bell is the host's.
    fn bell(&mut self, window: WindowId) -> ShellResult;
    /// Hold (`on`) or release an idle inhibitor on the window's surface.  `Unsupported` without the protocol.
    fn inhibit_idle(&mut self, window: WindowId, on: bool) -> ShellResult;
    /// The window's xdg-foreign handle (no scheme prefix), exported at the first call and cached until the window
    /// goes; waits at most `timeout_ms` for it (`Timeout`).  `Unsupported` without the exporter.
    fn window_export(&mut self, window: WindowId, timeout_ms: u32) -> ShellResult<String>;
    /// Open the session (`id` None: a new one) for `reason` (`NHSHELL_SESSION_*`); `ShellEvent::Session` answers.
    /// `Unsupported` without session management.
    fn session_open(&mut self, id: Option<&str>, reason: u32) -> ShellResult;
    /// The open session's id (`State` before the compositor has said).
    fn session_id(&self) -> ShellResult<String>;
    /// Take the window out of the session: the compositor forgets its name.  `State` for an unnamed window.
    fn window_session_remove(&mut self, window: WindowId) -> ShellResult;

    /// The window's preferred scale times 120 (120 when the compositor has not said).
    fn window_scale(&self, window: WindowId) -> ShellResult<u32>;
    /// Whether the compositor decorates the window (server-side decorations were granted to it).
    fn window_ssd(&self, window: WindowId) -> ShellResult<bool>;
    /// Ask again for server-side decorations (`on`) or for none: a top-level's `undecorated` changed at run time.
    /// The compositor's answer comes with a configure; nothing when decorations are forced client-side.
    fn set_decorated(&mut self, window: WindowId, on: bool) -> ShellResult;
    /// The window's opacity, a multiplier of its pixels' alpha (`u32::MAX` is opaque).  The window owes a commit
    /// (its ancestors too, for a child).  `Unsupported` when the compositor cannot apply it: the host multiplies.
    fn set_alpha(&mut self, window: WindowId, factor: u32) -> ShellResult;

    /// Create a child (`NHSHELL_KIND_CHILD`) or popup (`NHSHELL_KIND_POPUP`) window.
    fn child_new(&mut self, desc: &ChildDesc) -> ShellResult<WindowId>;
    /// Move a child (applied with its parent's next commit) or a popup (its anchor rectangle's origin).
    /// `true` when the whole window must be redrawn (a popup that was made again).
    fn set_position(&mut self, window: WindowId, x: i32, y: i32) -> ShellResult<bool>;
    /// Put a child above or below `sibling` (0: its parent), applied with the parent's next commit.
    fn stack(&mut self, window: WindowId, sibling: WindowId, above: bool) -> ShellResult;
    /// The part of the window that takes input (logical pixels); none at all means the whole surface.
    fn set_input_region(&mut self, window: WindowId, rects: &[Rect]) -> ShellResult;
    /// Off: show nothing (a null buffer); on: the next present shows the window again.
    fn set_mapped(&mut self, window: WindowId, on: bool) -> ShellResult;
    /// Whether popup `window` holds a grab (asked with `NHSHELL_WIN_GRAB`, granted, not dismissed); false for other
    /// windows.
    fn window_grabbed(&self, window: WindowId) -> ShellResult<bool>;
    /// A popup's new size (logical pixels), placed again by its anchor.  `true` when the whole window must be redrawn
    /// (a popup made again), as for `set_position`; the next configure asks for a buffer of the new size.  `State`
    /// for windows that are not popups (their buffers set their sizes).
    fn set_size(&mut self, window: WindowId, w: i32, h: i32) -> ShellResult<bool>;

    /// Own selection `kind` (`NHSHELL_SEL_*`) with these MIME types and bytes (copied); other clients' pastes are
    /// answered from the copy.  `State` without keyboard focus.
    fn selection_own(&mut self, kind: u32, items: &[(&str, &[u8])]) -> ShellResult;
    fn selection_disown(&mut self, kind: u32) -> ShellResult;
    /// Whether our source is the current selection `kind`.
    fn selection_owned(&self, kind: u32) -> ShellResult<bool>;
    /// The current selection's MIME types (empty when there is none).
    fn selection_types(&self, kind: u32) -> ShellResult<Vec<String>>;
    /// The selection's bytes as `mime`: our own without a round trip, another client's within `timeout_ms` or
    /// `Timeout`.  `State` when there is no selection or it has no such type.
    fn selection_read(&mut self, kind: u32, mime: &str, timeout_ms: u32) -> ShellResult<Vec<u8>>;

    /// Answer the drag over our window: accept `mime` (None: nothing) with the actions we take (`NHSHELL_DND_*`) and
    /// the one we prefer (0 or one of them).  None with 0 actions rejects the drop.  `State` without a drag over us;
    /// `Arg` for bits outside the set or a preferred action that is not one of `actions`.
    fn dnd_status(&mut self, mime: Option<&str>, actions: u32, preferred: u32) -> ShellResult;
    /// Done with the dropped offer: on `success`, tell the source the drop is finished, which needs a dropped offer,
    /// an accepted type and an action both sides agreed on (else `State`); then, in every case, let the offer go.
    fn dnd_finish(&mut self, success: bool) -> ShellResult;
    /// Start dragging `items` (MIME type, bytes; copied) out of `window` with the actions it allows.  It needs the
    /// newest button press or touch down whose release has not come (`State` without one); there is no drag icon.  The
    /// bytes are written to targets from the shell's own thread, and stay readable as `NHSHELL_SEL_DND` until the next
    /// drag starts.  `DRAG_ACTION`, `DRAG_DROPPED` and one `DRAG_DONE` follow; a drag that was running is dropped
    /// without one.
    fn drag_start(
        &mut self,
        window: WindowId,
        items: &[(&str, &[u8])],
        actions: u32,
    ) -> ShellResult;
    /// Give up our drag: its source goes, and no `DRAG_DONE` follows.  Nothing when no drag runs.
    fn drag_cancel(&mut self) -> ShellResult;

    /// Text input for `window` on or off; the state survives the window losing and regaining text-input focus.
    fn text_input_enable(&mut self, window: WindowId, on: bool) -> ShellResult;
    /// The cursor rectangle in the window (logical pixels); sent to the input method only when it changed.
    fn text_input_cursor(
        &mut self,
        window: WindowId,
        x: i32,
        y: i32,
        w: i32,
        h: i32,
    ) -> ShellResult;
    /// The text around the cursor (UTF-8; `cursor` and `anchor` are byte offsets), cut to 4000 bytes.
    fn text_input_surrounding(
        &mut self,
        window: WindowId,
        text: &str,
        cursor: usize,
        anchor: usize,
    ) -> ShellResult;
    /// The size of text `id` from a `PREEDIT` or `COMMIT_TEXT` record.
    fn text_len(&self, id: u32) -> ShellResult<usize>;
    /// Take text `id` (single-use).
    fn take_text(&mut self, id: u32) -> ShellResult<Vec<u8>>;

    /// Publish the tree of root `root` at a consistency point: one frame per drawable surface, the root first, then
    /// its child windows deepest first.  The shell presents: `FRAME_READY` comes for roots only, when the tree's pacer
    /// lets it present (inline mode), and the host answers with `present_tree`.  Each surface's scene is committed and its damaged pixels copied; nothing is
    /// read after the call.  `true` when a packet was queued (damage, or a content ack that waits for a picture drawn
    /// after it).  `t_key_ns` is the key the picture answers (0: none).
    fn publish(
        &mut self,
        root: WindowId,
        frames: &[SurfaceFrame],
        t_key_ns: u64,
    ) -> ShellResult<bool>;
    /// Inline mode: present root `root`'s tree by the present path's rule; `true` when the root committed a buffer.
    /// `State` in thread mode.
    fn present_tree(&mut self, root: WindowId) -> ShellResult<bool>;
    /// Present on the shell's own thread (`on`) or inline, when the host calls `present_tree` (off at open).
    fn set_presenter_thread(&mut self, on: bool) -> ShellResult;
    fn presenter_stats(&self) -> ShellResult<NhShellPresenterStats>;

    /// What a host needs to abandon this backend without touching its state again: the glue calls it once at open and
    /// uses it only after a panic left the backend's state in doubt.  Every thread the backend runs is in the list.
    fn abandon(&self) -> Abandon;
    /// The presenter's own timing since open (Rust only): which frames it committed late.
    fn presenter_timing(&self) -> ShellResult<PresenterTiming>;
}

/// A backend's connection and threads, as a host that abandons the backend sees them (`ShellBackend::abandon`).  The
/// host shuts the connection down, stops the threads, waits a bounded time and leaks the backend.
pub struct Abandon {
    /// The connection's socket (-1: none).  `shutdown` on it makes the compositor drop every object of ours.
    pub conn_fd: i32,
    pub threads: Vec<Arc<dyn BackendThread>>,
}

/// A thread a backend runs.  Both calls touch only atomics and the thread's wake-up, never the backend's state.
pub trait BackendThread: Send + Sync {
    /// Ask the thread to leave its loop, and wake it so that it does.
    fn stop(&self);
    /// Wait until the thread has finished, at most until `deadline`; whether it has.
    fn wait_done(&self, deadline: Instant) -> bool;
}

/// A thread's "finished" report: the thread sets it last, and a host waits for it with a deadline.
#[derive(Default)]
pub struct ThreadDone {
    done: AtomicBool,
    waiter: Mutex<Option<Thread>>,
}

impl ThreadDone {
    /// Called by the thread as the last thing it does.
    pub fn set(&self) {
        self.done.store(true, Ordering::Release);
        let waiter = self.waiter.lock().unwrap_or_else(|e| e.into_inner()).take();
        if let Some(t) = waiter {
            t.unpark();
        }
    }

    /// A new thread takes the report over (the old one has finished).
    pub fn reset(&self) {
        self.done.store(false, Ordering::Release);
    }

    /// Wait until `set`, at most until `deadline`; whether it was set.
    pub fn wait(&self, deadline: Instant) -> bool {
        *self.waiter.lock().unwrap_or_else(|e| e.into_inner()) = Some(std::thread::current());
        loop {
            if self.done.load(Ordering::Acquire) {
                return true;
            }
            let now = Instant::now();
            if now >= deadline {
                return false;
            }
            std::thread::park_timeout(deadline - now);
        }
    }
}

/// The presenter's timing.  A frame is a root buffer that shows published drawing; it was ready when its tree could
/// first commit it (the later of its first publish and the pacer's opening: a frame callback, the end of starvation).
/// It is late when committed more than one refresh period after that (the period presentation feedback reports, else
/// 1/120 s): a vblank the presenter missed, whatever the host's thread did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PresenterTiming {
    pub frames: u64,
    pub late: u64,
    pub ready_to_commit_max_ns: u64,
}

/// `SurfaceFrame` flags: the surface may show a buffer (without it, its damage waits); the presenter multiplies the
/// surface's pixels by its alpha (the compositor cannot).
pub const NHSHELL_SF_DRAWABLE: u32 = 1;
pub const NHSHELL_SF_ALPHA_HOST: u32 = 2;

/// One surface of a published tree.  `pixels` is the host's backing, `h` rows of `stride` bytes (`w` x `h` device
/// pixels, premultiplied ARGB32), borrowed for the call.  `glitnir_root` is the surface's scene; `(src_x, src_y)`
/// is where its layer starts in the backing.  `alpha_cut`: with `NHSHELL_SF_ALPHA_HOST`, the pixels are multiplied
/// by `1 - alpha_cut / u32::MAX`.
#[derive(Clone, Copy, Debug)]
pub struct SurfaceFrame<'a> {
    pub window: WindowId,
    pub glitnir_root: u64,
    pub pixels: &'a [u8],
    pub w: i32,
    pub h: i32,
    pub stride: i32,
    pub src_x: i32,
    pub src_y: i32,
    pub flags: u32,
    pub alpha_cut: u32,
}

/// The presenter's counters since open: packets published and merged into one still waiting (`superseded`), buffer
/// and bare commits, the longest wait of the host's thread and the longest publish copy, and fronts dropped idle.
/// C reads it as `NhgPresenterCounters` (`nhglue.h`), inside the glue's `NhgPresenterStats`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct NhShellPresenterStats {
    pub packets: u64,
    pub superseded: u64,
    pub commits: u64,
    pub bare_commits: u64,
    pub main_wait_max_ns: u64,
    pub publish_copy_max_ns: u64,
    pub front_trims: u64,
    pub thread_on: u32,
}

#[cfg(test)]
mod tests {
    /// The shell API names no backend: the files of the API name no Wayland backend identifier, and, apart from
    /// `caps.rs` (whose table names protocol interfaces), no Wayland object type.  (The patterns are built from halves so this
    /// file does not match itself.)
    #[test]
    fn no_wayland_names_in_ffi() {
        let wayland = concat!("wayland", "_");
        let wl = concat!("wl", "_");
        let files = [
            ("api.rs", include_str!("api.rs"), true),
            ("caps.rs", include_str!("caps.rs"), false),
            ("event.rs", include_str!("event.rs"), true),
            ("lib.rs", include_str!("lib.rs"), true),
        ];
        for (name, text, check_wl) in files {
            // this test's own name is the one line allowed to say it
            let text: String = text.lines().filter(|l| !l.contains("fn no_")).collect();
            assert!(!text.contains(wayland), "{name} names {wayland}");
            if check_wl {
                assert!(!text.contains(wl), "{name} names {wl}");
            }
        }
    }
}

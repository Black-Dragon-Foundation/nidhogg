//! The shell API: `ShellBackend`, the trait every windowing backend implements, and the plain types around it.
//!
//! The C ABI (`ffi.rs`) holds a `Box<dyn ShellBackend>` and knows nothing of the backend behind it.  All calls come
//! from the host's main thread.  Sizes are logical pixels unless a name says `dev` (device pixels).

use crate::caps::NhShellCaps;
use crate::event::NhShellEvent;
use glitnir::geom::Rect;

pub type WindowId = u64;

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

/// Why a call failed; `code` is the C ABI's value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShellError {
    Arg,
    State,
    Unsupported,
    Disconnected,
    NoMem,
}

impl ShellError {
    pub fn code(self) -> i32 {
        match self {
            ShellError::Arg => NHSHELL_E_ARG,
            ShellError::State => NHSHELL_E_STATE,
            ShellError::Unsupported => NHSHELL_E_UNSUPPORTED,
            ShellError::Disconnected => NHSHELL_E_DISCONNECTED,
            ShellError::NoMem => NHSHELL_E_NOMEM,
        }
    }
}

pub type ShellResult<T = ()> = Result<T, ShellError>;

pub const NHSHELL_WIN_UNDECORATED: u32 = 1 << 0;
pub const NHSHELL_WIN_FULLSCREEN: u32 = 1 << 1;
pub const NHSHELL_WIN_MAXIMIZED: u32 = 1 << 2;

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

/// What to open a window with.  Strings are copied by the callee.
#[derive(Clone, Debug)]
pub struct WindowDesc {
    pub w: i32,
    pub h: i32,
    pub title: String,
    pub app_id: String,
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
pub trait ShellBackend {
    /// A descriptor that becomes readable when `next_event` has something; the host adds it to its wait set.
    fn wake_fd(&self) -> i32;
    fn caps(&self) -> NhShellCaps;

    /// The next queued event, or `None` when the queue is empty.  After the connection is lost, one `DISCONNECTED`
    /// event comes out, then `ShellError::Disconnected`.
    fn next_event(&mut self) -> ShellResult<Option<NhShellEvent>>;
    /// Run the work that must happen on the host's thread (configure and surface events).
    fn dispatch_main(&mut self) -> ShellResult;

    fn window_new(&mut self, desc: &WindowDesc) -> ShellResult<WindowId>;
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
    /// with the next commit.
    fn set_geometry(&mut self, window: WindowId, x: i32, y: i32, w: i32, h: i32) -> ShellResult;
    fn ack_configure(&mut self, window: WindowId, serial: u32) -> ShellResult;
    fn set_cursor(&mut self, window: WindowId, shape: u32) -> ShellResult;

    /// The window's preferred scale times 120 (120 when the compositor has not said).
    fn window_scale(&self, window: WindowId) -> ShellResult<u32>;
    fn window_logical_size(&self, window: WindowId) -> ShellResult<(i32, i32)>;

    /// Begin a frame of `dev_w` x `dev_h` device pixels.
    fn present_begin(
        &mut self,
        window: WindowId,
        dev_w: i32,
        dev_h: i32,
    ) -> ShellResult<NhShellBuffer>;
    /// Show the begun buffer; `damage` is in device pixels.
    fn present_commit(&mut self, window: WindowId, damage: &[Rect]) -> ShellResult;
}

#[cfg(test)]
mod tests {
    /// The shell API names no backend: the files behind the C ABI name no Wayland backend identifier, and, apart
    /// from `caps.rs` (whose table names protocol interfaces), no Wayland object type.  (The patterns are built from halves so this
    /// file does not match itself.)
    #[test]
    fn no_wayland_names_in_ffi() {
        let wayland = concat!("wayland", "_");
        let wl = concat!("wl", "_");
        let files = [
            ("api.rs", include_str!("api.rs"), true),
            ("caps.rs", include_str!("caps.rs"), false),
            ("event.rs", include_str!("event.rs"), true),
            ("ffi.rs", include_str!("ffi.rs"), true),
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

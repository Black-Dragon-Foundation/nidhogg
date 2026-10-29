//! nhshell, Nidhogg's native shell layer: a Rust library whose one user is the glue (`nidhogg/glue`).
//!
//! An rlib: Emacs links it through `nidhogg/glue`'s `libnhglue.a`, the one Rust static library in the binary (two
//! would collide on std's symbols), which carries Glitnir's C ABI too.  The Wayland backend sits behind the default
//! feature `wayland`.

pub mod api;
pub mod caps;
pub mod event;
#[cfg(feature = "wayland")]
pub mod wayland;

use api::{ShellBackend, ShellResult};
use std::ffi::CStr;

/// Connect to `display` (`None`: the environment's).  The backends are added here as they are written.
#[cfg(feature = "wayland")]
pub fn open(display: Option<&CStr>) -> ShellResult<Box<dyn ShellBackend>> {
    let shell = wayland::WaylandShell::open(display)?;
    Ok(Box::new(shell))
}

/// Without a backend (the Android check builds without `wayland`), nothing can connect.
#[cfg(not(feature = "wayland"))]
pub fn open(_display: Option<&CStr>) -> ShellResult<Box<dyn ShellBackend>> {
    Err(api::ShellError::Unsupported)
}

#[cfg(feature = "wayland")]
pub use wayland::reader::is_reader_thread;
/// Whether the calling thread is a backend's reader thread: never, without a backend.
#[cfg(not(feature = "wayland"))]
pub fn is_reader_thread() -> bool {
    false
}

/// Glitnir's whole API, re-exported so its `#[no_mangle]` functions stay in this archive.
pub use glitnir::*;

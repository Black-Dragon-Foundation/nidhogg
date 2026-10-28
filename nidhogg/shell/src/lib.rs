//! nhshell, Nidhogg's native shell layer with a C ABI (`include/nhshell.h`).
//!
//! An rlib: Emacs links it through `nidhogg/glue`'s `libnhglue.a`, the one Rust static library in the binary (two
//! would collide on std's symbols), which carries this crate's and Glitnir's C ABIs too.  The Wayland backend sits
//! behind the default feature `wayland`.

pub mod abi;
pub mod api;
pub mod caps;
pub mod event;
pub mod ffi;
#[cfg(feature = "wayland")]
pub mod wayland;

#[cfg(feature = "wayland")]
pub use wayland::reader::is_reader_thread;
/// Whether the calling thread is a backend's reader thread: never, without a backend.
#[cfg(not(feature = "wayland"))]
pub fn is_reader_thread() -> bool {
    false
}

/// Glitnir's C ABI under another name, because this crate's own `ffi` shadows the glob below.
pub use glitnir::ffi as glitnir_ffi;
/// Glitnir's whole API, re-exported so its `#[no_mangle]` functions stay in this archive.
pub use glitnir::*;

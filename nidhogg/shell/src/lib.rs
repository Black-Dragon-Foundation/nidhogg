//! nhshell, Nidhogg's native shell layer with a C ABI (`include/nhshell.h`).
//!
//! The archive `libnhshell.a` carries Glitnir too: under `--with-nidhogg-wayland` the Makefile links it instead of
//! `libglitnir.a`, because two Rust static libraries in one binary would collide on std's symbols.

pub mod abi;
pub mod api;
pub mod caps;
pub mod event;
pub mod ffi;

/// Glitnir's C ABI under another name, because this crate's own `ffi` shadows the glob below.
pub use glitnir::ffi as glitnir_ffi;
/// Glitnir's whole API, re-exported so its `#[no_mangle]` functions stay in this archive.
pub use glitnir::*;

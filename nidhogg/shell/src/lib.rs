//! nhshell, Nidhogg's native shell layer with a C ABI (`include/nhshell.h`).
//!
//! The archive `libnhshell.a` carries Glitnir too: under `--with-nidhogg-wayland` the Makefile links it instead of
//! `libglitnir.a`, because two Rust static libraries in one binary would collide on std's symbols.

pub mod abi;

/// Glitnir's whole API, re-exported so its `#[no_mangle]` functions stay in this archive.
pub use glitnir::*;

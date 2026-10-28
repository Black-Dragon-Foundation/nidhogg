//! nhglue, the glue between Emacs's C hooks (`src/nhwl*.c`) and Nidhogg's native shell (`nhshell`), with a C ABI
//! (`include/nhglue.h`).
//!
//! C touches Emacs and calls in here; the glue holds the shell's state and never calls back into Emacs. `libnhglue.a`
//! is the one Rust static library Emacs links under `--with-nidhogg-wayland`, because two Rust static libraries in one
//! binary would collide on std's symbols: it carries the shell's and Glitnir's C ABIs too, re-exported below.

pub mod abi;
#[macro_use]
pub mod guard;
pub mod action;
pub mod backing;
pub mod bench;
pub mod configure;
pub mod display;
pub mod drain;
pub mod ffi;
pub mod focus;
pub mod frame;
pub mod ime;
pub mod keys;
pub mod monitors;
pub mod pointer;
pub mod present;
pub mod query;
pub mod scroll;
pub mod select;

pub mod csd;

/// Glitnir's C ABI under another name, because this crate's own `ffi` shadows the glob below.
pub use glitnir::ffi as glitnir_ffi;
/// Glitnir's whole API, re-exported so its `#[no_mangle]` functions stay in this archive (`nidhogg-glitnir.c` calls
/// them).
pub use glitnir::*;
/// The shell's C ABI (`nhshell.h`), which `src/nhwl*.c` calls until its calls are ported.
pub use nhshell::{abi as nhshell_abi, ffi as nhshell_ffi};

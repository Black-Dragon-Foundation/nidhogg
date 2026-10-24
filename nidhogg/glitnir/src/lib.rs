//! Glitnir, a compositor library with a C ABI (`include/glitnir.h`).
//!
//! The crate's public API takes ids, rectangles, pixels and parameters only.

use std::sync::Mutex;

pub mod ffi;

/// All of the library's global state, behind one lock.
#[derive(Debug, Default)]
pub(crate) struct State {}

// Nothing reads it until the first stateful export lands.
#[allow(dead_code)]
pub(crate) static STATE: Mutex<State> = Mutex::new(State {});

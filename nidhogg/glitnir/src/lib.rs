//! Glitnir, a compositor library with a C ABI (`include/glitnir.h`).
//!
//! The crate's public API takes ids, rectangles, pixels and parameters only.

use std::sync::Mutex;

pub mod ffi;
pub mod geom;
pub mod region;
pub mod rng;
pub mod stats;

/// All of the library's global state, behind one lock.
#[derive(Debug, Default)]
pub(crate) struct State {
    pub(crate) rings: stats::Rings,
}

pub(crate) static STATE: Mutex<State> = Mutex::new(State {
    rings: stats::Rings::new(),
});

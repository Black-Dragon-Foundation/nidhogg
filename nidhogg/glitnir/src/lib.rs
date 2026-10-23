//! Glitnir, a compositor library with a C ABI (`include/glitnir.h`).
//!
//! The crate's public API takes ids, rectangles, pixels and parameters only.

use std::sync::Mutex;

pub mod compose;
pub mod damage;
pub mod ffi;
pub mod geom;
pub mod kernels;
#[cfg(all(test, feature = "pixman-oracle"))]
mod oracle;
pub mod region;
pub mod rng;
pub mod scene;
pub mod shadow;
pub mod stack;
pub mod stats;

/// All of the library's global state, behind one lock.
#[derive(Debug, Default)]
pub(crate) struct State {
    pub(crate) rings: stats::Rings,
    pub(crate) damage: damage::Frames,
    pub(crate) scenes: scene::Scenes,
    pub(crate) composer: compose::Composer,
}

pub(crate) static STATE: Mutex<State> = Mutex::new(State {
    rings: stats::Rings::new(),
    damage: damage::Frames::new(),
    scenes: scene::Scenes::new(),
    composer: compose::Composer::new(),
});

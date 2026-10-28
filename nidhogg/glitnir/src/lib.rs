//! Glitnir, a compositor library with a C ABI (`include/glitnir.h`).
//!
//! The crate's public API takes ids, rectangles, pixels and parameters only.
//!
//! The scenes, the damage and the sprite cache are the host's main thread's, behind one lock.  A commit hands out an
//! immutable snapshot ([`committed::Committed`]) that another thread composes with its own [`compose::Composer`],
//! without that lock; frame-time records have their own lock ([`stats`]).

use std::cell::Cell;
use std::sync::{Arc, LazyLock, Mutex, MutexGuard};

pub mod committed;
pub mod compose;
pub mod damage;
pub mod decor;
pub mod ffi;
pub mod geom;
pub mod kernels;
#[cfg(all(test, feature = "pixman-oracle"))]
mod oracle;
pub mod pool;
pub mod probe;
pub mod region;
pub mod rng;
pub mod scene;
pub mod shadow;
pub mod snap;
pub mod stack;
pub mod stats;

/// The library's global state, behind one lock, used from the host's main thread only.
#[derive(Debug, Default)]
pub(crate) struct State {
    pub(crate) damage: damage::Frames,
    pub(crate) scenes: scene::Scenes,
    /// Shared with the snapshots; a put copies it on write.
    pub(crate) sprites: Arc<decor::Sprites>,
}

static STATE: LazyLock<Mutex<State>> = LazyLock::new(Mutex::default);

thread_local! {
    static COMPOSE_THREAD: Cell<bool> = const { Cell::new(false) };
}

/// Mark the calling thread as a compose thread: it composes snapshots and never touches the global state.  From then
/// on, in a debug build, any access to that state from this thread panics.
pub fn mark_compose_thread() {
    COMPOSE_THREAD.set(true);
}

/// Whether the calling thread is a compose thread (the shell glue refuses to run there).
pub fn is_compose_thread() -> bool {
    COMPOSE_THREAD.get()
}

/// Lock the global state (a poisoned lock is taken as it is).  Debug builds check that the caller is not a compose
/// thread.
pub(crate) fn state() -> MutexGuard<'static, State> {
    debug_assert!(
        !COMPOSE_THREAD.get(),
        "glitnir: a compose thread touched the global state"
    );
    STATE.lock().unwrap_or_else(|p| p.into_inner())
}

#[cfg(test)]
mod state {
    #[test]
    fn state_is_main_thread_only() {
        let compose = std::thread::spawn(|| {
            crate::mark_compose_thread();
            crate::damage::with(8601, |d| d.is_empty())
        });
        let err = compose.join().expect_err("a compose thread took the state");
        let msg = err
            .downcast_ref::<&str>()
            .map(|s| s.to_string())
            .or_else(|| err.downcast_ref::<String>().cloned())
            .unwrap_or_default();
        assert!(msg.contains("compose thread"), "{msg}");
        // the lock is not poisoned, and other threads go on using it
        let other = std::thread::spawn(|| crate::damage::with(8601, |d| d.is_empty()));
        assert!(other.join().unwrap());
        crate::damage::forget(8601);
    }
}

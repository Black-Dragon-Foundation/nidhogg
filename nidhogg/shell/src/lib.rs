//! nhshell, Nidhogg's native shell layer: a Rust library whose one user is the glue (`nidhogg/glue`).
//!
//! An rlib: Emacs links it through `nidhogg/glue`'s `libnhglue.a`, the one Rust static library in the binary (two
//! would collide on std's symbols), which carries Glitnir's C ABI too.  The Wayland backend sits behind the default
//! feature `wayland`.

pub mod api;
pub mod caps;
pub mod event;
pub mod mac_child;
pub mod mac_chrome;
pub mod mac_input;
#[cfg(all(feature = "macos", target_os = "macos"))]
pub mod macos;
pub mod ring;
pub mod scope;
pub mod shapes;
#[cfg(feature = "svg")]
pub mod svg;
pub mod trace;
pub mod wait;
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

/// macOS: the AppKit backend, on the main thread; there is no display name.
#[cfg(all(not(feature = "wayland"), feature = "macos", target_os = "macos"))]
pub fn open(_display: Option<&CStr>) -> ShellResult<Box<dyn ShellBackend>> {
    Ok(Box::new(macos::MacShell::open()?))
}

/// Without a backend (the Android check builds without `wayland`), nothing can connect.
#[cfg(not(any(feature = "wayland", all(feature = "macos", target_os = "macos"))))]
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

/// Run `f` inside an autorelease pool on macOS, so the objects AppKit autoreleases during a glue call go at its end.
/// On every thread: Emacs's main thread runs no `NSApplication` run loop of its own, so it has no outer pool either.
#[cfg(all(feature = "macos", target_os = "macos"))]
pub fn with_pool<R>(f: impl FnOnce() -> R) -> R {
    objc2::rc::autoreleasepool(|_| f())
}
/// Run `f` (an autorelease pool on macOS).
#[cfg(not(all(feature = "macos", target_os = "macos")))]
pub fn with_pool<R>(f: impl FnOnce() -> R) -> R {
    f()
}

/// Run `f`, turning an Objective-C exception raised in it into a panic, which the glue's guard catches and poisons
/// the display for: an exception that reached `catch_unwind` itself would abort.
#[cfg(all(feature = "macos", target_os = "macos"))]
pub fn catch_objc<R>(f: impl FnOnce() -> R) -> R {
    match objc2::exception::catch(std::panic::AssertUnwindSafe(f)) {
        Ok(r) => r,
        Err(e) => {
            let what = e.map_or_else(|| "nil".to_string(), |e| format!("{e:?}"));
            panic!("an Objective-C exception: {what}")
        }
    }
}
/// Run `f` (an Objective-C exception becomes a panic on macOS).
#[cfg(not(all(feature = "macos", target_os = "macos")))]
pub fn catch_objc<R>(f: impl FnOnce() -> R) -> R {
    f()
}

/// Glitnir's whole API, re-exported so its `#[no_mangle]` functions stay in this archive.
pub use glitnir::*;

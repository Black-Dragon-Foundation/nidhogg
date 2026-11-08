//! The AppKit backend (Naglfar, macOS): `NSApplication`, one `NSWindow` per top-level window whose root view is
//! layer-hosting, and the screens.  Every AppKit call is made on the main thread: the functions that touch AppKit take
//! a `MainThreadMarker`, and the backend refuses to open anywhere else.  The pumps run inside
//! `objc2::exception::catch`; a caught Objective-C exception becomes a panic, which the glue's guard turns into a
//! poisoned display, as it does any panic.

mod app;
mod appdelegate;
mod ax;
mod defaults;
pub mod draw;
mod dialogs;
mod dnd;
pub mod fonts;
pub mod images;
mod ime;
#[cfg(feature = "test-hooks")]
mod inject;
mod kq;
mod layers;
mod overhang;
mod pacer;
mod menus;
mod pasteboard;
mod power;
mod resize;
mod runloop;
mod screens;
mod services;
mod surface;
pub mod text;
pub mod startup;
mod view;
mod window;
mod windowing;

pub use app::MacShell;

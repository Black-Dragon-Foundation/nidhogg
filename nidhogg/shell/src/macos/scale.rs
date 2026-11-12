//! The macOS backend's one read of AppKit's backing scale.  The rule and every conversion are `crate::units`'.

use crate::units::Scale;
use objc2_app_kit::{NSScreen, NSView, NSWindow};

/// Window `w`'s backing scale now, which AppKit decides once the window is on a screen.  Read by `Win::placed` and
/// `Win::scale_now` (the host is told it), by the views for an event's position (the live scale is the event's, in
/// the moment between AppKit's change and the compare it brings), and by the test hooks.
pub fn of_window(w: &NSWindow) -> Scale {
    Scale::of_factor(w.backingScaleFactor())
}

/// The scale of the window view `v` is in (1x outside a window).
pub fn of_view(v: &NSView) -> Scale {
    v.window().map_or(Scale::ONE, |w| of_window(&w))
}

/// Screen `s`'s backing scale.
pub fn of_screen(s: &NSScreen) -> Scale {
    Scale::of_factor(s.backingScaleFactor())
}

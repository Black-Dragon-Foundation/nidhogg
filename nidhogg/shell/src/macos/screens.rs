//! The screens, as `OutputAdd` records: position in points from the main screen's top-left (y down, as NS's
//! monitor list gives it), size in device pixels, the scale times 120.

use crate::event::OutputEv;
use objc2::MainThreadMarker;
use objc2_app_kit::{NSScreen, NSWindow};

/// The record of the screen `window` is on (AppKit's `screen`: the one holding most of it), as [`outputs`] gives it.
pub fn output_of(mtm: MainThreadMarker, window: &NSWindow) -> Option<OutputEv> {
    let screen = window.screen()?;
    let i = NSScreen::screens(mtm)
        .iter()
        .position(|s| std::ptr::eq(&*s, &*screen))?;
    outputs(mtm).get(i).copied()
}

/// One record per screen, the main screen first (id 1 up, in `NSScreen.screens` order).
pub fn outputs(mtm: MainThreadMarker) -> Vec<OutputEv> {
    let screens = NSScreen::screens(mtm);
    // AppKit's y runs up from the main screen's bottom
    let top = screens
        .iter()
        .next()
        .map_or(0.0, |s| s.frame().origin.y + s.frame().size.height);
    screens
        .iter()
        .enumerate()
        .map(|(i, screen)| {
            let frame = screen.frame();
            let scale = super::scale::of_screen(&screen);
            let name = screen.localizedName().to_string();
            let bytes = name.as_bytes();
            let n = bytes.len().min(31);
            let mut buf = [0u8; 32];
            buf[..n].copy_from_slice(&bytes[..n]);
            OutputEv {
                id: i as u64 + 1,
                x: frame.origin.x as i32,
                y: (top - frame.origin.y - frame.size.height) as i32,
                w: scale.px(frame.size.width).round() as i32,
                h: scale.px(frame.size.height).round() as i32,
                mm_w: 0,
                mm_h: 0,
                scale120: scale.s120(),
                name: buf,
            }
        })
        .collect()
}

/// Each screen as AppKit has it, the main one first, for Lisp: `((X Y W H VX VY VW VH SCALE) ...)`, `frame` and
/// `visibleFrame` (the part the menu bar and the Dock leave) in points from the main screen's top left (y down), and
/// `backingScaleFactor`.
pub fn frames_text(mtm: MainThreadMarker) -> String {
    let screens = NSScreen::screens(mtm);
    let top = screens
        .iter()
        .next()
        .map_or(0.0, |s| s.frame().origin.y + s.frame().size.height);
    let rect = |r: objc2_foundation::NSRect| {
        format!(
            "{} {} {} {}",
            r.origin.x,
            top - r.origin.y - r.size.height,
            r.size.width,
            r.size.height
        )
    };
    let all: Vec<String> = screens
        .iter()
        .map(|s| {
            format!(
                "({} {} {})",
                rect(s.frame()),
                rect(s.visibleFrame()),
                super::scale::of_screen(&s).factor()
            )
        })
        .collect();
    format!("({})", all.join(" "))
}

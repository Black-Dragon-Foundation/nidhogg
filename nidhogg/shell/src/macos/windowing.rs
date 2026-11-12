//! Window behaviours: native and non-native full screen, zoom, Spaces and tabbing, the appearance and the title bar,
//! and the application's presentation options.  Variables the Lisp side pushes (`ns-use-native-fullscreen`,
//! `ns-auto-hide-menu-bar`) arrive as `variable` calls.

use super::window::Win;
use crate::vars::Settings;
use objc2::runtime::AnyObject;
use objc2::{msg_send, sel};
use objc2_app_kit::{
    NSAppearance, NSAppearanceCustomization, NSApplication, NSApplicationPresentationOptions,
    NSWindow, NSWindowCollectionBehavior, NSWindowStyleMask, NSWindowTabbingMode,
};
use objc2_foundation::{NSArray, NSString};

/// A new window: full screen is its own Space's, and windows never group into tabs.
pub fn setup(window: &NSWindow) {
    window.setCollectionBehavior(NSWindowCollectionBehavior::FullScreenPrimary);
    window.setTabbingMode(NSWindowTabbingMode::Disallowed);
}

/// The application's presentation options for `auto_hide`.
pub fn presentation(app: &NSApplication, s: &Settings) {
    // AppKit refuses a menu bar that hides without the Dock hiding too (NS sets both)
    let o = if s.auto_hide_menu_bar {
        NSApplicationPresentationOptions::AutoHideMenuBar
            | NSApplicationPresentationOptions::AutoHideDock
    } else {
        NSApplicationPresentationOptions::Default
    };
    app.setPresentationOptions(o);
}

pub fn is_fullscreen(w: &Win) -> bool {
    w.window.styleMask().contains(NSWindowStyleMask::FullScreen) || w.saved.is_some()
}

/// Full screen on or off: AppKit's own, or (non-native) the screen's frame without a title bar.  A request made
/// during a native transition, native or not, waits for the transition's end (`fullscreen_transition`), which
/// compares it with the state the window ends in: the style mask changes when the transition starts, so it cannot be
/// compared with now.  Leaving full screen leaves the kind the window is in, whatever `native` says now.
pub fn fullscreen(w: &mut Win, on: bool, native: bool) {
    if w.fs_busy {
        // AppKit drops a toggle inside a transition
        w.fs_want = Some(on);
        super::runloop::fs_log(8);
        return;
    }
    // a request now replaces one that waited for a transition's end
    w.fs_want = None;
    if on == is_fullscreen(w) {
        return;
    }
    let in_native = w.window.styleMask().contains(NSWindowStyleMask::FullScreen);
    if (on && native) || (!on && in_native) {
        // the transition starts now; its will-notification reaches `fullscreen_transition` only at the next pump,
        // and a request made before that would be dropped by AppKit (NS's `in_fullscreen_transition`)
        w.window.toggleFullScreen(None);
        super::runloop::fs_log(7);
        w.fs_busy = true;
    } else if on {
        w.saved = Some((w.window.frame(), w.window.styleMask()));
        if let Some(screen) = w.window.screen() {
            w.window.setStyleMask(NSWindowStyleMask::Borderless);
            w.window.setFrame_display(screen.frame(), true);
        }
    } else if let Some((frame, style)) = w.saved.take() {
        w.window.setStyleMask(style);
        w.window.setFrame_display(frame, true);
    }
}

/// A native transition started (`busy`) or ended.  At its end, what was asked for during it (full screen, then zoom)
/// goes out on the run loop's next turn (`want`): AppKit is still in the transition while it posts the did-
/// notification, and drops a toggle made then (round 4: an exit asked for while entering never started, and the
/// window stayed in full screen with its transition marked busy for good).  A delegate's event: the AppKit calls go to
/// `effects`, made once the backend's borrow is put back.
pub fn fullscreen_transition(w: &mut Win, busy: bool, effects: &mut Vec<Box<dyn FnOnce()>>) {
    w.fs_busy = busy;
    if !busy && (w.fs_want.is_some() || w.zoom_want.is_some()) {
        let d = w._delegate.clone();
        effects.push(Box::new(move || {
            let modes = NSArray::from_retained_slice(&[NSString::from_str("kCFRunLoopCommonModes")]);
            // SAFETY: `nhmacFullScreenWant:` is defined on the delegate; the run loop retains the target until it
            // fires.
            unsafe {
                let _: () = msg_send![&*d, performSelector: sel!(nhmacFullScreenWant:), withObject: std::ptr::null::<AnyObject>(), afterDelay: 0.0f64, inModes: &*modes];
            }
        }));
    }
}

/// The run loop's turn after a transition's end: the full-screen state asked for during it, unless the window is in
/// it already, then the zoom asked for, once out of full screen.  A new transition meanwhile keeps both for its end.
pub fn want(w: &mut Win, effects: &mut Vec<Box<dyn FnOnce()>>) {
    super::runloop::fs_log(9);
    if w.fs_busy {
        return;
    }
    if let Some(on) = w.fs_want.take()
        && on != is_fullscreen(w)
    {
        let window = w.window.clone();
        effects.push(Box::new(move || {
            window.toggleFullScreen(None);
            super::runloop::fs_log(7);
        }));
        w.fs_busy = true;
        return;
    }
    if !w.window.styleMask().contains(NSWindowStyleMask::FullScreen)
        && let Some(on) = w.zoom_want.take()
    {
        let window = w.window.clone();
        effects.push(Box::new(move || {
            if window.isZoomed() != on {
                window.zoom(None);
            }
        }));
    }
}

/// Zoom (maximize) on or off.  In native full screen or during a transition it waits for the window to be out of full
/// screen: a full-screen window reads as zoomed, and the glue's `fullscreen nil` asks for both full screen and zoom
/// off, so a zoom then would land on the exit's transition (NS's `ns_fullscreen_hook` never zooms there).
pub fn maximize(w: &mut Win, on: bool) {
    if w.fs_busy || w.window.styleMask().contains(NSWindowStyleMask::FullScreen) {
        w.zoom_want = Some(on);
        return;
    }
    if w.window.isZoomed() != on {
        w.window.zoom(None);
    }
}

/// `ns-appearance` (`dark`, `light`, anything else follows the system) and `ns-transparent-titlebar`.
pub fn look(window: &NSWindow, appearance: &str, transparent_titlebar: bool) {
    let name = match appearance {
        "dark" => Some("NSAppearanceNameDarkAqua"),
        "light" => Some("NSAppearanceNameAqua"),
        _ => None,
    };
    window.setAppearance(
        name.and_then(|n| NSAppearance::appearanceNamed(&NSString::from_str(n)))
            .as_deref(),
    );
    window.setTitlebarAppearsTransparent(transparent_titlebar);
}

/// Test hook: the application's appearance dark, or the system's again: what a switch of the system's appearance
/// changes in `effectiveAppearance`.
#[cfg(feature = "test-hooks")]
pub fn test_appearance(app: &NSApplication, dark: bool) {
    use objc2_app_kit::{NSAppearance, NSAppearanceNameDarkAqua};
    // SAFETY: a named system appearance; the statics are AppKit's.
    let a = dark
        .then(|| unsafe { NSAppearance::appearanceNamed(NSAppearanceNameDarkAqua) })
        .flatten();
    app.setAppearance(a.as_deref());
}

/// Whether the system is in dark mode now.
pub fn system_dark(app: &NSApplication) -> bool {
    app.effectiveAppearance()
        .name()
        .to_string()
        .contains("Dark")
}

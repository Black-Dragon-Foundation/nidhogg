//! Window behaviours: native and non-native full screen, zoom, Spaces and tabbing, the appearance and the title bar,
//! and the application's presentation options.  Variables the Lisp side pushes (`ns-use-native-fullscreen`,
//! `ns-auto-hide-menu-bar`) arrive as `variable` calls.

use super::window::Win;
use crate::vars::Settings;
use objc2_app_kit::{
    NSAppearance, NSAppearanceCustomization, NSApplication, NSApplicationPresentationOptions,
    NSWindow, NSWindowCollectionBehavior, NSWindowStyleMask, NSWindowTabbingMode,
};
use objc2_foundation::NSString;

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

/// Full screen on or off: AppKit's own, or (non-native) the screen's frame without a title bar.
pub fn fullscreen(w: &mut Win, on: bool, native: bool) {
    if on == is_fullscreen(w) {
        return;
    }
    if native {
        w.window.toggleFullScreen(None);
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

/// Zoom (maximize) on or off.
pub fn maximize(w: &Win, on: bool) {
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

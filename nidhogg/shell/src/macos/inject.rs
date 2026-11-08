//! Test injection: keys and mouse events built as `NSEvent`s and posted to the application's queue, so the next pump
//! sends them through `sendEvent:` to the view, as the window server's would be.  For `nhmac--test-post-key` and
//! `nhmac--test-post-mouse`.  `real_mouse` posts through the window server itself, for the rows that must show a
//! real click reaching the window.

use super::window::Win;
use crate::api::{ShellError, ShellResult};
use objc2::encode::{Encoding, RefEncode};
use objc2::rc::Retained;
use objc2::{ClassType, MainThreadMarker, msg_send};
use objc2_app_kit::{NSApplication, NSEvent, NSEventModifierFlags, NSEventType, NSScreen};
use objc2_foundation::{NSPoint, NSString};
use std::ffi::c_void;

/// Core Graphics' event, opaque.
#[repr(C)]
struct CGEventOpaque {
    _p: [u8; 0],
}

// SAFETY: `CGEventRef` is a pointer to the opaque `struct __CGEvent`.
unsafe impl RefEncode for CGEventOpaque {
    const ENCODING_REF: Encoding = Encoding::Pointer(&Encoding::Struct("__CGEvent", &[]));
}

#[repr(C)]
#[derive(Clone, Copy)]
struct CGPoint {
    x: f64,
    y: f64,
}

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGEventCreateScrollWheelEvent2(
        source: *const c_void,
        units: u32,
        wheel_count: u32,
        wheel1: i32,
        wheel2: i32,
        wheel3: i32,
    ) -> *mut CGEventOpaque;
    fn CGEventSetLocation(event: *mut CGEventOpaque, location: CGPoint);
    fn CGEventCreateMouseEvent(
        source: *const c_void,
        kind: u32,
        location: CGPoint,
        button: u32,
    ) -> *mut CGEventOpaque;
    fn CGEventPost(tap: u32, event: *mut CGEventOpaque);
    fn CGPreflightPostEventAccess() -> bool;
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFRelease(cf: *const c_void);
}

/// `kCGScrollEventUnitLine`.
const CG_SCROLL_LINE: u32 = 1;
/// `kCGHIDEventTap`: where the window server takes events as if from the hardware.
const CG_HID_TAP: u32 = 0;
/// `kCGEventLeftMouseDown`, `kCGEventLeftMouseUp`, `kCGEventLeftMouseDragged`.
const CG_LEFT_DOWN: u32 = 1;
const CG_LEFT_UP: u32 = 2;
const CG_LEFT_DRAGGED: u32 = 6;

/// The mouse kinds `nhmac--test-post-mouse` takes.
pub const MOUSE_DOWN: i64 = 0;
pub const MOUSE_UP: i64 = 1;
pub const MOUSE_DRAG: i64 = 2;
pub const MOUSE_MOVE: i64 = 3;
pub const MOUSE_WHEEL: i64 = 4;

fn one_char(c: i64) -> ShellResult<Retained<NSString>> {
    if c == 0 {
        // a dead key's characters
        return Ok(NSString::new());
    }
    let c = u32::try_from(c)
        .ok()
        .and_then(char::from_u32)
        .ok_or(ShellError::Arg)?;
    Ok(NSString::from_str(&c.to_string()))
}

/// Make `w` the key window of the active application, so key events reach its view.
fn focus(app: &NSApplication, w: &Win) {
    #[allow(deprecated)]
    app.activateIgnoringOtherApps(true);
    w.window.makeKeyAndOrderFront(None);
}

/// Post a press and a release of the key whose `characters` is `c` (0: none, as a dead key's) and
/// `charactersIgnoringModifiers` `ignoring`, with `NSEventModifierFlags` `flags` and the virtual key code `code` (the
/// input system reads it for dead keys).
pub fn key(
    app: &NSApplication,
    w: &Win,
    c: i64,
    ignoring: i64,
    flags: i64,
    code: u16,
) -> ShellResult {
    focus(app, w);
    let (chars, ignoring) = (one_char(c)?, one_char(ignoring)?);
    let flags = NSEventModifierFlags(usize::try_from(flags).map_err(|_| ShellError::Arg)?);
    for kind in [NSEventType::KeyDown, NSEventType::KeyUp] {
        let ev = NSEvent::keyEventWithType_location_modifierFlags_timestamp_windowNumber_context_characters_charactersIgnoringModifiers_isARepeat_keyCode(
            kind,
            NSPoint::new(0.0, 0.0),
            flags,
            0.0,
            w.window.windowNumber(),
            None,
            &chars,
            &ignoring,
            false,
            code,
        )
        .ok_or(ShellError::State)?;
        app.postEvent_atStart(&ev, false);
    }
    Ok(())
}

/// Post a mouse event of `kind` at `x`, `y` (device pixels from the view's top left): `button` (0 left, 1 right, 2
/// middle) for a press, release or drag, `button` lines (positive down) for a wheel.
pub fn mouse(
    mtm: MainThreadMarker,
    app: &NSApplication,
    w: &Win,
    kind: i64,
    (x, y): (f64, f64),
    button: i64,
    flags: i64,
) -> ShellResult {
    focus(app, w);
    let scale = w.window.backingScaleFactor();
    let height = w.view.bounds().size.height;
    // window coordinates: points from the content's bottom left
    let at = NSPoint::new(x / scale, height - y / scale);
    let flags = NSEventModifierFlags(usize::try_from(flags).map_err(|_| ShellError::Arg)?);
    if kind == MOUSE_WHEEL {
        let lines = i32::try_from(button).map_err(|_| ShellError::Arg)?;
        let screen = w.window.convertPointToScreen(at);
        let top = NSScreen::screens(mtm)
            .firstObject()
            .map_or(0.0, |s| s.frame().size.height);
        // SAFETY: Core Graphics' constructor; the event is released once, after AppKit made its own copy.
        unsafe {
            let cg =
                CGEventCreateScrollWheelEvent2(std::ptr::null(), CG_SCROLL_LINE, 1, -lines, 0, 0);
            if cg.is_null() {
                return Err(ShellError::NoMem);
            }
            CGEventSetLocation(
                cg,
                CGPoint {
                    x: screen.x,
                    y: top - screen.y,
                },
            );
            let ev: Option<Retained<NSEvent>> = msg_send![NSEvent::class(), eventWithCGEvent: cg];
            CFRelease(cg.cast());
            let ev = ev.ok_or(ShellError::State)?;
            app.postEvent_atStart(&ev, false);
        }
        return Ok(());
    }
    let ty = match (kind, button) {
        (MOUSE_DOWN, 0) => NSEventType::LeftMouseDown,
        (MOUSE_UP, 0) => NSEventType::LeftMouseUp,
        (MOUSE_DRAG, 0) => NSEventType::LeftMouseDragged,
        (MOUSE_DOWN, 1) => NSEventType::RightMouseDown,
        (MOUSE_UP, 1) => NSEventType::RightMouseUp,
        (MOUSE_DRAG, 1) => NSEventType::RightMouseDragged,
        (MOUSE_DOWN, _) => NSEventType::OtherMouseDown,
        (MOUSE_UP, _) => NSEventType::OtherMouseUp,
        (MOUSE_DRAG, _) => NSEventType::OtherMouseDragged,
        (MOUSE_MOVE, _) => NSEventType::MouseMoved,
        _ => return Err(ShellError::Arg),
    };
    let ev = NSEvent::mouseEventWithType_location_modifierFlags_timestamp_windowNumber_context_eventNumber_clickCount_pressure(
        ty,
        at,
        flags,
        0.0,
        w.window.windowNumber(),
        None,
        0,
        1,
        if kind == MOUSE_DOWN || kind == MOUSE_DRAG { 1.0 } else { 0.0 },
    )
    .ok_or(ShellError::State)?;
    app.postEvent_atStart(&ev, false);
    Ok(())
}

/// Whether this process may post events through the window server.
pub fn post_access() -> bool {
    // SAFETY: a query with no arguments.
    unsafe { CGPreflightPostEventAccess() }
}

/// Post a real left press (`MOUSE_DOWN`), release or drag at `at` (screen points, origin bottom left) through the
/// window server's HID tap: it reaches whichever window the window server finds there, as a click would.
pub fn real_mouse(mtm: MainThreadMarker, kind: i64, at: (f64, f64)) -> ShellResult {
    let kind = match kind {
        MOUSE_DOWN => CG_LEFT_DOWN,
        MOUSE_UP => CG_LEFT_UP,
        MOUSE_DRAG => CG_LEFT_DRAGGED,
        _ => return Err(ShellError::Arg),
    };
    let top = NSScreen::screens(mtm)
        .firstObject()
        .map_or(0.0, |s| s.frame().size.height);
    let at = CGPoint {
        x: at.0,
        y: top - at.1,
    };
    // SAFETY: Core Graphics' constructor; the event is posted, then released once.
    unsafe {
        let cg = CGEventCreateMouseEvent(std::ptr::null(), kind, at, 0);
        if cg.is_null() {
            return Err(ShellError::NoMem);
        }
        CGEventPost(CG_HID_TAP, cg);
        CFRelease(cg.cast());
    }
    Ok(())
}

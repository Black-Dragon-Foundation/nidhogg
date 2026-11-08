//! Test injection: keys and mouse events built as `NSEvent`s and posted to the application's queue, so the next pump
//! sends them through `sendEvent:` to the view, as the window server's would be.  For `nhmac--test-post-key` and
//! `nhmac--test-post-mouse`.

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
    fn CGEventSetIntegerValueField(event: *mut CGEventOpaque, field: u32, value: i64);
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFRelease(cf: *const c_void);
}

/// `kCGScrollEventUnitLine`.
const CG_SCROLL_LINE: u32 = 1;
/// `kCGMouseEventWindowUnderMousePointer` and `kCGMouseEventWindowUnderMousePointerThatCanHandleThisEvent`: the
/// window an `NSEvent` made from the event names, which `sendEvent:` sends it to.
const CG_WINDOW_FIELDS: [u32; 2] = [91, 92];

/// The mouse kinds `nhmac--test-post-mouse` takes.
pub const MOUSE_DOWN: i64 = 0;
pub const MOUSE_UP: i64 = 1;
pub const MOUSE_DRAG: i64 = 2;
pub const MOUSE_MOVE: i64 = 3;
pub const MOUSE_WHEEL: i64 = 4;

fn one_char(c: i64) -> ShellResult<Retained<NSString>> {
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

/// Post a press and a release of the key whose `characters` is `c` and `charactersIgnoringModifiers` `ignoring`, with
/// `NSEventModifierFlags` `flags`.
pub fn key(app: &NSApplication, w: &Win, c: i64, ignoring: i64, flags: i64) -> ShellResult {
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
            0,
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
        let number = i64::try_from(w.window.windowNumber()).map_err(|_| ShellError::State)?;
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
            // a wheel event the window server did not route has no window: name ours, or AppKit drops it
            for field in CG_WINDOW_FIELDS {
                CGEventSetIntegerValueField(cg, field, number);
            }
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

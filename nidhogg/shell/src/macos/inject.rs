//! Test injection: keys and mouse events built as `NSEvent`s and posted to the application's queue, so the next pump
//! sends them through `sendEvent:` to the view, as the window server's would be.  For `nhmac--test-post-key` and
//! `nhmac--test-post-mouse`.  `real_mouse` posts through the window server itself, for the rows that must show a
//! real click reaching the window.

use super::window::Win;
use crate::api::{ShellError, ShellResult};
use objc2::encode::{Encoding, RefEncode};
use objc2::rc::Retained;
use objc2::{ClassType, MainThreadMarker, msg_send};
use objc2_app_kit::{
    NSApplication, NSEvent, NSEventModifierFlags, NSEventType, NSScreen, NSWorkspace,
};
use objc2_foundation::{NSPoint, NSString};
use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
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
    fn CGEventCreateMouseEvent(
        source: *const c_void,
        kind: u32,
        location: CGPoint,
        button: u32,
    ) -> *mut CGEventOpaque;
    fn CGEventPost(tap: u32, event: *mut CGEventOpaque);
    fn CGEventCreateKeyboardEvent(
        source: *const c_void,
        key: u16,
        down: bool,
    ) -> *mut CGEventOpaque;
    fn CGEventSetFlags(event: *mut CGEventOpaque, flags: u64);
    fn CGPreflightPostEventAccess() -> bool;
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFRelease(cf: *const c_void);
    static kCFBooleanTrue: *const c_void;
}

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn AXUIElementCreateApplication(pid: i32) -> *const c_void;
    fn AXUIElementSetAttributeValue(
        element: *const c_void,
        attribute: *const c_void,
        value: *const c_void,
    ) -> i32;
}

/// `kCGScrollEventUnitPixel` and `kCGScrollEventUnitLine`.
const CG_SCROLL_PIXEL: u32 = 0;
const CG_SCROLL_LINE: u32 = 1;
/// `kCGScrollWheelEventScrollPhase` and `kCGScrollWheelEventMomentumPhase`.
const CG_FIELD_SCROLL_PHASE: u32 = 99;
const CG_FIELD_MOMENTUM_PHASE: u32 = 123;
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
/// The pointer leaving the view at `x`, `y` (outside it): `mouseExited:`, sent to the view as its tracking area would.
pub const MOUSE_EXIT: i64 = 5;
/// A trackpad's precise scroll: `button` points (positive down), `flags` its phases (`PHASE_*`).
pub const MOUSE_PRECISE: i64 = 6;
/// A precise scroll's phases in `flags`: the gesture's (`NSEventPhaseChanged`), or momentum's after the fingers left
/// (`kCGMomentumScrollPhaseContinue`).
pub const PHASE_GESTURE: i64 = 0;
pub const PHASE_MOMENTUM: i64 = 1;

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

/// A press and a release of the function key `c` (`NSF1FunctionKey` … `NSF35FunctionKey`) posted at the end of the
/// queue for window number `window`, with no focus change: the wake `test-wake-after` arms.
pub fn function_key(app: &NSApplication, window: isize, c: u16) {
    let chars = NSString::from_str(
        &char::from_u32(u32::from(c))
            .map(String::from)
            .unwrap_or_default(),
    );
    for kind in [NSEventType::KeyDown, NSEventType::KeyUp] {
        if let Some(ev) = NSEvent::keyEventWithType_location_modifierFlags_timestamp_windowNumber_context_characters_charactersIgnoringModifiers_isARepeat_keyCode(
            kind,
            NSPoint::new(0.0, 0.0),
            NSEventModifierFlags(0),
            0.0,
            window,
            None,
            &chars,
            &chars,
            false,
            0,
        ) {
            app.postEvent_atStart(&ev, false);
        }
    }
}

/// The subtype of the application-defined event a posted press, release, drag or move waits as.
const POSTED: i16 = 0x6e6d;

/// A posted mouse event, made when the pump drains it: its screen point (points, y up) and the window's number.
struct Posted {
    ty: NSEventType,
    screen: NSPoint,
    flags: NSEventModifierFlags,
    window: isize,
    pressure: f32,
}

thread_local! {
    /// The posted mouse events, in their order in the application's queue.
    static POSTED_EVENTS: RefCell<VecDeque<Posted>> = const { RefCell::new(VecDeque::new()) };
    /// While a posted button is down: the content's top left on the screen at the press.
    static PRESS: Cell<Option<NSPoint>> = const { Cell::new(None) };
}

/// How many posted mouse events the pump has not sent yet (`test-posted`): 0 once every one posted is delivered.
pub fn posted_pending() -> usize {
    POSTED_EVENTS.with(|q| q.borrow().len())
}

/// The content's top left on the screen, in points (y up).
fn content_top_left(w: &Win) -> NSPoint {
    let c = w.window.contentRectForFrameRect(w.window.frame());
    NSPoint::new(c.origin.x, c.origin.y + c.size.height)
}

/// Send `ev` if it is a posted mouse event (true): made now, at its screen point in the window's frame of now.  A
/// resize step's commit runs in the pump before the drain and moves the window's origin; a location in window
/// coordinates made at the post moved with it, so a drag of the left or top edge, or one about the centre, chased
/// its own steps (the live session, 2026-11-13: the left edge grew 1500 points for a 60-point drag).
pub fn send_posted(app: &NSApplication, ev: &NSEvent) -> bool {
    if ev.r#type() != NSEventType::ApplicationDefined || ev.subtype().0 != POSTED {
        return false;
    }
    let Some(p) = POSTED_EVENTS.with(|q| q.borrow_mut().pop_front()) else {
        return true;
    };
    let Some(window) = app.windowWithWindowNumber(p.window) else {
        return true;
    };
    let made = NSEvent::mouseEventWithType_location_modifierFlags_timestamp_windowNumber_context_eventNumber_clickCount_pressure(
        p.ty,
        window.convertPointFromScreen(p.screen),
        p.flags,
        0.0,
        p.window,
        None,
        0,
        1,
        p.pressure,
    );
    if let Some(made) = made {
        app.sendEvent(&made);
    }
    true
}

/// Post a mouse event of `kind` at `x`, `y` (device pixels from the view's top left; while a posted button is down,
/// from where the view's top left was at the press): `button` (0 left, 1 right, 2 middle) for a press, release or
/// drag, `button` lines (positive down) for a wheel, points for a precise scroll.
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
    let height = w.view.bounds().size.height;
    // window coordinates: points from the content's bottom left
    let (px, py) = w.scale().view_point(x, y, height);
    let at = NSPoint::new(px, py);
    if kind == MOUSE_WHEEL || kind == MOUSE_PRECISE {
        let n = i32::try_from(button).map_err(|_| ShellError::Arg)?;
        let (units, momentum) = match (kind, flags) {
            (MOUSE_WHEEL, _) => (CG_SCROLL_LINE, None),
            (_, PHASE_GESTURE) => (CG_SCROLL_PIXEL, Some(false)),
            (_, PHASE_MOMENTUM) => (CG_SCROLL_PIXEL, Some(true)),
            _ => return Err(ShellError::Arg),
        };
        let top = NSScreen::screens(mtm)
            .firstObject()
            .map_or(0.0, |s| s.frame().size.height);
        // A scroll event made here has no window: AppKit routes a wheel to the window the window server named, and
        // drops one it did not route.  So it goes to the view itself, as `sendEvent:` would hand it, at a location
        // that reads as the window point `at` (an event with no window reports its screen location, which is the
        // Core Graphics one flipped against the main screen).
        // SAFETY: Core Graphics' constructor; the event is released once, after AppKit made its own copy.
        unsafe {
            let cg = CGEventCreateScrollWheelEvent2(std::ptr::null(), units, 1, -n, 0, 0);
            if cg.is_null() {
                return Err(ShellError::NoMem);
            }
            // a pixel event is continuous (precise deltas); its phase: the gesture's changed (4), or none and
            // momentum's continue (2)
            match momentum {
                Some(false) => CGEventSetIntegerValueField(cg, CG_FIELD_SCROLL_PHASE, 4),
                Some(true) => CGEventSetIntegerValueField(cg, CG_FIELD_MOMENTUM_PHASE, 2),
                None => {}
            }
            CGEventSetLocation(
                cg,
                CGPoint {
                    x: at.x,
                    y: top - at.y,
                },
            );
            let ev: Option<Retained<NSEvent>> = msg_send![NSEvent::class(), eventWithCGEvent: cg];
            CFRelease(cg.cast());
            let ev = ev.ok_or(ShellError::State)?;
            let () = msg_send![&*w.view, scrollWheel: &*ev];
        }
        return Ok(());
    }
    let flags = NSEventModifierFlags(usize::try_from(flags).map_err(|_| ShellError::Arg)?);
    if kind == MOUSE_EXIT {
        // AppKit hands an exit to its tracking area's owner itself; a posted one reaches no area
        // SAFETY: AppKit's constructor with null user data; the view takes the event as AppKit would give it.
        unsafe {
            let ev = NSEvent::enterExitEventWithType_location_modifierFlags_timestamp_windowNumber_context_eventNumber_trackingNumber_userData(
                NSEventType::MouseExited,
                at,
                flags,
                0.0,
                w.window.windowNumber(),
                None,
                0,
                0,
                std::ptr::null_mut(),
            )
            .ok_or(ShellError::State)?;
            let () = msg_send![&*w.view, mouseExited: &*ev];
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
    // the screen point: from the content's top left now, or at the press while the button is down
    let origin = match kind {
        MOUSE_DOWN => {
            let o = content_top_left(w);
            PRESS.with(|p| p.set(Some(o)));
            o
        }
        MOUSE_DRAG => PRESS.with(Cell::get).unwrap_or_else(|| content_top_left(w)),
        MOUSE_UP => PRESS
            .with(Cell::take)
            .unwrap_or_else(|| content_top_left(w)),
        _ => content_top_left(w),
    };
    let scale = w.scale();
    let screen = NSPoint::new(origin.x + scale.pt(x), origin.y - scale.pt(y));
    let marker = NSEvent::otherEventWithType_location_modifierFlags_timestamp_windowNumber_context_subtype_data1_data2(
        NSEventType::ApplicationDefined,
        NSPoint::new(0.0, 0.0),
        NSEventModifierFlags(0),
        0.0,
        0,
        None,
        POSTED,
        0,
        0,
    )
    .ok_or(ShellError::State)?;
    POSTED_EVENTS.with(|q| {
        q.borrow_mut().push_back(Posted {
            ty,
            screen,
            flags,
            window: w.window.windowNumber(),
            pressure: if kind == MOUSE_DOWN || kind == MOUSE_DRAG {
                1.0
            } else {
                0.0
            },
        });
    });
    app.postEvent_atStart(&marker, false);
    Ok(())
}

/// Whether this application is active and `w` its key window, which application is frontmost, and whether this one
/// may use Accessibility: `(:active :key :frontmost :frontmost-pid :pid :ax-trusted)`.  Since macOS 14 activation is
/// cooperative: an inactive application has no key window, so the window server's keys reach nothing and a press on
/// its window is an activating click (test diagnostics: the input rows say which it was).
pub fn app_state(app: &NSApplication, w: &Win) -> String {
    let b = |v: bool| if v { "t" } else { "nil" };
    let key = app.keyWindow().is_some_and(|k| {
        Retained::as_ptr(&k).cast::<c_void>() == Retained::as_ptr(&w.window).cast::<c_void>()
    });
    let (front, front_pid) = NSWorkspace::sharedWorkspace()
        .frontmostApplication()
        .map_or((String::new(), -1), |a| {
            (
                a.bundleIdentifier()
                    .map_or_else(String::new, |s| s.to_string()),
                a.processIdentifier(),
            )
        });
    // round 11: active and key, yet the keys that go through the text input system (Option, Command, dead keys,
    // marked text) reached nothing after the full-screen and Dock rows: who takes the window's keys, and its input
    // context
    let responder = w.window.firstResponder();
    let is_view = responder.as_ref().is_some_and(|r| {
        Retained::as_ptr(r).cast::<c_void>() == Retained::as_ptr(&w.view).cast::<c_void>()
    });
    let class = responder
        .as_ref()
        .map_or_else(|| "nil".to_owned(), |r| format!("{:?}", r.class().name()));
    // SAFETY: NSTextInputContext's class property and NSResponder's `inputContext`, read on the main thread.
    let (context, own): (
        Option<Retained<objc2::runtime::AnyObject>>,
        Option<Retained<objc2::runtime::AnyObject>>,
    ) = unsafe {
        (
            msg_send![objc2::class!(NSTextInputContext), currentInputContext],
            msg_send![&*w.view, inputContext],
        )
    };
    // the current context is the view's own: AppKit routes the text input system's keys through it
    let current_is_own = matches!((&context, &own), (Some(c), Some(o))
        if Retained::as_ptr(c) == Retained::as_ptr(o));
    format!(
        "(:active {} :key {} :frontmost {front:?} :frontmost-pid {front_pid} :pid {} :ax-trusted {} \
         :first-responder {class} :responder-is-view {} :input-context {} :view-context {} :current-is-view-context {} \
         :key-window-number {} :window-number {})",
        b(app.isActive()),
        b(key),
        std::process::id(),
        b(super::nsapp::ax_trusted()),
        b(is_view),
        b(context.is_some()),
        b(own.is_some()),
        b(current_is_own),
        app.keyWindow().map_or(-1, |k| k.windowNumber()),
        w.window.windowNumber()
    )
}

/// Make this application frontmost through Accessibility (its `AXFrontmost`, as automation tools raise an
/// application; cooperative activation does not refuse it as it may refuse `activate`), then `w` key and in front.
/// The AX error (0: done; -1: no element).
pub fn ax_activate(app: &NSApplication, w: &Win) -> i32 {
    let attr = NSString::from_str("AXFrontmost");
    let pid = i32::try_from(std::process::id()).unwrap_or(-1);
    // SAFETY: an element made and released here; the attribute is a CFString (toll-free bridged NSString) and the
    // value the CFBoolean constant.
    let rc = unsafe {
        let el = AXUIElementCreateApplication(pid);
        if el.is_null() {
            return -1;
        }
        let rc = AXUIElementSetAttributeValue(el, Retained::as_ptr(&attr).cast(), kCFBooleanTrue);
        CFRelease(el);
        rc
    };
    focus(app, w);
    rc
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

/// Post a real press and release of the key with virtual key code `code` and `CGEventFlags` `flags` (Option is
/// 1 << 19, as `NSEventModifierFlagOption`) through the window server's HID tap: it reaches the key window as typing
/// would, translated by the current keyboard layout there (a dead key's state included), which an event the app posts
/// itself (`key`) never is.
pub fn real_key(code: u16, flags: u64) -> ShellResult {
    for down in [true, false] {
        // SAFETY: Core Graphics' constructor; the event is posted, then released once.
        unsafe {
            let ev = CGEventCreateKeyboardEvent(std::ptr::null(), code, down);
            if ev.is_null() {
                return Err(ShellError::NoMem);
            }
            CGEventSetFlags(ev, flags);
            CGEventPost(CG_HID_TAP, ev);
            CFRelease(ev.cast());
        }
    }
    Ok(())
}

/// Post a real left drag through the window server from `from` to `to` (screen points, origin bottom left): the press
/// now, then from another thread, after `delay`, `steps` moves 20 ms apart and the release at `to`.  The caller starts
/// a drag (`beginDraggingSession`) once the press has arrived; the moves and the release then reach that session as the
/// pointer's would.  For the drag-out row.
pub fn real_drag(
    mtm: MainThreadMarker,
    from: (f64, f64),
    to: (f64, f64),
    delay: std::time::Duration,
    steps: u32,
) -> ShellResult {
    let top = NSScreen::screens(mtm)
        .firstObject()
        .map_or(0.0, |s| s.frame().size.height);
    let cg = move |(x, y): (f64, f64)| CGPoint { x, y: top - y };
    let post = move |kind: u32, at: CGPoint| {
        // SAFETY: Core Graphics' constructor; the event is posted, then released once.  Both calls are thread-safe.
        unsafe {
            let ev = CGEventCreateMouseEvent(std::ptr::null(), kind, at, 0);
            if !ev.is_null() {
                CGEventPost(CG_HID_TAP, ev);
                CFRelease(ev.cast());
            }
        }
    };
    post(CG_LEFT_DOWN, cg(from));
    std::thread::spawn(move || {
        std::thread::sleep(delay);
        let n = steps.max(1);
        for i in 1..=n {
            let t = f64::from(i) / f64::from(n);
            post(
                CG_LEFT_DRAGGED,
                cg((from.0 + (to.0 - from.0) * t, from.1 + (to.1 - from.1) * t)),
            );
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        post(CG_LEFT_UP, cg(to));
    });
    Ok(())
}

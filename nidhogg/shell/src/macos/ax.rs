//! Accessibility and Zoom, from the state C pushed at the last consistency point (`ime::Pushed`): the root view is a
//! text area whose value is the line around point and whose selected text is the region within it, its
//! `accessibilityFrame` is the cursor's rectangle on the screen (as NS's), and Zoom follows the cursor: when Zoom is on
//! and the cursor moved, `UAZoomChangeFocus` gets its rectangle, as NS calls it from `ns_update_end`.

use super::ime;
use super::view::NhView;
use crate::api::WindowId;
use objc2_app_kit::{
    NSAccessibilityFocusedUIElementChangedNotification, NSAccessibilityPostNotification,
};
use objc2_foundation::{NSPoint, NSRect, NSSize};
use std::cell::Cell;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

#[repr(C)]
#[derive(Clone, Copy)]
struct CGRect {
    x: f64,
    y: f64,
    w: f64,
    h: f64,
}

/// `kUAZoomFocusTypeInsertionPoint`.
const ZOOM_INSERTION_POINT: u32 = 1;

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn UAZoomEnabled() -> u8;
    fn UAZoomChangeFocus(rect: *const CGRect, highlight: *const CGRect, kind: u32) -> i32;
}

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGMainDisplayID() -> u32;
    fn CGDisplayBounds(display: u32) -> CGRect;
}

/// `UAZoomChangeFocus` calls made (`(nhmac-stats)`'s `:zoom-calls`).
pub static ZOOM_CALLS: AtomicU64 = AtomicU64::new(0);

/// How long a `UAZoomEnabled` answer is kept: the call is a round trip to the accessibility server, made at every
/// cursor push, and turning Zoom on takes the user longer than this.
const ZOOM_CACHE: Duration = Duration::from_secs(1);

thread_local! {
    /// The last `UAZoomEnabled` answer and when it was asked.
    static ENABLED: Cell<Option<(bool, Instant)>> = const { Cell::new(None) };
    /// Zoom was on at the last cursor push: the first push after it comes on moves Zoom even with the cursor where
    /// it was.
    static WAS_ON: Cell<bool> = const { Cell::new(false) };
    /// The cursor rectangle Zoom was last given.
    static LAST: Cell<[i32; 4]> = const { Cell::new([0; 4]) };
    /// Test hook: Zoom counted as on.
    static FORCE: Cell<bool> = const { Cell::new(false) };
}

#[cfg(feature = "test-hooks")]
thread_local! {
    /// Test hook: each Zoom move since the last `test_zoom_log`: the cursor rectangle and whether it was the first after
    /// Zoom came on.
    static MOVES: std::cell::RefCell<Vec<([i32; 4], bool)>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// Test hook (`test-zoom-log`): the Zoom moves since the last call, as `((X Y W H FIRST) ...)`, and forget them.
#[cfg(feature = "test-hooks")]
pub fn test_zoom_log() -> String {
    let moves = MOVES.with(|m| std::mem::take(&mut *m.borrow_mut()));
    let items: Vec<String> = moves
        .iter()
        .map(|(c, first)| {
            let first = if *first { "t" } else { "nil" };
            format!("({} {} {} {} {first})", c[0], c[1], c[2], c[3])
        })
        .collect();
    format!("({})", items.join(" "))
}

fn zoom_on() -> bool {
    if FORCE.with(Cell::get) {
        return true;
    }
    let now = Instant::now();
    if let Some((on, at)) = ENABLED.with(Cell::get)
        && now.duration_since(at) < ZOOM_CACHE
    {
        return on;
    }
    // SAFETY: a query with no arguments.
    let on = unsafe { UAZoomEnabled() } != 0;
    ENABLED.with(|e| e.set(Some((on, now))));
    on
}

/// Window `id`'s cursor rectangle, `cols` cursor widths wide, on the screen (points, origin bottom left): what
/// `firstRectForCharacterRange:` and `accessibilityFrame` answer.
pub fn cursor_on_screen(view: &NhView, id: WindowId, cols: f64) -> NSRect {
    let [x, y, w, h] = ime::with(id, |p| p.cursor);
    let height = view.bounds().size.height;
    let r = NSRect::new(
        NSPoint::new(f64::from(x), height - f64::from(y) - f64::from(h)),
        NSSize::new(f64::from(w) * cols, f64::from(h)),
    );
    let r = view.convertRect_toView(r, None);
    view.window().map_or(r, |win| win.convertRectToScreen(r))
}

/// The cursor was pushed for window `id`: give Zoom its rectangle if Zoom is on and it moved.
pub fn cursor_pushed(view: &NhView, id: WindowId) {
    if !zoom_on() {
        WAS_ON.with(|w| w.set(false));
        return;
    }
    let force = !WAS_ON.with(|w| w.replace(true));
    let cursor = ime::with(id, |p| p.cursor);
    if cursor[2] == 0 && cursor[3] == 0 {
        return;
    }
    // a move is a new place: the same place with another width is the cursor redrawn (round 4: a move pushed 1 wide,
    // then 7 wide when the cursor was drawn, and Zoom got two calls)
    if !force && LAST.with(Cell::get)[..2] == cursor[..2] {
        return;
    }
    LAST.with(|l| l.set(cursor));
    let r = cursor_on_screen(view, id, 1.0);
    // SAFETY: Core Graphics' main display and its bounds.
    let top = unsafe { CGDisplayBounds(CGMainDisplayID()) }.h;
    // Core Graphics' coordinates: from the main display's top left; Zoom ignores a tiny rectangle
    let rect = CGRect {
        x: r.origin.x,
        y: top - r.origin.y - r.size.height,
        w: r.size.width.max(6.0),
        h: r.size.height.max(10.0),
    };
    // SAFETY: rectangles that live across the calls; no highlight; the view is a live accessibility element.
    unsafe {
        if force {
            // Zoom moves only when the position changes: a step aside first, as NS does when Zoom comes on
            let aside = CGRect {
                x: rect.x + 1.0,
                y: rect.y + 1.0,
                ..rect
            };
            UAZoomChangeFocus(&aside, std::ptr::null(), ZOOM_INSERTION_POINT);
        }
        UAZoomChangeFocus(&rect, std::ptr::null(), ZOOM_INSERTION_POINT);
        NSAccessibilityPostNotification(view, NSAccessibilityFocusedUIElementChangedNotification);
    }
    ZOOM_CALLS.fetch_add(1, Ordering::Relaxed);
    #[cfg(feature = "test-hooks")]
    MOVES.with(|m| m.borrow_mut().push((cursor, force)));
}

/// Test hook: count Zoom as on (`true`) or ask the system again.
#[cfg(feature = "test-hooks")]
pub fn test_force_zoom(on: bool) {
    FORCE.with(|f| f.set(on));
    WAS_ON.with(|w| w.set(false));
}

/// Test hook (`test-ax`): what an assistive application reads from the view, through its accessibility methods.
#[cfg(feature = "test-hooks")]
pub fn test_state(view: &NhView, window: &objc2_app_kit::NSWindow) -> String {
    use objc2::msg_send;
    use objc2::rc::Retained;
    use objc2_foundation::NSString;
    let rect = |r: NSRect| {
        format!(
            "({} {} {} {})",
            r.origin.x.round(),
            r.origin.y.round(),
            r.size.width.round(),
            r.size.height.round()
        )
    };
    let text = |s: Option<Retained<NSString>>| {
        s.map_or("nil".into(), |s| {
            crate::pasteboard::lisp_string(&s.to_string())
        })
    };
    // SAFETY: the view is the NhView; these are the accessibility methods it defines, with their return types.
    unsafe {
        let element: bool = msg_send![view, isAccessibilityElement];
        let role: Option<Retained<NSString>> = msg_send![view, accessibilityRole];
        let value: Option<Retained<NSString>> = msg_send![view, accessibilityValue];
        let selected: Option<Retained<NSString>> = msg_send![view, accessibilitySelectedText];
        let frame: NSRect = msg_send![view, accessibilityFrame];
        format!(
            "(:element {} :role {} :value {} :selected {} :frame {} :window {})",
            if element { "t" } else { "nil" },
            text(role),
            text(value),
            text(selected),
            rect(frame),
            rect(window.frame())
        )
    }
}

//! The application calls behind the `ns-*` functions NS defines in C (`lisp/term/nhmac-win.el`'s compatibility
//! layer): the Dock tile's badge and progress, the attention request, the power activity that blocks idle sleep,
//! AppleScript, the Accessibility trust, the pointer, the stacking order and the sharing picker.  Each does what
//! `nsfns.m` does.

use super::ax;
use super::view::NhView;
use crate::api::WindowId;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2::{AnyThread, MainThreadMarker, MainThreadOnly, msg_send};
use objc2_app_kit::{
    NSApplication, NSEvent, NSImageView, NSLevelIndicator, NSLevelIndicatorStyle,
    NSRequestUserAttentionType, NSScreen, NSSharingServicePicker, NSWindow, NSWindowOrderingMode,
};
use objc2_foundation::{
    NSActivityOptions, NSAppleEventDescriptor, NSAppleScript, NSArray, NSFileManager,
    NSObjectProtocol, NSPoint, NSProcessInfo, NSRect, NSRectEdge, NSSize, NSString, NSURL,
};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;

type Activity = Retained<ProtocolObject<dyn NSObjectProtocol>>;

thread_local! {
    /// The outstanding attention request (`requestUserAttention:`), if any.
    static ATTENTION: Cell<Option<isize>> = const { Cell::new(None) };
    /// The activities blocking idle sleep, by the token Lisp holds, and the last token given.
    static SLEEP: RefCell<(u32, HashMap<u32, Activity>)> = RefCell::new((0, HashMap::new()));
}

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn AXIsProcessTrusted() -> u8;
}

#[repr(C)]
#[derive(Clone, Copy)]
struct CGPoint {
    x: f64,
    y: f64,
}

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGWarpMouseCursorPosition(p: CGPoint) -> i32;
}

/// The text of a call's one string argument, "(TEXT)".
pub fn text_arg(arg: &str) -> &str {
    let inner = arg.strip_prefix('(').unwrap_or(arg);
    inner.strip_suffix(')').unwrap_or(inner)
}

/// The main screen's top, in AppKit's coordinates (y up from its bottom): NS's origin for absolute positions.
fn main_top(mtm: MainThreadMarker) -> f64 {
    NSScreen::screens(mtm)
        .iter()
        .next()
        .map_or(0.0, |s| s.frame().origin.y + s.frame().size.height)
}

/// `ns-badge`: the Dock tile's badge, `text` (empty: none).
pub fn badge(app: &NSApplication, text: &str) {
    let label = (!text.is_empty()).then(|| NSString::from_str(text));
    app.dockTile().setBadgeLabel(label.as_deref());
}

/// `ns-request-user-attention`: `urgency` 0 cancels the outstanding request, 1 informational, 2 critical.
pub fn attention(app: &NSApplication, urgency: i64) {
    if let Some(id) = ATTENTION.with(Cell::take) {
        app.cancelUserAttentionRequest(id);
    }
    let kind = match urgency {
        1 => NSRequestUserAttentionType::InformationalRequest,
        2 => NSRequestUserAttentionType::CriticalRequest,
        _ => return,
    };
    ATTENTION.with(|a| a.set(Some(app.requestUserAttention(kind))));
}

/// `ns-progress-indicator`: a level indicator over the Dock tile's icon at `progress` (0 to 1); hidden outside that.
pub fn progress(mtm: MainThreadMarker, app: &NSApplication, progress: f64) {
    let tile = app.dockTile();
    let content = tile.contentView(mtm).unwrap_or_else(|| {
        let image = NSImageView::new(mtm);
        image.setImage(app.applicationIconImage().as_deref());
        tile.setContentView(Some(&image));
        Retained::into_super(Retained::into_super(image))
    });
    let existing = content
        .subviews()
        .iter()
        .find_map(|v| v.downcast::<NSLevelIndicator>().ok());
    let level = existing.unwrap_or_else(|| {
        let size = tile.size();
        let l = NSLevelIndicator::initWithFrame(
            NSLevelIndicator::alloc(mtm),
            NSRect::new(
                NSPoint::new(0.0, 0.0),
                NSSize::new(size.width, 0.1 * size.height),
            ),
        );
        l.setLevelIndicatorStyle(NSLevelIndicatorStyle::ContinuousCapacity);
        l.setMinValue(0.0);
        l.setMaxValue(1.0);
        l.setEnabled(false);
        content.addSubview(&l);
        l
    });
    let shown = (0.0..=1.0).contains(&progress);
    level.setDoubleValue(if shown { progress } else { 0.0 });
    level.setHidden(!shown);
    tile.display();
}

/// `ns-block-system-sleep`: a power activity that keeps the system (and, unless `display_may_sleep`, the display)
/// awake, for `why`; the token that ends it.
pub fn sleep_block(why: &str, display_may_sleep: bool) -> String {
    let mut options = NSActivityOptions::UserInitiated | NSActivityOptions::IdleSystemSleepDisabled;
    if !display_may_sleep {
        options |= NSActivityOptions::IdleDisplaySleepDisabled;
    }
    let reason = NSString::from_str(if why.is_empty() { "Emacs" } else { why });
    let activity = NSProcessInfo::processInfo().beginActivityWithOptions_reason(options, &reason);
    SLEEP.with(|s| {
        let mut s = s.borrow_mut();
        s.0 += 1;
        let token = s.0;
        s.1.insert(token, activity);
        token.to_string()
    })
}

/// `ns-unblock-system-sleep`: end the activity `token` names; `t` when there was one.
pub fn sleep_unblock(token: u32) -> String {
    match SLEEP.with(|s| s.borrow_mut().1.remove(&token)) {
        Some(activity) => {
            // SAFETY: an activity `beginActivityWithOptions:reason:` returned, ended once.
            unsafe { NSProcessInfo::processInfo().endActivity(&activity) };
            "t".into()
        }
        None => "nil".into(),
    }
}

/// A four-character Apple event code.
const fn four(c: &[u8; 4]) -> u32 {
    u32::from_be_bytes(*c)
}

/// `ns-do-applescript`: `script`'s result as NS reads it: `nil` for none, a text result as a Lisp string, any other
/// as the number its text coerces to (`t` when it has no text); `(error MESSAGE)` when the script fails.
pub fn applescript(script: &str) -> String {
    let error = || {
        format!(
            "(error {})",
            crate::pasteboard::lisp_string("AppleScript error")
        )
    };
    let Some(s) =
        NSAppleScript::initWithSource(NSAppleScript::alloc(), &NSString::from_str(script))
    else {
        return error();
    };
    let mut info: Option<Retained<AnyObject>> = None;
    // SAFETY: the script runs on the calling (main) thread; `info` is a writable slot for the error dictionary.  The
    // result is nil when the script fails (the generated binding says never, and would panic on it)
    let result: Option<Retained<NSAppleEventDescriptor>> =
        unsafe { msg_send![&*s, executeAndReturnError: Some(&mut info)] };
    let Some(result) = result.filter(|_| info.is_none()) else {
        return error();
    };
    // SAFETY: `descriptorType` and `coerceToDescriptorType:` of the result descriptor (a four-character code each;
    // the generated bindings leave them out with Core Services' types)
    let (kind, text) = unsafe {
        let kind: u32 = msg_send![&*result, descriptorType];
        let text: Option<Retained<NSAppleEventDescriptor>> =
            msg_send![&*result, coerceToDescriptorType: four(b"utf8")];
        (kind, text)
    };
    if kind == four(b"null") {
        return "nil".into();
    }
    let Some(text) = text else {
        return "t".into();
    };
    if [four(b"utxt"), four(b"ut16"), four(b"utf8"), four(b"TEXT")].contains(&kind) {
        text.stringValue().map_or("t".into(), |t| {
            crate::pasteboard::lisp_string(&t.to_string())
        })
    } else {
        text.int32Value().to_string()
    }
}

/// `ns-process-is-accessibility-trusted`.
pub fn ax_trusted() -> bool {
    // SAFETY: a query with no arguments.
    unsafe { AXIsProcessTrusted() != 0 }
}

/// `ns-mouse-absolute-pixel-position`: the pointer, in points from the main screen's top left, as "(X . Y)".
pub fn mouse_position(mtm: MainThreadMarker) -> String {
    let p = NSEvent::mouseLocation();
    format!(
        "({} . {})",
        p.x.round() as i64,
        (main_top(mtm) - p.y).round() as i64
    )
}

/// `ns-set-mouse-absolute-pixel-position`: the pointer to (`x`, `y`) points from the main screen's top left (Core
/// Graphics' global coordinates).
pub fn warp(x: f64, y: f64) {
    // SAFETY: a point by value.
    unsafe { CGWarpMouseCursorPosition(CGPoint { x, y }) };
}

/// `ns-frame-list-z-order`: the places in `windows` in AppKit's front-to-back order (windows not on screen left out).
pub fn z_order(app: &NSApplication, windows: &[&NSWindow]) -> String {
    let places: Vec<String> = app
        .orderedWindows()
        .iter()
        .filter_map(|w| windows.iter().position(|ours| std::ptr::eq(*ours, &*w)))
        .map(|i| i.to_string())
        .collect();
    format!("({})", places.join(" "))
}

/// `ns-frame-restack`: `window` just above (or below) `other`.
pub fn restack(window: &NSWindow, other: &NSWindow, above: bool) {
    let mode = if above {
        NSWindowOrderingMode::Above
    } else {
        NSWindowOrderingMode::Below
    };
    window.orderWindow_relativeTo(mode, other.windowNumber());
}

/// `ns-send-items`: the sharing picker for `items` (an existing file as its URL, anything else as text), under
/// window `id`'s cursor.
pub fn send_items(view: &NhView, id: WindowId, items: &[&str]) {
    let fm = NSFileManager::defaultManager();
    let objs: Vec<Retained<AnyObject>> = items
        .iter()
        .map(|i| {
            let s = NSString::from_str(i);
            if fm.fileExistsAtPath(&s) {
                Retained::into_super(Retained::into_super(NSURL::fileURLWithPath(&s)))
            } else {
                Retained::into_super(Retained::into_super(s))
            }
        })
        .collect();
    // SAFETY: the picker takes any objects it can share; these are URLs and strings.
    let picker = unsafe {
        NSSharingServicePicker::initWithItems(
            NSSharingServicePicker::alloc(),
            &NSArray::from_retained_slice(&objs),
        )
    };
    let cursor = ax::cursor_on_screen(view, id, 1.0);
    let at = view.window().map_or(cursor, |w| {
        let r = w.convertRectFromScreen(cursor);
        view.convertRect_fromView(r, None)
    });
    picker.showRelativeToRect_ofView_preferredEdge(at, view, NSRectEdge::MinY);
}

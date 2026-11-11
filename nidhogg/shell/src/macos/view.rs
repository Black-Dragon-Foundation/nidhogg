//! A window's root view: layer-hosting, so Core Animation shows our layers as they are and AppKit draws nothing; and
//! the window's first responder, so its keys and mouse arrive here.  Each becomes the shared event record
//! (`crate::mac_input`) and reaches the backend through the scope (`runloop::deliver`).

use super::ax;
use super::dnd;
use super::ime;
use super::runloop::{MacEv, deliver};
use crate::api::WindowId;
use crate::event::{
    KeyEv, NHSHELL_BUTTON_PRESSED, NHSHELL_BUTTON_RELEASED, NHSHELL_PINCH_BEGIN, NHSHELL_PINCH_END,
    NHSHELL_PINCH_UPDATE, PinchEv, PointerEv, ShellEvent,
};
use crate::mac_input;
use crate::outputs::Magnify;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject, Sel};
use objc2::{
    AnyThread, ClassType, DefinedClass, MainThreadMarker, MainThreadOnly, Message, define_class,
    msg_send, sel,
};
use objc2_app_kit::{
    NSAccessibilityTextAreaRole, NSDragOperation, NSDraggingDestination, NSDraggingInfo, NSEvent,
    NSEventModifierFlags, NSEventPhase, NSResponder, NSTextInputClient, NSTrackingArea,
    NSTrackingAreaOptions, NSView,
};
use objc2_foundation::{
    NSArray, NSAttributedString, NSNotFound, NSObjectProtocol, NSRange, NSString,
};
use objc2_foundation::{NSPoint, NSRect, NSSize};
use objc2_quartz_core::CALayer;
use std::cell::RefCell;

thread_local! {
    /// The scale of the magnify gesture now running.
    static MAGNIFY: RefCell<Magnify> = RefCell::new(Magnify::default());
}

define_class!(
    /// The root view of window `ivars`.
    #[unsafe(super(NSView, NSResponder, objc2_foundation::NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "NhmacView"]
    #[ivars = WindowId]
    pub struct NhView;

    unsafe impl NSObjectProtocol for NhView {}

    unsafe impl NSDraggingDestination for NhView {
        #[unsafe(method(draggingEntered:))]
        fn dragging_entered(&self, info: &ProtocolObject<dyn NSDraggingInfo>) -> NSDragOperation {
            NSDragOperation(dnd::entered(*self.ivars(), self, info))
        }

        #[unsafe(method(draggingUpdated:))]
        fn dragging_updated(&self, info: &ProtocolObject<dyn NSDraggingInfo>) -> NSDragOperation {
            NSDragOperation(dnd::updated(*self.ivars(), self, info))
        }

        #[unsafe(method(draggingExited:))]
        fn dragging_exited(&self, info: Option<&ProtocolObject<dyn NSDraggingInfo>>) {
            dnd::exited(*self.ivars(), self, info);
        }

        #[unsafe(method(performDragOperation:))]
        fn perform_drag(&self, info: &ProtocolObject<dyn NSDraggingInfo>) -> bool {
            dnd::dropped(*self.ivars(), self, info)
        }
    }

    impl NhView {
        #[unsafe(method(acceptsFirstResponder))]
        fn accepts_first_responder(&self) -> bool {
            true
        }

        #[unsafe(method(acceptsFirstMouse:))]
        fn accepts_first_mouse(&self, _e: Option<&NSEvent>) -> bool {
            true
        }

        /// A key that makes text, a dead key, or any key while text is marked goes through the input system
        /// (`crate::macos::ime`); a chord or a named key is sent as it is.
        #[unsafe(method(keyDown:))]
        fn key_down(&self, e: &NSEvent) {
            let ignoring = e.charactersIgnoringModifiers().map(|s| s.to_string()).unwrap_or_default();
            let chars = e.characters().map(|s| s.to_string()).unwrap_or_default();
            let flags = e.modifierFlags();
            let mut k = mac_input::key(&ignoring, &chars, flags.0, e.isARepeat());
            if let Some(k) = &mut k {
                // the virtual key code, + 1 so 0 stays "none": the glue pairs the release with it
                k.keycode = u32::from(e.keyCode()) + 1;
            }
            let chord = NSEventModifierFlags::Control | NSEventModifierFlags::Command;
            let dead = chars.is_empty() && !ignoring.is_empty() && !flags.intersects(chord);
            let id = *self.ivars();
            if ime::wants(id, k.as_ref(), dead) {
                let events = NSArray::from_slice(&[e]);
                ime::interpret(k, chars, || self.interpretKeyEvents(&events));
            } else if let Some(k) = k {
                self.send(ShellEvent::Key(k));
            }
        }

        /// A release names only its key code: the glue gives it its press's fields, or drops it when the press gave
        /// no `KEY` action (the input system took it).
        #[unsafe(method(keyUp:))]
        fn key_up(&self, e: &NSEvent) {
            self.send(ShellEvent::Key(KeyEv {
                keycode: u32::from(e.keyCode()) + 1,
                keysym: 0,
                mods: mac_input::mods(e.modifierFlags().0),
                pressed: 0,
                repeat: 0,
                utf8: [0; 16],
            }));
        }

        #[unsafe(method(flagsChanged:))]
        fn flags_changed(&self, e: &NSEvent) {
            self.send(ShellEvent::Modifiers {
                mods: mac_input::mods(e.modifierFlags().0),
            });
        }

        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, e: &NSEvent) {
            dnd::note_press(e);
            self.button(e, NHSHELL_BUTTON_PRESSED);
        }

        #[unsafe(method(mouseUp:))]
        fn mouse_up(&self, e: &NSEvent) {
            self.button(e, NHSHELL_BUTTON_RELEASED);
        }

        #[unsafe(method(rightMouseDown:))]
        fn right_mouse_down(&self, e: &NSEvent) {
            self.button(e, NHSHELL_BUTTON_PRESSED);
        }

        #[unsafe(method(rightMouseUp:))]
        fn right_mouse_up(&self, e: &NSEvent) {
            self.button(e, NHSHELL_BUTTON_RELEASED);
        }

        #[unsafe(method(otherMouseDown:))]
        fn other_mouse_down(&self, e: &NSEvent) {
            self.button(e, NHSHELL_BUTTON_PRESSED);
        }

        #[unsafe(method(otherMouseUp:))]
        fn other_mouse_up(&self, e: &NSEvent) {
            self.button(e, NHSHELL_BUTTON_RELEASED);
        }

        #[unsafe(method(mouseMoved:))]
        fn mouse_moved(&self, e: &NSEvent) {
            self.motion(e);
        }

        #[unsafe(method(mouseDragged:))]
        fn mouse_dragged(&self, e: &NSEvent) {
            self.motion(e);
        }

        /// The pointer left the view (its tracking area): the backend gives the window it was over a leave unless it
        /// went onto another view of the same root (an overhang panel), whose motion moves it there.  The record's
        /// `x`, `y` are where it went, in screen points.
        #[unsafe(method(mouseExited:))]
        fn mouse_exited(&self, e: &NSEvent) {
            let at = self
                .window()
                .map_or_else(NSEvent::mouseLocation, |w| w.convertPointToScreen(e.locationInWindow()));
            self.send(ShellEvent::PointerLeave(PointerEv {
                x: at.x,
                y: at.y,
                ..PointerEv::default()
            }));
        }

        #[unsafe(method(rightMouseDragged:))]
        fn right_mouse_dragged(&self, e: &NSEvent) {
            self.motion(e);
        }

        #[unsafe(method(otherMouseDragged:))]
        fn other_mouse_dragged(&self, e: &NSEvent) {
            self.motion(e);
        }

        #[unsafe(method(magnifyWithEvent:))]
        fn magnify(&self, e: &NSEvent) {
            let phase = e.phase();
            let (x, y) = self.position(e);
            let (kind, scale) = MAGNIFY.with(|m| {
                let mut m = m.borrow_mut();
                if phase.contains(NSEventPhase::Began) {
                    m.begin();
                    (NHSHELL_PINCH_BEGIN, 1.0)
                } else if phase.contains(NSEventPhase::Ended)
                    || phase.contains(NSEventPhase::Cancelled)
                {
                    (NHSHELL_PINCH_END, 1.0)
                } else {
                    (NHSHELL_PINCH_UPDATE, m.update(e.magnification()))
                }
            });
            self.send(ShellEvent::Pinch(PinchEv {
                phase: kind,
                fingers: 2,
                scale,
                x,
                y,
                ..PinchEv::default()
            }));
        }

        #[unsafe(method(scrollWheel:))]
        fn scroll_wheel(&self, e: &NSEvent) {
            self.motion(e);
            let (dx, dy) = (e.scrollingDeltaX(), e.scrollingDeltaY());
            if dx != 0.0 || dy != 0.0 {
                self.send(ShellEvent::Scroll(mac_input::wheel(dx, dy, e.hasPreciseScrollingDeltas())));
            }
        }
    }

    unsafe impl NSTextInputClient for NhView {
        #[unsafe(method(insertText:replacementRange:))]
        fn insert_text(&self, s: &AnyObject, _replacement: NSRange) {
            ime::insert(*self.ivars(), &text_of(s));
        }

        #[unsafe(method(doCommandBySelector:))]
        fn do_command(&self, selector: Sel) {
            ime::command(*self.ivars(), selector == sel!(deleteBackward:));
        }

        #[unsafe(method(setMarkedText:selectedRange:replacementRange:))]
        fn set_marked_text(&self, s: &AnyObject, _selected: NSRange, _replacement: NSRange) {
            ime::mark(*self.ivars(), &text_of(s));
        }

        #[unsafe(method(unmarkText))]
        fn unmark_text(&self) {
            ime::unmark(*self.ivars());
        }

        #[unsafe(method(selectedRange))]
        fn selected_range(&self) -> NSRange {
            let (l, n) = ime::with(*self.ivars(), |p| p.text.selected());
            NSRange::new(l, n)
        }

        #[unsafe(method(markedRange))]
        fn marked_range(&self) -> NSRange {
            match ime::with(*self.ivars(), |p| p.text.marked_range()) {
                Some((l, n)) => NSRange::new(l, n),
                None => NSRange::new(NSNotFound as usize, 0),
            }
        }

        #[unsafe(method(hasMarkedText))]
        fn has_marked_text(&self) -> bool {
            ime::with(*self.ivars(), |p| p.text.marked > 0)
        }

        #[unsafe(method_id(attributedSubstringForProposedRange:actualRange:))]
        fn attributed_substring(
            &self,
            range: NSRange,
            actual: *mut NSRange,
        ) -> Option<Retained<NSAttributedString>> {
            substring(*self.ivars(), range, actual)
        }

        #[unsafe(method_id(validAttributesForMarkedText))]
        fn valid_attributes(&self) -> Retained<NSArray<NSString>> {
            NSArray::new()
        }

        /// Below the cursor, as wide as the range in cursor widths (NS's columns), on the screen.
        #[unsafe(method(firstRectForCharacterRange:actualRange:))]
        fn first_rect(&self, range: NSRange, actual: *mut NSRange) -> NSRect {
            if !actual.is_null() {
                // SAFETY: AppKit passes null or a range to write.
                unsafe { actual.write(range) };
            }
            ax::cursor_on_screen(self, *self.ivars(), range.length as f64)
        }

        /// NS answers 0 too.
        #[unsafe(method(characterIndexForPoint:))]
        fn character_index(&self, _p: NSPoint) -> usize {
            0
        }
    }

    /// The accessibility element (NSView's informal accessibility methods, overridden): a text area whose value and
    /// selection are the pushed line and region, framed by the cursor, as NS's view answers.
    impl NhView {
        #[unsafe(method(isAccessibilityElement))]
        fn ax_element(&self) -> bool {
            true
        }

        #[unsafe(method_id(accessibilityRole))]
        fn ax_role(&self) -> Option<Retained<NSString>> {
            // SAFETY: an immutable string constant of AppKit's.
            Some(unsafe { NSAccessibilityTextAreaRole }.retain())
        }

        /// The line around point, as pushed (within the push's cap).
        #[unsafe(method_id(accessibilityValue))]
        fn ax_value(&self) -> Option<Retained<NSString>> {
            Some(NSString::from_str(&ime::with(*self.ivars(), |p| p.text.text.clone())))
        }

        /// The region's text within the pushed line.
        #[unsafe(method_id(accessibilitySelectedText))]
        fn ax_selected_text(&self) -> Option<Retained<NSString>> {
            Some(NSString::from_str(&ime::with(*self.ivars(), |p| p.text.selected_text())))
        }

        /// NS's: the cursor's rectangle on the screen.
        #[unsafe(method(accessibilityFrame))]
        fn ax_frame(&self) -> NSRect {
            ax::cursor_on_screen(self, *self.ivars(), 1.0)
        }
    }
);

/// `attributedSubstringForProposedRange:actualRange:` of window `id`'s pushed line: `nil` outside it.
fn substring(
    id: WindowId,
    range: NSRange,
    actual: *mut NSRange,
) -> Option<Retained<NSAttributedString>> {
    let (text, (l, n)) = ime::with(id, |p| p.text.substring(range.location, range.length))?;
    if !actual.is_null() {
        // SAFETY: AppKit passes null or a range to write.
        unsafe { actual.write(NSRange::new(l, n)) };
    }
    Some(NSAttributedString::from_nsstring(&NSString::from_str(
        &text,
    )))
}

/// The text of `s`, an NSString or an NSAttributedString, as AppKit passes it.
fn text_of(s: &AnyObject) -> String {
    // SAFETY: AppKit passes one of the two; both answer the messages sent.
    unsafe {
        let attributed: bool = msg_send![s, isKindOfClass: NSAttributedString::class()];
        let string: Retained<NSString> = if attributed {
            msg_send![s, string]
        } else {
            msg_send![s, copy]
        };
        string.to_string()
    }
}

impl NhView {
    fn send(&self, ev: ShellEvent) {
        deliver(MacEv::Input(*self.ivars(), ev));
    }

    /// The event's position in this view, in device pixels from the top left.
    fn position(&self, e: &NSEvent) -> (f64, f64) {
        let p = self.convertPoint_fromView(e.locationInWindow(), None);
        let scale = self.window().map_or(1.0, |w| w.backingScaleFactor());
        mac_input::device_point(p.x, p.y, self.bounds().size.height, scale)
    }

    fn motion(&self, e: &NSEvent) {
        let (x, y) = self.position(e);
        self.send(ShellEvent::PointerMotion(PointerEv {
            x,
            y,
            ..PointerEv::default()
        }));
    }

    /// A press or release: the modifiers and the position first, as the Wayland backend's records come.
    fn button(&self, e: &NSEvent, state: u32) {
        self.send(ShellEvent::Modifiers {
            mods: mac_input::mods(e.modifierFlags().0),
        });
        self.motion(e);
        let (x, y) = self.position(e);
        self.send(ShellEvent::Button(PointerEv {
            x,
            y,
            button: mac_input::button(e.buttonNumber()),
            state,
        }));
    }
}

/// Window `id`'s root view, of `size` points, and its layer.  The layer is set before `wantsLayer`: that order makes
/// the view layer-hosting rather than layer-backed.
pub fn layer_hosting(
    mtm: MainThreadMarker,
    id: WindowId,
    size: NSSize,
) -> (Retained<NhView>, Retained<CALayer>) {
    let this = NhView::alloc(mtm).set_ivars(id);
    // SAFETY: NSView's designated initialiser, on a fresh allocation with its ivars set.
    let view: Retained<NhView> =
        unsafe { msg_send![super(this), initWithFrame: NSRect::new(NSPoint::new(0.0, 0.0), size)] };
    let layer = CALayer::new();
    view.setLayer(Some(&layer));
    view.setWantsLayer(true);
    dnd::register(&view);
    // `mouseExited:` whenever the pointer leaves the view, key or not, the area following the view's size
    let opts = NSTrackingAreaOptions::MouseEnteredAndExited
        | NSTrackingAreaOptions::ActiveAlways
        | NSTrackingAreaOptions::InVisibleRect;
    // SAFETY: NSTrackingArea's initialiser on a fresh allocation; the view owns the area, and the area does not retain
    // its owner.
    let area = unsafe {
        NSTrackingArea::initWithRect_options_owner_userInfo(
            NSTrackingArea::alloc(),
            NSRect::ZERO,
            opts,
            Some(&view),
            None,
        )
    };
    view.addTrackingArea(&area);
    (view, layer)
}

//! The input method on AppKit: the root view's `NSTextInputClient`, answered from pushed state.
//!
//! A key that makes text (a plain or shifted printable key, an Option key that is no modifier, a dead key) goes
//! through `interpretKeyEvents:` while it is the current key; any key does while text is marked.  The input system
//! then calls back: `insertText:` with the key's own characters and nothing marked is that key, sent as a key; other
//! text is a commit; `setMarkedText:` is a preedit; `doCommandBySelector:` sends the current key, or, with none (a
//! backspace the input method passes on), a delete, as NS does.  Its questions are answered from what C pushed at the
//! last consistency point (`crate::ime_text`): the cursor rectangle, the line around point and NS's offsets of point
//! and the region.  Nothing here calls Emacs, and no key waits on anything.

use super::runloop::{MacEv, deliver};
use crate::api::{ShellError, ShellResult, WindowId};
use crate::event::{KeyEv, ShellEvent, TextEv};
use crate::ime_text::Surrounding;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;

/// What C pushed for one window: the cursor rectangle (points from the content's top left) and the text around it.
#[derive(Clone, Debug, Default)]
pub struct Pushed {
    pub cursor: [i32; 4],
    pub text: Surrounding,
}

thread_local! {
    static PUSHED: RefCell<HashMap<WindowId, Pushed>> = RefCell::new(HashMap::new());
    /// The key `interpretKeyEvents:` is running for, and its characters.
    static CURRENT: RefCell<Option<(Option<KeyEv>, String)>> = const { RefCell::new(None) };
    /// The texts of the preedit and commit records, until the host takes them.
    static TEXTS: RefCell<HashMap<u32, Vec<u8>>> = RefCell::new(HashMap::new());
    static NEXT: Cell<u32> = const { Cell::new(1) };
}

/// `window`'s pushed state, changed by `f`.
pub fn with<R>(window: WindowId, f: impl FnOnce(&mut Pushed) -> R) -> R {
    PUSHED.with(|p| f(p.borrow_mut().entry(window).or_default()))
}

/// The window went.
pub fn forget(window: WindowId) {
    PUSHED.with(|p| p.borrow_mut().remove(&window));
}

/// `ShellBackend::text_len`.
pub fn text_len(id: u32) -> ShellResult<usize> {
    TEXTS.with(|t| t.borrow().get(&id).map(Vec::len).ok_or(ShellError::State))
}

/// `ShellBackend::take_text`.
pub fn take_text(id: u32) -> ShellResult<Vec<u8>> {
    TEXTS.with(|t| t.borrow_mut().remove(&id).ok_or(ShellError::State))
}

fn text_event(text: &str) -> TextEv {
    if text.is_empty() {
        return TextEv::default();
    }
    let id = NEXT.with(|n| {
        let id = n.get();
        n.set(id.wrapping_add(1).max(1));
        id
    });
    TEXTS.with(|t| t.borrow_mut().insert(id, text.as_bytes().to_vec()));
    TextEv {
        id,
        len: u32::try_from(text.len()).unwrap_or(u32::MAX),
        ..TextEv::default()
    }
}

fn send(window: WindowId, ev: ShellEvent) {
    deliver(MacEv::Input(window, ev));
}

/// Whether key `k` goes through the input system: it makes text, it is `dead` (no characters, no chord), or text is
/// marked.
pub fn wants(window: WindowId, k: Option<&KeyEv>, dead: bool) -> bool {
    dead || with(window, |p| p.text.marked > 0) || k.is_some_and(|k| k.utf8[0] != 0)
}

/// Run `interpret` (the view's `interpretKeyEvents:`) with `k` and `chars` as the current key.
pub fn interpret(k: Option<KeyEv>, chars: String, interpret: impl FnOnce()) {
    CURRENT.with(|c| *c.borrow_mut() = Some((k, chars)));
    interpret();
    CURRENT.with(|c| c.borrow_mut().take());
}

/// `setMarkedText:selectedRange:replacementRange:` (empty text ends the composition).
pub fn mark(window: WindowId, text: &str) {
    with(window, |p| p.text.marked = text.chars().count());
    send(window, ShellEvent::Preedit(text_event(text)));
}

/// `unmarkText`.
pub fn unmark(window: WindowId) {
    if with(window, |p| std::mem::take(&mut p.text.marked)) > 0 {
        send(window, ShellEvent::Preedit(TextEv::default()));
    }
}

/// `insertText:replacementRange:`.
pub fn insert(window: WindowId, text: &str) {
    let had = with(window, |p| std::mem::take(&mut p.text.marked)) > 0;
    if had {
        send(window, ShellEvent::Preedit(TextEv::default()));
    }
    let own = CURRENT.with(|c| match c.borrow().as_ref() {
        Some((Some(k), chars)) if !had && chars == text => Some(*k),
        _ => None,
    });
    match own {
        Some(k) => send(window, ShellEvent::Key(k)),
        None if !text.is_empty() => send(window, ShellEvent::CommitText(text_event(text))),
        None => {}
    }
}

/// `doCommandBySelector:`: the current key, or a delete for `deleteBackward:` with none.
pub fn command(window: WindowId, delete_backward: bool) {
    let k = CURRENT.with(|c| c.borrow().as_ref().and_then(|(k, _)| *k));
    match k {
        Some(k) => send(window, ShellEvent::Key(k)),
        None if delete_backward => {
            if let Some(k) = crate::mac_input::key("\u{7f}", "\u{7f}", 0, false) {
                send(window, ShellEvent::Key(k));
            }
        }
        None => {}
    }
}

/// Test hook: send `view` what an input method sends: `op` 0 marked `text`, 1 inserted `text`, 2 the composition's
/// end, 3 the command named by selector `text`.
#[cfg(feature = "test-hooks")]
pub fn test_send(view: &super::view::NhView, op: i64, text: &str) -> ShellResult {
    use objc2::msg_send;
    use objc2_foundation::{NSNotFound, NSRange, NSString};
    let s = NSString::from_str(text);
    let none = NSRange::new(NSNotFound as usize, 0);
    // SAFETY: the view is the NhView, an NSTextInputClient; the arguments are the types its methods take.
    unsafe {
        match op {
            0 => {
                let _: () = msg_send![view, setMarkedText: &*s, selectedRange: NSRange::new(s.length(), 0), replacementRange: none];
            }
            1 => {
                let _: () = msg_send![view, insertText: &*s, replacementRange: none];
            }
            2 => {
                let _: () = msg_send![view, unmarkText];
            }
            3 => {
                let sel = objc2::runtime::Sel::register(
                    &std::ffi::CString::new(text).map_err(|_| ShellError::Arg)?,
                );
                let _: () = msg_send![view, doCommandBySelector: sel];
            }
            _ => return Err(ShellError::Arg),
        }
    }
    Ok(())
}

/// Test hook: what `view` answers an input method, as a plist: `:selected`, `:marked` (nil without), `:rect` (the
/// first rectangle of one character at the selection, screen points), `:window` (the window's frame), and the
/// substring of (`loc`, `len`) with its `:actual` range.
#[cfg(feature = "test-hooks")]
pub fn test_state(
    view: &super::view::NhView,
    window: &objc2_app_kit::NSWindow,
    loc: usize,
    len: usize,
) -> String {
    use objc2::msg_send;
    use objc2::rc::Retained;
    use objc2_foundation::{NSAttributedString, NSNotFound, NSRange, NSRect};
    let rect = |r: NSRect| {
        format!(
            "({} {} {} {})",
            r.origin.x.round(),
            r.origin.y.round(),
            r.size.width.round(),
            r.size.height.round()
        )
    };
    // SAFETY: the view is the NhView, an NSTextInputClient; the arguments are the types its methods take.
    unsafe {
        let sel: NSRange = msg_send![view, selectedRange];
        let marked: NSRange = msg_send![view, markedRange];
        let mut actual = NSRange::new(0, 0);
        let first: NSRect = msg_send![view, firstRectForCharacterRange: NSRange::new(sel.location, 1), actualRange: &mut actual as *mut NSRange];
        let mut sub_range = NSRange::new(0, 0);
        let sub: Option<Retained<NSAttributedString>> = msg_send![view, attributedSubstringForProposedRange: NSRange::new(loc, len), actualRange: &mut sub_range as *mut NSRange];
        format!(
            "(:selected ({} {}) :marked {} :rect {} :window {} :substring {} :actual ({} {}))",
            sel.location,
            sel.length,
            if marked.location == NSNotFound as usize {
                "nil".to_string()
            } else {
                format!("({} {})", marked.location, marked.length)
            },
            rect(first),
            rect(window.frame()),
            sub.map_or("nil".into(), |s| crate::pasteboard::lisp_string(
                &s.string().to_string()
            )),
            sub_range.location,
            sub_range.length
        )
    }
}

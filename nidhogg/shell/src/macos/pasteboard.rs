//! The general pasteboard as the clipboard: text and a file's URL, written and read through `nhmac--call` and
//! `nhmac--query` (`ns-own-selection-internal` and the other `ns-*` selection names of `nhmac-win.el`).  There is no
//! primary selection on macOS: a request for any selection but `CLIPBOARD` answers nothing.

use crate::api::{ShellError, ShellResult};
use crate::pasteboard::{lisp_string, uti_for_target, word_and_text};
use objc2_app_kit::NSPasteboard;
use objc2_foundation::NSString;
use std::cell::Cell;

thread_local! {
    /// The pasteboard's change count right after our last write: the clipboard is ours while it still holds.
    static OURS: Cell<isize> = const { Cell::new(-1) };
}

const TEXT: &str = "public.utf8-plain-text";

/// `pasteboard-own` "(CLIPBOARD TEXT)": replace the clipboard with TEXT.
pub fn own(arg: &str) -> ShellResult {
    let (sel, text) = word_and_text(arg).ok_or(ShellError::Arg)?;
    if sel != "CLIPBOARD" {
        return Ok(());
    }
    let pb = NSPasteboard::generalPasteboard();
    pb.clearContents();
    if !pb.setString_forType(&NSString::from_str(text), &NSString::from_str(TEXT)) {
        return Err(ShellError::State);
    }
    OURS.with(|o| o.set(pb.changeCount()));
    Ok(())
}

/// `pasteboard-disown` "(CLIPBOARD)": empty the clipboard when it is still ours.
pub fn disown(arg: &str) -> ShellResult {
    let (sel, _) = word_and_text(arg).ok_or(ShellError::Arg)?;
    if sel == "CLIPBOARD" && owned() {
        NSPasteboard::generalPasteboard().clearContents();
        OURS.with(|o| o.set(-1));
    }
    Ok(())
}

fn owned() -> bool {
    OURS.with(|o| o.get()) == NSPasteboard::generalPasteboard().changeCount()
}

/// `pasteboard-owned` "(SEL)": `t` or `nil`.
pub fn owned_text(arg: &str) -> String {
    let ours = word_and_text(arg).is_some_and(|(s, _)| s == "CLIPBOARD") && owned();
    if ours { "t" } else { "nil" }.to_string()
}

/// `pasteboard-exists` "(SEL)": whether the clipboard holds anything.
pub fn exists(arg: &str) -> String {
    let any = word_and_text(arg).is_some_and(|(s, _)| s == "CLIPBOARD")
        && NSPasteboard::generalPasteboard()
            .types()
            .is_some_and(|t| t.count() > 0);
    if any { "t" } else { "nil" }.to_string()
}

/// `pasteboard-read` "(SEL TARGET)": the clipboard's text for TARGET (a file's URL for the file targets) as a Lisp
/// string, or `nil`.
pub fn read(arg: &str) -> String {
    let Some((sel, target)) = word_and_text(arg) else {
        return "nil".into();
    };
    if sel != "CLIPBOARD" {
        return "nil".into();
    }
    let pb = NSPasteboard::generalPasteboard();
    match pb.stringForType(&NSString::from_str(uti_for_target(target))) {
        Some(s) => lisp_string(&s.to_string()),
        None => "nil".into(),
    }
}

/// Test hook: put the file at `path` on the clipboard as a file URL.
#[cfg(feature = "test-hooks")]
pub fn test_own_file(path: &str) -> ShellResult {
    use objc2_foundation::NSURL;
    let url = NSURL::fileURLWithPath(&NSString::from_str(path));
    let text = url.absoluteString().ok_or(ShellError::State)?;
    let pb = NSPasteboard::generalPasteboard();
    pb.clearContents();
    if pb.setString_forType(&text, &NSString::from_str("public.file-url")) {
        Ok(())
    } else {
        Err(ShellError::State)
    }
}

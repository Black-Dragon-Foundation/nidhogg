//! The user defaults NS's `ns-get-resource` and `ns-set-resource` read and write (`NSUserDefaults`), as `nsterm.m`'s
//! `ns_get_defaults_value` does: a key's object as its description, or none.

use objc2::msg_send;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_foundation::{NSString, NSUserDefaults};

/// The argument list `nhmac--call` passes, printed without quotes ("(NAME VALUE)"): the first word, and the rest.
fn split(arg: &str) -> (&str, &str) {
    let inner = arg
        .trim()
        .trim_start_matches('(')
        .trim_end_matches(')')
        .trim();
    match inner.split_once(char::is_whitespace) {
        Some((name, rest)) => (name, rest.trim()),
        None => (inner, ""),
    }
}

/// `get-resource` (NAME): the default as a Lisp string's text, or `nil`.
pub fn get(arg: &str) -> String {
    let (name, _) = split(arg);
    let d = NSUserDefaults::standardUserDefaults();
    let key = NSString::from_str(name);
    // SAFETY: `objectForKey:` and `description` on Foundation objects; both return retained-or-nil objects.
    let text = unsafe {
        let obj: Option<Retained<AnyObject>> = msg_send![&d, objectForKey: &*key];
        obj.map(|o| {
            let s: Retained<NSString> = msg_send![&o, description];
            s.to_string()
        })
    };
    match text {
        Some(t) => format!("\"{}\"", t.replace('\\', "\\\\").replace('"', "\\\"")),
        None => "nil".into(),
    }
}

/// `set-resource` (NAME VALUE) sets the default; `unset-resource` (NAME) removes it.
pub fn set(arg: &str, unset: bool) {
    let (name, value) = split(arg);
    let d = NSUserDefaults::standardUserDefaults();
    let key = NSString::from_str(name);
    // SAFETY: Foundation's setters on the standard defaults, with live string objects.
    unsafe {
        if unset {
            let _: () = msg_send![&d, removeObjectForKey: &*key];
        } else {
            let v = NSString::from_str(value);
            let _: () = msg_send![&d, setObject: &*v, forKey: &*key];
        }
    }
}

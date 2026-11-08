//! The AppKit half of startup (`ns_init_locale` of `nsterm.m`): `LANG` from `NSLocale` when the environment has none
//! (a Finder launch sets no locale).  It runs before Lisp is initialised and calls nothing in Emacs.
//!
//! No outer pool, as `ns_init_pool` keeps: no Objective-C runs outside an entry point, and each entry point drains
//! its own pool (the glue's guard), so a pool made here would be drained when the call returns.

use crate::startup;
use objc2_foundation::NSLocale;
use std::ffi::{CString, c_char, c_int};

unsafe extern "C" {
    fn setlocale(category: c_int, locale: *const c_char) -> *mut c_char;
}

/// `LC_ALL` on macOS.
const LC_ALL: c_int = 6;

/// Whether the C library takes `lang` for `LC_ALL` (the call is the check `ns_init_locale` makes).
fn usable(lang: &str) -> bool {
    let Ok(c) = CString::new(lang) else {
        return false;
    };
    // SAFETY: a NUL-terminated string; the locale it may set is replaced by Emacs's own `setlocale` call.
    unsafe { !setlocale(LC_ALL, c.as_ptr()).is_null() }
}

/// Set `LANG`.  Once, from the main thread, as the first thing `main` does.
pub fn init() {
    // "C" asks for no locale work: Emacs's main skips it too
    if std::env::var("LC_ALL").is_ok_and(|v| v == "C") {
        return;
    }
    let current = std::env::var("LANG").ok();
    let system = NSLocale::currentLocale().localeIdentifier().to_string();
    let (lang, fell_back) = startup::lang(current.as_deref(), &system, usable);
    if fell_back {
        eprintln!(
            "LANG={} cannot be used, using {lang} instead.",
            current.unwrap_or_default()
        );
    }
    // SAFETY: before Emacs starts any thread of its own, which is when `setenv` is safe.
    unsafe { std::env::set_var("LANG", lang) };
}

/// `epath` placed in the app bundle this executable is in, if it is a relative path that exists there.
pub fn relocate(epath: &str) -> Option<String> {
    let root = startup::bundle_root(&std::env::current_exe().ok()?)?;
    let moved = startup::relocate(&root, epath, |p| p.exists());
    (moved != epath).then_some(moved)
}

#[cfg(test)]
#[path = "startup_tests.rs"]
mod tests;

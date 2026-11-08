//! The AppKit half of startup (`ns_init_pool`, `ns_init_locale`, `ns_term_shutdown` of `nsterm.m`): the outer
//! autorelease pool Emacs's main thread keeps for its life, and `LANG` from `NSLocale` when the environment has none
//! (a Finder launch sets no locale).  It runs before Lisp is initialised and calls nothing in Emacs.

use crate::startup;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{class, msg_send};
use objc2_foundation::NSLocale;
use std::ffi::{CString, c_char, c_int};
use std::sync::atomic::{AtomicPtr, Ordering};

unsafe extern "C" {
    fn setlocale(category: c_int, locale: *const c_char) -> *mut c_char;
}

/// `LC_ALL` on macOS.
const LC_ALL: c_int = 6;

/// The outer pool, kept until [`shutdown`].
static OUTER: AtomicPtr<AnyObject> = AtomicPtr::new(std::ptr::null_mut());

/// Whether the C library takes `lang` for `LC_ALL` (the call is the check `ns_init_locale` makes).
fn usable(lang: &str) -> bool {
    let Ok(c) = CString::new(lang) else {
        return false;
    };
    // SAFETY: a NUL-terminated string; the locale it may set is replaced by Emacs's own `setlocale` call.
    unsafe { !setlocale(LC_ALL, c.as_ptr()).is_null() }
}

/// Make the outer pool and set `LANG`.  Once, from the main thread, as the first thing `main` does.
pub fn init() {
    if OUTER.load(Ordering::Acquire).is_null() {
        // SAFETY: `+new` on NSAutoreleasePool; the pool is kept until `shutdown` on this thread.
        let pool: Retained<AnyObject> = unsafe { msg_send![class!(NSAutoreleasePool), new] };
        OUTER.store(Retained::into_raw(pool), Ordering::Release);
    }
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

/// Release the outer pool (`ns_term_shutdown`).
pub fn shutdown() {
    let p = OUTER.swap(std::ptr::null_mut(), Ordering::AcqRel);
    if !p.is_null() {
        // SAFETY: the pointer `init` kept; releasing a pool drains it.
        let _: () = unsafe { msg_send![p, release] };
    }
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

//! The guard every entry point runs in, and `nhg_export!`, the one way to write an entry point.
//!
//! In order, the guard:
//! 1. refuses to start with less than `HEADROOM` of stack left (`NHG_E_NOMEM`, nothing poisoned), so that no glue
//!    call runs near the end of the stack, where a stack probe would land in Emacs's SIGSEGV handler and its
//!    `siglongjmp`;
//! 2. takes the display's lock with `try_lock`: a lock that is held means an earlier call was abandoned (a `longjmp`
//!    crossed it) or the call re-entered, and both poison the display instead of blocking Emacs;
//! 3. runs the body in `catch_unwind`: a panic poisons the display, its first report (message, location and backtrace)
//!    is kept for `nhg_panic_message`, its message and location are written to stderr once, and the call returns
//!    `NHG_E_PANIC`.  The backtrace is resolved when the report is read; C writes the whole report to stderr when it
//!    drops the display.
//!
//! A poisoned display answers every later call with `NHG_E_PANIC` without touching its state.  Nothing here aborts.

use crate::abi::{NHG_E_NOMEM, NHG_E_PANIC};
use crate::display::{Display, NhgDisplay};
use std::backtrace::Backtrace;
use std::cell::{Cell, RefCell};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Once, TryLockError};

/// No glue call starts with less stack than this left.
pub const HEADROOM: usize = 256 * 1024;

/// An error an entry point returns; each is a negative `NHG_E_*` code.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GlueError {
    Arg,
    State,
    NoMem,
    Thread,
    /// A shell error, by its `NHSHELL_E_*` code (equal to the `NHG_E_*` one).
    Shell(i32),
    /// An `NHG_E_*` code from a module that returns codes (`backing.rs`).
    Code(i32),
}

impl GlueError {
    pub fn code(self) -> i32 {
        use crate::abi::*;
        match self {
            GlueError::Arg => NHG_E_ARG,
            GlueError::State => NHG_E_STATE,
            GlueError::NoMem => NHG_E_NOMEM,
            GlueError::Thread => NHG_E_THREAD,
            GlueError::Shell(c) | GlueError::Code(c) => c,
        }
    }
}

impl From<i32> for GlueError {
    fn from(code: i32) -> GlueError {
        GlueError::Code(code)
    }
}

impl From<nhshell::api::ShellError> for GlueError {
    fn from(e: nhshell::api::ShellError) -> GlueError {
        match e {
            nhshell::api::ShellError::Reconnecting => {
                GlueError::Code(crate::abi::NHG_E_RECONNECTING)
            }
            e => GlueError::Shell(e.code()),
        }
    }
}

/// What an entry point returns: a code, or a pointer (null for every error).
pub trait Ret {
    fn err(code: i32) -> Self;
}

impl Ret for i32 {
    fn err(code: i32) -> i32 {
        code
    }
}

impl<T> Ret for *mut T {
    fn err(_: i32) -> *mut T {
        std::ptr::null_mut()
    }
}

thread_local! {
    /// The lowest address of this thread's stack: 0 not read yet, `usize::MAX` unknown.
    static STACK_LOW: Cell<usize> = const { Cell::new(0) };
    /// How many guards this thread is inside: the panic hook keeps the report of a panic inside one.
    static IN_GUARD: Cell<u32> = const { Cell::new(0) };
    /// The hook's report of the last panic inside a guard on this thread.
    static REPORT: RefCell<Option<PanicReport>> = const { RefCell::new(None) };
}

/// The lowest address of the calling thread's stack, from `pthread_getattr_np`.
#[cfg(any(target_os = "linux", target_os = "android"))]
fn read_stack_low() -> Option<usize> {
    // SAFETY: `attr` is initialised by `pthread_getattr_np` before it is read and destroyed once; `addr` and `size` are
    // writable locals.
    unsafe {
        let mut attr: libc::pthread_attr_t = std::mem::zeroed();
        if libc::pthread_getattr_np(libc::pthread_self(), &mut attr) != 0 {
            return None;
        }
        let mut addr: *mut libc::c_void = std::ptr::null_mut();
        let mut size: libc::size_t = 0;
        let rc = libc::pthread_attr_getstack(&attr, &mut addr, &mut size);
        libc::pthread_attr_destroy(&mut attr);
        (rc == 0 && !addr.is_null()).then_some(addr as usize)
    }
}

#[cfg(not(any(target_os = "linux", target_os = "android")))]
fn read_stack_low() -> Option<usize> {
    None
}

/// Whether at least `HEADROOM` of stack is left below the caller (the stack grows down).  An unknown bound passes.
fn headroom_ok() -> bool {
    let mut low = STACK_LOW.get();
    if low == 0 {
        low = read_stack_low().unwrap_or(usize::MAX);
        STACK_LOW.set(low);
    }
    if low == usize::MAX {
        return true;
    }
    let here = 0u8;
    let sp = std::hint::black_box(&here) as *const u8 as usize;
    sp.checked_sub(low).is_some_and(|left| left >= HEADROOM)
}

/// Test hook: take `low` as the calling thread's lowest stack address (`None`: read it again).
#[cfg(feature = "test-hooks")]
#[doc(hidden)]
pub fn set_stack_low_for_test(low: Option<usize>) {
    STACK_LOW.set(low.unwrap_or(0));
}

/// What went wrong on a display: the message and location at once, the backtrace captured then but resolved only when
/// the report is read (symbolising can take hundreds of milliseconds on a large binary; the guard returns at once).
pub struct PanicReport {
    pub head: String,
    pub backtrace: Option<Backtrace>,
}

impl PanicReport {
    /// A panic's report headed `head`, with the caller's backtrace.
    pub fn here(head: String) -> PanicReport {
        PanicReport {
            head,
            backtrace: Some(Backtrace::force_capture()),
        }
    }

    /// A report with no backtrace (a poison that is not a panic: the first capture can take tens of milliseconds).
    pub fn plain(head: String) -> PanicReport {
        PanicReport {
            head,
            backtrace: None,
        }
    }

    /// The whole report, backtrace resolved.
    pub fn text(&self) -> String {
        match &self.backtrace {
            Some(bt) => format!("{}\nbacktrace:\n{bt}", self.head),
            None => self.head.clone(),
        }
    }
}

/// Keep the report of a panic inside a guard (message, location, backtrace) instead of printing it; other panics go
/// to the hook that was there before.
fn install_hook() {
    static HOOK: Once = Once::new();
    HOOK.call_once(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            if IN_GUARD.try_with(Cell::get).unwrap_or(0) > 0 {
                let report = PanicReport::here(info.to_string());
                let _ = REPORT.try_with(|r| *r.borrow_mut() = Some(report));
            } else {
                previous(info);
            }
        }));
    });
}

/// Run `body` inside `IN_GUARD`, catching a panic; `Err` holds its report.
pub(crate) fn catching<R>(body: impl FnOnce() -> R) -> Result<R, PanicReport> {
    install_hook();
    IN_GUARD.set(IN_GUARD.get() + 1);
    let caught = catch_unwind(AssertUnwindSafe(body));
    IN_GUARD.set(IN_GUARD.get() - 1);
    caught.map_err(|payload| {
        let report = REPORT.take().unwrap_or_else(|| {
            let msg = payload
                .downcast_ref::<&str>()
                .map(|s| s.to_string())
                .or_else(|| payload.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "a panic with no message".into());
            PanicReport::here(format!("panicked: {msg}"))
        });
        // a payload whose drop panics again would unwind out of here; keep it
        std::mem::forget(payload);
        report
    })
}

/// The guard of an entry point on a display: headroom, `try_lock`, `catch_unwind` (see the module comment).
///
/// `name` is the entry point's, for the report.
///
/// # Safety
/// `d` is null or a live display from `nhg_open`.
pub unsafe fn guard<R: Ret>(
    name: &str,
    d: *mut NhgDisplay,
    body: impl FnOnce(&mut Display) -> Result<R, GlueError>,
) -> R {
    if !headroom_ok() {
        return R::err(NHG_E_NOMEM);
    }
    // SAFETY: null or live, by the caller's promise.
    let Some(d) = (unsafe { d.as_ref() }) else {
        return R::err(GlueError::Arg.code());
    };
    if d.is_poisoned() {
        return R::err(NHG_E_PANIC);
    }
    let mut state = match d.state.try_lock() {
        Ok(s) => s,
        Err(TryLockError::WouldBlock) => {
            d.poison(PanicReport::plain(format!(
                "{name}: the display's state was locked on entry: a call re-entered, or a longjmp abandoned an \
                 earlier one"
            )));
            return R::err(NHG_E_PANIC);
        }
        Err(TryLockError::Poisoned(_)) => {
            d.poison(PanicReport::plain(format!(
                "{name}: the display's lock was poisoned"
            )));
            return R::err(NHG_E_PANIC);
        }
    };
    let ran = catching(|| {
        debug_assert!(
            !glitnir::is_compose_thread() && !nhshell::is_reader_thread(),
            "a glue entry point ran on a compose or reader thread"
        );
        body(&mut state)
    });
    drop(state);
    match ran {
        Ok(Ok(r)) => r,
        Ok(Err(e)) => R::err(e.code()),
        Err(report) => {
            d.poison(report);
            R::err(NHG_E_PANIC)
        }
    }
}

/// The guard of an entry point that takes no display lock (the ABI version, open, close, the panic message):
/// headroom and `catch_unwind` only.  A panic here has no display to poison; its report goes to stderr.
pub fn unlocked<R: Ret>(body: impl FnOnce() -> Result<R, GlueError>) -> R {
    if !headroom_ok() {
        return R::err(NHG_E_NOMEM);
    }
    match catching(body) {
        Ok(Ok(r)) => r,
        Ok(Err(e)) => R::err(e.code()),
        Err(report) => {
            use std::io::Write;
            let _ = writeln!(std::io::stderr(), "nhglue: {}", report.text());
            R::err(NHG_E_PANIC)
        }
    }
}

/// Write an entry point.  Every `extern "C"` function of the glue comes from here (`tests/ffi.rs` checks the source),
/// so each one runs in a guard; each needs a doc comment with a `# Safety` section (checked there too).
///
/// - `fn NAME(d: *mut NhgDisplay, ARGS) -> RET |state| BODY`: `guard`, with `state: &mut Display`;
/// - `unlocked fn NAME(ARGS) -> RET BODY`: `unlocked`.
///
/// `RET` is `i32` (a count or a negative `NHG_E_*`) or a pointer (null for every error); `BODY` returns
/// `Result<RET, GlueError>`.
macro_rules! nhg_export {
    ($(#[$attr:meta])+
     fn $name:ident($d:ident: *mut NhgDisplay $(, $arg:ident: $ty:ty)* $(,)?) -> $ret:ty
     |$state:pat_param| $body:block) => {
        $(#[$attr])+
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn $name($d: *mut NhgDisplay $(, $arg: $ty)*) -> $ret {
            // outside the unsafe block below, so the body's own unsafe blocks keep their SAFETY comments
            let body = |$state: &mut $crate::display::Display| -> Result<$ret, $crate::guard::GlueError> {
                $body
            };
            // SAFETY: `d` is null or live, by the entry point's contract.
            unsafe { $crate::guard::guard(stringify!($name), $d, body) }
        }
    };
    ($(#[$attr:meta])+
     unlocked fn $name:ident($($arg:ident: $ty:ty),* $(,)?) -> $ret:ty $body:block) => {
        $(#[$attr])+
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn $name($($arg: $ty),*) -> $ret {
            $crate::guard::unlocked(|| -> Result<$ret, $crate::guard::GlueError> { $body })
        }
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A panic caught by `catching` (as `nhg_open`'s backend open is) gives its report to the caller, which prints
    /// it, and leaves none behind for the next panic's.
    #[test]
    fn catching_hands_the_report_over() {
        let r = catching(|| -> i32 { panic!("the backend's open") });
        let report = r.expect_err("the panic is caught");
        assert!(
            report.head.contains("the backend's open"),
            "{}",
            report.head
        );
        assert!(REPORT.take().is_none());
        assert_eq!(catching(|| 7).ok(), Some(7));
    }
}

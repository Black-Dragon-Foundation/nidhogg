//! The guard of Sjá's one entry point, the per-view locks and the breaker.
//!
//! Every call runs on the Lisp thread that holds Emacs's global lock, so a held lock means only a bug or a call a
//! `longjmp` abandoned.  In order, the guard:
//! 1. refuses to start with less than `HEADROOM` of stack left (`SJA_E_NOMEM`), so that no call runs where a stack
//!    probe would land in Emacs's SIGSEGV handler;
//! 2. takes the process-wide table (view name → view, handle → view name) with `try_lock`, finds the one view the call
//!    touches and lets the table go;
//! 3. takes that view's lock with `try_lock`: held, the call answers as a resync would (nothing blocks Emacs);
//! 4. runs the body in `catch_unwind`.  A panic drops only that view's model (`clear_poison` after), keeps its
//!    instances, their handles and their selection ids (the next reset re-resolves them), and answers `resync`.  Two
//!    panics on one view within 60 s trip its breaker: the view is `broken` until its last instance closes.
//!
//! Nothing here aborts, and nothing calls Emacs.

use std::cell::Cell;
use std::collections::HashMap;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, Once, TryLockError};

use crate::view::View;

/// No call starts with less stack than this left (the glue's figure).
pub const HEADROOM: usize = 256 * 1024;

/// The bytes all views together may hold.
pub const TOTAL_BYTES: usize = 32 << 20;

/// Two panics within this window trip a view's breaker.
pub const BREAKER_MS: u64 = 60_000;

thread_local! {
    /// The lowest address of this thread's stack: 0 not read yet, `usize::MAX` unknown.
    static STACK_LOW: Cell<usize> = const { Cell::new(0) };
    /// How many guards this thread is inside: the panic hook keeps quiet about panics inside one.
    static IN_GUARD: Cell<u32> = const { Cell::new(0) };
}

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

/// Whether at least `HEADROOM` of stack is left below the caller.  An unknown bound passes.
pub(crate) fn headroom_ok() -> bool {
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

/// Panics inside a guard print one line ("sja: …") instead of the default report; others go to the hook before.
fn install_hook() {
    static HOOK: Once = Once::new();
    HOOK.call_once(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            if IN_GUARD.try_with(Cell::get).unwrap_or(0) > 0 {
                use std::io::Write;
                let _ = writeln!(std::io::stderr(), "sja: {info}");
            } else {
                previous(info);
            }
        }));
    });
}

/// Run `body`, catching a panic: `None` when it panicked.
pub(crate) fn catching<R>(body: impl FnOnce() -> R) -> Option<R> {
    install_hook();
    IN_GUARD.set(IN_GUARD.get() + 1);
    let r = catch_unwind(AssertUnwindSafe(body));
    IN_GUARD.set(IN_GUARD.get() - 1);
    match r {
        Ok(v) => Some(v),
        Err(payload) => {
            // a payload whose drop panics again would unwind out of here; keep it
            std::mem::forget(payload);
            None
        }
    }
}

/// The clock the breaker reads, in milliseconds (a test may set it).
fn now_ms() -> u64 {
    #[cfg(feature = "test-hooks")]
    if let Some(t) = TEST_NOW.with(Cell::get) {
        return t;
    }
    use std::time::Instant;
    static ORIGIN: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    let origin = *ORIGIN.get_or_init(Instant::now);
    u64::try_from(origin.elapsed().as_millis()).unwrap_or(u64::MAX)
}

#[cfg(feature = "test-hooks")]
thread_local! {
    static TEST_NOW: Cell<Option<u64>> = const { Cell::new(None) };
}

/// Test hook: the breaker's clock reads `ms` (`None`: the real clock).
#[cfg(feature = "test-hooks")]
pub fn set_now_for_test(ms: Option<u64>) {
    TEST_NOW.with(|t| t.set(ms));
}

/// One view in the table.
#[derive(Default)]
pub struct Slot {
    pub view: Mutex<View>,
    /// The times of its last panics (at most two kept) and whether its breaker tripped.
    breaker: Mutex<(Vec<u64>, bool)>,
    /// What its model holds, for the other views' budget.
    bytes: AtomicUsize,
    panics: AtomicU32,
}

impl Slot {
    pub fn broken(&self) -> bool {
        self.breaker.lock().map(|b| b.1).unwrap_or(true)
    }

    /// A panic happened in a call on this view: drop its model, clear the poison, count it toward the breaker.
    fn panicked(&self) {
        self.panics.fetch_add(1, Ordering::Relaxed);
        let mut v = self.view.lock().unwrap_or_else(|e| e.into_inner());
        v.model = None;
        drop(v);
        self.view.clear_poison();
        self.bytes.store(0, Ordering::Relaxed);
        let now = now_ms();
        if let Ok(mut b) = self.breaker.lock() {
            b.0.retain(|t| now.saturating_sub(*t) < BREAKER_MS);
            b.0.push(now);
            if b.0.len() >= 2 {
                b.1 = true;
            }
        }
    }
}

/// The process-wide table.
#[derive(Default)]
pub struct Table {
    pub views: HashMap<String, Arc<Slot>>,
    pub handles: HashMap<u32, String>,
    next: u32,
}

static TABLE: Mutex<Option<Table>> = Mutex::new(None);

/// Why a call couldn't run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refused {
    /// Bad arguments: an unknown handle, a wrong type.
    Arg,
    /// Too little stack.
    NoMem,
    /// The table was held or poisoned.
    State,
}

/// Run `f` on the table (headroom, `try_lock`, `catch_unwind`).
pub fn with_table<R>(f: impl FnOnce(&mut Table) -> R) -> Result<R, Refused> {
    if !headroom_ok() {
        return Err(Refused::NoMem);
    }
    let mut g = match TABLE.try_lock() {
        Ok(g) => g,
        Err(TryLockError::WouldBlock) => return Err(Refused::State),
        Err(TryLockError::Poisoned(p)) => {
            // a panic while the table was held: it holds only maps, keep them
            TABLE.clear_poison();
            p.into_inner()
        }
    };
    let t = g.get_or_insert_with(Table::default);
    catching(|| f(t)).ok_or(Refused::State)
}

/// What a guarded call on one view did.
pub enum Ran<R> {
    Done(R),
    /// The view was locked, panicked now or before (no model yet): the caller answers as a resync.
    Resync,
    Broken,
}

impl Table {
    /// The slot of `name`, made if missing.
    pub fn slot(&mut self, name: &str) -> Arc<Slot> {
        self.views.entry(name.to_string()).or_default().clone()
    }

    /// A new handle on view `name`.
    pub fn open(&mut self, name: &str) -> (u32, Arc<Slot>) {
        self.next = self.next.wrapping_add(1).max(1);
        while self.handles.contains_key(&self.next) {
            self.next = self.next.wrapping_add(1).max(1);
        }
        self.handles.insert(self.next, name.to_string());
        (self.next, self.slot(name))
    }

    /// The view of handle `h`.
    pub fn of(&self, h: u32) -> Option<Arc<Slot>> {
        self.views.get(self.handles.get(&h)?).cloned()
    }

    /// The bytes every view but `name` holds.
    pub fn others(&self, name: &str) -> usize {
        self.views
            .iter()
            .filter(|(n, _)| *n != name)
            .map(|(_, s)| s.bytes.load(Ordering::Relaxed))
            .sum()
    }

    /// Counts: views, instances, bytes, panics, broken views.
    pub fn stats(&self) -> [usize; 5] {
        let s = || self.views.values();
        [
            self.views.len(),
            self.handles.len(),
            s().map(|v| v.bytes.load(Ordering::Relaxed)).sum(),
            s().map(|v| v.panics.load(Ordering::Relaxed) as usize).sum(),
            s().filter(|v| v.broken()).count(),
        ]
    }
}

/// Run `body` on `slot`'s view under its own lock and `catch_unwind` (see the module).
pub fn on_view<R>(slot: &Slot, body: impl FnOnce(&mut View) -> R) -> Ran<R> {
    if slot.broken() {
        return Ran::Broken;
    }
    let ran = {
        let mut v = match slot.view.try_lock() {
            Ok(v) => v,
            Err(TryLockError::WouldBlock) => return Ran::Resync,
            Err(TryLockError::Poisoned(_)) => {
                slot.panicked();
                return if slot.broken() {
                    Ran::Broken
                } else {
                    Ran::Resync
                };
            }
        };
        let r = catching(|| body(&mut v));
        if r.is_some() {
            slot.bytes.store(v.bytes(), Ordering::Relaxed);
        }
        r
    };
    match ran {
        Some(r) => Ran::Done(r),
        None => {
            slot.panicked();
            if slot.broken() {
                Ran::Broken
            } else {
                Ran::Resync
            }
        }
    }
}

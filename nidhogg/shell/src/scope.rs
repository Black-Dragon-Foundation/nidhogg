//! The take-out scope: how a platform callback reaches the backend's state while the backend is inside a call.
//!
//! AppKit calls its delegates synchronously from inside the calls the backend makes (`sendEvent:`, `setFrame:`,
//! `runModalSession:`), while the backend's state is mutably borrowed further up the stack.  The pump lends the state
//! to a [`Scope`] for its whole length ([`Scope::lend`]); a callback reaches it only through [`Scope::take`], which
//! empties the scope until the [`Taken`] borrow is dropped, unwind included.  A callback that finds the scope empty (no
//! pump is running, or another take is live) [`queue`](Scope::queue)s its event, and the next lend applies it.  AppKit
//! calls made while a borrow is held go through [`Taken::defer`]: they run after the borrow is put back, so the
//! callbacks they cause can take it in turn.
//!
//! Platform-neutral, so its tests run on Linux; the macOS backend keeps one per thread.

use std::cell::{Cell, RefCell};
use std::ops::{Deref, DerefMut};
use std::ptr::NonNull;

/// The counters `(nhmac-stats)` reports.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ScopeStats {
    /// Takes that found the state lent and got it.
    pub takes: u64,
    /// Takes that succeeded while another borrow was live: always 0 (a live borrow empties the scope).
    pub overlapping_takes: u64,
    /// Events queued because the scope was empty.
    pub queued: u64,
    /// Queued events applied by a later lend.
    pub applied: u64,
}

/// A slot that holds a lent `&mut T` for the length of [`Scope::lend`], and the events `E` that found it empty.
pub struct Scope<T, E> {
    slot: Cell<Option<NonNull<T>>>,
    live: Cell<u32>,
    pending: RefCell<Vec<E>>,
    stats: Cell<ScopeStats>,
}

impl<T, E> Default for Scope<T, E> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T, E> Scope<T, E> {
    pub const fn new() -> Self {
        Scope {
            slot: Cell::new(None),
            live: Cell::new(0),
            pending: RefCell::new(Vec::new()),
            stats: Cell::new(ScopeStats {
                takes: 0,
                overlapping_takes: 0,
                queued: 0,
                applied: 0,
            }),
        }
    }

    fn bump(&self, f: impl FnOnce(&mut ScopeStats)) {
        let mut s = self.stats.get();
        f(&mut s);
        self.stats.set(s);
    }

    /// Lend `state` while `f` runs: callbacks inside `f` take it.  The events queued before or during `f` are then
    /// applied to it with `apply`, oldest first (an event `apply` queues again waits for the next lend).  The state
    /// is back in the caller's hands when this returns or unwinds.
    pub fn lend<R>(
        &self,
        state: &mut T,
        f: impl FnOnce() -> R,
        mut apply: impl FnMut(&mut T, E),
    ) -> R {
        struct Unlend<'s, T, E>(&'s Scope<T, E>);
        impl<T, E> Drop for Unlend<'_, T, E> {
            fn drop(&mut self) {
                // a borrow outliving the lend would point at a state the caller has back: never, as `Taken` borrows
                // the scope, which the lend's caller cannot drop while it runs
                debug_assert_eq!(self.0.live.get(), 0, "a Taken outlived its lend");
                self.0.slot.set(None);
            }
        }
        assert!(
            self.slot.get().is_none() && self.live.get() == 0,
            "a scope lent twice"
        );
        self.slot.set(Some(NonNull::from(&mut *state)));
        let r = {
            let _unlend = Unlend(self);
            f()
        };
        let pending = std::mem::take(&mut *self.pending.borrow_mut());
        for e in pending {
            self.bump(|s| s.applied += 1);
            apply(state, e);
        }
        r
    }

    /// The lent state, emptied from the scope until the borrow drops; `None` outside a lend or while another borrow
    /// is live.
    pub fn take(&self) -> Option<Taken<'_, T, E>> {
        let ptr = self.slot.take()?;
        self.live.set(self.live.get() + 1);
        if self.live.get() > 1 {
            self.bump(|s| s.overlapping_takes += 1);
            debug_assert!(false, "two borrows of one scope");
        }
        self.bump(|s| s.takes += 1);
        Some(Taken {
            scope: self,
            ptr,
            effects: Vec::new(),
        })
    }

    /// Keep `event` for the next lend: for a callback that found the scope empty.
    pub fn queue(&self, event: E) {
        self.bump(|s| s.queued += 1);
        self.pending.borrow_mut().push(event);
    }

    /// Hand `event` to `handle` on the lent state now, or queue it when the scope is empty.
    pub fn deliver(&self, event: E, handle: impl FnOnce(&mut Taken<'_, T, E>, E)) {
        match self.take() {
            Some(mut t) => handle(&mut t, event),
            None => self.queue(event),
        }
    }

    pub fn stats(&self) -> ScopeStats {
        self.stats.get()
    }

    /// Whether the state is lent and free to take.
    pub fn available(&self) -> bool {
        self.slot.get().is_some()
    }
}

/// The state, taken from a [`Scope`]; put back when dropped, then the deferred effects run.
pub struct Taken<'s, T, E> {
    scope: &'s Scope<T, E>,
    ptr: NonNull<T>,
    effects: Vec<Box<dyn FnOnce() + 's>>,
}

impl<'s, T, E> Taken<'s, T, E> {
    /// Run `effect` (an AppKit call that may call back) once this borrow is put back.
    pub fn defer(&mut self, effect: impl FnOnce() + 's) {
        self.effects.push(Box::new(effect));
    }
}

impl<T, E> Deref for Taken<'_, T, E> {
    type Target = T;
    fn deref(&self) -> &T {
        // SAFETY: the pointer came from the `&mut T` the live lend holds; taking it emptied the scope, so this is the
        // only path to the state until the borrow drops.
        unsafe { self.ptr.as_ref() }
    }
}

impl<T, E> DerefMut for Taken<'_, T, E> {
    fn deref_mut(&mut self) -> &mut T {
        // SAFETY: as in `deref`; `&mut self` makes this access unique.
        unsafe { self.ptr.as_mut() }
    }
}

impl<T, E> Drop for Taken<'_, T, E> {
    fn drop(&mut self) {
        self.scope.live.set(self.scope.live.get() - 1);
        self.scope.slot.set(Some(self.ptr));
        let effects = std::mem::take(&mut self.effects);
        // while unwinding, an effect that panicked again would abort: they are dropped instead
        if !std::thread::panicking() {
            for e in effects {
                e();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::panic::{AssertUnwindSafe, catch_unwind};

    #[derive(Default)]
    struct State {
        log: Vec<&'static str>,
    }

    #[test]
    fn take_and_put_back() {
        let scope: Scope<State, &'static str> = Scope::new();
        let mut st = State::default();
        assert!(scope.take().is_none(), "nothing to take outside a lend");
        scope.lend(
            &mut st,
            || {
                {
                    let mut t = scope.take().expect("lent");
                    t.log.push("first");
                }
                let mut t = scope.take().expect("put back after the first borrow");
                t.log.push("second");
            },
            |_, _| unreachable!(),
        );
        assert_eq!(st.log, ["first", "second"]);
        assert!(scope.take().is_none(), "the lend is over");
        assert_eq!(scope.stats().takes, 2);
        assert_eq!(scope.stats().overlapping_takes, 0);
    }

    #[test]
    fn take_while_taken_queues() {
        let scope: Scope<State, &'static str> = Scope::new();
        let mut st = State::default();
        scope.lend(
            &mut st,
            || {
                let mut outer = scope.take().unwrap();
                outer.log.push("outer");
                // a callback inside the borrow: the scope is empty, so its event waits
                scope.deliver("reentrant", |t, e| t.log.push(e));
            },
            |st, e| st.log.push(e),
        );
        assert_eq!(st.log, ["outer", "reentrant"]);
        let s = scope.stats();
        assert_eq!((s.queued, s.applied, s.overlapping_takes), (1, 1, 0));
    }

    #[test]
    fn queued_outside_a_lend_waits_for_the_next() {
        let scope: Scope<State, &'static str> = Scope::new();
        let mut st = State::default();
        scope.deliver("early", |_, _| unreachable!());
        assert!(st.log.is_empty());
        scope.lend(&mut st, || (), |st, e| st.log.push(e));
        assert_eq!(st.log, ["early"]);
    }

    #[test]
    fn a_panic_inside_a_take_restores_the_borrow() {
        let scope: Scope<State, &'static str> = Scope::new();
        let mut st = State::default();
        scope.lend(
            &mut st,
            || {
                let r = catch_unwind(AssertUnwindSafe(|| {
                    let mut t = scope.take().unwrap();
                    t.log.push("before");
                    panic!("inside the take");
                }));
                assert!(r.is_err());
                let mut t = scope.take().expect("the borrow came back on unwind");
                t.log.push("after");
            },
            |_, _| unreachable!(),
        );
        assert_eq!(st.log, ["before", "after"]);
    }

    #[test]
    fn a_panic_in_the_lend_leaves_the_scope_empty() {
        let scope: Scope<State, &'static str> = Scope::new();
        let mut st = State::default();
        let r = catch_unwind(AssertUnwindSafe(|| {
            scope.lend(&mut st, || panic!("in the pump"), |_, _| ())
        }));
        assert!(r.is_err());
        assert!(!scope.available());
        scope.lend(&mut st, || assert!(scope.take().is_some()), |_, _| ());
    }

    #[test]
    fn effects_run_with_the_scope_filled() {
        let scope: Scope<State, &'static str> = Scope::new();
        let mut st = State::default();
        scope.lend(
            &mut st,
            || {
                let mut t = scope.take().unwrap();
                t.log.push("take");
                // the effect's callback finds the borrow put back and takes it itself
                t.defer(|| {
                    assert!(scope.available());
                    scope.deliver("from the effect", |t, e| t.log.push(e));
                });
                assert!(!scope.available());
            },
            |_, _| unreachable!(),
        );
        assert_eq!(st.log, ["take", "from the effect"]);
        assert_eq!(scope.stats().queued, 0);
    }
}

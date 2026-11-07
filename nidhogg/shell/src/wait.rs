//! The main wait, backend-neutral (Naglfar's first): Emacs's fds through one kqueue, the fds kqueue refuses through a
//! helper thread, and the wake flag that ends a parked wait from any thread.
//!
//! [`run`] is `nhg_wait`: it diffs the registrations ([`Regs`]), returns at once when an fd is ready, and otherwise
//! parks in the backend's [`WaitOps::sleep`] (the run loop) until an event, an fd, a wake or the timeout.  The backend
//! supplies the primitives (`macos/kq.rs`); the order that keeps a wake from being lost is [`Park`]'s, written once
//! here and tested with a fake sleeper.

use std::collections::BTreeSet;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicPtr, AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// A registration's filter: readable, writable.
pub const FILT_READ: u8 = 1;
pub const FILT_WRITE: u8 = 2;

/// One registration: an fd and one filter.
pub type Reg = (i32, u8);

/// A change to the kqueue: add (or re-assert) or delete one registration.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Change {
    pub fd: i32,
    pub filt: u8,
    pub add: bool,
}

/// Why a sleep ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Woke {
    /// An `NSEvent` arrived (and was sent).
    Event,
    /// The kqueue's run-loop source fired.
    Kq,
    /// A wake ([`Park::wake`]): another thread, or the helper.
    Kick,
    Timeout,
}

/// What a backend gives the wait.
pub trait WaitOps {
    /// Apply `changes` in one call; `errs` gets one entry per change: 0 or its errno.
    fn kq_apply(&mut self, changes: &[Change], errs: &mut Vec<i32>);
    /// The ready registrations now, without blocking.
    fn kq_poll(&mut self, out: &mut Vec<Reg>);
    /// Which of the helper's registrations are ready now, without blocking.
    fn helper_poll(&mut self, helper: &[Reg], out: &mut Vec<Reg>) {
        select_now(helper, out);
    }
    /// Run the run loop until an event, the kqueue's source, a wake, the helper's report on `helper` (a wake) or
    /// `timeout` (`None`: none).
    fn sleep(&mut self, helper: &[Reg], timeout: Option<Duration>) -> Woke;
    /// The descriptor the shell's own events wake (its queue's), which must never be the helper's.
    fn key_fd(&self) -> i32;
    /// Test hook: whether the next new registration goes to the helper as if kqueue had refused it.
    fn take_forced(&mut self) -> bool {
        false
    }
}

/// What the kqueue holds and what the helper serves.
#[derive(Default, Debug)]
pub struct Regs {
    kq: BTreeSet<Reg>,
    helper: BTreeSet<Reg>,
}

impl Regs {
    /// The changes that make the kqueue hold `want` less the helper's registrations, and how many of them are real
    /// changes (deletes and first adds).  Kept registrations are re-asserted too: a closed fd loses its kevents and a
    /// new fd can reuse the number, so an add that is already there (a modify) is the one way to be sure.  The helper
    /// forgets what is no longer wanted.
    pub fn plan(&mut self, want: &BTreeSet<Reg>, ops: &mut dyn WaitOps) -> (Vec<Change>, usize) {
        self.helper.retain(|r| want.contains(r));
        let mut changes: Vec<Change> = self
            .kq
            .difference(want)
            .map(|&(fd, filt)| Change {
                fd,
                filt,
                add: false,
            })
            .collect();
        let mut real = changes.len();
        for &r in want {
            if self.helper.contains(&r) {
                continue;
            }
            let new = !self.kq.contains(&r);
            if new && ops.take_forced() {
                self.helper.insert(r);
                continue;
            }
            real += usize::from(new);
            changes.push(Change {
                fd: r.0,
                filt: r.1,
                add: true,
            });
        }
        (changes, real)
    }

    /// Record `changes`' results (`errs`, as [`WaitOps::kq_apply`] gives them): an add kqueue refuses with `EINVAL`
    /// goes to the helper; one refused otherwise (a closed fd) is not registered and is tried again next time.
    pub fn applied(&mut self, changes: &[Change], errs: &[i32]) {
        for (c, &e) in changes.iter().zip(errs) {
            let r = (c.fd, c.filt);
            if !c.add {
                self.kq.remove(&r);
            } else if e == 0 {
                self.kq.insert(r);
            } else {
                self.kq.remove(&r);
                if e == libc::EINVAL {
                    self.helper.insert(r);
                }
            }
        }
    }

    /// The helper's registrations.
    pub fn helper(&self) -> Vec<Reg> {
        self.helper.iter().copied().collect()
    }
}

/// The wake flag, the parked flag and the host's lock-waiter count, in the order that loses no wake (Dekker): a waker
/// publishes its reason (the wake, or a count it raised), then reads "parked" and kicks only a parked waiter (W1, W2);
/// the waiter sets "parked" (M1), then re-reads every reason before it sleeps (M2).  One of the two always sees the
/// other (all `SeqCst`), and a kick that lands before the sleep is kept by the sleeper (a signalled run-loop source
/// stays signalled), so the sleep returns at once.
#[derive(Debug, Default)]
pub struct Park {
    pending: AtomicBool,
    parked: AtomicBool,
    /// The host's count of threads that want its lock (Naglfar: `nhmac_lock_waiters`), null until it is handed over.
    waiters: AtomicPtr<AtomicI32>,
}

impl Park {
    pub const fn new() -> Park {
        Park {
            pending: AtomicBool::new(false),
            parked: AtomicBool::new(false),
            waiters: AtomicPtr::new(std::ptr::null_mut()),
        }
    }

    /// Read the lock waiters from `count` from now on.
    pub fn set_waiters(&self, count: &'static AtomicI32) {
        self.waiters
            .store(std::ptr::from_ref(count).cast_mut(), Ordering::SeqCst);
    }

    /// The host's lock waiters now (0 before the count is handed over).
    pub fn waiters(&self) -> i32 {
        let p = self.waiters.load(Ordering::SeqCst);
        // SAFETY: null, or the `'static` count `set_waiters` was given.
        unsafe { p.as_ref() }.map_or(0, |c| c.load(Ordering::SeqCst))
    }

    /// M1: the waiter is parked from here on.
    pub fn begin(&self) {
        self.parked.store(true, Ordering::SeqCst);
    }

    /// M2: whether a reason holds (a wake, a lock waiter, or `ready`), so the waiter must not sleep.
    pub fn check(&self, ready: impl FnOnce() -> bool) -> bool {
        self.pending.load(Ordering::SeqCst) || self.waiters() > 0 || ready()
    }

    /// The waiter is no longer parked.
    pub fn end(&self) {
        self.parked.store(false, Ordering::SeqCst);
    }

    /// Any thread: publish a wake, then kick a parked waiter.  Whether it kicked.
    pub fn wake(&self, kick: impl FnOnce()) -> bool {
        self.pending.store(true, Ordering::SeqCst);
        if self.parked.load(Ordering::SeqCst) {
            kick();
            true
        } else {
            false
        }
    }

    /// The waiter: park, re-read the reasons (`ready`, and a wake), and `sleep` only when none holds.  Whether it
    /// slept.  A wake stays published for [`Park::take`], whether it ended the sleep or kept it from starting.
    pub fn park(&self, ready: impl FnOnce() -> bool, sleep: impl FnOnce()) -> bool {
        self.begin();
        let skip = self.check(ready);
        if !skip {
            sleep();
        }
        self.end();
        !skip
    }

    /// Take a pending wake.
    pub fn take(&self) -> bool {
        self.pending.swap(false, Ordering::SeqCst)
    }
}

/// The wait's counters, for `(nhmac-stats)`.
#[derive(Debug, Default)]
pub struct WaitStats {
    pub waits: AtomicU64,
    pub sleeps: AtomicU64,
    pub kq_wakes: AtomicU64,
    /// Sleeps the kqueue's source ended with nothing ready.
    pub empty_wakes: AtomicU64,
    pub kicks: AtomicU64,
    /// Registrations the helper serves now.
    pub helper_fds: AtomicU64,
    /// The helper's reports that ended a sleep.
    pub helper_wakes: AtomicU64,
    /// Helper reports that carried the shell's own descriptor (keys): always 0, keys never take the hop.
    pub helper_hops_keys: AtomicU64,
    /// Registration changes made (deletes and first adds).
    pub kq_changes: AtomicU64,
    /// Sleeps that began with lock waiters and lasted over 100 ms: a lost hand-off, always 0.
    pub parks_with_waiters: AtomicU64,
}

impl WaitStats {
    pub const fn new() -> WaitStats {
        WaitStats {
            waits: AtomicU64::new(0),
            sleeps: AtomicU64::new(0),
            kq_wakes: AtomicU64::new(0),
            empty_wakes: AtomicU64::new(0),
            kicks: AtomicU64::new(0),
            helper_fds: AtomicU64::new(0),
            helper_wakes: AtomicU64::new(0),
            helper_hops_keys: AtomicU64::new(0),
            kq_changes: AtomicU64::new(0),
            parks_with_waiters: AtomicU64::new(0),
        }
    }

    /// The counters as a plist's body, for the stats text, with `park`'s lock waiters now.
    pub fn text(&self, park: &Park) -> String {
        let g = |c: &AtomicU64| c.load(Ordering::Relaxed);
        format!(
            ":lock-waiters {} :parks-with-waiters {} :waits {} :sleeps {} :kq-wakes {} :empty-wakes {} :kicks {} \
             :helper-fds {} :helper-wakes {} :helper-hops-keys {} :kq-changes {}",
            park.waiters(),
            g(&self.parks_with_waiters),
            g(&self.waits),
            g(&self.sleeps),
            g(&self.kq_wakes),
            g(&self.empty_wakes),
            g(&self.kicks),
            g(&self.helper_fds),
            g(&self.helper_wakes),
            g(&self.helper_hops_keys),
            g(&self.kq_changes)
        )
    }
}

/// The process's wake flag and counters: `nhg_wake` and the helper reach the wait through them.
pub static PARK: Park = Park::new();
pub static STATS: WaitStats = WaitStats::new();

/// The backend's kick (it ends a parked sleep), installed when it opens; any thread.
static KICK: std::sync::OnceLock<fn()> = std::sync::OnceLock::new();

/// Install the backend's kick (once per process).
pub fn set_kick(kick: fn()) {
    let _ = KICK.set(kick);
}

/// Any thread: end the main wait if it is parked, or the next one before it sleeps.
pub fn wake() {
    if PARK.wake(|| {
        if let Some(k) = KICK.get() {
            k();
        }
    }) {
        STATS.kicks.fetch_add(1, Ordering::Relaxed);
    }
}

/// Which of `regs` are ready now: one `select` with a zero timeout (it, unlike `poll`, serves macOS's devices).
pub fn select_now(regs: &[Reg], out: &mut Vec<Reg>) {
    let fits = |fd: i32| (0..libc::FD_SETSIZE as i32).contains(&fd);
    // SAFETY: fd_set is plain data; FD_ZERO and FD_SET write only inside it, for fds below FD_SETSIZE.
    let (mut r, mut w) = unsafe {
        let mut r: libc::fd_set = std::mem::zeroed();
        let mut w: libc::fd_set = std::mem::zeroed();
        libc::FD_ZERO(&mut r);
        libc::FD_ZERO(&mut w);
        (r, w)
    };
    let mut max = -1;
    for &(fd, filt) in regs.iter().filter(|r| fits(r.0)) {
        // SAFETY: `fd` is below FD_SETSIZE.
        unsafe { libc::FD_SET(fd, if filt == FILT_WRITE { &mut w } else { &mut r }) };
        max = max.max(fd);
    }
    if max < 0 {
        return;
    }
    let mut zero = libc::timeval {
        tv_sec: 0,
        tv_usec: 0,
    };
    // SAFETY: the sets and the timeout are ours and live for the call.
    let n = unsafe { libc::select(max + 1, &mut r, &mut w, std::ptr::null_mut(), &mut zero) };
    if n <= 0 {
        return;
    }
    for &(fd, filt) in regs.iter().filter(|r| fits(r.0)) {
        // SAFETY: `fd` is below FD_SETSIZE; the sets are the ones select filled.
        if unsafe { libc::FD_ISSET(fd, if filt == FILT_WRITE { &w } else { &r }) } {
            out.push((fd, filt));
        }
    }
}

/// One wait: register `want`, then return the ready registrations in `out`; empty after a wake or the timeout.
pub fn run(
    ops: &mut dyn WaitOps,
    regs: &mut Regs,
    park: &Park,
    stats: &WaitStats,
    want: &BTreeSet<Reg>,
    timeout: Option<Duration>,
    out: &mut Vec<Reg>,
) {
    let add = |c: &AtomicU64, n: u64| {
        c.fetch_add(n, Ordering::Relaxed);
    };
    add(&stats.waits, 1);
    out.clear();
    let (changes, real) = regs.plan(want, ops);
    if !changes.is_empty() {
        let mut errs = Vec::with_capacity(changes.len());
        ops.kq_apply(&changes, &mut errs);
        regs.applied(&changes, &errs);
    }
    add(&stats.kq_changes, real as u64);
    let helper = regs.helper();
    stats
        .helper_fds
        .store(helper.len() as u64, Ordering::Relaxed);
    let deadline = timeout.map(|t| Instant::now() + t);
    let ops = std::cell::RefCell::new(ops);
    let mut last: Option<Woke> = None;
    loop {
        let mut ops_now = ops.borrow_mut();
        ops_now.kq_poll(out);
        if last == Some(Woke::Kq) && out.is_empty() {
            add(&stats.empty_wakes, 1);
        }
        let n = out.len();
        if !helper.is_empty() {
            ops_now.helper_poll(&helper, out);
        }
        if out.len() > n {
            let key = ops_now.key_fd();
            if out[n..].iter().any(|r| r.0 == key) {
                add(&stats.helper_hops_keys, 1);
            }
            if last == Some(Woke::Kick) {
                add(&stats.helper_wakes, 1);
            }
        }
        drop(ops_now);
        // any return answers a pending wake: the caller comes back through its yield
        if park.take() || !out.is_empty() || park.waiters() > 0 || last == Some(Woke::Timeout) {
            return;
        }
        let left = match deadline {
            Some(d) => {
                let now = Instant::now();
                if now >= d {
                    return;
                }
                Some(d - now)
            }
            None => None,
        };
        let woke = std::cell::Cell::new(Woke::Kick);
        let slept = park.park(
            || {
                let mut probe = Vec::new();
                let mut o = ops.borrow_mut();
                o.kq_poll(&mut probe);
                if !helper.is_empty() {
                    o.helper_poll(&helper, &mut probe);
                }
                !probe.is_empty()
            },
            || {
                let (at, waiters) = (Instant::now(), park.waiters());
                woke.set(ops.borrow_mut().sleep(&helper, left));
                if waiters > 0 && at.elapsed() > Duration::from_millis(100) {
                    add(&stats.parks_with_waiters, 1);
                }
            },
        );
        last = slept.then(|| woke.get());
        if slept {
            add(&stats.sleeps, 1);
            if woke.get() == Woke::Kq {
                add(&stats.kq_wakes, 1);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    const R: u8 = FILT_READ;
    const W: u8 = FILT_WRITE;

    /// A fake backend: a scripted kqueue and sleeps.
    #[derive(Default)]
    struct Fake {
        /// errno per fd for adds (default 0).
        refuse: Vec<(i32, i32)>,
        applied: Vec<Change>,
        /// What each poll returns, in turn (then nothing).
        polls: Vec<Vec<Reg>>,
        helper_ready: Vec<Reg>,
        sleeps: Vec<Woke>,
        slept: usize,
        forced: u32,
        /// Run during a sleep, before it ends.
        during_sleep: Option<Box<dyn FnMut()>>,
    }

    impl WaitOps for Fake {
        fn kq_apply(&mut self, changes: &[Change], errs: &mut Vec<i32>) {
            errs.clear();
            for c in changes {
                self.applied.push(*c);
                let e = self
                    .refuse
                    .iter()
                    .find(|r| c.add && r.0 == c.fd)
                    .map_or(0, |r| r.1);
                errs.push(e);
            }
        }
        fn kq_poll(&mut self, out: &mut Vec<Reg>) {
            if !self.polls.is_empty() {
                out.extend(self.polls.remove(0));
            }
        }
        fn helper_poll(&mut self, helper: &[Reg], out: &mut Vec<Reg>) {
            out.extend(self.helper_ready.iter().filter(|r| helper.contains(r)));
        }
        fn sleep(&mut self, _helper: &[Reg], _timeout: Option<Duration>) -> Woke {
            self.slept += 1;
            if let Some(f) = self.during_sleep.as_mut() {
                f();
            }
            if self.sleeps.is_empty() {
                Woke::Timeout
            } else {
                self.sleeps.remove(0)
            }
        }
        fn key_fd(&self) -> i32 {
            9
        }
        fn take_forced(&mut self) -> bool {
            if self.forced > 0 {
                self.forced -= 1;
                true
            } else {
                false
            }
        }
    }

    fn set(r: &[Reg]) -> BTreeSet<Reg> {
        r.iter().copied().collect()
    }

    fn add(fd: i32, filt: u8) -> Change {
        Change {
            fd,
            filt,
            add: true,
        }
    }

    fn del(fd: i32, filt: u8) -> Change {
        Change {
            fd,
            filt,
            add: false,
        }
    }

    #[test]
    fn plan_diffs_and_reasserts_kept_registrations() {
        let (mut regs, mut f) = (Regs::default(), Fake::default());
        let (c, n) = regs.plan(&set(&[(3, R), (4, R), (4, W)]), &mut f);
        assert_eq!(c, vec![add(3, R), add(4, R), add(4, W)]);
        assert_eq!(n, 3);
        regs.applied(&c, &[0, 0, 0]);
        // 3 and 4's write go; 4's read is kept (re-asserted, not counted); 5 is new
        let (c, n) = regs.plan(&set(&[(4, R), (5, R)]), &mut f);
        assert_eq!(c, vec![del(3, R), del(4, W), add(4, R), add(5, R)]);
        assert_eq!(n, 3);
        regs.applied(&c, &[0, 0, 0, 0]);
        // the same set again: only re-assertions, no real change
        let (c, n) = regs.plan(&set(&[(4, R), (5, R)]), &mut f);
        assert_eq!(c, vec![add(4, R), add(5, R)]);
        assert_eq!(n, 0);
    }

    #[test]
    fn einval_moves_to_the_helper_until_unwanted() {
        let (mut regs, mut f) = (Regs::default(), Fake::default());
        let want = set(&[(3, R), (7, R)]);
        let (c, _) = regs.plan(&want, &mut f);
        regs.applied(&c, &[0, libc::EINVAL]);
        assert_eq!(regs.helper(), vec![(7, R)]);
        // the helper keeps it: kqueue is not asked again
        let (c, n) = regs.plan(&want, &mut f);
        assert_eq!(c, vec![add(3, R)]);
        assert_eq!(n, 0);
        regs.applied(&c, &[0]);
        // no longer wanted: the helper forgets it, and a new fd with the number tries kqueue first
        let (c, _) = regs.plan(&set(&[(3, R)]), &mut f);
        regs.applied(&c, &vec![0; c.len()]);
        assert!(regs.helper().is_empty());
        let (c, n) = regs.plan(&want, &mut f);
        assert_eq!(c, vec![add(3, R), add(7, R)]);
        assert_eq!(n, 1);
    }

    #[test]
    fn other_refusals_are_tried_again() {
        let (mut regs, mut f) = (Regs::default(), Fake::default());
        let want = set(&[(6, R)]);
        let (c, _) = regs.plan(&want, &mut f);
        regs.applied(&c, &[libc::EBADF]);
        assert!(regs.helper().is_empty());
        let (c, n) = regs.plan(&want, &mut f);
        assert_eq!(c, vec![add(6, R)]);
        assert_eq!(n, 1, "never registered: still a first add");
    }

    #[test]
    fn a_forced_registration_goes_to_the_helper_without_a_change() {
        let mut regs = Regs::default();
        let mut f = Fake {
            forced: 1,
            ..Fake::default()
        };
        let (c, _) = regs.plan(&set(&[(3, R), (8, R)]), &mut f);
        assert_eq!(c, vec![add(8, R)]);
        assert_eq!(regs.helper(), vec![(3, R)]);
    }

    /// A sleeper that keeps a kick: a kick before the sleep makes it return at once.
    struct Sleeper {
        tokens: Cell<u32>,
        slept_full: Cell<u32>,
    }

    impl Sleeper {
        fn new() -> Sleeper {
            Sleeper {
                tokens: Cell::new(0),
                slept_full: Cell::new(0),
            }
        }
        fn kick(&self) {
            self.tokens.set(self.tokens.get() + 1);
        }
        fn sleep(&self) {
            if self.tokens.get() > 0 {
                self.tokens.set(self.tokens.get() - 1);
            } else {
                self.slept_full.set(self.slept_full.get() + 1);
            }
        }
    }

    #[test]
    fn a_wake_before_the_park_is_seen_without_sleeping() {
        let (p, s) = (Park::new(), Sleeper::new());
        assert!(!p.wake(|| s.kick()), "nobody parked: no kick");
        assert!(!p.park(|| false, || s.sleep()));
        assert_eq!(s.slept_full.get(), 0);
        assert!(p.take(), "the wake stays for the caller");
    }

    #[test]
    fn a_wake_between_the_check_and_the_sleep_is_not_lost() {
        let (p, s) = (Park::new(), Sleeper::new());
        // the waker lands after the waiter re-read the reasons, just before it sleeps
        let slept = p.park(
            || false,
            || {
                assert!(p.wake(|| s.kick()), "parked: the waker kicks");
                s.sleep();
            },
        );
        assert!(slept);
        assert_eq!(
            s.slept_full.get(),
            0,
            "the kick was kept: the sleep returned at once"
        );
        assert!(p.take(), "the reason stays published for the caller");
    }

    #[test]
    fn a_wake_during_the_check_is_not_lost() {
        let (p, s) = (Park::new(), Sleeper::new());
        let slept = p.park(
            || {
                p.wake(|| s.kick());
                false
            },
            || s.sleep(),
        );
        assert_eq!(s.slept_full.get(), 0);
        let _ = slept;
    }

    #[test]
    fn a_wake_after_the_sleep_waits_for_the_next_park() {
        let (p, s) = (Park::new(), Sleeper::new());
        assert!(p.park(|| false, || s.sleep()));
        assert_eq!(s.slept_full.get(), 1);
        assert!(!p.wake(|| s.kick()), "not parked any more");
        assert!(!p.park(|| false, || s.sleep()), "the next park sees it");
        assert_eq!(s.slept_full.get(), 1);
        assert!(p.take());
    }

    #[test]
    fn a_wake_before_the_wait_parks_returns_it_at_once() {
        let (mut regs, park, stats) = (Regs::default(), Park::new(), WaitStats::new());
        park.wake(|| ());
        let mut f = Fake::default();
        let mut out = Vec::new();
        run(
            &mut f,
            &mut regs,
            &park,
            &stats,
            &set(&[(3, R)]),
            None,
            &mut out,
        );
        assert!(out.is_empty());
        assert_eq!(f.slept, 0);
        assert!(!park.take());
    }

    #[test]
    fn ready_fds_return_without_sleeping() {
        let (mut regs, park, stats) = (Regs::default(), Park::new(), WaitStats::new());
        let mut f = Fake {
            polls: vec![vec![(3, R)]],
            ..Fake::default()
        };
        let mut out = Vec::new();
        run(
            &mut f,
            &mut regs,
            &park,
            &stats,
            &set(&[(3, R)]),
            None,
            &mut out,
        );
        assert_eq!(out, vec![(3, R)]);
        assert_eq!(f.slept, 0);
    }

    #[test]
    fn an_event_then_ready_fd_returns_it() {
        let (mut regs, park, stats) = (Regs::default(), Park::new(), WaitStats::new());
        let mut f = Fake {
            polls: vec![vec![], vec![], vec![(9, R)]],
            sleeps: vec![Woke::Event],
            ..Fake::default()
        };
        let mut out = Vec::new();
        run(
            &mut f,
            &mut regs,
            &park,
            &stats,
            &set(&[(9, R)]),
            None,
            &mut out,
        );
        assert_eq!(out, vec![(9, R)]);
        assert_eq!(f.slept, 1);
    }

    #[test]
    fn a_kick_ends_the_wait_with_nothing_ready() {
        let (mut regs, stats) = (Regs::default(), WaitStats::new());
        let park = std::rc::Rc::new(Park::new());
        let p2 = park.clone();
        let mut f = Fake {
            sleeps: vec![Woke::Kick],
            during_sleep: Some(Box::new(move || {
                p2.wake(|| ());
            })),
            ..Fake::default()
        };
        let mut out = vec![(1, R)];
        run(
            &mut f,
            &mut regs,
            &park,
            &stats,
            &set(&[(3, R)]),
            Some(Duration::from_secs(5)),
            &mut out,
        );
        assert!(out.is_empty());
        assert_eq!(f.slept, 1);
        assert!(!park.take(), "the wait consumed the wake");
    }

    #[test]
    fn an_empty_kq_wake_is_counted_and_the_wait_goes_on() {
        let (mut regs, park, stats) = (Regs::default(), Park::new(), WaitStats::new());
        let mut f = Fake {
            sleeps: vec![Woke::Kq, Woke::Timeout],
            ..Fake::default()
        };
        let mut out = Vec::new();
        run(
            &mut f,
            &mut regs,
            &park,
            &stats,
            &set(&[(3, R)]),
            Some(Duration::from_secs(5)),
            &mut out,
        );
        assert!(out.is_empty());
        assert_eq!(f.slept, 2);
        assert_eq!(stats.empty_wakes.load(Ordering::Relaxed), 1);
        assert_eq!(stats.kq_wakes.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn the_helper_serves_refused_fds() {
        let (mut regs, park, stats) = (Regs::default(), Park::new(), WaitStats::new());
        let mut f = Fake {
            refuse: vec![(7, libc::EINVAL)],
            helper_ready: vec![(7, R)],
            ..Fake::default()
        };
        let mut out = Vec::new();
        run(
            &mut f,
            &mut regs,
            &park,
            &stats,
            &set(&[(3, R), (7, R)]),
            None,
            &mut out,
        );
        assert_eq!(out, vec![(7, R)]);
        assert_eq!(stats.helper_fds.load(Ordering::Relaxed), 1);
        assert_eq!(stats.helper_hops_keys.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn a_timeout_returns_nothing() {
        let (mut regs, park, stats) = (Regs::default(), Park::new(), WaitStats::new());
        let mut f = Fake::default();
        let mut out = Vec::new();
        run(
            &mut f,
            &mut regs,
            &park,
            &stats,
            &set(&[(3, R)]),
            Some(Duration::from_millis(1)),
            &mut out,
        );
        assert!(out.is_empty());
    }

    /// The count's steps, M1 M2 (main) and W1 W2 (a thread that wants the lock), in every interleaving: main never
    /// sleeps while the count is above 0 unless a kick it keeps ends the sleep at once (review I1).
    #[test]
    fn no_lost_wake() {
        #[derive(Clone, Copy, PartialEq)]
        enum Step {
            M1,
            M2,
            W1,
            W2,
        }
        use Step::*;
        // the orders that keep M1 before M2 and W1 before W2
        let orders = [
            [M1, M2, W1, W2],
            [M1, W1, M2, W2],
            [M1, W1, W2, M2],
            [W1, M1, M2, W2],
            [W1, M1, W2, M2],
            [W1, W2, M1, M2],
        ];
        for order in orders {
            let count: &'static AtomicI32 = Box::leak(Box::new(AtomicI32::new(0)));
            let (p, s) = (Park::new(), Sleeper::new());
            p.set_waiters(count);
            let mut skip = None;
            for step in order {
                match step {
                    M1 => p.begin(),
                    M2 => skip = Some(p.check(|| false)),
                    W1 => {
                        count.fetch_add(1, Ordering::SeqCst);
                    }
                    W2 => {
                        p.wake(|| s.kick());
                    }
                }
            }
            if skip == Some(false) {
                s.sleep();
            }
            p.end();
            assert_eq!(
                s.slept_full.get(),
                0,
                "a full sleep with the count at {} after {:?}",
                count.load(Ordering::SeqCst),
                order.map(|s| match s {
                    M1 => "M1",
                    M2 => "M2",
                    W1 => "W1",
                    W2 => "W2",
                })
            );
        }
    }

    /// 10^6 rounds on real threads: a waiter raises the count and wakes, main parks with a fake sleeper that keeps
    /// kicks; no sleep that began with the count above 0 lasts over 100 ms.  Each round hands off through a condvar
    /// both ways (main has seen the count; the waiter lowered it), so neither thread spins on the other: a spin on a
    /// one-CPU container waits a whole timeslice per round.  The test fails at its 120 s deadline instead of hanging.
    #[test]
    fn no_lost_wake_under_stress() {
        use std::sync::atomic::AtomicBool;
        use std::sync::{Arc, Condvar, Mutex};
        const ROUNDS: u32 = 1_000_000;
        const DEADLINE: Duration = Duration::from_secs(120);
        let count: &'static AtomicI32 = Box::leak(Box::new(AtomicI32::new(0)));
        let park: &'static Park = Box::leak(Box::new(Park::new()));
        park.set_waiters(count);
        let end = Instant::now() + DEADLINE;
        let stop: &'static AtomicBool = Box::leak(Box::new(AtomicBool::new(false)));
        // the fake run loop: kicks are tokens; a sleep takes one or times out after 100 ms
        let tokens = Arc::new((Mutex::new(0u32), Condvar::new()));
        // the round's hand-off: (the round main has seen, the round whose count the waiter lowered)
        let hand = Arc::new((Mutex::new((0u32, 0u32)), Condvar::new()));
        // wait on `cv` until `done` holds, or the deadline (which stops both threads)
        fn wait_for<T>(
            m: &Mutex<T>,
            cv: &Condvar,
            end: Instant,
            stop: &AtomicBool,
            done: impl Fn(&T) -> bool,
        ) -> bool {
            let mut g = m.lock().unwrap();
            while !done(&g) {
                let now = Instant::now();
                if now >= end || stop.load(Ordering::SeqCst) {
                    stop.store(true, Ordering::SeqCst);
                    cv.notify_all();
                    return false;
                }
                g = cv
                    .wait_timeout(g, (end - now).min(Duration::from_millis(100)))
                    .unwrap()
                    .0;
            }
            true
        }
        let (t2, h2) = (tokens.clone(), hand.clone());
        let waiter = std::thread::spawn(move || {
            for round in 1..=ROUNDS {
                count.fetch_add(1, Ordering::SeqCst);
                park.wake(|| {
                    *t2.0.lock().unwrap() += 1;
                    t2.1.notify_one();
                });
                if !wait_for(&h2.0, &h2.1, end, stop, |h| h.0 >= round) {
                    return;
                }
                count.fetch_sub(1, Ordering::SeqCst);
                h2.0.lock().unwrap().1 = round;
                h2.1.notify_all();
            }
        });
        let mut lost = 0u32;
        let mut round = 0;
        while round < ROUNDS && !stop.load(Ordering::SeqCst) {
            if Instant::now() >= end {
                stop.store(true, Ordering::SeqCst);
                hand.1.notify_all();
                break;
            }
            park.park(
                || false,
                || {
                    let start = count.load(Ordering::SeqCst);
                    let at = Instant::now();
                    let mut g = tokens.0.lock().unwrap();
                    while *g == 0 && at.elapsed() < Duration::from_millis(100) {
                        g = tokens
                            .1
                            .wait_timeout(g, Duration::from_millis(100))
                            .unwrap()
                            .0;
                    }
                    if *g > 0 {
                        *g -= 1;
                    } else if start > 0 {
                        lost += 1;
                    }
                },
            );
            park.take();
            if count.load(Ordering::SeqCst) > 0 {
                round += 1;
                hand.0.lock().unwrap().0 = round;
                hand.1.notify_all();
                // the waiter lowers the count before its next round
                if !wait_for(&hand.0, &hand.1, end, stop, |h| h.1 >= round) {
                    break;
                }
            }
        }
        waiter.join().unwrap();
        assert!(
            !stop.load(Ordering::SeqCst),
            "the stress test passed its {DEADLINE:?} deadline at round {round} of {ROUNDS}"
        );
        assert_eq!(lost, 0, "sleeps that began with a waiter and timed out");
    }

    #[test]
    fn a_count_above_zero_returns_the_wait_at_once() {
        let count: &'static AtomicI32 = Box::leak(Box::new(AtomicI32::new(1)));
        let (mut regs, park, stats) = (Regs::default(), Park::new(), WaitStats::new());
        park.set_waiters(count);
        let mut f = Fake::default();
        let mut out = Vec::new();
        run(
            &mut f,
            &mut regs,
            &park,
            &stats,
            &set(&[(3, R)]),
            None,
            &mut out,
        );
        assert!(out.is_empty());
        assert_eq!(f.slept, 0);
        assert!(
            stats
                .text(&park)
                .contains(":lock-waiters 1 :parks-with-waiters 0")
        );
    }

    #[test]
    fn select_now_reports_a_ready_pipe() {
        let mut p = [-1; 2];
        // SAFETY: two writable descriptors' room.
        assert_eq!(unsafe { libc::pipe(p.as_mut_ptr()) }, 0);
        let mut out = Vec::new();
        select_now(&[(p[0], R), (p[1], W)], &mut out);
        assert_eq!(
            out,
            vec![(p[1], W)],
            "the empty pipe is writable, not readable"
        );
        // SAFETY: our pipe's write end; one byte.
        assert_eq!(unsafe { libc::write(p[1], [1u8].as_ptr().cast(), 1) }, 1);
        out.clear();
        select_now(&[(p[0], R)], &mut out);
        assert_eq!(out, vec![(p[0], R)]);
        for fd in p {
            // SAFETY: ours, closed once.
            unsafe { libc::close(fd) };
        }
    }
}

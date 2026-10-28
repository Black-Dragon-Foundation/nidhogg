//! The presenter thread: one per display (`nh-present`), composing and committing off the host's thread.
//!
//! The host's thread publishes packets (`publish.rs`) and records commit-time state on the trees (`present.rs`); the
//! reader's handlers record frame callbacks, releases and feedback.  Each of them wakes the presenter through its
//! eventfd instead of posting `FRAME_READY`.  A wake runs one turn:
//! 1. drain the mailbox, writing patches into the fronts;
//! 2. for each registered root, `present_tree`: decide under the tree's mutex and take the buffers out, compose
//!    without it, then take it again and, unless the tree was retired meanwhile, send and commit;
//! 3. flush (a full socket is the reader's to retry);
//! 4. drop fronts idle for the pool's quiet period, at a timerfd armed for exactly that, then disarmed.
//!
//! The thread blocks every asynchronous signal, runs its loop in `catch_unwind` (a panic stops presenting and posts
//! `DISCONNECTED`), never calls into C or Lisp, never reads the socket and never dispatches a queue.  It owns the stage
//! (fronts, the composer) while it runs and hands it back when it stops; inline mode runs `present_tree` on the
//! host's thread with the same stage, so there is one rule.  Glitnir's global state is the host's: the thread is
//! marked a compose thread.
//!
//! **Waits of the host's thread** are bounded: a tree's mutex (the presenter holds it to decide and to send, never
//! across a compose or a flush), the mailbox's and the root list's locks (held for a push or a copy), and the join at
//! close (at most one compose).  The longest of them is `main_wait_max_ns`.

use crate::api::*;
use crate::wayland::MainState;
use crate::wayland::conn::{Shared, Wake};
use crate::wayland::present::{Present, present_tree};
use crate::wayland::publish::{Mailbox, Stage, now_ns};
use crate::wayland::reader::ReaderState;
use rustix::event::{PollFd, PollFlags, poll};
use rustix::fd::{FromRawFd, OwnedFd};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use wayland_client::protocol::wl_compositor::WlCompositor;
use wayland_client::protocol::wl_shm::WlShm;
use wayland_client::{Connection, QueueHandle};
use wayland_protocols::wp::presentation_time::client::wp_presentation::WpPresentation;

/// A trim that could not run (a publish was in progress) is tried again this much later.
const TRIM_RETRY_NS: u64 = 10_000_000;
/// The refresh period assumed until presentation feedback reports one (120 Hz).
pub(crate) const DEFAULT_REFRESH_NS: u64 = 8_333_333;

/// What presenting sends with: the connection, the globals its requests use and the queues its objects go on.
pub(crate) struct Wire {
    pub(crate) conn: Connection,
    pub(crate) shared: Arc<Shared>,
    pub(crate) compositor: Option<WlCompositor>,
    pub(crate) presentation: Option<WpPresentation>,
    pub(crate) shm: Option<WlShm>,
    pub(crate) mqh: QueueHandle<MainState>,
    pub(crate) rqh: QueueHandle<ReaderState>,
}

/// The presenter's counters since open; any thread adds to them.
#[derive(Default)]
pub struct Counters {
    pub packets: AtomicU64,
    pub superseded: AtomicU64,
    pub commits: AtomicU64,
    pub bare_commits: AtomicU64,
    pub main_wait_max_ns: AtomicU64,
    pub publish_copy_max_ns: AtomicU64,
    pub front_trims: AtomicU64,
    /// Root buffers that showed published drawing, those committed more than a refresh period after the tree could
    /// have committed, and the longest such time.
    pub frames: AtomicU64,
    pub late: AtomicU64,
    pub ready_max_ns: AtomicU64,
}

impl Counters {
    pub fn add(c: &AtomicU64, n: u64) {
        c.fetch_add(n, Ordering::Relaxed);
    }

    pub fn max(c: &AtomicU64, v: u64) {
        c.fetch_max(v, Ordering::Relaxed);
    }

    pub fn stats(&self, thread_on: bool) -> NhShellPresenterStats {
        let g = |c: &AtomicU64| c.load(Ordering::Relaxed);
        NhShellPresenterStats {
            packets: g(&self.packets),
            superseded: g(&self.superseded),
            commits: g(&self.commits),
            bare_commits: g(&self.bare_commits),
            main_wait_max_ns: g(&self.main_wait_max_ns),
            publish_copy_max_ns: g(&self.publish_copy_max_ns),
            front_trims: g(&self.front_trims),
            thread_on: u32::from(thread_on),
        }
    }

    pub fn timing(&self) -> PresenterTiming {
        let g = |c: &AtomicU64| c.load(Ordering::Relaxed);
        PresenterTiming {
            frames: g(&self.frames),
            late: g(&self.late),
            ready_to_commit_max_ns: g(&self.ready_max_ns),
        }
    }

    /// A root buffer that shows published drawing committed `delay` ns after its tree could commit (the later of its
    /// first publish and the pacer's opening), with the output refreshing every `refresh` ns: later than one refresh is
    /// a frame the presenter missed.
    pub fn frame(&self, delay: u64, refresh: u64) {
        Counters::add(&self.frames, 1);
        Counters::max(&self.ready_max_ns, delay);
        if delay > refresh {
            Counters::add(&self.late, 1);
        }
    }
}

fn locked<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// What a display's threads share with its presenter: the wake, the mode, the registered roots and the counters.
pub struct Link {
    wake: Wake,
    /// Thread mode: pacers wake the presenter instead of posting `FRAME_READY`.
    on: AtomicBool,
    stop: AtomicBool,
    /// The running thread's "finished" report (`None`: no thread was started).  A thread started again gets a new one.
    done: Mutex<Option<Arc<ThreadDone>>>,
    /// Roots whose role setup is done, oldest first.
    roots: Mutex<Vec<(u64, Arc<Mutex<Present>>)>>,
    wire: Mutex<Option<Arc<Wire>>>,
    pub counters: Arc<Counters>,
    /// Test hook: composing takes at least this long (`NHSHELL_TEST_COMPOSE_DELAY_MS` at open).
    delay_ms: AtomicU64,
    /// The running thread's kernel id (0: none), for the tests that watch its wake-ups.
    tid: AtomicI32,
}

impl Link {
    pub(crate) fn new() -> ShellResult<Link> {
        let delay = std::env::var("NHSHELL_TEST_COMPOSE_DELAY_MS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(0);
        Ok(Link {
            wake: Wake::new()?,
            on: AtomicBool::new(false),
            stop: AtomicBool::new(false),
            done: Mutex::default(),
            roots: Mutex::default(),
            wire: Mutex::default(),
            counters: Arc::default(),
            delay_ms: AtomicU64::new(delay),
            tid: AtomicI32::new(0),
        })
    }

    /// Thread mode is on.
    pub fn on(&self) -> bool {
        self.on.load(Ordering::Acquire)
    }

    pub(crate) fn wake(&self) {
        self.wake.signal();
    }

    pub(crate) fn set_wire(&self, w: Wire) {
        *locked(&self.wire) = Some(Arc::new(w));
    }

    pub(crate) fn wire(&self) -> Option<Arc<Wire>> {
        locked(&self.wire).clone()
    }

    /// The shell closes, its presenter joined: the roots and the connection go (a presenter state that a queued
    /// event keeps must not keep them).
    pub(crate) fn release(&self) {
        locked(&self.roots).clear();
        *locked(&self.wire) = None;
    }

    fn timed_roots(&self) -> MutexGuard<'_, Vec<(u64, Arc<Mutex<Present>>)>> {
        if let Ok(g) = self.roots.try_lock() {
            return g;
        }
        let t = now_ns();
        let g = locked(&self.roots);
        Counters::max(&self.counters.main_wait_max_ns, now_ns().saturating_sub(t));
        g
    }

    /// Root `id` is registered (its role setup is done).
    pub(crate) fn add_root(&self, id: u64, tree: Arc<Mutex<Present>>) {
        self.timed_roots().push((id, tree));
    }

    pub(crate) fn remove_root(&self, tree: &Arc<Mutex<Present>>) {
        self.timed_roots().retain(|(_, t)| !Arc::ptr_eq(t, tree));
    }

    /// The registered roots, for a host that switches modes.
    pub(crate) fn roots(&self) -> Vec<(u64, Arc<Mutex<Present>>)> {
        locked(&self.roots).clone()
    }

    pub(crate) fn set_delay_ms(&self, ms: u64) {
        self.delay_ms.store(ms, Ordering::Relaxed);
    }

    fn delay(&self) -> Duration {
        Duration::from_millis(self.delay_ms.load(Ordering::Relaxed))
    }

    /// A host thread's wait for a lock the presenter may hold.
    pub(crate) fn note_wait(&self, ns: u64) {
        if !is_presenter_thread() && !crate::wayland::reader::is_reader_thread() {
            Counters::max(&self.counters.main_wait_max_ns, ns);
        }
    }
}

/// The presenter as a host abandoning the shell sees it (`ShellBackend::abandon`): listed from open, it covers the
/// thread whenever one runs.  Both calls touch only the stop flag, the wake-up and the done report.
impl BackendThread for Link {
    fn stop(&self) {
        self.stop.store(true, Ordering::Release);
        self.wake.signal();
    }

    /// Done at once when no thread was started.
    fn wait_done(&self, deadline: Instant) -> bool {
        let done = locked(&self.done).clone();
        done.is_none_or(|d| d.wait(deadline))
    }
}

thread_local! {
    static PRESENTER_THREAD: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Whether the calling thread is a presenter thread (the glue refuses to run there).
pub fn is_presenter_thread() -> bool {
    PRESENTER_THREAD.get()
}

/// The trim timer: a monotonic timerfd armed at an absolute time, or disarmed.
struct Timer(Option<OwnedFd>);

impl Timer {
    fn new() -> Timer {
        // SAFETY: plain timerfd creation; the descriptor is owned from here on.
        let fd = unsafe {
            libc::timerfd_create(
                libc::CLOCK_MONOTONIC,
                libc::TFD_NONBLOCK | libc::TFD_CLOEXEC,
            )
        };
        // SAFETY: a descriptor timerfd_create just returned, owned by nothing else.
        Timer((fd >= 0).then(|| unsafe { OwnedFd::from_raw_fd(fd) }))
    }

    /// Fire at `at` (`CLOCK_MONOTONIC` ns), or never with `None`.
    fn set(&self, at: Option<u64>) {
        let Some(fd) = &self.0 else { return };
        let at = at.unwrap_or(0).max(u64::from(at.is_some()));
        let spec = libc::itimerspec {
            it_interval: libc::timespec {
                tv_sec: 0,
                tv_nsec: 0,
            },
            it_value: libc::timespec {
                tv_sec: (at / 1_000_000_000) as libc::time_t,
                tv_nsec: (at % 1_000_000_000) as libc::c_long,
            },
        };
        // SAFETY: `spec` is a valid itimerspec; the old value is not asked for.
        unsafe {
            libc::timerfd_settime(
                std::os::fd::AsRawFd::as_raw_fd(fd),
                libc::TFD_TIMER_ABSTIME,
                &spec,
                std::ptr::null_mut(),
            )
        };
    }

    fn drain(&self) {
        if let Some(fd) = &self.0 {
            let mut buf = [0u8; 8];
            let _ = rustix::io::read(fd, &mut buf);
        }
    }
}

/// Start the presenter with `stage`, every signal blocked (the host's handlers run on the host's threads only).
pub(crate) fn spawn(
    link: Arc<Link>,
    mail: Arc<Mailbox>,
    stage: Stage,
) -> std::io::Result<JoinHandle<Option<Stage>>> {
    link.stop.store(false, Ordering::Release);
    let done = Arc::new(ThreadDone::default());
    *locked(&link.done) = Some(done.clone());
    let finished = done.clone();
    // SAFETY: plain libc calls on stack-local sigset_t values; `pthread_sigmask` changes only this thread's mask,
    // which is restored right after the spawn (the new thread inherits the full mask).
    let old = unsafe {
        let mut all: libc::sigset_t = std::mem::zeroed();
        let mut old: libc::sigset_t = std::mem::zeroed();
        libc::sigfillset(&mut all);
        libc::pthread_sigmask(libc::SIG_SETMASK, &all, &mut old);
        old
    };
    let spawned = std::thread::Builder::new()
        .name("nh-present".into())
        .spawn(move || {
            PRESENTER_THREAD.set(true);
            glitnir::mark_compose_thread();
            // SAFETY: gettid has no preconditions.
            link.tid.store(unsafe { libc::gettid() }, Ordering::Release);
            let out = catch_unwind(AssertUnwindSafe(|| run(&link, &mail, stage)));
            if out.is_err()
                && let Some(w) = link.wire()
            {
                // a panic is a bug: presenting stops, and the host hears DISCONNECTED once
                w.shared.close();
            }
            let stage = out.ok();
            drop(link);
            finished.set();
            stage
        });
    if spawned.is_err() {
        // no thread: nothing to wait for
        done.set();
    }
    // SAFETY: restores the mask saved above.
    unsafe { libc::pthread_sigmask(libc::SIG_SETMASK, &old, std::ptr::null_mut()) };
    spawned
}

/// The loop; returns the stage when asked to stop.
fn run(link: &Link, mail: &Mailbox, mut stage: Stage) -> Stage {
    let timer = Timer::new();
    let mut armed: Option<u64> = None;
    loop {
        if link.stop.load(Ordering::Acquire) {
            return stage;
        }
        let wire = link.wire();
        let (gone, timer_fired) = {
            let wake = link.wake.fd();
            // the connection's loss: readable from then on
            let lost = wire.as_ref().map(|w| w.shared.gone.fd());
            let mut fds = vec![PollFd::new(&wake, PollFlags::IN)];
            if let Some(l) = &lost {
                fds.push(PollFd::new(l, PollFlags::IN));
            }
            let at = fds.len();
            if let Some(t) = timer.0.as_ref().filter(|_| armed.is_some()) {
                fds.push(PollFd::new(t, PollFlags::IN));
            }
            if let Err(e) = poll(&mut fds, None)
                && e != rustix::io::Errno::INTR
            {
                // a poll that cannot wait would spin: stop presenting, as on a panic
                panic!("nh-present: poll failed: {e}");
            }
            let fired = |i: usize| {
                fds.get(i)
                    .is_some_and(|f| f.revents().contains(PollFlags::IN))
            };
            (wire.is_some() && fired(1), fired(at))
        };
        if gone {
            // nothing can be sent any more: presenting ends, and the stage goes back at the join
            return stage;
        }
        // drained before the work: a wake during it leaves the descriptor readable for the next turn
        link.wake.drain();
        if link.stop.load(Ordering::Acquire) {
            return stage;
        }
        stage.drain(mail);
        if let Some(wire) = wire {
            let delay = link.delay();
            for (id, tree) in link.roots() {
                present_tree(&mut stage, &wire, id, &tree, delay);
            }
            // EAGAIN is the reader's to retry; any other failure reaches the host through the reader's DISCONNECTED
            let _ = wire.shared.host_flush(&wire.conn);
        }
        if timer_fired {
            timer.drain();
        }
        let now = now_ns();
        let mut next = stage.next_trim().map(|t| t + 1_000_000);
        if next.is_some_and(|t| t <= now) {
            next = if mail.trim(|| stage.trim(now)) {
                stage.next_trim().map(|t| t + 1_000_000)
            } else {
                Some(now + TRIM_RETRY_NS)
            };
        }
        if next != armed {
            timer.set(next);
            armed = next;
        }
    }
}

impl crate::wayland::WaylandShell {
    /// Present on the `nh-present` thread (`on`) or inline.  Turning it on hands the stage to a new thread; turning it
    /// off stops and joins the thread, takes the stage back, and lets each ready root post `FRAME_READY` again.
    pub(super) fn set_presenter_thread_impl(&mut self, on: bool) -> ShellResult {
        if on == self.presenting.thread.is_some() {
            return Ok(());
        }
        if on {
            let stage = self.presenting.stage.take().ok_or(ShellError::State)?;
            self.link.on.store(true, Ordering::Release);
            match spawn(self.link.clone(), self.main.mailbox.clone(), stage) {
                Ok(t) => {
                    self.presenting.thread = Some(t);
                    self.link.wake();
                    Ok(())
                }
                Err(_) => {
                    self.link.on.store(false, Ordering::Release);
                    self.presenting.restage(None);
                    Err(ShellError::NoMem)
                }
            }
        } else {
            self.presenter_stop();
            for (_, tree) in self.link.roots() {
                let mut p = crate::wayland::present::lock(&tree);
                p.renotify();
            }
            Ok(())
        }
    }

    /// Stop and join the presenter thread (at most one compose), and take its stage back.
    pub(super) fn presenter_stop(&mut self) {
        self.link.on.store(false, Ordering::Release);
        if let Some(t) = self.presenting.thread.take() {
            BackendThread::stop(&*self.link);
            let stage = t.join().ok().flatten();
            self.presenting.restage(stage);
        }
    }

    /// Test hook: composing takes at least `ms` (the presenter thread only).
    #[doc(hidden)]
    pub fn set_compose_delay(&self, ms: u64) {
        self.link.set_delay_ms(ms);
    }

    /// Test hook: the presenter thread's kernel id, once it runs.
    #[doc(hidden)]
    pub fn presenter_tid(&self) -> Option<i32> {
        let tid = self.link.tid.load(Ordering::Acquire);
        (self.presenting.thread.is_some() && tid != 0).then_some(tid)
    }
}

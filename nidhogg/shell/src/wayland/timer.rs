//! The host's timer (`ShellBackend::set_timer`): one deadline on `CLOCK_MONOTONIC`, kept by a thread of its own that
//! posts a `Timer` record and wakes the host when it is due, so the host's own deadlines (the glue's minimap timers)
//! end its wait as an event does.  The thread starts with the first deadline and stops with the shell.

use crate::event::{Record, ShellEvent};
use crate::wayland::conn::Shared;
use crate::wayland::publish::now_ns;
use std::sync::{Arc, Condvar, Mutex, Weak};
use std::thread::JoinHandle;
use std::time::Duration;

#[derive(Default)]
struct State {
    at: Option<u64>,
    stop: bool,
}

type Inner = (Mutex<State>, Condvar);

/// One deadline and the thread that keeps it.
#[derive(Default)]
pub struct HostTimer {
    inner: Arc<Inner>,
    thread: Option<JoinHandle<()>>,
}

impl HostTimer {
    /// Post `Timer` at `at_ns` (`None`: never), replacing the deadline set before.  A deadline already past posts at
    /// once.
    pub fn set(&mut self, shared: &Arc<Shared>, at_ns: Option<u64>) {
        let (m, cv) = &*self.inner;
        m.lock().unwrap_or_else(|e| e.into_inner()).at = at_ns;
        cv.notify_one();
        if self.thread.is_none() && at_ns.is_some() {
            let inner = Arc::clone(&self.inner);
            let shared = Arc::downgrade(shared);
            self.thread = std::thread::Builder::new()
                .name("nhshell-timer".into())
                .spawn(move || run(&inner, &shared))
                .ok();
        }
    }
}

fn run(inner: &Inner, shared: &Weak<Shared>) {
    let (m, cv) = inner;
    let mut s = m.lock().unwrap_or_else(|e| e.into_inner());
    loop {
        if s.stop {
            return;
        }
        match s.at {
            None => s = cv.wait(s).unwrap_or_else(|e| e.into_inner()),
            Some(t) => {
                let now = now_ns();
                if now < t {
                    s = cv
                        .wait_timeout(s, Duration::from_nanos(t - now))
                        .unwrap_or_else(|e| e.into_inner())
                        .0;
                    continue;
                }
                s.at = None;
                drop(s);
                if let Some(sh) = shared.upgrade() {
                    sh.post(Record::new(ShellEvent::Timer, 0, now));
                    sh.wake.signal();
                }
                s = m.lock().unwrap_or_else(|e| e.into_inner());
            }
        }
    }
}

impl Drop for HostTimer {
    fn drop(&mut self) {
        let (m, cv) = &*self.inner;
        m.lock().unwrap_or_else(|e| e.into_inner()).stop = true;
        cv.notify_one();
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Wait (at most 5 s) until `n` `Timer` records are queued; how many there are.
    fn timers(sh: &Shared, n: usize) -> usize {
        let mut got = 0;
        let end = now_ns() + 5_000_000_000;
        loop {
            got += std::iter::from_fn(|| sh.take().ok().flatten())
                .filter(|r| matches!(r.event, ShellEvent::Timer))
                .count();
            if got >= n || now_ns() > end {
                return got;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    /// A deadline posts one `Timer` record; a new deadline replaces the one before, `None` cancels it, and one already
    /// past posts at once.
    #[test]
    fn a_deadline_posts_one_timer_and_a_new_one_replaces_it() {
        let shared = Arc::new(Shared::new().unwrap());
        let mut t = HostTimer::default();
        t.set(&shared, Some(now_ns() + 2_000_000_000));
        t.set(&shared, Some(now_ns() + 10_000_000));
        assert_eq!(timers(&shared, 1), 1);
        // cancelled long before it is due: nothing comes (the 2 s one was replaced too)
        t.set(&shared, Some(now_ns() + 1_000_000_000));
        t.set(&shared, None);
        std::thread::sleep(Duration::from_millis(1500));
        assert_eq!(timers(&shared, 0), 0);
        t.set(&shared, Some(0));
        assert_eq!(timers(&shared, 1), 1);
    }
}

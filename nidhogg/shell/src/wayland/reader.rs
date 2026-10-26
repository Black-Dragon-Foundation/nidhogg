//! The reader thread (MP-NS11, spec §2.3): the only caller of `prepare_read`/`read_events`.
//!
//! Emacs drops its global lock around `pselect`, and libwayland's `read_events` waits for every thread that prepared a
//! read, so reading from the host's threads can deadlock with Lisp threads.  Here one thread reads, with every signal
//! blocked and its loop in `catch_unwind`.  Its queue owns input, ping, key repeat and frame callbacks; its handlers
//! answer pings and post plain records to the host, which it wakes through the eventfd.  Configure and surface objects
//! sit on the main queue, which the host's thread dispatches.
//!
//! The loop: `prepare_read` (dispatching what is queued until it succeeds) → flush → `poll(fd, POLLIN | (want_flush ?
//! POLLOUT : 0))` together with `reader_wake` → read, or cancel → dispatch the reader queue.  A failed read, a failed
//! dispatch or a panic posts `DISCONNECTED` and ends the thread.

use crate::event::{FocusEv, NHSHELL_EV_FOCUS, NhShellEvent};
use crate::wayland::conn::Shared;
use crate::wayland::serial::SerialKind;
use rustix::event::{PollFd, PollFlags, poll};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::task::{Context, Poll, Wake, Waker};
use std::thread::{JoinHandle, ThreadId};
use wayland_client::backend::WaylandError;
use wayland_client::protocol::wl_keyboard::{self, KeyState, WlKeyboard};
use wayland_client::protocol::wl_seat::{self, Capability, WlSeat};
use wayland_client::{Connection, Dispatch, EventQueue, Proxy, QueueHandle, WEnum};
use wayland_protocols::xdg::shell::client::xdg_wm_base::{self, XdgWmBase};

/// What the reader's handlers own.  Objects created with this queue's handle run their events on the reader thread.
pub struct ReaderState {
    pub shared: Arc<Shared>,
    pub seat: Option<WlSeat>,
    pub wm_base: Option<XdgWmBase>,
    keyboard: Option<WlKeyboard>,
    /// A record was posted since the last wake.
    posted: bool,
}

impl ReaderState {
    pub fn new(shared: Arc<Shared>) -> ReaderState {
        ReaderState {
            shared,
            seat: None,
            wm_base: None,
            keyboard: None,
            posted: false,
        }
    }

    fn post(&mut self, ev: NhShellEvent) {
        self.shared.post(ev);
        self.posted = true;
    }

    /// Wake the host once for the records posted since the last call.
    pub fn wake_host(&mut self) {
        if std::mem::take(&mut self.posted) {
            self.shared.wake.signal();
        }
    }
}

/// The reader queue's waker: an event routed into the reader's queue by another thread (the host's dispatch of the
/// shared libwayland queue) wakes the reader's poll.  On the reader thread itself it does nothing: the loop dispatches
/// after every read anyway.
struct ReaderWaker {
    shared: Arc<Shared>,
    me: ThreadId,
}

impl Wake for ReaderWaker {
    fn wake(self: Arc<Self>) {
        if std::thread::current().id() != self.me {
            self.shared.reader_wake.signal();
        }
    }
}

struct Reader {
    conn: Connection,
    queue: EventQueue<ReaderState>,
    state: ReaderState,
    waker: Waker,
}

/// The connection failed (or the reader may not go on).
struct Lost;

impl Reader {
    /// Dispatch the reader queue (and register the waker), then wake the host for what was posted.
    fn dispatch(&mut self) -> Result<(), Lost> {
        let mut cx = Context::from_waker(&self.waker);
        if let Poll::Ready(Err(_)) = self.queue.poll_dispatch_pending(&mut cx, &mut self.state) {
            return Err(Lost);
        }
        self.state.wake_host();
        Ok(())
    }

    /// Flush the reader's own requests (pongs) and whatever the host left behind; EAGAIN waits for POLLOUT.
    fn flush(&self) -> Result<(), Lost> {
        match self.conn.flush() {
            Ok(()) => Ok(()),
            Err(WaylandError::Io(e)) if e.kind() == std::io::ErrorKind::WouldBlock => {
                self.state.shared.want_flush.store(true, Ordering::Release);
                Ok(())
            }
            Err(_) => Err(Lost),
        }
    }

    /// Ok when the shell asked the reader to stop; Err when the connection is lost.
    fn run(&mut self) -> Result<(), Lost> {
        let shared = self.state.shared.clone();
        loop {
            if shared.stop.load(Ordering::Acquire) {
                return Ok(());
            }
            // 1. prepare; a non-empty libwayland queue is routed and dispatched first
            let guard = loop {
                if let Some(g) = self.conn.prepare_read() {
                    break g;
                }
                self.conn
                    .backend()
                    .dispatch_inner_queue()
                    .map_err(|_| Lost)?;
                self.dispatch()?;
            };
            // 2. send what is queued
            self.flush()?;
            // 3. wait for the compositor, a writable socket when a flush is pending, or the shell
            let want_out = shared.want_flush.load(Ordering::Acquire);
            let (conn_rev, wake_rev) = {
                let fd = guard.connection_fd();
                let wake_fd = shared.reader_wake.fd();
                let mut flags = PollFlags::IN;
                if want_out {
                    flags |= PollFlags::OUT;
                }
                let mut fds = [
                    PollFd::new(&fd, flags),
                    PollFd::new(&wake_fd, PollFlags::IN),
                ];
                match poll(&mut fds, None) {
                    Ok(_) => {}
                    // signals are blocked here, but a cancelled read and another turn is always safe
                    Err(rustix::io::Errno::INTR) => continue,
                    Err(_) => return Err(Lost),
                }
                (fds[0].revents(), fds[1].revents())
            };
            if wake_rev.contains(PollFlags::IN) {
                shared.reader_wake.drain();
            }
            if conn_rev.contains(PollFlags::OUT) {
                shared.want_flush.store(false, Ordering::Release);
                self.flush()?;
            }
            // 4. read, or cancel the prepared read
            if conn_rev.intersects(PollFlags::IN | PollFlags::ERR | PollFlags::HUP) {
                match guard.read() {
                    Ok(_) => {}
                    Err(WaylandError::Io(e)) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                    Err(_) => return Err(Lost),
                }
            } else {
                drop(guard);
            }
            // 5. the reader's handlers
            self.dispatch()?;
        }
    }
}

/// Start the reader on `queue` (already holding the setup's events) with every signal blocked: the host's signal
/// handlers run on the host's threads only.
pub fn spawn(
    conn: Connection,
    queue: EventQueue<ReaderState>,
    mut state: ReaderState,
) -> std::io::Result<JoinHandle<()>> {
    // events dispatched during setup may have posted records before the loop could wake the host
    state.wake_host();
    let shared = state.shared.clone();
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
        .name("nhshell-reader".into())
        .spawn(move || {
            let outcome = catch_unwind(AssertUnwindSafe(move || {
                let waker = Waker::from(Arc::new(ReaderWaker {
                    shared: state.shared.clone(),
                    me: std::thread::current().id(),
                }));
                Reader {
                    conn,
                    queue,
                    state,
                    waker,
                }
                .run()
            }));
            // a lost connection or a panic: the host hears DISCONNECTED once
            if !matches!(outcome, Ok(Ok(()))) {
                shared.close();
            }
        });
    // SAFETY: restores the mask saved above.
    unsafe { libc::pthread_sigmask(libc::SIG_SETMASK, &old, std::ptr::null_mut()) };
    spawned
}

impl Dispatch<WlSeat, ()> for ReaderState {
    fn event(
        state: &mut Self,
        seat: &WlSeat,
        event: wl_seat::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let wl_seat::Event::Capabilities {
            capabilities: WEnum::Value(caps),
        } = event
        {
            let has = caps.contains(Capability::Keyboard);
            match (has, state.keyboard.take()) {
                (true, None) => state.keyboard = Some(seat.get_keyboard(qh, ())),
                (false, Some(kb)) if kb.version() >= 3 => kb.release(),
                (_, kb) => state.keyboard = kb,
            }
        }
    }
}

impl Dispatch<WlKeyboard, ()> for ReaderState {
    fn event(
        state: &mut Self,
        _: &WlKeyboard,
        event: wl_keyboard::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        // The window id is the surface's protocol id until windows have their own (W0 Task 5).
        let focus = |surface: &wayland_client::protocol::wl_surface::WlSurface, on: u8| {
            let mut ev = NhShellEvent::new(NHSHELL_EV_FOCUS, surface.id().protocol_id().into(), 0);
            ev.u.focus = FocusEv { r#in: on };
            ev
        };
        match event {
            wl_keyboard::Event::Enter { surface, .. } => state.post(focus(&surface, 1)),
            wl_keyboard::Event::Leave { surface, .. } => state.post(focus(&surface, 0)),
            wl_keyboard::Event::Key {
                serial,
                state: WEnum::Value(KeyState::Pressed),
                ..
            } => {
                let mut serials = state
                    .shared
                    .serials
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                serials.update(SerialKind::KeyPress, serial);
            }
            // the keymap's descriptor is closed when the event drops; keys and repeat are W0 Task 7
            _ => {}
        }
    }
}

impl Dispatch<XdgWmBase, ()> for ReaderState {
    fn event(
        _: &mut Self,
        wm_base: &XdgWmBase,
        event: xdg_wm_base::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        // answered here, whatever the host's thread is doing; the loop flushes before it polls again
        if let xdg_wm_base::Event::Ping { serial } = event {
            wm_base.pong(serial);
        }
    }
}

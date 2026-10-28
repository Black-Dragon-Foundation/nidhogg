//! The reader thread: the only caller of `prepare_read`/`read_events`.
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
use crate::wayland::clipboard::SendBytes;
use crate::wayland::conn::Shared;
use crate::wayland::gesture::PinchState;
use crate::wayland::keyboard::KeyboardState;
use crate::wayland::pointer::{self, PointerState};
use crate::wayland::selection;
use crate::wayland::serial::SerialKind;
use crate::wayland::touch::{self, TouchState};
use rustix::event::{PollFd, PollFlags, poll};
use std::os::fd::AsFd;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Weak};
use std::task::{Context, Poll, Wake, Waker};
use std::thread::{JoinHandle, ThreadId};
use wayland_client::backend::WaylandError;
use wayland_client::protocol::wl_keyboard::{self, KeyState, WlKeyboard};
use wayland_client::protocol::wl_seat::{self, Capability, WlSeat};
use wayland_client::{Connection, Dispatch, EventQueue, Proxy, QueueHandle, WEnum};
use wayland_protocols::wp::cursor_shape::v1::client::wp_cursor_shape_manager_v1::WpCursorShapeManagerV1;
use wayland_protocols::wp::pointer_gestures::zv1::client::zwp_pointer_gestures_v1::ZwpPointerGesturesV1;
use wayland_protocols::xdg::shell::client::xdg_wm_base::{self, XdgWmBase};

/// What the reader's handlers own.  Objects created with this queue's handle run their events on the reader thread.
pub struct ReaderState {
    pub shared: Arc<Shared>,
    pub seat: Option<WlSeat>,
    pub wm_base: Option<XdgWmBase>,
    keyboard: Option<WlKeyboard>,
    /// xkb, compose and key repeat (`keyboard.rs`).
    pub kbd: KeyboardState,
    pub cursor_manager: Option<WpCursorShapeManagerV1>,
    pub pointer: PointerState,
    /// Pointer gestures (`gesture.rs`): the global and the pointer's pinch.
    pub gestures: Option<ZwpPointerGesturesV1>,
    pub pinch: PinchState,
    /// Touch points (`touch.rs`).
    pub touch: TouchState,
    /// text-input-v3 (`text_input.rs`).
    pub text_input: Arc<crate::wayland::text_input::TextInput>,
    /// A record was posted since the last wake.
    posted: bool,
    /// Our selection bytes being written to other clients (`selection.rs`).
    pub sends: Vec<SendBytes>,
}

impl ReaderState {
    pub fn new(shared: Arc<Shared>) -> ReaderState {
        ReaderState {
            shared,
            seat: None,
            wm_base: None,
            keyboard: None,
            kbd: KeyboardState::new(),
            cursor_manager: None,
            pointer: PointerState::default(),
            gestures: None,
            pinch: PinchState::default(),
            touch: TouchState::default(),
            text_input: Arc::default(),
            posted: false,
            sends: Vec::new(),
        }
    }

    fn post_all(&mut self, evs: Vec<NhShellEvent>) {
        for ev in evs {
            self.post(ev);
        }
    }

    pub(super) fn post(&mut self, ev: NhShellEvent) {
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
/// after every read anyway.  The shared state is held weakly, as the main queue's waker holds it.
struct ReaderWaker {
    shared: Weak<Shared>,
    me: ThreadId,
}

impl Wake for ReaderWaker {
    fn wake(self: Arc<Self>) {
        if std::thread::current().id() != self.me
            && let Some(shared) = self.shared.upgrade()
        {
            shared.reader_wake.signal();
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
                // take what is queued for these handlers: an event left in a queue nobody dispatches keeps its
                // object's data, and any descriptor it carries, for good
                let _ = self.dispatch();
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
            let (conn_rev, wake_rev, timer_rev, send_revs) = {
                let fd = guard.connection_fd();
                let wake_fd = shared.reader_wake.fd();
                let mut flags = PollFlags::IN;
                if want_out {
                    flags |= PollFlags::OUT;
                }
                // key repeat's timerfd (absent when the system could not make one: no repeat then)
                let timer_fd = self.state.kbd.timer_fd().map(|t| t.as_fd());
                let mut fds = vec![
                    PollFd::new(&fd, flags),
                    PollFd::new(&wake_fd, PollFlags::IN),
                ];
                if let Some(t) = &timer_fd {
                    fds.push(PollFd::new(t, PollFlags::IN));
                }
                // selection bytes waiting for another client to read them, until the earliest one's deadline
                let first_send = fds.len();
                for s in &self.state.sends {
                    fds.push(PollFd::new(s, PollFlags::OUT));
                }
                match poll(&mut fds, self.state.send_timeout().as_ref()) {
                    Ok(_) => {}
                    // signals are blocked here, but a cancelled read and another turn is always safe
                    Err(rustix::io::Errno::INTR) => continue,
                    Err(_) => return Err(Lost),
                }
                let timer = match timer_fd {
                    Some(_) => fds[2].revents(),
                    None => PollFlags::empty(),
                };
                let sends: Vec<_> = fds[first_send..].iter().map(PollFd::revents).collect();
                (fds[0].revents(), fds[1].revents(), timer, sends)
            };
            if wake_rev.contains(PollFlags::IN) {
                shared.reader_wake.drain();
            }
            if timer_rev.contains(PollFlags::IN) {
                let mut out = Vec::new();
                self.state.kbd.on_timer(&shared, &mut out);
                self.state.post_all(out);
                self.state.wake_host();
            }
            if !send_revs.is_empty() {
                self.state.service_sends(&send_revs);
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

thread_local! {
    /// Set on a reader thread when it starts.
    static READER_THREAD: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Whether the calling thread is a reader thread (the glue refuses to run there).
pub fn is_reader_thread() -> bool {
    READER_THREAD.get()
}

/// The reader as a host abandoning the shell sees it: the stop flag, the reader's wake-up and its done report.
pub struct ReaderThread(pub Arc<Shared>);

impl crate::api::BackendThread for ReaderThread {
    fn stop(&self) {
        self.0.stop.store(true, Ordering::Release);
        self.0.reader_wake.signal();
    }

    fn wait_done(&self, deadline: std::time::Instant) -> bool {
        self.0.reader_done.wait(deadline)
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
            READER_THREAD.set(true);
            let outcome = catch_unwind(AssertUnwindSafe(move || {
                let waker = Waker::from(Arc::new(ReaderWaker {
                    shared: Arc::downgrade(&state.shared),
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
            shared.reader_done.set();
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
            pointer::seat_capabilities(state, seat, caps, qh);
            touch::seat_capabilities(state, seat, caps, qh);
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
        // The window id is the surface's protocol id.
        let focus = |surface: &wayland_client::protocol::wl_surface::WlSurface, on: u8| {
            let mut ev = NhShellEvent::new(NHSHELL_EV_FOCUS, surface.id().protocol_id().into(), 0);
            ev.u.focus = FocusEv { r#in: on };
            ev
        };
        match event {
            wl_keyboard::Event::Enter {
                serial, surface, ..
            } => {
                selection::lock(&state.shared).enter(serial);
                state.kbd.enter(surface.id().protocol_id().into());
                state.post(focus(&surface, 1));
            }
            wl_keyboard::Event::Leave { surface, .. } => {
                selection::lock(&state.shared).leave();
                state.kbd.leave(surface.id().protocol_id().into());
                state.post(focus(&surface, 0));
            }
            wl_keyboard::Event::Keymap {
                format: WEnum::Value(wl_keyboard::KeymapFormat::XkbV1),
                fd,
                size,
            } => {
                if let Some(text) = keymap_text(fd, size) {
                    state.kbd.set_keymap(text);
                }
            }
            wl_keyboard::Event::RepeatInfo { rate, delay } => {
                state.kbd.set_repeat_info(rate, delay)
            }
            wl_keyboard::Event::Modifiers {
                mods_depressed,
                mods_latched,
                mods_locked,
                group,
                ..
            } => {
                let mut out = Vec::new();
                state
                    .kbd
                    .modifiers(mods_depressed, mods_latched, mods_locked, group, &mut out);
                state.post_all(out);
            }
            wl_keyboard::Event::Key {
                serial,
                time,
                key,
                state: WEnum::Value(key_state),
            } => {
                let pressed = key_state == KeyState::Pressed;
                if pressed {
                    let mut serials = state
                        .shared
                        .serials
                        .lock()
                        .unwrap_or_else(|e| e.into_inner());
                    serials.update(SerialKind::KeyPress, serial);
                    selection::lock(&state.shared).input(serial);
                }
                let mut out = Vec::new();
                state.kbd.key(time, key, pressed, &mut out);
                state.post_all(out);
            }
            _ => {}
        }
    }
}

/// The keymap text from the compositor's descriptor.  Read at offset 0 with `pread`: the descriptor shares its file
/// offset with the compositor's.
fn keymap_text(fd: std::os::fd::OwnedFd, size: u32) -> Option<String> {
    use std::os::unix::fs::FileExt;
    let file = std::fs::File::from(fd);
    let mut buf = vec![0u8; size as usize];
    file.read_exact_at(&mut buf, 0).ok()?;
    if let Some(nul) = buf.iter().position(|&b| b == 0) {
        buf.truncate(nul);
    }
    String::from_utf8(buf).ok()
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The marker belongs to the thread that set it, as the reader sets it at its start.
    #[test]
    fn reader_marker_is_per_thread() {
        assert!(!is_reader_thread() && !crate::is_reader_thread());
        let on_reader = std::thread::spawn(|| {
            READER_THREAD.set(true);
            crate::is_reader_thread()
        })
        .join()
        .unwrap();
        assert!(on_reader);
        assert!(!crate::is_reader_thread());
    }
}

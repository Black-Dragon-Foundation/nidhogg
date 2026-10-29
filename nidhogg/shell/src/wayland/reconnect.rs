//! Reconnecting in place: the compositor went (a hangup, not a protocol error) and comes back on the same socket, so
//! the shell connects again and keeps its windows, pools and host-side state, as Qt does with `QT_WAYLAND_RECONNECT`.
//!
//! - **The loss** (reader thread, `reader.rs`): with reconnecting on, a hangup posts the releases of held buttons,
//!   touch cancels, an empty `PREEDIT`, our drag's `DRAG_DONE` and then `RECONNECTING`, holds the records (nothing of
//!   the old connection is queued after it), signals `Shared::gone` and ends the reader.  Its key repeat ends with it.
//! - **The host takes `RECONNECTING`** (`take_record`): each window's `LEAVE_OUTPUT` goes right before it, the
//!   presenter thread is joined, and from then on every call that would send returns `Reconnecting`.  The thread left
//!   at the loss; a compose that ended after it sent nothing (`Wire::gone`) and gave its buffers back, so the stage it
//!   hands back keeps every front, the damage waiting in them and the packets still queued.  Begun buffers, waiting
//!   pacers and content layers belong to the old connection and go when the windows are made again.
//! - **Each `reconnect` step** is bounded by its timeout.  One attempt lives across steps: a non-blocking connect to
//!   the socket path saved at open (a missing, refusing or full socket waits for the next step), then `get_registry`
//!   and `sync` on the new connection, read here until the `sync` is done.  A compositor that keeps its socket
//!   (`kwin_wayland_wrapper`) has the connection wait in its backlog until it accepts.  The connect switches the shell
//!   over at once: the old connection's last clone goes, so libwayland frees its proxies without a request, and its
//!   outputs are removed.  A hangup during the attempt starts another; a protocol error, or a required global missing
//!   after `done`, gives up with `DISCONNECTED`.
//! - **On success:** the globals are bound again (`bind_globals`, the same code as open, so capabilities follow the
//!   new compositor), the seat and its devices come back on a new reader queue, every pool's buffers are made again
//!   from their own memfds with every slot released, the presenter's wire is replaced, a new reader starts, the windows
//!   are made again (`remake_windows`: popups closed, top-levels and children made again with their state, owned
//!   selections offered again at the first serial), the presenter thread starts again if it ran (nothing presents a
//!   tree before its new configure, whose content ack takes a packet published after it) and `RECONNECTED` is
//!   posted.  A drag over us that had not dropped
//!   left at the loss (`DND_LEAVE`, before `RECONNECTING`).

use crate::api::*;
use crate::event::{
    NHSHELL_EV_RECONNECTED, NHSHELL_EV_RECONNECTING, NHSHELL_EV_SELECTION, NhShellEvent,
    SelectionEv,
};
use crate::wayland::conn::{self, Refused};
use crate::wayland::reader::{self, ReaderState};
use crate::wayland::{MainState, WaylandShell, present, selection};
use rustix::event::{PollFd, PollFlags, Timespec, poll};
use std::os::unix::net::UnixStream;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use wayland_client::backend::WaylandError;
use wayland_client::protocol::wl_callback::{self, WlCallback};
use wayland_client::{Connection, Dispatch, QueueHandle};

/// The shell is reconnecting: the host took `RECONNECTING`.
#[derive(Default)]
pub(super) struct Reconnecting {
    /// The attempt in progress: its connection is the shell's since the connect, and this is its `sync`'s `done`.
    attempt: Option<Arc<AtomicBool>>,
    /// The presenter ran on its thread at the loss: it starts again on success.
    presenter_was_on: bool,
}

impl Reconnecting {
    /// The shell's connection is the new one (the old one's queue is never dispatched again).
    pub(super) fn switched(&self) -> bool {
        self.attempt.is_some()
    }
}

/// The user data of the attempt's `sync` callback.
pub(super) struct Synced(Arc<AtomicBool>);

impl Dispatch<WlCallback, Synced> for MainState {
    fn event(
        _: &mut Self,
        _: &WlCallback,
        event: wl_callback::Event,
        done: &Synced,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_callback::Event::Done { .. } = event {
            done.0.store(true, Ordering::Release);
        }
    }
}

/// How a step of the attempt ended early.
enum Step {
    /// Not there yet: try again at the next step.
    NotYet,
    /// The new display failed: give up.
    Failed,
}

impl MainState {
    /// Each window's outputs, left: the records, and no window is on any output any more.
    fn leave_all_outputs(&mut self) -> Vec<NhShellEvent> {
        let mut out = Vec::new();
        for (&id, s) in &mut self.scales {
            for name in s.entered.drain(..) {
                let mut ev = NhShellEvent::new(crate::event::NHSHELL_EV_LEAVE_OUTPUT, id, 0);
                ev.u.output.id = u64::from(name);
                out.push(ev);
            }
        }
        out
    }

    /// The old connection is gone: its globals are forgotten and its outputs removed (`OUTPUT_REMOVE` each, no
    /// request); the new registry announces its own.  The Xcursor theme was loaded on the old connection.
    fn forget_connection(&mut self) {
        self.globals.list.clear();
        let names: Vec<u32> = self.outputs.keys().copied().collect();
        self.outputs.clear();
        for name in names {
            let mut ev = NhShellEvent::new(crate::event::NHSHELL_EV_OUTPUT_REMOVE, 0, 0);
            ev.u.output.id = u64::from(name);
            self.post(ev);
        }
        self.cursor.forget_connection();
    }
}

impl WaylandShell {
    /// The next record.  When it is `RECONNECTING`, the windows' `LEAVE_OUTPUT` records go before it, and taking it
    /// starts reconnecting.
    pub(super) fn take_record(&mut self) -> ShellResult<Option<NhShellEvent>> {
        let ev = self.shared.take()?;
        if let Some(e) = ev
            && e.kind == NHSHELL_EV_RECONNECTING
            && self.reconnect.is_none()
        {
            let mut leaves = self.main.leave_all_outputs();
            if !leaves.is_empty() {
                leaves.push(e);
                self.shared.push_front(leaves);
                return self.take_record();
            }
            self.loss_taken();
        }
        Ok(ev)
    }

    /// The host took `RECONNECTING`: the presenter thread, which left at the loss, is joined, and its stage keeps
    /// every front for the new connection, whole.
    fn loss_taken(&mut self) {
        let presenter_was_on = self.presenting.thread.is_some();
        self.presenter_stop();
        if let Some(stage) = self.presenting.stage.as_mut() {
            stage.connection_lost();
        }
        self.reconnect = Some(Reconnecting {
            attempt: None,
            presenter_was_on,
        });
    }

    /// A bounded wait gave up because the connection went: `Reconnecting` when it went for a reconnect.
    pub(super) fn waited<T>(&self, r: ShellResult<T>) -> ShellResult<T> {
        match r {
            Err(ShellError::Disconnected) if self.shared.reconnect_pending() => {
                Err(ShellError::Reconnecting)
            }
            r => r,
        }
    }

    /// The main queue may be dispatched: not while it is the old connection's.
    pub(super) fn main_queue_live(&self) -> bool {
        match &self.reconnect {
            Some(r) => r.switched(),
            None => !self.shared.reconnect_pending(),
        }
    }

    /// `ShellBackend::set_reconnect`.
    pub(super) fn set_reconnect_impl(&mut self, on: bool) -> ShellResult {
        if self.shared.lost() {
            return Err(ShellError::Disconnected);
        }
        if on && self.socket_path.is_none() {
            return Err(ShellError::Unsupported);
        }
        self.shared.reconnect.store(on, Ordering::Release);
        Ok(())
    }

    /// `ShellBackend::connection_state`.
    pub(super) fn connection_state_impl(&self) -> ConnectionState {
        if self.shared.lost() {
            ConnectionState::Lost
        } else if self.reconnect.is_some() {
            ConnectionState::Reconnecting
        } else {
            ConnectionState::Up
        }
    }

    /// `ShellBackend::reconnect`: one step, at most `timeout_ms`.
    pub(super) fn reconnect_impl(&mut self, timeout_ms: u32) -> ShellResult {
        if self.shared.is_closed() {
            return Err(ShellError::Disconnected);
        }
        let switched = match &self.reconnect {
            Some(r) => r.switched(),
            None if self.shared.reconnect_pending() => return Err(ShellError::State),
            None => return Ok(()),
        };
        let deadline = Instant::now() + Duration::from_millis(u64::from(timeout_ms));
        let step = if switched {
            Ok(())
        } else {
            self.attempt_start()
        }
        .and_then(|()| self.attempt_sync(deadline));
        match step {
            Ok(()) => self.finish(),
            Err(Step::NotYet) => Err(ShellError::Timeout),
            Err(Step::Failed) => Err(self.give_up()),
        }
    }

    /// The new display failed: the host hears `DISCONNECTED`.
    fn give_up(&mut self) -> ShellError {
        self.shared.close();
        ShellError::Disconnected
    }

    /// Connect again and switch the shell over to the new connection.
    fn attempt_start(&mut self) -> Result<(), Step> {
        let path = self.socket_path.clone().ok_or(Step::Failed)?;
        // the old reader's last act was posting RECONNECTING: it has finished or is about to
        if let Some(t) = self.reader.take() {
            let _ = t.join();
        }
        match conn::connect_again(&path) {
            Ok(stream) => self.switch_to(stream),
            Err(Refused::NotYet) => Err(Step::NotYet),
            Err(Refused::Failed) => Err(Step::Failed),
        }
    }

    /// The new connection becomes the shell's: the old one's queue, wire and connection go (libwayland frees its
    /// proxies without a request when the last clone drops), the records flow again, and the new registry and `sync`
    /// are sent.
    fn switch_to(&mut self, stream: UnixStream) -> Result<(), Step> {
        let conn = Connection::from_socket(stream).map_err(|_| Step::Failed)?;
        // the descriptor the host shuts down when it abandons the shell follows the connection
        if let Some(d) = &mut self.socket_dup {
            let _ = rustix::io::dup3(conn.backend().poll_fd(), d, rustix::io::DupFlags::CLOEXEC);
        }
        let queue = conn.new_event_queue::<MainState>();
        let mqh = queue.handle();
        self.link.clear_wire();
        self.main_queue = queue;
        self.conn = conn;
        self.shared.resume();
        self.main.forget_connection();
        self.main.registry = Some(self.conn.display().get_registry(&mqh, ()));
        let synced = Arc::new(AtomicBool::new(false));
        self.conn.display().sync(&mqh, Synced(synced.clone()));
        if let Some(r) = &mut self.reconnect {
            r.attempt = Some(synced);
        }
        Ok(())
    }

    /// A failure on the attempt's connection: a hangup (the new compositor went too) starts another attempt at the
    /// next step, anything else gives up.
    fn attempt_failed(&mut self) -> Step {
        let hangup = self.conn.protocol_error().is_none()
            && matches!(
                self.conn.backend().last_error(),
                Some(WaylandError::Io(e)) if matches!(e.raw_os_error(), Some(libc::EPIPE | libc::ECONNRESET))
            );
        if !hangup {
            return Step::Failed;
        }
        if let Some(r) = &mut self.reconnect {
            r.attempt = None;
        }
        Step::NotYet
    }

    /// Read and dispatch the attempt's connection until its `sync` is done, at most until `deadline`.  Nothing else
    /// reads it yet (the reader starts on success).
    fn attempt_sync(&mut self, deadline: Instant) -> Result<(), Step> {
        loop {
            if self.dispatch_main_queue().is_err() {
                return Err(self.attempt_failed());
            }
            let synced = self
                .reconnect
                .as_ref()
                .and_then(|r| r.attempt.as_ref())
                .is_some_and(|s| s.load(Ordering::Acquire));
            if synced {
                return if self.main.globals.required() {
                    Ok(())
                } else {
                    Err(Step::Failed)
                };
            }
            match self.conn.flush() {
                Ok(()) => {}
                Err(WaylandError::Io(e)) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(_) => return Err(self.attempt_failed()),
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err(Step::NotYet);
            }
            let Some(guard) = self.conn.prepare_read() else {
                // events wait in libwayland's queue: route them, then dispatch
                if self.conn.backend().dispatch_inner_queue().is_err() {
                    return Err(self.attempt_failed());
                }
                continue;
            };
            let readable = {
                let fd = guard.connection_fd();
                let mut fds = [PollFd::new(&fd, PollFlags::IN)];
                let t = Timespec {
                    tv_sec: left.as_secs() as i64,
                    tv_nsec: i64::from(left.subsec_nanos()),
                };
                matches!(poll(&mut fds, Some(&t)), Ok(n) if n > 0)
            };
            if readable {
                match guard.read() {
                    Ok(_) => {}
                    Err(WaylandError::Io(e)) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                    Err(_) => return Err(self.attempt_failed()),
                }
            } else {
                drop(guard);
            }
        }
    }

    /// The new compositor announced its globals: bind them, bring the input, pools, presenter and windows back, start
    /// the reader and post `RECONNECTED`.
    fn finish(&mut self) -> ShellResult {
        let mqh = self.main_queue.handle();
        let reader_queue = self.conn.new_event_queue::<ReaderState>();
        let rqh = reader_queue.handle();
        let mut rstate = ReaderState::new(self.shared.clone());
        // what the host asked of text input stays; the new object is set on it
        rstate.text_input = self.text_input.clone();
        if self
            .main
            .bind_globals(&mut rstate, &mqh, &rqh, &self.shared)
            .is_err()
        {
            return Err(self.give_up());
        }
        let wire = self.main.wire(&self.conn, &self.shared, &mqh, &rqh);
        self.remake_pools(&wire);
        self.link.set_wire(wire);
        // the old reader has finished; the new one reports through the same `reader_done`
        self.shared.reader_done.reset();
        match reader::spawn(self.conn.clone(), reader_queue, rstate) {
            Ok(t) => self.reader = Some(t),
            Err(_) => return Err(self.give_up()),
        }
        let presenter_was_on = self.reconnect.take().is_some_and(|r| r.presenter_was_on);
        self.remake_windows()?;
        if presenter_was_on {
            let _ = self.set_presenter_thread_impl(true);
        }
        self.shared
            .post(NhShellEvent::new(NHSHELL_EV_RECONNECTED, 0, 0));
        self.shared.wake.signal();
        self.flush()
    }

    /// Every window's buffers get new `wl_buffer`s from their own memfds (what they hold stays) and count as released;
    /// content layers go (presenting promotes them again).
    fn remake_pools(&mut self, wire: &crate::wayland::presenter::Wire) {
        for p in self.main.presents.values() {
            present::lock(p).reconnected(p, wire);
        }
    }

    /// The windows, made again on the new connection before `RECONNECTED` is posted.  Their records, ids, buffers and
    /// fronts are kept; their protocol objects belonged to the old connection.
    /// - Another client's selection that went posts an empty `SELECTION`; what we own is offered again at the first
    ///   serial (`Selections::reconnected`).
    /// - Popups close, topmost first: `POPUP_DONE` each, and their records go.
    /// - An open session is opened again with reason `recover` and its id.
    /// - Top-levels are made again with what they asked for (title, app id, min size, maximized or fullscreen, the
    ///   decoration mode), each restored under its session name before its role-setup commit; then the children,
    ///   parents first, as sync subsurfaces at their offsets and in their stacking order.
    /// - Each window is back at its state at creation (`Present::remade`): nothing commits it before the new
    ///   compositor's configure, which is a content ack.  Window ids do not change (`window_of`).
    /// - Idle inhibitors are held again; export handles and activation tokens are forgotten.
    pub(super) fn remake_windows(&mut self) -> ShellResult {
        for kind in selection::lock(&self.shared).take_lost() {
            let mut ev = NhShellEvent::new(NHSHELL_EV_SELECTION, 0, 0);
            ev.u.selection = SelectionEv { kind };
            self.shared.post(ev);
        }
        self.popups_closed();
        let qh = self.main_queue.handle();
        let names = self.main.session.reconnected(&qh);
        let mut tops: Vec<WindowId> = self.main.windows.keys().copied().collect();
        tops.sort_unstable();
        for id in tops {
            // only a missing core global fails, and `bind_globals` checked those
            let _ = self.toplevel_again(id, names.get(&id).map(String::as_str));
        }
        self.children_again();
        let m = &mut self.main;
        m.idle.again(&m.surfaces, &qh);
        m.foreign.reconnected();
        m.tokens.reconnected();
        self.flush()
    }
}

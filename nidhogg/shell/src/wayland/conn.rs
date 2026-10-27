//! The connection, and the state the reader thread and the host's thread share: the event records, the two wake
//! descriptors and the flags between them.
//!
//! Hand-off rules (the reader thread posts, the host's thread takes; neither may lose a wake-up):
//! - the reader posts records and then signals `wake`; the host drains `wake` only when the records are empty, then
//!   looks again, so a record posted in between is never lost;
//! - a main-queue event makes the main waker set `main_pending` and signal `wake`; the host dispatches the main queue
//!   when it finds the flag;
//! - a host flush that returns EAGAIN sets `want_flush` and signals `reader_wake`; the reader then polls for POLLOUT
//!   and flushes;
//! - when the connection fails the reader posts one `DISCONNECTED` record and closes the records.

use crate::api::{ShellError, ShellResult};
use crate::event::{NHSHELL_EV_DISCONNECTED, NHSHELL_EV_KEY, NhShellEvent};
use crate::wayland::serial::SerialTracker;
use rustix::event::{EventfdFlags, eventfd};
use rustix::fd::{AsFd, AsRawFd, BorrowedFd, OwnedFd};
use std::collections::VecDeque;
use std::ffi::{CStr, OsStr};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, MutexGuard};
use wayland_client::Connection;
use wayland_client::backend::WaylandError;
use wayland_protocols::wp::cursor_shape::v1::client::wp_cursor_shape_device_v1::WpCursorShapeDeviceV1;

/// Connect to `display`: a socket name in `$XDG_RUNTIME_DIR` or an absolute path; `None` (or empty) uses the
/// environment (`WAYLAND_SOCKET`, `WAYLAND_DISPLAY`).
pub fn connect(display: Option<&CStr>) -> ShellResult<Connection> {
    let name = display.map(|d| Path::new(OsStr::from_bytes(d.to_bytes())));
    let Some(name) = name.filter(|n| !n.as_os_str().is_empty()) else {
        return Connection::connect_to_env().map_err(|_| ShellError::Unsupported);
    };
    let path = if name.is_absolute() {
        name.to_path_buf()
    } else {
        PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR").ok_or(ShellError::Unsupported)?)
            .join(name)
    };
    let stream = UnixStream::connect(path).map_err(|_| ShellError::Unsupported)?;
    Connection::from_socket(stream).map_err(|_| ShellError::Unsupported)
}

/// An eventfd (`EFD_NONBLOCK | EFD_CLOEXEC`) used as a level: `signal` makes it readable, `drain` reads it to zero.
pub struct Wake(OwnedFd);

impl Wake {
    pub fn new() -> ShellResult<Wake> {
        eventfd(0, EventfdFlags::NONBLOCK | EventfdFlags::CLOEXEC)
            .map(Wake)
            .map_err(|_| ShellError::NoMem)
    }

    pub fn signal(&self) {
        // Fails only when the counter would overflow, and then the descriptor is readable already.
        let _ = rustix::io::write(&self.0, &1u64.to_ne_bytes());
    }

    pub fn drain(&self) {
        let mut buf = [0u8; 8];
        let _ = rustix::io::read(&self.0, &mut buf);
    }

    pub fn raw(&self) -> i32 {
        self.0.as_raw_fd()
    }

    pub fn fd(&self) -> BorrowedFd<'_> {
        self.0.as_fd()
    }
}

#[derive(Default)]
struct Records {
    queue: VecDeque<NhShellEvent>,
    /// `DISCONNECTED` was posted: nothing more is queued, and once it is taken the records report the loss.
    closed: bool,
}

pub struct Shared {
    records: Mutex<Records>,
    /// The host's wake descriptor (a keyboard wait descriptor in Emacs).
    pub wake: Wake,
    /// Wakes the reader from its poll: a flush to retry, or `stop`.
    pub reader_wake: Wake,
    /// A flush returned EAGAIN; the reader flushes again once the socket is writable.
    pub want_flush: AtomicBool,
    /// The main queue has events for the host's thread to dispatch.
    pub main_pending: AtomicBool,
    /// The shell is closing; the reader leaves its loop without posting `DISCONNECTED`.
    pub stop: AtomicBool,
    /// Input serials, written by the reader's handlers and read by the host's requests (move, resize, menus).
    pub serials: Mutex<SerialTracker>,
    /// The scale (times 120) of each surface, mirrored from the main queue for the pointer's device pixels.
    pub scale120: Mutex<std::collections::HashMap<u64, u32>>,
    /// The seat pointer's `wp_cursor_shape_device_v1`, when the compositor has the manager.
    pub cursor_device: Mutex<Option<WpCursorShapeDeviceV1>>,
}

fn relock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    // A panic elsewhere never leaves these records half-written, so a poisoned lock is still usable.
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl Shared {
    pub fn new() -> ShellResult<Shared> {
        Ok(Shared {
            records: Mutex::default(),
            wake: Wake::new()?,
            reader_wake: Wake::new()?,
            want_flush: AtomicBool::new(false),
            main_pending: AtomicBool::new(false),
            stop: AtomicBool::new(false),
            serials: Mutex::default(),
            scale120: Mutex::default(),
            cursor_device: Mutex::default(),
        })
    }

    /// Queue a record; the poster signals `wake` once per batch.
    pub fn post(&self, ev: NhShellEvent) {
        let mut r = relock(&self.records);
        if !r.closed {
            r.queue.push_back(ev);
        }
    }

    /// Post the one `DISCONNECTED` record and wake the host.
    pub fn close(&self) {
        {
            let mut r = relock(&self.records);
            if r.closed {
                return;
            }
            r.queue
                .push_back(NhShellEvent::new(NHSHELL_EV_DISCONNECTED, 0, 0));
            r.closed = true;
        }
        self.wake.signal();
    }

    /// The next record; `Disconnected` once `DISCONNECTED` has been taken.
    pub fn take(&self) -> ShellResult<Option<NhShellEvent>> {
        let mut r = relock(&self.records);
        match r.queue.pop_front() {
            Some(ev) => Ok(Some(ev)),
            None if r.closed => Err(ShellError::Disconnected),
            None => Ok(None),
        }
    }

    /// A repeat of `keycode` is queued and not yet taken (the reader's repeat coalescing).
    pub fn has_unread_repeat(&self, keycode: u32) -> bool {
        relock(&self.records).queue.iter().any(|e| {
            // SAFETY: a KEY record's valid member is `key`.
            e.kind == NHSHELL_EV_KEY && unsafe { e.u.key.repeat == 1 && e.u.key.keycode == keycode }
        })
    }

    pub fn is_closed(&self) -> bool {
        relock(&self.records).closed
    }

    /// The host thread's flush.  A full socket is left to the reader (POLLOUT); any other failure wakes the reader,
    /// whose next dispatch finds the connection's error and posts `DISCONNECTED`.
    pub fn host_flush(&self, conn: &Connection) -> ShellResult {
        match conn.flush() {
            Ok(()) => Ok(()),
            Err(WaylandError::Io(e)) if e.kind() == std::io::ErrorKind::WouldBlock => {
                self.want_flush.store(true, Ordering::Release);
                self.reader_wake.signal();
                Ok(())
            }
            Err(_) => {
                self.reader_wake.signal();
                Err(ShellError::Disconnected)
            }
        }
    }
}

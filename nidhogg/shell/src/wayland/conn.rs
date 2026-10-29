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
//! - when the connection fails the reader posts one `DISCONNECTED` record and closes the records;
//! - when it hangs up with reconnecting on, the reader posts `RECONNECTING` instead and holds the records: nothing
//!   from the old connection is queued after it, until a reconnect resumes them (`reconnect.rs`).

use crate::api::{ShellError, ShellResult, ThreadDone};
use crate::event::{Record, ShellEvent};
use crate::wayland::selection::Selections;
use crate::wayland::serial::SerialTracker;
use rustix::event::{EventfdFlags, eventfd};
use rustix::fd::{AsFd, AsRawFd, BorrowedFd, OwnedFd};
use std::collections::VecDeque;
use std::ffi::{CStr, OsStr, c_char, c_int, c_void};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, MutexGuard};
use wayland_client::Connection;
use wayland_client::backend::WaylandError;
use wayland_client::protocol::wl_pointer::WlPointer;
use wayland_protocols::wp::cursor_shape::v1::client::wp_cursor_shape_device_v1::WpCursorShapeDeviceV1;

/// Connect to `display`: a socket name in `$XDG_RUNTIME_DIR` or an absolute path; `None` (or empty) uses the
/// environment (`WAYLAND_SOCKET`, `WAYLAND_DISPLAY`).
pub fn connect(display: Option<&CStr>) -> ShellResult<Connection> {
    install_log_handler();
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

/// The absolute path of the socket `connect(display)` reaches, for connecting again; `None` for an inherited socket
/// (`WAYLAND_SOCKET`), which cannot be.
pub fn socket_path(display: Option<&CStr>) -> Option<PathBuf> {
    let given = display
        .map(|d| OsStr::from_bytes(d.to_bytes()).to_os_string())
        .filter(|n| !n.is_empty());
    let name = match given {
        Some(n) => n,
        None if std::env::var_os("WAYLAND_SOCKET").is_some() => return None,
        None => std::env::var_os("WAYLAND_DISPLAY")
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| "wayland-0".into()),
    };
    let name = Path::new(&name);
    if name.is_absolute() {
        Some(name.to_path_buf())
    } else {
        Some(PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR")?).join(name))
    }
}

/// Why connecting again did not work.
pub enum Refused {
    /// The display is not there yet (no socket, refused, a full backlog): try again later.
    NotYet,
    /// Anything else: give up.
    Failed,
}

/// Connect to `path` without waiting: a compositor that keeps its socket (`kwin_wayland_wrapper`) has the connection
/// wait in the backlog, and one that binds the name again refuses or has no socket until it does.
pub fn connect_again(path: &Path) -> Result<UnixStream, Refused> {
    use rustix::io::Errno;
    use rustix::net::{
        AddressFamily, SocketAddrUnix, SocketFlags, SocketType, connect, socket_with,
    };
    let not_yet = |e: Errno| match e {
        Errno::NOENT | Errno::CONNREFUSED | Errno::AGAIN => Refused::NotYet,
        _ => Refused::Failed,
    };
    let fd = socket_with(
        AddressFamily::UNIX,
        SocketType::STREAM,
        SocketFlags::CLOEXEC | SocketFlags::NONBLOCK,
        None,
    )
    .map_err(|_| Refused::Failed)?;
    let addr = SocketAddrUnix::new(path).map_err(|_| Refused::Failed)?;
    connect(&fd, &addr).map_err(not_yet)?;
    let stream = UnixStream::from(fd);
    // as `connect` makes it: libwayland passes MSG_DONTWAIT itself
    stream.set_nonblocking(false).map_err(|_| Refused::Failed)?;
    Ok(stream)
}

/// A protocol error as libwayland logged it.
#[derive(Debug, PartialEq, Eq)]
struct Logged {
    interface: String,
    id: u32,
    code: u32,
    message: String,
}

/// The protocol errors libwayland logged in this process, newest last.  libwayland hands an error's message only to
/// its log handler (the backend's error has none), so the shell installs one.
static LOGGED: Mutex<VecDeque<Logged>> = Mutex::new(VecDeque::new());
const LOGGED_KEPT: usize = 8;

unsafe extern "C" {
    /// libwayland-client's; the handler's second argument is a `va_list`.
    fn wl_log_set_handler_client(handler: unsafe extern "C" fn(*const c_char, *mut c_void));
    fn vsnprintf(buf: *mut c_char, cap: usize, fmt: *const c_char, args: *mut c_void) -> c_int;
}

fn install_log_handler() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    // SAFETY: libwayland's process-wide handler becomes a function that lives as long as the process.
    ONCE.call_once(|| unsafe { wl_log_set_handler_client(log_line) });
}

/// The log handler: the line goes to stderr, as with libwayland's own handler, and a protocol error's is kept.  The
/// `va_list` is passed on to `vsnprintf` as the word it arrived in: every Linux ABI passes a `va_list` argument as one
/// word (a pointer, or the one-pointer struct itself on 32-bit ARM).
unsafe extern "C" fn log_line(fmt: *const c_char, args: *mut c_void) {
    let mut buf = [0u8; 1024];
    // SAFETY: libwayland's format and its arguments, consumed once; at most `buf.len()` bytes are written.
    let n = unsafe { vsnprintf(buf.as_mut_ptr().cast(), buf.len(), fmt, args) };
    let Ok(n) = usize::try_from(n) else {
        return;
    };
    let line = &buf[..n.min(buf.len() - 1)];
    // never unwind into libwayland
    let _ = std::panic::catch_unwind(|| {
        let _ = std::io::Write::write_all(&mut std::io::stderr(), line);
        if let Some(e) = parse_logged(&String::from_utf8_lossy(line)) {
            let mut kept = relock(&LOGGED);
            if kept.len() == LOGGED_KEPT {
                kept.pop_front();
            }
            kept.push_back(e);
        }
    });
}

/// `INTERFACE#ID: error CODE: MESSAGE` (`@` before libwayland 1.20), newline-terminated.
fn parse_logged(line: &str) -> Option<Logged> {
    let (object, rest) = line.trim_end_matches('\n').split_once(": error ")?;
    let (code, message) = rest.split_once(": ")?;
    let at = object.rfind(['#', '@'])?;
    Some(Logged {
        interface: object[..at].to_string(),
        id: object[at + 1..].parse().ok()?,
        code: code.parse().ok()?,
        message: message.to_string(),
    })
}

/// The protocol error that ended `conn`, as `INTERFACE@ID: error CODE: MESSAGE` with the message libwayland logged for
/// it; empty while the connection lives or after a loss without one (a hangup).
pub fn error_text(conn: &Connection) -> String {
    let Some(e) = conn.protocol_error() else {
        return String::new();
    };
    let message = relock(&LOGGED)
        .iter()
        .rev()
        .find(|l| l.interface == e.object_interface && l.id == e.object_id && l.code == e.code)
        .map_or(e.message.clone(), |l| l.message.clone());
    format!(
        "{}@{}: error {}: {}",
        e.object_interface, e.object_id, e.code, message
    )
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
    queue: VecDeque<Record>,
    /// `DISCONNECTED` was posted: nothing more is queued, and once it is taken the records report the loss.
    closed: bool,
    /// `RECONNECTING` was posted: nothing more is queued until the reconnect resumes the records.
    held: bool,
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
    /// Readable from the moment the connection is lost, until a reconnect drains it: the host's bounded waits (a
    /// selection read) and the presenter thread poll it and give up at once.
    pub gone: Wake,
    /// A hangup posts `RECONNECTING` instead of `DISCONNECTED` (`set_reconnect`).
    pub reconnect: AtomicBool,
    /// Input serials, written by the reader's handlers and read by the host's requests (move, resize, menus).
    pub serials: Mutex<SerialTracker>,
    /// The scale (times 120) of each surface, mirrored from the main queue for the pointer's device pixels.
    pub scale120: Mutex<std::collections::HashMap<u64, u32>>,
    /// The seat pointer's `wp_cursor_shape_device_v1`, when the compositor has the manager.
    pub cursor_device: Mutex<Option<WpCursorShapeDeviceV1>>,
    /// The seat's `wl_pointer`, for the cursor requests the host thread sends (hidden, theme).
    pub pointer: Mutex<Option<WlPointer>>,
    /// The clipboard and the primary selection (`selection.rs`): offers, our sources and the focus they need.
    pub selections: Mutex<Selections>,
    /// A drag is in progress on the seat (another client's over our surfaces, or our own source's): the compositor
    /// holds the pointer, so a popup takes no grab.
    pub dragging: AtomicBool,
    /// The reader thread has finished (its last act).
    pub reader_done: ThreadDone,
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
            gone: Wake::new()?,
            reconnect: AtomicBool::new(false),
            serials: Mutex::default(),
            scale120: Mutex::default(),
            cursor_device: Mutex::default(),
            pointer: Mutex::default(),
            selections: Mutex::default(),
            dragging: AtomicBool::new(false),
            reader_done: ThreadDone::default(),
        })
    }

    /// Queue a record; the poster signals `wake` once per batch.
    pub fn post(&self, ev: Record) {
        let mut r = relock(&self.records);
        if !r.closed && !r.held {
            r.queue.push_back(ev);
        }
    }

    /// The connection hung up and the shell reconnects (the reader has posted what the loss ends: held buttons, touch
    /// points, a preedit): queue the `DND_LEAVE` of a drag over us that has not dropped, our drag's `DRAG_DONE`
    /// (cancelled) as `close` does, and `RECONNECTING`; hold the
    /// records, and wake the host and every bounded wait.
    pub fn reconnecting(&self) {
        // the selections' lock is never taken while the records' is held
        let drop_leave = crate::wayland::dnd::drop_lost(self);
        let drag_done = crate::wayland::dnd::drag_lost(self);
        {
            let mut r = relock(&self.records);
            if r.closed || r.held {
                return;
            }
            r.queue.extend(drop_leave);
            r.queue.extend(drag_done);
            r.queue
                .push_back(Record::new(ShellEvent::Reconnecting, 0, 0));
            r.held = true;
        }
        self.gone.signal();
        self.wake.signal();
    }

    /// `RECONNECTING` was posted and no reconnect has resumed the records since.
    pub fn reconnect_pending(&self) -> bool {
        relock(&self.records).held
    }

    /// Put `evs` before every queued record (the host's thread adds what the loss ends on its side).
    pub fn push_front(&self, evs: Vec<Record>) {
        let mut r = relock(&self.records);
        for ev in evs.into_iter().rev() {
            r.queue.push_front(ev);
        }
    }

    /// A new connection is in place: the records flow again, and the state of the old connection is forgotten (the
    /// loss signal, flushes, input serials, the pointer's objects, other clients' offers and our sources).
    pub fn resume(&self) {
        relock(&self.records).held = false;
        self.gone.drain();
        self.want_flush.store(false, Ordering::Release);
        // the held presses go with the serials
        *relock(&self.serials) = SerialTracker::default();
        *relock(&self.cursor_device) = None;
        *relock(&self.pointer) = None;
        relock(&self.selections).reconnected();
        self.dragging.store(false, Ordering::Release);
    }

    /// Post the one `DISCONNECTED` record and wake the host.  Our drag, if one runs, ends first: its `DRAG_DONE`
    /// (cancelled) comes right before.
    pub fn close(&self) {
        if relock(&self.records).closed {
            return;
        }
        // the selections' lock is never taken while the records' is held
        let drag_done = crate::wayland::dnd::drag_lost(self);
        {
            let mut r = relock(&self.records);
            if r.closed {
                return;
            }
            r.queue.extend(drag_done);
            r.queue
                .push_back(Record::new(ShellEvent::Disconnected, 0, 0));
            r.closed = true;
        }
        self.gone.signal();
        self.wake.signal();
    }

    /// The next record; `Disconnected` once `DISCONNECTED` has been taken.
    pub fn take(&self) -> ShellResult<Option<Record>> {
        let mut r = relock(&self.records);
        match r.queue.pop_front() {
            Some(ev) => Ok(Some(ev)),
            None if r.closed => Err(ShellError::Disconnected),
            None => Ok(None),
        }
    }

    /// A repeat of `keycode` is queued and not yet taken (the reader's repeat coalescing).
    pub fn has_unread_repeat(&self, keycode: u32) -> bool {
        relock(&self.records)
            .queue
            .iter()
            .any(|e| matches!(e.event, ShellEvent::Key(k) if k.repeat == 1 && k.keycode == keycode))
    }

    pub fn is_closed(&self) -> bool {
        relock(&self.records).closed
    }

    /// The host has taken `DISCONNECTED`: from now on every call reports the loss.  (Records queued before it are
    /// still taken and acted on, a committed text among them.)
    pub fn lost(&self) -> bool {
        let r = relock(&self.records);
        r.closed && r.queue.is_empty()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn logged_error_lines() {
        let want = Logged {
            interface: "xdg_wm_base".into(),
            id: 9,
            code: 4,
            message: "wrong configure serial: 7".into(),
        };
        let line = "xdg_wm_base#9: error 4: wrong configure serial: 7\n";
        assert_eq!(parse_logged(line), Some(want));
        let old = parse_logged("wl_shm@4: error 1: invalid wl_shm_pool size\n").unwrap();
        assert_eq!((old.interface.as_str(), old.id, old.code), ("wl_shm", 4, 1));
        assert_eq!(parse_logged("[destroyed object]: error 2: x\n"), None);
        assert_eq!(
            parse_logged("error in client communication (pid 1)\n"),
            None
        );
    }
}

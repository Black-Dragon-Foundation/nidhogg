// SPDX-License-Identifier: GPL-3.0-or-later AND Apache-2.0
// Copyright 2022-2026 Zed Industries, Inc.
// Adapted-from: https://github.com/zed-industries/zed@347f171a3fd00a3b672382d78348b37bd969d17c crates/gpui_linux/src/linux/wayland/clipboard.rs
// Modified for Nidhogg: only the descriptor handling is kept.  `ReceiveData` and the MIME list per offer are Zed's;
// `read_bytes` became `receive_pipe` + `read_with_timeout` (rustix's pipe and poll, a caller's timeout, errors as
// `ShellError`); `send_bytes` became `SendBytes`, written from the reader thread's poll loop instead of a calloop
// source, with the descriptor made non-blocking and a 5 s limit.  Zed's item types, image formats, text
// normalisation and self-MIME check are not taken (the glue converts; ownership is tracked by our source).

//! Selection bytes over the descriptors of `receive` (we paste) and `send` (another client pastes ours).

use crate::api::{ShellError, ShellResult};
use rustix::event::{PollFd, PollFlags, Timespec, poll};
use rustix::fd::{AsFd, BorrowedFd, OwnedFd};
use rustix::fs::{OFlags, fcntl_getfl, fcntl_setfl};
use rustix::io::Errno;
use rustix::pipe::{PipeFlags, pipe_with};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};
use wayland_client::protocol::wl_data_offer::WlDataOffer;
use wayland_protocols::wp::primary_selection::zv1::client::zwp_primary_selection_offer_v1::ZwpPrimarySelectionOfferV1;

/// How long another client may take to read what we own before the reader closes the descriptor.
pub const SEND_TIMEOUT: Duration = Duration::from_secs(5);

pub trait ReceiveData {
    fn receive_data(&self, mime_type: String, fd: BorrowedFd<'_>);
}

impl ReceiveData for WlDataOffer {
    fn receive_data(&self, mime_type: String, fd: BorrowedFd<'_>) {
        self.receive(mime_type, fd);
    }
}

impl ReceiveData for ZwpPrimarySelectionOfferV1 {
    fn receive_data(&self, mime_type: String, fd: BorrowedFd<'_>) {
        self.receive(mime_type, fd);
    }
}

/// The MIME types an offer announced: its user data, filled by its `offer` events (which come before the device's
/// `selection` event names the offer).
#[derive(Default)]
pub struct OfferTypes(Mutex<Vec<String>>);

impl OfferTypes {
    fn lock(&self) -> MutexGuard<'_, Vec<String>> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn add(&self, mime_type: String) {
        self.lock().push(mime_type);
    }

    pub fn list(&self) -> Vec<String> {
        self.lock().clone()
    }
}

/// Ask `offer` to write `mime_type` into a new pipe; the read end comes back.  The write end is closed here
/// (libwayland duplicated it when the request was queued), so the read sees end of file when the source is done.
pub fn receive_pipe<T: ReceiveData>(offer: &T, mime_type: &str) -> ShellResult<OwnedFd> {
    let Ok((read, write)) = pipe_with(PipeFlags::CLOEXEC) else {
        return Err(ShellError::NoMem);
    };
    offer.receive_data(mime_type.to_string(), write.as_fd());
    drop(write);
    Ok(read)
}

/// Read `fd` to end of file, giving up with `Timeout` once `timeout` has passed.  Signals do not shorten the wait.
pub fn read_with_timeout(fd: OwnedFd, timeout: Duration) -> ShellResult<Vec<u8>> {
    let deadline = Instant::now() + timeout;
    let mut out = Vec::new();
    let mut chunk = vec![0u8; 64 * 1024];
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(ShellError::Timeout);
        }
        let t = timespec(left);
        let mut fds = [PollFd::new(&fd, PollFlags::IN)];
        match poll(&mut fds, Some(&t)) {
            Ok(_) | Err(Errno::INTR) => {}
            Err(_) => return Err(ShellError::State),
        }
        if fds[0].revents().is_empty() {
            continue;
        }
        match rustix::io::read(&fd, &mut chunk) {
            Ok(0) => return Ok(out),
            Ok(n) => out.extend_from_slice(&chunk[..n]),
            Err(Errno::INTR | Errno::AGAIN) => {}
            Err(_) => return Err(ShellError::State),
        }
    }
}

/// A `Timespec` for poll, rounded up to the next millisecond so a wait never ends just short of a deadline.
pub fn timespec(d: Duration) -> Timespec {
    let d = d + Duration::from_nanos(999_999);
    Timespec {
        tv_sec: d.as_secs() as i64,
        tv_nsec: i64::from(d.subsec_millis()) * 1_000_000,
    }
}

/// One `send` being answered: our bytes written to another client's descriptor as fast as it reads them, never
/// blocking the reader thread.
pub struct SendBytes {
    fd: OwnedFd,
    bytes: Arc<[u8]>,
    written: usize,
    pub deadline: Instant,
}

impl SendBytes {
    /// Start answering a `send`; `None` when it is finished already (or the descriptor cannot be made
    /// non-blocking, which closes it: a blocking write could stall the reader).
    pub fn start(fd: OwnedFd, bytes: Arc<[u8]>) -> Option<SendBytes> {
        let flags = fcntl_getfl(&fd).ok()?;
        fcntl_setfl(&fd, flags | OFlags::NONBLOCK).ok()?;
        let mut send = SendBytes {
            fd,
            bytes,
            written: 0,
            deadline: Instant::now() + SEND_TIMEOUT,
        };
        (!send.write_some()).then_some(send)
    }

    /// Write what the descriptor takes now; true when finished (everything written, or the other end is gone).
    pub fn write_some(&mut self) -> bool {
        while self.written < self.bytes.len() {
            match rustix::io::write(&self.fd, &self.bytes[self.written..]) {
                Ok(n) => self.written += n,
                Err(Errno::AGAIN) => return false,
                Err(Errno::INTR) => {}
                Err(_) => return true,
            }
        }
        true
    }
}

impl AsFd for SendBytes {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.fd.as_fd()
    }
}

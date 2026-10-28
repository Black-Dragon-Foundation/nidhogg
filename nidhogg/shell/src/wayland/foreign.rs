//! xdg-foreign: export a top-level window and hand out its handle (`zxdg_exported_v2`), cached per window until the
//! window goes.  Also the bounded wait the handle and the activation token share.

use crate::api::*;
use crate::wayland::{MainState, WaylandShell};
use rustix::event::{PollFd, PollFlags, Timespec, poll};
use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};
use wayland_client::{Connection, Dispatch, QueueHandle, delegate_noop};
use wayland_protocols::xdg::foreign::zv2::client::zxdg_exported_v2::{self, ZxdgExportedV2};
use wayland_protocols::xdg::foreign::zv2::client::zxdg_exporter_v2::ZxdgExporterV2;

delegate_noop!(MainState: ZxdgExporterV2);

/// One window's export: the object, and the handle once the compositor has sent it.
struct Export {
    object: ZxdgExportedV2,
    handle: Option<String>,
}

#[derive(Default)]
pub struct Foreign {
    pub exporter: Option<ZxdgExporterV2>,
    exports: HashMap<u64, Export>,
}

impl Foreign {
    pub fn forget(&mut self, id: u64) {
        if let Some(e) = self.exports.remove(&id) {
            e.object.destroy();
        }
    }

    /// A reconnect: the handles were the old compositor's; the next request exports the window again.
    pub fn reconnected(&mut self) {
        self.exports.clear();
    }
}

impl WaylandShell {
    /// What window `id` owns beside its surface (idle inhibitor, export, session entry) goes before the surface.
    pub(super) fn extras_forget(&mut self, id: u64) {
        self.main.idle.forget(id);
        self.main.foreign.forget(id);
        self.main.session.forget(id);
    }

    /// Run the main queue until `done` holds, `timeout_ms` passes (`Timeout`) or the display goes.  The reader's
    /// wake descriptor says when the main queue has something; queued input records are left alone.
    pub(super) fn wait_main(
        &mut self,
        timeout_ms: u32,
        mut done: impl FnMut(&MainState) -> bool,
    ) -> ShellResult {
        let deadline = Instant::now() + Duration::from_millis(timeout_ms.into());
        self.flush()?;
        loop {
            if self.shared.is_closed() {
                return Err(ShellError::Disconnected);
            }
            self.shared.main_pending.store(false, Ordering::Release);
            self.dispatch_main_queue()?;
            if done(&self.main) {
                return Ok(());
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err(ShellError::Timeout);
            }
            if self.shared.main_pending.load(Ordering::Acquire) {
                continue;
            }
            let slice = left.min(Duration::from_millis(5));
            let t = Timespec {
                tv_sec: 0,
                tv_nsec: i64::from(slice.subsec_nanos()),
            };
            let wake = self.shared.wake.fd();
            let mut fds = [PollFd::new(&wake, PollFlags::IN)];
            let _ = poll(&mut fds, Some(&t));
            // readable for queued records, not for us: do not spin
            if !self.shared.main_pending.load(Ordering::Acquire) {
                std::thread::sleep(Duration::from_millis(1));
            }
        }
    }

    /// The handle of window `id`'s export (made at the first call), within `timeout_ms`.
    pub(super) fn export_impl(&mut self, id: WindowId, timeout_ms: u32) -> ShellResult<String> {
        let qh = self.main_queue.handle();
        let m = &mut self.main;
        let exporter = m.foreign.exporter.as_ref().ok_or(ShellError::Unsupported)?;
        if !m.windows.contains_key(&id) {
            return Err(ShellError::State);
        }
        if !m.foreign.exports.contains_key(&id) {
            let surface = m.surfaces.get(&id).ok_or(ShellError::State)?;
            let object = exporter.export_toplevel(surface, &qh, id);
            m.foreign.exports.insert(
                id,
                Export {
                    object,
                    handle: None,
                },
            );
        }
        let have = |m: &MainState| {
            m.foreign
                .exports
                .get(&id)
                .is_some_and(|e| e.handle.is_some())
        };
        self.wait_main(timeout_ms, have)?;
        self.main
            .foreign
            .exports
            .get(&id)
            .and_then(|e| e.handle.clone())
            .ok_or(ShellError::State)
    }
}

impl Dispatch<ZxdgExportedV2, u64> for MainState {
    fn event(
        state: &mut Self,
        _: &ZxdgExportedV2,
        event: zxdg_exported_v2::Event,
        id: &u64,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let zxdg_exported_v2::Event::Handle { handle } = event
            && let Some(e) = state.foreign.exports.get_mut(id)
        {
            e.handle = Some(handle);
        }
    }
}

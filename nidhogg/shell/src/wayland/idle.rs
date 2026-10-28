//! Idle inhibition (`zwp_idle_inhibit_manager_v1`): one inhibitor on a window's surface while the host asks for it.
//! The compositor applies it only while the surface is visible.

use crate::api::*;
use crate::wayland::{MainState, WaylandShell};
use std::collections::HashMap;
use wayland_client::protocol::wl_surface::WlSurface;
use wayland_client::{QueueHandle, delegate_noop};
use wayland_protocols::wp::idle_inhibit::zv1::client::zwp_idle_inhibit_manager_v1::ZwpIdleInhibitManagerV1;
use wayland_protocols::wp::idle_inhibit::zv1::client::zwp_idle_inhibitor_v1::ZwpIdleInhibitorV1;

delegate_noop!(MainState: ZwpIdleInhibitManagerV1);
delegate_noop!(MainState: ZwpIdleInhibitorV1);

#[derive(Default)]
pub struct Idle {
    pub manager: Option<ZwpIdleInhibitManagerV1>,
    inhibitors: HashMap<u64, ZwpIdleInhibitorV1>,
}

impl Idle {
    /// Window `id` is going: its inhibitor goes before its surface.
    pub fn forget(&mut self, id: u64) {
        if let Some(i) = self.inhibitors.remove(&id) {
            i.destroy();
        }
    }

    /// The windows were made again on a new connection (a reconnect): each window that held an inhibitor holds one
    /// again on its new surface, when the compositor still has the protocol.
    pub fn again(&mut self, surfaces: &HashMap<u64, WlSurface>, qh: &QueueHandle<MainState>) {
        let held: Vec<u64> = self.inhibitors.drain().map(|(id, _)| id).collect();
        let Some(manager) = &self.manager else { return };
        for id in held {
            if let Some(surface) = surfaces.get(&id) {
                self.inhibitors
                    .insert(id, manager.create_inhibitor(surface, qh, ()));
            }
        }
    }
}

impl WaylandShell {
    /// Hold (`on`) or release the idle inhibitor on window `id`'s surface.
    pub(super) fn inhibit_idle_impl(&mut self, id: WindowId, on: bool) -> ShellResult {
        let qh = self.main_queue.handle();
        let m = &mut self.main;
        let manager = m.idle.manager.as_ref().ok_or(ShellError::Unsupported)?;
        let surface = m.surfaces.get(&id).ok_or(ShellError::State)?;
        if on {
            m.idle
                .inhibitors
                .entry(id)
                .or_insert_with(|| manager.create_inhibitor(surface, &qh, ()));
        } else {
            m.idle.forget(id);
        }
        self.flush()
    }
}

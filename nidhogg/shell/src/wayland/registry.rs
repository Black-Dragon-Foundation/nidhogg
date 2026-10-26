//! The registry: the globals the compositor offers, the versions this crate binds, and the capabilities record.
//!
//! The registry lives on the main queue (globals that come and go later, such as outputs, are the host thread's
//! business).  Every capability is `min(advertised, SUPPORTED)`; the required globals are checked in `required`.

use crate::caps::NhShellCaps;
use crate::wayland::MainState;
use wayland_client::protocol::wl_registry::{self, WlRegistry};
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle};

/// The highest version of each global this crate's code is written against.  A newer version can change what an
/// event means (wl_seat 10's repeated key state, wl_compositor 7), so these are raised by the task that handles it,
/// not by a protocol update.
pub const SUPPORTED: [(&str, u32); 26] = [
    ("wl_compositor", 6),
    ("wl_subcompositor", 1),
    ("wl_shm", 2),
    ("wl_seat", 9),
    ("wl_output", 4),
    ("xdg_wm_base", 7),
    ("zxdg_decoration_manager_v1", 1),
    ("wp_fractional_scale_manager_v1", 1),
    ("wp_viewporter", 1),
    ("wp_cursor_shape_manager_v1", 2),
    ("zwp_text_input_manager_v3", 1),
    ("zwp_primary_selection_device_manager_v1", 1),
    ("wl_data_device_manager", 3),
    ("wp_presentation", 2),
    ("wp_fifo_manager_v1", 1),
    ("wp_commit_timing_manager_v1", 1),
    ("wp_alpha_modifier_v1", 1),
    ("xdg_activation_v1", 1),
    ("wp_pointer_warp_v1", 1),
    ("xdg_system_bell_v1", 1),
    ("xdg_session_manager_v1", 1),
    ("ext_background_effect_manager_v1", 1),
    ("zwp_pointer_gestures_v1", 3),
    ("zwp_idle_inhibit_manager_v1", 1),
    ("wp_single_pixel_buffer_manager_v1", 1),
    ("zwp_linux_dmabuf_v1", 4),
];

pub fn supported(interface: &str) -> Option<u32> {
    SUPPORTED
        .iter()
        .find(|(n, _)| *n == interface)
        .map(|&(_, v)| v)
}

#[derive(Clone, Debug)]
pub struct Global {
    pub name: u32,
    pub interface: String,
    pub version: u32,
}

#[derive(Default)]
pub struct Globals {
    pub list: Vec<Global>,
}

impl Globals {
    /// The first global of `interface`.
    pub fn find(&self, interface: &str) -> Option<&Global> {
        self.list.iter().find(|g| g.interface == interface)
    }

    /// The version to bind `interface` at: `min(advertised, SUPPORTED)`, or `None` if it is absent or unknown.
    pub fn version(&self, interface: &str) -> Option<u32> {
        let g = self.find(interface)?;
        Some(g.version.min(supported(interface)?))
    }

    /// Bind the first global of `I` on the queue of `qh` (which decides the thread that runs its events).
    pub fn bind<I, U, D>(&self, registry: &WlRegistry, qh: &QueueHandle<D>, udata: U) -> Option<I>
    where
        I: Proxy + 'static,
        U: Send + Sync + 'static,
        D: Dispatch<I, U> + 'static,
    {
        let interface = I::interface().name;
        let g = self.find(interface)?;
        let v = self.version(interface)?;
        Some(registry.bind::<I, U, D>(g.name, v, qh, udata))
    }

    pub fn caps(&self) -> NhShellCaps {
        let mut caps = NhShellCaps::default();
        for g in &self.list {
            if let (Some(v), Some(f)) = (self.version(&g.interface), caps.field_mut(&g.interface))
                && *f == 0
            {
                *f = v;
            }
        }
        caps
    }

    /// The globals nothing works without: `wl_compositor` >= 4, `wl_shm`, `wl_subcompositor`, `xdg_wm_base` >= 2 and
    /// `wl_seat`.
    pub fn required(&self) -> bool {
        let caps = self.caps();
        caps.compositor >= 4
            && self.find("wl_shm").is_some()
            && caps.subcompositor >= 1
            && caps.xdg_wm_base >= 2
            && caps.seat >= 1
    }
}

impl Dispatch<WlRegistry, ()> for MainState {
    fn event(
        state: &mut Self,
        _: &WlRegistry,
        event: wl_registry::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            wl_registry::Event::Global {
                name,
                interface,
                version,
            } => state.globals.list.push(Global {
                name,
                interface,
                version,
            }),
            wl_registry::Event::GlobalRemove { name } => {
                state.globals.list.retain(|g| g.name != name)
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::caps::{GLOBAL_FIELDS, GLOBALS_WITHOUT_FIELD};

    /// Every capability field and every field-less global the shell uses has a supported version.
    #[test]
    fn supported_covers_caps() {
        for (name, _) in GLOBAL_FIELDS {
            assert!(supported(name).is_some(), "{name} has no supported version");
        }
        for name in GLOBALS_WITHOUT_FIELD {
            assert!(supported(name).is_some(), "{name} has no supported version");
        }
    }

    #[test]
    fn caps_take_the_lower_version() {
        let mut g = Globals::default();
        for (i, (iface, v)) in [("xdg_wm_base", 9), ("wl_seat", 10), ("wl_compositor", 4)]
            .into_iter()
            .enumerate()
        {
            g.list.push(Global {
                name: i as u32 + 1,
                interface: iface.into(),
                version: v,
            });
        }
        let caps = g.caps();
        assert_eq!((caps.xdg_wm_base, caps.seat, caps.compositor), (7, 9, 4));
        assert!(!g.required(), "no wl_shm, no wl_subcompositor");
    }
}

//! Outputs: `wl_output` (v4, with names) on the main queue; `OUTPUT_ADD` when an output is complete (each `done`) and
//! `OUTPUT_REMOVE` when its global goes away.  An output's id is its registry name.

use crate::event::{NHSHELL_EV_OUTPUT_ADD, NHSHELL_EV_OUTPUT_REMOVE, NhShellEvent};
use crate::wayland::MainState;
use wayland_client::protocol::wl_output::{self, Mode, WlOutput};
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle, WEnum};

pub struct Output {
    proxy: WlOutput,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    mm_w: i32,
    mm_h: i32,
    scale: i32,
    name: String,
}

impl Output {
    pub fn new(proxy: WlOutput) -> Output {
        Output {
            proxy,
            x: 0,
            y: 0,
            w: 0,
            h: 0,
            mm_w: 0,
            mm_h: 0,
            scale: 1,
            name: String::new(),
        }
    }

    /// The integer `wl_output.scale` times 120.
    pub fn scale120(&self) -> u32 {
        u32::try_from(self.scale).unwrap_or(1).max(1) * 120
    }
}

impl MainState {
    /// Post a record and wake the host (this runs on the host's thread, inside a dispatch).
    pub(crate) fn post(&self, ev: NhShellEvent) {
        if let Some(shared) = &self.shared {
            shared.post(ev);
            shared.wake.signal();
        }
    }

    /// The global `name` went away.
    pub(crate) fn output_removed(&mut self, name: u32) {
        let Some(out) = self.outputs.remove(&name) else {
            return;
        };
        if out.proxy.version() >= 3 {
            out.proxy.release();
        }
        let mut ev = NhShellEvent::new(NHSHELL_EV_OUTPUT_REMOVE, 0, 0);
        ev.u.output.id = u64::from(name);
        self.post(ev);
        let ids: Vec<u64> = self.scales.keys().copied().collect();
        for id in ids {
            if let Some(s) = self.scales.get_mut(&id) {
                s.entered.retain(|&o| o != name);
            }
            self.recompute_scale(id);
        }
    }
}

impl Dispatch<WlOutput, u32> for MainState {
    fn event(
        state: &mut Self,
        proxy: &WlOutput,
        event: wl_output::Event,
        name: &u32,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let out = state
            .outputs
            .entry(*name)
            .or_insert_with(|| Output::new(proxy.clone()));
        match event {
            wl_output::Event::Geometry {
                x,
                y,
                physical_width,
                physical_height,
                ..
            } => {
                out.x = x;
                out.y = y;
                out.mm_w = physical_width;
                out.mm_h = physical_height;
            }
            wl_output::Event::Mode {
                flags,
                width,
                height,
                ..
            } => {
                if matches!(flags, WEnum::Value(f) if f.contains(Mode::Current)) {
                    out.w = width;
                    out.h = height;
                }
            }
            wl_output::Event::Scale { factor } => out.scale = factor,
            wl_output::Event::Name { name } => out.name = name,
            wl_output::Event::Done => {
                let mut ev = NhShellEvent::new(NHSHELL_EV_OUTPUT_ADD, 0, 0);
                ev.u.output.id = u64::from(*name);
                ev.u.output.x = out.x;
                ev.u.output.y = out.y;
                ev.u.output.w = out.w;
                ev.u.output.h = out.h;
                ev.u.output.mm_w = out.mm_w;
                ev.u.output.mm_h = out.mm_h;
                ev.u.output.scale120 = out.scale120();
                let bytes = out.name.as_bytes();
                let n = bytes.len().min(31);
                let mut buf = [0u8; 32];
                buf[..n].copy_from_slice(&bytes[..n]);
                ev.u.output.name = buf;
                state.post(ev);
                state.recompute_scales_on(*name);
            }
            _ => {}
        }
    }
}

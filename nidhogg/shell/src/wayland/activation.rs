//! xdg-activation: ask the compositor to activate a window.  A token handed in (emacsclient's
//! `XDG_ACTIVATION_TOKEN`) is used as is; without one the shell requests a token with the latest input serial and the
//! window's surface, and activates when the compositor sends it.  All of it runs on the main queue.

use crate::api::*;
use crate::wayland::{MainState, WaylandShell};
use std::collections::HashMap;
use wayland_client::{Connection, Dispatch, QueueHandle, delegate_noop};
use wayland_protocols::xdg::activation::v1::client::xdg_activation_token_v1::{
    self, XdgActivationTokenV1,
};
use wayland_protocols::xdg::activation::v1::client::xdg_activation_v1::XdgActivationV1;

delegate_noop!(MainState: XdgActivationV1);

impl WaylandShell {
    /// Activate window `id` with `token`, or with a token requested for it (`State` without an input serial, and
    /// `Unsupported` without `xdg_activation_v1`).
    pub(super) fn window_activate_impl(
        &mut self,
        id: WindowId,
        token: Option<&str>,
    ) -> ShellResult {
        let surface = self
            .main
            .surfaces
            .get(&id)
            .ok_or(ShellError::State)?
            .clone();
        let manager = self
            .main
            .activation
            .as_ref()
            .ok_or(ShellError::Unsupported)?;
        match token {
            Some("") => return Err(ShellError::Arg),
            Some(t) => manager.activate(t.to_owned(), &surface),
            None => {
                let serial = self
                    .shared
                    .serials
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .selection_serial()
                    .ok_or(ShellError::State)?
                    .as_raw();
                let seat = self.main.seat.as_ref().ok_or(ShellError::State)?;
                let request = manager.get_activation_token(&self.main_queue.handle(), id);
                request.set_serial(serial, seat);
                request.set_surface(&surface);
                request.commit();
            }
        }
        self.flush()
    }
}

/// Tokens asked for by `activation_token`: request number -> the token once it arrived.  A request that timed out is
/// removed, so its late answer is dropped.
#[derive(Default)]
pub struct Tokens {
    next: u64,
    wanted: HashMap<u64, Option<String>>,
}

/// The user data of a token request made for the host (it activates nothing).
pub struct Fetch(u64);

impl WaylandShell {
    /// A fresh activation token for window `id`, made with the newest input serial: within `timeout_ms` or `Timeout`
    /// (`State` before any input, `Unsupported` without `xdg_activation_v1`).
    pub(super) fn activation_token_impl(
        &mut self,
        id: WindowId,
        timeout_ms: u32,
    ) -> ShellResult<String> {
        let m = &mut self.main;
        let manager = m.activation.as_ref().ok_or(ShellError::Unsupported)?;
        let surface = m.surfaces.get(&id).ok_or(ShellError::State)?;
        let seat = m.seat.as_ref().ok_or(ShellError::State)?;
        let serial = self
            .shared
            .serials
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .selection_serial()
            .ok_or(ShellError::State)?
            .as_raw();
        let n = m.tokens.next;
        m.tokens.next += 1;
        m.tokens.wanted.insert(n, None);
        let request = manager.get_activation_token(&self.main_queue.handle(), Fetch(n));
        request.set_serial(serial, seat);
        request.set_surface(surface);
        request.commit();
        let r = self.wait_main(timeout_ms, |m| {
            m.tokens.wanted.get(&n).is_some_and(Option::is_some)
        });
        let token = self.main.tokens.wanted.remove(&n).flatten();
        r?;
        token.ok_or(ShellError::State)
    }
}

impl Dispatch<XdgActivationTokenV1, Fetch> for MainState {
    fn event(
        state: &mut Self,
        request: &XdgActivationTokenV1,
        event: xdg_activation_token_v1::Event,
        fetch: &Fetch,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_activation_token_v1::Event::Done { token } = event {
            if let Some(slot) = state.tokens.wanted.get_mut(&fetch.0) {
                *slot = Some(token);
            }
            request.destroy();
        }
    }
}

impl Dispatch<XdgActivationTokenV1, u64> for MainState {
    fn event(
        state: &mut Self,
        request: &XdgActivationTokenV1,
        event: xdg_activation_token_v1::Event,
        id: &u64,
        conn: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_activation_token_v1::Event::Done { token } = event {
            if let (Some(manager), Some(surface)) = (&state.activation, state.surfaces.get(id)) {
                manager.activate(token, surface);
            }
            request.destroy();
            // this runs from `next_event`'s dispatch, which does not flush
            if let Some(shared) = &state.shared {
                let _ = shared.host_flush(conn);
            }
        }
    }
}

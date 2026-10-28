//! The system bell (`xdg_system_bell_v1`): ring on a window's surface, at most once per 20 ms (GTK4's minimum delay).
//! The visible bell is the host's: it draws the flash into its buffer and shows it through damage like any drawing.

use crate::api::*;
use crate::wayland::{MainState, WaylandShell};
use std::time::{Duration, Instant};
use wayland_client::delegate_noop;
use wayland_protocols::xdg::system_bell::v1::client::xdg_system_bell_v1::XdgSystemBellV1;

delegate_noop!(MainState: XdgSystemBellV1);

/// No two rings closer than this.
const MIN_DELAY: Duration = Duration::from_millis(20);

#[derive(Default)]
pub struct Bell {
    pub manager: Option<XdgSystemBellV1>,
    last: Option<Instant>,
}

impl Bell {
    /// Whether a ring at `now` goes out (and if so, it is counted).
    fn allow(&mut self, now: Instant) -> bool {
        if self.last.is_some_and(|t| now.duration_since(t) < MIN_DELAY) {
            return false;
        }
        self.last = Some(now);
        true
    }
}

impl WaylandShell {
    /// Ring the bell on window `id`'s surface (0: no surface).  A ring inside the delay is dropped.
    pub(super) fn bell_impl(&mut self, id: WindowId) -> ShellResult {
        let m = &mut self.main;
        if m.bell.manager.is_none() {
            return Err(ShellError::Unsupported);
        }
        let surface = match id {
            0 => None,
            _ => Some(m.surfaces.get(&id).ok_or(ShellError::State)?),
        };
        if !m.bell.allow(Instant::now()) {
            return Ok(());
        }
        if let Some(manager) = &m.bell.manager {
            manager.ring(surface);
        }
        self.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rings_are_spaced() {
        let mut b = Bell::default();
        let t = Instant::now();
        assert!(b.allow(t));
        assert!(!b.allow(t + Duration::from_millis(19)));
        assert!(b.allow(t + Duration::from_millis(20)));
    }
}

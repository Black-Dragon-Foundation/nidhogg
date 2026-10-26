//! Cursor shapes through `wp_cursor_shape_v1` (the Xcursor fallback is W1's).

use super::WaylandShell;
use crate::api::*;
use crate::wayland::serial::{SerialKind, SerialTracker};
use wayland_protocols::wp::cursor_shape::v1::client::wp_cursor_shape_device_v1::Shape;

/// The protocol shape of an `NHSHELL_CURSOR_*` name; `None` for a shape the protocol cannot show (`HIDDEN`).
fn shape_of(shape: u32) -> Option<Shape> {
    Some(match shape {
        NHSHELL_CURSOR_DEFAULT => Shape::Default,
        NHSHELL_CURSOR_TEXT => Shape::Text,
        NHSHELL_CURSOR_POINTER => Shape::Pointer,
        NHSHELL_CURSOR_CROSSHAIR => Shape::Crosshair,
        NHSHELL_CURSOR_WAIT => Shape::Wait,
        NHSHELL_CURSOR_HELP => Shape::Help,
        NHSHELL_CURSOR_MOVE => Shape::Move,
        NHSHELL_CURSOR_NOT_ALLOWED => Shape::NotAllowed,
        NHSHELL_CURSOR_RESIZE_NS => Shape::NsResize,
        NHSHELL_CURSOR_RESIZE_EW => Shape::EwResize,
        NHSHELL_CURSOR_RESIZE_NWSE => Shape::NwseResize,
        NHSHELL_CURSOR_RESIZE_NESW => Shape::NeswResize,
        _ => return None,
    })
}

/// The serial and shape a `set_shape` request is sent with: the latest pointer `enter` serial.
pub fn shape_request(serials: &SerialTracker, shape: u32) -> ShellResult<(u32, Shape)> {
    let shape = shape_of(shape).ok_or(ShellError::Arg)?;
    Ok((serials.get(SerialKind::MouseEnter).as_raw(), shape))
}

impl WaylandShell {
    /// Show `shape` over the pointer.  Without `wp_cursor_shape_v1` this does nothing.
    pub(super) fn window_set_cursor(&mut self, id: WindowId, shape: u32) -> ShellResult {
        if !self.main.windows.contains_key(&id) {
            return Err(ShellError::State);
        }
        let device = self
            .shared
            .cursor_device
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        let (serial, shape) = {
            let serials = self
                .shared
                .serials
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            shape_request(&serials, shape)?
        };
        if let Some(device) = device {
            device.set_shape(serial, shape);
            self.flush()?;
        }
        Ok(())
    }

    /// Test hook: the serial of the latest button press the reader saw.
    #[doc(hidden)]
    pub fn latest_press_serial(&self) -> u32 {
        self.shared
            .serials
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(SerialKind::MousePress)
            .as_raw()
    }
}

//! Cursor shapes through `wp_cursor_shape_v1`, or the Xcursor theme (`cursor_theme.rs`) without it.

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
    /// Show `shape` over the pointer: through `wp_cursor_shape_v1`, or the desktop's Xcursor theme without it.
    /// `NHSHELL_CURSOR_HIDDEN` shows no cursor at all.
    pub(super) fn window_set_cursor(&mut self, id: WindowId, shape: u32) -> ShellResult {
        if !self.main.is_window(id) {
            return Err(ShellError::State);
        }
        self.apply_cursor(id, shape)?;
        self.main.cursor.last = Some((id, shape));
        Ok(())
    }

    /// Use the Xcursor theme `name` at logical `size`; a cursor shown from the theme is redrawn at once.
    pub(super) fn set_cursor_theme_impl(&mut self, name: &str, size: u32) -> ShellResult {
        self.main.cursor.set_theme(name, size);
        match self.main.cursor.last {
            Some((id, shape)) if self.main.is_window(id) => self.apply_cursor(id, shape),
            _ => Ok(()),
        }
    }

    fn apply_cursor(&mut self, id: WindowId, shape: u32) -> ShellResult {
        let serial = self
            .shared
            .serials
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(SerialKind::MouseEnter)
            .as_raw();
        let pointer = self
            .shared
            .pointer
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        if shape == NHSHELL_CURSOR_HIDDEN {
            if let Some(pointer) = pointer {
                pointer.set_cursor(serial, None, 0, 0);
                self.flush()?;
            }
            return Ok(());
        }
        let device = self
            .shared
            .cursor_device
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        if let Some(device) = device {
            let (serial, shape) = shape_request(
                &self
                    .shared
                    .serials
                    .lock()
                    .unwrap_or_else(|e| e.into_inner()),
                shape,
            )?;
            device.set_shape(serial, shape);
            return self.flush();
        }
        let names = super::cursor_theme::names_of(shape).ok_or(ShellError::Arg)?;
        let (Some(pointer), Some(shm), Some(compositor)) =
            (pointer, self.main.shm.clone(), self.main.compositor.clone())
        else {
            return Ok(());
        };
        let scale = self
            .main
            .scales
            .get(&id)
            .map_or(120, |s| s.scale120())
            .div_ceil(120);
        let qh = self.main_queue.handle();
        self.main.cursor.show(
            &self.conn,
            &shm,
            &compositor,
            &qh,
            &pointer,
            serial,
            names,
            scale,
        )?;
        self.flush()
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

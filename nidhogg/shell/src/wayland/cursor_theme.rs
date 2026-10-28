// SPDX-License-Identifier: GPL-3.0-or-later AND Apache-2.0
// Copyright 2022-2026 Zed Industries, Inc.
// Adapted-from: https://github.com/zed-industries/zed@347f171a3fd00a3b672382d78348b37bd969d17c crates/gpui_linux/src/linux/wayland/cursor.rs
// Modified for Nidhogg: the theme is loaded for a size times the surface's integer scale and reloaded when that
// changes (Zed reloads on a name change only); the cursor names come from `NHSHELL_CURSOR_*` shapes with the
// legacy X names after the CSS ones; the pointer, the serial and the surface scale are passed in by the caller
// (Zed keeps them on its client state); animated cursors show their first frame.

//! The cursor drawn from the desktop's Xcursor theme, for compositors without `wp_cursor_shape_v1`.

use crate::api::*;
use wayland_client::protocol::wl_compositor::WlCompositor;
use wayland_client::protocol::wl_pointer::WlPointer;
use wayland_client::protocol::wl_shm::WlShm;
use wayland_client::protocol::wl_surface::WlSurface;
use wayland_client::{Connection, QueueHandle};
use wayland_cursor::CursorTheme;

/// The theme's name and size when the desktop has not said.
const DEFAULT_NAME: &str = "default";
const DEFAULT_SIZE: u32 = 24;

/// The Xcursor names of `shape`, most specific first (CSS names, then the legacy X ones).
pub fn names_of(shape: u32) -> Option<&'static [&'static str]> {
    Some(match shape {
        NHSHELL_CURSOR_DEFAULT => &["default", "left_ptr", "arrow"],
        NHSHELL_CURSOR_TEXT => &["text", "xterm", "ibeam"],
        NHSHELL_CURSOR_POINTER => &["pointer", "hand2", "hand1", "hand"],
        NHSHELL_CURSOR_CROSSHAIR => &["crosshair", "cross"],
        NHSHELL_CURSOR_WAIT => &["wait", "watch"],
        NHSHELL_CURSOR_HELP => &["help", "question_arrow", "left_ptr_help"],
        NHSHELL_CURSOR_MOVE => &["move", "fleur", "all-scroll"],
        NHSHELL_CURSOR_NOT_ALLOWED => &["not-allowed", "crossed_circle", "circle"],
        NHSHELL_CURSOR_RESIZE_NS => &["ns-resize", "sb_v_double_arrow", "row-resize"],
        NHSHELL_CURSOR_RESIZE_EW => &["ew-resize", "sb_h_double_arrow", "col-resize"],
        NHSHELL_CURSOR_RESIZE_NWSE => &["nwse-resize", "bd_double_arrow", "size_fdiag"],
        NHSHELL_CURSOR_RESIZE_NESW => &["nesw-resize", "fd_double_arrow", "size_bdiag"],
        _ => return None,
    })
}

/// The Xcursor theme, loaded once per (name, size x scale), and the surface the pointer shows it on.
#[derive(Default)]
pub struct ThemeCursor {
    name: String,
    size: u32,
    /// The loaded theme and the pixel size it was loaded for.
    loaded: Option<(CursorTheme, u32)>,
    surface: Option<WlSurface>,
    /// The latest shape request, so a new theme can be shown at once.
    pub last: Option<(WindowId, u32)>,
}

impl ThemeCursor {
    /// Use theme `name` at logical `size` from now on (an empty name or a zero size: the defaults).
    pub fn set_theme(&mut self, name: &str, size: u32) {
        if name != self.name || size != self.size {
            self.name = name.to_string();
            self.size = size;
            self.loaded = None;
        }
    }

    /// The connection was made again: the theme and the surface belonged to the old one (the name, size and latest
    /// shape stay).
    pub fn forget_connection(&mut self) {
        self.loaded = None;
        self.surface = None;
    }

    /// Show the first of `names` the theme has over `pointer`, with `serial`, at the integer `scale`.  A theme with
    /// none of them leaves the cursor as it was.
    #[allow(clippy::too_many_arguments)]
    pub fn show(
        &mut self,
        conn: &Connection,
        shm: &WlShm,
        compositor: &WlCompositor,
        qh: &QueueHandle<super::MainState>,
        pointer: &WlPointer,
        serial: u32,
        names: &[&str],
        scale: u32,
    ) -> ShellResult {
        let scale = scale.max(1);
        let size = if self.size == 0 {
            DEFAULT_SIZE
        } else {
            self.size
        };
        let px = size * scale;
        if self.loaded.as_ref().is_none_or(|(_, loaded)| *loaded != px) {
            let name = if self.name.is_empty() {
                DEFAULT_NAME
            } else {
                &self.name
            };
            let theme = CursorTheme::load_from_name(conn, shm.clone(), name, px)
                .map_err(|_| ShellError::Unsupported)?;
            self.loaded = Some((theme, px));
        }
        let Some((theme, _)) = self.loaded.as_mut() else {
            return Ok(());
        };
        let Some(name) = names.iter().find(|n| theme.get_cursor(n).is_some()) else {
            return Ok(());
        };
        let Some(cursor) = theme.get_cursor(name) else {
            return Ok(());
        };
        let image = &cursor[0];
        let (w, h) = image.dimensions();
        let (hot_x, hot_y) = image.hotspot();
        let surface = self
            .surface
            .get_or_insert_with(|| compositor.create_surface(qh, ()));
        let scale = scale as i32;
        // the role first: the surface's commit follows the `set_cursor` request
        pointer.set_cursor(
            serial,
            Some(surface),
            hot_x as i32 / scale,
            hot_y as i32 / scale,
        );
        surface.set_buffer_scale(scale);
        surface.attach(Some(image), 0, 0);
        surface.damage_buffer(0, 0, w as i32, h as i32);
        surface.commit();
        Ok(())
    }
}

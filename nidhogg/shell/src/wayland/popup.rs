//! Popup windows: `xdg_popup`s placed by an `xdg_positioner`, for the tips that must stay on screen (plan call W1-3).
//!
//! A popup has no configure handshake the host takes part in: the handler acks every configure at once, and the size
//! is the one the positioner asked for.  The compositor dismissing it posts `POPUP_DONE`.

use crate::api::*;
use crate::event::*;
use crate::wayland::{MainState, WaylandShell};
use wayland_client::protocol::wl_surface::WlSurface;
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle};
use wayland_protocols::xdg::shell::client::xdg_popup::{self, XdgPopup};
use wayland_protocols::xdg::shell::client::xdg_positioner::{
    Anchor as PAnchor, ConstraintAdjustment, Gravity, XdgPositioner,
};
use wayland_protocols::xdg::shell::client::xdg_surface::XdgSurface;
use wayland_protocols::xdg::shell::client::xdg_wm_base::XdgWmBase;

pub struct Popup {
    surface: WlSurface,
    xdg_surface: XdgSurface,
    popup: XdgPopup,
    /// The window the host named, and the `xdg_surface` the popup hangs on (the nearest ancestor that has one) with
    /// the offset from it to the window the host named.
    pub parent: WindowId,
    base: XdgSurface,
    offset: (i32, i32),
    anchor: Anchor,
    size: (i32, i32),
    token: u32,
    /// The next configure is the popup's first (or the first after it was made again): a content ack.
    first: bool,
}

impl Popup {
    pub(super) fn xdg_surface(&self) -> &XdgSurface {
        &self.xdg_surface
    }
}

fn positioner(
    wm_base: &XdgWmBase,
    qh: &QueueHandle<MainState>,
    size: (i32, i32),
    anchor: &Anchor,
    offset: (i32, i32),
) -> ShellResult<XdgPositioner> {
    // the anchor edge and the gravity are the same direction (see `Anchor`)
    let edge = PAnchor::try_from(anchor.gravity).map_err(|_| ShellError::Arg)?;
    let gravity = Gravity::try_from(anchor.gravity).map_err(|_| ShellError::Arg)?;
    let p = wm_base.create_positioner(qh, ());
    p.set_size(size.0.max(1), size.1.max(1));
    p.set_anchor_rect(
        anchor.x + offset.0,
        anchor.y + offset.1,
        anchor.w.max(1),
        anchor.h.max(1),
    );
    p.set_anchor(edge);
    p.set_gravity(gravity);
    p.set_constraint_adjustment(ConstraintAdjustment::from_bits_truncate(anchor.adjust & 15));
    Ok(p)
}

impl WaylandShell {
    /// The `xdg_surface` a popup of window `parent` hangs on, and the offset of `parent` inside it.
    fn popup_base(&self, parent: WindowId) -> ShellResult<(XdgSurface, (i32, i32))> {
        let (mut id, mut off) = (parent, (0, 0));
        // a child is a subsurface: climb to the window that has an xdg_surface
        while let Some(c) = self.main.children.get(&id) {
            off = (off.0 + c.x, off.1 + c.y);
            id = c.parent;
        }
        if let Some(w) = self.main.windows.get(&id) {
            return Ok((w.xdg_surface.clone(), off));
        }
        if let Some(p) = self.main.popups.get(&id) {
            return Ok((p.xdg_surface.clone(), off));
        }
        Err(ShellError::State)
    }

    pub(super) fn popup_create(&mut self, d: &ChildDesc) -> ShellResult<WindowId> {
        if d.w <= 0 || d.h <= 0 {
            return Err(ShellError::Arg);
        }
        let (base, offset) = self.popup_base(d.parent)?;
        let qh = self.main_queue.handle();
        let m = &mut self.main;
        let wm_base = m.wm_base.as_ref().ok_or(ShellError::State)?;
        let pos = positioner(wm_base, &qh, (d.w, d.h), &d.anchor, offset)?;
        let surface = m
            .compositor
            .as_ref()
            .ok_or(ShellError::State)?
            .create_surface(&qh, ());
        let id = u64::from(surface.id().protocol_id());
        let xdg_surface = wm_base.get_xdg_surface(&surface, &qh, id);
        let popup = xdg_surface.get_popup(Some(&base), &pos, &qh, id);
        pos.destroy();
        m.attach_scale(&surface, &qh);
        // the compositor sends the first configure only after this bare commit
        surface.commit();
        m.surfaces.insert(id, surface.clone());
        m.popups.insert(
            id,
            Popup {
                surface,
                xdg_surface,
                popup,
                parent: d.parent,
                base,
                offset,
                anchor: d.anchor,
                size: (d.w, d.h),
                token: 0,
                first: true,
            },
        );
        self.flush()?;
        Ok(id)
    }

    pub(super) fn popup_destroy(&mut self, id: WindowId) -> ShellResult {
        let p = self.main.popups.remove(&id).ok_or(ShellError::State)?;
        self.main.surfaces.remove(&id);
        self.main.detach_scale(id);
        self.present_drop(id);
        p.popup.destroy();
        p.xdg_surface.destroy();
        p.surface.destroy();
        self.flush()
    }

    /// Move popup `id`'s anchor rectangle to (`x`, `y`): `xdg_popup.reposition` (v3+), else the popup is destroyed
    /// and made again on the same surface.
    /// Returns whether the popup was made again (it shows nothing until a buffer maps it: redraw all).
    pub(super) fn popup_reposition(&mut self, id: WindowId, x: i32, y: i32) -> ShellResult<bool> {
        let qh = self.main_queue.handle();
        let m = &mut self.main;
        let wm_base = m.wm_base.as_ref().ok_or(ShellError::State)?;
        let p = m.popups.get_mut(&id).ok_or(ShellError::State)?;
        p.anchor.x = x;
        p.anchor.y = y;
        let pos = positioner(wm_base, &qh, p.size, &p.anchor, p.offset)?;
        let recreate = p.popup.version() < 3;
        if !recreate {
            p.token = p.token.wrapping_add(1);
            p.popup.reposition(&pos, p.token);
        } else {
            p.first = true;
            p.popup.destroy();
            p.xdg_surface.destroy();
            p.surface.attach(None, 0, 0);
            p.surface.commit();
            p.xdg_surface = wm_base.get_xdg_surface(&p.surface, &qh, id);
            p.popup = p.xdg_surface.get_popup(Some(&p.base), &pos, &qh, id);
            p.surface.commit();
        }
        pos.destroy();
        if recreate {
            self.main.present_reset(id);
        }
        self.flush()?;
        Ok(recreate)
    }
}

impl MainState {
    /// Record a popup's configure ack (sent with the next commit); false when `id` is not a popup.  The first
    /// configure changes what the popup shows (a buffer maps it); a configure after `repositioned` is a bare ack.
    pub(super) fn popup_ack(&mut self, id: u64, serial: u32) -> bool {
        let Some(p) = self.popups.get_mut(&id) else {
            return false;
        };
        let content = std::mem::take(&mut p.first);
        self.present_ack(id, serial, content);
        true
    }
}

impl Dispatch<XdgPopup, u64> for MainState {
    fn event(
        state: &mut Self,
        _: &XdgPopup,
        event: xdg_popup::Event,
        id: &u64,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_popup::Event::PopupDone = event
            && let Some(shared) = state.shared.as_ref()
        {
            shared.post(NhShellEvent::new(NHSHELL_EV_POPUP_DONE, *id, 0));
        }
    }
}

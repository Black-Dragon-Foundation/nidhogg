//! Popup windows: `xdg_popup`s placed by an `xdg_positioner`, for tips that must stay on screen (plan call W1-3) and
//! for menus, which grab the keyboard and pointer (W2-5).
//!
//! A popup's configures are recorded acks (present path §3.1): the first, the first after it was made again and the
//! first after a new size are content acks; one after `repositioned` is a bare ack.  The compositor dismissing it
//! posts `POPUP_DONE`.
//!
//! **Grabs.**  `xdg_popup.grab` goes out before the popup's first commit, with the newest button press, key press or
//! touch down serial (GTK4's `gdkpopup-wayland.c` and Chromium's `xdg_popup.cc` do the same).  None is sent while a
//! drag is in progress, without a serial, or when the popup would not become the topmost grab: its parent must be a
//! top-level window while no popup holds a grab, or the popup holding the topmost grab (wlroots refuses the rest with
//! `not_the_topmost_popup`, xdg-shell with `invalid_grab`).
//!
//! **Destroy order.**  A popup is destroyed only when nothing is above it: `window_destroy` takes its dependents
//! first (deepest first), and `popup_destroy` takes every grabbing popup above it first, topmost first, so the
//! compositor never sees `not_the_topmost_popup`.

use crate::api::*;
use crate::event::*;
use crate::wayland::{MainState, WaylandShell, present};
use std::sync::atomic::Ordering;
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
    /// The window the host named, and the window whose `xdg_surface` the popup hangs on (the nearest ancestor that
    /// has one) with that `xdg_surface` and the offset from it to the window the host named.
    pub parent: WindowId,
    base_window: WindowId,
    base: XdgSurface,
    offset: (i32, i32),
    anchor: Anchor,
    size: (i32, i32),
    token: u32,
    /// The next configure is a content ack: the popup's first, the first after it was made again, or the first after
    /// a new size.
    first: bool,
    /// The host asked for a grab (`NHSHELL_WIN_GRAB`).
    wants_grab: bool,
    /// The popup holds a grab: one was sent, and the compositor has not dismissed the popup since.
    grabbed: bool,
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

impl MainState {
    /// The popup holding the topmost grab, if any popup holds one.
    fn top_grab(&self) -> Option<WindowId> {
        self.grabs
            .iter()
            .rev()
            .find(|g| self.popups.get(g).is_some_and(|p| p.grabbed))
            .copied()
    }
}

impl WaylandShell {
    /// The window whose `xdg_surface` a popup of window `parent` hangs on, that `xdg_surface`, and the offset of
    /// `parent` inside it.
    fn popup_base(&self, parent: WindowId) -> ShellResult<(WindowId, XdgSurface, (i32, i32))> {
        let (mut id, mut off) = (parent, (0, 0));
        // a child is a subsurface: climb to the window that has an xdg_surface
        while let Some(c) = self.main.children.get(&id) {
            off = (off.0 + c.x, off.1 + c.y);
            id = c.parent;
        }
        if let Some(w) = self.main.windows.get(&id) {
            return Ok((id, w.xdg_surface.clone(), off));
        }
        if let Some(p) = self.main.popups.get(&id) {
            return Ok((id, p.xdg_surface.clone(), off));
        }
        Err(ShellError::State)
    }

    /// The serial a new grabbing popup on window `base` grabs with, or `None` when it must not grab (W2-5).
    fn grab_serial(&self, base: WindowId) -> Option<u32> {
        if self.shared.dragging.load(Ordering::Acquire) {
            return None;
        }
        let top = self.main.top_grab();
        let on_top = if self.main.windows.contains_key(&base) {
            top.is_none()
        } else {
            top == Some(base)
        };
        if !on_top {
            return None;
        }
        self.shared
            .serials
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .grab_serial()
    }

    /// Send popup `id`'s grab when its rules allow one; it goes out before the commit that makes the popup.
    fn popup_grab(&mut self, id: WindowId) {
        let Some(p) = self.main.popups.get(&id) else {
            return;
        };
        if !p.wants_grab {
            return;
        }
        let (Some(serial), Some(seat)) = (self.grab_serial(p.base_window), self.main.seat.clone())
        else {
            return;
        };
        if let Some(p) = self.main.popups.get_mut(&id) {
            p.popup.grab(&seat, serial);
            p.grabbed = true;
            self.main.grabs.retain(|g| *g != id);
            self.main.grabs.push(id);
        }
    }

    pub(super) fn popup_create(&mut self, d: &ChildDesc) -> ShellResult<WindowId> {
        if d.w <= 0 || d.h <= 0 {
            return Err(ShellError::Arg);
        }
        let (base_window, base, offset) = self.popup_base(d.parent)?;
        let qh = self.main_queue.handle();
        let m = &mut self.main;
        let wm_base = m.wm_base.clone().ok_or(ShellError::State)?;
        if m.compositor.is_none() {
            return Err(ShellError::State);
        }
        let pos = positioner(&wm_base, &qh, (d.w, d.h), &d.anchor, offset)?;
        let (surface, id) = m.window_surface(&qh)?;
        let xdg_surface = wm_base.get_xdg_surface(&surface, &qh, id);
        let popup = xdg_surface.get_popup(Some(&base), &pos, &qh, id);
        pos.destroy();
        m.attach_scale(&surface, &qh);
        m.surfaces.insert(id, surface.clone());
        m.popups.insert(
            id,
            Popup {
                surface: surface.clone(),
                xdg_surface: xdg_surface.clone(),
                popup,
                parent: d.parent,
                base_window,
                base,
                offset,
                anchor: d.anchor,
                size: (d.w, d.h),
                token: 0,
                first: true,
                wants_grab: d.flags & NHSHELL_WIN_GRAB != 0,
                grabbed: false,
            },
        );
        self.popup_grab(id);
        // the compositor sends the first configure only after this bare commit
        surface.commit();
        self.main.present_register(id, Some(xdg_surface), None);
        self.flush()?;
        Ok(id)
    }

    pub(super) fn popup_destroy(&mut self, id: WindowId) -> ShellResult {
        // grabbing popups above this one go first, topmost first
        if let Some(i) = self.main.grabs.iter().position(|g| *g == id) {
            let above: Vec<WindowId> = self.main.grabs[i + 1..].iter().rev().copied().collect();
            for a in above {
                let _ = self.window_destroy(a);
            }
        }
        if !self.main.popups.contains_key(&id) {
            return Err(ShellError::State);
        }
        self.present_retire(id, |m| {
            m.grabs.retain(|g| *g != id);
            m.surfaces.remove(&id);
            m.detach_scale(id);
            if let Some(p) = m.popups.remove(&id) {
                p.popup.destroy();
                p.xdg_surface.destroy();
                p.surface.destroy();
            }
        });
        self.flush()
    }

    /// Move popup `id`'s anchor rectangle to (`x`, `y`).  Returns whether the popup was made again.
    pub(super) fn popup_reposition(&mut self, id: WindowId, x: i32, y: i32) -> ShellResult<bool> {
        self.popup_place(id, |p| {
            p.anchor.x = x;
            p.anchor.y = y;
        })
    }

    /// Give popup `id` a new size (logical pixels); the next configure is a content ack.  Returns whether the popup
    /// was made again.
    pub(super) fn popup_set_size(&mut self, id: WindowId, w: i32, h: i32) -> ShellResult<bool> {
        if w <= 0 || h <= 0 {
            return Err(ShellError::Arg);
        }
        let p = self.main.popups.get(&id).ok_or(ShellError::State)?;
        if p.size == (w, h) {
            return Ok(false);
        }
        self.popup_place(id, |p| {
            p.size = (w, h);
            p.first = true;
        })
    }

    /// Change popup `id`'s placement with `change`, then send it: `xdg_popup.reposition` (v3+), else the popup is
    /// destroyed and made again on the same surface (and grabs again if it held the topmost grab).
    /// Returns whether the popup was made again (it shows nothing until a buffer maps it: redraw all).
    fn popup_place(&mut self, id: WindowId, change: impl FnOnce(&mut Popup)) -> ShellResult<bool> {
        let p = self.main.popups.get(&id).ok_or(ShellError::State)?;
        let recreate = p.popup.version() < 3;
        // destroying a popup that has popups of its own is `not_the_topmost_popup`
        if recreate && self.main.popups.values().any(|q| q.base == p.xdg_surface) {
            return Err(ShellError::State);
        }
        let qh = self.main_queue.handle();
        // making it again is role setup, done under the tree's mutex like a destroy
        let tree = self.main.presents.get(&id).cloned().filter(|_| recreate);
        let mut tree = tree.as_ref().map(|t| present::lock(t));
        let m = &mut self.main;
        let wm_base = m.wm_base.as_ref().ok_or(ShellError::State)?;
        let p = m.popups.get_mut(&id).ok_or(ShellError::State)?;
        change(p);
        let pos = positioner(wm_base, &qh, p.size, &p.anchor, p.offset)?;
        if !recreate {
            p.token = p.token.wrapping_add(1);
            p.popup.reposition(&pos, p.token);
            pos.destroy();
            self.flush()?;
            return Ok(false);
        }
        p.first = true;
        let held = std::mem::take(&mut p.grabbed);
        p.popup.destroy();
        p.xdg_surface.destroy();
        p.surface.attach(None, 0, 0);
        p.surface.commit();
        p.xdg_surface = wm_base.get_xdg_surface(&p.surface, &qh, id);
        p.popup = p.xdg_surface.get_popup(Some(&p.base), &pos, &qh, id);
        pos.destroy();
        let (surface, xdg) = (p.surface.clone(), p.xdg_surface.clone());
        m.grabs.retain(|g| *g != id);
        if held {
            self.popup_grab(id);
        }
        surface.commit();
        if let Some(t) = tree.as_deref_mut() {
            t.reset(xdg);
        }
        drop(tree);
        self.flush()?;
        Ok(true)
    }

    /// A reconnect closes every popup, topmost first (the deepest, then the newest grab): each posts `POPUP_DONE`, and
    /// its record goes with what hangs on it.  Its objects went with the old connection; a popup shown again is a new
    /// one.
    pub(super) fn popups_closed(&mut self) {
        let m = &self.main;
        let depth = |mut id: WindowId| {
            let mut d = 0;
            loop {
                if let Some(p) = m.popups.get(&id) {
                    d += 1;
                    id = p.parent;
                } else if let Some(c) = m.children.get(&id) {
                    id = c.parent;
                } else {
                    return d;
                }
            }
        };
        let grab = |id: WindowId| m.grabs.iter().position(|g| *g == id).map_or(0, |i| i + 1);
        let mut order: Vec<(usize, usize, WindowId)> = m
            .popups
            .keys()
            .map(|&id| (depth(id), grab(id), id))
            .collect();
        order.sort_unstable_by(|a, b| b.cmp(a));
        for &(_, _, id) in &order {
            self.shared
                .post(NhShellEvent::new(NHSHELL_EV_POPUP_DONE, id, 0));
        }
        for (_, _, id) in order {
            let _ = self.window_destroy_local(id);
        }
    }

    /// Whether window `id` is a popup that holds a grab.
    pub(super) fn popup_grabbed(&self, id: WindowId) -> ShellResult<bool> {
        if let Some(p) = self.main.popups.get(&id) {
            return Ok(p.grabbed);
        }
        if self.main.is_window(id) {
            Ok(false)
        } else {
            Err(ShellError::State)
        }
    }
}

impl MainState {
    /// Record a popup's configure ack (sent with the next commit); false when `id` is not a popup.  The first
    /// configure (or the first after a new size) changes what the popup shows; a configure after `repositioned` is a
    /// bare ack.
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
        if let xdg_popup::Event::PopupDone = event {
            // dismissed: the grab is gone, but the popup stays until the host destroys it (still topmost first)
            if let Some(p) = state.popups.get_mut(id) {
                p.grabbed = false;
            }
            if let Some(shared) = state.shared.as_ref() {
                shared.post(NhShellEvent::new(NHSHELL_EV_POPUP_DONE, *id, 0));
            }
        }
    }
}

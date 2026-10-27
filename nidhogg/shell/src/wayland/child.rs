//! Child windows: desynchronised `wl_subsurface`s of another window's surface, and the requests every window kind
//! shares (input region, mapped).
//!
//! A child presents on its own (its buffers and frame callbacks are the presenter's, per window).  Its position and
//! stacking are the parent's state: they take effect with the parent's next commit (Wayland's rule).

use crate::api::*;
use crate::wayland::WaylandShell;
use glitnir::geom::Rect;
use wayland_client::Proxy;
use wayland_client::protocol::wl_subsurface::WlSubsurface;
use wayland_client::protocol::wl_surface::WlSurface;

pub struct Child {
    pub surface: WlSurface,
    subsurface: WlSubsurface,
    pub parent: WindowId,
    /// The offset in the parent, logical pixels.
    pub x: i32,
    pub y: i32,
}

impl WaylandShell {
    pub(super) fn child_create(&mut self, d: &ChildDesc) -> ShellResult<WindowId> {
        let qh = self.main_queue.handle();
        let m = &mut self.main;
        let parent = m.surfaces.get(&d.parent).ok_or(ShellError::State)?.clone();
        let compositor = m.compositor.as_ref().ok_or(ShellError::State)?;
        let sub = m.subcompositor.as_ref().ok_or(ShellError::State)?;
        let surface = compositor.create_surface(&qh, ());
        let id = u64::from(surface.id().protocol_id());
        let subsurface = sub.get_subsurface(&surface, &parent, &qh, ());
        subsurface.set_desync();
        subsurface.set_position(d.x, d.y);
        m.attach_scale(&surface, &qh);
        m.surfaces.insert(id, surface.clone());
        m.children.insert(
            id,
            Child {
                surface,
                subsurface,
                parent: d.parent,
                x: d.x,
                y: d.y,
            },
        );
        self.flush()?;
        Ok(id)
    }

    pub(super) fn child_destroy(&mut self, id: WindowId) -> ShellResult {
        let c = self.main.children.remove(&id).ok_or(ShellError::State)?;
        self.main.surfaces.remove(&id);
        self.main.detach_scale(id);
        self.present_drop(id);
        c.subsurface.destroy();
        c.surface.destroy();
        self.flush()
    }

    /// Windows whose parent is `id` (children and popups), for destroying them with it.
    pub(super) fn dependents_of(&self, id: WindowId) -> Vec<WindowId> {
        let kids = self.main.children.iter().filter(|(_, c)| c.parent == id);
        let pops = self.main.popups.iter().filter(|(_, p)| p.parent == id);
        kids.map(|(k, _)| *k).chain(pops.map(|(k, _)| *k)).collect()
    }

    pub(super) fn window_set_position(&mut self, id: WindowId, x: i32, y: i32) -> ShellResult {
        if let Some(c) = self.main.children.get_mut(&id) {
            c.x = x;
            c.y = y;
            c.subsurface.set_position(x, y);
            // The protocol applies the position with the parent's commit.  Some compositors (Smithay) apply a
            // desync subsurface's position only with the child's own commit, so send one: it carries no new buffer.
            c.surface.commit();
            return self.flush();
        }
        self.popup_reposition(id, x, y)
    }

    pub(super) fn window_stack(
        &mut self,
        id: WindowId,
        sibling: WindowId,
        above: bool,
    ) -> ShellResult {
        let c = self.main.children.get(&id).ok_or(ShellError::State)?;
        let sibling = if sibling == 0 { c.parent } else { sibling };
        let other = self.main.surfaces.get(&sibling).ok_or(ShellError::Arg)?;
        if above {
            c.subsurface.place_above(other);
        } else {
            c.subsurface.place_below(other);
        }
        Ok(())
    }

    pub(super) fn window_set_input_region(&mut self, id: WindowId, rects: &[Rect]) -> ShellResult {
        let surface = self.main.surfaces.get(&id).ok_or(ShellError::State)?;
        if rects.is_empty() {
            surface.set_input_region(None);
        } else {
            let compositor = self.main.compositor.as_ref().ok_or(ShellError::State)?;
            let region = compositor.create_region(&self.main_queue.handle(), ());
            for r in rects {
                region.add(r.x, r.y, r.w, r.h);
            }
            surface.set_input_region(Some(&region));
            region.destroy();
        }
        // the region is double-buffered: it counts from the surface's next commit
        surface.commit();
        self.flush()
    }

    pub(super) fn window_set_mapped(&mut self, id: WindowId, on: bool) -> ShellResult {
        if on {
            // a frame callback the compositor dropped while the window was hidden must not hold the pacer back: a
            // waiting pacer asks again (frame callback and an empty commit)
            if !self.main.surfaces.contains_key(&id) {
                return Err(ShellError::State);
            }
            let conn = self.conn.clone();
            self.main.present_rearm(id, &conn);
            return self.flush();
        }
        let surface = self.main.surfaces.get(&id).ok_or(ShellError::State)?;
        surface.attach(None, 0, 0);
        surface.commit();
        self.flush()
    }
}

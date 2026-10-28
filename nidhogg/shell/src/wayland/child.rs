//! Child windows: synchronised `wl_subsurface`s of another window's surface, and the requests every window kind
//! shares (input region, mapped).
//!
//! A child has its own buffers but no pacer: it is presented with its tree (the root's pacer, see `present`).  A sync
//! child's commit is cached until its parent commits, so every change of a child (position, stacking, input region,
//! unmap) is recorded, makes each ancestor up to the root owe a commit, and goes out with the tree's commits, deepest
//! first.  Its position
//! and stacking are the parent's state (Wayland's rule); Smithay keeps the position in the child's own cached state, so
//! a move makes the child owe a commit too.

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
        // sync (the default): the child shows with its parent's commit
        let subsurface = sub.get_subsurface(&surface, &parent, &qh, ());
        subsurface.set_position(d.x, d.y);
        m.attach_scale(&surface, &qh);
        m.surfaces.insert(id, surface.clone());
        m.children.insert(
            id,
            Child {
                surface,
                subsurface: subsurface.clone(),
                parent: d.parent,
                x: d.x,
                y: d.y,
            },
        );
        m.present_register(id, None, Some(subsurface));
        self.flush()?;
        Ok(id)
    }

    pub(super) fn child_destroy(&mut self, id: WindowId) -> ShellResult {
        if !self.main.children.contains_key(&id) {
            return Err(ShellError::State);
        }
        self.present_retire(id, |m| {
            m.surfaces.remove(&id);
            m.detach_scale(id);
            if let Some(c) = m.children.remove(&id) {
                c.subsurface.destroy();
                c.surface.destroy();
            }
        });
        self.flush()
    }

    /// Windows whose parent is `id` (children and popups), for destroying them with it.
    pub(super) fn dependents_of(&self, id: WindowId) -> Vec<WindowId> {
        let kids = self.main.children.iter().filter(|(_, c)| c.parent == id);
        let pops = self.main.popups.iter().filter(|(_, p)| p.parent == id);
        kids.map(|(k, _)| *k).chain(pops.map(|(k, _)| *k)).collect()
    }

    pub(super) fn window_set_position(
        &mut self,
        id: WindowId,
        x: i32,
        y: i32,
    ) -> ShellResult<bool> {
        if let Some(c) = self.main.children.get_mut(&id) {
            c.x = x;
            c.y = y;
            self.main
                .present_record(id, |p| p.rec.position = Some((x, y)));
            // The protocol applies the position with the parent's commit; Smithay with the child's own (cached until
            // the parent's): the child and every ancestor owe a commit, made with the tree's next present.
            self.main.present_owe(id, true, true);
            return Ok(false);
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
        let parent = c.parent;
        let sibling = if sibling == 0 { parent } else { sibling };
        let other = self.main.surfaces.get(&sibling).ok_or(ShellError::Arg)?;
        let entry = (c.subsurface.clone(), other.clone(), above);
        // the parent's state, sent with its commit: the parent and every ancestor above it owe a commit
        self.main
            .present_record(parent, |p| p.rec.stack.push(entry));
        self.main.present_owe(id, false, true);
        Ok(())
    }

    pub(super) fn window_set_input_region(&mut self, id: WindowId, rects: &[Rect]) -> ShellResult {
        if !self.main.surfaces.contains_key(&id) {
            return Err(ShellError::State);
        }
        // the region is double-buffered: it counts from the surface's next commit (a child's: once its ancestors
        // have committed after it), which the window now owes
        self.main
            .present_record(id, |p| p.rec.input = Some(rects.to_vec()));
        self.main.present_owe(id, true, true);
        Ok(())
    }

    pub(super) fn window_set_mapped(&mut self, id: WindowId, on: bool) -> ShellResult {
        if !self.main.surfaces.contains_key(&id) {
            return Err(ShellError::State);
        }
        if on {
            // an unmap not sent yet is taken back
            self.main.present_record(id, |p| p.rec.unmap = false);
            // a child has no pacer to re-arm: its next buffer goes out with its tree
            if self.main.children.contains_key(&id) {
                return Ok(());
            }
            // a frame callback the compositor dropped while the window was hidden must not hold the pacer back: a
            // waiting pacer asks again (frame callback and an empty commit)
            self.main.present_rearm(id);
            return Ok(());
        }
        // a null buffer with the window's next commit (a child's applies once its ancestors have committed after it)
        self.main.present_record(id, |p| p.rec.unmap = true);
        self.main.present_owe(id, true, true);
        Ok(())
    }
}

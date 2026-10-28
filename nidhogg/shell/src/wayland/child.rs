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
use crate::wayland::{MainState, WaylandShell};
use glitnir::geom::Rect;
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

impl MainState {
    /// Child `id` of `parent` leaves the stacking order (and its own children's list goes with it).
    fn unstack(&mut self, parent: WindowId, id: WindowId) {
        if let Some(order) = self.stacking.get_mut(&parent) {
            order.retain(|&k| k != id);
            if order.len() <= 1 {
                self.stacking.remove(&parent);
            }
        }
        self.stacking.remove(&id);
    }
}

impl WaylandShell {
    pub(super) fn child_create(&mut self, d: &ChildDesc) -> ShellResult<WindowId> {
        let qh = self.main_queue.handle();
        let m = &mut self.main;
        let parent = m.surfaces.get(&d.parent).ok_or(ShellError::State)?.clone();
        if m.subcompositor.is_none() {
            return Err(ShellError::State);
        }
        let (surface, id) = m.window_surface(&qh)?;
        let sub = m.subcompositor.as_ref().ok_or(ShellError::State)?;
        // sync (the default): the child shows with its parent's commit
        let subsurface = sub.get_subsurface(&surface, &parent, &qh, ());
        subsurface.set_position(d.x, d.y);
        m.attach_scale(&surface, &qh);
        // a new subsurface goes on top of its parent's stack
        m.stacking
            .entry(d.parent)
            .or_insert_with(|| vec![d.parent])
            .push(id);
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
                m.unstack(c.parent, id);
                c.subsurface.destroy();
                c.surface.destroy();
            }
        });
        self.flush()
    }

    /// Make child `id` again on the new connection (a reconnect): a surface that keeps the id, a sync subsurface of
    /// its parent's new surface at its offset, and its scale state.  Its parent was made again before it.
    fn child_again(&mut self, id: WindowId) -> ShellResult {
        let qh = self.main_queue.handle();
        let m = &mut self.main;
        let c = m.children.get(&id).ok_or(ShellError::State)?;
        let (parent, x, y) = (c.parent, c.x, c.y);
        let parent = m.surfaces.get(&parent).ok_or(ShellError::State)?.clone();
        let surface = m.window_surface_again(id, &qh).ok_or(ShellError::State)?;
        let sub = m.subcompositor.as_ref().ok_or(ShellError::State)?;
        let subsurface = sub.get_subsurface(&surface, &parent, &qh, ());
        subsurface.set_position(x, y);
        m.reattach_scale(&surface, &qh);
        m.surfaces.insert(id, surface.clone());
        if let Some(c) = m.children.get_mut(&id) {
            c.surface = surface;
            c.subsurface = subsurface.clone();
        }
        m.present_again(id, None, Some(subsurface), &qh);
        Ok(())
    }

    /// Make every child window again (a reconnect), parents first, and their stacking with them: a new subsurface
    /// goes on top, so each parent's children are made bottom to top, and those below the parent are placed below it
    /// (recorded on the parent, sent with its commit).
    pub(super) fn children_again(&mut self) {
        let mut roots: Vec<WindowId> = self.main.windows.keys().copied().collect();
        roots.sort_unstable();
        let mut parents: std::collections::VecDeque<WindowId> = roots.into();
        while let Some(p) = parents.pop_front() {
            let Some(order) = self.main.stacking.get(&p).cloned() else {
                continue;
            };
            for &k in order.iter().filter(|&&k| k != p) {
                if self.child_again(k).is_ok() {
                    parents.push_back(k);
                }
            }
            let at = order.iter().position(|&k| k == p).unwrap_or(0);
            let mut over = p;
            for &k in order[..at].iter().rev() {
                let (Some(c), Some(above)) =
                    (self.main.children.get(&k), self.main.surfaces.get(&over))
                else {
                    continue;
                };
                let entry = (c.subsurface.clone(), above.clone(), false);
                self.main.present_record(p, |r| r.rec.stack.push(entry));
                over = k;
            }
        }
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
        if let Some(order) = self.main.stacking.get_mut(&parent)
            && order.contains(&sibling)
        {
            order.retain(|&k| k != id);
            let at = order.iter().position(|&k| k == sibling).unwrap_or(0);
            order.insert(if above { at + 1 } else { at }, id);
        }
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

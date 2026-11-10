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
use crate::wayland::publish::Mail;
use crate::wayland::{MainState, WaylandShell};
use glitnir::geom::Rect;
use glitnir::motion::MotionSpec;
use wayland_client::protocol::wl_subsurface::WlSubsurface;
use wayland_client::protocol::wl_surface::WlSurface;

pub struct Child {
    pub surface: WlSurface,
    subsurface: WlSubsurface,
    pub parent: WindowId,
    /// The offset in the parent, logical pixels.
    pub x: i32,
    pub y: i32,
    /// The host has it shown (`set_mapped`), and it has transitions (`set_motion`): whoever presents its tree then
    /// decides when it maps and unmaps.
    mapped: bool,
    motion: bool,
    /// Its backdrop's σ in logical pixels, 0 for none (`set_backdrop`): presenting follows its position, and a
    /// snapshot draws it.
    pub blur_px: f32,
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

    /// Append `id` and the child windows under it to `out` bottom to top, a parent's place among its children kept.
    fn paint_order(&self, id: WindowId, out: &mut Vec<WindowId>) {
        match self.stacking.get(&id) {
            Some(order) => order.iter().for_each(|&k| {
                if k == id {
                    out.push(id);
                } else {
                    self.paint_order(k, out);
                }
            }),
            None => out.push(id),
        }
    }

    /// The child windows under `id` as stacked, bottom to top.
    pub(crate) fn stacked_under(&self, id: WindowId) -> Vec<WindowId> {
        let mut out = Vec::new();
        self.paint_order(id, &mut out);
        out.retain(|&k| k != id);
        out
    }

    /// Root `root`, its child windows and the popups hanging from them, each with its own: what shows, bottom to top.
    /// A popup is above every window of the tree it hangs from, and above the popups made before it, in the order the
    /// shell gave the compositor (ids are used again: a popup's id may be lower than its parent's).  A popup's parent
    /// was made before it, so it is listed by the time the popup comes.
    pub(crate) fn painted(&self, root: WindowId) -> Vec<WindowId> {
        let mut out = Vec::new();
        self.paint_order(root, &mut out);
        let mut popups: Vec<_> = self
            .popups
            .iter()
            .map(|(&id, p)| (p.order, id, p.parent))
            .collect();
        popups.sort_unstable();
        for (_, id, parent) in popups {
            if out.contains(&parent) {
                self.paint_order(id, &mut out);
            }
        }
        out
    }

    /// Where window `id`'s surface starts in its tree's root, logical pixels: a child at its offsets, a popup where the
    /// compositor placed it in the window geometry of the window it hangs on.
    pub(crate) fn origin(&self, id: WindowId) -> (i32, i32) {
        if let Some(c) = self.children.get(&id) {
            let (x, y) = self.origin(c.parent);
            return (x + c.x, y + c.y);
        }
        let Some(p) = self.popups.get(&id) else {
            return (0, 0);
        };
        let (x, y) = self.origin(p.base_window);
        let (gx, gy) = self.present_geometry_origin(p.base_window);
        (x + gx + p.placed.0, y + gy + p.placed.1)
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
        m.inherit_scale(id, d.parent);
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
                mapped: true,
                motion: false,
                blur_px: 0.0,
            },
        );
        m.present_register(id, None, Some(subsurface));
        self.flush()?;
        Ok(id)
    }

    pub(super) fn child_destroy(&mut self, id: WindowId) -> ShellResult {
        let Some(c) = self.main.children.get(&id) else {
            return Err(ShellError::State);
        };
        let fading = c.motion
            && self.main.present_hiding(id)
            && self.published_front(id)
            && self.live().is_ok();
        if fading {
            // its id and records go now; its surface, viewport and buffers go when the leg ends
            self.present_ghost(id, |m| {
                m.surfaces.remove(&id);
                m.detach_scale_keeping_viewport(id);
                if let Some(c) = m.children.remove(&id) {
                    m.unstack(c.parent, id);
                }
            });
            return self.flush();
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
            if c.blur_px > 0.0 {
                self.backdrop_placed(id, (x, y));
            }
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

    /// `ShellBackend::set_child_place`: recorded and owed as `window_set_position` and `window_set_input_region` do,
    /// with no wake.
    pub(super) fn window_set_child_place(
        &mut self,
        id: WindowId,
        pos: Option<(i32, i32)>,
        input: Option<&[Rect]>,
    ) -> ShellResult {
        let c = self.main.children.get_mut(&id).ok_or(ShellError::State)?;
        if let Some((x, y)) = pos {
            (c.x, c.y) = (x, y);
        }
        if pos.is_none() && input.is_none() {
            return Ok(());
        }
        self.main.present_record(id, |p| {
            if pos.is_some() {
                p.rec.position = pos;
            }
            if let Some(rects) = input {
                p.rec.input = Some(rects.to_vec());
            }
        });
        self.main.present_owe(id, true, false);
        Ok(())
    }

    pub(super) fn window_set_mapped(&mut self, id: WindowId, on: bool) -> ShellResult {
        if !self.main.surfaces.contains_key(&id) {
            return Err(ShellError::State);
        }
        if let Some(c) = self.main.children.get_mut(&id) {
            c.mapped = on;
            if c.motion {
                // the surface stays as it is: the tree's commits start the leg, and the end of a hide unmaps
                self.main.present_record(id, |p| {
                    p.rec.unmap = false;
                    p.hiding = !on;
                });
                let root = self.main.tree_root(id);
                self.motion_post(root, Mail::Mapped(id, on));
                return Ok(());
            }
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

    /// Child `id`'s transitions.  `None` ends a leg that runs, on the stage, which unmaps a hide.
    pub(super) fn window_set_motion(
        &mut self,
        id: WindowId,
        spec: Option<MotionSpec>,
    ) -> ShellResult {
        let c = self.main.children.get_mut(&id).ok_or(ShellError::State)?;
        if !c.motion && spec.is_none() {
            return Ok(());
        }
        c.motion = spec.is_some();
        let mapped = c.mapped;
        if spec.is_none() {
            self.main.present_record(id, |p| p.hiding = false);
        }
        let root = self.main.tree_root(id);
        self.motion_post(root, Mail::Motion(id, spec, mapped));
        Ok(())
    }

    /// Child `id`'s backdrop: σ `blur_px` logical pixels, 0 for none.
    pub(super) fn window_set_backdrop(&mut self, id: WindowId, blur_px: f32) -> ShellResult {
        if !blur_px.is_finite() || blur_px < 0.0 {
            return Err(ShellError::Arg);
        }
        let c = self.main.children.get_mut(&id).ok_or(ShellError::State)?;
        if c.blur_px == 0.0 && blur_px == 0.0 {
            return Ok(());
        }
        c.blur_px = blur_px;
        let (parent, at) = (c.parent, (c.x, c.y));
        let root = self.main.tree_root(id);
        self.motion_post(root, Mail::Backdrop(id, parent, blur_px, at));
        Ok(())
    }
}

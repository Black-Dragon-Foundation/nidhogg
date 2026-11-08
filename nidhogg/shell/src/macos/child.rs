//! Child windows as layers (design §5.6): a child is a container layer in its parent's layer (the root's, or another
//! child's container), holding a shadow sublayer (empty until Glitnir's chrome moves to Core Animation), and a content
//! layer that shows the child's own ring of surfaces.
//!
//! Every change the host asks for (position, input region, stacking, mapping, alpha, a new or a gone child) is recorded
//! and makes the child's root owe a commit; the layers take it in the root tree's next commit, one flushed transaction
//! with the tree's pictures, so a move sets the container's position and nothing else.  The commit comes with the
//! tree's next publish, or at the main thread's next pump when none comes.
//!
//! A root's child whose content crosses the root's bounds moves into an overhang panel (`overhang`) and back once it
//! fits (`crate::mac_child::overhangs`), with no redraw: the commit orders the panel in, showing the child's surface,
//! before the transaction that moves the container into it, and orders it out after the one that moves it back.
//!
//! Geometry: AppKit's layers have their origin at the bottom left, the host's places are from the top left, so a
//! container's position is its parent's height less its bottom, and a parent's resize moves its children's layers.

use super::super::layers::{self, PROPS, count};
use super::super::overhang::Panel;
use super::super::surface::Surfaces;
use super::MacShell;
use crate::api::{ChildDesc, NHSHELL_KIND_CHILD, ShellError, ShellResult, WindowId};
use crate::event::{PointerEv, ShellEvent};
use crate::mac_child::{self, Node};
use glitnir::geom::Rect;
use objc2::rc::Retained;
use objc2_foundation::{NSPoint, NSRect, NSSize};
use objc2_quartz_core::CALayer;

/// Where a child's container is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum Host {
    /// In no layer yet.
    #[default]
    None,
    /// In its parent's layer.
    Parent,
    /// In its overhang panel's layer.
    Panel,
}

/// What a child's layers show now, as last set.
#[derive(Debug, Default)]
struct Shown {
    position: Option<(f64, f64)>,
    size: Option<(i32, i32)>,
    hidden: Option<bool>,
    opacity: Option<f32>,
    host: Host,
}

/// A child's layers, made by its root's first commit after it (on the main thread).
struct Layers {
    container: Retained<CALayer>,
    _shadow: Retained<CALayer>,
    content: Retained<CALayer>,
}

impl Layers {
    fn new() -> Layers {
        let container = origin_layer();
        let shadow = origin_layer();
        shadow.setHidden(true);
        let content = origin_layer();
        container.addSublayer(&shadow);
        container.addSublayer(&content);
        // nothing shows before the first picture
        container.setHidden(true);
        Layers {
            container,
            _shadow: shadow,
            content,
        }
    }
}

pub(super) struct Kid {
    pub parent: WindowId,
    layers: Option<Layers>,
    pub surfaces: Surfaces,
    /// The surface's place in its parent's surface and its size, points.
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    /// The input region in its own surface, points (`None`: all of it).
    input: Option<Rect>,
    mapped: bool,
    alpha: f32,
    shown: Shown,
    panel: Option<Panel>,
    /// It overhangs (its container is, or goes, in the panel).
    out: bool,
}

/// An anchor-point-at-the-origin layer, so a container's position is its bottom-left corner.
fn origin_layer() -> Retained<CALayer> {
    let l = CALayer::new();
    l.setAnchorPoint(NSPoint::new(0.0, 0.0));
    l.setPosition(NSPoint::new(0.0, 0.0));
    l
}

impl Kid {
    fn new(parent: WindowId, d: &ChildDesc, srgb: bool) -> Kid {
        let mut surfaces = Surfaces::default();
        surfaces.reset(srgb);
        Kid {
            parent,
            layers: None,
            surfaces,
            x: d.x,
            y: d.y,
            w: d.w.max(1),
            h: d.h.max(1),
            input: None,
            mapped: true,
            alpha: 1.0,
            shown: Shown {
                hidden: Some(true),
                ..Shown::default()
            },
            panel: None,
            out: false,
        }
    }

    fn node(&self, id: WindowId) -> Node {
        Node {
            id,
            parent: self.parent,
            x: self.x,
            y: self.y,
            w: self.w,
            h: self.h,
            input: self.input,
            shown: self.visible(),
        }
    }

    /// It shows: mapped, with a picture.
    fn visible(&self) -> bool {
        self.mapped && self.surfaces.newest().is_some()
    }
}

/// A rectangle of root `w`'s content (points from its top left) on the screen (AppKit's points, from the bottom left).
fn on_screen(w: &super::super::window::Win, x: i32, y: i32, cw: i32, ch: i32) -> NSRect {
    let c = w.window.contentRectForFrameRect(w.window.frame());
    NSRect::new(
        NSPoint::new(
            c.origin.x + f64::from(x),
            c.origin.y + c.size.height - f64::from(y) - f64::from(ch),
        ),
        NSSize::new(f64::from(cw.max(1)), f64::from(ch.max(1))),
    )
}

impl MacShell {
    /// The root of `id`'s tree (`id` itself for a root), `None` for a window the shell does not have.
    pub(super) fn root_of(&self, id: WindowId) -> Option<WindowId> {
        let mut at = id;
        for _ in 0..=self.kids.len() {
            if self.windows.contains_key(&at) {
                return Some(at);
            }
            at = self.kids.get(&at)?.parent;
        }
        None
    }

    /// A top-level or a child the shell has.
    pub(super) fn known(&self, id: WindowId) -> ShellResult {
        if self.windows.contains_key(&id) || self.kids.contains_key(&id) {
            Ok(())
        } else {
            Err(ShellError::State)
        }
    }

    /// The surfaces of window `id`, a top-level's or a child's.
    pub(super) fn surfaces_mut(&mut self, id: WindowId) -> Option<&mut Surfaces> {
        match self.windows.get_mut(&id) {
            Some(w) => Some(&mut w.surfaces),
            None => self.kids.get_mut(&id).map(|k| &mut k.surfaces),
        }
    }

    /// Root `root`'s tree owes a commit: the next publish or pump makes it.
    fn owe(&mut self, root: WindowId) {
        if self.owed.insert(root) {
            crate::wait::wake();
        }
    }

    fn owe_for(&mut self, id: WindowId) {
        if let Some(r) = self.root_of(id) {
            self.owe(r);
        }
    }

    pub(super) fn child_create(&mut self, d: &ChildDesc) -> ShellResult<WindowId> {
        if d.kind != NHSHELL_KIND_CHILD {
            return Err(ShellError::Unsupported);
        }
        if d.parent == 0 || self.root_of(d.parent).is_none() {
            return Err(ShellError::State);
        }
        let id = self.next_id;
        self.next_id += 1;
        self.kids.insert(id, Kid::new(d.parent, d, self.srgb));
        // a new child goes on top of its siblings
        self.order.entry(d.parent).or_default().push(id);
        Ok(id)
    }

    /// Child `id` and every child under it go; their containers leave the layers in the root's next commit.
    pub(super) fn child_destroy(&mut self, id: WindowId) -> ShellResult {
        let root = self.root_of(id).ok_or(ShellError::State)?;
        let k = self.kids.remove(&id).ok_or(ShellError::State)?;
        if let Some(o) = self.order.get_mut(&k.parent) {
            o.retain(|&c| c != id);
        }
        let under = self.order.remove(&id).unwrap_or_default();
        self.retired.push((root, k));
        for c in under {
            let _ = self.child_destroy(c);
        }
        self.hover.retain(|_, t| *t != id);
        if self.grab.is_some_and(|(_, t)| t == id) {
            self.grab = None;
        }
        self.owe(root);
        Ok(())
    }

    /// A top-level goes: its children with it.
    pub(super) fn children_gone(&mut self, root: WindowId) {
        for c in self.order.remove(&root).unwrap_or_default() {
            let _ = self.child_destroy(c);
        }
        self.retired.retain(|(r, _)| *r != root);
        self.owed.remove(&root);
        self.hover.remove(&root);
    }

    pub(super) fn kid_position(&mut self, id: WindowId, x: i32, y: i32) -> ShellResult<bool> {
        let k = self.kids.get_mut(&id).ok_or(ShellError::State)?;
        if (k.x, k.y) != (x, y) {
            (k.x, k.y) = (x, y);
            self.owe_for(id);
        }
        Ok(false)
    }

    pub(super) fn kid_input(&mut self, id: WindowId, rects: &[Rect]) -> ShellResult {
        let k = self.kids.get_mut(&id).ok_or(ShellError::State)?;
        // the shell takes the rectangles' hull: the host gives one, the native area
        let hull = rects.iter().copied().reduce(|a, b| a.bounds(&b));
        if k.input != hull {
            k.input = hull;
            self.owe_for(id);
        }
        Ok(())
    }

    pub(super) fn kid_place(
        &mut self,
        id: WindowId,
        pos: Option<(i32, i32)>,
        input: Option<&[Rect]>,
    ) -> ShellResult {
        if !self.kids.contains_key(&id) {
            return Err(ShellError::State);
        }
        if let Some((x, y)) = pos {
            self.kid_position(id, x, y)?;
        }
        if let Some(r) = input {
            self.kid_input(id, r)?;
        }
        Ok(())
    }

    pub(super) fn kid_stack(
        &mut self,
        id: WindowId,
        sibling: WindowId,
        above: bool,
    ) -> ShellResult {
        let parent = self.kids.get(&id).ok_or(ShellError::State)?.parent;
        let o = self.order.entry(parent).or_default();
        o.retain(|&c| c != id);
        let at = match o.iter().position(|&c| c == sibling) {
            Some(i) if above => i + 1,
            Some(i) => i,
            // its parent: a child above it is the lowest of its children, one below it is not one the shell has
            None if sibling == 0 || sibling == parent => 0,
            None => return Err(ShellError::Arg),
        };
        o.insert(at, id);
        self.order_dirty.insert(parent);
        self.owe_for(id);
        Ok(())
    }

    pub(super) fn kid_mapped(&mut self, id: WindowId, on: bool) -> ShellResult {
        let k = self.kids.get_mut(&id).ok_or(ShellError::State)?;
        if k.mapped != on {
            k.mapped = on;
            self.owe_for(id);
        }
        Ok(())
    }

    pub(super) fn kid_alpha(&mut self, id: WindowId, factor: u32) -> ShellResult {
        let k = self.kids.get_mut(&id).ok_or(ShellError::State)?;
        let a = (f64::from(factor) / f64::from(u32::MAX)) as f32;
        if k.alpha != a {
            k.alpha = a;
            self.owe_for(id);
        }
        Ok(())
    }

    /// Root `root`'s children as the hit-test sees them, each parent's bottom to top.
    fn nodes(&self, root: WindowId) -> Vec<Node> {
        let mut out = Vec::new();
        let mut todo = vec![root];
        while let Some(p) = todo.pop() {
            for &c in self.order.get(&p).into_iter().flatten() {
                if let Some(k) = self.kids.get(&c) {
                    out.push(k.node(c));
                    todo.push(c);
                }
            }
        }
        out
    }

    /// Root `root`'s children, each parent before its own.
    fn tree(&self, root: WindowId) -> Vec<WindowId> {
        self.nodes(root).iter().map(|n| n.id).collect()
    }

    /// Root `root`'s children as stacked, bottom to top, hidden ones too, each child's own right after it.
    pub(super) fn stacking(&self, root: WindowId) -> Vec<WindowId> {
        fn walk(s: &MacShell, p: WindowId, out: &mut Vec<WindowId>, depth: usize) {
            if depth > s.kids.len() {
                return;
            }
            for &c in s.order.get(&p).into_iter().flatten() {
                out.push(c);
                walk(s, c, out, depth + 1);
            }
        }
        let mut out = Vec::new();
        walk(self, root, &mut out, 0);
        out
    }

    /// Commit root `root`'s tree (main thread): the pictures of `drawn` (its windows that drew), and every change its
    /// children owe, in one flushed transaction; the overhang panels ordered in before it and out after it.
    pub(super) fn commit_tree(&mut self, root: WindowId, drawn: &[WindowId]) {
        let Some(w) = self.windows.get_mut(&root) else {
            return;
        };
        if drawn.contains(&root) && w.surfaces.newest().is_some() {
            let (sw, sh) = w.surfaces.size();
            if w.follow_surface(sw, sh) {
                layers::changed_outside();
                let size = w.size;
                crate::trace::note(|| {
                    format!("window {root} takes its surface's size: {size:?} points")
                });
            }
        }
        let scale = w.window.backingScaleFactor();
        let (rw, rh) = w.size;
        let tree = self.tree(root);
        // which of the root's own children overhang now
        let mut going_out = Vec::new();
        let mut going_back = Vec::new();
        for &id in self.order.get(&root).into_iter().flatten() {
            let Some(k) = self.kids.get(&id) else {
                continue;
            };
            let c = k.node(id).content();
            let now = k.visible()
                && mac_child::overhangs(Rect::new(k.x + c.x, k.y + c.y, c.w, c.h), rw, rh, k.out);
            match (k.out, now) {
                (false, true) => going_out.push(id),
                (true, false) => going_back.push(id),
                _ => {}
            }
        }
        // going out: the panel shows the child's surface, and is ordered in, before its container leaves the root
        if !going_out.is_empty() {
            let MacShell {
                mtm, windows, kids, ..
            } = self;
            let (mtm, w) = (*mtm, &windows[&root]);
            layers::transaction(|| {
                for id in &going_out {
                    let Some(k) = kids.get_mut(id) else { continue };
                    let frame = on_screen(w, k.x, k.y, k.w, k.h);
                    let p = k.panel.get_or_insert_with(|| Panel::new(mtm, *id, frame));
                    p.place(frame);
                    if let Some(s) = k.surfaces.newest() {
                        layers::show(&p.layer, s, false, scale);
                    }
                }
            });
            for id in &going_out {
                if let Some(p) = kids.get_mut(id).and_then(|k| k.panel.as_mut()) {
                    p.order_in(&w.window);
                    count(&PROPS.panel_orders);
                }
                if let Some(k) = kids.get_mut(id) {
                    k.out = true;
                }
                count(&PROPS.switches);
            }
            #[cfg(feature = "test-hooks")]
            self.capture_point(root, 0);
        }
        for id in &going_back {
            if let Some(k) = self.kids.get_mut(id) {
                k.out = false;
            }
            count(&PROPS.switches);
        }
        let order_dirty: Vec<WindowId> = self
            .order_dirty
            .iter()
            .copied()
            .filter(|p| *p == root || tree.contains(p))
            .collect();
        let MacShell {
            windows,
            kids,
            order,
            retired,
            ..
        } = self;
        let w = &windows[&root];
        layers::transaction(|| {
            if drawn.contains(&root)
                && let Some(s) = w.surfaces.newest()
            {
                layers::show(&w.layer, s, w.surfaces.was_same(), scale);
            }
            for (r, k) in retired.iter() {
                if *r == root
                    && let Some(l) = &k.layers
                {
                    l.container.removeFromSuperlayer();
                    count(&PROPS.host);
                }
            }
            for &id in &tree {
                if let Some(k) = kids.get_mut(&id)
                    && k.layers.is_none()
                {
                    k.layers = Some(Layers::new());
                }
            }
            for &id in &tree {
                let Some(k) = kids.get(&id) else { continue };
                let (parent_layer, parent_h) = match kids.get(&k.parent) {
                    Some(p) => match &p.layers {
                        Some(l) => (l.container.clone(), p.h),
                        None => continue,
                    },
                    None => (w.layer.clone(), rh),
                };
                let k = kids.get_mut(&id).expect("listed above");
                let Some(l) = &k.layers else { continue };
                let (container, content) = (l.container.clone(), l.content.clone());
                if drawn.contains(&id)
                    && let Some(s) = k.surfaces.newest()
                {
                    layers::show(&content, s, k.surfaces.was_same(), scale);
                    let (sw, sh) = k.surfaces.size();
                    (k.w, k.h) = (
                        (f64::from(sw) / scale).round() as i32,
                        (f64::from(sh) / scale).round() as i32,
                    );
                }
                let host = if k.out { Host::Panel } else { Host::Parent };
                if k.shown.host != host {
                    match (host, k.panel.as_ref()) {
                        (Host::Panel, Some(p)) => {
                            p.layer.addSublayer(&container);
                            // SAFETY: nil contents: the container shows the child now.
                            unsafe { p.layer.setContents(None) };
                        }
                        _ => {
                            if let (Some(p), Some(s)) = (k.panel.as_ref(), k.surfaces.newest()) {
                                // the panel keeps showing the child until it is ordered out
                                layers::show(&p.layer, s, false, scale);
                            }
                            parent_layer.addSublayer(&container);
                        }
                    }
                    k.shown.host = host;
                    count(&PROPS.host);
                }
                let position = match host {
                    Host::Panel => (0.0, 0.0),
                    _ => (f64::from(k.x), f64::from(parent_h - k.y - k.h)),
                };
                if k.shown.position != Some(position) {
                    container.setPosition(NSPoint::new(position.0, position.1));
                    k.shown.position = Some(position);
                    count(&PROPS.position);
                }
                if k.shown.size != Some((k.w, k.h)) {
                    let b = NSRect::new(
                        NSPoint::new(0.0, 0.0),
                        NSSize::new(f64::from(k.w), f64::from(k.h)),
                    );
                    container.setBounds(b);
                    content.setBounds(b);
                    k.shown.size = Some((k.w, k.h));
                    count(&PROPS.bounds);
                }
                let hidden = !k.visible();
                if k.shown.hidden != Some(hidden) {
                    container.setHidden(hidden);
                    k.shown.hidden = Some(hidden);
                    count(&PROPS.hidden);
                }
                if k.shown.opacity != Some(k.alpha) {
                    container.setOpacity(k.alpha);
                    k.shown.opacity = Some(k.alpha);
                    count(&PROPS.opacity);
                }
            }
            // a parent whose children changed places takes them again, bottom to top
            for p in &order_dirty {
                let layer = match kids.get(p) {
                    Some(k) => match &k.layers {
                        Some(l) => l.container.clone(),
                        None => continue,
                    },
                    None => w.layer.clone(),
                };
                for c in order.get(p).into_iter().flatten() {
                    if let Some(k) = kids.get(c)
                        && k.shown.host == Host::Parent
                        && let Some(l) = &k.layers
                    {
                        layer.addSublayer(&l.container);
                        count(&PROPS.order);
                    }
                }
            }
        });
        retired.retain(|(r, _)| *r != root);
        for p in &order_dirty {
            self.order_dirty.remove(p);
        }
        // the panels follow their children; going back, the panel is ordered out after the container is home
        let w = &self.windows[&root];
        for &id in self.order.get(&root).into_iter().flatten() {
            let Some(k) = self.kids.get_mut(&id) else {
                continue;
            };
            let frame = on_screen(w, k.x, k.y, k.w, k.h);
            if let Some(p) = k.panel.as_mut() {
                if k.out {
                    p.place(frame);
                } else if p.shown {
                    p.order_out(&w.window);
                    count(&PROPS.panel_orders);
                }
            }
        }
        #[cfg(feature = "test-hooks")]
        if !going_out.is_empty() {
            self.capture_point(root, 1);
        }
    }

    /// A pointer record of window `id`'s view (a root's, or an overhang panel's), in that view's device pixels: the
    /// window under it takes it, in its own surface's device pixels.  Leaving one window for another gives a leave and
    /// an enter; a press holds the pointer on its window until the release.
    pub(super) fn pointer(&mut self, id: WindowId, ev: ShellEvent) {
        let Some(root) = self.root_of(id) else {
            return;
        };
        let Some(scale) = self
            .windows
            .get(&root)
            .map(|w| w.window.backingScaleFactor())
        else {
            return;
        };
        let nodes = self.nodes(root);
        // a panel's view: its point in the root
        let (ox, oy) = if id == root {
            (0.0, 0.0)
        } else {
            let (x, y) = mac_child::origin(&nodes, id);
            (f64::from(x) * scale, f64::from(y) * scale)
        };
        let p = match ev {
            ShellEvent::PointerMotion(p) | ShellEvent::Button(p) => p,
            ShellEvent::Scroll(_) => {
                let t = self.hover.get(&root).copied().unwrap_or(root);
                self.push(ev, t);
                return;
            }
            _ => return,
        };
        let (rx, ry) = (p.x + ox, p.y + oy);
        let (target, lx, ly) = match self.grab {
            Some((r, t)) if r == root => {
                let (x, y) = mac_child::origin(&nodes, t);
                (t, rx - f64::from(x) * scale, ry - f64::from(y) * scale)
            }
            _ => {
                let (t, x, y) = mac_child::hit(root, &nodes, rx / scale, ry / scale);
                (t, x * scale, y * scale)
            }
        };
        let old = self.hover.insert(root, target);
        if old.is_some_and(|o| o != target) {
            let o = old.unwrap_or(root);
            self.push(ShellEvent::PointerLeave(PointerEv::default()), o);
            let enter = PointerEv {
                x: lx,
                y: ly,
                ..PointerEv::default()
            };
            self.push(ShellEvent::PointerEnter(enter), target);
        }
        let moved = PointerEv { x: lx, y: ly, ..p };
        match ev {
            ShellEvent::Button(_) => {
                self.grab =
                    (p.state == crate::event::NHSHELL_BUTTON_PRESSED).then_some((root, target));
                self.push(ShellEvent::Button(moved), target);
            }
            _ => self.push(ShellEvent::PointerMotion(moved), target),
        }
    }
}

#[cfg(feature = "test-hooks")]
impl MacShell {
    /// `test-child` (WINDOW): child WINDOW's layers as a Lisp plist: its container's position (points, from the
    /// parent's bottom left) and size, where it is (`parent`, `panel` or nil), hidden, opacity, its index among its
    /// layer's sublayers, whether its content shows its newest surface, and its panel's screen frame.
    pub(super) fn child_info(&self, id: WindowId) -> ShellResult<String> {
        let k = self.kids.get(&id).ok_or(ShellError::State)?;
        let l = k.layers.as_ref().ok_or(ShellError::State)?;
        let p = l.container.position();
        let b = l.container.bounds();
        // SAFETY: `contents` is an object or nil; we only compare its address.
        let contents = unsafe { l.content.contents() };
        let newest = match (contents, k.surfaces.newest()) {
            (Some(c), Some(n)) => {
                std::ptr::eq(Retained::as_ptr(&c).cast::<std::ffi::c_void>(), n.as_ptr())
            }
            _ => false,
        };
        let index = l.container.superlayer().map_or(-1, |s| {
            // SAFETY: the sublayers array is read, not changed.
            unsafe { s.sublayers() }.map_or(-1, |a| {
                a.iter()
                    .position(|s| std::ptr::eq(&*s, &*l.container))
                    .map_or(-1, |i| i as i64)
            })
        });
        let host = match k.shown.host {
            Host::None => "nil",
            Host::Parent => "parent",
            Host::Panel => "panel",
        };
        let panel = k.panel.as_ref().filter(|p| p.shown).map_or_else(
            || "nil".to_string(),
            |p| {
                let f = p.panel.frame();
                format!(
                    "({} {} {} {})",
                    f.origin.x, f.origin.y, f.size.width, f.size.height
                )
            },
        );
        let input = k.input.map_or_else(
            || "nil".to_string(),
            |r| format!("({} {} {} {})", r.x, r.y, r.w, r.h),
        );
        Ok(format!(
            "(:x {} :y {} :w {} :h {} :host {host} :hidden {} :opacity {} :index {index} :contents-is-newest {} \
             :panel {panel} :input {input} :place ({} {}))",
            p.x,
            p.y,
            b.size.width,
            b.size.height,
            if l.container.isHidden() { "t" } else { "nil" },
            l.container.opacity(),
            if newest { "t" } else { "nil" },
            k.x,
            k.y,
        ))
    }
}

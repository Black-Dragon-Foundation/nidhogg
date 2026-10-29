//! The scene: one root's layers as a tree, their stacking, and the commit that turns changes into damage.
//!
//! A root's scene holds frame layers (the root's own surface), window layers (a window's rectangle on its frame's
//! surface) and child nodes (a child frame's surface, with its windows and nested children under it). Every
//! rectangle is in the root's device pixels. The host changes the scene at any time; a commit compares it with the
//! last committed scene and returns the scene damage:
//!
//! - each frame's (or child frame's) own damage, clipped to its layer and moved into the root's coordinates;
//! - a scroll operation: its window layer's rectangle (P1 composes a scroll as damage);
//! - the old and the new bounds, shadow included, of every layer that moved, resized, appeared, disappeared, changed
//!   chrome or changed place in the stacking; all of them when the root's scale changed.
//!
//! Comparing with the committed scene, rather than logging changes, makes a hide and a show of a layer within one
//! commit cancel. Ids are unique within a root, whatever their kind.
//!
//! A window layer right under the root's frame layer may be its own output ([`Scene::set_output`]): the shell shows
//! it on a surface of its own, over the frame's.  Its rectangle is then snapped outward to the device rows and
//! columns a subsurface can be placed on ([`crate::snap::outward`]).  The commit routes the scene damage in that
//! rectangle to the output, in the output's coordinates ([`crate::committed::OutputFrame`]); the frame keeps the rest,
//! and the part of the snapped rectangle outside the window (the overlap with its neighbours) goes to both.  A
//! full-window scroll of an own output is a scroll distance and its exposed rows, not damage over the window.

use crate::committed::{Committed, OutputFrame};
use crate::damage::{Damage, Frames};
use crate::decor::{Anchor, Anchors, Label, Sprites};
use crate::ffi::{GLITNIR_E_ARG, GLITNIR_E_STATE};
use crate::geom::Rect;
use crate::stack::Stack;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

/// Every root's scene.
pub type Scenes = BTreeMap<u64, Scene>;

/// The most window layers of one root that are their own outputs at once.
pub const MAX_OUTPUTS: usize = 4;

/// An own output's rectangle: the window's R snapped outward at the root's SCALE (root device px).
fn snapped(r: Rect, scale: f64) -> Rect {
    let s120 = (scale * 120.0).round() as u32;
    crate::snap::stable_rect(crate::snap::outward(r, s120), 0, 0, s120)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// A frame's surface (the root's own).
    Frame,
    /// A window's rectangle on its frame's surface.
    Window,
    /// A child frame's surface, with its windows and nested child frames under it.
    Child,
}

impl Kind {
    /// The C value: 1 frame, 2 window, 3 child node.
    pub fn from_c(k: u32) -> Option<Kind> {
        match k {
            1 => Some(Kind::Frame),
            2 => Some(Kind::Window),
            3 => Some(Kind::Child),
            _ => None,
        }
    }
}

/// One layer as the host describes it.  Layout equals `GlitnirLayerDesc`.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LayerDesc {
    pub id: u64,
    /// 0 for a root's frame layer.
    pub parent: u64,
    /// [`Kind::from_c`].
    pub kind: u32,
    /// Root device px.
    pub rect: Rect,
    pub scale: f64,
    pub visible: u8,
}

/// A layer's chrome, device px; colours premultiplied ARGB in Cairo's order.  Layout equals `GlitnirChrome`.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Chrome {
    pub radius_px: f32,
    /// The CSS blur radius (σ = blur / 2).
    pub blur_px: f32,
    pub offset_x: f32,
    pub offset_y: f32,
    pub spread_px: f32,
    pub shadow_opacity: f32,
    pub shadow_argb: u32,
    pub border_px: f32,
    pub border_argb: u32,
}

/// Clamp to i32 for a pixel coordinate.
fn px(v: f64) -> i64 {
    v.clamp(i32::MIN as f64, i32::MAX as f64) as i64
}

impl Chrome {
    pub fn has_shadow(&self) -> bool {
        self.shadow_opacity > 0.0
    }

    /// The shadow image of a layer at RECT: the layer grown by the spread and the blur's reach, then offset, rounded
    /// outward (the image [`crate::shadow::ShadowTiles`] draws).  Empty without a shadow.
    pub fn shadow_rect(&self, rect: Rect) -> Rect {
        if rect.is_empty() || !self.has_shadow() {
            return Rect::default();
        }
        let spread = self.spread_px.round() as i64;
        let reach = crate::shadow::reach(self.blur_px) as i64;
        let pad = reach + spread;
        let w = (rect.w as i64 + 2 * spread).max(0) + 2 * reach;
        let h = (rect.h as i64 + 2 * spread).max(0) + 2 * reach;
        let x = (rect.x as i64 - pad) as f64 + self.offset_x as f64;
        let y = (rect.y as i64 - pad) as f64 + self.offset_y as f64;
        let (x0, y0) = (px(x.floor()), px(y.floor()));
        let (x1, y1) = (px((x + w as f64).ceil()), px((y + h as f64).ceil()));
        let side = |a: i64, b: i64| (b - a).min(i32::MAX as i64) as i32;
        Rect::new(x0 as i32, y0 as i32, side(x0, x1), side(y0, y1))
    }
}

/// The pixels a layer at RECT with CHROME can touch: its rectangle and its shadow image.
pub fn bounds(rect: Rect, chrome: Option<&Chrome>) -> Rect {
    match chrome {
        Some(c) if !rect.is_empty() => rect.bounds(&c.shadow_rect(rect)),
        _ => rect,
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct Layer {
    kind: Kind,
    rect: Rect,
    scale: f64,
    visible: bool,
    chrome: Option<Chrome>,
    /// A window layer that is its own output.
    own: bool,
}

/// A layer of a committed scene.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Placed {
    pub id: u64,
    pub parent: u64,
    pub kind: Kind,
    pub rect: Rect,
    pub scale: f64,
    pub chrome: Option<Chrome>,
    /// [`bounds`] of the rectangle and chrome.
    pub bounds: Rect,
}

impl Placed {
    fn looks_same(&self, o: &Placed) -> bool {
        self.kind == o.kind
            && self.rect == o.rect
            && self.scale == o.scale
            && self.chrome == o.chrome
    }
}

/// One root's scene: the layers now, their stacking, and the committed scene.
#[derive(Debug, Default)]
pub struct Scene {
    layers: BTreeMap<u64, Layer>,
    stack: Stack,
    /// Layers whose place in the stacking changed since the last commit.
    restacked: BTreeSet<u64>,
    /// The committed scene, with its labels.
    snapshot: Arc<Committed>,
    listed: Vec<u64>,
    /// Each window layer's label anchors, layer-local.
    anchors: BTreeMap<u64, Anchors>,
}

impl Scene {
    /// Add or change a layer.  A new layer, or one under a new parent, goes on top of its parent's children.
    /// `GLITNIR_E_ARG` for an unknown kind, id 0, a scale that is not positive, or a parent under the layer.
    pub fn upsert(&mut self, d: &LayerDesc) -> Result<(), i32> {
        let kind = Kind::from_c(d.kind).ok_or(GLITNIR_E_ARG)?;
        let scale_ok = d.scale.is_finite() && d.scale > 0.0;
        if d.id == 0 || !scale_ok || self.stack.would_cycle(d.id, d.parent) {
            return Err(GLITNIR_E_ARG);
        }
        let had = self.stack.parent_of(d.id);
        if had != Some(d.parent) {
            self.stack.place_top(d.id, d.parent);
            if had.is_some() {
                self.mark_restacked(d.id);
            }
        }
        let (chrome, own) = self
            .layers
            .get(&d.id)
            .map_or((None, false), |l| (l.chrome, l.own));
        self.layers.insert(
            d.id,
            Layer {
                kind,
                rect: d.rect,
                scale: d.scale,
                visible: d.visible != 0,
                chrome,
                own: own && kind == Kind::Window,
            },
        );
        Ok(())
    }

    /// Make window layer ID its own output (OWN), or a part of its frame's again.  It is an output at a commit only
    /// while it is visible and right under the root's frame layer.  `GLITNIR_E_ARG` for an unknown id or a layer that
    /// is not a window; `GLITNIR_E_STATE` for a window past [`MAX_OUTPUTS`].
    pub fn set_output(&mut self, id: u64, own: bool) -> Result<(), i32> {
        match self.layers.get(&id) {
            Some(l) if l.kind == Kind::Window => {}
            _ => return Err(GLITNIR_E_ARG),
        }
        let others = self
            .layers
            .iter()
            .filter(|(i, l)| **i != id && l.own)
            .count();
        if own && others >= MAX_OUTPUTS {
            return Err(GLITNIR_E_STATE);
        }
        if let Some(l) = self.layers.get_mut(&id) {
            l.own = own;
        }
        Ok(())
    }

    /// WINDOW's pixels in AREA moved by DY on ROOT's frame, EXPOSED left stale; D is the frame's damage.  When WINDOW
    /// is a visible own output right under ROOT's frame layer and AREA holds its whole rectangle, the move is its
    /// scroll ([`Damage::scroll_own`]); otherwise both rectangles are damaged and the window's layer is damaged at the
    /// commit, as for any window.
    pub fn scroll(
        &self,
        root: u64,
        d: &mut Damage,
        window: u64,
        area: Rect,
        dy: i32,
        exposed: Rect,
    ) {
        let root_scale = self.layers.get(&root).map(|l| l.scale);
        match (self.layers.get(&window), root_scale) {
            (Some(l), Some(scale))
                if l.own
                    && l.visible
                    && self.stack.parent_of(window) == Some(root)
                    && !l.rect.is_empty()
                    && area.intersect(&l.rect) == l.rect =>
            {
                d.scroll_own(window, area, dy, exposed, l.rect, snapped(l.rect, scale))
            }
            _ => {
                d.scroll(area, exposed);
                d.scrolled(window);
            }
        }
    }

    /// Remove ID and every layer under it (an unknown id is no change).
    pub fn remove(&mut self, id: u64) {
        for c in self.stack.remove_subtree(id) {
            self.layers.remove(&c);
            self.anchors.remove(&c);
            self.restacked.remove(&c);
        }
    }

    /// Set (or, with `None`, clear) ID's chrome.  `GLITNIR_E_ARG` for an unknown id.
    pub fn set_chrome(&mut self, id: u64, chrome: Option<Chrome>) -> Result<(), i32> {
        self.layers.get_mut(&id).ok_or(GLITNIR_E_ARG)?.chrome = chrome;
        Ok(())
    }

    /// Record anchor A on LAYER (`GLITNIR_E_ARG` for an unknown layer).
    pub fn decor_put(&mut self, layer: u64, a: &Anchor) -> Result<(), i32> {
        self.known(layer)?;
        self.anchors.entry(layer).or_default().put(a);
        Ok(())
    }

    /// Drop LAYER's anchors whose glyph meets EXTENT.
    pub fn decor_drop_extent(&mut self, layer: u64, extent: Rect) -> Result<(), i32> {
        self.known(layer)?;
        if let Some(a) = self.anchors.get_mut(&layer) {
            a.drop_extent(extent);
        }
        Ok(())
    }

    /// Drop LAYER's anchors whose glyph lies wholly in R.
    pub fn decor_drop_rect(&mut self, layer: u64, r: Rect) -> Result<(), i32> {
        self.known(layer)?;
        if let Some(a) = self.anchors.get_mut(&layer) {
            a.drop_rect(r);
        }
        Ok(())
    }

    /// Drop LAYER's anchors (0: every layer's).
    pub fn decor_drop_all(&mut self, layer: u64) -> Result<(), i32> {
        if layer == 0 {
            self.anchors.clear();
            return Ok(());
        }
        self.known(layer)?;
        self.anchors.remove(&layer);
        Ok(())
    }

    /// Move LAYER's anchors in BAND by DY; those that leave the band or the layer go.
    pub fn decor_scroll(&mut self, layer: u64, band: Rect, dy: i32) -> Result<(), i32> {
        self.known(layer)?;
        let extent = self
            .layers
            .get(&layer)
            .map_or(Rect::default(), |l| Rect::new(0, 0, l.rect.w, l.rect.h));
        if let Some(a) = self.anchors.get_mut(&layer) {
            a.scroll(band, dy, extent);
        }
        Ok(())
    }

    /// The labels of the committed scene, in paint order within each window layer.
    pub fn labels(&self) -> &[Label] {
        &self.snapshot.labels
    }

    /// The committed scene as a snapshot that later changes leave alone.
    pub fn snapshot(&self) -> Arc<Committed> {
        Arc::clone(&self.snapshot)
    }

    /// The labels the visible window layers of PLACED have now.
    fn placed_labels(&self, placed: &[Placed], sprites: &Sprites) -> Vec<Label> {
        let mut out = Vec::new();
        for p in placed.iter().filter(|p| p.kind == Kind::Window) {
            if let Some(a) = self.anchors.get(&p.id) {
                crate::decor::place(p.id, p.rect, a.as_slice(), sprites, &mut out);
            }
        }
        out
    }

    fn known(&self, id: u64) -> Result<(), i32> {
        if self.layers.contains_key(&id) {
            Ok(())
        } else {
            Err(GLITNIR_E_ARG)
        }
    }

    fn mark_restacked(&mut self, id: u64) {
        self.restacked.extend(self.stack.subtree(id));
    }

    /// Raise ID to the top of its siblings.
    pub fn raise(&mut self, id: u64) -> Result<(), i32> {
        self.known(id)?;
        if self.stack.raise(id) {
            self.mark_restacked(id);
        }
        Ok(())
    }

    /// Lower ID to the bottom of its siblings.
    pub fn lower(&mut self, id: u64) -> Result<(), i32> {
        self.known(id)?;
        if self.stack.lower(id) {
            self.mark_restacked(id);
        }
        Ok(())
    }

    /// Move ID, with everything under it, to the top of PARENT's children (0: the top level).
    pub fn reparent(&mut self, id: u64, parent: u64) -> Result<(), i32> {
        self.known(id)?;
        if self.stack.would_cycle(id, parent) {
            return Err(GLITNIR_E_ARG);
        }
        self.stack.place_top(id, parent);
        self.mark_restacked(id);
        Ok(())
    }

    /// The child nodes, hidden ones included, bottom to top: depth first, so a nested child sits right above its
    /// parent and below the parent's upper siblings.  Valid until the next call.
    pub fn list(&mut self) -> &[u64] {
        self.listed.clear();
        let mut todo: Vec<u64> = self.stack.children(0).iter().rev().copied().collect();
        while let Some(id) = todo.pop() {
            if self.layers.get(&id).is_some_and(|l| l.kind == Kind::Child) {
                self.listed.push(id);
            }
            todo.extend(self.stack.children(id).iter().rev());
        }
        &self.listed
    }

    /// The visible layers now, in paint order: depth first, bottom to top; under each layer its windows, then its
    /// child nodes in stacking order.
    fn placed(&self) -> Vec<Placed> {
        let mut out = Vec::new();
        self.walk(0, &mut out);
        out
    }

    fn walk(&self, parent: u64, out: &mut Vec<Placed>) {
        for windows in [true, false] {
            for &id in self.stack.children(parent) {
                let Some(l) = self.layers.get(&id) else {
                    continue;
                };
                if !l.visible || (l.kind == Kind::Window) != windows {
                    continue;
                }
                out.push(Placed {
                    id,
                    parent,
                    kind: l.kind,
                    rect: l.rect,
                    scale: l.scale,
                    chrome: l.chrome,
                    bounds: bounds(l.rect, l.chrome.as_ref()),
                });
                self.walk(id, out);
            }
        }
    }

    /// The committed scene's visible layers in paint order.
    pub fn committed(&self) -> &[Placed] {
        &self.snapshot.layers
    }

    /// Commit ROOT's scene: take its frames' damage from FRAMES, damage what changed since the last commit, clipped
    /// to the root's frame layer, and return it (ROOT's committed damage, kept in FRAMES until the next commit).
    pub fn commit<'a>(&mut self, root: u64, frames: &'a mut Frames) -> &'a [Rect] {
        self.commit_with(root, frames, &Arc::new(Sprites::new()))
    }

    /// [`Scene::commit`] with the sprite cache the labels are placed from: a label that appeared, moved, changed
    /// sprite or went adds its old and new sprite bounds to the damage.  The new snapshot keeps SPRITES; every placed
    /// label's sprite counts as used.
    pub fn commit_with<'a>(
        &mut self,
        root: u64,
        frames: &'a mut Frames,
        sprites: &Arc<Sprites>,
    ) -> &'a [Rect] {
        let new = self.placed();
        let new_labels = self.placed_labels(&new, sprites);
        for l in &new_labels {
            sprites.get(l.sprite);
        }
        let old_labels = Arc::clone(&self.snapshot.labels);
        let old = self.snapshot.layers.as_slice();
        let mut out: Vec<Rect> = Vec::new();
        let mut outputs: Vec<OutputFrame> = Vec::new();
        {
            let new_by: BTreeMap<u64, &Placed> = new.iter().map(|p| (p.id, p)).collect();
            let old_by: BTreeMap<u64, &Placed> = old.iter().map(|p| (p.id, p)).collect();
            let mut offsets: BTreeMap<u64, i32> = BTreeMap::new();
            // Layer damage: a frame's own, in its coordinates.  A hidden layer's is dropped.
            for (&id, l) in &self.layers {
                if l.kind == Kind::Window {
                    continue;
                }
                let Some((rects, scrolls, moved)) = frames.get_mut(&id).map(Damage::drain) else {
                    continue;
                };
                offsets.extend(moved);
                let Some(p) = new_by.get(&id) else {
                    continue;
                };
                let local = Rect::new(0, 0, p.rect.w, p.rect.h);
                out.extend(
                    rects
                        .iter()
                        .map(|r| r.intersect(&local).translate(p.rect.x, p.rect.y)),
                );
                out.extend(scrolls.iter().filter_map(|w| new_by.get(w)).map(|w| w.rect));
            }
            if !self.layers.contains_key(&root)
                && let Some(d) = frames.get_mut(&root)
            {
                out.extend(d.drain().0);
            }
            // Geometry, chrome, visibility and stacking: old and new bounds.
            let scale = |by: &BTreeMap<u64, &Placed>| by.get(&root).map(|p| p.scale);
            let rescaled = matches!((scale(&old_by), scale(&new_by)), (Some(a), Some(b)) if a != b);
            let ids: BTreeSet<u64> = old_by.keys().chain(new_by.keys()).copied().collect();
            for id in ids {
                let (o, n) = (old_by.get(&id), new_by.get(&id));
                let changed = match (o, n) {
                    (Some(o), Some(n)) => {
                        rescaled || !o.looks_same(n) || self.restacked.contains(&id)
                    }
                    _ => true,
                };
                if changed {
                    out.extend(o.map(|p| p.bounds));
                    out.extend(n.map(|p| p.bounds));
                }
            }
            // Labels: the sprite bounds of every label that is not the same in both.
            let (new_l, old_l) = (&new_labels, &*old_labels);
            out.extend(
                new_l
                    .iter()
                    .filter(|l| !old_l.contains(l))
                    .map(Label::visible),
            );
            out.extend(
                old_l
                    .iter()
                    .filter(|l| !new_l.contains(l))
                    .map(Label::visible),
            );
            let clip = new_by.get(&root).map(|p| p.rect);
            if let Some(clip) = clip {
                for r in &mut out {
                    *r = r.intersect(&clip);
                }
            }
            // Own outputs: the damage in each snapped rectangle, in its coordinates; the whole of a new one, a moved
            // one, or every one at a new scale.
            let scale = new_by.get(&root).map_or(0.0, |p| p.scale);
            let mut windows: Vec<Rect> = Vec::new();
            for p in new.iter().filter(|p| {
                scale > 0.0
                    && p.kind == Kind::Window
                    && p.parent == root
                    && self.layers.get(&p.id).is_some_and(|l| l.own)
            }) {
                let rect = snapped(p.rect, scale);
                let before = self.snapshot.outputs.iter().find(|o| o.layer == p.id);
                let full = rescaled || before.is_none_or(|o| o.rect != rect);
                let damage = if full {
                    vec![Rect::new(0, 0, rect.w, rect.h)]
                } else {
                    out.iter()
                        .map(|r| r.intersect(&rect))
                        .filter(|r| !r.is_empty())
                        .map(|r| r.translate(-rect.x, -rect.y))
                        .collect()
                };
                let scroll = if full {
                    0
                } else {
                    offsets.get(&p.id).copied().unwrap_or(0)
                };
                windows.push(p.rect);
                outputs.push(OutputFrame {
                    layer: p.id,
                    rect,
                    scroll,
                    damage,
                    full,
                });
            }
            // An output that went or moved: the frame shows its old rectangle again.
            for o in &self.snapshot.outputs {
                if outputs
                    .iter()
                    .find(|n| n.layer == o.layer)
                    .is_none_or(|n| n.rect != o.rect)
                {
                    out.push(clip.map_or(o.rect, |c| o.rect.intersect(&c)));
                }
            }
            // The frame's damage: all but the own outputs' windows (their overlap stays).
            for w in &windows {
                out = out
                    .iter()
                    .flat_map(|r| crate::compose::subtract(*r, *w))
                    .collect();
            }
        }
        let labels = if new_labels == *old_labels {
            old_labels
        } else {
            Arc::new(new_labels)
        };
        self.snapshot = Arc::new(Committed {
            root,
            layers: new,
            labels,
            sprites: Arc::clone(sprites),
            outputs,
        });
        self.restacked.clear();
        let d = frames.entry(root).or_default();
        for r in out {
            d.add(r);
        }
        d.commit()
    }
}

/// Run `f` on ROOT's scene; `GLITNIR_E_STATE` when ROOT has none.
pub fn with<R>(root: u64, f: impl FnOnce(&mut Scene) -> Result<R, i32>) -> Result<R, i32> {
    let mut st = crate::state();
    st.scenes.get_mut(&root).ok_or(GLITNIR_E_STATE).and_then(f)
}

/// ROOT's committed scene as its last commit left it; `None` when ROOT has no scene.
pub fn committed(root: u64) -> Option<Arc<Committed>> {
    crate::state().scenes.get(&root).map(Scene::snapshot)
}

/// Give ROOT a scene (an existing one is kept).
pub fn root_new(root: u64) {
    crate::state().scenes.entry(root).or_default();
}

/// Drop ROOT's scene; its commits fall back to its own damage.
pub fn root_drop(root: u64) {
    crate::state().scenes.remove(&root);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rng::XorShift;
    use crate::shadow::{ShadowCache, ShadowSpec};

    const ROOT: u64 = 1;

    fn desc(id: u64, parent: u64, kind: u32, rect: Rect, scale: f64, visible: bool) -> LayerDesc {
        LayerDesc {
            id,
            parent,
            kind,
            rect,
            scale,
            visible: visible as u8,
        }
    }

    /// A scene with the root's frame layer at W×H, committed once.
    fn scene(w: i32, h: i32, scale: f64) -> (Scene, Frames) {
        let (mut s, mut f) = (Scene::default(), Frames::new());
        s.upsert(&desc(ROOT, 0, 1, Rect::new(0, 0, w, h), scale, true))
            .unwrap();
        s.commit(ROOT, &mut f);
        (s, f)
    }

    fn child(s: &mut Scene, id: u64, parent: u64, rect: Rect) {
        s.upsert(&desc(id, parent, 3, rect, 1.0, true)).unwrap();
    }

    fn commit(s: &mut Scene, f: &mut Frames) -> Vec<Rect> {
        s.commit(ROOT, f).to_vec()
    }

    fn covered(damage: &[Rect], x: i32, y: i32) -> bool {
        damage.iter().any(|r| r.contains(x, y))
    }

    /// Every pixel of R is in DAMAGE.
    fn covers(damage: &[Rect], r: Rect) -> bool {
        (r.y..r.y + r.h.max(0)).all(|y| (r.x..r.x + r.w.max(0)).all(|x| covered(damage, x, y)))
    }

    fn shadow() -> Chrome {
        Chrome {
            radius_px: 16.0,
            blur_px: 32.0,
            offset_x: 0.0,
            offset_y: 12.0,
            spread_px: 0.0,
            shadow_opacity: 0.35,
            shadow_argb: 0xff00_0000,
            border_px: 2.0,
            border_argb: 0xff80_8080,
        }
    }

    #[test]
    fn commit_damage_covers_old_and_new_bounds() {
        const N: i32 = 160;
        let root = Rect::new(0, 0, N, N);
        for seed in 1..=100u64 {
            let mut g = XorShift::new(seed);
            let (mut s, mut f) = scene(N, N, 1.0);
            let rnd = |g: &mut XorShift| {
                Rect::new(
                    g.below(N as u32) as i32 - 20,
                    g.below(N as u32) as i32 - 20,
                    g.below(60) as i32,
                    g.below(60) as i32,
                )
            };
            // (rect, visible, chrome) per child id 10..14
            let mut model: Vec<(Rect, bool, Option<Chrome>)> = Vec::new();
            for i in 0..4u64 {
                let r = rnd(&mut g);
                child(&mut s, 10 + i, ROOT, r);
                model.push((r, true, None));
            }
            commit(&mut s, &mut f);
            for _round in 0..4 {
                let before = model.clone();
                for (i, m) in model.iter_mut().enumerate() {
                    let id = 10 + i as u64;
                    match g.below(5) {
                        0 => m.0 = rnd(&mut g),
                        1 => m.1 = !m.1,
                        2 => {
                            let mut c = shadow();
                            c.blur_px = g.below(20) as f32;
                            c.offset_x = g.below(9) as f32 - 4.5;
                            m.2 = Some(c);
                            s.set_chrome(id, m.2).unwrap();
                        }
                        _ => {}
                    }
                    s.upsert(&desc(id, ROOT, 3, m.0, 1.0, m.1)).unwrap();
                }
                let d = commit(&mut s, &mut f);
                for (o, n) in before.iter().zip(&model) {
                    if o == n {
                        continue;
                    }
                    for (r, vis, c) in [o, n] {
                        if *vis {
                            let b = bounds(*r, c.as_ref()).intersect(&root);
                            assert!(covers(&d, b), "seed {seed}: {b:?} not in {d:?}");
                        }
                    }
                }
                assert!(d.iter().all(|r| root.intersect(r) == *r), "seed {seed}");
            }
            // nothing changed: nothing damaged
            assert!(commit(&mut s, &mut f).is_empty(), "seed {seed}");
        }
        // a child frame's own damage lands at its place; a scroll damages its window layer
        let (mut s, mut f) = scene(800, 600, 1.0);
        child(&mut s, 10, ROOT, Rect::new(100, 50, 200, 100));
        s.upsert(&desc(11, 10, 2, Rect::new(104, 54, 192, 80), 1.0, true))
            .unwrap();
        commit(&mut s, &mut f);
        let fd = f.entry(10).or_default();
        fd.add(Rect::new(0, 0, 5, 5));
        fd.add(Rect::new(190, 90, 50, 50));
        fd.scrolled(11);
        let d = commit(&mut s, &mut f);
        assert!(covers(&d, Rect::new(100, 50, 5, 5)));
        assert!(covers(&d, Rect::new(290, 140, 10, 10)));
        assert!(covers(&d, Rect::new(104, 54, 192, 80)));
        assert!(!covered(&d, 300, 150), "clipped to the child: {d:?}");
    }

    #[test]
    fn shadow_extent_is_in_the_damage() {
        let (mut s, mut f) = scene(800, 600, 2.0);
        let r = Rect::new(200, 150, 300, 200);
        let c = shadow();
        s.upsert(&desc(10, ROOT, 3, r, 2.0, true)).unwrap();
        s.set_chrome(10, Some(c)).unwrap();
        // the image the shadow tiles draw: pad above and left of the layer, then the offset
        let tiles = ShadowCache::new().get(
            r.w as u32,
            r.h as u32,
            &ShadowSpec {
                radius: c.radius_px,
                blur: c.blur_px,
                spread: c.spread_px,
                colour: c.shadow_argb,
                scale: 2.0,
            },
        );
        let (iw, ih) = tiles.image_size(r.w as u32, r.h as u32);
        let image = Rect::new(
            r.x - tiles.pad(),
            r.y - tiles.pad() + c.offset_y as i32,
            iw as i32,
            ih as i32,
        );
        assert_eq!(c.shadow_rect(r), image);
        let d = commit(&mut s, &mut f);
        assert!(covers(&d, image) && covers(&d, r), "{d:?}");
        let b = r.bounds(&image);
        assert!(d.iter().all(|x| b.intersect(x) == *x), "{d:?} past {b:?}");
        // re-chromed: the shadow's extent again
        s.set_chrome(
            10,
            Some(Chrome {
                shadow_opacity: 0.6,
                ..c
            }),
        )
        .unwrap();
        assert!(covers(&commit(&mut s, &mut f), image));
        // moved: old and new shadows
        s.upsert(&desc(10, ROOT, 3, r.translate(40, 0), 2.0, true))
            .unwrap();
        let d = commit(&mut s, &mut f);
        assert!(covers(&d, image) && covers(&d, image.translate(40, 0)));
        // a fractional offset rounds outward
        let mut half = c;
        half.offset_x = 0.5;
        let h = half.shadow_rect(r);
        assert_eq!((h.x, h.w), (image.x, image.w + 1));
        // no shadow: the rectangle only
        assert_eq!(bounds(r, Some(&Chrome::default())), r);
    }

    #[test]
    fn hide_then_show_in_one_commit_cancels() {
        let (mut s, mut f) = scene(800, 600, 1.0);
        let r = Rect::new(10, 10, 100, 40);
        child(&mut s, 10, ROOT, r);
        s.set_chrome(10, Some(shadow())).unwrap();
        assert!(!commit(&mut s, &mut f).is_empty());
        s.upsert(&desc(10, ROOT, 3, r, 1.0, false)).unwrap();
        s.upsert(&desc(10, ROOT, 3, r, 1.0, true)).unwrap();
        assert!(commit(&mut s, &mut f).is_empty());
        // showing raises: already on top, nothing moves
        s.raise(10).unwrap();
        assert!(commit(&mut s, &mut f).is_empty());
        // a hide alone damages the bounds; so does the show after it
        let b = bounds(r, Some(&shadow())).intersect(&Rect::new(0, 0, 800, 600));
        s.upsert(&desc(10, ROOT, 3, r, 1.0, false)).unwrap();
        assert!(covers(&commit(&mut s, &mut f), b));
        s.upsert(&desc(10, ROOT, 3, r, 1.0, true)).unwrap();
        assert!(covers(&commit(&mut s, &mut f), b));
    }

    #[test]
    fn raise_lower_order() {
        for seed in 1..=200u64 {
            let mut g = XorShift::new(seed);
            let (mut s, _) = scene(800, 600, 1.0);
            let mut model: Vec<u64> = Vec::new();
            for _ in 0..300 {
                let id = 10 + g.below(8) as u64;
                let known = model.contains(&id);
                match g.below(4) {
                    _ if !known => {
                        child(&mut s, id, ROOT, Rect::new(0, 0, 10, 10));
                        model.push(id);
                    }
                    0 => {
                        s.raise(id).unwrap();
                        model.retain(|&x| x != id);
                        model.push(id);
                    }
                    1 => {
                        s.lower(id).unwrap();
                        model.retain(|&x| x != id);
                        model.insert(0, id);
                    }
                    2 => {
                        s.remove(id);
                        model.retain(|&x| x != id);
                    }
                    _ => {
                        // a change of geometry keeps the place
                        child(&mut s, id, ROOT, Rect::new(5, 5, 20, 20));
                    }
                }
                assert_eq!(s.list(), model.as_slice(), "seed {seed}");
            }
        }
        let (mut s, _) = scene(800, 600, 1.0);
        assert_eq!(s.raise(99), Err(GLITNIR_E_ARG));
        assert_eq!(s.lower(99), Err(GLITNIR_E_ARG));
    }

    #[test]
    fn reparent_moves_the_subtree() {
        let (mut s, mut f) = scene(800, 600, 1.0);
        let (a, b, c, w) = (10, 20, 30, 31);
        let (ra, rc) = (Rect::new(10, 10, 100, 100), Rect::new(20, 20, 30, 30));
        child(&mut s, a, ROOT, ra);
        child(&mut s, b, ROOT, Rect::new(50, 50, 100, 100));
        child(&mut s, c, a, rc);
        s.upsert(&desc(w, c, 2, Rect::new(22, 22, 26, 26), 1.0, true))
            .unwrap();
        commit(&mut s, &mut f);
        assert_eq!(s.list(), &[a, c, b]);
        s.reparent(a, b).unwrap();
        assert_eq!(s.list(), &[b, a, c]);
        let ids: Vec<u64> = s.placed().iter().map(|p| p.id).collect();
        assert_eq!(ids, vec![ROOT, b, a, c, w]);
        let d = commit(&mut s, &mut f);
        assert!(covers(&d, ra) && covers(&d, rc), "{d:?}");
        // under its own subtree: refused, nothing changes
        assert_eq!(s.reparent(b, c), Err(GLITNIR_E_ARG));
        assert_eq!(
            s.upsert(&desc(b, w, 3, Rect::new(0, 0, 1, 1), 1.0, true)),
            Err(GLITNIR_E_ARG)
        );
        assert_eq!(s.list(), &[b, a, c]);
        // an upsert under a new parent reparents
        child(&mut s, c, ROOT, rc);
        assert_eq!(s.list(), &[b, a, c]);
        s.reparent(a, ROOT).unwrap();
        assert_eq!(s.list(), &[b, c, a]);
        assert!(covers(&commit(&mut s, &mut f), ra));
    }

    #[test]
    fn nested_child_stacks_inside_its_parent() {
        let (mut s, mut f) = scene(800, 600, 1.0);
        let (a, b, c, d, w) = (10, 20, 30, 40, 11);
        child(&mut s, a, ROOT, Rect::new(0, 0, 300, 300));
        child(&mut s, b, ROOT, Rect::new(100, 100, 300, 300));
        child(&mut s, c, a, Rect::new(10, 10, 50, 50));
        assert_eq!(s.list(), &[a, c, b]);
        s.raise(c).unwrap();
        assert_eq!(s.list(), &[a, c, b], "C stays inside A, below B");
        s.raise(a).unwrap();
        assert_eq!(s.list(), &[b, a, c]);
        child(&mut s, d, a, Rect::new(20, 20, 50, 50));
        assert_eq!(s.list(), &[b, a, c, d]);
        s.lower(c).unwrap();
        assert_eq!(s.list(), &[b, a, c, d]);
        s.lower(d).unwrap();
        assert_eq!(s.list(), &[b, a, d, c]);
        // A's window paints above A and below A's children, whenever it came
        s.upsert(&desc(w, a, 2, Rect::new(2, 2, 290, 290), 1.0, true))
            .unwrap();
        commit(&mut s, &mut f);
        let ids: Vec<u64> = s.committed().iter().map(|p| p.id).collect();
        assert_eq!(ids, vec![ROOT, b, a, w, d, c]);
        // a hidden parent hides its subtree from the paint, not from the stacking
        s.upsert(&desc(a, ROOT, 3, Rect::new(0, 0, 300, 300), 1.0, false))
            .unwrap();
        commit(&mut s, &mut f);
        let ids: Vec<u64> = s.committed().iter().map(|p| p.id).collect();
        assert_eq!(ids, vec![ROOT, b]);
        assert_eq!(s.list(), &[b, a, d, c]);
    }

    #[test]
    fn removed_layer_has_no_damage_source() {
        let (mut s, mut f) = scene(800, 600, 1.0);
        let (c, n) = (10, 20);
        let r = Rect::new(100, 100, 50, 50);
        child(&mut s, c, ROOT, r);
        child(&mut s, n, c, Rect::new(110, 110, 10, 10));
        commit(&mut s, &mut f);
        f.entry(c).or_default().add(Rect::new(0, 0, 10, 10));
        s.remove(c);
        assert!(s.list().is_empty(), "the subtree went too");
        assert_eq!(s.set_chrome(n, None), Err(GLITNIR_E_ARG));
        assert!(covers(&commit(&mut s, &mut f), r));
        // the frame still draws (it is alive, e.g. on its way to another root): nothing reaches this root
        f.entry(c).or_default().add(Rect::new(0, 0, 10, 10));
        f.entry(c).or_default().scrolled(n);
        assert!(commit(&mut s, &mut f).is_empty());
        assert!(s.committed().iter().all(|p| p.id == ROOT));
        // and its damage is still the frame's own
        assert!(!f.entry(c).or_default().is_empty());
    }

    /// A window layer id as a host may tag it (high bit set).
    const WIN: u64 = (1 << 63) | 7;

    /// An 800x600 root at SCALE with window WIN at R, its own output, committed once.
    fn own(scale: f64, r: Rect) -> (Scene, Frames) {
        let (mut s, mut f) = scene(800, 600, scale);
        s.upsert(&desc(WIN, ROOT, 2, r, scale, true)).unwrap();
        s.set_output(WIN, true).unwrap();
        commit(&mut s, &mut f);
        (s, f)
    }

    fn output(s: &Scene) -> OutputFrame {
        s.snapshot
            .outputs
            .iter()
            .find(|o| o.layer == WIN)
            .cloned()
            .unwrap()
    }

    /// WIN's run, as `glitnir_scroll` reports it.
    fn run(s: &Scene, f: &mut Frames, area: Rect, dy: i32, exposed: Rect) {
        s.scroll(ROOT, f.entry(ROOT).or_default(), WIN, area, dy, exposed);
    }

    fn add(f: &mut Frames, r: Rect) {
        f.entry(ROOT).or_default().add(r);
    }

    /// No rectangle of D meets R.
    fn misses(d: &[Rect], r: Rect) -> bool {
        d.iter().all(|x| x.intersect(&r).is_empty())
    }

    #[test]
    fn own_output_takes_its_window_damage() {
        let r = Rect::new(10, 20, 400, 300);
        let (mut s, mut f) = own(1.0, r);
        let o = output(&s);
        assert!(o.full);
        assert_eq!((o.rect, o.damage), (r, vec![Rect::new(0, 0, 400, 300)]));
        // inside the window: the output's, in its coordinates; outside: the frame's
        add(&mut f, Rect::new(50, 50, 10, 10));
        add(&mut f, Rect::new(600, 500, 10, 10));
        let d = commit(&mut s, &mut f);
        let o = output(&s);
        assert_eq!((o.full, o.scroll), (false, 0));
        assert_eq!(o.damage, vec![Rect::new(40, 30, 10, 10)]);
        assert!(
            covers(&d, Rect::new(600, 500, 10, 10)) && misses(&d, r),
            "{d:?}"
        );
        // across the window's edge: split
        add(&mut f, Rect::new(400, 100, 20, 10));
        let d = commit(&mut s, &mut f);
        assert_eq!(output(&s).damage, vec![Rect::new(390, 80, 10, 10)]);
        assert!(
            covers(&d, Rect::new(410, 100, 10, 10)) && misses(&d, r),
            "{d:?}"
        );
        // nothing changed: nothing on either
        assert!(commit(&mut s, &mut f).is_empty() && output(&s).damage.is_empty());
        // a new scale: drawn whole again
        s.upsert(&desc(ROOT, 0, 1, Rect::new(0, 0, 800, 600), 2.0, true))
            .unwrap();
        commit(&mut s, &mut f);
        assert!(output(&s).full);
        // a window under a child frame is never an output; a hidden one is none while hidden
        let (mut s, mut f) = own(1.0, r);
        child(&mut s, 30, ROOT, Rect::new(500, 0, 200, 200));
        s.upsert(&desc(31, 30, 2, Rect::new(510, 10, 100, 100), 1.0, true))
            .unwrap();
        s.set_output(31, true).unwrap();
        s.upsert(&desc(WIN, ROOT, 2, r, 1.0, false)).unwrap();
        commit(&mut s, &mut f);
        assert!(s.snapshot.outputs.is_empty());
        s.upsert(&desc(WIN, ROOT, 2, r, 1.0, true)).unwrap();
        commit(&mut s, &mut f);
        assert!(output(&s).full);
        assert_eq!(s.snapshot.outputs.len(), 1);
    }

    #[test]
    fn own_output_scroll_is_an_offset() {
        let r = Rect::new(10, 20, 400, 300);
        let (mut s, mut f) = own(1.0, r);
        // drawn before the scroll: it moves with the pixels
        add(&mut f, Rect::new(50, 100, 10, 10));
        // the content moves up 40 rows, the whole window
        run(&s, &mut f, r, -40, Rect::new(10, 280, 400, 40));
        let d = commit(&mut s, &mut f);
        let o = output(&s);
        assert_eq!((o.scroll, o.full), (-40, false));
        assert!(
            covers(&o.damage, Rect::new(0, 260, 400, 40)),
            "{:?}",
            o.damage
        );
        assert!(
            covers(&o.damage, Rect::new(40, 40, 10, 10)),
            "{:?}",
            o.damage
        );
        // the rows that only moved are not damage
        assert!(!covered(&o.damage, 200, 100) && !covered(&o.damage, 45, 85));
        assert!(misses(&d, r), "{d:?}");
        // runs in one commit add up; rows exposed by the first move with the second
        run(&s, &mut f, r, -10, Rect::new(10, 310, 400, 10));
        run(&s, &mut f, r, 25, Rect::new(10, 20, 400, 25));
        commit(&mut s, &mut f);
        let o = output(&s);
        assert_eq!(o.scroll, 15);
        assert!(
            covers(&o.damage, Rect::new(0, 0, 400, 25)),
            "{:?}",
            o.damage
        );
        assert!(!covered(&o.damage, 0, 295), "{:?}", o.damage);
        // a run wider than the window: the frame takes the columns beside it
        let wide = Rect::new(0, 20, 420, 300);
        run(&s, &mut f, wide, -20, Rect::new(0, 300, 420, 20));
        let d = commit(&mut s, &mut f);
        assert_eq!(output(&s).scroll, -20);
        assert!(covers(&d, Rect::new(0, 20, 10, 300)) && covers(&d, Rect::new(410, 20, 10, 300)));
        assert!(misses(&d, r), "{d:?}");
        assert!(covers(&output(&s).damage, Rect::new(0, 280, 400, 20)));
    }

    #[test]
    fn partial_scroll_stays_damage() {
        let r = Rect::new(10, 20, 400, 300);
        let (mut s, mut f) = own(1.0, r);
        // rows 100..220 move up 20: a run inside the window
        let area = Rect::new(10, 100, 400, 120);
        run(&s, &mut f, area, -20, Rect::new(10, 200, 400, 20));
        let d = commit(&mut s, &mut f);
        let o = output(&s);
        assert_eq!((o.scroll, o.full), (0, false));
        assert!(
            covers(&o.damage, area.translate(-10, -20)),
            "{:?}",
            o.damage
        );
        assert!(misses(&d, r), "{d:?}");
        // a full-window run of a window that is no output is damage too
        s.set_output(WIN, false).unwrap();
        commit(&mut s, &mut f);
        run(&s, &mut f, r, -40, Rect::new(10, 280, 400, 40));
        let d = commit(&mut s, &mut f);
        assert!(covers(&d, r) && s.snapshot.outputs.is_empty(), "{d:?}");
    }

    #[test]
    fn overlap_damage_goes_to_both_outputs() {
        // at 1.5 the window's left and top edges fall between the rows and columns a subsurface can take
        let r = Rect::new(31, 31, 301, 151);
        let (mut s, mut f) = own(1.5, r);
        let o = output(&s);
        assert_eq!(o.rect, Rect::new(30, 30, 302, 152));
        // a pixel of the neighbour the snapped rectangle covers, and one of the window
        add(&mut f, Rect::new(30, 100, 1, 1));
        add(&mut f, Rect::new(100, 100, 5, 5));
        let d = commit(&mut s, &mut f);
        let o = output(&s);
        assert!(
            covered(&o.damage, 0, 70) && covered(&d, 30, 100),
            "{d:?} {o:?}"
        );
        assert!(covers(&o.damage, Rect::new(70, 70, 5, 5)) && !covered(&d, 100, 100));
        // the frame's damage stops at the window, not at the snapped rectangle
        assert!(misses(&d, r), "{d:?}");
        // a full-window scroll moves the overlap with the view though its pixels stayed: it is damage
        run(&s, &mut f, r, -10, Rect::new(31, 172, 301, 10));
        let d = commit(&mut s, &mut f);
        let o = output(&s);
        assert_eq!(o.scroll, -10);
        assert!(covers(&o.damage, Rect::new(0, 0, 1, 152)), "{:?}", o.damage);
        assert!(covers(&o.damage, Rect::new(0, 0, 302, 1)), "{:?}", o.damage);
        assert!(
            covers(&o.damage, Rect::new(1, 142, 301, 10)),
            "{:?}",
            o.damage
        );
        assert!(!covered(&o.damage, 100, 50) && misses(&d, r), "{d:?} {o:?}");
    }

    #[test]
    fn demote_damages_the_window_rect_on_the_frame() {
        let r = Rect::new(31, 31, 301, 151);
        let (mut s, mut f) = own(1.5, r);
        let snapped = output(&s).rect;
        assert!(commit(&mut s, &mut f).is_empty());
        s.set_output(WIN, false).unwrap();
        let d = commit(&mut s, &mut f);
        assert!(s.snapshot.outputs.is_empty());
        assert!(covers(&d, snapped), "{d:?}");
        assert!(commit(&mut s, &mut f).is_empty());
        // a removed output: the same
        s.set_output(WIN, true).unwrap();
        commit(&mut s, &mut f);
        assert!(output(&s).full);
        s.remove(WIN);
        assert!(covers(&commit(&mut s, &mut f), snapped));
        // a resized one starts over, and the frame gets back what it no longer covers
        s.upsert(&desc(WIN, ROOT, 2, r, 1.5, true)).unwrap();
        s.set_output(WIN, true).unwrap();
        commit(&mut s, &mut f);
        s.upsert(&desc(WIN, ROOT, 2, Rect::new(31, 31, 301, 100), 1.5, true))
            .unwrap();
        let d = commit(&mut s, &mut f);
        let o = output(&s);
        assert_eq!((o.full, o.rect), (true, Rect::new(30, 30, 302, 101)));
        assert!(covers(&d, Rect::new(30, 131, 302, 51)), "{d:?}");
    }

    #[test]
    fn scale_change_damages_everything() {
        let (mut s, mut f) = scene(800, 600, 1.0);
        child(&mut s, 10, ROOT, Rect::new(100, 100, 50, 50));
        commit(&mut s, &mut f);
        assert!(commit(&mut s, &mut f).is_empty());
        let all = Rect::new(0, 0, 800, 600);
        s.upsert(&desc(ROOT, 0, 1, all, 2.0, true)).unwrap();
        let d = commit(&mut s, &mut f);
        assert!(covers(&d, all), "{d:?}");
        assert!(commit(&mut s, &mut f).is_empty());
        // a child's scale alone damages the child
        s.upsert(&desc(10, ROOT, 3, Rect::new(100, 100, 50, 50), 2.0, true))
            .unwrap();
        let d = commit(&mut s, &mut f);
        assert!(covers(&d, Rect::new(100, 100, 50, 50)) && !covered(&d, 0, 0));
    }
}

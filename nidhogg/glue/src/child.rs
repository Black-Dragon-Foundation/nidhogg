//! Child frames.  Moved from `src/nhwlchild.c` (W1's child frames, written fresh; the chrome's hand-over to the layer
//! adapted from Nidhogg's own `glitnir_pgtk_params_changed`).
//!
//! A child frame's window is a sync subsurface of its parent frame's (shell kind `NHSHELL_KIND_CHILD`), published and
//! presented with its tree: its position, stacking, input region and mapping are its parent's state, recorded by the
//! shell and sent with the tree's commit.  It has its own backing and Glitnir root; the root's one layer carries the
//! frame's chrome (rounded corners, shadow).  The shadow needs room around the native area: that margin is the frame's
//! border (`csd.rs` holds it, as it holds a top-level's title bar), so the backing, the damage and the pointer use the
//! offsets they already have.  The surface is the native area plus the margin; its input region is the native area.
//!
//! The facts it reads (design §3.3): the parent, the position (the frame's left and top), visibility and
//! `no-accept-focus` are pushed (`nhg_child_set`, when the window is made, moved, shown or hidden); the chrome is
//! pulled from C's `glitnir--resolve` call (`nhg_child_chrome`); the native size at each publish (`nhg_csd_paint`);
//! a raise or lower (`nhg_child_raise`).  The stacking order among siblings is the glue's (`Stacking`, moved from C's
//! `child_z`): the surface list C passes to `nhg_present` is put in it.  The scale is the one of the frame's last
//! layout.  The keyboard focus redirect stays C.

use crate::backing::{NHG_GEOM_LAYER_CHANGED, NhgFrameGeom, NhgRect};
use crate::csd::NhgCsdLayout;
use crate::display::Display;
use crate::frame::{Frame, NHG_FRAME_CHILD};
use crate::guard::GlueError;
use crate::present::NhgSurface;
use glitnir::ffi::glitnir_layer_set_chrome;
use glitnir::geom::Rect;
use glitnir::scene::Chrome;
use std::ptr;

/// `nhg_child_set` flags: the child is shown (its surface is mapped); the frame is `no-accept-focus`.
pub const NHG_CHILD_VISIBLE: u32 = 1;
pub const NHG_CHILD_NO_FOCUS: u32 = 2;
const FLAGS: u32 = NHG_CHILD_VISIBLE | NHG_CHILD_NO_FOCUS;

/// The chrome `glitnir--resolve` gives a child frame, in logical pixels (C decodes the Lisp plist): `on` 0 for none,
/// the corner radius, the shadow's blur, offset, spread, opacity (0 to 1) and colour (`0xRRGGBB`).  Layout equals
/// `NhgChrome`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct NhgChrome {
    pub on: u32,
    pub radius: f32,
    pub blur: f32,
    pub ox: f32,
    pub oy: f32,
    pub spread: f32,
    pub opacity: f32,
    pub rgb: u32,
}

/// A child frame's state, on its window's frame.  A window made anew (a reparent) starts it over, and C pushes its
/// facts and chrome again.
#[derive(Debug, Default)]
pub struct Child {
    /// The parent's window (0: C has not pushed the facts yet).
    parent: u64,
    /// The native area's place in the parent's native area, device pixels (the frame's left and top).
    x: i32,
    y: i32,
    /// `NHG_CHILD_*` as last pushed.
    flags: u32,
    /// The frame's z-group as of the last raise or lower (Emacs's `enum z_group`: 0 none, 1 above, 2 below, 3
    /// above-suspended).
    z_group: i32,
    /// Its place among its siblings (moved from `struct nhwl_output`'s `child_z`): above all when made or raised,
    /// below all when lowered (`Stacking`); 0 before the first push.
    z: i32,
    /// The surface is shown (not unmapped).
    mapped: bool,
    chrome: NhgChrome,
    /// The chrome changed since the layer last took it, and the scale it took it at.
    chrome_dirty: bool,
    chrome_s120: u32,
    /// The Glitnir layer's rectangle as last handed over (None: not yet).
    layer: Option<NhgRect>,
    /// The surface's offset in the parent's surface (logical) and its input region (logical; all 0 for none), as
    /// last sent.
    pos: Option<(i32, i32)>,
    region: Option<[i32; 4]>,
}

impl Child {
    /// The parent's window (0: no facts pushed yet).
    pub fn parent(&self) -> u64 {
        self.parent
    }

    /// Whether the frame is `no-accept-focus`.
    pub fn no_focus(&self) -> bool {
        self.flags & NHG_CHILD_NO_FOCUS != 0
    }

    /// The frame's z-group as of its last raise or lower.
    pub fn z_group(&self) -> i32 {
        self.z_group
    }

    /// Whether the surface is mapped.
    pub fn mapped(&self) -> bool {
        self.mapped
    }

    /// Its place among its siblings: higher is above.
    pub fn z(&self) -> i32 {
        self.z
    }
}

/// The display's stacking counters (moved from `nhwl_z_top` and `nhwl_z_bottom`): a child made or raised takes a
/// number above all, a lowered one below all.
#[derive(Debug, Default)]
pub struct Stacking {
    top: i32,
    bottom: i32,
}

impl Stacking {
    fn above_all(&mut self) -> i32 {
        self.top = self.top.wrapping_add(1);
        self.top
    }

    fn below_all(&mut self) -> i32 {
        self.bottom = self.bottom.wrapping_sub(1);
        self.bottom
    }
}

/// `s`, a tree's surface list as `present::drawable` accepts it (the root first, each parent before its children, no
/// window twice), in stacking order: each surface followed by its children lowest first by `z` (equals in the list's
/// order), each followed by its own.  Moved from `nhwl_surface_add`'s pass per stacking level; C lists the children
/// unsorted.
pub fn stacked(s: &[NhgSurface], z: impl Fn(u64) -> i32) -> Vec<NhgSurface> {
    let mut out = Vec::with_capacity(s.len());
    let mut todo = if s.is_empty() { vec![] } else { vec![0] };
    while let Some(i) = todo.pop() {
        out.push(s[i]);
        let mut kids: Vec<usize> = (1..s.len())
            .filter(|&j| s[j].parent == s[i].window)
            .collect();
        kids.sort_by_key(|&j| z(s[j].window));
        todo.extend(kids.into_iter().rev());
    }
    out
}

/// The surface list `s` (checked by `present::drawable`) in stacking order, from the children's places the glue
/// keeps.
pub fn stack_order(state: &Display, s: &[NhgSurface]) -> Vec<NhgSurface> {
    stacked(s, |w| state.frame(w).map_or(0, |f| f.child.z))
}

/// Device pixels to logical at `s120`/120 (0: 1), rounded to nearest, half away from zero (C's `lround`).
fn to_logical(dev: f64, s120: u32) -> i32 {
    let s = if s120 == 0 { 120 } else { s120 };
    (dev * 120.0 / f64::from(s)).round() as i32
}

/// A new child window's offset in its parent's surface for the shell, logical pixels at `s120`: (`x`, `y`) is where
/// the surface starts in the parent's native area (device pixels: the frame's left and top less its own margin), and
/// the parent's native area starts at (`pl`, `pt`) in its surface.  Rounded to nearest, also for negatives.  Moved
/// from `nhwl_child_describe`.
pub fn new_offset(pl: i32, pt: i32, x: i32, y: i32, s120: u32) -> (i32, i32) {
    let at = |p: i32, v: i32| to_logical(f64::from(p.wrapping_add(v)), s120);
    (at(pl, x), at(pt, y))
}

/// `new_offset` for the window `d` describes, whose parent the glue knows (an unknown parent has no border; the shell
/// refuses it).
pub(crate) fn new_window_offset(state: &Display, d: &crate::window::NhgWindowDesc) -> (i32, i32) {
    let [pl, pt, ..] = state.frame(d.parent).map_or([0; 4], |p| p.csd.border().0);
    new_offset(pl, pt, d.x, d.y, d.scale120)
}

/// The root of `window`'s tree and where `window`'s native area starts in the root's surface, device pixels: the
/// root's border plus each child's position on the way up (the frames of a tree share one scale).  Moved from
/// `nhwl_native_origin`.  `State` for a window the glue does not know, or a chain that does not end.
pub fn native_origin(state: &Display, window: u64) -> Result<(u64, i32, i32), GlueError> {
    let (mut w, mut x, mut y) = (window, 0i32, 0i32);
    for _ in 0..=state.frames().count() {
        let f = state.frame(w)?;
        if f.flags & NHG_FRAME_CHILD == 0 || f.child.parent == 0 {
            let ([l, t, ..], _) = f.csd.border();
            return Ok((w, x.wrapping_add(l), y.wrapping_add(t)));
        }
        x = x.wrapping_add(f.child.x);
        y = y.wrapping_add(f.child.y);
        w = f.child.parent;
    }
    Err(GlueError::State)
}

/// The frame's scale times 120: its last layout's, or the one its window was made at (0: none known, as 120).
fn scale120(f: &Frame) -> u32 {
    match f.csd.border().1 {
        0 => f.scale120,
        s => s,
    }
}

/// The room the shadow needs on each side (L, T, R, B), device pixels at `s120`: the layer grown by the spread and
/// the blur's reach (3 sigma, sigma = blur / 2; one pixel more for rounding), moved by the offset.
fn margins(c: &NhgChrome, s120: u32) -> [i32; 4] {
    // as C's `opacity > 0`: none at 0 or below, or NaN
    if c.on == 0 || c.opacity.is_nan() || c.opacity <= 0.0 {
        return [0; 4];
    }
    let s = f64::from(if s120 == 0 { 120 } else { s120 }) / 120.0;
    let pad = ((1.5 * f64::from(c.blur) * s).ceil() as i32)
        .saturating_add(1)
        .saturating_add((f64::from(c.spread) * s).round() as i32);
    let ox = (f64::from(c.ox) * s).round() as i32;
    let oy = (f64::from(c.oy) * s).round() as i32;
    [
        pad.saturating_sub(ox).max(0),
        pad.saturating_sub(oy).max(0),
        pad.saturating_add(ox).max(0),
        pad.saturating_add(oy).max(0),
    ]
}

/// The frame of child `window`; `State` for a window that is not a registered child.
fn child_mut(state: &mut Display, window: u64) -> Result<&mut Frame, GlueError> {
    let f = state.frame_mut(window)?;
    if f.flags & NHG_FRAME_CHILD == 0 {
        return Err(GlueError::State);
    }
    Ok(f)
}

/// Lay child `window`'s border out again (`nhg_csd_layout` for a child frame): the shadow margin its chrome needs at
/// `s120`, with no title strip.  Whether it changed, and the layout.
pub fn layout(
    state: &mut Display,
    window: u64,
    s120: u32,
) -> Result<(bool, NhgCsdLayout), GlueError> {
    let f = child_mut(state, window)?;
    let m = margins(&f.child.chrome, s120);
    let changed = f.csd.set_margins(s120, m);
    Ok((
        changed,
        NhgCsdLayout {
            l: m[0],
            t: m[1],
            r: m[2],
            b: m[3],
            strip: 0,
            on: 0,
        },
    ))
}

/// Where child `window`'s surface sits in its parent's surface, logical pixels at the parent's scale: its native area
/// at the frame's left and top in the parent's native area, the surface starting the shadow margin before it.
fn position(state: &Display, window: u64) -> Result<(i32, i32), GlueError> {
    let f = state.frame(window)?;
    let p = state.frame(f.child.parent)?;
    let ([pl, pt, ..], _) = p.csd.border();
    let ([l, t, ..], _) = f.csd.border();
    let s = scale120(p);
    let at = |parent: i32, own: i32, margin: i32| {
        to_logical(f64::from(parent) + f64::from(own) - f64::from(margin), s)
    };
    Ok((at(pl, f.child.x, l), at(pt, f.child.y, t)))
}

/// Send child `window`'s position if it changed: it takes effect with its parent's next commit (Wayland's rule), so
/// the shell marks the child and every ancestor owed, and the tree's pacer commits them, deepest first.  A failed
/// request is tried again at the next change or publish.
fn send_position(state: &mut Display, window: u64) {
    let Ok(pos) = position(state, window) else {
        return;
    };
    let Ok((backend, _, f)) = state.csd_parts(window) else {
        return;
    };
    if f.child.pos == Some(pos) {
        return;
    }
    if backend.set_position(window, pos.0, pos.1).is_ok() {
        f.child.pos = Some(pos);
    }
}

/// C pushed child `window`'s facts: its parent's window, its native area's place in the parent's (device pixels) and
/// `NHG_CHILD_*` flags.  The first push after the window is made records where it was made (`new_offset` placed it
/// at this position), that it is mapped, and its place above its siblings; a later one sends the position if it changed, and maps or
/// unmaps the surface as `NHG_CHILD_VISIBLE` says.  `true` when the surface was mapped again: all of it is drawn
/// again.
pub fn set(
    state: &mut Display,
    window: u64,
    parent: u64,
    x: i32,
    y: i32,
    flags: u32,
) -> Result<bool, GlueError> {
    if flags & !FLAGS != 0 || parent == window {
        return Err(GlueError::Arg);
    }
    state.frame(parent)?;
    let f = child_mut(state, window)?;
    f.facts.parent = parent;
    let c = &mut f.child;
    let first = c.parent == 0;
    (c.parent, c.x, c.y, c.flags) = (parent, x, y, flags);
    if first {
        c.mapped = true;
        // a window made goes above its siblings
        let z = state.stacking.above_all();
        let pos = position(state, window).ok();
        let c = &mut child_mut(state, window)?.child;
        (c.z, c.pos) = (z, pos);
        return Ok(false);
    }
    send_position(state, window);
    let on = flags & NHG_CHILD_VISIBLE != 0;
    let (backend, _, f) = state.csd_parts(window)?;
    let c = &mut f.child;
    // as C did, the shell's answer does not stop the change: a lost connection reaches C as the next event
    if on && !c.mapped {
        let _ = backend.set_mapped(window, true);
        c.mapped = true;
        return Ok(true);
    }
    if !on && c.mapped {
        let _ = backend.set_mapped(window, false);
        c.mapped = false;
    }
    Ok(false)
}

/// Child `window`'s chrome as `glitnir--resolve` gave it now: whether it changed (C then lays the frame out again,
/// which gives the margin, and draws it again).  The layer takes it at the next `nhg_layer_ensure`.
pub fn chrome(state: &mut Display, window: u64, c: &NhgChrome) -> Result<bool, GlueError> {
    let f = child_mut(state, window)?;
    let n = if c.on == 0 { NhgChrome::default() } else { *c };
    if f.child.chrome == n {
        return Ok(false);
    }
    f.child.chrome = n;
    f.child.chrome_dirty = true;
    Ok(true)
}

/// The sibling a raised (`raise`) or lowered child is stacked against, and whether it goes above it: the highest of
/// its `siblings` (window, place) for a raise, the lowest for a lower (None: alone).  Moved from
/// `nhwl_frame_raise_lower`, whose sorted list put the child at its top or bottom and stacked it against its
/// neighbour there (the last of equal highest, the first of equal lowest, as its stable sort left them).
pub fn neighbour(siblings: &[(u64, i32)], raise: bool) -> Option<(u64, bool)> {
    let it = siblings.iter().copied();
    let w = if raise {
        it.max_by_key(|s| s.1)
    } else {
        it.min_by_key(|s| s.1)
    };
    w.map(|(w, _)| (w, raise))
}

/// Child `window` was raised (`raise`) or lowered, with its frame's `z_group`: it takes a place above or below all its
/// siblings, and goes right above the highest of them, or below the lowest (stacking is the parent's state: the
/// parent and its ancestors owe a commit); alone, only its place changes.
pub fn raise(state: &mut Display, window: u64, raise: bool, z_group: i32) -> Result<(), GlueError> {
    let parent = child_mut(state, window)?.child.parent;
    let z = if raise {
        state.stacking.above_all()
    } else {
        state.stacking.below_all()
    };
    let siblings: Vec<(u64, i32)> = state
        .child_windows(parent)
        .into_iter()
        .filter(|&w| w != window && parent != 0)
        .map(|w| (w, state.frame(w).map_or(0, |f| f.child.z)))
        .collect();
    let next = neighbour(&siblings, raise);
    let (backend, _, f) = state.csd_parts(window)?;
    (f.child.z, f.child.z_group) = (z, z_group);
    if let Some((sibling, above)) = next {
        // as C did, a refused request changes nothing else
        let _ = backend.stack(window, sibling, above);
    }
    Ok(())
}

/// A top-level's border changed (its native area moved inside its surface): its child frames' surfaces follow, with
/// its next commit.
pub fn follow_parent(state: &mut Display, parent: u64) {
    for w in state.child_windows(parent) {
        send_position(state, w);
    }
}

/// The publish step of a drawable child `window` whose native size is `native_w` x `native_h` (from
/// `nhg_csd_paint`): its position and input region (the native area, without the shadow margin) go to the shell if
/// they changed, with the tree's next commit.  Nothing for any other window.
pub fn sync(state: &mut Display, window: u64, native_w: i32, native_h: i32) {
    match state.frame(window) {
        Ok(f) if f.flags & NHG_FRAME_CHILD != 0 && f.child.parent != 0 => {}
        _ => return,
    }
    send_position(state, window);
    let Ok((backend, _, f)) = state.csd_parts(window) else {
        return;
    };
    let s = scale120(f);
    let (m, _) = f.csd.border();
    let lg = |v: i32| to_logical(f64::from(v), s);
    let reg = if m != [0; 4] {
        [lg(m[0]), lg(m[1]), lg(native_w).max(1), lg(native_h).max(1)]
    } else {
        [0; 4]
    };
    let c = &mut f.child;
    if c.region == Some(reg) {
        return;
    }
    if c.region.is_none() && reg[2] == 0 {
        // the whole surface is the default
        c.region = Some(reg);
        return;
    }
    let rect = [Rect::new(reg[0], reg[1], reg[2], reg[3])];
    let rects: &[Rect] = if reg[2] != 0 { &rect } else { &[] };
    if backend.set_input_region(window, rects).is_ok() {
        c.region = Some(reg);
    }
}

/// `nhg_layer_ensure` for the frame of `g.window` when it is a child frame: its layer is its native area inside the
/// shadow margin when it has chrome (else all of the backing, which then is the native area), and
/// `NHG_GEOM_LAYER_CHANGED` is set when the layer, the chrome or the scale differ from what Glitnir holds, so the layer
/// is upserted and takes the chrome (`apply_chrome`).  The child's window, or `None` for any other frame.
pub fn layer(state: &mut Display, g: &mut NhgFrameGeom) -> Option<u64> {
    let f = state
        .frame_mut(g.window)
        .ok()
        .filter(|f| f.flags & NHG_FRAME_CHILD != 0)?;
    let c = &mut f.child;
    let s120 = if g.scale120 == 0 { 120 } else { g.scale120 };
    if c.chrome.on != 0 {
        g.layer = NhgRect {
            x: g.l,
            y: g.t,
            w: g.native_w,
            h: g.native_h,
        };
    }
    if c.layer != Some(g.layer) || c.chrome_dirty || c.chrome_s120 != s120 {
        g.flags |= NHG_GEOM_LAYER_CHANGED;
    }
    c.layer = Some(g.layer);
    Some(g.window)
}

/// Hand child `window`'s chrome, in device pixels at `s120` (0: 120), to the one layer of Glitnir root `key`, right
/// after the layer was upserted.
pub fn apply_chrome(state: &mut Display, window: u64, key: u64, s120: u32) {
    let Ok(f) = state.frame_mut(window) else {
        return;
    };
    let c = &mut f.child;
    let s120 = if s120 == 0 { 120 } else { s120 };
    let s = f64::from(s120) / 120.0;
    c.chrome_dirty = false;
    c.chrome_s120 = s120;
    let ch = c.chrome;
    let px = |v: f32| (f64::from(v) * s) as f32;
    let g = (ch.on != 0).then(|| Chrome {
        radius_px: px(ch.radius),
        blur_px: px(ch.blur),
        offset_x: px(ch.ox),
        offset_y: px(ch.oy),
        spread_px: px(ch.spread),
        shadow_opacity: ch.opacity,
        shadow_argb: 0xff00_0000 | ch.rgb,
        ..Chrome::default()
    });
    let p = g.as_ref().map_or(ptr::null(), |g| g as *const Chrome);
    // SAFETY: `p` is null or points to `g`, a readable Chrome for the call.  Glitnir's answer is ignored, as C did (a
    // root without that layer has nothing to dress).
    unsafe { glitnir_layer_set_chrome(key, key, p) };
}

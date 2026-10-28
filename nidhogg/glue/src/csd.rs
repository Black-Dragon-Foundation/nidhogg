//! The client-side title bar: its layout, hit tests and buttons' state, the window geometry, and the strip's draw
//! list.  Moved from `src/nhwlcsd.c` (W0, and W1 Task 19's full title bar; foot's render.c CSD was the reference, the
//! code was written fresh).  Its painting stays C: `nhwl_csd_draw` (`src/nhwldraw.c`) paints the `NhgCsdPaint` record
//! with Cairo, in the colours and font it resolves from Lisp.
//!
//! Where the compositor offers no server-side decorations (and the frame is not undecorated), the frame draws a title
//! strip and keeps resize margins around itself, inside the one surface: the backing is the native area plus this
//! border, Emacs draws translated into the native area, and the window geometry excludes the margins.  A fullscreen
//! frame has no border, a maximized one only the strip, and a tiled side no margin.  Everything is device pixels
//! except the protocol (the strip's 32 and the margin's 8 are logical).
//!
//! The facts it reads (design §3.3): the scale and `undecorated` are pulled (arguments of `nhg_csd_layout`), the title
//! and what else C paints from is pulled as one hash (`nhg_csd_paint`), the native size is pulled at each paint, and
//! the toplevel's states are the glue's own configure state.  Hit tests use the native size of the last paint, the
//! size the surface shows.

use crate::action::{NHG_ACT_DELETE, NHG_ACT_TITLE_BAR, NhgAction};
use crate::backing::NhgRect;
use crate::display::Display;
use crate::frame::NHG_FACT_CSD;
use crate::guard::GlueError;
use nhshell::api::{
    NHSHELL_CURSOR_DEFAULT, NHSHELL_CURSOR_RESIZE_EW, NHSHELL_CURSOR_RESIZE_NESW,
    NHSHELL_CURSOR_RESIZE_NS, NHSHELL_CURSOR_RESIZE_NWSE, NHSHELL_EDGE_BOTTOM, NHSHELL_EDGE_LEFT,
    NHSHELL_EDGE_RIGHT, NHSHELL_EDGE_TOP,
};
use nhshell::event::*;

/// Button kinds (`nhg_csd_style`, `NhgCsdPaint`): the Lisp style's `close`, `maximize`, `minimize` and `appmenu`.
pub const NHG_CSD_CLOSE: u32 = 1;
pub const NHG_CSD_MAX: u32 = 2;
pub const NHG_CSD_MIN: u32 = 3;
pub const NHG_CSD_MENU: u32 = 4;
/// A button's state in `NhgCsdPaint`: under the pointer, and pressed (armed) as well.
pub const NHG_BTN_HOVER: u32 = 1;
pub const NHG_BTN_PRESS: u32 = 2;
/// The most buttons a strip has (both sides).
pub const NHG_CSD_MAX_BUTTONS: usize = 8;
/// `nhg_csd_layout` flags: the frame is `undecorated`.
pub const NHG_CSD_UNDECORATED: u32 = 1;
/// `TITLE_BAR` action flags: the strip's look changed (C redisplays the frame); the pointer is over the border (C
/// clears the mouse face and shows the action's `code`, an `NHSHELL_CURSOR_*`); the pointer left the border for the
/// native area (C forgets its pointer shape, so Emacs sets its own at the next glyph).
pub const NHG_TB_REDRAW: u32 = 1;
pub const NHG_TB_POINTER: u32 = 2;
pub const NHG_TB_LEFT: u32 = 4;

/// Logical pixels: the strip's height and the resize margin.
const STRIP: i32 = 32;
const MARGIN: i32 = 8;
const DOUBLE_CLICK_NS: u64 = 400_000_000;
/// evdev's `BTN_LEFT` and `BTN_RIGHT`.
const BTN_LEFT: u32 = 0x110;
const BTN_RIGHT: u32 = 0x111;
/// The largest native side and scale the glue takes (a surface stops at 16384 a side; scale 64 is far beyond any
/// output), so that the layout's arithmetic cannot overflow.
const MAX_SIDE: i32 = 1 << 16;
const MAX_S120: u32 = 120 * 64;

/// The layout `nhg_csd_layout` gives: the native area's offsets in the backing (T includes the strip), the strip's
/// height (0: none), and whether the frame draws its own title bar.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct NhgCsdLayout {
    pub l: i32,
    pub t: i32,
    pub r: i32,
    pub b: i32,
    pub strip: i32,
    pub on: u32,
}

/// One button of the strip: its kind (`NHG_CSD_*`), its state (0, `NHG_BTN_HOVER` or `NHG_BTN_PRESS`) and its cell.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct NhgCsdButton {
    pub kind: u32,
    pub state: u32,
    pub r: NhgRect,
}

/// The strip's draw list (device pixels of the backing): the strip, the box the title is centred in (C clips to it),
/// whether the window is active, and the buttons.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct NhgCsdPaint {
    pub strip: NhgRect,
    pub title: NhgRect,
    pub active: u32,
    pub nbuttons: u32,
    pub button: [NhgCsdButton; NHG_CSD_MAX_BUTTONS],
}

/// The title bars' style, from Lisp (`nhwl--set-title-bar-style`): the buttons of each side in order.  Before Lisp
/// says, the buttons are W0's.  `generation` counts the changes.
#[derive(Debug, Default)]
pub struct Style {
    styled: bool,
    left: Vec<u32>,
    right: Vec<u32>,
    generation: u32,
}

/// What the strip was last painted for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Look {
    native: (i32, i32),
    offsets: [i32; 4],
    look: u64,
    states: u32,
    hover: u32,
    down: u32,
    style: u32,
}

/// One frame's title bar.
#[derive(Debug, Default)]
pub struct Csd {
    /// The compositor offers no server decorations, and the frame is decorated.
    pub on: bool,
    /// The scale times 120 of the last layout (0: none yet, as 120).
    s120: u32,
    /// M: the resize margin of a side that has one (none when maximized, and none on a tiled side); L, R, B the
    /// sides', T the top margin and the strip.
    m: i32,
    strip: i32,
    l: i32,
    t: i32,
    r: i32,
    b: i32,
    /// The native size of the last paint: what the surface shows, which the hit tests use.
    native: (i32, i32),
    painted: Option<Look>,
    /// The window geometry last sent (logical), if any.
    geom: Option<[i32; 4]>,
    /// The pointer in the surface (device pixels).
    px: f64,
    py: f64,
    /// The armed button and the button under the pointer (`NHG_CSD_*`, 0: none).
    down: u32,
    hover: u32,
    /// A press in the border was consumed, so its release is too; the pointer is over the border.
    grab: bool,
    in_border: bool,
    /// The time of the last press on the strip (0: none), for the double click.
    click_ns: u64,
}

/// Where a point of the surface lies.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Region {
    /// The native area, or no title bar.
    None,
    /// A resize margin, with the `NHSHELL_EDGE_*` it borders.
    Edge(u32),
    Strip,
    Button(u32),
}

/// Device size of `logical` pixels at `s120`/120, as `nhwl_to_device` rounds (half away from zero, in f32 like the
/// presenter's mutter-compatible rounding).
fn to_device(logical: i32, s120: u32) -> i32 {
    (logical as f32 * s120 as f32 / 120.0f32).round() as i32
}

/// Logical size of `dev` device pixels at `s120`/120, as `nhwl_to_logical` rounds.
fn to_logical(dev: i32, s120: u32) -> i32 {
    let s = i64::from(s120.max(1));
    ((i64::from(dev) * 120 + s / 2) / s) as i32
}

impl Csd {
    fn s120(&self) -> u32 {
        if self.s120 == 0 { 120 } else { self.s120 }
    }

    fn offsets(&self) -> [i32; 4] {
        [self.l, self.t, self.r, self.b]
    }

    /// The backing's width for the native width `w`.
    fn width(&self, w: i32) -> i32 {
        w + self.l + self.r
    }

    /// The strip's top: below the top margin.
    fn mbar(&self) -> i32 {
        self.t - self.strip
    }
}

/// The buttons for the native width `w`: each one's kind and the backing x of its cell (a strip-high square), the left
/// side's from the left edge in order, the right side's ending at the right edge.
fn buttons(style: &Style, c: &Csd, w: i32) -> ([(u32, i32); NHG_CSD_MAX_BUTTONS], usize) {
    let w = c.width(w);
    let mut v = [(0, 0); NHG_CSD_MAX_BUTTONS];
    let mut n = 0;
    if !style.styled {
        // W0's: minimize, maximize, close on the right
        for (k, kind) in [NHG_CSD_MIN, NHG_CSD_MAX, NHG_CSD_CLOSE]
            .into_iter()
            .enumerate()
        {
            v[n] = (kind, w - c.r - (3 - k as i32) * c.strip);
            n += 1;
        }
        return (v, n);
    }
    for &kind in style.left.iter().take(NHG_CSD_MAX_BUTTONS) {
        v[n] = (kind, c.l + n as i32 * c.strip);
        n += 1;
    }
    let nr = style.right.len().min(NHG_CSD_MAX_BUTTONS - n);
    for (k, &kind) in style.right.iter().take(nr).enumerate() {
        v[n] = (kind, w - c.r - (nr - k) as i32 * c.strip);
        n += 1;
    }
    (v, n)
}

/// Where the surface point (`dx`, `dy`) lies, for the native size of the last paint.
fn hit(style: &Style, c: &Csd, dx: f64, dy: f64) -> Region {
    if !c.on {
        return Region::None;
    }
    let (w, h) = (c.width(c.native.0), c.native.1 + c.t + c.b);
    let mbar = c.mbar();
    let mut edges = 0;
    // a tiled side has no margin
    if dx < f64::from(c.l) {
        edges |= NHSHELL_EDGE_LEFT;
    }
    if dx >= f64::from(w - c.r) {
        edges |= NHSHELL_EDGE_RIGHT;
    }
    if dy < f64::from(mbar) {
        edges |= NHSHELL_EDGE_TOP;
    }
    if dy >= f64::from(h - c.b) {
        edges |= NHSHELL_EDGE_BOTTOM;
    }
    if edges != 0 {
        return Region::Edge(edges);
    }
    if c.strip != 0
        && dy >= f64::from(mbar)
        && dy < f64::from(c.t)
        && dx >= f64::from(c.l)
        && dx < f64::from(w - c.r)
    {
        let (v, n) = buttons(style, c, c.native.0);
        return v[..n]
            .iter()
            .find(|&&(_, x)| dx >= f64::from(x) && dx < f64::from(x + c.strip))
            .map_or(Region::Strip, |&(kind, _)| Region::Button(kind));
    }
    Region::None
}

/// The pointer shape for a margin bordering `edges`.
fn cursor(edges: u32) -> u32 {
    const TL: u32 = NHSHELL_EDGE_TOP | NHSHELL_EDGE_LEFT;
    const BR: u32 = NHSHELL_EDGE_BOTTOM | NHSHELL_EDGE_RIGHT;
    const TR: u32 = NHSHELL_EDGE_TOP | NHSHELL_EDGE_RIGHT;
    const BL: u32 = NHSHELL_EDGE_BOTTOM | NHSHELL_EDGE_LEFT;
    match edges {
        TL | BR => NHSHELL_CURSOR_RESIZE_NWSE,
        TR | BL => NHSHELL_CURSOR_RESIZE_NESW,
        e if e & (NHSHELL_EDGE_TOP | NHSHELL_EDGE_BOTTOM) != 0 => NHSHELL_CURSOR_RESIZE_NS,
        e if e & (NHSHELL_EDGE_LEFT | NHSHELL_EDGE_RIGHT) != 0 => NHSHELL_CURSOR_RESIZE_EW,
        _ => NHSHELL_CURSOR_DEFAULT,
    }
}

/// The draw list for the native width `w`.
fn record(style: &Style, c: &Csd, states: u32, w: i32) -> NhgCsdPaint {
    let width = c.width(w);
    let mbar = c.mbar();
    let (v, n) = buttons(style, c, w);
    let mut p = NhgCsdPaint {
        strip: NhgRect {
            x: c.l,
            y: mbar,
            w: width - c.l - c.r,
            h: c.strip,
        },
        active: u32::from(states & NHSHELL_STATE_ACTIVATED != 0),
        nbuttons: n as u32,
        ..NhgCsdPaint::default()
    };
    // the title is centred between the buttons (GNOME's place)
    let (mut tl, mut tr) = (c.l, width - c.r);
    for (i, &(kind, x)) in v[..n].iter().enumerate() {
        let state = if c.down == kind && c.hover == kind {
            NHG_BTN_PRESS
        } else if c.hover == kind {
            NHG_BTN_HOVER
        } else {
            0
        };
        p.button[i] = NhgCsdButton {
            kind,
            state,
            r: NhgRect {
                x,
                y: mbar,
                w: c.strip,
                h: c.strip,
            },
        };
        if x < width / 2 {
            tl = tl.max(x + c.strip);
        } else {
            tr = tr.min(x);
        }
    }
    p.title = NhgRect {
        x: tl,
        y: mbar,
        w: (tr - tl).max(0),
        h: c.strip,
    };
    p
}

/// The window geometry to send now, if it changed: where the window sits inside the surface (the native area and the
/// strip, without the margins), in logical pixels.  Nothing for a frame that never had a border.
fn geometry(c: &Csd) -> Option<[i32; 4]> {
    let s120 = c.s120();
    let sw = to_logical(c.width(c.native.0), s120);
    let sh = to_logical(c.native.1 + c.t + c.b, s120);
    let ml = if c.l != 0 { MARGIN } else { 0 };
    let mr = if c.r != 0 { MARGIN } else { 0 };
    let mt = if c.t > c.strip { MARGIN } else { 0 };
    let mb = if c.b != 0 { MARGIN } else { 0 };
    let g = match (c.on, c.geom) {
        (false, None) => return None,
        (false, Some(_)) => [0, 0, sw, sh],
        (true, _) => [ml, mt, sw - ml - mr, sh - mt - mb],
    };
    (g[2] > 0 && g[3] > 0 && c.geom != Some(g)).then_some(g)
}

fn check_size(w: i32, h: i32) -> Result<(), GlueError> {
    if (0..=MAX_SIDE).contains(&w) && (0..=MAX_SIDE).contains(&h) {
        Ok(())
    } else {
        Err(GlueError::Arg)
    }
}

/// Lisp gave the title bars their style: the buttons of each side (`NHG_CSD_*`; other codes are ignored).  Every
/// title bar is painted again.
pub fn style(state: &mut Display, left: &[u32], right: &[u32]) {
    let valid = |k: &&u32| (NHG_CSD_CLOSE..=NHG_CSD_MENU).contains(*k);
    let s = &mut state.csd_style;
    s.left = left.iter().filter(valid).copied().collect();
    s.right = right.iter().filter(valid).copied().collect();
    s.styled = true;
    s.generation = s.generation.wrapping_add(1);
}

/// Lay `window`'s border out again from the compositor's decoration answer, the toplevel's states, `s120` and
/// `flags` (`NHG_CSD_*`): whether it changed, and the layout.  The configure step reads the result (a state change
/// under our own title bar gives a content ack).
pub fn layout(
    state: &mut Display,
    window: u64,
    s120: u32,
    flags: u32,
) -> Result<(bool, NhgCsdLayout), GlueError> {
    if s120 > MAX_S120 {
        return Err(GlueError::Arg);
    }
    // the window's own answer: the compositor may grant one window server decorations and refuse another
    let ssd = state.backend().window_ssd(window);
    let f = state.frame_mut(window)?;
    let states = f.cfg.states;
    let c = &mut f.csd;
    c.s120 = s120;
    c.on = matches!(ssd, Ok(false)) && flags & NHG_CSD_UNDECORATED == 0;
    if c.on {
        f.facts.flags |= NHG_FACT_CSD;
    } else {
        f.facts.flags &= !NHG_FACT_CSD;
    }
    let s120 = c.s120();
    let (mut m, mut strip) = (0, 0);
    if c.on && states & NHSHELL_STATE_FULLSCREEN == 0 {
        strip = to_device(STRIP, s120);
        if states & NHSHELL_STATE_MAXIMIZED == 0 {
            m = to_device(MARGIN, s120);
        }
    }
    let side = |tiled: u32| if states & tiled != 0 { 0 } else { m };
    let new = (
        m,
        strip,
        side(NHSHELL_STATE_TILED_L),
        side(NHSHELL_STATE_TILED_T) + strip,
        side(NHSHELL_STATE_TILED_R),
        side(NHSHELL_STATE_TILED_B),
    );
    let changed = new != (c.m, c.strip, c.l, c.t, c.r, c.b);
    if changed {
        (c.m, c.strip, c.l, c.t, c.r, c.b) = new;
        c.painted = None;
    }
    Ok((
        changed,
        NhgCsdLayout {
            l: c.l,
            t: c.t,
            r: c.r,
            b: c.b,
            strip: c.strip,
            on: u32::from(c.on),
        },
    ))
}

/// `window`'s offsets around the native area: L, T, R, B.
pub fn offsets(state: &Display, window: u64) -> Result<[i32; 4], GlueError> {
    Ok(state.frame(window)?.csd.offsets())
}

/// The publish step of `window`, whose native size is `native_w` x `native_h`: send the window geometry if it
/// changed, then the strip's draw list when it must be painted (it is not what the backing shows for `look`, C's hash
/// of the title and what else it paints from), or `None`.
pub fn paint(
    state: &mut Display,
    window: u64,
    native_w: i32,
    native_h: i32,
    look: u64,
) -> Result<Option<NhgCsdPaint>, GlueError> {
    check_size(native_w, native_h)?;
    let (backend, style, f) = state.csd_parts(window)?;
    let states = f.cfg.states;
    let c = &mut f.csd;
    c.native = (native_w, native_h);
    if let Some(g) = geometry(c) {
        // as C did, a failed request is tried again at the next publish
        if backend.set_geometry(window, g[0], g[1], g[2], g[3]).is_ok() {
            c.geom = Some(g);
        }
    }
    if !c.on || c.t + c.b == 0 {
        return Ok(None);
    }
    let now = Look {
        native: c.native,
        offsets: c.offsets(),
        look,
        states,
        hover: c.hover,
        down: c.down,
        style: style.generation,
    };
    if c.painted == Some(now) {
        return Ok(None);
    }
    c.painted = Some(now);
    Ok(Some(record(style, c, states, native_w)))
}

/// `window`'s strip as laid out for the native size `native_w` x `native_h` (for `nhwl--title-bar`), or `None` when
/// it draws none.  Changes nothing.
pub fn title_bar(
    state: &mut Display,
    window: u64,
    native_w: i32,
    native_h: i32,
) -> Result<Option<NhgCsdPaint>, GlueError> {
    check_size(native_w, native_h)?;
    let (_, style, f) = state.csd_parts(window)?;
    let c = &f.csd;
    Ok((c.on && c.strip != 0).then(|| record(style, c, f.cfg.states, native_w)))
}

// ---- Pointer events (the event step) --------------------------------------------------------------------------------

/// A pointer event routed through its window's title bar: `true` when the title bar consumed it (its actions are
/// pushed), `false` when it goes on to C as a raw event, after the actions pushed here.  `false` for a window the glue
/// does not know.
pub fn pointer(state: &mut Display, ev: &NhShellEvent) -> bool {
    match ev.kind {
        NHSHELL_EV_POINTER_ENTER | NHSHELL_EV_POINTER_MOTION => motion(state, ev),
        NHSHELL_EV_POINTER_LEAVE => {
            leave(state, ev);
            false
        }
        NHSHELL_EV_BUTTON => button(state, ev),
        _ => false,
    }
}

/// A `TITLE_BAR` action of `flags` for `ev`'s window.
fn title_bar_action(ev: &NhShellEvent, flags: u32) -> NhgAction {
    let mut a = NhgAction::new(NHG_ACT_TITLE_BAR, ev.window, ev.time_ns);
    a.flags = flags;
    a
}

/// The pointer is at a surface point: a button under it is drawn hovered, and over the border it takes the border's
/// shape there.
fn motion(state: &mut Display, ev: &NhShellEvent) -> bool {
    let Ok((_, style, f)) = state.csd_parts(ev.window) else {
        return false;
    };
    // SAFETY: a POINTER_ENTER or POINTER_MOTION record carries `pointer`.
    let p = unsafe { ev.u.pointer };
    let c = &mut f.csd;
    c.px = p.x;
    c.py = p.y;
    let region = hit(style, c, c.px, c.py);
    let mut a = title_bar_action(ev, 0);
    let hover = match region {
        Region::Button(kind) => kind,
        _ => 0,
    };
    if hover != c.hover {
        c.hover = hover;
        if c.on {
            a.flags |= NHG_TB_REDRAW;
        }
    }
    let consumed = match region {
        Region::None => {
            if c.in_border {
                c.in_border = false;
                a.flags |= NHG_TB_LEFT;
            }
            false
        }
        Region::Edge(edges) => {
            c.in_border = true;
            a.flags |= NHG_TB_POINTER;
            a.code = cursor(edges) as i32;
            true
        }
        Region::Strip | Region::Button(_) => {
            c.in_border = true;
            a.flags |= NHG_TB_POINTER;
            a.code = NHSHELL_CURSOR_DEFAULT as i32;
            true
        }
    };
    if a.flags != 0 {
        state.actions.push(a);
    }
    consumed
}

/// The pointer left the surface: no button is hovered any more.
fn leave(state: &mut Display, ev: &NhShellEvent) {
    let Ok(f) = state.frame_mut(ev.window) else {
        return;
    };
    let c = &mut f.csd;
    if c.hover != 0 {
        c.hover = 0;
        if c.on {
            state.actions.push(title_bar_action(ev, NHG_TB_REDRAW));
        }
    }
}

/// A pointer button.  A press in the border starts a move or resize (left), the window menu (right), or arms a button
/// that the release over it fires; the release of a consumed press is consumed too.  A fired close becomes a
/// `DELETE` action.
fn button(state: &mut Display, ev: &NhShellEvent) -> bool {
    let window = ev.window;
    let Ok((backend, style, f)) = state.csd_parts(window) else {
        return false;
    };
    // SAFETY: a BUTTON record carries `pointer`.
    let p = unsafe { ev.u.pointer };
    let states = f.cfg.states;
    let maximized = states & NHSHELL_STATE_MAXIMIZED != 0;
    let c = &mut f.csd;
    let s = f64::from(c.s120()) / 120.0;
    let region = hit(style, c, c.px, c.py);
    let mut actions: [Option<NhgAction>; 2] = [None, None];
    // as C did, the shell's errors are not the drain's: a lost connection reaches C as the next event
    if p.state != NHSHELL_BUTTON_PRESSED {
        let armed = c.down;
        if !c.grab {
            return false;
        }
        c.grab = false;
        c.down = 0;
        if armed != 0 && c.on {
            actions[0] = Some(title_bar_action(ev, NHG_TB_REDRAW));
        }
        if armed != 0 && region == Region::Button(armed) {
            match armed {
                NHG_CSD_CLOSE => {
                    actions[1] = Some(NhgAction::new(NHG_ACT_DELETE, window, ev.time_ns));
                }
                NHG_CSD_MAX => {
                    let _ = backend.set_maximized(window, !maximized);
                }
                NHG_CSD_MIN => {
                    let _ = backend.minimize(window);
                }
                _ => {
                    // `appmenu': the window menu, below the button
                    let (v, n) = buttons(style, c, c.native.0);
                    let x = v[..n]
                        .iter()
                        .rev()
                        .find(|&&(kind, _)| kind == NHG_CSD_MENU)
                        .map_or(c.px as i32, |&(_, x)| x);
                    let _ = backend.show_menu(
                        window,
                        (f64::from(x) / s) as i32,
                        (f64::from(c.t) / s) as i32,
                    );
                }
            }
        }
    } else {
        if region == Region::None {
            return false;
        }
        c.grab = true;
        c.down = 0;
        match region {
            Region::Edge(edges) => {
                if p.button == BTN_LEFT {
                    let _ = backend.resize(window, edges);
                }
            }
            Region::Strip if p.button == BTN_LEFT => {
                if c.click_ns != 0 && ev.time_ns.wrapping_sub(c.click_ns) < DOUBLE_CLICK_NS {
                    c.click_ns = 0;
                    let _ = backend.set_maximized(window, !maximized);
                } else {
                    c.click_ns = ev.time_ns.max(1);
                    let _ = backend.move_window(window);
                }
            }
            Region::Strip => {
                if p.button == BTN_RIGHT {
                    let _ = backend.show_menu(window, (c.px / s) as i32, (c.py / s) as i32);
                }
            }
            Region::Button(kind) => {
                if p.button == BTN_LEFT {
                    c.down = kind;
                    if c.on {
                        actions[0] = Some(title_bar_action(ev, NHG_TB_REDRAW));
                    }
                }
            }
            Region::None => {}
        }
    }
    for a in actions.into_iter().flatten() {
        state.actions.push(a);
    }
    true
}

impl Csd {
    /// Whether a press in the border is held (its release will be consumed).
    pub fn grabbed(&self) -> bool {
        self.grab
    }

    /// The button under the pointer and the armed one (`NHG_CSD_*`, 0: none).
    pub fn buttons_state(&self) -> (u32, u32) {
        (self.hover, self.down)
    }
}

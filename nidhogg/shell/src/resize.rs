//! Our own edge-drag resize, the geometry: which band of a window's frame a press is in, and the frame a drag step
//! asks for (Option about the centre, Shift keeping the aspect ratio, both together, the minimum size).  Rectangles
//! are AppKit's: points, the origin at the bottom left, y up.  Platform-neutral, so its tests run on Linux.

/// Edges a drag moves: bits of [`Edges`].
pub const LEFT: u8 = 1;
pub const RIGHT: u8 = 2;
pub const BOTTOM: u8 = 4;
pub const TOP: u8 = 8;

/// The edges a press grabbed (`LEFT`, `RIGHT`, `BOTTOM`, `TOP`; two for a corner).
pub type Edges = u8;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

/// The band's sizes in points: how far outside the frame and inside it a press grabs an edge, and how far along an
/// edge from a corner it grabs the corner.
#[derive(Clone, Copy, Debug)]
pub struct Band {
    pub outside: f64,
    pub inside: f64,
    pub corner: f64,
}

/// AppKit's: a few points outside the frame, a few inside, and a corner of about a title bar's height.
pub const BAND: Band = Band {
    outside: 5.0,
    inside: 3.0,
    corner: 16.0,
};

/// The edges `p` grabs in `frame`'s band, or `None` outside it (or deep inside the frame).
pub fn hit(frame: Rect, p: (f64, f64), band: Band) -> Option<Edges> {
    let (x, y) = p;
    let (l, r, b, t) = (frame.x, frame.x + frame.w, frame.y, frame.y + frame.h);
    let within = |v: f64, lo: f64, hi: f64| v >= lo - band.outside && v <= hi + band.outside;
    if !within(x, l, r) || !within(y, b, t) {
        return None;
    }
    let near = |v: f64, edge: f64, outward: f64| {
        let d = (v - edge) * outward;
        d >= -band.inside && d <= band.outside
    };
    let mut e = 0;
    if near(x, l, -1.0) {
        e |= LEFT;
    } else if near(x, r, 1.0) {
        e |= RIGHT;
    }
    if near(y, b, -1.0) {
        e |= BOTTOM;
    } else if near(y, t, 1.0) {
        e |= TOP;
    }
    // along an edge, near a corner, the press takes the corner
    if e == LEFT || e == RIGHT {
        if y - b < band.corner {
            e |= BOTTOM;
        } else if t - y < band.corner {
            e |= TOP;
        }
    } else if e == BOTTOM || e == TOP {
        if x - l < band.corner {
            e |= LEFT;
        } else if r - x < band.corner {
            e |= RIGHT;
        }
    }
    (e != 0).then_some(e)
}

/// The modifiers of a drag step.
#[derive(Clone, Copy, Debug, Default)]
pub struct Mods {
    /// Option: about the centre.
    pub symmetric: bool,
    /// Shift: the aspect ratio held.
    pub aspect: bool,
}

/// The frame a drag of `edges` by (`dx`, `dy`) points from `start` asks for, at least `min` (w, h) in size.
pub fn drag(start: Rect, edges: Edges, dx: f64, dy: f64, mods: Mods, min: (f64, f64)) -> Rect {
    let twice = if mods.symmetric { 2.0 } else { 1.0 };
    let horizontal = edges & (LEFT | RIGHT) != 0;
    let vertical = edges & (BOTTOM | TOP) != 0;
    let dw = if edges & RIGHT != 0 {
        dx
    } else if edges & LEFT != 0 {
        -dx
    } else {
        0.0
    };
    let dh = if edges & TOP != 0 {
        dy
    } else if edges & BOTTOM != 0 {
        -dy
    } else {
        0.0
    };
    let mut w = start.w + dw * twice;
    let mut h = start.h + dh * twice;
    if mods.aspect && start.w > 0.0 && start.h > 0.0 {
        let s = match (horizontal, vertical) {
            (true, true) => (w / start.w).max(h / start.h),
            (true, false) => w / start.w,
            _ => h / start.h,
        };
        let s = s.max(min.0 / start.w).max(min.1 / start.h);
        (w, h) = (start.w * s, start.h * s);
    } else {
        (w, h) = (w.max(min.0), h.max(min.1));
    }
    // the side that stays: the opposite edge, or the centre about which it grows
    let place = |pos: f64, old: f64, new: f64, near: bool, far: bool| {
        if mods.symmetric {
            pos + (old - new) / 2.0
        } else if near && !far {
            pos + old - new
        } else {
            pos
        }
    };
    Rect {
        x: place(start.x, start.w, w, edges & LEFT != 0, edges & RIGHT != 0),
        y: place(start.y, start.h, h, edges & BOTTOM != 0, edges & TOP != 0),
        w,
        h,
    }
}

/// Drag steps while a redraw runs: the newest replaces the one waiting.
#[derive(Debug, Default)]
pub struct Coalesce<T> {
    waiting: Option<T>,
}

impl<T> Coalesce<T> {
    pub fn push(&mut self, step: T) {
        self.waiting = Some(step);
    }

    pub fn take(&mut self) -> Option<T> {
        self.waiting.take()
    }
}

#[cfg(test)]
#[path = "resize_tests.rs"]
mod tests;

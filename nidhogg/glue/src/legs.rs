//! Layout legs: Lisp asks the presenter for a picture of a window's layout before it draws a new one (the capture),
//! then for the leg that moves the old layout to the new one (`ShellBackend::layout_capture`, `layout_leg`).
//!
//! - **The vector:** Lisp resolves the window parameters `glitnir-layout-capture` and `glitnir-layout-motion` into a
//!   number vector (`glitnir--leg-vector`, whose slots `glitnir--leg-slots` in `lisp/glitnir.el` names), C copies it
//!   (`glitnir--layout-leg`), and [`decode`] checks it.  Rectangles are device pixels: the window's text area with the
//!   native area's origin, the items' with the text area's.
//! - **What goes to the shell:** a capture number above the one sent before asks for a capture of the text area (in
//!   the backing); a motion whose capture number is above the one sent before asks for its leg; an empty vector, or a
//!   text area that moved, ends the leg.  The shell's key for the window is its `sequence_number`.
//! - **The window stays part of its frame** while it has a capture or a leg (way (a) of the plan): the capture demotes
//!   it if it is its own output (`layers.rs`), so the leg's frames are the frame's.  Its next full-window scroll ends
//!   the leg (a scroll ends a leg), and promotes it as before.

use crate::backing::{Backings, NhgRect};
use crate::frame::NHG_FRAME_KINDS;
use crate::guard::GlueError;
use glitnir::ease::Curve;
use glitnir::geom::Rect;
use glitnir::layout::{Item, ItemKind, LayoutLeg};
use std::collections::BTreeMap;

pub const SLOT_CAPTURE: usize = 0;
pub const SLOT_X: usize = 1;
pub const SLOT_Y: usize = 2;
pub const SLOT_W: usize = 3;
pub const SLOT_H: usize = 4;
pub const SLOT_MOTION: usize = 5;
pub const SLOT_BACKGROUND: usize = 6;
pub const SLOT_MS: usize = 7;
pub const SLOT_X1: usize = 8;
pub const SLOT_Y1: usize = 9;
pub const SLOT_X2: usize = 10;
pub const SLOT_Y2: usize = 11;
pub const SLOT_STEP: usize = 12;
pub const SLOT_COUNT: usize = 13;
/// The slots before the items.
pub const HEAD: usize = 14;
/// Each item's slots: kind (1 move, 2 enter, 3 exit), id, its old rectangle, its new one.
pub const ITEM: usize = 10;

/// The vector, checked: the capture number and the text area, and the motion when it asks for one.
#[derive(Clone, Debug, PartialEq)]
pub struct Decoded {
    pub capture: u64,
    pub rect: NhgRect,
    /// The leg, its rectangles still the vector's: the text area's for the items.
    pub motion: Option<LayoutLeg>,
}

/// Check `v` and decode it; `None` for an empty vector (no capture, no leg).  `GlueError::Arg` for a length that is
/// not the items' count, a slot that is not finite or is out of range.
pub fn decode(v: &[f32]) -> Result<Option<Decoded>, GlueError> {
    if v.is_empty() {
        return Ok(None);
    }
    if v.len() < HEAD || v.iter().any(|x| !x.is_finite()) {
        return Err(GlueError::Arg);
    }
    // a whole number in 0..=max
    let whole = |x: f32, max: f32| {
        if x.fract() == 0.0 && (0.0..=max).contains(&x) {
            Ok(x as u64)
        } else {
            Err(GlueError::Arg)
        }
    };
    // a rectangle of whole pixels, at most a million from the origin
    let rect = |s: &[f32]| -> Result<NhgRect, GlueError> {
        // the size (slots 2 and 3) is not negative
        let bad = |i: usize, x: f32| x.fract() != 0.0 || x.abs() > 1e6 || (i >= 2 && x < 0.0);
        if s.iter().enumerate().any(|(i, x)| bad(i, *x)) {
            return Err(GlueError::Arg);
        }
        Ok(NhgRect {
            x: s[0] as i32,
            y: s[1] as i32,
            w: s[2] as i32,
            h: s[3] as i32,
        })
    };
    // numbers up to 2^24 are exact in a float
    let capture = whole(v[SLOT_CAPTURE], 16_777_216.0)?;
    let text = rect(&v[SLOT_X..SLOT_X + 4])?;
    let count = whole(v[SLOT_COUNT], 4096.0)? as usize;
    let from = whole(v[SLOT_MOTION], 16_777_216.0)?;
    if v.len() != HEAD + count * ITEM || capture == 0 || (from != 0 && from != capture) {
        return Err(GlueError::Arg);
    }
    if from == 0 {
        return Ok(Some(Decoded {
            capture,
            rect: text,
            motion: None,
        }));
    }
    let background = whole(v[SLOT_BACKGROUND], 16_777_215.0)? as u32;
    let ms = whole(v[SLOT_MS], 10_000.0)?;
    let (x1, y1, x2, y2) = (v[SLOT_X1], v[SLOT_Y1], v[SLOT_X2], v[SLOT_Y2]);
    let curve = Curve::new(x1, y1, x2, y2).ok_or(GlueError::Arg)?;
    let mut items = Vec::with_capacity(count);
    for s in v[HEAD..].as_chunks::<ITEM>().0 {
        let kind = match whole(s[0], 3.0)? {
            1 => ItemKind::Move,
            2 => ItemKind::Enter,
            3 => ItemKind::Exit,
            _ => return Err(GlueError::Arg),
        };
        let id = whole(s[1], 16_777_216.0)? as u32;
        let (from, to) = (rect(&s[2..6])?, rect(&s[6..10])?);
        let (from, to) = (glitnir_rect(from), glitnir_rect(to));
        items.push(Item {
            id,
            kind,
            from,
            start: from,
            to,
        });
    }
    Ok(Some(Decoded {
        capture,
        rect: text,
        motion: Some(LayoutLeg {
            capture,
            rect: glitnir_rect(text),
            items,
            background: 0xff00_0000 | background,
            duration_ns: ms * 1_000_000,
            curve,
            step: v[SLOT_STEP] != 0.0,
        }),
    }))
}

fn glitnir_rect(r: NhgRect) -> Rect {
    Rect::new(r.x, r.y, r.w, r.h)
}

/// What the glue last sent for a window.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Sent {
    capture: u64,
    motion: u64,
    /// The text area in the backing.
    rect: Rect,
}

/// A frame's windows with a capture or a leg, by `sequence_number`.
#[derive(Debug, Default)]
pub struct Legs {
    sent: BTreeMap<u64, Sent>,
    /// Legs a scroll ended, for the shell (`take_ended`).
    ended: Vec<u64>,
}

impl Legs {
    /// Window `seq` has a capture or a leg.
    pub fn holds(&self, seq: u64) -> bool {
        self.sent.contains_key(&seq)
    }

    /// Window `seq`'s leg ends (a scroll): the shell learns it from `take_ended`.
    pub fn end(&mut self, seq: u64) {
        if self.sent.remove(&seq).is_some() {
            self.ended.push(seq);
        }
    }
}

/// What to ask the shell for a window.
#[derive(Clone, Debug, PartialEq)]
pub enum Ask {
    /// A capture: its number and the text area in the backing.
    Capture(u64, Rect),
    Leg(LayoutLeg),
    End,
}

/// Window `seq` of frame `key` has the decoded vector `d` now: what to ask the shell, in order.  A capture demotes the
/// window when it is its own output.
pub fn apply_at(
    b: &mut Backings,
    key: u64,
    seq: u64,
    decoded: Option<Decoded>,
) -> Result<Vec<Ask>, i32> {
    let f = b.frame_mut(key)?;
    let (ox, oy) = f.origin();
    let prev = f.legs.sent.get(&seq).copied();
    let Some(d) = decoded else {
        // none: a leg that was asked for ends
        return Ok(match f.legs.sent.remove(&seq) {
            Some(_) => vec![Ask::End],
            None => Vec::new(),
        });
    };
    let text = Rect::new(d.rect.x + ox, d.rect.y + oy, d.rect.w, d.rect.h);
    let mut asks = Vec::new();
    // a text area that moved ends the leg; a new capture of it is asked anew
    let mut was = prev.unwrap_or_default();
    if prev.is_some_and(|p| p.rect != text) {
        asks.push(Ask::End);
        was = Sent::default();
    }
    if d.capture > was.capture {
        // the leg's frames are the frame's: the window is no output of its own while it has one
        crate::layers::demote(key, &mut f.layers, seq);
        asks.push(Ask::Capture(d.capture, text));
        was.capture = d.capture;
    }
    if let Some(mut leg) = d.motion.filter(|l| l.capture > was.motion) {
        leg.rect = text;
        // the items are the text area's
        let at = |r: Rect| r.translate(text.x, text.y);
        for it in &mut leg.items {
            (it.from, it.start, it.to) = (at(it.from), at(it.start), at(it.to));
        }
        was.motion = leg.capture;
        asks.push(Ask::Leg(leg));
    }
    was.rect = text;
    f.legs.sent.insert(seq, was);
    Ok(asks)
}

/// The windows of frame `key` whose legs a scroll ended since the last call.
pub fn take_ended(b: &mut Backings, key: u64) -> Vec<u64> {
    b.frame_mut(key)
        .map_or_else(|_| Vec::new(), |f| std::mem::take(&mut f.legs.ended))
}

/// `nhg_layout_leg`: window `seq` of frame `key` (shell window `window`, a top-level) has the vector `v`; the number
/// of the newest capture the presenter took of it (0: none).
pub fn apply(
    state: &mut crate::display::Display,
    window: u64,
    key: u64,
    seq: u64,
    v: &[f32],
) -> Result<u64, GlueError> {
    let kind = crate::layers::root(state, window)?.kind;
    if kind & NHG_FRAME_KINDS != 0 {
        return Err(GlueError::State);
    }
    let asks = apply_at(&mut state.backings, key, seq, decode(v)?)?;
    let backend = state.backend();
    for a in asks {
        match a {
            Ask::Capture(n, rect) => backend.layout_capture(window, seq, n, rect)?,
            Ask::Leg(leg) => backend.layout_leg(window, seq, Some(leg))?,
            Ask::End => backend.layout_leg(window, seq, None)?,
        }
    }
    Ok(backend.layout_captured(window, seq)?.captured)
}

/// The legs of frame `key`'s windows that a scroll ended go to the shell (`window`, its shell window): each ends at
/// once.
pub fn send_ended(state: &mut crate::display::Display, window: u64, key: u64) {
    for seq in take_ended(&mut state.backings, key) {
        let _ = state.backend().layout_leg(window, seq, None);
    }
}

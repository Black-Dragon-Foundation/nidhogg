//! Window cards: an Emacs window with the card parameters (`corner-radius`, `shadow`, `ring`; Lisp resolves them into
//! the window's card vector, which C lists with the window) is drawn as a card by Glitnir.
//!
//! - **The layer:** each card window gets a Glitnir WINDOW layer of its own, id `w->sequence_number | CARD_TAG`, at the
//!   window's outer rectangle (its edges without the dividers, the mode line in) in the backing, with the chrome
//!   converted to device px at the frame's scale and its [`Card`]: the colour beneath (what the corners show), the edge
//!   ring, and what the shadow and the ring keep out of: the other windows' outer rectangles, and everything outside
//!   the window area (the bounding box of the listed windows' outer rectangles), where the tab, tool and menu bars are.
//! - **Card groups:** windows whose cards name one group (`card-group`) and tile a rectangle as one column or row
//!   ([`glitnir::scene::card_group`]) get one card layer, the first window's (its id, chrome and card), at the
//!   rectangle, with the dividers between them as its fill.  Windows that stop tiling one fall back to their own cards
//!   in the same sync, so a group re-forms in one commit.
//! - **Scroll layers:** a card window may be promoted (`layers.rs`). Its output stays `seq | WINDOW_TAG` at the window
//!   box; the card is its own layer, so promotion and demotion never touch it, and a card's change never demotes.
//!   Glitnir composes the card's corners into the output's buffer and damages them when the output scrolls. The card
//!   layers stay above the frame's other window layers, so their corners go over every label.
//! - **When:** at each publish C lists the windows (`nhg_layers_sync`) while the frame has cards or own outputs;
//!   [`sync`] upserts what changed and removes the layers of windows that lost their card or are gone. A scale change
//!   applies the cards again, and so does the first sync after a reconnect ([`forget_all`]).

use crate::backing::{Backings, CARD_TAG, NhgRect, WINDOW_TAG};
use crate::chrome::SLOTS;
use crate::display::Display;
use crate::guard::GlueError;
use crate::layers::NhgLayerWindow;
use glitnir::geom::Rect;
use glitnir::scene::{Card, Chrome, Kind, LayerDesc, Placed, card_bounds};
use nhshell::api::CardImage;
use std::collections::BTreeMap;

/// Glitnir's `GLITNIR_LAYER_WINDOW`.
const LAYER_WINDOW: u32 = 2;

/// What a card layer was last given.
#[derive(Debug, Clone, PartialEq)]
struct Applied {
    outer: NhgRect,
    /// The card's rectangle (a group's) and the dividers it fills, root px.
    rect: Rect,
    fill: Vec<Rect>,
    s120: u32,
    origin: (i32, i32),
    card: [f32; SLOTS],
    keep_out: Vec<Rect>,
}

/// A frame's card layers, by window `sequence_number`.
#[derive(Debug, Default)]
pub struct Cards {
    applied: BTreeMap<u64, Applied>,
}

impl Cards {
    pub fn len(&self) -> usize {
        self.applied.len()
    }

    pub fn is_empty(&self) -> bool {
        self.applied.is_empty()
    }
}

/// A colour 0xRRGGBB at OPACITY, premultiplied ARGB (pixman's x·a/255, rounded).
fn premultiplied(rgb: u32, opacity: f32) -> u32 {
    let a = (opacity.clamp(0.0, 1.0) * 255.0).round() as u32;
    let mul = |x: u32| {
        let t = x * a + 0x80;
        ((t >> 8) + t) >> 8
    };
    (a << 24) | (mul((rgb >> 16) & 0xff) << 16) | (mul((rgb >> 8) & 0xff) << 8) | mul(rgb & 0xff)
}

/// Window `w`'s chrome and card at scale `s`, its rectangle `rect` and the others' `keep_out`; `None` when it has no
/// card (or a vector the glue refuses: the window is drawn plain).
fn card_of(w: &NhgLayerWindow, s: f64, keep_out: &[Rect]) -> Option<(Chrome, Card)> {
    let d = w.card().ok()??;
    let ch = d.chrome?;
    let px = |v: f32| (f64::from(v) * s) as f32;
    let chrome = Chrome {
        radius_px: px(ch.radius),
        blur_px: px(ch.blur),
        offset_x: px(ch.ox),
        offset_y: px(ch.oy),
        spread_px: px(ch.spread),
        shadow_opacity: ch.opacity,
        shadow_argb: 0xff00_0000 | ch.rgb,
        ..Chrome::default()
    };
    let card = Card {
        beneath_argb: 0xff00_0000 | d.beneath_rgb.unwrap_or(0),
        ring_argb: d.ring.map_or(0, |(rgb, o)| premultiplied(rgb, o)),
        // one logical px, whole device px
        ring_px: d.ring.map_or(0, |_| s.round().max(1.0) as u32),
        keep_out: keep_out.into(),
        ..Card::default()
    };
    Some((chrome, card))
}

/// How far past the window area the bands around it reach: no chrome reaches further than a surface's side.
const REACH: i32 = crate::csd::SURFACE_MAX_SIDE;

/// The four bands around the window area, the bounding box of `outers` (none without a window): what lies there (the
/// tab, tool and menu bars, the frame's edge) is no window, so only these keep a card's chrome off it.
fn outside(outers: impl Iterator<Item = Rect>) -> Vec<Rect> {
    let Some(a) = outers.reduce(|a, b| a.bounds(&b)) else {
        return Vec::new();
    };
    let (x0, x1) = (a.x - REACH, a.x + a.w + REACH);
    vec![
        Rect::new(x0, a.y - REACH, x1 - x0, REACH),
        Rect::new(x0, a.y + a.h, x1 - x0, REACH),
        Rect::new(x0, a.y, REACH, a.h),
        Rect::new(a.x + a.w, a.y, REACH, a.h),
    ]
}

/// The layer id of window `seq`'s card.
pub fn card_id(seq: u64) -> u64 {
    seq | CARD_TAG
}

/// What frame `windows`' cards are at scale `s120` with the layer's origin `origin`: per card (a group's first window
/// stands for it) its window, what it was given, its chrome and its card, in window order.
fn plan(
    windows: &[NhgLayerWindow],
    s120: u32,
    origin: (i32, i32),
) -> Vec<(u64, Applied, Chrome, Card)> {
    let s = f64::from(s120) / 120.0;
    let at = |r: NhgRect| {
        Rect::new(
            r.x.saturating_add(origin.0),
            r.y.saturating_add(origin.1),
            r.w,
            r.h,
        )
    };
    let outers: Vec<(u64, Rect)> = windows
        .iter()
        .map(|w| (w.seq, at(w.outer)))
        .filter(|(_, r)| !r.is_empty())
        .collect();
    let around = outside(outers.iter().map(|(_, r)| *r));
    // each card: its window, its rectangle, the windows it covers and the dividers it fills (a group's members are
    // in window order, so the first stands for them)
    let group_of = |w: &NhgLayerWindow| {
        w.card()
            .ok()
            .flatten()
            .filter(|d| d.chrome.is_some())
            .map_or(0, |d| d.group)
    };
    let mut units: Vec<(&NhgLayerWindow, Rect, Vec<u64>, Vec<Rect>)> = Vec::new();
    let mut grouped: Vec<u64> = Vec::new();
    for w in windows {
        let g = group_of(w);
        if g == 0 || grouped.contains(&w.seq) || at(w.outer).is_empty() {
            continue;
        }
        let members: Vec<&NhgLayerWindow> = windows
            .iter()
            .filter(|m| group_of(m) == g && !at(m.outer).is_empty())
            .collect();
        let seqs: Vec<u64> = members.iter().map(|m| m.seq).collect();
        let rects: Vec<Rect> = members.iter().map(|m| at(m.outer)).collect();
        let others: Vec<Rect> = outers
            .iter()
            .filter(|(q, _)| !seqs.contains(q))
            .map(|(_, r)| *r)
            .collect();
        if let Some((rect, fill)) = glitnir::scene::card_group(&rects, &others) {
            grouped.extend(&seqs);
            units.push((w, rect, seqs, fill));
        }
    }
    for w in windows {
        if !grouped.contains(&w.seq) {
            units.push((w, at(w.outer), vec![w.seq], Vec::new()));
        }
    }
    let mut out = Vec::new();
    for (w, rect, members, fill) in units {
        if rect.is_empty() {
            continue;
        }
        let keep_out: Vec<Rect> = outers
            .iter()
            .filter(|(q, _)| !members.contains(q))
            .map(|(_, r)| *r)
            .chain(around.iter().copied())
            .collect();
        let Some((chrome, mut card)) = card_of(w, s, &keep_out) else {
            continue;
        };
        if !fill.is_empty() {
            let rgb = w
                .card()
                .ok()
                .flatten()
                .and_then(|d| d.fill_rgb)
                .unwrap_or(0);
            card.fill = fill.clone().into();
            card.fill_argb = 0xff00_0000 | rgb;
        }
        let now = Applied {
            outer: w.outer,
            rect,
            fill,
            s120,
            origin,
            card: w.card,
            keep_out,
        };
        out.push((w.seq, now, chrome, card));
    }
    out
}

/// Frame `key` is published with its windows `windows`: each window with a card gets (or keeps) its card layer, those
/// of windows without one, or gone, leave the scene, and the card layers go above the frame's other window layers.
/// Returns the number of cards.  A frame without a scale yet has none.
pub fn sync_at(b: &mut Backings, key: u64, windows: &[NhgLayerWindow]) -> Result<u32, i32> {
    let f = b.frame_mut(key)?;
    let (s120, origin) = (f.scale120(), f.origin());
    if s120 == 0 {
        return Ok(0);
    }
    let s = f64::from(s120) / 120.0;
    let planned = plan(windows, s120, origin);
    let cards = &mut f.cards;
    let mut seen: Vec<u64> = Vec::new();
    glitnir::scene::with(key, |sc| {
        for (seq, now, chrome, card) in planned {
            seen.push(seq);
            if cards.applied.get(&seq) == Some(&now) {
                continue;
            }
            let (w, rect) = (seq, now.rect);
            let id = card_id(w);
            sc.upsert(&LayerDesc {
                id,
                parent: key,
                kind: LAYER_WINDOW,
                rect,
                scale: s,
                visible: 1,
            })?;
            sc.set_chrome(id, Some(chrome))?;
            sc.set_card(id, Some(card))?;
            cards.applied.insert(w, now);
        }
        let gone: Vec<u64> = cards
            .applied
            .keys()
            .filter(|q| !seen.contains(q))
            .copied()
            .collect();
        for seq in gone {
            sc.remove(card_id(seq));
            cards.applied.remove(&seq);
        }
        // the cards above every other window layer of the frame (an own output made later goes on top), in their order
        let kids = sc.children(key).to_vec();
        let is_card = |id: &u64| id & CARD_TAG != 0 && id & WINDOW_TAG == 0;
        let is_output = |id: &u64| id & WINDOW_TAG != 0;
        if let (Some(first), Some(last)) = (
            kids.iter().position(is_card),
            kids.iter().rposition(is_output),
        ) && first < last
        {
            let order: Vec<u64> = kids.iter().filter(|id| is_card(id)).copied().collect();
            for id in order {
                sc.raise(id)?;
            }
        }
        Ok(())
    })?;
    Ok(u32::try_from(seen.len()).unwrap_or(u32::MAX))
}

/// [`sync_at`] for a shell that shows the cards on layers of its own (`cards_on_layers`): no scene; when a card came,
/// went or changed, every card of the frame drawn alone ([`glitnir::compose::card_overlay`]: a corner shows its
/// neighbours' chrome, so one change draws them all again), placed in the native area.  `None` when nothing changed.
pub fn images_at(
    b: &mut Backings,
    key: u64,
    windows: &[NhgLayerWindow],
) -> Result<Option<Vec<CardImage>>, i32> {
    let f = b.frame_mut(key)?;
    let (s120, origin) = (f.scale120(), f.origin());
    if s120 == 0 {
        return Ok(None);
    }
    let s = f64::from(s120) / 120.0;
    let planned = plan(windows, s120, origin);
    let cards = &mut f.cards;
    if planned.len() == cards.applied.len()
        && planned
            .iter()
            .all(|(q, now, ..)| cards.applied.get(q) == Some(now))
    {
        return Ok(None);
    }
    cards.applied = planned
        .iter()
        .map(|(q, now, ..)| (*q, now.clone()))
        .collect();
    let scene: Vec<Placed> = planned
        .into_iter()
        .map(|(q, now, chrome, card)| Placed {
            id: card_id(q),
            parent: key,
            kind: Kind::Window,
            rect: now.rect,
            scale: s,
            bounds: card_bounds(now.rect, Some(&chrome), Some(&card)),
            chrome: Some(chrome),
            card: Some(card),
        })
        .collect();
    Ok(Some(
        scene
            .iter()
            .filter_map(|p| {
                let (r, px) = glitnir::compose::card_overlay(&scene, p.id, s)?;
                Some(CardImage {
                    id: p.id,
                    rect: (r.x - origin.0, r.y - origin.1, r.w as u32, r.h as u32),
                    argb: Some(px.into()),
                })
            })
            .collect(),
    ))
}

/// `nhg_layers_sync`'s card half for root `window` (key `frame`): [`sync_at`] on the display's backings, or
/// [`images_at`] handed to a shell that shows the cards itself.
pub fn sync(
    state: &mut Display,
    window: u64,
    frame: u64,
    windows: &[NhgLayerWindow],
) -> Result<u32, GlueError> {
    let on_layers =
        matches!(state.csd_parts(window), Ok((backend, ..)) if backend.cards_on_layers());
    if !on_layers {
        return Ok(sync_at(&mut state.backings, frame, windows)?);
    }
    if let Some(list) = images_at(&mut state.backings, frame, windows)?
        && let Ok((backend, ..)) = state.csd_parts(window)
    {
        // as for a child's chrome, a refused list is given again with the next change
        let _ = backend.set_cards(window, &list);
    }
    Ok(u32::try_from(count(&mut state.backings, frame)).unwrap_or(u32::MAX))
}

/// The next sync gives every frame's cards again (a reconnect: the shell's surfaces start over).
pub fn forget_all(b: &mut Backings) {
    for (_, f) in b.frames_mut() {
        f.cards.applied.clear();
    }
}

/// How many of frame `key`'s windows have a card layer (0 for a frame without a backing).
pub fn count(b: &mut Backings, key: u64) -> usize {
    b.frame_mut(key).map_or(0, |f| f.cards.len())
}

//! Window edges: an Emacs window with the parameter `glitnir-edges` (Lisp resolves it into `glitnir--edges`; C resolves
//! each place against the window's current matrix) has thin lines drawn over its text by Glitnir
//! ([`glitnir::scene::Edge`]): polylines with rounded bends, an arrowhead or dashes (the heap graph's references).
//!
//! - **The layer:** each window with edges gets a Glitnir WINDOW layer of its own, id `w->sequence_number |
//!   EDGE_TAG`, at the window's text area, holding all its edges in root device px, above its blocks and the frame's
//!   outputs.  One layer (one image on a shell that shows its cards as images) per window, kept while it does not
//!   change: a scroll that leaves the edges where they were changes nothing.
//! - **What C gives ([`NhgEdge`]):** the text area and up to 8 points in device px, flags, the colour as 0xRRGGBB and
//!   an opacity.  The sizes are the glue's, from the frame's scale: the line one logical px in whole device px (the
//!   blocks' rule), the bends' radius 6 logical px, the arrow 6 long, a dash and its gap 3 each.
//! - **When:** at each publish while the frame has blocks, edges or their layers ([`sync_at`]); a reconnect gives
//!   them all again ([`forget_all`]).

use crate::backing::{Backings, EDGE_TAG, NhgRect};
use crate::cards::premultiplied;
use crate::display::Display;
use crate::guard::GlueError;
use glitnir::geom::Rect;
use glitnir::scene::{Edge, LayerDesc};
use nhshell::api::CardImage;
use std::collections::BTreeMap;
use std::sync::Arc;

/// Glitnir's `GLITNIR_LAYER_WINDOW`.
const LAYER_WINDOW: u32 = 2;

/// [`NhgEdge::flags`]: an arrowhead at the last point.
pub const NHG_EDGE_ARROW: u32 = 1;
/// [`NhgEdge::flags`]: dashed.
pub const NHG_EDGE_DASH: u32 = 2;
/// The most points an edge has (the start, 6 bends, the end).
pub const NHG_EDGE_POINTS: usize = 8;

/// A point, device px with the native area's origin.  Layout equals `NhgPoint`.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct NhgPoint {
    pub x: i32,
    pub y: i32,
}

/// One edge of one Emacs window as C resolves it at publish.  Layout equals `NhgEdge`.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct NhgEdge {
    /// The window's `sequence_number`.
    pub seq: u64,
    /// The window's text area (`window_box (w, TEXT_AREA)`), device px with the native area's origin.
    pub clip: NhgRect,
    /// The start, the bends and the end; the first `npoints` count.
    pub points: [NhgPoint; NHG_EDGE_POINTS],
    pub npoints: u32,
    /// `NHG_EDGE_*`.
    pub flags: u32,
    /// The colour 0xRRGGBB and its opacity (0 to 1).
    pub rgb: i32,
    pub opacity: f32,
}

/// What an edge layer was last given: its rectangle, scale and edges.
type Applied = (Rect, u32, Arc<[Edge]>);

/// A frame's edge layers, by window `sequence_number`.
#[derive(Debug, Default)]
pub struct Edges {
    applied: BTreeMap<u64, Applied>,
    /// The images last given to a shell that shows them itself (`cards_on_layers`): id and place in the native area,
    /// so the cards' and blocks' lists keep them.
    pub(crate) sent: Vec<(u64, (i32, i32, u32, u32))>,
}

impl Edges {
    pub fn len(&self) -> usize {
        self.applied.len()
    }

    pub fn is_empty(&self) -> bool {
        self.applied.is_empty()
    }
}

/// The layer id of window `seq`'s edges.
pub fn edge_id(seq: u64) -> u64 {
    seq | EDGE_TAG
}

/// Each window's text area and edges (root device px) from C's list `edges`, at scale `s120` with the layer's origin
/// `origin`, by window.  An edge of fewer than 2 points, or with no colour, is left out.
pub fn plan(edges: &[NhgEdge], s120: u32, origin: (i32, i32)) -> BTreeMap<u64, (Rect, Vec<Edge>)> {
    let s = f64::from(s120) / 120.0;
    let px = |logical: f64| (logical * s).round().max(1.0) as u32;
    // one logical px, whole device px
    let line = s.round().max(1.0) as u32;
    let mut out: BTreeMap<u64, (Rect, Vec<Edge>)> = BTreeMap::new();
    for e in edges {
        let clip = Rect::new(
            e.clip.x.saturating_add(origin.0),
            e.clip.y.saturating_add(origin.1),
            e.clip.w,
            e.clip.h,
        );
        let n = (e.npoints as usize).min(NHG_EDGE_POINTS);
        let Ok(rgb) = u32::try_from(e.rgb) else {
            continue;
        };
        if clip.is_empty() || n < 2 {
            continue;
        }
        let points: Arc<[(i32, i32)]> = e.points[..n]
            .iter()
            .map(|p| (p.x.saturating_add(origin.0), p.y.saturating_add(origin.1)))
            .collect();
        let edge = Edge {
            points,
            width_px: line,
            radius_px: px(6.0),
            argb: premultiplied(rgb & 0xff_ffff, e.opacity),
            arrow_px: if e.flags & NHG_EDGE_ARROW != 0 {
                px(6.0)
            } else {
                0
            },
            dash_px: if e.flags & NHG_EDGE_DASH != 0 {
                px(3.0)
            } else {
                0
            },
        };
        let entry = out.entry(e.seq).or_insert_with(|| (clip, Vec::new()));
        entry.0 = clip;
        entry.1.push(edge);
    }
    out
}

/// Frame `key` is published with the edges `edges`: each window with edges gets (or keeps) its edge layer, those of
/// windows without any leave the scene, and the edge layers go on top of the frame's other layers.  Returns the number
/// of edge layers.  A frame without a scale yet has none.
pub fn sync_at(b: &mut Backings, key: u64, edges: &[NhgEdge]) -> Result<u32, i32> {
    let f = b.frame_mut(key)?;
    let (s120, origin) = (f.scale120(), f.origin());
    if s120 == 0 {
        return Ok(0);
    }
    let s = f64::from(s120) / 120.0;
    let planned = plan(edges, s120, origin);
    let applied = &mut f.edges.applied;
    glitnir::scene::with(key, |sc| {
        for (seq, (clip, v)) in &planned {
            if let Some((r, q, old)) = applied.get(seq)
                && *r == *clip
                && *q == s120
                && **old == **v
            {
                continue;
            }
            let id = edge_id(*seq);
            sc.upsert(&LayerDesc {
                id,
                parent: key,
                kind: LAYER_WINDOW,
                rect: *clip,
                scale: s,
                visible: 1,
            })?;
            let v: Arc<[Edge]> = v.as_slice().into();
            sc.set_edges(id, Arc::clone(&v))?;
            applied.insert(*seq, (*clip, s120, v));
        }
        let gone: Vec<u64> = applied
            .keys()
            .filter(|q| !planned.contains_key(q))
            .copied()
            .collect();
        for seq in gone {
            sc.remove(edge_id(seq));
            applied.remove(&seq);
        }
        // the edge layers on top of the frame's children (blocks and outputs made later go above them), in their order
        let kids = sc.children(key).to_vec();
        let is_edge = |id: &u64| id & EDGE_TAG != 0;
        if let (Some(first), Some(last)) = (
            kids.iter().position(is_edge),
            kids.iter().rposition(|id| !is_edge(id)),
        ) && first < last
        {
            let order: Vec<u64> = kids.iter().filter(|id| is_edge(id)).copied().collect();
            for id in order {
                sc.raise(id)?;
            }
        }
        Ok(())
    })?;
    Ok(u32::try_from(applied.len()).unwrap_or(u32::MAX))
}

/// [`sync_at`] for a shell that shows the cards on layers of its own (`cards_on_layers`, macOS): each window's edges
/// drawn alone over its text area ([`glitnir::compose::edge_overlay`]), one image per window placed in the frame's
/// native area, whenever any changed; `None` when nothing did (or the frame has no scale yet).  An empty list: the
/// edges went.
pub fn images_at(
    b: &mut Backings,
    key: u64,
    edges: &[NhgEdge],
) -> Result<Option<Vec<CardImage>>, i32> {
    let f = b.frame_mut(key)?;
    let (s120, origin) = (f.scale120(), f.origin());
    if s120 == 0 {
        return Ok(None);
    }
    let planned = plan(edges, s120, origin);
    let applied = &mut f.edges.applied;
    let same = planned.len() == applied.len()
        && planned.iter().all(|(seq, (clip, v))| {
            applied
                .get(seq)
                .is_some_and(|(r, q, old)| r == clip && *q == s120 && **old == **v)
        });
    if same {
        return Ok(None);
    }
    *applied = planned
        .iter()
        .map(|(seq, (clip, v))| (*seq, (*clip, s120, Arc::from(v.as_slice()))))
        .collect();
    let s = f64::from(s120) / 120.0;
    Ok(Some(
        planned
            .into_iter()
            .map(|(seq, (clip, v))| CardImage {
                id: edge_id(seq),
                rect: (
                    clip.x - origin.0,
                    clip.y - origin.1,
                    clip.w as u32,
                    clip.h as u32,
                ),
                argb: Some(glitnir::compose::edge_overlay(clip, &v, s).into()),
            })
            .collect(),
    ))
}

/// [`sync_at`] for frame `frame` of shell window `window`.  A shell that shows window cards on layers of its own
/// (`cards_on_layers`, macOS) shows the edges the same way: [`images_at`]'s images, with the frame's cards and block
/// images as they are, as the window's list.
pub fn sync(
    state: &mut Display,
    window: u64,
    frame: u64,
    edges: &[NhgEdge],
) -> Result<u32, GlueError> {
    let on_layers =
        matches!(state.csd_parts(window), Ok((backend, ..)) if backend.cards_on_layers());
    if !on_layers {
        return Ok(sync_at(&mut state.backings, frame, edges)?);
    }
    if let Some(list) = images_at(&mut state.backings, frame, edges)? {
        let (cards, blocks) = state
            .backings
            .frame_mut(frame)
            .map(|f| (f.cards.sent.clone(), f.blocks.sent.clone()))
            .unwrap_or_default();
        let mut full: Vec<CardImage> = cards
            .iter()
            .chain(blocks.iter())
            .map(|&(id, rect)| CardImage {
                id,
                rect,
                argb: None,
            })
            .collect();
        full.extend(list.iter().cloned());
        let given = match state.csd_parts(window) {
            Ok((backend, ..)) => backend.set_cards(window, &full).is_ok(),
            Err(_) => false,
        };
        let f = state.backings.frame_mut(frame)?;
        if given {
            f.edges.sent = list.iter().map(|c| (c.id, c.rect)).collect();
        } else {
            // refused: given again at the next publish
            f.edges.applied.clear();
        }
    }
    Ok(u32::try_from(count(&mut state.backings, frame)).unwrap_or(u32::MAX))
}

/// The next sync gives every frame's edges again (a reconnect: the shell's surfaces start over).
pub fn forget_all(b: &mut Backings) {
    for (_, f) in b.frames_mut() {
        f.edges.applied.clear();
        f.edges.sent.clear();
    }
}

/// How many of frame `key`'s windows have an edge layer (0 for a frame without a backing): while it is not 0, C lists
/// the frame's edges at each publish, so the publish after the last edge went still removes its layer.
pub fn count(b: &mut Backings, key: u64) -> usize {
    b.frame_mut(key).map_or(0, |f| f.edges.len())
}

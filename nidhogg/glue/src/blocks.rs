//! Window blocks: an Emacs window with the parameter `glitnir-blocks` (Lisp resolves it into `glitnir--blocks`; C
//! resolves each block's rows against the window's current matrix) has rings or rules drawn around runs of its rows by
//! Glitnir ([`glitnir::scene::Block`]).
//!
//! - **The layer:** each window with blocks gets a Glitnir WINDOW layer of its own, id `w->sequence_number |
//!   BLOCK_TAG`, at the window's text area in the backing, holding its blocks in root device px.  Compose draws them
//!   over the frame's pixels inside that rectangle; the rows give their own fill.  The block layers stay above the
//!   frame's own outputs, so a promoted window's buffer has them composed in.
//! - **Per frame:** C lists the blocks of every frame of a published tree, child frames too (each is a root of its
//!   own), in the frame's native-area coordinates: a child frame that moves keeps them, one whose scale changes gets
//!   them again at the new scale.
//! - **What C gives ([`NhgBlock`]):** the rectangle and the text area in device px, the radius in device px, the
//!   ring's and the beneath colour as 0xRRGGBB, and whether a side was cut off by scrolling (a block reaching past the
//!   text area is open there too).  The line is one logical px, whole device px.
//! - **When:** at each publish while the frame has blocks or block layers; [`sync_at`] upserts what changed and
//!   removes the layers of windows that lost their blocks.  A reconnect gives them all again ([`forget_all`]).

use crate::backing::{BLOCK_TAG, Backings, NhgRect, WINDOW_TAG};
use crate::cards::premultiplied;
use crate::display::Display;
use crate::guard::GlueError;
use glitnir::geom::Rect;
use glitnir::scene::{Block, BlockKind, LayerDesc};
use nhshell::api::CardImage;
use std::collections::BTreeMap;
use std::sync::Arc;

/// Glitnir's `GLITNIR_LAYER_WINDOW`.
const LAYER_WINDOW: u32 = 2;

/// [`NhgBlock::flags`]: a rule (the bottom line only), not a ring.
pub const NHG_BLOCK_RULE: u32 = 1;
/// [`NhgBlock::flags`]: the block goes on above the window's first text row (scrolled out): no top side.
pub const NHG_BLOCK_OPEN_TOP: u32 = 2;
/// [`NhgBlock::flags`]: the block goes on below the window's last text row: no bottom side.
pub const NHG_BLOCK_OPEN_BOTTOM: u32 = 4;
/// [`NhgBlock::flags`]: a bar (the whole rectangle filled in the line's colour), not a ring.
pub const NHG_BLOCK_BAR: u32 = 8;

/// One block of one Emacs window as C resolves it at publish.  Layout equals `NhgBlock`.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct NhgBlock {
    /// The window's `sequence_number`.
    pub seq: u64,
    /// The window's text area (`window_box (w, TEXT_AREA)`), device px with the native area's origin.
    pub clip: NhgRect,
    /// The block's rows from x0 to x1, device px with the native area's origin.
    pub rect: NhgRect,
    /// `NHG_BLOCK_*`.
    pub flags: u32,
    /// The outer corner radius, device px.
    pub radius: i32,
    /// The line's colour 0xRRGGBB (-1: none) and opacity (0 to 1).
    pub ring_rgb: i32,
    pub ring_opacity: f32,
    /// The colour beneath the block, 0xRRGGBB: what a ring's corners show outside its rounded path.
    pub beneath_rgb: i32,
}

/// What a block layer was last given: its rectangle, scale and blocks.
type Applied = (Rect, u32, Arc<[Block]>);

/// A frame's block layers, by window `sequence_number`.
#[derive(Debug, Default)]
pub struct Blocks {
    applied: BTreeMap<u64, Applied>,
    /// The block images last given to a shell that shows them itself (id and place): the cards' list carries them,
    /// as images kept, since the shell's list is the window's whole list.
    pub(crate) sent: Vec<(u64, (i32, i32, u32, u32))>,
}

impl Blocks {
    pub fn len(&self) -> usize {
        self.applied.len()
    }

    pub fn is_empty(&self) -> bool {
        self.applied.is_empty()
    }
}

/// The layer id of window `seq`'s blocks.
pub fn block_id(seq: u64) -> u64 {
    seq | BLOCK_TAG
}

/// Each window's text area and blocks (root device px) from C's list `blocks`, at scale `s120` with the layer's origin
/// `origin`, by window.  A block with no area, or outside its window, is left out.
pub fn plan(
    blocks: &[NhgBlock],
    s120: u32,
    origin: (i32, i32),
) -> BTreeMap<u64, (Rect, Vec<Block>)> {
    let s = f64::from(s120) / 120.0;
    let at = |r: NhgRect| {
        Rect::new(
            r.x.saturating_add(origin.0),
            r.y.saturating_add(origin.1),
            r.w,
            r.h,
        )
    };
    // one logical px, whole device px
    let line = s.round().max(1.0) as u32;
    let mut out: BTreeMap<u64, (Rect, Vec<Block>)> = BTreeMap::new();
    for b in blocks {
        let (clip, rect) = (at(b.clip), at(b.rect));
        if clip.is_empty() || rect.is_empty() || rect.intersect(&clip).is_empty() {
            continue;
        }
        let rgb = |c: i32| u32::try_from(c).ok().map(|c| c & 0xff_ffff);
        let block = Block {
            rect,
            kind: if b.flags & NHG_BLOCK_BAR != 0 {
                BlockKind::Bar
            } else if b.flags & NHG_BLOCK_RULE != 0 {
                BlockKind::Rule
            } else {
                BlockKind::Ring
            },
            radius_px: u32::try_from(b.radius).unwrap_or(0),
            ring_px: line,
            ring_argb: rgb(b.ring_rgb).map_or(0, |c| premultiplied(c, b.ring_opacity)),
            beneath_argb: 0xff00_0000 | rgb(b.beneath_rgb).unwrap_or(0),
            open_top: b.flags & NHG_BLOCK_OPEN_TOP != 0 || rect.y < clip.y,
            open_bottom: b.flags & NHG_BLOCK_OPEN_BOTTOM != 0 || rect.y + rect.h > clip.y + clip.h,
        };
        let e = out.entry(b.seq).or_insert_with(|| (clip, Vec::new()));
        e.0 = clip;
        e.1.push(block);
    }
    out
}

/// Frame `key` is published with the blocks `blocks`: each window with blocks gets (or keeps) its block layer, those
/// of windows without any leave the scene, and the block layers go above the frame's own outputs.  Returns the number
/// of block layers.  A frame without a scale yet has none.
pub fn sync_at(b: &mut Backings, key: u64, blocks: &[NhgBlock]) -> Result<u32, i32> {
    let f = b.frame_mut(key)?;
    let (s120, origin) = (f.scale120(), f.origin());
    if s120 == 0 {
        return Ok(0);
    }
    let s = f64::from(s120) / 120.0;
    let planned = plan(blocks, s120, origin);
    let applied = &mut f.blocks.applied;
    glitnir::scene::with(key, |sc| {
        for (seq, (clip, v)) in &planned {
            let id = block_id(*seq);
            if let Some((r, q, old)) = applied.get(seq)
                && *r == *clip
                && *q == s120
                && **old == **v
            {
                continue;
            }
            sc.upsert(&LayerDesc {
                id,
                parent: key,
                kind: LAYER_WINDOW,
                rect: *clip,
                scale: s,
                visible: 1,
            })?;
            let v: Arc<[Block]> = v.as_slice().into();
            sc.set_blocks(id, Arc::clone(&v))?;
            applied.insert(*seq, (*clip, s120, v));
        }
        let gone: Vec<u64> = applied
            .keys()
            .filter(|q| !planned.contains_key(q))
            .copied()
            .collect();
        for seq in gone {
            sc.remove(block_id(seq));
            applied.remove(&seq);
        }
        // the block layers above the frame's own outputs (an output made later goes on top), in their order
        let kids = sc.children(key).to_vec();
        let is_block = |id: &u64| id & BLOCK_TAG != 0 && id & WINDOW_TAG == 0;
        let is_output = |id: &u64| id & WINDOW_TAG != 0;
        if let (Some(first), Some(last)) = (
            kids.iter().position(is_block),
            kids.iter().rposition(is_output),
        ) && first < last
        {
            let order: Vec<u64> = kids.iter().filter(|id| is_block(id)).copied().collect();
            for id in order {
                sc.raise(id)?;
            }
        }
        Ok(())
    })?;
    Ok(u32::try_from(applied.len()).unwrap_or(u32::MAX))
}

/// [`sync_at`] for a shell that shows the cards on layers of its own (`cards_on_layers`, macOS): each window's blocks
/// drawn alone over its text area ([`glitnir::compose::block_overlay`]), as images placed in the frame's native area,
/// whenever any changed; `None` when nothing did (or the frame has no scale yet).  An empty list: the blocks went.
pub fn images_at(
    b: &mut Backings,
    key: u64,
    blocks: &[NhgBlock],
) -> Result<Option<Vec<CardImage>>, i32> {
    let f = b.frame_mut(key)?;
    let (s120, origin) = (f.scale120(), f.origin());
    if s120 == 0 {
        return Ok(None);
    }
    let planned = plan(blocks, s120, origin);
    let applied = &mut f.blocks.applied;
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
                id: block_id(seq),
                rect: (
                    clip.x - origin.0,
                    clip.y - origin.1,
                    clip.w as u32,
                    clip.h as u32,
                ),
                argb: Some(glitnir::compose::block_overlay(clip, &v, s).into()),
            })
            .collect(),
    ))
}

/// [`sync_at`] for frame `frame`, whose shell window is `window` (a child frame's own).  A shell that shows window
/// cards on layers of its own (`cards_on_layers`, macOS) shows the blocks the same way: [`images_at`]'s images, with
/// the frame's cards as they are, as the window's list.
pub fn sync(
    state: &mut Display,
    window: u64,
    frame: u64,
    blocks: &[NhgBlock],
) -> Result<u32, GlueError> {
    if !state.backend().cards_on_layers() {
        return Ok(sync_at(&mut state.backings, frame, blocks)?);
    }
    if let Some(list) = images_at(&mut state.backings, frame, blocks)? {
        let cards = state
            .backings
            .frame_mut(frame)
            .map(|f| f.cards.sent.clone())
            .unwrap_or_default();
        let mut full: Vec<CardImage> = cards
            .iter()
            .map(|&(id, rect)| CardImage {
                id,
                rect,
                argb: None,
            })
            .collect();
        full.extend(list.iter().cloned());
        let given = state.backend().set_cards(window, &full).is_ok();
        let f = state.backings.frame_mut(frame)?;
        if given {
            f.blocks.sent = list.iter().map(|c| (c.id, c.rect)).collect();
        } else {
            // refused: given again at the next publish
            f.blocks.applied.clear();
        }
    }
    Ok(u32::try_from(count(&mut state.backings, frame)).unwrap_or(u32::MAX))
}

/// The next sync gives every frame's blocks again (a reconnect: the shell's surfaces start over).
pub fn forget_all(b: &mut Backings) {
    for (_, f) in b.frames_mut() {
        f.blocks.applied.clear();
        f.blocks.sent.clear();
    }
}

/// How many of frame `key`'s windows have a block layer (0 for a frame without a backing): while it is not 0, C lists
/// the frame's blocks at each publish, so the publish after the last block went still removes its layer.
pub fn count(b: &mut Backings, key: u64) -> usize {
    b.frame_mut(key).map_or(0, |f| f.blocks.len())
}

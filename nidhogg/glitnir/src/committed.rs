//! A committed scene as an immutable snapshot.
//!
//! A commit ([`crate::damage::commit_snapshot`]) yields an `Arc<Committed>`: the root's visible layers in paint order,
//! their labels and the sprites those labels were placed from.  Nothing the host does afterwards changes it (a later
//! commit makes a new one), so a thread other than the host's can compose it
//! ([`crate::compose::compose_committed`]) without the library's global lock.

use crate::decor::{Labels, Sprites};
use crate::geom::Rect;
use crate::scene::Placed;
use std::sync::Arc;

/// A window layer that is its own output ([`crate::scene::Scene::set_output`]) as a commit left it.  To bring the
/// output's picture up to date: move its content by `scroll` rows ([`crate::pool::Ring::scroll`]), then redraw
/// `damage`; with `full`, redraw the whole of it instead.
#[derive(Debug, Clone, PartialEq)]
pub struct OutputFrame {
    pub layer: u64,
    /// The window's rectangle snapped outward to device rows and columns a subsurface can take (root device px).
    pub rect: Rect,
    /// How far the content moved since the last commit (negative: up), by full-window scrolls.
    pub scroll: i32,
    /// What changed, after the scroll, in the output's coordinates (its top-left is 0, 0).
    pub damage: Vec<Rect>,
    /// A new output, a moved or resized one, or a new scale: nothing of its last picture can be kept.
    pub full: bool,
}

/// One root's scene as a commit left it.
#[derive(Debug, Clone, Default)]
pub struct Committed {
    pub root: u64,
    /// The visible layers in paint order.
    pub layers: Vec<Placed>,
    /// The labels, in paint order within each window layer.
    pub labels: Arc<Labels>,
    /// The sprite cache as it was at the commit.
    pub sprites: Arc<Sprites>,
    /// The window layers that are their own outputs, in paint order.
    pub outputs: Vec<OutputFrame>,
}

// A snapshot crosses to the compose thread.
const _: fn() = || {
    fn shared<T: Send + Sync>() {}
    shared::<Committed>();
};

#[cfg(test)]
mod tests {
    use crate::decor::Anchor;
    use crate::ffi::{
        GLITNIR_OK, glitnir_damage_forget, glitnir_decor_put, glitnir_decor_sprite_put,
        glitnir_layer_upsert, glitnir_root_drop, glitnir_root_new,
    };
    use crate::geom::Rect;
    use crate::scene::LayerDesc;

    fn upsert(root: u64, id: u64, parent: u64, kind: u32, rect: Rect) {
        let d = LayerDesc {
            id,
            parent,
            kind,
            rect,
            scale: 1.0,
            visible: 1,
        };
        assert_eq!(unsafe { glitnir_layer_upsert(root, &d) }, GLITNIR_OK);
    }

    fn sprite(key: u64, colour: u32) {
        let px = [colour; 6 * 4];
        assert_eq!(
            unsafe { glitnir_decor_sprite_put(key, px.as_ptr().cast(), 6 * 4, 6, 4) },
            GLITNIR_OK
        );
    }

    #[test]
    fn snapshot_is_immutable_after_commit() {
        let (root, win, kid) = (8501, 8502, 8503);
        let key = 0x8501_0001;
        assert_eq!(glitnir_root_new(root), GLITNIR_OK);
        upsert(root, root, 0, 1, Rect::new(0, 0, 200, 100));
        upsert(root, win, root, 2, Rect::new(10, 10, 180, 80));
        upsert(root, kid, root, 3, Rect::new(50, 20, 40, 30));
        sprite(key, 0xff10_2030);
        let a = Anchor {
            glyph: Rect::new(20, 30, 8, 16),
            clip: Rect::new(0, 0, 180, 80),
            sprite: key,
            row_top: 0,
        };
        assert_eq!(unsafe { glitnir_decor_put(root, win, &a) }, GLITNIR_OK);
        let (damage, snap) = crate::damage::commit_snapshot(root).unwrap();
        assert!(!damage.is_empty());
        assert_eq!(snap.root, root);
        assert!(snap.outputs.is_empty());
        let layers = snap.layers.clone();
        let labels = (*snap.labels).clone();
        assert_eq!(
            layers.iter().map(|p| p.id).collect::<Vec<_>>(),
            [root, win, kid]
        );
        assert_eq!(labels.len(), 1);
        let generation = snap.sprites.peek(key).unwrap().2;

        // the host moves on: a layer moves, another appears, the label goes, its sprite is replaced
        upsert(root, kid, root, 3, Rect::new(90, 40, 40, 30));
        upsert(root, kid + 1, root, 3, Rect::new(0, 0, 10, 10));
        assert_eq!(crate::ffi::glitnir_decor_drop_all(root, win), GLITNIR_OK);
        sprite(key, 0xff40_5060);
        assert_eq!(snap.layers, layers);
        assert_eq!(*snap.labels, labels);
        let s = snap.sprites.lookup(key).unwrap();
        assert_eq!((s.generation(), s.px[0]), (generation, 0xff10_2030));

        // the next commit is a new snapshot; the old one is as it was
        let (_, next) = crate::damage::commit_snapshot(root).unwrap();
        assert!(!std::sync::Arc::ptr_eq(&snap, &next));
        assert_eq!(next.layers.len(), 4);
        assert!(next.labels.is_empty());
        assert_eq!(snap.layers, layers);
        assert_eq!(*snap.labels, labels);
        // unchanged labels are shared from one snapshot to the next
        let (_, again) = crate::damage::commit_snapshot(root).unwrap();
        assert!(std::sync::Arc::ptr_eq(&next.labels, &again.labels));

        // a root without a scene has no snapshot
        assert!(crate::damage::commit_snapshot(root + 100).is_none());
        assert_eq!(glitnir_root_drop(root), GLITNIR_OK);
        for f in [root, win, kid, kid + 1] {
            assert_eq!(glitnir_damage_forget(f), GLITNIR_OK);
        }
    }
}

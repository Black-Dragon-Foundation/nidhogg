//! Child windows on macOS, the pure half: the hit-test that picks the window under the pointer, and the overhang
//! decision that moves a child into a panel and back.  Platform-neutral, so its tests run on Linux; the macOS backend
//! (`macos::child`) feeds it its layers' places.
//!
//! A child window is a container layer in its parent's layer (design §5.6).  Only its **content** rectangle takes the
//! pointer: the input region the host gives (the native area inside the shadow margin), else all of its surface.  A
//! click on the shadow therefore reaches whatever lies beneath.  Children are tried top-down: a parent's children
//! above it, the last made or raised on top, each child's own children above it.
//!
//! A child of a root whose content rectangle crosses the root's bounds overhangs: its container moves into a panel.
//! It comes back once its content fits [`BAND`] points inside, so a child sitting on the edge switches once.

use glitnir::geom::Rect;

/// How far inside the root's bounds an overhanging child's content must come before it goes back (points).
pub const BAND: i32 = 2;

/// A child window as the hit-test and the overhang decision see it: its parent (a root or a child), its surface's
/// place in the parent's surface and its size (points), the input region in its own surface (`None`: all of it) and
/// whether it shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Node {
    pub id: u64,
    pub parent: u64,
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
    pub input: Option<Rect>,
    pub shown: bool,
}

impl Node {
    /// The content rectangle in the node's own surface.
    pub fn content(&self) -> Rect {
        self.input.unwrap_or(Rect::new(0, 0, self.w, self.h))
    }
}

/// Where `id`'s surface starts in its root's surface (points): the places on the way up.  A root is at 0, 0.
pub fn origin(nodes: &[Node], id: u64) -> (i32, i32) {
    let (mut x, mut y, mut at) = (0i32, 0i32, id);
    for _ in 0..=nodes.len() {
        let Some(n) = nodes.iter().find(|n| n.id == at) else {
            break;
        };
        x = x.saturating_add(n.x);
        y = y.saturating_add(n.y);
        at = n.parent;
    }
    (x, y)
}

/// The window under the point (`x`, `y`) of root `root`'s surface (points), and the point in that window's surface.
/// `nodes` lists each parent's children bottom to top, in the order they are stacked; a hidden child hides its own.
pub fn hit(root: u64, nodes: &[Node], x: f64, y: f64) -> (u64, f64, f64) {
    under(root, nodes, (0.0, 0.0), x, y, 0).unwrap_or((root, x, y))
}

/// The child of `parent` (whose surface starts at `at` in the root) or one of their own under the point, top-down.
fn under(
    parent: u64,
    nodes: &[Node],
    at: (f64, f64),
    x: f64,
    y: f64,
    depth: usize,
) -> Option<(u64, f64, f64)> {
    if depth > nodes.len() {
        return None;
    }
    for n in nodes.iter().rev().filter(|n| n.parent == parent && n.shown) {
        let o = (at.0 + f64::from(n.x), at.1 + f64::from(n.y));
        if let Some(h) = under(n.id, nodes, o, x, y, depth + 1) {
            return Some(h);
        }
        let (lx, ly) = (x - o.0, y - o.1);
        let c = n.content();
        let inside = lx >= f64::from(c.x)
            && ly >= f64::from(c.y)
            && lx < f64::from(c.x) + f64::from(c.w)
            && ly < f64::from(c.y) + f64::from(c.h);
        if inside {
            return Some((n.id, lx, ly));
        }
    }
    None
}

/// Whether a root's child overhangs now: its content rectangle `content` (points, in the root's surface) against the
/// root's `w` x `h`, and whether it overhung before (`was`).  It goes out when the content crosses the bounds, and
/// back once it fits [`BAND`] points inside.  Empty content never overhangs.
pub fn overhangs(content: Rect, w: i32, h: i32, was: bool) -> bool {
    if content.w <= 0 || content.h <= 0 {
        return false;
    }
    let inset = if was { BAND } else { 0 };
    let fits = content.x >= inset
        && content.y >= inset
        && content.x.saturating_add(content.w) <= w.saturating_sub(inset)
        && content.y.saturating_add(content.h) <= h.saturating_sub(inset);
    !fits
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(id: u64, parent: u64, x: i32, y: i32, w: i32, h: i32) -> Node {
        Node {
            id,
            parent,
            x,
            y,
            w,
            h,
            input: None,
            shown: true,
        }
    }

    #[test]
    fn a_click_on_the_shadow_falls_through_to_what_lies_beneath() {
        // child 2 at 100,100, 60 x 40 with a 10 point shadow margin around its native area
        let mut c = node(2, 1, 100, 100, 60, 40);
        c.input = Some(Rect::new(10, 10, 40, 20));
        let nodes = [c];
        assert_eq!(hit(1, &nodes, 115.0, 115.0), (2, 15.0, 15.0));
        // the shadow margin: the root takes it
        assert_eq!(hit(1, &nodes, 105.0, 105.0), (1, 105.0, 105.0));
        assert_eq!(hit(1, &nodes, 155.0, 135.0), (1, 155.0, 135.0));
    }

    #[test]
    fn the_hit_test_goes_top_down() {
        // 2 below 3 (3 made after), both over 50,50; 4 is 2's own child, above 2 but below 3
        let nodes = [
            node(2, 1, 40, 40, 40, 40),
            node(4, 2, 0, 0, 20, 20),
            node(3, 1, 45, 45, 40, 40),
        ];
        assert_eq!(hit(1, &nodes, 50.0, 50.0).0, 3);
        // outside 3, inside 4 (at 40..60): 4 above its parent 2
        assert_eq!(hit(1, &nodes, 42.0, 42.0), (4, 2.0, 2.0));
        // outside 3 and 4, inside 2
        assert_eq!(hit(1, &nodes, 42.0, 70.0), (2, 2.0, 30.0));
        // a nested child outside its parent's rectangle still takes the pointer
        let nodes = [node(2, 1, 40, 40, 10, 10), node(4, 2, 20, 20, 10, 10)];
        assert_eq!(hit(1, &nodes, 65.0, 65.0), (4, 5.0, 5.0));
        // a hidden child hides its own
        let mut hidden = nodes;
        hidden[0].shown = false;
        assert_eq!(hit(1, &hidden, 65.0, 65.0).0, 1);
        assert_eq!(origin(&nodes, 4), (60, 60));
    }

    #[test]
    fn a_child_sitting_on_the_edge_switches_once_per_crossing() {
        let (w, h) = (400, 300);
        let mut out = false;
        let mut switches = 0;
        // across the right edge and back, a point at a time, then dithering on the edge
        let xs: Vec<i32> = (330..=360)
            .chain((330..=360).rev())
            .chain([350, 351, 350, 351, 349, 350, 351])
            .collect();
        let mut on_edge_switches = 0;
        for (i, x) in xs.iter().enumerate() {
            let now = overhangs(Rect::new(*x, 10, 50, 20), w, h, out);
            if now != out {
                switches += 1;
                if i >= 62 {
                    on_edge_switches += 1;
                }
            }
            out = now;
        }
        // out once at 351, back once below 348; the dither after crosses out once more and stays
        assert_eq!(switches, 3);
        assert_eq!(on_edge_switches, 1);
        assert!(!overhangs(Rect::new(0, 0, 400, 300), w, h, false));
        assert!(overhangs(Rect::new(-1, 0, 10, 10), w, h, false));
        assert!(!overhangs(Rect::new(-5, 0, 0, 10), w, h, true));
    }
}

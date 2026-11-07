//! Changes of the screen list and of a magnify gesture, platform-neutral so their tests run on Linux.

use crate::event::OutputEv;

/// What happened to the output with this id.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Change {
    Added,
    Removed,
    Changed,
}

fn same(a: &OutputEv, b: &OutputEv) -> bool {
    (a.x, a.y, a.w, a.h, a.mm_w, a.mm_h, a.scale120, a.name)
        == (b.x, b.y, b.w, b.h, b.mm_w, b.mm_h, b.scale120, b.name)
}

/// The records that take a host that knows `old` to `new`: removals of ids that went, additions of new ids, and
/// changes of ids whose fields differ.  A removed output's record has every field but the id zeroed.
pub fn diff(old: &[OutputEv], new: &[OutputEv]) -> Vec<(Change, OutputEv)> {
    let mut out = Vec::new();
    for o in old {
        if !new.iter().any(|n| n.id == o.id) {
            out.push((
                Change::Removed,
                OutputEv {
                    id: o.id,
                    ..OutputEv::default()
                },
            ));
        }
    }
    for n in new {
        match old.iter().find(|o| o.id == n.id) {
            None => out.push((Change::Added, *n)),
            Some(o) if !same(o, n) => out.push((Change::Changed, *n)),
            Some(_) => {}
        }
    }
    out
}

/// A magnify gesture's scale since it began: AppKit reports each event's change, the host wants the product.
#[derive(Clone, Copy, Debug)]
pub struct Magnify(f64);

impl Default for Magnify {
    fn default() -> Self {
        Magnify(1.0)
    }
}

impl Magnify {
    /// Start over (a gesture began).
    pub fn begin(&mut self) {
        self.0 = 1.0;
    }

    /// Add one event's `magnification` (0.1 is ten percent bigger); the scale since the start.
    pub fn update(&mut self, magnification: f64) -> f64 {
        self.0 *= 1.0 + magnification;
        self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn out(id: u64, w: i32, scale120: u32) -> OutputEv {
        OutputEv {
            id,
            w,
            scale120,
            ..OutputEv::default()
        }
    }

    #[test]
    fn nothing_changed_is_no_records() {
        let a = [out(1, 100, 240), out(2, 50, 120)];
        assert!(diff(&a, &a).is_empty());
    }

    #[test]
    fn added_removed_and_changed() {
        let old = [out(1, 100, 240), out(2, 50, 120)];
        let new = [out(1, 100, 120), out(3, 70, 120)];
        let d = diff(&old, &new);
        assert_eq!(d.len(), 3);
        assert_eq!(d[0], (Change::Removed, out(2, 0, 0)));
        assert_eq!(d[1], (Change::Changed, out(1, 100, 120)));
        assert_eq!(d[2], (Change::Added, out(3, 70, 120)));
    }

    #[test]
    fn magnify_multiplies() {
        let mut m = Magnify::default();
        m.begin();
        assert!((m.update(0.5) - 1.5).abs() < 1e-9);
        assert!((m.update(-0.5) - 0.75).abs() < 1e-9);
        m.begin();
        assert!((m.update(0.0) - 1.0).abs() < 1e-9);
    }
}

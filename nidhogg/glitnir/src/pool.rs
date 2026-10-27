//! Buffer pool policy for a shell that presents through shared-memory buffers.
//!
//! Wayland-free: the pool decides which buffer a frame is drawn into, how old its contents are (the host repairs from
//! the age), and which buffers may go; the shell owns the memory and the protocol objects.  One buffer is enough
//! when idle and two are used in motion (a third only if a measurement says it helps); buffers idle for more than
//! `IDLE_NS` are released, keeping one.
//!
//! Ages follow the usual convention: after `submitted` has run G times, a buffer last submitted as submission g has
//! age `G - g + 1` (1 is the previous frame); 0 is a buffer that was never written.

use crate::geom::Rect;

/// A quiet period after which idle buffers are released (2 s).
pub const IDLE_NS: u64 = 2_000_000_000;
/// The most buffers in use at once.
pub const MAX_BUFFERS: usize = 2;
/// The damage of more rectangles than this becomes its bounding box (as the damage tracker's merge).
pub const MAX_RECTS: usize = 32;

/// A buffer the pool handed out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BufferSlot {
    pub id: usize,
    /// 0 = never written (a new buffer); 1 = holds the previous frame, and so on.
    pub age: u32,
    pub w: i32,
    pub h: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Free,
    /// Handed out by `begin`, not yet submitted.
    Begun,
    /// Submitted; the compositor owns it until `released`.
    Held,
}

#[derive(Debug)]
struct Slot {
    id: usize,
    w: i32,
    h: i32,
    state: State,
    /// The submission this buffer's contents come from (0 = never written).
    written: u64,
}

#[derive(Debug, Default)]
pub struct Pool {
    slots: Vec<Slot>,
    next_id: usize,
    /// Submissions so far.
    seq: u64,
    /// Buffers the pool dropped on its own (wrong size); `trim` reports them.
    retired: Vec<BufferSlot>,
    /// The size of the latest `begin`.
    size: (i32, i32),
    /// The time of the latest `begin` or `submitted`.
    active_ns: u64,
}

/// The bytes between rows: 4 per pixel, rounded up to 256.
pub fn stride_for(w: i32) -> i32 {
    (4 * w.max(1) + 255) & !255
}

/// `damage` as at most `MAX_RECTS` rectangles: empty ones dropped, the bounding box when there are more.
pub fn merged(damage: &[Rect]) -> Vec<Rect> {
    let mut v: Vec<Rect> = damage.iter().copied().filter(|r| !r.is_empty()).collect();
    if v.len() > MAX_RECTS {
        let b = v.iter().skip(1).fold(v[0], |a, r| a.bounds(r));
        v = vec![b];
    }
    v
}

impl Pool {
    pub fn new() -> Pool {
        Pool::default()
    }

    fn info(s: &Slot, seq: u64) -> BufferSlot {
        BufferSlot {
            id: s.id,
            age: if s.written == 0 {
                0
            } else {
                (seq - s.written + 1) as u32
            },
            w: s.w,
            h: s.h,
        }
    }

    fn retire(&mut self, index: usize) {
        let s = self.slots.remove(index);
        self.retired.push(Self::info(&s, self.seq));
    }

    /// A free slot of `w` x `h`, with its age; `None` when every buffer is busy.  A slot with `age == 0` is new: the
    /// shell makes its memory.  Free buffers of another size are retired (`trim` reports them).
    pub fn begin(&mut self, w: i32, h: i32, now_ns: u64) -> Option<BufferSlot> {
        self.size = (w, h);
        self.active_ns = now_ns;
        while let Some(i) = self
            .slots
            .iter()
            .position(|s| s.state == State::Free && (s.w, s.h) != (w, h))
        {
            self.retire(i);
        }
        let pick = self
            .slots
            .iter()
            .enumerate()
            .filter(|(_, s)| s.state == State::Free)
            .max_by_key(|(_, s)| s.written)
            .map(|(i, _)| i);
        let i = match pick {
            Some(i) => i,
            None if self.slots.len() < MAX_BUFFERS => {
                let id = self.next_id;
                self.next_id += 1;
                self.slots.push(Slot {
                    id,
                    w,
                    h,
                    state: State::Free,
                    written: 0,
                });
                self.slots.len() - 1
            }
            None => return None,
        };
        self.slots[i].state = State::Begun;
        Some(Self::info(&self.slots[i], self.seq))
    }

    fn find(&mut self, slot: BufferSlot) -> Option<&mut Slot> {
        self.slots.iter_mut().find(|s| s.id == slot.id)
    }

    /// The begun slot goes to the compositor; its contents are now the newest frame.
    pub fn submitted(&mut self, slot: BufferSlot) {
        self.seq += 1;
        let seq = self.seq;
        if let Some(s) = self.find(slot) {
            s.state = State::Held;
            s.written = seq;
        }
    }

    /// The compositor is done with the slot.  A buffer of an outdated size is retired instead of reused.
    pub fn released(&mut self, slot: BufferSlot) {
        let size = self.size;
        let Some(i) = self.slots.iter().position(|s| s.id == slot.id) else {
            return;
        };
        if self.slots[i].state != State::Held {
            return;
        }
        if (self.slots[i].w, self.slots[i].h) != size && size != (0, 0) {
            self.retire(i);
        } else {
            self.slots[i].state = State::Free;
        }
    }

    /// A begun slot that is not submitted after all (its frame was dropped).
    pub fn abandon(&mut self, slot: BufferSlot) {
        if let Some(s) = self.find(slot)
            && s.state == State::Begun
        {
            s.state = State::Free;
        }
    }

    /// The buffers to free: the retired ones, and, when nothing was begun or submitted for more than `IDLE_NS`, every
    /// free buffer but the newest (one stays, whatever its state).
    pub fn trim(&mut self, now_ns: u64) -> Vec<BufferSlot> {
        let mut out = std::mem::take(&mut self.retired);
        if now_ns.saturating_sub(self.active_ns) > IDLE_NS {
            while self.slots.len() > 1 {
                let Some(i) = self
                    .slots
                    .iter()
                    .enumerate()
                    .filter(|(_, s)| s.state == State::Free)
                    .min_by_key(|(_, s)| s.written)
                    .map(|(i, _)| i)
                else {
                    break;
                };
                let s = self.slots.remove(i);
                out.push(Self::info(&s, self.seq));
            }
        }
        out
    }

    /// The number of buffers the pool keeps.
    pub fn live(&self) -> usize {
        self.slots.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const S: u64 = 1_000_000_000;

    #[test]
    fn stride_is_256_aligned() {
        assert_eq!(stride_for(1), 256);
        assert_eq!(stride_for(64), 256);
        assert_eq!(stride_for(65), 512);
    }

    #[test]
    fn ages() {
        let mut p = Pool::new();
        let a = p.begin(100, 50, 0).unwrap();
        assert_eq!(a.age, 0);
        p.submitted(a);
        let b = p.begin(100, 50, 0).unwrap();
        assert_ne!(a.id, b.id);
        assert_eq!(b.age, 0);
        p.submitted(b);
        p.released(a);
        let a2 = p.begin(100, 50, 0).unwrap();
        assert_eq!((a2.id, a2.age), (a.id, 2));
        p.submitted(a2);
        p.released(b);
        let b2 = p.begin(100, 50, 0).unwrap();
        assert_eq!((b2.id, b2.age), (b.id, 2));
    }

    #[test]
    fn idle_trims_to_one_after_2s() {
        let mut p = Pool::new();
        let a = p.begin(8, 8, 0).unwrap();
        p.submitted(a);
        let b = p.begin(8, 8, 0).unwrap();
        p.submitted(b);
        p.released(a);
        assert!(p.trim(S).is_empty(), "not idle yet");
        assert!(p.trim(2 * S).is_empty(), "exactly 2 s is not more than 2 s");
        let freed = p.trim(2 * S + 1);
        assert_eq!(freed.len(), 1);
        assert_eq!(freed[0].id, a.id);
        assert_eq!(p.live(), 1);
        assert!(p.trim(10 * S).is_empty(), "one buffer stays");
    }

    #[test]
    fn never_hands_out_a_held_buffer() {
        let mut p = Pool::new();
        let a = p.begin(8, 8, 0).unwrap();
        p.submitted(a);
        let b = p.begin(8, 8, 0).unwrap();
        p.submitted(b);
        assert!(p.begin(8, 8, 0).is_none(), "both are held");
        p.released(b);
        let c = p.begin(8, 8, 0).unwrap();
        assert_eq!(c.id, b.id);
        assert!(p.begin(8, 8, 0).is_none(), "one held, one begun");
    }

    #[test]
    fn a_new_size_retires_free_buffers() {
        let mut p = Pool::new();
        let a = p.begin(8, 8, 0).unwrap();
        p.submitted(a);
        p.released(a);
        let b = p.begin(16, 8, 0).unwrap();
        assert_eq!((b.age, b.w), (0, 16));
        let freed = p.trim(0);
        assert_eq!(freed.len(), 1);
        assert_eq!(freed[0].id, a.id);
    }

    #[test]
    fn many_rects_merge_to_the_bounding_box() {
        let rs: Vec<Rect> = (0..40).map(|i| Rect::new(i, 0, 1, 1)).collect();
        assert_eq!(merged(&rs), vec![Rect::new(0, 0, 40, 1)]);
    }
}

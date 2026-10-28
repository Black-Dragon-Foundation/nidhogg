//! The frame-time record: a ring of 1024 entries per root, and its summary.

use std::collections::BTreeMap;
use std::sync::{Mutex, MutexGuard};

/// Entries kept per root.
pub const RING: usize = 1024;

/// `backfill_rate` when no entry knows whether its paint back-filled (`backfill` 2 everywhere).
pub const UNKNOWN_RATE: f64 = -1.0;

/// One paint, as the glue measures it.  Mirrors `GlitnirStatsEntry` in `include/glitnir.h`.
#[repr(C)]
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct StatsEntry {
    pub t_update_ns: u64,
    pub t_commit_ns: u64,
    pub t_freeze_ns: u64,
    pub t_draw_start_ns: u64,
    pub t_draw_end_ns: u64,
    pub t_compose_start_ns: u64,
    pub t_compose_end_ns: u64,
    pub clock_frame_us: i64,
    pub presented_us: i64,
    pub damage_px: u64,
    pub scroll_px: u64,
    pub painted_px: u64,
    pub layers: u32,
    pub ticks: u32,
    pub late_ticks: u32,
    pub violations: u32,
    pub backfill: u8,
    pub scale_mismatch: u8,
}

/// p50, p95 and max of one series, in milliseconds.  Mirrors `GlitnirPercentiles`.
#[repr(C)]
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct Percentiles {
    pub p50: f64,
    pub p95: f64,
    pub max: f64,
}

/// Mirrors `GlitnirStatsSummary`.
#[repr(C)]
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct Summary {
    pub count: u64,
    pub compose_ms: Percentiles,
    pub draw_ms: Percentiles,
    pub latency_ms: Percentiles,
    pub freeze_ms: Percentiles,
    pub damage_px_p50: u64,
    pub painted_px_p50: u64,
    pub backfill_rate: f64,
    pub ticks: u64,
    pub late_ticks: u64,
    pub violations: u64,
    pub scale_mismatch: u64,
    /// From the update to the end of the paint, over the entries with an update time only.
    pub update_ms: Percentiles,
}

/// The last `RING` entries of one root, and the totals since the last reset.
#[derive(Debug)]
pub struct Ring {
    buf: Vec<StatsEntry>,
    next: usize,
    count: u64,
    ticks: u64,
    late_ticks: u64,
    violations: u64,
    scale_mismatch: u64,
}

impl Ring {
    fn new() -> Ring {
        Ring {
            buf: Vec::with_capacity(RING),
            next: 0,
            count: 0,
            ticks: 0,
            late_ticks: 0,
            violations: 0,
            scale_mismatch: 0,
        }
    }

    fn push(&mut self, e: StatsEntry) {
        if self.buf.len() < RING {
            self.buf.push(e);
        } else {
            self.buf[self.next] = e;
        }
        self.next = (self.next + 1) % RING;
        self.count += 1;
        self.ticks += u64::from(e.ticks);
        self.late_ticks += u64::from(e.late_ticks);
        self.violations += u64::from(e.violations);
        self.scale_mismatch += u64::from(e.scale_mismatch != 0);
    }

    fn clear(&mut self) {
        self.buf.clear();
        self.next = 0;
        self.count = 0;
        self.ticks = 0;
        self.late_ticks = 0;
        self.violations = 0;
        self.scale_mismatch = 0;
    }
}

/// Nearest-rank percentile (`pct` in 1..=100) of an ascending list; 0 for an empty one.
fn rank<T: Copy + Default>(sorted: &[T], pct: usize) -> T {
    if sorted.is_empty() {
        return T::default();
    }
    sorted[(sorted.len() * pct).div_ceil(100) - 1]
}

fn percentiles(mut v: Vec<f64>) -> Percentiles {
    v.sort_by(f64::total_cmp);
    Percentiles {
        p50: rank(&v, 50),
        p95: rank(&v, 95),
        max: v.last().copied().unwrap_or(0.0),
    }
}

fn ms(from: u64, to: u64) -> f64 {
    if from == 0 || to < from {
        0.0
    } else {
        (to - from) as f64 / 1e6
    }
}

fn summarise(r: &Ring) -> Summary {
    let b = &r.buf;
    let series = |f: &dyn Fn(&StatsEntry) -> f64| percentiles(b.iter().map(f).collect());
    let mut damage: Vec<u64> = b.iter().map(|e| e.damage_px).collect();
    let mut painted: Vec<u64> = b.iter().map(|e| e.painted_px).collect();
    damage.sort_unstable();
    painted.sort_unstable();
    let known = b.iter().filter(|e| e.backfill < 2).count();
    let filled = b.iter().filter(|e| e.backfill == 1).count();
    Summary {
        count: r.count,
        compose_ms: series(&|e| ms(e.t_compose_start_ns, e.t_compose_end_ns)),
        draw_ms: series(&|e| ms(e.t_draw_start_ns, e.t_draw_end_ns)),
        latency_ms: series(&|e| ms(e.t_commit_ns, e.t_draw_end_ns)),
        freeze_ms: series(&|e| ms(e.t_freeze_ns, e.t_commit_ns)),
        damage_px_p50: rank(&damage, 50),
        painted_px_p50: rank(&painted, 50),
        backfill_rate: if known == 0 {
            UNKNOWN_RATE
        } else {
            filled as f64 / known as f64
        },
        ticks: r.ticks,
        late_ticks: r.late_ticks,
        violations: r.violations,
        scale_mismatch: r.scale_mismatch,
        update_ms: percentiles(
            b.iter()
                .filter(|e| e.t_update_ns != 0)
                .map(|e| ms(e.t_update_ns, e.t_draw_end_ns))
                .collect(),
        ),
    }
}

/// The rings of every root.
pub type Rings = BTreeMap<u64, Ring>;

/// The rings, under their own lock: a compose thread records while the host holds the global state.
static STATS: Mutex<Rings> = Mutex::new(BTreeMap::new());

fn rings() -> MutexGuard<'static, Rings> {
    STATS.lock().unwrap_or_else(|p| p.into_inner())
}

/// Append one entry to ROOT's ring (allocated on the first record).  Any thread may record.
pub fn record(root: u64, e: StatsEntry) {
    rings().entry(root).or_insert_with(Ring::new).push(e);
}

/// ROOT's summary, then empty its ring when `reset`.  A root with no record yet summarises as an empty ring (zeros,
/// no back-fill rate).
pub fn summary(root: u64, reset: bool) -> Summary {
    let mut rs = rings();
    let Some(r) = rs.get_mut(&root) else {
        return summarise(&Ring::new());
    };
    let s = summarise(r);
    if reset {
        r.clear();
    }
    s
}

/// Drop ROOT's ring (the root is gone).
pub fn forget(root: u64) {
    rings().remove(&root);
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Rng(u64);
    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }
    }

    fn seed() -> u64 {
        let s = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(1)
            | 1;
        eprintln!("stats test seed {s}");
        s
    }

    fn random_entry(r: &mut Rng) -> StatsEntry {
        let t = 1 + r.next() % 1_000_000;
        StatsEntry {
            t_commit_ns: t,
            t_draw_start_ns: t + r.next() % 5_000_000,
            t_draw_end_ns: t + 5_000_000 + r.next() % 5_000_000,
            t_compose_start_ns: t,
            t_compose_end_ns: t + r.next() % 3_000_000,
            damage_px: r.next() % 1000,
            painted_px: r.next() % 1000,
            backfill: (r.next() % 3) as u8,
            ticks: (r.next() % 3) as u32,
            ..StatsEntry::default()
        }
    }

    #[test]
    fn ring_keeps_the_last_1024() {
        let root = 9001;
        let mut rng = Rng(seed());
        let all: Vec<StatsEntry> = (0..RING + 77).map(|_| random_entry(&mut rng)).collect();
        for e in &all {
            record(root, *e);
        }
        let rs = rings();
        let ring = &rs[&root];
        assert_eq!(ring.buf.len(), RING);
        assert_eq!(ring.count, (RING + 77) as u64);
        // every one of the last RING entries is in the ring, none older
        for e in &all[77..] {
            assert!(ring.buf.contains(e));
        }
        assert!(!ring.buf.contains(&all[0]) || all[0] == all[77]);
        drop(rs);
        forget(root);
    }

    #[test]
    fn record_without_state_lock() {
        let root = 9006;
        // the host holds the global state; a compose thread records and reads its summary meanwhile
        let held = crate::state();
        let (tx, rx) = std::sync::mpsc::channel();
        let compose = std::thread::spawn(move || {
            crate::mark_compose_thread();
            let mut rng = Rng(seed());
            for _ in 0..10 {
                record(root, random_entry(&mut rng));
            }
            tx.send(summary(root, false).count).unwrap();
        });
        let got = rx.recv_timeout(std::time::Duration::from_secs(10));
        drop(held);
        compose.join().unwrap();
        assert_eq!(got, Ok(10));
        forget(root);
    }

    #[test]
    fn summary_percentiles_match_a_sorted_list() {
        let root = 9002;
        let mut rng = Rng(seed());
        let n = 200 + (rng.next() % (RING - 200) as u64) as usize; // the ring keeps them all
        let all: Vec<StatsEntry> = (0..n).map(|_| random_entry(&mut rng)).collect();
        for e in &all {
            record(root, *e);
        }
        let s = summary(root, false);
        let mut draw: Vec<f64> = all
            .iter()
            .map(|e| (e.t_draw_end_ns - e.t_draw_start_ns) as f64 / 1e6)
            .collect();
        draw.sort_by(f64::total_cmp);
        assert_eq!(s.count, n as u64);
        assert_eq!(s.draw_ms.p50, draw[(n * 50).div_ceil(100) - 1]);
        assert_eq!(s.draw_ms.p95, draw[(n * 95).div_ceil(100) - 1]);
        assert_eq!(s.draw_ms.max, draw[n - 1]);
        let mut painted: Vec<u64> = all.iter().map(|e| e.painted_px).collect();
        painted.sort_unstable();
        assert_eq!(s.painted_px_p50, painted[(n * 50).div_ceil(100) - 1]);
        let known = all.iter().filter(|e| e.backfill < 2).count();
        let filled = all.iter().filter(|e| e.backfill == 1).count();
        assert_eq!(s.backfill_rate, filled as f64 / known as f64);
        assert_eq!(s.ticks, all.iter().map(|e| u64::from(e.ticks)).sum::<u64>());
        forget(root);
    }

    #[test]
    fn update_series_skips_entries_without_an_update() {
        let root = 9005;
        let mut rng = Rng(seed());
        for i in 0..40u64 {
            let e = random_entry(&mut rng);
            let t_update_ns = if i % 4 == 0 {
                e.t_draw_end_ns - 2_000_000
            } else {
                0
            };
            record(
                root,
                StatsEntry {
                    t_update_ns,
                    t_commit_ns: e.t_draw_end_ns - 1_000_000,
                    ..e
                },
            );
        }
        let s = summary(root, false);
        assert_eq!(s.count, 40);
        // ten keyed entries, each 2 ms; the other thirty would pull the median to 0
        assert_eq!(s.update_ms.p50, 2.0);
        assert_eq!(s.update_ms.max, 2.0);
        forget(root);
    }

    #[test]
    fn backfill_rate_is_unknown_without_a_known_entry() {
        let root = 9004;
        let mut rng = Rng(seed());
        for _ in 0..10 {
            record(
                root,
                StatsEntry {
                    backfill: 2,
                    ..random_entry(&mut rng)
                },
            );
        }
        assert_eq!(summary(root, false).backfill_rate, UNKNOWN_RATE);
        record(
            root,
            StatsEntry {
                backfill: 1,
                ..random_entry(&mut rng)
            },
        );
        assert_eq!(summary(root, false).backfill_rate, 1.0);
        forget(root);
    }

    #[test]
    fn reset_empties_the_ring() {
        let root = 9003;
        let mut rng = Rng(seed());
        for _ in 0..10 {
            record(root, random_entry(&mut rng));
        }
        let empty = Summary {
            backfill_rate: UNKNOWN_RATE,
            ..Summary::default()
        };
        assert_eq!(summary(root, true).count, 10);
        assert_eq!(summary(root, false), empty);
        record(root, random_entry(&mut rng));
        assert_eq!(summary(root, false).count, 1);
        forget(root);
        assert_eq!(summary(root, false), empty);
    }
}

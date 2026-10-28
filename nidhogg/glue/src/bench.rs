//! The full-redraw bench of the W0 budgets: present N frames of a tree, each damaged whole, and time them.
//!
//! Moved from `src/nhwlterm.c` (`nhwl--bench-full-redraw`).  Each frame is damaged, published at a consistency point
//! and presented through the presenter (`present.rs`): on its thread (its count of buffer commits rises), or inline,
//! where the present asks itself every millisecond until the root commits a buffer (the previous present's frame
//! callback, about one refresh).  The bench measures one present (the publish, and the try that committed) and the
//! time between two commits; their percentiles are the budgets' numbers.

use crate::display::Display;
use crate::guard::GlueError;
use crate::present::{
    NHG_PRESENT_BACKSTOP, NHG_PRESENT_READY, NHG_PRESENTED_COMMITTED, NhgSurface, present, stats,
};
use std::time::{Duration, Instant};

/// The most frames a bench may present.
pub const MAX_FRAMES: u32 = 100_000;

/// How many 1 ms waits a frame gets for the previous one's callback before the bench gives up.
const MAX_WAITS: u32 = 5000;

/// The bench's result.  Layout equals `NhgBenchOut`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct NhgBenchOut {
    /// The frames presented (fewer than asked when one never committed).
    pub frames: u64,
    /// Milliseconds one present took: the median and the 95th percentile.
    pub compose_p50: f64,
    pub compose_p95: f64,
    /// Milliseconds between two presents, 95th percentile.
    pub interval_p95: f64,
}

/// What the bench drives: one frame's damage and publish, and one try to present it.
pub trait Redraw {
    /// Damage the whole frame and publish it.
    fn damage(&mut self) -> Result<(), GlueError>;
    /// Try to present; the time the present took when the root committed a buffer, else `None`.
    fn attempt(&mut self) -> Result<Option<Duration>, GlueError>;
}

/// The `p`-th percentile (nearest rank) of `sorted`.
pub fn percentile(sorted: &[f64], p: f64) -> f64 {
    let k = (p * sorted.len() as f64).ceil() as usize;
    sorted[k.saturating_sub(1).min(sorted.len() - 1)]
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1e3
}

/// Present `frames` frames of `r`.  `frames` in the result is how many committed; the percentiles are 0 below two.
pub fn run(r: &mut dyn Redraw, frames: u32) -> Result<NhgBenchOut, GlueError> {
    if !(1..=MAX_FRAMES).contains(&frames) {
        return Err(GlueError::Arg);
    }
    let mut compose: Vec<f64> = Vec::new();
    let mut interval: Vec<f64> = Vec::new();
    compose
        .try_reserve(frames as usize)
        .and_then(|()| interval.try_reserve(frames as usize))
        .map_err(|_| GlueError::NoMem)?;
    let mut prev: Option<Instant> = None;
    'frames: for _ in 0..frames {
        r.damage()?;
        for waits in 0..=MAX_WAITS {
            if waits > 0 {
                std::thread::sleep(Duration::from_millis(1));
            }
            if let Some(took) = r.attempt()? {
                let now = Instant::now();
                compose.push(ms(took));
                if let Some(p) = prev {
                    interval.push(ms(now - p));
                }
                prev = Some(now);
                continue 'frames;
            }
        }
        break;
    }
    let mut out = NhgBenchOut {
        frames: compose.len() as u64,
        ..Default::default()
    };
    if compose.len() >= 2 {
        compose.sort_by(f64::total_cmp);
        interval.sort_by(f64::total_cmp);
        out.compose_p50 = percentile(&compose, 0.5);
        out.compose_p95 = percentile(&compose, 0.95);
        out.interval_p95 = percentile(&interval, 0.95);
    }
    Ok(out)
}

/// The bench on a tree of the shell: `s` is C's surface list, the root first.
struct Tree<'a> {
    state: &'a mut Display,
    s: &'a [NhgSurface],
    /// The presenter's commit count when the frame was published, and how long the publish took.
    commits: u64,
    publish: Duration,
}

impl Redraw for Tree<'_> {
    fn damage(&mut self) -> Result<(), GlueError> {
        let root = self.s[0];
        self.state.backings.damage_all(root.frame)?;
        self.commits = stats(self.state.backend())?.counters.commits;
        let t0 = Instant::now();
        let (backend, backings, presenting) = self.state.presenting();
        present(
            backend,
            backings,
            presenting,
            root.window,
            self.s,
            NHG_PRESENT_BACKSTOP,
            0,
        )?;
        self.publish = t0.elapsed();
        Ok(())
    }

    fn attempt(&mut self) -> Result<Option<Duration>, GlueError> {
        let t0 = Instant::now();
        let (backend, backings, presenting) = self.state.presenting();
        let mut committed = present(
            backend,
            backings,
            presenting,
            self.s[0].window,
            &[],
            NHG_PRESENT_READY,
            0,
        )? & NHG_PRESENTED_COMMITTED
            != 0;
        if !committed {
            committed = stats(self.state.backend())?.counters.commits > self.commits;
        }
        Ok(committed.then(|| self.publish + t0.elapsed()))
    }
}

/// Present `frames` frames of the tree `s` (bottom to top, the root first, as `nhg_present` takes it), each damaged
/// whole.  `State` when the root has no backing or fewer than two frames committed (the count is in the result).
pub fn full_redraw(
    state: &mut Display,
    s: &[NhgSurface],
    frames: u32,
) -> Result<NhgBenchOut, GlueError> {
    let root = s.first().ok_or(GlueError::Arg)?;
    state.frame(root.window)?;
    run(
        &mut Tree {
            state,
            s,
            commits: 0,
            publish: Duration::ZERO,
        },
        frames,
    )
}

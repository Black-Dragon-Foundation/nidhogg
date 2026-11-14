//! A bar's layout: which of its segments fit a width, and where its one cuttable segment is cut.
//!
//! The editor measures (one width per segment text it has not met, and the cuttable segment's characters once per
//! text); this lays out.  Pure: no view, no state.  The spec is a line of integers (`Spec::read`); the answer names
//! segments by their index, so the texts and their properties never cross the door.
//!
//! The fit, in order: the segments of the drop order go, first ranked first, while the bar is too wide; then the
//! cuttable segment is cut, but not below the spec's minimum; then the others go, the right run's last first, then the
//! left run's last (the cuttable one never); last the cuttable segment is cut as far as it takes (to one space).

/// One segment as the editor measured it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Seg {
    /// Its rank in the drop order (lower goes first), or `None`: dropped only when nothing else fits.
    pub drop: Option<i64>,
    /// Its width in the bar's units (pixels, or columns on a terminal).
    pub width: i64,
    /// The cuttable segment's parts: the room either side of its text that stays (`extra`) and its text's characters'
    /// widths, in order.
    pub cut: Option<Cut>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cut {
    pub extra: i64,
    pub chars: Vec<i64>,
}

/// A bar to lay out.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Spec {
    /// The room the bar has.
    pub width: i64,
    /// The padding at each end, one space, the separators of the left and the right runs, and the ellipsis.
    pub pad: i64,
    pub space: i64,
    pub left_sep: i64,
    pub right_sep: i64,
    pub ellipsis: i64,
    /// The width the cuttable segment is cut to before other segments go for it.
    pub cut_min: i64,
    /// Columns: an ellipsis wider than the room is left out and the text cut to the room (a terminal's
    /// `truncate-string-to-width`); in pixels a cut that leaves no character is empty.
    pub columns: bool,
    pub left: Vec<Seg>,
    pub right: Vec<Seg>,
}

/// Where the cuttable segment is cut: its first `keep` characters, then the ellipsis when `ellipsis`; `width` is its
/// width after the cut.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cutting {
    pub index: usize,
    pub keep: usize,
    pub ellipsis: bool,
    pub width: i64,
}

/// The layout: the indices of the left and right segments kept, in order; the cut, if any; the right run's width.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fit {
    pub left: Vec<usize>,
    pub right: Vec<usize>,
    pub cut: Option<Cutting>,
    pub right_width: i64,
}

/// The spec's version, the first integer of its line.
pub const VERSION: i64 = 1;

impl Spec {
    /// Read a spec line: `1 WIDTH PAD SPACE LSEP RSEP ELLIPSIS CUT-MIN COLUMNS NLEFT NRIGHT`, then each segment, left
    /// ones first: `DROP WIDTH CUT` (DROP -1: none), and when CUT is 1 `EXTRA N` and N character widths.  `None` when it
    /// is not one.
    pub fn read(line: &str) -> Option<Spec> {
        let mut it = line.split_ascii_whitespace().map(|t| t.parse::<i64>().ok());
        let mut next = || it.next().flatten();
        if next()? != VERSION {
            return None;
        }
        let (width, pad, space, left_sep, right_sep, ellipsis, cut_min) = (
            next()?,
            next()?,
            next()?,
            next()?,
            next()?,
            next()?,
            next()?,
        );
        let columns = next()? != 0;
        let (nl, nr) = (count(next()?)?, count(next()?)?);
        let mut seg = || -> Option<Seg> {
            let drop = next()?;
            let width = next()?;
            let cut = match next()? {
                0 => None,
                1 => {
                    let extra = next()?;
                    let n = count(next()?)?;
                    Some(Cut {
                        extra,
                        chars: (0..n).map(|_| next()).collect::<Option<_>>()?,
                    })
                }
                _ => return None,
            };
            Some(Seg {
                drop: (drop >= 0).then_some(drop),
                width,
                cut,
            })
        };
        let left = (0..nl).map(|_| seg()).collect::<Option<Vec<_>>>()?;
        let right = (0..nr).map(|_| seg()).collect::<Option<Vec<_>>>()?;
        if next().is_some() {
            return None;
        }
        Some(Spec {
            width,
            pad,
            space,
            left_sep,
            right_sep,
            ellipsis,
            cut_min,
            columns,
            left,
            right,
        })
    }
}

/// A count in a spec: at most 4096 (a bar, not a list).
fn count(n: i64) -> Option<usize> {
    usize::try_from(n).ok().filter(|&n| n <= 4096)
}

/// The run being laid out: each segment's current width and whether it went.
struct Run {
    widths: Vec<i64>,
    gone: Vec<bool>,
}

impl Run {
    fn new(segs: &[Seg]) -> Run {
        Run {
            widths: segs.iter().map(|s| s.width).collect(),
            gone: vec![false; segs.len()],
        }
    }
    /// The kept segments' total width and count.
    fn sum(&self) -> (i64, i64) {
        self.widths
            .iter()
            .zip(&self.gone)
            .filter(|(_, g)| !**g)
            .fold((0, 0), |(w, n), (x, _)| (w + x, n + 1))
    }
    fn width(&self, sep: i64) -> i64 {
        let (w, n) = self.sum();
        w + if n > 1 { sep * (n - 1) } else { 0 }
    }
    fn last_kept(&self, skip: Option<usize>) -> Option<usize> {
        (0..self.gone.len())
            .rev()
            .find(|&i| !self.gone[i] && Some(i) != skip)
    }
    fn kept(&self) -> Vec<usize> {
        (0..self.gone.len()).filter(|&i| !self.gone[i]).collect()
    }
}

/// Lay a bar out.
pub fn fit(spec: &Spec) -> Fit {
    let mut left = Run::new(&spec.left);
    let mut right = Run::new(&spec.right);
    let total = |l: &Run, r: &Run| {
        let (ln, rn) = (l.sum().1, r.sum().1);
        2 * spec.pad
            + l.width(spec.left_sep)
            + r.width(spec.right_sep)
            + if rn > 0 {
                spec.space * if ln > 0 { 2 } else { 1 }
            } else {
                0
            }
    };
    let cuttable = spec.left.iter().position(|s| s.cut.is_some());
    let mut cutting: Option<Cutting> = None;
    // 1. the drop order, first ranked first
    let mut ranks: Vec<i64> = spec
        .left
        .iter()
        .chain(&spec.right)
        .filter_map(|s| s.drop)
        .collect();
    ranks.sort_unstable();
    ranks.dedup();
    for rank in ranks {
        if total(&left, &right) <= spec.width {
            break;
        }
        for (run, segs) in [(&mut left, &spec.left), (&mut right, &spec.right)] {
            for (i, s) in segs.iter().enumerate() {
                if s.drop == Some(rank) {
                    run.gone[i] = true;
                }
            }
        }
    }
    // 2. the cuttable segment cut, not below the minimum
    if total(&left, &right) > spec.width {
        cut_to(
            spec,
            &mut left,
            &right,
            cuttable,
            spec.cut_min,
            &mut cutting,
            total,
        );
    }
    // 3. the others go: the right run's last, then the left run's last but the cuttable one
    while total(&left, &right) > spec.width {
        if let Some(i) = right.last_kept(None) {
            right.gone[i] = true;
        } else if let Some(i) = left.last_kept(cuttable) {
            left.gone[i] = true;
        } else {
            break;
        }
    }
    // 4. the cuttable segment cut as far as it takes
    if total(&left, &right) > spec.width {
        cut_to(
            spec,
            &mut left,
            &right,
            cuttable,
            spec.space,
            &mut cutting,
            total,
        );
    }
    Fit {
        left: left.kept(),
        right: right.kept(),
        right_width: right.width(spec.right_sep),
        cut: cutting.filter(|c| !left.gone[c.index]),
    }
}

/// Cut the cuttable segment at `index` so the bar fits, but not below `min` (and one space).
fn cut_to(
    spec: &Spec,
    left: &mut Run,
    right: &Run,
    index: Option<usize>,
    min: i64,
    cutting: &mut Option<Cutting>,
    total: impl Fn(&Run, &Run) -> i64,
) {
    let Some(i) = index.filter(|&i| !left.gone[i]) else {
        return;
    };
    let Some(cut) = spec.left[i].cut.as_ref() else {
        return;
    };
    let excess = total(left, right) - spec.width;
    let now = left.widths[i];
    if excess <= 0 {
        return;
    }
    let target = min.max(spec.space).max(now - excess);
    if target >= now {
        return;
    }
    let (keep, ellipsis, width) = cut_text(spec, cut, target);
    left.widths[i] = width;
    *cutting = Some(Cutting {
        index: i,
        keep,
        ellipsis,
        width,
    });
}

/// The cuttable segment's text cut to fit `target` with its room: (characters kept, the ellipsis, the new width).
fn cut_text(spec: &Spec, cut: &Cut, target: i64) -> (usize, bool, i64) {
    let room = target - cut.extra;
    let longest = |limit: i64| {
        let mut w = 0;
        let mut n = 0;
        for &c in &cut.chars {
            if w + c > limit {
                break;
            }
            w += c;
            n += 1;
        }
        (n, w)
    };
    if spec.columns {
        // `truncate-string-to-width' with the ellipsis only when it fits the room
        if spec.ellipsis > room {
            let (n, w) = longest(room.max(0));
            return (n, false, cut.extra + w);
        }
        let (n, w) = longest(room - spec.ellipsis);
        return (n, true, cut.extra + w + spec.ellipsis);
    }
    if room <= 0 {
        return (0, false, 0);
    }
    match longest(room - spec.ellipsis) {
        (0, _) => (0, false, 0),
        (n, w) => (n, true, cut.extra + w + spec.ellipsis),
    }
}

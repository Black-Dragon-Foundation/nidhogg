//! What the GUI rows log about a display's life when a frame fails to show (checking builds only: the `test-hooks`
//! feature; without it every call here does nothing).  The startup trace holds the first events of a window's life,
//! each stamped with its milliseconds since the first: the window made, its configure queued and applied, the waits
//! for it, the visibility pushes, the first draw.  The image draws are the last few `nhg_draw_image` calls: the
//! container's first word and the picture made from it, so a wrong pixel names whether the container or the drawing
//! holds it.

use std::collections::{BTreeMap, VecDeque};
use std::fmt::Write as _;
use std::sync::Mutex;
use std::time::Instant;

/// The startup trace keeps this many events: the first ones, a window's life up to its first draws.
const NOTES: usize = 64;
/// The image draws kept: the newest.
const IMAGES: usize = 32;
/// The pump decisions and paints kept: the newest.
const PUMPS: usize = 128;
/// The room of a test query's answer (`nhmac--query`'s buffer in nhmacterm.c).
const ANSWER: usize = 16 * 1024;

/// One image draw: the container (`w` x `h`, `bpp`, with a mask or not) and its first word, the drawn rectangle, and
/// its first clip (`None`: unclipped), the first word of the picture when it was made by this draw (`None`: a cached
/// picture), the part's origin in the drawn image and the image's matrix (`NhgPix`'s, all zero for none).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ImageDraw {
    pub w: i32,
    pub h: i32,
    pub bpp: i32,
    pub mask: bool,
    pub word: u32,
    pub made: Option<u32>,
    pub rect: (i32, i32, i32, i32),
    pub clip: Option<(i32, i32, i32, i32)>,
    pub src: (i32, i32),
    pub matrix: [f64; 6],
}

struct Trace {
    start: Option<Instant>,
    notes: Vec<String>,
    /// Each window's waits for its first configure: rounds, and those that timed out.
    waits: BTreeMap<u64, (u32, u32)>,
    images: VecDeque<ImageDraw>,
    /// The newest pump decisions (a drain's end, a wait's pump), read and cleared by `pumps_text`.
    pumps: VecDeque<String>,
}

static TRACE: Mutex<Trace> = Mutex::new(Trace {
    start: None,
    notes: Vec::new(),
    waits: BTreeMap::new(),
    images: VecDeque::new(),
    pumps: VecDeque::new(),
});

/// One pump decision: who asked and what was done.
pub fn pump(what: impl FnOnce() -> String) {
    with(|t| {
        if t.pumps.len() == PUMPS {
            t.pumps.pop_front();
        }
        t.pumps.push_back(what());
    });
}

/// The pump decisions since the last read, oldest first, as a Lisp list of strings; the list is cleared.  The newest
/// that fit a query's answer are given, after a first string saying how many older ones were left out.
pub fn pumps_text() -> String {
    with(|t| {
        let items: Vec<String> = t.pumps.drain(..).map(|p| format!("{p:?}")).collect();
        // room for the parentheses and the "left out" string
        let mut room = ANSWER - 64;
        let keep = items
            .iter()
            .rev()
            .take_while(|s| {
                let fits = s.len() < room;
                room = room.saturating_sub(s.len() + 1);
                fits
            })
            .count();
        let out = items.len() - keep;
        let mut kept: Vec<String> = items.into_iter().skip(out).collect();
        if out > 0 {
            kept.insert(0, format!("\"{out} older entries left out\""));
        }
        format!("({})", kept.join(" "))
    })
    .unwrap_or_else(|| "nil".into())
}

fn with<R>(f: impl FnOnce(&mut Trace) -> R) -> Option<R> {
    cfg!(feature = "test-hooks").then(|| f(&mut TRACE.lock().unwrap_or_else(|p| p.into_inner())))
}

/// Add `what` (made only while the trace has room) to the startup trace.
pub fn note(what: impl FnOnce() -> String) {
    with(|t| {
        if t.notes.len() < NOTES {
            let ms = t
                .start
                .get_or_insert_with(Instant::now)
                .elapsed()
                .as_secs_f64()
                * 1000.0;
            t.notes.push(format!("{ms:.1} {}", what()));
        }
    });
}

/// One round of the wait for `window`'s first configure; `timed_out` when nothing came.
pub fn wait_round(window: u64, timed_out: bool) {
    with(|t| {
        let w = t.waits.entry(window).or_default();
        w.0 += 1;
        w.1 += u32::from(timed_out);
    });
}

/// An image draw.
pub fn image(d: ImageDraw) {
    with(|t| {
        if t.images.len() == IMAGES {
            t.images.pop_front();
        }
        t.images.push_back(d);
    });
}

/// The startup trace as a Lisp list of strings, then each window's waits as `(wait WINDOW :rounds R :timeouts T)`.
pub fn startup_text() -> String {
    with(|t| {
        let mut s = String::from("(");
        for n in &t.notes {
            let _ = write!(s, "{:?} ", n);
        }
        for (w, (r, to)) in &t.waits {
            let _ = write!(s, "(wait {w} :rounds {r} :timeouts {to}) ");
        }
        s.truncate(s.trim_end().len());
        s.push(')');
        s
    })
    .unwrap_or_else(|| "nil".into())
}

/// The image draws, oldest first, as a Lisp list of plists.
pub fn images_text() -> String {
    with(|t| {
        let items: Vec<String> = t
            .images
            .iter()
            .map(|d| {
                let r = |(x, y, w, h): (i32, i32, i32, i32)| format!("({x} {y} {w} {h})");
                let m = d.matrix.map(|v| format!("{v}")).join(" ");
                format!(
                    "(:size ({} {}) :bpp {} :mask {} :word {} :made {} :rect {} :clip {} :src ({} {}) :matrix ({m}))",
                    d.w,
                    d.h,
                    d.bpp,
                    if d.mask { "t" } else { "nil" },
                    d.word,
                    d.made.map_or("nil".into(), |m| m.to_string()),
                    r(d.rect),
                    d.clip.map_or("nil".into(), r),
                    d.src.0,
                    d.src.1
                )
            })
            .collect();
        format!("({})", items.join(" "))
    })
    .unwrap_or_else(|| "nil".into())
}

#[cfg(all(test, feature = "test-hooks"))]
mod tests {
    use super::*;

    /// One test: the trace is the process's.
    #[test]
    fn trace_keeps_the_first_notes_the_waits_and_the_newest_images() {
        for i in 0..NOTES + 5 {
            note(|| format!("n{i}"));
        }
        wait_round(7, true);
        wait_round(7, false);
        let s = startup_text();
        assert!(
            s.starts_with("(\"")
                && s.contains(" n0\"")
                && s.contains(&format!(" n{}\"", NOTES - 1))
        );
        assert!(!s.contains(&format!(" n{}\"", NOTES)), "{s}");
        assert!(s.ends_with("(wait 7 :rounds 2 :timeouts 1))"), "{s}");
        for i in 0..IMAGES as i32 + 2 {
            let clip = (i == 2).then_some((1, 2, 3, 4));
            image(ImageDraw {
                w: i,
                h: 8,
                bpp: 32,
                mask: false,
                word: 0xffff_0000,
                made: Some(1),
                rect: (0, 0, 8, 8),
                clip,
                src: (0, 0),
                matrix: [0.0; 6],
            });
        }
        let s = images_text();
        assert_eq!(s.matches("(:size").count(), IMAGES, "{s}");
        assert!(
            s.starts_with(
                "((:size (2 8) :bpp 32 :mask nil :word 4294901760 :made 1 :rect (0 0 8 8) :clip (1 2 3 4) :src (0 0) :matrix (0 0 0 0 0 0)) (:size (3 8)"
            ),
            "{s}"
        );
        assert!(
            s.ends_with(":made 1 :rect (0 0 8 8) :clip nil :src (0 0) :matrix (0 0 0 0 0 0)))"),
            "{s}"
        );
        // a full pump trace of long paints still fits a query's answer: the newest entries, and how many were left out
        for i in 0..PUMPS {
            pump(|| format!("p{i} {}", "x".repeat(200)));
        }
        let s = pumps_text();
        assert!(s.len() < ANSWER, "{} bytes", s.len());
        assert!(
            s.starts_with("(\"") && s.contains(" older entries left out\""),
            "{s}"
        );
        assert!(
            s.ends_with(&format!("\"p{} {}\")", PUMPS - 1, "x".repeat(200))),
            "{s}"
        );
        assert_eq!(pumps_text(), "()");
    }
}

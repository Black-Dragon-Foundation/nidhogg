//! The C ABI: status codes, the panic guard and the exported functions.

use crate::compose::{Source, SourceView, Target};
use crate::geom::Rect;
use crate::scene::{Chrome, LayerDesc};
use crate::stats::{StatsEntry, Summary};
use std::panic::{UnwindSafe, catch_unwind};

pub const GLITNIR_OK: i32 = 0;
pub const GLITNIR_E_PANIC: i32 = -1;
pub const GLITNIR_E_ARG: i32 = -2;
/// A call in the wrong state (a scene call for a root without a scene).
pub const GLITNIR_E_STATE: i32 = -3;

/// The ABI version the header describes.
const ABI_VERSION: u32 = 1;

/// Every exported C symbol; a test checks this list against `include/glitnir.h`.
pub const EXPORTS: &[&str] = &[
    "glitnir_abi_version",
    "glitnir_commit",
    "glitnir_compose",
    "glitnir_damage_add_n",
    "glitnir_damage_all",
    "glitnir_damage_forget",
    "glitnir_damage_pending",
    "glitnir_debug_panic",
    "glitnir_img_decode",
    "glitnir_img_free",
    "glitnir_layer_remove",
    "glitnir_layer_set_chrome",
    "glitnir_layer_upsert",
    "glitnir_root_drop",
    "glitnir_root_new",
    "glitnir_scroll",
    "glitnir_stack_list",
    "glitnir_stack_lower",
    "glitnir_stack_raise",
    "glitnir_stack_reparent",
    "glitnir_stats_forget",
    "glitnir_stats_record",
    "glitnir_stats_summary",
];

fn code(r: Result<(), i32>) -> i32 {
    r.err().unwrap_or(GLITNIR_OK)
}

/// Run an export's body; a panic becomes `GLITNIR_E_PANIC` instead of unwinding into C.
pub(crate) fn ffi_guard<F: FnOnce() -> i32 + UnwindSafe>(f: F) -> i32 {
    catch_unwind(f).unwrap_or(GLITNIR_E_PANIC)
}

/// Write the ABI version to `out`.
///
/// # Safety
/// `out` must be null or point to a writable `u32`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn glitnir_abi_version(out: *mut u32) -> i32 {
    ffi_guard(|| {
        if out.is_null() {
            return GLITNIR_E_ARG;
        }
        // SAFETY: non-null, and the caller promises it is writable.
        unsafe { out.write(ABI_VERSION) };
        GLITNIR_OK
    })
}

/// Panic inside the guard (tests, and the C side's fallback check); returns `GLITNIR_E_PANIC`.
#[unsafe(no_mangle)]
pub extern "C" fn glitnir_debug_panic() -> i32 {
    ffi_guard(|| panic!("glitnir_debug_panic"))
}

/// Append one paint's record to ROOT's ring, its compose times and layers filled from ROOT's composes when it has
/// none ([`crate::compose::fill_stats`]).
///
/// # Safety
/// `entry` must be null or point to a readable `StatsEntry`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn glitnir_stats_record(root: u64, entry: *const StatsEntry) -> i32 {
    ffi_guard(|| {
        if entry.is_null() {
            return GLITNIR_E_ARG;
        }
        // SAFETY: non-null, and the caller promises it is readable.
        let mut e = unsafe { entry.read() };
        crate::compose::fill_stats(root, &mut e);
        crate::stats::record(root, e);
        GLITNIR_OK
    })
}

/// Write ROOT's summary to `out`, then empty its ring when `reset` is non-zero.
///
/// # Safety
/// `out` must be null or point to a writable `Summary`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn glitnir_stats_summary(root: u64, reset: i32, out: *mut Summary) -> i32 {
    ffi_guard(|| {
        if out.is_null() {
            return GLITNIR_E_ARG;
        }
        // SAFETY: non-null, and the caller promises it is writable.
        unsafe { out.write(crate::stats::summary(root, reset != 0)) };
        GLITNIR_OK
    })
}

/// Drop ROOT's ring (the root is gone).
#[unsafe(no_mangle)]
pub extern "C" fn glitnir_stats_forget(root: u64) -> i32 {
    ffi_guard(|| {
        crate::stats::forget(root);
        crate::compose::forget(root);
        GLITNIR_OK
    })
}

/// Add N rectangles (device pixels) to FRAME's pending damage.
///
/// # Safety
/// `rects` must point to `n` readable rectangles, or be null when `n` is 0.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn glitnir_damage_add_n(frame: u64, rects: *const Rect, n: usize) -> i32 {
    ffi_guard(|| {
        if n == 0 {
            return GLITNIR_OK;
        }
        if rects.is_null() {
            return GLITNIR_E_ARG;
        }
        // SAFETY: non-null, and the caller promises N readable rectangles.
        let rs = unsafe { std::slice::from_raw_parts(rects, n) };
        crate::damage::with(frame, |d| rs.iter().for_each(|r| d.add(*r)));
        GLITNIR_OK
    })
}

/// Damage the whole of FRAME.
#[unsafe(no_mangle)]
pub extern "C" fn glitnir_damage_all(frame: u64) -> i32 {
    ffi_guard(|| {
        crate::damage::with(frame, |d| d.add(crate::damage::ALL));
        GLITNIR_OK
    })
}

/// WINDOW's pixels in AREA moved by DY on FRAME; EXPOSED is what the move left stale.
#[unsafe(no_mangle)]
pub extern "C" fn glitnir_scroll(
    frame: u64,
    window: u64,
    area: Rect,
    dy: i32,
    exposed: Rect,
) -> i32 {
    // P1 composes a scroll as damage over the window's layer; the distance is for a backing with overscan later.
    let _ = dy;
    ffi_guard(|| {
        crate::damage::with(frame, |d| {
            d.scroll(area, exposed);
            d.scrolled(window);
        });
        GLITNIR_OK
    })
}

/// Take FRAME's pending damage: `*out` points to `*n` rectangles, valid until FRAME's next take or forget.
///
/// # Safety
/// `out` and `n` must be null or writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn glitnir_damage_pending(
    frame: u64,
    out: *mut *const Rect,
    n: *mut usize,
) -> i32 {
    ffi_guard(|| {
        if out.is_null() || n.is_null() {
            return GLITNIR_E_ARG;
        }
        let (p, len) = crate::damage::take(frame);
        // SAFETY: both non-null, and the caller promises they are writable.
        unsafe {
            out.write(p);
            n.write(len);
        }
        GLITNIR_OK
    })
}

/// What a commit hands back: ROOT's damage, valid until ROOT's next commit.  Mirrors `GlitnirCommitOut`.
#[repr(C)]
#[derive(Debug)]
pub struct CommitOut {
    pub rects: *const Rect,
    pub n: usize,
}

/// Commit ROOT at a consistency point: `out` gets its damage, valid until ROOT's next commit or forget.
///
/// # Safety
/// `out` must be null or point to a writable `CommitOut`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn glitnir_commit(root: u64, out: *mut CommitOut) -> i32 {
    ffi_guard(|| {
        if out.is_null() {
            return GLITNIR_E_ARG;
        }
        let (rects, n) = crate::damage::commit(root);
        // SAFETY: non-null, and the caller promises it is writable.
        unsafe { out.write(CommitOut { rects, n }) };
        GLITNIR_OK
    })
}

/// Drop FRAME's damage (the frame is gone).
#[unsafe(no_mangle)]
pub extern "C" fn glitnir_damage_forget(frame: u64) -> i32 {
    ffi_guard(|| {
        crate::damage::forget(frame);
        GLITNIR_OK
    })
}

/// Give ROOT a scene (an existing one is kept); its commits return the scene damage from then on.
#[unsafe(no_mangle)]
pub extern "C" fn glitnir_root_new(root: u64) -> i32 {
    ffi_guard(|| {
        crate::scene::root_new(root);
        GLITNIR_OK
    })
}

/// Drop ROOT's scene.
#[unsafe(no_mangle)]
pub extern "C" fn glitnir_root_drop(root: u64) -> i32 {
    ffi_guard(|| {
        crate::scene::root_drop(root);
        GLITNIR_OK
    })
}

/// Add or change a layer of ROOT's scene.
///
/// # Safety
/// `desc` must be null or point to a readable `LayerDesc`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn glitnir_layer_upsert(root: u64, desc: *const LayerDesc) -> i32 {
    ffi_guard(|| {
        if desc.is_null() {
            return GLITNIR_E_ARG;
        }
        // SAFETY: non-null, and the caller promises it is readable.
        let d = unsafe { desc.read() };
        code(crate::scene::with(root, |s| s.upsert(&d)))
    })
}

/// Remove layer ID, and every layer under it, from ROOT's scene.
#[unsafe(no_mangle)]
pub extern "C" fn glitnir_layer_remove(root: u64, id: u64) -> i32 {
    ffi_guard(|| {
        code(crate::scene::with(root, |s| {
            s.remove(id);
            Ok(())
        }))
    })
}

/// Set layer ID's chrome; null clears it.
///
/// # Safety
/// `chrome` must be null or point to a readable `Chrome`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn glitnir_layer_set_chrome(
    root: u64,
    id: u64,
    chrome: *const Chrome,
) -> i32 {
    ffi_guard(|| {
        // SAFETY: null, or readable as the caller promises.
        let c = (!chrome.is_null()).then(|| unsafe { chrome.read() });
        code(crate::scene::with(root, |s| s.set_chrome(id, c)))
    })
}

/// Raise layer ID to the top of its siblings.
#[unsafe(no_mangle)]
pub extern "C" fn glitnir_stack_raise(root: u64, id: u64) -> i32 {
    ffi_guard(|| code(crate::scene::with(root, |s| s.raise(id))))
}

/// Lower layer ID to the bottom of its siblings.
#[unsafe(no_mangle)]
pub extern "C" fn glitnir_stack_lower(root: u64, id: u64) -> i32 {
    ffi_guard(|| code(crate::scene::with(root, |s| s.lower(id))))
}

/// Move layer ID, with its subtree, to the top of PARENT's children (0: the top level).
#[unsafe(no_mangle)]
pub extern "C" fn glitnir_stack_reparent(root: u64, id: u64, parent: u64) -> i32 {
    ffi_guard(|| code(crate::scene::with(root, |s| s.reparent(id, parent))))
}

/// ROOT's child nodes bottom to top: `*ids` points to `*n` ids, valid until ROOT's next list or drop.
///
/// # Safety
/// `ids` and `n` must be null or writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn glitnir_stack_list(root: u64, ids: *mut *const u64, n: *mut usize) -> i32 {
    ffi_guard(|| {
        if ids.is_null() || n.is_null() {
            return GLITNIR_E_ARG;
        }
        match crate::scene::with(root, |s| {
            let l = s.list();
            Ok((l.as_ptr(), l.len()))
        }) {
            Ok((p, len)) => {
                // SAFETY: both non-null, and the caller promises they are writable.
                unsafe {
                    ids.write(p);
                    n.write(len);
                }
                GLITNIR_OK
            }
            Err(e) => e,
        }
    })
}

/// Compose ROOT's committed scene into TARGET within the NCLIP rectangles at CLIP, from the NSRC sources at SOURCES
/// ([`crate::compose`]).
///
/// # Safety
/// `target` must be null or point to a readable `Target` whose pixels are writable as it describes; `clip` must point
/// to `nclip` readable rectangles and `sources` to `nsrc` readable `Source`s (either may be null when its count is
/// 0), whose pixels are readable as they describe and overlap none of the target's.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn glitnir_compose(
    root: u64,
    target: *const Target,
    clip: *const Rect,
    nclip: usize,
    sources: *const Source,
    nsrc: usize,
) -> i32 {
    ffi_guard(|| {
        if target.is_null() || (clip.is_null() && nclip > 0) || (sources.is_null() && nsrc > 0) {
            return GLITNIR_E_ARG;
        }
        let clips: &[Rect] = if nclip == 0 {
            &[]
        } else {
            // SAFETY: non-null, and the caller promises NCLIP readable rectangles.
            unsafe { std::slice::from_raw_parts(clip, nclip) }
        };
        let srcs: &[Source] = if nsrc == 0 {
            &[]
        } else {
            // SAFETY: non-null, and the caller promises NSRC readable sources.
            unsafe { std::slice::from_raw_parts(sources, nsrc) }
        };
        // SAFETY: the caller promises readable sources, each with readable pixels.
        let views: Result<Vec<SourceView>, i32> =
            srcs.iter().map(|s| unsafe { s.view() }).collect();
        // SAFETY: non-null and readable, with writable pixels no source overlaps, as the caller promises.
        let t = unsafe { target.read() };
        // SAFETY: as above.
        let mut t = match unsafe { t.view() } {
            Ok(t) => t,
            Err(e) => return e,
        };
        code(views.and_then(|v| crate::compose::compose_root(root, &mut t, clips, &v)))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
        for e in std::fs::read_dir(dir).unwrap() {
            let p = e.unwrap().path();
            if p.is_dir() {
                rust_files(&p, out);
            } else if p.extension().is_some_and(|x| x == "rs") {
                out.push(p);
            }
        }
    }

    fn crate_file(rel: &str) -> String {
        std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join(rel)).unwrap()
    }

    #[test]
    fn a_panicking_export_returns_the_error_code() {
        assert_eq!(glitnir_debug_panic(), GLITNIR_E_PANIC);
    }

    #[test]
    fn abi_version_is_written() {
        let mut v = 0;
        assert_eq!(unsafe { glitnir_abi_version(&mut v) }, GLITNIR_OK);
        assert_eq!(v, 1);
        assert_eq!(
            unsafe { glitnir_abi_version(std::ptr::null_mut()) },
            GLITNIR_E_ARG
        );
    }

    fn pending(frame: u64) -> Vec<Rect> {
        let (mut p, mut n) = (std::ptr::null::<Rect>(), 0usize);
        assert_eq!(
            unsafe { glitnir_damage_pending(frame, &mut p, &mut n) },
            GLITNIR_OK
        );
        if n == 0 {
            return Vec::new();
        }
        // SAFETY: the library keeps N rectangles at P until FRAME's next take.
        unsafe { std::slice::from_raw_parts(p, n) }.to_vec()
    }

    #[test]
    fn damage_round_trips_through_the_abi() {
        let f = 8001;
        assert!(pending(f).is_empty());
        let rects = [
            Rect::new(0, 0, 10, 2),
            Rect::new(0, 0, 4, 1),
            Rect::new(50, 50, 0, 3),
        ];
        assert_eq!(
            unsafe { glitnir_damage_add_n(f, rects.as_ptr(), rects.len()) },
            GLITNIR_OK
        );
        assert_eq!(pending(f), vec![Rect::new(0, 0, 10, 2)]);
        assert!(pending(f).is_empty());
        let area = Rect::new(0, 100, 300, 200);
        assert_eq!(
            glitnir_scroll(f, 3, area, -40, Rect::new(0, 260, 300, 40)),
            GLITNIR_OK
        );
        assert_eq!(pending(f), vec![area]);
        assert_eq!(glitnir_damage_all(f), GLITNIR_OK);
        assert_eq!(
            unsafe { glitnir_damage_add_n(f, rects.as_ptr(), 1) },
            GLITNIR_OK
        );
        assert_eq!(pending(f), vec![Rect::new(0, 0, i32::MAX, i32::MAX)]);
        assert_eq!(glitnir_damage_forget(f), GLITNIR_OK);
        assert!(pending(f).is_empty());
    }

    #[test]
    fn commit_round_trips_through_the_abi() {
        let f = 8002;
        let mut out = CommitOut {
            rects: std::ptr::null(),
            n: 7,
        };
        assert_eq!(unsafe { glitnir_commit(f, &mut out) }, GLITNIR_OK);
        assert_eq!(out.n, 0);
        let rects = [Rect::new(3, 4, 5, 6), Rect::new(100, 0, 2, 2)];
        assert_eq!(
            unsafe { glitnir_damage_add_n(f, rects.as_ptr(), rects.len()) },
            GLITNIR_OK
        );
        assert_eq!(unsafe { glitnir_commit(f, &mut out) }, GLITNIR_OK);
        // SAFETY: the library keeps OUT.N rectangles at OUT.RECTS until F's next commit.
        let mut got = unsafe { std::slice::from_raw_parts(out.rects, out.n) }.to_vec();
        got.sort_by_key(|r| (r.x, r.y));
        assert_eq!(got, rects.to_vec());
        assert!(pending(f).is_empty());
        assert_eq!(
            unsafe { glitnir_commit(f, std::ptr::null_mut()) },
            GLITNIR_E_ARG
        );
        assert_eq!(glitnir_damage_forget(f), GLITNIR_OK);
    }

    #[test]
    fn damage_rejects_null_arguments() {
        let (mut p, mut n) = (std::ptr::null::<Rect>(), 0usize);
        assert_eq!(
            unsafe { glitnir_damage_add_n(1, std::ptr::null(), 2) },
            GLITNIR_E_ARG
        );
        assert_eq!(
            unsafe { glitnir_damage_add_n(1, std::ptr::null(), 0) },
            GLITNIR_OK
        );
        assert_eq!(
            unsafe { glitnir_damage_pending(1, std::ptr::null_mut(), &mut n) },
            GLITNIR_E_ARG
        );
        assert_eq!(
            unsafe { glitnir_damage_pending(1, &mut p, std::ptr::null_mut()) },
            GLITNIR_E_ARG
        );
    }

    #[test]
    fn rect_has_the_layout_of_glitnir_rect() {
        assert_eq!(std::mem::size_of::<Rect>(), 16);
        assert_eq!(std::mem::align_of::<Rect>(), 4);
    }

    #[test]
    fn layer_desc_and_chrome_have_the_c_layout() {
        assert_eq!(std::mem::size_of::<LayerDesc>(), 56);
        assert_eq!(std::mem::offset_of!(LayerDesc, rect), 20);
        assert_eq!(std::mem::offset_of!(LayerDesc, scale), 40);
        assert_eq!(std::mem::offset_of!(LayerDesc, visible), 48);
        assert_eq!(std::mem::size_of::<Chrome>(), 36);
        assert_eq!(std::mem::offset_of!(Chrome, border_argb), 32);
    }

    #[test]
    fn source_and_target_have_the_c_layout() {
        assert_eq!(std::mem::size_of::<Source>(), 40);
        assert_eq!(std::mem::offset_of!(Source, pixels), 8);
        assert_eq!(std::mem::offset_of!(Source, height), 24);
        assert_eq!(std::mem::offset_of!(Source, scale), 32);
        assert_eq!(std::mem::size_of::<Target>(), 40);
        assert_eq!(std::mem::offset_of!(Target, origin_y), 24);
        assert_eq!(std::mem::offset_of!(Target, scale), 32);
    }

    #[test]
    fn layers_round_trip_through_the_abi() {
        let r = 8101;
        let d = |id, parent, kind, rect| LayerDesc {
            id,
            parent,
            kind,
            rect,
            scale: 1.0,
            visible: 1,
        };
        let root = d(r, 0, 1, Rect::new(0, 0, 400, 300));
        assert_eq!(unsafe { glitnir_layer_upsert(r, &root) }, GLITNIR_E_STATE);
        assert_eq!(glitnir_root_new(r), GLITNIR_OK);
        assert_eq!(unsafe { glitnir_layer_upsert(r, &root) }, GLITNIR_OK);
        assert_eq!(
            unsafe { glitnir_layer_upsert(r, std::ptr::null()) },
            GLITNIR_E_ARG
        );
        assert_eq!(
            unsafe { glitnir_layer_upsert(r, &d(5, r, 9, Rect::default())) },
            GLITNIR_E_ARG
        );
        for id in [10, 20] {
            let c = d(id, r, 3, Rect::new(10, 10, 50, 50));
            assert_eq!(unsafe { glitnir_layer_upsert(r, &c) }, GLITNIR_OK);
        }
        let ch = Chrome {
            blur_px: 8.0,
            shadow_opacity: 0.5,
            ..Chrome::default()
        };
        assert_eq!(unsafe { glitnir_layer_set_chrome(r, 10, &ch) }, GLITNIR_OK);
        assert_eq!(
            unsafe { glitnir_layer_set_chrome(r, 99, std::ptr::null()) },
            GLITNIR_E_ARG
        );
        let list = || {
            let (mut p, mut n) = (std::ptr::null::<u64>(), 0usize);
            assert_eq!(unsafe { glitnir_stack_list(r, &mut p, &mut n) }, GLITNIR_OK);
            // SAFETY: valid until R's next list.
            unsafe { std::slice::from_raw_parts(p, n) }.to_vec()
        };
        assert_eq!(list(), vec![10, 20]);
        assert_eq!(glitnir_stack_raise(r, 10), GLITNIR_OK);
        assert_eq!(list(), vec![20, 10]);
        assert_eq!(glitnir_stack_lower(r, 10), GLITNIR_OK);
        assert_eq!(glitnir_stack_reparent(r, 20, 10), GLITNIR_OK);
        assert_eq!(list(), vec![10, 20]);
        assert_eq!(glitnir_stack_reparent(r, 10, 20), GLITNIR_E_ARG);
        assert_eq!(glitnir_stack_raise(r, 99), GLITNIR_E_ARG);
        let mut out = CommitOut {
            rects: std::ptr::null(),
            n: 0,
        };
        assert_eq!(unsafe { glitnir_commit(r, &mut out) }, GLITNIR_OK);
        // SAFETY: valid until R's next commit.
        let got = unsafe { std::slice::from_raw_parts(out.rects, out.n) };
        assert!(got.contains(&Rect::new(0, 0, 400, 300)), "{got:?}");
        assert_eq!(glitnir_layer_remove(r, 10), GLITNIR_OK);
        assert!(list().is_empty());
        assert_eq!(glitnir_root_drop(r), GLITNIR_OK);
        assert_eq!(glitnir_stack_raise(r, 20), GLITNIR_E_STATE);
        assert_eq!(glitnir_damage_forget(r), GLITNIR_OK);
    }

    #[test]
    fn the_header_lists_every_export() {
        let header = crate_file("include/glitnir.h");
        let mut found: Vec<String> = Vec::new();
        let b = header.as_bytes();
        let mut i = 0;
        while let Some(off) = header[i..].find("glitnir_") {
            let start = i + off;
            let mut end = start;
            while end < b.len()
                && (b[end].is_ascii_lowercase() || b[end].is_ascii_digit() || b[end] == b'_')
            {
                end += 1;
            }
            // a function declaration: the name is followed by " ("
            if header[end..].starts_with(" (") {
                found.push(header[start..end].to_string());
            }
            i = end.max(start + 1);
        }
        found.sort();
        found.dedup();
        let mut want: Vec<String> = EXPORTS.iter().map(|s| s.to_string()).collect();
        want.sort();
        assert_eq!(found, want);
    }

    #[test]
    fn the_crate_names_no_host() {
        // The needles are assembled so that this file does not contain them.
        let needles = [
            ["nid", "hogg"].concat(),
            ["drgn", "x"].concat(),
            ["ema", "cs"].concat(),
        ];
        let mut files = Vec::new();
        rust_files(
            &Path::new(env!("CARGO_MANIFEST_DIR")).join("src"),
            &mut files,
        );
        let mut texts: Vec<(String, String)> = files
            .iter()
            .map(|p| (p.display().to_string(), std::fs::read_to_string(p).unwrap()))
            .collect();
        texts.push(("include/glitnir.h".into(), crate_file("include/glitnir.h")));
        for (name, text) in texts {
            let lower = text.to_lowercase();
            for n in &needles {
                assert!(!lower.contains(n.as_str()), "{name} mentions {n}");
            }
        }
    }
}

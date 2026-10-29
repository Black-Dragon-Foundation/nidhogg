//! The C ABI: the header against the Rust side.

mod abi {
    use nhglue::abi::*;
    use nhglue::action::*;
    use nhglue::backing::{
        NHG_GEOM_FRESH, NHG_GEOM_LAYER_CHANGED, NhgBacking, NhgFrameGeom, NhgRect,
    };
    use nhglue::bench::NhgBenchOut;
    use nhglue::ffi::{
        NhgDisplay, nhg_abi_version, nhg_close, nhg_dispatch_main, nhg_fd, nhg_open,
    };
    use nhglue::ffi::{
        nhg_backing, nhg_backing_forget, nhg_damage, nhg_damage_all, nhg_layer_ensure, nhg_scroll,
        nhg_snapshot,
    };
    use nhglue::ffi::{
        nhg_bell, nhg_bench_full_redraw, nhg_display_info, nhg_pointer_visible,
        nhg_selection_disown, nhg_selection_owned, nhg_selection_read, nhg_selection_set,
        nhg_selection_types,
    };
    use nhglue::ffi::{nhg_frame_csd, nhg_frame_resized, nhg_frame_scale, nhg_next};
    use nhglue::ffi::{nhg_frame_forget, nhg_frame_register, nhg_panic_message};
    use nhglue::ffi::{nhg_present, nhg_presenter_stats, nhg_presenter_thread};
    use nhglue::frame::*;
    use nhglue::keys::{NHG_KEY_CHAR, NHG_KEY_REPEAT, NHG_KEY_SYMBOL, NHG_KEY_TEXT};
    use nhglue::present::*;
    use nhglue::query::{NhgDisplayInfo, NhgMonitor};
    use std::ffi::c_char;
    use std::mem::offset_of;
    use std::process::Command;

    /// Compile a probe that includes `nhglue.h` (and `glitnir.h`, whose rectangle `NhgRect` matches) and prints each
    /// of `exprs` as a signed integer; return the values.  `decls` go before `main` (they must compile against the
    /// header, with every warning an error).
    fn probe(decls: &str, exprs: &[&str]) -> Vec<i64> {
        // one directory per probe: the tests run in parallel
        static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("nhglue-layout-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut c = format!(
            "#include <stdio.h>\n#include <stddef.h>\n#include \"nhglue.h\"\n#include \"glitnir.h\"\n{decls}\nint main(void) {{\n"
        );
        for e in exprs {
            c += &format!("  printf(\"%lld\\n\", (long long) ({e}));\n");
        }
        c += "  return 0;\n}\n";
        let src = dir.join("probe.c");
        let exe = dir.join("probe");
        std::fs::write(&src, c).unwrap();
        let root = env!("CARGO_MANIFEST_DIR");
        let cc = Command::new("cc")
            .args(["-std=gnu11", "-Wall", "-Werror"])
            .args(["-I", &format!("{root}/include")])
            .args(["-I", &format!("{root}/../glitnir/include")])
            .arg("-o")
            .arg(&exe)
            .arg(&src)
            .output()
            .expect("cc not found");
        assert!(
            cc.status.success(),
            "probe compile failed:\n{}",
            String::from_utf8_lossy(&cc.stderr)
        );
        let out = Command::new(&exe).output().unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .map(|l| l.parse().unwrap())
            .collect()
    }

    /// The header's constants equal the Rust ones, its codes equal the shell's where they mean the same, and its
    /// prototypes have the types the Rust entry points have (each side is pinned to the table below).
    #[test]
    fn header_matches_records() {
        type F = NhgFrameFacts;
        type A = NhgAction;
        let rows: &[(&str, i64)] = &[
            ("NHG_ABI_VERSION", NHG_ABI_VERSION.into()),
            ("NHG_OK", NHG_OK.into()),
            ("NHG_E_PANIC", NHG_E_PANIC.into()),
            ("NHG_E_ARG", NHG_E_ARG.into()),
            ("NHG_E_STATE", NHG_E_STATE.into()),
            ("NHG_E_UNSUPPORTED", NHG_E_UNSUPPORTED.into()),
            ("NHG_E_DISCONNECTED", NHG_E_DISCONNECTED.into()),
            ("NHG_E_NOMEM", NHG_E_NOMEM.into()),
            ("NHG_E_TIMEOUT", NHG_E_TIMEOUT.into()),
            ("NHG_E_TOO_SMALL", NHG_E_TOO_SMALL.into()),
            ("NHG_E_THREAD", NHG_E_THREAD.into()),
            ("NHG_E_RECONNECTING", NHG_E_RECONNECTING.into()),
            // the codes equal the shell's where they mean the same
            ("NHG_E_PANIC", nhshell::api::NHSHELL_E_PANIC.into()),
            ("NHG_E_ARG", nhshell::api::NHSHELL_E_ARG.into()),
            ("NHG_E_STATE", nhshell::api::NHSHELL_E_STATE.into()),
            (
                "NHG_E_UNSUPPORTED",
                nhshell::api::NHSHELL_E_UNSUPPORTED.into(),
            ),
            (
                "NHG_E_DISCONNECTED",
                nhshell::api::NHSHELL_E_DISCONNECTED.into(),
            ),
            ("NHG_E_NOMEM", nhshell::api::NHSHELL_E_NOMEM.into()),
            ("NHG_E_TIMEOUT", nhshell::api::NHSHELL_E_TIMEOUT.into()),
            ("NHG_E_TOO_SMALL", nhshell::api::NHSHELL_E_TOO_SMALL.into()),
            // backings, damage and scroll
            ("NHG_GEOM_LAYER_CHANGED", NHG_GEOM_LAYER_CHANGED.into()),
            ("NHG_GEOM_FRESH", NHG_GEOM_FRESH.into()),
            ("sizeof (NhgRect)", size_of::<NhgRect>() as i64),
            ("sizeof (GlitnirRect)", size_of::<NhgRect>() as i64),
            ("offsetof (NhgRect, h)", offset_of!(NhgRect, h) as i64),
            ("sizeof (NhgFrameGeom)", size_of::<NhgFrameGeom>() as i64),
            (
                "offsetof (NhgFrameGeom, b)",
                offset_of!(NhgFrameGeom, b) as i64,
            ),
            (
                "offsetof (NhgFrameGeom, bg_argb)",
                offset_of!(NhgFrameGeom, bg_argb) as i64,
            ),
            (
                "offsetof (NhgFrameGeom, flags)",
                offset_of!(NhgFrameGeom, flags) as i64,
            ),
            (
                "offsetof (NhgFrameGeom, layer)",
                offset_of!(NhgFrameGeom, layer) as i64,
            ),
            ("sizeof (NhgBacking)", size_of::<NhgBacking>() as i64),
            ("offsetof (NhgBacking, h)", offset_of!(NhgBacking, h) as i64),
            (
                "offsetof (NhgBacking, gen)",
                offset_of!(NhgBacking, generation) as i64,
            ),
            // presenting
            ("NHG_SURF_DRAWABLE", NHG_SURF_DRAWABLE.into()),
            ("NHG_SURF_ALPHA_HOST", NHG_SURF_ALPHA_HOST.into()),
            ("NHG_PRESENT_BACKSTOP", NHG_PRESENT_BACKSTOP.into()),
            ("NHG_PRESENT_READY", NHG_PRESENT_READY.into()),
            ("NHG_PRESENT_KEY_SEEN", NHG_PRESENT_KEY_SEEN.into()),
            ("NHG_PRESENTED_PUBLISHED", NHG_PRESENTED_PUBLISHED.into()),
            ("NHG_PRESENTED_COMMITTED", NHG_PRESENTED_COMMITTED.into()),
            ("NHG_PRESENTED_KEY_DONE", NHG_PRESENTED_KEY_DONE.into()),
            ("sizeof (NhgSurface)", size_of::<NhgSurface>() as i64),
            (
                "offsetof (NhgSurface, parent)",
                offset_of!(NhgSurface, parent) as i64,
            ),
            (
                "offsetof (NhgSurface, frame)",
                offset_of!(NhgSurface, frame) as i64,
            ),
            (
                "offsetof (NhgSurface, flags)",
                offset_of!(NhgSurface, flags) as i64,
            ),
            (
                "offsetof (NhgSurface, alpha_cut)",
                offset_of!(NhgSurface, alpha_cut) as i64,
            ),
            (
                "sizeof (NhgPresenterStats)",
                size_of::<NhgPresenterStats>() as i64,
            ),
            (
                "offsetof (NhgPresenterStats, frames)",
                offset_of!(NhgPresenterStats, frames) as i64,
            ),
            (
                "offsetof (NhgPresenterStats, ready_to_commit_max_ns)",
                offset_of!(NhgPresenterStats, ready_to_commit_max_ns) as i64,
            ),
            // the window registry
            ("NHG_FRAME_CHILD", NHG_FRAME_CHILD.into()),
            ("NHG_FRAME_TIP", NHG_FRAME_TIP.into()),
            ("NHG_FRAME_POPUP", NHG_FRAME_POPUP.into()),
            ("NHG_FACT_VISIBLE", NHG_FACT_VISIBLE.into()),
            ("NHG_FACT_ICONIFIED", NHG_FACT_ICONIFIED.into()),
            // NhgFrameFacts (F; FOFF is its offsetof, defined in the probe)
            ("sizeof (F)", size_of::<F>() as i64),
            ("_Alignof (F)", align_of::<F>() as i64),
            ("FOFF (native_w)", offset_of!(F, native_w) as i64),
            ("FOFF (native_h)", offset_of!(F, native_h) as i64),
            ("FOFF (col_w)", offset_of!(F, col_w) as i64),
            ("FOFF (line_h)", offset_of!(F, line_h) as i64),
            ("FOFF (parent)", offset_of!(F, parent) as i64),
            ("FOFF (flags)", offset_of!(F, flags) as i64),
            ("NHG_FACT_CSD", NHG_FACT_CSD.into()),
            // the event step
            ("NHG_IN_INPUT_WAITING", NHG_IN_INPUT_WAITING.into()),
            ("NHG_IN_COALESCE_WHEEL", NHG_IN_COALESCE_WHEEL.into()),
            ("NHG_ACT_KEY", NHG_ACT_KEY.into()),
            ("NHG_ACT_CLICK", NHG_ACT_CLICK.into()),
            ("NHG_ACT_WHEEL", NHG_ACT_WHEEL.into()),
            ("NHG_ACT_MOTION", NHG_ACT_MOTION.into()),
            ("NHG_ACT_LEAVE", NHG_ACT_LEAVE.into()),
            ("NHG_ACT_RESIZE", NHG_ACT_RESIZE.into()),
            ("NHG_ACT_SCALE", NHG_ACT_SCALE.into()),
            ("NHG_ACT_GARBAGE", NHG_ACT_GARBAGE.into()),
            ("NHG_ACT_DELETE", NHG_ACT_DELETE.into()),
            ("NHG_ACT_FOCUS_IN", NHG_ACT_FOCUS_IN.into()),
            ("NHG_ACT_FOCUS_OUT", NHG_ACT_FOCUS_OUT.into()),
            ("NHG_ACT_VISIBLE", NHG_ACT_VISIBLE.into()),
            ("NHG_ACT_ICONIFIED", NHG_ACT_ICONIFIED.into()),
            ("NHG_ACT_READY", NHG_ACT_READY.into()),
            ("NHG_ACT_MONITORS", NHG_ACT_MONITORS.into()),
            ("NHG_ACT_LOST", NHG_ACT_LOST.into()),
            ("NHG_ACT_PREEDIT", NHG_ACT_PREEDIT.into()),
            ("NHG_ACT_COMMIT", NHG_ACT_COMMIT.into()),
            ("NHG_ACT_IM_DELETE", NHG_ACT_IM_DELETE.into()),
            ("NHG_ACT_IM_SURROUNDING", NHG_ACT_IM_SURROUNDING.into()),
            ("NHG_ACT_SELECTION_CLEAR", NHG_ACT_SELECTION_CLEAR.into()),
            ("NHG_WHEEL_HIRES", nhglue::scroll::NHG_WHEEL_HIRES.into()),
            ("NHG_WHEEL_STOP", nhglue::scroll::NHG_WHEEL_STOP.into()),
            (
                "NHG_WHEEL_COALESCED",
                nhglue::scroll::NHG_WHEEL_COALESCED.into(),
            ),
            ("NHG_WHEEL_UP", nhglue::scroll::NHG_WHEEL_UP.into()),
            ("NHG_WHEEL_DOWN", nhglue::scroll::NHG_WHEEL_DOWN.into()),
            ("NHG_WHEEL_LEFT", nhglue::scroll::NHG_WHEEL_LEFT.into()),
            ("NHG_WHEEL_RIGHT", nhglue::scroll::NHG_WHEEL_RIGHT.into()),
            ("NHG_RESIZE_FULLSCREEN", NHG_RESIZE_FULLSCREEN.into()),
            ("sizeof (NhgNextIn)", size_of::<NhgNextIn>() as i64),
            ("sizeof (A)", size_of::<A>() as i64),
            ("_Alignof (A)", align_of::<A>() as i64),
            ("AOFF (window)", offset_of!(A, window) as i64),
            ("AOFF (time_ms)", offset_of!(A, time_ms) as i64),
            ("AOFF (x)", offset_of!(A, x) as i64),
            ("AOFF (code)", offset_of!(A, code) as i64),
            ("AOFF (mods)", offset_of!(A, mods) as i64),
            ("AOFF (flags)", offset_of!(A, flags) as i64),
            ("AOFF (dx)", offset_of!(A, dx) as i64),
            ("AOFF (scale)", offset_of!(A, scale) as i64),
            ("AOFF (text)", offset_of!(A, text) as i64),
            ("AOFF (len)", offset_of!(A, len) as i64),
            ("sizeof (NhgActionList)", size_of::<NhgActionList>() as i64),
            (
                "offsetof (NhgActionList, n)",
                offset_of!(NhgActionList, n) as i64,
            ),
            // the keyboard
            ("NHG_KEY_REPEAT", NHG_KEY_REPEAT.into()),
            ("NHG_KEY_CHAR", NHG_KEY_CHAR.into()),
            ("NHG_KEY_SYMBOL", NHG_KEY_SYMBOL.into()),
            ("NHG_KEY_TEXT", NHG_KEY_TEXT.into()),
            // selections, display queries and the bench
            ("sizeof (NhgMonitor)", size_of::<NhgMonitor>() as i64),
            ("offsetof (NhgMonitor, x)", offset_of!(NhgMonitor, x) as i64),
            (
                "offsetof (NhgMonitor, mm_h)",
                offset_of!(NhgMonitor, mm_h) as i64,
            ),
            (
                "offsetof (NhgMonitor, scale120)",
                offset_of!(NhgMonitor, scale120) as i64,
            ),
            (
                "offsetof (NhgMonitor, name)",
                offset_of!(NhgMonitor, name) as i64,
            ),
            (
                "sizeof (NhgDisplayInfo)",
                size_of::<NhgDisplayInfo>() as i64,
            ),
            (
                "offsetof (NhgDisplayInfo, mm_h)",
                offset_of!(NhgDisplayInfo, mm_h) as i64,
            ),
            (
                "offsetof (NhgDisplayInfo, scale120)",
                offset_of!(NhgDisplayInfo, scale120) as i64,
            ),
            (
                "offsetof (NhgDisplayInfo, caps)",
                offset_of!(NhgDisplayInfo, caps) as i64,
            ),
            ("sizeof (NhgBenchOut)", size_of::<NhgBenchOut>() as i64),
            (
                "offsetof (NhgBenchOut, compose_p50)",
                offset_of!(NhgBenchOut, compose_p50) as i64,
            ),
            (
                "offsetof (NhgBenchOut, interval_p95)",
                offset_of!(NhgBenchOut, interval_p95) as i64,
            ),
        ];
        // the prototypes, C side (checked at compile time: the probe references no function, so it links alone)
        let decls = "\
#define PROTO(f, t) _Static_assert (__builtin_types_compatible_p (__typeof__ (f), t), #f)
typedef NhgFrameFacts F;
#define FOFF(m) offsetof (F, m)
typedef NhgAction A;
#define AOFF(m) offsetof (A, m)
PROTO (nhg_frame_resized, int32_t (NhgDisplay *, uint64_t, int32_t, int32_t));
PROTO (nhg_frame_scale, int32_t (NhgDisplay *, uint64_t, uint32_t));
PROTO (nhg_frame_csd, int32_t (NhgDisplay *, uint64_t, int32_t));
PROTO (nhg_next, int32_t (NhgDisplay *, const NhgNextIn *, NhgActionList *));
PROTO (nhg_abi_version, int32_t (uint32_t *));
PROTO (nhg_open, NhgDisplay *(const char *, char *, size_t));
PROTO (nhg_close, int32_t (NhgDisplay *));
PROTO (nhg_fd, int32_t (NhgDisplay *));
PROTO (nhg_layer_ensure, int32_t (NhgDisplay *, uint64_t *, const NhgFrameGeom *));
PROTO (nhg_backing, int32_t (NhgDisplay *, uint64_t, const NhgFrameGeom *, NhgBacking *));
PROTO (nhg_damage, int32_t (NhgDisplay *, uint64_t, const NhgRect *, size_t));
PROTO (nhg_damage_all, int32_t (NhgDisplay *, uint64_t));
PROTO (nhg_scroll, int32_t (NhgDisplay *, uint64_t, uint64_t, NhgRect, int32_t));
PROTO (nhg_snapshot, int32_t (NhgDisplay *, uint64_t, NhgBacking *));
PROTO (nhg_backing_forget, int32_t (NhgDisplay *, uint64_t));
PROTO (nhg_present, int32_t (NhgDisplay *, uint64_t, const NhgSurface *, size_t, uint32_t, uint64_t));
PROTO (nhg_presenter_thread, int32_t (NhgDisplay *, int32_t));
PROTO (nhg_presenter_stats, int32_t (NhgDisplay *, NhgPresenterStats *));
PROTO (nhg_frame_register, int32_t (NhgDisplay *, uint64_t, uint32_t));
PROTO (nhg_frame_forget, int32_t (NhgDisplay *, uint64_t));
PROTO (nhg_panic_message, int32_t (NhgDisplay *, char *, size_t, size_t *));
PROTO (nhg_debug_check_frame, int32_t (NhgDisplay *, uint64_t, const NhgFrameFacts *));
PROTO (nhg_test_panic, int32_t (NhgDisplay *));
PROTO (nhg_selection_set, int32_t (NhgDisplay *, uint32_t, const char *const *, const uint8_t *const *, const size_t *, size_t));
PROTO (nhg_selection_disown, int32_t (NhgDisplay *, uint32_t));
PROTO (nhg_selection_owned, int32_t (NhgDisplay *, uint32_t));
PROTO (nhg_selection_types, int32_t (NhgDisplay *, uint32_t, const char **, size_t *));
PROTO (nhg_selection_read, int32_t (NhgDisplay *, uint32_t, const char *, uint32_t, const uint8_t **, size_t *));
PROTO (nhg_display_info, int32_t (NhgDisplay *, NhgDisplayInfo *));
PROTO (nhg_bench_full_redraw, int32_t (NhgDisplay *, const NhgSurface *, size_t, uint32_t, NhgBenchOut *));
PROTO (nhg_bell, int32_t (NhgDisplay *, uint64_t));
PROTO (nhg_pointer_visible, int32_t (NhgDisplay *, uint64_t, int32_t, uint32_t));
PROTO (nhg_frame_units, int32_t (NhgDisplay *, uint64_t, int32_t, int32_t));
PROTO (nhg_im_enable, int32_t (NhgDisplay *, uint64_t, int32_t));
PROTO (nhg_im_cursor, int32_t (NhgDisplay *, uint64_t, NhgRect));
PROTO (nhg_im_surrounding, int32_t (NhgDisplay *, uint64_t, const char *, size_t, int32_t, int32_t));
PROTO (nhg_monitors, int32_t (NhgDisplay *, NhgMonitor *, size_t, size_t *));
PROTO (nhg_frame_outputs, int32_t (NhgDisplay *, uint64_t, uint64_t *, size_t, size_t *));";
        // the same table, Rust side
        let _: unsafe extern "C" fn(*mut u32) -> i32 = nhg_abi_version;
        let _: unsafe extern "C" fn(*const c_char, *mut c_char, usize) -> *mut NhgDisplay =
            nhg_open;
        let _: unsafe extern "C" fn(*mut NhgDisplay) -> i32 = nhg_close;
        let _: unsafe extern "C" fn(*mut NhgDisplay) -> i32 = nhg_fd;
        // backings, damage and scroll
        let _: unsafe extern "C" fn(*mut NhgDisplay, *mut u64, *const NhgFrameGeom) -> i32 =
            nhg_layer_ensure;
        let _: unsafe extern "C" fn(
            *mut NhgDisplay,
            u64,
            *const NhgFrameGeom,
            *mut NhgBacking,
        ) -> i32 = nhg_backing;
        let _: unsafe extern "C" fn(*mut NhgDisplay, u64, *const NhgRect, usize) -> i32 =
            nhg_damage;
        let _: unsafe extern "C" fn(*mut NhgDisplay, u64) -> i32 = nhg_damage_all;
        let _: unsafe extern "C" fn(*mut NhgDisplay, u64, u64, NhgRect, i32) -> i32 = nhg_scroll;
        let _: unsafe extern "C" fn(*mut NhgDisplay, u64, *mut NhgBacking) -> i32 = nhg_snapshot;
        let _: unsafe extern "C" fn(*mut NhgDisplay, u64) -> i32 = nhg_backing_forget;
        // presenting
        let _: unsafe extern "C" fn(
            *mut NhgDisplay,
            u64,
            *const NhgSurface,
            usize,
            u32,
            u64,
        ) -> i32 = nhg_present;
        let _: unsafe extern "C" fn(*mut NhgDisplay, i32) -> i32 = nhg_presenter_thread;
        let _: unsafe extern "C" fn(*mut NhgDisplay, *mut NhgPresenterStats) -> i32 =
            nhg_presenter_stats;
        // the window registry and the guard
        let _: unsafe extern "C" fn(*mut NhgDisplay, u64, u32) -> i32 = nhg_frame_register;
        let _: unsafe extern "C" fn(*mut NhgDisplay, u64) -> i32 = nhg_frame_forget;
        let _: unsafe extern "C" fn(*mut NhgDisplay, *mut c_char, usize, *mut usize) -> i32 =
            nhg_panic_message;
        // the pushes and the event step
        let _: unsafe extern "C" fn(*mut NhgDisplay, u64, i32, i32) -> i32 = nhg_frame_resized;
        let _: unsafe extern "C" fn(*mut NhgDisplay, u64, u32) -> i32 = nhg_frame_scale;
        let _: unsafe extern "C" fn(*mut NhgDisplay, u64, i32) -> i32 = nhg_frame_csd;
        let _: unsafe extern "C" fn(*mut NhgDisplay, *const NhgNextIn, *mut NhgActionList) -> i32 =
            nhg_next;
        #[cfg(feature = "test-hooks")]
        {
            use nhglue::ffi::{nhg_debug_check_frame, nhg_test_panic};
            let _: unsafe extern "C" fn(*mut NhgDisplay, u64, *const NhgFrameFacts) -> i32 =
                nhg_debug_check_frame;
            let _: unsafe extern "C" fn(*mut NhgDisplay) -> i32 = nhg_test_panic;
        }
        // selections, display queries, the bench, the bell and the pointer
        let _: unsafe extern "C" fn(
            *mut NhgDisplay,
            u32,
            *const *const c_char,
            *const *const u8,
            *const usize,
            usize,
        ) -> i32 = nhg_selection_set;
        let _: unsafe extern "C" fn(*mut NhgDisplay, u32) -> i32 = nhg_selection_disown;
        let _: unsafe extern "C" fn(*mut NhgDisplay, u32) -> i32 = nhg_selection_owned;
        let _: unsafe extern "C" fn(*mut NhgDisplay, u32, *mut *const c_char, *mut usize) -> i32 =
            nhg_selection_types;
        let _: unsafe extern "C" fn(
            *mut NhgDisplay,
            u32,
            *const c_char,
            u32,
            *mut *const u8,
            *mut usize,
        ) -> i32 = nhg_selection_read;
        let _: unsafe extern "C" fn(*mut NhgDisplay, *mut NhgDisplayInfo) -> i32 = nhg_display_info;
        let _: unsafe extern "C" fn(
            *mut NhgDisplay,
            *const NhgSurface,
            usize,
            u32,
            *mut NhgBenchOut,
        ) -> i32 = nhg_bench_full_redraw;
        let _: unsafe extern "C" fn(*mut NhgDisplay, u64) -> i32 = nhg_bell;
        let _: unsafe extern "C" fn(*mut NhgDisplay, u64, i32, u32) -> i32 = nhg_pointer_visible;
        // wheel units, the input method and outputs
        {
            use nhglue::ffi::{
                nhg_frame_outputs, nhg_frame_units, nhg_im_cursor, nhg_im_enable,
                nhg_im_surrounding, nhg_monitors,
            };
            let _: unsafe extern "C" fn(*mut NhgDisplay, u64, i32, i32) -> i32 = nhg_frame_units;
            let _: unsafe extern "C" fn(*mut NhgDisplay, u64, i32) -> i32 = nhg_im_enable;
            let _: unsafe extern "C" fn(*mut NhgDisplay, u64, NhgRect) -> i32 = nhg_im_cursor;
            let _: unsafe extern "C" fn(
                *mut NhgDisplay,
                u64,
                *const c_char,
                usize,
                i32,
                i32,
            ) -> i32 = nhg_im_surrounding;
            let _: unsafe extern "C" fn(
                *mut NhgDisplay,
                *mut NhgMonitor,
                usize,
                *mut usize,
            ) -> i32 = nhg_monitors;
            let _: unsafe extern "C" fn(*mut NhgDisplay, u64, *mut u64, usize, *mut usize) -> i32 =
                nhg_frame_outputs;
        }

        let exprs: Vec<&str> = rows.iter().map(|(e, _)| *e).collect();
        let got = probe(decls, &exprs);
        assert_eq!(got.len(), rows.len());
        for ((expr, rust), c) in rows.iter().zip(got) {
            assert_eq!(*rust, c, "{expr}: Rust {rust}, C {c}");
        }
    }

    /// The title bar's part of the header (its own table, so the tasks' additions merge apart).
    #[test]
    fn title_bar_header_matches() {
        use nhglue::csd::*;
        use nhglue::ffi::{
            nhg_csd_layout, nhg_csd_paint, nhg_csd_style, nhg_csd_title_bar, nhg_frame_offsets,
        };
        type L = NhgCsdLayout;
        type P = NhgCsdPaint;
        let rows: &[(&str, i64)] = &[
            ("NHG_ACT_TITLE_BAR", NHG_ACT_TITLE_BAR.into()),
            ("NHG_CSD_CLOSE", NHG_CSD_CLOSE.into()),
            ("NHG_CSD_MAX", NHG_CSD_MAX.into()),
            ("NHG_CSD_MIN", NHG_CSD_MIN.into()),
            ("NHG_CSD_MENU", NHG_CSD_MENU.into()),
            ("NHG_CSD_MAX_BUTTONS", NHG_CSD_MAX_BUTTONS as i64),
            ("NHG_BTN_HOVER", NHG_BTN_HOVER.into()),
            ("NHG_BTN_PRESS", NHG_BTN_PRESS.into()),
            ("NHG_CSD_UNDECORATED", NHG_CSD_UNDECORATED.into()),
            ("NHG_TB_REDRAW", NHG_TB_REDRAW.into()),
            ("NHG_TB_POINTER", NHG_TB_POINTER.into()),
            ("NHG_TB_LEFT", NHG_TB_LEFT.into()),
            ("sizeof (L)", size_of::<L>() as i64),
            ("offsetof (L, t)", offset_of!(L, t) as i64),
            ("offsetof (L, strip)", offset_of!(L, strip) as i64),
            ("offsetof (L, on)", offset_of!(L, on) as i64),
            ("sizeof (P)", size_of::<P>() as i64),
            ("_Alignof (P)", align_of::<P>() as i64),
            ("offsetof (P, title)", offset_of!(P, title) as i64),
            ("offsetof (P, active)", offset_of!(P, active) as i64),
            ("offsetof (P, nbuttons)", offset_of!(P, nbuttons) as i64),
            ("offsetof (P, button)", offset_of!(P, button) as i64),
            (
                "sizeof (((P *) 0)->button[0])",
                size_of::<NhgCsdButton>() as i64,
            ),
            (
                "offsetof (P, button[1].state)",
                (offset_of!(P, button)
                    + size_of::<NhgCsdButton>()
                    + offset_of!(NhgCsdButton, state)) as i64,
            ),
            (
                "offsetof (P, button[1].r)",
                (offset_of!(P, button) + size_of::<NhgCsdButton>() + offset_of!(NhgCsdButton, r))
                    as i64,
            ),
        ];
        let decls = "\
#define PROTO(f, t) _Static_assert (__builtin_types_compatible_p (__typeof__ (f), t), #f)
typedef NhgCsdLayout L;
typedef NhgCsdPaint P;
PROTO (nhg_csd_style, int32_t (NhgDisplay *, const uint32_t *, size_t, const uint32_t *, size_t));
PROTO (nhg_csd_layout, int32_t (NhgDisplay *, uint64_t, uint32_t, uint32_t, NhgCsdLayout *));
PROTO (nhg_frame_offsets, int32_t (NhgDisplay *, uint64_t, int32_t *));
PROTO (nhg_csd_paint, int32_t (NhgDisplay *, uint64_t, int32_t, int32_t, uint64_t, NhgCsdPaint *));
PROTO (nhg_csd_title_bar, int32_t (NhgDisplay *, uint64_t, int32_t, int32_t, NhgCsdPaint *));";
        let _: unsafe extern "C" fn(*mut NhgDisplay, *const u32, usize, *const u32, usize) -> i32 =
            nhg_csd_style;
        let _: unsafe extern "C" fn(*mut NhgDisplay, u64, u32, u32, *mut L) -> i32 = nhg_csd_layout;
        let _: unsafe extern "C" fn(*mut NhgDisplay, u64, *mut i32) -> i32 = nhg_frame_offsets;
        let _: unsafe extern "C" fn(*mut NhgDisplay, u64, i32, i32, u64, *mut P) -> i32 =
            nhg_csd_paint;
        let _: unsafe extern "C" fn(*mut NhgDisplay, u64, i32, i32, *mut P) -> i32 =
            nhg_csd_title_bar;
        let exprs: Vec<&str> = rows.iter().map(|(e, _)| *e).collect();
        let got = probe(decls, &exprs);
        assert_eq!(got.len(), rows.len());
        for ((expr, rust), c) in rows.iter().zip(got) {
            assert_eq!(*rust, c, "{expr}: Rust {rust}, C {c}");
        }
    }

    /// Window operations and tips: the header's records, constants and prototypes against the Rust side.
    #[test]
    fn window_header_matches_records() {
        use nhglue::tip::NHG_DEL_TIP;
        use nhglue::window::*;
        type W = NhgWindowDesc;
        let rows: &[(&str, i64)] = &[
            ("NHG_ALPHA_DAMAGE", NHG_ALPHA_DAMAGE.into()),
            ("NHG_ALPHA_HOST", NHG_ALPHA_HOST.into()),
            ("NHG_FULLSCREEN_NONE", NHG_FULLSCREEN_NONE.into()),
            ("NHG_FULLSCREEN_WIDTH", NHG_FULLSCREEN_WIDTH.into()),
            ("NHG_FULLSCREEN_HEIGHT", NHG_FULLSCREEN_HEIGHT.into()),
            ("NHG_FULLSCREEN_BOTH", NHG_FULLSCREEN_BOTH.into()),
            ("NHG_FULLSCREEN_MAXIMIZED", NHG_FULLSCREEN_MAXIMIZED.into()),
            ("NHG_DEL_TIP", NHG_DEL_TIP.into()),
            ("sizeof (NhgAnchor)", size_of::<NhgAnchor>() as i64),
            (
                "offsetof (NhgAnchor, adjust)",
                offset_of!(NhgAnchor, adjust) as i64,
            ),
            ("sizeof (W)", size_of::<W>() as i64),
            ("_Alignof (W)", align_of::<W>() as i64),
            ("WOFF (title)", offset_of!(W, title) as i64),
            ("WOFF (title_len)", offset_of!(W, title_len) as i64),
            ("WOFF (app_id_len)", offset_of!(W, app_id_len) as i64),
            (
                "WOFF (session_name_len)",
                offset_of!(W, session_name_len) as i64,
            ),
            ("WOFF (flags)", offset_of!(W, flags) as i64),
            ("WOFF (kind)", offset_of!(W, kind) as i64),
            ("WOFF (parent)", offset_of!(W, parent) as i64),
            ("WOFF (y)", offset_of!(W, y) as i64),
            ("WOFF (anchor)", offset_of!(W, anchor) as i64),
            ("WOFF (role)", offset_of!(W, role) as i64),
            ("WOFF (facts)", offset_of!(W, facts) as i64),
            ("WOFF (native_h)", offset_of!(W, native_h) as i64),
            ("WOFF (scale120)", offset_of!(W, scale120) as i64),
        ];
        let decls = "\
#define PROTO(f, t) _Static_assert (__builtin_types_compatible_p (__typeof__ (f), t), #f)
typedef NhgWindowDesc W;
#define WOFF(m) offsetof (W, m)
PROTO (nhg_window_new, int32_t (NhgDisplay *, const NhgWindowDesc *, uint64_t *));
PROTO (nhg_window_title, int32_t (NhgDisplay *, uint64_t, const char *, size_t));
PROTO (nhg_window_undecorated, int32_t (NhgDisplay *, uint64_t, int32_t));
PROTO (nhg_window_alpha, int32_t (NhgDisplay *, uint64_t, double, double, uint32_t *));
PROTO (nhg_window_size, int32_t (NhgDisplay *, uint64_t, int32_t, int32_t));
PROTO (nhg_window_min_size, int32_t (NhgDisplay *, uint64_t, int32_t, int32_t));
PROTO (nhg_window_visible, int32_t (NhgDisplay *, uint64_t, int32_t));
PROTO (nhg_window_iconify, int32_t (NhgDisplay *, uint64_t));
PROTO (nhg_window_fullscreen, int32_t (NhgDisplay *, uint64_t, uint32_t));
PROTO (nhg_window_destroy, int32_t (NhgDisplay *, uint64_t));
PROTO (nhg_window_wait_configured, int32_t (NhgDisplay *, uint64_t, uint32_t));
PROTO (nhg_tip_show, int32_t (NhgDisplay *, uint64_t *, uint64_t, NhgRect, int32_t, int32_t, int32_t, int32_t));
PROTO (nhg_tip_hide, int32_t (NhgDisplay *, uint64_t));";
        {
            use nhglue::ffi::*;
            let _: unsafe extern "C" fn(*mut NhgDisplay, *const W, *mut u64) -> i32 =
                nhg_window_new;
            let _: unsafe extern "C" fn(*mut NhgDisplay, u64, *const c_char, usize) -> i32 =
                nhg_window_title;
            let _: unsafe extern "C" fn(*mut NhgDisplay, u64, i32) -> i32 = nhg_window_undecorated;
            let _: unsafe extern "C" fn(*mut NhgDisplay, u64, f64, f64, *mut u32) -> i32 =
                nhg_window_alpha;
            let _: unsafe extern "C" fn(*mut NhgDisplay, u64, i32, i32) -> i32 = nhg_window_size;
            let _: unsafe extern "C" fn(*mut NhgDisplay, u64, i32, i32) -> i32 =
                nhg_window_min_size;
            let _: unsafe extern "C" fn(*mut NhgDisplay, u64, i32) -> i32 = nhg_window_visible;
            let _: unsafe extern "C" fn(*mut NhgDisplay, u64) -> i32 = nhg_window_iconify;
            let _: unsafe extern "C" fn(*mut NhgDisplay, u64, u32) -> i32 = nhg_window_fullscreen;
            let _: unsafe extern "C" fn(*mut NhgDisplay, u64) -> i32 = nhg_window_destroy;
            let _: unsafe extern "C" fn(*mut NhgDisplay, u64, u32) -> i32 =
                nhg_window_wait_configured;
            let _: unsafe extern "C" fn(
                *mut NhgDisplay,
                *mut u64,
                u64,
                NhgRect,
                i32,
                i32,
                i32,
                i32,
            ) -> i32 = nhg_tip_show;
            let _: unsafe extern "C" fn(*mut NhgDisplay, u64) -> i32 = nhg_tip_hide;
        }
        let exprs: Vec<&str> = rows.iter().map(|(e, _)| *e).collect();
        let got = probe(decls, &exprs);
        assert_eq!(got.len(), rows.len());
        for ((expr, rust), c) in rows.iter().zip(got) {
            assert_eq!(*rust, c, "{expr}: Rust {rust}, C {c}");
        }
    }

    /// The shell's part of the header: its values, its capabilities and counters as C reads them, and the calls
    /// that reach it, against the shell's Rust side.
    #[test]
    fn shell_header_matches() {
        use nhshell::api::*;
        use nhshell::caps::NhShellCaps;
        use nhshell::event::*;
        type C = NhShellCaps;
        type P = NhShellPresenterStats;
        let rows: &[(&str, i64)] = &[
            ("NHG_MOD_SHIFT", NHSHELL_MOD_SHIFT.into()),
            ("NHG_MOD_CTRL", NHSHELL_MOD_CTRL.into()),
            ("NHG_MOD_ALT", NHSHELL_MOD_ALT.into()),
            ("NHG_MOD_SUPER", NHSHELL_MOD_SUPER.into()),
            ("NHG_MOD_HYPER", NHSHELL_MOD_HYPER.into()),
            ("NHG_MOD_META", NHSHELL_MOD_META.into()),
            ("NHG_MOD_CAPS", NHSHELL_MOD_CAPS.into()),
            ("NHG_STATE_MAXIMIZED", NHSHELL_STATE_MAXIMIZED.into()),
            ("NHG_STATE_FULLSCREEN", NHSHELL_STATE_FULLSCREEN.into()),
            ("NHG_STATE_ACTIVATED", NHSHELL_STATE_ACTIVATED.into()),
            ("NHG_STATE_RESIZING", NHSHELL_STATE_RESIZING.into()),
            ("NHG_STATE_TILED_L", NHSHELL_STATE_TILED_L.into()),
            ("NHG_STATE_TILED_R", NHSHELL_STATE_TILED_R.into()),
            ("NHG_STATE_TILED_T", NHSHELL_STATE_TILED_T.into()),
            ("NHG_STATE_TILED_B", NHSHELL_STATE_TILED_B.into()),
            ("NHG_STATE_SUSPENDED", NHSHELL_STATE_SUSPENDED.into()),
            ("NHG_BUTTON_RELEASED", NHSHELL_BUTTON_RELEASED.into()),
            ("NHG_BUTTON_PRESSED", NHSHELL_BUTTON_PRESSED.into()),
            ("NHG_WIN_UNDECORATED", NHSHELL_WIN_UNDECORATED.into()),
            ("NHG_WIN_FULLSCREEN", NHSHELL_WIN_FULLSCREEN.into()),
            ("NHG_WIN_MAXIMIZED", NHSHELL_WIN_MAXIMIZED.into()),
            ("NHG_WIN_GRAB", NHSHELL_WIN_GRAB.into()),
            ("NHG_WIN_RESTORE", NHSHELL_WIN_RESTORE.into()),
            ("NHG_EDGE_TOP", NHSHELL_EDGE_TOP.into()),
            ("NHG_EDGE_BOTTOM", NHSHELL_EDGE_BOTTOM.into()),
            ("NHG_EDGE_LEFT", NHSHELL_EDGE_LEFT.into()),
            ("NHG_EDGE_RIGHT", NHSHELL_EDGE_RIGHT.into()),
            ("NHG_CURSOR_DEFAULT", NHSHELL_CURSOR_DEFAULT.into()),
            ("NHG_CURSOR_TEXT", NHSHELL_CURSOR_TEXT.into()),
            ("NHG_CURSOR_POINTER", NHSHELL_CURSOR_POINTER.into()),
            ("NHG_CURSOR_CROSSHAIR", NHSHELL_CURSOR_CROSSHAIR.into()),
            ("NHG_CURSOR_WAIT", NHSHELL_CURSOR_WAIT.into()),
            ("NHG_CURSOR_HELP", NHSHELL_CURSOR_HELP.into()),
            ("NHG_CURSOR_MOVE", NHSHELL_CURSOR_MOVE.into()),
            ("NHG_CURSOR_NOT_ALLOWED", NHSHELL_CURSOR_NOT_ALLOWED.into()),
            ("NHG_CURSOR_RESIZE_NS", NHSHELL_CURSOR_RESIZE_NS.into()),
            ("NHG_CURSOR_RESIZE_EW", NHSHELL_CURSOR_RESIZE_EW.into()),
            ("NHG_CURSOR_RESIZE_NWSE", NHSHELL_CURSOR_RESIZE_NWSE.into()),
            ("NHG_CURSOR_RESIZE_NESW", NHSHELL_CURSOR_RESIZE_NESW.into()),
            ("NHG_CURSOR_HIDDEN", NHSHELL_CURSOR_HIDDEN.into()),
            ("NHG_KIND_TOPLEVEL", NHSHELL_KIND_TOPLEVEL.into()),
            ("NHG_KIND_CHILD", NHSHELL_KIND_CHILD.into()),
            ("NHG_KIND_POPUP", NHSHELL_KIND_POPUP.into()),
            ("NHG_ADJUST_SLIDE_X", NHSHELL_ADJUST_SLIDE_X.into()),
            ("NHG_ADJUST_SLIDE_Y", NHSHELL_ADJUST_SLIDE_Y.into()),
            ("NHG_ADJUST_FLIP_X", NHSHELL_ADJUST_FLIP_X.into()),
            ("NHG_ADJUST_FLIP_Y", NHSHELL_ADJUST_FLIP_Y.into()),
            ("NHG_SEL_CLIPBOARD", NHSHELL_SEL_CLIPBOARD.into()),
            ("NHG_SEL_PRIMARY", NHSHELL_SEL_PRIMARY.into()),
            // NhgCaps
            ("sizeof (C)", size_of::<C>() as i64),
            ("_Alignof (C)", align_of::<C>() as i64),
            ("COFF (compositor)", offset_of!(C, compositor) as i64),
            ("COFF (text_input_v3)", offset_of!(C, text_input_v3) as i64),
            ("COFF (activation)", offset_of!(C, activation) as i64),
            ("COFF (xdg_foreign)", offset_of!(C, xdg_foreign) as i64),
            ("COFF (ssd)", offset_of!(C, ssd) as i64),
            ("COFF (force_csd)", offset_of!(C, force_csd) as i64),
            // NhgPresenterCounters
            ("sizeof (P)", size_of::<P>() as i64),
            ("_Alignof (P)", align_of::<P>() as i64),
            ("POFF (packets)", offset_of!(P, packets) as i64),
            ("POFF (bare_commits)", offset_of!(P, bare_commits) as i64),
            ("POFF (front_trims)", offset_of!(P, front_trims) as i64),
            ("POFF (thread_on)", offset_of!(P, thread_on) as i64),
            (
                "offsetof (NhgPresenterStats, frames)",
                offset_of!(NhgPresenterStats, frames) as i64,
            ),
        ];
        let decls = "\
#define PROTO(f, t) _Static_assert (__builtin_types_compatible_p (__typeof__ (f), t), #f)
typedef NhgCaps C;
#define COFF(m) offsetof (C, m)
typedef NhgPresenterCounters P;
#define POFF(m) offsetof (P, m)
PROTO (nhg_dispatch_main, int32_t (NhgDisplay *));
PROTO (nhg_caps, int32_t (NhgDisplay *, NhgCaps *));
PROTO (nhg_window_move, int32_t (NhgDisplay *, uint64_t));
PROTO (nhg_window_resize, int32_t (NhgDisplay *, uint64_t, uint32_t));
PROTO (nhg_window_show_menu, int32_t (NhgDisplay *, uint64_t, int32_t, int32_t));
PROTO (nhg_window_set_cursor, int32_t (NhgDisplay *, uint64_t, uint32_t));
PROTO (nhg_window_activate, int32_t (NhgDisplay *, uint64_t, const char *));
PROTO (nhg_set_cursor_theme, int32_t (NhgDisplay *, const char *, uint32_t));";
        {
            use nhglue::ffi::*;
            let _: unsafe extern "C" fn(*mut NhgDisplay) -> i32 = nhg_dispatch_main;
            let _: unsafe extern "C" fn(*mut NhgDisplay, *mut C) -> i32 = nhg_caps;
            let _: unsafe extern "C" fn(*mut NhgDisplay, u64) -> i32 = nhg_window_move;
            let _: unsafe extern "C" fn(*mut NhgDisplay, u64, u32) -> i32 = nhg_window_resize;
            let _: unsafe extern "C" fn(*mut NhgDisplay, u64, i32, i32) -> i32 =
                nhg_window_show_menu;
            let _: unsafe extern "C" fn(*mut NhgDisplay, u64, u32) -> i32 = nhg_window_set_cursor;
            let _: unsafe extern "C" fn(*mut NhgDisplay, u64, *const c_char) -> i32 =
                nhg_window_activate;
            let _: unsafe extern "C" fn(*mut NhgDisplay, *const c_char, u32) -> i32 =
                nhg_set_cursor_theme;
        }
        let exprs: Vec<&str> = rows.iter().map(|(e, _)| *e).collect();
        let got = probe(decls, &exprs);
        assert_eq!(got.len(), rows.len());
        for ((expr, rust), c) in rows.iter().zip(got) {
            assert_eq!(*rust, c, "{expr}: Rust {rust}, C {c}");
        }
    }

    /// The child frames' part of the header (its own table, so the tasks' additions merge apart).
    #[test]
    fn child_header_matches() {
        use nhglue::child::*;
        use nhglue::ffi::{nhg_child_chrome, nhg_child_set, nhg_child_stack};
        type C = NhgChrome;
        let rows: &[(&str, i64)] = &[
            ("NHG_CHILD_VISIBLE", NHG_CHILD_VISIBLE.into()),
            ("NHG_CHILD_NO_FOCUS", NHG_CHILD_NO_FOCUS.into()),
            ("sizeof (C)", size_of::<C>() as i64),
            ("offsetof (C, radius)", offset_of!(C, radius) as i64),
            ("offsetof (C, oy)", offset_of!(C, oy) as i64),
            ("offsetof (C, opacity)", offset_of!(C, opacity) as i64),
            ("offsetof (C, rgb)", offset_of!(C, rgb) as i64),
            ("sizeof (NhgFrameGeom)", size_of::<NhgFrameGeom>() as i64),
            (
                "offsetof (NhgFrameGeom, window)",
                offset_of!(NhgFrameGeom, window) as i64,
            ),
        ];
        let decls = "\
#define PROTO(f, t) _Static_assert (__builtin_types_compatible_p (__typeof__ (f), t), #f)
typedef NhgChrome C;
PROTO (nhg_child_set, int32_t (NhgDisplay *, uint64_t, uint64_t, int32_t, int32_t, uint32_t));
PROTO (nhg_child_chrome, int32_t (NhgDisplay *, uint64_t, const NhgChrome *));
PROTO (nhg_child_stack, int32_t (NhgDisplay *, uint64_t, const uint64_t *, size_t, int32_t));";
        let _: unsafe extern "C" fn(*mut NhgDisplay, u64, u64, i32, i32, u32) -> i32 =
            nhg_child_set;
        let _: unsafe extern "C" fn(*mut NhgDisplay, u64, *const C) -> i32 = nhg_child_chrome;
        let _: unsafe extern "C" fn(*mut NhgDisplay, u64, *const u64, usize, i32) -> i32 =
            nhg_child_stack;
        let exprs: Vec<&str> = rows.iter().map(|(e, _)| *e).collect();
        let got = probe(decls, &exprs);
        assert_eq!(got.len(), rows.len());
        for ((expr, rust), c) in rows.iter().zip(got) {
            assert_eq!(*rust, c, "{expr}: Rust {rust}, C {c}");
        }
    }

    /// The menu popups' part of the header (its own table, so the tasks' additions merge apart).
    #[test]
    fn popup_header_matches() {
        use nhglue::ffi::{nhg_popup_hide, nhg_popup_place, nhg_popup_show, nhg_popup_size};
        use nhglue::popup::{NHG_DEL_POPUP, NHG_POPUP_GRAB};
        let rows: &[(&str, i64)] = &[
            ("NHG_POPUP_GRAB", NHG_POPUP_GRAB.into()),
            ("NHG_DEL_POPUP", NHG_DEL_POPUP.into()),
            ("NHG_FRAME_POPUP", NHG_FRAME_POPUP.into()),
        ];
        let decls = "\
#define PROTO(f, t) _Static_assert (__builtin_types_compatible_p (__typeof__ (f), t), #f)
PROTO (nhg_popup_show, int32_t (NhgDisplay *, uint64_t *, uint64_t, uint64_t, NhgRect, uint32_t, uint32_t, uint32_t, int32_t, int32_t));
PROTO (nhg_popup_place, int32_t (NhgDisplay *, uint64_t, NhgRect, uint32_t, uint32_t, uint32_t));
PROTO (nhg_popup_size, int32_t (NhgDisplay *, uint64_t, int32_t, int32_t));
PROTO (nhg_popup_hide, int32_t (NhgDisplay *, uint64_t, uint64_t));";
        let _: unsafe extern "C" fn(
            *mut NhgDisplay,
            *mut u64,
            u64,
            u64,
            NhgRect,
            u32,
            u32,
            u32,
            i32,
            i32,
        ) -> i32 = nhg_popup_show;
        let _: unsafe extern "C" fn(*mut NhgDisplay, u64, NhgRect, u32, u32, u32) -> i32 =
            nhg_popup_place;
        let _: unsafe extern "C" fn(*mut NhgDisplay, u64, i32, i32) -> i32 = nhg_popup_size;
        let _: unsafe extern "C" fn(*mut NhgDisplay, u64, u64) -> i32 = nhg_popup_hide;
        let exprs: Vec<&str> = rows.iter().map(|(e, _)| *e).collect();
        let got = probe(decls, &exprs);
        assert_eq!(got.len(), rows.len());
        for ((expr, rust), c) in rows.iter().zip(got) {
            assert_eq!(*rust, c, "{expr}: Rust {rust}, C {c}");
        }
    }

    /// The bell's part of the header (its own table).
    #[test]
    fn bell_header_matches() {
        use nhglue::bell::NhgBellGeom;
        use nhglue::ffi::{
            nhg_activation_token, nhg_bell_flash, nhg_bell_restore, nhg_inhibit_idle,
            nhg_window_export,
        };
        type B = NhgBellGeom;
        let rows: &[(&str, i64)] = &[
            ("sizeof (NhgBellGeom)", size_of::<B>() as i64),
            (
                "offsetof (NhgBellGeom, height)",
                offset_of!(B, height) as i64,
            ),
            (
                "offsetof (NhgBellGeom, line_h)",
                offset_of!(B, line_h) as i64,
            ),
            (
                "offsetof (NhgBellGeom, border)",
                offset_of!(B, border) as i64,
            ),
            (
                "offsetof (NhgBellGeom, top_margin)",
                offset_of!(B, top_margin) as i64,
            ),
            (
                "offsetof (NhgBellGeom, bottom_margin)",
                offset_of!(B, bottom_margin) as i64,
            ),
        ];
        let decls = "\
#define PROTO(f, t) _Static_assert (__builtin_types_compatible_p (__typeof__ (f), t), #f)
PROTO (nhg_bell_flash, int32_t (NhgDisplay *, uint64_t, const NhgBellGeom *));
PROTO (nhg_bell_restore, int32_t (NhgDisplay *, uint64_t));
PROTO (nhg_inhibit_idle, int32_t (NhgDisplay *, uint64_t, int32_t));
PROTO (nhg_window_export, int32_t (NhgDisplay *, uint64_t, uint32_t, char *, size_t, size_t *));
PROTO (nhg_activation_token, int32_t (NhgDisplay *, uint64_t, uint32_t, char *, size_t, size_t *));";
        let _: unsafe extern "C" fn(*mut NhgDisplay, u64, *const B) -> i32 = nhg_bell_flash;
        let _: unsafe extern "C" fn(*mut NhgDisplay, u64) -> i32 = nhg_bell_restore;
        let _: unsafe extern "C" fn(*mut NhgDisplay, u64, i32) -> i32 = nhg_inhibit_idle;
        let _: unsafe extern "C" fn(
            *mut NhgDisplay,
            u64,
            u32,
            *mut c_char,
            usize,
            *mut usize,
        ) -> i32 = nhg_window_export;
        let _: unsafe extern "C" fn(
            *mut NhgDisplay,
            u64,
            u32,
            *mut c_char,
            usize,
            *mut usize,
        ) -> i32 = nhg_activation_token;
        let exprs: Vec<&str> = rows.iter().map(|(e, _)| *e).collect();
        let got = probe(decls, &exprs);
        assert_eq!(got.len(), rows.len());
        for ((expr, rust), c) in rows.iter().zip(got) {
            assert_eq!(*rust, c, "{expr}: Rust {rust}, C {c}");
        }
    }

    /// The drop target's part of the header (its own table).
    #[test]
    fn dnd_header_matches() {
        use nhglue::ffi::{nhg_dnd_finish, nhg_dnd_register, nhg_dnd_status};
        let rows: &[(&str, i64)] = &[
            ("NHG_ACT_DND_ENTER", NHG_ACT_DND_ENTER.into()),
            ("NHG_ACT_DND_MOTION", NHG_ACT_DND_MOTION.into()),
            ("NHG_ACT_DND_LEAVE", NHG_ACT_DND_LEAVE.into()),
            ("NHG_ACT_DND_DROP", NHG_ACT_DND_DROP.into()),
            ("NHG_SEL_DND", nhshell::api::NHSHELL_SEL_DND.into()),
            ("NHG_DND_COPY", nhshell::api::NHSHELL_DND_COPY.into()),
            ("NHG_DND_MOVE", nhshell::api::NHSHELL_DND_MOVE.into()),
            ("NHG_DND_ASK", nhshell::api::NHSHELL_DND_ASK.into()),
        ];
        let decls = "\
#define PROTO(f, t) _Static_assert (__builtin_types_compatible_p (__typeof__ (f), t), #f)
PROTO (nhg_dnd_register, int32_t (NhgDisplay *, uint64_t, const char *const *, size_t));
PROTO (nhg_dnd_status, int32_t (NhgDisplay *, uint32_t, uint32_t));
PROTO (nhg_dnd_finish, int32_t (NhgDisplay *, int32_t, uint32_t));";
        let _: unsafe extern "C" fn(*mut NhgDisplay, u64, *const *const c_char, usize) -> i32 =
            nhg_dnd_register;
        let _: unsafe extern "C" fn(*mut NhgDisplay, u32, u32) -> i32 = nhg_dnd_status;
        let _: unsafe extern "C" fn(*mut NhgDisplay, i32, u32) -> i32 = nhg_dnd_finish;
        let exprs: Vec<&str> = rows.iter().map(|(e, _)| *e).collect();
        let got = probe(decls, &exprs);
        assert_eq!(got.len(), rows.len());
        for ((expr, rust), c) in rows.iter().zip(got) {
            assert_eq!(*rust, c, "{expr}: Rust {rust}, C {c}");
        }
    }

    #[test]
    fn session_header_matches() {
        use nhglue::ffi::{nhg_session_id, nhg_session_open, nhg_session_remove};
        use nhglue::session::*;
        let rows: &[(&str, i64)] = &[
            ("NHG_SESSION_LAUNCH", NHG_SESSION_LAUNCH.into()),
            ("NHG_SESSION_RECOVER", NHG_SESSION_RECOVER.into()),
            ("NHG_SESSION_RESTORE", NHG_SESSION_RESTORE.into()),
            ("NHG_ACT_SESSION", NHG_ACT_SESSION.into()),
            ("NHG_SESSION_CREATED", NHG_SESSION_CREATED.into()),
            ("NHG_SESSION_RESTORED", NHG_SESSION_RESTORED.into()),
            ("NHG_SESSION_REPLACED", NHG_SESSION_REPLACED.into()),
        ];
        let decls = "\
#define PROTO(f, t) _Static_assert (__builtin_types_compatible_p (__typeof__ (f), t), #f)
PROTO (nhg_session_open, int32_t (NhgDisplay *, const char *, size_t, uint32_t));
PROTO (nhg_session_id, int32_t (NhgDisplay *, char *, size_t, size_t *));
PROTO (nhg_session_remove, int32_t (NhgDisplay *, uint64_t));";
        let _: unsafe extern "C" fn(*mut NhgDisplay, *const c_char, usize, u32) -> i32 =
            nhg_session_open;
        let _: unsafe extern "C" fn(*mut NhgDisplay, *mut c_char, usize, *mut usize) -> i32 =
            nhg_session_id;
        let _: unsafe extern "C" fn(*mut NhgDisplay, u64) -> i32 = nhg_session_remove;
        let exprs: Vec<&str> = rows.iter().map(|(e, _)| *e).collect();
        let got = probe(decls, &exprs);
        assert_eq!(got.len(), rows.len());
        for ((expr, rust), c) in rows.iter().zip(got) {
            assert_eq!(*rust, c, "{expr}: Rust {rust}, C {c}");
        }
    }

    #[test]
    fn touch_header_matches() {
        use nhglue::touch::*;
        let rows: &[(&str, i64)] = &[
            ("NHG_ACT_TOUCH", NHG_ACT_TOUCH.into()),
            ("NHG_ACT_PINCH", NHG_ACT_PINCH.into()),
            ("NHG_TOUCH_BEGIN", NHG_TOUCH_BEGIN.into()),
            ("NHG_TOUCH_UPDATE", NHG_TOUCH_UPDATE.into()),
            ("NHG_TOUCH_END", NHG_TOUCH_END.into()),
            ("NHG_TOUCH_CANCEL", NHG_TOUCH_CANCEL.into()),
            ("sizeof (NhgTouchPoint)", size_of::<NhgTouchPoint>() as i64),
            (
                "offsetof (NhgTouchPoint, y)",
                offset_of!(NhgTouchPoint, y) as i64,
            ),
            (
                "offsetof (NhgTouchPoint, id)",
                offset_of!(NhgTouchPoint, id) as i64,
            ),
        ];
        let exprs: Vec<&str> = rows.iter().map(|(e, _)| *e).collect();
        let got = probe("", &exprs);
        assert_eq!(got.len(), rows.len());
        for ((expr, rust), c) in rows.iter().zip(got) {
            assert_eq!(*rust, c, "{expr}: Rust {rust}, C {c}");
        }
    }

    /// The drag source's part of the header (`x-begin-drag`).
    #[test]
    fn drag_header_matches() {
        use nhglue::dnd::{
            NHG_DRAG_ALLOW_CURRENT, NHG_DRAG_FRAME, NHG_DRAG_RETURN_FRAME, NHG_DRAG_RETURN_NOW,
        };
        use nhglue::dnd::{
            NHG_DRAG_PHASE_DONE, NHG_DRAG_PHASE_DROPPED, NHG_DRAG_PHASE_LIVE,
            NHG_DRAG_PHASE_WAITING,
        };
        use nhglue::ffi::{nhg_drag_cancel, nhg_drag_start, nhg_drag_state};
        let rows: &[(&str, i64)] = &[
            ("NHG_ACT_DRAG_ACTION", NHG_ACT_DRAG_ACTION.into()),
            ("NHG_ACT_DRAG_DROPPED", NHG_ACT_DRAG_DROPPED.into()),
            ("NHG_ACT_DRAG_DONE", NHG_ACT_DRAG_DONE.into()),
            ("NHG_DRAG_ALLOW_CURRENT", NHG_DRAG_ALLOW_CURRENT.into()),
            ("NHG_DRAG_RETURN_FRAME", NHG_DRAG_RETURN_FRAME.into()),
            ("NHG_DRAG_RETURN_NOW", NHG_DRAG_RETURN_NOW.into()),
            ("NHG_DRAG_FRAME", NHG_DRAG_FRAME.into()),
            ("NHG_DRAG_PHASE_WAITING", NHG_DRAG_PHASE_WAITING.into()),
            ("NHG_DRAG_PHASE_LIVE", NHG_DRAG_PHASE_LIVE.into()),
            ("NHG_DRAG_PHASE_DROPPED", NHG_DRAG_PHASE_DROPPED.into()),
            ("NHG_DRAG_PHASE_DONE", NHG_DRAG_PHASE_DONE.into()),
        ];
        let decls = "\
#define PROTO(f, t) _Static_assert (__builtin_types_compatible_p (__typeof__ (f), t), #f)
PROTO (nhg_drag_start, int32_t (NhgDisplay *, uint64_t, const char *const *, const uint8_t *const *, const size_t *, size_t, uint32_t, uint32_t, uint32_t));
PROTO (nhg_drag_cancel, int32_t (NhgDisplay *));
PROTO (nhg_drag_state, int32_t (NhgDisplay *, uint32_t *, uint64_t *));";
        let _: unsafe extern "C" fn(
            *mut NhgDisplay,
            u64,
            *const *const c_char,
            *const *const u8,
            *const usize,
            usize,
            u32,
            u32,
            u32,
        ) -> i32 = nhg_drag_start;
        let _: unsafe extern "C" fn(*mut NhgDisplay) -> i32 = nhg_drag_cancel;
        let _: unsafe extern "C" fn(*mut NhgDisplay, *mut u32, *mut u64) -> i32 = nhg_drag_state;
        let exprs: Vec<&str> = rows.iter().map(|(e, _)| *e).collect();
        let got = probe(decls, &exprs);
        assert_eq!(got.len(), rows.len());
        for ((expr, rust), c) in rows.iter().zip(got) {
            assert_eq!(*rust, c, "{expr}: Rust {rust}, C {c}");
        }
    }

    #[test]
    fn reconnect_header_matches() {
        use nhglue::ffi::{nhg_reconnect_settle, nhg_reconnect_step, nhg_set_reconnect};
        use nhglue::reconnect::*;
        let rows: &[(&str, i64)] = &[
            ("NHG_ACT_RECONNECTING", NHG_ACT_RECONNECTING.into()),
            ("NHG_ACT_RECONNECTED", NHG_ACT_RECONNECTED.into()),
            ("NHG_SETTLE_REPORT", NHG_SETTLE_REPORT.into()),
            ("NHG_SETTLE_MONITORS", NHG_SETTLE_MONITORS.into()),
            ("NHG_SETTLE_FOCUS_OUT", NHG_SETTLE_FOCUS_OUT.into()),
        ];
        let decls = "\
#define PROTO(f, t) _Static_assert (__builtin_types_compatible_p (__typeof__ (f), t), #f)
PROTO (nhg_set_reconnect, int32_t (NhgDisplay *, int32_t));
PROTO (nhg_reconnect_step, int32_t (NhgDisplay *, uint32_t, uint32_t *));
PROTO (nhg_reconnect_settle, int32_t (NhgDisplay *, uint64_t *, uint32_t *));";
        let _: unsafe extern "C" fn(*mut NhgDisplay, i32) -> i32 = nhg_set_reconnect;
        let _: unsafe extern "C" fn(*mut NhgDisplay, u32, *mut u32) -> i32 = nhg_reconnect_step;
        let _: unsafe extern "C" fn(*mut NhgDisplay, *mut u64, *mut u32) -> i32 =
            nhg_reconnect_settle;
        let exprs: Vec<&str> = rows.iter().map(|(e, _)| *e).collect();
        let got = probe(decls, &exprs);
        assert_eq!(got.len(), rows.len());
        for ((expr, rust), c) in rows.iter().zip(got) {
            assert_eq!(*rust, c, "{expr}: Rust {rust}, C {c}");
        }
    }

    #[test]
    fn give_up_header_matches() {
        use nhglue::ffi::{nhg_give_up, nhg_last_error};
        let decls = "\
#define PROTO(f, t) _Static_assert (__builtin_types_compatible_p (__typeof__ (f), t), #f)
PROTO (nhg_give_up, int32_t (NhgDisplay *));
PROTO (nhg_last_error, int32_t (NhgDisplay *, char *, size_t, size_t *));";
        let _: unsafe extern "C" fn(*mut NhgDisplay) -> i32 = nhg_give_up;
        let _: unsafe extern "C" fn(*mut NhgDisplay, *mut c_char, usize, *mut usize) -> i32 =
            nhg_last_error;
        let got = probe(decls, &["NHG_OK"]);
        assert_eq!(got, [i64::from(NHG_OK)]);
    }

    #[test]
    fn version_is_one_and_null_is_refused() {
        let mut v = 0u32;
        // SAFETY: `v` is a writable u32.
        assert_eq!(unsafe { nhg_abi_version(&mut v) }, NHG_OK);
        assert_eq!(v, 1);
        // SAFETY: null is allowed and reported.
        assert_eq!(unsafe { nhg_abi_version(std::ptr::null_mut()) }, NHG_E_ARG);
    }

    #[test]
    fn null_display_is_accepted() {
        let null = std::ptr::null_mut();
        // SAFETY: null is allowed for each.
        unsafe {
            assert_eq!(nhg_close(null), NHG_OK);
            assert_eq!(nhg_fd(null), NHG_E_ARG);
            assert_eq!(nhg_dispatch_main(null), NHG_E_ARG);
        }
    }

    /// A display that does not exist: null, with a reason that fits the buffer and is NUL-terminated.
    #[test]
    fn open_failure_gives_a_reason() {
        let name = c"nhglue-test-no-such-display";
        let mut err = [0x55 as c_char; 16];
        // SAFETY: the name is NUL-terminated and `err` holds 16 writable bytes.
        let d = unsafe { nhg_open(name.as_ptr(), err.as_mut_ptr(), err.len()) };
        assert!(d.is_null());
        let n = err
            .iter()
            .position(|&b| b == 0)
            .expect("no NUL in the reason");
        assert!(n > 0 && n < err.len(), "reason length {n}");
        // SAFETY: a null buffer with no room is accepted.
        let d = unsafe { nhg_open(name.as_ptr(), std::ptr::null_mut(), 0) };
        assert!(d.is_null());
    }

    /// The window layers' part of the header (its own table, so the tasks' additions merge apart).
    #[test]
    fn layers_header_matches() {
        use nhglue::ffi::{nhg_layers_count, nhg_layers_sync, nhg_scroll_window};
        use nhglue::layers::*;
        type L = NhgLayerWindow;
        let rows: &[(&str, i64)] = &[
            ("NHG_LAYERS_ON", NHG_LAYERS_ON.into()),
            (
                "NHG_LAYERS_INTEGER_SCALES",
                NHG_LAYERS_INTEGER_SCALES.into(),
            ),
            ("NHG_LAYERS_OPAQUE_BG", NHG_LAYERS_OPAQUE_BG.into()),
            ("sizeof (L)", size_of::<L>() as i64),
            ("_Alignof (L)", align_of::<L>() as i64),
            ("offsetof (L, seq)", offset_of!(L, seq) as i64),
            ("offsetof (L, rect)", offset_of!(L, rect) as i64),
        ];
        let decls = "\
#define PROTO(f, t) _Static_assert (__builtin_types_compatible_p (__typeof__ (f), t), #f)
typedef NhgLayerWindow L;
PROTO (nhg_scroll_window, int32_t (NhgDisplay *, uint64_t, uint64_t, uint64_t, NhgRect, NhgRect, int32_t, uint32_t));
PROTO (nhg_layers_sync, int32_t (NhgDisplay *, uint64_t, uint64_t, const NhgLayerWindow *, size_t, uint32_t));
PROTO (nhg_layers_count, int32_t (NhgDisplay *, uint64_t));";
        let _: unsafe extern "C" fn(
            *mut NhgDisplay,
            u64,
            u64,
            u64,
            NhgRect,
            NhgRect,
            i32,
            u32,
        ) -> i32 = nhg_scroll_window;
        let _: unsafe extern "C" fn(*mut NhgDisplay, u64, u64, *const L, usize, u32) -> i32 =
            nhg_layers_sync;
        let _: unsafe extern "C" fn(*mut NhgDisplay, u64) -> i32 = nhg_layers_count;
        let exprs: Vec<&str> = rows.iter().map(|(e, _)| *e).collect();
        let got = probe(decls, &exprs);
        assert_eq!(got.len(), rows.len());
        for ((expr, rust), c) in rows.iter().zip(got) {
            assert_eq!(*rust, c, "{expr}: Rust {rust}, C {c}");
        }
    }

    /// The entry points of what moved from C into the glue late (pure functions, stacking, geometry), in their own
    /// table.
    #[test]
    fn moved_header_matches() {
        use nhglue::ffi::{
            nhg_csd_mix, nhg_frame_monitors, nhg_monitor_origin, nhg_premultiply,
            nhg_selection_pick, nhg_snapshot_copy,
        };
        let decls = "\
#define PROTO(f, t) _Static_assert (__builtin_types_compatible_p (__typeof__ (f), t), #f)
PROTO (nhg_selection_pick, int32_t (NhgDisplay *, const char *, size_t, const char *const *, size_t, const char **));
PROTO (nhg_csd_mix, int32_t (uint32_t, uint32_t, uint32_t *, uint32_t *));
PROTO (nhg_premultiply, int32_t (uint32_t, double, uint32_t *));
PROTO (nhg_snapshot_copy, int32_t (NhgDisplay *, uint64_t, uint8_t *, int32_t, int32_t, int32_t));
PROTO (nhg_frame_monitors, int32_t (NhgDisplay *, uint64_t, uint32_t *));
PROTO (nhg_monitor_origin, int32_t (int32_t, int32_t, uint32_t, int32_t *, int32_t *));";
        let _: unsafe extern "C" fn(
            *mut NhgDisplay,
            *const c_char,
            usize,
            *const *const c_char,
            usize,
            *mut *const c_char,
        ) -> i32 = nhg_selection_pick;
        let _: unsafe extern "C" fn(u32, u32, *mut u32, *mut u32) -> i32 = nhg_csd_mix;
        let _: unsafe extern "C" fn(u32, f64, *mut u32) -> i32 = nhg_premultiply;
        let _: unsafe extern "C" fn(*mut NhgDisplay, u64, *mut u8, i32, i32, i32) -> i32 =
            nhg_snapshot_copy;
        let _: unsafe extern "C" fn(*mut NhgDisplay, u64, *mut u32) -> i32 = nhg_frame_monitors;
        let _: unsafe extern "C" fn(i32, i32, u32, *mut i32, *mut i32) -> i32 = nhg_monitor_origin;
        let got = probe(decls, &["NHG_OK"]);
        assert_eq!(got, [i64::from(NHG_OK)]);
    }
}

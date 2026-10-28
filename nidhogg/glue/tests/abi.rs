//! The C ABI: the header against the Rust side.

mod abi {
    use nhglue::abi::*;
    use nhglue::action::*;
    use nhglue::backing::{
        NHG_GEOM_FRESH, NHG_GEOM_LAYER_CHANGED, NhgBacking, NhgFrameGeom, NhgRect,
    };
    use nhglue::bench::NhgBenchOut;
    use nhglue::ffi::{NhgDisplay, nhg_abi_version, nhg_close, nhg_fd, nhg_open, nhg_shell};
    use nhglue::ffi::{
        nhg_backing, nhg_backing_forget, nhg_damage, nhg_damage_all, nhg_layer_ensure, nhg_scroll,
        nhg_snapshot,
    };
    use nhglue::ffi::{
        nhg_bell, nhg_bench_full_redraw, nhg_display_info, nhg_monitor_remove, nhg_monitor_set,
        nhg_pointer_visible, nhg_selection_disown, nhg_selection_owned, nhg_selection_read,
        nhg_selection_set, nhg_selection_types,
    };
    use nhglue::ffi::{nhg_frame_csd, nhg_frame_resized, nhg_frame_scale, nhg_next, nhg_raw_event};
    use nhglue::ffi::{nhg_frame_forget, nhg_frame_register, nhg_panic_message};
    use nhglue::ffi::{nhg_present, nhg_presenter_stats, nhg_presenter_thread};
    use nhglue::frame::*;
    use nhglue::keys::NHG_KEY_REPEAT;
    use nhglue::present::*;
    use nhglue::query::{NhgDisplayInfo, NhgMonitor};
    use nhshell::event::NhShellEvent;
    use nhshell::ffi::NhShell;
    use std::ffi::c_char;
    use std::mem::offset_of;
    use std::process::Command;

    /// Compile a probe that includes `nhglue.h` and prints each of `exprs` as a signed integer; return the values.
    /// `decls` go before `main` (they must compile against the header, with every warning an error).
    fn probe(decls: &str, exprs: &[&str]) -> Vec<i64> {
        let dir = std::env::temp_dir().join(format!("nhglue-layout-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut c = format!(
            "#include <stdio.h>\n#include <stddef.h>\n#include \"nhglue.h\"\n{decls}\nint main(void) {{\n"
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
            .args(["-I", &format!("{root}/../shell/include")])
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
            ("NHSHELL_E_PANIC", NHG_E_PANIC.into()),
            ("NHSHELL_E_ARG", NHG_E_ARG.into()),
            ("NHSHELL_E_STATE", NHG_E_STATE.into()),
            ("NHSHELL_E_UNSUPPORTED", NHG_E_UNSUPPORTED.into()),
            ("NHSHELL_E_DISCONNECTED", NHG_E_DISCONNECTED.into()),
            ("NHSHELL_E_NOMEM", NHG_E_NOMEM.into()),
            ("NHSHELL_E_TIMEOUT", NHG_E_TIMEOUT.into()),
            ("NHSHELL_E_TOO_SMALL", NHG_E_TOO_SMALL.into()),
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
            ("NHG_ACT_RAW", NHG_ACT_RAW.into()),
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
PROTO (nhg_raw_event, int32_t (NhgDisplay *, NhShellEvent *));
PROTO (nhg_abi_version, int32_t (uint32_t *));
PROTO (nhg_open, NhgDisplay *(const char *, char *, size_t));
PROTO (nhg_close, int32_t (NhgDisplay *));
PROTO (nhg_fd, int32_t (NhgDisplay *));
PROTO (nhg_shell, NhShell *(NhgDisplay *));
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
PROTO (nhg_keyboard_window, int32_t (NhgDisplay *, uint64_t *));
PROTO (nhg_selection_set, int32_t (NhgDisplay *, uint32_t, const char *const *, const uint8_t *const *, const size_t *, size_t));
PROTO (nhg_selection_disown, int32_t (NhgDisplay *, uint32_t));
PROTO (nhg_selection_owned, int32_t (NhgDisplay *, uint32_t));
PROTO (nhg_selection_types, int32_t (NhgDisplay *, uint32_t, const char **, size_t *));
PROTO (nhg_selection_read, int32_t (NhgDisplay *, uint32_t, const char *, uint32_t, const uint8_t **, size_t *));
PROTO (nhg_monitor_set, int32_t (NhgDisplay *, const NhgMonitor *));
PROTO (nhg_monitor_remove, int32_t (NhgDisplay *, uint64_t));
PROTO (nhg_display_info, int32_t (NhgDisplay *, NhgDisplayInfo *));
PROTO (nhg_bench_full_redraw, int32_t (NhgDisplay *, const NhgSurface *, size_t, uint32_t, NhgBenchOut *));
PROTO (nhg_bell, int32_t (NhgDisplay *, uint64_t));
PROTO (nhg_pointer_visible, int32_t (NhgDisplay *, uint64_t, int32_t, uint32_t));";
        // the same table, Rust side
        let _: unsafe extern "C" fn(*mut u32) -> i32 = nhg_abi_version;
        let _: unsafe extern "C" fn(*const c_char, *mut c_char, usize) -> *mut NhgDisplay =
            nhg_open;
        let _: unsafe extern "C" fn(*mut NhgDisplay) -> i32 = nhg_close;
        let _: unsafe extern "C" fn(*mut NhgDisplay) -> i32 = nhg_fd;
        let _: unsafe extern "C" fn(*mut NhgDisplay) -> *mut NhShell = nhg_shell;
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
        let _: unsafe extern "C" fn(*mut NhgDisplay, *mut NhShellEvent) -> i32 = nhg_raw_event;
        #[cfg(feature = "test-hooks")]
        {
            use nhglue::ffi::{nhg_debug_check_frame, nhg_test_panic};
            let _: unsafe extern "C" fn(*mut NhgDisplay, u64, *const NhgFrameFacts) -> i32 =
                nhg_debug_check_frame;
            let _: unsafe extern "C" fn(*mut NhgDisplay) -> i32 = nhg_test_panic;
        }
        // the keyboard
        {
            use nhglue::ffi::nhg_keyboard_window;
            let _: unsafe extern "C" fn(*mut NhgDisplay, *mut u64) -> i32 = nhg_keyboard_window;
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
        let _: unsafe extern "C" fn(*mut NhgDisplay, *const NhgMonitor) -> i32 = nhg_monitor_set;
        let _: unsafe extern "C" fn(*mut NhgDisplay, u64) -> i32 = nhg_monitor_remove;
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
            assert!(nhg_shell(null).is_null());
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
}

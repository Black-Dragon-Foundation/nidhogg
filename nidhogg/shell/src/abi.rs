//! The ABI version, and the layout test that pins the C header to the Rust records.

use crate::api::{NHSHELL_E_ARG, NHSHELL_E_PANIC, NHSHELL_OK};
use std::panic::catch_unwind;

/// The ABI version the header describes.
const ABI_VERSION: u32 = 1;

/// Write the ABI version to `out`.
///
/// # Safety
/// `out` must be null or point to a writable `u32`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn nhshell_abi_version(out: *mut u32) -> i32 {
    catch_unwind(|| {
        if out.is_null() {
            return NHSHELL_E_ARG;
        }
        // SAFETY: non-null, and the caller promises it is writable.
        unsafe { out.write(ABI_VERSION) };
        NHSHELL_OK
    })
    .unwrap_or(NHSHELL_E_PANIC)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::NhShellBuffer;
    use crate::caps::NhShellCaps;
    use crate::event::*;
    use crate::ffi::NhShellWindowDesc;
    use std::mem::{align_of, offset_of, size_of};
    use std::process::Command;

    #[test]
    fn version_is_one() {
        let mut v = 0u32;
        // SAFETY: `v` is a writable u32.
        assert_eq!(unsafe { nhshell_abi_version(&mut v) }, NHSHELL_OK);
        assert_eq!(v, 1);
        // SAFETY: null is allowed and reported.
        assert_eq!(
            unsafe { nhshell_abi_version(std::ptr::null_mut()) },
            NHSHELL_E_ARG
        );
    }

    /// Sizes, alignments and offsets of the records equal what a C compiler makes of `nhshell.h`.
    #[test]
    fn event_layout() {
        type E = NhShellEvent;
        // (C expression, Rust value); `S(x)` is sizeof, `A(x)` alignof, `O(x)` offsetof.
        let mut rows: Vec<(String, usize)> = Vec::new();
        let mut ty = |c: &str, size: usize, align: usize| {
            rows.push((format!("sizeof({c})"), size));
            rows.push((format!("_Alignof({c})"), align));
        };
        ty("NhShellEvent", size_of::<E>(), align_of::<E>());
        ty(
            "NhShellCaps",
            size_of::<NhShellCaps>(),
            align_of::<NhShellCaps>(),
        );
        ty(
            "NhShellBuffer",
            size_of::<NhShellBuffer>(),
            align_of::<NhShellBuffer>(),
        );
        ty(
            "NhShellWindowDesc",
            size_of::<NhShellWindowDesc>(),
            align_of::<NhShellWindowDesc>(),
        );
        ty(
            "__typeof__(((NhShellEvent *)0)->u)",
            size_of::<NhShellEventData>(),
            align_of::<NhShellEventData>(),
        );
        ty(
            "__typeof__(((NhShellEvent *)0)->u.key)",
            size_of::<KeyEv>(),
            align_of::<KeyEv>(),
        );
        ty(
            "__typeof__(((NhShellEvent *)0)->u.pointer)",
            size_of::<PointerEv>(),
            align_of::<PointerEv>(),
        );
        ty(
            "__typeof__(((NhShellEvent *)0)->u.scroll)",
            size_of::<ScrollEv>(),
            align_of::<ScrollEv>(),
        );
        ty(
            "__typeof__(((NhShellEvent *)0)->u.configure)",
            size_of::<ConfigureEv>(),
            align_of::<ConfigureEv>(),
        );
        ty(
            "__typeof__(((NhShellEvent *)0)->u.scale)",
            size_of::<ScaleEv>(),
            align_of::<ScaleEv>(),
        );
        ty(
            "__typeof__(((NhShellEvent *)0)->u.focus)",
            size_of::<FocusEv>(),
            align_of::<FocusEv>(),
        );
        ty(
            "__typeof__(((NhShellEvent *)0)->u.frame_done)",
            size_of::<FrameDoneEv>(),
            align_of::<FrameDoneEv>(),
        );
        ty(
            "__typeof__(((NhShellEvent *)0)->u.output)",
            size_of::<OutputEv>(),
            align_of::<OutputEv>(),
        );
        ty(
            "__typeof__(((NhShellEvent *)0)->u.selection)",
            size_of::<SelectionEv>(),
            align_of::<SelectionEv>(),
        );
        ty(
            "__typeof__(((NhShellEvent *)0)->u.text)",
            size_of::<TextEv>(),
            align_of::<TextEv>(),
        );
        ty(
            "__typeof__(((NhShellEvent *)0)->u.touch)",
            size_of::<TouchEv>(),
            align_of::<TouchEv>(),
        );
        ty(
            "__typeof__(((NhShellEvent *)0)->u.pinch)",
            size_of::<PinchEv>(),
            align_of::<PinchEv>(),
        );
        let mut off = |c: &str, o: usize| rows.push((format!("offsetof(NhShellEvent, {c})"), o));
        off("kind", offset_of!(E, kind));
        off("window", offset_of!(E, window));
        off("time_ns", offset_of!(E, time_ns));
        off("u", offset_of!(E, u));
        macro_rules! members {
            ($m:ident, $t:ty, $($f:ident),*) => {
                $( off(concat!("u.", stringify!($m), ".", stringify!($f)), offset_of!(E, u) + offset_of!($t, $f)); )*
            };
        }
        members!(key, KeyEv, keycode, keysym, mods, pressed, repeat, utf8);
        members!(pointer, PointerEv, x, y, button, state);
        members!(scroll, ScrollEv, dx, dy, v120x, v120y, source, stop);
        members!(configure, ConfigureEv, w, h, states, serial);
        members!(frame_done, FrameDoneEv, presented_ns, refresh_ns);
        members!(output, OutputEv, id, x, y, w, h, mm_w, mm_h, scale120, name);
        members!(
            text,
            TextEv,
            id,
            len,
            cursor_begin,
            cursor_end,
            before,
            after
        );
        members!(touch, TouchEv, id, op, x, y);
        members!(
            pinch, PinchEv, phase, fingers, dx, dy, scale, rotation, x, y, cancelled
        );
        off(
            "u.scale.scale120",
            offset_of!(E, u) + offset_of!(ScaleEv, scale120),
        );
        off("u.focus.in", offset_of!(E, u) + offset_of!(FocusEv, r#in));
        off(
            "u.selection.kind",
            offset_of!(E, u) + offset_of!(SelectionEv, kind),
        );

        let dir = std::env::temp_dir().join(format!("nhshell-layout-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut c = String::from(
            "#include <stdio.h>\n#include <stddef.h>\n#include \"nhshell.h\"\nint main(void) {\n",
        );
        for (expr, _) in &rows {
            c += &format!("  printf(\"%zu\\n\", (size_t) {expr});\n");
        }
        c += "  return 0;\n}\n";
        let src = dir.join("probe.c");
        let exe = dir.join("probe");
        std::fs::write(&src, c).unwrap();
        let inc = concat!(env!("CARGO_MANIFEST_DIR"), "/include");
        let gl = concat!(env!("CARGO_MANIFEST_DIR"), "/../glitnir/include");
        let cc = Command::new("cc")
            .args(["-std=gnu11", "-Wall", "-Werror", "-I", inc, "-I", gl, "-o"])
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
        let got: Vec<usize> = String::from_utf8_lossy(&out.stdout)
            .lines()
            .map(|l| l.parse().unwrap())
            .collect();
        assert_eq!(got.len(), rows.len());
        for ((expr, rust), c) in rows.iter().zip(got) {
            assert_eq!(*rust, c, "{expr}: Rust {rust}, C {c}");
        }
    }
}

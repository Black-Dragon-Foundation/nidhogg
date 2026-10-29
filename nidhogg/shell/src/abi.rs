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
    use crate::api::{NhShellBuffer, NhShellPresenterStats};
    use crate::caps::NhShellCaps;
    use crate::ffi::{NhShellSurfaceFrame, NhShellWindowDesc};
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
    fn record_layout() {
        // (C expression, Rust value)
        let mut rows: Vec<(String, usize)> = Vec::new();
        let mut ty = |c: &str, size: usize, align: usize| {
            rows.push((format!("sizeof({c})"), size));
            rows.push((format!("_Alignof({c})"), align));
        };
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
        // the publishing records
        type F = NhShellSurfaceFrame;
        type P = NhShellPresenterStats;
        for (c, size, align) in [
            ("NhShellSurfaceFrame", size_of::<F>(), align_of::<F>()),
            ("NhShellPresenterStats", size_of::<P>(), align_of::<P>()),
        ] {
            rows.push((format!("sizeof({c})"), size));
            rows.push((format!("_Alignof({c})"), align));
        }
        for (c, o) in [
            ("NhShellSurfaceFrame, pixels", offset_of!(F, pixels)),
            ("NhShellSurfaceFrame, src_y", offset_of!(F, src_y)),
            ("NhShellSurfaceFrame, alpha_cut", offset_of!(F, alpha_cut)),
            (
                "NhShellPresenterStats, front_trims",
                offset_of!(P, front_trims),
            ),
            ("NhShellPresenterStats, thread_on", offset_of!(P, thread_on)),
        ] {
            rows.push((format!("offsetof({c})"), o));
        }

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

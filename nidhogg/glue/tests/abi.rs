//! The C ABI: the header against the Rust side.

mod abi {
    use nhglue::abi::*;
    use nhglue::ffi::{NhgDisplay, nhg_abi_version, nhg_close, nhg_fd, nhg_open, nhg_shell};
    use nhshell::ffi::NhShell;
    use std::ffi::c_char;
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
        ];
        // the prototypes, C side (checked at compile time: the probe references no function, so it links alone)
        let decls = "\
#define PROTO(f, t) _Static_assert (__builtin_types_compatible_p (__typeof__ (f), t), #f)
PROTO (nhg_abi_version, int32_t (uint32_t *));
PROTO (nhg_open, NhgDisplay *(const char *, char *, size_t));
PROTO (nhg_close, int32_t (NhgDisplay *));
PROTO (nhg_fd, int32_t (NhgDisplay *));
PROTO (nhg_shell, NhShell *(NhgDisplay *));";
        // the same table, Rust side
        let _: unsafe extern "C" fn(*mut u32) -> i32 = nhg_abi_version;
        let _: unsafe extern "C" fn(*const c_char, *mut c_char, usize) -> *mut NhgDisplay =
            nhg_open;
        let _: unsafe extern "C" fn(*mut NhgDisplay) -> i32 = nhg_close;
        let _: unsafe extern "C" fn(*mut NhgDisplay) -> i32 = nhg_fd;
        let _: unsafe extern "C" fn(*mut NhgDisplay) -> *mut NhShell = nhg_shell;

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

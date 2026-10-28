# nhglue

The glue between Emacs and Nidhogg's native shell, in Rust: the shell's state and every decision that needs no live
Emacs state. The C side (`src/nhwl*.c`) keeps what touches Emacs (Lisp objects, Emacs structs, the keyboard queue,
the hook tables, `DEFUN`s) and all Cairo drawing, and calls in here through `include/nhglue.h`.

- **One library.** Under `--with-nidhogg-wayland` configure links `libnhglue.a` instead of `libglitnir.a`. It is the
  one Rust static library in the binary (two would collide on std's symbols), so `src/lib.rs` re-exports Glitnir's
  and the shell's C ABIs: `nidhogg-glitnir.c` and the C calls not yet ported keep linking. `nhshell` is an rlib.
- **Calls go one way.** C calls `nhg_*`; the glue answers with plain records (ids, rectangles, ARGB colours, UTF-8
  text, pixel pointers into Rust-owned backings) and never calls Emacs. It never sees a `Lisp_Object` or an Emacs
  struct, and C passes it no function pointers.
- **Threads.** The glue runs on the Lisp thread that holds Emacs's global lock, never on the shell's reader or the
  presenter thread.
- **Dependencies:** `nhshell`, `glitnir` and `libc`, built offline and locked from the shell's `vendor/`
  (`.cargo/config.toml`). `Cargo.lock` is the shell's plus the `nhglue` package; after the shell revendors, copy its
  lock here and add the package back (`check.sh` fails until they agree).
- **Features:** `wayland` (default) turns on the shell's Wayland backend; `test-hooks` adds entry points that exist
  only for tests. Configure passes `--features test-hooks` (through `GLITNIR_CARGO_FLAGS`) and turns on Rust's debug
  assertions (`CARGO_PROFILE_RELEASE_DEBUG_ASSERTIONS`) under `--enable-checking`.

## Checks

- `check.sh [--container DISTRO] [--test NAME]`: the lock against the shell's, fmt, clippy `-D warnings` (also with
  `test-hooks`), tests and cargo-deny. `--full` adds the release archive and checks that it exports every function of
  `nhglue.h`, `nhshell.h` and `glitnir.h`. `--android` runs `cargo check --no-default-features --target
  aarch64-linux-android` (no NDK, no link) and exits 3 when the toolchain has no std for that target.
- `tests/abi.rs` (`abi::header_matches_records`) compiles `nhglue.h` with `cc` and compares its constants and
  prototypes with the Rust side.
- `nm-check.sh BUILD_DIR`, on a `--nidhogg-wayland` build: the archive's undefined symbols minus the ones it defines
  itself must not include any symbol Emacs's own `src/*.o` define (Rust never calls Emacs), and every `nhg_*` call in
  `src/nhwl*.c` goes through the `NHG (…)` macro. Calls through function pointers are invisible to it; there are none.

## Debugging

The release profile keeps full debug info (`debug = 2`), so gdb shows the glue's locals as well as the C side's.
`rust-gdb` is gdb with Rust's pretty-printers loaded; point it at the dumped binary:

```sh
rust-gdb --args src/emacs -Q            # in the build tree; or: rust-gdb -p PID
(gdb) break rust_panic                  # stops at any panic, before the boundary catches it
(gdb) set print pretty on
```

Without `rust-gdb`, load the printers by hand: `gdb -iex "add-auto-load-safe-path $(rustc --print sysroot)/lib/rustlib/etc"
-x $(rustc --print sysroot)/lib/rustlib/etc/gdb_load_rust_pretty_printers.py`. Emacs's own `src/.gdbinit` still
applies to the C frames.

A panic inside the glue is caught at the entry point and reported as `NHG_E_PANIC`; the display it happened on is
dropped and Emacs keeps running. `(nhwl-glue-panics)` returns the first panic's message, location and backtrace.

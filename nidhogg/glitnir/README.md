# glitnir

Glitnir (the compositor) is a small Rust library with a C ABI (`include/glitnir.h`). It composes layers, shadows
and rounded corners into a buffer it is handed, and tracks the damage. Its API takes ids, rectangles, pixels and
parameters only, so it knows nothing of the program that embeds it.

## Crate rules

- Standard library only. A third-party crate needs a reason, and every one is vendored (`vendor/`), locked
  (`Cargo.lock`), licence-checked (`deny.toml`) and listed here with that reason. None today.
- Builds are offline and locked: `cargo build --release --offline --locked`, with `CARGO_HOME` inside the build
  directory, never `~/.cargo`.
- `panic = "unwind"`: every `extern "C"` body runs inside `ffi_guard` and returns `GLITNIR_E_PANIC` (-1) on a panic.
- Every exported symbol is listed in `ffi::EXPORTS` and declared in `include/glitnir.h`; a test checks that the two
  agree.

## Checks

`check.sh [--container DISTRO] [--test NAME]` runs `cargo fmt --check`, `cargo clippy -D warnings`, `cargo test`,
`cargo deny check licenses bans sources` (when `cargo-deny` is installed) and a vendor-drift check (it vendors a copy
of the crate and compares; the source tree is never written). Without `--container` it uses the host's cargo; with
`--container fedora-44` it runs in a podman container capped at 2 GiB.

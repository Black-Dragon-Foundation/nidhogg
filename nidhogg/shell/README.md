# nhshell

Nidhogg's native shell layer: a Rust crate with a C ABI (`include/nhshell.h`, from Task 2 on), built as one static
library, `libnhshell.a`, that also carries every `glitnir_*` symbol. Under `--with-nidhogg-wayland` the Makefile links
it instead of `libglitnir.a`; two Rust static libraries in one binary would collide on std's symbols.

- Dependencies are vendored in `vendor/` (`cargo vendor-filterer`, Linux and macOS, x86_64 and aarch64) and the build
  runs offline and locked (`.cargo/config.toml`). Third-party crates are limited to wayland-client, wayland-backend,
  wayland-protocols, xkbcommon, rustix and their dependencies.
- `check.sh [--container DISTRO] [--test NAME]` runs fmt, clippy `-D warnings`, tests, cargo-deny, the vendor-drift
  check and an `nm` check that every Glitnir export and `nhshell_abi_version` are in the archive.
  `check.sh --container DISTRO --revendor` regenerates `Cargo.lock` and `vendor/` (needs the network).

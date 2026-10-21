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

## Compose kernels and shadows

Pixels are premultiplied ARGB32 (Cairo's native-endian `0xAARRGGBB`), read and written in place through strided
views (`kernels::{Pixels, PixelsMut, Mask}`).

- `kernels::kernels()` returns the machine's kernels, chosen once: on x86-64 the AVX2 build where the CPU has it,
  else the baseline; on aarch64 the baseline build is already NEON. Both builds compile the same row bodies.
  `copy`, `over_const` (a layer's opacity), `over` (a translucent source), `mask_over` (source × coverage ×
  opacity) and `solid_over` (a colour through coverage) use pixman's exact x·a/255 arithmetic: OVER equals
  `pixman_image_composite32` bit for bit for premultiplied input.
- `corner_coverage(r)` is the 2r×2r coverage of the disc around the corner arcs' centre; its quadrants are a
  rectangle's four r×r corner squares in place. Pixels on the arc take 16×16 samples (within 1/255 of 16×16
  supersampling); the rest are exact. Cached per radius.
- `ring` redraws the host's square border along the rounded path, in the corner squares only, so straight edges keep
  the host's pixels. `ring_coverage(r, b)` gives the ring mask and the `inner` mask a bordered layer's corners use.
- `shadow::ShadowCache` holds nine-slice shadow tiles (one corner tile mirrored four ways, two 1-D edge profiles and
  the middle value) of the Gaussian-blurred rounded rectangle in closed form (Evan Wallace's method, 8 samples), keyed
  by size class, radius, blur, spread, colour and scale, the 64 most recently used. σ = blur / 2; the shadow reaches
  3σ past its box. Tiles hold one sample every 2 px from σ 8 and every 4 px from σ 16, sampled bilinearly: the
  coarsest that keeps the error within 2/255 of an 8×8-supersampled rounded rectangle convolved with an exact
  Gaussian.

The `pixman-oracle` feature (Linux; `build.rs` links the system's `pixman-1`) adds the test that compares the
kernels with pixman; `check.sh` runs it on Linux.

## Checks

`check.sh [--container DISTRO] [--test NAME]` runs `cargo fmt --check`, `cargo clippy -D warnings`, `cargo test`,
`cargo deny check licenses bans sources` (when `cargo-deny` is installed) and a vendor-drift check (it vendors a copy
of the crate and compares; the source tree is never written). Without `--container` it uses the host's cargo; with
`--container fedora-44` it runs in a podman container capped at 2 GiB.

## Damage

`damage.rs` keeps each frame's pending damage (device pixels; at most 32 rectangles, `region.rs`):
`glitnir_damage_add_n` adds rectangles, `glitnir_damage_all` the whole frame, `glitnir_scroll` a scroll's area and the
strip it left stale. A rectangle the damage already contains adds nothing, and a new one drops those it contains.
`glitnir_damage_pending` takes the damage (the frame then has none); the rectangles stay valid until that frame's next
take. Whole-frame damage reads back as `{0, 0, INT32_MAX, INT32_MAX}`, which the host clips to the frame.

`glitnir_commit` runs at the host's consistency points (no drawing in progress on a root or its children): it takes
the root's pending damage, into which the host has first moved its child frames' damage (in root coordinates), and
hands it back for invalidation; the rectangles stay valid until the root's next commit, apart from any debug take.

The frame-time record's `backfill_rate` is -1 when no entry knows whether its paint back-filled (`backfill` 2).

## Limits

A panic inside the library is caught at the boundary (`GLITNIR_E_PANIC`), and so is any other error a call returns:
the glue then thaws every frozen root, finishes a running draw with the stock picture, restores pgtk's whole-widget
invalidations, redraws every frame and switches Glitnir off for the session (no further `glitnir_*` call;
`glitnir-active-p` and `glitnir-stats` are nil, `glitnir-snapshot` still works). It shows one message.
`glitnir--debug-panic` (only with `glitnir-debug` set; WHERE `commit` or `draw`) makes the next such call panic, and
`revive` turns Glitnir back on for the tests that follow.

Three failures the fallback cannot catch still abort the process: an allocation failure, a second panic while one is
being handled, and a stack overflow. To keep the first rare, the frame-time rings are preallocated and the caches
bounded.

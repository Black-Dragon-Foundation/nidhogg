# glitnir

Glitnir (the compositor) is a small Rust library with a C ABI (`include/glitnir.h`). It composes layers, shadows
and rounded corners into a buffer it is handed, and tracks the damage. Its API takes ids, rectangles, pixels and
parameters only, so it knows nothing of the program that embeds it.

## Crate rules

- Standard library only. A third-party crate needs a reason, and every one is vendored (`vendor/`), locked
  (`Cargo.lock`), licence-checked (`deny.toml`) and listed here with that reason:
  - `png =0.18.1` (feature `images`, no default features): PNG decoding in safe Rust, so untrusted PNGs never reach
    libpng. It brings `fdeflate`, `flate2` (with `miniz_oxide`), `crc32fast`, `bitflags`, `adler2`, `simd-adler32`
    and `cfg-if`; none starts a thread. `zlib-rs` is locked and vendored as an optional dependency of `flate2` but
    not built.
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

## Compose

`glitnir_compose` draws a root's committed scene into a target buffer the host hands it (its pixels and their place
in the root's device pixels), within the given rectangles (the damage, or a draw's clip), from the frames' surfaces
(sources, by layer id), bottom to top: layers without chrome are copied; a chromed layer's shadow tiles go over what
lies below, then the layer is copied but for its four corner squares, which go through the corner mask, with the
border ring when it has a border. Window layers are their frame's pixels (P1 composes a scroll as damage). Pixels no
layer covers are left alone. A pixel's value never depends on how the rectangles cut it, so composing the damage
equals composing everything. A source whose device scale is not the target's is refused (`GLITNIR_E_STATE`) and the
host draws that root the stock way. The time taken and the layers drawn fill the root's next frame-time entry.

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

## Image decoders

Feature `images` (on by default): `glitnir_img_decode (format, data, len, params, out)` decodes encoded bytes into
`width * height` words `0x00RRGGBB` and an optional mask (255 draw, 0 transparent), both from the C library's
`calloc`, so the host frees them with `free` or `glitnir_img_free`. The ABI is stateless: no lock, no global state,
no thread, no callback, so the host calls it on any thread (its image worker) and outside `ffi_guard`; a panic is
caught at the boundary and returns `GLITNIR_E_PANIC`, freeing whatever the decode had allocated.

Every allocation is bounded: the header's width and height are checked against the caller's limits and
`MAX_PIXELS` (2^27) before the result is allocated, rows are decoded one at a time (no whole raw image), and the
`png` crate's own limit (64 MiB for its buffers and chunks) stays at its default.

PNG (`images/png.rs`) gives libpng's pixels for the host's transformations: 16-bit samples keep their high byte,
palettes and low-depth gray expand (gray scaled ×255, ×85, ×17), gray becomes RGB; a `tRNS` key or a palette whose
alphas are all 0 or 255 makes a mask, any other alpha is composited over the caller's background with
`png_composite`'s rounding (`images/composite.rs`, checked against every 8-bit case) at the file's depth, before
the 16-bit strip. `tRNS` and `bKGD` are read as libpng reads them (lengths, order, CRC). libpng composites in linear
light when the file carries `gAMA`, `sRGB`, `iCCP` or `cICP`; such an image with alpha to composite returns
`GLITNIR_IMG_E_HOST`, and the host decodes it itself. Error texts are the crate's.

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

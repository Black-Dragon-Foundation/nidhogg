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
  - `gif =0.14.2` (feature `images`, no default features but `std`, without which it is empty): GIF's LZW rasters
    in safe Rust, instead of giflib. It brings `weezl`; no thread.
  - `image-webp =0.2.4` (feature `images`, no default features): VP8, VP8L and ALPH decoding in safe Rust, instead
    of libwebp. It brings `byteorder-lite` and `quick-error`; no thread.
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
- `decor_corners` (C: `glitnir_decor_corners`) draws a decorated box's rounded corners (PD) from one premultiplied
  tile per (radius, border, colours) built from those masks (`decor_tile`, the 32 most recently used): OVER the
  pixels for a box just drawn, replacing the corner squares for the repaint of a run's end. A Cairo fill along a
  curve costs several µs per call; the tile is a copy or one OVER pass.
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

**Threads.** The scenes, the damage and the sprite cache belong to the host's main thread, behind one lock; a thread
that calls `glitnir::mark_compose_thread` and then touches them panics in a debug build. A commit through the Rust
API (`damage::commit_snapshot`) also returns the committed scene as an immutable `Arc<Committed>`: the layers in paint
order, their labels and the sprites those labels were placed from (the sprite cache is copied on write, so a later
`glitnir_decor_sprite_put` leaves a snapshot alone). `compose::compose_committed` composes a snapshot with a
caller-owned `Composer` (shadow tiles and per-root tallies) and takes no lock, so another thread can compose while the
host goes on drawing. Frame-time records have their own lock, and any thread may record. `glitnir_compose` composes
the root's latest snapshot with one composer for the process.

**Own outputs.** `glitnir_layer_set_output` makes a window layer right under the root's frame layer its own output
(at most 4 per root): the shell shows it on a surface of its own. The host gives the window's exact rectangle; the
commit snaps it outward to the rows and columns a subsurface can take at the root's scale (`snap::outward`, stable
rounding) and lists the output in the snapshot (`Committed::outputs`: rectangle, scroll, damage in its own
coordinates, `full` when nothing of its last picture can be kept). Scene damage inside the window goes to the output
only; the snapped rectangle's overlap with the neighbours goes to both. A `glitnir_scroll` whose area holds the whole
window becomes the output's scroll distance plus its exposed rows, and damage drawn before it moves with the pixels;
any other scroll stays damage. An output that goes (demoted, hidden, removed) or moves damages its old rectangle on
the frame. `pool::Ring` is the output buffers' row arithmetic: buffers `min(2·view, 8192)` rows tall, a scroll moves
the view's offset and rebases to the middle at the buffer's end, and `punch_rows` names the rows more than half a
view away, whose pages the shell may release.

The `pixman-oracle` feature (Linux; `build.rs` links the system's `pixman-1`) adds the test that compares the
kernels with pixman; `check.sh` runs it on Linux.

## Decorations

A window layer carries label anchors (`glitnir_decor_*`): the first glyph of a labelled run, its clip and a sprite key,
in layer pixels. The host keeps them as true as the pixels (a glyph draw drops and records, a clear drops, a scroll
moves). At commit the scene places a label per anchor whose sprite is cached (`glitnir_decor_sprite_put`, 64 sprites,
least recently used out): above the glyph, below it on the window's first text row, shifted right past an earlier label
on the row, one row further from the text when the shift would leave the layer. A label that appears, moves or goes
damages its old and new bounds; compose draws the sprites over the window layer's pixels, clipped. Sprites are drawn
as given, at the layer's device scale, never resampled.

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

GIF (`images/gif.rs`) reads the file's records as giflib's `DGifSlurp` does (its error texts and quirks: an
extension's continuation sub-blocks, a code size over 8, a missing trailer, a frame outside the screen checked after
the whole file is read) with a walker of its own, and decodes each frame's raster with the `gif` crate's `Decoder`
over a one-frame file built from the frame's bytes: the crate's `FrameDecoder` loops forever on LZW data that ends
early, so it is not used. Frames are composed by a port of the host's GIF loader: the frame background outside frame
0, disposal (none, background, previous), transparency, the delay and the extension blocks (returned as
function, length, bytes triples). WebP (`images/webp.rs`) decodes a still with `image-webp` and composites it over
the background with the host's single-precision formula; an animation's container is read as `WebPDemux` reads it,
each frame's chunks are decoded as a still image of their own (`image-webp`'s own animation path composites with
the file's background hint, libwebp's does not), and the canvas is built by a port of libwebp's `anim_decode.c`
(key frames, `BlendPixelNonPremult` checked against every alpha pair, dispose to background; `images/anim.rs`).

Animations: the crate keeps no state between calls. When the caller sets `keep`, the decode returns its animation
state (`out.anim`, an opaque pointer with its size in `anim_bytes`) and the next call may hand it back in
`params.anim` (the call then owns it, also when it fails; `data` may be NULL) to decode the next frame from the
current one, as the C cores' caches do; `glitnir_img_anim_free` frees one the caller drops. An animation has at most
`MAX_FRAMES` (2^18) frames; more fail as an allocation would.

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

## Headless mutter

The GUI rows run under `mutter --headless` (`nidhogg/test/gui.sh`), which loads mutter's default plugin, not GNOME
Shell. In mutter 51.1 (Fedora 45) that plugin's size-change effect (`size_change` / `size_changed`,
`src/compositor/plugins/default.c`) leaks: each fullscreen or maximize change paints the window into a new clone
actor, which is destroyed only when its 250 ms effect stops normally. A change that comes while an effect still runs
overwrites the clone pointer, a switch between two states of one size (fullscreen <-> maximized: a headless monitor
has no panels) never reaches `size_changed`, and with `MUTTER_DEBUG_DISABLE_ANIMATIONS=1` no clone is ever destroyed.
Every leaked clone keeps a window-sized texture: about 40 MB at 1920x1080, 50-150 MB at 2560x1440 and 1.25. A plain
GTK 3 window and Nidhogg with Glitnir off leak alike, at every scale; resizes and `glitnir-exact-scale` toggles do
not leak. I2's OOM (335 MB -> 5.7 GB at 1.25) came from rows switching fullboth <-> maximized every 0.1 s. The rows
therefore change these states only from a plain window and `glitnir-p15-gui--settle` (0.6 s) apart
(`glitnir-p15-gui--state`), and `glitnir-gui/exact-compositor-memory-bounded` keeps mutter's growth under 300 MB
across toggles, resizes and maximize/fullscreen changes at the exact scale.

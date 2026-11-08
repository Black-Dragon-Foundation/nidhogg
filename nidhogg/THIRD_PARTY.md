# Third-party sources

Files derived from another project's code carry an `Adapted-from: <source>` line in their header. `<source>` is the
`##` heading of an entry below; `nidhogg/test/tree-test.sh` checks both directions (every `Adapted-from` names an
entry, every entry is named by at least one file).

Format of an entry, one `##` section per source:

    ## <name or URL used in the Adapted-from line>

    - URL: where the code lives
    - Commit: the revision the code was taken from
    - Licence: the SPDX identifier of the licence it was taken under
    - Files: the files in this tree derived from it

The licence texts of these sources are in `nidhogg/licenses/`; the tarball carries that directory and this file in
`share/nidhogg/licenses/`.

Entries:

## https://github.com/zed-industries/zed@347f171a3fd00a3b672382d78348b37bd969d17c crates/gpui_linux/src/linux/wayland/serial.rs

- URL: https://github.com/zed-industries/zed
- Commit: 347f171a3fd00a3b672382d78348b37bd969d17c (2026-10-24)
- Licence: Apache-2.0 (`nidhogg/licenses/LICENSE-APACHE`); the adapted file is `GPL-3.0-or-later AND Apache-2.0`
- Files: `nidhogg/shell/src/wayland/serial.rs` (the input serial tracker, copied with small changes noted in its
  header)

## https://github.com/zed-industries/zed@347f171a3fd00a3b672382d78348b37bd969d17c crates/gpui_linux/src/linux/wayland/clipboard.rs

- URL: https://github.com/zed-industries/zed
- Commit: 347f171a3fd00a3b672382d78348b37bd969d17c (2026-10-24)
- Licence: Apache-2.0 (`nidhogg/licenses/LICENSE-APACHE`); the adapted file is `GPL-3.0-or-later AND Apache-2.0`
- Files: `nidhogg/shell/src/wayland/clipboard.rs` (the offers' MIME tracking, reading a `receive` pipe and writing a
  `send` descriptor; the calloop parts rewritten for the reader thread, the changes noted in its header)
## https://codeberg.org/dnkl/foot@cb2771788998f6a6288e2de8273c55a8214551e0 ime.c

- URL: https://codeberg.org/dnkl/foot
- Commit: cb2771788998f6a6288e2de8273c55a8214551e0 (2026-09-18)
- Licence: MIT (`nidhogg/licenses/LICENSE-MIT-foot`); the adapted file is `GPL-3.0-or-later AND MIT`
- Files: `nidhogg/shell/src/wayland/text_input.rs` (foot's text-input-v3 logic: pending state applied on `done`, the
  commit-serial check, enable on enter and disable on leave; rewritten in Rust)

## https://github.com/zed-industries/zed@347f171a3fd00a3b672382d78348b37bd969d17c crates/gpui_linux/src/linux/wayland/cursor.rs

- URL: https://github.com/zed-industries/zed
- Commit: 347f171a3fd00a3b672382d78348b37bd969d17c (2026-10-24)
- Licence: Apache-2.0 (`nidhogg/licenses/LICENSE-APACHE`); the adapted file is `GPL-3.0-or-later AND Apache-2.0`
- Files: `nidhogg/shell/src/wayland/cursor_theme.rs` (loading an Xcursor theme through `wayland-cursor` and showing
  one cursor image on a pointer surface; sizes, names and the reload rules are Nidhogg's, noted in its header)

Crates locked for the cursor theme (`nidhogg/shell/Cargo.lock`; fetched from crates.io, vendored unmodified in the release's vendor tarball; MIT, accepted by `deny.toml`): `wayland-cursor` 0.31.14 and `xcursor` 0.3.11, from crates.io.

## https://github.com/WebKit/WebKit@029da7d3d074a75a24c2413e98b183a8bbfcfb62 Source/WebCore/platform/graphics/UnitBezier.h

- URL: https://github.com/WebKit/WebKit
- Commit: 029da7d3d074a75a24c2413e98b183a8bbfcfb62 (2026-08-28, the last commit that touched the file)
- Licence: BSD-2-Clause (`nidhogg/licenses/LICENSE-BSD2-webkit`); the adapted file is `GPL-3.0-or-later AND BSD-2-Clause`
- Files: `nidhogg/glitnir/src/ease.rs` (the cubic-Bézier solver for CSS easing curves: sample table, Newton steps,
  bisection; rewritten in Rust)

Crates locked for SVG on macOS (the shell's feature `svg`; `nidhogg/shell/Cargo.lock` and `nidhogg/glue/Cargo.lock`;
fetched from crates.io, vendored unmodified in the release's vendor tarball; each under a licence `deny.toml` accepts,
which `cargo deny` checks): `resvg` 0.45.1 and `usvg` 0.45.1, with `arrayref` 0.3.9, `arrayvec`
0.7.8, `autocfg` 1.5.1, `base64` 0.22.1, `bitflags` 1.3.2, `bytemuck` 1.25.2, `core_maths` 0.1.1, `data-url` 0.3.2,
`euclid` 0.22.14, `float-cmp` 0.9.0, `fontconfig-parser` 0.5.8, `fontdb` 0.23.0, `imagesize` 0.13.0, `kurbo` 0.11.3,
`libm` 0.2.16, `memmap2` 0.9.12, `num-traits` 0.2.19, `pico-args` 0.5.0, `png` 0.17.16, `rgb` 0.8.53, `roxmltree`
0.20.0, `rustybuzz` 0.20.1, `simplecss` 0.2.2, `siphasher` 1.0.4, `slotmap` 1.1.1, `strict-num` 0.1.1, `svgtypes`
0.15.3, `tiny-skia` 0.11.4, `tiny-skia-path` 0.11.4, `tinyvec` 1.13.3, `ttf-parser` 0.25.1, `unicode-bidi` 0.3.18,
`unicode-bidi-mirroring` 0.4.0, `unicode-ccc` 0.4.0, `unicode-properties` 0.1.5, `unicode-script` 0.5.9, `unicode-vo`
0.1.0, `version_check` 0.9.5 and `xmlwriter` 0.1.0.

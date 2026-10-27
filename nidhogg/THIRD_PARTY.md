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

Crates vendored for the cursor theme (in `nidhogg/shell/vendor/`, unmodified, MIT, accepted by `deny.toml`): `wayland-cursor` 0.31.14 and `xcursor` 0.3.11, from crates.io.

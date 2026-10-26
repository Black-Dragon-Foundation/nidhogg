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

# Nidhogg

Nidhogg is GNU Emacs 31 with dragonX's patches: the Emacs that [dragonX](https://github.com/Black-Dragon-Foundation/drgnx)
runs on, maintained by the Black Dragon Foundation. In Norse myth Nidhogg is the dragon gnawing at the root of the world
tree; here it is the dragon underneath dragonX.

**Status: early.** v0 is upstream Emacs 31.1 plus Nidhogg's identity (version, feature, command). The compositor, speed
work and fixes come next. Packages keep working, because Nidhogg is still Emacs.

## Relationship to GNU Emacs

Nidhogg is a downstream build. It is not affiliated with or endorsed by the FSF or the GNU project. Like Emacs it is
licensed GPL-3.0-or-later; upstream's own `README`, `COPYING` and `etc/NEWS` apply unchanged. The patches are not meant
for upstreaming.

## Identity

- Command: `nidhogg` (and `nidhoggclient`). Feature: `nidhogg` (`(require 'nidhogg)`). Variable: `nidhogg-version`.
- `emacs-version` stays GNU Emacs's own (`"31.1"`): packages call `version<` on it, and `version-to-list` rejects
  `"31.1-nidhogg.1"`. Nidhogg's version lives in `nidhogg-version` and is appended to `M-x emacs-version` output.

## Version scheme

`<GNU Emacs version>-nidhogg.<N>`. N counts Nidhogg releases on one upstream release and restarts at 1 on the next
(31.2 gives `31.2-nidhogg.1`). The git tag is `v31.1-nidhogg.1`. The single source is the file `nidhogg/version`.

## Branches and the patch series

- `emacs-31` is an exact mirror of upstream's `emacs-31`; it is never committed to.
- `nidhogg-31` is the series, rebased onto the upstream release tag (`emacs-31.1` for the first release).
- Every commit on `emacs-31.1..nidhogg-31` has a subject starting `nidhogg: `.
- Files only Nidhogg owns live under `nidhogg/`, `.github/` and `README.md`, so rebases do not conflict.
- C patches sit behind `--with-nidhogg-<feature>` configure flags. The identity patch is unconditional.
- Each flag group has commits with subjects starting `nidhogg: <group>:` (groups: `glitnir`, `watchdog`, `workers`; the stack order is watchdog, then workers, then glitnir, and the check is
  order-independent; `FLAG_GROUPS`,
  overridable: name:define:feature) and its own base: `<GROUP>_BASE` (overridable, e.g. `GLITNIR_BASE`) is the parent of
  the oldest such commit on HEAD; a group with none is skipped. The groups may stack in either order. With the defines
  of every group that has commits in `base..HEAD` undefined (`HAVE_GLITNIR`, `HAVE_NIDHOGG_WATCHDOG`, `HAVE_NIDHOGG_WORKERS`), every C file under
  `src/` the group touched must equal its base version (`unifdef -U…`); a changed `lisp/` file is new or `lisp/loadup.el`
  with one `(featurep 'FEATURE)` line per group; a commit in the range touching `src/` that belongs to no group goes
  below the groups. Needs `unifdef`.
- `bash nidhogg/test/tree-test.sh [BASE]` checks these rules; `TREE_TEST_SELF=1` runs the flag-group rule on fixtures.

## Rebasing onto a new upstream release

```bash
git fetch upstream emacs-31 tag emacs-31.2 --no-tags   # and fast-forward the emacs-31 mirror
git tag v31.1-nidhogg.N nidhogg-31                       # the old series stays reachable (already tagged at release)
git rebase --onto emacs-31.2 emacs-31.1 nidhogg-31
echo 31.2-nidhogg.1 > nidhogg/version                    # amend into the tree commit
bash nidhogg/test/tree-test.sh emacs-31.2
```

`AGENTS.md` is Nidhogg's own (it replaces upstream's, so no agent ever sends anything to GNU), `CLAUDE.md` is
`@AGENTS.md`, and `README.md` is Nidhogg's README (upstream ships only `README`; if it ever adds a `README.md`, ours
wins). If the rebase stops, and `git diff --name-only --diff-filter=U` lists only `AGENTS.md`, `CLAUDE.md` and/or
`README.md`, keep Nidhogg's version and continue (during a rebase "theirs" is the commit being replayed, i.e. Nidhogg's):

```bash
git checkout --theirs -- AGENTS.md CLAUDE.md README.md && git add AGENTS.md CLAUDE.md README.md && git rebase --continue
```

(Name only the files that conflict.) Any other conflict stops for a human. An upstream edit to `CLAUDE.md` that applies
without a conflict is caught by `tree-test.sh`: restore it with `printf '@AGENTS.md\n' > CLAUDE.md` and amend the agents
commit. `bash nidhogg/test/agents-test.sh` checks these commands on a throwaway repo, including upstream adding
`README.md`.

## Building

```bash
nidhogg/build.sh (--container DISTRO | --host) [--prefix DIR] [--jobs N] [--memory SIZE]
                 [--build-dir DIR] [--check FILE] [--dev] [--dry-run]
```

- Targets today: Linux (pgtk) in a container or on the host. macOS (NS app, `Nidhogg.app`) is planned: `--host` has the
  code path, but it has not been built or tested yet.
- Requirements: podman for `--container` (never docker) and enough free memory (below).
- `--container DISTRO` (`fedora-44`, `fedora-45`, `ubuntu-26.04`) builds in a memory-capped podman container (podman only,
  never docker). `--host` builds on this machine (on Linux the packages in `nidhogg/deps/<distro>.build`
  must be installed).
- Configure flags: `--with-native-compilation=aot --with-tree-sitter --with-pgtk` (Linux), `--with-ns ...` (macOS), each followed by `--with-glitnir` unless `--without-glitnir` is given.
- Memory rule: the build is refused below 3 GiB (3072 MiB) of available memory; the container cap is max(2, min(6, free - 1)) GiB.
  `make -j` is min(CPUs, cap / 1.5 GiB). `--memory` and `--jobs` override; on a host build either one turns the refusal
  into a warning (the caller takes responsibility).
- `--prefix` defaults to `/opt/nidhogg/<version>` (macOS: where `Nidhogg.app` goes).
- `--build-dir` keeps the tree and logs (`autogen.log configure.log make.log check.log install.log`) for incremental
  rebuilds; by default a temporary directory is used and removed on success. A failed build names its log.
- Output: one `== <phase>` line per phase (`autogen`, `configure`, `make -jN`, `check <target>`, `install`, `tarball`), a
  `... make still running, N min` line every 5 minutes (`NIDHOGG_BUILD_HEARTBEAT` seconds), and on failure the last 100
  lines of the failing log. The container is named `nidhogg-build-<pid>` and stopped and removed on exit, INT or TERM.
  `NIDHOGG_BUILD_PKG_CACHE=DIR` keeps the container's downloaded packages (dnf, apt) in DIR between builds (CI caches it).
  `NIDHOGG_BUILD_CCACHE=DIR` (containers) compiles the C code through ccache (installed in the container, compiler checked
  by content) with its cache in DIR; the `.eln` files from native compilation do not go through ccache.
- **Glitnir (the compositor)** is Nidhogg's Rust library (`nidhogg/glitnir/`). Source builds therefore need **Rust**:
  `cargo` and `rustc` 1.93 or newer (the build lists name the packages: `cargo` + `rust` on Fedora, `cargo` + `rustc` on
  Ubuntu, `rust` from Homebrew on macOS). `build.sh` passes `--with-glitnir` on pgtk and NS; `--without-glitnir` builds
  without it and without installing Rust. The configure flag is `--with-glitnir` (default no; `HAVE_GLITNIR`): configure
  checks `cargo` and `rustc` only under the flag and otherwise stops with `--with-glitnir needs cargo and rustc 1.93 or
  newer`. `make` builds the crate offline and locked, with its cargo home and target directory under `src/` of the build
  tree (never `~/.cargo`), and links it into `temacs`. With the flag the feature `glitnir` is provided, `glitnir-enable`
  (default `t`) and `glitnir-active-p` exist; without it none of them do. Release tarballs need no Rust.
  `glitnir-active-p` is non-nil for a pgtk frame (not a tooltip) that draws on an ARGB32 image surface (GDK's Wayland
  surfaces) while `glitnir-enable` is non-nil. For such a frame pgtk's drawing primitives record **damage** (device
  pixels, rounded outward; a scroll as one operation; the visible bell, resizes and scale changes as the whole frame;
  nothing while `x-export-frames` draws); an inactive frame makes no call into Glitnir. `(glitnir--damage &optional
  FRAME)` takes a frame's pending damage as `((X Y W H) …)`; the next consistency point takes it otherwise, unless
  `glitnir-debug` is `damage-hold` (the damage then stays until read and the points present whole frames).
  **Damage-only present.** The first damage on a root frame or its child frames freezes the root's GDK window
  (`gdk_window_freeze_updates`), so GTK never paints a half-drawn picture; so does a scene change made inside
  redisplay, and a child frame's new border ring (its first one is drawn before it has a surface, so no damage of its
  own would freeze the root). At a **consistency point** (redisplay's buffer
  flips unblocked, also after an error or a throw out of redisplay; a frame up to date outside a flip block;
  `flush_display` and the end of the pgtk handlers that draw — mouse highlight, focus, the visible bell — and of
  `pgtk_read_socket`, these only outside redisplay; an idle callback for any other drawing outside redisplay) the root's
  damage, its children's included, is committed and invalidated rectangle by rectangle on the root widget, in place of
  pgtk's whole-widget invalidation, and the root thaws and paints at once (`gdk_window_process_updates`; without it,
  GLib dispatch inside redisplay would find the root frozen and starve the display). With `glitnir-enable` nil the
  stock path runs: whole-widget paints, no call into Glitnir but the frame-time record, and a frozen root thaws at its
  next point. `glitnir-stats` records the commit time on both paths (`:latency-ms`), the freeze (`:freeze-ms`), the
  committed and the painted pixels; `:backfill-rate` is nil (GDK's staging surface hides back-fills). Test primitives:
  `glitnir--draw-count`, `glitnir--frozen-p`, `glitnir--last-paint`, `glitnir--now-ns`, `glitnir--max-paint-gap-ms`,
  `glitnir--motion` (a synthetic pointer motion, handled by GTK at once), `glitnir--draw-undamaged` and
  `glitnir--compose-with-pending` (see *Debug modes*).
  **Debug modes** (`glitnir-debug`; nil in normal use):
  - `damage`: every committed rectangle is painted over the frame for 100 ms in the background of the
    `glitnir-debug-damage` face (half transparent; the frame's surface never holds it).
  - `verify`: a copy of each frame's surface (a child frame's too) is kept as of the previous commit. At each commit,
    pixels that changed outside the committed damage are logged once to stderr, `glitnir verify: FRAME X Y W H after
    PRIMITIVE` (the bounding box in device pixels, the drawing primitive that recorded damage last), then in braces
    the count of such pixels, the first one's old and new value and the committed rectangles, and counted in
    `:violations` of `glitnir-stats`. A GTK paint of a root that still has uncommitted damage or an uncommitted scene
    change logs `glitnir verify: compose with pending damage` and a backtrace (who dispatched GLib), and counts; a
    freeze that lasts over a second logs when it ends. Each counted line ends with the frame's state in brackets (root
    or child, its name, size and place, its surface, visibility, whether redisplay runs, the root's dirty, pending and
    frozen state, a monotonic time in ms). It costs a full
    surface compare per commit: for tests and hunting, not daily use. `(glitnir--draw-undamaged)` paints a rectangle
    without recording it (the next commit reports it); `(glitnir--compose-with-pending)` forces one paint of the root
    while damage is pending; `(glitnir--debug-freeze)` (any non-nil `glitnir-debug`) freezes the root as the first
    damage after a point does, so that a test can act between the freeze and the point that thaws it. Verify also
    counts a commit whose stacking of child frames differs from GDK's (`(glitnir--stacking)`: Glitnir's, bottom to top),
    and logs uncounted `glitnir verify: FRAME composed without content` for a child frame's surface composed with a
    transparent middle pixel on an opaque frame. A frame's surface made anew inside redisplay (a size or scale change
    at an `unblock_input`) is never shown before the frame has been redrawn whole: redisplay may clear the garbaged
    flag that would redraw it, so the last complete surface stays and the frame is garbaged again;
    `(glitnir--debug-reset-surface FRAME)` makes it anew for tests.
  - `full`: every commit damages the whole frame (the paint that Glitnir would otherwise have saved).
- **Dev build** (`--dev` or `NIDHOGG_BUILD_DEV=1`, for agents' task builds): configures `--with-native-compilation=yes`
  instead of `aot`, so only the preloaded Lisp is native-compiled and the other ~1,400 files stay bytecode (minutes instead
  of the better part of an hour). A `lisp/site-init.el` written into the build copy sets `native-comp-jit-compilation` to
  nil in the dumped image, so no background compiles run during tests (`nidhogg/test/gui.sh` and drgnx's harness need no
  change). GUI and functional tests are fine on it; performance runs (Gate A, budgets, A/B) and releases need the AOT
  build. It can't pass for a release: `--dev` refuses `--out` and `--stage pack`, and its compiled-Lisp cache stamp
  carries the mode, so dev and AOT entries never mix. Use a fresh `--build-dir` when switching modes.
- `--check FILE` runs the `make -C test` targets listed in FILE (`nidhogg/check-subset.txt` is what CI runs).
- `bash nidhogg/test/build-test.sh` checks the dry-run output.

## Release tarballs

`nidhogg/build.sh ... --out FILE` installs into a staging tree and writes `nidhogg-<version>-<distro>-<arch>.tar.xz`
(distro `fedora-44`, `fedora-45`, `ubuntu-26.04` or `macos`; arch is `uname -m`) and `FILE.sha256`.

- One top directory `nidhogg-<version>/` holding the prefix's contents (macOS: `Nidhogg.app`).
- `share/nidhogg/BUILDINFO` (`key=value`: `version commit distro arch prefix configure date`),
  `share/nidhogg/deps.runtime` (the distro's runtime packages) and `share/nidhogg/env.sh`. On macOS, `BUILDINFO` and
  `deps.runtime` sit beside the app, which relocates by itself.
- `bin/nidhogg.pdmp`, `bin/emacs.pdmp`, `bin/emacs-<upstream>.pdmp` are relative links to the dump, so a relocated binary
  finds it beside itself.
- At the `prefix` in `BUILDINFO` the tree runs as is. Anywhere else, source `env.sh` first:
  `NIDHOGG_ROOT=DIR . DIR/share/nidhogg/env.sh`.
- Limit: a relocated tree natively compiles non-preloaded libraries into the user's cache.
- `bash nidhogg/test/tarball-test.sh TARBALL DISTRO` unpacks at the prefix and elsewhere (a `--memory 2g` container on
  Linux) and runs the checks on each. On Linux they start with `ldd` on every program in `bin/` and `libexec/`: a
  library not found fails the test, all of them named at once (their packages belong in `deps/<distro>.runtime`).

## GUI tests

`bash nidhogg/test/gui.sh --prefix DIR [--scale S] [--size WxH] [--monitors N] [--select REGEXP] [--tests DIR]
[--daemon] [--container DISTRO] [--wayland-debug FILE] [--env K=V]... [--timeout SEC]` runs the `*-gui-tests.el` files of
`test/src/nidhogg-tests/` (ERT tests named `glitnir-gui/…`, `nidhogg-gui/…`) on a Nidhogg pgtk frame under a headless
mutter. `DIR` is a Nidhogg prefix (an unpacked tarball or an install).

- Everything is private to the run: `dbus-run-session` gives mutter its own session bus, mutter its own Wayland display
  (`glitnir-gui-<pid>`) and virtual monitors (`--size`, default 2880x1800, `--monitors`), and `HOME` and the XDG
  directories are temporary. `WAYLAND_DISPLAY`, `DISPLAY`, the session bus and `EMACS_SOCKET_NAME` of the caller are
  unset, so a run never reaches the caller's compositor, session or Emacs server. Everything started is stopped on exit.
- The scale: `gdctl set --logical-monitor --scale S` inside the session (mutter 50 accepts a fractional scale on a headless
  virtual monitor). GTK 3 knows only integer scales, so `(frame-scale-factor)` is `ceil(S)` and the monitor's GDK geometry
  `(WxH / S) * ceil(S)` (S = 1.25 on 2880x1800: 4608x2880). The runner waits up to 10 s for both before the tests start.
- `--daemon` starts `nidhogg --fg-daemon` (socket in the run's private runtime directory) and opens the frame with
  `nidhoggclient -c`; the runner starts the tests from `server-after-make-frame-hook`.
- `--container fedora-45` does the same in a `--memory 6g` podman container (image pinned by digest; `mutter`,
  `dbus-daemon` and the tarball's `deps.runtime` are installed there). A tarball is mounted at the prefix its `BUILDINFO` names.
- The tests start after the first frame is visible and redisplayed, never inside `command-line`. Exit 0 when all tests
  passed, 1 on a failure or a timeout (default 300 s).
- CI job `gui` (below) runs the fedora-45 tarball at scales 1 and 2.

## CI and releases

`.github/workflows/build.yml` runs on pushes to `nidhogg-31`, `v*-nidhogg.*` tags, pull requests and by hand
(`drgnx_ref` input: the drgnx branch, tag or commit under test). Without the input the drgnx commit is the one pinned
in `DRGNX_REF`, never drgnx's moving `main`: a drgnx push cannot turn Nidhogg red. Bump the pin by hand to a drgnx
`main` commit whose own CI is green.

- `tree`: `tree-test.sh` and `ci-test.sh` (full history and tags).
- `linux` (fedora-44, fedora-45, ubuntu-26.04): `build.sh --container D --stage build`, the cache saves, then `--stage check
  --check nidhogg/check-subset.txt`, then `--stage pack --out ...` (so a failing check keeps the build's caches), then
  `tarball-test.sh`. `macos`: Homebrew `deps/macos.build`, `build.sh --host` (`--stage build`, then `--stage pack`),
  `tarball-test.sh macos`. Each uploads its tarball and `.sha256`.
- `compile-noglitnir` (fedora-44): `build.sh --without-glitnir --stage build`, with no Rust installed (the tree still
  compiles without Glitnir). It is not part of the release gate.
- `drgnx-linux` (fedora-45 tarball) and `drgnx-macos`: first what drgnx's `ci.yml` builds for its suite, from the drgnx
  checkout and cached as there (Ratatoskr with the test hooks and `ratatoskr-mock`: `DRGNX_RATATOSKR_BIN`; huginn's
  `dump` example: `DRGNX_HUGINN_DUMP`), then `nidhogg/ci/drgnx-suite.sh TARBALL DISTRO DRGNX_DIR` runs drgnx's
  unit suite with `DRGNX_EMACS` set to the built Nidhogg (the `:timing` tests in a second run). Locally, a narrowed run:
  `DRGNX_TEST_SELECTOR='drgnx-doctor/' bash nidhogg/ci/drgnx-suite.sh TARBALL fedora-44 ../drgnx`.
- `gui` (fedora-45 tarball, after `linux`; scales 1 and 2; 60 minutes): `nidhogg/test/gui.sh --container fedora-45`
  (see GUI tests). It is not part of the release gate.
- `release` (a pushed `v*` tag only, never a manual run or a pull request) needs all five jobs, fails unless the tag is `v$(cat nidhogg/version)`, and publishes the
  tarballs, their `.sha256` files and `SHA256SUMS`; the notes name the built commit and the drgnx commit tested.
- Caches (`actions/cache/restore` before the build, `actions/cache/save` right after the step that produced them, so a
  later failing step, e.g. the tarball test, loses nothing; a save is skipped when its key was already found, and a build
  that failed saves nothing). A key is immutable: change its inputs and the key changes; to force a rebuild, edit
  `build.sh` or delete the entry in the repository's Actions caches. Linux (`D` = distro, `G` = hash of `deps/D.build`):
  - built tarball: `tarball-D-<git tree hash of HEAD>-<hash of build.sh>`, exact. A hit skips the build and Emacs's
    checks; the tarball test, the upload and drgnx's suite still run. Never used or saved for a release tag: a release is
    built fresh.
  - compiled Lisp: `lisp-D-G-<nidhogg/ci/lisp-key.sh>-<hash of build.sh>`, exact (no fallback: a partly stale set is not
    rebuilt correctly). The AOT native compilation of the Lisp (`.elc`, `.eln`) is the dominant build cost and ccache does
    not touch it. `lisp-key.sh` hashes the git trees of `lisp`, `leim`, `src` (the C side: `.eln` files embed the Emacs
    build's ABI hash, which the primitives determine), `lib`, `lib-src`, `etc`, the generators and configure inputs; docs,
    tests, `nidhogg/` and CI files are left out, so a change to those still hits. `build.sh` (`NIDHOGG_BUILD_LISP_CACHE`)
    saves the generated files of `lisp/` and `leim/` plus `native-lisp/` after `make`, and restores them with one
    timestamp newer than every source (CI first sets every tracked file's mtime to the commit time,
    `nidhogg/ci/set-mtimes.sh`), so `make` sees them up to date; an entry made by another gcc is ignored.
  - ccache for the C objects: `ccache-D-gcc-G-<commit>`, restored from the same without the commit (ccache hashes the
    preprocessed source and checks gcc by content). Downloaded packages (dnf, apt): `pkgs-D-G`.
  - macOS: the tarball (`tarball-macos-<arch>-...`), Homebrew's downloads (`brew-macos-<arch>-<hash of deps/macos.build>`,
    Homebrew verifies checksums) and compiled Lisp (`lisp-macos-<arch>-<gcc and libgccjit versions>-<runner temp>-...`).
  - Scoping: a run restores caches of its own branch or the default branch (`nidhogg-31`), and a pull request those of its
    base; never those of sibling branches or other tags. Keys are identical on every branch, so a run on `macos-ci` or a
    pull request reuses what `nidhogg-31` saved. Unused caches are evicted after 7 days and the oldest go first above 10 GB
    ([GitHub: dependency caching](https://docs.github.com/en/actions/reference/workflows-and-actions/dependency-caching)).
- Actions are pinned to commit SHAs. `bash nidhogg/test/ci-test.sh` checks these rules.

## Reporting issues

- Problems specific to Nidhogg (its patches, build, or `nidhogg-version`) go to this repository's issue tracker.
- Nidhogg's maintainers never forward reports to GNU, and nothing in Nidhogg sends anything upstream. If you reproduce
  a bug in a stock GNU Emacs build (not Nidhogg), you may report it to GNU yourself, in your own words, following
  GNU's own guidelines.

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
  `"31.1-nidhogg.3"`. Nidhogg's version lives in `nidhogg-version` and is appended to `M-x emacs-version` output.

## Version scheme

`<GNU Emacs version>-nidhogg.<N>`. N counts Nidhogg releases on one upstream release and restarts at 1 on the next
(31.2 gives `31.2-nidhogg.1`). The git tag is `v31.1-nidhogg.3`. The single source is the file `nidhogg/version`.

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

How much to expect: upstream's pgtk code (`src/pgtk*.c`, `src/gtkutil.c`) changed by +512/−185 lines from 29.1 to 30.1
and +528/−464 from 30.1 to 31.1 (measured 2026-10-21). Glitnir's hooks in it are 23 lines at five sites, and only
`pgtk_free_frame_resources` saw upstream hunks in that span: medium churn, acceptable.

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

## Stall watchdog

An always-on measurement of the stalls (busy spans of the main thread at least as long as a threshold, 100 ms by
default): a C monitor thread asks the main thread, by signal, for samples of what it is running, and Nidhogg's Lisp
turns the finished spans into JSON Lines records, names function, file and package, and ranks them.

- Build flag: `--with-nidhogg-watchdog` (defines `HAVE_NIDHOGG_WATCHDOG`; `build.sh` passes it on every target unless
  given `--without-nidhogg-watchdog`). `--with-nidhogg-watchdog-signal=NAME` picks the signal (default `SIGURG`).
  Linux (pgtk and terminal) first; elsewhere the flag builds and starting the watchdog signals an error.
- Off unless something turns it on (drgnx does). With the flag built in and nothing started there is no thread and no
  signal handler.
- With the flag off the build equals upstream's for every file the watchdog touches (checked by
  `nidhogg/test/tree-test.sh`).
- Files: `src/nidhogg-watchdog.h` (the shared layout and every cross-file prototype), `src/nidhogg-watchdog.c` (edges,
  spans, records), `-monitor.c` (monitor thread, park protocol), `-sample.c` (signal handler, raw ring), `-gc.c` (GC end
  hook), `-input.c` and `-phase.h` (phases, felt input), `-gtk.c` (GLib poll wrapper, pgtk only), `-park.h` (the park
  protocol's steps); Lisp in `lisp/nidhogg-watchdog*.el` (preloaded; the feature is `nidhogg-watchdog`).
- Tests: `watchdog/…` in `test/src/nidhogg-tests/watchdog-*-tests.el`, GUI rows `watchdog-gui/…` in `*-gui-tests.el`.

- **Spans and records** (`nidhogg-watchdog.c`): the main thread times every busy span at the two edges of the select in
  `really_call_select` (`thread.c`; also Android's non-threaded `android_select` and the tty `C-z` in `sys_suspend`).
  A wait ends a span only when it is responsive: the main thread's own select (never a Lisp thread's), inside
  `wait_reading_process_output` with `waiting_for_user_input_p` set, `inhibit-redisplay` nil and a non-zero timeout.
  `sleep-for`, `accept-process-output` inside a command, waits under `inhibit-redisplay` and lock contention stay busy.
  Every span ≥ the threshold becomes a record (`nidhogg-watchdog--take`; a ring of 32, the oldest dropped and counted),
  with or without samples; every span goes into the histograms of `nidhogg-watchdog--stats` by its non-GC time.
  `nidhogg-watchdog--test-clock` and `nidhogg-watchdog--test-idle-wait` drive the span rows on a virtual clock.
- **The monitor** (`-monitor.c`): a thread named `nh-watchdog`, created with every signal blocked; it takes only its own
  mutex. While Emacs waits it is parked on a condition variable and wakes for nothing; the main thread signals it when a
  busy span starts. It sleeps to half the threshold of the span; if the span is still busy, it notes the main thread's
  CPU clock and its own lateness, then every `max (threshold/10, 1 ms)` (1 ms in deep mode) raises a request and sends
  the main thread the watchdog signal (`pthread_kill`, never `kill`); a request the handler has not taken after two
  periods counts as missed. The park protocol (`-park.h`) is a Dekker pair of seq_cst stores and loads
  (`NH_WD_PARK_ORDER`), so no busy span is missed: `watchdog/no-lost-wakeup` runs its steps through every interleaving
  with store buffers modelled, and `nidhogg/test/watchdog-park-test.sh` runs them on real threads (ThreadSanitizer and a
  10⁷-cycle stress run; it needs gcc and libtsan, nothing from a build). While `nidhogg-watchdog--test-clock` is set the
  monitor never arms.
- Samples (`src/nidhogg-watchdog-sample.c`): the signal handler is installed with `sigaction` and `SA_RESTART`
  (interactive Emacs otherwise installs handlers without it), refuses to take a signal another library already handles,
  and answers only a request for the current busy span on the main thread; any other signal (a stray `kill`, a late
  request that lands in the idle wait) is counted as `:stale`. It calls nothing but `clock_gettime`, allocates nothing
  and takes no lock. It copies raw specpdl words (24 innermost frames and 8 outermost, 56 + 8 in deep mode) and a hook
  runner's hook name, and copies nothing while the specpdl is being reallocated (`grow_specpdl_allocation` brackets the
  realloc) or while a Lisp thread holds the global lock. A stall keeps at most 64 samples (32 deep): the handler thins
  them to every other one and doubles its stride. The words are never GC roots; `--take` turns a word into a function
  object only when `valid_lisp_object_p` calls it a live symbol, closure or subr (a non-heap subr only when it is a
  builtin's address), else into the symbol `nidhogg-watchdog--gone`.
- GC (`-gc.c`, one call at the end of `garbage_collect` in `alloc.c`, one flag set in `garbage-collect`): while the
  watchdog is on, every collection's pause is added to its span (a stall's `:gc-ms` and `:gcs`; `:nongc-ms` = wall −
  GC, which is what the 100 ms goal judges) and written to a 64-entry GC ring, taken with the stalls by
  `nidhogg-watchdog--take` as `(:gc ID :gcs-done N :start SECONDS :pause-ms F :span N :explicit BOOL :lisp-thread
  BOOL)`; a full ring drops its oldest entry and counts `:gc-dropped`. The pause runs from `garbage_collect`'s start
  to after `gcs_done` is incremented: it includes `run_finalizers` (Lisp finalizers) and
  `image_prune_animation_caches`, but not `post-gc-hook`. `:gcs-done` is the `gcs-done` a `post-gc-hook` function
  sees for the same collection, the key the Lisp GC row (and drgnx's GC census lines, `gcs_done`) joins on; never join
  by order. `:explicit` is a `garbage-collect` call (other C callers, such as `garbage-collect-maybe`, are automatic).
- Phases, felt input, typing (`-input.c`, `nidhogg-watchdog-phase.h`): a sample names the C phase on top of a small
  phase stack, set by one-line hooks: `redisplay` (`redisplay_internal`), `filter` and `sentinel` (process.c),
  `call-process`, `json` (parse, serialize, insert), `regex` (`re-search-*`, `looking-at`, `string-match`) and
  `process-wait` (a wait that is not responsive: `accept-process-output` or `sleep-for` inside a command, a wait under
  `inhibit-redisplay`); `lisp` when none is set. A responsive wait empties the stack, and a non-local exit
  (`unwind_to_catch`) pops every phase pushed inside its catch target, so a `condition-case` around a failing
  `json-parse-string` does not label the rest of the span `json`. Nothing goes on the specpdl. A stall is `:felt`
  when a keyboard, button or wheel event was stored during it (a key pressed while Emacs was busy) or when it ran a
  command; `:input-wait-ms` runs from that input's arrival (a pgtk event's own timestamp when it reads as
  `CLOCK_MONOTONIC` within the last 2 s, else the time it was stored) to the stall's end. `:commands` counts the
  commands in the span, and `:input-queued` says a second one ran in it (keys queued behind each other). The
  commands named by `nidhogg-watchdog--set-typing-commands` (16 at most) are timed one by one, from their key's
  arrival to the next key taken or the next wait, into the `:typing` histogram of `nidhogg-watchdog--stats`.
An open popup menu or dialog counts as the user waiting, not as a stall: on pgtk a wrapper around GLib's poll
function (`src/nidhogg-watchdog-gtk.c`) marks the wait. A Lisp timer that runs inside a menu or dialog is still
busy time and is reported.
The timing group (`watchdog-timing-tests.el`) runs on the real clock with wide margins (spins of 0.5 to 10 s
against a 100 ms threshold) and asserts counts, never sample numbers ("at least one sample, or `:missed` counted"): a
1 s spin from a timer (state CPU, the timer function on the stack), a blocked `call-process` (phase `call-process`),
no sample requests before half the threshold, child Nidhoggs on a pty for the real keyboard hooks (a key during a
spin makes the stall `:felt`), a Lisp thread that holds the global lock while the main thread idles (`:lisp-thread`),
`suspend-emacs` and `sit-for` (waiting, no stall), and a 10 s deep-sampling stress run (allocation, byte code, native
code, forced collections, a specpdl past 10⁵ entries) in which every sampled word is a live object or
`nidhogg-watchdog--gone`. GUI rows (`watchdog-gui-tests.el`, `nidhogg/test/gui.sh`): `sit-for` on a pgtk frame is
waiting, and a mode-line `:eval` that spins is a stall in the `redisplay` phase with the `:eval` function innermost.
`M-x nidhogg-watchdog-mode` (global, off by default) starts the monitor with `nidhogg-watchdog-threshold` (100 ms)
and `nidhogg-watchdog-hang-ms` (2000) and one repeating 1 s idle timer; a start that fails after the C side started
(too many typing commands, an unreadable log directory) stops it again and leaves the mode off. The timer's drain returns at once (no
allocation) when nothing is pending; otherwise it takes the rings at once and resolves records within 2 ms, the rest
waiting for the next idle period (`nidhogg-watchdog-drain-now` drains everything). Each stall is one JSON line (schema
`v` 1) in `YYYY-MM-DD.jsonl` under `nidhogg-watchdog-log-directory` (default `~/.emacs.d/watchdog/`); `nidhogg-watchdog-record-functions` sees each
resolved record. An append never prompts, even while a buffer visits the day file (it locks under a name no buffer
visits, with lock files off). At most 5,000 stall lines are written per day (the rest are only counted), day files older than 14
days are deleted, and a `summary` line (histogram deltas, counters, monitor CPU, the drain's histograms, p95 and
drains over budget) is appended at most every 10 minutes and when the mode is turned off or Emacs exits. A summary
never resets a counter: it subtracts the values it kept at the last line (`nidhogg-watchdog--stats-delta`, which a
perf probe can use the same way), so any number of readers can measure a stretch side by side.
`nidhogg-watchdog-stats` adds the drain's numbers to the C counters: `:drain`, the budgeted drains by duration in
buckets that start at 1 and 2 ms (<1 <2 <4 <8 <16 <33 <50 <100 ≥100), `:drain-take`, the part of each that takes the
rings and writes the collection lines (it runs before the budget's resolve loop), `:drain-over-budget` and
`:drain-p95-ms`; `nidhogg-watchdog-drain-now` (no budget: tests, perf, mode off) is not counted. The C counters
include `:samples` (samples the handler kept), to set `:missed` against; the summary line carries it too. A stall
record and its line say `deep` when the stall was sampled in deep mode (`nidhogg-watchdog--deep`).
Records are resolved at idle (`lisp/nidhogg-watchdog-resolve.el`): each function gets a name, file and package (native
subrs through their `.eln` unit, symbols through `function-history` or a `load-history` index built in 2 ms slices, a
record waiting until its symbols are indexed; closures are `<lambda>` and credited to the nearest named frame, native
lambdas are `<lambda in FILE>`). The stall line carries `entry` (command, timer, filter, sentinel, hook, redisplay,
gc, lisp-thread), `culprit` (innermost non-`emacs` Lisp frame in at least half the non-GC samples, with package, file,
share and dominant phase), `state` (`gc`, `cpu`, `blocked`, `mixed`), `phases`, `stacks` and a `signature`
(`entry.kind:entry.fn>culprit.fn/phase`) that byte code and native code share. `nidhogg-watchdog-package-functions`
(each called with a file name) can name a file's package before the defaults.
The GC row (`lisp/nidhogg-watchdog-gc.el`): GC pauses are judged by their own target, not the 100 ms goal. While
the mode is on, a `post-gc-hook` function only notes `gcs-done`, `garbage-collect-heapsize`, the origin and the
threshold in a 64-slot ring (no I/O); the drain joins those notes with the C GC ring's collections on `gcs-done`,
never by order, and writes one `gc` line per collection: `pause_ms` (exact, from C), `origin` (`idle`, `gap`,
`audit`, `explicit` or `auto`: the binding of `nidhogg-watchdog-gc-origin` around `garbage-collect`, else the C
side's explicit flag), `felt`, `span_wall_ms` and `span_nongc_ms` (when the collection's span is a stall; else
null), `gcs_done`, `heap` (live MB by type and free MB), `rss_mb` (at write) and `threshold_mb`. A collection or a
note whose partner was dropped by the other ring is written with the fields it has and counted.
`(nidhogg-watchdog-gc-row &key days)` reads the `gc` lines back (drgnx's GC census writes the same lines): per day
the count, total seconds, max, p50/p95/p99 from the histogram buckets, felt count and the last live heap; over the 7
days ending on a day the p95 (only from 20 collections on), the slope of pause against live heap (ms per 100 MB) and
the live-heap trend (MB per day). A day meets the row when that p95 is at most `nidhogg-watchdog-gc-target-p95`
(50 ms; unknown under 20 collections, then only the max judges) and no collection reaches `nidhogg-watchdog-gc-max`
(250 ms). The row reads only the files its windows need (the newest DAYS + 6) and keeps a small digest of each,
made again only when the file's size or time changed, so asking for it at idle every half hour costs no re-parse
of closed days.
The ranking (`lisp/nidhogg-watchdog-report.el`) reads the day files only when asked, never on a timer.
`(nidhogg-watchdog-ranking &key since felt-only)` returns the GC row first (`:gc t`, the collections' count, felt,
total, max and p50, the lines themselves and `nidhogg-watchdog-gc-row`'s numbers), then one row per package (the
culprit's package, `unsampled` when a stall has none), biggest score first. A package's score is the sum, over its
stalls whose non-GC time reaches the threshold, of that time minus the threshold, so many slightly slow stalls and
one long one weigh alike. Each row has the count, felt count, total, max and p50 of non-GC ms, the mix of states and
of phases (weighted by non-GC ms), the first and last time, a merged call tree of its stacks, and the same rows per
signature under it. `:since` (a time) limits it to lines from then on, `:felt-only` to felt stalls and
collections. `M-x nidhogg-watchdog-report` shows today's ranking in `*Nidhogg Watchdog*`: a header line (stalls,
felt, the longest, collections and their max, the share of typing commands at 16 ms or more), the GC row and the
packages with their signatures indented; RET opens a row's call tree, or on the GC row the day's collections; a
prefix argument shows felt stalls only.
Hangs (`-monitor.c`, `-sample.c`): past `nidhogg-watchdog-hang-ms` a stall is a hang, and its record says `:hang t`.
The monitor switches the handler to names mode: outside a collection it also copies the symbol names (64 bytes at
most, closures as `<lambda>`) of up to 32 frames below the innermost one, which stays a raw word, and publishes them
to the monitor through a small seqlock. The monitor owns the hang file, `hang-PID-START.txt` in the log directory
(START is the session's start in ns): it writes it through a temporary file (`open`, `write`, `fsync`, `rename`, and
an `fsync` of the directory, held open while the monitor runs, after the hang's first rename) once a names-mode copy is
in, then every 2 s, with the pid, the process start time (`/proc/self/stat` field 22) and the boot
id (both read once at start), the hang's start, its duration, its phase and its latest stacks; it skips the rename
when the span ended meanwhile, and deletes the file when the span has ended, before it parks. The main thread never
touches the file. A stop inside a hang keeps it: SIGTERM, SIGHUP and SIGINT make Emacs run `kill-emacs` from the
signal handler while Lisp is still busy, which turns the mode off, so `nidhogg-watchdog--stop` tells the monitor to
keep the file when the span in progress is past the hang threshold. When `nidhogg-watchdog-mode` starts, every hang
file whose process is gone (no such pid, or a pid with another start time or boot id) becomes a record with `:killed
t`, `:hang-pid` and its stacks (names), run through `nidhogg-watchdog-record-functions` and written to the log, and
is deleted; the session's own file (the mode turned off inside a hang without exiting) is only deleted. Rows:
`watchdog-hang-tests.el` (names mode, the identity, the file's schema and its deletion, a span that ends during a
write, using `nidhogg-watchdog--test-hang-now`, `--test-names` and `--test-hold-rename`) and
`watchdog/hang-file-survives-kill`, `-sigterm` and `-sighup` in the timing group (a child killed by that signal during
a 5 s hang; a fresh child records it).

## Work off the main thread

Nidhogg can move work that blocks the main thread onto worker threads: reading process output, parsing with
tree-sitter, decoding images, directory reads, file blocks, auto-save and host-name lookups. The main thread keeps
every Lisp object; a worker only reads bytes and files and hands a result back for the main thread to land.

- Build flag: `--with-nidhogg-workers` (defines `HAVE_NIDHOGG_WORKERS`; `build.sh` passes it on every target unless given
  `--without-nidhogg-workers`). Without it the build equals upstream's for every file the workers touch.
- Switches (customization group `nidhogg-workers`): `nidhogg-workers-process-reader` (on) with
  `nidhogg-workers-reader-pipes`, `-reader-sockets` and `-reader-ptys`; `nidhogg-workers-framing`,
  `nidhogg-workers-treesit`, `nidhogg-treesit-defer`, `nidhogg-workers-images`, `nidhogg-image-defer`,
  `nidhogg-workers-autosave`, `nidhogg-workers-dns`. Each is off until the default-on commit of its step.
  `nidhogg-redisplay-wait-budget` (8) is how many milliseconds a redisplay may wait for a worker.
- The environment variable `NIDHOGG_WORKERS`, read at startup: `default` (or unset), `on`, `off`, `inline`, or a list
  such as `+nidhogg-workers-treesit-nidhogg-workers-dns`. `nidhogg-workers-set-all` and `nidhogg-workers-apply` do the
  same at run time.
- No worker thread exists until the first job is submitted.
- Files: `src/nidhogg-workers.h` (the shared layout and every cross-file prototype), `src/nidhogg-workers-phase.h`,
  `src/nidhogg-{workers,treesit-bg,io,decode,json-tape,image-bg,blk}.c`; Lisp in `lisp/nidhogg-workers*.el`
  (preloaded; the feature is `nidhogg-workers`, the Lisp part `nidhogg-workers-lisp`; the tree-sitter and jsonrpc
  files load with their packages).
- Tests: `workers/…` in `test/src/nidhogg-tests/workers-*-tests.el`, GUI rows `workers-gui/…` in `*-gui-tests.el`;
  `nidhogg/test/workers-harness.sh` compiles the C files without Lisp (`--tsan MODULE…` runs the ThreadSanitizer
  drivers, `--fuzz SECONDS` the parser fuzz targets); `nidhogg/test/pkg-test.sh` runs the package suites (jsonrpc,
  treesit, process, files) with the workers on and off and `--compare`s them; `NIDHOGG_BUILD_SANITIZE=address,undefined
  nidhogg/build.sh --dev …` is the ASan build. CI jobs `workers-harness`, `workers-asan`, `workers-pkg` run them.
- The runtime (`src/nidhogg-workers.c`): five workers, `nh-io`, `nh-parse`, `nh-img` (one thread per two cores, at
  most two), `nh-blk` (one to four threads, grown when every thread has been busy 50 ms; directory reads, file reads
  and lookups take at most two each and always leave one for a class with nothing running) and `nh-save`. Threads
  start at their first job, never while dumping, with every asynchronous signal blocked (the fatal ones stay with
  Emacs's crash forwarding). Speculative jobs run at `SCHED_IDLE`; a wait on one that has not started moves it to the
  worker's normal-priority thread, and a wait on one already running tries to raise its thread (unprivileged this is
  refused once and counted as `boost-denied`). Finished jobs land in completion order at the next timer check, never
  inside a filter, through one completion fd (an eventfd; a pipe elsewhere) that is cleared before its queue is
  re-checked. Waits are bounded, survive signals and never go through `select`. `(nidhogg-workers-stats &optional
  RESET)` returns the counters. Test primitives: `nidhogg-workers--mode` (`threads` or `inline`), the pause points
  (`nidhogg-workers--pause-at`, `--release`, `--test-await-paused`), `--test-job`, `--test-wait`, `--test-job-info`,
  `--test-signal-storm`, `--test-wake-race`, `--test-fd-reserve`; `nidhogg/test/workers-harness.sh --tsan queue wake`
  runs the queues and the wake protocol under ThreadSanitizer.
- The fatal path: `shut_down_emacs` marks the workers dead first when Emacs dies of a signal or a fatal error. From
  then on no job is submitted, no wait waits, completions are dropped and the queues are abandoned (not freed), so a
  crash under a worker's mutex, or on a worker thread (forwarded to the main thread as usual), still prints its
  backtrace and writes the emergency auto-save. No signal handler takes a worker mutex: the SIGCHLD handler runs in
  signal context, where submit, wait and cancel refuse at once and a worker mutex taken anyway aborts the process
  rather than deadlock. `kill-emacs` with jobs in flight exits without waiting for them. Test primitives:
  `nidhogg-workers--test-crash`, `--test-fatal-under-lock`, `--test-signal-under-locks`.
- Waits and the redisplay budget: a main-thread wait on a worker pushes the watchdog's `worker-wait` (or `treesit-wait`)
  phase and counts its time in `:phase-ms`; it never enters `select`, so the watchdog's span runs through it. Every
  `redisplay_internal` call starts a wait budget of `nidhogg-redisplay-wait-budget` milliseconds (default 8), shared by
  all tree-sitter and image waits in that redisplay; once spent, a wait is granted nothing.
<!-- wk: Task 6 -->
<!-- wk: Task 7 -->
<!-- wk: Task 8 -->
- Tree-sitter's allocator: on a worker thread tree-sitter allocates with libc (a worker never reaches `xmalloc`, so the
  memory profiler is never entered from one); on the main thread it still goes through `xmalloc`. Test grammars (C,
  TypeScript, Python, Rust, HTML at pinned commits) are built by `test/src/nidhogg-tests/fixtures/grammars.sh` into
  `NIDHOGG_TEST_GRAMMARS` (default `fixtures/grammars-cache/`); the tree-sitter rows skip without them.
- Tree-sitter parse jobs (`src/nidhogg-treesit-bg.c`; tree-sitter 0.25 or later): with `nidhogg-workers-treesit` on, a
  job for a parser copies the buffer's visible text (two copies around the gap, the watchdog's `treesit` phase with
  reason `snapshot`), a copy of the parser's tree, the ranges a demand would parse with and the parser's edit
  generation; `nh-parse` parses that with a parser of its own (cancellable at any point; a cancelled parse is reset,
  never resumed) and computes the tree's subtree statistics. Nothing is published early: the next demand
  (`treesit_ensure_parsed`) takes the tree when the job is done and its language, visible range, ranges and
  generation are still the parser's, then runs today's tail (timestamp, changed ranges, notifiers), so notifiers run
  only at demands and nodes go out of date only there; any other job is dropped and today's synchronous parse runs.
  No job starts under a temporary narrowing, and a start never changes the parser's visible range or ranges. One job
  per parser, the latest snapshot wins; deleting the parser, killing its buffer or GC cancels it, and whichever of
  the parser and the worker lets go last frees it. `(nidhogg-treesit-parser-ready-p PARSER)` says whether a demand
  would not parse on the main thread; `treesit-subtree-stat` on a published root answers from the worker's count. Test
  primitives: `nidhogg-workers--test-ts-start`, `--test-ts-wait`, `--test-ts-info`, `--test-ts-stats`,
  `--test-ts-await-idle`.
- Edits during a parse job: every edit made to a parser's tree while it has a job (the changes Lisp makes, and the
  narrowing sync a demand does) is logged exactly as applied; a parser that has no tree yet logs the change in buffer
  bytes, which become tree edits at the publish. The publish replays the log onto the job's tree and, if there was any
  edit, reparses once on the main thread (the watchdog's `treesit` phase) with the parser's current ranges: the same
  incremental grouping as a synchronous parse at the snapshot, so typing during a long parse loses nothing the worker
  did. A parser that tracks line and column (`treesit-languages-require-line-column-tracking`) drops a job that started
  without a tree at its first edit. A start (unlike a demand) drops a job over older text for a new snapshot, and a
  cancelled parse is never resumed. The worker's `treesit-subtree-stat` answer is dropped when a replay reparsed.
  `--test-ts-stats` adds `:replayed`, `:replayed-edits`, `:superseded` and `:parses`; `--test-ts-info` adds `:log`.
- Where tree-sitter jobs start, and when the main thread waits: a job starts (1) when a major mode has finished, for
  each parser that never parsed (`nidhogg-workers-treesit--start-created` on `after-change-major-mode-hook`), so the
  first parse of a large file overlaps the rest of `find-file`; (2) at redisplay's demand: while redisplay fontifies a
  buffer (`nidhogg--redisplay-fontifying` names it), `treesit-parser-changed-regions` (the demand of
  `treesit--pre-redisplay`) first parses on the main thread for at most 1 ms, so ordinary typing parses exactly as
  before and takes no snapshot; a longer parse (a paste, a revert, `indent-region`) is cancelled and reset, never
  resumed, a job takes it, and the demand answers "not ready" (it returns nil and the parser still needs a reparse);
  every other demand (`treesit-parser-root-node`, so every node and query function) still ends with a tree; (3) when
  Emacs waits for keyboard input (`wait_reading_process_output` with `READ_KBD` ≠ 0), for parsers edited since by
  timers, filters or idle timers that no demand parsed. A demand that needs a tree while a job runs cancels the job
  and parses on the main thread when the job has not started, or when its snapshot is older than the newest edit and
  it has parsed less than half its input; otherwise it waits for the job (the watchdog's `treesit-wait` phase, the job
  moved to the normal-priority thread or raised, as for every wait) for at most the synchronous estimate, the last
  full parse's time per byte times the input (100 ns a byte before any; between 1 ms and 2 s), then publishes it, or
  cancels it and parses. Test primitives: `nidhogg-workers--test-fontifying` (a macro binding the variable),
  `--test-ts-select-entry` (batch Emacs never waits for keyboard input); `--test-ts-stats` adds `:attempts`,
  `:cancels`, `:started-demand`, `:started-create`, `:started-select`, `:waited` and `:abandoned`; `--test-ts-info`
  adds `:policy`.
<!-- wk: Task 13 -->
<!-- wk: Task 14 -->
<!-- wk: Task 15 -->
- The process reader's fd ownership (`src/nidhogg-io.c`): an eligible process (a subprocess with pipes or a pty, a
  `make-pipe-process` pipe, a plain network stream once connected, a connection a server accepted) is handed to `nh-io`
  when its fd would enter the select set, if `nidhogg-workers-process-reader` and its fd kind's sub-switch are on and two
  more fds stay 64 below `FD_SETSIZE` (else it keeps today's path). `nh-io` reads its own `F_DUPFD_CLOEXEC` copy of the
  fd; the select set holds a wake fd instead (so `process-prioritize-lower-fds` orders by wake fd); `p->infd` stays the
  real fd for writes, ioctls and `process-send-eof`. Never handed over: TLS, datagrams, servers, serial, a `:nowait`
  stream before it connects, filter `t` (until the filter changes), and a process after `(nidhogg-set-process-reader
  PROCESS nil)`. Hand-back is synchronous: before any of a process's fds closes, `nh-io` drops it and closes its copy,
  then acknowledges; the bytes it had read are delivered first. Delivery goes through the usual filter path, one
  delivery per channel per select (below). Test primitives: `nidhogg-workers--test-io-info`, `--test-io-log`,
  `--test-io-crash`, `--test-io-hold-lock`; `workers-harness.sh --tsan io`.
- The reader's flow control and drains: where Emacs stops reading a process (`set-process-filter` `t`, `stop-process` on
  a connection, the `SIGCHLD` handler when a child exits) `nh-io` pauses after the read in progress (an atomic flag and
  one byte to its command fd: no lock, so the signal handler may do it), and resumes where reading resumes. Each process
  queues at most max (4 × `read-process-output-max`, 256 KB) of unread output; at that cap `nh-io` stops reading it until
  delivery catches up, so the kernel's back-pressure applies as today (`stop-process` on a subprocess only stops the
  child; the cap bounds what is read meanwhile). When a process ends, its last output is delivered before its sentinel
  runs: the reader hands it back and Emacs reads the rest of the fd itself until it would block, so a grandchild that
  holds the pipe open does not delay the sentinel. Sentinels run, as today, only when no process has output waiting:
  Emacs first waits (at most a second) for output `nh-io` is reading or has yet to read, so a `:stderr` pipe's last
  output and EOF still come before the child's sentinel. A zero-timeout wait (`accept-process-output` with a timeout of
  0, how TRAMP polls) waits the same way, so it gets what the kernel holds, as today. `accept-process-output` with
  JUST-THIS-ONE waits on that process's wake fd only, and a process locked to a Lisp thread (`set-process-thread`) is
  delivered only on that thread.
  Test primitives: `nidhogg-workers--test-io-info`'s `:queued`, `:cap` and `:paused`; the `after-read` pause point
  (`nh-io` parks there when a queue reaches its cap).
- Decoding on the reader (`src/nidhogg-decode.c`): when a reader-owned process decodes with a stateless system
  (`utf-8`, `raw-text`, `no-conversion` or `binary` with a settled end of line: `-unix`, `-dos`, `-mac`; not
  `utf-8-emacs`, BOM variants or systems with a translation table or post-read conversion), `nh-io` decodes each chunk as
  it reads it, with copies of Emacs's own decoder and end-of-line rules (quirks included), and a filter gets that string:
  multibyte for UTF-8 (invalid bytes as raw-byte characters), unibyte for the raw-text systems, as today. Every other
  system, `undecided` and end-of-line detection stay on the main thread; once the main thread's detection settles a
  process on a stateless system, `nh-io` decodes from the next chunk. Each chunk keeps its raw bytes and the process's
  coding generation, which every `set-process-coding-system`, `set-process-buffer` and `set-process-filter` moves on: a
  chunk decoded under an older generation, any chunk while `standard-translation-table-for-decode` or
  `inhibit-eol-conversion` is in force, and any chunk whose carried-over bytes differ from the main thread's are decoded
  again on the main thread from the raw bytes, so filters see what they see without the reader. An incomplete sequence
  at the end of the stream is flushed as today. The default filter inserts those decodes too (below). Test primitives:
  `nidhogg-workers--test-decode` (the decoder, or with ORACLE the main thread's, over a list of chunks),
  `--test-io-info`'s `:coding-generation`, `:decoder`, `:decoded` and `:redecoded`; `workers-harness.sh --fuzz SECONDS
  decode`.
- Delivery from the reader: one delivery per channel per select, as today's one read, but it joins the chunks already
  queued (it never waits for more). The default filter (with `fast-read-process-output`) gets up to
  `read-process-output-max` bytes and inserts the reader's decodes directly, with today's process mark, marker and
  after-change bookkeeping (a raw-text system's bytes from 0x80 become raw-byte characters in a multibyte buffer, as
  today). Any other filter gets one string of at most `read-process-output-max` bytes and at most its estimated 4 ms of
  work (a moving average of its nanoseconds per byte; at least 512 bytes), and before a first call has measured it 4 KB
  on a pty, 64 KB on a pipe or socket: a cheap filter on a stream of small reads gets fewer, larger calls, and a costly
  one no longer call than today's. `nbytes_read` grows by the raw bytes, so `accept-process-output` keeps its meaning;
  the redisplay after each delivery stays. The C delivery runs in the watchdog's `deliver` phase and the filter in
  `filter`; `nidhogg-workers-stats`'s `:phase-ms` counts both for reader-owned processes. Adaptive read buffering
  (`process-adaptive-read-buffering`) does not apply to a reader-owned process. Test primitives:
  `nidhogg-workers--test-filter-cost` (fix a filter's estimated cost), `--test-io-info`'s `:filter-cost`.
- STARTTLS, ptys, comint and TRAMP under the reader: `gnutls-boot` (every STARTTLS upgrade: smtpmail, Gnus, ERC,
  `open-network-stream` with `:type` `starttls`, and a blocking `open-gnutls-stream`, which connects as a plain stream
  first) takes the socket back from `nh-io` before any GnuTLS call, and that process is never handed over again (also
  after `gnutls-deinit`). Bytes `nh-io` had read but not delivered go to GnuTLS, never to the filter (delivering them
  would be STARTTLS response injection): the session gets a pull function that returns them before it reads the socket,
  and TLS's pending check counts them. A pty's writes, `process-send-eof`, `set-process-window-size`,
  `interrupt-process` and `process-tty-name` use the real fd or the tty name as before; comint and TRAMP see the same
  results. Test primitives: `nidhogg-workers--test-io-info`'s `:tls-reclaimed`, `:tls-pulled`, `:tls-left` and
  `:tls-pull`.
<!-- wk: Task 21 -->
- JSON tape (`src/nidhogg-json-tape.c`): a tokenizer that copies json.c's rules byte for byte (strict UTF-8, surrogates,
  depth 10,000, integer overflow, floats parsed in the C locale, the same error symbol at the same position) and
  writes one malloc block (a tape of 16-byte entries, a string arena and a table with each distinct object key once),
  with no Lisp, so a worker can run it; the main thread materializes the tape into the objects `json-parse-string`
  and `json-parse-buffer` build. Test primitive: `(nidhogg-workers--test-tokenize STRING MODE &rest OPTIONS)`.
- JSON materializer slices: the main thread builds a message in slices (it looks at the clock every 4,096 entries and
  yields once the slice's budget is spent), in the watchdog phase `json`. Between slices the partial result, every open
  container's collected elements and the key cache live in a Lisp vector on a `staticpro`'d list, so GC may run;
  nothing is visible to Lisp until the message is complete. A slice left by a signal (`memory-full`) drops its state
  and the message becomes an error (`json-out-of-memory`). Each distinct key is interned once per message. Test
  primitives: `nidhogg-workers--test-json-start`, `-test-json-step`, `-test-json-in-flight`, and the variables
  `nidhogg-workers--test-slice-entries`, `-test-intern-count`, `-test-json-fail-at`.
<!-- wk: Task 24 -->
<!-- wk: Task 25 -->
<!-- wk: Task 26 -->
<!-- wk: Task 27 -->
**Image decoder cores.** PNG, JPEG, GIF and WebP decode through Lisp-free cores in `src/nidhogg-image-bg.c`, copies
of the loaders' decode paths behind the table `nh_img_decoders` of `src/nidhogg-image-bg.h`. With the flag the
synchronous loaders in `image.c` (Cairo frames) use the same cores, so a picture never depends on who asked first;
`nidhogg-workers--test-image-legacy` set to non-nil turns that off for a comparison. The cores keep their own animation
cache (by file name and modification time, or a hash of the data), so frame N of a GIF or WebP decodes from frame N-1.
Test primitives: `nidhogg-workers--test-image-decode`, `--test-image-decode-finish`, `--test-decode-count`,
`--test-image-cache-clear`, `--test-image-pixels`.
**Images decode on `nh-img` during redisplay.** With `nidhogg-workers-images` and `nidhogg-image-defer` on, a PNG,
JPEG, GIF or WebP that redisplay meets for the first time (on a Cairo frame) is decoded on `nh-img` (up to two
threads, one per two cores) from a copy of its `:data` or its file name, with the frame's and face's colours resolved
first. Redisplay waits for it within the shared wait budget; past it the image keeps its final size from the header
and shows the face's background (an empty rectangle) until it lands. The landing finishes it at the start of the next
redisplay, exactly as a synchronous load would, and redraws only the windows that show it. Clearing the image cache,
`image-flush`, eviction and deleting the frame drop a decode still running. A Lisp call that needs the pixels of an
image still decoding (`image-mask-p`) waits for it, or loads it itself after a second; `image-size` answers from the
header without waiting; display outside redisplay (`window-text-pixel-size`) and the tool bar load synchronously, as
today, and so does everything with either switch off. Watchdog phases: `image` (header, synchronous decodes, landing),
`worker-wait` with `display` and `image-pixels`; `:deferrals` counts the images that landed late. Test primitives:
`nidhogg-workers--test-await-landed`, `--test-marked-windows`, `--deferrable-p`, `--test-image-info`,
`--test-image-job`, `--test-image-forget`, `--test-image-land`, and `--test-image-pixels`' CACHED argument.
<!-- wk: Task 30 -->
<!-- wk: Task 31 -->
- Directory reads (`src/nidhogg-blk.c`, class `dir` of `nh-blk`): `(nidhogg-directory-files-async DIR CALLBACK &key
  match nosort id-format recursive max-depth max-entries skip)` reads a local directory on a worker (`lstat`,
  `readlink`, cached `getpwuid_r`/`getgrgid_r`) and calls `(CALLBACK BATCH DONE ERROR)` from the main loop with
  `(NAME . ATTRIBUTES)` lists exactly as `directory-files-and-attributes` returns them. Sorted results arrive in one
  batch, `:nosort` ones in batches of 1000. A name with a file name handler signals `nidhogg-not-local`.
- File blocks (`src/nidhogg-blk.c`, `src/nidhogg-decode.c`): `(nidhogg-read-file-async FILE CALLBACK &key beg end)`
  reads a byte range of a regular file on `nh-blk` and scans it (ASCII only; strictly valid UTF-8 with its character
  count; line-ending kind; null byte, ESC, BOM); `CALLBACK` gets `(BLOCK ERROR)`, BLOCK a `user-ptr` that owns the
  bytes. `(nidhogg-insert-file-block BLOCK)` chooses the coding on the main thread as `insert-file-contents` does and
  inserts with the worker's counts only for `undecided` with UTF-8 first and valid UTF-8 (no null, ESC, CR, BOM), or
  ASCII-only text under an ASCII-compatible coding; everything else goes through `decode_coding_gap`.
  `nidhogg-blk--insert-path` (`fast` or `decode`) is the test hook.
- Auto-save writes (`src/nidhogg-blk.c`, `nh-save`, one thread of its own): with `nidhogg-workers-autosave` on, an
  auto-save the keyboard loop triggers (after `auto-save-interval` events or `auto-save-timeout` idle seconds) copies
  each due buffer on the plain path (its auto-save file is not the visited file; no `write-region-annotate-functions`,
  formats, selective display or file name handler) and writes the copy on `nh-save`, encoded as `utf-8-emacs-unix`,
  into a new file in the same directory (an unnamed `O_TMPFILE` on Linux, else `.#NAME#.nh-PID-N`). At the next timer
  check the file is renamed onto the buffer's current `buffer-auto-save-file-name`, unless the job was superseded: a
  save of the buffer, `delete-file` of the name, `rename-file` of it to another name, a newer auto-save, or a
  synchronous one. `do-auto-save` from Lisp, `kill-emacs`, the fatal path and the tty emergency escape stay
  synchronous and also write any buffer whose job has not landed; `kill-buffer` lands a finished job, or writes a
  running one's auto-save, before it asks about the auto-save file. Hooks, counters, messages and `recent-auto-save-p`
  are as before. Within the contract: the file reaches the disk a few ms after "Auto-saving...done" and replaces the
  old one by a rename; a write error shows the usual warning at that point; while a buffer's previous job still runs
  its next auto-save is written synchronously with a warning, and a job running over 60 s turns auto-saving off for
  that buffer for 20 minutes, as a 60 s write does. Test primitives: `nidhogg-workers--test-keyboard-auto-save`,
  `--test-save-await`, `--test-save-jobs`, `--test-save-age`, `--test-save-encode`, `nidhogg-workers--save-unnamed`
  and the `before-write` pause point.
<!-- wk: Task 35 -->
<!-- wk: Task 36 -->
<!-- wk: Task 37 -->
<!-- wk: Task 38 -->

## Reporting issues

- Problems specific to Nidhogg (its patches, build, or `nidhogg-version`) go to this repository's issue tracker.
- Nidhogg's maintainers never forward reports to GNU, and nothing in Nidhogg sends anything upstream. If you reproduce
  a bug in a stock GNU Emacs build (not Nidhogg), you may report it to GNU yourself, in your own words, following
  GNU's own guidelines.

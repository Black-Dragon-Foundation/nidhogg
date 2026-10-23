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
- `bash nidhogg/test/tree-test.sh [BASE]` checks these rules.

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
- Configure flags: `--with-native-compilation=aot --with-tree-sitter --with-pgtk` (Linux), `--with-ns ...` (macOS).
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
- `drgnx-linux` (fedora-45 tarball) and `drgnx-macos`: `nidhogg/ci/drgnx-suite.sh TARBALL DISTRO DRGNX_DIR` runs drgnx's
  unit suite with `DRGNX_EMACS` set to the built Nidhogg (the `:timing` tests in a second run). Locally, a narrowed run:
  `DRGNX_TEST_SELECTOR='drgnx-doctor/' bash nidhogg/ci/drgnx-suite.sh TARBALL fedora-44 ../drgnx`.
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
and `nidhogg-watchdog-hang-ms` (2000) and one repeating 1 s idle timer. The timer's drain returns at once (no
allocation) when nothing is pending; otherwise it takes the rings at once and resolves records within 2 ms, the rest
waiting for the next idle period (`nidhogg-watchdog-drain-now` drains everything). Each stall is one JSON line (schema
`v` 1) in `YYYY-MM-DD.jsonl` under `nidhogg-watchdog-log-directory` (default `~/.emacs.d/watchdog/`); `nidhogg-watchdog-record-functions` sees each
resolved record. At most 5,000 stall lines are written per day (the rest are only counted), day files older than 14
days are deleted, and a `summary` line (histogram deltas, counters, monitor CPU, drain p95; `nidhogg-watchdog-stats`
adds the drain's histogram) is appended at most every 10 minutes and when the mode is turned off or Emacs exits.
<!-- wd: Task 15 -->
<!-- wd: Task 16 -->
<!-- wd: Task 17 -->
<!-- wd: Task 18 -->

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
<!-- wk: Task 10 -->
<!-- wk: Task 11 -->
<!-- wk: Task 12 -->
<!-- wk: Task 13 -->
<!-- wk: Task 14 -->
<!-- wk: Task 15 -->
<!-- wk: Task 16 -->
<!-- wk: Task 17 -->
<!-- wk: Task 18 -->
<!-- wk: Task 19 -->
<!-- wk: Task 20 -->
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
<!-- wk: Task 29 -->
<!-- wk: Task 30 -->
<!-- wk: Task 31 -->
<!-- wk: Task 32 -->
<!-- wk: Task 33 -->
<!-- wk: Task 34 -->
<!-- wk: Task 35 -->
<!-- wk: Task 36 -->
<!-- wk: Task 37 -->
<!-- wk: Task 38 -->

## Reporting issues

- Problems specific to Nidhogg (its patches, build, or `nidhogg-version`) go to this repository's issue tracker.
- Nidhogg's maintainers never forward reports to GNU, and nothing in Nidhogg sends anything upstream. If you reproduce
  a bug in a stock GNU Emacs build (not Nidhogg), you may report it to GNU yourself, in your own words, following
  GNU's own guidelines.

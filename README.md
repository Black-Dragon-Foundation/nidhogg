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

## Building

```bash
nidhogg/build.sh (--container DISTRO | --host) [--prefix DIR] [--jobs N] [--memory SIZE]
                 [--build-dir DIR] [--check FILE] [--dry-run]
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

## Reporting issues

- Problems specific to Nidhogg (its patches, build, or `nidhogg-version`) go to this repository's issue tracker.
- If a bug also reproduces in GNU Emacs, report it upstream with `M-x report-emacs-bug`.

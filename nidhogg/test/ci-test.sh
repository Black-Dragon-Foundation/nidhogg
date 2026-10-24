#!/usr/bin/env bash
# The CI workflow keeps its rules.  Usage: nidhogg/test/ci-test.sh   Exit 0 ok, 1 with "CI FAIL: <why>".
set -u
cd "$(git rev-parse --show-toplevel)" || exit 1
rc=0
fail() { echo "CI FAIL: $*"; rc=1; }
wf=.github/workflows/build.yml
test -f "$wf"                                                     || { fail "no $wf"; exit 1; }
for j in tree linux macos drgnx-linux drgnx-macos release; do grep -qE "^  $j:" "$wf" || fail "no job $j"; done
# drgnx's tests gate a release
awk '/^  release:/,0' "$wf" | grep -qE 'needs: \[tree, linux, macos, drgnx-linux, drgnx-macos\]' || fail "release not gated"
# a release only from a pushed v* tag, never from workflow_dispatch or a pull request
awk '/^  release:/,0' "$wf" | grep -qF "if: github.event_name == 'push' && startsWith(github.ref, 'refs/tags/v')" || fail "release not restricted to a pushed v* tag"
# pinned actions, podman only, builds only through build.sh
! grep -E 'uses: [^ ]+@' "$wf" | grep -vqE '@[0-9a-f]{40} # v'   || fail "unpinned action"
! grep -qw docker "$wf"                                           || fail "docker in $wf"
! grep -qE '\./configure|make -j' "$wf"                           || fail "a build outside build.sh"
grep -qE '^permissions:' "$wf"                                    || fail "no top-level permissions"
# every cache step keys on its inputs; the tarball cache is exact (no restore-keys) and never for a release tag
n=$(grep -c 'uses: actions/cache/restore@' "$wf")
[ "$n" -ge 7 ]                                                    || fail "expected the cache restore steps, found $n"
# caches are saved right after the build, not by the job's post step: no plain actions/cache, a save step for each restore
! grep -q 'uses: actions/cache@' "$wf"                            || fail "a plain actions/cache step (saves only at the end of a successful job)"
[ "$(grep -c 'uses: actions/cache/save@' "$wf")" -ge 7 ]          || fail "expected the cache save steps"
bad=$(grep -A6 'uses: actions/cache/' "$wf" | grep -E '^[ -]*key: ' | grep -vE 'hashFiles|tree\.outputs|lisp\.outputs' || true)
[ -z "$bad" ]                                                     || fail "a cache key without a hash of its inputs: $bad"
grep -A8 'id: tb' "$wf" | grep -q "startsWith(github.ref, 'refs/tags/')" || fail "the tarball cache is used for a release tag"
! grep -A12 'id: tb' "$wf" | grep -q restore-keys                 || fail "the tarball cache has restore-keys"
# the compiled-Lisp cache is exact-key (no restore-keys: a partly stale set is not rebuilt correctly)
! grep -A12 'id: lc' "$wf" | grep -q restore-keys                 || fail "the compiled-Lisp cache has restore-keys"
# every cache save comes after its build and only when the build succeeded
[ "$(grep -c "steps.build.outcome == 'success'" "$wf")" -ge 6 ]  || fail "a cache save not gated on the build step"
# the tree job runs Glitnir's checks and cargo-deny's advisories, with pixman's headers installed
tj=$(awk '/^  tree:/,/^  linux:/' "$wf")
grep -qF 'bash nidhogg/glitnir/check.sh' <<<"$tj"                 || fail "the tree job does not run glitnir/check.sh"
grep -qF 'cargo deny check advisories' <<<"$tj"                   || fail "the tree job does not run cargo deny check advisories"
grep -qF 'libpixman-1-dev' <<<"$tj"                               || fail "the tree job does not install libpixman-1-dev"
grep -qE 'sha256sum -c' <<<"$tj"                                  || fail "the tree job does not check cargo-deny's sha256"
# the build without Glitnir (no Rust installed) still compiles
nj=$(awk '/^  compile-noglitnir:/,/^  macos:/' "$wf")
[ -n "$nj" ]                                                      || fail "no job compile-noglitnir"
grep -qF -- '--without-glitnir' <<<"$nj"                          || fail "compile-noglitnir does not pass --without-glitnir"
grep -qF -- '--stage build' <<<"$nj"                              || fail "compile-noglitnir does not stop after the build stage"
grep -qF 'fedora-44' <<<"$nj"                                     || fail "compile-noglitnir is not on fedora-44"
# macOS runs the crate's tests with the host's cargo (Homebrew rust), before the build: the aarch64 NEON baseline
mj=$(awk '/^  macos:/,/^  drgnx-linux:/' "$wf")
grep -qF 'bash nidhogg/glitnir/check.sh' <<<"$mj"                 || fail "macos runs the glitnir checks"
[ "$(grep -n 'glitnir/check.sh' <<<"$mj" | head -1 | cut -d: -f1)" -lt "$(grep -n 'id: build' <<<"$mj" | head -1 | cut -d: -f1)" ] 2>/dev/null || fail "macos runs the glitnir checks after the build"
exit $rc

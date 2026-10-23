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
# the workers' jobs (engine C, PD-4): present, after linux, outside release's needs, never fail-fast, pkg suites on and off
job() { awk -v j="  $1:" '$0==j{p=1;print;next} p&&/^  [a-z]/{exit} p' "$wf"; }
for j in workers-harness workers-asan workers-pkg; do
  job "$j" | grep -qE '^    needs: linux'                          || fail "no job $j (or it does not need linux)"
  ! job "$j" | grep -q 'fail-fast: true'                            || fail "a workers job has fail-fast: $j"
  ! awk '/^  release:/,0' "$wf" | grep -qE "needs:.*$j"             || fail "release needs $j"
done
job workers-pkg | grep -q 'fail-fast: false'                       || fail "workers-pkg has no fail-fast: false"
{ job workers-pkg | grep -qE 'mode: \["on", "off"\]' && grep -q 'pkg-test.sh --compare' "$wf"; } || fail "workers-pkg does not run on and off and compare"
job workers-asan | grep -q 'NIDHOGG_BUILD_SANITIZE'                || fail "workers-asan does not build sanitized"
# pkg-test.sh --compare: an on-only failure fails, a failure in both passes, an expected failure passes
d=$(mktemp -d)
printf 'a\tpassed\nb\tfailed\nc\tfailed\nd\tfailed\n' > "$d/pkg-x-on.outcomes"
printf 'a\tpassed\nb\tpassed\nc\tfailed\nd\tpassed\n' > "$d/pkg-x-off.outcomes"
printf '# c\nb\treason\towner\n' > "$d/exp"
: > "$d/none"
! bash nidhogg/test/pkg-test.sh --compare "$d" --expected "$d/none" >/dev/null || fail "compare passed an on-only failure"
printf 'a\tpassed\nb\tfailed\nc\tfailed\n' > "$d/pkg-x-on.outcomes"
bash nidhogg/test/pkg-test.sh --compare "$d" --expected "$d/exp" >/dev/null    || fail "compare failed a failure in both modes or an expected failure"
rm -r "$d"
exit $rc

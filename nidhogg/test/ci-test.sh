#!/usr/bin/env bash
# The CI workflow keeps its rules.  Usage: nidhogg/test/ci-test.sh   Exit 0 ok, 1 with "CI FAIL: <why>".
set -u
cd "$(git rev-parse --show-toplevel)" || exit 1
rc=0
fail() { echo "CI FAIL: $*"; rc=1; }
wf=.github/workflows/build.yml
test -f "$wf"                                                     || { fail "no $wf"; exit 1; }
for j in tree linux macos drgnx release; do grep -qE "^  $j:" "$wf" || fail "no job $j"; done
# drgnx's tests gate a release
awk '/^  release:/,0' "$wf" | grep -qE 'needs: \[tree, linux, macos, drgnx, native-crates, native-gui\]' || fail "release not gated"
# the timing lane is report-only: its own job that never fails the run, and no release needs it
grep -qE "^  timing:" "$wf"                                       || fail "no job timing"
awk '/^  timing:/,/^  release:/' "$wf" | grep -qE '^    continue-on-error: true$' || fail "the timing job is not continue-on-error: true"
awk '/^  release:/,0' "$wf" | grep -E '^    needs:' | grep -qw timing && fail "release needs the timing job"
grep -qE '^    needs: .*timing' "$wf"                            && fail "a job needs the timing job"
# drgnx's tiers: drgnx's own workflow, at one pinned commit that is also the default drgnx_ref, on this run's tarballs
pin=$(grep -oE 'uses: Black-Dragon-Foundation/drgnx/\.github/workflows/drgnx-tiers\.yml@[0-9a-f]{40}' "$wf" | sed 's/.*@//')
[ -n "$pin" ]                                                     || fail "drgnx's tiers are not called at a pinned commit"
! grep -qF "inputs.drgnx_ref || '" "$wf"                          || fail "a literal default drgnx_ref (the uses line is the one pin)"
grep -qE '^  pin:' "$wf"                                          || fail "no pin job"
grep -qF 'inputs.drgnx_ref || needs.pin.outputs.ref' "$wf"        || fail "the default drgnx_ref is not the pin job's output"
# cache budget: only the default branch saves, so ci/* branches and tags restore only
[ "$(grep -c 'uses: actions/cache/save@' "$wf")" -eq "$(grep -c "github.ref == 'refs/heads/nidhogg-31'" "$wf")" ] || fail "a cache save step not limited to nidhogg-31"
grep -qF 'nidhogg: artifact' "$wf"                                || fail "drgnx's tiers do not run on this run's tarballs"
grep -qF "'unit no-lisp-writes ui macos'" "$wf"                   || fail "drgnx's tiers are not unit, gate mode, GUI and macOS"
# a release only from a pushed v* tag, never from workflow_dispatch or a pull request
awk '/^  release:/,0' "$wf" | grep -qF "if: github.event_name == 'push' && startsWith(github.ref, 'refs/tags/v')" || fail "release not restricted to a pushed v* tag"
# pinned actions, podman only, builds only through build.sh
! grep -E 'uses: [^ ]+@' "$wf" | grep -v 'drgnx-tiers\.yml@' | grep -vqE '@[0-9a-f]{40} # v' || fail "unpinned action"
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
mj=$(awk '/^  macos:/,/^  drgnx:/' "$wf")
grep -qF 'bash nidhogg/glitnir/check.sh' <<<"$mj"                 || fail "macos runs the glitnir checks"
[ "$(grep -n 'glitnir/check.sh' <<<"$mj" | head -1 | cut -d: -f1)" -lt "$(grep -n 'id: build' <<<"$mj" | head -1 | cut -d: -f1)" ] 2>/dev/null || fail "macos runs the glitnir checks after the build"
# one Linux path: the shell is build.sh's default, so no job names a toolkit, and nothing else is packed as a tarball
! grep -qiE 'pgtk|--nidhogg-wayland' "$wf"                        || fail "a job names pgtk or a window system of its own"
[ "$(grep -cE '^ +name: tarball-\$\{\{ matrix\.distro \}\}$|^ +name: tarball-macos$' "$wf")" -eq 2 ] || fail "a tarball artifact release would publish by mistake"
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
# the native shell: the crates' checks on the recorder, every GUI lane on the fedora-45 tarball
for s in 'recorder/check.sh --container fedora-45 --build' 'shell/check.sh --container fedora-45' 'glue/check.sh --container fedora-45'; do
  job native-crates | grep -qF "$s"                                || fail "native-crates does not run $s"
done
job native-gui | grep -qE '^    needs: \[linux, native-crates\]'   || fail "native-gui does not need linux and native-crates"
job native-gui | grep -q 'fail-fast: false'                        || fail "native-gui has no fail-fast: false"
job native-gui | grep -qE 'timeout-minutes: 60'                    || fail "native-gui has no 60-minute limit"
job native-gui | grep -qF 'tarball-fedora-45'                      || fail "native-gui does not use the fedora-45 tarball"
job native-gui | grep -qF -- '--container fedora-45'               || fail "native-gui does not run in a container"
job native-gui | grep -qF 'nidhogg/test/native-gui.sh'             || fail "native-gui does not run native-gui.sh"
# every lane in the matrix is a lane of the script and the other way round, and a lane has a floor: none is green on 0 rows
ci_lanes=$(job native-gui | sed -nE 's/^          - ([a-z0-9-]+)$/\1/p' | sort)
sh_lanes=$(sed -nE 's/^  ([a-z0-9-]+)\) +min=[1-9][0-9]*;.*/\1/p' nidhogg/test/native-gui.sh | sort)
[ -n "$ci_lanes" ] && [ "$ci_lanes" = "$sh_lanes" ]                || fail "native-gui's matrix and native-gui.sh's lanes (with a floor) differ"
grep -qF 'expected at least $min' nidhogg/test/native-gui.sh       || fail "native-gui.sh does not fail below a lane's floor"
# a release tag runs what release needs: the tagged commit passed the rest in its round
for j in compile-noglitnir workers-harness workers-asan workers-pkg workers-pkg-compare; do
  job "$j" | grep -qF "github.ref_type != 'tag'"                    || fail "$j runs for a release tag"
done
for j in tree linux macos drgnx native-crates native-gui; do
  ! job "$j" | grep -qF "github.ref_type != 'tag'"                  || fail "$j, which release needs, is skipped for a tag"
done
# pkg-test.sh --compare: an on-only failure fails, a failure in both passes, an expected failure passes
d=$(mktemp -d)
printf 'a\tpassed\nb\tfailed\nc\tfailed\nd\tfailed\n' > "$d/pkg-x-on.outcomes"
printf 'a\tpassed\nb\tpassed\nc\tfailed\nd\tpassed\n' > "$d/pkg-x-off.outcomes"
printf '# c\nb\treason\towner\n' > "$d/exp"
: > "$d/none"
! bash nidhogg/test/pkg-test.sh --compare "$d" --expected "$d/none" >/dev/null || fail "compare passed an on-only failure"
printf 'a\tpassed\nb\tfailed\nc\tfailed\nd\tpassed\n' > "$d/pkg-x-on.outcomes"
bash nidhogg/test/pkg-test.sh --compare "$d" --expected "$d/exp" >/dev/null    || fail "compare failed a failure in both modes or an expected failure"
# a test with an outcome in one mode only (a killed run) fails, whichever mode lost it
printf 'a\tpassed\nb\tfailed\n' > "$d/pkg-x-on.outcomes"
! bash nidhogg/test/pkg-test.sh --compare "$d" --expected "$d/exp" >/dev/null  || fail "compare passed a test missing from the on run"
printf 'a\tpassed\nb\tfailed\nc\tfailed\n' > "$d/pkg-x-on.outcomes"; printf 'a\tpassed\n' > "$d/pkg-x-off.outcomes"
! bash nidhogg/test/pkg-test.sh --compare "$d" --expected "$d/exp" >/dev/null  || fail "compare passed tests missing from the off run"
rm -r "$d"
exit $rc

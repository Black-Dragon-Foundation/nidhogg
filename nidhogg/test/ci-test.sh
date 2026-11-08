#!/usr/bin/env bash
# The CI workflow keeps its rules.  Usage: nidhogg/test/ci-test.sh   Exit 0 ok, 1 with "CI FAIL: <why>".
set -u
cd "$(git rev-parse --show-toplevel)" || exit 1
rc=0
fail() { echo "CI FAIL: $*"; rc=1; }
wf=.github/workflows/build.yml
test -f "$wf"                                                     || { fail "no $wf"; exit 1; }
job() { awk -v j="  $1:" '$0==j{p=1;print;next} p&&/^  [a-z]/{exit} p' "$wf"; }   # the text of job $1
for j in tree linux macos drgnx release; do grep -qE "^  $j:" "$wf" || fail "no job $j"; done
# drgnx's tests gate a release
awk '/^  release:/,0' "$wf" | grep -qE 'needs: \[tree, vendor, linux, macos, drgnx, native-crates, native-gui\]' || fail "release not gated"
# the vendored crates are not in git (Cargo.lock is the pin): a job builds nidhogg-vendor-<version>.tar.zst from the
# locks, never sees a token, and the release publishes it
grep -qE "^  vendor:" "$wf"                                       || fail "no job vendor"
job vendor | grep -qF 'ci/vendor.sh make'                         || fail "the vendor job does not build the tarball"
job vendor | grep -qF 'ci/vendor.sh check'                        || fail "the vendor job does not check the tarball against the locks"
! job vendor | grep -qE 'GH_TOKEN|github\.token|secrets\.'        || fail "the vendor job sees a token"
awk '/^  release:/,0' "$wf" | grep -qF 'name: vendor-crates'      || fail "the release does not publish the vendor tarball"
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
# cache writes: every save step is CACHE_SAVE's (nidhogg-31 and this repository's pull requests) or CACHE_SAVE_ALL's
# (nidhogg-31), and neither is ever true for a ci/* branch, a fork's pull request or a tag; a pull request's entries
# are scoped to its merge ref, so no other ref's run reads them, it saves no large prefix-restored entry, and its
# earlier runs' entries are deleted (pr-caches)
for l in $(grep -n 'uses: actions/cache/save@' "$wf" | cut -d: -f1); do
  sed -n "$((l - 2)),$((l - 1))p" "$wf" | grep -qE "if: .*env\.CACHE_SAVE(_ALL)? == 'true'" || fail "the cache save at line $l is not under CACHE_SAVE or CACHE_SAVE_ALL"
done
own_pr="(github.event_name == 'pull_request' && github.event.pull_request.head.repo.full_name == github.repository)"
grep -qxF "  CACHE_SAVE: \${{ $own_pr || (github.event_name != 'pull_request' && github.ref == 'refs/heads/nidhogg-31') }}" "$wf" || fail "CACHE_SAVE is not nidhogg-31's and this repository's pull requests'"
job pr-caches | grep -qF "if: \${{ !cancelled() && github.event_name == 'pull_request' && github.event.pull_request.head.repo.full_name == github.repository }}" || fail "pr-caches does not run for this repository's pull requests only"
job pr-caches | grep -qF 'select(.created_at < \"$start\")' || fail "pr-caches does not delete only the entries of earlier runs"
grep -qxF "  CACHE_SAVE_ALL: \${{ github.event_name != 'pull_request' && github.ref == 'refs/heads/nidhogg-31' }}" "$wf" || fail "CACHE_SAVE_ALL is not nidhogg-31's"
[ "$(job native-crates | grep -cF "env.CACHE_SAVE == 'true' && github.event_name != 'pull_request'")" = 2 ] || fail "a pull request saves the native crates' entry"
# a release tag's tarball has no restored compiled Lisp: the jobs that make tarballs skip the restore for a tag
for j in linux macos; do
  job "$j" | grep -A1 'id: lc$' | grep -qF "!startsWith(github.ref, 'refs/tags/')" || fail "$j restores compiled Lisp for a tag"
done
# the path filter: its self-test and the table's bite cases, a changes job every other job waits on, and a gate that
# always runs, needs every job but the report-only timing lane, and is named partial for a narrowed run
bash nidhogg/ci/changes.sh nidhogg/ci/paths.txt --self-test >/dev/null || fail "nidhogg/ci/changes.sh --self-test"
tbl_jobs=$(sed -n 's/^jobs: //p' nidhogg/ci/paths.txt)
for j in $tbl_jobs; do
  awk -v j="  $j:" '$0==j{p=1;next} p&&/^  [a-z]/{exit} p' "$wf" | grep -qF "needs.changes.outputs.$j != 'false'" || fail "job $j does not follow the changes job"
  grep -qF "      $j: \${{ steps.c.outputs.run_$(echo "$j" | tr - _) }}" "$wf" || fail "the changes job has no output $j"
done
gate=$(awk '/^  gate:/{p=1;print;next} p&&/^  [a-z]/{exit} p' "$wf")
grep -qF "name: \${{ needs.changes.outputs.partial == 'true' && 'Nidhogg gate (partial)' || 'Nidhogg gate' }}" <<<"$gate" || fail "the gate is not named partial for a narrowed run"
grep -qE '^    if: always\(\)$' <<<"$gate"                       || fail "the gate does not always run"
gneeds=$(sed -nE 's/^    needs: \[(.*)\]$/\1/p' <<<"$gate" | tr -d ' ' | tr , '\n' | sort)
jobs_all=$(awk '/^jobs:/{p=1;next} p' "$wf" | sed -nE 's/^  ([a-z0-9-]+):$/\1/p' | grep -vxE 'gate|release|timing|pr-caches|images' | sort)
[ "$gneeds" = "$jobs_all" ]                                       || fail "the gate does not need every job but release, timing, pr-caches and images"
grep -qF 'GATE: Nidhogg gate' "$wf"                               || fail "the changes job does not check the base's Nidhogg gate"
# a pull request tests its head, which a fast-forward lands, never GitHub's merge commit: every checkout and the path
# filter name the head, and a job (allowed to skip off pull requests) fails a head that does not descend from the base
[ "$(grep -cF 'uses: actions/checkout@' "$wf")" = "$(grep -cxF '          ref: ${{ github.event.pull_request.head.sha }}' "$wf")" ] ||
  fail "a checkout does not take a pull request's head"
grep -qF 'SHA: ${{ github.event.pull_request.head.sha || github.sha }}' "$wf" || fail "the path filter diffs a pull request's merge commit"
job fast-forward | grep -qF "if: github.event_name == 'pull_request'" || fail "no fast-forward job for pull requests"
job fast-forward | grep -qF 'gh api "repos/$REPO/compare/$tip...$HEAD_SHA"' || fail "the fast-forward job does not compare the base's tip with the head"
job fast-forward | grep -qF 'ahead|identical)' || fail "the fast-forward job does not require the head to be ahead of the base"
! job fast-forward | grep -qF 'actions/checkout@' || fail "the fast-forward job checks out the history (the compare API needs none)"
grep -qF '[ "$PR" = true ] || allow="$allow fast-forward "' <<<"$gate" || fail "the gate does not let fast-forward skip off pull requests"
# every job that restores a build cache gives the files their last commit's time; the cargo targets' entry names the
# commit it was built from, and what changed since then is stamped newer than it
for j in linux compile-noglitnir macos native-crates workers-asan workers-chaos; do
  job "$j" | grep -qF 'bash nidhogg/ci/set-mtimes.sh' || fail "job $j does not set the file times"
done
job native-crates | grep -qF 'set-mtimes.sh --since "$RUNNER_TEMP/nh-recorder-target/.nidhogg-source-commit"' || fail "native-crates does not stamp what changed since its cache"
job native-crates | grep -qF 'if [ "$BUILT" = true ]; then git rev-parse HEAD > "$RUNNER_TEMP/nh-recorder-target/.nidhogg-source-commit"; fi' || fail "native-crates' cache does not name its commit, or names it after a red check"
job native-crates | grep -qF "BUILT: \${{ steps.recorder.outcome == 'success' && steps.shell.outcome == 'success' && steps.glue.outcome == 'success' }}" || fail "native-crates names its commit without all three checks green"
grep -qF 'nidhogg: artifact' "$wf"                              || fail "drgnx's tiers do not run on this run's tarballs"
job changes | grep -qF '[ -n "$t" ] || t="unit no-lisp-writes ui macos"' || fail "drgnx's tiers are not unit, gate mode, GUI and macOS"
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
step() { awk -v l="$1" 'NR>l && /^      - /{exit} NR>=l' "$wf"; }   # the step from line $1 to the next one
for l in $(grep -nE 'id: (tb|lc|ln)$' "$wf" | cut -d: -f1); do
  # the tarball and compiled-Lisp caches are exact-key (no restore-keys: a partly stale set is not rebuilt correctly)
  ! step "$l" | grep -q 'restore-keys'                            || fail "the exact cache at line $l has restore-keys"
done
# a compiled-Lisp or tarball save comes after a build (or pack) that passed; a tarball from narrowed checks is never saved
[ "$(grep -A1 -E 'name: Save ((byte-|native-)?compiled Lisp|built tarball)$' "$wf" | grep -cE "steps\.(build|pack)\.outcome == 'success'|steps\.lcok\.outputs\.ok == 'true'")" -eq "$(grep -cE 'name: Save ((byte-|native-)?compiled Lisp|built tarball)$' "$wf")" ] || fail "a compiled-Lisp or tarball save not gated on its build"
# the dev builds' byte-compiled Lisp is keyed on the Lisp side alone, their .eln files on everything
for j in workers-asan workers-chaos; do
  job "$j" | grep -qF 'elc=$(bash nidhogg/ci/lisp-key.sh --elc)' || fail "job $j does not key its byte-compiled Lisp apart"
  [ "$(job "$j" | grep -cF -- "-eln-\${{ hashFiles('nidhogg/deps/fedora-45.build') }}-\${{ steps.tree.outputs.lisp }}-")" = 2 ] || fail "job $j's .eln entry is not keyed on all of lisp-key.sh"
done
[ "$(grep -A1 'name: Save built tarball$' "$wf" | grep -cF "inputs.checks == ''")" -eq "$(grep -c 'name: Save built tarball$' "$wf")" ] || fail "a narrowed run saves its tarball"
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
grep -qF 'fedora-45' <<<"$nj"                                     || fail "compile-noglitnir is not on fedora-45"
grep -qF 'distro: [fedora-45, ubuntu-26.04]' "$wf"                || fail "the Linux matrix is not fedora-45 and ubuntu-26.04"
! grep -qF 'fedora-44' "$wf"                                      || fail "the workflow still names fedora-44"
# macOS runs the crate's tests with the host's cargo (Homebrew rust), before the build: the aarch64 NEON baseline
mj=$(awk '/^  macos:/,/^  drgnx:/' "$wf")
grep -qF 'bash nidhogg/glitnir/check.sh' <<<"$mj"                 || fail "macos runs the glitnir checks"
[ "$(grep -n 'glitnir/check.sh' <<<"$mj" | head -1 | cut -d: -f1)" -lt "$(grep -n 'id: build' <<<"$mj" | head -1 | cut -d: -f1)" ] 2>/dev/null || fail "macos runs the glitnir checks after the build"
# one Linux path: the shell is build.sh's default, so no job names a toolkit, and nothing else is packed as a tarball
! grep -qiE 'pgtk|--nidhogg-wayland' "$wf"                        || fail "a job names pgtk or a window system of its own"
[ "$(grep -cE '^ +name: tarball-\$\{\{ matrix\.distro \}\}$|^ +name: tarball-macos$' "$wf")" -eq 2 ] || fail "a tarball artifact release would publish by mistake"
# the workers' jobs (engine C, PD-4): present, after linux, outside release's needs, never fail-fast, pkg suites on and off
for j in workers-harness workers-asan workers-pkg; do
  job "$j" | grep -qE '^    needs: \[changes, images, linux\]'     || fail "no job $j (or it does not need linux)"
  ! job "$j" | grep -q 'fail-fast: true'                            || fail "a workers job has fail-fast: $j"
  ! awk '/^  release:/,0' "$wf" | grep -qE "needs:.*$j"             || fail "release needs $j"
done
job workers-pkg | grep -q 'fail-fast: false'                       || fail "workers-pkg has no fail-fast: false"
{ job workers-pkg | grep -qE 'mode: \["on", "off"\]' && grep -q 'pkg-test.sh --compare' "$wf"; } || fail "workers-pkg does not run on and off and compare"
job workers-asan | grep -q 'NIDHOGG_BUILD_SANITIZE'                || fail "workers-asan does not build sanitized"
# the workers files under chaos: a chaos build of its own beside the Linux builds, seeds 1 to 5, outside release's needs
job workers-chaos | grep -qE '^    needs: \[changes, images\]$'      || fail "no job workers-chaos (or it waits on more than the changes and images jobs)"
job workers-chaos | grep -qF 'build.sh --container fedora-45 --dev --chaos --stage build' || fail "workers-chaos does not make a chaos build"
job workers-chaos | grep -qF 'nidhogg/test/chaos-lane.sh --build-dir "$RUNNER_TEMP/build-chaos" --seeds "1 2 3 4 5" --' || fail "workers-chaos does not run chaos-lane.sh with seeds 1 to 5"
! awk '/^  release:/,0' "$wf" | grep -qE 'needs:.*workers-chaos'   || fail "release needs workers-chaos"
# chaos-lane.sh lets no rerun off (a first-run failure is red though the rerun passed and flaky.txt lists it), a file
# that ran no test is red, and a red names its seed (a stand-in for build.sh's check stage)
d=$(mktemp -d)
cat > "$d/build" <<'STUB'
while [ $# -gt 0 ]; do case $1 in --build-dir) b=$2; shift 2 ;; --check) l=$2; shift 2 ;; *) shift ;; esac; done
for t in $(sed 's/#.*//' "$l"); do
  mkdir -p "$b/check-logs/$(dirname "$t")"
  case $NIDHOGG_TEST_SEED in
    2) echo 'Ran 3 tests, 2 results as expected, 1 unexpected' ;;
    3) echo 'Ran 0 tests, 0 results as expected, 0 unexpected' ;;
    *) echo 'Ran 3 tests, 3 results as expected, 0 unexpected' ;;
  esac > "$b/check-logs/$t.log"
done
[ "$NIDHOGG_TEST_SEED" != 2 ] || printf 'check-rerun: t had unexpected results; running it once more\nflaky (listed: a cause): t\n'
STUB
lane() { env -u GITHUB_ACTIONS bash nidhogg/test/chaos-lane.sh --build-dir "$d/b" --seeds "$1" -- bash "$d/build"; }
lane 1 >/dev/null                                                   || fail "chaos-lane.sh: a green seed is red"
o=$(lane "1 2")                                                     && fail "chaos-lane.sh passed a failure that the rerun let off"
{ grep -q 'RED seed 2' <<<"$o" && ! grep -q 'RED seed 1' <<<"$o"; } || fail "chaos-lane.sh does not name the red seed"
lane 3 >/dev/null                                                   && fail "chaos-lane.sh passed a seed that ran no test"
rm -r "$d"
# the native shell: the crates' checks on the recorder, every GUI lane on the fedora-45 tarball
for s in 'recorder/check.sh --container fedora-45 --build' 'shell/check.sh --container fedora-45' 'glue/check.sh --container fedora-45'; do
  job native-crates | grep -qF "$s"                                || fail "native-crates does not run $s"
done
job native-gui | grep -qE '^    needs: \[changes, images, linux, native-crates\]' || fail "native-gui does not need linux and native-crates"
job native-gui | grep -q 'fail-fast: false'                        || fail "native-gui has no fail-fast: false"
job native-gui | grep -qE 'timeout-minutes: 60'                    || fail "native-gui has no 60-minute limit"
job native-gui | grep -qF 'tarball-fedora-45'                      || fail "native-gui does not use the fedora-45 tarball"
job native-gui | grep -qF -- '--container fedora-45'               || fail "native-gui does not run in a container"
job native-gui | grep -qF 'nidhogg/test/native-gui.sh'             || fail "native-gui does not run native-gui.sh"
# the matrix is the script's lanes, each with a floor (none is green on 0 rows): the changes job reads them from it
job native-gui | grep -qF 'lane: ${{ fromJSON(needs.changes.outputs.lanes) }}' || fail "native-gui's matrix is not the changes job's lanes"
grep -qF "sed -nE 's/^  ([a-z0-9-]+)\\) +min=[1-9][0-9]*;.*/\\1/p' nidhogg/test/native-gui.sh" "$wf" || fail "the changes job does not read the lanes (with a floor) from native-gui.sh"
sh_lanes=$(sed -nE 's/^  ([a-z0-9-]+)\) +min=[1-9][0-9]*;.*/\1/p' nidhogg/test/native-gui.sh | sort)
[ "$(echo "$sh_lanes" | wc -l)" -ge 10 ]                           || fail "native-gui.sh has fewer than 10 lanes with a floor"
grep -qF 'expected at least $min' nidhogg/test/native-gui.sh       || fail "native-gui.sh does not fail below a lane's floor"
# the pre-flight leaves the shell's files to the check subset and these lanes: each is in one of them
for f in test/src/nidhogg-tests/nhwl-*-tests.el; do
  n=$(basename "$f" .el)
  case $n in
    nhwl-gui-*|nhwl-gui) grep -qF "'nhwl-gui*-tests.el'" nidhogg/test/native-gui.sh || fail "$n is in no GUI lane" ;;
    nhwl-ime-tests) grep -qF 'ime-matrix.sh' nidhogg/test/native-gui.sh || fail "$n is in no GUI lane" ;;
    *) grep -qx "src/nidhogg-tests/$n" nidhogg/check-subset.txt || fail "$n is in no check and no lane" ;;
  esac
done
# a release tag runs what release needs: the tagged commit passed the rest in its round
for j in compile-noglitnir workers-harness workers-asan workers-chaos workers-pkg workers-pkg-compare; do
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
# a chaos build's seed reaches the check container (the pre-flight's P3 runs seeds 1..N); none set, none exported
NIDHOGG_TEST_SEED=7 bash nidhogg/build.sh --container fedora-45 --dev --chaos --prefix /tmp/p --dry-run 2>/dev/null \
  | grep -qxF 'export NIDHOGG_TEST_SEED=7'                         || fail "build.sh does not pass NIDHOGG_TEST_SEED into its container"
env -u NIDHOGG_TEST_SEED bash nidhogg/build.sh --container fedora-45 --dev --chaos --prefix /tmp/p --dry-run 2>/dev/null \
  | grep -qF 'NIDHOGG_TEST_SEED'                                   && fail "build.sh exports NIDHOGG_TEST_SEED that was not set"
# the fedora-45 Linux leg builds the test grammars (grammars.sh) and mounts them into its check container, so the
# grammar rows run instead of skipping; no other leg builds them
lj=$(job linux)
grep -qF 'bash test/src/nidhogg-tests/fixtures/grammars.sh "$RUNNER_TEMP/grammars"' <<<"$lj" || fail "the linux job does not run grammars.sh"
grep -qF "matrix.distro == 'fedora-45' && steps.build.outcome == 'success'" <<<"$lj"       || fail "grammars.sh runs on more than the fedora-45 leg"
grep -qF "NIDHOGG_TEST_GRAMMARS: \${{ steps.grammars.outcome == 'success' && '/grammars' || '' }}" <<<"$lj" || fail "the check does not see the grammars"
grep -qF -- "-v {0}/grammars:/grammars:ro,z" <<<"$lj"                 || fail "the check container does not mount the grammars"
# the fedora-45 leg's check installs gnutls-utils and dbus-daemon, so the TLS rows (gnutls-serv) and the settings
# tests' bus run instead of skipping
grep -qF "NIDHOGG_CHECK_PACKAGES: \${{ matrix.distro == 'fedora-45' && 'gnutls-utils dbus-daemon' || '' }}" <<<"$lj" || fail "the linux check does not install gnutls-utils and dbus-daemon on fedora-45"
# the prebuilt images (images.yml, nidhogg/ci/image.sh): published only for a push to nidhogg-31 or a run by hand, never
# from a pull request; the Linux jobs run in them by digest, the package-download caches are gone with them
iw=.github/workflows/images.yml
grep -qxF "  PUBLISH: \${{ (github.event_name == 'push' && github.ref == 'refs/heads/nidhogg-31') || github.event_name == 'workflow_dispatch' }}" "$iw" || fail "images.yml publishes for more than nidhogg-31's pushes and runs by hand"
awk '/^on:/,/^permissions:/' "$iw" | grep -qE '^  (push|pull_request|pull_request_target|schedule):' && fail "images.yml has a trigger of its own besides a run by hand"
grep -qF 'if [ -z "$ref" ] && [ "${PUBLISH:-}" = true ]; then ref=$(bash "$SELF" publish' nidhogg/ci/image.sh || fail "image.sh publishes without PUBLISH"
[ "$(cat "$wf" "$iw" | grep -c 'image.sh publish\|podman push')" = 0 ] || fail "a workflow pushes an image itself (only image.sh outputs does, under PUBLISH)"
job images | grep -qF 'uses: ./.github/workflows/images.yml'      || fail "no images job calling images.yml"
! grep -qE 'registry\.fedoraproject\.org|docker\.io/' "$wf"       || fail "a job runs a base image instead of its prebuilt one"
! grep -qF 'key: pkgs-' "$wf"                                       || fail "a job still caches package downloads (the images have the packages)"
[ "$(grep -cF 'uses: ./.github/actions/nh-image' "$wf")" -ge 9 ]   || fail "a Linux job does not take its prebuilt image"
for n in $(bash nidhogg/ci/image.sh names); do bash nidhogg/ci/image.sh tag "$n" | grep -qxE '[0-9a-f]{16}' || fail "image.sh has no tag for $n"; done
# an image's tag moves with every file its build reads: the Containerfile, an input, the script
d=$(mktemp -d); t0=$(bash nidhogg/ci/image.sh tag build-fedora-45)
[ "$t0" != "$(bash nidhogg/ci/image.sh tag build-fedora-45-norust)" ] || fail "the two targets of one Containerfile share a tag"
cp -a nidhogg "$d/nidhogg"
echo "# ci-test" >> "$d/nidhogg/deps/fedora-45.build"
[ "$(bash "$d/nidhogg/ci/image.sh" tag build-fedora-45)" != "$t0" ] || fail "the build image's tag does not follow deps/fedora-45.build"
rm -r "$d"
# the release legs' and compile-noglitnir's compiled Lisp: keyed on Emacs's ABI inputs (lisp-key.sh --abi), never
# restored for nidhogg-31 (or a tag), which saves its cold build's; build.sh's guard checks a restored set after make,
# and the tarball test fails a stale .eln
for j in linux macos compile-noglitnir; do
  job "$j" | grep -qF 'abi=$(bash nidhogg/ci/lisp-key.sh --abi)'    || fail "$j does not key its compiled Lisp on the ABI inputs"
  job "$j" | grep -A1 'id: lc$' | grep -qF "github.ref != 'refs/heads/nidhogg-31'" || fail "$j restores compiled Lisp for nidhogg-31"
  [ "$(job "$j" | grep -cF -- '-${{ steps.tree.outputs.abi }}-')" = 2 ] || fail "$j's compiled-Lisp restore and save keys differ"
  job "$j" | grep -qF "steps.lc.outcome == 'skipped' && github.ref == 'refs/heads/nidhogg-31'" || fail "nidhogg-31's $j does not save its cold Lisp"
done
# the macOS job's byte-compiled Lisp is an entry of its own, keyed on the Lisp side alone (a C body or header change
# still finds it), restored and saved under one key; its .eln files keep the ABI key above
job macos | grep -qF 'elc=$(bash nidhogg/ci/lisp-key.sh --elc)'  || fail "macos does not key its byte-compiled Lisp apart"
[ "$(job macos | grep -cF -- '-elc-${{ steps.tree.outputs.elc }}-')" = 2 ] || fail "macos's byte-compiled Lisp restore and save keys differ"
[ "$(job macos | grep -cF -- '-eln-${{ steps.tree.outputs.abi }}-')" = 2 ] || fail "macos's .eln entry is not keyed on the ABI inputs"
# the macOS job is two jobs that run at once, the NS app and Naglfar: each its own build, neither waiting for the other
# (the paired timing alone fetches the NS job's tarball), the rows, inputs and artifacts as before
job macos-naglfar | grep -qF 'build.sh --host --naglfar --dev --checking' || fail "no Naglfar build in the macos-naglfar job"
! job macos | grep -qF -- '--host --naglfar'                             || fail "the NS job also builds Naglfar"
for st in "Naglfar's Rust unit tests" "Naglfar's boundary check (nm-check)" "Naglfar GUI rows" "Naglfar paired timing (pair)"; do
  job macos-naglfar | grep -qF "name: $st" || fail "macos-naglfar lacks the step $st"
  ! job macos | grep -qF "name: $st"       || fail "macos still has the step $st"
done
for j in macos macos-naglfar; do
  [ "$(job "$j" | grep -cE '^    needs: changes$')" = 1 ]        || fail "$j needs more than the changes job (the two macOS jobs run at once)"
done
job macos | grep -qF 'name: macOS GUI rows (nhmac-gui-tests.el, scales 1 and 2)' || fail "macos lost the NS GUI rows"
job macos | grep -qF 'ip=--interpose'                              || fail "macos lost the --interpose rule"
job macos | grep -qF 'name: nhmac-gui-logs'                        || fail "macos lost its nhmac-gui-logs artifact"
job macos | grep -qF 'name: tarball-macos'                         || fail "macos lost its tarball-macos artifact"
job macos | grep -qF 'inputs.ns_app_run'                           || fail "macos lost ns_app_run"
job macos-naglfar | grep -qF 'inputs.pair'                         || fail "macos-naglfar lost pair"
job macos-naglfar | grep -qF 'inputs.files'                        || fail "macos-naglfar lost files"
job macos-naglfar | grep -qF 'inputs.checks'                       || fail "macos-naglfar lost checks"
job macos-naglfar | grep -qF 'select(.name == "tarball-macos")'    || fail "the paired timing does not take the NS job's tarball"
job macos-naglfar | grep -qF "needs.changes.outputs.macos != 'false'" || fail "macos-naglfar does not follow the macOS path filter"
job gate | grep -qF 'macos-naglfar " ;; esac'                      || fail "the gate lets a filtered macOS run skip macos but not macos-naglfar"
# a scratch/* run builds the NS app with only the preloaded Lisp native-compiled (no JIT in the image) and says so; every
# other run (nidhogg-31, tags, pull requests) keeps full AOT, and the tarball test checks the mode it was built in
job macos | grep -qF "PRELOADED: \${{ startsWith(github.ref, 'refs/heads/scratch/') }}" || fail "macos does not pick the preloaded-only mode for scratch/* alone"
[ "$(job macos | grep -cF 'NIDHOGG_BUILD_NATIVE_PRELOADED:')" = 2 ]  || fail "macos's build and pack steps do not both take the native-compilation mode"
job macos | grep -qF 'NIDHOGG_BUILD_NATIVE_PRELOADED=${{ steps.mode.outputs.preloaded' || fail "macos's check stage does not take the native-compilation mode"
job macos | grep -qF 'native compilation: preloaded Lisp only' && job macos | grep -qF 'native compilation: all Lisp ahead of time' || fail "macos does not log its native-compilation mode"
job macos | grep -A1 'id: ln$' | grep -qF "steps.mode.outputs.preloaded != 'true'" || fail "macos restores .eln files into a preloaded-only build"
grep -qF -- '--native-preloaded' nidhogg/build.sh && grep -qF 'NIDHOGG_BUILD_LISP_CACHE_ELC' nidhogg/build.sh || fail "build.sh has no preloaded-only mode"
grep -qF 'native-comp-jit-compilation is on' nidhogg/test/tarball-test.sh && grep -qF 'an AOT build with only' nidhogg/test/tarball-test.sh || fail "the tarball test does not check what each native-compilation mode promises"
# the Naglfar build does not compile the Lisp again: it takes the NS job's byte-compiled entry (the same key) before make,
# with the file times set, and keeps its configure checks in an entry of its own
nk="lisp-macos-\${{ runner.arch }}-\${{ steps.tc.outputs.v }}-\${{ runner.temp }}-elc-\${{ steps.tree.outputs.elc }}-\${{ hashFiles('nidhogg/build.sh') }}"
job macos-naglfar | grep -qF "key: $nk"                            || fail "macos-naglfar does not restore the NS job's byte-compiled Lisp entry"
job macos | grep -qF "key: $nk"                                    || fail "the byte-compiled Lisp key moved in macos"
job macos-naglfar | grep -A1 'id: lc$' | grep -qF "github.ref != 'refs/heads/nidhogg-31'" || fail "macos-naglfar restores compiled Lisp for nidhogg-31"
job macos-naglfar | grep -A1 'id: lc$' | grep -qF "!startsWith(github.ref, 'refs/tags/')" || fail "macos-naglfar restores compiled Lisp for a tag"
job macos-naglfar | grep -qF "NIDHOGG_BUILD_LISP_CACHE_ELC: '1'"   || fail "macos-naglfar's build does not take the byte-compiled part of the NS entry"
job macos-naglfar | grep -qF 'NIDHOGG_BUILD_LISP_CACHE: ${{ runner.temp }}/lisp-cache' || fail "macos-naglfar's build has no compiled-Lisp cache directory"
nj2=$(job macos-naglfar)
ln_of() { grep -n "$1" <<<"$nj2" | head -1 | cut -d: -f1; }
[ "$(ln_of 'id: lc$')" -lt "$(ln_of 'bash nidhogg/ci/set-mtimes.sh')" ] && [ "$(ln_of 'bash nidhogg/ci/set-mtimes.sh')" -lt "$(ln_of 'id: nbuild')" ] || fail "macos-naglfar does not restore the Lisp, set the file times, then build"
[ "$(job macos-naglfar | grep -cF -- 'key: configure-macos-naglfar-')" = 2 ] || fail "macos-naglfar's configure checks are not restored and saved under one key"
job macos-naglfar | grep -qF 'NIDHOGG_BUILD_CONFIG_CACHE: ${{ runner.temp }}/config-cache' || fail "macos-naglfar's build keeps no configure cache"
# the NS step inserts the counting interposer exactly when the selector selects a baseline row (nhmac-interposer-*,
# nhmac-ns-*) or is empty: its decision, run for each selector
ipd=$(job macos | sed -n '/^ *ip=""$/,/ip=--interpose; fi$/p' | sed 's/^ *//')
ipfor() { (CHECKS=$1; ip=; eval "$ipd"; echo "${ip:-none}"); }
[ -n "$ipd" ] || fail "the NS step decides no interposer"
[ "$(ipfor '')" = --interpose ]                                     || fail "a run with no selector gets no interposer"
[ "$(ipfor 'nhmac-')" = --interpose ]                               || fail "nhmac- selects a baseline row but gets no interposer"
[ "$(ipfor 'nhmac-frame-opens|nhmac-ns-menu-dump')" = --interpose ] || fail "a selector naming a baseline row gets no interposer"
[ "$(ipfor 'nhmac-frame-opens')" = none ]                           || fail "nhmac-frame-opens selects no baseline row but gets the interposer"
# a row-only run leaves out drgnx's macOS suite and says so; a full run keeps every tier, and the gate accepts the
# skipped drgnx job only when no tier is left
job changes | grep -qF 'drgnx macOS suite: skipped, row-only run' || fail "a row-only run does not say it skipped drgnx's macOS suite"
job changes | grep -qF "''|none|build-only) ;;"                   || fail "the drgnx suite skip is not limited to row selectors"
job drgnx | grep -qF "needs.changes.outputs.tiers != 'none'"      || fail "the drgnx job runs with no tier"
job gate | grep -qF '[ "$TIERS" != none ] || allow="$allow drgnx "' || fail "the gate does not accept the drgnx job a row-only run skipped"
job compile-noglitnir | grep -qF -- '--without-glitnir --dev' || fail "compile-noglitnir is not a dev build"
grep -qF 'nidhogg-eln-guard-check' nidhogg/build.sh                || fail "build.sh does not guard restored .eln files"
grep -qF 'if [ "$rc" = 75 ]; then' nidhogg/build.sh                || fail "build.sh does not build cold after the guard's verdict"
grep -qF 'stale .eln' nidhogg/test/tarball-test.sh                 || fail "the tarball test does not fail a stale .eln"
# the large prefix-restored entries are nidhogg-31's alone (a pull request restores them), and no cache holds a whole
# cargo home (unpacked sources, tools, credentials)
[ "$(grep -A1 "name: Save Glitnir's cargo target" "$wf" | grep -cF "env.CACHE_SAVE_ALL == 'true'")" = 2 ] || fail "a pull request saves a Glitnir cargo target"
! grep -qE '^ +\$\{\{ runner\.temp \}\}/(nh-recorder-cargo|nhshell-cargo|nhglue-cargo)$' "$wf" || fail "a cache holds a whole cargo home"
grep -qF 'NIDHOGG_BUILD_CONFIG_CACHE' "$wf" && grep -qF 'NIDHOGG_BUILD_CARGO_TARGET' "$wf" || fail "the builds keep no configure cache or cargo target"
# the test images name every package gui.sh and pkg-test.sh install on Fedora (else the scripts install it, slowly)
ti=nidhogg/ci/images/test-fedora-45.Containerfile
gui_want=$(sed -n '/want="mutter sway/,/\$pkgs"$/p' nidhogg/test/gui.sh | grep -v -e 'kwin ' -e 'fcitx5 ' -e '\$pkgs' | grep -oE '\b[a-z][A-Za-z0-9.+-]+\b' | grep -vxE 'want|mutter|case|in|echo|esac|then|do')
for p in mutter $gui_want; do grep -qE "(^| )$p( |$)" "$ti" || fail "the GUI image lacks gui.sh's $p"; done
pkg_want=$(sed -n '/^  \*) want=/,/procps-ng"$/p' nidhogg/test/pkg-test.sh | sed 's/.*\.runtime)//' | grep -oE '\b[a-z][a-z0-9.+-]+\b' | grep -vxE 'want|sed|s|src|nidhogg|deps|DISTRO|runtime')
for p in $pkg_want; do grep -qE "(^| )$p( |$)" "$ti" || fail "the package suites' image lacks pkg-test.sh's $p"; done
exit $rc

#!/usr/bin/env bash
# The check image (nidhogg/ci/image.sh local; the crates' check.sh --container and gui.sh --container): the packages
# are installed into a local image once per hash of what it is built from, and every later run starts from it with no
# package manager.  A stub podman (stub-podman.sh) stands for the real one: no container starts.
#   bash nidhogg/test/check-image-test.sh        Exit 0 ok, 1 on a failure.
set -u
here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
nh=$(cd "$here/.." && pwd)
rc=0
fail() { echo "FAIL: $*" >&2; rc=1; }
d=$(mktemp -d "${TMPDIR:-/tmp}/check-image-test.XXXXXX")
trap 'rm -r "$d"' EXIT
mkdir "$d/bin"
cp "$here/stub-podman.sh" "$d/bin/podman"
chmod +x "$d/bin/podman"
export STUB_DIR=$d/stub
mkdir "$STUB_DIR"
unset NIDHOGG_CRATES_IMAGE GLITNIR_GUI_IMAGE NIDHOGG_BUILD_PODMAN
export PATH="$d/bin:$PATH"
builds() { grep -c '^build ' "$STUB_DIR/log" 2>/dev/null || true; }
reset() { rm -f "$STUB_DIR"/log "$STUB_DIR"/img-*; }

# image.sh local NAME: built when missing, found when there; the image name is nidhogg-KIND:DISTRO-HASH
for name in check-fedora-45; do
  reset
  kind=${name%%-*}
  ref=$(bash "$nh/ci/image.sh" local "$name" 2>"$d/err1") || fail "$name: image.sh local failed: $(cat "$d/err1")"
  [[ $ref == localhost/nidhogg-$kind:fedora-45-* ]]        || fail "$name: image name is '$ref'"
  grep -qxF "image: $ref (built)" "$d/err1"                || fail "$name: first run does not say built: $(cat "$d/err1")"
  [ "$(builds)" = 1 ]                                      || fail "$name: first run built $(builds) times"
  ref2=$(bash "$nh/ci/image.sh" local "$name" 2>"$d/err2")
  [ "$ref2" = "$ref" ]                                     || fail "$name: the second run's image is '$ref2'"
  grep -qxF "image: $ref (local)" "$d/err2"                || fail "$name: second run does not say local: $(cat "$d/err2")"
  [ "$(builds)" = 1 ]                                      || fail "$name: second run built again"
done

# the tag moves with the package list
reset
t0=$(bash "$nh/ci/image.sh" tag check-fedora-45)
cp -a "$nh" "$d/nidhogg"
printf '# check-image-test\n' >> "$d/nidhogg/deps/fedora-45.check"
[ "$(bash "$d/nidhogg/ci/image.sh" tag check-fedora-45)" != "$t0" ] || fail "the check image's tag does not follow deps/fedora-45.check"
rm -r "$d/nidhogg"

# each crate's check.sh --container: one build, then none; the run uses the image and runs no package manager
for c in shell glue glitnir; do
  reset
  case $c in
    shell) export NHSHELL_CARGO_DIR=$d/cargo-$c ;;
    glue) export NHGLUE_CARGO_DIR=$d/cargo-$c ;;
    glitnir) export GLITNIR_CARGO_DIR=$d/cargo-$c ;;
  esac
  for run in 1 2; do
    bash "$nh/$c/check.sh" --container fedora-45 --test x >"$d/out" 2>&1 || fail "$c run $run: check.sh failed: $(cat "$d/out")"
    ref=$(sed -n 's/^image: \(.*\) (\(built\|local\))$/\1/p' "$d/out")
    [[ $ref == localhost/nidhogg-check:fedora-45-* ]] || fail "$c run $run: no image line: $(cat "$d/out")"
    want=local; [ "$run" = 1 ] && want=built
    grep -qxF "image: $ref ($want)" "$d/out"          || fail "$c run $run: not $want: $(cat "$d/out")"
    [ "$(builds)" = 1 ]                               || fail "$c run $run: $(builds) builds in all"
    line=$(grep '^run ' "$STUB_DIR/log" | tail -n 1)
    [[ $line == *" $ref "* ]]                         || fail "$c run $run: the container does not run $ref: $line"
    ! grep -q 'dnf' <<<"$line"                        || fail "$c run $run: the container runs dnf: $line"
  done
done

[ $rc != 0 ] || echo "check-image-test: ok"
exit $rc

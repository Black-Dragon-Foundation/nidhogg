#!/usr/bin/env bash
# drgnx's unit suite against a built Nidhogg tarball, for a local run (CI runs drgnx's own tiers workflow instead:
# README.md, "CI and releases").
#   nidhogg/ci/drgnx-suite.sh TARBALL DISTRO DRGNX_DIR     (DISTRO: fedora-44 | fedora-45 | ubuntu-26.04 | macos)
# Linux: a fresh --memory ${NIDHOGG_SUITE_MEMORY:-4g} podman container of DISTRO; the tarball is unpacked at its BUILDINFO
# prefix and DRGNX_DIR is mounted read-only.  macOS: on the host (brew install of deps.runtime + ripgrep + rust).
# DRGNX_RATATOSKR_BIN (Ratatoskr with the test hooks, ratatoskr-mock beside it) and DRGNX_HUGINN_DUMP (huginn's dump
# example) are passed through when set (Linux: their directories mounted read-only): build them from DRGNX_DIR
# as drgnx's CI does.  Unset, the tests that need them skip, and a full run fails on drgnx's test/no-skip.eld.
# Run as drgnx's own unit tier does: DRGNX_TEST_NO_TIMING=1, DRGNX_TEST_RERUN=1, one run (no timing pass).  DRGNX_TEST_SELECTOR narrows it.
# Exit 0 when the suite passes, else non-zero.  bash 3.2-safe.
set -eu
[ $# -eq 3 ] || { echo "usage: drgnx-suite.sh TARBALL DISTRO DRGNX_DIR" >&2; exit 2; }
TARBALL=$(cd "$(dirname "$1")" && pwd)/$(basename "$1")
DISTRO=$2
DRGNX=$(cd "$3" && pwd)
PODMAN=${NIDHOGG_BUILD_PODMAN:-podman}
[ -f "$TARBALL" ] || { echo "drgnx-suite: $TARBALL does not exist" >&2; exit 1; }
[ -x "$DRGNX/bin/drgnx" ] || { echo "drgnx-suite: $DRGNX/bin/drgnx not found" >&2; exit 1; }
TOP=$(tar -tJf "$TARBALL" | head -n 1 | cut -d/ -f1)

if [ "$DISTRO" = macos ]; then
  W=$(mktemp -d "${TMPDIR:-/tmp}/nidhogg-suite.XXXXXX")
  trap 'rm -rf "$W"' EXIT
  tar -C "$W" -xJf "$TARBALL"
  # shellcheck disable=SC2046
  brew install $(sed 's/#.*//' "$W/$TOP/deps.runtime") ripgrep
  export DRGNX_EMACS="$W/$TOP/Nidhogg.app/Contents/MacOS/Emacs"
  cd "$DRGNX"
  export DRGNX_TEST_JOBS
  DRGNX_TEST_JOBS=$(sysctl -n hw.ncpu)
  export DRGNX_TEST_RERUN=1 DRGNX_TEST_NO_TIMING=1   # as drgnx's own unit tier: no timing tests, a failed test reruns once
  bin/drgnx test
  exit $?
fi

case $DISTRO in
  fedora-44) IMAGE=registry.fedoraproject.org/fedora:44 ;;
  fedora-45) IMAGE=registry.fedoraproject.org/fedora:45 ;;
  ubuntu-26.04) IMAGE=docker.io/library/ubuntu:26.04 ;;
  *) echo "drgnx-suite: unknown distro $DISTRO" >&2; exit 2 ;;
esac

# the in-container script is single-quoted on purpose: it expands there, not here
# shellcheck disable=SC2016
# xz and tar first (a bare image may lack xz), then everything else
case $DISTRO in
  ubuntu-*) INNER='set -eu
apt-get update -qq
DEBIAN_FRONTEND=noninteractive apt-get install -y -qq --no-install-recommends xz-utils tar > /dev/null' ;;
  *) INNER='set -eu
dnf -y -q --setopt=install_weak_deps=False install xz tar > /dev/null' ;;
esac
INNER="$INNER"'
tar -xJOf /t.tar.xz "$TOP/share/nidhogg/deps.runtime" | sed "s/#.*//" > /deps.runtime
prefix=$(tar -xJOf /t.tar.xz "$TOP/share/nidhogg/BUILDINFO" | sed -n "s/^prefix=//p")
'
# shellcheck disable=SC2016
case $DISTRO in
  ubuntu-*) INNER="$INNER"'DEBIAN_FRONTEND=noninteractive apt-get install -y -qq --no-install-recommends git diffutils findutils procps zsh fish ripgrep ca-certificates $(cat /deps.runtime) > /dev/null' ;;
  *) INNER="$INNER"'dnf -y -q --setopt=install_weak_deps=False install git diffutils findutils procps-ng util-linux zsh fish ripgrep $(cat /deps.runtime) > /dev/null' ;;
esac
# shellcheck disable=SC2016
INNER="$INNER"'
mkdir -p "$prefix" && tar -C "$prefix" --strip-components=1 -xJf /t.tar.xz
export HOME=/tmp/home DRGNX_EMACS="$prefix/bin/nidhogg" DRGNX_TEST_RERUN=1 DRGNX_TEST_NO_TIMING=1 DRGNX_TEST_JOBS=$(nproc 2>/dev/null || sysctl -n hw.ncpu)
ln -sf "$prefix/bin/nidhogg" /usr/local/bin/emacs
mkdir -p "$HOME"
git config --global --add safe.directory "*"
cd /drgnx
bin/drgnx test'

OPTS=()
[ -z "${DRGNX_TEST_SELECTOR:-}" ] || OPTS=(-e "DRGNX_TEST_SELECTOR=$DRGNX_TEST_SELECTOR")
if [ -n "${DRGNX_RATATOSKR_BIN:-}" ]; then
  OPTS+=(-v "$(cd "$(dirname "$DRGNX_RATATOSKR_BIN")" && pwd):/opt/ratatoskr:ro,z"
        -e "DRGNX_RATATOSKR_BIN=/opt/ratatoskr/$(basename "$DRGNX_RATATOSKR_BIN")")
fi
if [ -n "${DRGNX_HUGINN_DUMP:-}" ]; then
  OPTS+=(-v "$(cd "$(dirname "$DRGNX_HUGINN_DUMP")" && pwd):/opt/huginn:ro,z"
        -e "DRGNX_HUGINN_DUMP=/opt/huginn/$(basename "$DRGNX_HUGINN_DUMP")")
fi
"$PODMAN" run --rm --memory "${NIDHOGG_SUITE_MEMORY:-4g}" --memory-swap "${NIDHOGG_SUITE_MEMORY:-4g}" \
  -e "TOP=$TOP" ${OPTS[@]+"${OPTS[@]}"} \
  -v "$TARBALL:/t.tar.xz:ro,z" -v "$DRGNX:/drgnx:ro,z" "$IMAGE" bash -c "$INNER"

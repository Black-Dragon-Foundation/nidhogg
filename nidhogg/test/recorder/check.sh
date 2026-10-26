#!/usr/bin/env bash
# nh-recorder's checks: fmt, clippy, tests, licence policy; --build makes the release binary.  The crate directory is
# never written.  Usage: check.sh [--container DISTRO] [--test NAME[\|NAME...]] [--build]   Exit 0 ok, 1 "check.sh: FAIL <check>",
# 2 usage.  Without --container the host's cargo runs (CI).  With it, podman runs a memory-capped container (3 GiB) that
# installs cargo, clippy, rustfmt and the -devel packages; CARGO_HOME and CARGO_TARGET_DIR live under
# $NH_RECORDER_CARGO_DIR (default $T/nh-recorder-cargo for CARGO_HOME, $T/nh-recorder-target for the target), and the
# container may use the network to fetch crates.  --build leaves the release binary at <target>/release/nh-recorder.
set -u
HERE=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
DISTRO="" TEST="" BUILD=0 INNER=0
usage() { echo "usage: check.sh [--container DISTRO] [--test NAME[\|NAME...]] [--build]" >&2; exit 2; }
while [ $# -gt 0 ]; do
  case $1 in
    --container) [ $# -ge 2 ] || usage; DISTRO=$2; shift 2 ;;
    --test) [ $# -ge 2 ] || usage; TEST=$2; shift 2 ;;
    --build) BUILD=1; shift ;;
    --inner) INNER=1; shift ;;
    *) usage ;;
  esac
done

if [ -n "$DISTRO" ] && [ "$INNER" = 0 ]; then
  command -v podman >/dev/null 2>&1 || { echo "check.sh: podman not found (podman only; no docker)" >&2; exit 1; }
  HOME_DIR=${NH_RECORDER_CARGO_HOME:-${T:+$T/nh-recorder-cargo}}
  TARGET_DIR=${NH_RECORDER_TARGET_DIR:-${T:+$T/nh-recorder-target}}
  [ -n "$HOME_DIR" ] && [ -n "$TARGET_DIR" ] || { echo "check.sh: set T (or NH_RECORDER_CARGO_HOME and NH_RECORDER_TARGET_DIR)" >&2; exit 2; }
  mkdir -p "$HOME_DIR" "$TARGET_DIR"
  EXTRA=()
  if [ -n "${NIDHOGG_BUILD_PKG_CACHE:-}" ]; then
    mkdir -p "$NIDHOGG_BUILD_PKG_CACHE/dnf"
    EXTRA=(-v "$NIDHOGG_BUILD_PKG_CACHE/dnf:/var/cache/libdnf5:Z")
  fi
  NAME=nh-recorder-check-$$
  cleanup() { podman rm -f "$NAME" >/dev/null 2>&1 || true; }
  trap cleanup EXIT
  trap 'exit 130' INT
  trap 'exit 143' TERM
  inner="dnf -y --setopt=install_weak_deps=False ${NIDHOGG_BUILD_PKG_CACHE:+--setopt=keepcache=1 }install cargo rust clippy rustfmt gcc pixman-devel libxkbcommon-devel wayland-devel diffutils >/dev/null && bash /crate/check.sh --inner${TEST:+ --test $(printf '%q' "$TEST")}"
  [ "$BUILD" = 1 ] && inner="$inner --build"
  podman run --rm --name "$NAME" --memory 3g --memory-swap 3g \
    -v "$HERE:/crate:ro,Z" -v "$HOME_DIR:/cargo-home:Z" -v "$TARGET_DIR:/cargo-target:Z" "${EXTRA[@]}" \
    -e CARGO_HOME=/cargo-home -e CARGO_TARGET_DIR=/cargo-target \
    "registry.fedoraproject.org/fedora:${DISTRO#fedora-}" bash -euo pipefail -c "$inner" &
  pid=$!
  wait "$pid"
  exit $?
fi

# --- the checks (host, or inside the container) ---------------------------------------------------------------
command -v cargo >/dev/null 2>&1 || { echo "check.sh: cargo not found (use --container fedora-45)" >&2; exit 1; }
cd "$HERE" || exit 1
: "${CARGO_TARGET_DIR:=$(mktemp -d)}"
export CARGO_TARGET_DIR
fail() { echo "check.sh: FAIL $1" >&2; exit 1; }

cargo fmt --check                                              || fail fmt
cargo clippy --all-targets --locked -- -D warnings             || fail clippy
# --test takes test file names separated by \| (or |): "shell\|scale" runs tests/shell.rs and tests/scale.rs
TESTARGS=()
if [ -n "$TEST" ]; then
  IFS='|' read -ra names <<< "${TEST//\\|/|}"
  for n in "${names[@]}"; do TESTARGS+=(--test "$n"); done
fi
cargo test --locked "${TESTARGS[@]}"                           || fail test
if command -v cargo-deny >/dev/null 2>&1; then
  cargo deny check licenses bans sources                       || fail deny
else
  echo "check.sh: cargo-deny not on PATH, skipping the licence check"
fi
if [ "$BUILD" = 1 ]; then
  cargo build --release --locked                               || fail build
fi
echo "check.sh: ok"

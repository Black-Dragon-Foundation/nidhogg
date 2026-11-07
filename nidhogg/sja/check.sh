#!/usr/bin/env bash
# Sjá's checks: fmt, clippy, tests (with and without test-hooks), licence policy.  The crate directory is never written.
# Cargo runs --locked: without a vendor/ it fetches the locked crates into CARGO_HOME.
# Usage: check.sh [--container DISTRO] [--test NAME] [--android]   Exit 0 ok, 1 "check.sh: FAIL <check>", 2 usage.
# --android checks only that the crate builds for aarch64-linux-android (the Android door): it needs that target's std.
# Without --container the host's cargo runs (CI, macOS, or inside a container already).  With it, podman runs a
# memory-capped container (2 GiB) from the check image (fedora-45: nidhogg-check:fedora-45-HASH, ci/image.sh local).
# CARGO_HOME and CARGO_TARGET_DIR live under $SJA_CARGO_DIR (default $T/sja-cargo).
# The MSRV (1.93): run it without --container under a 1.93 toolchain.
set -u
HERE=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
DISTRO="" TEST="" INNER=0 ANDROID=0
usage() { echo "usage: check.sh [--container DISTRO] [--test NAME] [--android]" >&2; exit 2; }
while [ $# -gt 0 ]; do
  case $1 in
    --container) [ $# -ge 2 ] || usage; DISTRO=$2; shift 2 ;;
    --test) [ $# -ge 2 ] || usage; TEST=$2; shift 2 ;;
    --inner) INNER=1; shift ;;
    --android) ANDROID=1; shift ;;
    *) usage ;;
  esac
done

if [ -n "$DISTRO" ] && [ "$INNER" = 0 ]; then
  command -v podman >/dev/null 2>&1 || { echo "check.sh: podman not found (podman only; no docker)" >&2; exit 1; }
  CARGO_DIR=${SJA_CARGO_DIR:-${T:+$T/sja-cargo}}
  [ -n "$CARGO_DIR" ] || { echo "check.sh: set SJA_CARGO_DIR (or T)" >&2; exit 2; }
  mkdir -p "$CARGO_DIR"
  NAME=sja-check-$$
  cleanup() { podman rm -f "$NAME" >/dev/null 2>&1 || true; }
  trap cleanup EXIT
  trap 'exit 130' INT
  trap 'exit 143' TERM
  inner="bash /nh/sja/check.sh --inner${TEST:+ --test $TEST}$( [ "$ANDROID" = 0 ] || echo " --android")"
  if [ "$DISTRO" = fedora-45 ]; then
    IMAGE=$(bash "$HERE/../ci/image.sh" local check-fedora-45) || { echo "check.sh: the check image failed" >&2; exit 1; }
  else
    inner="dnf -y --setopt=install_weak_deps=False install cargo rust clippy rustfmt >/dev/null && $inner"
    IMAGE=registry.fedoraproject.org/fedora:${DISTRO#fedora-}
  fi
  podman run --rm --name "$NAME" --memory 2g --memory-swap 2g \
    -v "$(cd "$HERE/.." && pwd):/nh:ro,z" -v "$CARGO_DIR:/cargo:Z" \
    -e CARGO_HOME=/cargo/home -e CARGO_TARGET_DIR=/cargo/target \
    "$IMAGE" bash -euo pipefail -c "$inner" &
  pid=$!
  wait "$pid"
  exit $?
fi

command -v cargo >/dev/null 2>&1 || { echo "check.sh: cargo not found (use --container fedora-45)" >&2; exit 1; }
cd "$HERE" || exit 1
: "${CARGO_TARGET_DIR:=$(mktemp -d)}"
export CARGO_TARGET_DIR
fail() { echo "check.sh: FAIL $1" >&2; exit 1; }

if [ "$ANDROID" = 1 ]; then
  sysroot=$(rustc --print sysroot)
  [ -d "$sysroot/lib/rustlib/aarch64-linux-android" ] ||
    fail "android (no std for aarch64-linux-android: rustup target add aarch64-linux-android)"
  cargo check --locked --target aarch64-linux-android || fail android
  echo "check.sh: android ok"
  exit 0
fi

cargo fmt --check                                                   || fail fmt
cargo clippy --all-targets --locked -- -D warnings                  || fail clippy
cargo clippy --all-targets --locked --features test-hooks -- -D warnings || fail clippy-test-hooks
cargo test --locked ${TEST:+"$TEST"}                                || fail test
cargo test --locked --features test-hooks ${TEST:+"$TEST"}          || fail test-test-hooks
if command -v cargo-deny >/dev/null 2>&1; then
  cargo deny check licenses bans sources                            || fail deny
else
  echo "check.sh: cargo-deny not on PATH, skipping the licence check"
fi
echo "check.sh: ok"

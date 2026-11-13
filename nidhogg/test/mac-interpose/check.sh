#!/usr/bin/env bash
# The lane's interposer under clippy for macOS (aarch64-apple-darwin, warnings as errors), on the toolchain the shell's
# and the glue's checks use: the container's own rustc version, with the darwin std from ci/target-std.sh, never a
# runner's moving stable.  Nothing links or runs (the crate is macOS-only).
# Usage: check.sh --container DISTRO   (podman: NIDHOGG_CRATES_IMAGE, else the local check image for fedora-45)
# CARGO_HOME lives under $MAC_INTERPOSE_CARGO_DIR (default $T/mac-interpose-cargo); nidhogg/ is mounted read-only.
# Exit 0 ok, 1 "check.sh: FAIL <why>", 2 usage, 3 the darwin toolchain is unavailable (says why).
set -u
HERE=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
if [ "${1:-}" = --inner ]; then
  cd "$HERE" || exit 1
  # the build image has no rustup-init (the check image does): the one install this check makes
  command -v rustup-init >/dev/null 2>&1 || dnf -y --setopt=install_weak_deps=False install rustup >/dev/null 2>&1 || true
  . ../../ci/target-std.sh
  target_std aarch64-apple-darwin || exit 3
  CARGO_TARGET_DIR=${CARGO_HOME%/*}/target cargo clippy --locked --target aarch64-apple-darwin -- -D warnings ||
    { echo "check.sh: FAIL clippy (mac-interpose, aarch64-apple-darwin)" >&2; exit 1; }
  echo "check.sh: mac-interpose ok (clippy, aarch64-apple-darwin, rustc $(rustc --version | awk '{print $2}'))"
  exit 0
fi
[ "${1:-}" = --container ] && [ -n "${2:-}" ] || { echo "usage: check.sh --container DISTRO" >&2; exit 2; }
DISTRO=$2
command -v podman >/dev/null 2>&1 || { echo "check.sh: podman not found (podman only; no docker)" >&2; exit 1; }
CARGO_DIR=${MAC_INTERPOSE_CARGO_DIR:-${T:+$T/mac-interpose-cargo}}
[ -n "$CARGO_DIR" ] || { echo "check.sh: set MAC_INTERPOSE_CARGO_DIR (or T)" >&2; exit 2; }
mkdir -p "$CARGO_DIR"
NH=$(cd "$HERE/../.." && pwd)
IMAGE=${NIDHOGG_CRATES_IMAGE:-}
if [ -z "$IMAGE" ]; then
  [ "$DISTRO" = fedora-45 ] || { echo "check.sh: no image for $DISTRO (set NIDHOGG_CRATES_IMAGE)" >&2; exit 2; }
  IMAGE=$(bash "$NH/ci/image.sh" local check-fedora-45) || { echo "check.sh: the check image failed" >&2; exit 1; }
fi
NAME=mac-interpose-check-$$
trap 'podman rm -f "$NAME" >/dev/null 2>&1 || true' EXIT
podman run --rm --name "$NAME" --memory 2g --memory-swap 2g -v "$NH:/nh:ro,z" -v "$CARGO_DIR:/cargo:z" \
  -e CARGO_HOME=/cargo/crates "$IMAGE" bash /nh/test/mac-interpose/check.sh --inner

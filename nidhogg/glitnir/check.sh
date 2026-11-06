#!/usr/bin/env bash
# Glitnir's checks: fmt, clippy, tests, licence policy, vendor drift (when a vendor/ is unpacked).  The crate directory
# is never written.  Cargo runs --locked: with a vendor/ and its .cargo/config.toml (nidhogg-vendor-VERSION.tar.zst
# unpacked in nidhogg/) it needs no network, without them it fetches the locked crates into CARGO_HOME.
# Usage: check.sh [--container DISTRO] [--test NAME] [--android]   Exit 0 ok, 1 "check.sh: FAIL <check>", 2 usage.
# --android checks only that the crate with its image decoders builds for aarch64-linux-android (the Android door,
# gate 53): it needs that target's std (rustup target add aarch64-linux-android), which Fedora's packages lack.
# Without --container the host's cargo runs (CI, macOS).  With it, podman runs a memory-capped container (2 GiB) from the
# check image (fedora-45: nidhogg-check:fedora-45-HASH, built once from deps/fedora-45.check by ci/image.sh local; the
# run prints "image: REF (local|built)" and installs nothing).  Another distro installs cargo, clippy, rustfmt and
# pixman-devel per run (the cache under $NIDHOGG_BUILD_PKG_CACHE when set).  CARGO_HOME and CARGO_TARGET_DIR live
# under $GLITNIR_CARGO_DIR (default $T/glitnir-cargo).
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
  CARGO_DIR=${GLITNIR_CARGO_DIR:-${T:+$T/glitnir-cargo}}
  [ -n "$CARGO_DIR" ] || { echo "check.sh: set GLITNIR_CARGO_DIR (or T)" >&2; exit 2; }
  mkdir -p "$CARGO_DIR"
  EXTRA=()
  if [ -n "${NIDHOGG_BUILD_PKG_CACHE:-}" ]; then
    mkdir -p "$NIDHOGG_BUILD_PKG_CACHE/dnf"
    EXTRA=(-v "$NIDHOGG_BUILD_PKG_CACHE/dnf:/var/cache/libdnf5:Z")
  fi
  NAME=glitnir-check-$$
  cleanup() { podman rm -f "$NAME" >/dev/null 2>&1 || true; }
  trap cleanup EXIT
  trap 'exit 130' INT
  trap 'exit 143' TERM
  inner="bash /nh/glitnir/check.sh --inner${TEST:+ --test $TEST}$( [ "$ANDROID" = 0 ] || echo " --android")"
  IMAGE=""
  if [ "$DISTRO" = fedora-45 ]; then
    # the check image (deps/fedora-45.check), built here once: no package manager runs
    IMAGE=$(bash "$HERE/../ci/image.sh" local check-fedora-45) || { echo "check.sh: the check image failed" >&2; exit 1; }
  else
    inner="dnf -y --setopt=install_weak_deps=False ${NIDHOGG_BUILD_PKG_CACHE:+--setopt=keepcache=1 }install cargo rust clippy rustfmt pixman-devel diffutils >/dev/null && $inner"
    IMAGE=registry.fedoraproject.org/fedora:${DISTRO#fedora-}
  fi
  podman run --rm --name "$NAME" --memory 2g --memory-swap 2g \
    -v "$(cd "$HERE/.." && pwd):/nh:ro,z" -v "$CARGO_DIR:/cargo:Z" "${EXTRA[@]}" \
    -e CARGO_HOME=/cargo/home -e CARGO_TARGET_DIR=/cargo/target \
    "$IMAGE" bash -euo pipefail -c "$inner" &
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

if [ "$ANDROID" = 1 ]; then
  sysroot=$(rustc --print sysroot)
  [ -d "$sysroot/lib/rustlib/aarch64-linux-android" ] ||
    fail "android (no std for aarch64-linux-android: rustup target add aarch64-linux-android)"
  cargo check --locked --target aarch64-linux-android --features images || fail android
  echo "check.sh: android ok"
  exit 0
fi

cargo fmt --check                                                   || fail fmt
cargo clippy --all-targets --locked -- -D warnings                  || fail clippy
cargo test --locked ${TEST:+"$TEST"}                                || fail test
# the pixman oracle: the kernels' OVER against pixman_image_composite32, bit for bit (Linux, where pixman is)
if [ "$(uname -s)" = Linux ]; then
  cargo test --locked --features pixman-oracle ${TEST:+"$TEST"} || fail test-pixman-oracle
fi
if command -v cargo-deny >/dev/null 2>&1; then
  cargo deny check licenses bans sources                            || fail deny
else
  echo "check.sh: cargo-deny not on PATH, skipping the licence check"
fi
# vendor drift: when a vendor/ is unpacked here (the release's nidhogg-vendor-VERSION.tar.zst), it must be the lock's
# packages and nothing else, every file as its .cargo-checksum.json lists it (ci/vendor.sh check).  Without one the
# checks above fetched the locked crates, and there is nothing to compare.
if [ -d vendor ]; then
  bash ../ci/vendor.sh check .. glitnir                              || fail vendor-drift
else
  echo "check.sh: no vendor/ here, the crates came from crates.io: vendor drift not checked"
fi
echo "check.sh: ok"

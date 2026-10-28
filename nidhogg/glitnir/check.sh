#!/usr/bin/env bash
# Glitnir's checks: fmt, clippy, tests, licence policy, vendor drift.  The crate directory is never written.
# Usage: check.sh [--container DISTRO] [--test NAME] [--android]   Exit 0 ok, 1 "check.sh: FAIL <check>", 2 usage.
# --android checks only that the crate with its image decoders builds for aarch64-linux-android (the Android door,
# gate 53): it needs that target's std (rustup target add aarch64-linux-android), which Fedora's packages lack.
# Without --container the host's cargo runs (CI, macOS).  With it, podman runs a memory-capped container (2 GiB) that
# installs cargo, clippy, rustfmt and pixman-devel; CARGO_HOME and CARGO_TARGET_DIR live under $GLITNIR_CARGO_DIR
# (default $T/glitnir-cargo), the package cache under $NIDHOGG_BUILD_PKG_CACHE when set.
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
  inner="dnf -y --setopt=install_weak_deps=False ${NIDHOGG_BUILD_PKG_CACHE:+--setopt=keepcache=1 }install cargo rust clippy rustfmt pixman-devel diffutils >/dev/null && bash /crate/check.sh --inner${TEST:+ --test $TEST}$( [ "$ANDROID" = 0 ] || echo " --android")"
  podman run --rm --name "$NAME" --memory 2g --memory-swap 2g \
    -v "$HERE:/crate:ro,Z" -v "$CARGO_DIR:/cargo:Z" "${EXTRA[@]}" \
    -e CARGO_HOME=/cargo/home -e CARGO_TARGET_DIR=/cargo/target \
    "registry.fedoraproject.org/fedora:${DISTRO#fedora-}" bash -euo pipefail -c "$inner" &
  pid=$!
  wait "$pid"
  exit $?
fi

# --- the checks (host, or inside the container) ---------------------------------------------------------------
command -v cargo >/dev/null 2>&1 || { echo "check.sh: cargo not found (use --container fedora-44)" >&2; exit 1; }
cd "$HERE" || exit 1
: "${CARGO_TARGET_DIR:=$(mktemp -d)}"
export CARGO_TARGET_DIR
fail() { echo "check.sh: FAIL $1" >&2; exit 1; }

if [ "$ANDROID" = 1 ]; then
  sysroot=$(rustc --print sysroot)
  [ -d "$sysroot/lib/rustlib/aarch64-linux-android" ] ||
    fail "android (no std for aarch64-linux-android: rustup target add aarch64-linux-android)"
  cargo check --offline --locked --target aarch64-linux-android --features images || fail android
  echo "check.sh: android ok"
  exit 0
fi

cargo fmt --check                                                   || fail fmt
cargo clippy --all-targets --offline --locked -- -D warnings        || fail clippy
cargo test --offline --locked ${TEST:+"$TEST"}                      || fail test
# the pixman oracle: the kernels' OVER against pixman_image_composite32, bit for bit (Linux, where pixman is)
if [ "$(uname -s)" = Linux ]; then
  cargo test --offline --locked --features pixman-oracle ${TEST:+"$TEST"} || fail test-pixman-oracle
fi
if command -v cargo-deny >/dev/null 2>&1; then
  cargo deny check licenses bans sources                            || fail deny
else
  echo "check.sh: cargo-deny not on PATH, skipping the licence check"
fi
# vendor drift: every vendored file is the one its .cargo-checksum.json lists, and no file is unlisted; then a copy
# vendored offline from vendor/ itself is compared with it, which shows a locked crate missing or a directory no
# lock entry needs.  Re-vendoring a directory source drops the dot files and Cargo.toml.orig and rewrites the
# checksum lists (it does not check them), so those are not compared.
SHA256="sha256sum"
command -v sha256sum >/dev/null 2>&1 || SHA256="shasum -a 256"
for d in vendor/*/; do
  sums=$(grep -o '"[^"]*":"[0-9a-f]\{64\}"' "$d.cargo-checksum.json" | grep -v '^"package":' |
           sed 's/^"\(.*\)":"\(.*\)"$/\2  \1/')
  ( cd "$d" && printf '%s\n' "$sums" | $SHA256 -c - >/dev/null )   || fail "vendor-checksum $d"
  [ "$(printf '%s\n' "$sums" | cut -c67- | sort)" = \
    "$(cd "$d" && find . -type f ! -name .cargo-checksum.json | sed 's|^\./||' | sort)" ] || fail "vendor-files $d"
done
copy="$CARGO_TARGET_DIR/vendor-check"
rm -rf "$copy" && mkdir -p "$copy"
for f in Cargo.toml Cargo.lock build.rs src include; do cp -r "$f" "$copy/" || fail vendor; done
( cd "$copy" && cargo vendor --locked --offline --respect-source-config \
    --config "source.crates-io.replace-with='vendored-sources'" \
    --config "source.vendored-sources.directory='$HERE/vendor'" vendor >/dev/null ) || fail vendor
diff -r -x '.*' -x Cargo.toml.orig "$copy/vendor" vendor              || fail vendor-drift
echo "check.sh: ok"

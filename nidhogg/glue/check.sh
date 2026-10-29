#!/usr/bin/env bash
# nhglue's checks: the lock against the shell's, fmt, clippy, tests, licence policy; --full adds the release archive and
# its exports; --android only checks that the crate builds for Android without the Wayland backend.
# Usage: check.sh [--container DISTRO] [--test NAME] [--full] [--android] [--no-run]
# --no-run compiles the tests without running them (the compile-only author check).
# Exit 0 ok, 1 "check.sh: FAIL <check>", 2 usage, 3 --android skipped (the target's std is not installed; says why).
# Without --container the host's cargo runs (CI, macOS).  With it, podman runs a memory-capped container (2 GiB) that
# installs cargo, clippy, rustfmt and the -devel packages the shell needs; CARGO_HOME and CARGO_TARGET_DIR live under
# $NHGLUE_CARGO_DIR (default $T/nhglue-cargo), the package cache under $NIDHOGG_BUILD_PKG_CACHE when set.  nidhogg/ is
# mounted read-only: the crate builds from the shell's vendor/ (.cargo/config.toml) and depends on ../shell and
# ../glitnir.  NH_RECORDER (a built nidhogg/test/recorder) is passed to the T1 tests (tests/*.rs), mounted.
# The link check against a built Emacs is nm-check.sh (it needs the build tree).
set -u
HERE=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
DISTRO="" TEST="" INNER=0 FULL=0 ANDROID=0 NORUN=0
ANDROID_TARGET=aarch64-linux-android
usage() { echo "usage: check.sh [--container DISTRO] [--test NAME] [--full] [--android] [--no-run]" >&2; exit 2; }
while [ $# -gt 0 ]; do
  case $1 in
    --container) [ $# -ge 2 ] || usage; DISTRO=$2; shift 2 ;;
    --test) [ $# -ge 2 ] || usage; TEST=$2; shift 2 ;;
    --full) FULL=1; shift ;;
    --no-run) NORUN=1; shift ;;
    --android) ANDROID=1; shift ;;
    --inner) INNER=1; shift ;;
    *) usage ;;
  esac
done

if [ -n "$DISTRO" ] && [ "$INNER" = 0 ]; then
  command -v podman >/dev/null 2>&1 || { echo "check.sh: podman not found (podman only; no docker)" >&2; exit 1; }
  CARGO_DIR=${NHGLUE_CARGO_DIR:-${T:+$T/nhglue-cargo}}
  [ -n "$CARGO_DIR" ] || { echo "check.sh: set NHGLUE_CARGO_DIR (or T)" >&2; exit 2; }
  mkdir -p "$CARGO_DIR"
  EXTRA=()
  if [ -n "${NIDHOGG_BUILD_PKG_CACHE:-}" ]; then
    mkdir -p "$NIDHOGG_BUILD_PKG_CACHE/dnf"
    EXTRA=(-v "$NIDHOGG_BUILD_PKG_CACHE/dnf:/var/cache/libdnf5:z")
  fi
  if [ -n "${NH_RECORDER:-}" ]; then
    [ -x "$NH_RECORDER" ] || { echo "check.sh: NH_RECORDER is not an executable: $NH_RECORDER" >&2; exit 2; }
    EXTRA+=(-v "$NH_RECORDER:/nh-recorder:ro,z" -e NH_RECORDER=/nh-recorder)
  fi
  NH=$(cd "$HERE/.." && pwd)
  NAME=nhglue-check-$$
  cleanup() { podman rm -f "$NAME" >/dev/null 2>&1 || true; }
  trap cleanup EXIT
  trap 'exit 130' INT
  trap 'exit 143' TERM
  args=""
  [ -n "$TEST" ] && args="$args --test $(printf %q "$TEST")"
  [ "$FULL" = 1 ] && args="$args --full"
  [ "$NORUN" = 1 ] && args="$args --no-run"
  [ "$ANDROID" = 1 ] && args="$args --android"
  # the Android target's std, where the distro packages it (Fedora does not: --android then exits 3)
  android_std=""
  [ "$ANDROID" = 1 ] && android_std="(dnf -y install rust-std-static-$ANDROID_TARGET >/dev/null 2>&1 || true) && "
  inner="dnf -y --setopt=install_weak_deps=False ${NIDHOGG_BUILD_PKG_CACHE:+--setopt=keepcache=1 }install cargo rust clippy rustfmt gcc binutils diffutils gawk pkgconf-pkg-config wayland-devel libxkbcommon-devel xkeyboard-config libX11-common pixman-devel >/dev/null && (dnf -y install cargo-deny >/dev/null 2>&1 || true) && ${android_std}bash /nh/glue/check.sh --inner$args"
  podman run --rm --name "$NAME" --memory 2g --memory-swap 2g \
    -v "$NH:/nh:ro,z" -v "$CARGO_DIR:/cargo:z" "${EXTRA[@]}" \
    -e CARGO_HOME=/cargo/home -e CARGO_TARGET_DIR=/cargo/target -e CARGO_BUILD_JOBS=2 \
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
[ -n "${CARGO_HOME:-}" ] && PATH="$CARGO_HOME/bin:$PATH"
fail() { echo "check.sh: FAIL $1" >&2; exit 1; }

if [ "$ANDROID" = 1 ]; then
  # cargo check only (no NDK, no link): the glue and the shell without the Wayland backend
  libdir=$(rustc --print target-libdir --target "$ANDROID_TARGET" 2>/dev/null)
  if [ -z "$libdir" ] || ! ls "$libdir"/libstd-*.rlib >/dev/null 2>&1; then
    echo "check.sh: android skipped: no std for $ANDROID_TARGET in this toolchain (rustup is not available offline)"
    exit 3
  fi
  cargo check --offline --locked --no-default-features --target "$ANDROID_TARGET" || fail android
  echo "check.sh: android ok"
  exit 0
fi

# the glue builds from the shell's vendor/, so its lock is the shell's plus the nhglue package
drop_self() { awk 'BEGIN { RS = ""; ORS = "\n\n" } !/^\[\[package\]\]\nname = "nhglue"\n/' "$1"; }
diff <(drop_self Cargo.lock) <(drop_self ../shell/Cargo.lock) >/dev/null ||
  fail "lock (Cargo.lock must be ../shell/Cargo.lock plus the nhglue package)"
cargo fmt --check                                                   || fail fmt
# the tests drive the test hooks (tests/ffi.rs and tests/display.rs require them), so they build with the feature
cargo clippy --all-targets --offline --locked --features test-hooks -- -D warnings || fail clippy-test-hooks
cargo clippy --lib --offline --locked -- -D warnings                || fail clippy
# NAME may list several filters separated by '|' or '\|' (cargo's filter is a substring, not a regexp)
filters=()
if [ -n "$TEST" ]; then
  IFS='|' read -ra parts <<< "${TEST//\\/}"
  for p in "${parts[@]}"; do [ -n "$p" ] && filters+=("$p"); done
fi
if [ "$NORUN" = 1 ]; then
  cargo test --offline --locked --features test-hooks --no-run       || fail test-build
else
  cargo test --offline --locked --no-fail-fast --features test-hooks -- ${filters[@]+"${filters[@]}"} || fail test
fi
if command -v cargo-deny >/dev/null 2>&1; then
  cargo deny check licenses bans sources                            || fail deny
else
  echo "check.sh: cargo-deny not on PATH, skipping the licence check"
fi
[ "$FULL" = 1 ] || { echo "check.sh: ok (without --full: no release archive)"; exit 0; }
# the archive Emacs links: every nhg_* and glitnir_* function of the two headers is exported (built with
# the test hooks, as an --enable-checking Emacs links it, so that the hooks' exports are checked too)
cargo build --release --offline --locked --features test-hooks      || fail build
lib=$CARGO_TARGET_DIR/release/libnhglue.a
[ -f "$lib" ] || fail "build (no libnhglue.a)"
exports=$(nm -g --defined-only "$lib" 2>/dev/null | awk '$2 ~ /^[TDBR]$/ {print $3}')
for sym in $(grep -ho '\(nhg\|glitnir\)_[a-z0-9_]* (\?(' include/nhglue.h ../glitnir/include/glitnir.h |
               tr -d '( ' | sort -u); do
  printf '%s\n' "$exports" | grep -qx "$sym" || fail "exports $sym"
done
echo "check.sh: ok"

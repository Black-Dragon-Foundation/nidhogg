#!/usr/bin/env bash
# nhshell's checks: fmt, clippy, tests, licence policy, vendor drift, and the exports of libnhshell.a.
# Usage: check.sh [--container DISTRO] [--test NAME] [--full] [--revendor]   Exit 0 ok, 1 "check.sh: FAIL <check>", 2 usage.
# --full adds the slow checks (vendor drift, which needs the network, and the LTO release build with its exports); a
# task's author check runs without it, integration builds and gates with it.
# Without --container the host's cargo runs (CI, macOS).  With it, podman runs a memory-capped container (2 GiB) that
# installs cargo, clippy, rustfmt and the -devel packages; CARGO_HOME and CARGO_TARGET_DIR live under
# $NHSHELL_CARGO_DIR (default $T/nhshell-cargo), the package cache under $NIDHOGG_BUILD_PKG_CACHE when set.  The
# crate directory is only written by --revendor, which regenerates Cargo.lock and vendor/ (needs the network).
# The vendor-drift check and --revendor use cargo-vendor-filterer, installed into $CARGO_HOME/bin on first use.
# NH_RECORDER (S0's nh-recorder binary) is passed to the T1 tests (tests/reader.rs); in a container it is mounted.
set -u
HERE=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
DISTRO="" TEST="" INNER=0 REVENDOR=0 FULL=0
usage() { echo "usage: check.sh [--container DISTRO] [--test NAME] [--full] [--revendor]" >&2; exit 2; }
while [ $# -gt 0 ]; do
  case $1 in
    --container) [ $# -ge 2 ] || usage; DISTRO=$2; shift 2 ;;
    --test) [ $# -ge 2 ] || usage; TEST=$2; shift 2 ;;
    --revendor) REVENDOR=1; shift ;;
    --full) FULL=1; shift ;;
    --inner) INNER=1; shift ;;
    *) usage ;;
  esac
done
# The platforms the vendored tree covers (Global Constraints of the W0 plan).
PLATFORMS=(--platform x86_64-unknown-linux-gnu --platform aarch64-unknown-linux-gnu
           --platform aarch64-apple-darwin --platform x86_64-apple-darwin)

if [ -n "$DISTRO" ] && [ "$INNER" = 0 ]; then
  command -v podman >/dev/null 2>&1 || { echo "check.sh: podman not found (podman only; no docker)" >&2; exit 1; }
  CARGO_DIR=${NHSHELL_CARGO_DIR:-${T:+$T/nhshell-cargo}}
  [ -n "$CARGO_DIR" ] || { echo "check.sh: set NHSHELL_CARGO_DIR (or T)" >&2; exit 2; }
  mkdir -p "$CARGO_DIR"
  EXTRA=()
  if [ -n "${NIDHOGG_BUILD_PKG_CACHE:-}" ]; then
    mkdir -p "$NIDHOGG_BUILD_PKG_CACHE/dnf"
    EXTRA=(-v "$NIDHOGG_BUILD_PKG_CACHE/dnf:/var/cache/libdnf5:Z")
  fi
  # the T1 tests' recorder (S0's nh-recorder): the binary read-only (shared label: other containers use it too)
  if [ -n "${NH_RECORDER:-}" ]; then
    [ -x "$NH_RECORDER" ] || { echo "check.sh: NH_RECORDER is not an executable: $NH_RECORDER" >&2; exit 2; }
    EXTRA+=(-v "$NH_RECORDER:/nh-recorder:ro,z" -e NH_RECORDER=/nh-recorder)
  fi
  # nidhogg/ is mounted whole: the crate depends on ../glitnir.  Read-only, except for --revendor.
  NH=$(cd "$HERE/.." && pwd)
  MODE=ro; [ "$REVENDOR" = 1 ] && MODE=rw
  NAME=nhshell-check-$$
  cleanup() { podman rm -f "$NAME" >/dev/null 2>&1 || true; }
  trap cleanup EXIT
  trap 'exit 130' INT
  trap 'exit 143' TERM
  inner="dnf -y --setopt=install_weak_deps=False ${NIDHOGG_BUILD_PKG_CACHE:+--setopt=keepcache=1 }install cargo rust clippy rustfmt gcc binutils diffutils pkgconf-pkg-config openssl-devel cmake wayland-devel libxkbcommon-devel pixman-devel >/dev/null && (dnf -y install cargo-deny >/dev/null 2>&1 || true) && bash /nh/shell/check.sh --inner${TEST:+ --test $TEST}$([ "$REVENDOR" = 1 ] && echo ' --revendor')$([ "$FULL" = 1 ] && echo ' --full')"
  podman run --rm --name "$NAME" --memory 2g --memory-swap 2g \
    -v "$NH:/nh:$MODE,Z" -v "$CARGO_DIR:/cargo:Z" "${EXTRA[@]}" \
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

ensure_filterer() {
  command -v cargo-vendor-filterer >/dev/null 2>&1 && return 0
  cargo install cargo-vendor-filterer --locked || fail vendor-filterer-install
}

if [ "$REVENDOR" = 1 ]; then
  # Regenerate Cargo.lock and vendor/ from crates.io (the source replacement is off for this run).
  ensure_filterer
  work=$CARGO_TARGET_DIR/revendor
  rm -rf "$work" && mkdir -p "$work/shell" "$work/glitnir"
  for f in Cargo.toml src include; do [ -e "$f" ] && cp -r "$f" "$work/shell/"; done
  [ -e Cargo.lock ] && cp Cargo.lock "$work/shell/"
  cp -r ../glitnir/Cargo.toml ../glitnir/Cargo.lock ../glitnir/build.rs ../glitnir/src ../glitnir/include "$work/glitnir/"
  ( cd "$work/shell" && cargo generate-lockfile && cargo vendor-filterer "${PLATFORMS[@]}" vendor ) || fail revendor
  rm -rf vendor && cp -r "$work/shell/vendor" vendor && cp "$work/shell/Cargo.lock" Cargo.lock
  echo "check.sh: revendored"
  exit 0
fi

cargo fmt --check                                                   || fail fmt
cargo clippy --all-targets --offline --locked -- -D warnings        || fail clippy
# NAME may list several filters separated by '|' or '\|' (cargo's filter is a substring, not a regexp)
filters=()
if [ -n "$TEST" ]; then
  IFS='|' read -ra parts <<< "${TEST//\\/}"
  for p in "${parts[@]}"; do [ -n "$p" ] && filters+=("$p"); done
fi
cargo test --offline --locked -- "${filters[@]}"                     || fail test
if command -v cargo-deny >/dev/null 2>&1; then
  cargo deny check licenses bans sources                            || fail deny
else
  echo "check.sh: cargo-deny not on PATH, skipping the licence check"
fi
[ "$FULL" = 1 ] || { echo "check.sh: ok (without --full: no vendor drift, no release build)"; exit 0; }
# vendor drift: vendor a copy from crates.io (network), compare with vendor/
ensure_filterer
copy=$CARGO_TARGET_DIR/vendor-check
rm -rf "$copy" && mkdir -p "$copy/shell" "$copy/glitnir"
for f in Cargo.toml Cargo.lock src include; do [ -e "$f" ] && cp -r "$f" "$copy/shell/"; done
cp -r ../glitnir/Cargo.toml ../glitnir/Cargo.lock ../glitnir/build.rs ../glitnir/src ../glitnir/include "$copy/glitnir/"
( cd "$copy/shell" && cargo vendor-filterer "${PLATFORMS[@]}" vendor >/dev/null ) || fail vendor
diff Cargo.lock "$copy/shell/Cargo.lock" || fail lock-drift  # the filterer has no --locked: an unchanged lock is the same
diff -r -x .keep "$copy/shell/vendor" vendor                        || fail vendor-drift
# the archive: one static library with every Glitnir symbol and the shell's own
cargo build --release --offline --locked                            || fail build
lib=$CARGO_TARGET_DIR/release/libnhshell.a
[ -f "$lib" ] || fail "build (no libnhshell.a)"
exports=$(nm -g --defined-only "$lib" 2>/dev/null | awk '$2 ~ /^[TDBR]$/ {print $3}')
for sym in nhshell_abi_version $(grep -o 'glitnir_[a-z0-9_]*(' ../glitnir/include/glitnir.h | tr -d '(' | sort -u); do
  printf '%s\n' "$exports" | grep -qx "$sym" || fail "exports $sym"
done
echo "check.sh: ok"

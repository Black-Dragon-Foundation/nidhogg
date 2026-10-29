#!/usr/bin/env bash
# nhshell's checks: fmt, clippy, tests, licence policy, vendor drift, and the release build.
# Usage: check.sh [--container DISTRO] [--test NAME] [--full] [--revendor] [--tsan]   Exit 0 ok, 1 "check.sh: FAIL <check>",
# 2 usage, 3 the TSan lane is unavailable.
# --full adds the slow checks (vendor drift, which needs the network, and the LTO release build with its exports); a
# task's author check runs without it, integration builds and gates with it.
# Without --container the host's cargo runs (CI, macOS).  With it, podman runs a memory-capped container (2 GiB) that
# installs cargo, clippy, rustfmt and the -devel packages; CARGO_HOME and CARGO_TARGET_DIR live under
# $NHSHELL_CARGO_DIR (default $T/nhshell-cargo), the package cache under $NIDHOGG_BUILD_PKG_CACHE when set.  The
# crate directory is only written by --revendor, which regenerates Cargo.lock and vendor/ (needs the network).
# The vendor-drift check and --revendor use cargo-vendor-filterer, installed into $CARGO_HOME/bin on first use.
# --tsan runs only the named tests (all without --test) under ThreadSanitizer: a nightly toolchain (rustup, installed
# once into $NHSHELL_CARGO_DIR/rustup and tsan-home, so the stable lanes keep their cargo) with -Zbuild-std, whose std
# dependencies are vendored together with the crate's (network).  Where that cannot be had it exits 3 and says why;
# a data race the sanitizer finds fails the lane.  Its target directory is separate (target/tsan).
# NH_RECORDER (a built nidhogg/test/recorder) is passed to the T1 tests (tests/*.rs); in a container it is mounted.
set -u
HERE=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
DISTRO="" TEST="" INNER=0 REVENDOR=0 FULL=0 TSAN=0
usage() { echo "usage: check.sh [--container DISTRO] [--test NAME] [--full] [--revendor] [--tsan]" >&2; exit 2; }
while [ $# -gt 0 ]; do
  case $1 in
    --container) [ $# -ge 2 ] || usage; DISTRO=$2; shift 2 ;;
    --test) [ $# -ge 2 ] || usage; TEST=$2; shift 2 ;;
    --revendor) REVENDOR=1; shift ;;
    --full) FULL=1; shift ;;
    --tsan) TSAN=1; shift ;;
    --inner) INNER=1; shift ;;
    *) usage ;;
  esac
done
# The platforms the vendored tree covers: Linux and macOS, x86_64 and aarch64.
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
  # the T1 tests' recorder (nh-recorder): the binary read-only (shared label: other containers use it too)
  if [ -n "${NH_RECORDER:-}" ]; then
    [ -x "$NH_RECORDER" ] || { echo "check.sh: NH_RECORDER is not an executable: $NH_RECORDER" >&2; exit 2; }
    EXTRA+=(-v "$NH_RECORDER:/nh-recorder:ro,z" -e NH_RECORDER=/nh-recorder)
  fi
  # nidhogg/ is mounted whole: the crate depends on ../glitnir.  Read-only, except for --revendor.
  NH=$(cd "$HERE/.." && pwd)
  MODE=ro; [ "$REVENDOR" = 1 ] && MODE=rw
  TSAN_PKGS="" MEM=2g
  [ "$TSAN" = 0 ] || { TSAN_PKGS=" rustup"; MEM=4g; }
  NAME=nhshell-check-$$
  cleanup() { podman rm -f "$NAME" >/dev/null 2>&1 || true; }
  trap cleanup EXIT
  trap 'exit 130' INT
  trap 'exit 143' TERM
  inner="dnf -y --setopt=install_weak_deps=False ${NIDHOGG_BUILD_PKG_CACHE:+--setopt=keepcache=1 }install cargo rust clippy rustfmt gcc binutils diffutils pkgconf-pkg-config openssl-devel cmake wayland-devel libxkbcommon-devel xkeyboard-config libX11-common pixman-devel${TSAN_PKGS:-} >/dev/null && (dnf -y install cargo-deny >/dev/null 2>&1 || true) && bash /nh/shell/check.sh --inner${TEST:+ --test $(printf %q "$TEST")}$([ "$REVENDOR" = 1 ] && echo ' --revendor')$([ "$FULL" = 1 ] && echo ' --full')$([ "$TSAN" = 1 ] && echo ' --tsan')"
  podman run --rm --name "$NAME" --memory "$MEM" --memory-swap "$MEM" \
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

# The TSan lane (see the header).  Everything it needs besides the Fedora packages is cached under /cargo.
tsan_lane() {
  unavailable() { echo "check.sh: TSan lane unavailable: $1" >&2; exit 3; }
  local home=${CARGO_HOME%/*}/tsan-home vend=${CARGO_HOME%/*}/tsan-vendor
  export RUSTUP_HOME=${CARGO_HOME%/*}/rustup
  command -v rustup-init >/dev/null 2>&1 || unavailable "rustup-init is not installed"
  if [ ! -x "$home/bin/cargo" ]; then
    CARGO_HOME=$home rustup-init -y --no-modify-path --profile minimal --default-toolchain nightly -c rust-src >/dev/null 2>&1 \
      || unavailable "no nightly toolchain with rust-src (rustup failed)"
  fi
  export PATH="$home/bin:$PATH"
  local std host
  std=$(rustc --print sysroot)/lib/rustlib/src/rust/library/Cargo.toml
  [ -f "$std" ] || unavailable "std's sources are not in the toolchain ($std)"
  host=$(rustc -vV | sed -n 's/^host: //p')
  if [ ! -f "$vend/.done" ]; then
    # the crate's dependencies and std's, from crates.io, in one directory source
    local work=$CARGO_TARGET_DIR/tsan-vendor-src
    rm -rf "$work" "$vend" && mkdir -p "$work/shell" "$work/glitnir"
    for f in Cargo.toml Cargo.lock src include; do [ -e "$f" ] && cp -r "$f" "$work/shell/"; done
    cp -r ../glitnir/Cargo.toml ../glitnir/Cargo.lock ../glitnir/build.rs ../glitnir/src ../glitnir/include "$work/glitnir/"
    ( cd "$work/shell" && CARGO_HOME=$home cargo vendor --sync "$std" "$vend" >/dev/null 2>"$work/vendor.log" ) \
      || unavailable "could not vendor std's dependencies: $(tail -n 1 "$work/vendor.log")"
    touch "$vend/.done"
  fi
  export CARGO_TARGET_DIR=$CARGO_TARGET_DIR/tsan
  local cfg=(--config "source.crates-io.replace-with='tsan-vendor'" --config "source.tsan-vendor.directory='$vend'")
  local filters=()
  if [ -n "$TEST" ]; then
    IFS='|' read -ra parts <<< "${TEST//\\/}"
    for p in "${parts[@]}"; do [ -n "$p" ] && filters+=("$p"); done
  fi
  # rustix goes through libc here: its raw syscalls map and unmap the shm buffers behind TSan's back, so a buffer
  # at an address another library's mapping (libxkbcommon's Compose file) had used reads as racing with that mmap
  export RUSTFLAGS="-Zsanitizer=thread --cfg rustix_use_libc"
  cargo test --offline -Zbuild-std --target "$host" "${cfg[@]}" --no-run >"$CARGO_TARGET_DIR.build.log" 2>&1 \
    || unavailable "the sanitized build failed: $(grep -m1 -E '^error' "$CARGO_TARGET_DIR.build.log")"
  # --tests: rustdoc does not get RUSTFLAGS, so a doc test would mix sanitizer settings
  TSAN_OPTIONS=halt_on_error=1 cargo test --offline -Zbuild-std --target "$host" "${cfg[@]}" --tests -- "${filters[@]}" || fail tsan
  echo "check.sh: tsan ok"
  exit 0
}
[ "$TSAN" = 0 ] || tsan_lane

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
# the filters also select glitnir's own tests (pool policy and the like) when a task names them
if [ -n "$TEST" ]; then (cd ../glitnir && cargo test --offline --locked -- "${filters[@]}") || fail test-glitnir; fi
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
# the release build (an rlib; the archive Emacs links, with this crate's exports, is nidhogg/glue's: its check.sh --full)
cargo build --release --offline --locked                            || fail build
echo "check.sh: ok"

#!/usr/bin/env bash
# Do the workers' C files compile against Android's headers?  (Task 38; no Android build exists, kb: android-door-open)
# Usage: android-check.sh      Exit 0 ok, 1 with "ANDROID FAIL: <why>", 2 when the NDK cannot be had.
# A syntax-only compile (clang -fsyntax-only, aarch64-linux-android26, harness mode as workers-harness.sh) of each
# worker file against the NDK's bionic headers.  Run it in the container: $CTR bash nidhogg/test/android-check.sh.
# The NDK is pinned by version and sha256; set NDK_CACHE to keep the unpacked toolchain between runs.
# First run after changing NDK_VERSION: the sha256 below is "unpinned", the script prints the downloaded file's and
# stops (exit 2); copy it into NDK_SHA256.
set -euo pipefail
ROOT=$(cd "$(dirname "$0")/../.." && pwd)
NDK_VERSION=r27c
NDK_URL=https://dl.google.com/android/repository/android-ndk-$NDK_VERSION-linux.zip
# Google publishes no sha256 for r27c.  This one was computed (2026-10-23) from the zip whose sha1
# (090e8083a715fdb1a3e402d0763c388abb03fb4e) and size (663987688) match Google's repository manifest
# (dl.google.com/android/repository/repository2-3.xml, package ndk;27.2.12479018).
NDK_SHA256=59c2f6dc96743b5daf5d1626684640b20a6bd2b1d85b13156b90333741bad5cc
CACHE=${NDK_CACHE:-${TMPDIR:-/tmp}/nidhogg-ndk}
API=26			# pthread_getname_np needs 26; the Android port's minimum is the same or lower
FILES="nidhogg-workers nidhogg-treesit-bg nidhogg-io nidhogg-decode nidhogg-json-tape nidhogg-image-bg nidhogg-blk"

NDK=$CACHE/android-ndk-$NDK_VERSION
LLVM=$NDK/toolchains/llvm/prebuilt/linux-x86_64
if [ ! -x "$LLVM/bin/clang" ]; then
  mkdir -p "$CACHE"
  zip=$CACHE/android-ndk-$NDK_VERSION-linux.zip
  if [ ! -f "$zip" ]; then
    curl -fsSL --retry 3 -o "$zip.part" "$NDK_URL"
    mv "$zip.part" "$zip"
  fi
  got=$(sha256sum "$zip" | cut -d' ' -f1)
  if [ "$NDK_SHA256" = unpinned ]; then
    echo "ANDROID: $NDK_URL sha256 $got (set NDK_SHA256 to it)"; exit 2
  fi
  if [ "$got" != "$NDK_SHA256" ]; then
    rm -f "$zip"; echo "ANDROID FAIL: $NDK_URL has sha256 $got, expected $NDK_SHA256"; exit 1
  fi
  command -v unzip >/dev/null || { [ "$(id -u)" = 0 ] && dnf install -y -q unzip >/dev/null; }
  unzip -q -o "$zip" "android-ndk-$NDK_VERSION/toolchains/llvm/prebuilt/linux-x86_64/*" -d "$CACHE"
fi

CC=("$LLVM/bin/clang" --target=aarch64-linux-android$API --sysroot="$LLVM/sysroot")
FLAGS=(-std=gnu11 -Wall -Werror=implicit-function-declaration -Werror=incompatible-pointer-types
       -Werror=int-conversion -DHAVE_NIDHOGG_WORKERS -DNH_WK_HARNESS -I"$ROOT/src" -fsyntax-only)

fail=0
out=$(mktemp -d)
trap 'rm -r "$out"' EXIT

# The fixture that must fail: glibc's getaddrinfo_a does not exist in bionic.
cat >"$out/fixture.c" <<'EOF'
#define _GNU_SOURCE
#include <netdb.h>
int
main (void)
{
  struct gaicb *list[1] = { 0 };
  return getaddrinfo_a (GAI_NOWAIT, list, 1, 0);
}
EOF
if "${CC[@]}" "${FLAGS[@]}" "$out/fixture.c" 2>"$out/fixture.err"; then
  echo "ANDROID FAIL: the getaddrinfo_a fixture compiled (the check does not see bionic's headers)"; fail=1
elif ! grep -q getaddrinfo_a "$out/fixture.err"; then
  echo "ANDROID FAIL: the fixture failed for another reason:"; cat "$out/fixture.err"; fail=1
fi

for f in $FILES; do
  if "${CC[@]}" "${FLAGS[@]}" "$ROOT/src/$f.c" 2>"$out/$f.err"; then
    echo "ok   $f"
  else
    echo "ANDROID FAIL: $f.c against bionic:"; cat "$out/$f.err"; fail=1
  fi
done
exit $fail

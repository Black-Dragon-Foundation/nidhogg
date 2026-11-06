#!/usr/bin/env bash
# The vendored crates of the native code (the shell, the glue, Glitnir), which are not in git: the Cargo.lock files are the
# pin, and a release publishes the crates as nidhogg-vendor-<version>.tar.zst.
# Usage: vendor.sh make OUT.tar.zst       build the tarball from the lockfiles (needs the network, cargo,
#                                         cargo-vendor-filterer and zstd; fails when a lockfile would change)
#        vendor.sh check [DIR] [CRATE...]  DIR (default: this nidhogg/) is a tree with the tarball unpacked in it, or a
#                                         .tar.zst; checks, offline, that every vendored file is the one its
#                                         .cargo-checksum.json lists and that the vendored packages are exactly the
#                                         ones the Cargo.lock files list (by sha256). CRATEs: shell, glitnir
#                                         (default: those with a vendor/ directory); the glue is checked with the shell.
# Exit 0 ok, 1 "vendor.sh: FAIL <what>", 2 usage.
# The tarball unpacks into nidhogg/ (tar --zstd -xf nidhogg-vendor-V.tar.zst -C nidhogg): shell/vendor, glitnir/vendor, and
# the .cargo/config.toml of the shell, the glue and Glitnir, which point cargo at them. With those in place a build never
# touches the network; without them cargo fetches the locked crates from crates.io.
set -u
HERE=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
NH=$(cd "$HERE/.." && pwd)
fail() { echo "vendor.sh: FAIL $1" >&2; exit 1; }
usage() { echo "usage: vendor.sh make OUT.tar.zst | check [DIR|TARBALL] [shell|glitnir...]" >&2; exit 2; }
SHA256="sha256sum"
command -v sha256sum >/dev/null 2>&1 || SHA256="shasum -a 256"
# The platforms the vendored tree of the shell covers: Linux and macOS, x86_64 and aarch64.
PLATFORMS=(--platform x86_64-unknown-linux-gnu --platform aarch64-unknown-linux-gnu
           --platform aarch64-apple-darwin --platform x86_64-apple-darwin)

[ $# -ge 1 ] || usage
cmd=$1; shift

# the sha256 of every registry package a lockfile lists
lock_sums() { grep -o '^checksum = "[0-9a-f]\{64\}"' "$1" | grep -o '[0-9a-f]\{64\}' | sort -u; }
# the sha256 of every package a vendor directory holds
vendor_sums() { cat "$1"/*/.cargo-checksum.json | grep -o '"package":"[0-9a-f]\{64\}"' | grep -o '[0-9a-f]\{64\}' | sort -u; }

# every vendored file is the one its .cargo-checksum.json lists, and no file is unlisted
verify_files() {
  local d sums
  for d in "$1"/*/; do
    sums=$(grep -o '"[^"]*":"[0-9a-f]\{64\}"' "$d.cargo-checksum.json" | grep -v '^"package":' |
             sed 's/^"\(.*\)":"\(.*\)"$/\2  \1/')
    ( cd "$d" && printf '%s\n' "$sums" | $SHA256 -c - >/dev/null ) || fail "vendor-checksum $d"
    [ "$(printf '%s\n' "$sums" | cut -c67- | sort)" = \
      "$(cd "$d" && find . -type f ! -name .cargo-checksum.json | sed 's|^\./||' | sort)" ] || fail "vendor-files $d"
  done
}

case $cmd in
  check)
    root=$NH tmp=""
    if [ $# -ge 1 ]; then
      case $1 in
        shell|glitnir) ;;
        *) root=$1; shift ;;
      esac
    fi
    if [ -f "$root" ]; then
      command -v zstd >/dev/null 2>&1 || fail "no zstd to unpack $root"
      tmp=$(mktemp -d) || exit 1
      trap 'rm -rf "$tmp"' EXIT
      tar --zstd -xf "$root" -C "$tmp" || fail "unpack $root"
      root=$tmp
    fi
    crates=("$@")
    if [ ${#crates[@]} = 0 ]; then
      for c in shell glitnir; do [ ! -d "$root/$c/vendor" ] || crates+=("$c"); done
      [ ${#crates[@]} -gt 0 ] || fail "no vendor/ directory in $root (unpack nidhogg-vendor-<version>.tar.zst there)"
    fi
    for c in "${crates[@]}"; do
      [ -d "$root/$c/vendor" ] || fail "no $c/vendor in $root"
      verify_files "$root/$c/vendor"
      [ "$(vendor_sums "$root/$c/vendor")" = "$(lock_sums "$NH/$c/Cargo.lock")" ] ||
        fail "vendor-drift $c (its vendor/ and $c/Cargo.lock list different packages)"
      # the glue builds from the shell's vendor/: every package of its lock is in it
      if [ "$c" = shell ] && [ -f "$NH/glue/Cargo.lock" ]; then
        missing=$(comm -23 <(lock_sums "$NH/glue/Cargo.lock") <(vendor_sums "$root/shell/vendor"))
        [ -z "$missing" ] || fail "vendor-drift glue (packages of glue/Cargo.lock missing from shell/vendor: $missing)"
      fi
      echo "vendor.sh: $c ok ($(ls "$root/$c/vendor" | wc -l | tr -d ' ') crates)"
    done
    ;;
  make)
    [ $# = 1 ] || usage
    out=$1
    for t in cargo cargo-vendor-filterer zstd; do
      command -v "$t" >/dev/null 2>&1 || fail "$t not found (cargo install cargo-vendor-filterer --locked)"
    done
    work=$(mktemp -d) || exit 1
    trap 'rm -rf "$work"' EXIT
    mkdir -p "$work/src/shell" "$work/src/glitnir" "$work/stage/shell" "$work/stage/glitnir" "$work/stage/glue"
    for f in Cargo.toml Cargo.lock src include; do [ -e "$NH/shell/$f" ] && cp -r "$NH/shell/$f" "$work/src/shell/"; done
    for f in Cargo.toml Cargo.lock build.rs src include; do cp -r "$NH/glitnir/$f" "$work/src/glitnir/"; done
    # --locked everywhere it can be; the filterer has no such flag, so an unchanged lock is the same file
    ( cd "$work/src/shell" && cargo vendor-filterer "${PLATFORMS[@]}" "$work/stage/shell/vendor" >/dev/null ) || fail vendor-shell
    cmp -s "$NH/shell/Cargo.lock" "$work/src/shell/Cargo.lock" || fail "lock-drift shell (cargo would change shell/Cargo.lock)"
    ( cd "$work/src/glitnir" && cargo vendor --locked "$work/stage/glitnir/vendor" >/dev/null ) || fail vendor-glitnir
    for c in shell glitnir glue; do
      mkdir -p "$work/stage/$c/.cargo"
      dir=vendor; [ "$c" != glue ] || dir=../shell/vendor
      printf '[source.crates-io]\nreplace-with = "vendored-sources"\n\n[source.vendored-sources]\ndirectory = "%s"\n' "$dir" \
        > "$work/stage/$c/.cargo/config.toml"
    done
    # the checks pass on what is about to be published
    bash "$0" check "$work/stage" shell glitnir || exit 1
    mkdir -p "$(dirname "$out")"
    ( cd "$work/stage" && tar --sort=name --mtime=@0 --owner=0 --group=0 --numeric-owner -cf - shell glitnir glue ) |
      zstd -19 -q -o "$out" -f || fail pack
    echo "vendor.sh: wrote $out"
    ;;
  *) usage ;;
esac

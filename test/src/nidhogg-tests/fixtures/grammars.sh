#!/usr/bin/env bash
# Build the tree-sitter grammars the workers' rows use (C, TypeScript, Python, Rust, HTML) at pinned
# commits into a cache directory, and check each one's ABI against tree-sitter 0.25 (13 to 15).
#   grammars.sh [CACHE-DIR [LANGUAGE...]]   (default dir: $NIDHOGG_TEST_GRAMMARS, else grammars-cache/ beside this
#                                            script; default languages: all five, and `crash')
# `crash' is the test grammar of workers/worker-crash-forwarded-grammar: tree-sitter-python with its external
# scanner wrapped by crash-scanner.c, which writes through a null pointer at a backquote.
# Prints the directory.  A grammar already built is kept.  Needs git, a C compiler (a C++ one only for a
# grammar with a scanner.cc) and the network for the first run.  The rows look in NIDHOGG_TEST_GRAMMARS, else
# in grammars-cache/ beside this script; the cache directory is never committed.
set -euo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
OUT=${1:-${NIDHOGG_TEST_GRAMMARS:-$(dirname "$0")/grammars-cache}}
[ $# -eq 0 ] || shift
WANT=" $* "
mkdir -p "$OUT"
OUT=$(cd "$OUT" && pwd)
CC=${CC:-cc}
CXX=${CXX:-c++}
MIN_ABI=13
MAX_ABI=15

# lang  repository  commit  subdirectory-of-the-grammar
GRAMMARS="
c          tree-sitter/tree-sitter-c          7fa1be1b694b6e763686793d97da01f36a0e5c12 .
typescript tree-sitter/tree-sitter-typescript f975a621f4e7f532fe322e13c4f79495e0a7b2e7 typescript
python     tree-sitter/tree-sitter-python     293fdc02038ee2bf0e2e206711b69c90ac0d413f .
rust       tree-sitter/tree-sitter-rust       18b0515fca567f5a10aee9978c6d2640e878671a .
html       tree-sitter/tree-sitter-html       5a5ca8551a179998360b4a4ca2c0f366a35acc03 .
crash      tree-sitter/tree-sitter-python     293fdc02038ee2bf0e2e206711b69c90ac0d413f .
"

work=$(mktemp -d "${TMPDIR:-/tmp}/nh-grammars.XXXXXX")
trap 'rm -r "$work"' EXIT

while read -r lang repo commit sub; do
  [ -n "$lang" ] || continue
  [ "$WANT" = "  " ] || [[ "$WANT" == *" $lang "* ]] || continue
  lib="$OUT/libtree-sitter-$lang.so"
  [ -f "$lib" ] && continue
  src="$work/$lang"
  mkdir -p "$src"
  git -C "$src" init -q
  for try in 1 2 3; do
    git -C "$src" fetch -q --depth 1 "https://github.com/$repo" "$commit" && break
    [ "$try" -lt 3 ] || exit 1
    sleep 5
  done
  git -C "$src" checkout -q FETCH_HEAD
  dir="$src/$sub/src"
  abi=$(sed -n 's/^#define LANGUAGE_VERSION \([0-9]*\).*/\1/p' "$dir/parser.c" | head -1)
  if [ -z "$abi" ] || [ "$abi" -lt "$MIN_ABI" ] || [ "$abi" -gt "$MAX_ABI" ]; then
    echo "grammars.sh: $lang has ABI '${abi:-?}', tree-sitter 0.25 reads $MIN_ABI to $MAX_ABI" >&2
    exit 1
  fi
  objs=() pflags=() sflags=()
  if [ "$lang" = crash ]; then
    # Python renamed: parser.c calls nh_crash_scan (crash-scanner.c), which calls Python's scan
    pflags=(-Dtree_sitter_python=tree_sitter_crash -Dtree_sitter_python_external_scanner_scan=nh_crash_scan)
    sflags=(-Dtree_sitter_python_external_scanner_scan=nh_crash_real_scan)
    "$CC" -fPIC -O1 -I"$dir" -c "$HERE/crash-scanner.c" -o "$work/$lang-crash-scanner.o"
    objs+=("$work/$lang-crash-scanner.o")
  fi
  for f in "$dir/parser.c" "$dir/scanner.c"; do
    [ -f "$f" ] || continue
    flags=("${pflags[@]}"); [ "$(basename "$f")" = parser.c ] || flags=("${sflags[@]}")
    "$CC" -fPIC -O1 -I"$dir" "${flags[@]}" -c "$f" -o "$work/$lang-$(basename "$f").o"
    objs+=("$work/$lang-$(basename "$f").o")
  done
  link=$CC
  if [ -f "$dir/scanner.cc" ]; then
    "$CXX" -fPIC -O1 -I"$dir" -c "$dir/scanner.cc" -o "$work/$lang-scanner-cc.o"
    objs+=("$work/$lang-scanner-cc.o")
    link=$CXX
  fi
  "$link" -shared "${objs[@]}" -o "$lib.tmp"
  mv "$lib.tmp" "$lib"
  echo "grammars.sh: $lang (ABI $abi) built" >&2
done <<<"$GRAMMARS"
echo "$OUT"

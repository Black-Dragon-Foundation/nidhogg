#!/usr/bin/env bash
# Print the hash that keys the compiled-Lisp cache: git's tree hashes of everything that determines the build's Lisp output
# (the Lisp sources, the C side whose primitives and fingerprint the .eln files embed, the generators and the configure
# inputs).  Documentation, tests, the nidhogg/ scripts and CI files are left out on purpose: they do not change the output.
# nidhogg/build.sh (the configure flags) and the compiler are keyed by the workflow.  Run in the checkout.
#   lisp-key.sh --elc   the byte-compiled part only (the generated files under lisp/ and leim/, without the .eln files),
#                       for the dev test lanes and the macOS job off nidhogg-31 and tags, never a release build: the C
#                       side's bodies are left out, as make itself
#                       treats byte code, but the names of its functions, variables and features are in (code that tests
#                       `fboundp', `boundp' or `featurep' at compile time compiles differently when one comes or goes)
#   lisp-key.sh --abi   the byte-compiled part and the .eln files, keyed on what Emacs's own .eln check hashes
#                       (src/comp.c, hash_native_abi: ABI_VERSION, the version, the configuration and options, and every
#                       primitive's name and arity) instead of the C bodies: --elc's inputs, nidhogg/version (in the
#                       version and the configure options), the DEFUN signatures (name, min and max arguments), and the
#                       files that decide what the native compiler emits and the layouts it inlines (src/comp.c, every
#                       src/*.h, lib/).  The configuration, the compiler and libgccjit are the workflow's part of the key.
#                       A C body change then reuses the cache; nidhogg/eln-guard.el checks the restored .eln files
#                       against the new binary after the build, and a mismatch builds the Lisp cold (README.md, "CI and
#                       releases").  Never restored for nidhogg-31 or a release tag.
set -euo pipefail
parts="lisp leim src lib lib-src lwlib oldXMenu exec nextstep modules m4 admin build-aux etc configure.ac autogen.sh Makefile.in GNUmakefile"
elc=0 abi=0
case ${1:-} in
  --elc) elc=1 parts="lisp leim m4 admin build-aux etc configure.ac autogen.sh Makefile.in GNUmakefile" ;;
  --abi) elc=1 abi=1 parts="lisp leim m4 admin build-aux etc configure.ac autogen.sh Makefile.in GNUmakefile nidhogg/version lib src/comp.c" ;;
  '') ;;
  *) echo "usage: lisp-key.sh [--elc|--abi]" >&2; exit 2 ;;
esac
{
  [ $abi = 0 ] || echo abi
  for p in $parts; do
    printf '%s %s\n' "$p" "$(git rev-parse "HEAD:$p")"
  done
  if [ $elc = 1 ]; then
    # every DEFUN and DEFVAR name and every provide in the C sources, sorted
    printf 'c-names %s\n' "$(git grep -h -E '^[[:space:]]*(DEFUN|DEFVAR_[A-Z_]+) \("|Fprovide \(' HEAD -- 'src/*.c' 'src/*.m' \
      | sed -E 's/^[[:space:]]*(DEFUN|DEFVAR_[A-Z_]+) \("([^"]*)".*/\1 \2/; s/^[[:space:]]+//' | LC_ALL=C sort -u | sha256sum | cut -c1-40)"
  fi
  if [ $abi = 1 ]; then
    # every primitive's signature as hash_native_abi sees it (comp--subr-signature: the name and the arity): DEFUN's
    # first, fourth and fifth arguments, which may run over three lines
    printf 'defuns %s\n' "$(git grep -h -A3 -E '^[[:space:]]*DEFUN \(' HEAD -- 'src/*.c' 'src/*.m' | awk '
      function flush(  s, n, a, o) {
        if (acc == "") return
        s = acc; sub(/^[^(]*\(/, "", s); n = split(s, a, ",")
        if (n >= 5) { o = a[1] "," a[4] "," a[5]; gsub(/[[:space:]]+/, "", o); print o }
        acc = ""
      }
      /^[[:space:]]*DEFUN \(/ { flush(); acc = $0; next }
      /^--$/ { flush(); next }
      acc != "" { acc = acc " " $0 }
      END { flush() }' | LC_ALL=C sort | sha256sum | cut -c1-40)"
    # every header of src/ (lisp.h's layouts, which the emitted code inlines, among them)
    printf 'headers %s\n' "$(git ls-tree HEAD src/ | awk '$4 ~ /\.h$/ {print $3, $4}' | sha256sum | cut -c1-40)"
  fi
} | sha256sum | cut -c1-40

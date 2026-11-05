#!/usr/bin/env bash
# Print the hash that keys the compiled-Lisp cache: git's tree hashes of everything that determines the build's Lisp output
# (the Lisp sources, the C side whose primitives and fingerprint the .eln files embed, the generators and the configure
# inputs).  Documentation, tests, the nidhogg/ scripts and CI files are left out on purpose: they do not change the output.
# nidhogg/build.sh (the configure flags) and the compiler are keyed by the workflow.  Run in the checkout.
#   lisp-key.sh --elc   the byte-compiled part only (the generated files under lisp/ and leim/, without the .eln files),
#                       for the dev test lanes, never a release build: the C side's bodies are left out, as make itself
#                       treats byte code, but the names of its functions, variables and features are in (code that tests
#                       `fboundp', `boundp' or `featurep' at compile time compiles differently when one comes or goes)
set -euo pipefail
parts="lisp leim src lib lib-src lwlib oldXMenu exec nextstep modules m4 admin build-aux etc configure.ac autogen.sh Makefile.in GNUmakefile"
elc=0
case ${1:-} in
  --elc) elc=1 parts="lisp leim m4 admin build-aux etc configure.ac autogen.sh Makefile.in GNUmakefile" ;;
  '') ;;
  *) echo "usage: lisp-key.sh [--elc]" >&2; exit 2 ;;
esac
{
  for p in $parts; do
    printf '%s %s\n' "$p" "$(git rev-parse "HEAD:$p")"
  done
  if [ $elc = 1 ]; then
    # every DEFUN and DEFVAR name and every provide in the C sources, sorted
    printf 'c-names %s\n' "$(git grep -h -E '^[[:space:]]*(DEFUN|DEFVAR_[A-Z_]+) \("|Fprovide \(' HEAD -- 'src/*.c' 'src/*.m' \
      | sed -E 's/^[[:space:]]*(DEFUN|DEFVAR_[A-Z_]+) \("([^"]*)".*/\1 \2/; s/^[[:space:]]+//' | LC_ALL=C sort -u | sha256sum | cut -c1-40)"
  fi
} | sha256sum | cut -c1-40

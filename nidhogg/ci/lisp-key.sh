#!/usr/bin/env bash
# Print the hash that keys the compiled-Lisp cache: git's tree hashes of everything that determines the build's Lisp output
# (the Lisp sources, the C side whose primitives and fingerprint the .eln files embed, the generators and the configure
# inputs).  Documentation, tests, the nidhogg/ scripts and CI files are left out on purpose: they do not change the output.
# nidhogg/build.sh (the configure flags) and the compiler are keyed by the workflow.  Run in the checkout.
set -euo pipefail
for p in lisp leim src lib lib-src lwlib oldXMenu exec nextstep modules m4 admin build-aux etc configure.ac autogen.sh Makefile.in GNUmakefile; do
  printf '%s %s\n' "$p" "$(git rev-parse "HEAD:$p")"
done | sha256sum | cut -c1-40

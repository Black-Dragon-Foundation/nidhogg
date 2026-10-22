#!/usr/bin/env bash
# The Lisp-free harness for the workers' C files (PD-16).  Stub (Task 2): it compiles each
# nidhogg-*.c of the workers with -DNH_WK_HARNESS, which leaves out everything that needs Lisp,
# so the runtime, the queues and the wake protocol can be driven alone.  Task 3 adds the
# ThreadSanitizer driver (`--tsan queue wake'), Tasks 6, 10, 16, 18, 22, 24, 27-29 and 32 the fuzz targets.
set -euo pipefail
ROOT=$(cd "$(dirname "$0")/../.." && pwd)
CC=${CC:-cc}
FILES="nidhogg-workers nidhogg-treesit-bg nidhogg-io nidhogg-decode nidhogg-json-tape nidhogg-image-bg nidhogg-blk"
fail=0
for f in $FILES; do
  if "$CC" -std=gnu11 -Wall -Werror -DHAVE_NIDHOGG_WORKERS -DNH_WK_HARNESS -I"$ROOT/src" \
       -c "$ROOT/src/$f.c" -o /dev/null; then
    echo "ok   $f.c"
  else
    echo "FAIL $f.c"; fail=1
  fi
done
exit $fail

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
if [ "${1:-}" = --fuzz ]; then
  # --fuzz SECONDS [TARGET...]: each fuzz/TARGET.c (default: all) for SECONDS, under ASan and UBSan when the
  # compiler has them (dnf installs the runtimes when run as root).  libFuzzer when the compiler has it, else
  # fuzz/driver.c (mutations of the seeds under fixtures/images, fixed random seed).
  SECS=${2:?--fuzz SECONDS}; shift 2
  FZ="$ROOT/test/src/nidhogg-tests/fuzz"; OUT=${TMPDIR:-/tmp}/nh-fuzz-$$; mkdir -p "$OUT"
  SAN=-fsanitize=address,undefined
  if ! echo 'int main(void){return 0;}' | "$CC" $SAN -x c - -o "$OUT/probe" 2>/dev/null; then
    [ "$(id -u)" = 0 ] && dnf install -y -q libasan libubsan >/dev/null 2>&1 || true
    echo 'int main(void){return 0;}' | "$CC" $SAN -x c - -o "$OUT/probe" 2>/dev/null || { SAN=; echo "fuzz: no sanitizers"; }
  fi
  TARGETS=${*:-$(cd "$FZ" && ls *.c | grep -v '^driver\.c$' | sed 's/\.c$//')}
  for t in $TARGETS; do
    case $t in header) SRCS="nidhogg-image-bg" ;; *) echo "fuzz: unknown target $t"; fail=1; continue ;; esac
    srcs=(); for m in $SRCS; do srcs+=("$ROOT/src/$m.c"); done
    seeds=("$ROOT"/test/src/nidhogg-tests/fixtures/images/*)
    if echo 'int main(void){return 0;}' | "$CC" -fsanitize=fuzzer -x c - -o "$OUT/probe" 2>/dev/null; then
      "$CC" -std=gnu11 -g -O1 $SAN -fsanitize=fuzzer -DHAVE_NIDHOGG_WORKERS -DNH_WK_HARNESS -I"$ROOT/src" \
        "$FZ/$t.c" "${srcs[@]}" -o "$OUT/$t" && "$OUT/$t" -max_total_time="$SECS" -seed=1 "${seeds[@]}" || fail=1
    else
      "$CC" -std=gnu11 -g -O1 $SAN -DHAVE_NIDHOGG_WORKERS -DNH_WK_HARNESS -I"$ROOT/src" \
        "$FZ/$t.c" "$FZ/driver.c" "${srcs[@]}" -o "$OUT/$t" && "$OUT/$t" "$SECS" "${seeds[@]}" || fail=1
    fi
    [ $fail = 0 ] && echo "ok   fuzz $t" || echo "FAIL fuzz $t"
  done
  rm -r "$OUT"
  exit $fail
fi
for f in $FILES; do
  if "$CC" -std=gnu11 -Wall -Werror -DHAVE_NIDHOGG_WORKERS -DNH_WK_HARNESS -I"$ROOT/src" \
       -c "$ROOT/src/$f.c" -o /dev/null; then
    echo "ok   $f.c"
  else
    echo "FAIL $f.c"; fail=1
  fi
done
exit $fail

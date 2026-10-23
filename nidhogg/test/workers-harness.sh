#!/usr/bin/env bash
# The Lisp-free harness for the workers' C files (PD-16).
#   workers-harness.sh                    compile each nidhogg-*.c of the workers with -DNH_WK_HARNESS, which
#                                         leaves out everything that needs Lisp
#   workers-harness.sh --tsan MODULE...   build and run test/src/nidhogg-tests/fuzz/tsan-MODULE.c under
#                                         ThreadSanitizer; exit 0 = every driver passed with no race report
# Modules: queue (two producers, one consumer, 10^6 jobs through the runtime), wake (the completion channel's
# clear/re-check protocol, 10^6 races), io (the reader's hand-over, wake fds and synchronous hand-back over pipes
# filled by writer threads: every byte once and in order).  Tasks 6, 10, 16, 18, 22, 24, 27-29 and 32 add drivers and fuzz targets.
set -euo pipefail
ROOT=$(cd "$(dirname "$0")/../.." && pwd)
CC=${CC:-cc}
FLAGS=(-std=gnu11 -Wall -Werror -DHAVE_NIDHOGG_WORKERS -DNH_WK_HARNESS -I"$ROOT/src")

if [ "${1:-}" = --tsan ]; then
  shift
  [ $# -gt 0 ] || { echo "workers-harness: --tsan needs a module" >&2; exit 2; }
  out=$(mktemp -d)
  trap 'rm -r "$out"' EXIT
  fail=0
  for m in "$@"; do
    src="$ROOT/test/src/nidhogg-tests/fuzz/tsan-$m.c"
    if [ ! -f "$src" ]; then
      echo "FAIL no $m module"; fail=1; continue
    fi
    if ! "$CC" "${FLAGS[@]}" -fsanitize=thread -g -O1 -pthread "$src" -o "$out/$m"; then
      echo "FAIL $m (build)"; fail=1; continue
    fi
    if TSAN_OPTIONS="halt_on_error=1 exitcode=66 ${TSAN_OPTIONS:-}" "$out/$m"; then
      echo "ok   $m"
    else
      echo "FAIL $m"; fail=1
    fi
  done
  exit $fail
fi

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
  TARGETS=${*:-$(cd "$FZ" && ls *.c | grep -v '^driver\.c$' | grep -v '^tsan-' | sed 's/\.c$//')}
  for t in $TARGETS; do
    DEFS=; LIBS=
    case $t in header) SRCS="nidhogg-image-bg" ;; json) SRCS="" ;;
      # Task 28: the image decoder cores, with the decoder libraries
      cores) SRCS="nidhogg-image-bg"; DEFS="-DHAVE_PNG -DHAVE_JPEG -DHAVE_GIF -DHAVE_WEBP"
             LIBS="-lpng -ljpeg -lgif -lwebp -lwebpdemux -lm" ;;
      *) echo "fuzz: unknown target $t"; fail=1; continue ;; esac
    srcs=(); for m in $SRCS; do srcs+=("$ROOT/src/$m.c"); done
    seeds=("$ROOT"/test/src/nidhogg-tests/fixtures/images/*)
    if echo 'int main(void){return 0;}' | "$CC" -fsanitize=fuzzer -x c - -o "$OUT/probe" 2>/dev/null; then
      "$CC" -std=gnu11 -g -O1 $SAN -fsanitize=fuzzer $DEFS -DHAVE_NIDHOGG_WORKERS -DNH_WK_HARNESS -I"$ROOT/src" \
        "$FZ/$t.c" "${srcs[@]}" $LIBS -o "$OUT/$t" && "$OUT/$t" -max_total_time="$SECS" -seed=1 "${seeds[@]}" || fail=1
    else
      "$CC" -std=gnu11 -g -O1 $SAN $DEFS -DHAVE_NIDHOGG_WORKERS -DNH_WK_HARNESS -I"$ROOT/src" \
        "$FZ/$t.c" "$FZ/driver.c" "${srcs[@]}" $LIBS -o "$OUT/$t" && "$OUT/$t" "$SECS" "${seeds[@]}" || fail=1
    fi
    [ $fail = 0 ] && echo "ok   fuzz $t" || echo "FAIL fuzz $t"
  done
  rm -r "$OUT"
  exit $fail
fi
for f in $FILES; do
  if "$CC" "${FLAGS[@]}" -c "$ROOT/src/$f.c" -o /dev/null; then
    echo "ok   $f.c"
  else
    echo "FAIL $f.c"; fail=1
  fi
done
exit $fail

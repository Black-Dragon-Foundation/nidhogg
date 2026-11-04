#!/usr/bin/env bash
# The Lisp-free harness for the workers' C files (PD-16).
#   workers-harness.sh                    compile each nidhogg-*.c of the workers with -DNH_WK_HARNESS, which
#                                         leaves out everything that needs Lisp
#   workers-harness.sh --tsan MODULE...   build and run test/src/nidhogg-tests/fuzz/tsan-MODULE.c under
#                                         ThreadSanitizer; exit 0 = every driver passed with no race report
#   workers-harness.sh --sched MODULE... [--seeds N]
#                                         build fuzz/sched-MODULE.c and run it under exact thread orders (PCT,
#                                         fuzz/sched-pct.h), seeds 1 to N (default 2000); exit 0 = every seed passed and
#                                         seed 1 gave one trace twice.  A failure prints `seed S'; --seeds S:S replays it
#                                         and prints its trace.  NH_SCHED_CFLAGS adds compiler flags.
#   workers-harness.sh --sched --exhaustive [--preemptions K] [--max-runs N] MODULE...
#                                         the same drivers, every schedule with at most K preemptions (default 2,
#                                         fuzz/sched-pct.h's sched_explore); exit 0 = every schedule passed.  A failure
#                                         prints `schedule [C1,...]'; running the driver with x:K:C1,... replays it and
#                                         prints its trace.  Over N schedules (default 200000) a module fails with its
#                                         count: narrow the driver, never the bound.  Module selftest is the mode's own
#                                         test: clean at K=0, red at each K from 1 on, the same failures on two runs.
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

if [ "${1:-}" = --sched ]; then
  shift
  mods=() seeds=2000 exhaustive=0 k=2 max=200000
  while [ $# -gt 0 ]; do
    case $1 in
      --seeds) seeds=${2:?--seeds N or S:S}; shift 2 ;;
      --exhaustive) exhaustive=1; shift ;;
      --preemptions) k=${2:?--preemptions K}; shift 2 ;;
      --max-runs) max=${2:?--max-runs N}; shift 2 ;;
      *) mods+=("$1"); shift ;;
    esac
  done
  [ ${#mods[@]} -gt 0 ] || { echo "workers-harness: --sched needs a module" >&2; exit 2; }
  out=$(mktemp -d)
  trap 'rm -r "$out"' EXIT
  fail=0
  for m in "${mods[@]}"; do
    src="$ROOT/test/src/nidhogg-tests/fuzz/sched-$m.c"
    if [ ! -f "$src" ]; then
      echo "FAIL no $m module"; fail=1; continue
    fi
    # The exhaustive mode builds the drivers at their small sizes (NH_SCHED_SMALL), under the schedule cap.
    small=; [ $exhaustive = 0 ] || small=-DNH_SCHED_SMALL
    # shellcheck disable=SC2086
    if ! "$CC" "${FLAGS[@]}" -DNH_SCHED $small ${NH_SCHED_CFLAGS:-} -g -O1 -pthread "$src" -o "$out/$m"; then
      echo "FAIL $m (build)"; fail=1; continue
    fi
    if [ $exhaustive = 1 ] && [ "$m" = selftest ]; then
      # The mode's own test: no failure at K=0; at each K from 1 to the bound the lost update found (exit 1, the
      # closed-form count holds: exit 4 if not), twice, with the same output.
      bad= rc=0
      "$out/$m" x:0 "$max" > "$out/s0" 2>&1 || rc=$?
      [ $rc = 0 ] || bad="K=0: exit $rc: $(tail -1 "$out/s0")"
      summary="K=0 $(grep -o '[0-9]* schedules, [0-9]* failed' "$out/s0" || true)"
      for ((j = 1; j <= k; j++)); do
        [ -z "$bad" ] || break
        ra=0 rb=0
        "$out/$m" "x:$j" "$max" > "$out/sa" 2>&1 || ra=$?
        "$out/$m" "x:$j" "$max" > "$out/sb" 2>&1 || rb=$?
        if [ $ra != 1 ] || [ $rb != 1 ]; then
          bad="K=$j: exit $ra, then $rb (want 1, the lost update found): $(tail -1 "$out/sa")"
        elif ! cmp -s "$out/sa" "$out/sb"; then
          bad="K=$j: two runs gave different failures"
        else
          summary+="; K=$j $(grep -o '[0-9]* schedules, [0-9]* failed' "$out/sa"), $(grep -m1 '^schedule' "$out/sa") twice"
        fi
      done
      if [ -z "$bad" ]; then echo "ok   $m ($summary)"; else echo "FAIL $m ($bad)"; fail=1; fi
      continue
    fi
    if [ $exhaustive = 1 ]; then
      rc=0
      "$out/$m" "x:$k" "$max" || rc=$?
      case $rc in
        0) echo "ok   $m" ;;
        3) echo "FAIL $m (over $max schedules: narrow the driver)"; fail=1 ;;
        *) echo "FAIL $m"; fail=1 ;;
      esac
      continue
    fi
    if ! "$out/$m" "$seeds"; then
      echo "FAIL $m"; fail=1; continue
    fi
    a=$("$out/$m" 1:1 | grep '^trace' || true) b=$("$out/$m" 1:1 | grep '^trace' || true)
    if [ -n "$a" ] && [ "$a" = "$b" ]; then
      echo "ok   $m ($a)"
    else
      echo "FAIL $m (seed 1 gave '$a', then '$b')"; fail=1
    fi
  done
  exit $fail
fi

FILES="nidhogg-workers nidhogg-treesit-bg nidhogg-io nidhogg-decode nidhogg-json-tape nidhogg-image-bg nidhogg-blk"

fail=0
if [ "${1:-}" = --fuzz ]; then
  # --fuzz SECONDS [TARGET...]: each fuzz/TARGET.c (default: all) for SECONDS, under ASan and UBSan when the
  # compiler has them (dnf installs the runtimes when run as root).  libFuzzer when the compiler has it, else
  # fuzz/driver.c (mutations of the seeds under fixtures/images, or the target's own fixtures, fixed random seed).
  # Targets: json, decode, header, cores, framers (Task 24: content-length, lines and records, whole and cut).
  SECS=${2:?--fuzz SECONDS}; shift 2
  FZ="$ROOT/test/src/nidhogg-tests/fuzz"; OUT=${TMPDIR:-/tmp}/nh-fuzz-$$; mkdir -p "$OUT"
  SAN=-fsanitize=address,undefined
  if ! echo 'int main(void){return 0;}' | "$CC" $SAN -x c - -o "$OUT/probe" 2>/dev/null; then
    [ "$(id -u)" = 0 ] && dnf install -y -q libasan libubsan >/dev/null 2>&1 || true
    echo 'int main(void){return 0;}' | "$CC" $SAN -x c - -o "$OUT/probe" 2>/dev/null || { SAN=; echo "fuzz: no sanitizers"; }
  fi
  TARGETS=${*:-$(cd "$FZ" && ls *.c | grep -v '^driver\.c$' | grep -v -E '^(tsan|sched)-' | sed 's/\.c$//')}
  for t in $TARGETS; do
    # this target's result (fail is the whole run's: an earlier target's failure must not mark this one)
    DEFS=; LIBS=; bad=0; SEEDS=images
    case $t in header) SRCS="nidhogg-image-bg" ;; json) SRCS="" ;;
      # Task 18: the stateless decoders
      decode) SRCS="nidhogg-decode" ;;
      # Task 24: the framers (the target includes nidhogg-io.c and nidhogg-json-tape.c), seeded with recorded streams
      framers) SRCS="nidhogg-workers nidhogg-decode"; SEEDS="lsp ndjson" ;;
      # Task 28: the image decoder cores, with the decoder libraries
      cores) SRCS="nidhogg-image-bg"; DEFS="-DHAVE_PNG -DHAVE_JPEG -DHAVE_GIF -DHAVE_WEBP"
             LIBS="-lpng -ljpeg -lgif -lwebp -lwebpdemux -lm" ;;
      *) echo "fuzz: unknown target $t"; fail=1; continue ;; esac
    srcs=(); for m in $SRCS; do srcs+=("$ROOT/src/$m.c"); done
    seeds=(); for d in $SEEDS; do seeds+=("$ROOT"/test/src/nidhogg-tests/fixtures/$d/*); done
    if echo 'int main(void){return 0;}' | "$CC" -fsanitize=fuzzer -x c - -o "$OUT/probe" 2>/dev/null; then
      "$CC" -std=gnu11 -g -O1 $SAN -fsanitize=fuzzer $DEFS -DHAVE_NIDHOGG_WORKERS -DNH_WK_HARNESS -I"$ROOT/src" \
        "$FZ/$t.c" "${srcs[@]}" $LIBS -o "$OUT/$t" && "$OUT/$t" -max_total_time="$SECS" -seed=1 "${seeds[@]}" || bad=1
    else
      "$CC" -std=gnu11 -g -O1 $SAN $DEFS -DHAVE_NIDHOGG_WORKERS -DNH_WK_HARNESS -I"$ROOT/src" \
        "$FZ/$t.c" "$FZ/driver.c" "${srcs[@]}" $LIBS -o "$OUT/$t" && "$OUT/$t" "$SECS" "${seeds[@]}" || bad=1
    fi
    if [ $bad = 0 ]; then echo "ok   fuzz $t"; else echo "FAIL fuzz $t"; fail=1; fi
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

#!/usr/bin/env bash
# The stall watchdog's park protocol on real threads (src/nidhogg-watchdog-park.h).
#
#   bash nidhogg/test/watchdog-park-test.sh [--container fedora-45] [--cycles N] [--tsan-cycles N]
#
# Needs gcc with ThreadSanitizer (libtsan), nothing from an Emacs build; --container installs both in a
# throwaway Fedora container, so the host needs neither.
# It compiles the header's steps with a two-thread driver: a main thread that flips busy/waiting
# (default 10^7 cycles) against a monitor that parks with the same steps the watchdog's monitor uses.
# After each busy edge the main thread waits until the monitor has seen the span. Every park is a
# condition-variable wait bounded by 100 ms: a park that times out and then finds a busy span outlived
# that span without a wake-up, and counts as lost.
#   1. the interleaving model (the header's own, store buffers modelled): lost must be 0;
#   2. -fsanitize=thread (default 10^5 cycles): no data race on the driver's or the header's words;
#      TSan cannot see a Dekker ordering failure, so this is not the ordering check;
#   3. an optimised stress run (x86 reorders a store before a later load, so a weakened order can
#      lose wake-ups there): lost must be 0.
# NH_WD_PARK_TEST_ORDER=__ATOMIC_RELAXED (say) compiles every run with that order instead of the
# header's (the mutation check). Exit 0: no race reported and lost 0 everywhere; 1 otherwise; 2 usage.
set -euo pipefail
root=$(cd "$(dirname "$0")/../.." && pwd)
H=$root/src/nidhogg-watchdog-park.h
cycles=10000000 tsan_cycles=100000 distro=
while [ $# -gt 0 ]; do
  case "$1" in
    --cycles) cycles=$2; shift 2 ;;
    --tsan-cycles) tsan_cycles=$2; shift 2 ;;
    --container) distro=$2; shift 2 ;;
    *) echo "usage: watchdog-park-test.sh [--container fedora-45] [--cycles N] [--tsan-cycles N]" >&2; exit 2 ;;
  esac
done
# --container: the same script inside a memory-capped Fedora container that installs gcc and libtsan itself
# (the host needs neither); the checkout is mounted read-only.
if [ -n "$distro" ]; then
  command -v podman >/dev/null 2>&1 || { echo "park test: podman not found" >&2; exit 1; }
  exec podman run --rm --name "nhpark-$$" --memory 2g --memory-swap 2g -v "$root:/src:ro,z" \
    "registry.fedoraproject.org/fedora:${distro#fedora-}" bash -euo pipefail -c \
    'dnf -y -q --setopt=install_weak_deps=False install gcc libtsan glibc-devel >/dev/null && exec bash /src/nidhogg/test/watchdog-park-test.sh "$@"' \
    _ --cycles "$cycles" --tsan-cycles "$tsan_cycles"
fi
for f in nh_wd_park_busy nh_wd_park_must_wait nh_wd_park_leave nh_wd_park_model_run; do
  grep -q "^$f " "$H" 2>/dev/null || { echo "missing nidhogg-watchdog-park.h steps ($f)"; exit 1; }
done
order=()
[ -z "${NH_WD_PARK_TEST_ORDER:-}" ] || order=(-DNH_WD_PARK_ORDER="$NH_WD_PARK_TEST_ORDER")
tmp=$(mktemp -d); trap 'rm -r "$tmp"' EXIT

cat > "$tmp/driver.c" <<'EOF'
#define _GNU_SOURCE
#include <errno.h>
#include <pthread.h>
#include <stdio.h>
#include <stdlib.h>
#include <time.h>
#include "nidhogg-watchdog-park.h"

static uint64_t span;
static uint32_t waiting = 1, parked;
static uint64_t seen;		/* the busy span the monitor saw last */
static uint32_t stop;
static long lost;
static pthread_mutex_t m = PTHREAD_MUTEX_INITIALIZER;
static pthread_cond_t cv;

static uint64_t
now_ns (void)
{
  struct timespec ts;
  clock_gettime (CLOCK_MONOTONIC, &ts);
  return ts.tv_sec * 1000000000ull + ts.tv_nsec;
}

static void *
monitor (void *arg)
{
  struct nh_wd_park_mem mem = { &span, &waiting, &parked, NULL };
  uint64_t last = 0;
  (void) arg;
  pthread_mutex_lock (&m);
  while (!__atomic_load_n (&stop, __ATOMIC_ACQUIRE))
    {
      bool wait = nh_wd_park_must_wait (&mem, 0, last, true);
      while (wait && !__atomic_load_n (&stop, __ATOMIC_ACQUIRE))
	{
	  uint64_t t = now_ns () + 100000000ull;
	  struct timespec ts = { t / 1000000000ull, t % 1000000000ull };
	  int rc = pthread_cond_timedwait (&cv, &m, &ts);
	  wait = nh_wd_park_must_wait (&mem, 1, last, true);
	  if (rc == ETIMEDOUT && !wait)
	    lost++;
	}
      nh_wd_park_leave (&mem);
      last = __atomic_load_n (&span, __ATOMIC_SEQ_CST);
      if (!__atomic_load_n (&waiting, __ATOMIC_SEQ_CST))
	__atomic_store_n (&seen, last, __ATOMIC_RELEASE);
      /* Let the main thread take the mutex to signal.  */
      pthread_mutex_unlock (&m);
      pthread_mutex_lock (&m);
    }
  pthread_mutex_unlock (&m);
  return NULL;
}

int
main (int argc, char **argv)
{
  long cycles = atol (argv[1]);
  struct nh_wd_park_mem mem = { &span, &waiting, &parked, NULL };
  long cases, model_lost;
  pthread_condattr_t ca;
  pthread_t mon;

  nh_wd_park_model_run (&cases, &model_lost);
  printf ("model: %ld interleavings, lost %ld\n", cases, model_lost);

  pthread_condattr_init (&ca);
  pthread_condattr_setclock (&ca, CLOCK_MONOTONIC);
  pthread_cond_init (&cv, &ca);
  pthread_create (&mon, NULL, monitor, NULL);
  for (long i = 1; i <= cycles; i++)
    {
      if (nh_wd_park_busy (&mem, i))
	{
	  pthread_mutex_lock (&m);
	  pthread_cond_signal (&cv);
	  pthread_mutex_unlock (&m);
	}
      uint64_t t0 = now_ns ();
      while (__atomic_load_n (&seen, __ATOMIC_ACQUIRE) != (uint64_t) i)
	if (now_ns () - t0 > 5000000000ull)
	  {
	    printf ("stuck at cycle %ld: the monitor never saw the span\n", i);
	    return 1;
	  }
      __atomic_store_n (&waiting, 1, NH_WD_PARK_ORDER);
    }
  __atomic_store_n (&stop, 1, __ATOMIC_RELEASE);
  pthread_mutex_lock (&m);
  pthread_cond_broadcast (&cv);
  pthread_mutex_unlock (&m);
  pthread_join (mon, NULL);
  printf ("threads: %ld cycles, lost %ld\n", cycles, lost);
  return model_lost || lost;
}
EOF

rc=0
echo "== tsan ($tsan_cycles cycles)"
gcc -std=gnu11 -O1 -g -fsanitize=thread -pthread -I"$root/src" ${order[@]+"${order[@]}"} \
  -o "$tmp/tsan" "$tmp/driver.c"
TSAN_OPTIONS='halt_on_error=1 exitcode=66' "$tmp/tsan" "$tsan_cycles" || rc=1
echo "== stress ($cycles cycles)"
gcc -std=gnu11 -O2 -pthread -I"$root/src" ${order[@]+"${order[@]}"} -o "$tmp/stress" "$tmp/driver.c"
"$tmp/stress" "$cycles" || rc=1
[ "$rc" = 0 ] && echo "park test: ok" || echo "park test: FAILED"
exit "$rc"

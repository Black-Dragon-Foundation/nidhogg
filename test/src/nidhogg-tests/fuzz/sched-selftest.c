/* The exhaustive mode's own test (workers-harness.sh --sched --exhaustive
   selftest): two threads each add one to a counter, the read and the
   write split by a sched point, among M points in all.  The lost update
   needs one preemption, at that point: with K = 0 every schedule holds,
   with K >= 1 some fail.  The number of schedules must equal its closed
   form, checked here; that the failures are the same on every run, the
   harness checks.

Copyright (C) 2026 Black Dragon Foundation contributors.

This file is part of Nidhogg.

Nidhogg is free software: you can redistribute it and/or modify
it under the terms of the GNU General Public License as published by
the Free Software Foundation, either version 3 of the License, or (at
your option) any later version.

Nidhogg is distributed in the hope that it will be useful,
but WITHOUT ANY WARRANTY; without even the implied warranty of
MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
GNU General Public License for more details.

You should have received a copy of the GNU General Public License
along with Nidhogg.  If not, see <https://www.gnu.org/licenses/>.  */

#include "sched-pct.h"

enum { M = 4 };

static long counter;
static int finished;
static pthread_mutex_t m = PTHREAD_MUTEX_INITIALIZER;
static pthread_cond_t c = PTHREAD_COND_INITIALIZER;

/* NH_SCHED_SELFTEST_DIVERGE: a driver that is not deterministic, to test
   that the explorer notices.  Every other schedule run, the second adder
   skips its first points, so it ends earlier.  RUNS counts the runs, in memory shared with
   the children.  */
static long *runs;

static void *
adder (void *arg)
{
  for (int i = 0; i < M; i++)
    if (i < M / 2 && arg && runs && (*runs & 1))
      continue;
    else if (i == M / 2)
      {
	long v = counter;
	NH_SCHED_POINT ();
	counter = v + 1;
      }
    else
      NH_SCHED_POINT ();
  /* Only the last one wakes the main thread, which then has nothing to
     choose: the count stays the two threads'.  */
  pthread_mutex_lock (&m);
  if (++finished == 2)
    pthread_cond_signal (&c);
  pthread_mutex_unlock (&m);
  return NULL;
}

static int
body (void)
{
  pthread_t t[2];
  if (runs)
    ++*runs;
  for (int i = 0; i < 2; i++)
    if (pthread_create (&t[i], NULL, adder, i ? (void *) 1 : NULL) != 0)
      return 2;
  pthread_mutex_lock (&m);
  while (finished < 2)
    pthread_cond_wait (&c, &m);
  pthread_mutex_unlock (&m);
  if (counter != 2)
    {
      fprintf (stderr, "selftest: an update was lost (counter %ld)\n",
	       counter);
      return 1;
    }
  return 0;
}

static long
choose (int n, int r)
{
  if (r < 0 || r > n)
    return 0;
  long v = 1;
  for (int i = 1; i <= r; i++)
    v = v * (n - r + i) / i;
  return v;
}

/* Schedules of two threads of M + 1 segments each, with at most K
   preemptions.  P preemptions make P + 2 runs, alternating, the last
   switch free (a thread's end); either thread can start, and each splits
   its segments into its runs.  */
static long
closed_form (int k)
{
  long n = 0;
  for (int p = 0; p <= k; p++)
    {
      int r = p + 2;
      n += 2 * choose (M, (r + 1) / 2 - 1) * choose (M, r / 2 - 1);
    }
  return n;
}

int
main (int argc, char **argv)
{
  if (getenv ("NH_SCHED_SELFTEST_DIVERGE"))
    {
      runs = mmap (NULL, sizeof *runs, PROT_READ | PROT_WRITE,
		   MAP_SHARED | MAP_ANONYMOUS, -1, 0);
      if (runs == MAP_FAILED)
	return 2;
    }
  int rc = sched_main (argc, argv, 40, body);
  if ((rc == 0 || rc == 1) && sched_x.k >= 0 && sched_x.runs != closed_form (sched_x.k))
    {
      printf ("selftest: %ld schedules for K=%d, the closed form gives %ld\n",
	      sched_x.runs, sched_x.k, closed_form (sched_x.k));
      return 4;
    }
  return rc;
}

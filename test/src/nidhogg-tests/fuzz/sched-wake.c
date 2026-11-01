/* The completion channel's clear-then-re-check under exact thread orders
   (workers-harness.sh --sched wake): two producers push, one consumer
   drains as the drainer does.  A job left in the FIFO with the fd clear
   and nothing armed is a lost wake-up.

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
#include "nidhogg-workers.c"

enum { PRODUCERS = 2, JOBS = 4 };

static struct nh_wk_job jobs[PRODUCERS][JOBS];
static _Atomic int producers_done;

static void *
producer (void *arg)
{
  struct nh_wk_job *j = arg;
  for (int i = 0; i < JOBS; i++)
    nh_wk_cq_push (&nh_wk_race_q, &j[i]);
  atomic_fetch_add (&producers_done, 1);
  return NULL;
}

static int
body (void)
{
  if (!nh_wk_race_open ())
    return 2;
  /* The fd readable and the FIFO empty, as after a push whose write came
     once the drain had taken its job: the order the re-check is for.  */
  nh_wk_wake_signal (&nh_wk_race_q.wake);
  pthread_t t[PRODUCERS];
  for (int p = 0; p < PRODUCERS; p++)
    if (pthread_create (&t[p], NULL, producer, jobs[p]) != 0)
      return 2;
  int popped = 0;
  while (popped < PRODUCERS * JOBS)
    {
      bool readable = nh_wk_wake_readable (&nh_wk_race_q.wake);
      if (readable)
	nh_wk_cq_ready (&nh_wk_race_q);
      if (atomic_exchange (&nh_wk_race_armed, false))
	while (nh_wk_cq_pop (&nh_wk_race_q))
	  popped++;
      else if (!readable && atomic_load (&producers_done) == PRODUCERS
	       && nh_wk_cq_nonempty (&nh_wk_race_q))
	{
	  fprintf (stderr, "wake: a job in the FIFO, the fd clear, nothing armed\n");
	  return 1;
	}
      sched_yield ();
    }
  return 0;
}

int
main (int argc, char **argv)
{
  return sched_main (argc, argv, 80, body);
}

/* The runtime's queues under exact thread orders (workers-harness.sh
   --sched queue): two producers submit to nh-blk and nh-img, some
   speculative, and wait on some; the pools' threads run them; the main
   thread drains the completion FIFO.  Every job runs, lands and is freed
   once, no wake-up is lost and nothing deadlocks.

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

enum { PRODUCERS = 2, JOBS = 6 };

static _Atomic long ran, finished, freed, refused;

static void
q_run (struct nh_wk_job *j)
{
  (void) j;
  atomic_fetch_add (&ran, 1);
}

static void
q_finish (struct nh_wk_job *j)
{
  (void) j;
  atomic_fetch_add (&finished, 1);
}

static void
q_free (struct nh_wk_job *j)
{
  atomic_fetch_add (&freed, 1);
  free (j);
}

static void *
producer (void *arg)
{
  int p = (int) (intptr_t) arg;
  for (int i = 0; i < JOBS; i++)
    {
      struct nh_wk_job *j = calloc (1, sizeof *j);
      if (!j)
	abort ();
      j->w = p == 0 ? NH_WK_BLK : NH_WK_IMG;
      j->cls = i % NH_WK_CL_N;
      j->speculative = p == 1 && (i & 1);
      j->run = q_run;
      j->finish = q_finish;
      j->free = q_free;
      atomic_store (&j->refs, 1);
      if (!nh_wk_submit (j))
	{
	  atomic_fetch_add (&refused, 1);
	  nh_wk_job_unref (j);
	  continue;
	}
      if (i % 3 == 0 && nh_wk_wait (j, 10 * NH_WK_MS * 1000, 0, 0) != 1)
	{
	  fprintf (stderr, "queue: a wait did not see its job done\n");
	  _exit (1);
	}
      nh_wk_job_unref (j);
    }
  return NULL;
}

static int
body (void)
{
  nh_wk_init ();
  if (!nh_wk_done_open ())
    return 2;
  pthread_t t[PRODUCERS];
  for (intptr_t p = 0; p < PRODUCERS; p++)
    if (pthread_create (&t[p], NULL, producer, (void *) p) != 0)
      return 2;
  /* A wake-up lost leaves this loop with nothing to do: the scheduler's
     step limit ends the seed.  */
  while (atomic_load (&freed) < PRODUCERS * JOBS)
    {
      if (nh_wk_wake_readable (&nh_wk_done_q.wake))
	nh_wk_cq_ready (&nh_wk_done_q);
      if (atomic_exchange (&nh_wk_harness_armed, false))
	{
	  struct nh_wk_job *j;
	  while ((j = nh_wk_cq_pop (&nh_wk_done_q)))
	    {
	      j->finish (j);
	      nh_wk_job_unref (j);
	    }
	}
      sched_yield ();
    }
  long done = atomic_load (&ran) + atomic_load (&refused);
  return done == PRODUCERS * JOBS && atomic_load (&finished) == atomic_load (&ran)
	 ? 0 : 1;
}

int
main (int argc, char **argv)
{
  return sched_main (argc, argv, 380, body);
}

/* The workers' queues under ThreadSanitizer (workers-harness.sh --tsan
   queue): two producers submit 10^6 jobs in all (nh-blk across its three
   classes, nh-img half speculative, every 1000th waited on), one
   consumer lands them through the completion fd as Emacs does.  Exit 0 =
   every job ran, landed once and was freed, no wake-up was lost (and,
   under TSan, no race report).

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

#include "nidhogg-workers.c"

enum { NJOBS = 1000000, INFLIGHT = 20000 };

static _Atomic long ran, finished, freed, submitted, refused, waited_bad;

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
  for (long i = 0; i < NJOBS / 2; i++)
    {
      while (atomic_load (&submitted) - atomic_load (&finished) > INFLIGHT)
	sched_yield ();
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
      atomic_fetch_add (&submitted, 1);
      if (i % 1000 == 0 && nh_wk_wait (j, 10 * NH_WK_MS * 1000, 0, 0) != 1)
	atomic_fetch_add (&waited_bad, 1);
      nh_wk_job_unref (j);
    }
  return NULL;
}

int
main (void)
{
  pthread_t t[2];
  int lost = 0, idle_polls = 0;

  nh_wk_init ();
  if (!nh_wk_done_open ())
    {
      fprintf (stderr, "queue: cannot open the completion fd\n");
      return 1;
    }
  for (intptr_t p = 0; p < 2; p++)
    if (pthread_create (&t[p], NULL, producer, (void *) p) != 0)
      abort ();

  while (atomic_load (&finished) < NJOBS - atomic_load (&refused))
    {
      struct pollfd pfd = { .fd = nh_wk_done_q.wake.rfd, .events = POLLIN };
      if (poll (&pfd, 1, 2000) > 0)
	{
	  idle_polls = 0;
	  nh_wk_cq_ready (&nh_wk_done_q);
	}
      else if (nh_wk_cq_nonempty (&nh_wk_done_q)
	       && !atomic_load (&nh_wk_harness_armed))
	{
	  lost++;
	  atomic_store (&nh_wk_harness_armed, true);
	}
      else if (++idle_polls > 30)
	{
	  fprintf (stderr, "queue: no progress for 60 s (%ld of %d landed)\n",
		   atomic_load (&finished), NJOBS);
	  return 1;
	}
      if (atomic_exchange (&nh_wk_harness_armed, false))
	{
	  struct nh_wk_job *j;
	  while ((j = nh_wk_cq_pop (&nh_wk_done_q)))
	    {
	      j->finish (j);
	      nh_wk_job_unref (j);
	    }
	}
    }
  for (int p = 0; p < 2; p++)
    pthread_join (t[p], NULL);

  int blk;
  pthread_mutex_lock (&nh_wk_w[NH_WK_BLK].m);
  blk = nh_wk_w[NH_WK_BLK].pool[NH_WK_POOL_NORMAL].threads;
  pthread_mutex_unlock (&nh_wk_w[NH_WK_BLK].m);
  printf ("queue: %ld ran, %ld landed, %ld freed, %ld refused, %ld bad waits,"
	  " %d lost wake-ups, %d nh-blk threads\n",
	  atomic_load (&ran), atomic_load (&finished), atomic_load (&freed),
	  atomic_load (&refused), atomic_load (&waited_bad), lost, blk);
  return !(atomic_load (&ran) == NJOBS && atomic_load (&finished) == NJOBS
	   && atomic_load (&freed) == NJOBS && atomic_load (&refused) == 0
	   && atomic_load (&waited_bad) == 0 && lost == 0
	   && 1 <= blk && blk <= NH_WK_BLK_MAX);
}

/* Exact thread orders for the workers' C in the harness: PCT.

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

/* Probabilistic Concurrency Testing (Burckhardt et al., ASPLOS 2010),
   for depth 2: one thread runs at a time, the highest-priority one that
   can; priorities are random, and at one random step the running thread
   drops below all others.  An ordering bug of depth 1 shows at least
   once in about n runs of n threads, whatever the machine's timing.

   A driver includes this file, then nidhogg-workers.c (or nidhogg-io.c),
   and is built with -DNH_SCHED.  NH_CHAOS_POINT then means
   NH_SCHED_POINT, and everything through which a harness thread can
   block is the macros at the end of this file: every mutex, condition
   variable, thread creation, yield, sleep and clock read of the included
   C goes through the one running thread, so nothing holds the run token
   outside this file.  Time is virtual: it moves a little with each step
   and jumps to the next timeout only when no thread can run, so a seed
   gives one trace.  A fixed set of drivers: threads outside them, or a
   blocking read or poll, are not modelled.

   sched_main runs each seed in a child process, since the workers'
   state is global.  A driver's body returns 0 when its invariants hold,
   and its STEPS is about its longest run: the change point is drawn
   from there, so a seed past the run's end changes nothing.  A failed
   seed prints `seed S'; the argument S:S replays it and prints its
   trace.  */

#ifndef SCHED_PCT_H
#define SCHED_PCT_H

#ifndef _GNU_SOURCE
# define _GNU_SOURCE
#endif

#include <errno.h>
#include <pthread.h>
#include <sched.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>
#include <unistd.h>
#include <sys/wait.h>

#define SCHED_THREADS 32
/* Steps of one seed before it counts as a livelock.  */
#define SCHED_LIMIT 2000000

struct sched_thread
{
  pthread_cond_t go;
  bool ready;			/* else it waits, or is done */
  bool done;
  bool yielded;			/* gave way until another thread stepped */
  bool timed_out;
  const void *on;		/* the mutex or condition it waits on */
  int64_t until;		/* virtual ns; -1: no timeout */
  uint64_t prio;
  void *(*fn) (void *);
  void *arg;
};

static struct
{
  pthread_mutex_t m;
  struct sched_thread t[SCHED_THREADS];
  int n, cur;
  uint64_t rng, hash;
  long step, change_at;
  int64_t now;
} sched = { .m = PTHREAD_MUTEX_INITIALIZER };

static _Thread_local int sched_self;

static uint64_t
sched_random (void)
{
  uint64_t x = sched.rng;
  x ^= x << 13;
  x ^= x >> 7;
  x ^= x << 17;
  sched.rng = x;
  return x;
}

static void
sched_fail (const char *why)
{
  fprintf (stderr, "sched: %s at step %ld\n", why, sched.step);
  fflush (NULL);
  _exit (1);
}

/* The thread to run next.  */
static int
sched_pick (void)
{
  int best = -1;
  for (int pass = 0; pass < 2 && best < 0; pass++)
    for (int i = 0; i < sched.n; i++)
      if (sched.t[i].ready && !sched.t[i].done
	  && (pass == 1 || !sched.t[i].yielded)
	  && (best < 0 || sched.t[i].prio > sched.t[best].prio))
	best = i;
  if (best < 0)
    {
      /* Nothing can run: the next timeout comes due.  */
      for (int i = 0; i < sched.n; i++)
	if (!sched.t[i].done && !sched.t[i].ready && sched.t[i].until >= 0
	    && (best < 0 || sched.t[i].until < sched.t[best].until))
	  best = i;
      if (best < 0)
	sched_fail ("deadlock");
      if (sched.t[best].until > sched.now)
	sched.now = sched.t[best].until;
      sched.t[best].ready = true;
      sched.t[best].timed_out = true;
      sched.t[best].on = NULL;
    }
  for (int i = 0; i < sched.n; i++)
    if (i != best)
      sched.t[i].yielded = false;
  return best;
}

/* With sched.m held: one step.  Return when this thread runs again; a
   thread that is done does not wait.  */
static void
sched_step (void)
{
  int me = sched_self;
  sched.now += 1000;
  if (++sched.step == sched.change_at)
    sched.t[sched.cur].prio = 1;
  if (sched.step > SCHED_LIMIT)
    sched_fail ("livelock");
  int next = sched_pick ();
  sched.hash = (sched.hash ^ (uint64_t) next) * UINT64_C (1099511628211);
  sched.cur = next;
  if (next == me)
    return;
  pthread_cond_signal (&sched.t[next].go);
  if (!sched.t[me].done)
    while (sched.cur != me)
      pthread_cond_wait (&sched.t[me].go, &sched.m);
}

static inline void
sched_point (void)
{
  pthread_mutex_lock (&sched.m);
  sched_step ();
  pthread_mutex_unlock (&sched.m);
}

#define NH_SCHED_POINT() sched_point ()

/* Give way until another thread has stepped.  A loop that waits for
   another thread to do something says so with this.  */
static __attribute__ ((unused)) int
sched_yield_step (void)
{
  pthread_mutex_lock (&sched.m);
  sched.t[sched_self].yielded = true;
  sched_step ();
  pthread_mutex_unlock (&sched.m);
  return 0;
}

/* Wait on ON until sched_wake, or until the virtual time UNTIL (-1:
   none).  Return whether it timed out.  */
static bool
sched_block (const void *on, int64_t until)
{
  pthread_mutex_lock (&sched.m);
  struct sched_thread *t = &sched.t[sched_self];
  t->ready = false;
  t->on = on;
  t->until = until;
  t->timed_out = false;
  sched_step ();
  bool timed_out = t->timed_out;
  pthread_mutex_unlock (&sched.m);
  return timed_out;
}

static void
sched_wake (const void *on)
{
  pthread_mutex_lock (&sched.m);
  for (int i = 0; i < sched.n; i++)
    if (!sched.t[i].ready && !sched.t[i].done && sched.t[i].on == on)
      {
	sched.t[i].ready = true;
	sched.t[i].on = NULL;
	sched.t[i].until = -1;
      }
  pthread_mutex_unlock (&sched.m);
}

static int
sched_mutex_lock (pthread_mutex_t *m)
{
  while (pthread_mutex_trylock (m) != 0)
    sched_block (m, -1);
  return 0;
}

static int
sched_mutex_unlock (pthread_mutex_t *m)
{
  pthread_mutex_unlock (m);
  sched_wake (m);
  return 0;
}

static __attribute__ ((unused)) int
sched_cond_wait (pthread_cond_t *c, pthread_mutex_t *m)
{
  sched_mutex_unlock (m);
  sched_block (c, -1);
  return sched_mutex_lock (m);
}

static __attribute__ ((unused)) int
sched_cond_timedwait (pthread_cond_t *c, pthread_mutex_t *m,
		      const struct timespec *at)
{
  int64_t until = at->tv_sec * INT64_C (1000000000) + at->tv_nsec;
  sched_mutex_unlock (m);
  bool timed_out = sched_block (c, until);
  sched_mutex_lock (m);
  return timed_out ? ETIMEDOUT : 0;
}

static __attribute__ ((unused)) int
sched_cond_wake (pthread_cond_t *c)
{
  sched_wake (c);
  return 0;
}

static void *
sched_trampoline (void *arg)
{
  int me = (int) (intptr_t) arg;
  sched_self = me;
  pthread_mutex_lock (&sched.m);
  while (sched.cur != me)
    pthread_cond_wait (&sched.t[me].go, &sched.m);
  pthread_mutex_unlock (&sched.m);
  void *r = sched.t[me].fn (sched.t[me].arg);
  pthread_mutex_lock (&sched.m);
  sched.t[me].done = true;
  sched_step ();
  pthread_mutex_unlock (&sched.m);
  return r;
}

static void
sched_add (int id, void *(*fn) (void *), void *arg)
{
  struct sched_thread *t = &sched.t[id];
  pthread_cond_init (&t->go, NULL);
  t->ready = true;
  t->until = -1;
  /* Distinct and above the changed priority, 1.  */
  t->prio = sched_random () >> 1 | 2;
  t->fn = fn;
  t->arg = arg;
}

static __attribute__ ((unused)) int
sched_create (pthread_t *thread, const pthread_attr_t *attr,
	      void *(*fn) (void *), void *arg)
{
  pthread_mutex_lock (&sched.m);
  if (sched.n == SCHED_THREADS)
    sched_fail ("too many threads");
  int id = sched.n++;
  sched_add (id, fn, arg);
  pthread_mutex_unlock (&sched.m);
  return pthread_create (thread, attr, sched_trampoline,
			 (void *) (intptr_t) id);
}

static __attribute__ ((unused)) int
sched_clock (clockid_t clock, struct timespec *ts)
{
  (void) clock;
  ts->tv_sec = sched.now / 1000000000;
  ts->tv_nsec = sched.now % 1000000000;
  return 0;
}

static __attribute__ ((unused)) int
sched_sleep (const struct timespec *req, struct timespec *rem)
{
  (void) rem;
  sched_block (&sched_self,
	       sched.now + req->tv_sec * INT64_C (1000000000) + req->tv_nsec);
  return 0;
}

static __attribute__ ((unused)) int
sched_main (int argc, char **argv, long steps, int (*body) (void))
{
  long from = 1, to = 2000;
  bool one = false;
  if (argc > 1)
    {
      char *end;
      to = strtol (argv[1], &end, 10);
      if (*end == ':')
	{
	  from = to;
	  to = strtol (end + 1, NULL, 10);
	  one = from == to;
	}
    }
  int bad = 0;
  for (long seed = from; seed <= to; seed++)
    {
      fflush (NULL);
      pid_t pid = fork ();
      if (pid < 0)
	return 2;
      if (pid == 0)
	{
	  alarm (60);
	  sched.rng = ((uint64_t) seed * UINT64_C (0x9E3779B97F4A7C15)) | 1;
	  sched.hash = UINT64_C (14695981039346656037);
	  sched_random ();
	  sched.change_at = 1 + (long) (sched_random () % (uint64_t) steps);
	  sched.n = 1;
	  sched_add (0, NULL, NULL);
	  int rc = body ();
	  if (one)
	    printf ("trace %016llx in %ld steps\n",
		    (unsigned long long) sched.hash, sched.step);
	  fflush (NULL);
	  _exit (rc);
	}
      int st;
      if (waitpid (pid, &st, 0) != pid || !WIFEXITED (st)
	  || WEXITSTATUS (st) != 0)
	{
	  printf ("seed %ld\n", seed);
	  bad++;
	}
    }
  printf ("%ld seeds, %d failed\n", to - from + 1, bad);
  return bad != 0;
}

/* From here on the included C runs under the scheduler.  */
#define pthread_mutex_lock sched_mutex_lock
#define pthread_mutex_unlock sched_mutex_unlock
#define pthread_cond_wait sched_cond_wait
#define pthread_cond_timedwait sched_cond_timedwait
#define pthread_cond_broadcast sched_cond_wake
#define pthread_cond_signal sched_cond_wake
#define pthread_create sched_create
#define sched_yield sched_yield_step
#define nanosleep sched_sleep
#define clock_gettime sched_clock

#endif /* SCHED_PCT_H */

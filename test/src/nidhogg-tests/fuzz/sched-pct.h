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
   trace.

   The argument x:K runs the bounded exhaustive mode instead
   (sched_explore): every schedule with at most K preemptions, by
   depth-first search (Musuvathi and Qadeer's iterative context
   bounding, PLDI 2007).  A preemption is a switch away from a thread
   that could go on; a switch at a block or a thread's end is free, and
   every thread that can run then is tried.  A thread that yields could
   go on (it spins): there the next thread by number is the free
   choice.  Each schedule runs in a child that follows a forced prefix
   of choices, then the natural ones (the running thread goes on, after
   a yield the next one, else the lowest-numbered), and leaves its
   choices in
   memory shared with the parent, which backtracks.  A failed schedule
   prints `schedule [C1,C2,...]', the threads chosen at its first
   decisions; the argument x:K:C1,C2,... replays it and prints its
   trace.  A second argument caps the number of schedules (default
   200000): over it, the run exits 3.  */

#ifndef SCHED_PCT_H
#define SCHED_PCT_H

#ifndef _GNU_SOURCE
# define _GNU_SOURCE
#endif

#include <errno.h>
#include <fcntl.h>
#include <pthread.h>
#include <sched.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>
#include <unistd.h>
#include <sys/mman.h>
#include <sys/wait.h>

#define SCHED_THREADS 32
/* Steps of one seed before it counts as a livelock.  */
#define SCHED_LIMIT 2000000
/* Decisions of one schedule the exhaustive mode can record.  */
#define SCHED_DECISIONS (1L << 20)

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

/* One decision of the exhaustive mode: a step at which more than one
   thread could run.  */
struct sched_choice
{
  uint8_t pick;			/* the thread that ran */
  uint8_t cur;			/* the thread that stepped */
  uint8_t natural;		/* the free choice; SCHED_FREE: any is */
  uint32_t enabled;		/* the threads that could run */
};
#define SCHED_FREE 0xff

static struct
{
  bool on;			/* in a child of sched_explore */
  bool trace;			/* print each decision (a replay) */
  int k;			/* the bound; -1: not the exhaustive mode */
  long runs;			/* schedules run */
  const uint8_t *forced;	/* the first choices */
  long nforced;
  long prev;			/* decisions of the run before, whose first
				   NFORCED the forced run must repeat */
  struct sched_choice *d;	/* shared with the parent */
  long *nd;
} sched_x = { .k = -1 };

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

/* The threads that can run: those that have not yielded, else any.  */
static uint32_t
sched_enabled (void)
{
  uint32_t en = 0;
  for (int pass = 0; pass < 2 && !en; pass++)
    for (int i = 0; i < sched.n; i++)
      if (sched.t[i].ready && !sched.t[i].done
	  && (pass == 1 || !sched.t[i].yielded))
	en |= UINT32_C (1) << i;
  return en;
}

/* The natural choice among EN, the threads that can run, when the
   running thread CUR steps: CUR if it can go on; after a yield the next
   thread by number (a thread that yields could go on: it spins); after
   a block or a thread's end, none (SCHED_FREE).  Another choice than
   the natural one is a preemption.  */
static int
sched_x_natural (uint32_t en, int cur)
{
  if (en >> cur & 1)
    return cur;
  if (!sched.t[cur].yielded || !sched.t[cur].ready || sched.t[cur].done)
    return SCHED_FREE;
  uint32_t above = en & ~((UINT32_C (2) << cur) - 1);
  return __builtin_ctz (above ? above : en);
}

/* The exhaustive mode's choice among EN: the forced one, else the
   natural one, else the lowest-numbered.  A step with one thread to run
   is no decision.  */
static int
sched_x_choose (uint32_t en)
{
  if (!(en & (en - 1)))
    return __builtin_ctz (en);
  int cur = sched.cur;
  int natural = sched_x_natural (en, cur);
  long d = *sched_x.nd;
  int pick = d < sched_x.nforced ? sched_x.forced[d]
    : natural != SCHED_FREE ? natural : __builtin_ctz (en);
  /* A replay of the run before up to its changed choice meets the same
     decisions: the same threads could run, the same one stepped.  The
     parent's log still holds that run's, until this one overwrites it.  */
  if (d < sched_x.prev && d < sched_x.nforced
      && (sched_x.d[d].enabled != en || sched_x.d[d].cur != cur))
    {
      fprintf (stderr, "sched: decision %ld differs from the run before: the"
	       " driver is not deterministic\n", d);
      fflush (NULL);
      _exit (6);
    }
  if (pick >= SCHED_THREADS || !(en >> pick & 1))
    {
      fprintf (stderr, "sched: decision %ld forces thread %d, which cannot"
	       " run: the driver is not deterministic\n", d, pick);
      fflush (NULL);
      _exit (5);
    }
  if (d == SCHED_DECISIONS)
    sched_fail ("too many decisions");
  sched_x.d[d] = (struct sched_choice) { pick, cur, natural, en };
  *sched_x.nd = d + 1;
  if (sched_x.trace)
    {
      printf ("decision %ld at step %ld: %d of {", d, sched.step, pick);
      for (int i = 0, sep = 0; i < sched.n; i++)
	if (en >> i & 1)
	  printf (sep++ ? ",%d" : "%d", i);
      printf ("}, %d %s%s\n", cur,
	      natural == SCHED_FREE ? "could not go on"
	      : natural == cur ? "could go on" : "yielded",
	      natural == SCHED_FREE || pick == natural ? "" : ": a preemption");
    }
  return pick;
}

/* The thread to run next.  */
static int
sched_pick (void)
{
  int best = -1;
  uint32_t en = sched_enabled ();
  if (en && sched_x.on)
    best = sched_x_choose (en);
  else
    for (int i = 0; i < sched.n; i++)
      if (en >> i & 1 && (best < 0 || sched.t[i].prio > sched.t[best].prio))
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

/* Run one schedule in a child: the choices FORCED[0..N), then the
   natural ones.  QUIET sends the child's stderr to /dev/null; TRACE
   prints its decisions.  Return the child's exit status, or 128 plus
   the signal that ended it; its decisions are in sched_x.d.  */
static int
sched_x_run (const uint8_t *forced, long n, int (*body) (void),
	     bool quiet, bool trace)
{
  *sched_x.nd = 0;
  fflush (NULL);
  pid_t pid = fork ();
  if (pid < 0)
    return -1;
  if (pid == 0)
    {
      alarm (60);
      if (quiet)
	{
	  int fd = open ("/dev/null", O_WRONLY);
	  if (fd >= 0)
	    dup2 (fd, 2);
	}
      sched.rng = UINT64_C (0x9E3779B97F4A7C15);
      sched.hash = UINT64_C (14695981039346656037);
      sched.change_at = 0;
      sched_x.on = true;
      sched_x.trace = trace;
      sched_x.forced = forced;
      sched_x.nforced = n;
      sched.n = 1;
      sched_add (0, NULL, NULL);
      int rc = body ();
      if (trace)
	printf ("trace %016llx in %ld steps, %ld decisions\n",
		(unsigned long long) sched.hash, sched.step, *sched_x.nd);
      fflush (NULL);
      _exit (rc);
    }
  int st;
  if (waitpid (pid, &st, 0) != pid)
    return -1;
  return WIFEXITED (st) ? WEXITSTATUS (st) : 128 + WTERMSIG (st);
}

/* The alternative to C's choice that comes after it, within BUDGET
   preemptions, or -1.  The order: the natural choice, then the others
   by number.  */
static int
sched_x_next (const struct sched_choice *c, int budget)
{
  if (c->natural != SCHED_FREE && budget < 1)
    return -1;			/* every other choice preempts */
  int natural = c->natural != SCHED_FREE ? c->natural
    : __builtin_ctz (c->enabled);
  for (int i = c->pick == natural ? 0 : c->pick + 1; i < SCHED_THREADS; i++)
    if (c->enabled >> i & 1 && i != natural)
      return i;
  return -1;
}

static void
sched_x_print (const char *what, const uint8_t *v, long n)
{
  printf ("%s [", what);
  for (long i = 0; i < n; i++)
    printf (i ? ",%d" : "%d", v[i]);
  printf ("]\n");
}

/* Every schedule of BODY with at most K preemptions, at most MAX of
   them.  Return 0 if each held, 1 if one failed, 3 over MAX, 2 on a
   harness error.  */
static int
sched_explore (int k, long max, int (*body) (void))
{
  uint8_t *prefix = malloc (SCHED_DECISIONS);
  uint8_t *bad = malloc (SCHED_DECISIONS);
  int *pre = malloc ((SCHED_DECISIONS + 1) * sizeof *pre);
  if (!prefix || !bad || !pre)
    return 2;
  long np = 0, nbad = -1, failed = 0, runs_prev = 0;
  sched_x.k = k;
  sched_x.runs = 0;
  for (;;)
    {
      if (sched_x.runs == max)
	{
	  printf ("over the bound: %ld schedules\n", sched_x.runs);
	  return 3;
	}
      sched_x.prev = runs_prev;
      int rc = sched_x_run (prefix, np, body, true, false);
      sched_x.runs++;
      /* A replay that ends before its forced choices are used up took
	 another path than the run it replays.  */
      if (rc >= 0 && *sched_x.nd < np)
	rc = 6;
      if (rc < 0 || rc == 5 || rc == 6)
	{
	  if (rc == 6)
	    printf ("the driver differs from the run before when replayed: ");
	  sched_x_print ("harness error in schedule", prefix, np);
	  return 2;
	}
      if (rc != 0)
	{
	  if (++failed <= 5)
	    sched_x_print ("schedule", prefix, np);
	  if (nbad < 0)
	    {
	      memcpy (bad, prefix, np);
	      nbad = np;
	    }
	}
      /* Back up to the last decision with a choice left.  */
      long n = *sched_x.nd;
      runs_prev = n;
      pre[0] = 0;
      for (long i = 0; i < n; i++)
	pre[i + 1] = pre[i] + (sched_x.d[i].natural != SCHED_FREE
			       && sched_x.d[i].pick != sched_x.d[i].natural);
      long i = n - 1;
      int alt = -1;
      for (; i >= 0 && alt < 0; i--)
	alt = sched_x_next (&sched_x.d[i], k - pre[i]);
      if (alt < 0)
	break;
      i++;
      for (long j = 0; j < i; j++)
	prefix[j] = sched_x.d[j].pick;
      prefix[i] = (uint8_t) alt;
      np = i + 1;
    }
  if (failed > 5)
    printf ("... and %ld more\n", failed - 5);
  printf ("exhaustive K=%d: %ld schedules, %ld failed\n", k, sched_x.runs,
	  failed);
  /* The first failure's own message.  */
  if (nbad >= 0)
    {
      sched_x.prev = 0;
      sched_x_run (bad, nbad, body, false, false);
    }
  return failed ? 1 : 0;
}

/* The argument x:K[:C1,C2,...] and the cap MAX: explore, or replay one
   schedule with its trace.  */
static int
sched_x_main (const char *arg, long max, int (*body) (void))
{
  char *end;
  int k = (int) strtol (arg + 2, &end, 10);
  if (end == arg + 2 || k < 0 || (*end && *end != ':'))
    {
      fprintf (stderr, "sched: x:K or x:K:C1,C2,... (got %s)\n", arg);
      return 2;
    }
  size_t size = SCHED_DECISIONS * sizeof *sched_x.d + sizeof (long);
  void *shared = mmap (NULL, size, PROT_READ | PROT_WRITE,
		       MAP_SHARED | MAP_ANONYMOUS, -1, 0);
  if (shared == MAP_FAILED)
    return 2;
  sched_x.d = shared;
  sched_x.nd = (long *) (sched_x.d + SCHED_DECISIONS);
  if (!*end)
    return sched_explore (k, max, body);
  static uint8_t choices[SCHED_DECISIONS];
  long n = 0;
  for (const char *p = end + 1; *p; n++)
    {
      long c = strtol (p, &end, 10);
      if (end == p || c < 0 || c >= SCHED_THREADS || n == SCHED_DECISIONS
	  || (*end && *end != ','))
	{
	  fprintf (stderr, "sched: bad schedule at %s\n", p);
	  return 2;
	}
      choices[n] = (uint8_t) c;
      p = *end ? end + 1 : end;
    }
  int rc = sched_x_run (choices, n, body, false, true);
  sched_x_print (rc ? "failed: schedule" : "held: schedule", choices, n);
  return rc != 0;
}

static __attribute__ ((unused)) int
sched_main (int argc, char **argv, long steps, int (*body) (void))
{
  long from = 1, to = 2000;
  bool one = false;
  if (argc > 1 && argv[1][0] == 'x' && argv[1][1] == ':')
    return sched_x_main (argv[1], argc > 2 ? strtol (argv[2], NULL, 10)
			 : 200000, body);
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

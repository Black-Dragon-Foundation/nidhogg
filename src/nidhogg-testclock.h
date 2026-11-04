/* Nidhogg's test clock: the hooks of Emacs's clock reads and waits.

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

/* nidhogg-testclock.c says what this is.  Off, each hook costs one
   atomic load.  */

#ifndef NIDHOGG_TESTCLOCK_H
#define NIDHOGG_TESTCLOCK_H

#include <stdint.h>

#include "systime.h"

#if defined HAVE_NIDHOGG_WORKERS && defined HAVE_GLITNIR

# define HAVE_NH_TEST_CLOCK 1

# include <pthread.h>
# include <stdatomic.h>
# include <sys/select.h>

/* The clock runs; it runs and auto-advances.  */
extern _Atomic bool nh_tc_on, nh_tc_auto;

extern struct timespec nh_tc_wall (void);
extern struct timespec nh_tc_mono (void);
extern uint64_t nh_tc_mono_ns (void);

/* What Lisp reads as the time.  */
static inline struct timespec
nh_tc_now (void)
{
  return nh_tc_on ? nh_tc_wall () : current_timespec ();
}

/* What a wait's own timeout runs on: the clock when it auto-advances.
   With a manual clock nothing can move time while the test is in the
   wait, so the wait's timeout stays real.  */
static inline struct timespec
nh_tc_wait_now (void)
{
  return nh_tc_auto ? nh_tc_wall () : current_timespec ();
}

extern void nh_tc_wake_up (void);

/* Something that held the clock may have stopped doing so.  Any thread.  */
static inline void
nh_tc_kick (void)
{
  if (nh_tc_auto)
    nh_tc_wake_up ();
}

/* A select of Lisp thread SELF, on the stack of really_call_select
   (thread.c), for as long as it lasts.  */
struct thread_state;
struct nh_tc_wait
{
  struct thread_state *self;
  int max_fds;
  fd_set *rp, *wp, *ep;		/* the caller's */
  fd_set r, w, e;		/* what it asked for */
  struct timespec *timeout;	/* the caller's */
  bool has_deadline;
  uint64_t deadline;		/* the clock's monotonic ns */
  _Atomic bool in_select;	/* blocked in it, not holding the lock */
  bool due;			/* another thread reached its deadline */
  bool done;			/* its deadline passed, or a deadlock */
  bool sig;			/* a signal is pending */
};

extern bool nh_tc_wait_begin (struct nh_tc_wait *, struct thread_state *,
			      int max_fds, fd_set *rfds, fd_set *wfds,
			      fd_set *efds, struct timespec *timeout);
extern void nh_tc_before (struct nh_tc_wait *, int *max_fds,
			  struct timespec **timeout);
extern void nh_tc_returned (struct nh_tc_wait *);
extern bool nh_tc_after (struct nh_tc_wait *, int *result);

/* After a select, with the global lock: signal the deadlock that select
   found, if any.  */
extern void nh_tc_raise (void);

/* thread.c, with the global lock held.  CV, a condition variable Lisp
   threads wait on as their wait_condvar, is broadcast (ALL) or
   signalled: its waiters, or one, can run.  */
extern void nh_tc_cond_woke (const void *cv, bool all);
/* SELF is back from a wait on its wait_condvar, which is still set.  */
extern void nh_tc_cond_back (struct thread_state *self);

/* A timed wait on a worker thread, with CV's mutex M held: wait on the
   clock.  JOB says a job waits, which stops holding the clock.  False
   if the clock is off.  Else *RESULT is what pthread_cond_timedwait
   would return.  */
extern bool nh_tc_cond_wait (pthread_cond_t *cv, pthread_mutex_t *m,
			     uint64_t deadline, bool job, int *result);

/* process.c.  */
extern bool nh_tc_child_alive (void);
extern Lisp_Object nh_tc_children (void);

/* Waiters the clock woke that have yet to run (a wake is not yet a
   thread); jobs in a timed wait.  */
extern _Atomic int64_t nh_tc_woken, nh_tc_idle_jobs;

/* nidhogg-workers.c: the jobs that can still run without the clock
   moving.  */
extern int64_t nh_wk_tc_busy (void);
/* nidhogg-io.c: nothing is on its way to a reader's FIFO; the fds that
   say otherwise; nh-io sits in its poll.  */
extern bool nh_io_tc_quiet (void);
extern Lisp_Object nh_io_tc_fds (void);
extern bool nh_io_tc_polling (void);

extern void nh_tc_syms (void);

#else /* not (HAVE_NIDHOGG_WORKERS && HAVE_GLITNIR) */

# define nh_tc_on false
# define nh_tc_auto false
# define nh_tc_mono_ns() UINT64_C (0)
# define nh_tc_now current_timespec
# define nh_tc_wait_now current_timespec
# define nh_tc_kick() ((void) 0)

#endif

#endif /* NIDHOGG_TESTCLOCK_H */

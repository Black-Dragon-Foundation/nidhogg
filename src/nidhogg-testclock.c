/* Nidhogg's test clock: time that moves only when nothing can run.

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

/* README.md, "The test clock", is the description for test authors.

   A batch Emacs with NIDHOGG_TEST_CLOCK=1 in its environment may call
   `nidhogg-test-clock-start'; nothing else changes anything.  From then
   on the Lisp-visible clocks read virtual time, kept with the deadline
   queue in the glitnir crate (testclock.rs).  Never gnulib's
   current_timespec: atimers need real intervals.  File times stay real.

   With a manual clock (no AUTO) only `nidhogg-test-clock-advance' moves
   it, and a wait's own timeout stays real, so a wait cannot hang on a
   clock that nobody in it can move.

   With AUTO, the wait moves it.  Every select comes through
   really_call_select (thread.c), which loops over one real select at a
   time, with the global lock held between them, so Lisp's state is
   stable for the check.  Each round:

     read the wake fd empty, then
     nothing can run (nh_tc_quiet)?
       an fd is ready: a select with zero timeout returns it, at this time
       the deadline has passed: the same; the caller goes on
       nothing left to wait for: a deadlock
       else: advance to the earliest deadline, and look again
     else: a select with no timeout, the wake fd among its fds

   The fds are looked at after the check, because what the check clears
   (a read that landed) is then there for the look.  Each thing that
   stops holding the clock kicks: writes the wake fd after the change
   (job over, parked at a pause point, in a timed wait, nh-io in its
   poll, a Lisp thread blocked or gone).  With the emptying before the
   check, that is the clear-then-recheck of nh_wk_cq_ready: a change
   after the check leaves the fd readable for the select.

   Several Lisp threads share the fd.  One that reads it empty has
   looked for all of them: the check covers every thread's fds and
   deadlines.  The exception is a thread whose deadline has come, which
   must itself run: it is `due' until it does, and nobody reads the fd
   meanwhile.

   A Lisp thread in a wait on its wait_condvar is blocked, unless a wake
   is on its way to it: whoever broadcasts or signals that condition
   variable, with the global lock, marks the waiters it woke
   (nh_tc_woken; a signal marks one).  A waiter back from the wait
   clears its mark, or, if a signal meant for another came to it, that
   one's, and kicks: it may wait again.  A spurious wake-up can clear a
   mark early, so time can still move a little early then, but never
   hang on a mark.  Glib's own timeouts in xg_select stay real (a GUI
   concern; tests on this clock are batch).  */

#include <config.h>

#include <errno.h>
#include <stdlib.h>

#include "lisp.h"
#include "nidhogg-testclock.h"

#ifdef HAVE_NH_TEST_CLOCK

#include <glitnir.h>

#include "keyboard.h"
#include "nidhogg-workers.h"
#ifdef HAVE_NIDHOGG_WATCHDOG
# include "nidhogg-watchdog.h"
#endif

/* What wait_reading_process_output passes to wait forever.  */
enum { NH_TC_FOREVER_SEC = 100000 };

_Atomic bool nh_tc_on, nh_tc_auto;
_Atomic int64_t nh_tc_woken, nh_tc_idle_jobs;

static struct nh_wk_wake nh_tc_wake = { -1, -1 };

/* Times a select blocked for real.  */
static _Atomic int64_t nh_tc_blocks;

/* The wall reading at the start.  */
static int64_t nh_tc_wall0;

/* The thread whose select found a deadlock; it raises it (lock holder).  */
static struct thread_state *nh_tc_deadlock;

static int64_t
nh_tc_ns (struct timespec t)
{
  return t.tv_sec * INT64_C (1000000000) + t.tv_nsec;
}

static struct timespec
nh_tc_timespec (int64_t ns)
{
  return make_timespec (ns / 1000000000, ns % 1000000000);
}

struct timespec
nh_tc_wall (void)
{
  return nh_tc_timespec (glitnir_tc_wall_ns ());
}

struct timespec
nh_tc_mono (void)
{
  return nh_tc_timespec (glitnir_tc_mono_ns ());
}

uint64_t
nh_tc_mono_ns (void)
{
  return glitnir_tc_mono_ns ();
}

void
nh_tc_wake_up (void)
{
  nh_wk_wake_signal (&nh_tc_wake);
}

/* Wake the workers' timed waits that are over.  */
static void
nh_tc_wake_waits (void)
{
  void *cv, *m;
  while (glitnir_tc_take_due (&cv, &m))
    {
      atomic_fetch_add (&nh_tc_woken, 1);
      nh_wk_lock (m);
      pthread_cond_broadcast (cv);
      pthread_mutex_unlock (m);
    }
}

bool
nh_tc_cond_wait (pthread_cond_t *cv, pthread_mutex_t *m, uint64_t deadline,
		 bool job, int *result)
{
  uint64_t id = glitnir_tc_register (deadline, cv, m);
  if (id == 0)
    return false;
  if (id == 1)
    {
      *result = ETIMEDOUT;
      return true;
    }
  /* After the registering: a job idle here has its deadline counted.  */
  if (job)
    atomic_fetch_add (&nh_tc_idle_jobs, 1);
  nh_tc_kick ();
  pthread_cond_wait (cv, m);
  if (job)
    atomic_fetch_sub (&nh_tc_idle_jobs, 1);
  if (!glitnir_tc_unregister (id))
    {
      atomic_fetch_sub (&nh_tc_woken, 1);
      nh_tc_kick ();
    }
  *result = nh_tc_on && glitnir_tc_mono_ns () < deadline ? 0 : ETIMEDOUT;
  return true;
}

/* Whether any of W's fds is ready.  */
static bool
nh_tc_fds_ready (struct nh_tc_wait *w)
{
  fd_set r = w->r, wr = w->w, e = w->e;
  struct timespec zero = make_timespec (0, 0);
  return pselect (w->max_fds, &r, w->wp ? &wr : NULL, w->ep ? &e : NULL,
		  &zero, NULL) > 0;
}

/* Threads marked nh_tc_woken; with the global lock.  */
static int nh_tc_cond_woken;

void
nh_tc_cond_woke (const void *cv, bool all)
{
  if (!nh_tc_auto)
    return;
  /* A marked waiter has a wake on its way already, so a signal's goes
     to an unmarked one, if any is left.  */
  for (struct thread_state *t = all_threads; t; t = t->next_thread)
    if (t->wait_condvar == cv && !t->nh_tc_woken)
      {
	t->nh_tc_woken = true;
	nh_tc_cond_woken++;
	if (!all)
	  return;
      }
}

void
nh_tc_cond_back (struct thread_state *self)
{
  struct thread_state *t = self;
  if (!t->nh_tc_woken)
    {
      if (nh_tc_cond_woken == 0)
	return;
      /* A signal's wake came to SELF, not to the waiter marked for it,
	 which still waits.  */
      for (t = all_threads; t; t = t->next_thread)
	if (t->nh_tc_woken && t->wait_condvar == self->wait_condvar)
	  break;
      if (!t)
	return;
    }
  t->nh_tc_woken = false;
  nh_tc_cond_woken--;
  /* SELF may wait again, or that waiter waits on: neither can run.  */
  nh_tc_kick ();
}

/* Can nothing run but what SELF, which holds the lock, will do?  */
static bool
nh_tc_quiet (struct thread_state *self)
{
  /* A thread out of its select waits for the lock, so it can run, and
     so can one a wake is on its way to.  */
  for (struct thread_state *t = all_threads; t; t = t->next_thread)
    {
      struct nh_tc_wait *o = t->nh_tc;
      if (t == self)
	continue;
      if (o ? !atomic_load (&o->in_select) || o->due || nh_tc_fds_ready (o)
	  : !t->wait_condvar || t->nh_tc_woken)
	return false;
    }
  return (nh_io_tc_quiet () && nh_wk_tc_busy () <= 0
	  && !nh_tc_child_alive ());
}

/* Is another thread's deadline waiting for that thread to run?  */
static bool
nh_tc_any_due (void)
{
  for (struct thread_state *t = all_threads; t; t = t->next_thread)
    if (t->nh_tc && t->nh_tc->due)
      return true;
  return false;
}

/* The earliest of W's deadline, the other threads' and the workers';
   UINT64_MAX if there is none.  */
static uint64_t
nh_tc_earliest (struct nh_tc_wait *w)
{
  uint64_t at = glitnir_tc_next_deadline ();
  if (at == 0)
    at = UINT64_MAX;
  if (w->has_deadline && w->deadline < at)
    at = w->deadline;
  for (struct thread_state *t = all_threads; t; t = t->next_thread)
    {
      struct nh_tc_wait *o = t->nh_tc;
      if (o && o->has_deadline && o->deadline < at)
	at = o->deadline;
    }
  return at;
}

/* Time has moved: the other threads whose deadline came must run.  */
static void
nh_tc_wake_threads (struct nh_tc_wait *w, uint64_t now)
{
  for (struct thread_state *t = all_threads; t; t = t->next_thread)
    {
      struct nh_tc_wait *o = t->nh_tc;
      if (o && o != w && o->has_deadline && o->deadline <= now)
	{
	  o->due = true;
	  nh_tc_wake_up ();
	}
    }
}

bool
nh_tc_wait_begin (struct nh_tc_wait *w, struct thread_state *self,
		  int max_fds, fd_set *rfds, fd_set *wfds, fd_set *efds,
		  struct timespec *timeout)
{
  if (!rfds || (timeout && timeout->tv_sec == 0 && timeout->tv_nsec == 0))
    return false;
  w->self = self;
  w->max_fds = max_fds;
  w->rp = rfds;
  w->wp = wfds;
  w->ep = efds;
  w->r = *rfds;
  if (wfds)
    w->w = *wfds;
  if (efds)
    w->e = *efds;
  w->timeout = timeout;
  w->has_deadline = (timeout && !(timeout->tv_sec == NH_TC_FOREVER_SEC
				  && timeout->tv_nsec == 0));
  if (w->has_deadline)
    w->deadline = glitnir_tc_mono_ns () + nh_tc_ns (*timeout);
  atomic_store (&w->in_select, false);
  w->due = w->done = w->sig = false;
  self->nh_tc = w;
  return true;
}

void
nh_tc_before (struct nh_tc_wait *w, int *max_fds, struct timespec **timeout)
{
  static struct timespec zero;

  *w->rp = w->r;
  if (w->wp)
    *w->wp = w->w;
  if (w->ep)
    *w->ep = w->e;
  *max_fds = w->max_fds;
  if (w->due)
    {
      /* Whoever moved the clock found that nothing else could run.  */
      w->due = false;
      w->done = true;
      *timeout = &zero;
      return;
    }
  for (;;)
    {
      if (!nh_tc_auto)
	{
	  *timeout = w->timeout;
	  return;
	}
      if (!nh_tc_any_due ())
	nh_wk_wake_clear (&nh_tc_wake);
      *timeout = &zero;
      if (pending_signals)
	{
	  w->sig = true;
	  return;
	}
      if (!nh_tc_quiet (w->self))
	break;
      if (nh_tc_fds_ready (w))
	return;
      uint64_t now = glitnir_tc_mono_ns ();
      if (w->has_deadline && w->deadline <= now)
	{
	  w->done = true;
	  return;
	}
      uint64_t at = nh_tc_earliest (w);
      if (at == UINT64_MAX)
	{
	  nh_tc_deadlock = w->self;
	  w->done = true;
	  return;
	}
      glitnir_tc_advance_to (at);
      nh_tc_wake_waits ();
      nh_tc_wake_threads (w, glitnir_tc_mono_ns ());
    }

  FD_SET (nh_tc_wake.rfd, w->rp);
  if (*max_fds <= nh_tc_wake.rfd)
    *max_fds = nh_tc_wake.rfd + 1;
  atomic_store (&w->in_select, true);
  atomic_fetch_add (&nh_tc_blocks, 1);
  *timeout = NULL;
}

void
nh_tc_returned (struct nh_tc_wait *w)
{
  atomic_store (&w->in_select, false);
}

bool
nh_tc_after (struct nh_tc_wait *w, int *result)
{
  bool again = false;
  int r = *result;
  if (r >= 0)
    {
      /* Only a blocking round has the wake fd in its set.  */
      if (r > 0 && FD_ISSET (nh_tc_wake.rfd, w->rp)
	  && !FD_ISSET (nh_tc_wake.rfd, &w->r))
	{
	  FD_CLR (nh_tc_wake.rfd, w->rp);
	  r--;
	}
      if (r == 0 && w->sig)
	{
	  r = -1;
	  errno = EINTR;
	}
      else if (r == 0 && !w->done && nh_tc_auto)
	again = true;
      *result = r;
    }
  if (!again)
    w->self->nh_tc = NULL;
  return again;
}

/* Lisp.  */

static Lisp_Object
nh_tc_kw (const char *name)
{
  return intern_c_string (name);
}

DEFUN ("nidhogg-test-clock-state", Fnidhogg_test_clock_state,
       Snidhogg_test_clock_state, 0, 0, 0,
       doc: /* Return what the test clock sees, a plist.
`:timers' is how many Lisp timers are pending, `:deadlines' how many
workers' timed waits, `:jobs' how many jobs can run, `:fds' the fds with
output on its way to a reader, `:children' the names of the live child
processes, `:blocks' how many selects blocked for real since the start.  */)
  (void)
{
  return listn (12,
		nh_tc_kw (":timers"),
		make_int (list_length (Vtimer_list)
			  + list_length (Vtimer_idle_list)),
		nh_tc_kw (":deadlines"), make_uint (glitnir_tc_waiting ()),
		nh_tc_kw (":jobs"), make_int (nh_wk_tc_busy ()),
		nh_tc_kw (":fds"), nh_io_tc_fds (),
		nh_tc_kw (":children"), nh_tc_children (),
		nh_tc_kw (":blocks"), make_int (atomic_load (&nh_tc_blocks)));
}

void
nh_tc_raise (void)
{
  if (nh_tc_deadlock == current_thread)
    {
      nh_tc_deadlock = NULL;
      xsignal (Qnidhogg_test_clock_deadlock, Fnidhogg_test_clock_state ());
    }
}

DEFUN ("nidhogg-test-clock-start", Fnidhogg_test_clock_start,
       Snidhogg_test_clock_start, 0, 1, 0,
       doc: /* Start the test clock at the real time.
From now on `current-time', Lisp timers and the workers' clocks read
virtual time, and only `nidhogg-test-clock-advance' moves it.  With AUTO
non-nil a wait moves it too, once nothing can run, and a wait that can
then never end signals `nidhogg-test-clock-deadlock'.

Signal an error unless Emacs runs in batch with NIDHOGG_TEST_CLOCK=1 in
its environment, or if the watchdog is on.  */)
  (Lisp_Object auto_advance)
{
  const char *env = egetenv ("NIDHOGG_TEST_CLOCK");
  if (!noninteractive || !env || strcmp (env, "1") != 0)
    error ("The test clock needs a batch Emacs with NIDHOGG_TEST_CLOCK=1");
#ifdef HAVE_NIDHOGG_WATCHDOG
  if (nh_wd_on)
    error ("The test clock and the watchdog are never on together");
#endif
  if (nh_tc_on)
    error ("The test clock is on");
  if (nh_tc_wake.rfd < 0 && nh_wk_wake_open (&nh_tc_wake) < 0)
    error ("The test clock has no wake fd");
  struct timespec mono;
  clock_gettime (CLOCK_MONOTONIC, &mono);
  atomic_store (&nh_tc_blocks, 0);
  nh_tc_wall0 = nh_tc_ns (current_timespec ());
  glitnir_tc_start (nh_tc_wall0, nh_tc_ns (mono));
  nh_tc_on = true;
  nh_tc_auto = !NILP (auto_advance);
  return Qnil;
}

DEFUN ("nidhogg-test-clock-stop", Fnidhogg_test_clock_stop,
       Snidhogg_test_clock_stop, 0, 0, 0,
       doc: /* Stop the test clock: real time again.  */)
  (void)
{
  if (nh_tc_on)
    {
      nh_tc_auto = false;
      nh_tc_on = false;
      glitnir_tc_stop ();
      nh_tc_wake_waits ();
      /* A thread in a select of the clock's has no timeout.  */
      nh_tc_wake_up ();
      nh_tc_deadlock = NULL;
    }
  return Qnil;
}

DEFUN ("nidhogg-test-clock-active-p", Fnidhogg_test_clock_active_p,
       Snidhogg_test_clock_active_p, 0, 0, 0,
       doc: /* Return t if the test clock is on.  */)
  (void)
{
  return nh_tc_on ? Qt : Qnil;
}

static void
nh_tc_check_on (void)
{
  if (!nh_tc_on)
    error ("The test clock is off");
}

DEFUN ("nidhogg-test-clock-advance", Fnidhogg_test_clock_advance,
       Snidhogg_test_clock_advance, 1, 1, 0,
       doc: /* Move the test clock SECS seconds on, a number.
Each timer that comes due runs, at its own time, in order, and each
worker's timed wait that comes due wakes.  */)
  (Lisp_Object secs)
{
  nh_tc_check_on ();
  double d = extract_float (secs);
  if (! (d >= 0))
    xsignal2 (Qargs_out_of_range, secs, make_fixnum (0));
  uint64_t target = glitnir_tc_mono_ns () + (uint64_t) (d * 1e9 + 0.5);
  for (;;)
    {
      /* Runs what is ripe; says when the next one is.  */
      struct timespec delay = timer_check ();
      uint64_t now = glitnir_tc_mono_ns ();
      uint64_t at = target;
      if (timespec_valid_p (delay) && now + nh_tc_ns (delay) < at)
	at = now + nh_tc_ns (delay);
      uint64_t queued = glitnir_tc_next_deadline ();
      if (queued != 0 && queued < at)
	at = queued;
      glitnir_tc_advance_to (at);
      nh_tc_wake_waits ();
      if (at >= target)
	break;
    }
  timer_check ();
  return Qnil;
}

DEFUN ("nidhogg-test-clock-now", Fnidhogg_test_clock_now,
       Snidhogg_test_clock_now, 0, 0, 0,
       doc: /* Return the seconds the test clock moved since its start, a float.  */)
  (void)
{
  nh_tc_check_on ();
  return make_float ((glitnir_tc_wall_ns () - nh_tc_wall0) / 1e9);
}

DEFUN ("nidhogg-test-clock--readings", Fnidhogg_test_clock__readings,
       Snidhogg_test_clock__readings, 0, 0, 0,
       doc: /* Return the clocks' readings in ns, for tests.
A list of the wall clock's, `monotonic_coarse_timespec's and the workers'.  */)
  (void)
{
  return list3 (make_int (nh_tc_ns (nh_tc_now ())),
		make_int (nh_tc_ns (monotonic_coarse_timespec ())),
		make_uint (nh_wk_now ()));
}

DEFUN ("nidhogg-test-clock--quiet-p", Fnidhogg_test_clock__quiet_p,
       Snidhogg_test_clock__quiet_p, 0, 0, 0,
       doc: /* Return t if nothing but this thread can run; for tests.  */)
  (void)
{
  return nh_tc_quiet (current_thread) ? Qt : Qnil;
}

void
nh_tc_syms (void)
{
  DEFSYM (Qnidhogg_test_clock_deadlock, "nidhogg-test-clock-deadlock");
  define_error (Qnidhogg_test_clock_deadlock,
		"A wait can never end on the test clock", Qerror);
  defsubr (&Snidhogg_test_clock_start);
  defsubr (&Snidhogg_test_clock_stop);
  defsubr (&Snidhogg_test_clock_active_p);
  defsubr (&Snidhogg_test_clock_advance);
  defsubr (&Snidhogg_test_clock_now);
  defsubr (&Snidhogg_test_clock_state);
  defsubr (&Snidhogg_test_clock__readings);
  defsubr (&Snidhogg_test_clock__quiet_p);
  Fprovide (intern_c_string ("nidhogg-test-clock"), Qnil);
}

#endif /* HAVE_NH_TEST_CLOCK */

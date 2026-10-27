/* Nidhogg's stall watchdog: the monitor thread and the park protocol.

   Task 18 adds the hang file writer and deleter here.

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

/* The main thread counts every busy span and turns each one at least
   as long as the threshold T into a record itself (nidhogg-watchdog.c);
   the monitor only adds evidence.  It is a plain pthread with every
   signal blocked, takes only nh_wd.m, and never touches Lisp, the
   specpdl or anyone else's locks:

     park while Emacs waits: no wake-ups at all while idle; the main
       thread signals it when a busy span starts (nidhogg-watchdog-park.h);
     for busy span s since t0: sleep until t0 + T/2 (most spans end
       first); if s is still busy, arm: note the main thread's CPU clock
       and the lateness; then every P = max (T/10, 1 ms) (1 ms while deep
       mode is pending) raise a request for s and send the main thread
       NIDHOGG_WATCHDOG_SIGNAL, whose handler takes a sample and clears
       the request (nidhogg-watchdog-sample.c); a request still pending
       after 2 P is missed; past the hang threshold, names mode.

   The main thread counts, so a monitor that never runs (a starved
   thread) costs samples, never a record.  While `--test-clock' is set
   the monitor never arms: it treats every span as idle.

   The main thread's CPU clock is read here (about 0.5 us, a real
   system call) and once more when a stall closes, never on an edge.  */

#include <config.h>

#ifdef HAVE_NIDHOGG_WATCHDOG

#include <errno.h>
#ifdef __linux__
# include <sched.h>
# include <sys/syscall.h>
# include <unistd.h>
#endif

#include "nidhogg-watchdog.h"
#include "nidhogg-watchdog-park.h"

enum
{
  NH_WD_MS = 1000000		/* ns */
};

/* A request: the pending bit and the low 31 bits of its span.  */
#define NH_WD_PENDING (UINT32_C (1) << 31)
#define NH_WD_SPAN_BITS UINT32_C (0x7fffffff)

/* The shared words as the park protocol's memory.  */
static struct nh_wd_park_mem
nh_wd_park_real (void)
{
  return (struct nh_wd_park_mem) { &nh_wd.span, &nh_wd.waiting,
				   &nh_wd.parked, NULL };
}

/* The main thread's side.  */

void
nh_wd_publish_waiting (uint64_t now)
{
  (void) now;
  __atomic_store_n (&nh_wd.waiting, 1, NH_WD_PARK_ORDER);
  /* From here on the handler counts any request as stale; drop the
     one in flight.  */
  __atomic_store_n (&nh_wd.request, 0, __ATOMIC_SEQ_CST);
}

void
nh_wd_publish_busy (uint64_t now)
{
  struct nh_wd_park_mem mem = nh_wd_park_real ();
  /* busy_since before span: the monitor reads span, busy_since, span.  */
  __atomic_store_n (&nh_wd.busy_since_ns, now, NH_WD_PARK_ORDER);
  if (nh_wd_park_busy (&mem, __atomic_load_n (&nh_wd.span,
					       __ATOMIC_RELAXED) + 1))
    {
      pthread_mutex_lock (&nh_wd.m);
      pthread_cond_signal (&nh_wd.cv);
      pthread_mutex_unlock (&nh_wd.m);
    }
}

#ifdef __linux__

static pthread_t nh_wd_mon_thread;
static bool nh_wd_mon_running;	/* start and stop only */
static bool nh_wd_mon_stop;	/* under nh_wd.m */
static pid_t nh_wd_mon_tid;	/* the monitor's task, for stop */
static clockid_t nh_wd_main_cpu; /* the main thread's CPU clock */
/* Requests of armed_span that the handler never took (atomic).  */
static uint32_t nh_wd_mon_missed;

static uint64_t
nh_wd_clock_ns (clockid_t clock)
{
  struct timespec ts;
  if (clock_gettime (clock, &ts) != 0)
    return 0;
  return (uint64_t) ts.tv_sec * 1000000000 + ts.tv_nsec;
}

/* With nh_wd.m held: wait on the condition variable until DEADLINE
   (CLOCK_MONOTONIC ns), a stop, or a spurious wake-up.  */
static void
nh_wd_mon_sleep_until (uint64_t deadline)
{
  struct timespec ts = { .tv_sec = deadline / 1000000000,
			 .tv_nsec = deadline % 1000000000 };
  pthread_cond_timedwait (&nh_wd.cv, &nh_wd.m, &ts);
}

/* Is span S still busy?  */
static bool
nh_wd_mon_busy (uint64_t s)
{
  return (__atomic_load_n (&nh_wd.span, __ATOMIC_SEQ_CST) == s
	  && !__atomic_load_n (&nh_wd.waiting, __ATOMIC_SEQ_CST));
}

/* A return from a wait that found no busy span.  */
static void
nh_wd_mon_woke_idle (void)
{
  __atomic_add_fetch (&nh_wd.monitor_wakeups, 1, __ATOMIC_RELAXED);
}

/* The sampling period: the configured one, else max (T/10, 1 ms);
   1 ms while deep mode is pending.  */
static uint64_t
nh_wd_mon_period (void)
{
  if (__atomic_load_n (&nh_wd.deep_left, __ATOMIC_RELAXED) > 0)
    return NH_WD_MS;
  uint64_t p = __atomic_load_n (&nh_wd.period_ns, __ATOMIC_RELAXED);
  if (!p)
    p = __atomic_load_n (&nh_wd.threshold_ns, __ATOMIC_RELAXED) / 10;
  return p < NH_WD_MS ? NH_WD_MS : p;
}

/* Gather evidence for busy span S, busy since T0, until it ends or the
   monitor stops.  Called and returns with nh_wd.m held.  */
static void
nh_wd_mon_watch (uint64_t s, uint64_t t0)
{
  uint64_t deadline
    = t0 + __atomic_load_n (&nh_wd.threshold_ns, __ATOMIC_RELAXED) / 2;
  uint64_t now = nh_wd_clock_ns (CLOCK_MONOTONIC);

  /* Most spans end before T/2.  */
  while (now < deadline)
    {
      nh_wd_mon_sleep_until (deadline);
      if (nh_wd_mon_stop)
	return;
      if (!nh_wd_mon_busy (s))
	{
	  nh_wd_mon_woke_idle ();
	  return;
	}
      now = nh_wd_clock_ns (CLOCK_MONOTONIC);
    }

  /* A stall candidate: arm.  The values first, then armed_span with a
     release store; the main thread's span close reads armed_span with
     acquire and uses the values only when it names its own span.  */
  __atomic_store_n (&nh_wd_mon_missed, 0, __ATOMIC_RELAXED);
  __atomic_store_n (&nh_wd.late_ns, now - deadline, __ATOMIC_RELAXED);
  __atomic_store_n (&nh_wd.cpu_armed_ns, nh_wd_clock_ns (nh_wd_main_cpu),
		    __ATOMIC_RELAXED);
  __atomic_store_n (&nh_wd.mode, 0, __ATOMIC_RELAXED);
  __atomic_store_n (&nh_wd.armed_span, s, __ATOMIC_RELEASE);

  uint32_t tag = ((uint32_t) s & NH_WD_SPAN_BITS) | NH_WD_PENDING;
  uint64_t raised = 0, next = now;
  for (;;)
    {
      uint64_t period = nh_wd_mon_period ();
      uint64_t hang = __atomic_load_n (&nh_wd.hang_ns, __ATOMIC_RELAXED);
      if (hang && now - t0 >= hang)
	__atomic_store_n (&nh_wd.mode, 1, __ATOMIC_RELAXED);

      uint32_t req = __atomic_load_n (&nh_wd.request, __ATOMIC_ACQUIRE);
      if (req == tag)
	{
	  /* The last request is still pending.  */
	  if (now - raised >= 2 * period)
	    {
	      __atomic_add_fetch (&nh_wd.missed, 1, __ATOMIC_RELAXED);
	      __atomic_add_fetch (&nh_wd_mon_missed, 1, __ATOMIC_RELAXED);
	      __atomic_add_fetch (&nh_wd.requests, 1, __ATOMIC_RELAXED);
	      raised = now;
	      pthread_kill (nh_wd.main_id, NIDHOGG_WATCHDOG_SIGNAL);
	    }
	}
      /* Raise it from the value just seen, after one more look at the
	 span: a raise that loses to publish_waiting's clear fails, and
	 the handler drops one that wins just before the clear (the span
	 is waiting by then).  */
      else if (nh_wd_mon_busy (s)
	       && __atomic_compare_exchange_n (&nh_wd.request, &req, tag,
					       false, __ATOMIC_SEQ_CST,
					       __ATOMIC_SEQ_CST))
	{
	  __atomic_add_fetch (&nh_wd.requests, 1, __ATOMIC_RELAXED);
	  raised = now;
	  pthread_kill (nh_wd.main_id, NIDHOGG_WATCHDOG_SIGNAL);
	}

      next += period;
      if (next <= now)
	next = now + period;
      nh_wd_mon_sleep_until (next);
      if (nh_wd_mon_stop)
	return;
      if (!nh_wd_mon_busy (s))
	{
	  nh_wd_mon_woke_idle ();
	  return;
	}
      now = nh_wd_clock_ns (CLOCK_MONOTONIC);
    }
}

static void *
nh_wd_monitor_main (void *arg)
{
  struct nh_wd_park_mem mem = nh_wd_park_real ();
  uint64_t last = __atomic_load_n (&nh_wd.span, __ATOMIC_SEQ_CST);
  uint64_t cpu_counted = 0;
  (void) arg;

  nh_wd_mon_tid = syscall (SYS_gettid);

  pthread_mutex_lock (&nh_wd.m);
  while (!nh_wd_mon_stop)
    {
      /* Park until a busy span (or, on the test clock, a new span).  */
      bool busy_ends = !nh_wd_test_clock_p ();
      if (nh_wd_park_must_wait (&mem, 0, last, busy_ends))
	{
	  /* Its own CPU time, added at each park (`--stats' may reset
	     the counter).  */
	  uint64_t cpu = nh_wd_clock_ns (CLOCK_THREAD_CPUTIME_ID);
	  __atomic_add_fetch (&nh_wd.monitor_cpu_ns, cpu - cpu_counted,
			      __ATOMIC_RELAXED);
	  cpu_counted = cpu;
	  bool wait = true;
	  while (wait && !nh_wd_mon_stop)
	    {
	      pthread_cond_wait (&nh_wd.cv, &nh_wd.m);
	      if (__atomic_load_n (&nh_wd.waiting, __ATOMIC_SEQ_CST))
		nh_wd_mon_woke_idle ();
	      busy_ends = !nh_wd_test_clock_p ();
	      wait = nh_wd_park_must_wait (&mem, 1, last, busy_ends);
	    }
	}
      nh_wd_park_leave (&mem);
      if (nh_wd_mon_stop)
	break;

      /* Span s, busy since t0: busy_since is stored before span.  */
      uint64_t s = __atomic_load_n (&nh_wd.span, __ATOMIC_SEQ_CST);
      uint64_t t0 = __atomic_load_n (&nh_wd.busy_since_ns, __ATOMIC_SEQ_CST);
      if (__atomic_load_n (&nh_wd.span, __ATOMIC_SEQ_CST) != s)
	continue;
      last = s;
      if (busy_ends && nh_wd_mon_busy (s))
	nh_wd_mon_watch (s, t0);
    }
  pthread_mutex_unlock (&nh_wd.m);
  return NULL;
}

/* Start the monitor thread (from the main thread).  Return 0, or an
   errno value.  With the test flag no_monitor, start nothing.  */
int
nh_wd_monitor_start (void)
{
  if (!main_thread_p (current_thread))
    return EINVAL;
  nh_wd.main_id = pthread_self ();
  if (nh_wd.no_monitor || nh_wd_mon_running)
    return 0;
  int err = pthread_getcpuclockid (nh_wd.main_id, &nh_wd_main_cpu);
  if (err)
    return err;
  __atomic_store_n (&nh_wd.armed_span, UINT64_MAX, __ATOMIC_RELAXED);

  /* The waits are timed on CLOCK_MONOTONIC.  Nobody waits on the
     condition variable while the monitor is not running.  */
  pthread_condattr_t attr;
  pthread_condattr_init (&attr);
  pthread_condattr_setclock (&attr, CLOCK_MONOTONIC);
  pthread_cond_destroy (&nh_wd.cv);
  pthread_cond_init (&nh_wd.cv, &attr);
  pthread_condattr_destroy (&attr);

  /* The thread inherits a mask with every signal blocked.  */
  sigset_t all, old;
  sigfillset (&all);
  nh_wd_mon_stop = false;
  pthread_sigmask (SIG_SETMASK, &all, &old);
  err = pthread_create (&nh_wd_mon_thread, NULL, nh_wd_monitor_main, NULL);
  pthread_sigmask (SIG_SETMASK, &old, NULL);
  if (err)
    return err;
  /* Named here, not by the thread itself, so the name is there when
     this returns.  */
  pthread_setname_np (nh_wd_mon_thread, "nh-watchdog");
  nh_wd_mon_running = true;
  return 0;
}

/* Stop the monitor thread and wait for it; safe when it is not
   running.  */
void
nh_wd_monitor_stop (void)
{
  if (!nh_wd_mon_running)
    return;
  pthread_mutex_lock (&nh_wd.m);
  nh_wd_mon_stop = true;
  pthread_cond_broadcast (&nh_wd.cv);
  pthread_mutex_unlock (&nh_wd.m);
  pthread_join (nh_wd_mon_thread, NULL);
  nh_wd_mon_running = false;
  /* pthread_join returns once the thread's tid is cleared, a moment
     before the kernel drops the task from /proc/self/task; wait for
     that (a few yields), so no caller still sees it.  */
  for (int i = 0;
       i < 10000 && syscall (SYS_tgkill, getpid (), nh_wd_mon_tid, 0) == 0;
       i++)
    sched_yield ();
}

/* The monitor's part of a stall that closes now (main thread, after
   nh_wd_publish_waiting): the lateness, the main thread's CPU time
   since the monitor armed, and the missed requests, when the monitor
   armed for this span.  If it never got to the span, the lateness is
   at least the part of the stall past T/2.  */
void
nh_wd_monitor_span_close (struct nh_wd_stall *st)
{
  uint64_t s = __atomic_load_n (&nh_wd.span, __ATOMIC_RELAXED);
  if (__atomic_load_n (&nh_wd.armed_span, __ATOMIC_ACQUIRE) == s)
    {
      st->late_ns = __atomic_load_n (&nh_wd.late_ns, __ATOMIC_RELAXED);
      st->cpu_ns = (nh_wd_clock_ns (CLOCK_THREAD_CPUTIME_ID)
		    - __atomic_load_n (&nh_wd.cpu_armed_ns, __ATOMIC_RELAXED));
      st->missed = __atomic_load_n (&nh_wd_mon_missed, __ATOMIC_RELAXED);
    }
  else if (nh_wd_mon_running
	   && !nh_wd_test_clock_p ())
    {
      uint64_t half = __atomic_load_n (&nh_wd.threshold_ns,
				       __ATOMIC_RELAXED) / 2;
      st->late_ns = st->wall_ns > half ? st->wall_ns - half : 0;
    }
}

#else  /* !__linux__ */

/* Not on this platform yet (macOS: the next plan's run-loop observer).  */

int
nh_wd_monitor_start (void)
{
  return ENOSYS;
}

void
nh_wd_monitor_stop (void)
{
}

void
nh_wd_monitor_span_close (struct nh_wd_stall *st)
{
  (void) st;
}

#endif /* !__linux__ */

DEFUN ("nidhogg-watchdog--test-interleavings",
       Fnidhogg_watchdog__test_interleavings,
       Snidhogg_watchdog__test_interleavings, 0, 0, 0,
       doc: /* Run the park protocol's interleaving model; for tests.
Every interleaving of the main thread's busy edge against the monitor's
park (the steps of nidhogg-watchdog-park.h, each side's stores going
through its own store buffer) is run to its end.  Return (:cases N
:lost K): N interleavings, K of them ending with the monitor still
waiting through the busy span (a lost wake-up).  */)
  (void)
{
  long cases, lost;
  nh_wd_park_model_run (&cases, &lost);
  return list4 (intern_c_string (":cases"), make_fixnum (cases),
		intern_c_string (":lost"), make_fixnum (lost));
}

void
nh_wd_monitor_syms (void)
{
  defsubr (&Snidhogg_watchdog__test_interleavings);
}

#endif /* HAVE_NIDHOGG_WATCHDOG */

/* Nidhogg's stall watchdog: edges, spans, records and the Lisp interface.

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

/* The main thread is authoritative: it times every
   busy span at the two edges of the responsive wait and turns every span
   at least as long as the threshold into a record itself.  The monitor
   (nidhogg-watchdog-monitor.c) and the signal handler
   (nidhogg-watchdog-sample.c) only add samples; a starved monitor costs
   samples, never a record.

   The edges are called from really_call_select (thread.c) with SELF,
   the thread that entered the select.  wait_enter runs before the
   global lock is released, so the responsive test reads the main
   thread's own bindings; wait_leave runs right after the select
   returns, before the lock is retaken (lock contention is busy), and so
   touches nothing but this file's main-thread state and atomics.  A Lisp
   thread's select never touches the state.

   The main thread keeps its own copy of "waiting" and "busy since" in
   the statics below and publishes them through the monitor file's
   nh_wd_publish_busy/_waiting for the monitor and the handler.  */

#include <config.h>

#ifdef HAVE_NIDHOGG_WATCHDOG

#include <stddef.h>
#include <stdlib.h>
#include <string.h>

#include "nidhogg-watchdog.h"
#include "nidhogg-watchdog-phase.h"
#include "coding.h"
#ifdef HAVE_PGTK
#include "dispextern.h"
#endif

/* The shared state (declared in nidhogg-watchdog.h).  All zero until
   `--start'; the lock and the condition variable are ready from the
   start.  */
struct nh_wd_shared nh_wd = {
  .m = PTHREAD_MUTEX_INITIALIZER,
  .cv = PTHREAD_COND_INITIALIZER,
};
struct nh_wd_cur nh_wd_cur;
struct nh_wd_input nh_wd_input;
volatile sig_atomic_t nh_wd_specpdl_unstable;
bool nh_wd_on, nh_wd_gc_explicit;
char *nh_wd_hang_file;

/* Main-thread state (written only by the main thread, with the global
   lock held or in wait_leave, which only the main thread runs).  */
static bool wd_waiting;		/* in a responsive wait */
static uint64_t wd_busy_since;	/* the current span's start */
static uint64_t wd_gc_ns0, wd_gcs0, wd_cmds0; /* snapshots at its start */
static int wd_wait_depth = -1;	/* phase depth to pop back to after a
				   non-responsive wait, or -1 */
static bool wd_monitor_started;

/* The ring of finished stalls (NH_WD_RING slots, oldest at HEAD).
   Written by the main thread's finalise, read by `--take'; both run with
   the global lock held, so no atomics.  Allocated by `--start', never a
   GC root: its words are raw.  */
static struct nh_wd_stall *wd_ring;
static unsigned wd_ring_head, wd_ring_n;
static uint64_t wd_next_id;

/* The histograms, by enum nh_wd_hist; relaxed atomics.  */
static uint64_t wd_hist[NH_WD_H_N][NH_WD_BUCKETS];
static const uint64_t wd_bucket_ms[NH_WD_BUCKETS - 1]
  = { 4, 8, 16, 33, 50, 100, 250, 1000 };

/* The virtual clock (`--test-clock').  */
static uint32_t wd_test_clock_on;
static uint64_t wd_test_clock_ns;

uint64_t
nh_wd_now_ns (void)
{
  if (__atomic_load_n (&wd_test_clock_on, __ATOMIC_ACQUIRE))
    return __atomic_load_n (&wd_test_clock_ns, __ATOMIC_RELAXED);
  struct timespec ts;
  clock_gettime (CLOCK_MONOTONIC, &ts);
  return (uint64_t) ts.tv_sec * 1000000000 + ts.tv_nsec;
}

bool
nh_wd_test_clock_p (void)
{
  return __atomic_load_n (&wd_test_clock_on, __ATOMIC_ACQUIRE) != 0;
}

void
nh_wd_hist_add (enum nh_wd_hist h, uint64_t ns)
{
  int b = 0;
  while (b < NH_WD_BUCKETS - 1 && ns >= wd_bucket_ms[b] * 1000000)
    b++;
  __atomic_fetch_add (&wd_hist[h][b], 1, __ATOMIC_RELAXED);
}

/* Busy starts at NOW.  */
static void
span_open (uint64_t now)
{
  wd_waiting = false;
  wd_busy_since = now;
  nh_wd_publish_busy (now);
  wd_gc_ns0 = __atomic_load_n (&nh_wd.gc_ns_total, __ATOMIC_RELAXED);
  wd_gcs0 = __atomic_load_n (&nh_wd.gc_count, __ATOMIC_RELAXED);
  wd_cmds0 = __atomic_load_n (&nh_wd_input.commands, __ATOMIC_RELAXED);
  nh_wd_input_span_open (now);
}

/* A ring slot for a new stall; a full ring drops its oldest.  */
static struct nh_wd_stall *
ring_slot (void)
{
  if (wd_ring_n == NH_WD_RING)
    {
      wd_ring_head = (wd_ring_head + 1) % NH_WD_RING;
      wd_ring_n--;
      __atomic_fetch_add (&nh_wd.dropped, 1, __ATOMIC_RELAXED);
    }
  return &wd_ring[(wd_ring_head + wd_ring_n++) % NH_WD_RING];
}

/* Turn the span that ends at NOW into a record.  The order is binding:
   publish "waiting" first, so from here on the
   handler counts any request as stale; only then close the samples
   (nh_wd_sample_close marks the accumulator closed before it copies).  */
static void
finalise (uint64_t now, uint64_t wall, uint64_t gc, uint64_t gcs,
	  uint64_t cmds)
{
  nh_wd_publish_waiting (now);
  struct nh_wd_stall *st = ring_slot ();
  memset (st, 0, offsetof (struct nh_wd_stall, s));
  nh_wd_sample_close (st);
  st->id = ++wd_next_id;
  st->span = __atomic_load_n (&nh_wd.span, __ATOMIC_RELAXED);
  st->start_ns = wd_busy_since;
  st->wall_ns = wall;
  st->gc_ns = gc;
  st->gcs = gcs < UINT32_MAX ? gcs : UINT32_MAX;
  st->commands = cmds < UINT32_MAX ? cmds : UINT32_MAX;
  st->kind = cmds > 0;
  nh_wd_monitor_span_close (st);
  nh_wd_input_span_close (st, now);
  if (st->deep)
    {
      int32_t left = __atomic_load_n (&nh_wd.deep_left, __ATOMIC_RELAXED);
      while (left > 0
	     && !__atomic_compare_exchange_n (&nh_wd.deep_left, &left, left - 1,
					      false, __ATOMIC_RELAXED,
					      __ATOMIC_RELAXED))
	;
    }
  __atomic_fetch_add (&nh_wd.stalls, 1, __ATOMIC_RELAXED);
}

/* The span ends at NOW: a responsive wait begins.  */
static void
span_close (uint64_t now)
{
  /* When still waiting, the last wait's leave never ran: a longjmp out
     of the select skips it (a tty C-g: handle_interrupt ->
     quit_throw_to_read_char).  The busy time since is unknown, so no
     span is counted.  */
  if (!wd_waiting)
    {
      uint64_t wall = now > wd_busy_since ? now - wd_busy_since : 0;
      uint64_t gc = (__atomic_load_n (&nh_wd.gc_ns_total, __ATOMIC_RELAXED)
		     - wd_gc_ns0);
      uint64_t gcs = (__atomic_load_n (&nh_wd.gc_count, __ATOMIC_RELAXED)
		      - wd_gcs0);
      uint64_t cmds = (__atomic_load_n (&nh_wd_input.commands,
					__ATOMIC_RELAXED)
		       - wd_cmds0);
      __atomic_fetch_add (&nh_wd.spans, 1, __ATOMIC_RELAXED);
      nh_wd_hist_add (cmds ? NH_WD_H_COMMAND : NH_WD_H_OTHER,
		      wall > gc ? wall - gc : 0);
      if (wall >= __atomic_load_n (&nh_wd.threshold_ns, __ATOMIC_RELAXED))
	finalise (now, wall, gc, gcs, cmds);
      else
	{
	  nh_wd_publish_waiting (now);
	  nh_wd_input_span_close (NULL, now);
	}
      wd_waiting = true;
    }
  nh_wd_phase_pop_to (0);
}

void
nh_wd_wait_enter (struct thread_state *self, const struct timespec *timeout)
{
  if (!main_thread_p (self) || !nh_wd_on)
    return;
  /* No wait is inside a specpdl growth: a memory_full longjmp out of
     xpalloc would otherwise leave the flag set for good.  */
  nh_wd_specpdl_unstable = 0;
  /* Responsive: returns on input, the screen can update, and it
     really waits (a NULL timeout waits forever).  SELF holds the lock
     here, so Vinhibit_redisplay is its own binding.  */
  if (self->m_waiting_for_user_input_p != 0
      && NILP (Vinhibit_redisplay)
      && (!timeout || timeout->tv_sec > 0 || timeout->tv_nsec > 0))
    {
      wd_wait_depth = -1;
      span_close (nh_wd_now_ns ());
    }
  else
    /* Busy: `accept-process-output' or `sleep-for' inside a command, a
       wait under `inhibit-redisplay', a zero-timeout poll.  */
    wd_wait_depth = nh_wd_phase_push (NH_WD_PH_PROCESS_WAIT);
}

void
nh_wd_wait_enter_modal (const struct timespec *timeout)
{
  if (!nh_wd_on || !pthread_equal (pthread_self (), nh_wd.main_id)
      || !main_thread_p (current_thread))
    return;
  if (timeout && timeout->tv_sec == 0 && timeout->tv_nsec == 0)
    return;
#ifdef HAVE_PGTK
  /* A person is using a menu or a dialog.  No waiting_for_user_input_p
     test: the field is the command's outer 0 here.  */
  if (!(popup_activated () || nh_wd_dialog_depth > 0))
    return;
  /* Only a wait that ends the span forgets a busy wait's phase depth.  */
  wd_wait_depth = -1;
  __atomic_fetch_add (&nh_wd.wrapper_edges, 1, __ATOMIC_RELAXED);
  span_close (nh_wd_now_ns ());
#endif
}

void
nh_wd_wait_leave (struct thread_state *self)
{
  if (!main_thread_p (self) || !nh_wd_on
      || !pthread_equal (pthread_self (), nh_wd.main_id))
    return;
  if (wd_wait_depth >= 0)
    {
      nh_wd_phase_pop_to (wd_wait_depth);
      wd_wait_depth = -1;
    }
  if (wd_waiting)
    span_open (nh_wd_now_ns ());
}

void
nh_wd_suspend (bool suspended)
{
  if (!main_thread_p (current_thread) || !nh_wd_on)
    return;
  if (suspended)
    {
      wd_wait_depth = -1;
      span_close (nh_wd_now_ns ());
    }
  else if (wd_waiting)
    span_open (nh_wd_now_ns ());
}

static void
wd_free (void)
{
  xfree (wd_ring);
  wd_ring = NULL;
  wd_ring_head = wd_ring_n = 0;
  xfree (nh_wd_hang_file);
  nh_wd_hang_file = NULL;
}

static uint64_t
ms_to_ns (Lisp_Object ms, Lisp_Object other)
{
  CHECK_NUMBER (ms);
  double d = XFLOATINT (ms);
  if (! (d > 0 && d < 1e9))
    args_out_of_range (ms, other);
  return d * 1e6;
}

DEFUN ("nidhogg-watchdog--start", Fnidhogg_watchdog__start,
       Snidhogg_watchdog__start, 3, MANY, 0,
       doc: /* Start the stall watchdog.
THRESHOLD-MS is the stall threshold, HANG-MS the hang threshold, both
positive numbers of milliseconds; HANG-FILE is the hang file's name, or
nil.  FLAGS may hold `:no-monitor' (tests: no monitor thread at all).
Signal an error when the watchdog is already on, when not called from
the main thread, or on a platform it does not support yet.
usage: (nidhogg-watchdog--start THRESHOLD-MS HANG-MS HANG-FILE &rest FLAGS)  */)
  (ptrdiff_t nargs, Lisp_Object *args)
{
#ifndef __linux__
  error ("nidhogg-watchdog: not on this platform yet");
#endif
  bool no_monitor = false;
  for (ptrdiff_t i = 3; i < nargs; i++)
    if (EQ (args[i], QCno_monitor))
      no_monitor = true;
    else
      signal_error ("nidhogg-watchdog: unknown flag", args[i]);
  uint64_t threshold_ns = ms_to_ns (args[0], args[1]);
  uint64_t hang_ns = ms_to_ns (args[1], args[0]);
  if (!NILP (args[2]))
    CHECK_STRING (args[2]);
  if (!main_thread_p (current_thread))
    error ("nidhogg-watchdog: start it from the main thread");
  if (nh_wd_on)
    error ("nidhogg-watchdog: already on");

  /* The name first: expanding and encoding it can signal, and nothing is
     allocated yet.  */
  if (!NILP (args[2]))
    nh_wd_hang_file
      = xstrdup (SSDATA (ENCODE_FILE (Fexpand_file_name (args[2], Qnil))));
  wd_ring = xzalloc (NH_WD_RING * sizeof *wd_ring);
  wd_ring_head = wd_ring_n = 0;
  nh_wd_gc_reset ();

  /* Nothing else runs on this state yet: no handler, no monitor, and
     the GC hook and the edges wait for nh_wd_on.  */
  __atomic_store_n (&nh_wd.threshold_ns, threshold_ns, __ATOMIC_RELAXED);
  __atomic_store_n (&nh_wd.hang_ns, hang_ns, __ATOMIC_RELAXED);
  __atomic_store_n (&nh_wd.period_ns,
		    threshold_ns / 10 > 1000000 ? threshold_ns / 10 : 1000000,
		    __ATOMIC_RELAXED);
  __atomic_store_n (&nh_wd.no_monitor, no_monitor, __ATOMIC_RELAXED);
  uint64_t *counters[] = {
    &nh_wd.spans, &nh_wd.stalls, &nh_wd.dropped, &nh_wd.wrapper_edges,
    &nh_wd.missed, &nh_wd.stale, &nh_wd.requests, &nh_wd.monitor_wakeups,
    &nh_wd.monitor_cpu_ns, &nh_wd.samples_taken, &nh_wd.gc_dropped,
  };
  for (size_t i = 0; i < ARRAYELTS (counters); i++)
    __atomic_store_n (counters[i], 0, __ATOMIC_RELAXED);
  __atomic_store_n (&nh_wd.deep_left, 0, __ATOMIC_RELAXED);
  __atomic_store_n (&nh_wd.request, 0, __ATOMIC_RELAXED);
  __atomic_store_n (&nh_wd.mode, 0, __ATOMIC_RELAXED);
  memset (wd_hist, 0, sizeof wd_hist);
  nh_wd.main_id = pthread_self ();
  wd_wait_depth = -1;
  nh_wd_phase_pop_to (0);

  if (nh_wd_handler_install () != 0)
    {
      wd_free ();
      error ("nidhogg-watchdog: cannot install the signal handler");
    }
  span_open (nh_wd_now_ns ());
  nh_wd_on = true;
  wd_monitor_started = false;
  if (!no_monitor)
    {
      if (nh_wd_monitor_start () != 0)
	{
	  nh_wd_on = false;
	  nh_wd_handler_uninstall ();
	  wd_free ();
	  error ("nidhogg-watchdog: cannot start the monitor thread");
	}
      wd_monitor_started = true;
    }
  return Qt;
}

DEFUN ("nidhogg-watchdog--stop", Fnidhogg_watchdog__stop,
       Snidhogg_watchdog__stop, 0, 0, 0,
       doc: /* Stop the stall watchdog.  Return nil.
Unfinished and untaken records are dropped.  Safe when it is off.  */)
  (void)
{
  if (!nh_wd_on)
    return Qnil;
  if (!main_thread_p (current_thread))
    error ("nidhogg-watchdog: stop it from the main thread");
  nh_wd_on = false;
  /* A stop inside a hang: SIGTERM, SIGHUP and SIGINT call `kill-emacs'
     from the signal handler, while the main thread is still busy.  The
     span in progress is dropped, so its hang file must stay for the next
     start's `:killed' record.  */
  uint64_t now = nh_wd_now_ns ();
  uint64_t hang = __atomic_load_n (&nh_wd.hang_ns, __ATOMIC_RELAXED);
  if (wd_monitor_started && !wd_waiting && hang
      && now >= wd_busy_since && now - wd_busy_since >= hang)
    nh_wd_monitor_keep_hang ();
  /* From here on the handler counts any request as stale.  */
  nh_wd_publish_waiting (now);
  if (wd_monitor_started)
    nh_wd_monitor_stop ();
  wd_monitor_started = false;
  nh_wd_handler_uninstall ();
  wd_free ();
  wd_wait_depth = -1;
  nh_wd_phase_pop_to (0);
  __atomic_store_n (&wd_test_clock_on, 0, __ATOMIC_RELEASE);
  return Qnil;
}

DEFUN ("nidhogg-watchdog--pending", Fnidhogg_watchdog__pending,
       Snidhogg_watchdog__pending, 0, 0, 0,
       doc: /* Return how many stalls and collections wait to be taken.  */)
  (void)
{
  return make_uint ((nh_wd_on ? wd_ring_n : 0) + nh_wd_gc_pending ());
}

static Lisp_Object
wd_ms (uint64_t ns)
{
  return make_float (ns / 1e6);
}

static Lisp_Object
wd_bool (unsigned x)
{
  return x ? Qt : Qnil;
}

/* The record plist of ST (`--take').  EPOCH is CLOCK_REALTIME minus CLOCK_MONOTONIC,
   in seconds.  */
static Lisp_Object
stall_to_lisp (const struct nh_wd_stall *st, double epoch)
{
  int n = st->samples < NH_WD_SAMPLES ? st->samples : NH_WD_SAMPLES;
  Lisp_Object samples = make_nil_vector (n);
  for (int i = 0; i < n; i++)
    ASET (samples, i,
	  nh_wd_sample_to_lisp (&st->s[st->order[i] % NH_WD_SAMPLES]));
  return listn (50,
		QCid, make_uint (st->id),
		QCspan, make_uint (st->span),
		QCstart, make_float (st->start_ns / 1e9 + epoch),
		QCwall_ms, wd_ms (st->wall_ns),
		QCgc_ms, wd_ms (st->gc_ns),
		QCgcs, make_uint (st->gcs),
		QCnongc_ms, wd_ms (st->wall_ns > st->gc_ns
				   ? st->wall_ns - st->gc_ns : 0),
		QCcpu_ms, wd_ms (st->cpu_ns),
		QClate_ms, wd_ms (st->late_ns),
		QCfelt, wd_bool (st->felt),
		QCinput_wait_ms, (st->input_wait_ns
				  ? wd_ms (st->input_wait_ns) : Qnil),
		QCcommands, make_uint (st->commands),
		QCinput_queued, wd_bool (st->input_queued),
		QChang, wd_bool (st->hang),
		QCkilled, wd_bool (st->killed),
		QClisp_thread, wd_bool (st->lisp_thread),
		QCdeep, wd_bool (st->deep),
		QCkind, st->kind ? Qcommand : Qother,
		QCsamples_taken, make_uint (st->samples),
		QCmissed, make_uint (st->missed),
		QCstale, make_uint (st->stale),
		QCthinned, make_uint (st->thinned),
		QCstride, make_uint (st->stride),
		QCpresented, st->presented ? make_uint (st->presented) : Qnil,
		QCsamples, samples);
}

DEFUN ("nidhogg-watchdog--take", Fnidhogg_watchdog__take,
       Snidhogg_watchdog__take, 0, 0, 0,
       doc: /* Take the finished stalls and collections; empty the rings.
Return (STALLS . COLLECTIONS), oldest first: STALLS a list of plists
\(:id :span :start :wall-ms :gc-ms :gcs :nongc-ms :cpu-ms :late-ms :felt
:input-wait-ms :commands :input-queued :hang :killed :lisp-thread :deep
:kind :samples-taken :missed :stale :thinned :stride :presented
:samples), COLLECTIONS as the GC ring gives them.  */)
  (void)
{
  Lisp_Object collections = nh_wd_gc_take ();
  Lisp_Object stalls = Qnil;
  if (nh_wd_on && wd_ring_n > 0)
    {
      struct timespec mono, real;
      clock_gettime (CLOCK_MONOTONIC, &mono);
      clock_gettime (CLOCK_REALTIME, &real);
      double epoch = ((real.tv_sec - mono.tv_sec)
		      + (real.tv_nsec - mono.tv_nsec) / 1e9);
      /* Converting runs no Lisp and never waits, so no finalise can
	 write the ring meanwhile.  */
      for (unsigned i = wd_ring_n; i > 0; i--)
	stalls = Fcons (stall_to_lisp (&wd_ring[(wd_ring_head + i - 1)
						% NH_WD_RING], epoch),
			stalls);
      wd_ring_head = wd_ring_n = 0;
    }
  return Fcons (stalls, collections);
}

DEFUN ("nidhogg-watchdog--stats", Fnidhogg_watchdog__stats,
       Snidhogg_watchdog__stats, 0, 1, 0,
       doc: /* Return the stall watchdog's counters, or nil when it is off.
\(:spans N :stalls N :dropped N :missed N :stale N :requests N :samples N
 :monitor-wakeups N :monitor-cpu-ms F :gc-dropped N :wrapper-edges N
 :histograms (:command V :other V :typing V :collections V)), each V a
vector of counts by bucket (ms): <4 <8 <16 <33 <50 <100 <250 <1000 >=1000.
With RESET non-nil, also zero them.  */)
  (Lisp_Object reset)
{
  if (!nh_wd_on)
    return Qnil;
  bool r = !NILP (reset);
#define WD_TAKE(p) (r ? __atomic_exchange_n (p, 0, __ATOMIC_RELAXED)	\
		    : __atomic_load_n (p, __ATOMIC_RELAXED))
  Lisp_Object h[NH_WD_H_N];
  for (int k = 0; k < NH_WD_H_N; k++)
    {
      h[k] = make_nil_vector (NH_WD_BUCKETS);
      for (int b = 0; b < NH_WD_BUCKETS; b++)
	ASET (h[k], b, make_uint (WD_TAKE (&wd_hist[k][b])));
    }
  Lisp_Object spans = make_uint (WD_TAKE (&nh_wd.spans));
  Lisp_Object stalls = make_uint (WD_TAKE (&nh_wd.stalls));
  Lisp_Object dropped = make_uint (WD_TAKE (&nh_wd.dropped));
  Lisp_Object missed = make_uint (WD_TAKE (&nh_wd.missed));
  Lisp_Object stale = make_uint (WD_TAKE (&nh_wd.stale));
  Lisp_Object requests = make_uint (WD_TAKE (&nh_wd.requests));
  Lisp_Object wakeups = make_uint (WD_TAKE (&nh_wd.monitor_wakeups));
  Lisp_Object cpu = wd_ms (WD_TAKE (&nh_wd.monitor_cpu_ns));
  Lisp_Object gc_dropped = make_uint (WD_TAKE (&nh_wd.gc_dropped));
  Lisp_Object edges = make_uint (WD_TAKE (&nh_wd.wrapper_edges));
  Lisp_Object samples = make_uint (WD_TAKE (&nh_wd.samples_taken));
#undef WD_TAKE
  return listn (24,
		QCspans, spans, QCstalls, stalls, QCdropped, dropped,
		QCmissed, missed, QCstale, stale, QCrequests, requests,
		QCsamples, samples,
		QCmonitor_wakeups, wakeups, QCmonitor_cpu_ms, cpu,
		QCgc_dropped, gc_dropped, QCwrapper_edges, edges,
		QChistograms,
		listn (8, QCcommand, h[NH_WD_H_COMMAND], QCother, h[NH_WD_H_OTHER],
		       QCtyping, h[NH_WD_H_TYPING],
		       QCcollections, h[NH_WD_H_COLLECTIONS]));
}

DEFUN ("nidhogg-watchdog--test-clock", Fnidhogg_watchdog__test_clock,
       Snidhogg_watchdog__test_clock, 1, 1, 0,
       doc: /* For tests: set the watchdog's clock to NS, or nil for the real one.
A number is a virtual time in nanoseconds; spans and thresholds use it,
and the monitor never arms while it is set.  Switching between the real
and the virtual clock starts a new span at the new clock.  */)
  (Lisp_Object ns)
{
  bool was = nh_wd_test_clock_p ();
  if (NILP (ns))
    __atomic_store_n (&wd_test_clock_on, 0, __ATOMIC_RELEASE);
  else
    {
      CHECK_FIXNAT (ns);
      __atomic_store_n (&wd_test_clock_ns, XFIXNAT (ns), __ATOMIC_RELAXED);
      __atomic_store_n (&wd_test_clock_on, 1, __ATOMIC_RELEASE);
    }
  if (nh_wd_on && was != !NILP (ns) && main_thread_p (current_thread)
      && !wd_waiting)
    span_open (nh_wd_now_ns ());
  return Qnil;
}

DEFUN ("nidhogg-watchdog--test-idle-wait", Fnidhogg_watchdog__test_idle_wait,
       Snidhogg_watchdog__test_idle_wait, 1, 1, 0,
       doc: /* For tests: an idle wait of SECS seconds, or forever if nil.
Waits in `wait_reading_process_output' with READ_KBD -1, as the command
loop does, so process.c sets and unwinds `waiting_for_user_input_p'.
Return nil.  */)
  (Lisp_Object secs)
{
  intmax_t sec = 0;
  int nsec = 0;
  if (!NILP (secs))
    {
      CHECK_NUMBER (secs);
      double d = XFLOATINT (secs);
      if (! (d > 0 && d < 1e6))
	args_out_of_range (secs, Qnil);
      sec = d;
      nsec = (d - sec) * 1e9;
      if (sec == 0 && nsec == 0)
	nsec = 1;
    }
  wait_reading_process_output (sec, nsec, -1, false, Qnil, NULL, 0);
  return Qnil;
}

void
syms_of_nidhogg_watchdog (void)
{
  syms_of_nidhogg_watchdog_gc ();
  DEFSYM (QCno_monitor, ":no-monitor");
  DEFSYM (QCid, ":id");
  DEFSYM (QCspan, ":span");
  DEFSYM (QCstart, ":start");
  DEFSYM (QCwall_ms, ":wall-ms");
  DEFSYM (QCgc_ms, ":gc-ms");
  DEFSYM (QCgcs, ":gcs");
  DEFSYM (QCnongc_ms, ":nongc-ms");
  DEFSYM (QCcpu_ms, ":cpu-ms");
  DEFSYM (QClate_ms, ":late-ms");
  DEFSYM (QCfelt, ":felt");
  DEFSYM (QCinput_wait_ms, ":input-wait-ms");
  DEFSYM (QCcommands, ":commands");
  DEFSYM (QCinput_queued, ":input-queued");
  DEFSYM (QChang, ":hang");
  DEFSYM (QCkilled, ":killed");
  DEFSYM (QClisp_thread, ":lisp-thread");
  DEFSYM (QCdeep, ":deep");
  DEFSYM (QCkind, ":kind");
  DEFSYM (QCsamples_taken, ":samples-taken");
  DEFSYM (QCmissed, ":missed");
  DEFSYM (QCstale, ":stale");
  DEFSYM (QCthinned, ":thinned");
  DEFSYM (QCstride, ":stride");
  DEFSYM (QCpresented, ":presented");
  DEFSYM (QCsamples, ":samples");
  DEFSYM (Qcommand, "command");
  DEFSYM (Qother, "other");
  DEFSYM (QCspans, ":spans");
  DEFSYM (QCstalls, ":stalls");
  DEFSYM (QCdropped, ":dropped");
  DEFSYM (QCrequests, ":requests");
  DEFSYM (QCmonitor_wakeups, ":monitor-wakeups");
  DEFSYM (QCmonitor_cpu_ms, ":monitor-cpu-ms");
  DEFSYM (QCgc_dropped, ":gc-dropped");
  DEFSYM (QCwrapper_edges, ":wrapper-edges");
  DEFSYM (QChistograms, ":histograms");
  DEFSYM (QCcommand, ":command");
  DEFSYM (QCother, ":other");
  DEFSYM (QCtyping, ":typing");
  DEFSYM (QCcollections, ":collections");

  defsubr (&Snidhogg_watchdog__start);
  defsubr (&Snidhogg_watchdog__stop);
  defsubr (&Snidhogg_watchdog__pending);
  defsubr (&Snidhogg_watchdog__take);
  defsubr (&Snidhogg_watchdog__stats);
  defsubr (&Snidhogg_watchdog__test_clock);
  defsubr (&Snidhogg_watchdog__test_idle_wait);
  nh_wd_monitor_syms ();

  nh_wd_syms_of_sample ();
  nh_wd_input_syms ();
#ifdef HAVE_PGTK
  nh_wd_gtk_syms ();
#endif

  Fprovide (intern_c_string ("nidhogg-watchdog"), Qnil);
}

#endif /* HAVE_NIDHOGG_WATCHDOG */

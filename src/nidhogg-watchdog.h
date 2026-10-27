/* Nidhogg's stall watchdog: the shared layout and every cross-file prototype.

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

/* The design is docs/specs/drgnx/2026-10-21-stall-watchdog-design.md
   (in the Foundation's workspace, not in this tree); README.md,
   "Stall watchdog", is the user-facing description.

   This header is the contract between the six nidhogg-watchdog*.c
   files.  Each later task owns one file and implements the prototypes
   below that name it; a change here is a change for all of them.  The
   per-file owner is in brackets next to each group of prototypes.  */

#ifndef NIDHOGG_WATCHDOG_H
#define NIDHOGG_WATCHDOG_H

#ifdef HAVE_NIDHOGG_WATCHDOG

#include <stdalign.h>
#include <stdbool.h>
#include <stdint.h>
#include <pthread.h>
#include <signal.h>
#include <time.h>

#include "lisp.h"
#include "thread.h"

#ifndef NIDHOGG_WATCHDOG_SIGNAL
# define NIDHOGG_WATCHDOG_SIGNAL SIGURG
#endif

/* Sizes.  */
enum
{
  NH_WD_WORDS = 64,		/* words per sample */
  NH_WD_INNER = 24,		/* innermost words kept (normal mode) */
  NH_WD_OUTER = 8,		/* outermost words kept (normal mode) */
  NH_WD_DEEP_INNER = 56,	/* innermost words kept in deep mode (+ 8 outer) */
  NH_WD_SAMPLES = 64,		/* samples a stall keeps (32 in deep mode) */
  NH_WD_RING = 32,		/* finished stall records waiting for Lisp */
  NH_WD_GC_RING = 64,		/* GC records waiting for Lisp */
  NH_WD_BUCKETS = 9,		/* histogram buckets, in ms: <4 <8 <16 <33
				   <50 <100 <250 <1000 >=1000 */
  NH_WD_PHASE_DEPTH = 8		/* the phase stack */
};

/* What the main thread is doing, as the hooks say.  The last five are
   reserved for engine C (nothing sets them yet).  */
enum nh_wd_phase
{
  NH_WD_PH_LISP,
  NH_WD_PH_REDISPLAY,
  NH_WD_PH_FILTER,
  NH_WD_PH_SENTINEL,
  NH_WD_PH_PROCESS_WAIT,
  NH_WD_PH_CALL_PROCESS,
  NH_WD_PH_JSON,
  NH_WD_PH_REGEX,
  NH_WD_PH_COMPOSE,
  NH_WD_PH_DELIVER,
  NH_WD_PH_TREESIT,
  NH_WD_PH_TREESIT_WAIT,
  NH_WD_PH_IMAGE,
  NH_WD_PH_WORKER_WAIT,
  NH_WD_PH_N
};

/* Why a phase waits (engine C: the worker is named by the reason).  */
enum nh_wd_reason
{
  NH_WD_RS_NONE,
  NH_WD_RS_SNAPSHOT,
  NH_WD_RS_DISPLAY,
  NH_WD_RS_IMAGE_PIXELS,
  NH_WD_RS_DETACH,
  NH_WD_RS_DRAIN
};

/* One stack sample, written by the signal handler.  WORDS are raw
   specpdl words, never Lisp objects: nothing here is traced by GC, and
   a word becomes a Lisp object only in nidhogg-watchdog--take, after
   valid_lisp_object_p.  */
struct nh_wd_sample
{
  uint64_t t_ns;
  uint16_t n_inner, n_outer, skipped;
  uint8_t phase, phase_reason, gc, lisp_thread, deep;
  uintptr_t words[NH_WD_WORDS];
  uintptr_t hook_sym[4];
  uint8_t hook_frame[4];
  uintptr_t thread_name;
};

/* A stall: one busy span at least as long as the threshold.  The same
   struct is the accumulator of the span in progress (nh_wd_cur.acc) and
   a finished record.  KIND: 0 other, 1 command.  SPAN is the span the
   accumulator belongs to.  ORDER holds the kept samples' slots in time
   order (thinning compacts this index array, never the samples).  */
struct nh_wd_stall
{
  uint64_t id, span, start_ns, wall_ns, gc_ns, cpu_ns, late_ns,
    input_wait_ns, presented;
  uint32_t gcs, commands, samples, missed, stale, thinned, stride;
  uint8_t felt, input_queued, hang, killed, lisp_thread, deep, kind;
  uint8_t order[NH_WD_SAMPLES];
  struct nh_wd_sample s[NH_WD_SAMPLES];
};

/* One collection, as the GC end hook records it.  GCS_SEQ is Emacs's
   `gcs_done' count after this collection (the field cannot be named
   gcs_done: that is a macro for a field of `globals', see globals.h).  */
struct nh_wd_gc
{
  uint64_t start_ns, pause_ns, span, gcs_seq;
  uint8_t explicit, lisp_thread;
};

/* Symbol names of the current span's innermost frames (names mode).  */
struct nh_wd_names
{
  uint64_t t_ns;
  uint16_t n;
  char name[32][64];
};

/* The span in progress, written by the handler only (Task 9).
   NAMES_NEXT is published with a release store after its slot is
   filled; the monitor reads it with acquire and re-checks it after
   copying (a seqlock, Task 18).  */
struct nh_wd_cur
{
  struct nh_wd_stall acc;
  volatile sig_atomic_t closed;
  struct nh_wd_names names[4];
  uint32_t names_next;
};

/* State shared by the main thread, the monitor thread and the signal
   handler.  Every word is accessed with __atomic_* (the handler only
   with the orders its task names).  Writers:

   main thread, only through nh_wd_publish_busy/_waiting (Task 8):
     span, waiting, busy_since_ns;
   monitor: parked, request (bit 31 = pending, low 31 bits = span;
     raised with a CAS, cleared by the handler and by publish_waiting),
     mode (0 sample, 1 names), cpu_armed_ns, late_ns and armed_span (the
     span they belong to; a reader ignores them on a mismatch),
     monitor_wakeups, monitor_cpu_ns, requests, missed (a request the
     handler has not cleared after two periods);
   handler: stride, samples_taken, stale;
   the GC end hook, on whichever Lisp thread holds the global lock
     (serialised by the lock): gc_ns_total, gc_count, gc_dropped;
   Lisp (`--deep') sets deep_left; the main thread's finalise of a deep
     stall decrements it; the monitor only reads it;
   main thread: spans, stalls, dropped, wrapper_edges.
   The settings (threshold_ns, hang_ns, period_ns, no_monitor) are set
   by `--start'.  */
struct nh_wd_shared
{
  alignas (64) uint64_t span;
  uint32_t waiting;
  uint64_t busy_since_ns;

  uint32_t parked;
  uint32_t request;
  uint32_t mode;
  uint64_t cpu_armed_ns;
  uint64_t late_ns;
  uint64_t armed_span;
  uint64_t monitor_wakeups;
  uint64_t monitor_cpu_ns;
  uint64_t requests;

  uint32_t stride;
  uint64_t samples_taken;
  uint64_t missed;
  uint64_t stale;

  uint64_t gc_ns_total;
  uint64_t gc_count;
  uint64_t gc_dropped;

  int32_t deep_left;

  uint64_t spans;
  uint64_t stalls;
  uint64_t dropped;
  uint64_t wrapper_edges;

  pthread_mutex_t m;
  pthread_cond_t cv;
  pthread_t main_id;

  uint64_t threshold_ns;
  uint64_t hang_ns;
  uint64_t period_ns;
  uint32_t no_monitor;		/* test flag: `--start' with :no-monitor */
};

/* Input accounting (Task 11).  Atomics, relaxed: kbd_buffer_store_event
   can run on a Lisp thread (maybe_quit -> gobble_input) while the main
   thread's lock-free wait_leave resets them.  */
struct nh_wd_input
{
  uint32_t felt;
  uint64_t arrival_ns;
  uint32_t queued;
  /* Commands started (command_loop_1, before command-execute): Task 11
     adds 1 per command on the main thread; never reset.  The edges
     (Task 7) take the delta per span for `commands' and `kind' and the
     span's histogram.  */
  uint64_t commands;
};

extern struct nh_wd_shared nh_wd;
extern struct nh_wd_cur nh_wd_cur;
extern struct nh_wd_input nh_wd_input;
extern volatile sig_atomic_t nh_wd_specpdl_unstable;
extern bool nh_wd_on, nh_wd_gc_explicit;

/* The specpdl growth bracket (Task 9 calls these around the realloc and
   the pointer stores in grow_specpdl_allocation).  The handler
   interrupts the main thread, so the compiler is the only reorderer: a
   signal fence on both sides of the flag.  */
INLINE void
nh_wd_specpdl_grow_begin (void)
{
  __atomic_signal_fence (__ATOMIC_SEQ_CST);
  nh_wd_specpdl_unstable = 1;
  __atomic_signal_fence (__ATOMIC_SEQ_CST);
}

INLINE void
nh_wd_specpdl_grow_end (void)
{
  __atomic_signal_fence (__ATOMIC_SEQ_CST);
  nh_wd_specpdl_unstable = 0;
  __atomic_signal_fence (__ATOMIC_SEQ_CST);
}

/* syms_of_nidhogg_watchdog, called from emacs.c.  [nidhogg-watchdog.c]  */
extern void syms_of_nidhogg_watchdog (void);

/* Edges and spans.  [Task 7, nidhogg-watchdog.c]  */
extern void nh_wd_wait_enter (struct thread_state *self,
			      const struct timespec *timeout);
extern void nh_wd_wait_enter_modal (const struct timespec *timeout);
extern void nh_wd_wait_leave (struct thread_state *self);
extern void nh_wd_suspend (bool);
extern uint64_t nh_wd_now_ns (void);

/* The histograms of `--stats' (design §6.4), one count per bucket of
   NH_WD_BUCKETS.  Any thread may add (relaxed atomics): the edges add
   every span by its non-GC time, Task 10 each collection's pause,
   Task 11 each typing command.  [Task 7]  */
enum nh_wd_hist
{
  NH_WD_H_COMMAND,
  NH_WD_H_OTHER,
  NH_WD_H_TYPING,
  NH_WD_H_COLLECTIONS,
  NH_WD_H_N
};
extern void nh_wd_hist_add (enum nh_wd_hist, uint64_t ns);

/* True while `--test-clock' holds a virtual time; the monitor never
   arms then (Task 8).  [Task 7]  */
extern bool nh_wd_test_clock_p (void);

/* `--start''s HANG-FILE, encoded, or NULL.  Set before the monitor
   starts, freed after it stopped (Task 18 reads it).  [Task 7]  */
extern char *nh_wd_hang_file;

/* The monitor and the park protocol, both sides.
   [Task 8, nidhogg-watchdog-monitor.c]  */
extern void nh_wd_publish_waiting (uint64_t now);
extern void nh_wd_publish_busy (uint64_t now);
extern int nh_wd_monitor_start (void);
extern void nh_wd_monitor_stop (void);
extern void nh_wd_monitor_span_close (struct nh_wd_stall *);
extern void nh_wd_monitor_syms (void);

/* The handler, the raw ring and `--take'.
   [Task 9, nidhogg-watchdog-sample.c]
   A sample's T_NS is nanoseconds since the span's busy start; WORDS
   holds N_INNER words innermost first, then N_OUTER words of the
   bottom of the stack, also innermost first; SKIPPED counts the frames
   between them; HOOK_FRAME indexes WORDS; THREAD_NAME holds the raw
   `struct thread_state *' of the Lisp thread that held the lock (its
   name is read at `--take').  In a stall, SAMPLES is the number of
   samples kept (S[0..SAMPLES) in time order once closed), THINNED the
   number thinning dropped, STRIDE the final stride.
   nh_wd_handler_install returns 0, or -1 off the main thread, when
   another handler owns the signal, or when sigaction fails.  */
extern int nh_wd_handler_install (void);
extern void nh_wd_handler_uninstall (void);
extern void nh_wd_sample_close (struct nh_wd_stall *out);
extern Lisp_Object nh_wd_sample_to_lisp (const struct nh_wd_sample *);
extern void nh_wd_syms_of_sample (void);

/* The GC end hook and the GC ring.  [Task 10, nidhogg-watchdog-gc.c]
   nh_wd_gc_pending is `--pending''s count of collections; each pause
   goes into the NH_WD_H_COLLECTIONS histogram;
   syms_of_nidhogg_watchdog calls syms_of_nidhogg_watchdog_gc.  */
extern void nh_wd_gc_done (struct timespec start);
extern Lisp_Object nh_wd_gc_take (void);
extern uint32_t nh_wd_gc_pending (void);
extern void syms_of_nidhogg_watchdog_gc (void);

/* Phases, felt input, typing.  [Task 11, nidhogg-watchdog-input.c]
   The inline phase push/pop are in nidhogg-watchdog-phase.h.
   nh_wd_input_span_close runs at every span's end: ST is the stall to
   fill, or NULL for a span below the threshold.  */
extern void nh_wd_input_span_open (uint64_t now);
extern void nh_wd_input_span_close (struct nh_wd_stall *, uint64_t now);
extern void nh_wd_unwind_phases (struct handler *catch);

/* The GLib poll wrapper and the dialog depth (pgtk only).
   [Task 12, nidhogg-watchdog-gtk.c]  */
extern void nh_wd_gtk_install (void);
extern void nh_wd_gtk_syms (void);
extern int nh_wd_dialog_depth;

#endif /* HAVE_NIDHOGG_WATCHDOG */

#endif /* NIDHOGG_WATCHDOG_H */

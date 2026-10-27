/* Nidhogg's stall watchdog: the phase stack and the input hooks.

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

/* A phase says what C is doing when the Lisp frames cannot (design
   §4.4): the one-line hooks in upstream files push one around the work
   and pop back to the depth the push returned.  The stack is the main
   thread's: a push anywhere else (a Lisp thread, the watchdog off, a
   full stack) pushes nothing and returns NH_WD_PHASE_NONE, and popping
   to that is a no-op.  A responsive wait resets it (the edges pop to
   0), a non-local exit pops what was pushed inside its catch target
   (nh_wd_unwind_phases, called by unwind_to_catch: PD-4).

   The signal handler reads the stack (nh_wd_phase_top) while it
   interrupts the main thread, so the compiler is the only reorderer:
   the push writes the slot, then a signal fence, then the depth (review
   S17); a pop only lowers the depth, and a slot above the depth is
   never read.  */

#ifndef NIDHOGG_WATCHDOG_PHASE_H
#define NIDHOGG_WATCHDOG_PHASE_H

#ifdef HAVE_NIDHOGG_WATCHDOG

#include <limits.h>

#include "nidhogg-watchdog.h"

/* What a push returns when it pushed nothing.  */
enum { NH_WD_PHASE_NONE = INT_MAX };

/* One pushed phase.  H is `handlerlist' at the push: a non-local exit
   to H or to an older handler leaves the push's scope.  */
struct nh_wd_phase_slot
{
  uint8_t phase, reason;
  struct handler *h;
};

struct nh_wd_phases
{
  int depth;
  struct nh_wd_phase_slot slot[NH_WD_PHASE_DEPTH];
};

extern struct nh_wd_phases nh_wd_phases;

/* True on the main thread once the watchdog has started there.  */
extern _Thread_local bool nh_wd_phase_thread;

/* The depth redisplay_internal's push returned, for unwind_redisplay
   (redisplay never nests).  */
extern int nh_wd_redisplay_depth;

/* The phases' Lisp names, by enum nh_wd_phase.  [nidhogg-watchdog-sample.c]  */
extern const char *const nh_wd_phase_names[NH_WD_PH_N];

/* The push of the phase `nidhogg-watchdog--test-sample-on-phase'
   names takes a sample (tests only).  */
extern void nh_wd_test_phase_hit (int ph);

/* Push phase PH with REASON (an enum nh_wd_reason); return the depth
   before, to pass to nh_wd_phase_pop_to.  */
INLINE int
nh_wd_phase_push_reason (int ph, int reason)
{
  if (!nh_wd_on || !nh_wd_phase_thread)
    return NH_WD_PHASE_NONE;
  int d = nh_wd_phases.depth;
  if (d >= NH_WD_PHASE_DEPTH)
    return NH_WD_PHASE_NONE;
  struct nh_wd_phase_slot *s = &nh_wd_phases.slot[d];
  s->phase = ph;
  s->reason = reason;
  s->h = handlerlist;
  __atomic_signal_fence (__ATOMIC_SEQ_CST);
  __atomic_store_n (&nh_wd_phases.depth, d + 1, __ATOMIC_RELAXED);
  if (!NILP (Vnidhogg_watchdog__test_sample_on_phase))
    nh_wd_test_phase_hit (ph);
  return d;
}

/* Push phase PH.  */
INLINE int
nh_wd_phase_push (int ph)
{
  return nh_wd_phase_push_reason (ph, NH_WD_RS_NONE);
}

/* Pop every phase above DEPTH; a no-op when the depth is already
   DEPTH or less (so popping twice, or after a reset, is harmless).  */
INLINE void
nh_wd_phase_pop_to (int depth)
{
  if (0 <= depth && depth < nh_wd_phases.depth)
    __atomic_store_n (&nh_wd_phases.depth, depth, __ATOMIC_RELAXED);
}

/* The top of the stack, for the signal handler: plain loads only, no
   call.  An empty stack is Lisp.  */
INLINE void
nh_wd_phase_top (uint8_t *phase, uint8_t *reason)
{
  int d = __atomic_load_n (&nh_wd_phases.depth, __ATOMIC_RELAXED);
  __atomic_signal_fence (__ATOMIC_SEQ_CST);
  if (0 < d && d <= NH_WD_PHASE_DEPTH)
    {
      *phase = nh_wd_phases.slot[d - 1].phase;
      *reason = nh_wd_phases.slot[d - 1].reason;
    }
  else
    {
      *phase = NH_WD_PH_LISP;
      *reason = NH_WD_RS_NONE;
    }
}

/* The keyboard hooks (keyboard.c).  [nidhogg-watchdog-input.c]
   nh_wd_command_start: command_loop_1, before `command-execute'.
   nh_wd_input_stored: kbd_buffer_store_buffered_event, for every event
     it is given; nh_wd_input_buffered: the same, with the event's slot
     in kbd_buffer once copied there.
   nh_wd_input_taken: kbd_buffer_get_event, with the slot it takes.  */
struct input_event;
extern void nh_wd_command_start (Lisp_Object command);
extern void nh_wd_input_stored (const struct input_event *);
extern void nh_wd_input_buffered (const struct input_event *slot);
extern void nh_wd_input_taken (const struct input_event *slot);

/* Lisp: the input file's primitives, from syms_of_nidhogg_watchdog.  */
extern void nh_wd_input_syms (void);

#endif /* HAVE_NIDHOGG_WATCHDOG */

#endif /* NIDHOGG_WATCHDOG_PHASE_H */

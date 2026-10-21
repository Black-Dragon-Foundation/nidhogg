/* Nidhogg's stall watchdog: the phase stack's inline push and pop.

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

/* Stubs (Task 4).  Task 11 replaces the bodies: the push writes the
   slot, fences with __atomic_signal_fence, then increments the depth;
   each push also records handlerlist so unwind_to_catch can pop it.  */

#ifndef NIDHOGG_WATCHDOG_PHASE_H
#define NIDHOGG_WATCHDOG_PHASE_H

#ifdef HAVE_NIDHOGG_WATCHDOG

#include "nidhogg-watchdog.h"

/* Push phase PH; return the depth to pass to nh_wd_phase_pop_to.  */
INLINE int
nh_wd_phase_push (int ph)
{
  (void) ph;
  return 0;
}

/* Likewise, with a REASON (an enum nh_wd_reason).  */
INLINE int
nh_wd_phase_push_reason (int ph, int reason)
{
  (void) ph;
  (void) reason;
  return 0;
}

/* Pop every phase above DEPTH.  */
INLINE void
nh_wd_phase_pop_to (int depth)
{
  (void) depth;
}

/* The top of the stack, for the signal handler (Task 9 reads it): plain
   loads only, no call.  */
INLINE void
nh_wd_phase_top (uint8_t *phase, uint8_t *reason)
{
  *phase = NH_WD_PH_LISP;
  *reason = NH_WD_RS_NONE;
}

#endif /* HAVE_NIDHOGG_WATCHDOG */

#endif /* NIDHOGG_WATCHDOG_PHASE_H */

/* Nidhogg's worker threads: phase macros that map onto the watchdog's.

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

/* NH_WK_PHASE_PUSH (ph, reason) returns the depth to hand to
   NH_WK_PHASE_POP.  With the watchdog built in they push onto its phase
   stack, but only on the main thread (the stack is the main thread's; a
   wait on a Lisp thread must not write it); without it they do nothing,
   so the workers build and run without the watchdog flag.  Stubs
   (Task 2); Task 5 counts the phases' ns.  */

#ifndef NIDHOGG_WORKERS_PHASE_H
#define NIDHOGG_WORKERS_PHASE_H

#ifdef HAVE_NIDHOGG_WORKERS

#ifdef HAVE_NIDHOGG_WATCHDOG

#include "nidhogg-watchdog.h"
#include "nidhogg-watchdog-phase.h"

# define NH_WK_PH_DELIVER NH_WD_PH_DELIVER
# define NH_WK_PH_TREESIT NH_WD_PH_TREESIT
# define NH_WK_PH_TREESIT_WAIT NH_WD_PH_TREESIT_WAIT
# define NH_WK_PH_IMAGE NH_WD_PH_IMAGE
# define NH_WK_PH_WORKER_WAIT NH_WD_PH_WORKER_WAIT
# define NH_WK_PH_JSON NH_WD_PH_JSON
# define NH_WK_PH_FILTER NH_WD_PH_FILTER
# define NH_WK_RS_SNAPSHOT NH_WD_RS_SNAPSHOT
# define NH_WK_RS_DISPLAY NH_WD_RS_DISPLAY
# define NH_WK_RS_IMAGE_PIXELS NH_WD_RS_IMAGE_PIXELS
# define NH_WK_RS_DETACH NH_WD_RS_DETACH
# define NH_WK_RS_DRAIN NH_WD_RS_DRAIN

INLINE int
nh_wk_phase_push (int ph, int reason)
{
  return main_thread_p (current_thread)
	 ? nh_wd_phase_push_reason (ph, reason) : -1;
}

INLINE void
nh_wk_phase_pop (int depth)
{
  if (depth >= 0)
    nh_wd_phase_pop_to (depth);
}

#else /* not HAVE_NIDHOGG_WATCHDOG */

enum nh_wk_phase
{
  NH_WK_PH_DELIVER,
  NH_WK_PH_TREESIT,
  NH_WK_PH_TREESIT_WAIT,
  NH_WK_PH_IMAGE,
  NH_WK_PH_WORKER_WAIT,
  NH_WK_PH_JSON,
  NH_WK_PH_FILTER
};

enum nh_wk_reason
{
  NH_WK_RS_NONE,
  NH_WK_RS_SNAPSHOT,
  NH_WK_RS_DISPLAY,
  NH_WK_RS_IMAGE_PIXELS,
  NH_WK_RS_DETACH,
  NH_WK_RS_DRAIN
};

INLINE int
nh_wk_phase_push (int ph, int reason)
{
  (void) ph;
  (void) reason;
  return 0;
}

INLINE void
nh_wk_phase_pop (int depth)
{
  (void) depth;
}

#endif /* not HAVE_NIDHOGG_WATCHDOG */

#define NH_WK_PHASE_PUSH(ph, reason) nh_wk_phase_push ((ph), (reason))
#define NH_WK_PHASE_POP(depth) nh_wk_phase_pop (depth)

#endif /* HAVE_NIDHOGG_WORKERS */

#endif /* NIDHOGG_WORKERS_PHASE_H */

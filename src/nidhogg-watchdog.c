/* Nidhogg's stall watchdog: edges, spans, records and the Lisp interface.

   Stubs (Task 4); Task 7 writes the busy/idle edges, the spans and the
   records, and the real `nidhogg-watchdog--start'.

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

#include <config.h>

#ifdef HAVE_NIDHOGG_WATCHDOG

#include "nidhogg-watchdog.h"

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

void
nh_wd_wait_enter (struct thread_state *self, const struct timespec *timeout)
{
  (void) self;
  (void) timeout;
}

void
nh_wd_wait_enter_modal (const struct timespec *timeout)
{
  (void) timeout;
}

void
nh_wd_wait_leave (struct thread_state *self)
{
  (void) self;
}

void
nh_wd_suspend (bool suspended)
{
  (void) suspended;
}

uint64_t
nh_wd_now_ns (void)
{
  return 0;
}

DEFUN ("nidhogg-watchdog--start", Fnidhogg_watchdog__start,
       Snidhogg_watchdog__start, 0, MANY, 0,
       doc: /* Start the stall watchdog.
Not built yet: signals an error.  (Task 7 writes it.)
usage: (nidhogg-watchdog--start &rest ARGS)  */)
  (ptrdiff_t nargs, Lisp_Object *args)
{
  (void) nargs;
  (void) args;
  error ("nidhogg-watchdog: not built yet");
}

DEFUN ("nidhogg-watchdog--stop", Fnidhogg_watchdog__stop,
       Snidhogg_watchdog__stop, 0, 0, 0,
       doc: /* Stop the stall watchdog.  Return nil.  */)
  (void)
{
  return Qnil;
}

DEFUN ("nidhogg-watchdog--stats", Fnidhogg_watchdog__stats,
       Snidhogg_watchdog__stats, 0, 0, 0,
       doc: /* Return the stall watchdog's counters, or nil when it is off.  */)
  (void)
{
  return Qnil;
}

void
syms_of_nidhogg_watchdog (void)
{
  defsubr (&Snidhogg_watchdog__start);
  defsubr (&Snidhogg_watchdog__stop);
  defsubr (&Snidhogg_watchdog__stats);

  Fprovide (intern_c_string ("nidhogg-watchdog"), Qnil);
}

#endif /* HAVE_NIDHOGG_WATCHDOG */

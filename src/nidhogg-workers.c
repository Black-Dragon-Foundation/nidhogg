/* Nidhogg's worker threads: threads, queues, jobs, waits, wake fds.

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

/* Stubs (Task 2).  Task 3 writes the runtime, Task 4 the dead flag,
   Task 5 the redisplay budget.  Every stub is the "no workers" answer:
   a submit is refused, so the caller takes today's path.  */

#ifndef NH_WK_HARNESS
# include <config.h>
#endif

#ifdef HAVE_NIDHOGG_WORKERS

#include "nidhogg-workers.h"
#ifndef NH_WK_HARNESS
# include "nidhogg-workers-phase.h"
#endif

_Atomic bool nh_wk_dead;
bool nh_wk_inline;

bool
nh_wk_submit (struct nh_wk_job *job)
{
  (void) job;
  return false;
}

int
nh_wk_wait (struct nh_wk_job *job, int64_t timeout_ns, int phase, int reason)
{
  (void) job;
  (void) timeout_ns;
  (void) phase;
  (void) reason;
  return 1;
}

void
nh_wk_cancel (struct nh_wk_job *job)
{
  (void) job;
}

void
nh_wk_job_ref (struct nh_wk_job *job)
{
  (void) job;
}

void
nh_wk_job_unref (struct nh_wk_job *job)
{
  (void) job;
}

void
nh_wk_complete (struct nh_wk_job *job)
{
  (void) job;
}

void
nh_wk_pause_point (enum nh_wk_pause p)
{
  (void) p;
}

int
nh_wk_wake_open (struct nh_wk_wake *w)
{
  w->rfd = w->wfd = -1;
  return -1;
}

void
nh_wk_wake_signal (struct nh_wk_wake *w)
{
  (void) w;
}

void
nh_wk_wake_clear (struct nh_wk_wake *w)
{
  (void) w;
}

void
nh_wk_wake_close (struct nh_wk_wake *w)
{
  (void) w;
}

bool
nh_wk_fd_budget_ok (int extra)
{
  (void) extra;
  return true;
}

bool
nh_wk_on_worker (void)
{
  return false;
}

void
nh_wk_fatal_begin (void)
{
}

int64_t
nh_wk_budget_take (int64_t want_ns)
{
  return want_ns;
}

void
nh_wk_budget_reset (void)
{
}

#ifndef NH_WK_HARNESS

void
syms_of_nidhogg_workers (void)
{
  nh_ts_syms ();
  nh_io_syms ();
  nh_dec_syms ();
  nh_json_syms ();
  nh_img_syms ();
  nh_blk_syms ();

  Fprovide (intern_c_string ("nidhogg-workers"), Qnil);
}

#endif /* not NH_WK_HARNESS */

#endif /* HAVE_NIDHOGG_WORKERS */

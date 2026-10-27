/* Nidhogg's stall watchdog: the GC end hook and the GC ring.

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

/* garbage_collect calls nh_wd_gc_done at its end (alloc.c), after
   `gcs_done' is incremented and before `post-gc-hook' runs, so a
   collection's pause includes the finalizers and the image cache
   pruning but no Lisp hook, and its `gcs_done' is the value a
   `post-gc-hook' function reads: the key the Lisp GC row joins on.

   The hook runs on whichever Lisp thread holds the global lock, so the
   ring and the histogram are serialised by the lock, as is
   nh_wd_gc_take (Lisp).  The span totals (gc_ns_total, gc_count) are
   atomics: the main thread reads them at its edges without the lock.
   No allocation, no Lisp, and nothing here is a GC root.  */

#include <config.h>

#ifdef HAVE_NIDHOGG_WATCHDOG

#include "nidhogg-watchdog.h"
#include "systime.h"

/* The GC ring: COUNT entries from HEAD (the oldest), WRITTEN entries
   ever written (the newest entry's id).  A full ring drops its
   oldest entry.  */
static struct nh_wd_gc nh_wd_gc_ring[NH_WD_GC_RING];
static int nh_wd_gc_head, nh_wd_gc_count;
static uint64_t nh_wd_gc_written;

static uint64_t
nh_wd_gc_ns (struct timespec t)
{
  return t.tv_sec < 0 ? 0 : (uint64_t) t.tv_sec * 1000000000 + t.tv_nsec;
}

/* The end of a collection that began at START (garbage_collect's
   `current_timespec').  */
void
nh_wd_gc_done (struct timespec start)
{
  /* Cleared on every collection, so a `garbage-collect' while the
     mode is off never marks a later automatic collection.  */
  bool explicit = nh_wd_gc_explicit;
  nh_wd_gc_explicit = false;
  if (!nh_wd_on)
    return;

  uint64_t pause = nh_wd_gc_ns (timespec_sub (current_timespec (), start));
  __atomic_fetch_add (&nh_wd.gc_ns_total, pause, __ATOMIC_RELAXED);
  __atomic_fetch_add (&nh_wd.gc_count, 1, __ATOMIC_RELAXED);
  nh_wd_hist_add (NH_WD_H_COLLECTIONS, pause);

  if (nh_wd_gc_count == NH_WD_GC_RING)
    {
      nh_wd_gc_head = (nh_wd_gc_head + 1) % NH_WD_GC_RING;
      nh_wd_gc_count--;
      __atomic_fetch_add (&nh_wd.gc_dropped, 1, __ATOMIC_RELAXED);
    }
  struct nh_wd_gc *e
    = &nh_wd_gc_ring[(nh_wd_gc_head + nh_wd_gc_count) % NH_WD_GC_RING];
  e->start_ns = nh_wd_gc_ns (start);
  e->pause_ns = pause;
  e->span = __atomic_load_n (&nh_wd.span, __ATOMIC_RELAXED);
  e->gcs_seq = gcs_done;
  e->explicit = explicit;
  e->lisp_thread = !main_thread_p (current_thread);
  nh_wd_gc_count++;
  nh_wd_gc_written++;
}

/* Collections waiting for `--take' (its `--pending' count).  */
uint32_t
nh_wd_gc_pending (void)
{
  return nh_wd_gc_count;
}

/* The waiting collections as a list, oldest first, and remove them:
   (:gc ID :gcs-done N :start SECONDS :pause-ms F :span N :explicit BOOL
    :lisp-thread BOOL).  */
Lisp_Object
nh_wd_gc_take (void)
{
  int n = nh_wd_gc_count;
  uint64_t first_id = nh_wd_gc_written - n + 1;
  Lisp_Object list = Qnil;
  for (int i = n - 1; i >= 0; i--)
    {
      struct nh_wd_gc const *e
	= &nh_wd_gc_ring[(nh_wd_gc_head + i) % NH_WD_GC_RING];
      list = Fcons (list (QCgc, make_uint (first_id + i),
			  QCgcs_done, make_int ((int64_t) e->gcs_seq),
			  QCstart, make_float (e->start_ns / 1e9),
			  QCpause_ms, make_float (e->pause_ns / 1e6),
			  QCspan, make_uint (e->span),
			  QCexplicit, e->explicit ? Qt : Qnil,
			  QClisp_thread, e->lisp_thread ? Qt : Qnil),
		    list);
    }
  /* Remove only what was copied (consing here never collects, but the
     ring stays right if it ever did).  */
  nh_wd_gc_head = (nh_wd_gc_head + n) % NH_WD_GC_RING;
  nh_wd_gc_count -= n;
  return list;
}

DEFUN ("nidhogg-watchdog--test-gc-ns", Fnidhogg_watchdog__test_gc_ns,
       Snidhogg_watchdog__test_gc_ns, 1, 1, 0,
       doc: /* Test only: add NS nanoseconds of pretend GC to the current span.
It counts as one collection in the span's GC time and count, for the
virtual-clock tests; no GC-ring entry is written.  Return nil.  */)
  (Lisp_Object ns)
{
  CHECK_FIXNAT (ns);
  __atomic_fetch_add (&nh_wd.gc_ns_total, (uint64_t) XFIXNAT (ns),
		      __ATOMIC_RELAXED);
  __atomic_fetch_add (&nh_wd.gc_count, 1, __ATOMIC_RELAXED);
  return Qnil;
}

void
syms_of_nidhogg_watchdog_gc (void)
{
  DEFSYM (QCgc, ":gc");
  DEFSYM (QCgcs_done, ":gcs-done");
  DEFSYM (QCstart, ":start");
  DEFSYM (QCpause_ms, ":pause-ms");
  DEFSYM (QCspan, ":span");
  DEFSYM (QCexplicit, ":explicit");
  DEFSYM (QClisp_thread, ":lisp-thread");

  defsubr (&Snidhogg_watchdog__test_gc_ns);
}

#endif /* HAVE_NIDHOGG_WATCHDOG */

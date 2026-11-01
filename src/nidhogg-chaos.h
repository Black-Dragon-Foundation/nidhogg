/* Seeded delays at the workers' sync points, for test runs.

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

/* A chaos build (configure --with-nidhogg-chaos, build.sh --dev --chaos)
   perturbs the order threads reach the workers' sync points, so that a
   test finds an ordering bug on a machine where the timing never shows
   it.  At each point a thread does nothing (about 3 in 4), yields, or
   sleeps 1-200 us, by a per-thread xorshift seeded from
   NIDHOGG_TEST_SEED (decimal, default 0; child Emacs processes inherit
   it) and the order threads first reach a point.  The first point prints
   `chaos seed N' to stderr.  A seed makes a bug likely, not certain:
   the kernel still schedules.

   Off in signal context and on the fatal path, where nothing may wait.
   SITE names the point, for a breakpoint under rr or gdb.  Anything but
   a chaos build compiles the points to nothing.  */

#ifndef NIDHOGG_CHAOS_H
#define NIDHOGG_CHAOS_H

#ifdef HAVE_NIDHOGG_WORKERS

#ifdef NH_CHAOS
extern void nh_chaos_point (const char *site);
# define NH_CHAOS_POINT(site) nh_chaos_point (site)
#else
# define NH_CHAOS_POINT(site) ((void) 0)
#endif

#endif /* HAVE_NIDHOGG_WORKERS */

#endif /* NIDHOGG_CHAOS_H */

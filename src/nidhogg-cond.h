/* Condition variables timed on CLOCK_MONOTONIC, for Nidhogg's threads.

Copyright (C) 2026 Black Dragon Foundation contributors

This file is part of Nidhogg.

Nidhogg is free software: you can redistribute it and/or modify
it under the terms of the GNU General Public License as published by
the Free Software Foundation, either version 3 of the License, or
(at your option) any later version.

Nidhogg is distributed in the hope that it will be useful,
but WITHOUT ANY WARRANTY; without even the implied warranty of
MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
GNU General Public License for more details.

You should have received a copy of the GNU General Public License
along with Nidhogg.  If not, see <https://www.gnu.org/licenses/>.  */

/* The worker threads and the watchdog's monitor both wait on these;
   either may be built without the other (--with-nidhogg-workers,
   --with-nidhogg-watchdog), so they live here, header-only.  A deadline
   is CLOCK_MONOTONIC in ns.  macOS has no pthread_condattr_setclock:
   there the wait is relative, for the time left until the deadline.  */

#ifndef NIDHOGG_COND_H
#define NIDHOGG_COND_H

#include <pthread.h>
#include <stdint.h>
#include <time.h>

/* CLOCK_MONOTONIC in ns.  */
static inline uint64_t
nh_cond_now_ns (void)
{
  struct timespec ts;
  clock_gettime (CLOCK_MONOTONIC, &ts);
  return (uint64_t) ts.tv_sec * 1000000000 + ts.tv_nsec;
}

static inline void
nh_cond_init (pthread_cond_t *cv)
{
#ifdef __APPLE__
  pthread_cond_init (cv, NULL);
#else
  pthread_condattr_t attr;
  pthread_condattr_init (&attr);
  pthread_condattr_setclock (&attr, CLOCK_MONOTONIC);
  pthread_cond_init (cv, &attr);
  pthread_condattr_destroy (&attr);
#endif
}

/* Wait on CV (M held) until DEADLINE, a signal or a spurious wake-up;
   pthread_cond_timedwait's value.  */
static inline int
nh_cond_wait_until (pthread_cond_t *cv, pthread_mutex_t *m,
		    uint64_t deadline)
{
#ifdef __APPLE__
  uint64_t now = nh_cond_now_ns ();
  uint64_t left = deadline > now ? deadline - now : 0;
  struct timespec rel = { .tv_sec = left / 1000000000,
			  .tv_nsec = left % 1000000000 };
  return pthread_cond_timedwait_relative_np (cv, m, &rel);
#else
  struct timespec ts = { .tv_sec = deadline / 1000000000,
			 .tv_nsec = deadline % 1000000000 };
  return pthread_cond_timedwait (cv, m, &ts);
#endif
}

#endif /* NIDHOGG_COND_H */

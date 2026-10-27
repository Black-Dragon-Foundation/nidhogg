/* Nidhogg's stall watchdog: the monitor thread and the park protocol.

   Stubs (Task 4); Task 8 writes the monitor and both sides of the park
   protocol, Task 18 the hang file writer and deleter.

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
#include "nidhogg-watchdog-park.h"

void
nh_wd_publish_waiting (uint64_t now)
{
  (void) now;
}

void
nh_wd_publish_busy (uint64_t now)
{
  (void) now;
}

int
nh_wd_monitor_start (void)
{
  return 0;
}

void
nh_wd_monitor_stop (void)
{
}

void
nh_wd_monitor_span_close (struct nh_wd_stall *st)
{
  (void) st;
}

#endif /* HAVE_NIDHOGG_WATCHDOG */

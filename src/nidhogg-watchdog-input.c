/* Nidhogg's stall watchdog: phases, felt input and typing commands.

   Stubs (Task 4); Task 11 writes them.

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
#include "nidhogg-watchdog-phase.h"

void
nh_wd_input_span_open (uint64_t now)
{
  (void) now;
}

void
nh_wd_input_span_close (struct nh_wd_stall *st, uint64_t now)
{
  (void) st;
  (void) now;
}

void
nh_wd_unwind_phases (struct handler *catch)
{
  (void) catch;
}

#endif /* HAVE_NIDHOGG_WATCHDOG */

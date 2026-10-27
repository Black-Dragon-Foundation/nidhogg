/* Nidhogg's stall watchdog: the GC end hook and the GC ring.

   Stubs (Task 4); Task 10 writes them.

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

void
nh_wd_gc_done (struct timespec start)
{
  (void) start;
}

Lisp_Object
nh_wd_gc_take (void)
{
  return Qnil;
}

#endif /* HAVE_NIDHOGG_WATCHDOG */

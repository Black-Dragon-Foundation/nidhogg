/* Nidhogg's stall watchdog: the signal handler, the raw ring and `--take'.

   Stubs (Task 4); Task 9 writes the handler, the guards, the specpdl
   walk, the thinning and the validation, Task 18 names mode.

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

int
nh_wd_handler_install (void)
{
  return 0;
}

void
nh_wd_handler_uninstall (void)
{
}

void
nh_wd_sample_close (struct nh_wd_stall *out)
{
  (void) out;
}

Lisp_Object
nh_wd_sample_to_lisp (const struct nh_wd_sample *s)
{
  (void) s;
  return Qnil;
}

#endif /* HAVE_NIDHOGG_WATCHDOG */

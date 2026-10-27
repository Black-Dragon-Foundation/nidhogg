/* Nidhogg's stall watchdog: the GLib poll wrapper and the dialog depth.

   Pgtk only (the body is under HAVE_PGTK).  Stubs (Task 4); Task 12
   writes them.

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

#ifdef HAVE_PGTK

#include "nidhogg-watchdog.h"

int nh_wd_dialog_depth;

void
nh_wd_gtk_install (void)
{
}

#endif /* HAVE_PGTK */

#endif /* HAVE_NIDHOGG_WATCHDOG */

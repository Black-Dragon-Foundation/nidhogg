/* Nidhogg's worker threads: directory reads, file blocks, auto-save, DNS.

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

/* Stubs (Task 2); Tasks 32 to 35 fill them.  */

#ifndef NH_WK_HARNESS
# include <config.h>
#endif

#ifdef HAVE_NIDHOGG_WORKERS

#include "nidhogg-workers.h"

bool nh_save_keyboard_triggered;

bool
nh_save_auto_save_1 (struct buffer *b)
{
  (void) b;
  return false;
}

bool
nh_dns_start (struct Lisp_Process *p, const char *host, const char *service,
	      const struct addrinfo *hints)
{
  (void) p;
  (void) host;
  (void) service;
  (void) hints;
  return false;
}

#ifndef NH_WK_HARNESS

bool
nh_save_name_in_flight (Lisp_Object name)
{
  (void) name;
  return false;
}

void
nh_blk_syms (void)
{
}

#endif /* not NH_WK_HARNESS */

#endif /* HAVE_NIDHOGG_WORKERS */

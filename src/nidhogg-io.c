/* Nidhogg's worker threads: the process reader (nh-io).

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

/* Stubs (Task 2); Tasks 16 to 20 and 24 fill them.  No process is ever
   handed over, so every caller keeps today's path.  */

#ifndef NH_WK_HARNESS
# include <config.h>
#endif

#ifdef HAVE_NIDHOGG_WORKERS

#include "nidhogg-workers.h"

bool
nh_io_handover (struct Lisp_Process *p)
{
  (void) p;
  return false;
}

void
nh_io_handback (struct Lisp_Process *p, int reason)
{
  (void) p;
  (void) reason;
}

int
nh_io_select_fd (struct Lisp_Process *p)
{
  (void) p;
  return -1;
}

void
nh_io_pause (int infd)
{
  (void) infd;
}

void
nh_io_resume (int infd)
{
  (void) infd;
}

ptrdiff_t
nh_io_read (struct Lisp_Process *p, int channel, bool *handled)
{
  (void) p;
  (void) channel;
  *handled = false;
  return 0;
}

void
nh_io_drain (struct Lisp_Process *p)
{
  (void) p;
}

void
nh_io_coding_changed (struct Lisp_Process *p)
{
  (void) p;
}

int
nh_io_tls_boot (struct Lisp_Process *p, void *gnutls_state)
{
  (void) p;
  (void) gnutls_state;
  return 0;
}

void
nh_io_dead_close (struct Lisp_Process *p)
{
  (void) p;
}

#ifndef NH_WK_HARNESS

void
nh_io_syms (void)
{
}

#endif /* not NH_WK_HARNESS */

#endif /* HAVE_NIDHOGG_WORKERS */

/* Nidhogg's worker threads: stateless decoders and the auto-save encoder.

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

/* Stubs (Task 2); Tasks 18, 33 and 34 fill them.  */

#ifndef NH_WK_HARNESS
# include <config.h>
#endif

#ifdef HAVE_NIDHOGG_WORKERS

#include "nidhogg-workers.h"

ptrdiff_t
nh_dec_chunk (int system, const unsigned char *in, ptrdiff_t n,
	      struct nh_dec_state *st, unsigned char *out, ptrdiff_t *nchars,
	      bool last)
{
  (void) system;
  (void) in;
  (void) n;
  (void) st;
  (void) out;
  (void) last;
  *nchars = 0;
  return 0;
}

#ifndef NH_WK_HARNESS

void
nh_dec_syms (void)
{
}

#endif /* not NH_WK_HARNESS */

#endif /* HAVE_NIDHOGG_WORKERS */

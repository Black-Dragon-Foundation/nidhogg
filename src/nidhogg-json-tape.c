/* Nidhogg's worker threads: the JSON tokenizer, tape and materializer.

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

/* Stubs (Task 2); Tasks 22 to 24 and 32 fill them.  */

#ifndef NH_WK_HARNESS
# include <config.h>
#endif

#ifdef HAVE_NIDHOGG_WORKERS

#include "nidhogg-workers.h"

struct nh_tape *
nh_json_tokenize (const unsigned char *bytes, size_t n, int mode,
		  const _Atomic bool *stop)
{
  (void) bytes;
  (void) n;
  (void) mode;
  (void) stop;
  return NULL;
}

bool
nh_json_materialize (struct nh_mat *mat, int64_t budget_ns)
{
  (void) mat;
  (void) budget_ns;
  return false;
}

#ifndef NH_WK_HARNESS

void
nh_json_syms (void)
{
}

#endif /* not NH_WK_HARNESS */

#endif /* HAVE_NIDHOGG_WORKERS */

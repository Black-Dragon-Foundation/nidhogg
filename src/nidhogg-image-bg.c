/* Nidhogg's worker threads: image headers and decoding (nh-img).

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

/* Stubs (Task 2); Tasks 27 to 30 fill them.  Nothing answers early:
   every caller keeps today's synchronous path.  */

#ifndef NH_WK_HARNESS
# include <config.h>
#endif

#ifdef HAVE_NIDHOGG_WORKERS

#include "nidhogg-workers.h"

bool
nh_img_lookup (struct frame *f, struct image *img, ptrdiff_t id)
{
  (void) f;
  (void) img;
  (void) id;
  return false;
}

bool
nh_img_sync_load (struct frame *f, struct image *img, int fmt)
{
  (void) f;
  (void) img;
  (void) fmt;
  return false;
}

#ifndef NH_WK_HARNESS

Lisp_Object
nh_img_size (Lisp_Object spec, Lisp_Object frame, bool pixels, bool *answered)
{
  (void) spec;
  (void) frame;
  (void) pixels;
  *answered = false;
  return Qnil;
}

void
nh_img_syms (void)
{
}

#endif /* not NH_WK_HARNESS */

#endif /* HAVE_NIDHOGG_WORKERS */

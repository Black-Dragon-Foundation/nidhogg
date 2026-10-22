/* Nidhogg's worker threads: tree-sitter parsing off the main thread.

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

/* Stubs (Task 2); Tasks 9 to 13 fill them.  */

#ifndef NH_WK_HARNESS
# include <config.h>
#endif

#ifdef HAVE_NIDHOGG_WORKERS

#include "nidhogg-workers.h"

void
nh_ts_alloc_install (void)
{
}

void
nh_ts_parser_freed (void *parser_c)
{
  (void) parser_c;
}

void
nh_ts_record_edit (void *parser_c, const void *ts_input_edit,
		   ptrdiff_t start_byte, ptrdiff_t old_end_byte,
		   ptrdiff_t new_end_byte)
{
  (void) parser_c;
  (void) ts_input_edit;
  (void) start_byte;
  (void) old_end_byte;
  (void) new_end_byte;
}

void
nh_ts_select_entry (void)
{
}

#ifndef NH_WK_HARNESS

bool
nh_ts_publish (Lisp_Object parser)
{
  (void) parser;
  return false;
}

void
nh_ts_maybe_start (Lisp_Object parser, int start_point)
{
  (void) parser;
  (void) start_point;
}

void
nh_ts_syms (void)
{
  DEFVAR_LISP ("nidhogg--redisplay-fontifying", Vnidhogg__redisplay_fontifying,
	       doc: /* The buffer a redisplay is fontifying, or nil.  */);
  Vnidhogg__redisplay_fontifying = Qnil;
}

#endif /* not NH_WK_HARNESS */

#endif /* HAVE_NIDHOGG_WORKERS */

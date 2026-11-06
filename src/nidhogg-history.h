/* The edit history's change hook: the guard every buffer-text change site calls.

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

/* README.md, "Edit history hook", is the description.  The rings are
   Glitnir's (nidhogg/glitnir/src/history.rs); nidhogg-history.c keeps
   the table of tracked texts and the Lisp interface.  */

#ifndef NIDHOGG_HISTORY_H
#define NIDHOGG_HISTORY_H

#ifdef HAVE_NIDHOGG_HISTORY

#include "lisp.h"
#include "buffer.h"

/* How many buffer texts are tracked.  While it is 0 a change site
   costs this one test and nothing else.  */
extern int nh_history_ntracked;

/* After a change to B's text: the text from START to OLD_END before the
   change is now the text from START to NEW_END, each position given as
   a character and as a byte.  Called right after the text changed,
   before any Lisp runs.  The sites pass both because some of them call
   it before the markers and point are moved, when a conversion between
   the two would read them.  */
extern void nh_history_change (struct buffer *b,
			       ptrdiff_t start, ptrdiff_t start_byte,
			       ptrdiff_t old_end, ptrdiff_t old_end_byte,
			       ptrdiff_t new_end, ptrdiff_t new_end_byte);

/* B's text was replaced as a whole (`buffer-swap-text',
   `set-buffer-multibyte'): its ring is marked for a resync.  */
extern void nh_history_resync (struct buffer *b);

/* B's text is about to be freed: forget it.  */
extern void nh_history_forget (struct buffer *b);

extern void syms_of_nidhogg_history (void);

/* The guard at the sites.  The positions are evaluated only when a
   text is tracked.  */
#define NH_HISTORY_CHANGE(b, start, start_byte, old_end, old_end_byte,	\
			  new_end, new_end_byte)			\
  do {									\
    if (nh_history_ntracked)						\
      nh_history_change (b, start, start_byte, old_end, old_end_byte,	\
			 new_end, new_end_byte);			\
  } while (false)

#define NH_HISTORY_RESYNC(b)			\
  do {						\
    if (nh_history_ntracked)			\
      nh_history_resync (b);			\
  } while (false)

#define NH_HISTORY_FORGET(b)			\
  do {						\
    if (nh_history_ntracked)			\
      nh_history_forget (b);			\
  } while (false)

#endif /* HAVE_NIDHOGG_HISTORY */

#endif /* NIDHOGG_HISTORY_H */

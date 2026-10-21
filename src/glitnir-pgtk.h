/* Glitnir (the compositor): the pgtk glue.  Header.

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

#ifndef GLITNIR_PGTK_H
#define GLITNIR_PGTK_H

#ifdef HAVE_GLITNIR

#include <stdint.h>
#include <gtk/gtk.h>
#include <cairo.h>

#include "lisp.h"
#include "frame.h"
#include "blockinput.h"
#include "glitnir.h"

/* Number of GLITNIR_CALLs since start.  */
extern uintmax_t glitnir_pgtk_calls;

/* Number of times a root's picture was copied for glitnir-snapshot.  */
extern uintmax_t glitnir_pgtk_snapshot_copies;

/* Every C->Rust call goes through this: input blocked around it (a draw may run
   atimers that would otherwise re-enter Emacs between two calls).  */
#define GLITNIR_CALL(expr)						\
  ({ glitnir_pgtk_calls++; GLITNIR_CALL_UNCOUNTED (expr); })

/* The same for the frame-time record, which the call counter leaves out (the
   record is made on every paint, also when Glitnir draws nothing).  */
#define GLITNIR_CALL_UNCOUNTED(expr)					\
  ({ block_input (); int32_t glitnir_rc_ = (expr); unblock_input ();	\
     glitnir_rc_; })

/* The id of F's root frame: monotonic, given at the first use.  */
extern uint64_t glitnir_pgtk_frame_id (struct frame *f);

/* Hooks called by pgtkterm.c.  */
extern void glitnir_pgtk_update_begin (struct frame *f);
extern void glitnir_pgtk_commit (struct frame *f);
extern bool glitnir_pgtk_draw_begin (struct frame *f, GtkWidget *widget,
				     cairo_t *cr);
extern void glitnir_pgtk_draw_end (struct frame *f);
extern void glitnir_pgtk_free_frame (struct frame *f);

/* Non-nil when Glitnir records F: `glitnir-enable', a pgtk frame that is
   not a tooltip, drawn on an ARGB32 image surface.  */
extern bool glitnir_pgtk_active_p (struct frame *f);

/* Damage, recorded by pgtk's drawing primitives.  Rectangles are in
   logical (frame) pixels; the glue turns them into device pixels with the
   scale of F's own surface, rounded outward, and batches them.  Nothing is
   recorded while F's cairo context is not its own (x-export-frames).  */
extern void glitnir_pgtk_damage (struct frame *f, int x, int y, int w, int h);
extern void glitnir_pgtk_damage_all (struct frame *f);
extern void glitnir_pgtk_scroll (struct frame *f, struct window *w, int x,
				 int from_y, int to_y, int width, int height);
extern void glitnir_pgtk_damage_glyphs (struct glyph_string *s);
extern void glitnir_pgtk_damage_cursor (struct window *w,
					struct glyph_row *row, bool on_p);
extern void glitnir_pgtk_damage_fringe (struct window *w,
					struct draw_fringe_bitmap_params *p);
extern void glitnir_pgtk_damage_internal_border (struct frame *f);

/* Take F's pending damage into OUT (at most NMAX rectangles, device pixels,
   clipped to F); return their number.  */
extern ptrdiff_t glitnir_pgtk_damage_take (struct frame *f, GlitnirRect *out,
					   ptrdiff_t nmax);

/* A new ARGB32 image surface with F's root frame as GTK last painted it
   (device pixels), made by arming the after-paint handler and waiting up to
   a second for a paint.  NULL with *ERR set to the error message.  */
extern cairo_surface_t *glitnir_pgtk_snapshot (struct frame *f,
					       const char **err);

#endif /* HAVE_GLITNIR */

#endif /* GLITNIR_PGTK_H */

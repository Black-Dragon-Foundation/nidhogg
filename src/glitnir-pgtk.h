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

/* True once a call into Glitnir failed (a panic, MP-21): Glitnir is off for
   the rest of the session and no further glitnir_* call is made.  */
extern bool glitnir_pgtk_dead;

/* A call WHAT returned the error RC: switch Glitnir off and make Nidhogg
   draw the stock way (see glitnir-pgtk.c).  */
extern void glitnir_pgtk_failed (const char *what, int32_t rc);

/* Every C->Rust call goes through this: input blocked around it (a draw may run
   atimers that would otherwise re-enter Emacs between two calls).  An error
   switches Glitnir off; once off, no call is made (GLITNIR_E_STATE).  */
#define GLITNIR_CALL(expr)						\
  ({ int32_t glitnir_rc_ = GLITNIR_E_STATE;				\
     if (!glitnir_pgtk_dead)						\
       {								\
	 glitnir_pgtk_calls++;						\
	 block_input (); glitnir_rc_ = (expr); unblock_input ();	\
	 if (glitnir_rc_ != GLITNIR_OK)					\
	   glitnir_pgtk_failed (#expr, glitnir_rc_);			\
       }								\
     glitnir_rc_; })

/* The same for the frame-time record, which the call counter leaves out (the
   record is made on every paint, also when Glitnir draws nothing).  Only a
   panic switches Glitnir off.  */
#define GLITNIR_CALL_UNCOUNTED(expr)					\
  ({ int32_t glitnir_rc_ = GLITNIR_E_STATE;				\
     if (!glitnir_pgtk_dead)						\
       {								\
	 block_input (); glitnir_rc_ = (expr); unblock_input ();	\
	 if (glitnir_rc_ == GLITNIR_E_PANIC)				\
	   glitnir_pgtk_failed (#expr, glitnir_rc_);			\
       }								\
     glitnir_rc_; })

/* Test primitive behind glitnir--debug-panic: the next call at WHERE (1 =
   commit, 2 = draw) panics; 0 disarms.  WHERE 3 turns Glitnir back on after
   a panic.  */
extern void glitnir_pgtk_debug_panic (int where);

/* The id of F's root frame: monotonic, given at the first use.  */
extern uint64_t glitnir_pgtk_frame_id (struct frame *f);

/* Hooks called by pgtkterm.c.  */
extern void glitnir_pgtk_update_begin (struct frame *f);
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

/* Consistency point WHICH (1-4) of F's root: no Emacs drawing is in
   progress on the root or its child frames.  1 = buffer flips unblocked
   (redisplay done, also after a non-local exit), 2 = frame up to date
   outside a flip block, 3 = flush_display, 4 = the end of a pgtk handler
   that drew, of pgtk_read_socket, or a scene change; 3 and 4 only outside
   redisplay and flip blocks.  When Glitnir draws the root, the root's
   damage (its child frames' included) is committed and invalidated on the
   root widget and the root (frozen since its first damage) thaws; it paints
   at once if a frame-clock cycle fell into a frozen span since its last
   paint, else on the frame clock.  The value is then true, and the caller
   skips pgtk's whole-widget invalidation.  Otherwise false: the stock
   present.  */
extern bool glitnir_pgtk_consistency_point (struct frame *f, int which);

/* Point WHICH for every root with something to commit.  */
extern void glitnir_pgtk_consistency_points (int which);

/* A pgtk hook changed F's scene outside Emacs drawing (move, resize, map,
   unmap, ...): F's root commits at the next point, at once outside
   redisplay.  */
extern void glitnir_pgtk_scene_changed (struct frame *f);

/* For tests; none calls into Glitnir.  */
extern uint64_t glitnir_pgtk_now_ns (void);
extern uintmax_t glitnir_pgtk_draw_count (struct frame *f);
extern void glitnir_pgtk_paint_counters (struct frame *f,
					 uintmax_t *paints_now,
					 uintmax_t *new_buffers);
extern bool glitnir_pgtk_frozen_p (struct frame *f);
extern void glitnir_pgtk_last_paint (struct frame *f, uint64_t *commit,
				     uint64_t *paint, uint64_t *present);
extern double glitnir_pgtk_max_paint_gap_ms (struct frame *f, bool reset);
extern bool glitnir_pgtk_motion (struct frame *f, int x, int y);

/* The debug modes `verify' (violations not yet in a frame-time entry, which
   RESET zeroes), and the test primitives behind glitnir--draw-undamaged and
   glitnir--compose-with-pending.  */
extern uint64_t glitnir_pgtk_violations (struct frame *f, bool reset);
extern bool glitnir_pgtk_draw_undamaged (struct frame *f);
extern bool glitnir_pgtk_compose_with_pending (struct frame *f);

/* A new ARGB32 image surface with F's root frame as GTK last painted it
   (device pixels), made by arming the after-paint handler and waiting up to
   a second for a paint.  NULL with *ERR set to the error message.  */
extern cairo_surface_t *glitnir_pgtk_snapshot (struct frame *f,
					       const char **err);

#endif /* HAVE_GLITNIR */

#endif /* GLITNIR_PGTK_H */

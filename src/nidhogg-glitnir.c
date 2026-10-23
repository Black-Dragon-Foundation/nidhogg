/* Lisp interface of Glitnir (the compositor), Nidhogg's Rust static library.

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

#ifdef HAVE_GLITNIR

#include <string.h>

#include "lisp.h"
#include "frame.h"
#ifdef HAVE_PGTK
#include "glitnir-pgtk.h"
#endif

DEFUN ("glitnir-active-p", Fglitnir_active_p, Sglitnir_active_p, 0, 1, 0,
       doc: /* Return non-nil if Glitnir (the compositor) draws FRAME.
FRAME defaults to the selected frame.  Only graphical frames of the pgtk
build that draw on an image surface can be drawn by Glitnir, and only
while `glitnir-enable' is non-nil; batch, terminal and tooltip frames
never are.  */)
  (Lisp_Object frame)
{
  struct frame *f = decode_live_frame (frame);
#ifdef HAVE_PGTK
  return glitnir_pgtk_active_p (f) ? Qt : Qnil;
#else
  (void) f;
  return Qnil;
#endif
}

DEFUN ("glitnir--damage", Fglitnir__damage, Sglitnir__damage, 0, 1, 0,
       doc: /* Take FRAME's pending damage and return it.
The value is a list of rectangles (X Y WIDTH HEIGHT) in device pixels,
what pgtk drew on FRAME since its damage was last taken; FRAME then has
none.  Without `glitnir-debug' `damage-hold', the next consistency point
takes a frame's damage to present it, so this sees only what was drawn
since.  FRAME defaults to the selected frame; the value is nil when
Glitnir does not draw FRAME.  */)
  (Lisp_Object frame)
{
  struct frame *f = decode_live_frame (frame);
#ifdef HAVE_PGTK
  GlitnirRect rects[64];
  ptrdiff_t n = glitnir_pgtk_damage_take (f, rects, ARRAYELTS (rects));
  Lisp_Object list = Qnil;
  while (n-- > 0)
    list = Fcons (list4i (rects[n].x, rects[n].y, rects[n].w, rects[n].h),
		  list);
  return list;
#else
  (void) f;
  return Qnil;
#endif
}

#ifdef HAVE_PGTK

static Lisp_Object
pctl (const char *key, const GlitnirPercentiles *p)
{
  return list2 (intern_c_string (key),
		listn (6, intern_c_string (":p50"), make_float (p->p50),
		       intern_c_string (":p95"), make_float (p->p95),
		       intern_c_string (":max"), make_float (p->max)));
}

#endif /* HAVE_PGTK */

DEFUN ("glitnir-stats", Fglitnir_stats, Sglitnir_stats, 0, 2, 0,
       doc: /* Return the frame-time record of FRAME's root frame, or nil.
The record covers the last 1024 paints (counts are since the last reset).
It is a plist (:count N :compose-ms (:p50 :p95 :max) :draw-ms (...)
:latency-ms (...) :freeze-ms (...) :damage-px N :painted-px N
:backfill-rate F :ticks N :late-ticks N :violations N :scale-mismatch N);
:damage-px and :painted-px are medians in device pixels.  Times are in
milliseconds; :latency-ms is the time from the commit of a redisplay to the
end of the paint that shows it, :freeze-ms the time the root was frozen
before that commit.  :backfill-rate is nil: GDK's back-fills cannot be
seen from the draw handler.  FRAME defaults to the selected frame; it
returns nil for a frame that is not a graphical pgtk frame.  Non-nil RESET
empties the record after reading it.  */)
  (Lisp_Object frame, Lisp_Object reset)
{
  struct frame *f = decode_live_frame (frame);
#ifdef HAVE_PGTK
  if (!FRAME_PGTK_P (f) || glitnir_pgtk_dead)
    return Qnil;
  GlitnirStatsSummary s;
  int32_t rc = GLITNIR_CALL_UNCOUNTED (glitnir_stats_summary
				       (glitnir_pgtk_frame_id (f),
					!NILP (reset), &s));
  if (rc != GLITNIR_OK)
    return Qnil;
  /* Violations not yet in an entry (the next paint records them).  */
  s.violations += glitnir_pgtk_violations (f, !NILP (reset));
  Lisp_Object plist
    = list2 (intern_c_string (":count"), make_uint (s.count));
  plist = nconc2 (plist, pctl (":compose-ms", &s.compose_ms));
  plist = nconc2 (plist, pctl (":draw-ms", &s.draw_ms));
  plist = nconc2 (plist, pctl (":latency-ms", &s.latency_ms));
  plist = nconc2 (plist, pctl (":freeze-ms", &s.freeze_ms));
  return nconc2 (plist,
		 listn (14, intern_c_string (":damage-px"),
			make_uint (s.damage_px_p50),
			intern_c_string (":painted-px"),
			make_uint (s.painted_px_p50),
			intern_c_string (":backfill-rate"),
			(s.backfill_rate < 0 ? Qnil
			 : make_float (s.backfill_rate)),
			intern_c_string (":ticks"), make_uint (s.ticks),
			intern_c_string (":late-ticks"),
			make_uint (s.late_ticks),
			intern_c_string (":violations"),
			make_uint (s.violations),
			intern_c_string (":scale-mismatch"),
			make_uint (s.scale_mismatch)));
#else
  (void) f;
  (void) reset;
  return Qnil;
#endif
}

#ifdef HAVE_PGTK

struct png_sink
{
  unsigned char *data;
  size_t len, cap;
};

static cairo_status_t
png_write (void *closure, const unsigned char *data, unsigned int length)
{
  struct png_sink *sink = closure;
  if (sink->len + length > sink->cap)
    {
      size_t cap = (sink->cap ? sink->cap : 65536);
      while (cap < sink->len + length)
	cap *= 2;
      sink->data = xrealloc (sink->data, cap);
      sink->cap = cap;
    }
  memcpy (sink->data + sink->len, data, length);
  sink->len += length;
  return CAIRO_STATUS_SUCCESS;
}

#endif /* HAVE_PGTK */

DEFUN ("glitnir-snapshot", Fglitnir_snapshot, Sglitnir_snapshot, 1, 2, 0,
       doc: /* Return the picture GTK last committed for FRAME's root frame.
FORMAT is `png' (the default; the value is a unibyte string holding a PNG)
or `raw' (the value is (WIDTH HEIGHT BYTES), BYTES a unibyte string of
ARGB32 pixels in the machine's byte order, device pixels, rows tightly
packed).  The picture is copied only when asked for: this call arms the
copy, requests a repaint, and waits up to one second for it.  */)
  (Lisp_Object frame, Lisp_Object format)
{
  struct frame *f = decode_live_frame (frame);
  if (!NILP (format) && !EQ (format, intern_c_string ("png"))
      && !EQ (format, intern_c_string ("raw")))
    error ("glitnir-snapshot: unknown format");
#ifdef HAVE_PGTK
  if (!FRAME_PGTK_P (f))
    error ("glitnir-snapshot: no image surface");
  const char *err = NULL;
  cairo_surface_t *s = glitnir_pgtk_snapshot (f, &err);
  if (s == NULL)
    error ("%s", err);
  int w = cairo_image_surface_get_width (s);
  int h = cairo_image_surface_get_height (s);
  Lisp_Object result;
  if (EQ (format, intern_c_string ("raw")))
    {
      /* ARGB32 rows are w * 4 bytes: no padding.  */
      result = list3 (make_fixnum (w), make_fixnum (h),
		      make_unibyte_string ((char *) cairo_image_surface_get_data (s),
					   (ptrdiff_t) w * 4 * h));
      cairo_surface_destroy (s);
    }
  else
    {
      struct png_sink sink = { NULL, 0, 0 };
      cairo_status_t st = cairo_surface_write_to_png_stream (s, png_write, &sink);
      cairo_surface_destroy (s);
      if (st != CAIRO_STATUS_SUCCESS)
	{
	  xfree (sink.data);
	  error ("glitnir-snapshot: cannot encode the picture");
	}
      result = make_unibyte_string ((char *) sink.data, sink.len);
      xfree (sink.data);
    }
  return result;
#else
  (void) f;
  error ("glitnir-snapshot: no image surface");
#endif
}

DEFUN ("glitnir--calls", Fglitnir__calls, Sglitnir__calls, 0, 0, 0,
       doc: /* Return the number of calls into Glitnir since start.
The frame-time record's calls are not counted.  */)
  (void)
{
#ifdef HAVE_PGTK
  return make_uint (glitnir_pgtk_calls);
#else
  return make_fixnum (0);
#endif
}

DEFUN ("glitnir--snapshot-copies", Fglitnir__snapshot_copies,
       Sglitnir__snapshot_copies, 0, 0, 0,
       doc: /* Return the number of pictures copied for `glitnir-snapshot' since start.  */)
  (void)
{
#ifdef HAVE_PGTK
  return make_uint (glitnir_pgtk_snapshot_copies);
#else
  return make_fixnum (0);
#endif
}

/* The consistency points' debug primitives (for tests).  None of them
   calls into Glitnir.  */

DEFUN ("glitnir--now-ns", Fglitnir__now_ns, Sglitnir__now_ns, 0, 0, 0,
       doc: /* Return the monotonic clock in nanoseconds, the clock of `glitnir--last-paint'.  */)
  (void)
{
#ifdef HAVE_PGTK
  return make_uint (glitnir_pgtk_now_ns ());
#else
  return make_fixnum (0);
#endif
}

DEFUN ("glitnir--draw-count", Fglitnir__draw_count, Sglitnir__draw_count,
       0, 1, 0,
       doc: /* Return the number of times GTK drew FRAME's root frame since start.
FRAME defaults to the selected frame.  */)
  (Lisp_Object frame)
{
  struct frame *f = decode_live_frame (frame);
#ifdef HAVE_PGTK
  return make_uint (glitnir_pgtk_draw_count (f));
#else
  (void) f;
  return make_fixnum (0);
#endif
}

DEFUN ("glitnir--paint-counters", Fglitnir__paint_counters,
       Sglitnir__paint_counters, 0, 1, 0,
       doc: /* Return counters of the paints of FRAME's root frame since start.
The value is (:paints-now N :new-buffers N): the consistency points that
painted at once, and the draws into a buffer GDK had not handed out
before (it allocates one when the compositor still holds the last, and
then copies every pixel the paint did not cover from that one).  FRAME
defaults to the selected frame.  */)
  (Lisp_Object frame)
{
  struct frame *f = decode_live_frame (frame);
  uintmax_t now = 0, buffers = 0;
#ifdef HAVE_PGTK
  glitnir_pgtk_paint_counters (f, &now, &buffers);
#else
  (void) f;
#endif
  return list4 (intern_c_string (":paints-now"), make_uint (now),
		intern_c_string (":new-buffers"), make_uint (buffers));
}

DEFUN ("glitnir--frozen-p", Fglitnir__frozen_p, Sglitnir__frozen_p, 0, 1, 0,
       doc: /* Return non-nil if FRAME's root frame is frozen.
A root is frozen from the first damage on it or its child frames until the
next consistency point: GTK paints nothing of it meanwhile.  FRAME
defaults to the selected frame.  */)
  (Lisp_Object frame)
{
  struct frame *f = decode_live_frame (frame);
#ifdef HAVE_PGTK
  return glitnir_pgtk_frozen_p (f) ? Qt : Qnil;
#else
  (void) f;
  return Qnil;
#endif
}

DEFUN ("glitnir--last-paint", Fglitnir__last_paint, Sglitnir__last_paint,
       0, 1, 0,
       doc: /* Return the times of the latest recorded paint of FRAME's root frame.
The value is (:commit-ns N :paint-ns N :present-ns N): the commit that
paint showed (0 if none), the end of its draw, and the frame clock's
after-paint that sent it to the compositor, on the clock of
`glitnir--now-ns'.  FRAME defaults to the selected frame.  */)
  (Lisp_Object frame)
{
  struct frame *f = decode_live_frame (frame);
  uint64_t commit = 0, paint = 0, present = 0;
#ifdef HAVE_PGTK
  glitnir_pgtk_last_paint (f, &commit, &paint, &present);
#else
  (void) f;
#endif
  return listn (6, intern_c_string (":commit-ns"), make_uint (commit),
		intern_c_string (":paint-ns"), make_uint (paint),
		intern_c_string (":present-ns"), make_uint (present));
}

DEFUN ("glitnir--max-paint-gap-ms", Fglitnir__max_paint_gap_ms,
       Sglitnir__max_paint_gap_ms, 0, 2, 0,
       doc: /* Return the longest time in ms between two paints of FRAME's root frame.
It covers the time since the last reset, the time since the latest paint
included.  Non-nil RESET starts a new span after reading.  FRAME defaults
to the selected frame.  */)
  (Lisp_Object frame, Lisp_Object reset)
{
  struct frame *f = decode_live_frame (frame);
#ifdef HAVE_PGTK
  return make_float (glitnir_pgtk_max_paint_gap_ms (f, !NILP (reset)));
#else
  (void) f;
  (void) reset;
  return make_float (0);
#endif
}

DEFUN ("glitnir--motion", Fglitnir__motion, Sglitnir__motion, 3, 3, 0,
       doc: /* Send a synthetic pointer motion to X, Y (logical pixels) of FRAME.
GTK handles it at once, as if the pointer had moved there.  Return non-nil
if it was sent.  */)
  (Lisp_Object frame, Lisp_Object x, Lisp_Object y)
{
  struct frame *f = decode_live_frame (frame);
  int ix = check_integer_range (x, INT_MIN, INT_MAX);
  int iy = check_integer_range (y, INT_MIN, INT_MAX);
#ifdef HAVE_PGTK
  return glitnir_pgtk_motion (f, ix, iy) ? Qt : Qnil;
#else
  (void) f;
  (void) ix;
  (void) iy;
  return Qnil;
#endif
}

DEFUN ("glitnir--draw-undamaged", Fglitnir__draw_undamaged,
       Sglitnir__draw_undamaged, 0, 1, 0,
       doc: /* Paint a rectangle on FRAME without recording its damage (for tests).
The mode `verify' reports it at FRAME's next commit.  FRAME defaults to the
selected frame.  Return non-nil if it was painted.  */)
  (Lisp_Object frame)
{
  struct frame *f = decode_live_frame (frame);
#ifdef HAVE_PGTK
  return glitnir_pgtk_draw_undamaged (f) ? Qt : Qnil;
#else
  (void) f;
  return Qnil;
#endif
}

DEFUN ("glitnir--compose-with-pending", Fglitnir__compose_with_pending,
       Sglitnir__compose_with_pending, 0, 1, 0,
       doc: /* Make GTK draw FRAME's root while it has uncommitted damage (for tests).
The root gets a pending rectangle, is thawed for one draw and frozen
again, and then reaches a consistency point.  The mode `verify' counts
the draw.  FRAME defaults to the selected frame.  Return non-nil if the
draw was forced.  */)
  (Lisp_Object frame)
{
  struct frame *f = decode_live_frame (frame);
#ifdef HAVE_PGTK
  return glitnir_pgtk_compose_with_pending (f) ? Qt : Qnil;
#else
  (void) f;
  return Qnil;
#endif
}

DEFUN ("glitnir--debug-freeze", Fglitnir__debug_freeze, Sglitnir__debug_freeze,
       0, 1, 0,
       doc: /* Freeze FRAME's root frame until its next consistency point (for tests).
The root freezes as at the first damage after a point, with nothing
drawn: GTK paints none of it until the next point commits and thaws it,
which outside redisplay comes when GLib next runs its idle callbacks.
Refused unless `glitnir-debug' is non-nil.  FRAME defaults to the
selected frame.  Return non-nil if the root is frozen.  */)
  (Lisp_Object frame)
{
  struct frame *f = decode_live_frame (frame);
  if (NILP (Vglitnir_debug))
    error ("glitnir--debug-freeze: `glitnir-debug' is nil");
#ifdef HAVE_PGTK
  return glitnir_pgtk_debug_freeze (f) ? Qt : Qnil;
#else
  (void) f;
  return Qnil;
#endif
}

DEFUN ("glitnir--debug-scale-mismatch", Fglitnir__debug_scale_mismatch,
       Sglitnir__debug_scale_mismatch, 0, 1, 0,
       doc: /* Make the next paint of FRAME's root frame see a scale mismatch (for tests).
Every draw of that paint finds the root's surface at another device scale
than GDK's buffer, as between a monitor's scale change and pgtk's next
look at it: the root and its child frames are drawn the stock way,
without chrome, and the paint counts in the `:scale-mismatch' of
`glitnir-stats'.  The whole root is queued for that paint.  Refused
unless `glitnir-debug' is non-nil.  FRAME defaults to the selected frame.
Return non-nil if the paint was armed.  */)
  (Lisp_Object frame)
{
  struct frame *f = decode_live_frame (frame);
  if (NILP (Vglitnir_debug))
    error ("glitnir--debug-scale-mismatch: `glitnir-debug' is nil");
#ifdef HAVE_PGTK
  return glitnir_pgtk_debug_scale_mismatch (f) ? Qt : Qnil;
#else
  (void) f;
  return Qnil;
#endif
}

DEFUN ("glitnir--stacking", Fglitnir__stacking, Sglitnir__stacking, 0, 1, 0,
       doc: /* Return the child frames of FRAME's root frame as Glitnir stacks them.
The list runs bottom to top, hidden frames included; a child frame's own
child frames come right after it.  FRAME defaults to the selected frame;
the value is nil when Glitnir does not draw its root.  */)
  (Lisp_Object frame)
{
  struct frame *f = decode_live_frame (frame);
#ifdef HAVE_PGTK
  struct frame *fs[256];
  ptrdiff_t n = glitnir_pgtk_stacking (f, fs, ARRAYELTS (fs));
  Lisp_Object list = Qnil;
  while (n-- > 0)
    {
      Lisp_Object c;
      XSETFRAME (c, fs[n]);
      list = Fcons (c, list);
    }
  return list;
#else
  (void) f;
  return Qnil;
#endif
}

DEFUN ("glitnir--layer-rect", Fglitnir__layer_rect, Sglitnir__layer_rect,
       0, 1, 0,
       doc: /* Return the rectangle of FRAME's layer in its root's scene, or nil.
The value is (X Y WIDTH HEIGHT) in the root's device pixels, as last
handed to Glitnir: a child frame at its position, with the size of its
last completely drawn picture.  FRAME defaults to the selected frame.  */)
  (Lisp_Object frame)
{
  struct frame *f = decode_live_frame (frame);
#ifdef HAVE_PGTK
  GlitnirRect r;
  if (!glitnir_pgtk_layer_rect (f, &r))
    return Qnil;
  return list4i (r.x, r.y, r.w, r.h);
#else
  (void) f;
  return Qnil;
#endif
}

DEFUN ("glitnir--chrome", Fglitnir__chrome, Sglitnir__chrome, 0, 1, 0,
       doc: /* Return the chrome Glitnir holds for FRAME's layer, or nil.
The value is (:radius R :shadow (:blur B :offset (X . Y) :spread S :opacity
O :color (R G B)) :border (:width W :color (R G B A))) in the layer's device
pixels (colour channels 0-255, the border's premultiplied).  FRAME defaults
to the selected frame.  */)
  (Lisp_Object frame)
{
  struct frame *f = decode_live_frame (frame);
#ifdef HAVE_PGTK
  GlitnirChrome c;
  if (!glitnir_pgtk_chrome (f, &c))
    return Qnil;
  Lisp_Object kw_radius = intern_c_string (":radius");
  Lisp_Object kw_shadow = intern_c_string (":shadow");
  Lisp_Object kw_border = intern_c_string (":border");
  Lisp_Object shadow
    = list (intern_c_string (":blur"), make_float (c.blur_px),
	    intern_c_string (":offset"),
	    Fcons (make_float (c.offset_x), make_float (c.offset_y)),
	    intern_c_string (":spread"), make_float (c.spread_px),
	    intern_c_string (":opacity"), make_float (c.shadow_opacity),
	    intern_c_string (":color"),
	    list3i ((c.shadow_argb >> 16) & 0xff, (c.shadow_argb >> 8) & 0xff,
		    c.shadow_argb & 0xff));
  Lisp_Object border
    = list (intern_c_string (":width"), make_float (c.border_px),
	    intern_c_string (":color"),
	    list4i ((c.border_argb >> 16) & 0xff, (c.border_argb >> 8) & 0xff,
		    c.border_argb & 0xff, c.border_argb >> 24));
  return list (kw_radius, make_float (c.radius_px), kw_shadow, shadow,
	       kw_border, border);
#else
  (void) f;
  return Qnil;
#endif
}

DEFUN ("glitnir--rechrome", Fglitnir__rechrome, Sglitnir__rechrome, 1, 1, 0,
       doc: /* Resolve FRAME's chrome again and hand it to Glitnir.
Return nil.  */)
  (Lisp_Object frame)
{
  struct frame *f = decode_live_frame (frame);
#ifdef HAVE_PGTK
  glitnir_pgtk_params_changed (f);
#else
  (void) f;
#endif
  return Qnil;
}

DEFUN ("glitnir--debug-panic", Fglitnir__debug_panic, Sglitnir__debug_panic,
       0, 1, 0,
       doc: /* Make the next call into Glitnir at WHERE panic (for tests).
WHERE is `commit' (the default; the next commit at a consistency point) or
`draw' (the next draw of a root frame).  The panic is caught at the library's
boundary and switches Glitnir off for the session: Nidhogg then draws the
stock way.  WHERE `revive' turns Glitnir back on after a panic, and nil
disarms.  Refused unless `glitnir-debug' is non-nil.  Return non-nil.  */)
  (Lisp_Object where)
{
  if (NILP (Vglitnir_debug))
    error ("glitnir--debug-panic: `glitnir-debug' is nil");
#ifdef HAVE_PGTK
  int w;
  if (NILP (where) || EQ (where, intern_c_string ("commit")))
    w = 1;
  else if (EQ (where, intern_c_string ("draw")))
    w = 2;
  else if (EQ (where, intern_c_string ("revive")))
    w = 3;
  else
    error ("glitnir--debug-panic: WHERE must be `commit' or `draw'");
  glitnir_pgtk_debug_panic (w);
  return Qt;
#else
  (void) where;
  return Qnil;
#endif
}

void
syms_of_nidhogg_glitnir (void)
{
  defsubr (&Sglitnir__debug_panic);
  defsubr (&Sglitnir__draw_undamaged);
  defsubr (&Sglitnir__compose_with_pending);
  defsubr (&Sglitnir_active_p);
  defsubr (&Sglitnir__damage);
  defsubr (&Sglitnir_stats);
  defsubr (&Sglitnir_snapshot);
  defsubr (&Sglitnir__calls);
  defsubr (&Sglitnir__snapshot_copies);
  defsubr (&Sglitnir__now_ns);
  defsubr (&Sglitnir__draw_count);
  defsubr (&Sglitnir__paint_counters);
  defsubr (&Sglitnir__frozen_p);
  defsubr (&Sglitnir__last_paint);
  defsubr (&Sglitnir__max_paint_gap_ms);
  defsubr (&Sglitnir__motion);
  defsubr (&Sglitnir__stacking);
  defsubr (&Sglitnir__debug_freeze);
  defsubr (&Sglitnir__debug_scale_mismatch);
  defsubr (&Sglitnir__layer_rect);
  defsubr (&Sglitnir__chrome);
  defsubr (&Sglitnir__rechrome);

  DEFVAR_BOOL ("glitnir-enable", glitnir_enable,
	       doc: /* Non-nil means Glitnir (the compositor) may draw graphical frames.
Setting it to nil makes Glitnir hand every frame back to the stock drawing.  */);
  glitnir_enable = true;

  DEFSYM (Qdamage_hold, "damage-hold");
  DEFSYM (Qglitnir__resolve, "glitnir--resolve");
  DEFSYM (Qglitnir_debug_damage, "glitnir-debug-damage");
  DEFVAR_LISP ("glitnir-debug", Vglitnir_debug,
	       doc: /* Glitnir's debug mode: nil (the default) or a symbol.
`damage' flashes every committed rectangle for 100 ms in the background of
the `glitnir-debug-damage' face.  `verify' keeps a copy of each frame's
surface as of the previous commit; at each commit, pixels that changed
outside the reported damage are logged once to stderr ("glitnir verify:
FRAME X Y W H after PRIMITIVE") and counted in the `:violations' of
`glitnir-stats', as are a paint while the root has uncommitted damage, a
freeze that lasts over a second, and a commit whose stacking of child
frames differs from GDK's.  `full' makes every commit damage the
whole frame.  `damage-hold' (for tests) keeps each frame's damage until
`glitnir--damage' takes it, instead of handing it to the next consistency
point; the points then present whole frames.  */);
  Vglitnir_debug = Qnil;

  Fprovide (intern_c_string ("glitnir"), Qnil);
}

#endif /* HAVE_GLITNIR */

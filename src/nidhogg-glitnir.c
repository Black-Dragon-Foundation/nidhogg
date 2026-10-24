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
build can be drawn by Glitnir; batch and terminal frames never are.  */)
  (Lisp_Object frame)
{
  struct frame *f = decode_live_frame (frame);
#ifdef HAVE_PGTK
  if (!FRAME_PGTK_P (f))
    return Qnil;
#else
  (void) f;
#endif
  /* Not drawn yet: the compositor takes over a root in a later change.  */
  return Qnil;
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
end of the paint that shows it.  FRAME defaults to the selected frame; it
returns nil for a frame that is not a graphical pgtk frame.  Non-nil RESET
empties the record after reading it.  */)
  (Lisp_Object frame, Lisp_Object reset)
{
  struct frame *f = decode_live_frame (frame);
#ifdef HAVE_PGTK
  if (!FRAME_PGTK_P (f))
    return Qnil;
  GlitnirStatsSummary s;
  int32_t rc = GLITNIR_CALL_UNCOUNTED (glitnir_stats_summary
				       (glitnir_pgtk_frame_id (f),
					!NILP (reset), &s));
  if (rc != GLITNIR_OK)
    return Qnil;
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
			make_float (s.backfill_rate),
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

void
syms_of_nidhogg_glitnir (void)
{
  defsubr (&Sglitnir_active_p);
  defsubr (&Sglitnir_stats);
  defsubr (&Sglitnir_snapshot);
  defsubr (&Sglitnir__calls);
  defsubr (&Sglitnir__snapshot_copies);

  DEFVAR_BOOL ("glitnir-enable", glitnir_enable,
	       doc: /* Non-nil means Glitnir (the compositor) may draw graphical frames.
Setting it to nil makes Glitnir hand every frame back to the stock drawing.  */);
  glitnir_enable = true;

  Fprovide (intern_c_string ("glitnir"), Qnil);
}

#endif /* HAVE_GLITNIR */

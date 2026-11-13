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

#include <stdlib.h>
#include <string.h>

#include "lisp.h"
#include "frame.h"
#ifdef HAVE_NHWL
#include "blockinput.h"
#include "dispextern.h"
#include "keyboard.h"
#include "nhwlterm.h"
#endif

/* True when the :box plist entry KEY VALUE is one of Glitnir's decoration keys
   (:corner-radius, :open, :label, :fit) with a valid value.  Built on every Glitnir
   build, so the other displays accept the keys (and ignore them).  */

bool
glitnir_box_key_ok (Lisp_Object key, Lisp_Object value)
{
  if (EQ (key, QCcorner_radius))
    return (FIXNUMP (value) ? XFIXNUM (value) >= 0
	    : FLOATP (value) && XFLOAT_DATA (value) >= 0);
  if (EQ (key, QCopen))
    {
      Lisp_Object tem = value;
      for (; CONSP (tem); tem = XCDR (tem))
	if (!(EQ (XCAR (tem), Qtop) || EQ (XCAR (tem), Qbottom)
	      || EQ (XCAR (tem), Qleft) || EQ (XCAR (tem), Qright)))
	  return false;
      return NILP (tem);
    }
  if (EQ (key, QClabel))
    return STRINGP (value);
  if (EQ (key, QCfit))
    return EQ (value, Qtext)
	   || (CONSP (value) && EQ (XCAR (value), Qtext)
	       && FIXNATP (XCDR (value)));
  return false;
}

#ifdef HAVE_NIDHOGG_WORKERS
/* glitnir-images.c: the image decoders on the image worker.  */
extern void syms_of_glitnir_images (void);
#endif
/* Sjá's Lisp door (nidhogg-views.c).  */
extern void syms_of_nidhogg_views (void);

DEFUN ("glitnir-active-p", Fglitnir_active_p, Sglitnir_active_p, 0, 1, 0,
       doc: /* Return non-nil if Glitnir (the compositor) draws FRAME.
FRAME defaults to the selected frame.  Frames of the native Wayland
shell can be drawn by Glitnir, and only while `glitnir-enable' is
non-nil; batch, terminal and tooltip frames never are.  */)
  (Lisp_Object frame)
{
  struct frame *f = decode_live_frame (frame);
#ifdef HAVE_NHWL
  return nhwl_glitnir_active_p (f) ? Qt : Qnil;
#else
  (void) f;
  return Qnil;
#endif
}

#ifdef HAVE_NHWL

static Lisp_Object
pctl (const char *key, const GlitnirPercentiles *p)
{
  return list2 (intern_c_string (key),
		listn (6, intern_c_string (":p50"), make_float (p->p50),
		       intern_c_string (":p95"), make_float (p->p95),
		       intern_c_string (":max"), make_float (p->max)));
}

#endif /* HAVE_NHWL */

DEFUN ("glitnir-stats", Fglitnir_stats, Sglitnir_stats, 0, 2, 0,
       doc: /* Return the frame-time record of FRAME's root frame, or nil.
The record covers the last 1024 paints (counts are since the last reset).
It is a plist (:count N :compose-ms (:p50 :p95 :max) :draw-ms (...)
:key-to-commit-ms (...) :damage-px N :painted-px N
:tick-interval-ms (...) :blur-ms (...) :leg-overrun-p95-ms F
:backfill-rate F :ticks N :late-ticks N :backdrop-px N
:leg-overrun-ms F);
:damage-px and :painted-px are medians in device pixels.  Times are in
milliseconds; :key-to-commit-ms is the time from a key event to the commit
of the buffer that shows it, over the paints that show a key only;
:draw-ms runs from the start of the draw to the buffer's commit, and
:painted-px is the area composed.  :tick-interval-ms is the time between
two commits of a transition, :blur-ms the time a recompute of the
backdrop blur took, each over the paints that had one,
:leg-overrun-ms how far the worst transition since the last reset ran
past its length, and :leg-overrun-p95-ms the 95th percentile of that
over the last 128 transitions.  :backfill-rate is nil.  FRAME defaults
to the selected frame; it returns nil for a frame that is not a
graphical nhwl frame.  Non-nil RESET empties the record after reading
it.  */)
  (Lisp_Object frame, Lisp_Object reset)
{
  struct frame *f = decode_live_frame (frame);
#ifdef HAVE_NHWL
  uint64_t id = 0;
  if (FRAME_NHWL_P (f))
    id = FRAME_NHWL_OUTPUT (f)->glitnir_id;
  if (!id)
    return Qnil;
  GlitnirStatsSummary s;
  int32_t rc = glitnir_stats_summary (id, !NILP (reset), &s);
  if (rc != GLITNIR_OK)
    return Qnil;
  Lisp_Object plist
    = list2 (intern_c_string (":count"), make_uint (s.count));
  plist = nconc2 (plist, pctl (":compose-ms", &s.compose_ms));
  plist = nconc2 (plist, pctl (":draw-ms", &s.draw_ms));
  plist = nconc2 (plist, pctl (":key-to-commit-ms", &s.update_ms));
  plist = nconc2 (plist, pctl (":tick-interval-ms", &s.tick_interval_ms));
  plist = nconc2 (plist, pctl (":blur-ms", &s.blur_ms));
  plist = nconc2 (plist, list2 (intern_c_string (":leg-overrun-p95-ms"),
				make_float (s.leg_overrun_p95_ms)));
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
			intern_c_string (":backdrop-px"),
			make_uint (s.backdrop_px),
			intern_c_string (":leg-overrun-ms"),
			make_float (s.leg_overrun_ms)));
#else
  (void) f;
  (void) reset;
  return Qnil;
#endif
}

#ifdef NHWL_WAYLAND

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

/* Root R's tree as the compositor was given it, or NULL: the glue composes
   what the tree published, so this costs presenting nothing.  */
static cairo_surface_t *
tree_picture (struct frame *r)
{
  const NhgSurface *list;
  NhgBacking b;
  cairo_surface_t *s = NULL;

  if (redisplaying_p || !FRAME_NHWL_OUTPUT (r)->cr_surface)
    return NULL;
  block_input ();
  ptrdiff_t n = nhwl_snapshot_list (r, &list);
  if (NHG (nhg_snapshot (FRAME_NHG (r), FRAME_NHWL_OUTPUT (r)->glitnir_id,
			 &b)) >= 0)
    {
      s = cairo_image_surface_create (CAIRO_FORMAT_ARGB32, b.w, b.h);
      cairo_surface_flush (s);
      if (NHG (nhg_tree_snapshot (FRAME_NHG (r), list, n, glitnir_enable,
				  cairo_image_surface_get_data (s),
				  cairo_image_surface_get_stride (s), b.w,
				  b.h)) >= 0)
	cairo_surface_mark_dirty (s);
      else
	{
	  cairo_surface_destroy (s);
	  s = NULL;
	}
    }
  unblock_input ();
  return s;
}
#endif /* NHWL_WAYLAND */

DEFUN ("glitnir-snapshot", Fglitnir_snapshot, Sglitnir_snapshot, 1, 2, 0,
       doc: /* Return the picture of FRAME's root frame.
The whole tree as the compositor was given it: the root, its visible child
frames and popups at their places and in their stacking, with their chrome,
shadows and alpha, inside the root's surface.  With `glitnir-enable' nil,
the frames' own pixels: no chrome, no shadows, no scene.
FORMAT is `png' (the default; the value is a unibyte string holding a PNG)
or `raw' (the value is (WIDTH HEIGHT BYTES), BYTES a unibyte string of
ARGB32 pixels in the machine's byte order, device pixels, rows tightly
packed).  */)
  (Lisp_Object frame, Lisp_Object format)
{
  struct frame *f = decode_live_frame (frame);
  if (!NILP (format) && !EQ (format, intern_c_string ("png"))
      && !EQ (format, intern_c_string ("raw")))
    error ("glitnir-snapshot: unknown format");
#ifdef NHWL_WAYLAND
  cairo_surface_t *s;
  if (!FRAME_NHWL_P (f))
    error ("glitnir-snapshot: no image surface");
  s = tree_picture (nhwl_tree_root (f));
  if (s == NULL)
    error ("glitnir-snapshot: the frame has no picture yet");
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

DEFUN ("glitnir--draw-count", Fglitnir__draw_count, Sglitnir__draw_count,
       0, 1, 0,
       doc: /* Return the commits of FRAME's root frame's tree that showed something.
That is a buffer, a child frame's buffer or a child frame's new place.
FRAME defaults to the selected frame.  */)
  (Lisp_Object frame)
{
  struct frame *f = decode_live_frame (frame);
#ifdef HAVE_NHWL
  uint64_t n = 0;
  if (FRAME_NHWL_P (f)
      && NHG (nhg_draw_count (FRAME_NHG (f),
			      nhwl_frame_window (nhwl_tree_root (f)), &n)) < 0)
    n = 0;
  return make_uint (n);
#else
  (void) f;
  return make_fixnum (0);
#endif
}

DEFUN ("glitnir--now-ns", Fglitnir__now_ns, Sglitnir__now_ns, 0, 0, 0,
       doc: /* Return the monotonic clock in nanoseconds, the clock of `glitnir--last-paint'.  */)
  (void)
{
#ifdef HAVE_NHWL
  return make_uint (nhwl_now_ns ());
#else
  return make_fixnum (0);
#endif
}

DEFUN ("glitnir--layout-leg", Fglitnir__layout_leg, Sglitnir__layout_leg,
       2, 2, 0,
       doc: /* Hand WINDOW's layout leg VECTOR to the shell.
VECTOR is what `glitnir--leg-vector' gives, or nil for none.  Return the
number of the newest capture the presenter took of WINDOW, 0 for none,
or nil when its frame has no layout legs (another display, a frame that
is not a top-level, a shell without them).  */)
  (Lisp_Object window, Lisp_Object vector)
{
  struct window *w = decode_live_window (window);
#ifdef HAVE_NHWL
  struct frame *f = XFRAME (w->frame);
  if (!FRAME_NHWL_P (f) || !FRAME_NHWL_OUTPUT (f)->glitnir_id)
    return Qnil;
  ptrdiff_t n = VECTORP (vector) ? ASIZE (vector) : 0;
  float *v;
  USE_SAFE_ALLOCA;
  SAFE_NALLOCA (v, 1, n);
  for (ptrdiff_t i = 0; i < n; i++)
    v[i] = NUMBERP (AREF (vector, i)) ? XFLOATINT (AREF (vector, i)) : 0;
  uint64_t captured = 0;
  block_input ();
  int32_t rc = NHG (nhg_layout_leg (FRAME_NHG (f), nhwl_frame_window (f),
				    FRAME_NHWL_OUTPUT (f)->glitnir_id,
				    w->sequence_number, v, n, &captured));
  unblock_input ();
  SAFE_FREE ();
  return rc < 0 ? Qnil : make_uint (captured);
#else
  (void) w;
  (void) vector;
  return Qnil;
#endif
}

DEFUN ("glitnir--last-paint", Fglitnir__last_paint, Sglitnir__last_paint,
       0, 1, 0,
       doc: /* Return the times of the newest recorded paint of FRAME's root frame, or nil.
The value is (:commit-ns N :paint-ns N :present-ns N): when Emacs
published the drawing that paint shows, when the paint ended, and when
its buffer was committed to the compositor (on the shell the paint ends
with that commit, so the last two are equal), on the clock of
`glitnir--now-ns'.  The paint is the newest in `glitnir-stats''s record,
and a reset of the record keeps it.  FRAME defaults to the selected
frame; the value is nil for a frame Glitnir does not record (batch,
terminal, a frame before its first paint).  */)
  (Lisp_Object frame)
{
  struct frame *f = decode_live_frame (frame);
#ifdef HAVE_NHWL
  uint64_t id = 0;
  if (FRAME_NHWL_P (f))
    id = FRAME_NHWL_OUTPUT (f)->glitnir_id;
  GlitnirStatsEntry e;
  if (!id || glitnir_stats_last (id, &e) != GLITNIR_OK)
    return Qnil;
  return listn (6, intern_c_string (":commit-ns"), make_uint (e.t_commit_ns),
		intern_c_string (":paint-ns"), make_uint (e.t_draw_end_ns),
		intern_c_string (":present-ns"),
		make_uint (e.t_draw_end_ns));
#else
  (void) f;
  return Qnil;
#endif
}

DEFUN ("glitnir--stacking", Fglitnir__stacking, Sglitnir__stacking, 0, 1, 0,
       doc: /* Return the child frames of FRAME's root frame as they are stacked.
The list runs bottom to top, hidden frames included; a child frame's own
child frames come right after it.  FRAME defaults to the selected frame;
the value is nil when Glitnir does not draw its root.  */)
  (Lisp_Object frame)
{
  struct frame *f = decode_live_frame (frame);
#ifdef HAVE_NHWL
  uint64_t w[256];
  int n = (FRAME_NHWL_P (f)
	   ? NHG (nhg_stacking (FRAME_NHG (f),
				nhwl_frame_window (nhwl_tree_root (f)),
				w, ARRAYELTS (w)))
	   : 0);
  Lisp_Object list = Qnil;
  while (n-- > 0)
    {
      struct frame *k = nhwl_window_frame (w[n]);
      if (k)
	{
	  Lisp_Object c;
	  XSETFRAME (c, k);
	  list = Fcons (c, list);
	}
    }
  return list;
#else
  (void) f;
  return Qnil;
#endif
}

DEFUN ("glitnir--chrome", Fglitnir__chrome, Sglitnir__chrome, 0, 1, 0,
       doc: /* Return the chrome the shell holds for FRAME, or nil.
The value is (:radius R :shadow (:blur B :offset (X . Y) :spread S :opacity
O :color (R G B))) in device pixels (colour channels 0-255).  FRAME defaults
to the selected frame.  */)
  (Lisp_Object frame)
{
  struct frame *f = decode_live_frame (frame);
#ifdef HAVE_NHWL
  /* The border is Emacs's own drawing, not chrome.  */
  if (!FRAME_NHWL_P (f) || FRAME_NHWL_OUTPUT (f)->chrome[NHG_CHROME_ON] == 0)
    return Qnil;
  /* The chrome vector C kept for the frame (`glitnir--chrome-vector').  */
  const float *c = FRAME_NHWL_OUTPUT (f)->chrome;
  double s = XFLOATINT (Fframe_scale_factor (frame));
  uint32_t rgb = (c[NHG_CHROME_RGB] > 0 && c[NHG_CHROME_RGB] <= 0xffffff
		  ? (uint32_t) c[NHG_CHROME_RGB] : 0);
  Lisp_Object shadow
    = (c[NHG_CHROME_OPACITY] <= 0 ? Qnil
       : list (intern_c_string (":blur"), make_float (s * c[NHG_CHROME_BLUR]),
	       intern_c_string (":offset"),
	       Fcons (make_float (s * c[NHG_CHROME_OX]),
		      make_float (s * c[NHG_CHROME_OY])),
	       intern_c_string (":spread"),
	       make_float (s * c[NHG_CHROME_SPREAD]),
	       intern_c_string (":opacity"), make_float (c[NHG_CHROME_OPACITY]),
	       intern_c_string (":color"),
	       list3i ((rgb >> 16) & 0xff, (rgb >> 8) & 0xff, rgb & 0xff)));
  return list4 (intern_c_string (":radius"),
		make_float (s * c[NHG_CHROME_RADIUS]),
		intern_c_string (":shadow"), shadow);
#else
  (void) f;
  return Qnil;
#endif
}

DEFUN ("glitnir--rechrome", Fglitnir__rechrome, Sglitnir__rechrome, 1, 1, 0,
       doc: /* Resolve FRAME's chrome again and hand it to the shell.
Return nil.  */)
  (Lisp_Object frame)
{
  struct frame *f = decode_live_frame (frame);
#ifdef HAVE_NHWL
  nhwl_child_params_changed (f);
#else
  (void) f;
#endif
  return Qnil;
}

DEFUN ("glitnir--probe", Fglitnir__probe, Sglitnir__probe, 3, 4, 0,
       doc: /* Measure a picture; KIND is `edges', `rule' or `seams'.
RAW is (WIDTH HEIGHT BYTES) as (glitnir-snapshot FRAME 'raw) returns it;
REGION is (X Y WIDTH HEIGHT) inside it.  ARG is the rule's orientation
(`vertical' or `horizontal') for `rule', and for `seams' the fill colour
as an integer 0xAARRGGBB, or (COLOUR . INSET) to leave INSET pixels at
the region's edges out.  Return a plist: (:spread :contrast :runs) for
`edges', (:width :peak :mass) for `rule', (:count) for `seams'.  */)
  (Lisp_Object kind, Lisp_Object raw, Lisp_Object region, Lisp_Object arg)
{
  if (!SYMBOLP (kind)
      || !(EQ (kind, intern_c_string ("edges"))
	   || EQ (kind, intern_c_string ("rule"))
	   || EQ (kind, intern_c_string ("seams"))))
    error ("glitnir--probe: KIND must be `edges', `rule' or `seams'");
  CHECK_CONS (raw);
  Lisp_Object lw = XCAR (raw), rest = XCDR (raw);
  CHECK_FIXNUM (lw);
  CHECK_CONS (rest);
  Lisp_Object lh = XCAR (rest), bytes = XCAR (XCDR (rest));
  CHECK_FIXNUM (lh);
  CHECK_STRING (bytes);
  EMACS_INT w = XFIXNUM (lw), h = XFIXNUM (lh);
  if (w <= 0 || h <= 0 || w > INT32_MAX / 4 || h > INT32_MAX / 4
      || w * h > INT32_MAX / 4)
    args_out_of_range (lw, lh);
  if (SBYTES (bytes) < 4 * w * h)
    args_out_of_range (bytes, make_fixnum (4 * w * h));
  CHECK_CONS (region);
  EMACS_INT rv[4];
  Lisp_Object tail = region;
  for (int i = 0; i < 4; i++)
    {
      CHECK_CONS (tail);
      CHECK_FIXNUM (XCAR (tail));
      rv[i] = XFIXNUM (XCAR (tail));
      tail = XCDR (tail);
    }
  if (rv[2] <= 0 || rv[3] <= 0 || rv[0] < 0 || rv[1] < 0
      || rv[0] + rv[2] > w || rv[1] + rv[3] > h)
    args_out_of_range (region, raw);
#ifdef HAVE_NHWL
  /* The shell links the crate: the probes are plain calls.  */
  GlitnirRect r = { rv[0], rv[1], rv[2], rv[3] };
  const uint8_t *px = (const uint8_t *) SDATA (bytes);
  int32_t rc;
  if (EQ (kind, intern_c_string ("edges")))
    {
      GlitnirEdgeStats s;
      rc = glitnir_probe_edges (px, 4 * w, w, h, r, &s);
      if (rc != GLITNIR_OK)
	error ("glitnir--probe: failed (%d)", rc);
      return listn (6, intern_c_string (":spread"), make_float (s.spread_px),
		    intern_c_string (":contrast"), make_float (s.contrast),
		    intern_c_string (":runs"), make_uint (s.runs));
    }
  if (EQ (kind, intern_c_string ("rule")))
    {
      bool vertical;
      if (EQ (arg, intern_c_string ("vertical")))
	vertical = true;
      else if (EQ (arg, intern_c_string ("horizontal")))
	vertical = false;
      else
	error ("glitnir--probe: ARG must be `vertical' or `horizontal'");
      GlitnirRuleStats s;
      rc = glitnir_probe_rule (px, 4 * w, w, h, r, vertical, &s);
      if (rc != GLITNIR_OK)
	error ("glitnir--probe: failed (%d)", rc);
      return listn (6, intern_c_string (":width"), make_float (s.width),
		    intern_c_string (":peak"), make_float (s.peak),
		    intern_c_string (":mass"), make_float (s.mass));
    }
  Lisp_Object colour = CONSP (arg) ? XCAR (arg) : arg;
  Lisp_Object inset = CONSP (arg) ? XCDR (arg) : make_fixnum (0);
  CHECK_FIXNUM (colour);
  CHECK_FIXNAT (inset);
  uint32_t count;
  rc = glitnir_probe_seams (px, 4 * w, w, h, r, (uint32_t) XFIXNUM (colour),
			    (uint32_t) XFIXNUM (inset), &count);
  if (rc != GLITNIR_OK)
    error ("glitnir--probe: failed (%d)", rc);
  return list2 (intern_c_string (":count"), make_uint (count));
#else
  (void) arg;
  error ("glitnir--probe: no Glitnir library");
#endif
}

#ifdef HAVE_NHWL
static int
glitnir_bench_cmp (const void *a, const void *b)
{
  uint64_t x = *(const uint64_t *) a, y = *(const uint64_t *) b;
  return x < y ? -1 : x > y;
}
#endif

DEFUN ("glitnir--decor-bench", Fglitnir__decor_bench, Sglitnir__decor_bench,
       1, 2, 0,
       doc: /* Time N full redraws of the selected frame; return per-string microseconds.
Each redraw garbages the frame and redisplays it at once.  STRINGS is the
number of decorated strings the redraw draws (default 1); the result is
\(:n N :p50-us A :p95-us B), the redraw times divided by STRINGS, in
microseconds.  For the decoration cost bench (PD R-D1).  */)
  (Lisp_Object n, Lisp_Object strings)
{
  CHECK_FIXNAT (n);
  EMACS_INT count = XFIXNUM (n);
  double per = NILP (strings) ? 1.0 : XFLOATINT (strings);
  if (count < 1 || count > 100000 || per < 1.0)
    error ("glitnir--decor-bench: N in 1..100000, STRINGS >= 1");
#ifdef HAVE_NHWL
  struct frame *f = SELECTED_FRAME ();
  uint64_t *t = xmalloc (count * sizeof *t);
  for (EMACS_INT i = 0; i < count; i++)
    {
      SET_FRAME_GARBAGED (f);
      uint64_t t0 = nhwl_now_ns ();
      Fredisplay (Qt);
      t[i] = nhwl_now_ns () - t0;
    }
  qsort (t, count, sizeof *t, glitnir_bench_cmp);
  double p50 = t[(count - 1) / 2] / 1000.0 / per;
  double p95 = t[(count - 1) * 95 / 100] / 1000.0 / per;
  xfree (t);
  return list (intern_c_string (":n"), make_fixnum (count),
	       intern_c_string (":p50-us"), make_float (p50),
	       intern_c_string (":p95-us"), make_float (p95));
#else
  return Qnil;
#endif
}

void
syms_of_nidhogg_glitnir (void)
{
  DEFSYM (QCcorner_radius, ":corner-radius");
  DEFSYM (QCopen, ":open");
  /* :fit text: the box hugs the text's height (the font's ascent and
     descent), not the row's, which `line-spacing' may make taller.  */
  DEFSYM (QCfit, ":fit");

  defsubr (&Sglitnir__probe);
  defsubr (&Sglitnir__decor_bench);
  defsubr (&Sglitnir_active_p);
  defsubr (&Sglitnir_stats);
  defsubr (&Sglitnir_snapshot);
  defsubr (&Sglitnir__draw_count);
  defsubr (&Sglitnir__now_ns);
  defsubr (&Sglitnir__last_paint);
  defsubr (&Sglitnir__layout_leg);
  defsubr (&Sglitnir__stacking);
  defsubr (&Sglitnir__chrome);
  defsubr (&Sglitnir__rechrome);

  DEFVAR_BOOL ("glitnir-enable", glitnir_enable,
	       doc: /* Non-nil means Glitnir (the compositor) decorates graphical frames.
Setting it to nil turns its decorations off.  */);
  glitnir_enable = true;

  DEFVAR_BOOL ("glitnir-confined-overlay-redisplay",
	       glitnir_confined_overlay_redisplay,
	       doc: /* Non-nil means an overlay change redisplays only its lines.
Stock Emacs redisplays a whole window after any overlay change in its
buffer.  With this, a change of an overlay that is not `invisible' or
`display' is redisplayed like a change of the overlay's text, and the
current line's number in its own face (`line-number-current-line')
needs a whole redisplay only when point's line changed.  */);
  glitnir_confined_overlay_redisplay = true;

  DEFVAR_INT ("glitnir--redisplay-changed-lines",
	      glitnir_redisplay_changed_lines,
	      doc: /* How many window redisplays updated only the changed lines.
Counts `try_window_id' successes; a test sets it to 0 and reads it.  */);
  glitnir_redisplay_changed_lines = 0;

  DEFVAR_INT ("glitnir--redisplay-same-start", glitnir_redisplay_same_start,
	      doc: /* How many window redisplays drew the window again from its start.
Counts the redisplays of a window from the same start as before (its
text, not just point, changed and only its lines could not be updated);
a test sets it to 0 and reads it.  */);
  glitnir_redisplay_same_start = 0;

  DEFVAR_INT ("glitnir--redisplay-gave-up", glitnir_redisplay_gave_up,
	      doc: /* Why the last try at redisplaying only a window's changed lines gave up.
The number of the reason in `try_window_id' (xdisp.c), for tests.  */);
  glitnir_redisplay_gave_up = 0;

  DEFSYM (Qglitnir__resolve, "glitnir--resolve");
  DEFSYM (Qglitnir__chrome_vector, "glitnir--chrome-vector");
  DEFSYM (Qglitnir__card, "glitnir--card");
  DEFSYM (Qglitnir__cards, "glitnir--cards");
  DEFSYM (Qglitnir__blocks, "glitnir--blocks");
  DEFSYM (Qglitnir__ruler, "glitnir--ruler");
  DEFSYM (Qglitnir__edges, "glitnir--edges");

#ifdef HAVE_NIDHOGG_WORKERS
  syms_of_glitnir_images ();
#endif
  syms_of_nidhogg_views ();
  Fprovide (intern_c_string ("glitnir"), Qnil);
  /* The overview ruler (window parameter glitnir-ruler): bars C lists
     beside the blocks.  */
  Fprovide (intern_c_string ("glitnir-ruler"), Qnil);
  /* An overlay change redisplays only its lines
     (`glitnir-confined-overlay-redisplay').  */
  Fprovide (intern_c_string ("glitnir-confined-overlays"), Qnil);
}

#endif /* HAVE_GLITNIR */

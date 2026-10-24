/* Glitnir (the compositor): the pgtk glue.

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
#include <time.h>

#include "lisp.h"
#include "frame.h"
#include "pgtkterm.h"
#include "process.h"
#include "glitnir-pgtk.h"

uintmax_t glitnir_pgtk_calls;
uintmax_t glitnir_pgtk_snapshot_copies;

/* What the glue keeps per root frame (FRAME_X_OUTPUT (f)->glitnir).  The
   stock path's whole picture is the widget's; the record is made by the
   toplevel frame clock's after-paint handler, one entry per paint.  */
struct glitnir_root
{
  uint64_t id;
  struct frame *frame;

  /* Monotonic ns of the last redisplay start, commit and draw (0: none since
     the last record).  */
  uint64_t t_update, t_commit, t_draw_start, t_draw_end;
  bool drawn;			/* a draw ran since the last record */

  /* What the last draw saw.  */
  uint64_t damage_px, painted_px;
  uint8_t backfill, scale_mismatch;
  const unsigned char *last_data;	/* compared, never dereferenced */
  int last_width, last_height;
  bool have_last;

  /* The widget's rectangle in the target, in device pixels.  */
  int crop_x, crop_y, crop_w, crop_h;

  /* The frame clock whose after-paint we are connected to.  */
  GdkFrameClock *clock;
  gulong after_paint_id;

  /* glitnir-snapshot: armed, the target to copy (referenced only while
     armed), and the copy.  */
  bool armed;
  cairo_surface_t *target;
  cairo_surface_t *snapshot;
  const char *snapshot_error;
};

static uint64_t
now_ns (void)
{
  struct timespec ts;
  clock_gettime (CLOCK_MONOTONIC, &ts);
  return (uint64_t) ts.tv_sec * 1000000000u + (uint64_t) ts.tv_nsec;
}

static struct frame *
glitnir_root_of (struct frame *f)
{
  while (FRAME_PARENT_FRAME (f))
    f = FRAME_PARENT_FRAME (f);
  return f;
}

/* The state of F's root, allocated on first use; NULL for a frame that is
   not a pgtk window.  */
static struct glitnir_root *
root_state (struct frame *f)
{
  static uint64_t last_id;
  f = glitnir_root_of (f);
  if (!FRAME_PGTK_P (f) || f->output_data.pgtk == NULL)
    return NULL;
  struct glitnir_root *r = FRAME_X_OUTPUT (f)->glitnir;
  if (r == NULL)
    {
      r = xzalloc (sizeof *r);
      r->id = ++last_id;
      r->frame = f;
      FRAME_X_OUTPUT (f)->glitnir = r;
    }
  return r;
}

uint64_t
glitnir_pgtk_frame_id (struct frame *f)
{
  struct glitnir_root *r = root_state (f);
  return r ? r->id : 0;
}

void
glitnir_pgtk_update_begin (struct frame *f)
{
  struct glitnir_root *r = root_state (f);
  if (r && r->t_update == 0)
    r->t_update = now_ns ();
}

void
glitnir_pgtk_commit (struct frame *f)
{
  struct glitnir_root *r = root_state (f);
  if (r)
    r->t_commit = now_ns ();
}

/* Copy the widget's rectangle of TARGET into a new ARGB32 surface.  */
static cairo_surface_t *
crop_target (struct glitnir_root *r, cairo_surface_t *target,
	     const char **err)
{
  if (cairo_surface_get_type (target) != CAIRO_SURFACE_TYPE_IMAGE)
    {
      *err = "glitnir-snapshot: no image surface";
      return NULL;
    }
  cairo_surface_flush (target);
  cairo_format_t fmt = cairo_image_surface_get_format (target);
  if (fmt != CAIRO_FORMAT_ARGB32 && fmt != CAIRO_FORMAT_RGB24)
    {
      *err = "glitnir-snapshot: no image surface";
      return NULL;
    }
  int tw = cairo_image_surface_get_width (target);
  int th = cairo_image_surface_get_height (target);
  int stride = cairo_image_surface_get_stride (target);
  const unsigned char *src = cairo_image_surface_get_data (target);
  cairo_surface_t *out
    = cairo_image_surface_create (CAIRO_FORMAT_ARGB32, r->crop_w, r->crop_h);
  if (cairo_surface_status (out) != CAIRO_STATUS_SUCCESS)
    {
      cairo_surface_destroy (out);
      *err = "glitnir-snapshot: no image surface";
      return NULL;
    }
  unsigned char *dst = cairo_image_surface_get_data (out);
  int dstride = cairo_image_surface_get_stride (out);
  memset (dst, 0, (size_t) dstride * r->crop_h);
  for (int y = 0; y < r->crop_h; y++)
    {
      int sy = r->crop_y + y;
      if (sy < 0 || sy >= th)
	continue;
      for (int x = 0; x < r->crop_w; x++)
	{
	  int sx = r->crop_x + x;
	  if (sx < 0 || sx >= tw)
	    continue;
	  uint32_t px;
	  memcpy (&px, src + (size_t) sy * stride + (size_t) sx * 4, 4);
	  if (fmt == CAIRO_FORMAT_RGB24)
	    px |= 0xff000000u;
	  memcpy (dst + (size_t) y * dstride + (size_t) x * 4, &px, 4);
	}
    }
  cairo_surface_mark_dirty (out);
  return out;
}

/* The frame clock's after-paint, connected after GDK's own: the target now
   holds the complete picture GDK committed.  One frame-time entry per paint
   we drew.  */
static void
after_paint (GdkFrameClock *clock, gpointer data)
{
  struct glitnir_root *r = data;
  if (!r->drawn)
    return;
  r->drawn = false;

  if (r->armed && r->target)
    {
      const char *err = NULL;
      cairo_surface_t *s = crop_target (r, r->target, &err);
      if (r->snapshot)
	cairo_surface_destroy (r->snapshot);
      r->snapshot = s;
      r->snapshot_error = s ? NULL : err;
      if (s)
	glitnir_pgtk_snapshot_copies++;
      r->armed = false;
      cairo_surface_destroy (r->target);
      r->target = NULL;
    }

  GlitnirStatsEntry e;
  memset (&e, 0, sizeof e);
  e.t_update_ns = r->t_update;
  e.t_commit_ns = r->t_commit;
  e.t_draw_start_ns = r->t_draw_start;
  e.t_draw_end_ns = r->t_draw_end;
  e.clock_frame_us = gdk_frame_clock_get_frame_time (clock);
  e.presented_us = -1;
  e.damage_px = r->damage_px;
  e.painted_px = r->painted_px;
  e.backfill = r->backfill;
  e.scale_mismatch = r->scale_mismatch;
  r->t_update = r->t_commit = 0;
  GLITNIR_CALL_UNCOUNTED (glitnir_stats_record (r->id, &e));
}

/* The draw handler of F's widget begins.  Returns true when F is a root we
   record; input is then blocked until glitnir_pgtk_draw_end.  */
bool
glitnir_pgtk_draw_begin (struct frame *f, GtkWidget *widget, cairo_t *cr)
{
  if (FRAME_PARENT_FRAME (f) || f->tooltip)
    return false;
  struct glitnir_root *r = root_state (f);
  if (r == NULL)
    return false;
  block_input ();
  r->t_draw_start = now_ns ();

  cairo_surface_t *target = cairo_get_target (cr);
  double sx = 1, sy = 1;
  cairo_surface_get_device_scale (target, &sx, &sy);
  GtkAllocation alloc;
  gtk_widget_get_allocation (widget, &alloc);
  double scale_frame = gtk_widget_get_scale_factor (widget);
  r->scale_mismatch = sx != scale_frame;

  /* Stock paints the whole widget; the damage is what GDK asked for.  */
  r->painted_px = (uint64_t) ((double) alloc.width * sx)
		  * (uint64_t) ((double) alloc.height * sy);
  double x1, y1, x2, y2;
  cairo_clip_extents (cr, &x1, &y1, &x2, &y2);
  r->damage_px = (x2 > x1 && y2 > y1)
    ? (uint64_t) ((x2 - x1) * sx) * (uint64_t) ((y2 - y1) * sy) : 0;

  /* GDK switched buffers when the target's data, width or height changed.  */
  if (cairo_surface_get_type (target) == CAIRO_SURFACE_TYPE_IMAGE)
    {
      const unsigned char *data = cairo_image_surface_get_data (target);
      int w = cairo_image_surface_get_width (target);
      int h = cairo_image_surface_get_height (target);
      r->backfill = (r->have_last
		     && (data != r->last_data || w != r->last_width
			 || h != r->last_height));
      r->last_data = data;
      r->last_width = w;
      r->last_height = h;
      r->have_last = true;
    }
  else
    r->backfill = 2;

  if (r->armed)
    {
      double ox = 0, oy = 0, dx = 0, dy = 0;
      cairo_user_to_device (cr, &ox, &oy);
      cairo_surface_get_device_offset (target, &dx, &dy);
      r->crop_x = (int) (ox * sx - dx + 0.5);
      r->crop_y = (int) (oy * sy - dy + 0.5);
      r->crop_w = (int) (alloc.width * sx + 0.5);
      r->crop_h = (int) (alloc.height * sy + 0.5);
      if (r->target)
	cairo_surface_destroy (r->target);
      r->target = cairo_surface_reference (target);
    }

  if (r->clock == NULL)
    {
      GdkFrameClock *clock = gtk_widget_get_frame_clock (widget);
      if (clock)
	{
	  r->clock = g_object_ref (clock);
	  r->after_paint_id
	    = g_signal_connect_after (clock, "after-paint",
				      G_CALLBACK (after_paint), r);
	}
    }
  return true;
}

void
glitnir_pgtk_draw_end (struct frame *f)
{
  struct glitnir_root *r = root_state (f);
  if (r)
    {
      r->t_draw_end = now_ns ();
      r->drawn = true;
    }
  unblock_input ();
}

/* F is being deleted: forget its ring and release what we hold.  */
void
glitnir_pgtk_free_frame (struct frame *f)
{
  if (!FRAME_PGTK_P (f) || f->output_data.pgtk == NULL)
    return;
  struct glitnir_root *r = FRAME_X_OUTPUT (f)->glitnir;
  if (r == NULL)
    return;
  if (r->clock)
    {
      g_signal_handler_disconnect (r->clock, r->after_paint_id);
      g_object_unref (r->clock);
    }
  if (r->target)
    cairo_surface_destroy (r->target);
  if (r->snapshot)
    cairo_surface_destroy (r->snapshot);
  GLITNIR_CALL_UNCOUNTED (glitnir_stats_forget (r->id));
  FRAME_X_OUTPUT (f)->glitnir = NULL;
  xfree (r);
}

cairo_surface_t *
glitnir_pgtk_snapshot (struct frame *f, const char **err)
{
  struct glitnir_root *r = root_state (f);
  if (r == NULL)
    {
      *err = "glitnir-snapshot: no image surface";
      return NULL;
    }
  GtkWidget *widget = FRAME_GTK_WIDGET (r->frame);
  if (widget == NULL)
    {
      *err = "glitnir-snapshot: no paint within 1 s";
      return NULL;
    }

  if (r->snapshot)
    cairo_surface_destroy (r->snapshot);
  r->snapshot = NULL;
  r->snapshot_error = NULL;
  r->armed = true;
  gtk_widget_queue_draw_area (widget, 0, 0, 1, 1);

  /* The draw and after-paint handlers run from the GTK main loop, which
     the wait iterates.  */
  for (int i = 0; i < 100 && r->snapshot == NULL && r->snapshot_error == NULL;
       i++)
    wait_reading_process_output (0, 10 * 1000 * 1000, 0, false, Qnil, NULL, 0);

  /* The frame may have been deleted while we waited.  */
  if (!FRAME_LIVE_P (r->frame) || FRAME_X_OUTPUT (r->frame)->glitnir != r)
    {
      *err = "glitnir-snapshot: no paint within 1 s";
      return NULL;
    }
  r->armed = false;
  if (r->target)
    {
      cairo_surface_destroy (r->target);
      r->target = NULL;
    }
  cairo_surface_t *s = r->snapshot;
  r->snapshot = NULL;
  if (s == NULL)
    *err = r->snapshot_error ? r->snapshot_error
			     : "glitnir-snapshot: no paint within 1 s";
  return s;
}

#endif /* HAVE_GLITNIR */

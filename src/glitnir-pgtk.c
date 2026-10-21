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

#include <math.h>
#include <string.h>
#include <time.h>

#include "lisp.h"
#include "frame.h"
#include "window.h"
#include "pgtkterm.h"
#include "process.h"
#include "glitnir-pgtk.h"

uintmax_t glitnir_pgtk_calls;
uintmax_t glitnir_pgtk_snapshot_copies;

/* Damage rectangles a frame batches before one glitnir_damage_add_n.  */
enum { GLITNIR_PGTK_BATCH = 32 };

/* What the glue keeps per frame (FRAME_X_OUTPUT (f)->glitnir): the damage
   of every recorded frame and, for a root frame, its frame-time record.
   The stock path's whole picture is the widget's; the record is made by the
   toplevel frame clock's after-paint handler, one entry per paint.  */
struct glitnir_root
{
  uint64_t id;
  struct frame *frame;

  /* Damage in device pixels not yet handed to Glitnir, the scale of the
     frame's own surface when last seen, and what Glitnir holds: anything
     (PENDING), the whole frame (ALL), an entry to forget (KNOWN).  */
  GlitnirRect batch[GLITNIR_PGTK_BATCH];
  int nbatch;
  double sx, sy;
  bool image, pending, all, known;

  /* A root's consistency points: damage on any of its frames since the
     last point (DIRTY), a scene change to commit (SCENE_PENDING), the GDK
     impl window frozen until the next point (FROZEN, referenced while
     frozen), and the idle point queued for damage made outside redisplay
     (IDLE_ID).  */
  bool dirty, scene_pending;
  GdkWindow *frozen;
  guint idle_id;

  /* Monotonic ns of the first redisplay start, freeze and commit, and of
     the last draw, since the last record (0: none).  */
  uint64_t t_update, t_freeze, t_commit, t_draw_start, t_draw_end;
  bool drawn;			/* a draw ran since the last record */

  /* What the last draw saw; the device pixels committed since the last
     record.  */
  uint64_t damage_px, painted_px, commit_px;
  uint8_t scale_mismatch;

  /* For tests: the root's draws, its latest recorded paint, and the
     longest time between two of its draws since GAP_FROM.  */
  uintmax_t draws;
  uint64_t last_commit, last_paint;
  uint64_t gap_from, max_gap;

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

/* The state of F, allocated on first use; NULL for a frame that is not a
   pgtk window.  */
static struct glitnir_root *
frame_state (struct frame *f)
{
  static uint64_t last_id;
  if (!FRAME_PGTK_P (f) || f->output_data.pgtk == NULL)
    return NULL;
  struct glitnir_root *r = FRAME_X_OUTPUT (f)->glitnir;
  if (r == NULL)
    {
      r = xzalloc (sizeof *r);
      r->id = ++last_id;
      r->frame = f;
      r->sx = r->sy = 1;
      FRAME_X_OUTPUT (f)->glitnir = r;
    }
  return r;
}

/* The state of F's root.  */
static struct glitnir_root *
root_state (struct frame *f)
{
  return frame_state (glitnir_root_of (f));
}

/* F's state when Glitnir records F's damage, else NULL: `glitnir-enable',
   a pgtk frame that is not a tooltip, whose cairo context is its own and
   draws on an ARGB32 image surface (as GDK's Wayland surfaces are).
   x-export-frames swaps in an RGB24 image or a vector surface: nothing is
   recorded meanwhile.  Between a resize and the next draw F has no
   context; it is recorded if its last one was an image.  */
static struct glitnir_root *
recording (struct frame *f)
{
  if (!glitnir_enable || !FRAME_PGTK_P (f) || f->output_data.pgtk == NULL
      || FRAME_TOOLTIP_P (f))
    return NULL;
  cairo_t *cr = f->output_data.pgtk->cr_context;
  if (cr == NULL)
    {
      struct glitnir_root *r = FRAME_X_OUTPUT (f)->glitnir;
      return r && r->image ? r : NULL;
    }
  cairo_surface_t *s = cairo_get_target (cr);
  if (cairo_surface_get_type (s) != CAIRO_SURFACE_TYPE_IMAGE
      || cairo_image_surface_get_format (s) != CAIRO_FORMAT_ARGB32)
    return NULL;
  struct glitnir_root *r = frame_state (f);
  cairo_surface_get_device_scale (s, &r->sx, &r->sy);
  r->image = true;
  return r;
}

bool
glitnir_pgtk_active_p (struct frame *f)
{
  return recording (f) != NULL;
}

/* Freeze ROOT's GDK impl window (every client-side child window shares
   it): GTK paints nothing of the root until the next consistency point
   thaws it (MP-22), so a paint never shows a half-drawn picture.  */
static void
freeze (struct glitnir_root *root)
{
  if (root->frozen)
    return;
  GtkWidget *w = FRAME_GTK_WIDGET (root->frame);
  GdkWindow *win = w ? gtk_widget_get_window (w) : NULL;
  if (win == NULL)
    return;
  root->frozen = g_object_ref (gdk_window_get_toplevel (win));
  if (root->t_freeze == 0)
    root->t_freeze = now_ns ();
  gdk_window_freeze_updates (root->frozen);
}

static void
thaw (struct glitnir_root *root)
{
  if (root->frozen == NULL)
    return;
  GdkWindow *win = root->frozen;
  root->frozen = NULL;
  gdk_window_thaw_updates (win);
  g_object_unref (win);
}

static gboolean
idle_point (gpointer data)
{
  struct glitnir_root *root = data;
  root->idle_id = 0;
  glitnir_pgtk_consistency_point (root->frame, 4);
  return G_SOURCE_REMOVE;
}

/* F recorded damage: its root has something to commit, and stays frozen
   until it does.  Drawing outside redisplay (an atimer, a timer, a GTK
   handler without a point of its own) gets a point when GLib next
   dispatches: a missed point costs latency, never a stuck display.  */
static void
mark_dirty (struct frame *f)
{
  struct glitnir_root *root = root_state (f);
  if (root == NULL)
    return;
  if (!root->dirty)
    {
      root->dirty = true;
      freeze (root);
    }
  if (root->idle_id == 0 && !redisplaying_p && !buffer_flipping_blocked_p ())
    root->idle_id = g_idle_add (idle_point, root);
}

/* The state changes before each call: the unblock_input that ends a call
   may run an atimer (the visible bell's) that records damage itself.  */

/* Hand F's batched rectangles to Glitnir.  */
static void
flush_damage (struct glitnir_root *r)
{
  int n = r->nbatch;
  if (n == 0)
    return;
  r->nbatch = 0;
  r->pending = r->known = true;
  GLITNIR_CALL (glitnir_damage_add_n (r->id, r->batch, n));
}

static void
damage_all (struct glitnir_root *r)
{
  mark_dirty (r->frame);
  if (r->all)
    return;
  r->nbatch = 0;		/* the whole frame holds them */
  r->pending = r->all = r->known = true;
  GLITNIR_CALL (glitnir_damage_all (r->id));
}

/* The device-pixel rectangle of F's logical X, Y, W, H: clipped to F,
   scaled by F's surface and rounded outward.  Empty when nothing is left.  */
static GlitnirRect
device_rect (struct frame *f, struct glitnir_root *r, int x, int y, int w,
	     int h)
{
  int x0 = max (x, 0), y0 = max (y, 0);
  int x1 = min (x + w, FRAME_PIXEL_WIDTH (f));
  int y1 = min (y + h, FRAME_PIXEL_HEIGHT (f));
  if (x1 <= x0 || y1 <= y0)
    return (GlitnirRect) { 0, 0, 0, 0 };
  int dx0 = floor (x0 * r->sx), dy0 = floor (y0 * r->sy);
  int dx1 = ceil (x1 * r->sx), dy1 = ceil (y1 * r->sy);
  return (GlitnirRect) { dx0, dy0, dx1 - dx0, dy1 - dy0 };
}

/* Add the device rectangle D to R's batch.  */
static void
batch_add (struct glitnir_root *r, GlitnirRect d)
{
  if (r->nbatch > 0)
    {
      /* A run of glyph strings on one row grows the last rectangle.  */
      GlitnirRect *l = &r->batch[r->nbatch - 1];
      if (l->y == d.y && l->h == d.h && d.x <= l->x + l->w
	  && d.x + d.w >= l->x)
	{
	  int right = max (l->x + l->w, d.x + d.w);
	  l->x = min (l->x, d.x);
	  l->w = right - l->x;
	  return;
	}
    }
  if (r->nbatch == GLITNIR_PGTK_BATCH)
    flush_damage (r);
  r->batch[r->nbatch++] = d;
}

void
glitnir_pgtk_damage (struct frame *f, int x, int y, int w, int h)
{
  struct glitnir_root *r = recording (f);
  if (r == NULL)
    return;
  /* No surface yet: its first picture is all new.  */
  if (r->all || f->output_data.pgtk->cr_context == NULL)
    {
      damage_all (r);
      return;
    }
  GlitnirRect d = device_rect (f, r, x, y, w, h);
  if (d.w <= 0 || d.h <= 0)
    return;
  mark_dirty (f);
  batch_add (r, d);
}

void
glitnir_pgtk_damage_all (struct frame *f)
{
  struct glitnir_root *r = recording (f);
  if (r)
    damage_all (r);
}

/* pgtk_scroll_run copied W's rows FROM_Y..FROM_Y+HEIGHT to TO_Y (logical
   pixels, X and WIDTH the window's box).  */
void
glitnir_pgtk_scroll (struct frame *f, struct window *w, int x, int from_y,
		     int to_y, int width, int height)
{
  struct glitnir_root *r = recording (f);
  if (r == NULL)
    return;
  if (r->all || f->output_data.pgtk->cr_context == NULL)
    {
      damage_all (r);
      return;
    }
  mark_dirty (f);
  int top = min (from_y, to_y), bottom = max (from_y, to_y) + height;
  GlitnirRect area = device_rect (f, r, x, top, width, bottom - top);
  /* The rows the copy uncovered.  */
  GlitnirRect exposed = (to_y < from_y
			 ? device_rect (f, r, x, to_y + height, width,
					from_y - to_y)
			 : device_rect (f, r, x, from_y, width,
					to_y - from_y));
  r->pending = r->known = true;
  GLITNIR_CALL (glitnir_scroll (r->id, w->sequence_number, area,
				lround ((to_y - from_y) * r->sy), exposed));
}

/* A glyph string covers its row's clipping rectangle vertically and its
   glyphs, their overhangs and the backgrounds it draws for its successors
   horizontally (pgtk_draw_glyph_string).  */
void
glitnir_pgtk_damage_glyphs (struct glyph_string *s)
{
  if (!glitnir_enable)
    return;
  NativeRectangle clip;
  get_glyph_string_clip_rect (s, &clip);
  int x0 = s->x - max (s->left_overhang, 0);
  int x1 = (s->x + max (s->width, s->background_width)
	    + max (s->right_overhang, 0));
  if (s->next && s->right_overhang && !s->for_overlaps)
    {
      int width = 0;
      for (struct glyph_string *next = s->next;
	   next && width < s->right_overhang;
	   width += next->width, next = next->next)
	if (next->first_glyph->type != IMAGE_GLYPH)
	  x1 = max (x1, next->x + next->background_width);
    }
  if (s->extends_to_end_of_line_p)
    x1 = max (x1, clip.x + (int) clip.width);
  glitnir_pgtk_damage (s->f, x0, clip.y, x1 - x0, clip.height);
}

/* A hollow, bar or hbar cursor (a box cursor is a glyph string): at most
   the cursor glyph, clipped to its row.  */
void
glitnir_pgtk_damage_cursor (struct window *w, struct glyph_row *row,
			    bool on_p)
{
  if (!on_p || !glitnir_enable)
    return;
  struct glyph *g = get_phys_cursor_glyph (w);
  if (g == NULL)
    return;
  glitnir_pgtk_damage (XFRAME (w->frame),
		       WINDOW_TEXT_TO_FRAME_PIXEL_X (w, w->phys_cursor.x),
		       WINDOW_TO_FRAME_PIXEL_Y (w, max (0, row->y)),
		       max (g->pixel_width, w->phys_cursor_width) + 1,
		       row->visible_height);
}

void
glitnir_pgtk_damage_fringe (struct window *w,
			    struct draw_fringe_bitmap_params *p)
{
  struct frame *f = XFRAME (WINDOW_FRAME (w));
  if (p->bx >= 0 && !p->overlay_p)
    glitnir_pgtk_damage (f, p->bx, p->by, p->nx, p->ny);
  if (p->which)
    glitnir_pgtk_damage (f, p->x, p->y, p->wd, p->h);
}

/* The four strips pgtk_clear_under_internal_border fills.  */
void
glitnir_pgtk_damage_internal_border (struct frame *f)
{
  int border = FRAME_INTERNAL_BORDER_WIDTH (f);
  int width = FRAME_PIXEL_WIDTH (f), height = FRAME_PIXEL_HEIGHT (f);
  int margin = FRAME_TOP_MARGIN_HEIGHT (f);
  int bottom_margin = FRAME_BOTTOM_MARGIN_HEIGHT (f);
  glitnir_pgtk_damage (f, 0, margin, width, border);
  glitnir_pgtk_damage (f, 0, 0, border, height);
  glitnir_pgtk_damage (f, width - border, 0, border, height);
  glitnir_pgtk_damage (f, 0, height - bottom_margin - border, width, border);
}

ptrdiff_t
glitnir_pgtk_damage_take (struct frame *f, GlitnirRect *out, ptrdiff_t nmax)
{
  struct glitnir_root *r = recording (f);
  if (r == NULL)
    return 0;
  flush_damage (r);
  if (!r->pending)
    return 0;
  const GlitnirRect *p = NULL;
  size_t n = 0;
  r->pending = r->all = false;
  if (GLITNIR_CALL (glitnir_damage_pending (r->id, &p, &n)) != GLITNIR_OK)
    n = 0;
  int64_t fw = ceil (FRAME_PIXEL_WIDTH (f) * r->sx);
  int64_t fh = ceil (FRAME_PIXEL_HEIGHT (f) * r->sy);
  ptrdiff_t k = 0;
  for (size_t i = 0; i < n && k < nmax; i++)
    {
      /* Whole-frame damage is {0, 0, INT32_MAX, INT32_MAX}.  */
      int64_t x0 = max (p[i].x, 0), y0 = max (p[i].y, 0);
      int64_t x1 = min ((int64_t) p[i].x + p[i].w, fw);
      int64_t y1 = min ((int64_t) p[i].y + p[i].h, fh);
      if (x1 > x0 && y1 > y0)
	out[k++] = (GlitnirRect) { x0, y0, x1 - x0, y1 - y0 };
    }
  return k;
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

/* Move the damage of ROOT's child frames into ROOT's batch, in ROOT's
   device pixels (each child where GTK has placed its widget).  */
static void
gather_children (struct glitnir_root *root)
{
  GtkWidget *rw = FRAME_GTK_WIDGET (root->frame);
  Lisp_Object tail, frame;
  FOR_EACH_FRAME (tail, frame)
    {
      struct frame *c = XFRAME (frame);
      if (c == root->frame || !FRAME_PGTK_P (c) || c->output_data.pgtk == NULL)
	continue;
      struct glitnir_root *s = FRAME_X_OUTPUT (c)->glitnir;
      if (s == NULL || (!s->pending && s->nbatch == 0)
	  || glitnir_root_of (c) != root->frame)
	continue;
      flush_damage (s);
      const GlitnirRect *p = NULL;
      size_t n = 0;
      s->pending = s->all = false;
      if (GLITNIR_CALL (glitnir_damage_pending (s->id, &p, &n)) != GLITNIR_OK)
	n = 0;
      GtkWidget *cw = FRAME_GTK_WIDGET (c);
      if (n == 0 || cw == NULL || rw == NULL || !gtk_widget_get_mapped (cw))
	continue;		/* nothing shown */
      int x, y;
      if (!gtk_widget_translate_coordinates (cw, rw, 0, 0, &x, &y))
	{
	  damage_all (root);
	  continue;
	}
      int64_t cw_px = ceil (FRAME_PIXEL_WIDTH (c) * s->sx);
      int64_t ch_px = ceil (FRAME_PIXEL_HEIGHT (c) * s->sy);
      int64_t dx = lround (x * root->sx), dy = lround (y * root->sy);
      for (size_t i = 0; i < n && !root->all; i++)
	{
	  int64_t x0 = max (p[i].x, 0), y0 = max (p[i].y, 0);
	  int64_t x1 = min ((int64_t) p[i].x + p[i].w, cw_px);
	  int64_t y1 = min ((int64_t) p[i].y + p[i].h, ch_px);
	  if (x1 > x0 && y1 > y0)
	    batch_add (root, (GlitnirRect) { x0 + dx, y0 + dy,
					     x1 - x0, y1 - y0 });
	}
    }
}

/* Hand ROOT's damage to Glitnir and invalidate what it gives back on
   the root widget, in logical pixels rounded outward.  Return true if
   anything was invalidated.  */
static bool
present (struct glitnir_root *root)
{
  struct frame *f = root->frame;
  GtkWidget *w = FRAME_GTK_WIDGET (f);
  if (w == NULL)
    return false;
  int64_t fw = ceil (FRAME_PIXEL_WIDTH (f) * root->sx);
  int64_t fh = ceil (FRAME_PIXEL_HEIGHT (f) * root->sy);
  if (EQ (Vglitnir_debug, Qdamage_hold))
    {
      /* The damage stays for glitnir--damage: present the whole root.  */
      gtk_widget_queue_draw (w);
      root->commit_px += fw * fh;
      return true;
    }
  gather_children (root);
  flush_damage (root);
  if (!root->pending)
    return false;
  root->pending = root->all = false;
  GlitnirCommitOut out = { NULL, 0 };
  if (GLITNIR_CALL (glitnir_commit (root->id, &out)) != GLITNIR_OK)
    {
      gtk_widget_queue_draw (w);	/* until Task 23's fallback */
      root->commit_px += fw * fh;
      return true;
    }
  bool any = false;
  for (size_t i = 0; i < out.n; i++)
    {
      /* Whole-frame damage is {0, 0, INT32_MAX, INT32_MAX}.  */
      int64_t x0 = max (out.rects[i].x, 0), y0 = max (out.rects[i].y, 0);
      int64_t x1 = min ((int64_t) out.rects[i].x + out.rects[i].w, fw);
      int64_t y1 = min ((int64_t) out.rects[i].y + out.rects[i].h, fh);
      if (x1 <= x0 || y1 <= y0)
	continue;
      int lx0 = floor (x0 / root->sx), ly0 = floor (y0 / root->sy);
      int lx1 = ceil (x1 / root->sx), ly1 = ceil (y1 / root->sy);
      gtk_widget_queue_draw_area (w, lx0, ly0, lx1 - lx0, ly1 - ly0);
      root->commit_px += (x1 - x0) * (y1 - y0);
      any = true;
    }
  return any;
}

/* Paint ROOT now (the amended MP-22): the paint lands inside the point
   instead of waiting for a frame-clock paint that would fall into the
   next frozen span.  The commit to the compositor still goes out at
   GDK's after-paint.  */
static void
paint_now (struct glitnir_root *root)
{
  GtkWidget *w = FRAME_GTK_WIDGET (root->frame);
  GdkWindow *win = w ? gtk_widget_get_window (w) : NULL;
  if (win == NULL)
    return;
  G_GNUC_BEGIN_IGNORE_DEPRECATIONS
  gdk_window_process_updates (gdk_window_get_toplevel (win), TRUE);
  G_GNUC_END_IGNORE_DEPRECATIONS
}

bool
glitnir_pgtk_consistency_point (struct frame *f, int which)
{
  if (which >= 3 && (redisplaying_p || buffer_flipping_blocked_p ()))
    return false;
  if (f == NULL || !FRAME_PGTK_P (f) || f->output_data.pgtk == NULL)
    return false;
  struct frame *rf = glitnir_root_of (f);
  struct glitnir_root *root = frame_state (rf);
  if (root == NULL)
    return false;
  if (recording (rf) == NULL)
    {
      /* The stock present: pgtk invalidates the whole widget at points 1
	 and 2.  A root frozen before Glitnir was turned off thaws and
	 shows everything once.  No call into Glitnir.  */
      if (root->dirty && FRAME_GTK_WIDGET (rf))
	gtk_widget_queue_draw (FRAME_GTK_WIDGET (rf));
      root->dirty = root->scene_pending = false;
      thaw (root);
      if (which <= 2 && root->t_commit == 0)
	root->t_commit = now_ns ();
      return false;
    }
  if (!root->dirty && !root->scene_pending)
    return true;
  root->dirty = root->scene_pending = false;
  uint64_t now = now_ns ();
  bool painted = present (root);
  if (painted && root->t_commit == 0)
    root->t_commit = now;
  thaw (root);
  if (painted)
    paint_now (root);
  return true;
}

void
glitnir_pgtk_consistency_points (int which)
{
  if (redisplaying_p || buffer_flipping_blocked_p ())
    return;
  Lisp_Object tail, frame;
  FOR_EACH_FRAME (tail, frame)
    {
      struct frame *f = XFRAME (frame);
      if (!FRAME_PGTK_P (f) || f->output_data.pgtk == NULL
	  || FRAME_PARENT_FRAME (f))
	continue;
      struct glitnir_root *r = FRAME_X_OUTPUT (f)->glitnir;
      if (r && (r->dirty || r->scene_pending))
	glitnir_pgtk_consistency_point (f, which);
    }
}

void
glitnir_pgtk_scene_changed (struct frame *f)
{
  struct glitnir_root *root = root_state (f);
  if (root == NULL)
    return;
  root->scene_pending = true;
  glitnir_pgtk_consistency_point (f, 4);
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
  e.t_freeze_ns = r->t_freeze;
  e.t_draw_start_ns = r->t_draw_start;
  e.t_draw_end_ns = r->t_draw_end;
  e.clock_frame_us = gdk_frame_clock_get_frame_time (clock);
  e.presented_us = -1;
  e.damage_px = r->damage_px;
  e.painted_px = r->painted_px;
  /* GDK draws into a staging surface that persists across buffer swaps:
     a back-fill cannot be seen from here (R3, DP-9).  */
  e.backfill = 2;
  e.scale_mismatch = r->scale_mismatch;
  r->last_commit = r->t_commit;
  r->last_paint = r->t_draw_end;
  r->t_update = r->t_freeze = r->t_commit = 0;
  r->commit_px = 0;
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
  r->draws++;

  if (recording (f))
    {
      /* Damage-only present: the paint covers GDK's clip, the damage
	 the points committed.  */
      uint64_t px = 0;
      cairo_rectangle_list_t *l = cairo_copy_clip_rectangle_list (cr);
      if (l->status == CAIRO_STATUS_SUCCESS)
	for (int i = 0; i < l->num_rectangles; i++)
	  px += ((uint64_t) (l->rectangles[i].width * sx + 0.5)
		 * (uint64_t) (l->rectangles[i].height * sy + 0.5));
      else
	px = (uint64_t) ((double) alloc.width * sx)
	     * (uint64_t) ((double) alloc.height * sy);
      cairo_rectangle_list_destroy (l);
      r->painted_px = px;
      r->damage_px = r->commit_px;
    }
  else
    {
      /* Stock paints the whole widget; the damage is what GDK asked for.  */
      r->painted_px = (uint64_t) ((double) alloc.width * sx)
		      * (uint64_t) ((double) alloc.height * sy);
      double x1, y1, x2, y2;
      cairo_clip_extents (cr, &x1, &y1, &x2, &y2);
      r->damage_px = (x2 > x1 && y2 > y1)
	? (uint64_t) ((x2 - x1) * sx) * (uint64_t) ((y2 - y1) * sy) : 0;
    }

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
      uint64_t now = now_ns ();
      r->t_draw_end = now;
      r->drawn = true;
      if (r->gap_from)
	r->max_gap = max (r->max_gap, now - r->gap_from);
      r->gap_from = now;
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
  if (r->idle_id)
    g_source_remove (r->idle_id);
  thaw (r);
  GLITNIR_CALL_UNCOUNTED (glitnir_stats_forget (r->id));
  if (r->known)
    GLITNIR_CALL (glitnir_damage_forget (r->id));
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

  /* Present what was drawn first (a root frozen with damage would not
     paint).  */
  glitnir_pgtk_consistency_point (r->frame, 4);
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

/* For tests.  None of these calls into Glitnir.  */

uint64_t
glitnir_pgtk_now_ns (void)
{
  return now_ns ();
}

uintmax_t
glitnir_pgtk_draw_count (struct frame *f)
{
  struct glitnir_root *r = root_state (f);
  return r ? r->draws : 0;
}

bool
glitnir_pgtk_frozen_p (struct frame *f)
{
  struct glitnir_root *r = root_state (f);
  return r && r->frozen;
}

void
glitnir_pgtk_last_paint (struct frame *f, uint64_t *commit, uint64_t *paint)
{
  struct glitnir_root *r = root_state (f);
  *commit = r ? r->last_commit : 0;
  *paint = r ? r->last_paint : 0;
}

double
glitnir_pgtk_max_paint_gap_ms (struct frame *f, bool reset)
{
  struct glitnir_root *r = root_state (f);
  if (r == NULL)
    return 0;
  uint64_t now = now_ns ();
  uint64_t gap = max (r->max_gap, r->gap_from ? now - r->gap_from : 0);
  if (reset)
    {
      r->max_gap = 0;
      r->gap_from = now;
    }
  return gap / 1e6;
}

/* Put a synthetic pointer motion to logical X, Y of F's widget on GDK's
   queue; whichever loop runs GLib next dispatches it.  */
bool
glitnir_pgtk_motion (struct frame *f, int x, int y)
{
  GtkWidget *w = FRAME_PGTK_P (f) ? FRAME_GTK_WIDGET (f) : NULL;
  GdkWindow *win = w ? gtk_widget_get_window (w) : NULL;
  if (win == NULL)
    return false;
  GdkSeat *seat = gdk_display_get_default_seat (gdk_window_get_display (win));
  GdkDevice *dev = seat ? gdk_seat_get_pointer (seat) : NULL;
  if (dev == NULL)
    return false;
  GdkEvent *ev = gdk_event_new (GDK_MOTION_NOTIFY);
  ev->motion.window = g_object_ref (win);
  ev->motion.send_event = TRUE;
  ev->motion.time = GDK_CURRENT_TIME;
  ev->motion.x = ev->motion.x_root = x;
  ev->motion.y = ev->motion.y_root = y;
  gdk_event_set_device (ev, dev);
  gdk_event_put (ev);
  gdk_event_free (ev);
  return true;
}

#endif /* HAVE_GLITNIR */

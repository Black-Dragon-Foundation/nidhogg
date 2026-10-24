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

#include <inttypes.h>
#include <limits.h>
#include <math.h>
#include <stdio.h>
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
bool glitnir_pgtk_dead;

/* glitnir--debug-panic: the next call at this point panics.  */
enum { PANIC_NONE, PANIC_COMMIT, PANIC_DRAW };
static int panic_armed;

/* The message of the fallback, shown once the main loop is idle (never from
   inside a redisplay or a draw).  */
static char *failed_message;

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
     (IDLE_ID).  TICK_FROZEN: a frame-clock cycle ran while the root was
     frozen (it painted nothing of the root) since the root's last paint.  */
  bool dirty, scene_pending, tick_frozen;
  GdkWindow *frozen;
  guint idle_id;

  /* Monotonic ns of the first redisplay start, freeze and commit, and of
     the last draw, since the last record (0: none).  */
  uint64_t t_update, t_freeze, t_commit, t_draw_start, t_draw_end;
  bool drawn;			/* a draw ran since the last record */

  /* Summed over every draw since the last record (a point that paints at
     once can draw several times in one frame): the time spent drawing,
     what the draws were asked for and painted; and the device pixels
     committed.  */
  uint64_t draw_ns, damage_px, painted_px, commit_px;
  uint8_t scale_mismatch;

  /* glitnir--debug-scale-mismatch: every draw of the root's next paint
     sees its surface at another scale than the target's.  */
  bool debug_mismatch;

  /* For tests: the root's draws, its latest recorded paint (and when the
     after-paint that sent it to the compositor ran), and the longest time
     between two of its draws since GAP_FROM.  */
  uintmax_t draws;
  uint64_t last_commit, last_paint, last_present;

  /* For tests: the points that painted at once (PAINTS_NOW), and the draws
     into a target GDK had not handed us before (NEW_BUFFERS: GDK allocates
     a buffer when the compositor still holds the last one, then copies
     every pixel the paint left alone from it).  */
  uintmax_t paints_now, new_buffers;
  uint64_t gap_from, max_gap;

  /* Debug modes.  `verify': the frame's surface at its previous commit
     (VCOPY, VW x VH pixels of VSTRIDE bytes), and the violations found since
     the last record.  `damage': the rectangles flashed (FLASH, device
     pixels, in the colour FLASH_RGB) until the timeout FLASH_ID removes
     them; the draw handler's context while it runs (DRAW_CR).  FREEZE_AT:
     when the root froze (0: not frozen).  */
  unsigned char *vcopy;
  int vw, vh, vstride;
  uint64_t violations, freeze_at;
  GlitnirRect flash[GLITNIR_PGTK_BATCH];
  int nflash;
  double flash_rgb[3];
  guint flash_id;
  cairo_t *draw_cr;

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

  /* The scene.  A root: Glitnir holds a scene for it (SCENE), which the
     next sync rebuilds (SCENE_STALE: its hooks ran while Glitnir did not
     draw it, so the stacking may have moved without Glitnir).  Every
     frame: the root whose scene holds its layer (LAYER_ROOT, 0: none), the
     layer as last handed over (LAYER), and its windows' layers (WINS,
     NWINS of WINS_SIZE).  */
  bool scene, scene_stale;
  uint64_t layer_root;
  GlitnirLayerDesc layer;
  struct win_layer *wins;
  ptrdiff_t nwins, wins_size;

  /* A child frame's chrome (Task 33), logical pixels as the resolver gave
     it (HAS_CHROME; the shadow colour as 0xRRGGBB), and the border pgtk
     drew last (BORDER_W px of the premultiplied ARGB BORDER_ARGB).  The
     layer gets it at the next sync: CHROME_DIRTY (changed since the last
     one), or a new layer, or a new scale (CHROME_SCALE).  APPLIED is what
     Glitnir holds (CHROME_SET).  */
  bool has_chrome, chrome_dirty, chrome_set;
  float ch_radius, ch_blur, ch_ox, ch_oy, ch_spread, ch_opacity;
  uint32_t ch_rgb, border_argb;
  int border_w;
  double chrome_scale;
  GlitnirChrome applied;
};

/* A window's layer as last handed over.  */
struct win_layer
{
  uint64_t id;
  GlitnirRect rect;
  double scale;
  bool seen;
};

/* A window's layer id: its sequence_number, tagged so that it never equals
   a frame's id (ids are unique within a root, whatever their kind).  */
#define WINDOW_LAYER_ID(w) \
  (((uint64_t) 1 << 63) | (uint64_t) (w)->sequence_number)

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
  if (glitnir_pgtk_dead || !glitnir_enable || !FRAME_PGTK_P (f)
      || f->output_data.pgtk == NULL || FRAME_TOOLTIP_P (f))
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
  root->freeze_at = now_ns ();
  if (root->t_freeze == 0)
    root->t_freeze = root->freeze_at;
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

static void verify_drop (struct glitnir_root *r);

static gboolean
failed_message_show (gpointer data)
{
  char *text = failed_message;
  failed_message = NULL;
  if (text)
    {
      CALLN (Fmessage, build_string ("%s"), build_string (text));
      xfree (text);
    }
  return G_SOURCE_REMOVE;
}

/* A call into Glitnir failed (MP-21).  Nothing of Glitnir is trusted any
   more: every frozen root thaws; a draw that was running paints the stock
   picture before it ends, so GDK's buffer is never left half composed; both
   whole-widget invalidations come back and every frame is redrawn; and
   Glitnir is off for the session (no further glitnir_* call).  */
void
glitnir_pgtk_failed (const char *what, int32_t rc)
{
  if (glitnir_pgtk_dead)
    return;
  glitnir_pgtk_dead = true;
  panic_armed = PANIC_NONE;
  (void) rc;
  Lisp_Object tail, frame;
  FOR_EACH_FRAME (tail, frame)
    {
      struct frame *f = XFRAME (frame);
      if (!FRAME_PGTK_P (f) || f->output_data.pgtk == NULL)
	continue;
      struct glitnir_root *r = FRAME_X_OUTPUT (f)->glitnir;
      if (r == NULL)
	continue;
      thaw (r);
      r->dirty = r->scene_pending = false;
      r->nbatch = 0;
      r->pending = r->all = false;
      r->known = false;
      if (r->draw_cr)
	{
	  /* The stock draw, in this very draw.  */
	  cairo_surface_t *src = FRAME_X_OUTPUT (f)->cr_surface_visible_bell;
	  if (src == NULL && f->output_data.pgtk->cr_active != NULL)
	    src = cairo_get_target (f->output_data.pgtk->cr_active);
	  if (src != NULL)
	    {
	      cairo_set_source_surface (r->draw_cr, src, 0, 0);
	      cairo_paint (r->draw_cr);
	    }
	  r->draw_cr = NULL;
	}
      if (r->nflash)
	{
	  r->nflash = 0;
	  if (r->flash_id)
	    g_source_remove (r->flash_id);
	  r->flash_id = 0;
	}
      verify_drop (r);
      GtkWidget *w = FRAME_GTK_WIDGET (f);
      if (w)
	gtk_widget_queue_draw (w);
    }
  xfree (failed_message);
  failed_message = xmalloc (strlen (what) + 200);
  sprintf (failed_message, "Glitnir (the compositor) stopped after an internal "
	   "error (%s); Nidhogg draws the stock way for the rest of the session",
	   what);
  g_idle_add (failed_message_show, NULL);
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

/* The state changes before each call, so a call never sees it half made
   (a call no longer runs atimers: GLITNIR_UNBLOCK_INPUT).  */

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

/* The drawing primitive that recorded damage last, for the `verify' log.  */
static const char *last_prim = "none";

void
glitnir_pgtk_damage (struct frame *f, int x, int y, int w, int h)
{
  struct glitnir_root *r = recording (f);
  if (r == NULL)
    return;
  last_prim = "pgtk_clear_area";
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
    {
      last_prim = "damage_all";
      damage_all (r);
    }
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
  last_prim = "pgtk_scroll_run";
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
  GLITNIR_CALL (glitnir_scroll (r->id, WINDOW_LAYER_ID (w), area,
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
  last_prim = "pgtk_draw_glyph_string";
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
  last_prim = "pgtk_draw_window_cursor";
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
  last_prim = "pgtk_draw_fringe_bitmap";
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
  last_prim = "pgtk_clear_under_internal_border";
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

/* The debug mode `verify': at each commit, the pixels of R's frame surface
   that changed since the previous commit and lie outside the committed
   rectangles are a missed rectangle.  */

/* Whether `glitnir-debug' is the symbol NAME (by name: a DEFSYM of a name
   another file defines too, such as "full", is a different symbol).  */
static bool
debug_is (const char *name)
{
  return (SYMBOLP (Vglitnir_debug)
	  && strcmp (SSDATA (SYMBOL_NAME (Vglitnir_debug)), name) == 0);
}

static bool
verifying (void)
{
  return debug_is ("verify");
}

static void
verify_drop (struct glitnir_root *r)
{
  if (r->vcopy)
    {
      xfree (r->vcopy);
      r->vcopy = NULL;
    }
}

static bool
covered (const GlitnirRect *rects, size_t n, int x, int y)
{
  for (size_t i = 0; i < n; i++)
    if (x >= rects[i].x && y >= rects[i].y
	&& (int64_t) x < (int64_t) rects[i].x + rects[i].w
	&& (int64_t) y < (int64_t) rects[i].y + rects[i].h)
      return true;
  return false;
}

/* `verify': the end of a log line, the state of R's frame and its root
   (a surface of -1x-1: none yet).  */
static void
verify_state (struct glitnir_root *r)
{
  struct frame *f = r->frame;
  struct frame *p = FRAME_PARENT_FRAME (f);
  GtkWidget *w = FRAME_GTK_WIDGET (f);
  cairo_t *cr = f->output_data.pgtk->cr_context;
  cairo_surface_t *s = cr ? cairo_get_target (cr) : NULL;
  bool img = s && cairo_surface_get_type (s) == CAIRO_SURFACE_TYPE_IMAGE;
  struct glitnir_root *root = root_state (f);
  fprintf (stderr,
	   " [%s %s, frame %dx%d at %d,%d, surface %dx%d at %g%s, visible %d"
	   " mapped %d, redisplaying %d flips-blocked %d, root dirty %d"
	   " scene-pending %d frozen %d, t %"PRIu64" ms]\n",
	   p ? "child" : "root", STRINGP (f->name) ? SSDATA (f->name) : "?",
	   FRAME_PIXEL_WIDTH (f), FRAME_PIXEL_HEIGHT (f), f->left_pos,
	   f->top_pos, img ? cairo_image_surface_get_width (s) : -1,
	   img ? cairo_image_surface_get_height (s) : -1, r->sx,
	   f->output_data.pgtk->cr_active != cr ? " (flip pending)" : "",
	   FRAME_VISIBLE_P (f), w && gtk_widget_get_mapped (w), redisplaying_p,
	   buffer_flipping_blocked_p (), root->dirty, root->scene_pending,
	   root->frozen != NULL, now_ns () / 1000000);
}

/* Compare R's frame surface with its copy at the previous commit; RECTS,
   N are what this commit reports (device pixels).  Log and count one
   violation if pixels changed outside them, then keep a new copy.  */
static void
verify_frame (struct glitnir_root *r, const GlitnirRect *rects, size_t n)
{
  struct frame *f = r->frame;
  cairo_t *cr = (FRAME_PGTK_P (f) && f->output_data.pgtk)
		? f->output_data.pgtk->cr_context : NULL;
  cairo_surface_t *s = cr ? cairo_get_target (cr) : NULL;
  if (s == NULL || cairo_surface_get_type (s) != CAIRO_SURFACE_TYPE_IMAGE
      || cairo_image_surface_get_format (s) != CAIRO_FORMAT_ARGB32)
    {
      verify_drop (r);
      return;
    }
  cairo_surface_flush (s);
  int w = cairo_image_surface_get_width (s);
  int h = cairo_image_surface_get_height (s);
  int stride = cairo_image_surface_get_stride (s);
  const unsigned char *cur = cairo_image_surface_get_data (s);
  size_t bytes = (size_t) stride * h;
  if (r->vcopy == NULL || r->vw != w || r->vh != h || r->vstride != stride)
    {
      /* A new surface: all of it is new, nothing to compare.  */
      verify_drop (r);
      r->vcopy = xmalloc (bytes);
      memcpy (r->vcopy, cur, bytes);
      r->vw = w;
      r->vh = h;
      r->vstride = stride;
      return;
    }
  int bx0 = INT_MAX, by0 = INT_MAX, bx1 = -1, by1 = -1;
  uint64_t nbad = 0;
  uint32_t was = 0, now = 0;
  for (int y = 0; y < h; y++)
    {
      const unsigned char *a = cur + (size_t) y * stride;
      const unsigned char *b = r->vcopy + (size_t) y * stride;
      if (memcmp (a, b, (size_t) w * 4) == 0)
	continue;
      for (int x = 0; x < w; x++)
	if (memcmp (a + (size_t) x * 4, b + (size_t) x * 4, 4) != 0
	    && !covered (rects, n, x, y))
	  {
	    if (nbad++ == 0)
	      {
		memcpy (&now, a + (size_t) x * 4, 4);
		memcpy (&was, b + (size_t) x * 4, 4);
	      }
	    bx0 = min (bx0, x);
	    by0 = min (by0, y);
	    bx1 = max (bx1, x);
	    by1 = max (by1, y);
	  }
    }
  memcpy (r->vcopy, cur, bytes);
  if (bx1 >= 0)
    {
      r->violations++;
      fprintf (stderr, "glitnir verify: %"PRIu64" %d %d %d %d after %s",
	       r->id, bx0, by0, bx1 - bx0 + 1, by1 - by0 + 1, last_prim);
      fprintf (stderr, " {%"PRIu64" px, first %08"PRIx32" -> %08"PRIx32
	       ", %zu rects:", nbad, was, now, n);
      for (size_t i = 0; i < n && i < 6; i++)
	fprintf (stderr, " %d,%d %dx%d", rects[i].x, rects[i].y, rects[i].w,
		 rects[i].h);
      fprintf (stderr, "}");
      verify_state (r);
    }
}

/* The debug mode `damage': each commit's rectangles are painted over the
   frame in the background of the `glitnir-debug-damage' face for 100 ms
   (by the draw handler: the frame's surface never holds them).  */

static void
flash_color (struct glitnir_root *r)
{
  struct frame *f = r->frame;
  unsigned long pixel = 0xff0000;
  if (FRAME_FACE_CACHE (f))
    {
      int id = lookup_named_face (NULL, f, Qglitnir_debug_damage, false);
      struct face *face = FACE_FROM_ID_OR_NULL (f, id);
      if (face)
	pixel = face->background;
    }
  Emacs_Color c;
  c.pixel = pixel;
  pgtk_query_color (f, &c);
  r->flash_rgb[0] = c.red / 65535.0;
  r->flash_rgb[1] = c.green / 65535.0;
  r->flash_rgb[2] = c.blue / 65535.0;
}

/* Invalidate N of the rectangles RECTS (device pixels) on R's widget.  */
static void
queue_rects (struct glitnir_root *r, const GlitnirRect *rects, int n)
{
  GtkWidget *w = FRAME_GTK_WIDGET (r->frame);
  if (w == NULL)
    return;
  for (int i = 0; i < n; i++)
    gtk_widget_queue_draw_area (w, floor (rects[i].x / r->sx),
				floor (rects[i].y / r->sy),
				ceil (rects[i].w / r->sx) + 1,
				ceil (rects[i].h / r->sy) + 1);
}

static gboolean
flash_expire (gpointer data)
{
  struct glitnir_root *r = data;
  GlitnirRect old[GLITNIR_PGTK_BATCH];
  int n = r->nflash;
  memcpy (old, r->flash, sizeof old);
  r->nflash = 0;
  r->flash_id = 0;
  queue_rects (r, old, n);
  return G_SOURCE_REMOVE;
}

/* ROOT's flash is now ROOT->flash; the rectangles OLD (NOLD of them) of
   the previous one are repainted without it.  */
static void
flash_start (struct glitnir_root *r, const GlitnirRect *old, int nold)
{
  if (r->flash_id)
    g_source_remove (r->flash_id);
  r->flash_id = r->nflash ? g_timeout_add (100, flash_expire, r) : 0;
  queue_rects (r, old, nold);
}

static void
paint_flash (struct glitnir_root *r, cairo_t *cr)
{
  cairo_save (cr);
  cairo_set_source_rgba (cr, r->flash_rgb[0], r->flash_rgb[1],
			 r->flash_rgb[2], 0.5);
  for (int i = 0; i < r->nflash; i++)
    cairo_rectangle (cr, r->flash[i].x / r->sx, r->flash[i].y / r->sy,
		     r->flash[i].w / r->sx, r->flash[i].h / r->sy);
  cairo_fill (cr);
  cairo_restore (cr);
}

/* Hand the damage of ROOT's child frames to Glitnir under their own ids:
   the commit moves each to its child's layer.  `verify' first takes it to
   check the child's surface, then gives it back.  */
static void
gather_children (struct glitnir_root *root)
{
  Lisp_Object tail, frame;
  FOR_EACH_FRAME (tail, frame)
    {
      struct frame *c = XFRAME (frame);
      if (c == root->frame || !FRAME_PGTK_P (c) || c->output_data.pgtk == NULL)
	continue;
      struct glitnir_root *s = FRAME_X_OUTPUT (c)->glitnir;
      if (s == NULL || glitnir_root_of (c) != root->frame)
	continue;
      if (!s->pending && s->nbatch == 0)
	{
	  /* Nothing recorded: verify finds anything drawn all the same.  */
	  if (verifying ())
	    verify_frame (s, NULL, 0);
	  else
	    verify_drop (s);
	  continue;
	}
      flush_damage (s);
      s->pending = s->all = false;
      if (!verifying ())
	{
	  verify_drop (s);
	  continue;
	}
      const GlitnirRect *p = NULL;
      size_t n = 0;
      if (GLITNIR_CALL (glitnir_damage_pending (s->id, &p, &n)) != GLITNIR_OK)
	continue;
      verify_frame (s, p, n);
      GlitnirRect back[2 * GLITNIR_PGTK_BATCH];
      if (n > ARRAYELTS (back))
	{
	  s->known = true;
	  GLITNIR_CALL (glitnir_damage_all (s->id));
	  continue;
	}
      memcpy (back, p, n * sizeof *back);
      s->known = true;
      GLITNIR_CALL (glitnir_damage_add_n (s->id, back, n));
    }
}

/* The scene (Task 32): ROOT's frame layer, a window layer per live window,
   and a child node per child frame, nested as the frames are, in ROOT's
   device pixels.  A sync hands Glitnir what changed since the last one;
   the commit compares the scene with the committed one.  The stacking of
   the child nodes follows GDK's: a new node goes on top of its parent's
   (new frames are synced in GDK's order), and raise, lower, map and
   reparent move it as GDK moves the frame's window.  */

static struct frame *
frame_of_id (uint64_t id)
{
  Lisp_Object tail, frame;
  FOR_EACH_FRAME (tail, frame)
    {
      struct frame *f = XFRAME (frame);
      if (FRAME_PGTK_P (f) && f->output_data.pgtk
	  && FRAME_X_OUTPUT (f)->glitnir
	  && ((struct glitnir_root *) FRAME_X_OUTPUT (f)->glitnir)->id == id)
	return f;
    }
  return NULL;
}

/* F and the frames under it hold no layer any more.  */
static void
forget_layers (struct frame *f)
{
  Lisp_Object tail, frame;
  FOR_EACH_FRAME (tail, frame)
    {
      struct frame *c = XFRAME (frame);
      if (!FRAME_PGTK_P (c) || c->output_data.pgtk == NULL
	  || FRAME_X_OUTPUT (c)->glitnir == NULL)
	continue;
      struct frame *a = c;
      while (a && a != f)
	a = FRAME_PARENT_FRAME (a);
      if (a)
	{
	  struct glitnir_root *s = FRAME_X_OUTPUT (c)->glitnir;
	  s->layer_root = 0;
	  s->nwins = 0;
	}
    }
}

/* Where F's window sits among its siblings in GDK, bottom to top: higher
   is upper.  A frame without a window yet comes on top (GDK puts a new
   window there).  */
static int
gdk_height (struct frame *f)
{
  GtkWidget *w = FRAME_GTK_WIDGET (f);
  GdkWindow *win = w ? gtk_widget_get_window (w) : NULL;
  GdkWindow *parent = win ? gdk_window_get_parent (win) : NULL;
  int i = parent ? g_list_index (gdk_window_peek_children (parent), win) : -1;
  return i < 0 ? INT_MAX : -i;	/* GDK lists children top first */
}

static int
height_cmp (const void *a, const void *b)
{
  int x = gdk_height (*(struct frame *const *) a);
  int y = gdk_height (*(struct frame *const *) b);
  return (x > y) - (x < y);
}

static bool
desc_equal (const GlitnirLayerDesc *a, const GlitnirLayerDesc *b)
{
  return (a->id == b->id && a->parent == b->parent && a->kind == b->kind
	  && a->rect.x == b->rect.x && a->rect.y == b->rect.y
	  && a->rect.w == b->rect.w && a->rect.h == b->rect.h
	  && a->scale == b->scale && a->visible == b->visible);
}

/* The windows of the tree at WINDOW on S's frame: their layers under S's,
   at S's place OX, OY at SCALE.  */
static void
sync_windows (struct glitnir_root *root, struct glitnir_root *s,
	      Lisp_Object window, int ox, int oy, double scale)
{
  for (; WINDOWP (window) && !glitnir_pgtk_dead;
       window = XWINDOW (window)->next)
    {
      struct window *w = XWINDOW (window);
      if (WINDOWP (w->contents))
	{
	  sync_windows (root, s, w->contents, ox, oy, scale);
	  continue;
	}
      int x, y, wd, ht;
      window_box (w, ANY_AREA, &x, &y, &wd, &ht);
      int x0 = floor (x * scale), y0 = floor (y * scale);
      GlitnirRect r = { ox + x0, oy + y0, (int) ceil ((x + wd) * scale) - x0,
			(int) ceil ((y + ht) * scale) - y0 };
      uint64_t id = WINDOW_LAYER_ID (w);
      ptrdiff_t i = 0;
      while (i < s->nwins && s->wins[i].id != id)
	i++;
      if (i == s->nwins)
	{
	  if (s->nwins == s->wins_size)
	    s->wins = xpalloc (s->wins, &s->wins_size, 1, -1,
			       sizeof *s->wins);
	  s->nwins++;
	}
      else if (s->wins[i].rect.x == r.x && s->wins[i].rect.y == r.y
	       && s->wins[i].rect.w == r.w && s->wins[i].rect.h == r.h
	       && s->wins[i].scale == scale)
	{
	  s->wins[i].seen = true;
	  continue;
	}
      s->wins[i] = (struct win_layer) { id, r, scale, true };
      GlitnirLayerDesc d = { .id = id, .parent = s->id,
			     .kind = GLITNIR_LAYER_WINDOW, .rect = r,
			     .scale = scale, .visible = 1 };
      GLITNIR_CALL (glitnir_layer_upsert (root->id, &d));
    }
}

/* Hand S's chrome, in device pixels at SCALE, to its layer (FRESH: the
   layer was just made, so it holds none).  */
static void
chrome_apply (struct glitnir_root *root, struct glitnir_root *s,
	      double scale, bool fresh)
{
  s->chrome_dirty = false;
  s->chrome_scale = scale;
  if (!s->has_chrome)
    {
      if (s->chrome_set && !fresh)
	GLITNIR_CALL (glitnir_layer_set_chrome (root->id, s->id, NULL));
      s->chrome_set = false;
      return;
    }
  GlitnirChrome c = {
    .radius_px = s->ch_radius * scale,
    .blur_px = s->ch_blur * scale,
    .offset_x = s->ch_ox * scale,
    .offset_y = s->ch_oy * scale,
    .spread_px = s->ch_spread * scale,
    .shadow_opacity = s->ch_opacity,
    .shadow_argb = 0xff000000u | s->ch_rgb,
    .border_px = s->border_w * scale,
    .border_argb = s->border_argb,
  };
  s->applied = c;
  s->chrome_set = true;
  GLITNIR_CALL (glitnir_layer_set_chrome (root->id, s->id, &c));
}

/* F's layer (under PARENT, at X, Y), its windows' and, recursively, its
   child frames'.  */
static void
sync_frame (struct glitnir_root *root, struct frame *f, uint64_t parent,
	    int x, int y)
{
  struct glitnir_root *s = frame_state (f);
  if (s == NULL || glitnir_pgtk_dead)
    return;
  GlitnirLayerDesc d = { .id = s->id, .parent = parent, .visible = 1 };
  if (f == root->frame)
    {
      d.kind = GLITNIR_LAYER_FRAME;
      d.scale = root->sx;
      d.rect = (GlitnirRect) { 0, 0, ceil (FRAME_PIXEL_WIDTH (f) * root->sx),
			       ceil (FRAME_PIXEL_HEIGHT (f) * root->sy) };
    }
  else
    {
      /* At its position now, with the size of its last completely drawn
	 surface (pgtk keeps it active until a redrawn one flips).  */
      d.kind = GLITNIR_LAYER_CHILD;
      cairo_t *cr = f->output_data.pgtk->cr_active;
      cairo_surface_t *sf = cr ? cairo_get_target (cr) : NULL;
      double sx = s->sx, sy = s->sy;
      GtkWidget *w = FRAME_GTK_WIDGET (f);
      if (sf && cairo_surface_get_type (sf) == CAIRO_SURFACE_TYPE_IMAGE)
	{
	  cairo_surface_get_device_scale (sf, &sx, &sy);
	  d.rect = (GlitnirRect) { x, y, cairo_image_surface_get_width (sf),
				   cairo_image_surface_get_height (sf) };
	  d.visible = w && gtk_widget_get_mapped (w);
	}
      else
	{
	  /* Never drawn: nothing to show.  */
	  d.rect = (GlitnirRect) { x, y, ceil (FRAME_PIXEL_WIDTH (f) * sx),
				   ceil (FRAME_PIXEL_HEIGHT (f) * sy) };
	  d.visible = 0;
	}
      d.scale = sx;
    }
  bool fresh = s->layer_root != root->id;
  if (fresh)
    s->nwins = 0;
  if (fresh || !desc_equal (&d, &s->layer))
    {
      s->layer = d;
      s->layer_root = root->id;
      GLITNIR_CALL (glitnir_layer_upsert (root->id, &d));
    }
  if (f != root->frame
      && (fresh || s->chrome_dirty || s->chrome_scale != d.scale))
    chrome_apply (root, s, d.scale, fresh);

  for (ptrdiff_t i = 0; i < s->nwins; i++)
    s->wins[i].seen = false;
  sync_windows (root, s, f->root_window, x, y, d.scale);
  ptrdiff_t k = 0;
  for (ptrdiff_t i = 0; i < s->nwins; i++)
    if (s->wins[i].seen)
      s->wins[k++] = s->wins[i];
    else if (!glitnir_pgtk_dead)
      GLITNIR_CALL (glitnir_layer_remove (root->id, s->wins[i].id));
  s->nwins = k;

  /* The child frames, bottom to top as GDK stacks them.  */
  ptrdiff_t n = 0;
  Lisp_Object tail, frame;
  FOR_EACH_FRAME (tail, frame)
    if (FRAME_PARENT_FRAME (XFRAME (frame)) == f)
      n++;
  if (n == 0)
    return;
  struct frame **kids = xnmalloc (n, sizeof *kids);
  n = 0;
  FOR_EACH_FRAME (tail, frame)
    {
      struct frame *c = XFRAME (frame);
      if (FRAME_PARENT_FRAME (c) == f && FRAME_PGTK_P (c)
	  && c->output_data.pgtk && !FRAME_TOOLTIP_P (c))
	kids[n++] = c;
    }
  qsort (kids, n, sizeof *kids, height_cmp);
  for (ptrdiff_t i = 0; i < n; i++)
    sync_frame (root, kids[i], s->id, x + lround (kids[i]->left_pos * root->sx),
		y + lround (kids[i]->top_pos * root->sy));
  xfree (kids);
}

/* Bring ROOT's scene in Glitnir up to date (made at the first sync, made
   again when stale).  */
static void
layers_sync (struct glitnir_root *root)
{
  if (root->scene && root->scene_stale)
    {
      root->scene = false;
      GLITNIR_CALL (glitnir_root_drop (root->id));
    }
  root->scene_stale = false;
  if (!root->scene)
    {
      forget_layers (root->frame);
      root->scene = true;
      if (GLITNIR_CALL (glitnir_root_new (root->id)) != GLITNIR_OK)
	return;
    }
  sync_frame (root, root->frame, 0, 0, 0);
}

/* `verify': Glitnir's stacking of ROOT's child frames is GDK's (each two
   siblings in the same order).  */
static void
verify_stacking (struct glitnir_root *root)
{
  const uint64_t *ids = NULL;
  size_t n = 0;
  if (GLITNIR_CALL (glitnir_stack_list (root->id, &ids, &n)) != GLITNIR_OK
      || n < 2)
    return;
  struct frame **fs = xnmalloc (n, sizeof *fs);
  for (size_t i = 0; i < n; i++)
    fs[i] = frame_of_id (ids[i]);
  bool bad = false;
  for (size_t i = 0; i < n && !bad; i++)
    for (size_t j = i + 1; j < n && !bad; j++)
      {
	struct frame *a = fs[i], *b = fs[j];
	if (a == NULL || b == NULL
	    || FRAME_PARENT_FRAME (a) != FRAME_PARENT_FRAME (b))
	  continue;
	GtkWidget *wa = FRAME_GTK_WIDGET (a), *wb = FRAME_GTK_WIDGET (b);
	GdkWindow *ga = wa ? gtk_widget_get_window (wa) : NULL;
	GdkWindow *gb = wb ? gtk_widget_get_window (wb) : NULL;
	if (ga == NULL || gb == NULL
	    || gdk_window_get_parent (ga) != gdk_window_get_parent (gb))
	  continue;
	/* A below B in Glitnir: A after B in GDK's list (top first).  */
	bad = gdk_height (a) > gdk_height (b);
      }
  if (bad)
    {
      root->violations++;
      fprintf (stderr,
	       "glitnir verify: %"PRIu64" stacking differs from GDK's {",
	       root->id);
      for (size_t i = 0; i < n; i++)
	fprintf (stderr, " %"PRIu64"@%d", ids[i],
		 fs[i] ? gdk_height (fs[i]) : INT_MIN);
      fprintf (stderr, "}");
      verify_state (root);
    }
  xfree (fs);
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
      verify_drop (root);
      gtk_widget_queue_draw (w);
      root->commit_px += fw * fh;
      return true;
    }
  gather_children (root);
  if (debug_is ("full"))
    {
      /* Every commit damages everything (not through damage_all: the
	 root is committing, it must not become dirty again).  */
      root->nbatch = 0;
      root->pending = root->all = root->known = true;
      GLITNIR_CALL (glitnir_damage_all (root->id));
    }
  flush_damage (root);
  layers_sync (root);
  if (glitnir_pgtk_dead)
    return false;
  root->pending = root->all = false;
  GlitnirCommitOut out = { NULL, 0 };
  int32_t rc;
  if (panic_armed == PANIC_COMMIT)
    {
      panic_armed = PANIC_NONE;
      rc = GLITNIR_CALL (glitnir_debug_panic ());
    }
  else
    rc = GLITNIR_CALL (glitnir_commit (root->id, &out));
  if (rc != GLITNIR_OK)
    {
      /* glitnir_pgtk_failed has switched Glitnir off and queued the
	 whole-widget draw: the stock way presents.  */
      verify_drop (root);
      return false;
    }
  if (verifying ())
    {
      verify_frame (root, out.rects, out.n);
      /* OUT stays valid: listing the stacking leaves the commit alone.  */
      verify_stacking (root);
    }
  else
    verify_drop (root);
  bool flash = debug_is ("damage");
  GlitnirRect old[GLITNIR_PGTK_BATCH];
  int nold = root->nflash;
  if (flash)
    {
      memcpy (old, root->flash, sizeof old);
      root->nflash = 0;
      flash_color (root);
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
      if (flash)
	{
	  GlitnirRect fr = { x0, y0, x1 - x0, y1 - y0 };
	  if (root->nflash < GLITNIR_PGTK_BATCH)
	    root->flash[root->nflash++] = fr;
	  else
	    {
	      /* Too many to keep: flash their bounding box.  */
	      GlitnirRect *l = &root->flash[GLITNIR_PGTK_BATCH - 1];
	      int rx = max (l->x + l->w, fr.x + fr.w);
	      int ry = max (l->y + l->h, fr.y + fr.h);
	      l->x = min (l->x, fr.x);
	      l->y = min (l->y, fr.y);
	      l->w = rx - l->x;
	      l->h = ry - l->y;
	    }
	}
    }
  if (flash)
    flash_start (root, old, nold);
  return any;
}

/* Paint ROOT now (the amended MP-22): the paint lands inside the point
   instead of waiting for a frame-clock paint that would fall into the
   next frozen span.  The commit to the compositor still goes out at
   GDK's after-paint.  A point calls it only when a frame-clock cycle
   already fell into a frozen span (TICK_FROZEN; amended 2026-10-23):
   otherwise the thaw has queued the paint on the frame clock, which paints
   once per cycle after the compositor's frame callback, as stock does.
   Painting at every point drew several times a frame, inside the command,
   and into a buffer the compositor still held, so GDK allocated a new one
   and copied every pixel the damage left alone into it.  */
static void
paint_now (struct glitnir_root *root)
{
  GtkWidget *w = FRAME_GTK_WIDGET (root->frame);
  GdkWindow *win = w ? gtk_widget_get_window (w) : NULL;
  if (win == NULL)
    return;
  root->paints_now++;
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
      /* The hooks change GDK's stacking meanwhile, not Glitnir's.  */
      if (!glitnir_enable || glitnir_pgtk_dead)
	root->scene_stale = true;
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
  if (verifying () && root->frozen && now - root->freeze_at > 1000000000u)
    fprintf (stderr, "glitnir verify: %"PRIu64" frozen for %d ms\n",
	     root->id, (int) ((now - root->freeze_at) / 1000000));
  bool painted = present (root);
  if (glitnir_pgtk_dead)
    return false;		/* pgtk invalidates the whole widget */
  if (painted && root->t_commit == 0)
    root->t_commit = now;
  thaw (root);
  if (painted && root->tick_frozen)
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
  /* Inside redisplay the change waits for point 1; GTK must not paint the
     root meanwhile (it would show the scene before the commit).  */
  if ((redisplaying_p || buffer_flipping_blocked_p ())
      && recording (root->frame))
    freeze (root);
  glitnir_pgtk_consistency_point (f, 4);
}

/* Raise (RAISE) or lower child frame F's node, as GDK just did its
   window.  */
static void
restack (struct frame *f, bool raise)
{
  struct glitnir_root *root = root_state (f), *s = frame_state (f);
  if (root == NULL || s == NULL)
    return;
  if (FRAME_PARENT_FRAME (f) && recording (root->frame))
    {
      layers_sync (root);	/* a node to move, and every newer one */
      if (s->layer_root == root->id)
	GLITNIR_CALL (raise ? glitnir_stack_raise (root->id, s->id)
		      : glitnir_stack_lower (root->id, s->id));
    }
  glitnir_pgtk_scene_changed (f);
}

void
glitnir_pgtk_raise_lower (struct frame *f, bool raise)
{
  /* pgtk raises and lowers visible frames only.  */
  if (FRAME_VISIBLE_P (f))
    restack (f, raise);
}

void
glitnir_pgtk_frame_mapped (struct frame *f)
{
  /* gtk_widget_show maps the frame's window, and gdk_window_show raises
     it; a widget GTK showed already (a child frame shows at its creation)
     stays where it is.  */
  int h = gdk_height (f);
  Lisp_Object tail, frame;
  FOR_EACH_FRAME (tail, frame)
    {
      struct frame *c = XFRAME (frame);
      if (c != f && FRAME_PARENT_FRAME (c)
	  && FRAME_PARENT_FRAME (c) == FRAME_PARENT_FRAME (f)
	  && FRAME_PGTK_P (c) && c->output_data.pgtk
	  && gdk_height (c) != INT_MAX && gdk_height (c) > h)
	{
	  glitnir_pgtk_scene_changed (f);
	  return;
	}
    }
  restack (f, true);
}

void
glitnir_pgtk_reparented (struct frame *f)
{
  struct glitnir_root *s = frame_state (f), *root = root_state (f);
  if (s == NULL || root == NULL)
    return;
  uint64_t old = s->layer_root;
  if (old != 0 && old != root->id)
    {
      /* F left the root whose scene holds its layer.  */
      if (old == s->id)
	{
	  /* F was a root: its scene goes; it is drawn by its new root.  */
	  s->scene = false;
	  s->dirty = s->scene_pending = false;
	  thaw (s);
	  GLITNIR_CALL (glitnir_root_drop (s->id));
	}
      else
	{
	  struct frame *of = frame_of_id (old);
	  struct glitnir_root *o = of ? FRAME_X_OUTPUT (of)->glitnir : NULL;
	  if (o && recording (of))
	    {
	      GLITNIR_CALL (glitnir_layer_remove (old, s->id));
	      glitnir_pgtk_scene_changed (of);
	    }
	  else if (o)
	    o->scene_stale = true;
	}
      forget_layers (f);
    }
  else if (old == root->id && FRAME_PARENT_FRAME (f) && recording (root->frame))
    {
      /* Within the root: on top of the new parent's nodes, as
	 gtk_fixed_put and the show after it put its window.  */
      struct glitnir_root *p = frame_state (FRAME_PARENT_FRAME (f));
      if (p && p->layer_root == root->id)
	{
	  s->layer.parent = p->id;
	  GLITNIR_CALL (glitnir_stack_reparent (root->id, s->id, p->id));
	}
    }
  glitnir_pgtk_scene_changed (f);
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
   we drew.  A cycle that ran while the root was frozen painted nothing of
   it: the next point paints at once.  */
static void
after_paint (GdkFrameClock *clock, gpointer data)
{
  struct glitnir_root *r = data;
  if (r->frozen)
    r->tick_frozen = true;
  if (!r->drawn)
    return;
  r->drawn = false;
  r->debug_mismatch = false;

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
  /* The entry's draw time is the sum of the frame's draws, ending at the
     last.  */
  e.t_draw_start_ns = r->t_draw_end - min (r->draw_ns, r->t_draw_end);
  e.t_draw_end_ns = r->t_draw_end;
  e.clock_frame_us = gdk_frame_clock_get_frame_time (clock);
  e.presented_us = -1;
  e.damage_px = r->damage_px;
  e.painted_px = r->painted_px;
  /* GDK draws into a staging surface that persists across buffer swaps:
     a back-fill cannot be seen from here (R3, DP-9).  */
  e.backfill = 2;
  e.scale_mismatch = r->scale_mismatch;
  r->scale_mismatch = 0;
  e.violations = r->violations;
  r->violations = 0;
  r->last_commit = r->t_commit;
  r->last_paint = r->t_draw_end;
  r->last_present = now_ns ();
  r->t_update = r->t_freeze = r->t_commit = 0;
  r->draw_ns = r->damage_px = r->painted_px = r->commit_px = 0;
  GLITNIR_CALL_UNCOUNTED (glitnir_stats_record (r->id, &e));
}

/* The draw handler of F's widget begins.  Returns true when F is a root,
   or a child frame of a root Glitnir draws; input is then blocked until
   glitnir_pgtk_draw_end.  A child frame's draw counts in its root's
   record (it is part of the root's paint).  */
bool
glitnir_pgtk_draw_begin (struct frame *f, GtkWidget *widget, cairo_t *cr)
{
  if (f->tooltip)
    return false;
  if (FRAME_PARENT_FRAME (f))
    {
      struct glitnir_root *s = frame_state (f), *rs = root_state (f);
      if (s == NULL || rs == NULL || recording (rs->frame) == NULL)
	return false;
      block_input ();
      s->t_draw_start = now_ns ();
      s->draw_cr = cr;
      return true;
    }
  struct glitnir_root *r = root_state (f);
  if (r == NULL)
    return false;
  block_input ();
  r->t_draw_start = now_ns ();

  cairo_surface_t *target = cairo_get_target (cr);
  static const cairo_user_data_key_t seen_key;
  if (cairo_surface_get_user_data (target, &seen_key) == NULL
      && cairo_surface_set_user_data (target, &seen_key, r, NULL)
	 == CAIRO_STATUS_SUCCESS)
    r->new_buffers++;
  double sx = 1, sy = 1;
  cairo_surface_get_device_scale (target, &sx, &sy);
  GtkAllocation alloc;
  gtk_widget_get_allocation (widget, &alloc);
  double scale_frame = gtk_widget_get_scale_factor (widget);
  if (sx != scale_frame)
    r->scale_mismatch = 1;
  r->draws++;
  r->draw_cr = cr;

  bool rec = recording (f) != NULL;
  if (rec && panic_armed == PANIC_DRAW)
    {
      panic_armed = PANIC_NONE;
      GLITNIR_CALL (glitnir_debug_panic ());
      if (glitnir_pgtk_dead)
	{
	  /* The failure painted the stock picture into CR; the draw ends
	     here, outside Glitnir.  */
	  r->draw_cr = NULL;
	  GLITNIR_UNBLOCK_INPUT ();
	  return false;
	}
    }
  if (rec)
    {
      if (verifying () && (r->dirty || r->scene_pending))
	{
	  fprintf (stderr, "glitnir verify: compose with pending damage");
	  verify_state (r);
	  /* Who dispatched GLib with the change uncommitted.  */
	  emacs_backtrace (100);
	  r->violations++;
	}
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
      r->painted_px += px;
      r->damage_px = r->commit_px;
    }
  else
    {
      /* Stock paints the whole widget; the damage is what GDK asked for.  */
      r->painted_px += (uint64_t) ((double) alloc.width * sx)
		       * (uint64_t) ((double) alloc.height * sy);
      double x1, y1, x2, y2;
      cairo_clip_extents (cr, &x1, &y1, &x2, &y2);
      r->damage_px += (x2 > x1 && y2 > y1)
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

/* Child frames as layers, paint-through (DP-1 (i)): every frame widget
   of a root Glitnir draws composes the root's committed scene into GDK's
   buffer, which the root and its child frames share (R2), within its own
   rectangle and GTK's clip.  The root leaves out the rectangles its mapped
   child frames paint (GTK draws them after it); a child frame leaves out
   its own.  So a rounded corner shows what lies beneath, a shadow falls
   on the root and on lower popups, and a move shows the committed
   position, never GTK's newer one.  */

/* Whether GTK draws child frame F inside a group: an `alpha' below 1 on F
   or a child frame it lies in (GTK pushes a group for a widget's
   opacity).  F then paints itself the stock way, so its parent's draw
   neither leaves its rectangle to it nor composes it.  pgtk sets the
   opacity only when the frame's focus changes, so the parameter counts
   too (GLITNIR-ALPHA-1: alpha children are drawn stock until P2).  */
static bool
draws_stock (struct frame *f)
{
  for (; FRAME_PARENT_FRAME (f); f = FRAME_PARENT_FRAME (f))
    {
      GtkWidget *w = FRAME_GTK_WIDGET (f);
      if ((w && gtk_widget_get_opacity (w) < 1)
	  || (f->alpha[0] >= 0 && f->alpha[0] < 1)
	  || (f->alpha[1] >= 0 && f->alpha[1] < 1))
	return true;
    }
  return false;
}

/* F's retained surface: the visible bell's while it shows, else the last
   completely drawn one, as the stock draw paints it.  NULL unless it is
   an ARGB32 image.  */
static cairo_surface_t *
frame_source (struct frame *f)
{
  cairo_surface_t *s = FRAME_X_OUTPUT (f)->cr_surface_visible_bell;
  if (s == NULL && f->output_data.pgtk->cr_active != NULL)
    s = cairo_get_target (f->output_data.pgtk->cr_active);
  if (s == NULL || cairo_surface_get_type (s) != CAIRO_SURFACE_TYPE_IMAGE
      || cairo_image_surface_get_format (s) != CAIRO_FORMAT_ARGB32
      || cairo_image_surface_get_data (s) == NULL)
    return NULL;
  return s;
}

/* The root device-pixel rectangle of a widget at logical X, Y (in the
   root widget) of logical size W x H, at scale SX, SY.  */
static cairo_rectangle_int_t
widget_rect (int x, int y, int w, int h, double sx, double sy)
{
  int x0 = lround (x * sx), y0 = lround (y * sy);
  return (cairo_rectangle_int_t) { x0, y0, (int) ceil ((x + w) * sx) - x0,
				   (int) ceil ((y + h) * sy) - y0 };
}

bool
glitnir_pgtk_draw (struct frame *f, cairo_t *cr)
{
  struct frame *rf = glitnir_root_of (f);
  struct glitnir_root *root = frame_state (rf);
  GtkWidget *w = FRAME_GTK_WIDGET (f), *rw = FRAME_GTK_WIDGET (rf);
  if (root == NULL || !root->scene || w == NULL || rw == NULL
      || recording (rf) == NULL)
    return false;
  /* Stock for this draw, no chrome: a child painted inside a group.  */
  cairo_surface_t *target = cairo_get_target (cr);
  if (cairo_get_group_target (cr) != target || draws_stock (f)
      || cairo_surface_get_type (target) != CAIRO_SURFACE_TYPE_IMAGE
      || cairo_image_surface_get_format (target) != CAIRO_FORMAT_ARGB32
      || cairo_image_surface_get_data (target) == NULL)
    return false;
  /* GTK places a child widget's draw with a translation of the CTM (the
     device transform holds the scale and offset).  */
  cairo_matrix_t m;
  cairo_get_matrix (cr, &m);
  int ox = 0, oy = 0;
  if (m.xx != 1 || m.yy != 1 || m.xy != 0 || m.yx != 0
      || (f != rf && !gtk_widget_translate_coordinates (w, rw, 0, 0,
							 &ox, &oy)))
    return false;
  double sx = 1, sy = 1, dx = 0, dy = 0;
  cairo_surface_get_device_scale (target, &sx, &sy);
  cairo_surface_get_device_offset (target, &dx, &dy);
  /* Root device px (X, Y) is the target's pixel (X + RX, Y + RY).  */
  int rx = lround ((m.x0 - ox) * sx + dx), ry = lround ((m.y0 - oy) * sy + dy);

  /* What this draw composes: GTK's clip within the widget, less the
     rectangles of the mapped child frames GTK draws after it.  */
  cairo_region_t *reg = cairo_region_create ();
  cairo_rectangle_list_t *l = cairo_copy_clip_rectangle_list (cr);
  int nl = l->status == CAIRO_STATUS_SUCCESS ? l->num_rectangles : 1;
  for (int i = 0; i < nl; i++)
    {
      double x1, y1, x2, y2;
      if (l->status == CAIRO_STATUS_SUCCESS)
	{
	  cairo_rectangle_t *c = &l->rectangles[i];
	  x1 = c->x, y1 = c->y, x2 = c->x + c->width, y2 = c->y + c->height;
	}
      else
	cairo_clip_extents (cr, &x1, &y1, &x2, &y2);
      int bx = floor ((x1 + m.x0) * sx + dx) - rx;
      int by = floor ((y1 + m.y0) * sy + dy) - ry;
      cairo_rectangle_int_t b
	= { bx, by, (int) ceil ((x2 + m.x0) * sx + dx) - rx - bx,
	    (int) ceil ((y2 + m.y0) * sy + dy) - ry - by };
      if (b.width > 0 && b.height > 0)
	cairo_region_union_rectangle (reg, &b);
    }
  cairo_rectangle_list_destroy (l);
  cairo_rectangle_int_t own
    = widget_rect (ox, oy, gtk_widget_get_allocated_width (w),
		   gtk_widget_get_allocated_height (w), sx, sy);
  cairo_region_intersect_rectangle (reg, &own);
  Lisp_Object tail, frame;
  ptrdiff_t nframes = 0;
  FOR_EACH_FRAME (tail, frame)
    {
      struct frame *c = XFRAME (frame);
      GtkWidget *cw;
      int cx, cy;
      nframes++;
      if (FRAME_PARENT_FRAME (c) != f || !FRAME_PGTK_P (c)
	  || c->output_data.pgtk == NULL || FRAME_TOOLTIP_P (c)
	  || (cw = FRAME_GTK_WIDGET (c)) == NULL
	  || !gtk_widget_is_drawable (cw) || draws_stock (c)
	  || !gtk_widget_translate_coordinates (cw, rw, 0, 0, &cx, &cy))
	continue;
      cairo_rectangle_int_t b
	= widget_rect (cx, cy, gtk_widget_get_allocated_width (cw),
		       gtk_widget_get_allocated_height (cw), sx, sy);
      cairo_region_subtract_rectangle (reg, &b);
    }
  int nclip = cairo_region_num_rectangles (reg);
  if (nclip == 0)
    {
      cairo_region_destroy (reg);
      return true;		/* the child frames paint all of it */
    }

  /* The sources: the retained surfaces of the root and of every child
     frame in its scene (but those painting themselves).  */
  GlitnirSource *src = xnmalloc (nframes, sizeof *src);
  ptrdiff_t nsrc = 0;
  FOR_EACH_FRAME (tail, frame)
    {
      struct frame *c = XFRAME (frame);
      if (nsrc == nframes || !FRAME_PGTK_P (c) || c->output_data.pgtk == NULL
	  || FRAME_TOOLTIP_P (c) || glitnir_root_of (c) != rf || draws_stock (c))
	continue;
      struct glitnir_root *s = FRAME_X_OUTPUT (c)->glitnir;
      cairo_surface_t *p;
      if (s == NULL || s->layer_root != root->id
	  || (p = frame_source (c)) == NULL)
	continue;
      cairo_surface_flush (p);
      double ssx = 1, ssy = 1;
      cairo_surface_get_device_scale (p, &ssx, &ssy);
      if (root->debug_mismatch && c == rf)
	ssx = sx + 1;
      src[nsrc++] = (GlitnirSource) {
	.layer = s->id, .pixels = cairo_image_surface_get_data (p),
	.stride = cairo_image_surface_get_stride (p),
	.width = cairo_image_surface_get_width (p),
	.height = cairo_image_surface_get_height (p), .scale = ssx };
    }
  GlitnirRect *clip = xnmalloc (nclip, sizeof *clip);
  for (int i = 0; i < nclip; i++)
    {
      cairo_rectangle_int_t b;
      cairo_region_get_rectangle (reg, i, &b);
      clip[i] = (GlitnirRect) { b.x, b.y, b.width, b.height };
    }
  cairo_region_destroy (reg);
  cairo_surface_flush (target);
  GlitnirTarget t = {
    .pixels = cairo_image_surface_get_data (target),
    .stride = cairo_image_surface_get_stride (target),
    .width = cairo_image_surface_get_width (target),
    .height = cairo_image_surface_get_height (target),
    .origin_x = -rx, .origin_y = -ry, .scale = sx };

  /* Not GLITNIR_CALL: GLITNIR_E_STATE (no scene, or a source at another
     scale than the target, as between a monitor's scale change and pgtk's
     next look at it) only draws this root the stock way.  */
  int32_t rc = GLITNIR_E_STATE;
  if (!glitnir_pgtk_dead)
    {
      glitnir_pgtk_calls++;
      block_input ();
      rc = glitnir_compose (root->id, &t, clip, nclip, src, nsrc);
      GLITNIR_UNBLOCK_INPUT ();
      if (rc != GLITNIR_OK && rc != GLITNIR_E_STATE)
	glitnir_pgtk_failed ("glitnir_compose", rc);
    }
  if (rc == GLITNIR_E_STATE)
    for (ptrdiff_t i = 0; i < nsrc; i++)
      if (src[i].scale != sx)
	root->scale_mismatch = 1;
  if (rc == GLITNIR_OK)
    for (int i = 0; i < nclip; i++)
      cairo_surface_mark_dirty_rectangle (target, clip[i].x + rx - dx,
					  clip[i].y + ry - dy, clip[i].w,
					  clip[i].h);
  xfree (clip);
  xfree (src);
  return rc == GLITNIR_OK;
}

/* For tests: every draw of F's root's next paint sees a source scale that
   is not the target's.  */
bool
glitnir_pgtk_debug_scale_mismatch (struct frame *f)
{
  struct glitnir_root *root = root_state (f);
  if (root == NULL || recording (root->frame) == NULL)
    return false;
  GtkWidget *w = FRAME_GTK_WIDGET (root->frame);
  if (w == NULL)
    return false;
  root->debug_mismatch = true;
  gtk_widget_queue_draw (w);
  return true;
}

void
glitnir_pgtk_draw_end (struct frame *f)
{
  struct glitnir_root *r = root_state (f);
  if (r && FRAME_PARENT_FRAME (f))
    {
      struct glitnir_root *s = frame_state (f);
      uint64_t now = now_ns ();
      s->draw_cr = NULL;
      r->draw_ns += now - s->t_draw_start;
      r->t_draw_end = max (r->t_draw_end, now);
      r->drawn = true;
      r->tick_frozen = false;
    }
  else if (r)
    {
      if (r->nflash && r->draw_cr)
	paint_flash (r, r->draw_cr);
      r->draw_cr = NULL;
      uint64_t now = now_ns ();
      r->t_draw_end = now;
      r->draw_ns += now - r->t_draw_start;
      r->drawn = true;
      r->tick_frozen = false;
      if (r->gap_from)
	r->max_gap = max (r->max_gap, now - r->gap_from);
      r->gap_from = now;
    }
  /* Not unblock_input: GTK's draw handler must not dispatch GLib again.  */
  GLITNIR_UNBLOCK_INPUT ();
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
  if (r->flash_id)
    g_source_remove (r->flash_id);
  verify_drop (r);
  thaw (r);
  /* Its node leaves its root's scene (F's own child frames went before
     it), so the root's next commit damages where it was and nothing reads
     its pixels after.  F is off the frame list already: the commit waits
     for the next point, the root frozen until then.  */
  struct frame *rf = (r->layer_root && r->layer_root != r->id
		      ? frame_of_id (r->layer_root) : NULL);
  struct glitnir_root *root = rf ? FRAME_X_OUTPUT (rf)->glitnir : NULL;
  if (root && recording (rf))
    {
      GLITNIR_CALL (glitnir_layer_remove (root->id, r->id));
      root->scene_pending = true;
      freeze (root);
      if (root->idle_id == 0)
	root->idle_id = g_idle_add (idle_point, root);
    }
  else if (root)
    root->scene_stale = true;
  if (r->scene)
    GLITNIR_CALL (glitnir_root_drop (r->id));
  xfree (r->wins);
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

/* Arm (WHERE 1 or 2) or disarm (0) the next panic; WHERE 3 turns Glitnir
   back on after one (every root's damage starts from the whole frame).  */
void
glitnir_pgtk_debug_panic (int where)
{
  if (where != 3)
    {
      panic_armed = where;
      return;
    }
  panic_armed = PANIC_NONE;
  if (!glitnir_pgtk_dead)
    return;
  glitnir_pgtk_dead = false;
  Lisp_Object tail, frame;
  FOR_EACH_FRAME (tail, frame)
    {
      struct frame *f = XFRAME (frame);
      if (!FRAME_PGTK_P (f) || f->output_data.pgtk == NULL)
	continue;
      struct glitnir_root *r = FRAME_X_OUTPUT (f)->glitnir;
      if (r)
	r->scene_stale = true;	/* the scenes before the panic */
      if (r && recording (f))
	damage_all (r);
    }
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

void
glitnir_pgtk_paint_counters (struct frame *f, uintmax_t *paints_now,
			     uintmax_t *new_buffers)
{
  struct glitnir_root *r = root_state (f);
  *paints_now = r ? r->paints_now : 0;
  *new_buffers = r ? r->new_buffers : 0;
}

bool
glitnir_pgtk_frozen_p (struct frame *f)
{
  struct glitnir_root *r = root_state (f);
  return r && r->frozen;
}

void
glitnir_pgtk_last_paint (struct frame *f, uint64_t *commit, uint64_t *paint,
			 uint64_t *present)
{
  struct glitnir_root *r = root_state (f);
  *commit = r ? r->last_commit : 0;
  *paint = r ? r->last_paint : 0;
  *present = r ? r->last_present : 0;
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

uint64_t
glitnir_pgtk_violations (struct frame *f, bool reset)
{
  struct glitnir_root *r = root_state (f);
  if (r == NULL)
    return 0;
  uint64_t n = r->violations;
  if (reset)
    r->violations = 0;
  return n;
}

ptrdiff_t
glitnir_pgtk_stacking (struct frame *f, struct frame **out, ptrdiff_t nmax)
{
  struct glitnir_root *root = root_state (f);
  if (root == NULL || recording (root->frame) == NULL)
    return 0;
  layers_sync (root);
  const uint64_t *ids = NULL;
  size_t n = 0;
  if (GLITNIR_CALL (glitnir_stack_list (root->id, &ids, &n)) != GLITNIR_OK)
    return 0;
  ptrdiff_t k = 0;
  for (size_t i = 0; i < n && k < nmax; i++)
    {
      struct frame *c = frame_of_id (ids[i]);
      if (c)
	out[k++] = c;
    }
  return k;
}

bool
glitnir_pgtk_layer_rect (struct frame *f, GlitnirRect *out)
{
  struct glitnir_root *s = frame_state (f);
  if (s == NULL || s->layer_root == 0)
    return false;
  *out = s->layer.rect;
  return true;
}

static float
chrome_num (Lisp_Object v)
{
  return NUMBERP (v) ? (float) max (XFLOATINT (v), 0.0) : 0;
}

static Lisp_Object
chrome_key (Lisp_Object plist, const char *name)
{
  return Fplist_get (plist, intern_c_string (name), Qnil);
}

void
glitnir_pgtk_params_changed (struct frame *f)
{
  static bool busy;		/* the resolver changed parameters itself */
  if (busy || !FRAME_PGTK_P (f) || f->output_data.pgtk == NULL
      || FRAME_TOOLTIP_P (f))
    return;
  struct glitnir_root *s = FRAME_X_OUTPUT (f)->glitnir;
  if (FRAME_PARENT_FRAME (f))
    s = frame_state (f);
  if (s == NULL || NILP (Ffboundp (Qglitnir__resolve)))
    return;
  Lisp_Object frame, res;
  XSETFRAME (frame, f);
  busy = true;
  res = safe_calln (Qglitnir__resolve, frame);
  busy = false;
  struct glitnir_root n = *s;
  n.has_chrome = CONSP (res);
  n.ch_radius = n.ch_blur = n.ch_ox = n.ch_oy = n.ch_spread = 0;
  n.ch_opacity = 0;
  n.ch_rgb = 0;
  if (n.has_chrome)
    {
      n.ch_radius = chrome_num (chrome_key (res, ":radius"));
      Lisp_Object sh = chrome_key (res, ":shadow");
      if (CONSP (sh))
	{
	  n.ch_blur = chrome_num (chrome_key (sh, ":blur"));
	  Lisp_Object off = chrome_key (sh, ":offset");
	  if (CONSP (off))
	    {
	      n.ch_ox = NUMBERP (XCAR (off)) ? XFLOATINT (XCAR (off)) : 0;
	      n.ch_oy = NUMBERP (XCDR (off)) ? XFLOATINT (XCDR (off)) : 0;
	    }
	  n.ch_spread = NUMBERP (chrome_key (sh, ":spread"))
	    ? XFLOATINT (chrome_key (sh, ":spread")) : 0;
	  n.ch_opacity = min (chrome_num (chrome_key (sh, ":opacity")), 1);
	  Lisp_Object col = chrome_key (sh, ":color");
	  uint32_t rgb = 0;
	  for (int i = 0; i < 3; i++, col = CONSP (col) ? XCDR (col) : Qnil)
	    rgb = rgb << 8 | (CONSP (col) && FIXNATP (XCAR (col))
			      ? min (XFIXNAT (XCAR (col)), 255) : 0);
	  n.ch_rgb = rgb;
	}
    }
  if (n.has_chrome == s->has_chrome && n.ch_radius == s->ch_radius
      && n.ch_blur == s->ch_blur && n.ch_ox == s->ch_ox
      && n.ch_oy == s->ch_oy && n.ch_spread == s->ch_spread
      && n.ch_opacity == s->ch_opacity && n.ch_rgb == s->ch_rgb)
    return;
  s->has_chrome = n.has_chrome;
  s->ch_radius = n.ch_radius;
  s->ch_blur = n.ch_blur;
  s->ch_ox = n.ch_ox;
  s->ch_oy = n.ch_oy;
  s->ch_spread = n.ch_spread;
  s->ch_opacity = n.ch_opacity;
  s->ch_rgb = n.ch_rgb;
  s->chrome_dirty = true;
  glitnir_pgtk_scene_changed (f);
}

void
glitnir_pgtk_record_border (struct frame *f, int width, unsigned long rgb,
			    double alpha)
{
  if (!FRAME_PARENT_FRAME (f))
    return;
  struct glitnir_root *s = frame_state (f);
  if (s == NULL)
    return;
  alpha = min (max (alpha, 0.0), 1.0);
  uint32_t a = lround (alpha * 255);
  uint32_t r = (rgb >> 16 & 0xff) * a / 255, g = (rgb >> 8 & 0xff) * a / 255,
    b = (rgb & 0xff) * a / 255;
  uint32_t argb = a << 24 | r << 16 | g << 8 | b;
  if (width == s->border_w && argb == s->border_argb)
    return;
  s->border_w = width;
  s->border_argb = argb;
  if (s->has_chrome)
    {
      /* The next point (the draw that records this makes one) commits
	 the scene with the new ring.  */
      s->chrome_dirty = true;
      root_state (f)->scene_pending = true;
    }
}

bool
glitnir_pgtk_chrome (struct frame *f, GlitnirChrome *out)
{
  struct glitnir_root *root = root_state (f);
  struct glitnir_root *s = frame_state (f);
  if (root == NULL || s == NULL)
    return false;
  if (recording (root->frame))
    layers_sync (root);
  if (!s->chrome_set || s->layer_root != root->id)
    return false;
  *out = s->applied;
  return true;
}

/* Paint a rectangle on F's surface without recording it.  */
bool
glitnir_pgtk_draw_undamaged (struct frame *f)
{
  struct glitnir_root *r = recording (f);
  if (r == NULL)
    return false;
  cairo_t *cr = f->output_data.pgtk->cr_context;
  if (cr == NULL)
    return false;
  cairo_save (cr);
  cairo_set_operator (cr, CAIRO_OPERATOR_SOURCE);
  cairo_set_source_rgb (cr, 0.2, 0.4, 0.6);
  cairo_rectangle (cr, FRAME_PIXEL_WIDTH (f) / 2, FRAME_PIXEL_HEIGHT (f) / 2,
		   40, 20);
  cairo_fill (cr);
  cairo_restore (cr);
  return true;
}

/* Freeze F's root as the first damage after a point does (the same path:
   dirty, frozen, the freeze time recorded, the idle point queued outside
   redisplay), with nothing drawn.  */
bool
glitnir_pgtk_debug_freeze (struct frame *f)
{
  struct glitnir_root *root = root_state (f);
  if (root == NULL || recording (root->frame) == NULL)
    return false;
  mark_dirty (root->frame);
  return root->frozen != NULL;
}

/* Make GTK draw F's root while it has uncommitted damage.  */
bool
glitnir_pgtk_compose_with_pending (struct frame *f)
{
  struct glitnir_root *root = root_state (f);
  if (root == NULL || recording (root->frame) == NULL)
    return false;
  GtkWidget *w = FRAME_GTK_WIDGET (root->frame);
  if (w == NULL || gtk_widget_get_window (w) == NULL)
    return false;
  glitnir_pgtk_damage (root->frame, 0, 0, 1, 1);
  if (root->frozen == NULL)
    return false;
  GdkWindow *top = g_object_ref (root->frozen);
  gdk_window_thaw_updates (top);
  gtk_widget_queue_draw (w);
  paint_now (root);
  gdk_window_freeze_updates (top);
  g_object_unref (top);
  glitnir_pgtk_consistency_point (root->frame, 4);
  return true;
}

/* Dispatch a synthetic pointer motion to logical X, Y of F's widget at
   once, as GTK would on receiving it (not through GDK's queue, which holds
   the newest motion back until another event follows).  */
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
  gtk_main_do_event (ev);
  gdk_event_free (ev);
  return true;
}

#endif /* HAVE_GLITNIR */

/* Naglfar's side of the drawing seam (HAVE_NHMAC): the primitives under
   nhwldraw.c's shared geometry (backgrounds, boxes, reliefs, stretches,
   decorations, cursors) and the drawing entries of the redisplay interface
   that are Cairo code on Wayland.  Each primitive becomes one draw record
   (nhg_draw), which the shell paints at once into the frame's backing with
   Core Graphics; the shell copies what changed into the window's
   IOSurfaces at each publish.  C reads Emacs's structures and writes no
   pixels.  Text and images are not drawn yet.

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

#include <math.h>

#include "lisp.h"
#include "nhwlterm.h"
#include "frame.h"
#include "window.h"
#include "keyboard.h"
#include "font.h"

/* The clip in force: the intersection of every clip set since the
   outermost nhwl_begin_cr_clip, as up to two rectangles (N 0: none).  One
   level per begin or save, as Cairo's save and restore nest; redisplay
   draws on one thread at a time, under the global lock.  */
struct nhmac_clip
{
  int n;
  NhgRect r[2];
};
static struct nhmac_clip nhmac_clips[16];
static int nhmac_depth;

static void
nhmac_push (void)
{
  if (nhmac_depth + 1 < ARRAYELTS (nhmac_clips))
    nhmac_clips[nhmac_depth + 1] = nhmac_clips[nhmac_depth];
  nhmac_depth++;
}

static void
nhmac_pop (void)
{
  if (nhmac_depth > 0)
    nhmac_depth--;
  if (nhmac_depth == 0)
    nhmac_clips[0].n = 0;
}

static struct nhmac_clip *
nhmac_top (void)
{
  return &nhmac_clips[min (nhmac_depth, ARRAYELTS (nhmac_clips) - 1)];
}

/* Narrow the clip to the union of the N rectangles at R.  */
static void
nhmac_clip_to (const XRectangle *r, int n)
{
  struct nhmac_clip *c = nhmac_top (), out = { 0 };
  for (int i = 0; i < n && i < 2; i++)
    {
      NhgRect a = { r[i].x, r[i].y, r[i].width, r[i].height };
      if (!c->n)
	out.r[out.n++] = a;
      for (int j = 0; j < c->n && out.n < 2; j++)
	{
	  int x0 = max (a.x, c->r[j].x), y0 = max (a.y, c->r[j].y);
	  int x1 = min (a.x + a.w, c->r[j].x + c->r[j].w);
	  int y1 = min (a.y + a.h, c->r[j].y + c->r[j].h);
	  if (x1 > x0 && y1 > y0)
	    out.r[out.n++] = (NhgRect) { x0, y0, x1 - x0, y1 - y0 };
	}
    }
  /* Nothing left: an empty rectangle clips everything away.  */
  if (!out.n)
    out.n = 1;
  *c = out;
}

/* COLOR (0xRRGGBB) as the records' ARGB, with F's alpha-background when
   RESPECT.  */
static uint32_t
nhmac_argb (struct frame *f, unsigned long color, bool respect)
{
  uint32_t a = respect ? clip_to_bounds (0, f->alpha_background * 255, 255)
		       : 255;
  return a << 24 | (color & 0xffffff);
}

/* Test diagnostics: the records whose rectangle meets a watched one, in
   the order they were painted (`nhmac--test-draw-watch').  */
static NhgRect nhmac_watch;
static NhgDrawRec nhmac_watched[64];
static int nhmac_nwatched;

static void
nhmac_watch_note (const NhgDrawRec *r)
{
  const NhgRect *a = &r->r, *w = &nhmac_watch;
  if (w->w > 0 && nhmac_nwatched < ARRAYELTS (nhmac_watched)
      && a->x < w->x + w->w && w->x < a->x + a->w
      && a->y < w->y + w->h && w->y < a->y + a->h)
    nhmac_watched[nhmac_nwatched++] = *r;
}

/* Paint record R into F's backing under the clip in force.  */
static void
nhmac_emit (struct frame *f, NhgDrawRec *r)
{
  struct nhmac_clip *c = nhmac_top ();
  r->nclip = c->n;
  memcpy (r->clip, c->r, sizeof r->clip);
  nhmac_watch_note (r);
  nhwl_glue_rc (FRAME_DISPLAY_INFO (f),
		NHG (nhg_draw (FRAME_NHG (f), FRAME_NHWL_OUTPUT (f)->glitnir_id,
			       r, 1)));
}

static void
nhmac_rect (struct frame *f, uint32_t kind, uint32_t flags, uint32_t fg,
	    int x, int y, int w, int h, int a, int b)
{
  NhgDrawRec r = { kind, flags, { x, y, w, h }, fg, 0, a, b };
  nhmac_emit (f, &r);
}

/* The backing.  */

nhwl_cr *
nhwl_begin_cr_clip (struct frame *f)
{
  nhwl_backing_resize (f);
  nhmac_push ();
  return (nhwl_cr *) f;
}

void
nhwl_end_cr_clip (struct frame *f)
{
  nhmac_pop ();
}

void
nhwl_set_cr_source_with_color (struct frame *f, unsigned long color,
			       bool respects_alpha_background)
{
}

void
nhwl_set_cr_source_with_gc_foreground (struct frame *f, Emacs_GC *gc,
				       bool respects_alpha_background)
{
}

void
nhwl_set_cr_source_with_gc_background (struct frame *f, Emacs_GC *gc,
				       bool respects_alpha_background)
{
}

/* Paint X, Y, WIDTH, HEIGHT with F's background, as the Cairo twin does: a
   clear that only recorded damage left the old picture there (the end of
   each line, a cleared frame).  */
void
nhwl_clear_area (struct frame *f, int x, int y, int width, int height)
{
  nhwl_begin_cr_clip (f);
  nhwl_damage (f, x, y, width, height);
  nhmac_rect (f, NHG_DRAW_FILL, NHG_DRAW_COPY,
	      nhmac_argb (f, FRAME_NHWL_OUTPUT (f)->background_color, true),
	      x, y, width, height, 0, 0);
  nhwl_end_cr_clip (f);
}

/* The face's background, then its stipple in its foreground where the
   stipple's bits are set, tiled from the frame's origin as X fills with
   a FillOpaqueStippled GC (and NS with a pattern colour): NHG_DRAW_BITMAP
   records of at most 16 bits a row, each bit a logical pixel (the frame's
   whole scale, as fringe bitmaps), cut to the area.  A narrow stipple is
   repeated across its record and down 64 rows, so a line takes a few
   records.  */
void
nhwl_fill_background_by_face (struct frame *f, struct face *face, int x,
			      int y, int width, int height)
{
  nhwl_begin_cr_clip (f);
  nhmac_rect (f, NHG_DRAW_FILL, NHG_DRAW_COPY,
	      nhmac_argb (f, face->background, true), x, y, width, height,
	      0, 0);
  Display_Info *dpyinfo = FRAME_DISPLAY_INFO (f);
  struct nhwl_bitmap_record *bm
    = (face->stipple > 0 && face->stipple <= dpyinfo->bitmaps_last
       ? &dpyinfo->bitmaps[face->stipple - 1] : NULL);
  if (bm && bm->bits && bm->width > 0 && bm->height > 0
      && width > 0 && height > 0)
    {
      XRectangle area = { x, y, width, height };
      int k = clip_to_bounds (1, FRAME_NHWL_OUTPUT (f)->scale120 / 120, 8);
      int bw = bm->width, bh = bm->height, stride = (bw + 7) / 8;
      int reps = bw <= 16 ? 16 / bw : 1;
      int vreps = max (1, 64 / bh);
      int rows = bh * vreps;
      uint32_t fg = nhmac_argb (f, face->foreground, true);
      USE_SAFE_ALLOCA;
      uint16_t *bits;
      SAFE_NALLOCA (bits, 1, rows);
      nhmac_clip_to (&area, 1);
      for (int cx = 0; cx < bw; cx += 16)
	{
	  int cw = bw <= 16 ? bw * reps : min (16, bw - cx);
	  int step = (bw <= 16 ? cw : bw) * k;
	  for (int r = 0; r < rows; r++)
	    {
	      const unsigned char *row
		= (const unsigned char *) bm->bits + (r % bh) * stride;
	      uint16_t v = 0;
	      for (int i = 0; i < cw; i++)
		{
		  int b = (cx + i) % bw;
		  if (row[b / 8] >> (b % 8) & 1)
		    v |= 1u << (cw - 1 - i);
		}
	      bits[r] = v;
	    }
	  int x0 = x - ((x - cx * k) % step + step) % step;
	  int y0 = y - (y % (rows * k) + rows * k) % (rows * k);
	  for (int ty = y0; ty < y + height; ty += rows * k)
	    for (int tx = x0; tx < x + width; tx += step)
	      {
		NhgDrawRec r = { NHG_DRAW_BITMAP, NHG_DRAW_OVERLAY,
				 { tx, ty, cw, rows }, fg, 0, k };
		r.bits = bits;
		nhmac_emit (f, &r);
	      }
	  if (bw <= 16)
	    break;
	}
      SAFE_FREE ();
    }
  nhwl_end_cr_clip (f);
}

/* The primitives under the shared geometry; their Cairo twins are in
   nhwldraw.c.  */

void
nhwl_fill_rectangle (struct frame *f, unsigned long color, int x, int y,
		     int width, int height, bool respect_alpha_background)
{
  nhwl_begin_cr_clip (f);
  nhmac_rect (f, NHG_DRAW_FILL,
	      respect_alpha_background ? NHG_DRAW_COPY : 0,
	      nhmac_argb (f, color, respect_alpha_background), x, y, width,
	      height, 0, 0);
  nhwl_end_cr_clip (f);
}

void
nhwl_draw_rectangle (struct frame *f, unsigned long color, int x, int y,
		     int width, int height, bool respect_alpha_background)
{
  nhwl_begin_cr_clip (f);
  nhmac_rect (f, NHG_DRAW_OUTLINE, 0,
	      nhmac_argb (f, color, respect_alpha_background), x, y, width,
	      height, 0, 0);
  nhwl_end_cr_clip (f);
}

void
nhwl_fill_trapezoid_for_relief (struct frame *f, unsigned long color, int x,
				int y, int width, int height, int top_p)
{
  nhwl_begin_cr_clip (f);
  nhmac_rect (f, NHG_DRAW_TRAPEZOID, top_p ? NHG_DRAW_TOP : 0,
	      nhmac_argb (f, color, false), x, y, width, height, 0, 0);
  nhwl_end_cr_clip (f);
}

void
nhwl_erase_corners_for_relief (struct frame *f, unsigned long color, int x,
			       int y, int width, int height, double radius,
			       double margin, int corners)
{
  nhwl_begin_cr_clip (f);
  nhmac_rect (f, NHG_DRAW_CORNERS, corners, nhmac_argb (f, color, false),
	      x, y, width, height, radius, margin);
  nhwl_end_cr_clip (f);
}

/* A box's ring for glitnir-decor.c: one NHG_DRAW_RING record on X, Y, W, H
   with radius RAD, lines HW high and VW wide, FLAGS its rounded corners and
   sides (the record's); with RELIEF, FG above the anti-diagonal and BG
   below.  */
void
nhmac_draw_ring (struct frame *f, int x, int y, int w, int h, int hw, int vw,
		 double rad, unsigned flags, unsigned long fg,
		 unsigned long bg, bool relief)
{
  NhgDrawRec r = { NHG_DRAW_RING, flags | (relief ? NHG_DRAW_RELIEF : 0),
		   { x, y, w, h }, nhmac_argb (f, fg, false),
		   nhmac_argb (f, bg, false), lround (rad),
		   min (hw, 255) | min (vw, 255) << 8 };
  nhmac_emit (f, &r);
}

void
nhwl_set_clip_rectangles (struct frame *f, nhwl_cr *cr,
			  XRectangle *rectangles, int n)
{
  if (n > 0)
    nhmac_clip_to (rectangles, n);
}

void
nhwl_clip_to_row (struct window *w, struct glyph_row *row,
		  enum glyph_row_area area, nhwl_cr *cr)
{
  int window_x, window_y, window_width;
  XRectangle r;

  window_box (w, area, &window_x, &window_y, &window_width, 0);
  r.x = window_x;
  r.y = max (WINDOW_TO_FRAME_PIXEL_Y (w, max (0, row->y)), window_y);
  r.width = window_width;
  r.height = row->visible_height;
  nhmac_clip_to (&r, 1);
}

void
nhwl_draw_horizontal_wave (struct frame *f, unsigned long color, int x,
			   int y, int width, int height, int wave_length)
{
  nhwl_begin_cr_clip (f);
  nhmac_rect (f, NHG_DRAW_WAVE, 0, nhmac_argb (f, color, false), x, y,
	      width, height, wave_length, 0);
  nhwl_end_cr_clip (f);
}

void
nhwl_draw_dash (struct frame *f, struct glyph_string *s,
		unsigned long foreground, int width, char segment, int offset,
		int thickness)
{
  nhwl_begin_cr_clip (f);
  nhmac_rect (f, NHG_DRAW_DASH, 0, nhmac_argb (f, foreground, false), s->x,
	      s->ybase + offset, width, thickness, segment, s->x);
  nhwl_end_cr_clip (f);
}

void
nhmac_save (nhwl_cr *cr)
{
  nhmac_push ();
}

void
nhmac_restore (nhwl_cr *cr)
{
  nhmac_pop ();
}

void
nhmac_set_clipping (struct glyph_string *s, nhwl_cr *cr)
{
  XRectangle r[2];
  int n = get_glyph_string_clip_rects (s, r, 2);

  if (n > 0)
    nhmac_clip_to (r, n);
}

void
nhmac_set_clipping_exactly (struct glyph_string *src,
			    struct glyph_string *dst, nhwl_cr *cr)
{
  XRectangle r = { src->x, src->y, src->width, src->height };

  dst->clip[0] = r;
  dst->num_clips = 1;
  nhmac_clip_to (&r, 1);
}

/* IMAGE's container (it has a pixmap) as the glue reads it.  */
static NhgPix
nhmac_pix (struct image *image)
{
  Emacs_Pixmap p = image->pixmap, m = image->mask;
  NhgPix pix = { (uint8_t *) p->data, m ? (uint8_t *) m->data : NULL,
		 p->width, p->height, p->bytes_per_line,
		 m ? m->bytes_per_line : 0, p->bits_per_pixel,
		 image->nhmac_smoothing };

  memcpy (pix.matrix, image->nhmac_transform, sizeof pix.matrix);
  return pix;
}

/* The part of IMAGE from SRC_X, SRC_Y, WIDTH by HEIGHT, at DEST_X,
   DEST_Y, over GC's background unless OVERLAY_P (nhwldraw.c's Cairo
   twin); a pending IMAGE draws its stand-in.  */
void
nhwl_cr_draw_image (struct frame *f, Emacs_GC *gc, nhwl_image image,
		    int src_x, int src_y, int width, int height,
		    int dest_x, int dest_y, bool overlay_p)
{
  NhgDrawRec r = { NHG_DRAW_IMAGE, overlay_p ? NHG_DRAW_OVERLAY : 0,
		   { dest_x, dest_y, width, height },
		   nhmac_argb (f, gc->foreground, false),
		   nhmac_argb (f, gc->background, true), src_x, src_y };
  uint64_t id = FRAME_NHWL_OUTPUT (f)->glitnir_id;

  nhwl_begin_cr_clip (f);
  r.nclip = nhmac_top ()->n;
  memcpy (r.clip, nhmac_top ()->r, sizeof r.clip);
  if (image->pixmap)
    {
      NhgPix pix = nhmac_pix (image);
      nhwl_glue_rc (FRAME_DISPLAY_INFO (f),
		    NHG (nhg_draw_image (FRAME_NHG (f), id, &r, &pix)));
    }
  else
    nhwl_glue_rc (FRAME_DISPLAY_INFO (f),
		  NHG (nhg_draw_stand_in (FRAME_NHG (f), id, &r, image)));
  nhwl_end_cr_clip (f);
}

/* The image worker's stand-in (image.c's nh_img_stand_in): pending IMG
   draws READY's picture, scaled into its own box, until it lands.  The
   glue keeps a copy, so READY may go first.  */
bool
nhmac_image_stand_in (struct frame *f, struct image *img,
		      struct image *ready)
{
  if (!ready->pixmap)
    return false;
  NhgPix pix = nhmac_pix (ready);
  img->nhmac_stand
    = NHG (nhg_image_stand_in (img, &pix, ready->face_foreground & 0xffffff,
			       ready->width, ready->height,
			       img->width, img->height)) == NHG_OK;
  return img->nhmac_stand;
}

/* IMG landed, is prepared again or is freed: its stand-in goes.  */
void
nhmac_image_stand_drop (struct image *img)
{
  if (img->nhmac_stand)
    NHG (nhg_image_stand_drop (img));
  img->nhmac_stand = false;
}

/* Glyphs FROM to TO of S, in the open font FONT, from X on the baseline
   Y, over their background when WITH_BACKGROUND, in macfont_draw's
   colours.  */
void
nhmac_glyph_run (struct glyph_string *s, uint64_t font, int from, int to,
		 int x, int y, bool with_background)
{
  struct frame *f = s->f;
  struct face *face = s->face;
  unsigned long cursor = FRAME_X_OUTPUT (f)->cursor_color;
  unsigned long fg = face->foreground, bg = face->background;
  uint16_t *glyphs;
  USE_SAFE_ALLOCA;

  if (to <= from)
    return;
  if (s->hl == DRAW_CURSOR)
    {
      fg = bg == cursor ? bg : FRAME_BACKGROUND_PIXEL (f);
      bg = bg == cursor ? face->foreground : cursor;
    }
  SAFE_NALLOCA (glyphs, 1, to - from);
  for (int i = from; i < to; i++)
    glyphs[i - from] = s->char2b[i];
  NhgDrawRec r = { NHG_DRAW_GLYPHS,
		   ((with_background ? NHG_DRAW_BACKGROUND : 0)
		    | (s->padding_p ? NHG_DRAW_PADDING : 0)),
		   { x, y - FONT_BASE (s->font), s->width,
		     FONT_HEIGHT (s->font) },
		   nhmac_argb (f, fg, false), nhmac_argb (f, bg, true), x, y };
  r.bits = glyphs;
  nhwl_begin_cr_clip (f);
  r.nclip = nhmac_top ()->n;
  memcpy (r.clip, nhmac_top ()->r, sizeof r.clip);
  nhmac_watch_note (&r);
  nhwl_glue_rc (FRAME_DISPLAY_INFO (f),
		NHG (nhg_draw_glyphs (FRAME_NHG (f),
				      FRAME_NHWL_OUTPUT (f)->glitnir_id,
				      font, &r, to - from)));
  nhwl_end_cr_clip (f);
  SAFE_FREE ();
}

/* The redisplay interface's drawing.  */

/* Fringe bitmaps are drawn at their own pixel size; the bits stay
   fringe.c's (bit WD - 1 is the leftmost pixel), so nothing is kept
   here.  */
void
nhwl_draw_fringe_bitmap (struct window *w, struct glyph_row *row,
			 struct draw_fringe_bitmap_params *p)
{
  struct frame *f = XFRAME (WINDOW_FRAME (w));
  struct face *face = p->face;

  nhwl_damage_fringe (w, p);
  nhwl_begin_cr_clip (f);
  nhwl_clip_to_row (w, row, ANY_AREA, NULL);
  if (p->bx >= 0 && !p->overlay_p)
    nhwl_fill_background_by_face (f, face, p->bx, p->by, p->nx, p->ny);
  if (p->which && p->bits && p->wd <= 16)
    {
      unsigned long fg = (p->cursor_p
			  ? (p->overlay_p ? face->background
			     : FRAME_X_OUTPUT (f)->cursor_color)
			  : face->foreground);
      bool left = p->x < window_box_left (w, TEXT_AREA);
      int fw = (left ? WINDOW_LEFT_FRINGE_WIDTH (w)
		: WINDOW_RIGHT_FRINGE_WIDTH (w));
      int k = clip_to_bounds (1, min (FRAME_NHWL_OUTPUT (f)->scale120 / 120,
				      fw / max (p->wd, 1)), 8);
      int bh = p->h + p->dh;
      NhgDrawRec r = { NHG_DRAW_BITMAP, p->overlay_p ? NHG_DRAW_OVERLAY : 0,
		       { p->x, p->y, p->wd, p->h },
		       nhmac_argb (f, fg, false),
		       nhmac_argb (f, face->background, false), k };
      r.bits = p->bits + p->dh;
      if (k > 1)
	{
	  /* NS draws a bitmap in points: at a scale, K pixels a bit while
	     it fits its fringe, centred across the fringe's column and cut
	     to it (the Wayland shell's nhwl_draw_fringe_scaled rule).
	     Down the row it keeps the bitmap's alignment, as a bitmap drawn
	     in points does: fringe.c put a top-aligned one (up-arrow, the
	     top angles) at the row's top, a bottom-aligned one at its
	     bottom, and centred the rest; centring a top-aligned one put
	     its first rows above the row, where the row's clip cut them
	     (round 2: up-arrow 22 of 28 bits at scale 2).  */
	  int top = (WINDOW_TO_FRAME_PIXEL_Y (w, row->y)
		     + (left ? row->left_fringe_offset
			: row->right_fringe_offset));
	  int dy = p->y - p->dh - top;
	  int y0 = p->y - p->dh + (bh - k * bh) / 2;
	  if (dy == 0 && (row->height - bh) / 2 != 0)
	    y0 = p->y - p->dh;
	  else if (dy == row->visible_height - bh
		   && dy != (row->height - bh) / 2)
	    y0 = p->y - p->dh + bh - k * bh;
	  XRectangle col = { left ? p->x - (fw - p->wd) / 2
			     : p->x - (fw - p->wd + 1) / 2,
			     p->y - p->dh - bh * k, fw, 3 * bh * k };
	  r.r = (NhgRect) { p->x + (p->wd - k * p->wd) / 2, y0, p->wd, bh };
	  r.bits = p->bits;
	  nhmac_clip_to (&col, 1);
	  /* what it covers, inside the row's clip, reaches the surface */
	  nhwl_damage (f, col.x, y0, fw, k * bh);
	}
      nhmac_emit (f, &r);
    }
  nhwl_end_cr_clip (f);
}

void
nhwl_define_fringe_bitmap (int which, unsigned short *bits, int h, int wd)
{
}

void
nhwl_destroy_fringe_bitmap (int which)
{
}

void
nhwl_draw_vertical_window_border (struct window *w, int x, int y0, int y1)
{
  struct frame *f = XFRAME (WINDOW_FRAME (w));
  struct face *face = FACE_FROM_ID_OR_NULL (f, VERTICAL_BORDER_FACE_ID);

  nhwl_damage (f, x, y0, 1, y1 - y0);
  if (face)
    nhwl_fill_rectangle (f, face->foreground, x, y0, 1, y1 - y0, false);
}

void
nhwl_draw_window_divider (struct window *w, int x0, int x1, int y0, int y1)
{
  struct frame *f = XFRAME (WINDOW_FRAME (w));
  struct face *face = FACE_FROM_ID_OR_NULL (f, WINDOW_DIVIDER_FACE_ID);
  struct face *first
    = FACE_FROM_ID_OR_NULL (f, WINDOW_DIVIDER_FIRST_PIXEL_FACE_ID);
  struct face *last
    = FACE_FROM_ID_OR_NULL (f, WINDOW_DIVIDER_LAST_PIXEL_FACE_ID);
  unsigned long fg = FRAME_FOREGROUND_PIXEL (f);
  unsigned long c = face ? face->foreground : fg;
  unsigned long c0 = first ? first->foreground : fg;
  unsigned long c1 = last ? last->foreground : fg;
  bool a = f->borders_respect_alpha_background;

  nhwl_damage (f, x0, y0, x1 - x0, y1 - y0);
  if (y1 - y0 > x1 - x0 && x1 - x0 > 2)
    {
      nhwl_fill_rectangle (f, c0, x0, y0, 1, y1 - y0, a);
      nhwl_fill_rectangle (f, c, x0 + 1, y0, x1 - x0 - 2, y1 - y0, a);
      nhwl_fill_rectangle (f, c1, x1 - 1, y0, 1, y1 - y0, a);
    }
  else if (x1 - x0 > y1 - y0 && y1 - y0 > 3)
    {
      nhwl_fill_rectangle (f, c0, x0, y0, x1 - x0, 1, a);
      nhwl_fill_rectangle (f, c, x0, y0 + 1, x1 - x0, y1 - y0 - 2, a);
      nhwl_fill_rectangle (f, c1, x0, y1 - 1, x1 - x0, 1, a);
    }
  else
    nhwl_fill_rectangle (f, c, x0, y0, x1 - x0, y1 - y0, a);
}

DEFUN ("nhmac--test-draw-watch", Fnhmac__test_draw_watch,
       Snhmac__test_draw_watch, 4, 4, 0,
       doc: /* Test only: log the draw records that meet a rectangle.
X, Y, W and H are device pixels of the frame's backing; W 0 stops.  The
log starts empty; `nhmac--test-draw-log' reads it.  */)
  (Lisp_Object x, Lisp_Object y, Lisp_Object w, Lisp_Object h)
{
  nhmac_watch = (NhgRect) { check_integer_range (x, INT_MIN, INT_MAX),
			    check_integer_range (y, INT_MIN, INT_MAX),
			    check_integer_range (w, 0, INT_MAX),
			    check_integer_range (h, 0, INT_MAX) };
  nhmac_nwatched = 0;
  return Qnil;
}

DEFUN ("nhmac--test-draw-log", Fnhmac__test_draw_log, Snhmac__test_draw_log,
       0, 0, 0,
       doc: /* Test only: the draw records logged since `nhmac--test-draw-watch'.
Each is (KIND FLAGS (X Y W H) FG BG CLIPS), CLIPS a list of (X Y W H),
in the order they were painted (at most 64).  */)
  (void)
{
  Lisp_Object out = Qnil;
  for (int i = nhmac_nwatched - 1; i >= 0; i--)
    {
      NhgDrawRec *r = &nhmac_watched[i];
      Lisp_Object clips = Qnil;
      for (int j = min (r->nclip, 2) - 1; j >= 0; j--)
	clips = Fcons (list4i (r->clip[j].x, r->clip[j].y, r->clip[j].w,
			       r->clip[j].h), clips);
      out = Fcons (list (make_fixnum (r->kind), make_fixnum (r->flags),
			 list4i (r->r.x, r->r.y, r->r.w, r->r.h),
			 make_uint (r->fg), make_uint (r->bg), clips),
		   out);
    }
  return out;
}

void
syms_of_nhmacdraw (void)
{
  defsubr (&Snhmac__test_draw_watch);
  defsubr (&Snhmac__test_draw_log);
}

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

/* Paint record R into F's backing under the clip in force.  */
static void
nhmac_emit (struct frame *f, NhgDrawRec *r)
{
  struct nhmac_clip *c = nhmac_top ();
  r->nclip = c->n;
  memcpy (r->clip, c->r, sizeof r->clip);
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

void
nhwl_clear_area (struct frame *f, int x, int y, int width, int height)
{
  nhwl_damage (f, x, y, width, height);
}

/* Stipples are not drawn yet: the face's background, as with no stipple.  */
void
nhwl_fill_background_by_face (struct frame *f, struct face *face, int x,
			      int y, int width, int height)
{
  nhwl_begin_cr_clip (f);
  nhmac_rect (f, NHG_DRAW_FILL, NHG_DRAW_COPY,
	      nhmac_argb (f, face->background, true), x, y, width, height,
	      0, 0);
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

/* The part of IMAGE from SRC_X, SRC_Y, WIDTH by HEIGHT, at DEST_X,
   DEST_Y, over GC's background unless OVERLAY_P (nhwldraw.c's Cairo
   twin).  */
void
nhwl_cr_draw_image (struct frame *f, Emacs_GC *gc, nhwl_image image,
		    int src_x, int src_y, int width, int height,
		    int dest_x, int dest_y, bool overlay_p)
{
  Emacs_Pixmap p = image->pixmap, m = image->mask;
  NhgPix pix = { (uint8_t *) p->data, m ? (uint8_t *) m->data : NULL,
		 p->width, p->height, p->bytes_per_line,
		 m ? m->bytes_per_line : 0, p->bits_per_pixel,
		 image->nhmac_smoothing };
  NhgDrawRec r = { NHG_DRAW_IMAGE, overlay_p ? NHG_DRAW_OVERLAY : 0,
		   { dest_x, dest_y, width, height },
		   nhmac_argb (f, gc->foreground, false),
		   nhmac_argb (f, gc->background, true), src_x, src_y };

  memcpy (pix.matrix, image->nhmac_transform, sizeof pix.matrix);
  nhwl_begin_cr_clip (f);
  r.nclip = nhmac_top ()->n;
  memcpy (r.clip, nhmac_top ()->r, sizeof r.clip);
  nhwl_glue_rc (FRAME_DISPLAY_INFO (f),
		NHG (nhg_draw_image (FRAME_NHG (f),
				     FRAME_NHWL_OUTPUT (f)->glitnir_id,
				     &r, &pix)));
  nhwl_end_cr_clip (f);
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
      NhgDrawRec r = { NHG_DRAW_BITMAP, p->overlay_p ? NHG_DRAW_OVERLAY : 0,
		       { p->x, p->y, p->wd, p->h },
		       nhmac_argb (f, fg, false),
		       nhmac_argb (f, face->background, false) };
      r.bits = p->bits + p->dh;
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

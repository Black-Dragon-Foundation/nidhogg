/* Drawing for Nidhogg's own Wayland shell (HAVE_NHWL), on the frame's
   retained backing.  Everything here is adapted from pgtkterm.c at
   v31.1-nidhogg.2 (each function keeps its pgtk name with the prefix
   changed), minus the GTK widget, the GSettings and the X paths.  Damage goes
   to Glitnir; nhwlterm.c composes and presents it.
   Copyright (C) 1989, 1993, 2005, 2008-2026 Free Software Foundation,
   Inc.

This file is part of GNU Emacs.

GNU Emacs is free software: you can redistribute it and/or modify
it under the terms of the GNU General Public License as published by
the Free Software Foundation, either version 3 of the License, or (at
your option) any later version.

GNU Emacs is distributed in the hope that it will be useful,
but WITHOUT ANY WARRANTY; without even the implied warranty of
MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
GNU General Public License for more details.

You should have received a copy of the GNU General Public License
along with GNU Emacs.  If not, see <https://www.gnu.org/licenses/>.  */

#include <config.h>

#include <cairo.h>
#include <math.h>
#include <string.h>
#include <time.h>

#include "lisp.h"
#include "nhwlterm.h"
#include "blockinput.h"
#include "frame.h"
#include "character.h"
#include "coding.h"
#include "composite.h"
#include "dispextern.h"
#include "fontset.h"
#include "font.h"
#include "keyboard.h"
#include "termhooks.h"
#include "window.h"

static void nhwl_fill_rectangle (struct frame *, unsigned long, int, int, int,
				 int, bool);
static void nhwl_clip_to_row (struct window *, struct glyph_row *,
			      enum glyph_row_area, cairo_t *);

/* Set S->gc to a suitable GC for drawing glyph string S in cursor
   face.  */

static void
nhwl_set_cursor_gc (struct glyph_string *s)
{
  if (s->font == FRAME_FONT (s->f)
      && s->face->background == FRAME_BACKGROUND_PIXEL (s->f)
      && s->face->foreground == FRAME_FOREGROUND_PIXEL (s->f) && !s->cmp)
      s->xgcv = FRAME_X_OUTPUT (s->f)->cursor_xgcv;
  else
    {
      /* Cursor on non-default face: must merge.  */
      Emacs_GC xgcv;

      xgcv.background = FRAME_X_OUTPUT (s->f)->cursor_color;
      xgcv.foreground = s->face->background;

      /* If the glyph would be invisible, try a different foreground.  */
      if (xgcv.foreground == xgcv.background)
	xgcv.foreground = s->face->foreground;
      if (xgcv.foreground == xgcv.background)
	xgcv.foreground = FRAME_X_OUTPUT (s->f)->cursor_foreground_color;
      if (xgcv.foreground == xgcv.background)
	xgcv.foreground = s->face->foreground;

      /* Make sure the cursor is distinct from text in this face.  */
      if (xgcv.background == s->face->background
	  && xgcv.foreground == s->face->foreground)
	{
	  xgcv.background = s->face->foreground;
	  xgcv.foreground = s->face->background;
	}

      s->xgcv = xgcv;
    }
}


/* Set up S->gc of glyph string S for drawing text in mouse face.  */

static void
nhwl_set_mouse_face_gc (struct glyph_string *s)
{
  prepare_face_for_display (s->f, s->face);

  if (s->font == s->face->font)
    {
      s->xgcv.foreground = s->face->foreground;
      s->xgcv.background = s->face->background;
    }
  else
    {
      /* Otherwise construct scratch_cursor_gc with values from FACE
         except for FONT.  */
      Emacs_GC xgcv;

      xgcv.background = s->face->background;
      xgcv.foreground = s->face->foreground;

      s->xgcv = xgcv;

    }
}


/* Set S->gc of glyph string S to a GC suitable for drawing a mode line.
   Faces to use in the mode line have already been computed when the
   matrix was built, so there isn't much to do, here.  */

static void
nhwl_set_mode_line_face_gc (struct glyph_string *s)
{
  s->xgcv.foreground = s->face->foreground;
  s->xgcv.background = s->face->background;
}


/* Set S->gc of glyph string S for drawing that glyph string.  Set
   S->stippled_p to a non-zero value if the face of S has a stipple
   pattern.  */

static void
nhwl_set_glyph_string_gc (struct glyph_string *s)
{
  prepare_face_for_display (s->f, s->face);

  if (s->hl == DRAW_NORMAL_TEXT)
    {
      s->xgcv.foreground = s->face->foreground;
      s->xgcv.background = s->face->background;
      s->stippled_p = s->face->stipple != 0;
    }
  else if (s->hl == DRAW_INVERSE_VIDEO)
    {
      nhwl_set_mode_line_face_gc (s);
      s->stippled_p = s->face->stipple != 0;
    }
  else if (s->hl == DRAW_CURSOR)
    {
      nhwl_set_cursor_gc (s);
      s->stippled_p = false;
    }
  else if (s->hl == DRAW_MOUSE_FACE)
    {
      nhwl_set_mouse_face_gc (s);
      s->stippled_p = s->face->stipple != 0;
    }
  else if (s->hl == DRAW_IMAGE_RAISED || s->hl == DRAW_IMAGE_SUNKEN)
    {
      s->xgcv.foreground = s->face->foreground;
      s->xgcv.background = s->face->background;
      s->stippled_p = s->face->stipple != 0;
    }
  else
    emacs_abort ();
}


/* Set clipping for output of glyph string S.  S may be part of a mode
   line or menu if we don't have X toolkit support.  */

static void
nhwl_set_glyph_string_clipping (struct glyph_string *s, cairo_t *cr)
{
  XRectangle r[2];
  int n = get_glyph_string_clip_rects (s, r, 2);

  if (n > 0)
    {
      for (int i = 0; i < n; i++)
	{
	  cairo_rectangle (cr, r[i].x, r[i].y, r[i].width, r[i].height);
	}
      cairo_clip (cr);
    }
}

/* Set SRC's clipping for output of glyph string DST.  This is called
   when we are drawing DST's left_overhang or right_overhang only in
   the area of SRC.  */

static void
nhwl_set_glyph_string_clipping_exactly (struct glyph_string *src,
					struct glyph_string *dst, cairo_t *cr)
{
  dst->clip[0].x = src->x;
  dst->clip[0].y = src->y;
  dst->clip[0].width = src->width;
  dst->clip[0].height = src->height;
  dst->num_clips = 1;

  cairo_rectangle (cr, src->x, src->y, src->width, src->height);
  cairo_clip (cr);
}

/* Fill rectangle X, Y, W, H with background color of glyph string
   S.  */
static void
nhwl_clear_glyph_string_rect (struct glyph_string *s, int x, int y,
			      int w, int h)
{
  nhwl_fill_rectangle (s->f, s->xgcv.background, x, y, w, h,
		       (s->first_glyph->type != STRETCH_GLYPH
			|| s->hl != DRAW_CURSOR));
}

static void
fill_background (struct glyph_string *s, int x, int y, int width, int height)
{
  nhwl_fill_background_by_face (s->f, s->face, x, y, width, height);
}

/* Draw the background of glyph_string S.  If S->background_filled_p
   is non-zero don't draw it.  FORCE_P non-zero means draw the
   background even if it wouldn't be drawn normally.  This is used
   when a string preceding S draws into the background of S, or S
   contains the first component of a composition.  */
static void
nhwl_draw_glyph_string_background (struct glyph_string *s, bool force_p)
{
  /* Nothing to do if background has already been drawn or if it
     shouldn't be drawn in the first place.  */
  if (!s->background_filled_p)
    {
      int box_line_width = max (s->face->box_horizontal_line_width, 0);

      if (s->stippled_p)
	{
	  /* Fill background with a stipple pattern.  */
	  fill_background (s, s->x, s->y + box_line_width,
			   s->background_width,
			   s->height - 2 * box_line_width);
	  s->background_filled_p = true;
	}
      else if (FONT_HEIGHT (s->font) < s->height - 2 * box_line_width
	       /* When xdisp.c ignores FONT_HEIGHT, we cannot trust
	          font dimensions, since the actual glyphs might be
	          much smaller.  So in that case we always clear the
	          rectangle with background color.  */
	       || FONT_TOO_HIGH (s->font)
	       || s->font_not_found_p
	       || s->extends_to_end_of_line_p || force_p)
	{
	  nhwl_clear_glyph_string_rect (s, s->x, s->y + box_line_width,
					s->background_width,
					s->height - 2 * box_line_width);
	  s->background_filled_p = true;
	}
    }
}


static void
nhwl_draw_rectangle (struct frame *f, unsigned long color, int x, int y,
		     int width, int height, bool respect_alpha_background)
{
  cairo_t *cr;

  cr = nhwl_begin_cr_clip (f);
  nhwl_set_cr_source_with_color (f, color, respect_alpha_background);
  cairo_rectangle (cr, x + 0.5, y + 0.5, width, height);
  cairo_set_line_width (cr, 1);
  cairo_stroke (cr);
  nhwl_end_cr_clip (f);
}

/* Draw the foreground of glyph string S.  */
static void
nhwl_draw_glyph_string_foreground (struct glyph_string *s)
{
  int i, x;

  /* If first glyph of S has a left box line, start drawing the text
     of S to the right of that box line.  */
  if (s->face->box != FACE_NO_BOX && s->first_glyph->left_box_line_p)
    x = s->x + max (s->face->box_vertical_line_width, 0);
  else
    x = s->x;

  /* Draw characters of S as rectangles if S's font could not be
     loaded.  */
  if (s->font_not_found_p)
    {
      for (i = 0; i < s->nchars; ++i)
	{
	  struct glyph *g = s->first_glyph + i;
	  nhwl_draw_rectangle (s->f,
			       s->face->foreground, x, s->y,
			       g->pixel_width - 1, s->height - 1,
			       false);
	  x += g->pixel_width;
	}
    }
  else
    {
      struct font *font = s->font;
      int boff = font->baseline_offset;
      int y;

      if (font->vertical_centering)
	boff = VCENTER_BASELINE_OFFSET (font, s->f) - boff;

      y = s->ybase - boff;
      if (s->for_overlaps || (s->background_filled_p && s->hl != DRAW_CURSOR))
	font->driver->draw (s, 0, s->nchars, x, y, false);
      else
	font->driver->draw (s, 0, s->nchars, x, y, true);
      if (s->face->overstrike)
	font->driver->draw (s, 0, s->nchars, x + 1, y, false);
    }
}

/* Draw the foreground of composite glyph string S.  */
static void
nhwl_draw_composite_glyph_string_foreground (struct glyph_string *s)
{
  int i, j, x;
  struct font *font = s->font;

  /* If first glyph of S has a left box line, start drawing the text
     of S to the right of that box line.  */
  if (s->face && s->face->box != FACE_NO_BOX
      && s->first_glyph->left_box_line_p)
    x = s->x + max (s->face->box_vertical_line_width, 0);
  else
    x = s->x;

  /* S is a glyph string for a composition.  S->cmp_from is the index
     of the first character drawn for glyphs of this composition.
     S->cmp_from == 0 means we are drawing the very first character of
     this composition.  */

  /* Draw a rectangle for the composition if the font for the very
     first character of the composition could not be loaded.  */
  if (s->font_not_found_p)
    {
      if (s->cmp_from == 0)
	nhwl_draw_rectangle (s->f, s->face->foreground, x, s->y,
			     s->width - 1, s->height - 1, false);
    }
  else if (!s->first_glyph->u.cmp.automatic)
    {
      int y = s->ybase;

      for (i = 0, j = s->cmp_from; i < s->nchars; i++, j++)
	/* TAB in a composition means display glyphs with padding
	   space on the left or right.  */
	if (COMPOSITION_GLYPH (s->cmp, j) != '\t')
	  {
	    int xx = x + s->cmp->offsets[j * 2];
	    int yy = y - s->cmp->offsets[j * 2 + 1];

	    font->driver->draw (s, j, j + 1, xx, yy, false);
	    if (s->face->overstrike)
	      font->driver->draw (s, j, j + 1, xx + 1, yy, false);
	  }
    }
  else
    {
      Lisp_Object gstring = composition_gstring_from_id (s->cmp_id);
      Lisp_Object glyph;
      int y = s->ybase;
      int width = 0;

      for (i = j = s->cmp_from; i < s->cmp_to; i++)
	{
	  glyph = LGSTRING_GLYPH (gstring, i);
	  if (NILP (LGLYPH_ADJUSTMENT (glyph)))
	    width += LGLYPH_WIDTH (glyph);
	  else
	    {
	      int xoff, yoff, wadjust;

	      if (j < i)
		{
		  font->driver->draw (s, j, i, x, y, false);
		  if (s->face->overstrike)
		    font->driver->draw (s, j, i, x + 1, y, false);
		  x += width;
		}
	      xoff = LGLYPH_XOFF (glyph);
	      yoff = LGLYPH_YOFF (glyph);
	      wadjust = LGLYPH_WADJUST (glyph);
	      font->driver->draw (s, i, i + 1, x + xoff, y + yoff, false);
	      if (s->face->overstrike)
		font->driver->draw (s, i, i + 1, x + xoff + 1, y + yoff,
				    false);
	      x += wadjust;
	      j = i + 1;
	      width = 0;
	    }
	}
      if (j < i)
	{
	  font->driver->draw (s, j, i, x, y, false);
	  if (s->face->overstrike)
	    font->driver->draw (s, j, i, x + 1, y, false);
	}
    }
}


/* Draw the foreground of glyph string S for glyphless characters.  */
static void
nhwl_draw_glyphless_glyph_string_foreground (struct glyph_string *s)
{
  struct glyph *glyph = s->first_glyph;
  unsigned char2b[8];
  int x, i, j;

  /* If first glyph of S has a left box line, start drawing the text
     of S to the right of that box line.  */
  if (s->face && s->face->box != FACE_NO_BOX
      && s->first_glyph->left_box_line_p)
    x = s->x + max (s->face->box_vertical_line_width, 0);
  else
    x = s->x;

  s->char2b = char2b;

  for (i = 0; i < s->nchars; i++, glyph++)
    {
#ifdef GCC_LINT
      enum
      { PACIFY_GCC_BUG_81401 = 1 };
#else
      enum
      { PACIFY_GCC_BUG_81401 = 0 };
#endif
      char buf[7 + PACIFY_GCC_BUG_81401];
      char *str = NULL;
      int len = glyph->u.glyphless.len;

      if (glyph->u.glyphless.method == GLYPHLESS_DISPLAY_ACRONYM)
	{
	  if (len > 0
	      && CHAR_TABLE_P (Vglyphless_char_display)
	      &&
	      (CHAR_TABLE_EXTRA_SLOTS (XCHAR_TABLE (Vglyphless_char_display))
	       >= 1))
	    {
	      Lisp_Object acronym
		= (!glyph->u.glyphless.for_no_font
		   ? CHAR_TABLE_REF (Vglyphless_char_display,
				     glyph->u.glyphless.ch)
		   : XCHAR_TABLE (Vglyphless_char_display)->extras[0]);
	      if (CONSP (acronym))
		acronym = XCAR (acronym);
	      if (STRINGP (acronym))
		str = SSDATA (acronym);
	    }
	}
      else if (glyph->u.glyphless.method == GLYPHLESS_DISPLAY_HEX_CODE)
	{
	  unsigned int ch = glyph->u.glyphless.ch;
	  eassume (ch <= MAX_CHAR);
	  sprintf (buf, "%0*X", ch < 0x10000 ? 4 : 6, ch);
	  str = buf;
	}

      if (str)
	{
	  int upper_len = (len + 1) / 2;

	  /* It is assured that all LEN characters in STR is ASCII.  */
	  for (j = 0; j < len; j++)
	    char2b[j]
	      = s->font->driver->encode_char (s->font, str[j]) & 0xFFFF;
	  s->font->driver->draw (s, 0, upper_len,
				 x + glyph->slice.glyphless.upper_xoff,
				 s->ybase + glyph->slice.glyphless.upper_yoff,
				 false);
	  s->font->driver->draw (s, upper_len, len,
				 x + glyph->slice.glyphless.lower_xoff,
				 s->ybase + glyph->slice.glyphless.lower_yoff,
				 false);
	}
      if (glyph->u.glyphless.method != GLYPHLESS_DISPLAY_THIN_SPACE)
	nhwl_draw_rectangle (s->f, s->face->foreground,
			     x, s->ybase - glyph->ascent,
			     glyph->pixel_width - 1,
			     glyph->ascent + glyph->descent - 1,
			     false);
      x += glyph->pixel_width;
    }

  /* Pacify GCC 12 even though s->char2b is not used after this
     function returns.  */
  s->char2b = NULL;
}

/* Brightness beyond which a color won't have its highlight brightness
   boosted.

   Nominally, highlight colors for `3d' faces are calculated by
   brightening an object's color by a constant scale factor, but this
   doesn't yield good results for dark colors, so for colors who's
   brightness is less than this value (on a scale of 0-65535) have an
   use an additional additive factor.

   The value here is set so that the default menu-bar/mode-line color
   (grey75) will not have its highlights changed at all.  */
#define HIGHLIGHT_COLOR_DARK_BOOST_LIMIT 48000


/* Compute a color which is lighter or darker than *PIXEL by FACTOR or
   DELTA.  Try a color with RGB values multiplied by FACTOR first.  If
   this produces the same color as PIXEL, try a color where all RGB
   values have DELTA added.  Return the computed color in *PIXEL.  F
   is the frame to act on.  */

static void
nhwl_compute_lighter_color (struct frame *f, unsigned long *pixel,
			    double factor, int delta)
{
  Emacs_Color color, new;
  long bright;

  /* Get RGB color values.  */
  color.pixel = *pixel;
  nhwl_query_color (f, &color);

  /* Change RGB values by specified FACTOR.  Avoid overflow!  */
  eassert (factor >= 0);
  new.red = min (0xffff, factor * color.red);
  new.green = min (0xffff, factor * color.green);
  new.blue = min (0xffff, factor * color.blue);

  /* Calculate brightness of COLOR.  */
  bright = (2 * color.red + 3 * color.green + color.blue) / 6;

  /* We only boost colors that are darker than
     HIGHLIGHT_COLOR_DARK_BOOST_LIMIT.  */
  if (bright < HIGHLIGHT_COLOR_DARK_BOOST_LIMIT)
    /* Make an additive adjustment to NEW, because it's dark enough so
       that scaling by FACTOR alone isn't enough.  */
    {
      /* How far below the limit this color is (0 - 1, 1 being darker).  */
      double dimness = 1 - (double) bright / HIGHLIGHT_COLOR_DARK_BOOST_LIMIT;
      /* The additive adjustment.  */
      int min_delta = delta * dimness * factor / 2;

      if (factor < 1)
	{
	  new.red = max (0, new.red - min_delta);
	  new.green = max (0, new.green - min_delta);
	  new.blue = max (0, new.blue - min_delta);
	}
      else
	{
	  new.red = min (0xffff, min_delta + new.red);
	  new.green = min (0xffff, min_delta + new.green);
	  new.blue = min (0xffff, min_delta + new.blue);
	}
    }

  new.pixel = (new.red >> 8 << 16
	       | new.green >> 8 << 8
	       | new.blue >> 8);

  if (new.pixel == *pixel)
    {
      /* If we end up with the same color as before, try adding
	 delta to the RGB values.  */
      new.red = min (0xffff, delta + color.red);
      new.green = min (0xffff, delta + color.green);
      new.blue = min (0xffff, delta + color.blue);
      new.pixel = (new.red >> 8 << 16
		   | new.green >> 8 << 8
		   | new.blue >> 8);
    }

  *pixel = new.pixel;
}

static void
nhwl_fill_trapezoid_for_relief (struct frame *f, unsigned long color, int x,
				int y, int width, int height, int top_p)
{
  cairo_t *cr;

  cr = nhwl_begin_cr_clip (f);
  nhwl_set_cr_source_with_color (f, color, false);
  cairo_move_to (cr, top_p ? x : x + height, y);
  cairo_line_to (cr, x, y + height);
  cairo_line_to (cr, top_p ? x + width - height : x + width, y + height);
  cairo_line_to (cr, x + width, y);
  cairo_fill (cr);
  nhwl_end_cr_clip (f);
}

enum corners
{
  CORNER_BOTTOM_RIGHT,		/* 0 -> pi/2 */
  CORNER_BOTTOM_LEFT,		/* pi/2 -> pi */
  CORNER_TOP_LEFT,		/* pi -> 3pi/2 */
  CORNER_TOP_RIGHT,		/* 3pi/2 -> 2pi */
  CORNER_LAST
};

static void
nhwl_erase_corners_for_relief (struct frame *f, unsigned long color, int x,
			       int y, int width, int height, double radius,
			       double margin, int corners)
{
  cairo_t *cr;
  int i;

  cr = nhwl_begin_cr_clip (f);
  nhwl_set_cr_source_with_color (f, color, false);
  for (i = 0; i < CORNER_LAST; i++)
    if (corners & (1 << i))
      {
	double xm, ym, xc, yc;

	if (i == CORNER_TOP_LEFT || i == CORNER_BOTTOM_LEFT)
	  xm = x - margin, xc = xm + radius;
	else
	  xm = x + width + margin, xc = xm - radius;
	if (i == CORNER_TOP_LEFT || i == CORNER_TOP_RIGHT)
	  ym = y - margin, yc = ym + radius;
	else
	  ym = y + height + margin, yc = ym - radius;

	cairo_move_to (cr, xm, ym);
	cairo_arc (cr, xc, yc, radius, i * M_PI_2, (i + 1) * M_PI_2);
      }
  cairo_clip (cr);
  cairo_rectangle (cr, x, y, width, height);
  cairo_fill (cr);
  nhwl_end_cr_clip (f);
}

static void
nhwl_setup_relief_color (struct frame *f, struct relief *relief, double factor,
			 int delta, unsigned long default_pixel)
{
  Emacs_GC xgcv;
  struct nhwl_output *di = FRAME_X_OUTPUT (f);
  unsigned long pixel;
  unsigned long background = di->relief_background;

  /* Allocate new color.  */
  xgcv.foreground = default_pixel;
  pixel = background;
  nhwl_compute_lighter_color (f, &pixel, factor, delta);
  xgcv.foreground = relief->pixel = pixel;

  relief->xgcv = xgcv;
}

/* Set up colors for the relief lines around glyph string S.  */
static void
nhwl_setup_relief_colors (struct glyph_string *s)
{
  struct nhwl_output *di = FRAME_X_OUTPUT (s->f);
  unsigned long color;

  if (s->face->use_box_color_for_shadows_p)
    color = s->face->box_color;
  else if (s->first_glyph->type == IMAGE_GLYPH
	   && s->img->pixmap
	   && !IMAGE_BACKGROUND_TRANSPARENT (s->img, s->f, 0))
    color = IMAGE_BACKGROUND (s->img, s->f, 0);
  else
    {
      /* Get the background color of the face.  */
      color = s->xgcv.background;
    }

  if (!di->relief_background_valid_p
      || di->relief_background != color)
    {
      di->relief_background_valid_p = true;
      di->relief_background = color;
      nhwl_setup_relief_color (s->f, &di->white_relief, 1.2, 0x8000,
			       WHITE_PIX_DEFAULT (s->f));
      nhwl_setup_relief_color (s->f, &di->black_relief, 0.6, 0x4000,
			       BLACK_PIX_DEFAULT (s->f));
    }
}

static void
nhwl_set_clip_rectangles (struct frame *f, cairo_t *cr,
			  XRectangle *rectangles, int n)
{
  if (n > 0)
    {
      for (int i = 0; i < n; i++)
	cairo_rectangle (cr, rectangles[i].x, rectangles[i].y,
			 rectangles[i].width, rectangles[i].height);
      cairo_clip (cr);
    }
}

/* Draw a relief on frame F inside the rectangle given by LEFT_X,
   TOP_Y, RIGHT_X, and BOTTOM_Y.  WIDTH is the thickness of the relief
   to draw, it must be >= 0.  RAISED_P means draw a raised
   relief.  LEFT_P means draw a relief on the left side of
   the rectangle.  RIGHT_P means draw a relief on the right
   side of the rectangle.  CLIP_RECT is the clipping rectangle to use
   when drawing.  */

static void
nhwl_draw_relief_rect (struct frame *f,
		       int left_x, int top_y, int right_x, int bottom_y,
		       int hwidth, int vwidth, bool raised_p, bool top_p,
		       bool bot_p, bool left_p, bool right_p,
		       XRectangle *clip_rect)
{
  unsigned long top_left_color, bottom_right_color;
  int corners = 0;

  cairo_t *cr = nhwl_begin_cr_clip (f);

  if (raised_p)
    {
      top_left_color = FRAME_X_OUTPUT (f)->white_relief.xgcv.foreground;
      bottom_right_color = FRAME_X_OUTPUT (f)->black_relief.xgcv.foreground;
    }
  else
    {
      top_left_color = FRAME_X_OUTPUT (f)->black_relief.xgcv.foreground;
      bottom_right_color = FRAME_X_OUTPUT (f)->white_relief.xgcv.foreground;
    }

  nhwl_set_clip_rectangles (f, cr, clip_rect, 1);

  if (left_p)
    {
      nhwl_fill_rectangle (f, top_left_color, left_x, top_y,
			   vwidth, bottom_y + 1 - top_y, false);
      if (top_p)
	corners |= 1 << CORNER_TOP_LEFT;
      if (bot_p)
	corners |= 1 << CORNER_BOTTOM_LEFT;
    }
  if (right_p)
    {
      nhwl_fill_rectangle (f, bottom_right_color, right_x + 1 - vwidth, top_y,
			   vwidth, bottom_y + 1 - top_y, false);
      if (top_p)
	corners |= 1 << CORNER_TOP_RIGHT;
      if (bot_p)
	corners |= 1 << CORNER_BOTTOM_RIGHT;
    }
  if (top_p)
    {
      if (!right_p)
	nhwl_fill_rectangle (f, top_left_color, left_x, top_y,
			     right_x + 1 - left_x, hwidth, false);
      else
	nhwl_fill_trapezoid_for_relief (f, top_left_color, left_x, top_y,
					right_x + 1 - left_x, hwidth, 1);
    }
  if (bot_p)
    {
      if (!left_p)
	nhwl_fill_rectangle (f, bottom_right_color, left_x,
			     bottom_y + 1 - hwidth, right_x + 1 - left_x,
			     hwidth, false);
      else
	nhwl_fill_trapezoid_for_relief (f, bottom_right_color,
					left_x, bottom_y + 1 - hwidth,
					right_x + 1 - left_x, hwidth, 0);
    }
  if (left_p && vwidth > 1)
    nhwl_fill_rectangle (f, bottom_right_color, left_x, top_y,
			 1, bottom_y + 1 - top_y, false);
  if (top_p && hwidth > 1)
    nhwl_fill_rectangle (f, bottom_right_color, left_x, top_y,
			 right_x + 1 - left_x, 1, false);
  if (corners)
    nhwl_erase_corners_for_relief (f, FRAME_BACKGROUND_PIXEL (f), left_x,
				   top_y, right_x - left_x + 1,
				   bottom_y - top_y + 1, 6, 1, corners);

  nhwl_end_cr_clip (f);
}

/* Draw a box on frame F inside the rectangle given by LEFT_X, TOP_Y,
   RIGHT_X, and BOTTOM_Y.  WIDTH is the thickness of the lines to
   draw, it must be >= 0.  LEFT_P means draw a line on the
   left side of the rectangle.  RIGHT_P means draw a line
   on the right side of the rectangle.  CLIP_RECT is the clipping
   rectangle to use when drawing.  */

static void
nhwl_draw_box_rect (struct glyph_string *s, int left_x,
		    int top_y, int right_x, int bottom_y, int hwidth,
		    int vwidth, bool left_p, bool right_p,
		    XRectangle * clip_rect)
{
  unsigned long foreground_backup;

  cairo_t *cr = nhwl_begin_cr_clip (s->f);

  foreground_backup = s->xgcv.foreground;
  s->xgcv.foreground = s->face->box_color;

  nhwl_set_clip_rectangles (s->f, cr, clip_rect, 1);

  /* Top.  */
  nhwl_fill_rectangle (s->f, s->xgcv.foreground,
		       left_x, top_y, right_x - left_x + 1, hwidth,
		       false);

  /* Left.  */
  if (left_p)
    nhwl_fill_rectangle (s->f, s->xgcv.foreground,
			 left_x, top_y, vwidth, bottom_y - top_y + 1,
			 false);

  /* Bottom.  */
  nhwl_fill_rectangle (s->f, s->xgcv.foreground,
		       left_x, bottom_y - hwidth + 1, right_x - left_x + 1,
		       hwidth, false);

  /* Right.  */
  if (right_p)
    nhwl_fill_rectangle (s->f, s->xgcv.foreground,
			 right_x - vwidth + 1, top_y, vwidth,
			 bottom_y - top_y + 1, false);

  s->xgcv.foreground = foreground_backup;

  nhwl_end_cr_clip (s->f);
}


/* Draw a box around glyph string S.  */

static void
nhwl_draw_glyph_string_box (struct glyph_string *s)
{
  int hwidth, vwidth, left_x, right_x, top_y, bottom_y, last_x;
  bool raised_p, left_p, right_p;
  struct glyph *last_glyph;
  XRectangle clip_rect;

  last_x = ((s->row->full_width_p && !s->w->pseudo_window_p)
	    ? WINDOW_RIGHT_EDGE_X (s->w) : window_box_right (s->w, s->area));

  /* The glyph that may have a right box line.  */
  last_glyph = (s->cmp || s->img
		? s->first_glyph : s->first_glyph + s->nchars - 1);

  vwidth = eabs (s->face->box_vertical_line_width);
  hwidth = eabs (s->face->box_horizontal_line_width);
  raised_p = s->face->box == FACE_RAISED_BOX;
  left_x = s->x;
  right_x = (s->row->full_width_p && s->extends_to_end_of_line_p
	     ? last_x - 1 : min (last_x, s->x + s->background_width) - 1);
  top_y = s->y;
  bottom_y = top_y + s->height - 1;

  left_p = (s->first_glyph->left_box_line_p
	    || (s->hl == DRAW_MOUSE_FACE
		&& (s->prev == NULL || s->prev->hl != s->hl)));
  right_p = (last_glyph->right_box_line_p
	     || (s->hl == DRAW_MOUSE_FACE
		 && (s->next == NULL || s->next->hl != s->hl)));

  get_glyph_string_clip_rect (s, &clip_rect);

  /* PD's keys (:corner-radius, :open): Glitnir's box.  */
  if (s->face->box != FACE_SIMPLE_BOX)
    nhwl_setup_relief_colors (s);
  if (glitnir_decor_draw_box (s, left_x, top_y, right_x, bottom_y, hwidth,
			      vwidth, left_p, right_p, &clip_rect))
    return;
  if (s->face->box == FACE_SIMPLE_BOX)
    nhwl_draw_box_rect (s, left_x, top_y, right_x, bottom_y, hwidth,
			vwidth, left_p, right_p, &clip_rect);
  else
    {
      nhwl_setup_relief_colors (s);
      nhwl_draw_relief_rect (s->f, left_x, top_y, right_x, bottom_y, hwidth,
			     vwidth, raised_p, true, true, left_p, right_p,
			     &clip_rect);
    }
}

static void
nhwl_draw_horizontal_wave (struct frame *f, unsigned long color, int x, int y,
			   int width, int height, int wave_length)
{
  cairo_t *cr;
  double dx = wave_length, dy = height - 1;
  int xoffset, n;

  cr = nhwl_begin_cr_clip (f);
  nhwl_set_cr_source_with_color (f, color, false);
  cairo_rectangle (cr, x, y, width, height);
  cairo_clip (cr);

  if (x >= 0)
    {
      xoffset = x % (wave_length * 2);
      if (xoffset == 0)
	xoffset = wave_length * 2;
    }
  else
    xoffset = x % (wave_length * 2) + wave_length * 2;
  n = (width + xoffset) / wave_length + 1;
  if (xoffset > wave_length)
    {
      xoffset -= wave_length;
      --n;
      y += height - 1;
      dy = -dy;
    }

  cairo_move_to (cr, x - xoffset + 0.5, y + 0.5);
  while (--n >= 0)
    {
      cairo_rel_line_to (cr, dx, dy);
      dy = -dy;
    }
  cairo_set_line_width (cr, 1);
  cairo_stroke (cr);
  nhwl_end_cr_clip (f);
}

static void
nhwl_draw_underwave (struct glyph_string *s, unsigned long color)
{
  int wave_height = 3, wave_length = 2;

  nhwl_draw_horizontal_wave (s->f, color, s->x, s->ybase - wave_height + 3,
			     s->width, wave_height, wave_length);
}

/* Draw a relief around the image glyph string S.  */

static void
nhwl_draw_image_relief (struct glyph_string *s)
{
  int x1, y1, thick;
  bool raised_p, top_p, bot_p, left_p, right_p;
  int extra_x, extra_y;
  XRectangle r;
  int x = s->x;
  int y = s->ybase - image_ascent (s->img, s->face, &s->slice);

  /* If first glyph of S has a left box line, start drawing it to the
     right of that line.  */
  if (s->face->box != FACE_NO_BOX
      && s->first_glyph->left_box_line_p
      && s->slice.x == 0)
    x += max (s->face->box_vertical_line_width, 0);

  /* If there is a margin around the image, adjust x- and y-position
     by that margin.  */
  if (s->slice.x == 0)
    x += s->img->hmargin;
  if (s->slice.y == 0)
    y += s->img->vmargin;

  if (s->hl == DRAW_IMAGE_SUNKEN
      || s->hl == DRAW_IMAGE_RAISED)
    {
      if (s->face->id == TAB_BAR_FACE_ID)
	thick = (tab_bar_button_relief < 0
		 ? DEFAULT_TAB_BAR_BUTTON_RELIEF
		 : min (tab_bar_button_relief, 1000000));
      else
	thick = (tool_bar_button_relief < 0
		 ? DEFAULT_TOOL_BAR_BUTTON_RELIEF
		 : min (tool_bar_button_relief, 1000000));
      raised_p = s->hl == DRAW_IMAGE_RAISED;
    }
  else
    {
      thick = eabs (s->img->relief);
      raised_p = s->img->relief > 0;
    }

  x1 = x + s->slice.width - 1;
  y1 = y + s->slice.height - 1;

  extra_x = extra_y = 0;
  if (s->face->id == TAB_BAR_FACE_ID)
    {
      if (CONSP (Vtab_bar_button_margin)
	  && FIXNUMP (XCAR (Vtab_bar_button_margin))
	  && FIXNUMP (XCDR (Vtab_bar_button_margin)))
	{
	  extra_x = XFIXNUM (XCAR (Vtab_bar_button_margin)) - thick;
	  extra_y = XFIXNUM (XCDR (Vtab_bar_button_margin)) - thick;
	}
      else if (FIXNUMP (Vtab_bar_button_margin))
	extra_x = extra_y = XFIXNUM (Vtab_bar_button_margin) - thick;
    }

  if (s->face->id == TOOL_BAR_FACE_ID)
    {
      if (CONSP (Vtool_bar_button_margin)
	  && FIXNUMP (XCAR (Vtool_bar_button_margin))
	  && FIXNUMP (XCDR (Vtool_bar_button_margin)))
	{
	  extra_x = XFIXNUM (XCAR (Vtool_bar_button_margin));
	  extra_y = XFIXNUM (XCDR (Vtool_bar_button_margin));
	}
      else if (FIXNUMP (Vtool_bar_button_margin))
	extra_x = extra_y = XFIXNUM (Vtool_bar_button_margin);
    }

  top_p = bot_p = left_p = right_p = false;

  if (s->slice.x == 0)
    x -= thick + extra_x, left_p = true;
  if (s->slice.y == 0)
    y -= thick + extra_y, top_p = true;
  if (s->slice.x + s->slice.width == s->img->width)
    x1 += thick + extra_x, right_p = true;
  if (s->slice.y + s->slice.height == s->img->height)
    y1 += thick + extra_y, bot_p = true;

  nhwl_setup_relief_colors (s);
  get_glyph_string_clip_rect (s, &r);
  nhwl_draw_relief_rect (s->f, x, y, x1, y1, thick, thick, raised_p,
			 top_p, bot_p, left_p, right_p, &r);
}

/* Draw part of the background of glyph string S.  X, Y, W, and H
   give the rectangle to draw.  */

static void
nhwl_draw_glyph_string_bg_rect (struct glyph_string *s, int x, int y, int w,
				int h)
{
  if (s->stippled_p)
    fill_background (s, x, y, w, h);
  else
    nhwl_clear_glyph_string_rect (s, x, y, w, h);
}

static void
nhwl_cr_draw_image (struct frame *f, Emacs_GC *gc, cairo_pattern_t *image,
		    int src_x, int src_y, int width, int height,
		    int dest_x, int dest_y, bool overlay_p)
{
  cairo_t *cr = nhwl_begin_cr_clip (f);

  if (overlay_p)
    cairo_rectangle (cr, dest_x, dest_y, width, height);
  else
    {
      nhwl_set_cr_source_with_gc_background (f, gc, false);
      cairo_rectangle (cr, dest_x, dest_y, width, height);
      cairo_fill_preserve (cr);
    }

  cairo_translate (cr, dest_x - src_x, dest_y - src_y);

  cairo_surface_t *surface;
  cairo_pattern_get_surface (image, &surface);
  cairo_format_t format = cairo_image_surface_get_format (surface);
  if (format != CAIRO_FORMAT_A8 && format != CAIRO_FORMAT_A1)
    {
      cairo_set_source (cr, image);
      cairo_fill (cr);
    }
  else
    {
      nhwl_set_cr_source_with_gc_foreground (f, gc, false);
      cairo_clip (cr);
      cairo_mask (cr, image);
    }

  nhwl_end_cr_clip (f);
}

/* Draw foreground of image glyph string S.  */

static void
nhwl_draw_image_foreground (struct glyph_string *s)
{
  int x = s->x;
  int y = s->ybase - image_ascent (s->img, s->face, &s->slice);

  /* If first glyph of S has a left box line, start drawing it to the
     right of that line.  */
  if (s->face->box != FACE_NO_BOX
      && s->first_glyph->left_box_line_p
      && s->slice.x == 0)
    x += max (s->face->box_vertical_line_width, 0);

  /* If there is a margin around the image, adjust x- and y-position
     by that margin.  */
  if (s->slice.x == 0)
    x += s->img->hmargin;
  if (s->slice.y == 0)
    y += s->img->vmargin;

  if (s->img->cr_data)
    {
      cairo_t *cr = nhwl_begin_cr_clip (s->f);
      nhwl_set_glyph_string_clipping (s, cr);
      nhwl_cr_draw_image (s->f, &s->xgcv, s->img->cr_data,
			  s->slice.x, s->slice.y, s->slice.width, s->slice.height,
			  x, y, true);
      if (!s->img->mask)
	{
	  /* When the image has a mask, we can expect that at
	     least part of a mouse highlight or a block cursor will
	     be visible.  If the image doesn't have a mask, make
	     a block cursor visible by drawing a rectangle around
	     the image.  I believe it's looking better if we do
	     nothing here for mouse-face.  */
	  if (s->hl == DRAW_CURSOR)
	    {
	      int relief = eabs (s->img->relief);
	      nhwl_draw_rectangle (s->f, s->xgcv.foreground, x - relief,
				   y - relief, s->slice.width + relief * 2 - 1,
				   s->slice.height + relief * 2 - 1, false);
	    }
	}
      nhwl_end_cr_clip (s->f);
    }
  else
    /* Draw a rectangle if image could not be loaded.  */
    nhwl_draw_rectangle (s->f, s->xgcv.foreground, x, y,
			 s->slice.width - 1, s->slice.height - 1, false);
}

/* Draw image glyph string S.

            s->y
   s->x      +-------------------------
	     |   s->face->box
	     |
	     |     +-------------------------
	     |     |  s->img->margin
	     |     |
	     |     |       +-------------------
	     |     |       |  the image

 */

static void
nhwl_draw_image_glyph_string (struct glyph_string *s)
{
  int box_line_hwidth = max (s->face->box_vertical_line_width, 0);
  int box_line_vwidth = max (s->face->box_horizontal_line_width, 0);
  int height;

  height = s->height;
  if (s->slice.y == 0)
    height -= box_line_vwidth;
  if (s->slice.y + s->slice.height >= s->img->height)
    height -= box_line_vwidth;

  /* Fill background with face under the image.  Do it only if row is
     taller than image or if image has a clip mask to reduce
     flickering.  */
  s->stippled_p = s->face->stipple != 0;
  if (height > s->slice.height
      || s->img->hmargin
      || s->img->vmargin
      || s->img->mask
      || s->img->pixmap == 0
      || s->width != s->background_width)
    {
      int x = s->x;
      int y = s->y;
      int width = s->background_width;

      if (s->first_glyph->left_box_line_p
	  && s->slice.x == 0)
	{
	  x += box_line_hwidth;
	  width -= box_line_hwidth;
	}

      if (s->slice.y == 0)
	y += box_line_vwidth;

      nhwl_draw_glyph_string_bg_rect (s, x, y, width, height);

      s->background_filled_p = true;
    }

  /* Draw the foreground.  */
  nhwl_draw_image_foreground (s);

  /* If we must draw a relief around the image, do it.  */
  if (s->img->relief
      || s->hl == DRAW_IMAGE_RAISED
      || s->hl == DRAW_IMAGE_SUNKEN)
    nhwl_draw_image_relief (s);
}

/* Draw stretch glyph string S.  */

static void
nhwl_draw_stretch_glyph_string (struct glyph_string *s)
{
  eassert (s->first_glyph->type == STRETCH_GLYPH);

  if (s->hl == DRAW_CURSOR && !x_stretch_cursor_p)
    {
      /* If `x-stretch-cursor' is nil, don't draw a block cursor as
         wide as the stretch glyph.  */
      int width, background_width = s->background_width;
      int x = s->x;

      if (!s->row->reversed_p)
	{
	  int left_x = window_box_left_offset (s->w, TEXT_AREA);

	  if (x < left_x)
	    {
	      background_width -= left_x - x;
	      x = left_x;
	    }
	}
      else
	{
	  /* In R2L rows, draw the cursor on the right edge of the
	     stretch glyph.  */
	  int right_x = window_box_right (s->w, TEXT_AREA);

	  if (x + background_width > right_x)
	    background_width -= x - right_x;
	  x += background_width;
	}
      width = min (FRAME_COLUMN_WIDTH (s->f), background_width);
      if (s->row->reversed_p)
	x -= width;

      /* Draw cursor.  */
      nhwl_draw_glyph_string_bg_rect (s, x, s->y, width, s->height);

      /* Clear rest using the GC of the original non-cursor face.  */
      if (width < background_width)
	{
	  int y = s->y;
	  int w = background_width - width, h = s->height;
	  XRectangle r;
	  unsigned long color;

	  if (!s->row->reversed_p)
	    x += width;
	  else
	    x = s->x;
	  if (s->row->mouse_face_p && cursor_in_mouse_face_p (s->w))
	    {
	      nhwl_set_mouse_face_gc (s);
	      color = s->xgcv.foreground;
	    }
	  else
	    color = s->face->background;

	  cairo_t *cr = nhwl_begin_cr_clip (s->f);

	  get_glyph_string_clip_rect (s, &r);
	  nhwl_set_clip_rectangles (s->f, cr, &r, 1);

	  if (s->face->stipple)
	    fill_background (s, x, y, w, h);
	  else
	    nhwl_fill_rectangle (s->f, color, x, y, w, h,
				 true);

	  nhwl_end_cr_clip (s->f);
	}
    }
  else if (!s->background_filled_p)
    {
      int background_width = s->background_width;
      int x = s->x, text_left_x = window_box_left (s->w, TEXT_AREA);

      /* Don't draw into left fringe or scrollbar area except for
	 header line and mode line.  */
      if (s->area == TEXT_AREA
	  && x < text_left_x && !s->row->mode_line_p)
	{
	  background_width -= text_left_x - x;
	  x = text_left_x;
	}

      if (background_width > 0)
	nhwl_draw_glyph_string_bg_rect (s, x, s->y, background_width, s->height);
    }

  s->background_filled_p = true;
}


/* Draw a dashed underline of thickness THICKNESS and width WIDTH onto F
   at a vertical offset of OFFSET from the position of the glyph string
   S, with each segment SEGMENT pixels in length.  */

static void
nhwl_draw_dash (struct frame *f, struct glyph_string *s,
		unsigned long foreground, int width,
		char segment, int offset, int thickness)
{
  cairo_t *cr;
  double cr_segment, y_center;

  cr = nhwl_begin_cr_clip (s->f);
  nhwl_set_cr_source_with_color (f, foreground, false);
  cr_segment = (double) segment;
  y_center = s->ybase + offset + (thickness / 2.0);

  cairo_set_dash (cr, &cr_segment, 1, s->x);
  cairo_set_line_width (cr, thickness);
  cairo_move_to (cr, s->x, y_center);
  cairo_line_to (cr, s->x + width, y_center);
  cairo_stroke (cr);
  nhwl_end_cr_clip (f);
}

/* Draw an underline of STYLE onto F at an offset of POSITION from the
   baseline of the glyph string S in the color provided by FOREGROUND,
   DECORATION_WIDTH in length, and THICKNESS in height.  */

static void
nhwl_fill_underline (struct frame *f, struct glyph_string *s,
		     unsigned long foreground,
		     enum face_underline_type style, int position,
		     int decoration_width, int thickness)
{
  int segment;

  segment = thickness * 3;

  switch (style)
    {
      /* FACE_UNDERLINE_DOUBLE_LINE is treated identically to SINGLE, as
	 the second line will be filled by another invocation of this
	 function.  */
    case FACE_UNDERLINE_SINGLE:
    case FACE_UNDERLINE_DOUBLE_LINE:
      nhwl_fill_rectangle (f, foreground, s->x, s->ybase + position,
			   decoration_width, thickness, false);
      break;

    case FACE_UNDERLINE_DOTS:
      segment = thickness;
      FALLTHROUGH;

    case FACE_UNDERLINE_DASHES:
      nhwl_draw_dash (f, s, foreground, decoration_width, segment,
		      position, thickness);
      break;

    case FACE_NO_UNDERLINE:
    case FACE_UNDERLINE_WAVE:
    default:
      emacs_abort ();
    }
}

void
nhwl_draw_glyph_string (struct glyph_string *s)
{
  bool relief_drawn_p = false;

  nhwl_damage_glyphs (s);
  /* If S draws into the background of its successors, draw the
     background of the successors first so that S can draw into it.
     This makes S->next use XDrawString instead of XDrawImageString.  */
  if (s->next && s->right_overhang && !s->for_overlaps)
    {
      int width;
      struct glyph_string *next;

      for (width = 0, next = s->next;
	   next && width < s->right_overhang;
	   width += next->width, next = next->next)
	if (next->first_glyph->type != IMAGE_GLYPH)
	  {
	    cairo_t *cr = nhwl_begin_cr_clip (next->f);
	    nhwl_set_glyph_string_gc (next);
	    nhwl_set_glyph_string_clipping (next, cr);
	    if (next->first_glyph->type == STRETCH_GLYPH)
	      nhwl_draw_stretch_glyph_string (next);
	    else
	      nhwl_draw_glyph_string_background (next, true);
	    next->num_clips = 0;
	    nhwl_end_cr_clip (next->f);
	  }
    }

  /* Set up S->gc, set clipping and draw S.  */
  nhwl_set_glyph_string_gc (s);

  cairo_t *cr = nhwl_begin_cr_clip (s->f);

  /* Draw relief (if any) in advance for char/composition so that the
     glyph string can be drawn over it.  */
  if (!s->for_overlaps
      && s->face->box != FACE_NO_BOX
      && (s->first_glyph->type == CHAR_GLYPH
	  || s->first_glyph->type == COMPOSITE_GLYPH))

    {
      nhwl_set_glyph_string_clipping (s, cr);
      nhwl_draw_glyph_string_background (s, true);
      nhwl_draw_glyph_string_box (s);
      nhwl_set_glyph_string_clipping (s, cr);
      relief_drawn_p = true;
    }
  else if (!s->clip_head	/* draw_glyphs didn't specify a clip mask. */
	   && !s->clip_tail
	   && ((s->prev && s->prev->hl != s->hl && s->left_overhang)
	       || (s->next && s->next->hl != s->hl && s->right_overhang)))
    /* We must clip just this glyph.  left_overhang part has already
       drawn when s->prev was drawn, and right_overhang part will be
       drawn later when s->next is drawn. */
    nhwl_set_glyph_string_clipping_exactly (s, s, cr);
  else
    nhwl_set_glyph_string_clipping (s, cr);

  switch (s->first_glyph->type)
    {
    case IMAGE_GLYPH:
      nhwl_draw_image_glyph_string (s);
      break;

    case STRETCH_GLYPH:
      nhwl_draw_stretch_glyph_string (s);
      break;

    case CHAR_GLYPH:
      if (s->for_overlaps)
	s->background_filled_p = true;
      else
	nhwl_draw_glyph_string_background (s, false);
      nhwl_draw_glyph_string_foreground (s);
      break;

    case COMPOSITE_GLYPH:
      if (s->for_overlaps || (s->cmp_from > 0
			      && !s->first_glyph->u.cmp.automatic))
	s->background_filled_p = true;
      else
	nhwl_draw_glyph_string_background (s, true);
      nhwl_draw_composite_glyph_string_foreground (s);
      break;

    case GLYPHLESS_GLYPH:
      if (s->for_overlaps)
	s->background_filled_p = true;
      else
	nhwl_draw_glyph_string_background (s, true);
      nhwl_draw_glyphless_glyph_string_foreground (s);
      break;

    default:
      emacs_abort ();
    }

  if (!s->for_overlaps)
    {
      /* Draw relief if not yet drawn.  */
      if (!relief_drawn_p && s->face->box != FACE_NO_BOX)
	nhwl_draw_glyph_string_box (s);

      /* Draw underline.  */
      if (s->face->underline)
	{
	  if (s->face->underline == FACE_UNDERLINE_WAVE)
	    {
	      if (s->face->underline_defaulted_p)
		nhwl_draw_underwave (s, s->xgcv.foreground);
	      else
		nhwl_draw_underwave (s, s->face->underline_color);
	    }
	  else if (s->face->underline >= FACE_UNDERLINE_SINGLE)
	    {
	      unsigned long thickness, position;
	      unsigned long foreground;

	      if (s->prev
		  && (s->prev->face->underline != FACE_UNDERLINE_WAVE
		      && s->prev->face->underline >= FACE_UNDERLINE_SINGLE)
		  && (s->prev->face->underline_at_descent_line_p
		      == s->face->underline_at_descent_line_p)
		  && (s->prev->face->underline_pixels_above_descent_line
		      == s->face->underline_pixels_above_descent_line))
		{
		  /* We use the same underline style as the previous one.  */
		  thickness = s->prev->underline_thickness;
		  position = s->prev->underline_position;
		}
	      else
		{
		  struct font *font = font_for_underline_metrics (s);

		  /* Get the underline thickness.  Default is 1 pixel.  */
		  if (font && font->underline_thickness > 0)
		    thickness = font->underline_thickness;
		  else
		    thickness = 1;
		  if ((x_underline_at_descent_line
		       || s->face->underline_at_descent_line_p))
		    position = ((s->height - thickness)
				- (s->ybase - s->y)
				- s->face->underline_pixels_above_descent_line);
		  else
		    {
		      /* Get the underline position.  This is the recommended
		         vertical offset in pixels from the baseline to the top of
		         the underline.  This is a signed value according to the
		         specs, and its default is

		         ROUND ((maximum descent) / 2), with
		         ROUND(x) = floor (x + 0.5)  */

		      if (x_use_underline_position_properties
			  && font && font->underline_position >= 0)
			position = font->underline_position;
		      else if (font)
			position = (font->descent + 1) / 2;
		      else
			position = underline_minimum_offset;
		    }

		  /* Ignore minimum_offset if the amount of pixels was
		     explicitly specified.  */
		  if (!s->face->underline_pixels_above_descent_line)
		    position = max (position, underline_minimum_offset);
		}
	      /* Check the sanity of thickness and position.  We should
	         avoid drawing underline out of the current line area.  */
	      if (s->y + s->height <= s->ybase + position)
		position = (s->height - 1) - (s->ybase - s->y);
	      if (s->y + s->height < s->ybase + position + thickness)
		thickness = (s->y + s->height) - (s->ybase + position);
	      s->underline_thickness = thickness;
	      s->underline_position = position;

	      if (s->face->underline_defaulted_p)
		foreground = s->xgcv.foreground;
	      else
		foreground = s->face->underline_color;

	      nhwl_fill_underline (s->f, s, foreground, s->face->underline,
				   position, s->width, thickness);

	      /* Place a second underline above the first if this was
		 requested in the face specification.  */

	      if (s->face->underline == FACE_UNDERLINE_DOUBLE_LINE)
		{
		  /* Compute the position of the second underline.  */
		  position = position - thickness - 1;
		  nhwl_fill_underline (s->f, s, foreground, s->face->underline,
				       position, s->width, thickness);
		}
	    }
	}
      /* Draw overline.  */
      if (s->face->overline_p)
	{
	  unsigned long dy = 0, h = 1;

	  if (s->face->overline_color_defaulted_p)
	    nhwl_fill_rectangle (s->f, s->xgcv.foreground, s->x, s->y + dy,
				 s->width, h, false);
	  else
	    nhwl_fill_rectangle (s->f, s->face->overline_color, s->x,
				 s->y + dy, s->width, h, false);
	}

      /* Draw strike-through.  */
      if (s->face->strike_through_p)
	{
	  /* Y-coordinate and height of the glyph string's first
	     glyph.  We cannot use s->y and s->height because those
	     could be larger if there are taller display elements
	     (e.g., characters displayed with a larger font) in the
	     same glyph row.  */
	  int glyph_y = s->ybase - s->first_glyph->ascent;
	  int glyph_height = s->first_glyph->ascent + s->first_glyph->descent;
	  /* Strike-through width and offset from the glyph string's
	     top edge.  */
          unsigned long h = 1;
          unsigned long dy = (glyph_height - h) / 2;

	  if (s->face->strike_through_color_defaulted_p)
	    nhwl_fill_rectangle (s->f, s->xgcv.foreground, s->x, glyph_y + dy,
				 s->width, h, false);
	  else
	    nhwl_fill_rectangle (s->f, s->face->strike_through_color, s->x,
				 glyph_y + dy, s->width, h, false);
	}

      if (s->prev)
	{
	  struct glyph_string *prev;

	  for (prev = s->prev; prev; prev = prev->prev)
	    if (prev->hl != s->hl
		&& prev->x + prev->width + prev->right_overhang > s->x)
	      {
		/* As prev was drawn while clipped to its own area, we
		   must draw the right_overhang part using s->hl now.  */
		enum draw_glyphs_face save = prev->hl;

		prev->hl = s->hl;
		nhwl_set_glyph_string_gc (prev);
		cairo_save (cr);
		nhwl_set_glyph_string_clipping_exactly (s, prev, cr);
		if (prev->first_glyph->type == CHAR_GLYPH)
		  nhwl_draw_glyph_string_foreground (prev);
		else
		  nhwl_draw_composite_glyph_string_foreground (prev);
		prev->hl = save;
		prev->num_clips = 0;
		cairo_restore (cr);
	      }
	}

      if (s->next)
	{
	  struct glyph_string *next;

	  for (next = s->next; next; next = next->next)
	    if (next->hl != s->hl
		&& next->x - next->left_overhang < s->x + s->width)
	      {
		/* As next will be drawn while clipped to its own area,
		   we must draw the left_overhang part using s->hl now.  */
		enum draw_glyphs_face save = next->hl;

		next->hl = s->hl;
		nhwl_set_glyph_string_gc (next);
		cairo_save (cr);
		nhwl_set_glyph_string_clipping_exactly (s, next, cr);
		if (next->first_glyph->type == CHAR_GLYPH)
		  nhwl_draw_glyph_string_foreground (next);
		else
		  nhwl_draw_composite_glyph_string_foreground (next);
		cairo_restore (cr);
		next->hl = save;
		next->num_clips = 0;
		next->clip_head = s->next;
	      }
	}
    }

  /* TODO: figure out in which cases the stipple is actually drawn on
     NHWL.  */
  if (!s->row->stipple_p)
    s->row->stipple_p = s->face->stipple;

  /* Reset clipping.  */
  nhwl_end_cr_clip (s->f);
  s->num_clips = 0;
}

/* RIF: Define cursor CURSOR on frame F.  */

static void
nhwl_draw_hollow_cursor (struct window *w, struct glyph_row *row)
{
  struct frame *f = XFRAME (WINDOW_FRAME (w));
  int x, y, wd, h;
  struct glyph *cursor_glyph;

  /* Get the glyph the cursor is on.  If we can't tell because
     the current matrix is invalid or such, give up.  */
  cursor_glyph = get_phys_cursor_glyph (w);
  if (cursor_glyph == NULL)
    return;

  /* Compute frame-relative coordinates for phys cursor.  */
  get_phys_cursor_geometry (w, row, cursor_glyph, &x, &y, &h);
  wd = w->phys_cursor_width - 1;

  /* The foreground of cursor_gc is typically the same as the normal
     background color, which can cause the cursor box to be invisible.  */
  cairo_t *cr = nhwl_begin_cr_clip (f);
  nhwl_set_cr_source_with_color (f, FRAME_X_OUTPUT (f)->cursor_color, false);

  /* When on R2L character, show cursor at the right edge of the
     glyph, unless the cursor box is as wide as the glyph or wider
     (the latter happens when x-stretch-cursor is non-nil).  */
  if ((cursor_glyph->resolved_level & 1) != 0
      && cursor_glyph->pixel_width > wd)
    {
      x += cursor_glyph->pixel_width - wd;
      if (wd > 0)
	wd -= 1;
    }
  /* Set clipping, draw the rectangle, and reset clipping again.  */
  nhwl_clip_to_row (w, row, TEXT_AREA, cr);
  nhwl_draw_rectangle (f, FRAME_X_OUTPUT (f)->cursor_color,
		       x, y, wd, h - 1, false);
  nhwl_end_cr_clip (f);
}

/* Draw a bar cursor on window W in glyph row ROW.

   Implementation note: One would like to draw a bar cursor with an
   angle equal to the one given by the font property XA_ITALIC_ANGLE.
   Unfortunately, I didn't find a font yet that has this property set.
   --gerd.  */

static void
nhwl_draw_bar_cursor (struct window *w, struct glyph_row *row, int width,
		   enum text_cursor_kinds kind)
{
  struct frame *f = XFRAME (w->frame);
  struct glyph *cursor_glyph;

  /* If cursor is out of bounds, don't draw garbage.  This can happen
     in mini-buffer windows when switching between echo area glyphs
     and mini-buffer.  */
  cursor_glyph = get_phys_cursor_glyph (w);
  if (cursor_glyph == NULL)
    return;

  /* Experimental avoidance of cursor on xwidget.  */
  if (cursor_glyph->type == XWIDGET_GLYPH)
    return;

  /* If on an image, draw like a normal cursor.  That's usually better
     visible than drawing a bar, esp. if the image is large so that
     the bar might not be in the window.  */
  if (cursor_glyph->type == IMAGE_GLYPH)
    {
      struct glyph_row *r;
      r = MATRIX_ROW (w->current_matrix, w->phys_cursor.vpos);
      draw_phys_cursor_glyph (w, r, DRAW_CURSOR);
    }
  else
    {
      struct face *face = FACE_FROM_ID (f, cursor_glyph->face_id);
      unsigned long color;

      cairo_t *cr = nhwl_begin_cr_clip (f);

      /* If the glyph's background equals the color we normally draw
         the bars cursor in, the bar cursor in its normal color is
         invisible.  Use the glyph's foreground color instead in this
         case, on the assumption that the glyph's colors are chosen so
         that the glyph is legible.  */
      if (face->background == FRAME_X_OUTPUT (f)->cursor_color)
	color = face->foreground;
      else
	color = FRAME_X_OUTPUT (f)->cursor_color;

      nhwl_clip_to_row (w, row, TEXT_AREA, cr);

      if (kind == BAR_CURSOR)
	{
	  int x = WINDOW_TEXT_TO_FRAME_PIXEL_X (w, w->phys_cursor.x);

	  if (width < 0)
	    width = FRAME_CURSOR_WIDTH (f);
	  width = min (cursor_glyph->pixel_width, width);

	  w->phys_cursor_width = width;

	  /* If the character under cursor is R2L, draw the bar cursor
	     on the right of its glyph, rather than on the left.  */
	  if ((cursor_glyph->resolved_level & 1) != 0)
	    x += cursor_glyph->pixel_width - width;

	  nhwl_fill_rectangle (f, color, x,
			       WINDOW_TO_FRAME_PIXEL_Y (w, w->phys_cursor.y),
			       width, row->height, false);
	}
      else			/* HBAR_CURSOR */
	{
	  int dummy_x, dummy_y, dummy_h;
	  int x = WINDOW_TEXT_TO_FRAME_PIXEL_X (w, w->phys_cursor.x);

	  if (width < 0)
	    width = row->height;

	  width = min (row->height, width);

	  get_phys_cursor_geometry (w, row, cursor_glyph, &dummy_x,
				    &dummy_y, &dummy_h);

	  if ((cursor_glyph->resolved_level & 1) != 0
	      && cursor_glyph->pixel_width > w->phys_cursor_width - 1)
	    x += cursor_glyph->pixel_width - w->phys_cursor_width + 1;
	  nhwl_fill_rectangle (f, color, x,
			       WINDOW_TO_FRAME_PIXEL_Y (w, w->phys_cursor.y +
							row->height - width),
			       w->phys_cursor_width - 1, width, false);
	}

      nhwl_end_cr_clip (f);
    }
}

/* RIF: Draw cursor on window W.  */

void
nhwl_draw_window_cursor (struct window *w, struct glyph_row *glyph_row, int x,
			 int y, enum text_cursor_kinds cursor_type,
			 int cursor_width, bool on_p, bool active_p)
{
  struct frame *f = XFRAME (w->frame);

  if (on_p)
    {
      w->phys_cursor_type = cursor_type;
      w->phys_cursor_on_p = true;

      if (glyph_row->exact_window_width_line_p
	  && (glyph_row->reversed_p
	      ? (w->phys_cursor.hpos < 0)
	      : (w->phys_cursor.hpos >= glyph_row->used[TEXT_AREA])))
	{
	  glyph_row->cursor_in_fringe_p = true;
	  draw_fringe_bitmap (w, glyph_row, glyph_row->reversed_p);
	}
      else
	{
	  switch (cursor_type)
	    {
	    case HOLLOW_BOX_CURSOR:
	      nhwl_draw_hollow_cursor (w, glyph_row);
	      break;

	    case FILLED_BOX_CURSOR:
	      draw_phys_cursor_glyph (w, glyph_row, DRAW_CURSOR);
	      break;

	    case BAR_CURSOR:
	      nhwl_draw_bar_cursor (w, glyph_row, cursor_width, BAR_CURSOR);
	      break;

	    case HBAR_CURSOR:
	      nhwl_draw_bar_cursor (w, glyph_row, cursor_width, HBAR_CURSOR);
	      break;

	    case NO_CURSOR:
	      w->phys_cursor_width = 0;
	      break;

	    default:
	      emacs_abort ();
	    }
	}
      /* The input method's cursor rectangle is W1's.  */
    }

  nhwl_damage_cursor (w, glyph_row, on_p);
}

/* Draw a vertical window border from (x,y0) to (x,y1)  */

void
nhwl_draw_vertical_window_border (struct window *w, int x, int y0, int y1)
{
  struct frame *f = XFRAME (WINDOW_FRAME (w));
  struct face *face;
  cairo_t *cr;

  cr = nhwl_begin_cr_clip (f);
  nhwl_damage (f, x, y0, 1, y1 - y0);

  face = FACE_FROM_ID_OR_NULL (f, VERTICAL_BORDER_FACE_ID);
  if (face)
    nhwl_set_cr_source_with_color (f, face->foreground, false);

  cairo_rectangle (cr, x, y0, 1, y1 - y0);
  cairo_fill (cr);

  nhwl_end_cr_clip (f);
}

/* Draw a window divider from (x0,y0) to (x1,y1)  */

void
nhwl_draw_window_divider (struct window *w, int x0, int x1, int y0, int y1)
{
  struct frame *f = XFRAME (WINDOW_FRAME (w));
  struct face *face = FACE_FROM_ID_OR_NULL (f, WINDOW_DIVIDER_FACE_ID);
  struct face *face_first
    = FACE_FROM_ID_OR_NULL (f, WINDOW_DIVIDER_FIRST_PIXEL_FACE_ID);
  struct face *face_last
    = FACE_FROM_ID_OR_NULL (f, WINDOW_DIVIDER_LAST_PIXEL_FACE_ID);
  unsigned long color = face ? face->foreground : FRAME_FOREGROUND_PIXEL (f);
  unsigned long color_first = (face_first
			       ? face_first->foreground
			       : FRAME_FOREGROUND_PIXEL (f));
  unsigned long color_last = (face_last
			      ? face_last->foreground
			      : FRAME_FOREGROUND_PIXEL (f));
  cairo_t *cr = nhwl_begin_cr_clip (f);
  nhwl_damage (f, x0, y0, x1 - x0, y1 - y0);

  if (y1 - y0 > x1 - x0 && x1 - x0 > 2)
    /* Vertical.  */
    {
      nhwl_set_cr_source_with_color (f, color_first,
                                     f->borders_respect_alpha_background);
      cairo_rectangle (cr, x0, y0, 1, y1 - y0);
      cairo_fill (cr);
      nhwl_set_cr_source_with_color (f, color,
                                     f->borders_respect_alpha_background);
      cairo_rectangle (cr, x0 + 1, y0, x1 - x0 - 2, y1 - y0);
      cairo_fill (cr);
      nhwl_set_cr_source_with_color (f, color_last,
                                     f->borders_respect_alpha_background);
      cairo_rectangle (cr, x1 - 1, y0, 1, y1 - y0);
      cairo_fill (cr);
    }
  else if (x1 - x0 > y1 - y0 && y1 - y0 > 3)
    /* Horizontal.  */
    {
      nhwl_set_cr_source_with_color (f, color_first,
                                     f->borders_respect_alpha_background);
      cairo_rectangle (cr, x0, y0, x1 - x0, 1);
      cairo_fill (cr);
      nhwl_set_cr_source_with_color (f, color,
                                     f->borders_respect_alpha_background);
      cairo_rectangle (cr, x0, y0 + 1, x1 - x0, y1 - y0 - 2);
      cairo_fill (cr);
      nhwl_set_cr_source_with_color (f, color_last,
                                     f->borders_respect_alpha_background);
      cairo_rectangle (cr, x0, y1 - 1, x1 - x0, 1);
      cairo_fill (cr);
    }
  else
    {
      nhwl_set_cr_source_with_color (f, color,
                                     f->borders_respect_alpha_background);
      cairo_rectangle (cr, x0, y0, x1 - x0, y1 - y0);
      cairo_fill (cr);
    }
  nhwl_end_cr_clip (f);
}

/* Fringe bitmaps.  */

static int max_fringe_bmp = 0;
static cairo_pattern_t **fringe_bmp = 0;

void
nhwl_define_fringe_bitmap (int which, unsigned short *bits, int h, int wd)
{
  int i, stride;
  cairo_surface_t *surface;
  unsigned char *data;
  cairo_pattern_t *pattern;

  if (which >= max_fringe_bmp)
    {
      i = max_fringe_bmp;
      max_fringe_bmp = which + 20;
      fringe_bmp
	= xrealloc (fringe_bmp, max_fringe_bmp * sizeof (cairo_pattern_t *));
      while (i < max_fringe_bmp)
	fringe_bmp[i++] = 0;
    }

  block_input ();

  surface = cairo_image_surface_create (CAIRO_FORMAT_A1, wd, h);
  stride = cairo_image_surface_get_stride (surface);
  data = cairo_image_surface_get_data (surface);

  for (i = 0; i < h; i++)
    {
      *((unsigned short *) data) = bits[i];
      data += stride;
    }

  cairo_surface_mark_dirty (surface);
  pattern = cairo_pattern_create_for_surface (surface);
  cairo_surface_destroy (surface);

  unblock_input ();

  fringe_bmp[which] = pattern;
}

void
nhwl_destroy_fringe_bitmap (int which)
{
  if (which >= max_fringe_bmp)
    return;

  if (fringe_bmp[which])
    {
      block_input ();
      cairo_pattern_destroy (fringe_bmp[which]);
      unblock_input ();
    }
  fringe_bmp[which] = 0;
}

static void
nhwl_clip_to_row (struct window *w, struct glyph_row *row,
		  enum glyph_row_area area, cairo_t * cr)
{
  int window_x, window_y, window_width;
  cairo_rectangle_int_t rect;

  window_box (w, area, &window_x, &window_y, &window_width, 0);

  rect.x = window_x;
  rect.y = WINDOW_TO_FRAME_PIXEL_Y (w, max (0, row->y));
  rect.y = max (rect.y, window_y);
  rect.width = window_width;
  rect.height = row->visible_height;

  cairo_rectangle (cr, rect.x, rect.y, rect.width, rect.height);
  cairo_clip (cr);
}

/* The device-grid mask of a fringe bitmap pattern at one scale, kept on the
   pattern (cairo_pattern_set_user_data), so that destroying the pattern
   frees it.  From P1.5 Task 26 (glitnir-pgtk.c, gl15-i2f3).  */
struct fringe_mask
{
  uint32_t scale120;
  cairo_surface_t *surface;	/* A8, the bitmap's size at the scale */
};

static cairo_user_data_key_t fringe_mask_key;

static void
fringe_mask_free (void *p)
{
  struct fringe_mask *m = p;

  cairo_surface_destroy (m->surface);
  xfree (m);
}

/* The A8 mask of PATTERN's A1 bitmap at SCALE120, from the pattern's cache
   or made now; NULL on failure.  */
static cairo_surface_t *
fringe_mask_get (cairo_pattern_t *pattern, uint32_t scale120)
{
  struct fringe_mask *m = cairo_pattern_get_user_data (pattern,
						       &fringe_mask_key);
  cairo_surface_t *src, *mask;
  int w, h, dw, dh;

  if (m && m->scale120 == scale120)
    return m->surface;
  if (cairo_pattern_get_surface (pattern, &src) != CAIRO_STATUS_SUCCESS)
    return NULL;
  w = cairo_image_surface_get_width (src);
  h = cairo_image_surface_get_height (src);
  dw = glitnir_stable_origin (w, scale120);
  dh = glitnir_stable_origin (h, scale120);
  if (dw <= 0 || dh <= 0)
    return NULL;
  mask = cairo_image_surface_create (CAIRO_FORMAT_A8, dw, dh);
  cairo_surface_flush (src);
  if (cairo_surface_status (mask) != CAIRO_STATUS_SUCCESS
      || (glitnir_mask_to_device
	  (cairo_image_surface_get_data (src),
	   cairo_image_surface_get_stride (src), w, h,
	   cairo_image_surface_get_format (src) == CAIRO_FORMAT_A1, scale120,
	   cairo_image_surface_get_data (mask),
	   cairo_image_surface_get_stride (mask)) != GLITNIR_OK))
    {
      cairo_surface_destroy (mask);
      return NULL;
    }
  cairo_surface_mark_dirty (mask);
  /* Replacing the user data frees the mask of the former scale.  */
  m = xmalloc (sizeof *m);
  m->scale120 = scale120;
  m->surface = mask;
  if (cairo_pattern_set_user_data (pattern, &fringe_mask_key, m,
				   fringe_mask_free) != CAIRO_STATUS_SUCCESS)
    {
      fringe_mask_free (m);
      return NULL;
    }
  return mask;
}

/* Draw bitmap P (after its background) at the frame's scale s, centred
   where the stock code would draw it at 1 to 1: a device-pixel frame's
   bitmap is s times as many pixels, on the device grid (P1.5 Task 26).
   Only while it fits its fringe (a fringe as wide as the bitmap keeps it
   1 to 1, as X11 does at a high DPI); false then, and the stock code
   draws.  CR is clipped to the row.  */
static bool
nhwl_draw_fringe_scaled (struct window *w, struct draw_fringe_bitmap_params *p,
			 cairo_t *cr, unsigned long fg, unsigned long bg)
{
  struct frame *f = XFRAME (WINDOW_FRAME (w));
  struct nhwl_output *out = FRAME_NHWL_OUTPUT (f);
  bool left = p->x < window_box_left (w, TEXT_AREA);
  int fw = left ? WINDOW_LEFT_FRINGE_WIDTH (w) : WINDOW_RIGHT_FRINGE_WIDTH (w);
  double s = (out->scale120 ? out->scale120 : 120) / 120.0;
  double k = p->wd > 0 ? min (s, (double) fw / p->wd) : 1;
  uint32_t k120 = lround (k * 120);
  cairo_surface_t *mask;
  int bh, x0, y0, col;

  if (k120 <= 120 || !fringe_bmp[p->which])
    return false;
  mask = fringe_mask_get (fringe_bmp[p->which], k120);
  if (!mask)
    return false;
  /* The bitmap's whole height, its top at P's first row less the rows
     cut off above (dh), and its centre kept.  */
  bh = p->h + p->dh;
  {
    cairo_surface_t *src;

    if (cairo_pattern_get_surface (fringe_bmp[p->which], &src)
	== CAIRO_STATUS_SUCCESS)
      bh = cairo_image_surface_get_height (src);
  }
  x0 = p->x + (p->wd - cairo_image_surface_get_width (mask)) / 2;
  y0 = p->y - p->dh + (bh - cairo_image_surface_get_height (mask)) / 2;
  col = left ? p->x - (fw - p->wd) / 2 : p->x - (fw - p->wd + 1) / 2;
  cairo_rectangle (cr, col, p->y, fw, p->h);
  cairo_clip (cr);
  if (!p->overlay_p)
    {
      /* The stock code fills the bitmap's cell in the background.  */
      nhwl_set_cr_source_with_color (f, bg, true);
      cairo_paint (cr);
    }
  nhwl_set_cr_source_with_color (f, fg, false);
  cairo_mask_surface (cr, mask, x0, y0);
  nhwl_damage (f, col, p->y, fw, p->h);
  return true;
}

void
nhwl_draw_fringe_bitmap (struct window *w, struct glyph_row *row,
			 struct draw_fringe_bitmap_params *p)
{
  struct frame *f = XFRAME (WINDOW_FRAME (w));
  struct face *face = p->face;

  cairo_t *cr = nhwl_begin_cr_clip (f);
  nhwl_damage_fringe (w, p);

  /* Must clip because of partially visible lines.  */
  nhwl_clip_to_row (w, row, ANY_AREA, cr);

  if (p->bx >= 0 && !p->overlay_p)
    nhwl_fill_background_by_face (f, face, p->bx, p->by, p->nx, p->ny);

  if (p->which
      && p->which < max_fringe_bmp
      && p->which < max_used_fringe_bitmap)
    {
      Emacs_GC gcv;

      if (!fringe_bmp[p->which])
	{
	  /* This fringe bitmap is known to fringe.c, but lacks the
	     cairo_pattern_t pattern which shadows that bitmap.  This
	     is typical to define-fringe-bitmap being called when the
	     selected frame was not a GUI frame, for example, when
	     packages that define fringe bitmaps are loaded by a
	     daemon Emacs.  Create the missing pattern now.  */
	  gui_define_fringe_bitmap (f, p->which);
	}

      gcv.foreground = (p->cursor_p
			? (p->overlay_p ? face->background
			   : FRAME_X_OUTPUT (f)->cursor_color)
			: face->foreground);
      gcv.background = face->background;
      if (!nhwl_draw_fringe_scaled (w, p, cr, gcv.foreground, gcv.background))
	nhwl_cr_draw_image (f, &gcv, fringe_bmp[p->which], 0, p->dh,
			    p->wd, p->h, p->x, p->y, p->overlay_p);
    }

  nhwl_end_cr_clip (f);
}

static void
nhwl_fill_rectangle (struct frame *f, unsigned long color, int x, int y,
		     int width, int height, bool respect_alpha_background)
{
  cairo_t *cr;
  cr = nhwl_begin_cr_clip (f);
  nhwl_set_cr_source_with_color (f, color, respect_alpha_background);
  cairo_rectangle (cr, x, y, width, height);
  cairo_fill (cr);
  nhwl_end_cr_clip (f);
}

/***********************************************************************
		      Scrolling and Glitnir's damage
 ***********************************************************************/

/* Drawing damages through nhwl_damage, nhwl_damage_all or the scroll, and
   the damage waits in Glitnir until the tree is published
   (nhwl_publish).  */

/* Adapted from pgtk_scroll_run (emacs-31.1): the glue moves the rows
   inside the backing (no temporary surface), and Glitnir learns the move
   and the rows it exposed; the decorations' anchors move with them.  A
   run that moves the whole window may give it a layer of its own (the
   glue decides from the window's box, nhwl_layer_flags).  */
void
nhwl_scroll_run (struct window *w, struct run *run)
{
  struct frame *f = XFRAME (w->frame);
  int x, y, width, height, from_y, to_y, bottom_y;
  struct nhwl_output *out = FRAME_NHWL_OUTPUT (f);
  NhgRect band, box;

  window_box (w, ANY_AREA, &x, &y, &width, &height);
  box = (NhgRect) { x, y, width, height };

  from_y = WINDOW_TO_FRAME_PIXEL_Y (w, run->current_y);
  to_y = WINDOW_TO_FRAME_PIXEL_Y (w, run->desired_y);
  bottom_y = y + height;

  if (to_y < from_y)
    {
      if (from_y + run->height > bottom_y)
	height = bottom_y - from_y;
      else
	height = run->height;
    }
  else
    {
      if (to_y + run->height > bottom_y)
	height = bottom_y - to_y;
      else
	height = run->height;
    }

  block_input ();

  /* Cursor off.  Will be switched on again in x_update_window_end.  */
  gui_clear_cursor (w);

  nhwl_backing_resize (f);
  cairo_surface_flush (out->cr_surface);
  nhwl_glitnir_ensure (f);
  glitnir_decor_scrolled (f, w, x, min (from_y, to_y), width,
			  max (from_y, to_y) + height - min (from_y, to_y),
			  to_y - from_y);
  band = (NhgRect) { x, from_y, width, height };
  nhwl_glue_rc (FRAME_DISPLAY_INFO (f),
		NHG (nhg_scroll_window (FRAME_NHG (f), out->window,
					out->glitnir_id, w->sequence_number,
					box, band, to_y, nhwl_layer_flags (f))));
  cairo_surface_mark_dirty (out->cr_surface);

  unblock_input ();
}

/* Damage the rectangle X, Y, W, H (device pixels) of F, clipped to F's
   native area.  */
void
nhwl_damage (struct frame *f, int x, int y, int w, int h)
{
  NhgRect r;
  int x1 = min (x + w, FRAME_PIXEL_WIDTH (f));
  int y1 = min (y + h, FRAME_PIXEL_HEIGHT (f));

  r.x = max (x, 0);
  r.y = max (y, 0);
  r.w = x1 - r.x;
  r.h = y1 - r.y;
  if (r.w <= 0 || r.h <= 0)
    return;
  nhwl_glitnir_ensure (f);
  NHG (nhg_damage (FRAME_NHG (f), FRAME_NHWL_OUTPUT (f)->glitnir_id, &r, 1));
}

/* Damage the whole of F, its margins and title strip included.  */
void
nhwl_damage_all (struct frame *f)
{
  nhwl_glitnir_ensure (f);
  NHG (nhg_damage_all (FRAME_NHG (f), FRAME_NHWL_OUTPUT (f)->glitnir_id));
}

/* Adapted from glitnir_pgtk_damage_glyphs: a glyph string covers its row's
   clipping rectangle vertically and its glyphs, their overhangs and the
   backgrounds it draws for its successors horizontally.  */
void
nhwl_damage_glyphs (struct glyph_string *s)
{
  NativeRectangle clip;
  int x0 = s->x - max (s->left_overhang, 0);
  int x1 = (s->x + max (s->width, s->background_width)
	    + max (s->right_overhang, 0));

  get_glyph_string_clip_rect (s, &clip);
  if (s->next && s->right_overhang && !s->for_overlaps)
    {
      int width = 0;
      struct glyph_string *next;

      for (next = s->next; next && width < s->right_overhang;
	   width += next->width, next = next->next)
	if (next->first_glyph->type != IMAGE_GLYPH)
	  x1 = max (x1, next->x + next->background_width);
    }
  if (s->extends_to_end_of_line_p)
    x1 = max (x1, clip.x + (int) clip.width);
  nhwl_damage (s->f, x0, clip.y, x1 - x0, clip.height);
  /* PD: rounded neighbours' corners, and the label anchors S covers.  */
  glitnir_decor_after_glyphs (s);
  glitnir_decor_record (s);
}

/* Adapted from glitnir_pgtk_damage_cursor: a hollow, bar or hbar cursor
   (a box cursor is a glyph string): at most the cursor glyph, clipped to
   its row.  */
void
nhwl_damage_cursor (struct window *w, struct glyph_row *row, bool on_p)
{
  struct glyph *g = on_p ? get_phys_cursor_glyph (w) : NULL;

  if (g)
    nhwl_damage (XFRAME (w->frame),
		 WINDOW_TEXT_TO_FRAME_PIXEL_X (w, w->phys_cursor.x),
		 WINDOW_TO_FRAME_PIXEL_Y (w, max (0, row->y)),
		 max (g->pixel_width, w->phys_cursor_width) + 1,
		 row->visible_height);
}

/* Adapted from glitnir_pgtk_damage_fringe.  */
void
nhwl_damage_fringe (struct window *w, struct draw_fringe_bitmap_params *p)
{
  struct frame *f = XFRAME (WINDOW_FRAME (w));

  if (p->bx >= 0 && !p->overlay_p)
    nhwl_damage (f, p->bx, p->by, p->nx, p->ny);
  if (p->which)
    nhwl_damage (f, p->x, p->y, p->wd, p->h);
}

/* Adapted from glitnir_pgtk_damage_internal_border.  */
void
nhwl_damage_internal_border (struct frame *f)
{
  int border = FRAME_INTERNAL_BORDER_WIDTH (f);
  int width = FRAME_PIXEL_WIDTH (f), height = FRAME_PIXEL_HEIGHT (f);
  int margin = FRAME_TOP_MARGIN_HEIGHT (f);
  int bottom_margin = FRAME_BOTTOM_MARGIN_HEIGHT (f);

  nhwl_damage (f, 0, margin, width, border);
  nhwl_damage (f, 0, 0, border, height);
  nhwl_damage (f, width - border, 0, border, height);
  nhwl_damage (f, 0, height - bottom_margin - border, width, border);
}


/***********************************************************************
	      The backing, and Glitnir's commit and compose
 ***********************************************************************/

/* The backing's pixels are the glue's; OUT->cr_surface draws on them, and
   Glitnir composes from them (GlitnirSource), reading this surface into
   the shell's buffer.  */

/* F's size for the glue: its native area, the border around it, and its
   scale.  */
static void
nhwl_frame_size (struct frame *f, NhgFrameGeom *g)
{
  struct nhwl_output *out = FRAME_NHWL_OUTPUT (f);

  memset (g, 0, sizeof *g);
  g->native_w = FRAME_PIXEL_WIDTH (f);
  g->native_h = FRAME_PIXEL_HEIGHT (f);
  g->l = out->csd.l, g->t = out->csd.t, g->r = out->csd.r, g->b = out->csd.b;
  g->scale120 = out->scale120;
  g->window = out->window;
}

/* F's geometry for the glue: its size, and its Glitnir layer, all of the
   backing (the glue puts a child frame's inside its shadow margin).  */
static void
nhwl_frame_geom (struct frame *f, NhgFrameGeom *g)
{
  nhwl_frame_size (f, g);
  g->layer = (NhgRect) { 0, 0, max (1, g->native_w + g->l + g->r),
			 max (1, g->native_h + g->t + g->b) };
}

/* Make F's Glitnir scene match the frame: one frame layer, device size,
   at the window's scale.  A new or resized layer damages the frame; a
   child frame's chrome goes with it (the glue's).  */
void
nhwl_glitnir_ensure (struct frame *f)
{
  struct nhwl_output *out = FRAME_NHWL_OUTPUT (f);
  NhgFrameGeom g;

  nhwl_frame_geom (f, &g);
  NHG (nhg_layer_ensure (FRAME_NHG (f), &out->glitnir_id, &g));
}

/* Whether Glitnir's decorations draw on F: a frame of the shell with a
   scene, not a tooltip, while `glitnir-enable' is on.  */
bool
nhwl_glitnir_active_p (struct frame *f)
{
  return (glitnir_enable && FRAME_NHWL_P (f) && f->output_data.nhwl
	  && !FRAME_TOOLTIP_P (f) && FRAME_NHWL_OUTPUT (f)->glitnir_id);
}

/* Where F's Glitnir layer starts, in F's pixels (device pixels): the
   backing's top-left, before the title bar or margins, or the native area
   for a child frame (the glue puts a child's layer inside its shadow
   margin; one without chrome has no margin, so both are the same).  */
void
nhwl_layer_origin (struct frame *f, int *x, int *y)
{
  struct nhwl_output *out = FRAME_NHWL_OUTPUT (f);

  if (nhwl_child_p (f))
    *x = *y = 0;
  else
    *x = -out->csd.l, *y = -out->csd.t;
}

/* The scene and layer window W's decorations go to (glitnir-decor.c):
   F's root and its one layer, at scale 1 (F's pixels are the device's).
   False when Glitnir's decorations do not draw on F.  */
bool
nhwl_decor_target (struct frame *f, struct window *w, uint64_t *root,
		   uint64_t *layer, double *sx, double *sy)
{
  if (!nhwl_glitnir_active_p (f))
    return false;
  *root = *layer = FRAME_NHWL_OUTPUT (f)->glitnir_id;
  *sx = *sy = 1;
  return true;
}

/* F is gone: Glitnir forgets its scene and damage, and the glue its
   backing (F's surface over it is gone already).  */
void
nhwl_glitnir_forget (struct frame *f)
{
  struct nhwl_output *out = FRAME_NHWL_OUTPUT (f);

  if (out->glitnir_id)
    {
      NHG (nhg_backing_forget (FRAME_NHG (f), out->glitnir_id));
      out->glitnir_id = 0;
    }
}

/* The frame's background as the backing stores it: premultiplied ARGB,
   as Cairo paints it (nhwl_set_cr_source_with_color with the background
   alpha); the glue does Cairo's arithmetic.  */
static uint32_t
nhwl_background_argb (struct frame *f)
{
  uint32_t argb = 0;

  NHG (nhg_premultiply (FRAME_NHWL_OUTPUT (f)->background_color & 0xffffff,
			f->alpha_background, &argb));
  return argb;
}

/* Make the backing F's size (the frame's pixel size, which is device
   pixels), keeping what it shows; a new one starts in the frame's
   background.  Adapted in role from pgtk_cr_update_surface_desired_size
   (emacs-31.1).  */
void
nhwl_backing_resize (struct frame *f)
{
  struct nhwl_output *out = FRAME_NHWL_OUTPUT (f);
  int w = max (1, FRAME_PIXEL_WIDTH (f) + out->csd.l + out->csd.r);
  int h = max (1, FRAME_PIXEL_HEIGHT (f) + out->csd.t + out->csd.b);
  NhgFrameGeom g;
  NhgBacking b;

  if (out->cr_context && out->cr_width == w && out->cr_height == h)
    return;

  if (!out->glitnir_id)
    nhwl_glitnir_ensure (f);
  nhwl_frame_size (f, &g);
  /* No surface (or the empty one a failed try left): a new backing.  */
  if (!out->cr_context || !out->cr_width)
    {
      g.flags = NHG_GEOM_FRESH;
      g.bg_argb = nhwl_background_argb (f);
    }
  if (out->cr_context)
    {
      cairo_surface_flush (out->cr_surface);
      cairo_destroy (out->cr_context);
      cairo_surface_destroy (out->cr_surface);
    }
  if (NHG (nhg_backing (FRAME_NHG (f), out->glitnir_id, &g, &b)) < 0)
    /* No backing: drawing does nothing until the next try.  */
    b = (NhgBacking) { NULL, 0, 0, 0, out->cr_gen };
  out->cr_surface
    = cairo_image_surface_create_for_data (b.pixels, CAIRO_FORMAT_ARGB32,
					   b.w, b.h, b.stride);
  out->cr_context = cairo_create (out->cr_surface);
  out->cr_width = b.w;
  out->cr_height = b.h;
  out->cr_gen = b.gen;
  nhwl_glitnir_ensure (f);
}

/* Monotonic nanoseconds.  */
uint64_t
nhwl_now_ns (void)
{
  struct timespec ts;

  clock_gettime (CLOCK_MONOTONIC, &ts);
  return (uint64_t) ts.tv_sec * 1000000000 + ts.tv_nsec;
}

/* A copy of F's backing, for glitnir-snapshot.  */
cairo_surface_t *
nhwl_snapshot (struct frame *f)
{
  struct nhwl_output *out = FRAME_NHWL_OUTPUT (f);
  cairo_surface_t *copy;
  NhgBacking b;

  if (!out->cr_surface)
    return NULL;
  cairo_surface_flush (out->cr_surface);
  if (NHG (nhg_snapshot (FRAME_NHG (f), out->glitnir_id, &b)) < 0)
    return NULL;
  copy = cairo_image_surface_create (CAIRO_FORMAT_ARGB32, b.w, b.h);
  cairo_surface_flush (copy);
  NHG (nhg_snapshot_copy (FRAME_NHG (f), out->glitnir_id,
			  cairo_image_surface_get_data (copy),
			  cairo_image_surface_get_stride (copy), b.w, b.h));
  cairo_surface_mark_dirty (copy);
  return copy;
}


/***********************************************************************
			   The title strip
 ***********************************************************************/

/* The client-side title bar's painting, moved from nhwlcsd.c (foot's
   render.c CSD was the reference; this was written fresh).  The glue lays
   the strip out, keeps its buttons' state and says at each publish whether
   the strip must be painted again (nhg_csd_paint, which gives the draw
   list); C resolves the colours and the font from the style Lisp gave
   (nhwlfns.c) and paints: the strip in the desktop's colours for an active
   or an inactive window, the frame's title, the buttons hovered and
   pressed, and rounded top corners on a floating window (Glitnir's corner
   kernel).  Device pixels; the corners' 12 is logical.  */

#define NHWL_CSD_RADIUS 12
#define NHWL_CSD_TILED (NHG_STATE_TILED_L | NHG_STATE_TILED_R \
			| NHG_STATE_TILED_T | NHG_STATE_TILED_B)

static void
nhwl_csd_source (cairo_t *cr, unsigned long fg, unsigned long bg, double mix)
{
  cairo_set_source_rgb (cr,
			(RED_FROM_ULONG (bg) * (1 - mix) + RED_FROM_ULONG (fg) * mix) / 255.0,
			(GREEN_FROM_ULONG (bg) * (1 - mix) + GREEN_FROM_ULONG (fg) * mix) / 255.0,
			(BLUE_FROM_ULONG (bg) * (1 - mix) + BLUE_FROM_ULONG (fg) * mix) / 255.0);
}

/* H with the N bytes at P mixed in (FNV-1a).  */
static uint64_t
nhwl_csd_mix (uint64_t h, const void *p, size_t n)
{
  const unsigned char *b = p;

  for (size_t i = 0; i < n; i++)
    h = (h ^ b[i]) * 1099511628211u;
  return h;
}

/* One button's glyph in a cell at X (the strip's top at Y, side SIDE),
   over a 10% foreground-tinted circle while hovered (20% pressed), as
   Adwaita draws them.  */
static void
nhwl_csd_draw_button (cairo_t *cr, uint32_t kind, double x, double y,
		      double side, unsigned long fg, unsigned long bg,
		      uint32_t state, double s)
{
  double cx = x + side / 2, cy = y + side / 2, g = side * 0.14;

  if (state)
    {
      nhwl_csd_source (cr, fg, bg, state == NHG_BTN_PRESS ? 0.2 : 0.1);
      cairo_arc (cr, cx, cy, side * 0.36, 0, 2 * M_PI);
      cairo_fill (cr);
    }
  nhwl_csd_source (cr, fg, bg, 1.0);
  cairo_set_line_width (cr, max (1.0, s));
  switch (kind)
    {
    case NHG_CSD_CLOSE:
      cairo_move_to (cr, cx - g, cy - g);
      cairo_line_to (cr, cx + g, cy + g);
      cairo_move_to (cr, cx + g, cy - g);
      cairo_line_to (cr, cx - g, cy + g);
      break;
    case NHG_CSD_MAX:
      cairo_rectangle (cr, cx - g, cy - g, 2 * g, 2 * g);
      break;
    case NHG_CSD_MIN:
      cairo_move_to (cr, cx - g, cy + g);
      cairo_line_to (cr, cx + g, cy + g);
      break;
    case NHG_CSD_MENU:
      for (int i = -1; i <= 1; i++)
	{
	  cairo_move_to (cr, cx - g, cy + i * g * 0.8);
	  cairo_line_to (cr, cx + g, cy + i * g * 0.8);
	}
      break;
    }
  cairo_stroke (cr);
}

/* F's publish step for its title bar (the backing at its current size):
   the glue sends the window geometry if it changed, and when the strip is
   not what the backing shows, the border is painted (margins clear, the
   strip, its title and buttons) and damaged.  */
void
nhwl_csd_draw (struct frame *f)
{
  struct nhwl_output *out = FRAME_NHWL_OUTPUT (f);
  struct nhwl_csd *c = &out->csd;
  int W = out->cr_width, H = out->cr_height;
  Lisp_Object title = !NILP (f->title) ? f->title : f->name;
  const char *name = STRINGP (title) ? SSDATA (ENCODE_UTF_8 (title)) : "";
  bool active = (out->states & NHG_STATE_ACTIVATED) != 0;
  bool round = !(out->states & (NHG_STATE_MAXIMIZED
				| NHG_STATE_FULLSCREEN | NHWL_CSD_TILED));
  double s = (out->scale120 ? out->scale120 : 120) / 120.0;
  unsigned long fg = 0, bg = 0;
  uint64_t look = 0;
  const char *family = NULL;
  double px = 0;
  cairo_t *cr;
  cairo_text_extents_t te;
  cairo_font_extents_t fe;
  GlitnirRect r[4];
  NhgCsdPaint p;
  int n = 0, i, mbar, tl;

  if (!out->window)
    return;
  /* What the glue does not see goes in as one hash: the title, the
     colours, the font (the style's generation) and the backing.  */
  if (c->on)
    {
      unsigned gen = nhwl_csd_font (&family, &px);

      nhwl_csd_colours (f, active, &fg, &bg);
      look = nhwl_csd_mix (14695981039346656037u, name, strlen (name) + 1);
      look = nhwl_csd_mix (look, &fg, sizeof fg);
      look = nhwl_csd_mix (look, &bg, sizeof bg);
      look = nhwl_csd_mix (look, &gen, sizeof gen);
      look = nhwl_csd_mix (look, &out->cr_gen, sizeof out->cr_gen);
    }
  if (nhwl_glue_rc (FRAME_DISPLAY_INFO (f),
		    NHG (nhg_csd_paint (FRAME_NHG (f), out->window,
					FRAME_PIXEL_WIDTH (f),
					FRAME_PIXEL_HEIGHT (f), look, &p)))
      != 1)
    return;
  mbar = p.strip.y;

  cr = out->cr_context;
  cairo_save (cr);
  /* The margins are transparent, and so is the strip's corners' outside.  */
  cairo_set_operator (cr, CAIRO_OPERATOR_CLEAR);
  cairo_rectangle (cr, 0, 0, W, mbar);
  cairo_rectangle (cr, 0, H - c->b, W, c->b);
  cairo_rectangle (cr, 0, mbar, c->l, H - mbar - c->b);
  cairo_rectangle (cr, W - c->r, mbar, c->r, H - mbar - c->b);
  cairo_fill (cr);
  cairo_set_operator (cr, CAIRO_OPERATOR_SOURCE);
  nhwl_csd_source (cr, fg, bg, 0.0);
  cairo_rectangle (cr, p.strip.x, p.strip.y, p.strip.w, p.strip.h);
  cairo_fill (cr);
  nhwl_csd_source (cr, fg, bg, 0.15);
  cairo_rectangle (cr, p.strip.x, p.strip.y + p.strip.h - 1, p.strip.w, 1);
  cairo_fill (cr);
  cairo_restore (cr);

  /* A floating window's top corners are rounded: Glitnir's corner kernel
     replaces the strip's corner squares, transparent outside the arc.  */
  if (round && p.strip.w > 0)
    {
      uint32_t fill = (0xff000000u | (RED_FROM_ULONG (bg) << 16)
		       | (GREEN_FROM_ULONG (bg) << 8) | BLUE_FROM_ULONG (bg));
      GlitnirRect run = { p.strip.x, p.strip.y, p.strip.w, p.strip.h };

      cairo_surface_flush (out->cr_surface);
      glitnir_decor_corners (cairo_image_surface_get_data (out->cr_surface),
			     cairo_image_surface_get_stride (out->cr_surface),
			     run, NHWL_CSD_RADIUS, fill, fill, 0, 0, 1 | 2, s);
      cairo_surface_mark_dirty (out->cr_surface);
    }

  cairo_save (cr);
  cairo_set_operator (cr, CAIRO_OPERATOR_OVER);
  for (i = 0; i < (int) min (p.nbuttons, NHG_CSD_MAX_BUTTONS); i++)
    nhwl_csd_draw_button (cr, p.button[i].kind, p.button[i].r.x,
			  p.button[i].r.y, p.button[i].r.w, fg, bg,
			  p.button[i].state, s);

  /* The title, centred between the buttons (GNOME's place), in the style's
     font at the device size.  */
  tl = p.title.x;
  cairo_rectangle (cr, p.title.x, p.title.y, p.title.w, p.title.h);
  cairo_clip (cr);
  cairo_select_font_face (cr, family, CAIRO_FONT_SLANT_NORMAL,
			  CAIRO_FONT_WEIGHT_BOLD);
  cairo_set_font_size (cr, px * s);
  cairo_font_extents (cr, &fe);
  cairo_text_extents (cr, name, &te);
  nhwl_csd_source (cr, fg, bg, 1.0);
  cairo_move_to (cr, max (tl + 8 * s, (W - te.x_advance) / 2),
		 mbar + (p.strip.h - (fe.ascent + fe.descent)) / 2 + fe.ascent);
  cairo_show_text (cr, name);
  cairo_restore (cr);
  cairo_surface_mark_dirty (out->cr_surface);

  r[n++] = (GlitnirRect) { 0, 0, W, c->t };
  if (c->b)
    r[n++] = (GlitnirRect) { 0, H - c->b, W, c->b };
  if (c->l)
    r[n++] = (GlitnirRect) { 0, c->t, c->l, H - c->t - c->b };
  if (c->r)
    r[n++] = (GlitnirRect) { W - c->r, c->t, c->r, H - c->t - c->b };
  nhwl_glitnir_ensure (f);
  glitnir_damage_add_n (out->glitnir_id, r, n);
}

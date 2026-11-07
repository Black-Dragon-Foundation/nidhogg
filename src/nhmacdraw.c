/* Naglfar's side of the drawing seam (HAVE_NHMAC): the table of
   primitives under nhwldraw.c's glyph-string walk (struct nhwl_draw_ops)
   and the drawing entries of the redisplay interface that are Cairo code
   on Wayland.  The backing is the glue's, as on Wayland; the shell copies
   what changed into the window's IOSurfaces at each publish.  Nothing
   draws into it yet: every drawing entry accepts its call and leaves the
   frame's pixels as they are.

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

/* The backing.  */

nhwl_cr *
nhwl_begin_cr_clip (struct frame *f)
{
  return NULL;
}

void
nhwl_end_cr_clip (struct frame *f)
{
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

void
nhwl_fill_background_by_face (struct frame *f, struct face *face, int x,
			      int y, int width, int height)
{
}

/* The redisplay interface's drawing.  */

void
nhwl_draw_fringe_bitmap (struct window *w, struct glyph_row *row,
			 struct draw_fringe_bitmap_params *p)
{
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
}

void
nhwl_draw_window_divider (struct window *w, int x0, int x1, int y0, int y1)
{
}

/* The glyph-string walk's primitives.  */

static void
nhmac_save (nhwl_cr *cr)
{
}

static void
nhmac_clipping (struct glyph_string *s, nhwl_cr *cr)
{
}

static void
nhmac_clipping_exactly (struct glyph_string *s, struct glyph_string *dst,
			nhwl_cr *cr)
{
}

static void
nhmac_fill (struct frame *f, unsigned long color, int x, int y, int w,
	    int h, bool respect_alpha_background)
{
}

static void
nhmac_background (struct glyph_string *s, bool force_p)
{
}

static void
nhmac_string (struct glyph_string *s)
{
}

static void
nhmac_underwave (struct glyph_string *s, unsigned long color)
{
}

static void
nhmac_underline (struct frame *f, struct glyph_string *s,
		 unsigned long color, enum face_underline_type type,
		 int width, int segment, int position)
{
}

static void
nhmac_hollow_cursor (struct window *w, struct glyph_row *row)
{
}

static void
nhmac_bar_cursor (struct window *w, struct glyph_row *row, int width,
		  enum text_cursor_kinds kind)
{
}

static const struct nhwl_draw_ops nhmac_draw_ops = {
  nhwl_begin_cr_clip, nhwl_end_cr_clip, nhmac_save, nhmac_save,
  nhmac_clipping, nhmac_clipping_exactly, nhmac_fill, nhmac_background,
  nhmac_string, nhmac_string, nhmac_string, nhmac_string, nhmac_string,
  nhmac_string, nhmac_underwave, nhmac_underline, nhmac_hollow_cursor,
  nhmac_bar_cursor,
};
const struct nhwl_draw_ops *nhwl_draw_ops = &nhmac_draw_ops;

/* Glitnir (the compositor): decorations drawn on the shell's box path.

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

/* Rounded and open runs of a face's :box (:corner-radius, :open).  The glyph
   string's background and text are already drawn when the box is; this file
   repaints the corners outside each arc in the colour beneath, then draws the
   ring along the rounded path.  Edges and line widths go through
   glitnir_snap_rect / glitnir_snap_line at the surface's device scale
   (1 on the shell, whose frames are device pixels).

   From P1.5/PD's pgtk file (gl15-i2f3), re-pointed to the shell (W1
   Task 16): the backing is nhwl's, damage goes through nhwl_damage, and a
   frame has one Glitnir layer, so a window's label anchors live in its
   frame's layer, clipped to the window's box.  */

#include <config.h>

#ifdef HAVE_NHWL

#include <math.h>
#include <string.h>
#include <cairo-ft.h>

#include "lisp.h"
#include "frame.h"
#include "window.h"
#include "dispextern.h"
#include "font.h"
#include "ftfont.h"
#include "nhwlterm.h"

enum { SIDE_LEFT, SIDE_RIGHT };

struct box_keys
{
  double radius;
  bool open_top, open_bottom, open_left, open_right;
};

/* Read the decoration keys of face FACE's :box plist.  True when it has
   :corner-radius > 0 or :open.  */

static bool
box_keys (struct face *face, struct box_keys *k)
{
  Lisp_Object box = face->lface[LFACE_BOX_INDEX];

  memset (k, 0, sizeof *k);
  for (; CONSP (box) && CONSP (XCDR (box)); box = XCDR (XCDR (box)))
    {
      Lisp_Object key = XCAR (box), val = XCAR (XCDR (box));
      if (EQ (key, QCcorner_radius) && NUMBERP (val))
	k->radius = XFLOATINT (val);
      else if (EQ (key, QCopen))
	for (; CONSP (val); val = XCDR (val))
	  {
	    Lisp_Object o = XCAR (val);
	    k->open_top |= EQ (o, Qtop);
	    k->open_bottom |= EQ (o, Qbottom);
	    k->open_left |= EQ (o, Qleft);
	    k->open_right |= EQ (o, Qright);
	  }
    }
  return k->radius > 0
	 || k->open_top || k->open_bottom || k->open_left || k->open_right;
}

static bool
face_bg (struct frame *f, struct glyph *g, unsigned long *out)
{
  struct face *face = FACE_FROM_ID_OR_NULL (f, g->face_id);
  if (face == NULL)
    return false;
  *out = face->background;
  return true;
}

unsigned long
glitnir_decor_colour_beneath (struct glyph_string *s, int side)
{
  struct frame *f = s->f;
  struct glyph_row *row = s->row;
  struct glyph *area = row->glyphs[s->area];
  struct glyph *end = area + row->used[s->area];
  struct glyph *last = s->first_glyph + s->nchars - 1;
  struct glyph *nb[2] = { s->first_glyph > area ? s->first_glyph - 1 : NULL,
			  last + 1 < end ? last + 1 : NULL };
  unsigned long c;

  if (s->cmp || s->img)
    nb[SIDE_RIGHT] = s->first_glyph + 1 < end ? s->first_glyph + 1 : NULL;
  if (nb[side] && face_bg (f, nb[side], &c))
    return c;
  if (nb[!side] && face_bg (f, nb[!side], &c))
    return c;
  if (row->used[s->area] > 0)
    {
      struct face *lf = FACE_FROM_ID_OR_NULL (f, end[-1].face_id);
      if (lf && FACE_EXTENSIBLE_P (lf))
	return lf->background;
    }
  int id = lookup_basic_face (s->w, f, DEFAULT_FACE_ID);
  struct face *def = FACE_FROM_ID_OR_NULL (f, id);
  return def ? def->background : s->face->background;
}

/* A rounded rectangle path; radii clockwise from the top left.  */

static void
rrect_path (cairo_t *cr, double x, double y, double w, double h,
	    double tl, double tr, double br, double bl)
{
  cairo_new_sub_path (cr);
  cairo_move_to (cr, x + tl, y);
  cairo_line_to (cr, x + w - tr, y);
  if (tr > 0)
    cairo_arc (cr, x + w - tr, y + tr, tr, -M_PI / 2, 0);
  cairo_line_to (cr, x + w, y + h - br);
  if (br > 0)
    cairo_arc (cr, x + w - br, y + h - br, br, 0, M_PI / 2);
  cairo_line_to (cr, x + bl, y + h);
  if (bl > 0)
    cairo_arc (cr, x + bl, y + h - bl, bl, M_PI / 2, M_PI);
  cairo_line_to (cr, x, y + tl);
  if (tl > 0)
    cairo_arc (cr, x + tl, y + tl, tl, M_PI, 3 * M_PI / 2);
  cairo_close_path (cr);
}

/* Corner I (0 top left, then clockwise) of the rounded rectangle X, Y, W, H
   with radius RAD, as a closed sub-path: the part outside the arc (OUTSIDE)
   or the quarter disc inside it.  The arc is rrect_path's, so the edges
   meet exactly.  */

static void
corner_path (cairo_t *cr, double x, double y, double w, double h,
	     int i, double rad, bool outside)
{
  static const double a0[4] = { M_PI, -M_PI / 2, 0, M_PI / 2 };
  bool left = i == 0 || i == 3, top = i < 2;
  double cx = left ? x + rad : x + w - rad;
  double cy = top ? y + rad : y + h - rad;

  cairo_new_sub_path (cr);
  if (outside)
    cairo_move_to (cr, left ? x : x + w, top ? y : y + h);
  else
    cairo_move_to (cr, cx, cy);
  cairo_arc (cr, cx, cy, rad, a0[i], a0[i] + M_PI / 2);
  cairo_close_path (cr);
}

static void
set_colour (struct frame *f, unsigned long pixel)
{
  nhwl_set_cr_source_with_color (f, pixel, false);
}

/* The ring of a box on the outer rectangle R (snapped), along the rounded
   path with corner radii RTL..RBL: one even-odd fill of the outer and the
   inner path, without a clip (a curved clip costs Cairo a mask per box).
   On open sides and at run continuations the inner edge lies on the outer
   one, so the two cancel and nothing is drawn there.  */

static void
draw_ring (cairo_t *cr, struct frame *f, struct face *face,
	   struct box_keys *k, GlitnirRectF r, double hw, double vw,
	   bool left_p, bool right_p,
	   double rtl, double rtr, double rbr, double rbl)
{
  double ix0 = (left_p && !k->open_left) ? r.x + vw : r.x;
  double ix1 = (right_p && !k->open_right) ? r.x + r.w - vw : r.x + r.w;
  double iy0 = k->open_top ? r.y : r.y + hw;
  double iy1 = k->open_bottom ? r.y + r.h : r.y + r.h - hw;
  double lw = fmax (hw, vw);
  double ritl = fmax (0, rtl - lw), ritr = fmax (0, rtr - lw);
  double ribr = fmax (0, rbr - lw), ribl = fmax (0, rbl - lw);

  cairo_save (cr);
  rrect_path (cr, r.x, r.y, r.w, r.h, rtl, rtr, rbr, rbl);
  rrect_path (cr, ix0, iy0, ix1 - ix0, iy1 - iy0, ritl, ritr, ribr, ribl);
  cairo_set_fill_rule (cr, CAIRO_FILL_RULE_EVEN_ODD);
  if (face->box == FACE_SIMPLE_BOX)
    {
      set_colour (f, face->box_color);
      cairo_fill (cr);
    }
  else
    {
      /* Raised/sunken: the relief colours, split along the anti-diagonal.  */
      bool raised = face->box == FACE_RAISED_BOX;
      struct nhwl_output *o = FRAME_X_OUTPUT (f);
      unsigned long tlc = raised ? o->white_relief.xgcv.foreground
				 : o->black_relief.xgcv.foreground;
      unsigned long brc = raised ? o->black_relief.xgcv.foreground
				 : o->white_relief.xgcv.foreground;
      cairo_path_t *ring = cairo_copy_path (cr);

      cairo_new_path (cr);
      cairo_move_to (cr, r.x - 1, r.y - 1);
      cairo_line_to (cr, r.x + r.w + 1, r.y - 1);
      cairo_line_to (cr, r.x - 1, r.y + r.h + 1);
      cairo_close_path (cr);
      cairo_save (cr);
      cairo_clip (cr);
      cairo_append_path (cr, ring);
      set_colour (f, tlc);
      cairo_fill (cr);
      cairo_restore (cr);

      cairo_new_path (cr);
      cairo_move_to (cr, r.x + r.w + 1, r.y - 1);
      cairo_line_to (cr, r.x + r.w + 1, r.y + r.h + 1);
      cairo_line_to (cr, r.x - 1, r.y + r.h + 1);
      cairo_close_path (cr);
      cairo_save (cr);
      cairo_clip (cr);
      cairo_append_path (cr, ring);
      set_colour (f, brc);
      cairo_fill (cr);
      cairo_restore (cr);
      cairo_path_destroy (ring);
    }
  cairo_restore (cr);
}

static uint32_t
premul (struct frame *f, unsigned long pixel)
{
  Emacs_Color c = { .pixel = pixel };
  nhwl_query_color (f, &c);
  return (0xff000000u | (uint32_t) (c.red >> 8) << 16
	  | (uint32_t) (c.green >> 8) << 8 | (uint32_t) (c.blue >> 8));
}

/* Box R (snapped) of FACE with corner radius RAD through Glitnir's coverage
   kernel (glitnir_decor_corners) for the CORNERS (1 top left, then
   clockwise) and Cairo rectangles on the device grid for the ring's straight
   EDGES (1 top, 2 right, 4 bottom, 8 left): a Cairo fill along a curve costs
   several microseconds, the kernel's cached tile one OVER or copy pass
   (R-D1).  BENEATH[SIDE] is the colour outside the arcs on each side.  FILL
   0 composes over the box just drawn; else the corner squares are repainted,
   FILL inside the ring.  False, with nothing drawn, when the kernel cannot
   draw it the way draw_ring would: a raised or sunken box, unequal line
   widths, a context transformed other than by a whole-pixel translation
   (the shell's title-bar offset), a surface that is not an image, or a run
   not wholly on the surface or a corner square outside CLIP.  */

static bool
kernel_box (cairo_t *cr, struct frame *f, struct face *face, GlitnirRectF r,
	    double rad, double hw, double vw, int edges, unsigned corners,
	    const unsigned long beneath[2], uint32_t fill, XRectangle clip)
{
  cairo_surface_t *sf = cairo_get_target (cr);
  cairo_matrix_t m;
  double sx, sy, ox, oy, tx, ty;

  cairo_get_matrix (cr, &m);
  cairo_surface_get_device_scale (sf, &sx, &sy);
  cairo_surface_get_device_offset (sf, &ox, &oy);
  tx = m.x0, ty = m.y0;
  if (face->box != FACE_SIMPLE_BOX || hw != vw || sx != sy || ox || oy
      || m.xx != 1 || m.yy != 1 || m.xy || m.yx
      || tx * sx != floor (tx * sx) || ty * sx != floor (ty * sx)
      || cairo_surface_get_type (sf) != CAIRO_SURFACE_TYPE_IMAGE
      || (cairo_image_surface_get_format (sf) != CAIRO_FORMAT_ARGB32
	  && cairo_image_surface_get_format (sf) != CAIRO_FORMAT_RGB24))
    return false;

  /* Device pixels of the surface; the rectangles below go back to user
     space (the translation taken off).  */
  int x0 = lround ((r.x + tx) * sx), y0 = lround ((r.y + ty) * sx);
  int x1 = lround ((r.x + r.w + tx) * sx);
  int y1 = lround ((r.y + r.h + ty) * sx);
  int b = lround (hw * sx);
  int R = min (lround (rad * sx), min (x1 - x0, y1 - y0) / 2);
  int cx0 = max (0, (int) ceil ((clip.x + tx) * sx));
  int cy0 = max (0, (int) ceil ((clip.y + ty) * sx));
  int cx1 = min (cairo_image_surface_get_width (sf),
		 (int) floor ((clip.x + (double) clip.width + tx) * sx));
  int cy1 = min (cairo_image_surface_get_height (sf),
		 (int) floor ((clip.y + (double) clip.height + ty) * sx));
  if (R <= 0)
    corners = 0;
  if (x0 < 0 || y0 < 0 || x1 <= x0 || y1 <= y0
      || x1 > cairo_image_surface_get_width (sf)
      || y1 > cairo_image_surface_get_height (sf))
    return false;
  for (int i = 0; i < 4; i++)
    if (corners & (1u << i))
      {
	int tx = (i == 0 || i == 3) ? x0 : x1 - R;
	int ty = i < 2 ? y0 : y1 - R;
	if (tx < cx0 || ty < cy0 || tx + R > cx1 || ty + R > cy1)
	  return false;
      }

  /* The straight edges, between the corner squares.  */
  int c[4];
  for (int i = 0; i < 4; i++)
    c[i] = corners & (1u << i) ? R : 0;
  if (edges & 1)
    cairo_rectangle (cr, (x0 + c[0]) / sx - tx, y0 / sx - ty,
		     (x1 - c[1] - x0 - c[0]) / sx, b / sx);
  if (edges & 2)
    cairo_rectangle (cr, (x1 - b) / sx - tx, (y0 + c[1]) / sx - ty,
		     b / sx, (y1 - c[2] - y0 - c[1]) / sx);
  if (edges & 4)
    cairo_rectangle (cr, (x0 + c[3]) / sx - tx, (y1 - b) / sx - ty,
		     (x1 - c[2] - x0 - c[3]) / sx, b / sx);
  if (edges & 8)
    cairo_rectangle (cr, x0 / sx - tx, (y0 + c[0]) / sx - ty,
		     b / sx, (y1 - c[3] - y0 - c[0]) / sx);
  set_colour (f, face->box_color);
  cairo_fill (cr);

  if (corners)
    {
      uint32_t ring = premul (f, face->box_color);
      GlitnirRect run = { x0, y0, x1 - x0, y1 - y0 };
      cairo_surface_flush (sf);
      for (int side = SIDE_LEFT; side <= SIDE_RIGHT; side++)
	{
	  unsigned mask = corners & (side == SIDE_LEFT ? 9 : 6);
	  if (mask)
	    glitnir_decor_corners (cairo_image_surface_get_data (sf),
				   cairo_image_surface_get_stride (sf), run,
				   R / sx, fill, ring,
				   premul (f, beneath[side]), b / sx, mask, sx);
	}
      cairo_surface_mark_dirty_rectangle (sf, x0, y0, x1 - x0, y1 - y0);
    }
  return true;
}

/* The box of S with Glitnir's keys (nhwl_draw_glyph_string_box's hook);
   false when S's face has none, and the stock box draws.  */

bool
glitnir_decor_draw_box (struct glyph_string *s, int left_x, int top_y,
			int right_x, int bottom_y, int hwidth, int vwidth,
			bool left_p, bool right_p, XRectangle *clip)
{
  struct frame *f = s->f;
  struct box_keys k;

  if (!nhwl_glitnir_active_p (f) || !box_keys (s->face, &k))
    return false;

  cairo_t *cr = nhwl_begin_cr_clip (f);
  double sx = 1, sy = 1;
  cairo_surface_get_device_scale (cairo_get_target (cr), &sx, &sy);

  /* The outer rectangle and the line widths on the device grid.  */
  GlitnirRectF r = glitnir_snap_rect (left_x, top_y, right_x + 1 - left_x,
				      bottom_y + 1 - top_y, sx);
  double hw = glitnir_snap_line (hwidth, sx);
  double vw = glitnir_snap_line (vwidth, sx);

  /* Only true run ends are rounded, and not at an open side.  */
  bool tl = left_p && !k.open_top && !k.open_left;
  bool bl = left_p && !k.open_bottom && !k.open_left;
  bool tr = right_p && !k.open_top && !k.open_right;
  bool br = right_p && !k.open_bottom && !k.open_right;
  double rad = fmin (k.radius, fmin (r.h / 2, r.w / 2));
  double rtl = tl ? rad : 0, rbl = bl ? rad : 0;
  double rtr = tr ? rad : 0, rbr = br ? rad : 0;

  cairo_rectangle (cr, clip->x, clip->y, clip->width, clip->height);
  cairo_clip (cr);

  /* Open sides: no line, but the band an outside line would cover gets the
     glyph string's background, which nhwl_draw_glyph_string_background and
     the text only paint inside that band.  */
  bool ht = s->face->box_horizontal_line_width > 0;
  bool vt = s->face->box_vertical_line_width > 0;
  if (k.open_top && ht)
    cairo_rectangle (cr, r.x, r.y, r.w, hw);
  if (k.open_bottom && ht)
    cairo_rectangle (cr, r.x, r.y + r.h - hw, r.w, hw);
  if (k.open_left && left_p && vt)
    cairo_rectangle (cr, r.x, r.y, vw, r.h);
  if (k.open_right && right_p && vt)
    cairo_rectangle (cr, r.x + r.w - vw, r.y, vw, r.h);
  nhwl_set_cr_source_with_color (f, s->xgcv.background, true);
  cairo_fill (cr);

  /* Corners outside the arcs: the colour beneath, over the square
     background already drawn there; then the ring.  */
  double rads[4] = { rtl, rtr, rbr, rbl };
  unsigned corners = 0;
  for (int i = 0; i < 4; i++)
    corners |= (rads[i] > 0) << i;
  unsigned long under[2] = { 0, 0 };
  if (corners & 9)
    under[SIDE_LEFT] = glitnir_decor_colour_beneath (s, SIDE_LEFT);
  if (corners & 6)
    under[SIDE_RIGHT] = glitnir_decor_colour_beneath (s, SIDE_RIGHT);
  int edges = ((!k.open_top) | (right_p && !k.open_right) << 1
	       | (!k.open_bottom) << 2 | (left_p && !k.open_left) << 3);
  if (kernel_box (cr, f, s->face, r, rad, hw, vw, edges, corners, under, 0,
		  *clip))
    {
      nhwl_end_cr_clip (f);
      return true;
    }

  /* Else Cairo: one fill per side, no clip.  */
  for (int side = SIDE_LEFT; side <= SIDE_RIGHT; side++)
    if (corners & (side == SIDE_LEFT ? 9 : 6))
      {
	for (int i = 0; i < 4; i++)
	  if (rads[i] > 0 && (i == 0 || i == 3) == (side == SIDE_LEFT))
	    corner_path (cr, r.x, r.y, r.w, r.h, i, rads[i], true);
	set_colour (f, under[side]);
	cairo_fill (cr);
      }

  draw_ring (cr, f, s->face, &k, r, hw, vw, left_p, right_p, rtl, rtr, rbr, rbl);

  nhwl_end_cr_clip (f);
  return true;
}

/* Repaint the corners of the rounded run end that the glyph on S's row
   beside S (NB, on SIDE of S) carries, after S's redraw may have painted over
   them: the r x r squares in the colour beneath (S's own background there),
   the run's rounded background, then the ring.  The run is NB and the glyphs
   of its face before it (SIDE_LEFT) or after it (SIDE_RIGHT).  */

static void
repaint_neighbour_end (struct glyph_string *s, struct glyph *nb, int side)
{
  struct frame *f = s->f;
  struct face *face = FACE_FROM_ID_OR_NULL (f, nb->face_id);
  struct box_keys k;

  if (face == NULL || face == s->face || face->box == FACE_NO_BOX
      || !box_keys (face, &k) || k.radius <= 0)
    return;
  if (side == SIDE_LEFT ? !nb->right_box_line_p : !nb->left_box_line_p)
    return;
  if (side == SIDE_LEFT ? k.open_right : k.open_left)
    return;

  struct glyph_row *row = s->row;
  struct glyph *area = row->glyphs[s->area];
  struct glyph *end = area + row->used[s->area];
  struct glyph *a = nb, *b = nb;
  int w = nb->pixel_width;
  bool left_p, right_p;

  if (side == SIDE_LEFT)
    {
      while (a > area && a[-1].face_id == nb->face_id && !a->left_box_line_p)
	w += (--a)->pixel_width;
      left_p = a->left_box_line_p;
      right_p = true;
    }
  else
    {
      while (b + 1 < end && b[1].face_id == nb->face_id
	     && !b->right_box_line_p)
	w += (++b)->pixel_width;
      left_p = true;
      right_p = b->right_box_line_p;
    }

  int left_x = side == SIDE_LEFT ? s->x - w : s->x + s->width;
  int top_y = s->y, h = s->height;
  struct glyph *beside = side == SIDE_LEFT ? s->first_glyph
					   : s->first_glyph + s->nchars - 1;
  unsigned long beneath;
  if (!face_bg (f, beside, &beneath))
    beneath = s->face->background;

  cairo_t *cr = nhwl_begin_cr_clip (f);
  double sx = 1, sy = 1;
  cairo_surface_get_device_scale (cairo_get_target (cr), &sx, &sy);
  GlitnirRectF r = glitnir_snap_rect (left_x, top_y, w, h, sx);
  double hw = glitnir_snap_line (eabs (face->box_horizontal_line_width), sx);
  double vw = glitnir_snap_line (eabs (face->box_vertical_line_width), sx);
  double rad = fmin (k.radius, fmin (r.h / 2, r.w / 2));
  bool top = !k.open_top, bot = !k.open_bottom;
  bool l = left_p && !k.open_left, rr = right_p && !k.open_right;
  double rtl = (l && top) ? rad : 0, rbl = (l && bot) ? rad : 0;
  double rtr = (rr && top) ? rad : 0, rbr = (rr && bot) ? rad : 0;

  NativeRectangle clip;
  get_glyph_string_clip_rect (s, &clip);
  cairo_rectangle (cr, left_x - 1, clip.y, w + 2, clip.height);
  cairo_clip (cr);

  /* The two corner squares at the end facing S (corners 0 top left, then
     clockwise): the colour beneath, then the run's quarter disc; then the
     ring there.  The coverage kernel when it can; else Cairo.  */
  unsigned long under[2] = { beneath, beneath };
  XRectangle end_clip = { left_x - 1, clip.y, w + 2, clip.height };
  bool kernel_p
    = kernel_box (cr, f, face, r, rad, hw, vw,
		  side == SIDE_LEFT ? (rr ? 2 : 0) : (l ? 8 : 0),
		  (side == SIDE_LEFT ? (rtr > 0) << 1 | (rbr > 0) << 2
		   : (rtl > 0) | (rbl > 0) << 3),
		  under, premul (f, face->background), end_clip);
  struct { int i; double rad, x, y; } sq[2];
  if (side == SIDE_LEFT)
    {
      sq[0].i = 1, sq[0].rad = rtr, sq[0].x = r.x + r.w - rtr, sq[0].y = r.y;
      sq[1].i = 2, sq[1].rad = rbr, sq[1].x = r.x + r.w - rbr;
      sq[1].y = r.y + r.h - rbr;
    }
  else
    {
      sq[0].i = 0, sq[0].rad = rtl, sq[0].x = r.x, sq[0].y = r.y;
      sq[1].i = 3, sq[1].rad = rbl, sq[1].x = r.x, sq[1].y = r.y + r.h - rbl;
    }
  for (int i = 0; i < 2 && !kernel_p; i++)
    if (sq[i].rad > 0)
      {
	cairo_rectangle (cr, sq[i].x, sq[i].y, sq[i].rad, sq[i].rad);
	set_colour (f, beneath);
	cairo_fill (cr);
	corner_path (cr, r.x, r.y, r.w, r.h, sq[i].i, sq[i].rad, false);
	set_colour (f, face->background);
	cairo_fill (cr);
      }
  if (!kernel_p)
    {
      cairo_save (cr);
      cairo_rectangle (cr, side == SIDE_LEFT ? r.x + r.w - rad : r.x,
		       r.y, rad, r.h);
      cairo_clip (cr);
      draw_ring (cr, f, face, &k, r, hw, vw, left_p, right_p,
		 rtl, rtr, rbr, rbl);
      cairo_restore (cr);
    }
  nhwl_end_cr_clip (f);

  nhwl_damage (f, side == SIDE_LEFT ? (int) (r.x + r.w - rad) : left_x,
	       clip.y, (int) ceil (rad) + 1, clip.height);
}

/* Called before S is drawn (nhwl_damage_glyphs): a row neighbour of S
   that ends a rounded run has its corner squares repainted, since S's
   background or overhang may reach into them.  The neighbours come from S's
   row, never from S->prev / S->next.  */

void
glitnir_decor_after_glyphs (struct glyph_string *s)
{
  if (!nhwl_glitnir_active_p (s->f) || s->row == NULL
      || s->first_glyph == NULL || s->nchars <= 0)
    return;

  struct glyph_row *row = s->row;
  struct glyph *area = row->glyphs[s->area];
  struct glyph *end = area + row->used[s->area];
  struct glyph *last = s->first_glyph + s->nchars - 1;

  if (s->first_glyph > area)
    repaint_neighbour_end (s, s->first_glyph - 1, SIDE_LEFT);
  if (last + 1 < end)
    repaint_neighbour_end (s, last + 1, SIDE_RIGHT);
}

/* Labels.  A face whose :box has :label STRING floats a pill with STRING
   above the first glyph of each run of that face.  This file keeps Glitnir's
   anchors true to the pixels (every glyph string drops the anchors of what it
   draws and records the one it starts, clears drop what they cover, scrolls
   move them) and renders the pill once per (text, family, size, colours,
   scale) with Cairo, in the face's own font (the shell has no Pango); the
   scene places and composes it (decor.rs).  */

static Lisp_Object
face_label (struct face *face)
{
  if (face == NULL || face->box == FACE_NO_BOX)
    return Qnil;
  Lisp_Object box = face->lface[LFACE_BOX_INDEX];
  for (; CONSP (box) && CONSP (XCDR (box)); box = XCDR (XCDR (box)))
    if (EQ (XCAR (box), QClabel) && STRINGP (XCAR (XCDR (box))))
      return XCAR (XCDR (box));
  return Qnil;
}

static bool
same_label (Lisp_Object a, Lisp_Object b)
{
  return STRINGP (a) && STRINGP (b) && SBYTES (a) == SBYTES (b)
	 && memcmp (SDATA (a), SDATA (b), SBYTES (a)) == 0;
}

/* The device-pixel rectangle of the frame pixels X, Y, W, H, relative to
   the layer whose origin is OX, OY (device px).  */

static GlitnirRect
local_rect (double sx, double sy, int ox, int oy, int x, int y, int w, int h)
{
  int x0 = floor (x * sx) - ox, y0 = floor (y * sy) - oy;
  int x1 = ceil ((x + w) * sx) - ox, y1 = ceil ((y + h) * sy) - oy;
  return (GlitnirRect) { x0, y0, max (x1 - x0, 0), max (y1 - y0, 0) };
}

/* The origin OX, OY (frame pixels) of the layer window W's anchors live
   in, and W's box in that layer (CLIP): the layer is W's frame's one
   layer, which starts where nhwl_layer_origin says.  */

static void
layer_box (struct window *w, double sx, double sy, int *ox, int *oy,
	   GlitnirRect *clip)
{
  int x, y, wd, ht;
  window_box (w, ANY_AREA, &x, &y, &wd, &ht);
  nhwl_layer_origin (WINDOW_XFRAME (w), ox, oy);
  *clip = local_rect (sx, sy, *ox, *oy, x, y, wd, ht);
}

static uint64_t
hash_bytes (uint64_t h, const void *p, size_t n)
{
  const unsigned char *b = p;
  for (size_t i = 0; i < n; i++)
    h = (h ^ b[i]) * 1099511628211u;
  return h;
}

static void
pixel_rgb (struct frame *f, unsigned long pixel, double *r, double *g,
	   double *b)
{
  Emacs_Color col;
  col.pixel = pixel;
  nhwl_query_color (f, &col);
  *r = col.red / 65535.0;
  *g = col.green / 65535.0;
  *b = col.blue / 65535.0;
}

/* A Cairo font for a label at PX device pixels: the font of FACE (the
   shell's fonts are Cairo's, ftcrfont) at that size, else a Cairo font of
   FACE's family.  NULL on failure; the caller destroys it.  */

static cairo_scaled_font_t *
label_font (struct face *face, double px)
{
  Lisp_Object fam = face->lface[LFACE_FAMILY_INDEX];
  cairo_font_face_t *ff, *toy = NULL;
  cairo_matrix_t m, ctm;
  cairo_font_options_t *o;
  cairo_scaled_font_t *sf;

  if (face->font && face->font->driver == &ftcrfont_driver
      && ((struct font_info *) face->font)->cr_scaled_font)
    ff = cairo_scaled_font_get_font_face
      (((struct font_info *) face->font)->cr_scaled_font);
  else
    ff = toy = cairo_toy_font_face_create (STRINGP (fam) ? SSDATA (fam)
					   : "monospace",
					   CAIRO_FONT_SLANT_NORMAL,
					   CAIRO_FONT_WEIGHT_NORMAL);
  cairo_matrix_init_scale (&m, px, px);
  cairo_matrix_init_identity (&ctm);
  o = cairo_font_options_create ();
  sf = cairo_scaled_font_create (ff, &m, &ctm, o);
  cairo_font_options_destroy (o);
  if (toy)
    cairo_font_face_destroy (toy);
  if (cairo_scaled_font_status (sf) != CAIRO_STATUS_SUCCESS)
    {
      cairo_scaled_font_destroy (sf);
      return NULL;
    }
  return sf;
}

/* Render LABEL's pill (background BG, text TEXT) in the font and at 0.85 of
   the size of S's face, at device scale SX, and cache it under KEY.  The
   text is laid out glyph by glyph from the font's character map (no
   shaping: a label is a short tag).  */

static bool
sprite_render (struct glyph_string *s, Lisp_Object label, double sx,
	       unsigned long bg, unsigned long text, uint64_t key)
{
  struct face *face = s->face;
  double px = (face->font ? face->font->pixel_size
	       : FRAME_LINE_HEIGHT (s->f) * 0.8) * 0.85 * sx;
  cairo_scaled_font_t *sf = label_font (face, px);
  cairo_glyph_t *glyphs = NULL;
  int n = 0;
  bool ok = false;

  if (!sf)
    return false;
  if (cairo_scaled_font_text_to_glyphs (sf, 0, 0, SSDATA (label),
					SBYTES (label), &glyphs, &n, NULL,
					NULL, NULL) != CAIRO_STATUS_SUCCESS)
    {
      cairo_scaled_font_destroy (sf);
      return false;
    }

  cairo_text_extents_t te;
  cairo_font_extents_t fe;
  cairo_scaled_font_glyph_extents (sf, glyphs, n, &te);
  cairo_scaled_font_extents (sf, &fe);
  int padx = (int) ceil (px * 0.5), pady = (int) ceil (px * 0.2);
  int w = (int) ceil (te.x_advance) + 2 * padx;
  int h = (int) ceil (fe.ascent + fe.descent) + 2 * pady;

  if (w > 0 && h > 0 && w <= 4096 && h <= 4096)
    {
      cairo_surface_t *img = cairo_image_surface_create (CAIRO_FORMAT_ARGB32,
							 w, h);
      cairo_t *cr = cairo_create (img);
      double r, g, b;
      pixel_rgb (s->f, bg, &r, &g, &b);
      cairo_set_source_rgb (cr, r, g, b);
      rrect_path (cr, 0, 0, w, h, h / 2.0, h / 2.0, h / 2.0, h / 2.0);
      cairo_fill (cr);
      pixel_rgb (s->f, text, &r, &g, &b);
      cairo_set_source_rgb (cr, r, g, b);
      cairo_set_scaled_font (cr, sf);
      for (int i = 0; i < n; i++)
	{
	  glyphs[i].x += padx;
	  glyphs[i].y += pady + fe.ascent;
	}
      cairo_show_glyphs (cr, glyphs, n);
      cairo_destroy (cr);
      cairo_surface_flush (img);
      ok = glitnir_decor_sprite_put (key, cairo_image_surface_get_data (img),
				     cairo_image_surface_get_stride (img),
				     w, h) == GLITNIR_OK;
      cairo_surface_destroy (img);
    }
  cairo_glyph_free (glyphs);
  cairo_scaled_font_destroy (sf);
  return ok;
}

void
glitnir_decor_record (struct glyph_string *s)
{
  struct window *w = s->w;
  if (w == NULL || s->row == NULL || s->first_glyph == NULL || s->nchars <= 0
      || s->row->mode_line_p)
    return;
  uint64_t root, layer;
  double sx, sy;
  if (!nhwl_decor_target (s->f, w, &root, &layer, &sx, &sy))
    return;

  int ox, oy;
  GlitnirRect wbox;
  layer_box (w, sx, sy, &ox, &oy, &wbox);
  GlitnirRect ext = local_rect (sx, sy, ox, oy, s->x, s->y, s->width,
				s->height);
  glitnir_decor_drop_extent (root, layer, ext);

  Lisp_Object label = face_label (s->face);
  if (!STRINGP (label))
    return;
  /* Only the first glyph of a run anchors: a left neighbour of the same
     label continues it (a different face, the cursor's, starts its own).  */
  struct glyph_row *row = s->row;
  struct glyph *area = row->glyphs[s->area];
  if (s->first_glyph > area && s->first_glyph[-1].type == CHAR_GLYPH
      && same_label (face_label (FACE_FROM_ID_OR_NULL (s->f,
						       s->first_glyph[-1]
						       .face_id)),
		     label))
    return;

  int did = lookup_basic_face (w, s->f, DEFAULT_FACE_ID);
  struct face *def = FACE_FROM_ID_OR_NULL (s->f, did);
  unsigned long bg = s->face->box_color;
  unsigned long text = def ? def->background : s->face->background;
  Lisp_Object fam = s->face->lface[LFACE_FAMILY_INDEX];
  uint64_t key = 14695981039346656037u;
  key = hash_bytes (key, SDATA (label), SBYTES (label));
  if (STRINGP (fam))
    key = hash_bytes (key, SDATA (fam), SBYTES (fam));
  int size = s->face->font ? s->face->font->pixel_size : 0;
  int scale = (int) lround (sx * 1000);
  key = hash_bytes (key, &size, sizeof size);
  key = hash_bytes (key, &bg, sizeof bg);
  key = hash_bytes (key, &text, sizeof text);
  key = hash_bytes (key, &scale, sizeof scale);
  if (glitnir_decor_sprite_has (key) != 1
      && !sprite_render (s, label, sx, bg, text, key))
    return;

  int gw = s->first_glyph->pixel_width;
  GlitnirAnchor a = {
    .glyph = local_rect (sx, sy, ox, oy, s->x, s->y, gw, s->height),
    .clip = wbox,
    .sprite = key,
    .row_top = wbox.y,
  };
  glitnir_decor_put (root, layer, &a);
}

/* Drop the anchors a clear of X, Y, W, H covers, in each leaf window of the
   tree at WINDOW (and its siblings).  */

static void
clear_windows (struct frame *f, Lisp_Object window, int x, int y, int w, int h)
{
  for (; WINDOWP (window); window = XWINDOW (window)->next)
    {
      struct window *win = XWINDOW (window);
      if (WINDOWP (win->contents))
	{
	  clear_windows (f, win->contents, x, y, w, h);
	  continue;
	}
      int wx, wy, wd, ht;
      window_box (win, ANY_AREA, &wx, &wy, &wd, &ht);
      int x0 = max (x, wx), y0 = max (y, wy);
      int x1 = min (x + w, wx + wd), y1 = min (y + h, wy + ht);
      uint64_t root, layer;
      double sx, sy;
      if (x1 <= x0 || y1 <= y0
	  || !nhwl_decor_target (f, win, &root, &layer, &sx, &sy))
	continue;
      int ox, oy;
      GlitnirRect wbox;
      layer_box (win, sx, sy, &ox, &oy, &wbox);
      glitnir_decor_drop_rect (root, layer,
			       local_rect (sx, sy, ox, oy, x0, y0,
					   x1 - x0, y1 - y0));
    }
}

/* A clear (nhwl_clear_area) of X, Y, W, H drops the anchors it covers.  */

void
glitnir_decor_clear (struct frame *f, int x, int y, int w, int h)
{
  if (nhwl_glitnir_active_p (f))
    clear_windows (f, f->root_window, x, y, w, h);
}

/* W's rows X, Y, WIDTH, HEIGHT moved by DY (nhwl_glitnir_scroll): their
   anchors move with them.  */

void
glitnir_decor_scrolled (struct frame *f, struct window *w, int x, int y,
			int width, int height, int dy)
{
  uint64_t root, layer;
  double sx, sy;
  if (!nhwl_decor_target (f, w, &root, &layer, &sx, &sy))
    return;
  int ox, oy;
  GlitnirRect wbox;
  layer_box (w, sx, sy, &ox, &oy, &wbox);
  glitnir_decor_scroll (root, layer,
			local_rect (sx, sy, ox, oy, x, y, width, height), dy);
}

#endif /* HAVE_NHWL */

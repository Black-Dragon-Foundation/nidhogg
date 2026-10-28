/* The bare client-side title bar of Nidhogg's own Wayland shell (HAVE_NHWL).
   Copyright (C) 2026 Free Software Foundation, Inc.

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

/* Where the compositor offers no server-side decorations (and the frame is
   not undecorated), the frame draws a title strip and keeps resize margins
   around itself, inside the one surface.  The backing is the native area
   plus this border (struct nhwl_csd): Emacs draws translated into the
   native area, and the strip is drawn here, in the same backing, so that
   Glitnir presents both.  The window geometry excludes the margins.  The
   strip has the frame's default colours, the frame title and three plain
   glyph buttons.  W1 (Task 19) makes it the full title bar: the buttons
   in GNOME's layout and order, the desktop's colours for an active and an
   inactive window (both handed over from Lisp by nhwl--set-title-bar-style,
   lisp/term/nhwl-settings.el), no resize margin on a tiled side, rounded
   top corners on a floating window (Glitnir's corner kernel), hover and
   pressed buttons, and the window menu from `appmenu'.  foot's render.c
   CSD was the reference; this is written fresh.  Everything here is device
   pixels except the protocol (the strip's 32, the margin's 8 and the
   corners' 12 are logical).  */

#include <config.h>

#include <math.h>

#include "lisp.h"
#include "nhwlterm.h"
#include "blockinput.h"
#include "coding.h"

#ifdef HAVE_NHWL

#define NHWL_CSD_STRIP 32	/* logical px */
#define NHWL_CSD_MARGIN 8
#define NHWL_CSD_RADIUS 12
#define NHWL_CSD_TITLE_PX 13
#define NHWL_CSD_DOUBLE_CLICK_NS 400000000ull
#define BTN_LEFT_CODE 0x110
#define BTN_RIGHT_CODE 0x111
#define NHWL_CSD_MAX_BUTTONS 8

enum { R_NONE, R_EDGE, R_STRIP, R_CLOSE, R_MAX, R_MIN, R_MENU };

/* The title bar's style, from Lisp (nhwl--set-title-bar-style): the
   buttons of each side (lists of `close', `maximize', `minimize' and
   `appmenu'), the colours of an active and an inactive window (strings,
   or nil for the frame's own), and the font (FAMILY . PIXELS); STYLE_GEN
   counts the changes.  Before Lisp says, the buttons are W0's.  */
static Lisp_Object csd_left, csd_right, csd_colours, csd_font;
static bool csd_styled;
static unsigned csd_style_gen;

#define NHWL_CSD_TILED (NHSHELL_STATE_TILED_L | NHSHELL_STATE_TILED_R \
			| NHSHELL_STATE_TILED_T | NHSHELL_STATE_TILED_B)

static uint32_t
nhwl_csd_s120 (struct frame *f)
{
  uint32_t s = FRAME_NHWL_OUTPUT (f)->scale120;

  return s ? s : 120;
}

/* Recompute the border from the compositor's decoration answer, the
   toplevel's states and the scale.  A fullscreen frame has none, a
   maximized one only the strip, and a tiled side no margin.  */
void
nhwl_csd_layout (struct frame *f)
{
  struct nhwl_output *out = FRAME_NHWL_OUTPUT (f);
  struct nhwl_display_info *dpyinfo = FRAME_DISPLAY_INFO (f);
  struct nhwl_csd *c = &out->csd;
  uint32_t s120 = nhwl_csd_s120 (f);
  int32_t ssd = 0;
  int m = 0, strip = 0, l, t, r, b;

  /* A child frame's offsets are its shadow margin (nhwlchild.c).  */
  if (nhwl_child_p (f))
    {
      nhwl_child_layout (f);
      return;
    }
  /* A popup frame has neither title bar nor margins.  */
  if (nhwl_popup_frame_p (f))
    return;
  /* The window's own answer: the compositor may grant one window server
     decorations and refuse another.  */
  c->on = (dpyinfo->shell
	   && nhshell_window_ssd (dpyinfo->shell, out->window, &ssd) >= 0
	   && !ssd && !FRAME_UNDECORATED (f));
  if (c->on && !(out->states & NHSHELL_STATE_FULLSCREEN))
    {
      strip = nhwl_to_device (NHWL_CSD_STRIP, s120);
      if (!(out->states & NHSHELL_STATE_MAXIMIZED))
	m = nhwl_to_device (NHWL_CSD_MARGIN, s120);
    }
  l = out->states & NHSHELL_STATE_TILED_L ? 0 : m;
  r = out->states & NHSHELL_STATE_TILED_R ? 0 : m;
  b = out->states & NHSHELL_STATE_TILED_B ? 0 : m;
  t = (out->states & NHSHELL_STATE_TILED_T ? 0 : m) + strip;
  if (c->m != m || c->strip != strip || c->l != l || c->t != t || c->r != r
      || c->b != b)
    {
      c->m = m;
      c->strip = strip;
      c->l = l, c->r = r, c->b = b, c->t = t;
      c->sig_valid = false;
      SET_FRAME_GARBAGED (f);
      nhwl_child_parent_layout (f);
    }
}

/* The title changed: the strip is drawn again at the next present.  */
void
nhwl_csd_changed (struct frame *f)
{
  struct nhwl_output *out = FRAME_NHWL_OUTPUT (f);

  if (out->csd.on)
    {
      out->csd.sig_valid = false;
      nhwl_dirty (f);
      fset_redisplay (f);
    }
}

/* The logical size of F's window geometry (the native area and the strip),
   and the native device size a compositor-suggested geometry of LW x LH
   gives.  */
void
nhwl_csd_geometry_size (struct frame *f, int *w, int *h)
{
  uint32_t s120 = nhwl_csd_s120 (f);

  *w = nhwl_to_logical (FRAME_PIXEL_WIDTH (f), s120);
  *h = nhwl_to_logical (FRAME_PIXEL_HEIGHT (f) + FRAME_NHWL_OUTPUT (f)->csd.strip,
			s120);
}

void
nhwl_csd_native_size (struct frame *f, int lw, int lh, int *dw, int *dh)
{
  uint32_t s120 = nhwl_csd_s120 (f);

  *dw = max (1, nhwl_to_device (lw, s120));
  *dh = max (1, nhwl_to_device (lh, s120) - FRAME_NHWL_OUTPUT (f)->csd.strip);
}

/* The native-area position of the surface point LX, LY, which the shell
   reports in device pixels already (logical times scale).  */
void
nhwl_csd_native_pos (struct frame *f, double lx, double ly, int *x, int *y)
{
  struct nhwl_csd *c = &FRAME_NHWL_OUTPUT (f)->csd;

  *x = (int) lround (lx) - c->l;
  *y = (int) lround (ly) - c->t;
}

/* Tell the compositor where the window sits inside the surface (before the
   commit that carries the new size).  Nothing is sent for a frame that
   never had a border.  */
void
nhwl_csd_send_geometry (struct frame *f)
{
  struct nhwl_output *out = FRAME_NHWL_OUTPUT (f);
  struct nhwl_csd *c = &out->csd;
  uint32_t s120 = nhwl_csd_s120 (f);
  int sw = nhwl_to_logical (FRAME_PIXEL_WIDTH (f) + c->l + c->r, s120);
  int sh = nhwl_to_logical (FRAME_PIXEL_HEIGHT (f) + c->t + c->b, s120);
  int ml = c->l ? NHWL_CSD_MARGIN : 0, mr = c->r ? NHWL_CSD_MARGIN : 0;
  int mt = c->t > c->strip ? NHWL_CSD_MARGIN : 0;
  int mb = c->b ? NHWL_CSD_MARGIN : 0;
  int g[4] = { ml, mt, sw - ml - mr, sh - mt - mb };

  if (!c->on && !c->geom_sent)
    return;
  if (!c->on)
    g[0] = g[1] = 0, g[2] = sw, g[3] = sh;
  if (g[2] <= 0 || g[3] <= 0
      || (c->geom_sent && !memcmp (g, c->geom, sizeof g)))
    return;
  if (nhshell_window_set_geometry (FRAME_NHWL_SHELL (f), out->window, g[0],
				   g[1], g[2], g[3]) >= 0)
    {
      memcpy (c->geom, g, sizeof g);
      c->geom_sent = true;
    }
}


/***********************************************************************
				Drawing
 ***********************************************************************/

static void
nhwl_csd_source (cairo_t *cr, unsigned long fg, unsigned long bg, double mix)
{
  cairo_set_source_rgb (cr,
			(RED_FROM_ULONG (bg) * (1 - mix) + RED_FROM_ULONG (fg) * mix) / 255.0,
			(GREEN_FROM_ULONG (bg) * (1 - mix) + GREEN_FROM_ULONG (fg) * mix) / 255.0,
			(BLUE_FROM_ULONG (bg) * (1 - mix) + BLUE_FROM_ULONG (fg) * mix) / 255.0);
}

static uint32_t
nhwl_csd_hash (const char *p)
{
  uint32_t h = 2166136261u;

  for (; *p; p++)
    h = (h ^ (unsigned char) *p) * 16777619u;
  return h;
}

/* The region of a button symbol (`close', `maximize', `minimize',
   `appmenu'), R_NONE for any other.  */
static int
nhwl_csd_kind (Lisp_Object sym)
{
  const char *n;

  if (!SYMBOLP (sym))
    return R_NONE;
  n = SSDATA (SYMBOL_NAME (sym));
  return (!strcmp (n, "close") ? R_CLOSE : !strcmp (n, "maximize") ? R_MAX
	  : !strcmp (n, "minimize") ? R_MIN : !strcmp (n, "appmenu") ? R_MENU
	  : R_NONE);
}

/* F's buttons: their regions in KIND and the surface x of their cells in
   X (each cell a strip-high square), the left side's from the left edge in
   order, the right side's ending at the right edge.  Return how many.  */
static int
nhwl_csd_buttons (struct frame *f, int *kind, int *x)
{
  struct nhwl_csd *c = &FRAME_NHWL_OUTPUT (f)->csd;
  int W = FRAME_PIXEL_WIDTH (f) + c->l + c->r, n = 0, nr = 0, k;
  Lisp_Object tail;

  if (!csd_styled)
    {
      /* W0's: minimize, maximize, close on the right.  */
      static const int w0[] = { R_MIN, R_MAX, R_CLOSE };
      for (k = 0; k < 3; k++)
	kind[n] = w0[k], x[n] = W - c->r - (3 - k) * c->strip, n++;
      return n;
    }
  for (tail = csd_left; CONSP (tail) && n < NHWL_CSD_MAX_BUTTONS;
       tail = XCDR (tail))
    if ((k = nhwl_csd_kind (XCAR (tail))) != R_NONE)
      kind[n] = k, x[n] = c->l + n * c->strip, n++;
  for (tail = csd_right; CONSP (tail); tail = XCDR (tail))
    if (nhwl_csd_kind (XCAR (tail)) != R_NONE)
      nr++;
  nr = min (nr, NHWL_CSD_MAX_BUTTONS - n);
  k = 0;
  for (tail = csd_right; CONSP (tail) && k < nr; tail = XCDR (tail))
    {
      int r = nhwl_csd_kind (XCAR (tail));

      if (r != R_NONE)
	kind[n] = r, x[n] = W - c->r - (nr - k) * c->strip, n++, k++;
    }
  return n;
}

/* The pixel of the colour named NAME on F in *PIXEL; false (and *PIXEL
   untouched) for nil or a name F cannot show.  */
static bool
nhwl_csd_colour (struct frame *f, Lisp_Object name, unsigned long *pixel)
{
  Emacs_Color c;

  if (!STRINGP (name) || !nhwl_defined_color (f, SSDATA (name), &c, false, 0))
    return false;
  *pixel = c.pixel;
  return true;
}

/* The title bar's colours for F now: the style's for an active or an
   inactive window, else W0's mix of the frame's colours.  */
static void
nhwl_csd_colours (struct frame *f, bool active, unsigned long *fg,
		  unsigned long *bg)
{
  unsigned long ffg = FRAME_FOREGROUND_PIXEL (f);
  unsigned long fbg = FRAME_BACKGROUND_PIXEL (f);
  Lisp_Object vfg = Qnil, vbg = Qnil;
  bool own_fg;

  if (VECTORP (csd_colours) && ASIZE (csd_colours) == 4)
    {
      vfg = AREF (csd_colours, active ? 0 : 2);
      vbg = AREF (csd_colours, active ? 1 : 3);
    }
  *fg = ffg;
  own_fg = !nhwl_csd_colour (f, vfg, fg);
  if (!nhwl_csd_colour (f, vbg, bg))
    {
      /* W0's: the background 12% toward the foreground.  */
      *bg = ((unsigned long) (RED_FROM_ULONG (fbg) * 0.88
			      + RED_FROM_ULONG (ffg) * 0.12) << 16
	     | (unsigned long) (GREEN_FROM_ULONG (fbg) * 0.88
				+ GREEN_FROM_ULONG (ffg) * 0.12) << 8
	     | (unsigned long) (BLUE_FROM_ULONG (fbg) * 0.88
				+ BLUE_FROM_ULONG (ffg) * 0.12));
      if (own_fg && !active)
	*fg = ((unsigned long) (RED_FROM_ULONG (*bg) * 0.45
				+ RED_FROM_ULONG (ffg) * 0.55) << 16
	       | (unsigned long) (GREEN_FROM_ULONG (*bg) * 0.45
				  + GREEN_FROM_ULONG (ffg) * 0.55) << 8
	       | (unsigned long) (BLUE_FROM_ULONG (*bg) * 0.45
				  + BLUE_FROM_ULONG (ffg) * 0.55));
    }
}

/* One button's glyph in a cell at X (the strip's top at Y, side SIDE),
   over a 10% foreground-tinted circle while hovered (20% pressed), as
   Adwaita draws them.  */
static void
nhwl_csd_draw_button (cairo_t *cr, int kind, double x, double y, double side,
		      unsigned long fg, unsigned long bg, int state, double s)
{
  double cx = x + side / 2, cy = y + side / 2, g = side * 0.14;

  if (state)
    {
      nhwl_csd_source (cr, fg, bg, state == 2 ? 0.2 : 0.1);
      cairo_arc (cr, cx, cy, side * 0.36, 0, 2 * M_PI);
      cairo_fill (cr);
    }
  nhwl_csd_source (cr, fg, bg, 1.0);
  cairo_set_line_width (cr, max (1.0, s));
  switch (kind)
    {
    case R_CLOSE:
      cairo_move_to (cr, cx - g, cy - g);
      cairo_line_to (cr, cx + g, cy + g);
      cairo_move_to (cr, cx + g, cy - g);
      cairo_line_to (cr, cx - g, cy + g);
      break;
    case R_MAX:
      cairo_rectangle (cr, cx - g, cy - g, 2 * g, 2 * g);
      break;
    case R_MIN:
      cairo_move_to (cr, cx - g, cy + g);
      cairo_line_to (cr, cx + g, cy + g);
      break;
    case R_MENU:
      for (int i = -1; i <= 1; i++)
	{
	  cairo_move_to (cr, cx - g, cy + i * g * 0.8);
	  cairo_line_to (cr, cx + g, cy + i * g * 0.8);
	}
      break;
    }
  cairo_stroke (cr);
}

/* Draw the border (margins clear, the strip, its title and buttons) when
   it is not what the backing already shows, and damage it.  Called with
   the backing at its current size.  */
void
nhwl_csd_draw (struct frame *f)
{
  struct nhwl_output *out = FRAME_NHWL_OUTPUT (f);
  struct nhwl_csd *c = &out->csd;
  int W = out->cr_width, H = out->cr_height;
  Lisp_Object title = !NILP (f->title) ? f->title : f->name;
  const char *name = STRINGP (title) ? SSDATA (ENCODE_UTF_8 (title)) : "";
  bool active = (out->states & NHSHELL_STATE_ACTIVATED) != 0;
  bool round = !(out->states & (NHSHELL_STATE_MAXIMIZED
				| NHSHELL_STATE_FULLSCREEN | NHWL_CSD_TILED));
  double s = nhwl_csd_s120 (f) / 120.0;
  unsigned long fg, bg;
  typeof (c->sig) sig;
  cairo_t *cr;
  cairo_text_extents_t te;
  cairo_font_extents_t fe;
  GlitnirRect r[4];
  int kind[NHWL_CSD_MAX_BUTTONS], bx[NHWL_CSD_MAX_BUTTONS];
  int n = 0, nb, i, mbar = c->t - c->strip, tl, tr;
  Lisp_Object family = CONSP (csd_font) ? XCAR (csd_font) : Qnil;
  Lisp_Object px = CONSP (csd_font) ? XCDR (csd_font) : Qnil;

  if (!c->on || c->t + c->b == 0)
    return;
  nhwl_csd_colours (f, active, &fg, &bg);
  memset (&sig, 0, sizeof sig);
  sig.w = W, sig.h = H, sig.l = c->l, sig.t = c->t, sig.r = c->r;
  sig.b = c->b;
  sig.title = nhwl_csd_hash (name);
  sig.states = out->states, sig.hover = c->hover, sig.down = c->down;
  sig.style = csd_style_gen, sig.fg = fg, sig.bg = bg, sig.gen = out->cr_gen;
  if (c->sig_valid && !memcmp (&sig, &c->sig, sizeof sig))
    return;

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
  cairo_rectangle (cr, c->l, mbar, W - c->l - c->r, c->strip);
  cairo_fill (cr);
  nhwl_csd_source (cr, fg, bg, 0.15);
  cairo_rectangle (cr, c->l, mbar + c->strip - 1, W - c->l - c->r, 1);
  cairo_fill (cr);
  cairo_restore (cr);

  /* A floating window's top corners are rounded: Glitnir's corner kernel
     replaces the strip's corner squares, transparent outside the arc.  */
  if (round && W - c->l - c->r > 0)
    {
      uint32_t fill = (0xff000000u | (RED_FROM_ULONG (bg) << 16)
		       | (GREEN_FROM_ULONG (bg) << 8) | BLUE_FROM_ULONG (bg));
      GlitnirRect run = { c->l, mbar, W - c->l - c->r, c->strip };

      cairo_surface_flush (out->cr_surface);
      glitnir_decor_corners (cairo_image_surface_get_data (out->cr_surface),
			     cairo_image_surface_get_stride (out->cr_surface),
			     run, NHWL_CSD_RADIUS, fill, fill, 0, 0, 1 | 2, s);
      cairo_surface_mark_dirty (out->cr_surface);
    }

  cairo_save (cr);
  cairo_set_operator (cr, CAIRO_OPERATOR_OVER);
  nb = nhwl_csd_buttons (f, kind, bx);
  for (i = 0; i < nb; i++)
    nhwl_csd_draw_button (cr, kind[i], bx[i], mbar, c->strip, fg, bg,
			  (c->down == kind[i] && c->hover == kind[i] ? 2
			   : c->hover == kind[i] ? 1 : 0), s);

  /* The title, centred between the buttons (GNOME's place), in the style's
     font at the device size.  */
  tl = c->l, tr = W - c->r;
  for (i = 0; i < nb; i++)
    if (bx[i] < W / 2)
      tl = max (tl, bx[i] + c->strip);
    else
      tr = min (tr, bx[i]);
  cairo_rectangle (cr, tl, mbar, max (0, tr - tl), c->strip);
  cairo_clip (cr);
  cairo_select_font_face (cr, STRINGP (family) ? SSDATA (family) : "sans-serif",
			  CAIRO_FONT_SLANT_NORMAL, CAIRO_FONT_WEIGHT_BOLD);
  cairo_set_font_size (cr, (NUMBERP (px) && XFLOATINT (px) > 0
			    ? XFLOATINT (px) : NHWL_CSD_TITLE_PX) * s);
  cairo_font_extents (cr, &fe);
  cairo_text_extents (cr, name, &te);
  nhwl_csd_source (cr, fg, bg, 1.0);
  cairo_move_to (cr, max (tl + 8 * s, (W - te.x_advance) / 2),
		 mbar + (c->strip - (fe.ascent + fe.descent)) / 2 + fe.ascent);
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
  c->sig = sig;
  c->sig_valid = true;
}


/***********************************************************************
				  Input
 ***********************************************************************/

/* Where the surface-device point DX, DY lies: a resize margin (EDGES the
   NHSHELL_EDGE_* it borders), the strip, one of its buttons, or none of
   these (the native area).  */
static int
nhwl_csd_hit (struct frame *f, double dx, double dy, uint32_t *edges)
{
  struct nhwl_csd *c = &FRAME_NHWL_OUTPUT (f)->csd;
  int W = FRAME_PIXEL_WIDTH (f) + c->l + c->r;
  int H = FRAME_PIXEL_HEIGHT (f) + c->t + c->b;
  int mbar = c->t - c->strip, i, nb;
  int kind[NHWL_CSD_MAX_BUTTONS], bx[NHWL_CSD_MAX_BUTTONS];

  *edges = 0;
  if (!c->on)
    return R_NONE;
  /* A tiled side has no margin (nhwl_csd_layout).  */
  if (dx < c->l)
    *edges |= NHSHELL_EDGE_LEFT;
  if (dx >= W - c->r)
    *edges |= NHSHELL_EDGE_RIGHT;
  if (dy < mbar)
    *edges |= NHSHELL_EDGE_TOP;
  if (dy >= H - c->b)
    *edges |= NHSHELL_EDGE_BOTTOM;
  if (*edges)
    return R_EDGE;
  if (c->strip && dy >= mbar && dy < c->t && dx >= c->l && dx < W - c->r)
    {
      nb = nhwl_csd_buttons (f, kind, bx);
      for (i = 0; i < nb; i++)
	if (dx >= bx[i] && dx < bx[i] + c->strip)
	  return kind[i];
      return R_STRIP;
    }
  return R_NONE;
}

static uint32_t
nhwl_csd_cursor (uint32_t e)
{
  const uint32_t TL = NHSHELL_EDGE_TOP | NHSHELL_EDGE_LEFT,
    BR = NHSHELL_EDGE_BOTTOM | NHSHELL_EDGE_RIGHT,
    TR = NHSHELL_EDGE_TOP | NHSHELL_EDGE_RIGHT,
    BL = NHSHELL_EDGE_BOTTOM | NHSHELL_EDGE_LEFT;

  if (e == TL || e == BR)
    return NHSHELL_CURSOR_RESIZE_NWSE;
  if (e == TR || e == BL)
    return NHSHELL_CURSOR_RESIZE_NESW;
  if (e & (NHSHELL_EDGE_TOP | NHSHELL_EDGE_BOTTOM))
    return NHSHELL_CURSOR_RESIZE_NS;
  if (e & (NHSHELL_EDGE_LEFT | NHSHELL_EDGE_RIGHT))
    return NHSHELL_CURSOR_RESIZE_EW;
  return NHSHELL_CURSOR_DEFAULT;
}

/* The pointer is at the surface point LX, LY in device pixels.  Return true
   when that is over the border (the pointer shape follows it), false for
   the native area.  */
bool
nhwl_csd_pointer (struct frame *f, double lx, double ly)
{
  struct nhwl_output *out = FRAME_NHWL_OUTPUT (f);
  struct nhwl_csd *c = &out->csd;
  uint32_t edges, shape;

  int region;

  c->px = lx;
  c->py = ly;
  region = nhwl_csd_hit (f, c->px, c->py, &edges);
  /* A button under the pointer is drawn hovered.  */
  if ((region >= R_CLOSE ? region : R_NONE) != c->hover)
    {
      c->hover = region >= R_CLOSE ? region : R_NONE;
      nhwl_csd_changed (f);
    }
  if (region == R_NONE)
    {
      if (c->in_border)
	{
	  /* Emacs sets its own shape again at the next glyph.  */
	  c->in_border = false;
	  out->current_cursor = NULL;
	  FRAME_DISPLAY_INFO (f)->last_mouse_glyph_frame = NULL;
	}
      return false;
    }
  c->in_border = true;
  shape = nhwl_csd_cursor (edges);
  if (out->current_cursor != nhwl_cursor_for_shape (shape))
    {
      nhshell_window_set_cursor (FRAME_NHWL_SHELL (f), out->window, shape);
      out->current_cursor = nhwl_cursor_for_shape (shape);
    }
  return true;
}

/* A pointer button event.  Return true when the border consumed it; then
   *CLOSE says the close button was clicked.  A press in the border starts
   a move or resize (left), the window menu (right), or arms a button that
   the release over it fires; the release of a consumed press is consumed
   too.  */
bool
nhwl_csd_button (struct frame *f, const NhShellEvent *ev, bool *close)
{
  struct nhwl_output *out = FRAME_NHWL_OUTPUT (f);
  struct nhwl_csd *c = &out->csd;
  NhShell *sh = FRAME_NHWL_SHELL (f);
  double s = nhwl_csd_s120 (f) / 120.0;
  uint32_t edges, button = ev->u.pointer.button;
  int region = nhwl_csd_hit (f, c->px, c->py, &edges);

  if (ev->u.pointer.state != NHSHELL_BUTTON_PRESSED)
    {
      int armed = c->down;

      if (!c->grab)
	return false;
      c->grab = false;
      c->down = R_NONE;
      if (armed >= R_CLOSE)
	nhwl_csd_changed (f);
      if (armed >= R_CLOSE && region == armed)
	{
	  if (armed == R_CLOSE)
	    *close = true;
	  else if (armed == R_MAX)
	    nhshell_window_set_maximized
	      (sh, out->window, !(out->states & NHSHELL_STATE_MAXIMIZED));
	  else if (armed == R_MIN)
	    nhshell_window_minimize (sh, out->window);
	  else
	    {
	      /* `appmenu': the window menu, below the button.  */
	      int kind[NHWL_CSD_MAX_BUTTONS], bx[NHWL_CSD_MAX_BUTTONS], i;
	      int nb = nhwl_csd_buttons (f, kind, bx), x = (int) c->px;

	      for (i = 0; i < nb; i++)
		if (kind[i] == R_MENU)
		  x = bx[i];
	      nhshell_window_show_menu (sh, out->window, (int) (x / s),
					(int) (c->t / s));
	    }
	}
      return true;
    }

  if (region == R_NONE)
    return false;
  c->grab = true;
  c->down = R_NONE;
  if (region == R_EDGE)
    {
      if (button == BTN_LEFT_CODE)
	nhshell_window_resize (sh, out->window, edges);
    }
  else if (region == R_STRIP)
    {
      if (button == BTN_LEFT_CODE)
	{
	  if (c->click_ns
	      && ev->time_ns - c->click_ns < NHWL_CSD_DOUBLE_CLICK_NS)
	    {
	      c->click_ns = 0;
	      nhshell_window_set_maximized
		(sh, out->window, !(out->states & NHSHELL_STATE_MAXIMIZED));
	    }
	  else
	    {
	      c->click_ns = ev->time_ns ? ev->time_ns : 1;
	      nhshell_window_move (sh, out->window);
	    }
	}
      else if (button == BTN_RIGHT_CODE)
	nhshell_window_show_menu (sh, out->window, (int) (c->px / s),
				  (int) (c->py / s));
    }
  else if (button == BTN_LEFT_CODE)
    {
      c->down = region;
      nhwl_csd_changed (f);
    }
  return true;
}

/* The pointer left F's surface: no button is hovered any more.  */
void
nhwl_csd_leave (struct frame *f)
{
  struct nhwl_csd *c = &FRAME_NHWL_OUTPUT (f)->csd;

  if (c->hover != R_NONE)
    {
      c->hover = R_NONE;
      nhwl_csd_changed (f);
    }
}


/***********************************************************************
				  Style
 ***********************************************************************/

DEFUN ("nhwl--set-title-bar-style", Fnhwl__set_title_bar_style,
       Snhwl__set_title_bar_style, 7, 7, 0,
       doc: /* Give the shell's title bars their style.
LEFT and RIGHT are the buttons of each side, lists of `close',
`maximize', `minimize' and `appmenu' in their order; other elements are
ignored.  ACTIVE-FG and ACTIVE-BG are the colours of an active window's
title bar, INACTIVE-FG and INACTIVE-BG an inactive one's (colour names,
or nil for colours from the frame's).  FONT is (FAMILY . PIXELS): the
title's font family (nil: sans-serif) and its size in logical pixels
(nil: 13).  Every title bar is drawn again.  Internal: called by
lisp/term/nhwl-settings.el when the desktop's settings or the title-bar
faces change.  */)
  (Lisp_Object left, Lisp_Object right, Lisp_Object active_fg,
   Lisp_Object active_bg, Lisp_Object inactive_fg, Lisp_Object inactive_bg,
   Lisp_Object font)
{
  Lisp_Object tail, frame;

  CHECK_LIST (left);
  CHECK_LIST (right);
  csd_left = Fcopy_sequence (left);
  csd_right = Fcopy_sequence (right);
  csd_colours = CALLN (Fvector, STRINGP (active_fg) ? active_fg : Qnil,
		       STRINGP (active_bg) ? active_bg : Qnil,
		       STRINGP (inactive_fg) ? inactive_fg : Qnil,
		       STRINGP (inactive_bg) ? inactive_bg : Qnil);
  csd_font = (CONSP (font)
	      ? Fcons (STRINGP (XCAR (font)) ? XCAR (font) : Qnil,
		       NUMBERP (XCDR (font)) ? XCDR (font) : Qnil)
	      : Qnil);
  csd_styled = true;
  csd_style_gen++;
  FOR_EACH_FRAME (tail, frame)
    if (FRAME_NHWL_P (XFRAME (frame)) && FRAME_NHWL_OUTPUT (XFRAME (frame)))
      nhwl_csd_changed (XFRAME (frame));
  return Qnil;
}

DEFUN ("nhwl--title-bar", Fnhwl__title_bar, Snhwl__title_bar, 0, 1, 0,
       doc: /* Return FRAME's title bar as the shell lays it out, or nil.
A plist in device pixels of the surface: `:strip' (X Y W H), `:margins'
\(LEFT TOP RIGHT BOTTOM), and `:buttons', a list of (NAME X Y W H) with
NAME one of `close', `maximize', `minimize' and `appmenu'.  nil when the
frame draws no title bar.  For tests.  */)
  (Lisp_Object frame)
{
  struct frame *f = decode_live_frame (frame);
  struct nhwl_csd *c;
  int kind[NHWL_CSD_MAX_BUTTONS], bx[NHWL_CSD_MAX_BUTTONS], nb, i;
  int W, mbar;
  Lisp_Object buttons = Qnil;
  static const char *const names[] =
    { "", "", "", "close", "maximize", "minimize", "appmenu" };

  if (!FRAME_NHWL_P (f) || !FRAME_NHWL_OUTPUT (f))
    return Qnil;
  c = &FRAME_NHWL_OUTPUT (f)->csd;
  if (!c->on || !c->strip)
    return Qnil;
  W = FRAME_PIXEL_WIDTH (f) + c->l + c->r;
  mbar = c->t - c->strip;
  nb = nhwl_csd_buttons (f, kind, bx);
  for (i = nb - 1; i >= 0; i--)
    buttons = Fcons (list5 (intern (names[kind[i]]), make_fixnum (bx[i]),
			    make_fixnum (mbar), make_fixnum (c->strip),
			    make_fixnum (c->strip)),
		     buttons);
  return CALLN (Flist, QCstrip, list4 (make_fixnum (c->l), make_fixnum (mbar),
				 make_fixnum (W - c->l - c->r),
				 make_fixnum (c->strip)),
		QCmargins, list4 (make_fixnum (c->l), make_fixnum (mbar),
				   make_fixnum (c->r), make_fixnum (c->b)),
		QCbuttons, buttons);
}

void
syms_of_nhwlcsd (void)
{
  DEFSYM (QCstrip, ":strip");
  DEFSYM (QCmargins, ":margins");
  DEFSYM (QCbuttons, ":buttons");
  csd_left = csd_right = csd_colours = csd_font = Qnil;
  staticpro (&csd_left);
  staticpro (&csd_right);
  staticpro (&csd_colours);
  staticpro (&csd_font);
  defsubr (&Snhwl__set_title_bar_style);
  defsubr (&Snhwl__title_bar);
}

#endif /* HAVE_NHWL */

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
   glyph buttons; W1 gives it the GNOME layout, theme colours and tiled
   states.  Everything here is device pixels except the protocol (the
   strip's 32 and the margin's 8 are logical).  */

#include <config.h>

#include <math.h>

#include "lisp.h"
#include "nhwlterm.h"
#include "blockinput.h"
#include "coding.h"

#ifdef HAVE_NHWL

#define NHWL_CSD_STRIP 32	/* logical px */
#define NHWL_CSD_MARGIN 8
#define NHWL_CSD_DOUBLE_CLICK_NS 400000000ull
#define BTN_LEFT_CODE 0x110
#define BTN_RIGHT_CODE 0x111

enum { R_NONE, R_EDGE, R_STRIP, R_CLOSE, R_MAX, R_MIN };

static uint32_t
nhwl_csd_s120 (struct frame *f)
{
  uint32_t s = FRAME_NHWL_OUTPUT (f)->scale120;

  return s ? s : 120;
}

/* Recompute the border from the compositor's decoration answer, the
   toplevel's states and the scale.  A fullscreen frame has none, a
   maximized one only the strip.  */
void
nhwl_csd_layout (struct frame *f)
{
  struct nhwl_output *out = FRAME_NHWL_OUTPUT (f);
  struct nhwl_display_info *dpyinfo = FRAME_DISPLAY_INFO (f);
  struct nhwl_csd *c = &out->csd;
  uint32_t s120 = nhwl_csd_s120 (f);
  int32_t ssd = 0;
  int m = 0, strip = 0;

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
  if (c->m != m || c->strip != strip)
    {
      c->m = m;
      c->strip = strip;
      c->l = c->r = c->b = m;
      c->t = m + strip;
      c->sig_valid = false;
      SET_FRAME_GARBAGED (f);
      nhwl_dirty (f);
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

/* The native-area position of the surface-local logical point LX, LY.  */
void
nhwl_csd_native_pos (struct frame *f, double lx, double ly, int *x, int *y)
{
  struct nhwl_csd *c = &FRAME_NHWL_OUTPUT (f)->csd;
  double s = nhwl_csd_s120 (f) / 120.0;

  *x = (int) lround (lx * s) - c->l;
  *y = (int) lround (ly * s) - c->t;
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
  int ml = c->m ? NHWL_CSD_MARGIN : 0;
  int g[4] = { ml, ml, sw - 2 * ml, sh - 2 * ml };

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

/* The button cells, right to left: close, maximize, minimize.  */
static int
nhwl_csd_button_x (struct frame *f, int which)
{
  struct nhwl_csd *c = &FRAME_NHWL_OUTPUT (f)->csd;

  return FRAME_PIXEL_WIDTH (f) + c->l - (which + 1) * c->strip;
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
  unsigned long fg = FRAME_FOREGROUND_PIXEL (f), bg = FRAME_BACKGROUND_PIXEL (f);
  Lisp_Object title = !NILP (f->title) ? f->title : f->name;
  const char *name = STRINGP (title) ? SSDATA (ENCODE_UTF_8 (title)) : "";
  bool active = (out->states & NHSHELL_STATE_ACTIVATED) != 0;
  double s = nhwl_csd_s120 (f) / 120.0;
  typeof (c->sig) sig;
  cairo_t *cr;
  cairo_font_extents_t fe;
  GlitnirRect r[4];
  int n = 0, i, mbar = c->t - c->strip, x0, bx;
  double g, lw;

  if (!c->on || c->t + c->b == 0)
    return;
  memset (&sig, 0, sizeof sig);
  sig.w = W, sig.h = H, sig.l = c->l, sig.t = c->t;
  sig.title = nhwl_csd_hash (name);
  sig.active = active, sig.fg = fg, sig.bg = bg, sig.gen = out->cr_gen;
  if (c->sig_valid && !memcmp (&sig, &c->sig, sizeof sig))
    return;

  cr = out->cr_context;
  cairo_save (cr);
  if (c->m)
    {
      /* the margins are transparent */
      cairo_set_operator (cr, CAIRO_OPERATOR_CLEAR);
      cairo_rectangle (cr, 0, 0, W, c->m);
      cairo_rectangle (cr, 0, H - c->m, W, c->m);
      cairo_rectangle (cr, 0, c->m, c->m, H - 2 * c->m);
      cairo_rectangle (cr, W - c->m, c->m, c->m, H - 2 * c->m);
      cairo_fill (cr);
    }
  cairo_set_operator (cr, CAIRO_OPERATOR_SOURCE);
  nhwl_csd_source (cr, fg, bg, 0.12);
  cairo_rectangle (cr, c->l, mbar, W - c->l - c->r, c->strip);
  cairo_fill (cr);
  nhwl_csd_source (cr, fg, bg, 0.3);
  cairo_rectangle (cr, c->l, mbar + c->strip - 1, W - c->l - c->r, 1);
  cairo_fill (cr);
  cairo_set_operator (cr, CAIRO_OPERATOR_OVER);

  /* The title, left of the buttons.  */
  x0 = nhwl_csd_button_x (f, 2);
  cairo_save (cr);
  cairo_rectangle (cr, c->l, mbar, max (0, x0 - c->l - 8 * s), c->strip);
  cairo_clip (cr);
  cairo_select_font_face (cr, "sans-serif", CAIRO_FONT_SLANT_NORMAL,
			  CAIRO_FONT_WEIGHT_NORMAL);
  cairo_set_font_size (cr, 13 * s);
  cairo_font_extents (cr, &fe);
  nhwl_csd_source (cr, fg, bg, active ? 1.0 : 0.55);
  cairo_move_to (cr, c->l + 12 * s,
		 mbar + (c->strip - (fe.ascent + fe.descent)) / 2 + fe.ascent);
  cairo_show_text (cr, name);
  cairo_restore (cr);

  /* The buttons: a bar, a square, a cross.  */
  g = c->strip * 0.14;
  lw = max (1.0, s);
  cairo_set_line_width (cr, lw);
  nhwl_csd_source (cr, fg, bg, active ? 1.0 : 0.55);
  for (i = 0; i < 3; i++)
    {
      double cx, cy = mbar + c->strip / 2.0;

      bx = nhwl_csd_button_x (f, i);
      cx = bx + c->strip / 2.0;
      if (i == 0)		/* close */
	{
	  cairo_move_to (cr, cx - g, cy - g);
	  cairo_line_to (cr, cx + g, cy + g);
	  cairo_move_to (cr, cx + g, cy - g);
	  cairo_line_to (cr, cx - g, cy + g);
	}
      else if (i == 1)		/* maximize */
	cairo_rectangle (cr, cx - g, cy - g, 2 * g, 2 * g);
      else			/* minimize */
	{
	  cairo_move_to (cr, cx - g, cy + g);
	  cairo_line_to (cr, cx + g, cy + g);
	}
      cairo_stroke (cr);
    }
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
  int mbar = c->t - c->strip, i;

  *edges = 0;
  if (!c->on)
    return R_NONE;
  if (c->m)
    {
      if (dx < c->m)
	*edges |= NHSHELL_EDGE_LEFT;
      if (dx >= W - c->m)
	*edges |= NHSHELL_EDGE_RIGHT;
      if (dy < c->m)
	*edges |= NHSHELL_EDGE_TOP;
      if (dy >= H - c->m)
	*edges |= NHSHELL_EDGE_BOTTOM;
      if (*edges)
	return R_EDGE;
    }
  if (c->strip && dy >= mbar && dy < c->t && dx >= c->l && dx < W - c->r)
    {
      for (i = 0; i < 3; i++)
	if (dx >= nhwl_csd_button_x (f, i))
	  return R_CLOSE + i;
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

/* The pointer is at the surface-local logical point LX, LY.  Return true
   when that is over the border (the pointer shape follows it), false for
   the native area.  */
bool
nhwl_csd_pointer (struct frame *f, double lx, double ly)
{
  struct nhwl_output *out = FRAME_NHWL_OUTPUT (f);
  struct nhwl_csd *c = &out->csd;
  double s = nhwl_csd_s120 (f) / 120.0;
  uint32_t edges, shape;

  c->px = lx * s;
  c->py = ly * s;
  if (nhwl_csd_hit (f, c->px, c->py, &edges) == R_NONE)
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
      if (armed >= R_CLOSE && region == armed)
	{
	  if (armed == R_CLOSE)
	    *close = true;
	  else if (armed == R_MAX)
	    nhshell_window_set_maximized
	      (sh, out->window, !(out->states & NHSHELL_STATE_MAXIMIZED));
	  else
	    nhshell_window_minimize (sh, out->window);
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
    c->down = region;
  return true;
}

#endif /* HAVE_NHWL */

/* Lisp primitives and frame creation for Nidhogg's own Wayland shell
   (HAVE_NHWL).
   Copyright (C) 1989, 1992-1995, 1997-2026 Free Software Foundation, Inc.

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

/* The primitives carry pgtk's names, so that term/pgtk-win.el loads
   unchanged (the Lisp-level window system is `pgtk', and `nidhogg-wayland'
   is provided too).  Functions adapted from pgtkfns.c name their origin;
   the upstream commit is a360712c9d272d950d8d8255ef74570f7e90b7d9 (GNU
   Emacs emacs-31.1), for every "Adapted from" note in this file.
   The frame parameters with a handler in nhwl_frame_parm_handlers have
   effects; every other parameter is accepted and stored.  The
   selection primitives are at the end of this file.  */

#include <config.h>

#include <math.h>
#include <c-strcase.h>

#include "lisp.h"
#include "nhwlterm.h"
#include "blockinput.h"
#include "window.h"
#include "character.h"
#include "coding.h"
#include "buffer.h"
#include "keyboard.h"
#include "termhooks.h"
#include "fontset.h"
#include "font.h"
#include "atimer.h"

static int x_decode_color (struct frame *f, Lisp_Object color_name,
			   int mono_color);



/***********************************************************************
			 Frame parameter setters
 ***********************************************************************/

/* Return the pixel color value for color COLOR_NAME on frame F.  Signal
   an error if color can't be allocated.  Adapted from x_decode_color
   (pgtkfns.c).  */
static int
x_decode_color (struct frame *f, Lisp_Object color_name, int mono_color)
{
  Emacs_Color cdef;

  CHECK_STRING (color_name);

  /* Return MONO_COLOR for monochrome frames.  */
  if (FRAME_DISPLAY_INFO (f)->n_planes == 1)
    return mono_color;

  if (nhwl_defined_color (f, SSDATA (color_name), &cdef, true, 0))
    return cdef.pixel;

  signal_error ("Undefined color", color_name);
}

/* Adapted from pgtk_set_foreground_color.  */
static void
nhwl_set_foreground_color (struct frame *f, Lisp_Object arg,
			   Lisp_Object oldval)
{
  unsigned long fg, old_fg;

  block_input ();
  old_fg = FRAME_FOREGROUND_COLOR (f);
  fg = x_decode_color (f, arg, BLACK_PIX_DEFAULT (f));
  FRAME_FOREGROUND_PIXEL (f) = fg;
  FRAME_X_OUTPUT (f)->foreground_color = fg;

  if (FRAME_X_OUTPUT (f)->window)
    {
      if (FRAME_X_OUTPUT (f)->cursor_color == old_fg)
	{
	  FRAME_X_OUTPUT (f)->cursor_color = fg;
	  FRAME_X_OUTPUT (f)->cursor_xgcv.background = fg;
	}

      update_face_from_frame_parameter (f, Qforeground_color, arg);
      if (FRAME_VISIBLE_P (f))
	SET_FRAME_GARBAGED (f);
    }
  unblock_input ();
}

/* Adapted from pgtk_set_background_color.  */
static void
nhwl_set_background_color (struct frame *f, Lisp_Object arg,
			   Lisp_Object oldval)
{
  unsigned long bg;

  block_input ();
  bg = x_decode_color (f, arg, WHITE_PIX_DEFAULT (f));
  FRAME_BACKGROUND_PIXEL (f) = bg;
  FRAME_X_OUTPUT (f)->background_color = bg;
  FRAME_X_OUTPUT (f)->cursor_xgcv.foreground = bg;

  /* Clear the frame.  */
  if (FRAME_VISIBLE_P (f))
    nhwl_clear_frame (f);

  update_face_from_frame_parameter (f, Qbackground_color, arg);

  if (FRAME_VISIBLE_P (f))
    SET_FRAME_GARBAGED (f);
  unblock_input ();
}

/* The backing carries the alpha: gui_set_alpha_background stores it in
   the frame and the drawing code paints with it.  */
static void
nhwl_set_alpha_background (struct frame *f, Lisp_Object arg,
			   Lisp_Object oldval)
{
  gui_set_alpha_background (f, arg, oldval);
  if (FRAME_VISIBLE_P (f))
    SET_FRAME_GARBAGED (f);
}

/* The `alpha' parameter: (ACTIVE . INACTIVE) or one value for both, a
   float from 0 to 1, an integer percentage, or nil (opaque).  Adapted
   from frame.c's gui_set_alpha, which is not built for this window
   system; nhwl_set_frame_alpha applies the values.  */
static void
nhwl_set_alpha (struct frame *f, Lisp_Object arg, Lisp_Object oldval)
{
  double newval[2];
  int i;
  Lisp_Object item;

  for (i = 0; i < 2; i++)
    {
      if (CONSP (arg))
	{
	  item = XCAR (arg);
	  arg = XCDR (arg);
	}
      else
	item = arg;

      if (NILP (item))
	newval[i] = -1.0;
      else if (FLOATP (item))
	{
	  newval[i] = XFLOAT_DATA (item);
	  if (! (0 <= newval[i] && newval[i] <= 1.0))
	    args_out_of_range (make_float (0.0), make_float (1.0));
	}
      else if (FIXNUMP (item))
	{
	  EMACS_INT ialpha = XFIXNUM (item);
	  if (! (0 <= ialpha && ialpha <= 100))
	    args_out_of_range (make_fixnum (0), make_fixnum (100));
	  newval[i] = ialpha / 100.0;
	}
      else
	wrong_type_argument (Qnumberp, item);
    }

  for (i = 0; i < 2; i++)
    f->alpha[i] = newval[i];
  block_input ();
  nhwl_set_frame_alpha (f);
  unblock_input ();
}

/* Adapted from pgtk_set_border_color.  */
static void
nhwl_set_border_color (struct frame *f, Lisp_Object arg, Lisp_Object oldval)
{
  int pix;

  CHECK_STRING (arg);
  pix = x_decode_color (f, arg, BLACK_PIX_DEFAULT (f));
  FRAME_X_OUTPUT (f)->border_pixel = pix;
  nhwl_frame_rehighlight (FRAME_DISPLAY_INFO (f));
}

/* Adapted from pgtk_set_cursor_color.  */
static void
nhwl_set_cursor_color (struct frame *f, Lisp_Object arg, Lisp_Object oldval)
{
  unsigned long fore_pixel, pixel;
  struct nhwl_output *x = f->output_data.nhwl;

  if (!NILP (Vx_cursor_fore_pixel))
    fore_pixel = x_decode_color (f, Vx_cursor_fore_pixel,
				 WHITE_PIX_DEFAULT (f));
  else
    fore_pixel = FRAME_BACKGROUND_PIXEL (f);

  pixel = x_decode_color (f, arg, BLACK_PIX_DEFAULT (f));

  /* Make sure that the cursor color differs from the background color.  */
  if (pixel == FRAME_BACKGROUND_PIXEL (f))
    {
      pixel = x->mouse_color;
      if (pixel == fore_pixel)
	fore_pixel = FRAME_BACKGROUND_PIXEL (f);
    }

  x->cursor_foreground_color = fore_pixel;
  x->cursor_color = pixel;

  if (FRAME_X_WINDOW (f) != 0)
    {
      x->cursor_xgcv.background = x->cursor_color;
      x->cursor_xgcv.foreground = fore_pixel;

      if (FRAME_VISIBLE_P (f))
	{
	  gui_update_cursor (f, false);
	  gui_update_cursor (f, true);
	}
    }

  update_face_from_frame_parameter (f, Qcursor_color, arg);
}

static void
nhwl_set_cursor_type (struct frame *f, Lisp_Object arg, Lisp_Object oldval)
{
  set_frame_cursor_types (f, arg);
}

static void
nhwl_set_mouse_color (struct frame *f, Lisp_Object arg, Lisp_Object oldval)
{
}

/* Adapted from pgtk_set_name.  */
static void
nhwl_set_name (struct frame *f, Lisp_Object name, int explicit)
{
  /* Make sure that requests from lisp code override requests from
     Emacs redisplay code.  */
  if (explicit)
    {
      /* If we're switching from explicit to implicit, we had better
	 update the mode lines and thereby update the title.  */
      if (f->explicit_name && NILP (name))
	update_mode_lines = 12;

      f->explicit_name = !NILP (name);
    }
  else if (f->explicit_name)
    return;

  if (NILP (name))
    name = build_string ("Nidhogg");
  else
    CHECK_STRING (name);

  /* Don't change the name if it's already NAME.  */
  if (!NILP (Fstring_equal (name, f->name)))
    return;

  fset_name (f, name);

  /* Title overrides explicit name.  */
  if (NILP (f->title))
    nhwl_set_title_hint (f);
}

static void
nhwl_explicitly_set_name (struct frame *f, Lisp_Object arg,
			  Lisp_Object oldval)
{
  nhwl_set_name (f, arg, true);
}

/* Adapted from pgtk_set_title.  */
static void
nhwl_set_title (struct frame *f, Lisp_Object name, Lisp_Object old_name)
{
  /* Don't change the title if it's already NAME.  */
  if (EQ (name, f->title))
    return;

  update_mode_lines = 22;

  fset_title (f, name);

  if (!NILP (name))
    CHECK_STRING (name);
  nhwl_set_title_hint (f);
}

/* Adapted from x_set_menu_bar_lines (xfns.c, the branch without a
   toolkit): the menu bar is drawn by Emacs.  */
static void
nhwl_set_menu_bar_lines (struct frame *f, Lisp_Object value,
			 Lisp_Object oldval)
{
  int nlines;
  int olines = FRAME_MENU_BAR_LINES (f);

  /* Right now, menu bars don't work properly in minibuf-only frames;
     most of the commands try to apply themselves to the minibuffer
     frame itself, and get an error because you can't switch buffers
     in or split the minibuffer window.  */
  if (FRAME_MINIBUF_ONLY_P (f) || FRAME_PARENT_FRAME (f))
    return;

  if (TYPE_RANGED_FIXNUMP (int, value))
    nlines = XFIXNUM (value);
  else
    nlines = 0;

  /* Make sure we redisplay all windows in this frame.  */
  fset_redisplay (f);

  FRAME_MENU_BAR_LINES (f) = nlines;
  FRAME_MENU_BAR_HEIGHT (f) = nlines * FRAME_LINE_HEIGHT (f);
  if (FRAME_X_WINDOW (f))
    nhwl_clear_under_internal_border (f);

  /* If the menu bar height gets changed, the internal border below
     the top margin has to be cleared.  Also, if the menu bar gets
     larger, the area for the added lines has to be cleared except for
     the first menu bar line that is to be drawn later.  */
  if (nlines != olines)
    {
      int height = FRAME_INTERNAL_BORDER_WIDTH (f);
      int width = FRAME_PIXEL_WIDTH (f);
      int y;

      adjust_frame_size (f, -1, -1, 3, true, Qmenu_bar_lines);

      /* height can be zero here. */
      if (FRAME_X_WINDOW (f) && height > 0 && width > 0)
	{
	  y = FRAME_TOP_MARGIN_HEIGHT (f);

	  block_input ();
	  nhwl_clear_area (f, 0, y, width, height);
	  unblock_input ();
	}

      if (nlines > 1 && nlines > olines)
	{
	  y = (olines == 0 ? 1 : olines) * FRAME_LINE_HEIGHT (f);
	  height = nlines * FRAME_LINE_HEIGHT (f) - y;

	  block_input ();
	  nhwl_clear_area (f, 0, y, width, height);
	  unblock_input ();
	}

      if (nlines == 0 && WINDOWP (f->menu_bar_window))
	clear_glyph_matrix (XWINDOW (f->menu_bar_window)->current_matrix);
    }
  adjust_frame_glyphs (f);
}

/* Adapted from x_set_tab_bar_lines (xfns.c).  */
static void
nhwl_set_tab_bar_lines (struct frame *f, Lisp_Object value,
			Lisp_Object oldval)
{
  int olines = FRAME_TAB_BAR_LINES (f);
  int nlines;

  /* Treat tab bars like menu bars.  */
  if (FRAME_MINIBUF_ONLY_P (f))
    return;

  /* Use VALUE only if an int >= 0.  */
  if (RANGED_FIXNUMP (0, value, INT_MAX))
    nlines = XFIXNAT (value);
  else
    nlines = 0;

  if (nlines != olines && (olines == 0 || nlines == 0))
    FRAME_TERMINAL (f)->change_tab_bar_height_hook
      (f, nlines * FRAME_LINE_HEIGHT (f));
}

/* Adapted from x_set_tool_bar_lines (xfns.c).  */
static void
nhwl_set_tool_bar_lines (struct frame *f, Lisp_Object value,
			 Lisp_Object oldval)
{
  int nlines;

  /* Treat tool bars like menu bars.  */
  if (FRAME_MINIBUF_ONLY_P (f))
    return;

  /* Use VALUE only if an int >= 0.  */
  if (RANGED_FIXNUMP (0, value, INT_MAX))
    nlines = XFIXNAT (value);
  else
    nlines = 0;

  FRAME_TERMINAL (f)->change_tool_bar_height_hook
    (f, nlines * FRAME_LINE_HEIGHT (f));
}

/* Adapted from pgtk_set_child_frame_border_width.  */
static void
nhwl_set_child_frame_border_width (struct frame *f, Lisp_Object arg,
				   Lisp_Object oldval)
{
  int border;

  if (NILP (arg))
    border = -1;
  else if (RANGED_FIXNUMP (0, arg, INT_MAX))
    border = XFIXNAT (arg);
  else
    signal_error ("Invalid child frame border width", arg);

  if (border != FRAME_CHILD_FRAME_BORDER_WIDTH (f))
    {
      f->child_frame_border_width = border;

      if (FRAME_X_WINDOW (f))
	{
	  adjust_frame_size (f, -1, -1, 3, false, Qchild_frame_border_width);
	  nhwl_clear_under_internal_border (f);
	}
    }
}

/* Adapted from pgtk_set_internal_border_width.  */
static void
nhwl_set_internal_border_width (struct frame *f, Lisp_Object arg,
				Lisp_Object oldval)
{
  int border = check_int_nonnegative (arg);

  if (border != FRAME_INTERNAL_BORDER_WIDTH (f))
    {
      f->internal_border_width = border;

      if (FRAME_X_WINDOW (f))
	{
	  adjust_frame_size (f, -1, -1, 3, false, Qinternal_border_width);
	  nhwl_clear_under_internal_border (f);
	}
    }
}

/* The shell takes the flag when the window is made, and
   nhwl_set_undecorated_hint changes it later.  */
static void
nhwl_set_undecorated (struct frame *f, Lisp_Object new_value,
		      Lisp_Object old_value)
{
  if (!EQ (new_value, old_value))
    {
      FRAME_UNDECORATED (f) = !NILP (new_value);
      nhwl_set_undecorated_hint (f, FRAME_UNDECORATED (f));
    }
}


/***********************************************************************
		    Child frames: parameters and chrome
 ***********************************************************************/

/* A child frame's layout, margin, position and stacking are the glue's
   (child.rs; nhwlterm.c pushes its facts).  Here: its parameters, and the
   chrome `glitnir--resolve' gives it.  */

static float
nhwl_chrome_num (Lisp_Object v)
{
  return NUMBERP (v) ? (float) max (XFLOATINT (v), 0.0) : 0;
}

static Lisp_Object
nhwl_chrome_key (Lisp_Object plist, const char *name)
{
  return Fplist_get (plist, intern_c_string (name), Qnil);
}

/* Take the chrome from `glitnir--resolve' (called after the frame's
   parameters changed) and hand it to the glue; adapted from Nidhogg's
   glitnir_pgtk_params_changed.  A change lays the frame out again (the
   shadow's margin) and draws it again.  The chrome is kept for the
   window's next making (nhwl_child_created).  */
static void nhwl_idle_param_changed (struct frame *);

void
nhwl_child_params_changed (struct frame *f)
{
  static bool busy;		/* the resolver changed parameters itself */
  struct nhwl_output *out;
  NhgChrome n;
  Lisp_Object frame, res;

  nhwl_popup_params_changed (f);
  nhwl_idle_param_changed (f);
  if (busy || !nhwl_child_p (f) || NILP (Ffboundp (Qglitnir__resolve)))
    return;
  out = FRAME_NHWL_OUTPUT (f);
  XSETFRAME (frame, f);
  busy = true;
  res = safe_calln (Qglitnir__resolve, frame);
  busy = false;

  memset (&n, 0, sizeof n);
  n.on = CONSP (res);
  if (n.on)
    {
      Lisp_Object sh = nhwl_chrome_key (res, ":shadow");

      n.radius = nhwl_chrome_num (nhwl_chrome_key (res, ":radius"));
      if (CONSP (sh))
	{
	  Lisp_Object off = nhwl_chrome_key (sh, ":offset");
	  Lisp_Object col = nhwl_chrome_key (sh, ":color");
	  uint32_t rgb = 0;
	  int i;

	  n.blur = nhwl_chrome_num (nhwl_chrome_key (sh, ":blur"));
	  if (CONSP (off))
	    {
	      n.ox = NUMBERP (XCAR (off)) ? XFLOATINT (XCAR (off)) : 0;
	      n.oy = NUMBERP (XCDR (off)) ? XFLOATINT (XCDR (off)) : 0;
	    }
	  n.spread = NUMBERP (nhwl_chrome_key (sh, ":spread"))
	    ? XFLOATINT (nhwl_chrome_key (sh, ":spread")) : 0;
	  n.opacity = min (nhwl_chrome_num (nhwl_chrome_key (sh, ":opacity")),
			   1);
	  for (i = 0; i < 3; i++, col = CONSP (col) ? XCDR (col) : Qnil)
	    rgb = rgb << 8 | (CONSP (col) && FIXNATP (XCAR (col))
			      ? min (XFIXNAT (XCAR (col)), 255) : 0);
	  n.rgb = rgb;
	}
    }
  out->chrome = n;
  if (out->window
      && nhwl_glue_rc (FRAME_DISPLAY_INFO (f),
		       NHG (nhg_child_chrome (FRAME_NHG (f), out->window, &n)))
	 == 1)
    {
      nhwl_csd_layout (f);
      SET_FRAME_GARBAGED (f);
    }
}

/* Adapted from pgtk_set_z_group (emacs-31.1): stored, and for a child
   frame acted on among its siblings.  */
void
nhwl_set_z_group (struct frame *f, Lisp_Object new_value,
		  Lisp_Object old_value)
{
  if (NILP (new_value))
    FRAME_Z_GROUP (f) = z_group_none;
  else if (EQ (new_value, Qabove))
    FRAME_Z_GROUP (f) = z_group_above;
  else if (EQ (new_value, Qabove_suspended))
    FRAME_Z_GROUP (f) = z_group_above_suspended;
  else if (EQ (new_value, Qbelow))
    FRAME_Z_GROUP (f) = z_group_below;
  else
    error ("Invalid z-group specification");

  if (nhwl_child_p (f) && !EQ (new_value, old_value))
    {
      if (EQ (new_value, Qabove))
	nhwl_frame_raise_lower (f, true);
      else if (EQ (new_value, Qbelow))
	nhwl_frame_raise_lower (f, false);
    }
}

/* A `no-accept-focus' child never takes its tree's keys
   (nhwl_child_focus); the glue gets the fact.  */
void
nhwl_set_no_accept_focus (struct frame *f, Lisp_Object new_value,
			  Lisp_Object old_value)
{
  FRAME_NO_ACCEPT_FOCUS (f) = !NILP (new_value);
  nhwl_child_reposition (f);
}

/* Make F's window (and those of the child frames under it) again, under
   F's current parent.  The old window of F is gone (its dependents with
   it).  */
static void
nhwl_create_tree (struct frame *f)
{
  Lisp_Object tail, frame;

  nhwl_create_frame_window (f);
  nhwl_child_created (f);
  if (FRAME_VISIBLE_P (f))
    {
      nhwl_damage_all (f);
      SET_FRAME_GARBAGED (f);
    }
  FOR_EACH_FRAME (tail, frame)
    if (FRAME_NHWL_P (XFRAME (frame)) && FRAME_PARENT_FRAME (XFRAME (frame)) == f)
      nhwl_create_tree (XFRAME (frame));
}

static void
nhwl_forget_windows_under (struct frame *f)
{
  Lisp_Object tail, frame;

  FOR_EACH_FRAME (tail, frame)
    if (FRAME_NHWL_P (XFRAME (frame)) && FRAME_PARENT_FRAME (XFRAME (frame)) == f)
      {
	struct nhwl_output *out = FRAME_NHWL_OUTPUT (XFRAME (frame));

	/* Its window is dropped with the parent's: the glue forgets it.  */
	if (out->window)
	  nhwl_window_forget (FRAME_DISPLAY_INFO (f), out->window);
	out->window = 0;
	nhwl_forget_windows_under (XFRAME (frame));
      }
}

/* The `parent-frame' parameter changed at run time.  A top-level cannot
   become a child frame or back; a child changes parent by getting a
   new window under the new one.  */
void
nhwl_set_parent_frame (struct frame *f, Lisp_Object new_value,
		       Lisp_Object old_value)
{
  struct frame *p = NULL, *a;
  struct nhwl_output *out = FRAME_NHWL_OUTPUT (f);

  if (!NILP (new_value)
      && (!FRAMEP (new_value) || !FRAME_LIVE_P (p = XFRAME (new_value))
	  || !FRAME_NHWL_P (p)))
    {
      store_frame_param (f, Qparent_frame, old_value);
      error ("Invalid specification of `parent-frame'");
    }
  if (p == FRAME_PARENT_FRAME (f))
    return;
  if (nhwl_popup_frame_p (f))
    {
      store_frame_param (f, Qparent_frame, old_value);
      error ("nidhogg-wayland: a popup frame keeps its parent");
    }
  if (!p || !FRAME_PARENT_FRAME (f))
    {
      store_frame_param (f, Qparent_frame, old_value);
      error ("nidhogg-wayland: a top-level frame cannot become a child frame, or back");
    }
  if (FRAME_DISPLAY_INFO (f) != FRAME_DISPLAY_INFO (p))
    {
      store_frame_param (f, Qparent_frame, old_value);
      error ("Cross display reparent");
    }
  for (a = p; a; a = FRAME_PARENT_FRAME (a))
    if (a == f)
      {
	store_frame_param (f, Qparent_frame, old_value);
	error ("Cannot make a frame its own ancestor");
      }

  block_input ();
  if (out->window)
    nhwl_window_destroy (FRAME_DISPLAY_INFO (f), out->window);
  out->window = 0;
  nhwl_forget_windows_under (f);
  fset_parent_frame (f, new_value);
  nhwl_create_tree (f);
  unblock_input ();
}

/* Adapted from pgtk_default_font_parameter: the font is opened at the
   display's resolution, which is 96 times the scale, so that its size is
   in device pixels.  */
void
nhwl_default_font_parameter (struct frame *f, Lisp_Object parms)
{
  struct nhwl_display_info *dpyinfo = FRAME_DISPLAY_INFO (f);
  Lisp_Object font_param
    = gui_display_get_arg (dpyinfo, parms, Qfont, NULL, NULL,
			   RES_TYPE_STRING);
  Lisp_Object font = Qnil;

  if (BASE_EQ (font_param, Qunbound))
    font_param = Qnil;

  if (NILP (font))
    font = !NILP (font_param) ? font_param
      : gui_display_get_arg (dpyinfo, parms, Qfont, "font", "Font",
			     RES_TYPE_STRING);

  if (!FONTP (font) && !STRINGP (font))
    {
      const char *names[] = {
	"monospace-10",
	"-adobe-courier-medium-r-*-*-*-120-*-*-*-*-iso8859-1",
	"-misc-fixed-medium-r-normal-*-*-140-*-*-c-*-iso8859-1",
	"-*-*-medium-r-normal-*-*-140-*-*-c-*-iso8859-1",
	/* This was formerly the first thing tried, but it finds
	   too many fonts and takes too long.  */
	"-*-*-medium-r-*-*-*-*-*-*-c-*-iso8859-1",
	/* If those didn't work, look for something which will
	   at least work.  */
	"-*-fixed-*-*-*-*-*-140-*-*-c-*-iso8859-1",
	"fixed",
	NULL
      };
      int i;

      for (i = 0; names[i]; i++)
	{
	  font = font_open_by_name (f, build_unibyte_string (names[i]));
	  if (!NILP (font))
	    break;
	}
      if (NILP (font))
	error ("No suitable font was found");
    }

  gui_default_parameter (f, parms, Qfont, font, "font", "Font",
			 RES_TYPE_STRING);
}

/* The parameter handlers, in the order of frame.c's table.  Entries that
   are 0 are accepted and stored only: each is a no-op the shell's README
   lists ("Frame parameters"), with the reason.  */
extern frame_parm_handler nhwl_frame_parm_handlers[];
frame_parm_handler nhwl_frame_parm_handlers[] =
  {
    gui_set_autoraise,		/* generic OK */
    gui_set_autolower,		/* generic OK */
    nhwl_set_background_color,
    nhwl_set_border_color,
    gui_set_border_width,
    nhwl_set_cursor_color,
    nhwl_set_cursor_type,
    gui_set_font,		/* generic OK */
    nhwl_set_foreground_color,
    0,				/* icon-name: Wayland has no icon title */
    0,				/* icon-type: the desktop file's icon */
    nhwl_set_child_frame_border_width,
    nhwl_set_internal_border_width,
    gui_set_right_divider_width,
    gui_set_bottom_divider_width,
    nhwl_set_menu_bar_lines,
    nhwl_set_mouse_color,
    nhwl_explicitly_set_name,
    0,				/* scroll-bar-width: no scroll bars */
    0,				/* scroll-bar-height */
    nhwl_set_title,
    gui_set_unsplittable,	/* generic OK */
    0,				/* vertical-scroll-bars */
    0,				/* horizontal-scroll-bars */
    gui_set_visibility,		/* generic OK */
    nhwl_set_tab_bar_lines,
    nhwl_set_tool_bar_lines,
    0,				/* scroll-bar-foreground */
    0,				/* scroll-bar-background */
    gui_set_screen_gamma,	/* generic OK */
    gui_set_line_spacing,	/* generic OK, sets f->extra_line_spacing to int */
    gui_set_left_fringe,	/* generic OK */
    gui_set_right_fringe,	/* generic OK */
    0,				/* wait-for-wm: the first configure is awaited */
    gui_set_fullscreen,		/* generic OK */
    gui_set_font_backend,	/* generic OK */
    nhwl_set_alpha,		/* nhwl_set_frame_alpha applies it */
    0,				/* sticky: no protocol */
    0,				/* tool-bar-position: the tool bar is Emacs's */
    0,				/* inhibit-double-buffering: one backing */
    nhwl_set_undecorated,
    nhwl_set_parent_frame,
    0,				/* skip-taskbar: no protocol */
    0,				/* no-focus-on-map: the compositor decides */
    nhwl_set_no_accept_focus,
    nhwl_set_z_group,
    0,				/* override-redirect: child frames only */
    gui_set_no_special_glyphs,
    nhwl_set_alpha_background,
    gui_set_borders_respect_alpha_background,
    NULL,
  };


/***********************************************************************
		   Frame creation and the display primitives
 ***********************************************************************/

/* Handler for signals raised during x_create_frame.  FRAME is the frame
   which is partially constructed.  Adapted from unwind_create_frame.  */
static Lisp_Object
unwind_create_frame (Lisp_Object frame)
{
  struct frame *f = XFRAME (frame);

  /* If frame is already dead, nothing to do.  */
  if (!FRAME_LIVE_P (f))
    return Qnil;

  /* If frame is ``official'', nothing to do.  */
  if (NILP (Fmemq (frame, Vframe_list)))
    {
      nhwl_free_frame_resources (f);
      free_glyphs (f);
      return Qt;
    }

  return Qnil;
}

static void
do_unwind_create_frame (Lisp_Object frame)
{
  unwind_create_frame (frame);
}

DEFUN ("x-create-frame", Fx_create_frame, Sx_create_frame, 1, 1, 0,
       doc: /* SKIP: real doc in xfns.c.  */ )
  (Lisp_Object parms)
{
  struct frame *f;
  Lisp_Object frame, tem;
  Lisp_Object name;
  bool minibuffer_only = false;
  bool undecorated = false;
  long window_prompting = 0;
  specpdl_ref count = SPECPDL_INDEX ();
  Lisp_Object display;
  struct nhwl_display_info *dpyinfo = NULL;
  Lisp_Object parent, parent_frame;
  struct kboard *kb;
  struct nhwl_output *out;

  parms = Fcopy_alist (parms);

  /* Use this general default value to start with
     until we know if this frame has a specified name.  */
  Vx_resource_name = Vinvocation_name;

  display = gui_display_get_arg (dpyinfo, parms, Qterminal, 0, 0,
				 RES_TYPE_NUMBER);
  if (BASE_EQ (display, Qunbound))
    display = gui_display_get_arg (dpyinfo, parms, Qdisplay, 0, 0,
				   RES_TYPE_STRING);
  if (BASE_EQ (display, Qunbound))
    display = Qnil;
  dpyinfo = check_nhwl_display_info (display);
  kb = dpyinfo->terminal->kboard;

  if (!dpyinfo->terminal->name)
    error ("Terminal is not live, can't create new frames on it");
  /* No window can be made until the compositor is back.  */
  if (dpyinfo->reconnecting)
    error ("Display is reconnecting");

  name = gui_display_get_arg (dpyinfo, parms, Qname, "name", "Name",
			      RES_TYPE_STRING);
  if (!STRINGP (name) && !BASE_EQ (name, Qunbound) && !NILP (name))
    error ("Invalid frame name--not a string or nil");

  if (STRINGP (name))
    Vx_resource_name = name;

  /* Child frames are subsurfaces of another frame's window (child.rs);
     a foreign parent window (parent-id) has no meaning on Wayland.  */
  parent = gui_display_get_arg (dpyinfo, parms, Qparent_id, NULL, NULL,
				RES_TYPE_NUMBER);
  if (BASE_EQ (parent, Qunbound))
    parent = Qnil;
  if (!NILP (parent))
    error ("nidhogg-wayland: parent-id is not supported");
  parent_frame = gui_display_get_arg (dpyinfo, parms, Qparent_frame, NULL,
				      NULL, RES_TYPE_SYMBOL);
  if (BASE_EQ (parent_frame, Qunbound) || NILP (parent_frame)
      || !FRAMEP (parent_frame) || !FRAME_LIVE_P (XFRAME (parent_frame))
      || !FRAME_NHWL_P (XFRAME (parent_frame))
      || FRAME_DISPLAY_INFO (XFRAME (parent_frame)) != dpyinfo)
    parent_frame = Qnil;

  frame = Qnil;
  tem = gui_display_get_arg (dpyinfo, parms, Qminibuffer, "minibuffer",
			     "Minibuffer", RES_TYPE_SYMBOL);
  if (EQ (tem, Qnone) || NILP (tem))
    f = make_frame_without_minibuffer (Qnil, kb, display);
  else if (EQ (tem, Qonly))
    {
      f = make_minibuffer_frame ();
      minibuffer_only = true;
    }
  else if (WINDOWP (tem))
    f = make_frame_without_minibuffer (tem, kb, display);
  else
    f = make_frame (true);

  fset_parent_frame (f, parent_frame);
  store_frame_param (f, Qparent_frame, parent_frame);

  if (!NILP (tem = gui_display_get_arg (dpyinfo, parms, Qundecorated, NULL,
					NULL, RES_TYPE_BOOLEAN))
      && !(BASE_EQ (tem, Qunbound)))
    undecorated = true;

  FRAME_UNDECORATED (f) = undecorated;
  store_frame_param (f, Qundecorated, undecorated ? Qt : Qnil);

  FRAME_OVERRIDE_REDIRECT (f) = false;
  store_frame_param (f, Qoverride_redirect, Qnil);

  XSETFRAME (frame, f);

  frame_set_id_from_params (f, parms);

  f->terminal = dpyinfo->terminal;

  f->output_method = output_nhwl;
  FRAME_X_OUTPUT (f) = xzalloc (sizeof *FRAME_X_OUTPUT (f));
  out = FRAME_NHWL_OUTPUT (f);
  out->frame = f;
  FRAME_FONTSET (f) = -1;
  out->white_relief.pixel = -1;
  out->black_relief.pixel = -1;

  fset_icon_name (f, gui_display_get_arg (dpyinfo, parms, Qicon_name,
					  "iconName", "Title", RES_TYPE_STRING));
  if (!STRINGP (f->icon_name))
    fset_icon_name (f, Qnil);

  FRAME_DISPLAY_INFO (f) = dpyinfo;

  /* With FRAME_DISPLAY_INFO set up, this unwind-protect is safe.  */
  record_unwind_protect (do_unwind_create_frame, frame);

  /* A menu's frame: its window is a popup, made when it is shown.  */
  nhwl_popup_init (f, parms);

  /* These colors will be set anyway later, but it's important
     to get the color reference counts right, so initialize them!  */
  {
    Lisp_Object black;

    /* Function x_decode_color can signal an error.  Make
       sure to initialize color slots so that we won't try
       to free colors we haven't allocated.  */
    FRAME_FOREGROUND_PIXEL (f) = -1;
    FRAME_BACKGROUND_PIXEL (f) = -1;
    out->cursor_color = -1;
    out->cursor_foreground_color = -1;
    out->border_pixel = -1;
    out->mouse_color = -1;

    black = build_string ("black");
    FRAME_FOREGROUND_PIXEL (f)
      = x_decode_color (f, black, BLACK_PIX_DEFAULT (f));
    FRAME_BACKGROUND_PIXEL (f)
      = x_decode_color (f, black, BLACK_PIX_DEFAULT (f));
    out->cursor_color = x_decode_color (f, black, BLACK_PIX_DEFAULT (f));
    out->cursor_foreground_color
      = x_decode_color (f, black, BLACK_PIX_DEFAULT (f));
    out->border_pixel = x_decode_color (f, black, BLACK_PIX_DEFAULT (f));
    out->mouse_color = x_decode_color (f, black, BLACK_PIX_DEFAULT (f));
  }

  /* Set the name; the functions to which we pass f expect the name to
     be set.  */
  if (BASE_EQ (name, Qunbound) || NILP (name))
    {
      fset_name (f, build_string (dpyinfo->x_id_name));
      f->explicit_name = false;
    }
  else
    {
      fset_name (f, name);
      f->explicit_name = true;
      /* Use the frame's title when getting resources for this frame.  */
      specbind (Qx_resource_name, name);
    }

  register_font_driver (&ftcrfont_driver, f);
#ifdef HAVE_HARFBUZZ
  register_font_driver (&ftcrhbfont_driver, f);
#endif	/* HAVE_HARFBUZZ */

  gui_default_parameter (f, parms, Qfont_backend, Qnil,
			 "fontBackend", "FontBackend", RES_TYPE_STRING);

  /* Extract the window parameters from the supplied values
     that are needed to determine window geometry.  */
  nhwl_default_font_parameter (f, parms);
  if (!FRAME_FONT (f))
    {
      delete_frame (frame, Qnoelisp);
      error ("Invalid frame font");
    }

  gui_default_parameter (f, parms, Qborder_width, make_fixnum (0),
			 "borderWidth", "BorderWidth", RES_TYPE_NUMBER);

  if (NILP (Fassq (Qinternal_border_width, parms)))
    {
      Lisp_Object value;

      value = gui_display_get_arg (dpyinfo, parms, Qinternal_border_width,
				   "internalBorder", "internalBorder",
				   RES_TYPE_NUMBER);
      if (!BASE_EQ (value, Qunbound))
	parms = Fcons (Fcons (Qinternal_border_width, value), parms);
    }

  gui_default_parameter (f, parms, Qinternal_border_width, make_fixnum (0),
			 "internalBorderWidth", "internalBorderWidth",
			 RES_TYPE_NUMBER);

  /* Same for child frames.  */
  if (NILP (Fassq (Qchild_frame_border_width, parms)))
    {
      Lisp_Object value;

      value = gui_display_get_arg (dpyinfo, parms, Qchild_frame_border_width,
				   "childFrameBorder", "childFrameBorder",
				   RES_TYPE_NUMBER);
      if (! BASE_EQ (value, Qunbound))
	parms = Fcons (Fcons (Qchild_frame_border_width, value), parms);
    }

  gui_default_parameter (f, parms, Qchild_frame_border_width, Qnil,
			 "childFrameBorderWidth", "childFrameBorderWidth",
			 RES_TYPE_NUMBER);
  gui_default_parameter (f, parms, Qright_divider_width, make_fixnum (0),
			 NULL, NULL, RES_TYPE_NUMBER);
  gui_default_parameter (f, parms, Qbottom_divider_width, make_fixnum (0),
			 NULL, NULL, RES_TYPE_NUMBER);
  /* The scroll bar parameters are stored and ignored.  */
  gui_default_parameter (f, parms, Qvertical_scroll_bars, Qnil,
			 "verticalScrollBars", "ScrollBars", RES_TYPE_SYMBOL);
  gui_default_parameter (f, parms, Qhorizontal_scroll_bars, Qnil,
			 "horizontalScrollBars", "ScrollBars", RES_TYPE_SYMBOL);
  /* Also do the stuff which must be set before the window exists.  */
  gui_default_parameter (f, parms, Qforeground_color, build_string ("black"),
			 "foreground", "Foreground", RES_TYPE_STRING);
  gui_default_parameter (f, parms, Qbackground_color, build_string ("white"),
			 "background", "Background", RES_TYPE_STRING);
  gui_default_parameter (f, parms, Qmouse_color, build_string ("black"),
			 "pointerColor", "Foreground", RES_TYPE_STRING);
  gui_default_parameter (f, parms, Qborder_color, build_string ("black"),
			 "borderColor", "BorderColor", RES_TYPE_STRING);
  gui_default_parameter (f, parms, Qscreen_gamma, Qnil,
			 "screenGamma", "ScreenGamma", RES_TYPE_FLOAT);
  gui_default_parameter (f, parms, Qline_spacing, Qnil,
			 "lineSpacing", "LineSpacing", RES_TYPE_NUMBER);
  gui_default_parameter (f, parms, Qleft_fringe, Qnil,
			 "leftFringe", "LeftFringe", RES_TYPE_NUMBER);
  gui_default_parameter (f, parms, Qright_fringe, Qnil,
			 "rightFringe", "RightFringe", RES_TYPE_NUMBER);
  gui_default_parameter (f, parms, Qno_special_glyphs, Qnil,
			 NULL, NULL, RES_TYPE_BOOLEAN);

  /* Init faces before gui_default_parameter is called for the
     scroll-bar-width parameter because otherwise we end up in
     init_iterator with a null face cache, which should not happen.  */
  init_frame_faces (f);

  /* We have to call adjust_frame_size here since otherwise
     nhwl_set_tool_bar_lines will already work with the character
     sizes installed by init_frame_faces while the frame's pixel size
     is still calculated from a character size of 1 and we
     subsequently hit the (height >= 0) assertion in
     window_box_height.  Also process `min-width' and `min-height'
     parameters right here because `frame-windows-min-size' needs
     them.  */
  tem = gui_display_get_arg (dpyinfo, parms, Qmin_width, NULL, NULL,
			     RES_TYPE_NUMBER);
  if (NUMBERP (tem))
    store_frame_param (f, Qmin_width, tem);
  tem = gui_display_get_arg (dpyinfo, parms, Qmin_height, NULL, NULL,
			     RES_TYPE_NUMBER);
  if (NUMBERP (tem))
    store_frame_param (f, Qmin_height, tem);
  adjust_frame_size (f, FRAME_COLS (f) * FRAME_COLUMN_WIDTH (f),
		     FRAME_LINES (f) * FRAME_LINE_HEIGHT (f), 5, true,
		     Qx_create_frame_1);

  /* Set the menu-bar-lines and tool-bar-lines parameters.  We don't
     look up the X resources controlling the menu-bar and tool-bar
     here; they are processed specially at startup, and reflected in
     the values of the mode variables.  */

  gui_default_parameter (f, parms, Qmenu_bar_lines,
			 NILP (Vmenu_bar_mode)
			 ? make_fixnum (0) : make_fixnum (1),
			 NULL, NULL, RES_TYPE_NUMBER);
  gui_default_parameter (f, parms, Qtab_bar_lines,
			 NILP (Vtab_bar_mode)
			 ? make_fixnum (0) : make_fixnum (1),
			 NULL, NULL, RES_TYPE_NUMBER);
  gui_default_parameter (f, parms, Qtool_bar_lines,
			 NILP (Vtool_bar_mode)
			 ? make_fixnum (0) : make_fixnum (1),
			 NULL, NULL, RES_TYPE_NUMBER);

  gui_default_parameter (f, parms, Qbuffer_predicate, Qnil,
			 "bufferPredicate", "BufferPredicate",
			 RES_TYPE_SYMBOL);
  gui_default_parameter (f, parms, Qtitle, Qnil,
			 "title", "Title", RES_TYPE_STRING);
  gui_default_parameter (f, parms, Qsession_name, Qnil, NULL, NULL,
			 RES_TYPE_STRING);
  gui_default_parameter (f, parms, Qwait_for_wm, Qt,
			 "waitForWM", "WaitForWM", RES_TYPE_BOOLEAN);
  gui_default_parameter (f, parms, Qtool_bar_position,
			 FRAME_TOOL_BAR_POSITION (f), 0, 0, RES_TYPE_SYMBOL);
  gui_default_parameter (f, parms, Qinhibit_double_buffering, Qnil,
			 "inhibitDoubleBuffering", "InhibitDoubleBuffering",
			 RES_TYPE_BOOLEAN);

  /* Compute the size of the window.  */
  window_prompting = gui_figure_window_size (f, parms, true, true);

  tem = gui_display_get_arg (dpyinfo, parms, Qunsplittable, 0, 0,
			     RES_TYPE_BOOLEAN);
  f->no_split = minibuffer_only || EQ (tem, Qt);

  /* The shell's window, at the size the frame will have.  */
  if (!nhwl_popup_frame_p (f))
    nhwl_create_frame_window (f);
  nhwl_child_created (f);
  out->next_output = dpyinfo->outputs;
  dpyinfo->outputs = out;

  out->current_cursor = out->nontext_cursor
    = nhwl_cursor_for_shape (NHG_CURSOR_DEFAULT);
  out->text_cursor = out->modeline_cursor
    = nhwl_cursor_for_shape (NHG_CURSOR_TEXT);
  out->hand_cursor = nhwl_cursor_for_shape (NHG_CURSOR_POINTER);
  out->hourglass_cursor = nhwl_cursor_for_shape (NHG_CURSOR_WAIT);
  out->horizontal_drag_cursor = out->left_edge_cursor = out->right_edge_cursor
    = nhwl_cursor_for_shape (NHG_CURSOR_RESIZE_EW);
  out->vertical_drag_cursor = out->top_edge_cursor = out->bottom_edge_cursor
    = nhwl_cursor_for_shape (NHG_CURSOR_RESIZE_NS);
  out->top_left_corner_cursor = out->bottom_right_corner_cursor
    = nhwl_cursor_for_shape (NHG_CURSOR_RESIZE_NWSE);
  out->top_right_corner_cursor = out->bottom_left_corner_cursor
    = nhwl_cursor_for_shape (NHG_CURSOR_RESIZE_NESW);

  /* Now consider the frame official.  */
  f->terminal->reference_count++;
  FRAME_DISPLAY_INFO (f)->reference_count++;
  Vframe_list = Fcons (frame, Vframe_list);

  gui_default_parameter (f, parms, Qicon_type, Qt,
			 "bitmapIcon", "BitmapIcon", RES_TYPE_BOOLEAN);

  gui_default_parameter (f, parms, Qauto_raise, Qnil,
			 "autoRaise", "AutoRaiseLower", RES_TYPE_BOOLEAN);
  gui_default_parameter (f, parms, Qauto_lower, Qnil,
			 "autoLower", "AutoRaiseLower", RES_TYPE_BOOLEAN);
  gui_default_parameter (f, parms, Qcursor_type, Qbox,
			 "cursorType", "CursorType", RES_TYPE_SYMBOL);
  gui_default_parameter (f, parms, Qscroll_bar_width, Qnil,
			 "scrollBarWidth", "ScrollBarWidth", RES_TYPE_NUMBER);
  gui_default_parameter (f, parms, Qscroll_bar_height, Qnil,
			 "scrollBarHeight", "ScrollBarHeight",
			 RES_TYPE_NUMBER);
  gui_default_parameter (f, parms, Qalpha, Qnil,
			 "alpha", "Alpha", RES_TYPE_NUMBER);
  gui_default_parameter (f, parms, Qalpha_background, Qnil,
			 "alphaBackground", "AlphaBackground", RES_TYPE_NUMBER);
  gui_default_parameter (f, parms, Qborders_respect_alpha_background, Qnil,
			 "bordersRespectAlphaBackground",
			 "BordersRespectAlphaBackground", RES_TYPE_NUMBER);

  gui_default_parameter (f, parms, Qno_focus_on_map, Qnil,
			 NULL, NULL, RES_TYPE_BOOLEAN);
  gui_default_parameter (f, parms, Qno_accept_focus, Qnil,
			 NULL, NULL, RES_TYPE_BOOLEAN);

  /* Consider frame official, now.  */
  f->can_set_window_size = true;

  adjust_frame_size (f, FRAME_TEXT_WIDTH (f), FRAME_TEXT_HEIGHT (f),
		     0, true, Qx_create_frame_2);

  /* Process fullscreen parameter here in the hope that normalizing a
     fullheight/fullwidth frame will produce the size set by the last
     adjust_frame_size call.  */
  gui_default_parameter (f, parms, Qfullscreen, Qnil,
			 "fullscreen", "Fullscreen", RES_TYPE_SYMBOL);

  /* Make the window appear on the frame and enable display, unless
     the caller says not to.  */
  {
    /* When called from `x-create-frame-with-faces' visibility is
       always explicitly nil.  */
    Lisp_Object visibility
      = gui_display_get_arg (dpyinfo, parms, Qvisibility, 0, 0,
			     RES_TYPE_SYMBOL);
    Lisp_Object height
      = gui_display_get_arg (dpyinfo, parms, Qheight, 0, 0, RES_TYPE_NUMBER);
    Lisp_Object width
      = gui_display_get_arg (dpyinfo, parms, Qwidth, 0, 0, RES_TYPE_NUMBER);

    if (EQ (visibility, Qicon))
      {
	f->was_invisible = true;
	nhwl_iconify_frame (f);
      }
    else
      {
	if (BASE_EQ (visibility, Qunbound))
	  visibility = Qt;

	if (!NILP (visibility))
	  nhwl_make_frame_visible (f);
	else
	  f->was_invisible = true;
      }

    /* Leave f->was_invisible true only if height or width were
       specified too.  This takes effect only when we are not called
       from `x-create-frame-with-faces' (see above comment).  */
    f->was_invisible
      = (f->was_invisible
	 && (!BASE_EQ (height, Qunbound) || !BASE_EQ (width, Qunbound)));

    store_frame_param (f, Qvisibility, visibility);
  }

  /* A client frame made by emacsclient carries the client's environment;
     its XDG_ACTIVATION_TOKEN lets the compositor give the new window the
     focus.  */
  if (!nhwl_child_p (f) && FRAME_NHWL_OUTPUT (f)->window)
    {
      Lisp_Object env = Fcdr (Fassq (intern_c_string ("environment"), parms));
      static const char prefix[] = "XDG_ACTIVATION_TOKEN=";

      for (; CONSP (env); env = XCDR (env))
	if (STRINGP (XCAR (env))
	    && strncmp (SSDATA (XCAR (env)), prefix, sizeof prefix - 1) == 0
	    && SDATA (XCAR (env))[sizeof prefix - 1])
	  {
	    block_input ();
	    NHG (nhg_window_activate (dpyinfo->glue,
				      FRAME_NHWL_OUTPUT (f)->window,
				      SSDATA (XCAR (env)) + sizeof prefix - 1));
	    unblock_input ();
	    break;
	  }
    }

  /* Works iff frame has been already mapped.  */
  gui_default_parameter (f, parms, Qskip_taskbar, Qnil,
			 NULL, NULL, RES_TYPE_BOOLEAN);
  /* The `z-group' parameter works only for visible frames.  */
  gui_default_parameter (f, parms, Qz_group, Qnil,
			 NULL, NULL, RES_TYPE_SYMBOL);

  /* Initialize `default-minibuffer-frame' in case this is the first
     frame on this terminal.  */
  if (FRAME_HAS_MINIBUF_P (f)
      && (!FRAMEP (KVAR (kb, Vdefault_minibuffer_frame))
	  || !FRAME_LIVE_P (XFRAME (KVAR (kb, Vdefault_minibuffer_frame)))))
    kset_default_minibuffer_frame (kb, frame);

  /* All remaining specified parameters, which have not been "used"
     by gui_display_get_arg and friends, now go in the misc. alist of the
     frame.  */
  for (tem = parms; CONSP (tem); tem = XCDR (tem))
    if (CONSP (XCAR (tem)) && !NILP (XCAR (XCAR (tem))))
      fset_param_alist (f, Fcons (XCAR (tem), f->param_alist));

  /* Make sure windows on this frame appear in calls to next-window
     and similar functions.  */
  Vwindow_list = Qnil;

  /* A child frame's chrome, now that all its parameters are in the alist.  */
  nhwl_child_params_changed (f);

  return unbind_to (count, frame);
}

/* Adapted from pgtk_display_info_for_name use: the primitives below take
   a terminal, a frame or a display name.  */

DEFUN ("x-server-max-request-size", Fx_server_max_request_size,
       Sx_server_max_request_size, 0, 1, 0,
       doc: /* SKIP: real doc in xfns.c.  */ )
  (Lisp_Object terminal)
{
  check_nhwl_display_info (terminal);
  return Qnil;
}

DEFUN ("x-display-screens", Fx_display_screens, Sx_display_screens, 0, 1, 0,
       doc: /* SKIP: real doc in xfns.c.  */)
  (Lisp_Object terminal)
{
  check_nhwl_display_info (terminal);
  return make_fixnum (1);
}

/* What the x-display-* functions ask of the display: its extent in
   pixels and millimetres, from the glue (the same arithmetic as pgtk's).  */
static NhgDisplayInfo
nhwl_display_info (Lisp_Object terminal)
{
  struct nhwl_display_info *dpyinfo = check_nhwl_display_info (terminal);
  NhgDisplayInfo info;

  memset (&info, 0, sizeof info);
  nhwl_glue_rc (dpyinfo, NHG (nhg_display_info (dpyinfo->glue, &info)));
  return info;
}

DEFUN ("x-display-mm-height", Fx_display_mm_height, Sx_display_mm_height, 0, 1,
       0, doc: /* SKIP: real doc in xfns.c.  */)
  (Lisp_Object terminal)
{
  return make_fixnum (nhwl_display_info (terminal).mm_h);
}

DEFUN ("x-display-mm-width", Fx_display_mm_width, Sx_display_mm_width, 0, 1, 0,
       doc: /* SKIP: real doc in xfns.c.  */)
  (Lisp_Object terminal)
{
  return make_fixnum (nhwl_display_info (terminal).mm_w);
}

DEFUN ("x-display-backing-store", Fx_display_backing_store,
       Sx_display_backing_store, 0, 1, 0,
       doc: /* SKIP: real doc in xfns.c.  */)
  (Lisp_Object terminal)
{
  check_nhwl_display_info (terminal);
  return Qnil;
}

DEFUN ("x-display-visual-class", Fx_display_visual_class,
       Sx_display_visual_class, 0, 1, 0,
       doc: /* SKIP: real doc in xfns.c.  */)
  (Lisp_Object terminal)
{
  return intern ("true-color");
}

DEFUN ("x-display-save-under", Fx_display_save_under, Sx_display_save_under,
       0, 1, 0, doc: /* SKIP: real doc in xfns.c.  */)
  (Lisp_Object terminal)
{
  check_nhwl_display_info (terminal);
  return Qnil;
}

DEFUN ("x-display-planes", Fx_display_planes, Sx_display_planes, 0, 1, 0,
       doc: /* SKIP: real doc in xfns.c.  */)
  (Lisp_Object terminal)
{
  check_nhwl_display_info (terminal);
  return make_fixnum (24);
}

DEFUN ("x-display-color-cells", Fx_display_color_cells,
       Sx_display_color_cells, 0, 1, 0,
       doc: /* SKIP: real doc in xfns.c.  */)
  (Lisp_Object terminal)
{
  check_nhwl_display_info (terminal);
  return make_fixnum (1 << 24);
}

DEFUN ("x-display-pixel-width", Fx_display_pixel_width,
       Sx_display_pixel_width, 0, 1, 0,
       doc: /* SKIP: real doc in xfns.c.  */)
  (Lisp_Object terminal)
{
  return make_fixnum (nhwl_display_info (terminal).w);
}

DEFUN ("x-display-pixel-height", Fx_display_pixel_height,
       Sx_display_pixel_height, 0, 1, 0,
       doc: /* SKIP: real doc in xfns.c.  */)
  (Lisp_Object terminal)
{
  return make_fixnum (nhwl_display_info (terminal).h);
}

DEFUN ("xw-color-defined-p", Fxw_color_defined_p, Sxw_color_defined_p, 1, 2, 0,
       doc: /* SKIP: real doc in xfns.c.  */)
  (Lisp_Object color, Lisp_Object frame)
{
  Emacs_Color col;
  struct frame *f = decode_window_system_frame (frame);

  CHECK_STRING (color);

  if (nhwl_defined_color (f, SSDATA (color), &col, false, false))
    return Qt;
  else
    return Qnil;
}

DEFUN ("xw-color-values", Fxw_color_values, Sxw_color_values, 1, 2, 0,
       doc: /* SKIP: real doc in xfns.c.  */)
  (Lisp_Object color, Lisp_Object frame)
{
  Emacs_Color col;
  struct frame *f = decode_window_system_frame (frame);

  CHECK_STRING (color);

  if (nhwl_defined_color (f, SSDATA (color), &col, false, false))
    return list3i (col.red, col.green, col.blue);
  else
    return Qnil;
}

DEFUN ("xw-display-color-p", Fxw_display_color_p, Sxw_display_color_p, 0, 1, 0,
       doc: /* SKIP: real doc in xfns.c.  */)
  (Lisp_Object terminal)
{
  check_nhwl_display_info (terminal);
  return Qt;
}

DEFUN ("x-display-grayscale-p", Fx_display_grayscale_p,
       Sx_display_grayscale_p, 0, 1, 0,
       doc: /* SKIP: real doc in xfns.c.  */)
  (Lisp_Object terminal)
{
  return Qt;
}

DEFUN ("x-open-connection", Fx_open_connection, Sx_open_connection, 1, 3, 0,
       doc: /* SKIP: real doc in xfns.c.  */)
  (Lisp_Object display, Lisp_Object resource_string, Lisp_Object must_succeed)
{
  struct nhwl_display_info *dpyinfo;

  if (NILP (display))
    display = build_string ("");

  CHECK_STRING (display);

  dpyinfo = nhwl_term_init (display);
  if (dpyinfo == 0)
    {
      if (!NILP (must_succeed))
	fatal ("Display on %s not responding.\n", SSDATA (display));
      else
	error ("Display on %s not responding.\n", SSDATA (display));
    }

  return Qnil;
}

DEFUN ("x-close-connection", Fx_close_connection, Sx_close_connection, 1, 1, 0,
       doc: /* SKIP: real doc in xfns.c.  */)
  (Lisp_Object terminal)
{
  struct nhwl_display_info *dpyinfo = check_nhwl_display_info (terminal);

  if (dpyinfo->reference_count > 0)
    error ("Display still has frames on it");

  nhwl_delete_terminal (dpyinfo->terminal);

  return Qnil;
}

DEFUN ("x-display-list", Fx_display_list, Sx_display_list, 0, 0, 0,
       doc: /* SKIP: real doc in xfns.c.  */)
  (void)
{
  Lisp_Object result = Qnil;
  struct nhwl_display_info *ndi;

  for (ndi = x_display_list; ndi; ndi = ndi->next)
    result = Fcons (XCAR (ndi->name_list_element), result);

  return result;
}

DEFUN ("pgtk-backend-display-class", Fpgtk_backend_display_class,
       Spgtk_backend_display_class, 0, 1, "",
       doc: /* Return the name of the display class of TERMINAL.
Under Nidhogg's own Wayland shell it is always "NhwlDisplay".  */)
  (Lisp_Object terminal)
{
  check_nhwl_display_info (terminal);
  return build_string ("NhwlDisplay");
}

/* Adapted from Fpgtk_display_monitor_attributes_list.  Geometry is in
   device pixels, as everything the shared code sees.  */
DEFUN ("pgtk-display-monitor-attributes-list",
       Fpgtk_display_monitor_attributes_list,
       Spgtk_display_monitor_attributes_list, 0, 1, 0,
       doc: /* Return a list of physical monitor attributes on the display TERMINAL.

The optional argument TERMINAL specifies which display to ask about.
TERMINAL should be a terminal object, a frame or a display name (a string).
If omitted or nil, that stands for the selected frame's display.

In addition to the standard attribute keys listed in
`display-monitor-attributes-list', the following keys are contained in
the attributes:

 source -- String describing the source from which multi-monitor
	   information is obtained, \"Wayland\"

Internal use only, use `display-monitor-attributes-list' instead.  */)
  (Lisp_Object terminal)
{
  struct nhwl_display_info *dpyinfo = check_nhwl_display_info (terminal);
  Lisp_Object attributes_list = Qnil;
  int i, n_monitors;
  size_t n_listed = 0;
  NhgMonitor list[NHWL_MAX_OUTPUTS];
  Lisp_Object monitor_frames, rest, frame;
  static const char *source = "Wayland";
  struct MonitorInfo *monitors;

  nhwl_glue_rc (dpyinfo, NHG (nhg_monitors (dpyinfo->glue, list,
					    NHWL_MAX_OUTPUTS, &n_listed)));
  n_monitors = n_listed;
  if (n_monitors == 0)
    return Qnil;

  block_input ();
  monitor_frames = make_nil_vector (n_monitors);
  monitors = xzalloc (n_monitors * sizeof *monitors);

  FOR_EACH_FRAME (rest, frame)
    {
      struct frame *f = XFRAME (frame);

      if (FRAME_NHWL_P (f) && FRAME_DISPLAY_INFO (f) == dpyinfo
	  && !FRAME_TOOLTIP_P (f))
	{
	  /* Each output the compositor says the frame is on; the first when
	     it has not said (before the first enter, or after its output
	     went away).  The glue matches them with the list.  */
	  uint64_t window = FRAME_NHWL_OUTPUT (f)->window;
	  uint32_t on = 1;
	  int j;

	  if (window)
	    nhwl_glue_rc (dpyinfo, NHG (nhg_frame_monitors (dpyinfo->glue,
							    window, &on)));
	  for (j = 0; j < n_monitors; j++)
	    if (on & (1u << j))
	      ASET (monitor_frames, j, Fcons (frame, AREF (monitor_frames, j)));
	}
    }

  for (i = 0; i < n_monitors; ++i)
    {
      NhgMonitor *m = &list[i];
      struct MonitorInfo *mi = &monitors[i];

      NHG (nhg_monitor_origin (m->x, m->y, m->scale120, &mi->geom.x,
			       &mi->geom.y));
      mi->geom.width = m->w;
      mi->geom.height = m->h;
      mi->work = mi->geom;
      mi->mm_width = m->mm_w;
      mi->mm_height = m->mm_h;
      mi->scale_factor = (m->scale120 ? m->scale120 : 120) / 120.0;
      dupstring (&mi->name, m->name);
    }

  attributes_list = make_monitor_attribute_list (monitors, n_monitors, 0,
						 monitor_frames, source);
  for (i = 0; i < n_monitors; ++i)
    xfree (monitors[i].name);
  xfree (monitors);
  unblock_input ();

  return attributes_list;
}

/* Return geometric attributes of FRAME.  Adapted from frame_geometry
   (pgtkfns.c): a Wayland client has no position, the decorations are
   drawn inside the frame, and the origin is (0, 0).  */
static Lisp_Object
frame_geometry (Lisp_Object frame, Lisp_Object attribute)
{
  struct frame *f = decode_live_frame (frame);
  int native_width = FRAME_PIXEL_WIDTH (f);
  int native_height = FRAME_PIXEL_HEIGHT (f);
  int left_pos = 0, top_pos = 0;
  int native_right = native_width, native_bottom = native_height;
  int internal_border_width = FRAME_INTERNAL_BORDER_WIDTH (f);
  int tab_bar_height = FRAME_TAB_BAR_HEIGHT (f);
  int tab_bar_width = (tab_bar_height
		       ? native_width - 2 * internal_border_width : 0);
  int tool_bar_height = FRAME_TOOLBAR_HEIGHT (f);
  int tool_bar_width = (tool_bar_height
			? native_width - 2 * internal_border_width : 0);

  if (FRAME_NHWL_OUTPUT (f) == NULL)
    return Qnil;	/* This can occur while creating a frame.  */

  /* Construct list.  */
  if (EQ (attribute, Qouter_edges))
    return list4 (make_fixnum (left_pos), make_fixnum (top_pos),
		  make_fixnum (native_right), make_fixnum (native_bottom));
  else if (EQ (attribute, Qnative_edges))
    return list4 (make_fixnum (left_pos), make_fixnum (top_pos),
		  make_fixnum (native_right), make_fixnum (native_bottom));
  else if (EQ (attribute, Qinner_edges))
    return list4 (make_fixnum (left_pos + internal_border_width),
		  make_fixnum (top_pos + tab_bar_height
			       + FRAME_TOOL_BAR_TOP_HEIGHT (f)
			       + internal_border_width),
		  make_fixnum (native_right - internal_border_width),
		  make_fixnum (native_bottom - internal_border_width
			       - FRAME_TOOL_BAR_BOTTOM_HEIGHT (f)));
  else
    return
      list (Fcons (Qouter_position,
		   Fcons (make_fixnum (left_pos), make_fixnum (top_pos))),
	    Fcons (Qouter_size,
		   Fcons (make_fixnum (native_width),
			  make_fixnum (native_height))),
	    Fcons (Qexternal_border_size,
		   Fcons (make_fixnum (0), make_fixnum (0))),
	    Fcons (Qtitle_bar_size,
		   Fcons (make_fixnum (0), make_fixnum (0))),
	    Fcons (Qmenu_bar_external, Qnil),
	    Fcons (Qmenu_bar_size, Fcons (make_fixnum (0), make_fixnum (0))),
	    Fcons (Qtab_bar_size,
		   Fcons (make_fixnum (tab_bar_width),
			  make_fixnum (tab_bar_height))),
	    Fcons (Qtool_bar_external, Qnil),
	    Fcons (Qtool_bar_position, FRAME_TOOL_BAR_POSITION (f)),
	    Fcons (Qtool_bar_size,
		   Fcons (make_fixnum (tool_bar_width),
			  make_fixnum (tool_bar_height))),
	    Fcons (Qinternal_border_width,
		   make_fixnum (internal_border_width)));
}

DEFUN ("pgtk-frame-geometry", Fpgtk_frame_geometry, Spgtk_frame_geometry, 0, 1,
       0, doc: /* Return geometric attributes of FRAME.
FRAME must be a live frame and defaults to the selected one.  The return
value is an association list of the attributes of `frame-geometry'.  Under
Nidhogg's own Wayland shell a frame has no position of its own: its
origin is (0, 0), and every size is in device pixels.  */)
  (Lisp_Object frame)
{
  return frame_geometry (frame, Qnil);
}

DEFUN ("pgtk-frame-edges", Fpgtk_frame_edges, Spgtk_frame_edges, 0, 2, 0,
       doc: /* Return edge coordinates of FRAME.
FRAME must be a live frame and defaults to the selected one.  The return
value is a list of the form (LEFT, TOP, RIGHT, BOTTOM), in device pixels.
TYPE is as for `frame-edges'.  */)
  (Lisp_Object frame, Lisp_Object type)
{
  return frame_geometry (frame, ((EQ (type, Qouter_edges)
				  || EQ (type, Qinner_edges))
				 ? type : Qnative_edges));
}

DEFUN ("pgtk-mouse-absolute-pixel-position",
       Fpgtk_mouse_absolute_pixel_position,
       Spgtk_mouse_absolute_pixel_position, 0, 0, 0,
       doc: /* Return the absolute position of the mouse: nil under Wayland.  */)
  (void)
{
  return Qnil;
}

DEFUN ("pgtk-set-mouse-absolute-pixel-position",
       Fpgtk_set_mouse_absolute_pixel_position,
       Spgtk_set_mouse_absolute_pixel_position, 2, 2, 0,
       doc: /* Move the mouse: not possible under Wayland; return nil.  */)
  (Lisp_Object x, Lisp_Object y)
{
  CHECK_FIXNUM (x);
  CHECK_FIXNUM (y);
  return Qnil;
}

/***********************************************************************
				Tool tips
 ***********************************************************************/

/* A tip frame is an xdg popup of the frame under the pointer, anchored
   at the pointer (the compositor places it: gravity bottom-right, flip
   and slide keep it on the output).  A popup cannot change size, so the
   frame exists first without a window; the window is made once the text
   is measured.  */

/* The frame of the currently visible tooltip.  Adapted from pgtkfns.c.  */
static Lisp_Object tip_frame;

/* A timer that hides the currently visible tooltip when it fires.  */
static Lisp_Object tip_timer;

/* STRING, normalized FRAME and PARMS arguments of the last `x-show-tip'
   call.  */
static Lisp_Object tip_last_string, tip_last_frame, tip_last_parms;

/* Adapted from unwind_create_tip_frame (pgtkfns.c).  */
static void
unwind_create_tip_frame (Lisp_Object frame)
{
  if (EQ (unwind_create_frame (frame), Qt))
    tip_frame = Qnil;
}

/* Adapted from pgtk_create_tip_frame: a frame for a tooltip on DPYINFO
   with the frame parameters PARMS; it has no window yet.  Value is the
   frame.  */
static Lisp_Object
nhwl_create_tip_frame (struct nhwl_display_info *dpyinfo, Lisp_Object parms)
{
  struct frame *f;
  struct nhwl_output *out;
  Lisp_Object frame, name;
  specpdl_ref count = SPECPDL_INDEX ();
  bool face_change_before = face_change;

  if (!dpyinfo->terminal->name)
    error ("Terminal is not live, can't create new frames on it");

  parms = Fcopy_alist (parms);
  name = gui_display_get_arg (dpyinfo, parms, Qname, "name", "Name",
			      RES_TYPE_STRING);
  if (!STRINGP (name) && !BASE_EQ (name, Qunbound) && !NILP (name))
    error ("Invalid frame name--not a string or nil");

  f = make_frame (false);
  f->wants_modeline = false;
  XSETFRAME (frame, f);
  record_unwind_protect (unwind_create_tip_frame, frame);

  f->terminal = dpyinfo->terminal;
  f->output_method = output_nhwl;
  FRAME_X_OUTPUT (f) = xzalloc (sizeof *FRAME_X_OUTPUT (f));
  out = FRAME_NHWL_OUTPUT (f);
  out->frame = f;
  FRAME_FONTSET (f) = -1;
  out->white_relief.pixel = -1;
  out->black_relief.pixel = -1;

  f->tooltip = true;
  fset_icon_name (f, Qnil);
  FRAME_DISPLAY_INFO (f) = dpyinfo;

  /* The colors are set later; this gets their reference counts right.  */
  {
    Lisp_Object black = build_string ("black");

    FRAME_FOREGROUND_PIXEL (f) = -1;
    FRAME_BACKGROUND_PIXEL (f) = -1;
    out->cursor_color = -1;
    out->cursor_foreground_color = -1;
    out->border_pixel = -1;
    out->mouse_color = -1;
    FRAME_FOREGROUND_PIXEL (f)
      = x_decode_color (f, black, BLACK_PIX_DEFAULT (f));
    FRAME_BACKGROUND_PIXEL (f)
      = x_decode_color (f, black, BLACK_PIX_DEFAULT (f));
    out->cursor_color = x_decode_color (f, black, BLACK_PIX_DEFAULT (f));
    out->cursor_foreground_color
      = x_decode_color (f, black, BLACK_PIX_DEFAULT (f));
    out->border_pixel = x_decode_color (f, black, BLACK_PIX_DEFAULT (f));
    out->mouse_color = x_decode_color (f, black, BLACK_PIX_DEFAULT (f));
  }

  if (BASE_EQ (name, Qunbound) || NILP (name))
    {
      fset_name (f, build_string (dpyinfo->x_id_name));
      f->explicit_name = false;
    }
  else
    {
      fset_name (f, name);
      f->explicit_name = true;
      specbind (Qx_resource_name, name);
    }

  register_font_driver (&ftcrfont_driver, f);
#ifdef HAVE_HARFBUZZ
  register_font_driver (&ftcrhbfont_driver, f);
#endif	/* HAVE_HARFBUZZ */

  gui_default_parameter (f, parms, Qfont_backend, Qnil,
			 "fontBackend", "FontBackend", RES_TYPE_STRING);
  nhwl_default_font_parameter (f, parms);

  gui_default_parameter (f, parms, Qborder_width, make_fixnum (0),
			 "borderWidth", "BorderWidth", RES_TYPE_NUMBER);
  if (NILP (Fassq (Qinternal_border_width, parms)))
    {
      Lisp_Object value
	= gui_display_get_arg (dpyinfo, parms, Qinternal_border_width,
			       "internalBorder", "internalBorder",
			       RES_TYPE_NUMBER);

      if (!BASE_EQ (value, Qunbound))
	parms = Fcons (Fcons (Qinternal_border_width, value), parms);
    }
  gui_default_parameter (f, parms, Qinternal_border_width, make_fixnum (1),
			 "internalBorderWidth", "internalBorderWidth",
			 RES_TYPE_NUMBER);
  gui_default_parameter (f, parms, Qright_divider_width, make_fixnum (0),
			 NULL, NULL, RES_TYPE_NUMBER);
  gui_default_parameter (f, parms, Qbottom_divider_width, make_fixnum (0),
			 NULL, NULL, RES_TYPE_NUMBER);
  gui_default_parameter (f, parms, Qforeground_color, build_string ("black"),
			 "foreground", "Foreground", RES_TYPE_STRING);
  gui_default_parameter (f, parms, Qbackground_color, build_string ("white"),
			 "background", "Background", RES_TYPE_STRING);
  gui_default_parameter (f, parms, Qmouse_color, build_string ("black"),
			 "pointerColor", "Foreground", RES_TYPE_STRING);
  gui_default_parameter (f, parms, Qcursor_color, build_string ("black"),
			 "cursorColor", "Foreground", RES_TYPE_STRING);
  gui_default_parameter (f, parms, Qborder_color, build_string ("black"),
			 "borderColor", "BorderColor", RES_TYPE_STRING);
  gui_default_parameter (f, parms, Qno_special_glyphs, Qnil,
			 NULL, NULL, RES_TYPE_BOOLEAN);

  init_frame_faces (f);

  gui_default_parameter (f, parms, Qinhibit_double_buffering, Qnil,
			 "inhibitDoubleBuffering", "InhibitDoubleBuffering",
			 RES_TYPE_BOOLEAN);
  gui_figure_window_size (f, parms, false, false);

  out->current_cursor = out->text_cursor
    = nhwl_cursor_for_shape (NHG_CURSOR_TEXT);

  gui_default_parameter (f, parms, Qauto_raise, Qnil,
			 "autoRaise", "AutoRaiseLower", RES_TYPE_BOOLEAN);
  gui_default_parameter (f, parms, Qauto_lower, Qnil,
			 "autoLower", "AutoRaiseLower", RES_TYPE_BOOLEAN);
  gui_default_parameter (f, parms, Qcursor_type, Qbox,
			 "cursorType", "CursorType", RES_TYPE_SYMBOL);
  gui_default_parameter (f, parms, Qalpha, Qnil,
			 "alpha", "Alpha", RES_TYPE_NUMBER);
  gui_default_parameter (f, parms, Qalpha_background, Qnil,
			 "alphaBackground", "AlphaBackground", RES_TYPE_NUMBER);
  gui_default_parameter (f, parms, Qborders_respect_alpha_background, Qnil,
			 "bordersRespectAlphaBackground",
			 "BordersRespectAlphaBackground", RES_TYPE_NUMBER);

  if (NILP (Fframe_parameter (frame, Qtooltip)))
    {
      AUTO_FRAME_ARG (arg, Qtooltip, Qt);
      Fmodify_frame_parameters (frame, arg);
    }
  if (NILP (Fframe_parameter (frame, Qdisplay_type)))
    {
      AUTO_FRAME_ARG (arg, Qdisplay_type, intern_c_string ("color"));
      Fmodify_frame_parameters (frame, arg);
    }

  /* Face setup merges attributes that must not reach the internal border
     (see pgtk_create_tip_frame).  */
  {
    Lisp_Object bg = Fframe_parameter (frame, Qbackground_color);

    calln (Qface_set_after_frame_default, frame, Qnil);
    if (!EQ (bg, Fframe_parameter (frame, Qbackground_color)))
      {
	AUTO_FRAME_ARG (arg, Qbackground_color, bg);
	Fmodify_frame_parameters (frame, arg);
      }
  }

  f->no_split = true;

  /* The frame is official now.  */
  FRAME_DISPLAY_INFO (f)->reference_count++;
  f->terminal->reference_count++;
  Vframe_list = Fcons (frame, Vframe_list);
  f->can_set_window_size = true;
  adjust_frame_size (f, FRAME_TEXT_WIDTH (f), FRAME_TEXT_HEIGHT (f),
		     0, true, Qtip_frame);

  /* Faces set from resources would clear every matrix; nothing changes
     for the other frames.  */
  face_change = face_change_before;

  return unbind_to (count, frame);
}

/* Where a tip hanging on PF goes, in device pixels of PF's surface: the
   pointer, moved by *ADX and *ADY (DX and DY), or the `left' and `top' of
   PARMS (native pixels in PF), unmoved; the glue makes it PF's logical
   pixels.  Adapted from compute_tip_xy (pgtkfns.c, emacs-31.1); the
   compositor's flip and slide replace its fitting to the monitor.  */
static void
nhwl_tip_anchor (struct frame *pf, Lisp_Object parms, Lisp_Object dx,
		 Lisp_Object dy, NhgRect *anchor, int *adx, int *ady)
{
  struct nhwl_output *po = FRAME_NHWL_OUTPUT (pf);
  struct nhwl_display_info *dpyinfo = FRAME_DISPLAY_INFO (pf);
  Lisp_Object left = Fcdr (Fassq (Qleft, parms));
  Lisp_Object top = Fcdr (Fassq (Qtop, parms));
  int x = 0, y = 0;

  if (dpyinfo->last_mouse_motion_frame == pf)
    {
      x = dpyinfo->last_mouse_motion_x;
      y = dpyinfo->last_mouse_motion_y;
    }
  *adx = FIXNUMP (left) ? 0 : XFIXNUM (dx);
  *ady = FIXNUMP (top) ? 0 : XFIXNUM (dy);
  x = FIXNUMP (left) ? XFIXNUM (left) : x;
  y = FIXNUMP (top) ? XFIXNUM (top) : y;
  *anchor = (NhgRect) { po->csd.l + x, po->csd.t + y, 1, 1 };
}

/* Hide the tooltip: its frame is deleted, as pgtk_hide_tip does while
   `tooltip-reuse-hidden-frame' is nil.  Value is t when a tip was
   showing.  Adapted from pgtk_hide_tip.  */
static Lisp_Object
nhwl_hide_tip (void)
{
  Lisp_Object was_open = Qnil;
  specpdl_ref count = SPECPDL_INDEX ();

  if (!NILP (tip_timer))
    {
      calln (Qcancel_timer, tip_timer);
      tip_timer = Qnil;
    }
  if (!FRAMEP (tip_frame))
    return Qnil;

  specbind (Qinhibit_redisplay, Qt);
  specbind (Qinhibit_quit, Qt);
  if (FRAME_LIVE_P (XFRAME (tip_frame)))
    {
      /* A tip the compositor dismissed is closed already.  */
      was_open = FRAME_VISIBLE_P (XFRAME (tip_frame)) ? Qt : Qnil;
      delete_frame (tip_frame, Qnil);
    }
  tip_frame = Qnil;
  return unbind_to (count, was_open);
}

/* The compositor dismissed the popup of tip frame F (xdg_popup
   popup_done); the event loop calls this, so no Lisp runs: the window
   goes at once, the frame object when the tip's timer, the next
   x-hide-tip or the next x-show-tip deletes it.  New for nhwl.  */
void
nhwl_tip_dismissed (struct frame *f)
{
  struct nhwl_output *out = FRAME_NHWL_OUTPUT (f);
  struct nhwl_display_info *dpyinfo = FRAME_DISPLAY_INFO (f);

  if (out->window)
    nhwl_glue_rc (dpyinfo, NHG (nhg_tip_hide (dpyinfo->glue, out->window)));
  out->window = 0;
  SET_FRAME_VISIBLE (f, 0);
}

/* Make tip frame TIP_F's popup, WIDTH x HEIGHT device pixels, at ANCHOR
   moved by DX, DY (nhwl_tip_anchor) in the surface of PF: the glue makes
   and places it.  Takes the place of the end of Fx_show_tip (pgtkfns.c,
   emacs-31.1), where GTK resizes, moves and shows the widget.  */
static void
nhwl_tip_map (struct frame *tip_f, struct frame *pf, int width, int height,
	      NhgRect anchor, int dx, int dy)
{
  struct nhwl_output *out = FRAME_NHWL_OUTPUT (tip_f);
  struct nhwl_output *po = FRAME_NHWL_OUTPUT (pf);
  struct nhwl_display_info *dpyinfo = FRAME_DISPLAY_INFO (tip_f);
  uint64_t id = 0;
  int rc;

  out->scale120 = po->scale120 ? po->scale120 : 120;
  tip_f->pixel_width = width;
  tip_f->pixel_height = height;

  block_input ();
  rc = nhwl_glue_rc (dpyinfo,
		     NHG (nhg_tip_show (dpyinfo->glue, &id, po->window, anchor,
					dx, dy, max (1, width),
					max (1, height))));
  unblock_input ();
  if (rc < 0)
    error ("nidhogg-wayland: cannot create a tooltip window (%d)", rc);
  out->window = id;
  /* The tip's new window starts from its wheel units too.  */
  if (FRAME_COLUMN_WIDTH (tip_f) > 0 && FRAME_LINE_HEIGHT (tip_f) > 0)
    nhwl_glue_rc (dpyinfo, NHG (nhg_frame_units (dpyinfo->glue, id,
						 FRAME_COLUMN_WIDTH (tip_f),
						 FRAME_LINE_HEIGHT (tip_f))));
  out->next_output = dpyinfo->outputs;
  dpyinfo->outputs = out;

  SET_FRAME_VISIBLE (tip_f, 1);
  out->has_been_visible = true;
  nhwl_backing_resize (tip_f);
  nhwl_damage_all (tip_f);
}

/* The frame a tip of F hangs on: the frame under the pointer when there
   is one.  New for nhwl (pgtk's tip is transient for F).  */
static struct frame *
nhwl_tip_parent (struct frame *f)
{
  struct frame *p = FRAME_DISPLAY_INFO (f)->last_mouse_motion_frame;

  if (p && FRAME_LIVE_P (p) && FRAME_NHWL_P (p) && FRAME_VISIBLE_P (p)
      && !FRAME_TOOLTIP_P (p) && FRAME_NHWL_OUTPUT (p)->window)
    return p;
  return f;
}

/* Adapted from Fx_show_tip (pgtkfns.c), without GTK system tooltips and
   without reusing a hidden frame.  */
DEFUN ("x-show-tip", Fx_show_tip, Sx_show_tip, 1, 6, 0,
       doc: /* SKIP: real doc in xfns.c.  */)
  (Lisp_Object string, Lisp_Object frame, Lisp_Object parms,
   Lisp_Object timeout, Lisp_Object dx, Lisp_Object dy)
{
  struct frame *f, *pf, *tip_f;
  struct window *w;
  struct buffer *old_buffer;
  struct text_pos pos;
  int width, height, adx, ady;
  NhgRect anchor;
  int old_windows_or_buffers_changed = windows_or_buffers_changed;
  specpdl_ref count = SPECPDL_INDEX ();
  Lisp_Object window, size, tip_buf;
  AUTO_STRING (tip, " *tip*");

  specbind (Qinhibit_redisplay, Qt);

  CHECK_STRING (string);
  if (SCHARS (string) == 0)
    string = make_unibyte_string (" ", 1);

  if (NILP (frame))
    frame = selected_frame;
  f = decode_window_system_frame (frame);
  if (!FRAME_NHWL_P (f) || !FRAME_NHWL_OUTPUT (f)->window)
    return unbind_to (count, Qnil);

  if (NILP (timeout))
    timeout = Vx_show_tooltip_timeout;
  CHECK_FIXNAT (timeout);
  if (NILP (dx))
    dx = make_fixnum (5);
  else
    CHECK_FIXNUM (dx);
  if (NILP (dy))
    dy = make_fixnum (-10);
  else
    CHECK_FIXNUM (dy);

  pf = nhwl_tip_parent (f);
  nhwl_tip_anchor (pf, parms, dx, dy, &anchor, &adx, &ady);

  if (FRAMEP (tip_frame) && FRAME_LIVE_P (XFRAME (tip_frame))
      && FRAME_VISIBLE_P (XFRAME (tip_frame))
      && EQ (frame, tip_last_frame)
      && !NILP (Fequal_including_properties (tip_last_string, string))
      && !NILP (Fequal (tip_last_parms, parms)))
    {
      /* Only DX and DY have changed.  */
      struct frame *tf = XFRAME (tip_frame);
      uint64_t tw = FRAME_NHWL_OUTPUT (tf)->window;

      if (!NILP (tip_timer))
	{
	  calln (Qcancel_timer, tip_timer);
	  tip_timer = Qnil;
	}
      block_input ();
      /* The move is a bare commit; below xdg_wm_base 3 the popup is made
	 again, and the whole tip is drawn again.  */
      if (tw
	  && (nhwl_glue_rc (FRAME_DISPLAY_INFO (f),
			    NHG (nhg_tip_show (FRAME_NHG (f), &tw,
					       FRAME_NHWL_OUTPUT (pf)->window,
					       anchor, adx, ady, 0, 0)))
	      == 1))
	nhwl_damage_all (tf);
      unblock_input ();
      goto start_timer;
    }
  nhwl_hide_tip ();

  tip_last_frame = frame;
  tip_last_string = string;
  tip_last_parms = parms;

  /* Add default values to frame parameters.  */
  if (NILP (Fassq (Qname, parms)))
    parms = Fcons (Fcons (Qname, build_string ("tooltip")), parms);
  if (NILP (Fassq (Qinternal_border_width, parms)))
    parms = Fcons (Fcons (Qinternal_border_width, make_fixnum (3)), parms);
  if (NILP (Fassq (Qborder_width, parms)))
    parms = Fcons (Fcons (Qborder_width, make_fixnum (1)), parms);
  if (NILP (Fassq (Qborder_color, parms)))
    parms = Fcons (Fcons (Qborder_color, build_string ("lightyellow")), parms);
  if (NILP (Fassq (Qbackground_color, parms)))
    parms = Fcons (Fcons (Qbackground_color, build_string ("lightyellow")),
		   parms);

  if (NILP ((tip_frame = nhwl_create_tip_frame (FRAME_DISPLAY_INFO (f), parms))))
    return unbind_to (count, Qnil);

  tip_f = XFRAME (tip_frame);
  window = FRAME_ROOT_WINDOW (tip_f);
  tip_buf = Fget_buffer_create (tip, Qnil);
  /* A pseudo-window cannot have display margins.  */
  bset_left_margin_cols (XBUFFER (tip_buf), make_fixnum (0));
  bset_right_margin_cols (XBUFFER (tip_buf), make_fixnum (0));
  set_window_buffer (window, tip_buf, false, false);
  w = XWINDOW (window);
  w->pseudo_window_p = true;

  /* Set up the root window only far enough for the text size
     calculations.  */
  w->left_col = 0;
  w->top_line = 0;
  w->pixel_left = 0;
  w->pixel_top = 0;

  if (CONSP (Vx_max_tooltip_size)
      && RANGED_FIXNUMP (1, XCAR (Vx_max_tooltip_size), INT_MAX)
      && RANGED_FIXNUMP (1, XCDR (Vx_max_tooltip_size), INT_MAX))
    {
      w->total_cols = XFIXNAT (XCAR (Vx_max_tooltip_size));
      w->total_lines = XFIXNAT (XCDR (Vx_max_tooltip_size));
    }
  else
    {
      w->total_cols = 80;
      w->total_lines = 40;
    }

  w->pixel_width = w->total_cols * FRAME_COLUMN_WIDTH (tip_f);
  w->pixel_height = w->total_lines * FRAME_LINE_HEIGHT (tip_f);
  FRAME_TOTAL_COLS (tip_f) = w->total_cols;
  adjust_frame_glyphs (tip_f);

  /* Insert STRING into the root window's buffer and fit the frame to
     it.  */
  specpdl_ref count_1 = SPECPDL_INDEX ();
  old_buffer = current_buffer;
  set_buffer_internal_1 (XBUFFER (w->contents));
  bset_truncate_lines (current_buffer, Qnil);
  specbind (Qinhibit_read_only, Qt);
  specbind (Qinhibit_modification_hooks, Qt);
  specbind (Qinhibit_point_motion_hooks, Qt);
  Ferase_buffer ();
  Finsert (1, &string);
  clear_glyph_matrix (w->desired_matrix);
  clear_glyph_matrix (w->current_matrix);
  SET_TEXT_POS (pos, BEGV, BEGV_BYTE);
  try_window (window, pos, TRY_WINDOW_IGNORE_FONTS_CHANGE);

  size = Fwindow_text_pixel_size (window, Qnil, Qnil, Qnil,
				  make_fixnum (w->pixel_height), Qnil, Qnil);
  width = XFIXNUM (Fcar (size)) + 2 * FRAME_INTERNAL_BORDER_WIDTH (tip_f);
  height = XFIXNUM (Fcdr (size)) + 2 * FRAME_INTERNAL_BORDER_WIDTH (tip_f);
  width += FRAME_COLUMN_WIDTH (tip_f);

  /* Show the tip: its popup, then its text.  */
  nhwl_tip_map (tip_f, pf, width, height, anchor, adx, ady);
  w->must_be_updated_p = true;
  update_single_window (w);
  nhwl_publish (tip_f);
  flush_frame (tip_f);
  set_buffer_internal_1 (old_buffer);
  unbind_to (count_1, Qnil);
  windows_or_buffers_changed = old_windows_or_buffers_changed;

 start_timer:
  /* Let the tip disappear after TIMEOUT seconds.  */
  tip_timer = calln (Qrun_at_time, timeout, Qnil, Qx_hide_tip);

  return unbind_to (count, Qnil);
}

DEFUN ("x-hide-tip", Fx_hide_tip, Sx_hide_tip, 0, 0, 0,
       doc: /* SKIP: real doc in xfns.c.  */)
  (void)
{
  return nhwl_hide_tip ();
}

/* Popup frames for Emacs-drawn menus.

   A menu level is a frame whose `nhwl-popup' parameter, given at
   creation with `parent-frame', makes its window an xdg popup of the
   parent's window: (X Y W H . PLIST), the rectangle it sits beside in
   device pixels of the parent's native area, and PLIST's `:gravity'
   (the side or corner it grows toward, `bottom-right' by default),
   `:adjust' (how the compositor keeps it on the output: `flip-x',
   `flip-y', `slide-x', `slide-y'; all by default) and `:grab' (it takes
   the keyboard and pointer, and a click elsewhere dismisses it).

   A popup frame is the root of its own tree (its pacer and its acks are
   its own) and is published by nhwl_publish like any frame.  Its
   window exists while it is shown: made by `make-frame-visible' at the
   current parameter, destroyed by `make-frame-invisible', its popup
   frames first.  The window, its place and size, its grab, the focus a
   top-level lends it, and its dismissal (a `delete-frame' event) are the
   glue's (popup.rs); this decodes the parameter and does what Emacs
   sees.  The menus themselves are Lisp (nhwl-menu.el), reached from
   `x-popup-menu' through nhwl_popup_menu.  */

/* The `nhwl-popup' parameter decoded: what the glue's popup calls
   take.  */
struct nhwl_popup_parm
{
  NhgRect anchor;		/* device pixels of the parent's native area */
  uint32_t gravity, adjust;	/* NhgAnchor's gravity, NHG_ADJUST_* */
  uint32_t flags;		/* NHG_POPUP_GRAB */
};

bool
nhwl_popup_frame_p (struct frame *f)
{
  return (FRAME_NHWL_P (f) && f->output_data.nhwl
	  && f->output_data.nhwl->popup_frame);
}

/* The top-level frame popup or child frame F hangs from.  */
static struct frame *
popup_owner (struct frame *f)
{
  while (nhwl_popup_frame_p (f) || nhwl_child_p (f))
    f = FRAME_PARENT_FRAME (f);
  return f;
}

/* Read the value V of `nhwl-popup' into P (its anchor, gravity, adjust
   and grab): `nhwl-popup-decode' (nhwl-win.el) checks it, signals
   `error' for an invalid one, and returns (X Y W H GRAVITY ADJUST FLAGS),
   all integers.  */
static void
popup_parse (Lisp_Object v, struct nhwl_popup_parm *p)
{
  Lisp_Object r = calln (Qnhwl_popup_decode, v);
  int32_t n[7];

  for (int i = 0; i < 7; i++, r = XCDR (r))
    {
      CHECK_CONS (r);
      CHECK_FIXNUM (XCAR (r));
      n[i] = XFIXNUM (XCAR (r));
    }
  p->anchor.x = n[0];
  p->anchor.y = n[1];
  p->anchor.w = n[2];
  p->anchor.h = n[3];
  p->gravity = n[4];
  p->adjust = n[5];
  p->flags = n[6];
}

/* Frame F is being made (x-create-frame, with PARMS): a popup frame when
   PARMS has `nhwl-popup'.  The parameter goes into F's parameters at
   once (a show while x-create-frame runs reads it there), and is not
   left over for x-create-frame's end.  */
void
nhwl_popup_init (struct frame *f, Lisp_Object parms)
{
  Lisp_Object tem = Fassq (Qnhwl_popup, parms);
  struct nhwl_popup_parm p;

  if (NILP (tem) || NILP (XCDR (tem)))
    return;
  if (!FRAME_PARENT_FRAME (f))
    error ("nidhogg-wayland: a popup frame needs a `parent-frame'");
  popup_parse (XCDR (tem), &p);
  FRAME_NHWL_OUTPUT (f)->popup_frame = true;
  store_frame_param (f, Qnhwl_popup, XCDR (tem));
  XSETCAR (tem, Qnil);
}

/* Show popup frame F: a new popup of its parent's window, at the current
   parameter and F's size.  */
void
nhwl_popup_show (struct frame *f)
{
  struct nhwl_output *out = FRAME_NHWL_OUTPUT (f);
  struct nhwl_display_info *dpyinfo = FRAME_DISPLAY_INFO (f);
  struct frame *pf = FRAME_PARENT_FRAME (f);
  struct nhwl_output *po;
  struct nhwl_popup_parm p;
  uint64_t id = 0, owner;
  int w, h, rc;

  if (out->window || !dpyinfo->glue)
    return;
  if (!pf || !FRAME_LIVE_P (pf) || !FRAME_NHWL_P (pf)
      || !(po = FRAME_NHWL_OUTPUT (pf))->window)
    error ("nidhogg-wayland: the parent of a popup frame is not shown");
  popup_parse (get_frame_param (f, Qnhwl_popup), &p);
  owner = FRAME_NHWL_OUTPUT (popup_owner (f))->window;

  /* A popup starts at its parent's scale, and at the size the frame is
     about to take.  */
  out->scale120 = po->scale120 ? po->scale120 : 120;
  w = f->new_size_p ? f->new_width : FRAME_PIXEL_WIDTH (f);
  h = f->new_size_p ? f->new_height : FRAME_PIXEL_HEIGHT (f);

  block_input ();
  rc = nhwl_glue_rc (dpyinfo,
		     NHG (nhg_popup_show (dpyinfo->glue, &id, po->window, owner,
					  p.anchor, p.gravity, p.adjust,
					  p.flags, max (1, w), max (1, h))));
  if (rc >= 0 && FRAME_COLUMN_WIDTH (f) > 0 && FRAME_LINE_HEIGHT (f) > 0)
    nhwl_glue_rc (dpyinfo, NHG (nhg_frame_units (dpyinfo->glue, id,
						 FRAME_COLUMN_WIDTH (f),
						 FRAME_LINE_HEIGHT (f))));
  unblock_input ();
  if (rc < 0)
    error ("nidhogg-wayland: cannot create a popup window (%d)", rc);
  out->window = id;
  /* `alpha' goes to the new window.  */
  out->alpha_cut = 0;
  out->alpha_host = false;
  nhwl_set_frame_alpha (f);

  SET_FRAME_VISIBLE (f, 1);
  SET_FRAME_ICONIFIED (f, false);
  nhwl_window_visible (f, true);
  out->has_been_visible = true;
  nhwl_damage_all (f);
  SET_FRAME_GARBAGED (f);
}

/* Hide popup frame F: its window goes, after those of the popup frames
   hanging from it (the compositor wants the topmost popup destroyed
   first).  When F's top-level had lent its keyboard focus to a grabbing
   popup and has lost it now (the glue says so), Emacs sees the
   focus-out.  */
void
nhwl_popup_hide (struct frame *f)
{
  struct nhwl_output *out = FRAME_NHWL_OUTPUT (f);
  struct nhwl_display_info *dpyinfo = FRAME_DISPLAY_INFO (f);
  struct nhwl_output *o;
  struct frame *r = popup_owner (f);
  int rc = 0;

  for (o = dpyinfo->outputs; o; o = o->next_output)
    if (o->popup_frame && o->window && FRAME_PARENT_FRAME (o->frame) == f)
      nhwl_popup_hide (o->frame);
  if (dpyinfo->glue)
    {
      block_input ();
      rc = nhwl_glue_rc (dpyinfo,
			 NHG (nhg_popup_hide (dpyinfo->glue, out->window,
					      FRAME_NHWL_OUTPUT (r)->window)));
      unblock_input ();
    }
  out->window = 0;
  SET_FRAME_VISIBLE (f, 0);
  SET_FRAME_ICONIFIED (f, false);
  if (rc == 1)
    nhwl_focus_lost (r);
}

/* Popup frame F's size is now WIDTH x HEIGHT device pixels: the popup
   takes the new size, placed again by its anchor.  The whole frame is
   drawn again when the shell made the popup again (below xdg_popup 3).  */
void
nhwl_popup_set_size (struct frame *f, int width, int height)
{
  struct nhwl_output *out = FRAME_NHWL_OUTPUT (f);
  int rc;

  if (!nhwl_popup_frame_p (f) || !out->window)
    return;
  block_input ();
  rc = nhwl_glue_rc (FRAME_DISPLAY_INFO (f),
		     NHG (nhg_popup_size (FRAME_NHG (f), out->window, width,
					  height)));
  unblock_input ();
  if (rc == 1)
    nhwl_damage_all (f);
}

/* F's parameters changed: a new `nhwl-popup' moves the shown popup
   (xdg_popup.reposition).  Its size, gravity, adjust and grab take
   effect when it is shown next.  */
void
nhwl_popup_params_changed (struct frame *f)
{
  struct nhwl_output *out;
  struct nhwl_popup_parm p;
  Lisp_Object v;
  int rc;

  if (!nhwl_popup_frame_p (f) || NILP (v = get_frame_param (f, Qnhwl_popup)))
    return;
  popup_parse (v, &p);
  out = FRAME_NHWL_OUTPUT (f);
  if (!out->window)
    return;
  block_input ();
  rc = nhwl_glue_rc (FRAME_DISPLAY_INFO (f),
		     NHG (nhg_popup_place (FRAME_NHG (f), out->window,
					   p.anchor, p.gravity, p.adjust,
					   p.flags)));
  unblock_input ();
  if (rc == 1)
    nhwl_damage_all (f);
}

/* `x-popup-menu' on frame F: the Lisp menu (nhwl-menu.el) shows MENU at
   POSITION and returns the choice.  */
Lisp_Object
nhwl_popup_menu (struct frame *f, Lisp_Object position, Lisp_Object menu)
{
  if (NILP (Ffboundp (Qnhwl_menu_popup)))
    error ("No popup menus: nhwl-menu.el is not loaded");
  return calln (Qnhwl_menu_popup, position, menu);
}

/* The font settings the shared Lisp reads (xsettings.c does not exist
   here): none.  */
DEFUN ("font-get-system-font", Ffont_get_system_font, Sfont_get_system_font,
       0, 0, 0, doc: /* SKIP: real doc in xsettings.c.  */)
  (void)
{
  return Qnil;
}

DEFUN ("font-get-system-normal-font", Ffont_get_system_normal_font,
       Sfont_get_system_normal_font, 0, 0, 0,
       doc: /* SKIP: real doc in xsettings.c.  */)
  (void)
{
  return Qnil;
}

DEFUN ("tool-bar-get-system-style", Ftool_bar_get_system_style,
       Stool_bar_get_system_style, 0, 0, 0,
       doc: /* SKIP: real doc in xsettings.c.  */)
  (void)
{
  return Qnil;
}

DEFUN ("pgtk-use-im-context", Fpgtk_use_im_context, Spgtk_use_im_context,
       1, 1, 0,
       doc: /* Set whether to use an input method (text-input-v3); on by default.  */)
  (Lisp_Object use_p)
{
  nhwl_use_im_context (!NILP (use_p));
  return Qnil;
}

DEFUN ("nhwl-set-cursor-theme", Fnhwl_set_cursor_theme,
       Snhwl_set_cursor_theme, 2, 2, 0,
       doc: /* Use the Xcursor theme NAME at SIZE logical pixels for the pointer.
It matters where the compositor has no cursor-shape protocol: the shell
draws the pointer from the theme then.  A pointer already shown from the
theme is redrawn.  Return t when the shell took the theme.  */)
  (Lisp_Object name, Lisp_Object size)
{
  CHECK_STRING (name);
  CHECK_FIXNAT (size);
  if (!x_display_list)
    return Qnil;

  block_input ();
  int rc = NHG (nhg_set_cursor_theme (x_display_list->glue, SSDATA (name),
				      (uint32_t) min (XFIXNAT (size), 512)));
  unblock_input ();
  return rc >= 0 ? Qt : Qnil;
}

/* The window of FRAME for an interactive move, resize or menu request;
   NULL when the shell is gone.  */
static struct nhwl_output *
nhwl_interactive_target (Lisp_Object frame, struct frame **fp)
{
  struct frame *f = decode_window_system_frame (frame);
  *fp = f;
  return FRAME_NHWL_DISPLAY_INFO (f)->glue ? FRAME_NHWL_OUTPUT (f) : NULL;
}

DEFUN ("nhwl-frame-begin-move", Fnhwl_frame_begin_move,
       Snhwl_frame_begin_move, 0, 1, 0,
       doc: /* Ask the compositor to move FRAME with the pointer.
The latest button press triggers it, and the compositor owns the move
from then on.  Return t when the request was sent.  */)
  (Lisp_Object frame)
{
  struct frame *f;
  struct nhwl_output *out = nhwl_interactive_target (frame, &f);
  if (!out)
    return Qnil;
  block_input ();
  int rc = NHG (nhg_window_move (FRAME_NHWL_DISPLAY_INFO (f)->glue,
				 out->window));
  unblock_input ();
  return rc >= 0 ? Qt : Qnil;
}

DEFUN ("nhwl-frame-begin-resize", Fnhwl_frame_begin_resize,
       Snhwl_frame_begin_resize, 1, 2, 0,
       doc: /* Ask the compositor to resize FRAME from EDGE with the pointer.
EDGE is one of `top', `bottom', `left', `right', `top-left', `top-right',
`bottom-left' and `bottom-right'.  The latest button press triggers it.
Return t when the request was sent.  */)
  (Lisp_Object edge, Lisp_Object frame)
{
  static const struct { const char *name; uint32_t edges; } table[] = {
    { "top", NHG_EDGE_TOP },
    { "bottom", NHG_EDGE_BOTTOM },
    { "left", NHG_EDGE_LEFT },
    { "right", NHG_EDGE_RIGHT },
    { "top-left", NHG_EDGE_TOP | NHG_EDGE_LEFT },
    { "top-right", NHG_EDGE_TOP | NHG_EDGE_RIGHT },
    { "bottom-left", NHG_EDGE_BOTTOM | NHG_EDGE_LEFT },
    { "bottom-right", NHG_EDGE_BOTTOM | NHG_EDGE_RIGHT },
  };
  CHECK_SYMBOL (edge);
  uint32_t edges = 0;
  for (int i = 0; i < ARRAYELTS (table); i++)
    if (!strcmp (SSDATA (SYMBOL_NAME (edge)), table[i].name))
      edges = table[i].edges;
  if (!edges)
    signal_error ("Unknown frame edge", edge);

  struct frame *f;
  struct nhwl_output *out = nhwl_interactive_target (frame, &f);
  if (!out)
    return Qnil;
  block_input ();
  int rc = NHG (nhg_window_resize (FRAME_NHWL_DISPLAY_INFO (f)->glue,
				   out->window, edges));
  unblock_input ();
  return rc >= 0 ? Qt : Qnil;
}

DEFUN ("nhwl-frame-show-window-menu", Fnhwl_frame_show_window_menu,
       Snhwl_frame_show_window_menu, 0, 3, 0,
       doc: /* Ask the compositor to show FRAME's window menu at X, Y.
X and Y are pixels inside the frame (default 0).  The latest button
press triggers it.  Return t when the request was sent.  */)
  (Lisp_Object frame, Lisp_Object x, Lisp_Object y)
{
  struct frame *f;
  struct nhwl_output *out = nhwl_interactive_target (frame, &f);
  intmax_t px = NILP (x) ? 0 : check_integer_range (x, -INT_MAX, INT_MAX);
  intmax_t py = NILP (y) ? 0 : check_integer_range (y, -INT_MAX, INT_MAX);
  if (!out)
    return Qnil;
  /* Device pixels in, logical pixels out, once, at the call.  */
  double s = nhwl_frame_scale_factor (f);
  block_input ();
  int rc = NHG (nhg_window_show_menu (FRAME_NHWL_DISPLAY_INFO (f)->glue,
				      out->window, (int) round (px / s),
				      (int) round (py / s)));
  unblock_input ();
  return rc >= 0 ? Qt : Qnil;
}

/***********************************************************************
	 Idle inhibition, export handles and activation tokens
 ***********************************************************************/

/* The `inhibit-idle' parameter, read after every change of the frame's
   parameters (nhwl_child_params_changed): the glue holds or releases the
   window's idle inhibitor, and does nothing when it is so already.  */
static void
nhwl_idle_param_changed (struct frame *f)
{
  struct nhwl_output *out = FRAME_NHWL_OUTPUT (f);

  if (out && out->window && FRAME_NHG (f))
    nhwl_glue_rc (FRAME_DISPLAY_INFO (f),
		  NHG (nhg_inhibit_idle (FRAME_NHG (f), out->window,
					 !NILP (Fcdr (assq_no_quit
						      (Qinhibit_idle,
						       f->param_alist))))));
}

/* How long a handle or a token may take.  */
#define NHWL_HANDLE_TIMEOUT_MS 500

DEFUN ("nhwl-frame-export-handle", Fnhwl_frame_export_handle,
       Snhwl_frame_export_handle, 0, 1, 0,
       doc: /* Return FRAME's xdg-foreign handle as "wayland:HANDLE", or nil.
The handle names the top-level of FRAME's tree for other programs (a portal
parents its dialog to it).  It is nil without the exporter or when the
compositor does not answer in 500 ms.  FRAME defaults to the selected
frame.  */)
  (Lisp_Object frame)
{
  struct frame *f;
  struct nhwl_output *out = nhwl_interactive_target (frame, &f);
  char buf[128];
  size_t len = 0;
  int32_t rc;

  if (!out || !FRAME_NHG (f))
    return Qnil;
  block_input ();
  rc = nhwl_glue_rc (FRAME_DISPLAY_INFO (f),
		     NHG (nhg_window_export (FRAME_NHG (f), out->window,
					     NHWL_HANDLE_TIMEOUT_MS, buf,
					     sizeof buf, &len)));
  unblock_input ();
  return rc >= 0 ? concat2 (build_string ("wayland:"),
			    make_string (buf, len)) : Qnil;
}

DEFUN ("nhwl-activation-token", Fnhwl_activation_token,
       Snhwl_activation_token, 0, 1, 0,
       doc: /* Return a fresh xdg-activation token made on FRAME, or nil.
The token carries the newest input serial, so it is nil before any input,
without the protocol, or when the compositor does not answer in 500 ms.
FRAME defaults to the selected frame.  */)
  (Lisp_Object frame)
{
  struct frame *f;
  struct nhwl_output *out = nhwl_interactive_target (frame, &f);
  char buf[512];
  size_t len = 0;
  int32_t rc;

  if (!out || !FRAME_NHG (f))
    return Qnil;
  block_input ();
  rc = nhwl_glue_rc (FRAME_DISPLAY_INFO (f),
		     NHG (nhg_activation_token (FRAME_NHG (f), out->window,
						NHWL_HANDLE_TIMEOUT_MS, buf,
						sizeof buf, &len)));
  unblock_input ();
  return rc >= 0 ? make_string (buf, len) : Qnil;
}

DEFUN ("nhwl-system-bell-p", Fnhwl_system_bell_p, Snhwl_system_bell_p, 0, 1, 0,
       doc: /* Return non-nil if the compositor offers a system bell.
That is the xdg_system_bell_v1 protocol on DISPLAY (a display name or a
frame; the selected frame's by default).  Without it `ding' does not sound
on a shell frame, whatever `visible-bell' says.  */)
  (Lisp_Object display)
{
  struct nhwl_display_info *dpyinfo = check_nhwl_display_info (display);
  NhgCaps caps;
  int32_t rc;

  if (!dpyinfo->glue)
    return Qnil;
  block_input ();
  rc = nhwl_glue_rc (dpyinfo, NHG (nhg_caps (dpyinfo->glue, &caps)));
  unblock_input ();
  return rc >= 0 && caps.system_bell ? Qt : Qnil;
}

/***********************************************************************
			   Session restore
 ***********************************************************************/

DEFUN ("nhwl--session-open", Fnhwl__session_open, Snhwl__session_open, 3, 3, 0,
       doc: /* Open the compositor's session for FRAME's display.
ID is the saved session's id (a string), or nil to ask for a new session.
REASON is `launch', `recover' or `restore'.  Return t when the request went
out, nil without session management.  The answer arrives as an
`nhwl-session' special event.  Frames made after this with a
`session-name' join the session before their first commit.  */)
  (Lisp_Object frame, Lisp_Object id, Lisp_Object reason)
{
  struct frame *f = decode_window_system_frame (frame);
  uint32_t r;
  int32_t rc;

  if (!FRAME_NHG (f))
    return Qnil;
  if (EQ (reason, Qnhwl_session_launch))
    r = NHG_SESSION_LAUNCH;
  else if (EQ (reason, Qnhwl_session_recover))
    r = NHG_SESSION_RECOVER;
  else if (EQ (reason, Qnhwl_session_restore))
    r = NHG_SESSION_RESTORE;
  else
    signal_error ("Unknown session reason", reason);
  if (!NILP (id))
    CHECK_STRING (id);
  if (STRINGP (id))
    id = ENCODE_UTF_8 (id);
  block_input ();
  rc = nhwl_glue_rc (FRAME_DISPLAY_INFO (f),
		     NHG (nhg_session_open (FRAME_NHG (f),
					    STRINGP (id) ? SSDATA (id) : NULL,
					    STRINGP (id) ? SBYTES (id) : 0,
					    r)));
  unblock_input ();
  return rc >= 0 ? Qt : Qnil;
}

DEFUN ("nhwl--session-remove", Fnhwl__session_remove, Snhwl__session_remove,
       1, 1, 0,
       doc: /* Take FRAME out of the compositor's session.
The compositor forgets the frame's name.  Return t when the frame was in
the session.  */)
  (Lisp_Object frame)
{
  struct frame *f = decode_window_system_frame (frame);
  struct nhwl_output *out = FRAME_NHWL_OUTPUT (f);
  int32_t rc;

  if (!out || !out->window || !FRAME_NHG (f))
    return Qnil;
  block_input ();
  rc = nhwl_glue_rc (FRAME_DISPLAY_INFO (f),
		     NHG (nhg_session_remove (FRAME_NHG (f), out->window)));
  unblock_input ();
  return rc > 0 ? Qt : Qnil;
}

/***********************************************************************
			The title bars' style
 ***********************************************************************/

/* Moved from nhwlcsd.c.  The title bar's layout and buttons are the
   glue's (csd.rs); its colours and font stay here, as Lisp gave them, for
   the painting (nhwl_csd_draw).  */

#define NHWL_CSD_TITLE_PX 13	/* logical px */
#define NHWL_CSD_MAX_SIDE 8	/* buttons kept per side (NHG_CSD_MAX_BUTTONS) */

/* The style from Lisp (nhwl--set-title-bar-style): each side's buttons
   (NHG_CSD_*), kept to hand them to a display opened later; the colours of
   an active and an inactive window (strings, or nil for the frame's own),
   and the font (FAMILY . PIXELS); STYLE_GEN counts the changes.  Before
   Lisp says, the buttons are minimize, maximize and close on the right.  */
static uint32_t csd_left[NHWL_CSD_MAX_SIDE], csd_right[NHWL_CSD_MAX_SIDE];
static size_t csd_nleft, csd_nright;
static bool csd_styled;
static Lisp_Object csd_colours, csd_font;
static unsigned csd_style_gen;

/* The first NHWL_CSD_MAX_SIDE button kinds (NHG_CSD_*) of LIST into V,
   other elements skipped; how many.  */
static size_t
nhwl_csd_kinds (Lisp_Object list, uint32_t *v)
{
  size_t n = 0;

  for (; CONSP (list) && n < NHWL_CSD_MAX_SIDE; list = XCDR (list))
    if (RANGED_FIXNUMP (NHG_CSD_CLOSE, XCAR (list), NHG_CSD_MENU))
      v[n++] = XFIXNUM (XCAR (list));
  return n;
}

/* Hand the title bars' style to DPYINFO's glue (once Lisp gave one).  */
void
nhwl_csd_push_style (struct nhwl_display_info *dpyinfo)
{
  if (csd_styled && dpyinfo->glue)
    nhwl_glue_rc (dpyinfo, NHG (nhg_csd_style (dpyinfo->glue, csd_left,
					       csd_nleft, csd_right,
					       csd_nright)));
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
   inactive window, else the glue's mix of the frame's colours.  */
void
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
      /* The default: the background 12% toward the foreground, and an inactive
	 window's own foreground toward that (the glue's mix).  */
      uint32_t mfg = 0, mbg = 0;

      NHG (nhg_csd_mix (ffg & 0xffffff, fbg & 0xffffff, &mfg, &mbg));
      *bg = mbg;
      if (own_fg && !active)
	*fg = mfg;
    }
}

/* The title's font: *FAMILY (sans-serif when the style names none) and
   *PX its size in logical pixels (13 when it names none).  Return the
   style's generation.  */
unsigned
nhwl_csd_font (const char **family, double *px)
{
  Lisp_Object fam = CONSP (csd_font) ? XCAR (csd_font) : Qnil;
  Lisp_Object size = CONSP (csd_font) ? XCDR (csd_font) : Qnil;

  *family = STRINGP (fam) ? SSDATA (fam) : "sans-serif";
  *px = (NUMBERP (size) && XFLOATINT (size) > 0
	 ? XFLOATINT (size) : NHWL_CSD_TITLE_PX);
  return csd_style_gen;
}

DEFUN ("nhwl--set-title-bar-style", Fnhwl__set_title_bar_style,
       Snhwl__set_title_bar_style, 7, 7, 0,
       doc: /* Give the shell's title bars their style.
LEFT and RIGHT are the buttons of each side in their order, lists of
numbers: 1 close, 2 maximize, 3 minimize and 4 appmenu (other elements
are ignored).  ACTIVE-FG and ACTIVE-BG are the colours of an active window's
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
  struct nhwl_display_info *dpyinfo;
  Lisp_Object tail, frame;

  CHECK_LIST (left);
  CHECK_LIST (right);
  csd_nleft = nhwl_csd_kinds (left, csd_left);
  csd_nright = nhwl_csd_kinds (right, csd_right);
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
  for (dpyinfo = x_display_list; dpyinfo; dpyinfo = dpyinfo->next)
    nhwl_csd_push_style (dpyinfo);
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
  struct nhwl_display_info *dpyinfo;
  struct nhwl_output *out;
  NhgCsdPaint p;
  int32_t off[4];
  Lisp_Object buttons = Qnil;
  static const char *const names[] =
    { "", "close", "maximize", "minimize", "appmenu" };

  if (!FRAME_NHWL_P (f) || !FRAME_NHWL_OUTPUT (f))
    return Qnil;
  out = FRAME_NHWL_OUTPUT (f);
  dpyinfo = FRAME_DISPLAY_INFO (f);
  if (!out->window
      || (nhwl_glue_rc (dpyinfo,
			NHG (nhg_csd_title_bar (dpyinfo->glue, out->window,
						FRAME_PIXEL_WIDTH (f),
						FRAME_PIXEL_HEIGHT (f), &p)))
	  != 1)
      || (nhwl_glue_rc (dpyinfo,
			NHG (nhg_frame_offsets (dpyinfo->glue, out->window,
						off)))
	  < 0))
    return Qnil;
  for (int i = (int) min (p.nbuttons, NHG_CSD_MAX_BUTTONS) - 1; i >= 0; i--)
    if (p.button[i].kind < ARRAYELTS (names))
      buttons = Fcons (list5 (intern (names[p.button[i].kind]),
			      make_fixnum (p.button[i].r.x),
			      make_fixnum (p.button[i].r.y),
			      make_fixnum (p.button[i].r.w),
			      make_fixnum (p.button[i].r.h)),
		       buttons);
  return CALLN (Flist, QCstrip, list4 (make_fixnum (p.strip.x),
				       make_fixnum (p.strip.y),
				       make_fixnum (p.strip.w),
				       make_fixnum (p.strip.h)),
		QCmargins, list4 (make_fixnum (off[0]), make_fixnum (p.strip.y),
				  make_fixnum (off[2]), make_fixnum (off[3])),
		QCbuttons, buttons);
}

/***********************************************************************
			 Selection primitives
 ***********************************************************************/

/* The five selection primitives of pgtk-win.el on top of the shell's
   selection calls.  Text is converted when a selection is owned (the
   shell's reader thread answers other clients' pastes from the copy);
   other targets are not offered.  Reads wait at most
   `nhwl-selection-timeout' seconds.  */

/* The MIME types we offer for text, in the order of preference.  */
static const char *const nhwl_text_mimes[] = {
  "text/plain;charset=utf-8", "UTF8_STRING", "text/plain", "STRING", "TEXT",
};
#define N_TEXT_MIMES ARRAYELTS (nhwl_text_mimes)

/* The shell's selection kind for SEL, or -1 when there is none.  NIL
   means PRIMARY, as in pgtk.  */
static int
nhwl_selection_kind (Lisp_Object sel)
{
  if (NILP (sel) || EQ (sel, QPRIMARY))
    return NHG_SEL_PRIMARY;
  if (EQ (sel, QCLIPBOARD))
    return NHG_SEL_CLIPBOARD;
  /* The offer of the drag over our windows (nhwl_dnd_event_arg).  */
  if (EQ (sel, QXdndSelection))
    return NHG_SEL_DND;
  return -1;
}

static bool
nhwl_text_target_p (Lisp_Object target)
{
  return (EQ (target, QUTF8_STRING) || EQ (target, QSTRING)
	  || EQ (target, QTEXT) || EQ (target, QCOMPOUND_TEXT)
	  || EQ (target, Qtext_plain) || EQ (target, Qtext_plain_utf8));
}

/* The current MIME types of KIND as a Lisp list of strings.  */
static Lisp_Object
nhwl_offer_types (struct nhwl_display_info *dpyinfo, int kind)
{
  const char *buf = NULL;
  size_t got = 0;
  Lisp_Object list = Qnil;

  if (nhwl_glue_rc (dpyinfo, NHG (nhg_selection_types (dpyinfo->glue, kind,
						       &buf, &got))) <= 0)
    return Qnil;
  /* BUF is the glue's, valid until its next call; the list is built
     before any.  */
  for (size_t i = 0; i < got; i += strlen (buf + i) + 1)
    list = Fcons (build_string (buf + i), list);
  return Fnreverse (list);
}

DEFUN ("pgtk-own-selection-internal", Fpgtk_own_selection_internal,
       Spgtk_own_selection_internal, 2, 3, 0,
       doc: /* Assert an X selection of type SELECTION and value VALUE.
Only strings are offered, as text.  Return VALUE, or nil when the
compositor refuses (no keyboard focus).  The value of `XdndSelection'
is kept for `x-begin-drag', which converts it.  */)
  (Lisp_Object selection, Lisp_Object value, Lisp_Object frame)
{
  if (EQ (selection, QXdndSelection))
    return Vnhwl_xdnd_value = value;
  int kind = nhwl_selection_kind (selection);
  if (kind < 0 || !x_display_list || !STRINGP (value))
    return Qnil;

  Lisp_Object bytes = code_convert_string_norecord (value, Qutf_8, true);
  const uint8_t *data[N_TEXT_MIMES];
  size_t lens[N_TEXT_MIMES];
  for (int i = 0; i < N_TEXT_MIMES; i++)
    {
      data[i] = (const uint8_t *) SDATA (bytes);
      lens[i] = SBYTES (bytes);
    }
  block_input ();
  int rc = nhwl_glue_rc (x_display_list,
			 NHG (nhg_selection_set (x_display_list->glue, kind,
						 nhwl_text_mimes, data, lens,
						 N_TEXT_MIMES)));
  unblock_input ();
  if (rc < 0)
    return Qnil;
  return value;
}

DEFUN ("pgtk-disown-selection-internal", Fpgtk_disown_selection_internal,
       Spgtk_disown_selection_internal, 1, 2, 0,
       doc: /* If we own the selection SELECTION, disown it.  */)
  (Lisp_Object selection, Lisp_Object terminal)
{
  if (EQ (selection, QXdndSelection))
    {
      Lisp_Object owned = NILP (Vnhwl_xdnd_value) ? Qnil : Qt;
      Vnhwl_xdnd_value = Qnil;
      return owned;
    }
  int kind = nhwl_selection_kind (selection);
  if (kind < 0 || !x_display_list)
    return Qnil;
  block_input ();
  int rc = nhwl_glue_rc (x_display_list,
			 NHG (nhg_selection_disown (x_display_list->glue,
						    kind)));
  unblock_input ();
  return rc > 0 ? Qt : Qnil;
}

DEFUN ("pgtk-selection-owner-p", Fpgtk_selection_owner_p,
       Spgtk_selection_owner_p, 0, 2, 0,
       doc: /* Whether the current Emacs process owns the selection SELECTION.  */)
  (Lisp_Object selection, Lisp_Object terminal)
{
  if (EQ (selection, QXdndSelection))
    return NILP (Vnhwl_xdnd_value) ? Qnil : Qt;
  int kind = nhwl_selection_kind (selection);
  if (kind < 0 || !x_display_list)
    return Qnil;
  return nhwl_glue_rc (x_display_list,
		       NHG (nhg_selection_owned (x_display_list->glue, kind)))
	 > 0 ? Qt : Qnil;
}

DEFUN ("pgtk-selection-exists-p", Fpgtk_selection_exists_p,
       Spgtk_selection_exists_p, 0, 2, 0,
       doc: /* Whether there is an owner for the given selection.  */)
  (Lisp_Object selection, Lisp_Object terminal)
{
  int kind = nhwl_selection_kind (selection);
  if (kind < 0 || !x_display_list)
    return Qnil;
  return nhwl_glue_rc (x_display_list,
		       NHG (nhg_selection_types (x_display_list->glue, kind,
						 NULL, NULL)))
	 > 0 ? Qt : Qnil;
}

DEFUN ("pgtk-get-selection-internal", Fpgtk_get_selection_internal,
       Spgtk_get_selection_internal, 2, 3, 0,
       doc: /* Return text selected from some X window.
SELECTION-SYMBOL is CLIPBOARD, PRIMARY or SECONDARY (nil), TARGET-TYPE the
target: TARGETS gives a vector of the offered types, a text target gives
the text, any other the bytes of the type of that name.  Waits at most
`nhwl-selection-timeout' seconds; nil when nothing arrives.  */)
  (Lisp_Object selection_symbol, Lisp_Object target_type,
   Lisp_Object terminal)
{
  int kind = nhwl_selection_kind (selection_symbol);
  if (kind < 0 || !x_display_list)
    return Qnil;
  CHECK_SYMBOL (target_type);

  Lisp_Object types = nhwl_offer_types (x_display_list, kind);
  if (NILP (types))
    return Qnil;

  if (EQ (target_type, QTARGETS))
    {
      ptrdiff_t n = list_length (types), i = 0;
      Lisp_Object vec = make_nil_vector (n);
      for (; CONSP (types); types = XCDR (types), i++)
	ASET (vec, i, Fintern (XCAR (types), Qnil));
      return vec;
    }

  /* The type: the target's name when offered, else for a text target the
     first text type offered (the glue picks from the types just read).  */
  bool text = nhwl_text_target_p (target_type);
  Lisp_Object name = SYMBOL_NAME (target_type);
  const char *mime = NULL;
  if (nhwl_glue_rc (x_display_list,
		    NHG (nhg_selection_pick (x_display_list->glue, SSDATA (name),
					     SBYTES (name),
					     text ? nhwl_text_mimes : NULL,
					     text ? N_TEXT_MIMES : 0, &mime)))
      <= 0)
    return Qnil;

  double secs = 1.0;
  if (!NILP (Fboundp (Qnhwl_selection_timeout))
      && NUMBERP (Fsymbol_value (Qnhwl_selection_timeout)))
    secs = XFLOATINT (Fsymbol_value (Qnhwl_selection_timeout));
  uint32_t ms = secs <= 0 ? 0 : secs > 60 ? 60000 : (uint32_t) (secs * 1000);

  const uint8_t *out = NULL;
  size_t len = 0;
  int rc = nhwl_glue_rc (x_display_list,
			 NHG (nhg_selection_read (x_display_list->glue, kind,
						  mime, ms, &out,
						  &len)));
  if (rc < 0)
    return Qnil;

  /* OUT is the glue's, valid until its next call; the string copies it.  */
  Lisp_Object str = make_unibyte_string ((const char *) out, len);
  if (text)
    return code_convert_string_norecord (str, Qutf_8, false);
  /* Raw bytes: gui-get-selection decodes them from the property.  */
  Fput_text_property (make_fixnum (0), make_fixnum (len),
		      intern ("foreign-selection"), target_type, str);
  return str;
}

/* The glue says another client took selection KIND from us (a
   SELECTION_CLEAR action): fill IE with the event.  */
void
nhwl_selection_clear_event (uint32_t kind, struct input_event *ie)
{
  ie->kind = SELECTION_CLEAR_EVENT;
  ie->arg = kind == NHG_SEL_PRIMARY ? QPRIMARY : QCLIPBOARD;
}

/* Called by the command loop for a SELECTION_CLEAR_EVENT.  */
void
nhwl_handle_selection_clear (struct input_event *ie)
{
  CALLN (Frun_hook_with_args, Qpgtk_lost_selection_functions, ie->arg);
  redisplay_preserve_echo_area (20);
}

/* Drag and drop onto our windows, with pgtk-dnd.el's semantics.  The glue
   (dnd.rs) reports the drag; nhwl_apply makes a DRAG_N_DROP_EVENT of each
   action, and pgtk-dnd.el answers with the two functions below.  */

/* The action symbol of the glue's action code CODE (NHG_DND_*).  */
static Lisp_Object
nhwl_dnd_action_symbol (int code)
{
  switch (code)
    {
    case NHG_DND_COPY:
      return Qcopy;
    case NHG_DND_MOVE:
      return Qmove;
    case NHG_DND_ASK:
      return Qnhwl_dnd_ask;
    default:
      return Qnil;
    }
}

/* The glue's action code of the Lisp action ACT.  Wayland has no link or
   private action: they are copies.  */
static uint32_t
nhwl_dnd_action_code (Lisp_Object act)
{
  if (NILP (act))
    return 0;
  if (EQ (act, Qcopy) || EQ (act, Qlink)
      || EQ (act, Qprivate))
    return NHG_DND_COPY;
  if (EQ (act, Qmove))
    return NHG_DND_MOVE;
  if (EQ (act, Qnhwl_dnd_ask))
    return NHG_DND_ASK;
  signal_error ("Invalid drag action", act);
}

/* The argument of the DRAG_N_DROP_EVENT for the glue's DND action A: motion
   (and the enter) (lambda XdndSelection TIME ACTION), drop
   (quote XdndSelection TIME), leave nil.  */
Lisp_Object
nhwl_dnd_event_arg (const NhgAction *a)
{
  Lisp_Object time = make_uint (a->time_ms);

  switch (a->kind)
    {
    case NHG_ACT_DND_ENTER:
    case NHG_ACT_DND_MOTION:
      return list4 (Qlambda, QXdndSelection, time,
		    nhwl_dnd_action_symbol (a->code));
    case NHG_ACT_DND_DROP:
      return list3 (Qquote, QXdndSelection, time);
    default:
      return Qnil;
    }
}

DEFUN ("pgtk-register-dnd-targets", Fpgtk_register_dnd_targets,
       Spgtk_register_dnd_targets, 2, 2, 0,
       doc: /* Register TARGETS on FRAME.
TARGETS should be a list of strings describing data types (selection
targets) that can be dropped on top of FRAME.  A drag that offers none
of them is refused.  */)
  (Lisp_Object frame, Lisp_Object targets)
{
  struct frame *f = decode_window_system_frame (frame);
  struct nhwl_output *out = FRAME_NHWL_OUTPUT (f);
  ptrdiff_t n = 0;
  USE_SAFE_ALLOCA;

  CHECK_LIST (targets);
  ptrdiff_t length = list_length (targets);
  const char **mimes;
  SAFE_NALLOCA (mimes, 1, length);
  Lisp_Object tem = targets;
  FOR_EACH_TAIL (tem)
    {
      CHECK_STRING (XCAR (tem));
      SAFE_ALLOCA_STRING (mimes[n], XCAR (tem));
      n++;
    }
  CHECK_LIST_END (tem, targets);

  if (out && out->window && FRAME_NHG (f))
    nhwl_glue_rc (FRAME_DISPLAY_INFO (f),
		  NHG (nhg_dnd_register (FRAME_NHG (f), out->window, mimes,
					 n)));
  SAFE_FREE ();
  return Qnil;
}

DEFUN ("pgtk-drop-finish", Fpgtk_drop_finish, Spgtk_drop_finish, 3, 3, 0,
       doc: /* Finish the drag-n-drop event that happened at TIMESTAMP.
SUCCESS is whether or not the drop was successful, i.e. the action
chosen in the last call to `pgtk-update-drop-status' was performed.
TIMESTAMP is the time associated with the drag-n-drop event that is
being finished.  A call with an older TIMESTAMP than the current drag's
is ignored.  The source is told only when an action was set.  DELETE is
ignored: a move is the source's job on Wayland.  */)
  (Lisp_Object success, Lisp_Object timestamp, Lisp_Object delete)
{
  uint32_t time;

  if (!x_display_list)
    return Qnil;
  CONS_TO_INTEGER (timestamp, uint32_t, time);
  nhwl_glue_rc (x_display_list,
		NHG (nhg_dnd_finish (x_display_list->glue, !NILP (success),
				     time)));
  return Qnil;
}

DEFUN ("pgtk-update-drop-status", Fpgtk_update_drop_status,
       Spgtk_update_drop_status, 2, 2, 0,
       doc: /* Update the status of the current drag-and-drop operation.
ACTION is the action the drop source should take: copy, move, ask, or
nil to accept nothing.  TIMESTAMP is the same as in `pgtk-drop-finish'.  */)
  (Lisp_Object action, Lisp_Object timestamp)
{
  uint32_t time;

  if (!x_display_list)
    return Qnil;
  CONS_TO_INTEGER (timestamp, uint32_t, time);
  nhwl_glue_rc (x_display_list,
		NHG (nhg_dnd_status (x_display_list->glue,
				     nhwl_dnd_action_code (action), time)));
  return Qnil;
}

/* Dragging out of our frames (x-begin-drag), with X's arguments and
   results.  The XdndSelection value is converted for every target when
   the drag starts; the glue (dnd.rs) runs the drag, and nhwl_drag_run
   (nhwlterm.c) waits for its end.  */

/* `nhwl-dnd-finish-timeout' in seconds, from 0 to 60.  */
static double
nhwl_dnd_finish_secs (void)
{
  double secs = (NUMBERP (Vnhwl_dnd_finish_timeout)
		 ? XFLOATINT (Vnhwl_dnd_finish_timeout) : 5.0);
  return secs < 0 ? 0 : secs > 60 ? 60 : secs;
}

DEFUN ("nhwl--drag-start", Fnhwl__drag_start, Snhwl__drag_start, 6, 6, 0,
       doc: /* Drag the items ITEMS-FN returns out of FRAME; internal.
Called by `x-begin-drag' (lisp/term/nhwl-dnd.el).  ITEMS-FN is called
with no arguments once the drag may start (FRAME is shown, the glue is
up and Lisp owns `XdndSelection') and returns a list of (MIME . BYTES),
BYTES unibyte.  OFFERED is the bit set of the actions offered and
PREFERRED the one our own frames prefer (1 copy, 2 move, 4 ask).
ALLOW-CURRENT-FRAME is as for `x-begin-drag', and RETURN-FRAME is nil, t
or `now'.  Block until the drag ends.  Return nil when it did not start,
a frame when the call is to return it, else the action performed (the
bit, 0: none).  */)
  (Lisp_Object frame, Lisp_Object items_fn, Lisp_Object offered_arg,
   Lisp_Object preferred_arg, Lisp_Object allow_current_frame,
   Lisp_Object return_frame)
{
  struct frame *f = decode_window_system_frame (frame);
  struct nhwl_output *out = FRAME_NHWL_OUTPUT (f);
  uint32_t offered, preferred, flags = 0;
  struct frame *to = NULL;
  Lisp_Object items, tail;
  ptrdiff_t n, i = 0;
  int rc, done;
  USE_SAFE_ALLOCA;

  CHECK_FIXNAT (offered_arg);
  CHECK_FIXNAT (preferred_arg);
  offered = XFIXNAT (offered_arg);
  preferred = XFIXNAT (preferred_arg);
  if (!NILP (allow_current_frame))
    flags |= NHG_DRAG_ALLOW_CURRENT;
  if (EQ (return_frame, Qnow))
    flags |= NHG_DRAG_RETURN_NOW;
  else if (!NILP (return_frame))
    flags |= NHG_DRAG_RETURN_FRAME;

  if (!out || !out->window || !FRAME_NHG (f) || NILP (Vnhwl_xdnd_value))
    return Qnil;
  items = calln (items_fn);
  CHECK_LIST (items);
  n = list_length (items);
  if (n == 0)
    return Qnil;

  const char **mimes;
  const uint8_t **data;
  size_t *lens;
  SAFE_NALLOCA (mimes, 1, n);
  SAFE_NALLOCA (data, 1, n);
  SAFE_NALLOCA (lens, 1, n);
  for (tail = items; CONSP (tail); tail = XCDR (tail), i++)
    {
      CHECK_CONS (XCAR (tail));
      CHECK_STRING (XCAR (XCAR (tail)));
      CHECK_STRING (XCDR (XCAR (tail)));
      mimes[i] = SSDATA (XCAR (XCAR (tail)));
      data[i] = SDATA (XCDR (XCAR (tail)));
      lens[i] = SBYTES (XCDR (XCAR (tail)));
    }
  block_input ();
  rc = nhwl_glue_rc (FRAME_DISPLAY_INFO (f),
		     NHG (nhg_drag_start (FRAME_NHG (f), out->window, mimes,
					  data, lens, n, offered, preferred,
					  flags)));
  unblock_input ();
  SAFE_FREE ();
  /* NHG_E_STATE: no button is held, and X returns nil at once too.  */
  if (rc < 0)
    return Qnil;

  done = nhwl_drag_run (FRAME_DISPLAY_INFO (f), nhwl_dnd_finish_secs (), &to);
  if (to)
    {
      Lisp_Object value;

      XSETFRAME (value, to);
      return value;
    }
  return make_fixnum (done);
}
void
syms_of_nhwlfns (void)
{
  DEFVAR_LISP ("x-cursor-fore-pixel", Vx_cursor_fore_pixel,
    doc: /* A string indicating the foreground color of the cursor box.  */);
  Vx_cursor_fore_pixel = Qnil;

  DEFVAR_LISP ("x-max-tooltip-size", Vx_max_tooltip_size,
    doc: /* SKIP: real doc in xfns.c.  */);
  Vx_max_tooltip_size = Qnil;

  DEFSYM (Qcancel_timer, "cancel-timer");
  DEFSYM (Qrun_at_time, "run-at-time");
  DEFSYM (Qx_hide_tip, "x-hide-tip");
  /* Popup frames for menus.  */
  DEFSYM (Qnhwl_popup, "nhwl-popup");
  DEFSYM (Qnhwl_menu_popup, "nhwl-menu-popup");
  DEFSYM (Qnhwl_popup_decode, "nhwl-popup-decode");
  tip_timer = tip_frame = tip_last_frame = tip_last_string = tip_last_parms
    = Qnil;
  staticpro (&tip_timer);
  staticpro (&tip_frame);
  staticpro (&tip_last_frame);
  staticpro (&tip_last_string);
  staticpro (&tip_last_parms);

  DEFVAR_BOOL ("font-use-system-font", use_system_font,
    doc: /* SKIP: real doc in xsettings.c.  */);
  use_system_font = false;

  DEFVAR_LISP ("xft-settings", Vxft_settings,
    doc: /* SKIP: real doc in xsettings.c.  */);
  Vxft_settings = Qnil;

  DEFVAR_LISP ("pgtk-use-im-context-on-new-connection",
	       Vpgtk_use_im_context_on_new_connection,
    doc: /* SKIP: real doc in pgtkim.c.  */);
  Vpgtk_use_im_context_on_new_connection = Qt;

  defsubr (&Sxw_display_color_p);
  defsubr (&Sx_display_grayscale_p);
  defsubr (&Sxw_color_defined_p);
  defsubr (&Sxw_color_values);
  defsubr (&Sx_server_max_request_size);
  defsubr (&Sx_display_pixel_width);
  defsubr (&Sx_display_pixel_height);
  defsubr (&Spgtk_display_monitor_attributes_list);
  defsubr (&Spgtk_frame_geometry);
  defsubr (&Spgtk_frame_edges);
  defsubr (&Spgtk_set_mouse_absolute_pixel_position);
  defsubr (&Spgtk_mouse_absolute_pixel_position);
  defsubr (&Sx_display_mm_width);
  defsubr (&Sx_display_mm_height);
  defsubr (&Sx_display_screens);
  defsubr (&Sx_display_planes);
  defsubr (&Sx_display_color_cells);
  defsubr (&Sx_display_visual_class);
  defsubr (&Sx_display_backing_store);
  defsubr (&Sx_display_save_under);
  defsubr (&Sx_create_frame);
  defsubr (&Sx_open_connection);
  defsubr (&Sx_close_connection);
  defsubr (&Sx_display_list);
  defsubr (&Sx_show_tip);
  defsubr (&Sx_hide_tip);
  defsubr (&Spgtk_backend_display_class);
  defsubr (&Sfont_get_system_font);
  defsubr (&Sfont_get_system_normal_font);
  defsubr (&Stool_bar_get_system_style);
  defsubr (&Spgtk_use_im_context);
  defsubr (&Snhwl_set_cursor_theme);
  defsubr (&Snhwl_frame_begin_move);
  defsubr (&Snhwl_frame_begin_resize);
  defsubr (&Snhwl_frame_show_window_menu);
  DEFSYM (Qinhibit_idle, "inhibit-idle");
  defsubr (&Snhwl_frame_export_handle);
  defsubr (&Snhwl_activation_token);
  defsubr (&Snhwl_system_bell_p);
  DEFSYM (Qsession_name, "session-name");
  DEFSYM (Qnhwl_session_start, "nhwl-session-start");
  DEFSYM (Qnhwl_session_name, "nhwl-session-name");
  DEFSYM (Qnhwl_session_launch, "launch");
  DEFSYM (Qnhwl_session_recover, "recover");
  DEFSYM (Qnhwl_session_restore, "restore");
  DEFSYM (Qcreated, "created");
  DEFSYM (Qnhwl_session_restored, "restored");
  DEFSYM (Qnhwl_session_replaced, "replaced");
  defsubr (&Snhwl__session_open);
  defsubr (&Snhwl__session_remove);

  /* The selection primitives.  */
  DEFSYM (QXdndSelection, "XdndSelection");
  DEFSYM (Qcopy, "copy");
  DEFSYM (Qmove, "move");
  DEFSYM (Qnhwl_dnd_ask, "ask");
  DEFSYM (Qlink, "link");
  DEFSYM (Qprivate, "private");
  DEFSYM (QCLIPBOARD, "CLIPBOARD");
  DEFSYM (Qnhwl_selection_timeout, "nhwl-selection-timeout");
  DEFSYM (Qpgtk_lost_selection_functions, "pgtk-lost-selection-functions");
  DEFSYM (QUTF8_STRING, "UTF8_STRING");
  DEFSYM (QTARGETS, "TARGETS");
  DEFSYM (QTEXT, "TEXT");
  DEFSYM (QSTRING, "STRING");
  DEFSYM (QCOMPOUND_TEXT, "COMPOUND_TEXT");
  DEFSYM (Qtext_plain, "text/plain");
  DEFSYM (Qtext_plain_utf8, "text/plain;charset=utf-8");

  DEFVAR_LISP ("pgtk-lost-selection-functions", Vpgtk_lost_selection_functions,
    doc: /* A list of functions to be called when Emacs loses a selection.
Each is called with the selection (a symbol) as its argument.  */);
  Vpgtk_lost_selection_functions = Qnil;

  DEFSYM (Qnow, "now");
  DEFVAR_LISP ("nhwl--xdnd-value", Vnhwl_xdnd_value,
    doc: /* The XdndSelection value Lisp set (`gui-set-selection'), or nil.
`x-begin-drag' converts it when a drag starts; no other client sees it
before.  The drag's bytes stay in the shell until the next drag.
Internal.  */);
  Vnhwl_xdnd_value = Qnil;
  DEFVAR_LISP ("nhwl-dnd-finish-timeout", Vnhwl_dnd_finish_timeout,
    doc: /* Seconds `x-begin-drag' waits for a target to finish a drop.
After this long without a word from the drag, or after the drop without
the target's finish, the drag is given up and `x-begin-drag' returns
nil.  At most 60.  */);
  Vnhwl_dnd_finish_timeout = make_float (5.0);
  defsubr (&Snhwl__drag_start);

  defsubr (&Spgtk_own_selection_internal);
  defsubr (&Spgtk_disown_selection_internal);
  defsubr (&Spgtk_selection_owner_p);
  defsubr (&Spgtk_selection_exists_p);
  defsubr (&Spgtk_get_selection_internal);
  defsubr (&Spgtk_register_dnd_targets);
  defsubr (&Spgtk_drop_finish);
  defsubr (&Spgtk_update_drop_status);
}

/* The title bars' symbols and functions (the name is the one emacs.c
   calls, from when they lived in nhwlcsd.c).  */
void
syms_of_nhwlcsd (void)
{
  DEFSYM (QCstrip, ":strip");
  DEFSYM (QCmargins, ":margins");
  DEFSYM (QCbuttons, ":buttons");
  csd_colours = csd_font = Qnil;
  staticpro (&csd_colours);
  staticpro (&csd_font);
  defsubr (&Snhwl__set_title_bar_style);
  defsubr (&Snhwl__title_bar);
}

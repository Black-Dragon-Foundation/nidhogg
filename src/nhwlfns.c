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
   is provided too).  Functions adapted from pgtkfns.c name their origin.
   The frame parameters of W0 (the plan's W0-5) have effects; every other
   parameter is accepted and stored.  Not in W0 (W0-6): selections, child
   and tip frames; they signal.  */

#include <config.h>

#include <math.h>
#include <c-strcase.h>

#include "lisp.h"
#include "nhwlterm.h"
#include "blockinput.h"
#include "window.h"
#include "character.h"
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
   the frame and Task 10 paints with it.  */
static void
nhwl_set_alpha_background (struct frame *f, Lisp_Object arg,
			   Lisp_Object oldval)
{
  gui_set_alpha_background (f, arg, oldval);
  if (FRAME_VISIBLE_P (f))
    SET_FRAME_GARBAGED (f);
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

/* The shell takes the flag when the window is made.  */
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
   are 0 are accepted and stored only (W0-5).  */
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
    0,				/* icon-name */
    0,				/* icon-type */
    nhwl_set_child_frame_border_width,
    nhwl_set_internal_border_width,
    gui_set_right_divider_width,
    gui_set_bottom_divider_width,
    nhwl_set_menu_bar_lines,
    nhwl_set_mouse_color,
    nhwl_explicitly_set_name,
    0,				/* scroll-bar-width: no scroll bars in W0 */
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
    0,				/* wait-for-wm */
    gui_set_fullscreen,		/* generic OK */
    gui_set_font_backend,	/* generic OK */
    0,				/* alpha */
    0,				/* sticky */
    0,				/* tool-bar-position */
    0,				/* inhibit-double-buffering */
    nhwl_set_undecorated,
    0,				/* parent-frame */
    0,				/* skip-taskbar */
    0,				/* no-focus-on-map */
    0,				/* no-accept-focus */
    0,				/* z-group */
    0,				/* override-redirect */
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

  name = gui_display_get_arg (dpyinfo, parms, Qname, "name", "Name",
			      RES_TYPE_STRING);
  if (!STRINGP (name) && !BASE_EQ (name, Qunbound) && !NILP (name))
    error ("Invalid frame name--not a string or nil");

  if (STRINGP (name))
    Vx_resource_name = name;

  /* Child frames (W0-6: not in W0).  */
  parent = gui_display_get_arg (dpyinfo, parms, Qparent_id, NULL, NULL,
				RES_TYPE_NUMBER);
  if (BASE_EQ (parent, Qunbound))
    parent = Qnil;
  parent_frame = gui_display_get_arg (dpyinfo, parms, Qparent_frame, NULL,
				      NULL, RES_TYPE_SYMBOL);
  if (!NILP (parent)
      || (!BASE_EQ (parent_frame, Qunbound) && !NILP (parent_frame)))
    error ("nidhogg-wayland: child frames arrive in W1");

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

  fset_parent_frame (f, Qnil);
  store_frame_param (f, Qparent_frame, Qnil);

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
  /* The scroll bar parameters are stored and ignored (W0-6).  */
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
  nhwl_create_frame_window (f);
  out->next_output = dpyinfo->outputs;
  dpyinfo->outputs = out;

  out->current_cursor = out->nontext_cursor
    = nhwl_cursor_for_shape (NHSHELL_CURSOR_DEFAULT);
  out->text_cursor = out->modeline_cursor
    = nhwl_cursor_for_shape (NHSHELL_CURSOR_TEXT);
  out->hand_cursor = nhwl_cursor_for_shape (NHSHELL_CURSOR_POINTER);
  out->hourglass_cursor = nhwl_cursor_for_shape (NHSHELL_CURSOR_WAIT);
  out->horizontal_drag_cursor = out->left_edge_cursor = out->right_edge_cursor
    = nhwl_cursor_for_shape (NHSHELL_CURSOR_RESIZE_EW);
  out->vertical_drag_cursor = out->top_edge_cursor = out->bottom_edge_cursor
    = nhwl_cursor_for_shape (NHSHELL_CURSOR_RESIZE_NS);
  out->top_left_corner_cursor = out->bottom_right_corner_cursor
    = nhwl_cursor_for_shape (NHSHELL_CURSOR_RESIZE_NWSE);
  out->top_right_corner_cursor = out->bottom_left_corner_cursor
    = nhwl_cursor_for_shape (NHSHELL_CURSOR_RESIZE_NESW);

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

/* The size of the display in millimetres: the same arithmetic as pgtk's
   (outputs at the origin count the largest, others add up).  */
static int
nhwl_display_mm (struct nhwl_display_info *dpyinfo, bool width)
{
  int at_0 = 0, at_other = 0, i;

  for (i = 0; i < dpyinfo->n_monitors; i++)
    {
      struct nhwl_monitor *m = &dpyinfo->monitors[i];
      int mm = width ? m->mm_w : m->mm_h;

      if ((width ? m->x : m->y) == 0)
	at_0 = max (at_0, mm);
      else
	at_other += mm;
    }
  return at_0 + at_other;
}

DEFUN ("x-display-mm-height", Fx_display_mm_height, Sx_display_mm_height, 0, 1,
       0, doc: /* SKIP: real doc in xfns.c.  */)
  (Lisp_Object terminal)
{
  return make_fixnum (nhwl_display_mm (check_nhwl_display_info (terminal),
				       false));
}

DEFUN ("x-display-mm-width", Fx_display_mm_width, Sx_display_mm_width, 0, 1, 0,
       doc: /* SKIP: real doc in xfns.c.  */)
  (Lisp_Object terminal)
{
  return make_fixnum (nhwl_display_mm (check_nhwl_display_info (terminal),
				       true));
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
  struct nhwl_display_info *dpyinfo = check_nhwl_display_info (terminal);
  int i, width = 0;

  for (i = 0; i < dpyinfo->n_monitors; i++)
    {
      struct nhwl_monitor *m = &dpyinfo->monitors[i];
      int x = (int) roundf (m->x * m->scale120 / 120.0f);

      width = max (width, x + m->w);
    }
  return make_fixnum (width ? width : nhwl_display_pixel_width (dpyinfo));
}

DEFUN ("x-display-pixel-height", Fx_display_pixel_height,
       Sx_display_pixel_height, 0, 1, 0,
       doc: /* SKIP: real doc in xfns.c.  */)
  (Lisp_Object terminal)
{
  struct nhwl_display_info *dpyinfo = check_nhwl_display_info (terminal);
  int i, height = 0;

  for (i = 0; i < dpyinfo->n_monitors; i++)
    {
      struct nhwl_monitor *m = &dpyinfo->monitors[i];
      int y = (int) roundf (m->y * m->scale120 / 120.0f);

      height = max (height, y + m->h);
    }
  return make_fixnum (height ? height : nhwl_display_pixel_height (dpyinfo));
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
	   information is obtained, \"Nhwl\"

Internal use only, use `display-monitor-attributes-list' instead.  */)
  (Lisp_Object terminal)
{
  struct nhwl_display_info *dpyinfo = check_nhwl_display_info (terminal);
  Lisp_Object attributes_list = Qnil;
  int i, n_monitors = dpyinfo->n_monitors;
  Lisp_Object monitor_frames, rest, frame;
  static const char *source = "Nhwl";
  struct MonitorInfo *monitors;

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
	/* The shell does not say which output a window is on: the first.  */
	ASET (monitor_frames, 0, Fcons (frame, AREF (monitor_frames, 0)));
    }

  for (i = 0; i < n_monitors; ++i)
    {
      struct nhwl_monitor *m = &dpyinfo->monitors[i];
      struct MonitorInfo *mi = &monitors[i];

      mi->geom.x = (int) roundf (m->x * m->scale120 / 120.0f);
      mi->geom.y = (int) roundf (m->y * m->scale120 / 120.0f);
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

DEFUN ("x-show-tip", Fx_show_tip, Sx_show_tip, 1, 6, 0,
       doc: /* SKIP: real doc in xfns.c.  */)
  (Lisp_Object string, Lisp_Object frame, Lisp_Object parms,
   Lisp_Object timeout, Lisp_Object dx, Lisp_Object dy)
{
  error ("nidhogg-wayland: tooltip frames arrive in W1");
}

DEFUN ("x-hide-tip", Fx_hide_tip, Sx_hide_tip, 0, 0, 0,
       doc: /* SKIP: real doc in xfns.c.  */)
  (void)
{
  return Qnil;
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

/* The selection primitives of pgtk-win.el (W0-6: not in W0, they arrive in
   W1).  Until then nothing is owned and nothing exists, quietly: every kill
   calls `gui-select-text', so an error here would break editing.  */

#define NHWL_NO_SELECTIONS return Qnil

DEFUN ("pgtk-own-selection-internal", Fpgtk_own_selection_internal,
       Spgtk_own_selection_internal, 2, 3, 0,
       doc: /* Assert an X selection of type SELECTION and value VALUE.  */)
  (Lisp_Object selection, Lisp_Object value, Lisp_Object frame)
{
  NHWL_NO_SELECTIONS;
}

DEFUN ("pgtk-disown-selection-internal", Fpgtk_disown_selection_internal,
       Spgtk_disown_selection_internal, 1, 2, 0,
       doc: /* If we own the selection SELECTION, disown it.  */)
  (Lisp_Object selection, Lisp_Object terminal)
{
  NHWL_NO_SELECTIONS;
}

DEFUN ("pgtk-selection-owner-p", Fpgtk_selection_owner_p,
       Spgtk_selection_owner_p, 0, 2, 0,
       doc: /* Whether the current Emacs process owns the selection SELECTION.  */)
  (Lisp_Object selection, Lisp_Object terminal)
{
  NHWL_NO_SELECTIONS;
}

DEFUN ("pgtk-selection-exists-p", Fpgtk_selection_exists_p,
       Spgtk_selection_exists_p, 0, 2, 0,
       doc: /* Whether there is an owner for the given selection.  */)
  (Lisp_Object selection, Lisp_Object terminal)
{
  NHWL_NO_SELECTIONS;
}

DEFUN ("pgtk-get-selection-internal", Fpgtk_get_selection_internal,
       Spgtk_get_selection_internal, 2, 3, 0,
       doc: /* Return text selected from some X window.  */)
  (Lisp_Object selection_symbol, Lisp_Object target_type,
   Lisp_Object terminal)
{
  NHWL_NO_SELECTIONS;
}

/* Drag and drop and input methods are not in W0: nothing happens, and the
   Lisp that registers them runs on.  */

DEFUN ("pgtk-register-dnd-targets", Fpgtk_register_dnd_targets,
       Spgtk_register_dnd_targets, 2, 2, 0,
       doc: /* Register the drop targets of FRAME: not in W0.  */)
  (Lisp_Object frame, Lisp_Object targets)
{
  return Qnil;
}

DEFUN ("pgtk-use-im-context", Fpgtk_use_im_context, Spgtk_use_im_context,
       1, 1, 0,
       doc: /* Set whether to use an input method context: not in W0.  */)
  (Lisp_Object use_p)
{
  return Qnil;
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

  DEFVAR_BOOL ("font-use-system-font", use_system_font,
    doc: /* SKIP: real doc in xsettings.c.  */);
  use_system_font = false;

  DEFVAR_LISP ("xft-settings", Vxft_settings,
    doc: /* SKIP: real doc in xsettings.c.  */);
  Vxft_settings = Qnil;

  DEFVAR_LISP ("pgtk-use-im-context-on-new-connection",
	       Vpgtk_use_im_context_on_new_connection,
    doc: /* SKIP: real doc in pgtkim.c.  */);
  Vpgtk_use_im_context_on_new_connection = Qnil;

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
  defsubr (&Spgtk_own_selection_internal);
  defsubr (&Spgtk_disown_selection_internal);
  defsubr (&Spgtk_selection_owner_p);
  defsubr (&Spgtk_selection_exists_p);
  defsubr (&Spgtk_get_selection_internal);
  defsubr (&Spgtk_register_dnd_targets);
  defsubr (&Spgtk_use_im_context);
}

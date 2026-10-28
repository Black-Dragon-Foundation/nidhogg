/* Terminal glue for Nidhogg's own Wayland shell (HAVE_NHWL).
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

/* This file implements Emacs's terminal hooks against the shell API
   (nhshell.h) once; it never calls Wayland.  Functions adapted from
   pgtkterm.c name their origin in their comment.  The frame's pixel unit
   is the device pixel, so a scale change re-fonts the frame as a move
   between monitors does, and no drawing happens at a fractional scale.

   Every shell call runs under block_input: the input polling timer can
   enter nhwl_read_socket from a signal handler, and the shell is not
   re-entrant.  */

#include <config.h>

#include <cairo.h>
#include <errno.h>
#include <fcntl.h>
#include <math.h>
#include <poll.h>
#include <stdlib.h>
#include <signal.h>
#include <string.h>
#include <time.h>
#include <unistd.h>

#include <c-ctype.h>
#include <c-strcase.h>

#include "lisp.h"
#include "nhwlterm.h"
#include "blockinput.h"
#include "frame.h"
#include "sysselect.h"
#include "systime.h"
#include "character.h"
#include "fontset.h"
#include "composite.h"

#include "termhooks.h"
#include "termopts.h"
#include "termchar.h"
#include "menu.h"
#include "window.h"
#include "keyboard.h"
#include "atimer.h"
#include "buffer.h"
#include "font.h"
#include "process.h"
#include "pdumper.h"

/* Chain of existing displays */
struct nhwl_display_info *x_display_list;

/* The modifier each of the shell's modifier bits stands for in Emacs is
   customizable (nhwl-{alt,meta,super,hyper}-modifier; the DEFVAR_LISP
   variables Vnhwl_* are in globals.h).  */

/* Non-zero means that a HELP_EVENT has been generated since Emacs
   start.  */
static bool any_help_event_p;

/* Run the shell calls of a function under block_input.  */
#define NHWL_SHELL_OK(rc) ((rc) >= 0)

/* The Emacs_Cursor for the shell's pointer shape SHAPE (and back).  */
Emacs_Cursor
nhwl_cursor_for_shape (uint32_t shape)
{
  return (Emacs_Cursor) (intptr_t) (shape + 1);
}

static void nhwl_new_focus_frame (struct nhwl_display_info *, struct frame *);
static void nhwl_delete_display (struct nhwl_display_info *);
static void nhwl_set_fullscreen_state (struct frame *);
static void nhwl_toggle_invisible_pointer (struct frame *, bool);


/***********************************************************************
			   Display lookup
 ***********************************************************************/

/* Adapted from check_pgtk_display_info (pgtkfns.c).  */
struct nhwl_display_info *
check_nhwl_display_info (Lisp_Object object)
{
  struct nhwl_display_info *dpyinfo = NULL;

  if (NILP (object))
    {
      struct frame *sf = XFRAME (selected_frame);

      if (FRAME_NHWL_P (sf) && FRAME_LIVE_P (sf))
	dpyinfo = FRAME_DISPLAY_INFO (sf);
      else if (x_display_list != 0)
	dpyinfo = x_display_list;
      else
	error ("Frames are not in use or not initialized");
    }
  else if (TERMINALP (object))
    {
      struct terminal *t = decode_live_terminal (object);

      if (t->type != output_nhwl)
	error ("Terminal %d is not a display", t->id);

      dpyinfo = t->display_info.nhwl;
    }
  else if (STRINGP (object))
    dpyinfo = nhwl_display_info_for_name (object);
  else
    {
      struct frame *f = decode_window_system_frame (object);
      dpyinfo = FRAME_DISPLAY_INFO (f);
    }

  return dpyinfo;
}

/* Called from frame.c.  */
struct nhwl_display_info *
check_x_display_info (Lisp_Object frame)
{
  return check_nhwl_display_info (frame);
}

/* The compositor went away.  The display's frames and its terminal are
   deleted without running Lisp, and the session goes on on its other
   frames (another display, a tty, the daemon's own frame).  With no
   frame left Emacs exits with status 70, as xterm does: before the
   deletion, since delete_frame would otherwise kill Emacs from inside
   it.  Frees DPYINFO.  */
static void
nhwl_connection_lost (struct nhwl_display_info *dpyinfo)
{
  struct terminal *t = dpyinfo->terminal, *p;
  Lisp_Object tail, frame;
  bool other = false;

  FOR_EACH_FRAME (tail, frame)
    {
      struct frame *f = XFRAME (frame);

      if (FRAME_LIVE_P (f) && !FRAME_TOOLTIP_P (f)
	  && !(FRAME_NHWL_P (f) && FRAME_DISPLAY_INFO (f) == dpyinfo))
	other = true;
    }
  if (!other)
    {
      fprintf (stderr, "nidhogg-wayland: connection to the display lost\n");
      shut_down_emacs (0, Qnil);
      exit (70);
    }

  FOR_EACH_FRAME (tail, frame)
    if (FRAME_NHWL_P (XFRAME (frame))
	&& FRAME_DISPLAY_INFO (XFRAME (frame)) == dpyinfo
	&& FRAME_LIVE_P (XFRAME (frame)))
      delete_frame (frame, Qnoelisp);

  /* Deleting the last frame deletes the terminal as well.  */
  for (p = terminal_list; p; p = p->next_terminal)
    if (p == t)
      {
	XSETTERMINAL (frame, t);
	Fdelete_terminal (frame, Qnoelisp);
	break;
      }
}

/* Return the display structure for the display named NAME, opening a
   new connection if necessary.  */
struct nhwl_display_info *
nhwl_display_info_for_name (Lisp_Object name)
{
  struct nhwl_display_info *dpyinfo;

  CHECK_STRING (name);

  for (dpyinfo = x_display_list; dpyinfo; dpyinfo = dpyinfo->next)
    if (!NILP (Fstring_equal (XCAR (dpyinfo->name_list_element), name)))
      return dpyinfo;

  /* Use this general default value to start with.  */
  Vx_resource_name = Vinvocation_name;

  validate_x_resource_name ();

  dpyinfo = nhwl_term_init (name);

  if (dpyinfo == 0)
    error ("Cannot connect to display server %s", SDATA (name));

  return dpyinfo;
}

/* The frame of display DPYINFO that owns shell window WINDOW.  Frames
   are found by the display's own chain, not Vframe_list, because the
   compositor's first events arrive before a frame is official.  */
static struct frame *
nhwl_window_to_frame (struct nhwl_display_info *dpyinfo, uint64_t window)
{
  struct nhwl_output *o;

  for (o = dpyinfo->outputs; o; o = o->next_output)
    if (o->window == window && o->window != 0)
      return o->frame;
  return NULL;
}


/***********************************************************************
				Colors
 ***********************************************************************/

/* The names etc/rgb.txt knows, as (NAME . #RRGGBB) pairs.  */
static bool
nhwl_lookup_color_name (struct nhwl_display_info *dpyinfo, const char *name,
			Emacs_Color *color)
{
  Lisp_Object tem;

  for (tem = dpyinfo->color_map; CONSP (tem); tem = XCDR (tem))
    {
      Lisp_Object col = XCAR (tem);

      if (CONSP (col) && STRINGP (XCAR (col))
	  && !xstrcasecmp (SSDATA (XCAR (col)), name))
	{
	  unsigned long clr = XFIXNUM (XCDR (col));

	  color->red = RED_FROM_ULONG (clr) * 257;
	  color->green = GREEN_FROM_ULONG (clr) * 257;
	  color->blue = BLUE_FROM_ULONG (clr) * 257;
	  color->pixel = clr & 0xffffff;
	  return true;
	}
    }
  return false;
}

/* Adapted from pgtk_parse_color: numeric specs, then the colour table.  */
int
nhwl_parse_color (struct frame *f, const char *color_name,
		  Emacs_Color *color)
{
  unsigned short r, g, b;

  if (parse_color_spec (color_name, &r, &g, &b))
    {
      color->red = r;
      color->green = g;
      color->blue = b;
      color->pixel = ((r >> 8) << 16 | (g >> 8) << 8 | (b >> 8) << 0);
      return 1;
    }
  return nhwl_lookup_color_name (FRAME_DISPLAY_INFO (f), color_name, color);
}

/* Adapted from pgtk_defined_color.  */
bool
nhwl_defined_color (struct frame *f, const char *name, Emacs_Color *color_def,
		    bool alloc, bool makeIndex)
{
  return nhwl_parse_color (f, name, color_def);
}

/* Adapted from pgtk_query_colors.  */
void
nhwl_query_colors (struct frame *f, Emacs_Color *colors, int ncolors)
{
  int i;

  for (i = 0; i < ncolors; i++)
    {
      unsigned long pixel = colors[i].pixel;

      colors[i].red = RED_FROM_ULONG (pixel) * 257;
      colors[i].green = GREEN_FROM_ULONG (pixel) * 257;
      colors[i].blue = BLUE_FROM_ULONG (pixel) * 257;
    }
}

void
nhwl_query_color (struct frame *f, Emacs_Color *color)
{
  nhwl_query_colors (f, color, 1);
}

/* Adapted from pgtk_query_frame_background_color.  */
static void
nhwl_query_frame_background_color (struct frame *f, Emacs_Color *bgcolor)
{
  bgcolor->pixel = FRAME_BACKGROUND_PIXEL (f);
  nhwl_query_color (f, bgcolor);
}


/***********************************************************************
		    The backing and the present path
 ***********************************************************************/

/* Adapted from pgtk_begin_cr_clip: the frame's retained backing, device
   size (the frame's pixel size), made or resized on use.  */
cairo_t *
nhwl_begin_cr_clip (struct frame *f)
{
  cairo_t *cr;

  nhwl_backing_resize (f);
  cr = FRAME_NHWL_OUTPUT (f)->cr_context;
  cairo_save (cr);
  /* Emacs draws in the native area; the title border lies around it.  */
  cairo_translate (cr, FRAME_NHWL_OUTPUT (f)->csd.l, FRAME_NHWL_OUTPUT (f)->csd.t);
  return cr;
}

void
nhwl_end_cr_clip (struct frame *f)
{
  cairo_restore (FRAME_NHWL_OUTPUT (f)->cr_context);
  nhwl_dirty (f);
}

/* Adapted from pgtk_cr_destroy_frame_context.  */
static void
nhwl_cr_destroy_frame_context (struct frame *f)
{
  struct nhwl_output *out = FRAME_NHWL_OUTPUT (f);

  if (out->cr_context)
    {
      cairo_destroy (out->cr_context);
      out->cr_context = NULL;
    }
  if (out->cr_surface)
    {
      cairo_surface_destroy (out->cr_surface);
      out->cr_surface = NULL;
    }
}

/* Adapted from pgtk_cr_update_surface_desired_size: the backing is
   re-created at the new size on its next use, and the frame redrawn.  */
void
nhwl_cr_update_surface_desired_size (struct frame *f, int width, int height,
				     bool force)
{
  struct nhwl_output *out = FRAME_NHWL_OUTPUT (f);

  if (out->cr_width != width || out->cr_height != height || force)
    {
      nhwl_cr_destroy_frame_context (f);
      out->cr_width = width;
      out->cr_height = height;
      SET_FRAME_GARBAGED (f);
    }
}

void
nhwl_set_cr_source_with_gc_foreground (struct frame *f, Emacs_GC *gc,
				       bool respects_alpha_background)
{
  nhwl_set_cr_source_with_color (f, gc->foreground, respects_alpha_background);
}

void
nhwl_set_cr_source_with_gc_background (struct frame *f, Emacs_GC *gc,
				       bool respects_alpha_background)
{
  nhwl_set_cr_source_with_color (f, gc->background, respects_alpha_background);
}

/* Adapted from pgtk_set_cr_source_with_color.  */
void
nhwl_set_cr_source_with_color (struct frame *f, unsigned long color,
			       bool respects_alpha_background)
{
  Emacs_Color col;
  cairo_t *cr = FRAME_NHWL_OUTPUT (f)->cr_context;

  col.pixel = color;
  nhwl_query_color (f, &col);

  if (!respects_alpha_background)
    {
      cairo_set_source_rgb (cr, col.red / 65535.0, col.green / 65535.0,
			    col.blue / 65535.0);
      cairo_set_operator (cr, CAIRO_OPERATOR_OVER);
    }
  else
    {
      cairo_set_source_rgba (cr, col.red / 65535.0, col.green / 65535.0,
			     col.blue / 65535.0, f->alpha_background);
      cairo_set_operator (cr, CAIRO_OPERATOR_SOURCE);
    }
}

/* The frame has something to show: the window's pacer answers with
   FRAME_READY when the frame may be presented.  */
void
nhwl_dirty (struct frame *f)
{
  struct nhwl_output *out = FRAME_NHWL_OUTPUT (f);
  struct nhwl_display_info *dpyinfo = FRAME_DISPLAY_INFO (f);

  if (dpyinfo->shell && out->window)
    nhshell_window_dirty (dpyinfo->shell, out->window);
}

/* Show the backing when the window's pacer says so (FRAME_READY): Glitnir
   commits the frame's damage, the shell hands out a buffer, Glitnir
   composes the damage (and what the buffer's age leaves stale) into it, and
   the buffer is committed.  The pacer owns when: frame callbacks, the
   suspended state and a lack of buffers are its states, not ours.  Nothing
   is done before the compositor's first configure, while the frame is
   invisible, or while the frame has not taken the size change_frame_size
   queued for the next redisplay (the next redisplay draws, and
   nhwl_frame_up_to_date asks again).  Return true when a buffer was
   committed.  */
bool
nhwl_present (struct frame *f)
{
  struct nhwl_output *out = FRAME_NHWL_OUTPUT (f);
  struct nhwl_display_info *dpyinfo = FRAME_DISPLAY_INFO (f);
  NhShellBuffer buf;
  int rc, ready = 0;

  if (!dpyinfo->shell || !out->window)
    return false;
  /* Asking is what lets the pacer say FRAME_READY again, so ask first.  */
  if (nhshell_window_ready (dpyinfo->shell, out->window, &ready) < 0
      || !ready || !out->configured || !FRAME_VISIBLE_P (f) || f->new_size_p
      || (nhwl_child_p (f) && !out->child.mapped))
    return false;

  block_input ();
  nhwl_backing_resize (f);
  nhwl_csd_draw (f);
  nhwl_child_sync (f);
  nhwl_glitnir_commit (f);

  rc = nhshell_present_begin (dpyinfo->shell, out->window, out->cr_width,
			      out->cr_height, &buf);
  if (rc < 0)
    {
      /* Every buffer is busy: the pacer is starved and says FRAME_READY
	 again at a buffer release.  */
      unblock_input ();
      return false;
    }

  nhwl_glitnir_compose (f, &buf);

  nhwl_csd_send_geometry (f);
  rc = nhshell_present_commit (dpyinfo->shell, out->window, out->pending,
			       out->npending);
  if (rc >= 0)
    nhwl_glitnir_presented (f);
  else
    nhwl_dirty (f);
  unblock_input ();
  return rc >= 0;
}

/* Adapted from pgtk_clear_area.  */
void
nhwl_clear_area (struct frame *f, int x, int y, int width, int height)
{
  cairo_t *cr;

  eassert (width > 0 && height > 0);

  cr = nhwl_begin_cr_clip (f);
  nhwl_damage (f, x, y, width, height);
  nhwl_set_cr_source_with_color (f, FRAME_NHWL_OUTPUT (f)->background_color,
				 true);
  cairo_rectangle (cr, x, y, width, height);
  cairo_fill (cr);
  nhwl_end_cr_clip (f);
}

static void
nhwl_clear_frame_area (struct frame *f, int x, int y, int width, int height)
{
  nhwl_clear_area (f, x, y, width, height);
}

/* Adapted from pgtk_clear_frame.  */
void
nhwl_clear_frame (struct frame *f)
{
  if (!FRAME_DEFAULT_FACE (f))
    return;

  mark_window_cursors_off (XWINDOW (FRAME_ROOT_WINDOW (f)));

  block_input ();
  nhwl_clear_area (f, 0, 0, FRAME_PIXEL_WIDTH (f), FRAME_PIXEL_HEIGHT (f));
  unblock_input ();
}

/* Adapted from fill_background_by_face (pgtkterm.c).  */
void
nhwl_fill_background_by_face (struct frame *f, struct face *face, int x,
			      int y, int width, int height)
{
  cairo_t *cr = nhwl_begin_cr_clip (f);
  double r, g, b, a;

  cairo_set_operator (cr, CAIRO_OPERATOR_SOURCE);
  cairo_rectangle (cr, x, y, width, height);
  cairo_clip (cr);

  r = ((face->background >> 16) & 0xff) / 255.0;
  g = ((face->background >> 8) & 0xff) / 255.0;
  b = ((face->background >> 0) & 0xff) / 255.0;
  a = f->alpha_background;
  cairo_set_source_rgba (cr, r, g, b, a);
  cairo_paint (cr);

  if (face->stipple != 0)
    {
      cairo_pattern_t *mask
	= FRAME_DISPLAY_INFO (f)->bitmaps[face->stipple - 1].pattern;

      r = ((face->foreground >> 16) & 0xff) / 255.0;
      g = ((face->foreground >> 8) & 0xff) / 255.0;
      b = ((face->foreground >> 0) & 0xff) / 255.0;
      cairo_set_source_rgba (cr, r, g, b, a);
      cairo_mask (cr, mask);
    }

  nhwl_end_cr_clip (f);
}

/* Adapted from pgtk_clear_under_internal_border.  */
void
nhwl_clear_under_internal_border (struct frame *f)
{
  if (FRAME_INTERNAL_BORDER_WIDTH (f) > 0 && FRAME_NHWL_OUTPUT (f)->window)
    {
      int border = FRAME_INTERNAL_BORDER_WIDTH (f);
      int width = FRAME_PIXEL_WIDTH (f);
      int height = FRAME_PIXEL_HEIGHT (f);
      int margin = FRAME_TOP_MARGIN_HEIGHT (f);
      int bottom_margin = FRAME_BOTTOM_MARGIN_HEIGHT (f);
      int face_id = (FRAME_PARENT_FRAME (f)
		     ? (!NILP (Vface_remapping_alist)
			? lookup_basic_face (NULL, f,
					     CHILD_FRAME_BORDER_FACE_ID)
			: CHILD_FRAME_BORDER_FACE_ID)
		     : (!NILP (Vface_remapping_alist)
			? lookup_basic_face (NULL, f,
					     INTERNAL_BORDER_FACE_ID)
			: INTERNAL_BORDER_FACE_ID));
      struct face *face = FACE_FROM_ID_OR_NULL (f, face_id);

      block_input ();
      nhwl_damage_internal_border (f);

      if (face)
	{
	  nhwl_fill_background_by_face (f, face, 0, margin, width, border);
	  nhwl_fill_background_by_face (f, face, 0, 0, border, height);
	  nhwl_fill_background_by_face (f, face, width - border, 0, border,
					height);
	  nhwl_fill_background_by_face (f, face, 0, (height - bottom_margin
						     - border),
					width, border);
	}
      else
	{
	  nhwl_clear_area (f, 0, 0, border, height);
	  nhwl_clear_area (f, 0, margin, width, border);
	  nhwl_clear_area (f, width - border, 0, border, height);
	  nhwl_clear_area (f, 0, height - bottom_margin - border, width,
			   border);
	}

      unblock_input ();
    }
}


/***********************************************************************
		 The redisplay interface
 ***********************************************************************/

/* Adapted from pgtk_compute_glyph_string_overhangs.  */
static void
nhwl_compute_glyph_string_overhangs (struct glyph_string *s)
{
  if (s->cmp == NULL
      && (s->first_glyph->type == CHAR_GLYPH
	  || s->first_glyph->type == COMPOSITE_GLYPH))
    {
      struct font_metrics metrics;

      if (s->first_glyph->type == CHAR_GLYPH)
	{
	  unsigned *code = alloca (sizeof (unsigned) * s->nchars);
	  struct font *font = s->font;
	  int i;

	  for (i = 0; i < s->nchars; i++)
	    code[i] = s->char2b[i];
	  font->driver->text_extents (font, code, s->nchars, &metrics);
	}
      else
	{
	  Lisp_Object gstring = composition_gstring_from_id (s->cmp_id);

	  composition_gstring_width (gstring, s->cmp_from, s->cmp_to,
				     &metrics);
	}
      s->right_overhang = (metrics.rbearing > metrics.width
			   ? metrics.rbearing - metrics.width : 0);
      s->left_overhang = metrics.lbearing < 0 ? -metrics.lbearing : 0;
    }
  else if (s->cmp)
    {
      s->right_overhang = s->cmp->rbearing - s->cmp->pixel_width;
      s->left_overhang = -s->cmp->lbearing;
    }
}

static void
nhwl_show_hourglass (struct frame *f)
{
}

static void
nhwl_hide_hourglass (struct frame *f)
{
}

/* Adapted from pgtk_define_frame_cursor.  */
static void
nhwl_define_frame_cursor (struct frame *f, Emacs_Cursor cursor)
{
  struct nhwl_output *out = FRAME_NHWL_OUTPUT (f);

  if (!f->pointer_invisible && out->current_cursor != cursor && out->window
      && cursor)
    {
      block_input ();
      nhshell_window_set_cursor (FRAME_NHWL_SHELL (f), out->window,
				 (uint32_t) ((intptr_t) cursor - 1));
      unblock_input ();
    }
  out->current_cursor = cursor;
}

/* Adapted from pgtk_after_update_window_line.  */
static void
nhwl_after_update_window_line (struct window *w, struct glyph_row *desired_row)
{
  struct frame *f;
  int width, height;

  eassert (w);

  if (!desired_row->mode_line_p && !w->pseudo_window_p)
    desired_row->redraw_fringe_bitmaps_p = 1;

  /* When a window has disappeared, make sure that no rest of
     full-width rows stays visible in the internal border.  */
  if (windows_or_buffers_changed
      && desired_row->full_width_p
      && (f = XFRAME (w->frame),
	  width = FRAME_INTERNAL_BORDER_WIDTH (f),
	  width != 0) && (height = desired_row->visible_height, height > 0))
    {
      int y = WINDOW_TO_FRAME_PIXEL_Y (w, max (0, desired_row->y));

      block_input ();
      nhwl_clear_frame_area (f, 0, y, width, height);
      nhwl_clear_frame_area (f, FRAME_PIXEL_WIDTH (f) - width, y, width,
			     height);
      unblock_input ();
    }
}

static void
nhwl_flush_display (struct frame *f)
{
}

static struct redisplay_interface nhwl_redisplay_interface = {
  nhwl_frame_parm_handlers,
  gui_produce_glyphs,
  gui_write_glyphs,
  gui_insert_glyphs,
  gui_clear_end_of_line,
  nhwl_scroll_run,
  nhwl_after_update_window_line,
  NULL, /* gui_update_window_begin, */
  NULL, /* gui_update_window_end, */
  nhwl_flush_display,
  gui_clear_window_mouse_face,
  gui_get_glyph_overhangs,
  gui_fix_overlapping_area,
  nhwl_draw_fringe_bitmap,
  nhwl_define_fringe_bitmap,
  nhwl_destroy_fringe_bitmap,
  nhwl_compute_glyph_string_overhangs,
  nhwl_draw_glyph_string,
  nhwl_define_frame_cursor,
  nhwl_clear_frame_area,
  nhwl_clear_under_internal_border,
  nhwl_draw_window_cursor,
  nhwl_draw_vertical_window_border,
  nhwl_draw_window_divider,
  NULL,				/* nhwl_shift_glyphs_for_insert, */
  nhwl_show_hourglass,
  nhwl_hide_hourglass,
  nhwl_default_font_parameter,
};


/***********************************************************************
			    Frame windows
 ***********************************************************************/

/* The scale of frame F's window as a factor (1.0 until the compositor
   says); the display's resolution is 96 times the display's scale.  */
static double
nhwl_frame_scale (struct frame *f)
{
  uint32_t s120 = FRAME_NHWL_OUTPUT (f)->scale120;

  return (s120 ? s120 : 120) / 120.0;
}

double
nhwl_frame_scale_factor (struct frame *f)
{
  return nhwl_frame_scale (f);
}

/* Logical size of DEV device pixels at scale S120/120, rounded the way
   the presenter rounds the viewport's destination.  */
int
nhwl_to_logical (int dev, uint32_t s120)
{
  int64_t s = max (1, (int64_t) s120);

  return (int) (((int64_t) dev * 120 + s / 2) / s);
}

/* Device size of LOGICAL pixels at scale S120/120 (round half away
   from zero, in f32 like the presenter's mutter-compatible rounding).  */
int
nhwl_to_device (int logical, uint32_t s120)
{
  return (int) roundf ((float) logical * (float) s120 / 120.0f);
}

/* Create frame F's shell window at its current pixel size.  Adapted from
   xg_create_frame_widgets' role in pgtk.  */
void
nhwl_create_frame_window (struct frame *f)
{
  struct nhwl_output *out = FRAME_NHWL_OUTPUT (f);
  struct nhwl_display_info *dpyinfo = FRAME_DISPLAY_INFO (f);
  NhShellWindowDesc desc;
  Lisp_Object title;
  Lisp_Object encoded = Qnil;
  uint64_t id = 0;
  int rc;

  title = !NILP (f->title) ? f->title : f->name;
  if (STRINGP (title))
    encoded = ENCODE_UTF_8 (title);

  memset (&desc, 0, sizeof desc);
  out->scale120 = dpyinfo->scale120 ? dpyinfo->scale120 : 120;
  desc.w = max (1, nhwl_to_logical (FRAME_PIXEL_WIDTH (f), out->scale120));
  desc.h = max (1, nhwl_to_logical (FRAME_PIXEL_HEIGHT (f), out->scale120));
  desc.title = STRINGP (encoded) ? SSDATA (encoded) : "";
  desc.app_id = "nidhogg";
  desc.flags = FRAME_UNDECORATED (f) ? NHSHELL_WIN_UNDECORATED : 0;
  if (nhwl_child_p (f))
    {
      /* A child frame starts at its parent's scale.  */
      out->scale120 = FRAME_NHWL_OUTPUT (FRAME_PARENT_FRAME (f))->scale120
	? FRAME_NHWL_OUTPUT (FRAME_PARENT_FRAME (f))->scale120 : out->scale120;
      desc.w = max (1, nhwl_to_logical (FRAME_PIXEL_WIDTH (f), out->scale120));
      desc.h = max (1, nhwl_to_logical (FRAME_PIXEL_HEIGHT (f), out->scale120));
      nhwl_child_describe (f, &desc);
    }

  block_input ();
  rc = nhshell_window_new (dpyinfo->shell, &desc, &id);
  unblock_input ();
  if (rc < 0)
    error ("nidhogg-wayland: cannot create a window (%d)", rc);
  out->window = id;
}

void
nhwl_set_title_hint (struct frame *f)
{
  struct nhwl_output *out = FRAME_NHWL_OUTPUT (f);
  Lisp_Object title = !NILP (f->title) ? f->title : f->name;

  if (out->window && STRINGP (title))
    {
      Lisp_Object encoded = ENCODE_UTF_8 (title);

      block_input ();
      nhshell_window_set_title (FRAME_NHWL_SHELL (f), out->window,
				SSDATA (encoded));
      unblock_input ();
      nhwl_csd_changed (f);
    }
}

void
nhwl_set_undecorated_hint (struct frame *f, bool undecorated)
{
  /* The shell takes the flag at creation (W0); a change later is W1's
     (it needs the decoration negotiation redone).  */
}

/* The largest side of a frame's surface, in device pixels.  Cairo's
   image surfaces stop at 32767 a side, and the shell's buffer is one
   shared-memory pool whose size the protocol carries as an int32, so
   4 x width x height must stay under 2 GiB: 16384 x 16384 x 4 is 1 GiB.  */
#define NHWL_MAX_SIDE 16384

/* Clamp the native size *W x *H of F so that its surface (the native
   area and our decorations) stays within NHWL_MAX_SIDE.  */
static void
nhwl_clamp_native_size (struct frame *f, int *w, int *h)
{
  struct nhwl_csd *c = &FRAME_NHWL_OUTPUT (f)->csd;

  *w = min (*w, NHWL_MAX_SIDE - c->l - c->r);
  *h = min (*h, NHWL_MAX_SIDE - c->t - c->b);
}

/* Adapted from pgtk_set_window_size.  The window's size is the size of
   the buffer we present, so the frame takes the size it asks for at
   once (the compositor only suggests).  */
void
nhwl_set_window_size (struct frame *f, bool change_gravity, int pixelwidth,
		      int pixelheight)
{
  nhwl_clamp_native_size (f, &pixelwidth, &pixelheight);
  change_frame_size (f, pixelwidth, pixelheight, false, true, false);
  SET_FRAME_GARBAGED (f);
}

/* Adapted from pgtk_set_offset: Wayland clients have no position.  */
static void
nhwl_set_offset (struct frame *f, int xoff, int yoff, int change_gravity)
{
  if (change_gravity > 0)
    {
      f->top_pos = yoff;
      f->left_pos = xoff;
      f->size_hint_flags &= ~ (XNegative | YNegative);
      if (xoff < 0)
	f->size_hint_flags |= XNegative;
      if (yoff < 0)
	f->size_hint_flags |= YNegative;
      f->win_gravity = NorthWestGravity;
    }
  /* A child frame has a position: in its parent.  */
  nhwl_child_reposition (f);
}

/* Adapted from pgtk_new_font.  */
static Lisp_Object
nhwl_new_font (struct frame *f, Lisp_Object font_object, int fontset)
{
  struct font *font = XFONT_OBJECT (font_object);
  int font_ascent, font_descent;

  if (fontset < 0)
    fontset = fontset_from_font (font_object);
  FRAME_FONTSET (f) = fontset;

  if (FRAME_FONT (f) == font)
    /* This font is already set in frame F.  There's nothing more to
       do.  */
    return font_object;

  FRAME_FONT (f) = font;

  FRAME_BASELINE_OFFSET (f) = font->baseline_offset;
  FRAME_COLUMN_WIDTH (f) = font->average_width;
  get_font_ascent_descent (font, &font_ascent, &font_descent);
  FRAME_LINE_HEIGHT (f) = font_ascent + font_descent;

  /* We could use a more elaborate calculation here.  */
  FRAME_TAB_BAR_HEIGHT (f) = FRAME_TAB_BAR_LINES (f) * FRAME_LINE_HEIGHT (f);

  /* Compute the scroll bar width in character columns.  */
  if (FRAME_CONFIG_SCROLL_BAR_WIDTH (f) > 0)
    {
      int wid = FRAME_COLUMN_WIDTH (f);
      FRAME_CONFIG_SCROLL_BAR_COLS (f)
	= (FRAME_CONFIG_SCROLL_BAR_WIDTH (f) + wid - 1) / wid;
    }
  else
    {
      int wid = FRAME_COLUMN_WIDTH (f);
      FRAME_CONFIG_SCROLL_BAR_COLS (f) = (14 + wid - 1) / wid;
    }

  /* Compute the scroll bar height in character lines.  */
  if (FRAME_CONFIG_SCROLL_BAR_HEIGHT (f) > 0)
    {
      int height = FRAME_LINE_HEIGHT (f);
      FRAME_CONFIG_SCROLL_BAR_LINES (f)
	= (FRAME_CONFIG_SCROLL_BAR_HEIGHT (f) + height - 1) / height;
    }
  else
    {
      int height = FRAME_LINE_HEIGHT (f);
      FRAME_CONFIG_SCROLL_BAR_LINES (f) = (14 + height - 1) / height;
    }

  /* Now make the frame display the given font.  */
  if (FRAME_NHWL_OUTPUT (f)->window)
    adjust_frame_size (f, FRAME_COLS (f) * FRAME_COLUMN_WIDTH (f),
		       FRAME_LINES (f) * FRAME_LINE_HEIGHT (f), 3,
		       false, Qfont);

  return font_object;
}

/* Wait for the compositor's first configure of frame F, handling the
   events that arrive meanwhile.  Adapted in role from
   pgtk_wait_for_map_event.  */
void
nhwl_wait_for_configure (struct frame *f)
{
  struct nhwl_output *out = FRAME_NHWL_OUTPUT (f);
  struct nhwl_display_info *dpyinfo = FRAME_DISPLAY_INFO (f);
  double limit = FLOATP (Vnhwl_wait_for_configure_timeout)
    ? XFLOAT_DATA (Vnhwl_wait_for_configure_timeout) : 0;
  int waited_ms = 0;

  int rounds = 0;

  while (!out->configured && dpyinfo->shell && waited_ms < limit * 1000
	 && rounds++ < 4000)
    {
      struct pollfd pfd = { .fd = dpyinfo->connection, .events = POLLIN };

      block_input ();
      nhshell_dispatch_main (dpyinfo->shell);
      unblock_input ();
      nhwl_drain_events (dpyinfo, NULL);
      if (out->configured)
	break;
      if (poll (&pfd, 1, 20) == 0)
	waited_ms += 20;
    }
}

/* Adapted from pgtk_make_frame_visible.  The toplevel is mapped by the
   first buffer we commit after the compositor's first configure.  */
void
nhwl_make_frame_visible (struct frame *f)
{
  struct nhwl_output *out = FRAME_NHWL_OUTPUT (f);

  if (!FRAME_VISIBLE_P (f))
    {
      nhwl_child_set_visible (f, true);
      if (!out->configured)
	nhwl_wait_for_configure (f);
      if (out->configured)
	{
	  if (f->want_fullscreen != FULLSCREEN_NONE)
	    nhwl_set_fullscreen_state (f);
	  SET_FRAME_VISIBLE (f, 1);
	  SET_FRAME_ICONIFIED (f, false);
	  out->has_been_visible = true;
	  SET_FRAME_GARBAGED (f);
	  nhwl_dirty (f);
	}
    }
}

/* Adapted from pgtk_make_frame_invisible.  The shell has no unmap in W0:
   the frame stops being presented.  */
void
nhwl_make_frame_invisible (struct frame *f)
{
  nhwl_child_set_visible (f, false);
  SET_FRAME_VISIBLE (f, 0);
  SET_FRAME_ICONIFIED (f, false);
}

static void
nhwl_make_frame_visible_invisible (struct frame *f, bool visible)
{
  if (visible)
    nhwl_make_frame_visible (f);
  else
    nhwl_make_frame_invisible (f);
}

/* Adapted from pgtk_iconify_frame.  */
void
nhwl_iconify_frame (struct frame *f)
{
  struct nhwl_display_info *dpyinfo = FRAME_DISPLAY_INFO (f);

  if (dpyinfo->highlight_frame == f)
    dpyinfo->highlight_frame = NULL;

  if (FRAME_ICONIFIED_P (f))
    return;

  if (FRAME_NHWL_OUTPUT (f)->window)
    {
      block_input ();
      nhshell_window_minimize (dpyinfo->shell, FRAME_NHWL_OUTPUT (f)->window);
      unblock_input ();
    }
  SET_FRAME_VISIBLE (f, 0);
  SET_FRAME_ICONIFIED (f, true);
}

/* Adapted from set_fullscreen_state (pgtkterm.c).  */
static void
nhwl_set_fullscreen_state (struct frame *f)
{
  struct nhwl_output *out = FRAME_NHWL_OUTPUT (f);
  NhShell *sh = FRAME_NHWL_SHELL (f);

  if (!out->window)
    return;

  block_input ();
  switch (f->want_fullscreen)
    {
    case FULLSCREEN_NONE:
      nhshell_window_set_fullscreen (sh, out->window, 0);
      nhshell_window_set_maximized (sh, out->window, 0);
      store_frame_param (f, Qfullscreen, Qnil);
      break;

    case FULLSCREEN_BOTH:
      nhshell_window_set_maximized (sh, out->window, 0);
      nhshell_window_set_fullscreen (sh, out->window, 1);
      store_frame_param (f, Qfullscreen, Qfullboth);
      break;

    case FULLSCREEN_MAXIMIZED:
      nhshell_window_set_fullscreen (sh, out->window, 0);
      nhshell_window_set_maximized (sh, out->window, 1);
      store_frame_param (f, Qfullscreen, Qmaximized);
      break;

    case FULLSCREEN_WIDTH:
    case FULLSCREEN_HEIGHT:
      nhshell_window_set_fullscreen (sh, out->window, 0);
      break;
    }
  unblock_input ();

  f->want_fullscreen = FULLSCREEN_NONE;
}

static void
nhwl_fullscreen_hook (struct frame *f)
{
  if (FRAME_VISIBLE_P (f))
    nhwl_set_fullscreen_state (f);
}

/* Adapted from pgtk_free_frame_resources.  */
void
nhwl_free_frame_resources (struct frame *f)
{
  struct nhwl_display_info *dpyinfo;
  struct nhwl_output *out = FRAME_NHWL_OUTPUT (f);
  Mouse_HLInfo *hlinfo;

  check_window_system (f);
  dpyinfo = FRAME_DISPLAY_INFO (f);
  hlinfo = MOUSE_HL_INFO (f);

  block_input ();

  free_frame_faces (f);

#define CLEAR_IF_EQ(FIELD)	\
  do { if (f == dpyinfo->FIELD) dpyinfo->FIELD = 0; } while (false)

  if (f == dpyinfo->x_focus_frame)
    {
      /* Clear the highlight first: the faces are gone.  */
      if (dpyinfo->highlight_frame == f)
	dpyinfo->highlight_frame = NULL;
      dpyinfo->x_focus_frame = NULL;
      nhwl_frame_rehighlight (dpyinfo);
    }

  CLEAR_IF_EQ (highlight_frame);
  CLEAR_IF_EQ (x_focus_event_frame);
  CLEAR_IF_EQ (last_mouse_frame);
  CLEAR_IF_EQ (last_mouse_motion_frame);
  CLEAR_IF_EQ (last_mouse_glyph_frame);

#undef CLEAR_IF_EQ

  if (f == hlinfo->mouse_face_mouse_frame)
    reset_mouse_highlight (hlinfo);

  /* Unlink from the display's chain.  */
  {
    struct nhwl_output **p;

    for (p = &dpyinfo->outputs; *p; p = &(*p)->next_output)
      if (*p == out)
	{
	  *p = out->next_output;
	  break;
	}
  }

  if (out->window && dpyinfo->shell)
    nhshell_window_destroy (dpyinfo->shell, out->window);
  out->window = 0;

  nhwl_cr_destroy_frame_context (f);
  nhwl_glitnir_forget (f);

  xfree (f->output_data.nhwl);
  f->output_data.nhwl = NULL;

  unblock_input ();
}

/* Adapted from pgtk_destroy_window.  */
void
nhwl_destroy_window (struct frame *f)
{
  struct nhwl_display_info *dpyinfo = FRAME_DISPLAY_INFO (f);

  check_window_system (f);
  if (dpyinfo->shell != NULL)
    nhwl_free_frame_resources (f);

  dpyinfo->reference_count--;
}

/* Adapted from pgtk_get_focus_frame.  */
Lisp_Object
nhwl_get_focus_frame (struct frame *frame)
{
  struct nhwl_display_info *dpyinfo = FRAME_DISPLAY_INFO (frame);
  Lisp_Object focus;

  if (!dpyinfo->x_focus_frame)
    return Qnil;

  XSETFRAME (focus, dpyinfo->x_focus_frame);
  return focus;
}

/* Adapted from pgtk_focus_frame.  Wayland gives a client no way to take
   the focus: the most it can do is ask the compositor to activate the
   window (xdg-activation, with a token requested from the latest input
   serial), which the compositor may refuse.  */
void
nhwl_focus_frame (struct frame *f, bool noactivate)
{
  struct nhwl_output *out = FRAME_NHWL_OUTPUT (f);

  if (noactivate || !out->window)
    return;
  block_input ();
  nhshell_window_activate (FRAME_NHWL_SHELL (f), out->window, NULL);
  unblock_input ();
}


/***********************************************************************
				 Focus
 ***********************************************************************/

/* Adapted from frame_highlight and frame_unhighlight (pgtkterm.c): the
   frame has no border to colour, only the cursor changes.  */
static void
frame_highlight (struct frame *f)
{
  gui_update_cursor (f, true);
}

static void
frame_unhighlight (struct frame *f)
{
  gui_update_cursor (f, true);
}

/* Adapted from pgtk_frame_rehighlight.  */
void
nhwl_frame_rehighlight (struct nhwl_display_info *dpyinfo)
{
  struct frame *old_highlight = dpyinfo->highlight_frame;

  if (dpyinfo->x_focus_frame)
    {
      dpyinfo->highlight_frame
	= ((FRAMEP (FRAME_FOCUS_FRAME (dpyinfo->x_focus_frame)))
	   ? XFRAME (FRAME_FOCUS_FRAME (dpyinfo->x_focus_frame))
	   : dpyinfo->x_focus_frame);
      if (!FRAME_LIVE_P (dpyinfo->highlight_frame))
	{
	  fset_focus_frame (dpyinfo->x_focus_frame, Qnil);
	  dpyinfo->highlight_frame = dpyinfo->x_focus_frame;
	}
    }
  else
    dpyinfo->highlight_frame = 0;

  if (old_highlight)
    frame_unhighlight (old_highlight);
  if (dpyinfo->highlight_frame)
    frame_highlight (dpyinfo->highlight_frame);
}

static void
nhwl_frame_rehighlight_hook (struct frame *frame)
{
  nhwl_frame_rehighlight (FRAME_DISPLAY_INFO (frame));
}

/* Adapted from pgtk_new_focus_frame.  */
static void
nhwl_new_focus_frame (struct nhwl_display_info *dpyinfo, struct frame *frame)
{
  if (frame != dpyinfo->x_focus_frame)
    dpyinfo->x_focus_frame = frame;

  nhwl_frame_rehighlight (dpyinfo);
}

/* Adapted from pgtk_focus_changed.  */
static void
nhwl_focus_changed (bool is_enter, int state,
		    struct nhwl_display_info *dpyinfo, struct frame *frame,
		    union buffered_input_event *bufp)
{
  if (is_enter)
    {
      if (dpyinfo->x_focus_event_frame != frame)
	{
	  nhwl_new_focus_frame (dpyinfo, frame);
	  dpyinfo->x_focus_event_frame = frame;

	  bufp->ie.arg = (((NILP (Vterminal_frame)
			    || !FRAME_NHWL_P (XFRAME (Vterminal_frame))
			    || IS_DAEMON)
			   && CONSP (Vframe_list)
			   && !NILP (XCDR (Vframe_list))) ? Qt : Qnil);
	  bufp->ie.kind = FOCUS_IN_EVENT;
	  XSETFRAME (bufp->ie.frame_or_window, frame);
	}

      frame->output_data.nhwl->focus_state |= state;
    }
  else
    {
      frame->output_data.nhwl->focus_state &= ~state;

      if (dpyinfo->x_focus_event_frame == frame)
	{
	  dpyinfo->x_focus_event_frame = 0;
	  nhwl_new_focus_frame (dpyinfo, NULL);

	  bufp->ie.kind = FOCUS_OUT_EVENT;
	  XSETFRAME (bufp->ie.frame_or_window, frame);
	}
    }
}


/***********************************************************************
				Events
 ***********************************************************************/

/* The Emacs modifier bits for the shell's modifier mask MODS.  */
static int
nhwl_emacs_modifiers (uint32_t mods)
{
  int mod = 0;
  Lisp_Object tem;
  int mod_alt = meta_modifier, mod_meta = meta_modifier;
  int mod_super = super_modifier, mod_hyper = hyper_modifier;

  tem = Fget (Vnhwl_alt_modifier, Qmodifier_value);
  if (FIXNUMP (tem))
    mod_alt = XFIXNUM (tem) & INT_MAX;
  tem = Fget (Vnhwl_meta_modifier, Qmodifier_value);
  if (FIXNUMP (tem))
    mod_meta = XFIXNUM (tem) & INT_MAX;
  tem = Fget (Vnhwl_super_modifier, Qmodifier_value);
  if (FIXNUMP (tem))
    mod_super = XFIXNUM (tem) & INT_MAX;
  tem = Fget (Vnhwl_hyper_modifier, Qmodifier_value);
  if (FIXNUMP (tem))
    mod_hyper = XFIXNUM (tem) & INT_MAX;

  if (mods & NHSHELL_MOD_SHIFT)
    mod |= shift_modifier;
  if (mods & NHSHELL_MOD_CTRL)
    mod |= ctrl_modifier;
  if (mods & NHSHELL_MOD_ALT)
    mod |= mod_alt;
  if (mods & NHSHELL_MOD_META)
    mod |= mod_meta;
  if (mods & NHSHELL_MOD_SUPER)
    mod |= mod_super;
  if (mods & NHSHELL_MOD_HYPER)
    mod |= mod_hyper;
  return mod;
}

#define IsCursorKey(keysym)       (0xff50 <= (keysym) && (keysym) < 0xff60)
#define IsMiscFunctionKey(keysym) (0xff60 <= (keysym) && (keysym) < 0xff6c)
#define IsKeypadKey(keysym)       (0xff80 <= (keysym) && (keysym) < 0xffbe)
#define IsFunctionKey(keysym)     (0xffbe <= (keysym) && (keysym) < 0xffe1)
#define IsModifierKey(keysym)						\
  ((((keysym) >= 0xffe1) && ((keysym) <= 0xffee))			\
   || (((keysym) >= 0xfe01) && ((keysym) <= 0xfe13))			\
   || ((keysym) == 0xff7e)						\
   || ((keysym) == 0xff7f))

/* Convert the key event EV on frame F into an input event.  Adapted
   from key_press_event (pgtkterm.c): the shell has already run the
   keysym through xkb and compose, so EV carries the keysym and the text
   the key produced.  Return true if INEV holds an event.  */
static bool
nhwl_key_event (struct frame *f, const NhShellEvent *ev,
		union buffered_input_event *inev)
{
  struct nhwl_display_info *dpyinfo = FRAME_DISPLAY_INFO (f);
  uint32_t keysym = ev->u.key.keysym;
  Mouse_HLInfo *hlinfo = MOUSE_HL_INFO (f);
  Lisp_Object c;
  size_t nbytes = strlen (ev->u.key.utf8);
  int modifiers;

  if (!ev->u.key.pressed)
    return false;

  /* If mouse-highlight is an integer, input clears out mouse
     highlighting.  */
  if (!hlinfo->mouse_face_hidden && FIXNUMP (Vmouse_highlight))
    {
      clear_mouse_face (hlinfo);
      hlinfo->mouse_face_hidden = true;
    }

  dpyinfo->mods = ev->u.key.mods;
  modifiers = nhwl_emacs_modifiers (ev->u.key.mods);
  modifiers |= extra_keyboard_modifiers & (shift_modifier | ctrl_modifier
					   | meta_modifier | alt_modifier
					   | super_modifier | hyper_modifier);

  XSETFRAME (inev->ie.frame_or_window, f);
  inev->ie.modifiers = modifiers;
  inev->ie.timestamp = ev->time_ns / 1000000;
  /* The frame-time record's start: our clock, not the compositor's.  */
  if (!FRAME_NHWL_OUTPUT (f)->t_key_ns)
    FRAME_NHWL_OUTPUT (f)->t_key_ns = nhwl_now_ns ();

  /* make_lispy_event turns characters into control characters: the
     keysym is the key's, not the control character's.  */

  /* First deal with keysyms which have defined translations to
     characters.  */
  if (keysym >= 32 && keysym < 128)
    {
      inev->ie.kind = ASCII_KEYSTROKE_EVENT;
      inev->ie.code = keysym;
      return true;
    }

  /* Keysyms directly mapped to Unicode characters.  */
  if (keysym >= 0x01000000 && keysym <= 0x0110FFFF)
    {
      if (keysym < 0x01000080)
	inev->ie.kind = ASCII_KEYSTROKE_EVENT;
      else
	inev->ie.kind = MULTIBYTE_CHAR_KEYSTROKE_EVENT;
      inev->ie.code = keysym & 0xFFFFFF;
      return true;
    }

  /* Now non-ASCII.  */
  if (HASH_TABLE_P (Vnhwl_keysym_table)
      && (c = Fgethash (make_fixnum (keysym), Vnhwl_keysym_table, Qnil),
	  FIXNATP (c)))
    {
      inev->ie.kind = (SINGLE_BYTE_CHAR_P (XFIXNAT (c))
		       ? ASCII_KEYSTROKE_EVENT
		       : MULTIBYTE_CHAR_KEYSTROKE_EVENT);
      inev->ie.code = XFIXNAT (c);
      return true;
    }

  /* Random non-modifier sorts of keysyms.  */
  if (((keysym >= 0xff08 && keysym <= 0xff1b)	/* BackSpace .. Escape */
       || keysym == 0xffff			/* Delete */
       || (keysym >= 0xfe20 && keysym <= 0xfe34) /* ISO_Left_Tab .. ISO_Enter */
       || IsCursorKey (keysym)
       || IsMiscFunctionKey (keysym)
       || (keysym >= 0xfe50 && keysym <= 0xfe5c) /* the dead keys Emacs knows */
       || IsKeypadKey (keysym)
       || IsFunctionKey (keysym)
       /* Any "vendor-specific" key is ok.  */
       || (keysym & (1 << 28))
       || (keysym != 0xffffff && nbytes == 0))
      && !(IsModifierKey (keysym)
	   || (keysym >= 0xfe01 && keysym <= 0xfe13)))
    {
      /* make_lispy_event will convert this to a symbolic key.  */
      inev->ie.kind = NON_ASCII_KEYSTROKE_EVENT;
      inev->ie.code = keysym;
      return true;
    }

  if (nbytes == 0)
    return false;

  /* A key that produced text: one multibyte character.  */
  {
    const unsigned char *p = (const unsigned char *) ev->u.key.utf8;
    int len;
    int ch = string_char_and_length (p, &len);

    inev->ie.kind = (ch < 128 ? ASCII_KEYSTROKE_EVENT
		     : MULTIBYTE_CHAR_KEYSTROKE_EVENT);
    inev->ie.code = ch;
  }
  return true;
}

/* evdev button codes to Emacs mouse button numbers (0 left, 1 middle,
   2 right, 7 back, 8 forward).  */
static int
nhwl_button_code (uint32_t button)
{
  switch (button)
    {
    case 0x110: return 0;	/* BTN_LEFT */
    case 0x112: return 1;	/* BTN_MIDDLE */
    case 0x111: return 2;	/* BTN_RIGHT */
    case 0x113: case 0x116: return 7;	/* BTN_SIDE, BTN_BACK */
    case 0x114: case 0x115: return 8;	/* BTN_EXTRA, BTN_FORWARD */
    default: return -1;
    }
}

/* Whether the mouse moved off the glyph it was on at the last sighting.
   Adapted from note_mouse_movement (pgtkterm.c).  */
static bool
nhwl_note_mouse_movement (struct frame *frame, int x, int y, Time time)
{
  XRectangle *r;
  struct nhwl_display_info *dpyinfo;

  if (!FRAME_X_OUTPUT (frame))
    return false;

  dpyinfo = FRAME_DISPLAY_INFO (frame);
  dpyinfo->last_mouse_movement_time = time;
  dpyinfo->last_mouse_motion_frame = frame;
  dpyinfo->last_mouse_motion_x = x;
  dpyinfo->last_mouse_motion_y = y;

  /* Has the mouse moved off the glyph it was on at the last sighting?  */
  r = &dpyinfo->last_mouse_glyph;
  if (frame != dpyinfo->last_mouse_glyph_frame
      || x < r->x || x >= r->x + (int) r->width
      || y < r->y || y >= r->y + (int) r->height)
    {
      frame->mouse_moved = true;
      dpyinfo->last_mouse_scroll_bar = NULL;
      note_mouse_highlight (frame, x, y);
      /* Remember which glyph we're now on.  */
      remember_mouse_glyph (frame, x, y, r);
      dpyinfo->last_mouse_glyph_frame = frame;
      return true;
    }

  return false;
}

/* The shell reported a new preferred scale for frame F: the display's
   resolution follows (96 dpi times the scale), and the fonts are
   re-opened at the device size as on a move between monitors.  */
static void
nhwl_scale_changed (struct frame *f, uint32_t scale120)
{
  struct nhwl_output *out = FRAME_NHWL_OUTPUT (f);
  struct nhwl_display_info *dpyinfo = FRAME_DISPLAY_INFO (f);

  uint32_t old = out->scale120 ? out->scale120 : 120;

  if (!scale120 || scale120 == out->scale120)
    return;

  out->scale120 = scale120;
  dpyinfo->scale120 = scale120;
  dpyinfo->resx = dpyinfo->resy = 96.0 * scale120 / 120.0;

  /* The fonts are re-opened at the new device size: the pixel size of
     the frame's font times the ratio of the scales, through the frame's
     font parameter.  */
  if (FRAME_FONT (f) && FIXNUMP (FRAME_FONT (f)->props[FONT_SIZE_INDEX]))
    {
      struct font *font = FRAME_FONT (f);
      Lisp_Object spec = font_spec_from_name (font->props[FONT_NAME_INDEX]);

      if (!NILP (spec))
	{
	  double px = XFIXNUM (font->props[FONT_SIZE_INDEX]);
	  Lisp_Object font_object;

	  ASET (spec, FONT_SIZE_INDEX,
		make_fixnum (max (1, lround (px * scale120 / old))));
	  font_object = font_open_by_spec (f, spec);
	  if (!NILP (font_object))
	    gui_set_font (f, font_object, get_frame_param (f, Qfont));
	}
    }
  nhwl_csd_layout (f);
  SET_FRAME_GARBAGED (f);
}

/* Handle CONFIGURE for frame F: the compositor's suggestion of a size
   (0: ours), the toplevel's states, and the serial to ack.  */
static void
nhwl_configure (struct frame *f, const NhShellEvent *ev)
{
  struct nhwl_output *out = FRAME_NHWL_OUTPUT (f);
  uint32_t states = ev->u.configure.states;
  Lisp_Object fs = Qnil;

  /* A content ack (only a buffer carries it): the first configure, a new
     size, a client-side title bar's state change.  */
  uint32_t ack_flags = out->configured ? 0 : NHSHELL_ACK_CONTENT;

  out->logical_w = ev->u.configure.w;
  out->logical_h = ev->u.configure.h;
  out->serial = ev->u.configure.serial;
  out->configured = true;
  if (out->csd.on
      && ((out->states ^ states)
	  & (NHSHELL_STATE_ACTIVATED | NHSHELL_STATE_MAXIMIZED
	     | NHSHELL_STATE_TILED_L | NHSHELL_STATE_TILED_R
	     | NHSHELL_STATE_TILED_T | NHSHELL_STATE_TILED_B)))
    ack_flags = NHSHELL_ACK_CONTENT;

  if (states & NHSHELL_STATE_FULLSCREEN)
    fs = Qfullboth;
  else if (states & NHSHELL_STATE_MAXIMIZED)
    fs = Qmaximized;
  if ((out->states ^ states)
      & (NHSHELL_STATE_FULLSCREEN | NHSHELL_STATE_MAXIMIZED))
    store_frame_param (f, Qfullscreen, fs);
  out->states = states;
  nhwl_csd_layout (f);

  /* The compositor's size is a suggestion in logical pixels, for the window
     geometry (the native area, and the title strip when we draw one); the
     frame keeps its own size when it already is that size once rounded.  */
  if (ev->u.configure.w > 0 && ev->u.configure.h > 0)
    {
      int gw, gh, dw, dh;

      nhwl_csd_geometry_size (f, &gw, &gh);
      if (ev->u.configure.w == gw && ev->u.configure.h == gh)
	goto done;
      nhwl_csd_native_size (f, ev->u.configure.w, ev->u.configure.h,
			    &dw, &dh);
      nhwl_clamp_native_size (f, &dw, &dh);
      change_frame_size (f, dw, dh, false, true, false);
      SET_FRAME_GARBAGED (f);
      ack_flags = NHSHELL_ACK_CONTENT;
    }
 done:

  /* The shell records the ack and sends it with the commit that carries it.  */
  nhshell_window_ack_configure (FRAME_DISPLAY_INFO (f)->shell, out->window,
				out->serial, ack_flags);
  nhwl_dirty (f);
  if (FRAME_VISIBLE_P (f))
    fset_redisplay (f);
}

/* One scroll record of frame F, as pgtk's scroll_event turns GDK's:
   wheel records add up to whole 120-unit notches (the fraction stays in
   the display's accumulator, which a new frame, source or direction
   resets); finger and continuous records carry their pixel delta in device
   pixels, for `pixel-scroll-precision-mode'; with
   `mwheel-coalesce-scroll-events' they add up to whole lines instead; a
   stop record is a TOUCH_END_EVENT (momentum).  Return the number of
   events stored.  */
static int
nhwl_scroll_record (struct frame *f, const NhShellEvent *ev,
		    struct input_event *hold_quit)
{
  struct nhwl_display_info *d = FRAME_DISPLAY_INFO (f);
  double scale = nhwl_frame_scale (f);
  double px = ev->u.scroll.dx * scale, py = ev->u.scroll.dy * scale;
  bool wheel = ev->u.scroll.v120x || ev->u.scroll.v120y;
  bool horizontal = wheel ? ev->u.scroll.v120x != 0 : fabs (px) > fabs (py);
  double *acc = horizontal ? &d->scroll.acc_x : &d->scroll.acc_y;
  double *other = horizontal ? &d->scroll.acc_y : &d->scroll.acc_x;
  double delta = wheel ? (horizontal ? ev->u.scroll.v120x : ev->u.scroll.v120y)
		       : (horizontal ? px : py);
  double unit = wheel ? 120.0
    : (horizontal ? FRAME_COLUMN_WIDTH (f) : FRAME_LINE_HEIGHT (f));
  union buffered_input_event inev;
  int n, i;

  EVENT_INIT (inev.ie);
  inev.ie.arg = Qnil;
  inev.ie.modifiers = nhwl_emacs_modifiers (d->mods);
  XSETINT (inev.ie.x, (int) d->last_mouse_motion_x);
  XSETINT (inev.ie.y, (int) d->last_mouse_motion_y);
  XSETFRAME (inev.ie.frame_or_window, f);
  inev.ie.timestamp = ev->time_ns / 1000000;

  if (ev->u.scroll.stop)
    {
      d->scroll.acc_x = d->scroll.acc_y = 0;
      inev.ie.kind = TOUCH_END_EVENT;
      kbd_buffer_store_event_hold (&inev.ie, hold_quit);
      return 1;
    }
  if (delta == 0)
    return 0;

  if (d->scroll.frame != f || d->scroll.source != ev->u.scroll.source
      || (*acc != 0 && (*acc < 0) != (delta < 0)))
    *acc = *other = 0;
  d->scroll.frame = f;
  d->scroll.source = ev->u.scroll.source;
  inev.ie.kind = horizontal ? HORIZ_WHEEL_EVENT : WHEEL_EVENT;
  inev.ie.modifiers |= delta < 0 ? up_modifier : down_modifier;

  if (!wheel && !mwheel_coalesce_scroll_events)
    {
      /* This record's pixel delta, for `pixel-scroll-precision', which
	 (as on X) takes a negative delta for wheel-down.  */
      inev.ie.arg = list3 (Qnil, make_float (-px), make_float (-py));
      kbd_buffer_store_event_hold (&inev.ie, hold_quit);
      return 1;
    }

  *acc += delta;
  n = (int) (fabs (*acc) / unit);
  if (n == 0)
    return 0;
  *acc -= (delta < 0 ? -n : n) * unit;
  if (mwheel_coalesce_scroll_events)
    {
      inev.ie.arg = list3 (make_fixnum (n), make_float (-px), make_float (-py));
      kbd_buffer_store_event_hold (&inev.ie, hold_quit);
      return 1;
    }
  /* A wheel without coalescing: one event per notch.  */
  for (i = 0; i < n; i++)
    {
      inev.ie.arg = list3 (Qnil, make_float (-px / n), make_float (-py / n));
      kbd_buffer_store_event_hold (&inev.ie, hold_quit);
    }
  return n;
}

/* The window of O is no longer on the output ID.  */
static void
nhwl_output_left (struct nhwl_output *o, uint64_t id)
{
  int i;

  for (i = 0; i < o->n_entered; i++)
    if (o->entered[i] == id)
      {
	o->entered[i] = o->entered[--o->n_entered];
	break;
      }
}

/* Make INEV the MONITORS_CHANGED_EVENT of DPYINFO's terminal (one per
   change: Lisp runs display-monitors-changed-functions for it).  */
static bool
nhwl_monitors_changed (struct nhwl_display_info *dpyinfo,
		       union buffered_input_event *inev)
{
  inev->ie.kind = MONITORS_CHANGED_EVENT;
  XSETTERMINAL (inev->ie.arg, dpyinfo->terminal);
  return true;
}

/* Drain DPYINFO's shell events into the keyboard buffer (through
   HOLD_QUIT where Emacs gave one); return the number of events stored.
   Callers hold block_input.  */
int
nhwl_drain_events (struct nhwl_display_info *dpyinfo,
		   struct input_event *hold_quit)
{
  NhShellEvent ev;
  int count = 0;
  int rc;

  if (!dpyinfo->shell)
    return 0;

  block_input ();
  for (;;)
    {
      union buffered_input_event inev;
      struct frame *f;
      bool have = false;
      bool csd_close = false;

      rc = nhshell_next_event (dpyinfo->shell, &ev);
      if (rc <= 0)
	break;

      EVENT_INIT (inev.ie);
      inev.ie.kind = NO_EVENT;
      inev.ie.arg = Qnil;
      f = ev.window ? nhwl_window_to_frame (dpyinfo, ev.window) : NULL;

      switch (ev.kind)
	{
	case NHSHELL_EV_KEY:
	  /* A key repeat behind input Lisp has not read yet is dropped.
	     The shell holds back a repeat only while the previous one is
	     in its own queue, and the input poll empties that queue into
	     ours even while Lisp is busy, so without this a stalled Lisp
	     would find one repeat per poll.  */
	  if (ev.u.key.repeat && kbd_buffer_events_waiting ())
	    break;
	  if (!f)
	    f = dpyinfo->x_focus_frame;
	  if (f)
	    have = nhwl_key_event (f, &ev, &inev);
	  break;

	case NHSHELL_EV_MODIFIERS:
	  dpyinfo->mods = ev.u.key.mods;
	  break;

	case NHSHELL_EV_POINTER_ENTER:
	case NHSHELL_EV_POINTER_MOTION:
	  if (f)
	    {
	      struct nhwl_display_info *d = FRAME_DISPLAY_INFO (f);
	      Mouse_HLInfo *hlinfo = MOUSE_HL_INFO (f);
	      int x, y;

	      /* A pointer hidden while typing comes back with the mouse.  */
	      if (f->pointer_invisible)
		nhwl_toggle_invisible_pointer (f, false);
	      if (nhwl_csd_pointer (f, ev.u.pointer.x, ev.u.pointer.y))
		{
		  clear_mouse_face (hlinfo);
		  break;
		}
	      nhwl_csd_native_pos (f, ev.u.pointer.x, ev.u.pointer.y, &x, &y);
	      d->last_mouse_frame = f;
	      previous_help_echo_string = help_echo_string;
	      help_echo_string = Qnil;
	      if (hlinfo->mouse_face_hidden)
		{
		  hlinfo->mouse_face_hidden = false;
		  clear_mouse_face (hlinfo);
		}
	      if (!nhwl_note_mouse_movement (f, x, y, ev.time_ns / 1000000))
		help_echo_string = previous_help_echo_string;
	      if (!NILP (help_echo_string)
		  || !NILP (previous_help_echo_string))
		{
		  Lisp_Object frame;

		  XSETFRAME (frame, f);
		  any_help_event_p = true;
		  gen_help_event (help_echo_string, frame, help_echo_window,
				  help_echo_object, help_echo_pos);
		}
	    }
	  break;

	case NHSHELL_EV_POINTER_LEAVE:
	  if (f)
	    {
	      Mouse_HLInfo *hlinfo = MOUSE_HL_INFO (f);

	      clear_mouse_face (hlinfo);
	      if (FRAME_DISPLAY_INFO (f)->last_mouse_frame == f)
		FRAME_DISPLAY_INFO (f)->last_mouse_frame = NULL;
	    }
	  break;

	case NHSHELL_EV_BUTTON:
	  if (f && nhwl_csd_button (f, &ev, &csd_close))
	    {
	      if (csd_close)
		{
		  inev.ie.kind = DELETE_WINDOW_EVENT;
		  XSETFRAME (inev.ie.frame_or_window, f);
		  have = true;
		  csd_close = false;
		}
	      break;
	    }
	  if (f)
	    {
	      int code = nhwl_button_code (ev.u.pointer.button);
	      bool down = ev.u.pointer.state == NHSHELL_BUTTON_PRESSED;

	      if (code >= 0)
		{
		  int x = FRAME_DISPLAY_INFO (f)->last_mouse_motion_x;
		  int y = FRAME_DISPLAY_INFO (f)->last_mouse_motion_y;

		  inev.ie.kind = MOUSE_CLICK_EVENT;
		  inev.ie.code = code;
		  inev.ie.modifiers
		    = (nhwl_emacs_modifiers (FRAME_DISPLAY_INFO (f)->mods)
		       | (down ? down_modifier : up_modifier));
		  XSETINT (inev.ie.x, x);
		  XSETINT (inev.ie.y, y);
		  XSETFRAME (inev.ie.frame_or_window, f);
		  inev.ie.timestamp = ev.time_ns / 1000000;
		  FRAME_DISPLAY_INFO (f)->last_mouse_frame = f;
		  have = true;
		}
	    }
	  break;

	case NHSHELL_EV_SCROLL:
	  if (f)
	    count += nhwl_scroll_record (f, &ev, hold_quit);
	  break;

	case NHSHELL_EV_CONFIGURE:
	  if (f)
	    nhwl_configure (f, &ev);
	  break;

	case NHSHELL_EV_CLOSE:
	  if (f)
	    {
	      inev.ie.kind = DELETE_WINDOW_EVENT;
	      XSETFRAME (inev.ie.frame_or_window, f);
	      have = true;
	    }
	  break;

	case NHSHELL_EV_SCALE:
	  if (f)
	    nhwl_scale_changed (f, ev.u.scale.scale120);
	  break;

	case NHSHELL_EV_FOCUS:
	  if (f)
	    {
	      nhwl_focus_changed (ev.u.focus.in != 0, FOCUS_EXPLICIT, dpyinfo,
				  f, &inev);
	      have = inev.ie.kind != NO_EVENT;
	    }
	  break;

	case NHSHELL_EV_FRAME_READY:
	  if (f)
	    nhwl_present (f);
	  break;

	case NHSHELL_EV_FRAME_DONE:
	  break;

	case NHSHELL_EV_SELECTION:
	  have = nhwl_selection_changed (dpyinfo, ev.u.selection.kind,
					 &inev.ie);
	  break;

	case NHSHELL_EV_POPUP_DONE:
	  /* Only tip frames are popups.  */
	  if (f && FRAME_TOOLTIP_P (f))
	    nhwl_tip_dismissed (f);
	  break;

	case NHSHELL_EV_OUTPUT_ADD:
	case NHSHELL_EV_OUTPUT_CHANGE:
	  {
	    int i;

	    for (i = 0; i < dpyinfo->n_monitors; i++)
	      if (dpyinfo->monitors[i].id == ev.u.output.id)
		break;
	    if (i == dpyinfo->n_monitors)
	      {
		if (i == NHWL_MAX_OUTPUTS)
		  break;
		dpyinfo->n_monitors++;
	      }
	    dpyinfo->monitors[i].id = ev.u.output.id;
	    dpyinfo->monitors[i].x = ev.u.output.x;
	    dpyinfo->monitors[i].y = ev.u.output.y;
	    /* The output's mode, in device pixels.  */
	    dpyinfo->monitors[i].w = ev.u.output.w;
	    dpyinfo->monitors[i].h = ev.u.output.h;
	    dpyinfo->monitors[i].mm_w = ev.u.output.mm_w;
	    dpyinfo->monitors[i].mm_h = ev.u.output.mm_h;
	    dpyinfo->monitors[i].scale120 = ev.u.output.scale120;
	    memcpy (dpyinfo->monitors[i].name, ev.u.output.name,
		    sizeof dpyinfo->monitors[i].name);
	    dpyinfo->monitors[i].name[sizeof dpyinfo->monitors[i].name - 1]
	      = '\0';
	    if (dpyinfo->scale120 == 0 && ev.u.output.scale120)
	      {
		dpyinfo->scale120 = ev.u.output.scale120;
		dpyinfo->resx = dpyinfo->resy
		  = 96.0 * dpyinfo->scale120 / 120.0;
	      }
	    have = nhwl_monitors_changed (dpyinfo, &inev);
	  }
	  break;

	case NHSHELL_EV_OUTPUT_REMOVE:
	  {
	    int i;

	    for (i = 0; i < dpyinfo->n_monitors; i++)
	      if (dpyinfo->monitors[i].id == ev.u.output.id)
		{
		  struct nhwl_output *o;

		  dpyinfo->monitors[i]
		    = dpyinfo->monitors[--dpyinfo->n_monitors];
		  for (o = dpyinfo->outputs; o; o = o->next_output)
		    nhwl_output_left (o, ev.u.output.id);
		  have = nhwl_monitors_changed (dpyinfo, &inev);
		  break;
		}
	  }
	  break;

	case NHSHELL_EV_ENTER_OUTPUT:
	  if (f && FRAME_NHWL_OUTPUT (f)->n_entered < NHWL_MAX_OUTPUTS)
	    {
	      struct nhwl_output *o = FRAME_NHWL_OUTPUT (f);
	      int i;

	      for (i = 0; i < o->n_entered; i++)
		if (o->entered[i] == ev.u.output.id)
		  break;
	      if (i == o->n_entered)
		o->entered[o->n_entered++] = ev.u.output.id;
	    }
	  break;

	case NHSHELL_EV_LEAVE_OUTPUT:
	  if (f)
	    nhwl_output_left (FRAME_NHWL_OUTPUT (f), ev.u.output.id);
	  break;

	case NHSHELL_EV_DISCONNECTED:
	  /* Handled after the loop, once the queue is empty.  */
	  dpyinfo->connection_lost = true;
	  break;

	default:
	  break;
	}

      if (have && inev.ie.kind != NO_EVENT)
	{
	  kbd_buffer_store_event_hold (&inev.ie, hold_quit);
	  count++;
	}
    }

  if (rc == NHSHELL_E_DISCONNECTED)
    dpyinfo->connection_lost = true;

  /* Flush what we sent while handling the events.  */
  if (rc != NHSHELL_E_DISCONNECTED)
    nhshell_dispatch_main (dpyinfo->shell);
  unblock_input ();
  return count;
}

/* The terminal's read_socket_hook.  */
static int
nhwl_read_socket (struct terminal *terminal, struct input_event *hold_quit)
{
  struct nhwl_display_info *dpyinfo = terminal->display_info.nhwl;
  int n;

  if (!dpyinfo || !dpyinfo->shell)
    return 0;
  n = nhwl_drain_events (dpyinfo, hold_quit);
  if (dpyinfo->connection_lost)
    {
      /* Frees DPYINFO and clears TERMINAL's pointer to it.  gobble_input
	 calls this hook again while it returns more than 0, so the events
	 this drain stored are not counted.  */
      nhwl_connection_lost (dpyinfo);
      return 0;
    }
  return n;
}

/* No popup menus in W0 (nil hooks).  */
int
popup_activated (void)
{
  return 0;
}

/* A Wayland client cannot move the pointer (adapted from pgtkterm.c).  */
void
frame_set_mouse_pixel_position (struct frame *f, int pix_x, int pix_y)
{
}

/* The name of a keysym the key tables do not know: none (the shell gives
   keysyms as numbers; Emacs falls back to its own naming).  */
char *
get_keysym_name (int keysym)
{
  return NULL;
}

/* Adapted from pgtk_mouse_position: the shell reports positions with
   its events, so the answer is the last one we saw.  */
static void
nhwl_mouse_position (struct frame **fp, int insist, Lisp_Object *bar_window,
		     enum scroll_bar_part *part, Lisp_Object *x,
		     Lisp_Object *y, Time *timestamp)
{
  struct frame *f1;
  struct nhwl_display_info *dpyinfo = FRAME_DISPLAY_INFO (*fp);
  Lisp_Object frame, tail;

  block_input ();

  /* Clear the mouse-moved flag for every frame on this display.  */
  FOR_EACH_FRAME (tail, frame)
    if (FRAME_NHWL_P (XFRAME (frame))
	&& FRAME_DISPLAY_INFO (XFRAME (frame)) == dpyinfo)
      XFRAME (frame)->mouse_moved = false;

  dpyinfo->last_mouse_scroll_bar = NULL;

  f1 = dpyinfo->last_mouse_motion_frame;
  if (f1 == NULL || !FRAME_NHWL_P (f1))
    {
      unblock_input ();
      return;
    }

  remember_mouse_glyph (f1, dpyinfo->last_mouse_motion_x,
			dpyinfo->last_mouse_motion_y,
			&dpyinfo->last_mouse_glyph);
  dpyinfo->last_mouse_glyph_frame = f1;

  *bar_window = Qnil;
  *part = 0;
  *fp = f1;
  XSETINT (*x, dpyinfo->last_mouse_motion_x);
  XSETINT (*y, dpyinfo->last_mouse_motion_y);
  *timestamp = dpyinfo->last_mouse_movement_time;

  unblock_input ();
}


/***********************************************************************
			       The terminal
 ***********************************************************************/

static void
nhwl_update_begin (struct frame *f)
{
  nhwl_clear_under_internal_border (f);
}

static void
nhwl_update_end (struct frame *f)
{
  /* Mouse highlight may be displayed again.  */
  MOUSE_HL_INFO (f)->mouse_face_defer = false;
}

/* Present what the redisplay drew.  Adapted from pgtk_frame_up_to_date.  */
static void
nhwl_frame_up_to_date (struct frame *f)
{
  block_input ();
  FRAME_MOUSE_UPDATE (f);
  if (!buffer_flipping_blocked_p () && FRAME_VISIBLE_P (f))
    nhwl_dirty (f);
  unblock_input ();
}

static void
nhwl_buffer_flipping_unblocked_hook (struct frame *f)
{
  if (FRAME_VISIBLE_P (f))
    nhwl_dirty (f);
}

static void
nhwl_ring_bell (struct frame *f)
{
}

static void
nhwl_toggle_invisible_pointer (struct frame *f, bool invisible)
{
  struct nhwl_output *out = FRAME_NHWL_OUTPUT (f);

  if (out->window)
    {
      block_input ();
      nhshell_window_set_cursor (FRAME_NHWL_SHELL (f), out->window,
				 invisible
				 ? NHSHELL_CURSOR_HIDDEN
				 : (out->current_cursor
				    ? (uint32_t) ((intptr_t) out->current_cursor
						  - 1)
				    : NHSHELL_CURSOR_DEFAULT));
      unblock_input ();
    }
  f->pointer_invisible = invisible;
}

/* The shell's own fonts and sizes: scroll bar widths come from the
   font, as no toolkit gives a default.  */
void
nhwl_set_scroll_bar_default_width (struct frame *f)
{
  int unit = FRAME_COLUMN_WIDTH (f);
  int minw = 14;

  FRAME_CONFIG_SCROLL_BAR_COLS (f) = (minw + unit - 1) / unit;
  FRAME_CONFIG_SCROLL_BAR_WIDTH (f) = minw;
}

void
nhwl_set_scroll_bar_default_height (struct frame *f)
{
  int height = FRAME_LINE_HEIGHT (f);
  int min_height = 14;

  FRAME_CONFIG_SCROLL_BAR_HEIGHT (f) = min_height;
  FRAME_CONFIG_SCROLL_BAR_LINES (f) = (min_height + height - 1) / height;
}

/* Terminals implement this instead of x-get-resource directly.  */
const char *
nhwl_get_string_resource (XrmDatabase rdb, const char *name,
			  const char *class)
{
  return NULL;
}

/* Adapted from pgtk_implicitly_set_name (pgtkfns.c): the title hint.  */
void
nhwl_implicitly_set_name (struct frame *f, Lisp_Object arg, Lisp_Object oldval)
{
  if (f->explicit_name)
    return;

  if (NILP (arg))
    arg = build_string ("Nidhogg");
  else
    CHECK_STRING (arg);

  if (!NILP (Fstring_equal (arg, f->name)))
    return;

  fset_name (f, arg);
  if (NILP (f->title))
    nhwl_set_title_hint (f);
}

static void
nhwl_free_pixmap (struct frame *f, Emacs_Pixmap pixmap)
{
  if (pixmap)
    {
      xfree (pixmap->data);
      xfree (pixmap);
    }
}

/* Set the pixel height of the tab bar of frame F to HEIGHT.  Adapted
   from x_change_tab_bar_height (xfns.c), as the bar is drawn by
   Emacs.  */
static void
nhwl_change_tab_bar_height (struct frame *f, int height)
{
  int unit = FRAME_LINE_HEIGHT (f);
  int old_height = FRAME_TAB_BAR_HEIGHT (f);
  int lines = height / unit;

  if (lines == 0 && height != 0)
    lines = 1;

  /* Make sure we redisplay all windows in this frame.  */
  fset_redisplay (f);

  /* Recalculate tab bar and frame text sizes.  */
  FRAME_TAB_BAR_HEIGHT (f) = height;
  FRAME_TAB_BAR_LINES (f) = lines;
  store_frame_param (f, Qtab_bar_lines, make_fixnum (lines));

  if (FRAME_NHWL_OUTPUT (f)->window && FRAME_TAB_BAR_HEIGHT (f) == 0)
    {
      clear_frame (f);
      clear_current_matrices (f);
    }

  if ((height < old_height) && WINDOWP (f->tab_bar_window))
    clear_glyph_matrix (XWINDOW (f->tab_bar_window)->current_matrix);

  if (!f->tab_bar_resized)
    {
      Lisp_Object fullscreen = get_frame_param (f, Qfullscreen);

      if (NILP (fullscreen) || EQ (fullscreen, Qfullwidth))
	adjust_frame_size (f, FRAME_TEXT_WIDTH (f), FRAME_TEXT_HEIGHT (f),
			   1, false, Qtab_bar_lines);
      else
	adjust_frame_size (f, -1, -1, 4, false, Qtab_bar_lines);

      f->tab_bar_resized = f->tab_bar_redisplayed;
    }
  else
    /* Any other change may leave the native size of F alone.  */
    adjust_frame_size (f, -1, -1, 3, false, Qtab_bar_lines);

  adjust_frame_glyphs (f);
  SET_FRAME_GARBAGED (f);
  if (FRAME_NHWL_OUTPUT (f)->window)
    nhwl_clear_under_internal_border (f);
}

/* Set the pixel height of the tool bar of frame F to HEIGHT.  Adapted
   from x_change_tool_bar_height (xfns.c, the non-GTK branch).  */
static void
nhwl_change_tool_bar_height (struct frame *f, int height)
{
  int unit = FRAME_LINE_HEIGHT (f);
  int old_height = FRAME_TOOL_BAR_HEIGHT (f);
  int lines = (height + unit - 1) / unit;
  Lisp_Object fullscreen = get_frame_param (f, Qfullscreen);

  /* Make sure we redisplay all windows in this frame.  */
  fset_redisplay (f);

  FRAME_TOOL_BAR_HEIGHT (f) = height;
  FRAME_TOOL_BAR_LINES (f) = lines;
  store_frame_param (f, Qtool_bar_lines, make_fixnum (lines));

  if (FRAME_NHWL_OUTPUT (f)->window && FRAME_TOOL_BAR_HEIGHT (f) == 0)
    {
      clear_frame (f);
      clear_current_matrices (f);
    }

  if ((height < old_height) && WINDOWP (f->tool_bar_window))
    clear_glyph_matrix (XWINDOW (f->tool_bar_window)->current_matrix);

  if (!f->tool_bar_resized)
    {
      /* As long as tool_bar_resized is false, effectively try to change
	 F's native height.  */
      if (NILP (fullscreen) || EQ (fullscreen, Qfullwidth))
	adjust_frame_size (f, FRAME_TEXT_WIDTH (f), FRAME_TEXT_HEIGHT (f),
			   1, false, Qtool_bar_lines);
      else
	adjust_frame_size (f, -1, -1, 4, false, Qtool_bar_lines);

      f->tool_bar_resized = f->tool_bar_redisplayed;
    }
  else
    /* Any other change may leave the native size of F alone.  */
    adjust_frame_size (f, -1, -1, 3, false, Qtool_bar_lines);

  adjust_frame_glyphs (f);
  SET_FRAME_GARBAGED (f);
  if (FRAME_NHWL_OUTPUT (f)->window)
    nhwl_clear_under_internal_border (f);
}

/* Adapted from pgtk_create_terminal; the scroll bar, menu and dialog
   hooks stay NULL: the shell has no scroll bars, menus or dialogs yet.  */
static struct terminal *
nhwl_create_terminal (struct nhwl_display_info *dpyinfo)
{
  struct terminal *terminal;

  terminal = create_terminal (output_nhwl, &nhwl_redisplay_interface);

  terminal->display_info.nhwl = dpyinfo;
  dpyinfo->terminal = terminal;

  terminal->clear_frame_hook = nhwl_clear_frame;
  terminal->ring_bell_hook = nhwl_ring_bell;
  terminal->toggle_invisible_pointer_hook = nhwl_toggle_invisible_pointer;
  terminal->update_begin_hook = nhwl_update_begin;
  terminal->update_end_hook = nhwl_update_end;
  terminal->read_socket_hook = nhwl_read_socket;
  terminal->frame_up_to_date_hook = nhwl_frame_up_to_date;
  terminal->mouse_position_hook = nhwl_mouse_position;
  terminal->frame_rehighlight_hook = nhwl_frame_rehighlight_hook;
  terminal->buffer_flipping_unblocked_hook
    = nhwl_buffer_flipping_unblocked_hook;
  terminal->frame_visible_invisible_hook = nhwl_make_frame_visible_invisible;
  terminal->fullscreen_hook = nhwl_fullscreen_hook;
  terminal->change_tab_bar_height_hook = nhwl_change_tab_bar_height;
  terminal->change_tool_bar_height_hook = nhwl_change_tool_bar_height;
  terminal->get_string_resource_hook = nhwl_get_string_resource;
  terminal->delete_frame_hook = nhwl_destroy_window;
  terminal->delete_terminal_hook = nhwl_delete_terminal;
  terminal->query_frame_background_color = nhwl_query_frame_background_color;
  terminal->defined_color_hook = nhwl_defined_color;
  terminal->set_new_font_hook = nhwl_new_font;
  terminal->implicit_set_name_hook = nhwl_implicitly_set_name;
  terminal->iconify_frame_hook = nhwl_iconify_frame;
  terminal->set_scroll_bar_default_width_hook
    = nhwl_set_scroll_bar_default_width;
  terminal->set_scroll_bar_default_height_hook
    = nhwl_set_scroll_bar_default_height;
  terminal->set_window_size_hook = nhwl_set_window_size;
  terminal->query_colors = nhwl_query_colors;
  terminal->get_focus_frame = nhwl_get_focus_frame;
  terminal->focus_frame_hook = nhwl_focus_frame;
  terminal->set_frame_offset_hook = nhwl_set_offset;
  terminal->frame_raise_lower_hook = nhwl_frame_raise_lower;
  terminal->free_pixmap = nhwl_free_pixmap;

  /* Other hooks are NULL by default.  */

  return terminal;
}

/* Adapted from pgtk_term_init.  DISPLAY_NAME "" means $WAYLAND_DISPLAY.  */
struct nhwl_display_info *
nhwl_term_init (Lisp_Object display_name)
{
  struct terminal *terminal;
  struct nhwl_display_info *dpyinfo;
  static bool x_initialized;
  static unsigned x_display_id = 0;
  char *dpy_name = SSDATA (display_name);
  Lisp_Object lisp_dpy_name;
  Lisp_Object color_file, system_name;
  NhShell *shell = NULL;
  ptrdiff_t nbytes;
  int rc;

  block_input ();

  if (!x_initialized)
    {
      any_help_event_p = false;

      /* The shell's events come through a descriptor and Emacs's input
	 polling, not SIGIO.  */
      Fset_input_interrupt_mode (Qnil);
      baud_rate = 19200;

      gui_init_fringe (&nhwl_redisplay_interface);
      x_initialized = true;
    }

  rc = nhshell_open (strlen (dpy_name) ? dpy_name : NULL, &shell);
  if (rc < 0 || !shell)
    {
      unblock_input ();
      return 0;
    }

  if (strlen (dpy_name) == 0)
    {
      const char *env = getenv ("WAYLAND_DISPLAY");

      dpy_name = (char *) (env && *env ? env : "wayland-0");
    }
  lisp_dpy_name = build_string (dpy_name);

  dpyinfo = xzalloc (sizeof *dpyinfo);
  dpyinfo->shell = shell;
  dpyinfo->connection = nhshell_wake_fd (shell);
  nhshell_caps (shell, &dpyinfo->caps);
  terminal = nhwl_create_terminal (dpyinfo);

  {
    struct nhwl_display_info *share;

    for (share = x_display_list; share; share = share->next)
      if (!strcmp (SSDATA (XCAR (share->name_list_element)), dpy_name))
	break;
    if (share)
      terminal->kboard = share->terminal->kboard;
    else
      {
	terminal->kboard = allocate_kboard (Qpgtk);

	/* Don't let the initial kboard remain current longer than necessary.
	   That would cause problems if a file loaded on startup tries to
	   prompt in the mini-buffer.  */
	if (current_kboard == initial_kboard)
	  current_kboard = terminal->kboard;
      }
    terminal->kboard->reference_count++;
  }

  /* Put this display on the chain.  */
  dpyinfo->next = x_display_list;
  x_display_list = dpyinfo;

  dpyinfo->name_list_element = Fcons (lisp_dpy_name, Qnil);

  dpyinfo->smallest_font_height = 1;
  dpyinfo->smallest_char_width = 1;

  /* Set the name of the terminal. */
  terminal->name = xlispstrdup (lisp_dpy_name);

  system_name = Fsystem_name ();
  if (ckd_add (&nbytes, SBYTES (Vinvocation_name), SBYTES (system_name) + 2))
    memory_full (SIZE_MAX);
  dpyinfo->x_id = ++x_display_id;
  dpyinfo->x_id_name = xmalloc (nbytes);
  {
    char *nametail = lispstpcpy (dpyinfo->x_id_name, Vinvocation_name);

    *nametail++ = '@';
    lispstpcpy (nametail, system_name);
  }

  dpyinfo->icon_bitmap_id = -1;
  dpyinfo->n_planes = 24;
  dpyinfo->color_p = 1;
  dpyinfo->invisible_cursor = nhwl_cursor_for_shape (NHSHELL_CURSOR_HIDDEN);
  dpyinfo->vertical_scroll_bar_cursor
    = nhwl_cursor_for_shape (NHSHELL_CURSOR_RESIZE_EW);
  dpyinfo->horizontal_scroll_bar_cursor
    = nhwl_cursor_for_shape (NHSHELL_CURSOR_RESIZE_NS);

  reset_mouse_highlight (&dpyinfo->mouse_highlight);

  /* The X colour names.  */
  color_file = Fexpand_file_name (build_string ("rgb.txt"),
				  Vdata_directory);
  dpyinfo->color_map = Fx_load_color_file (color_file);
  if (NILP (dpyinfo->color_map))
    fatal ("Could not read %s.\n", SDATA (color_file));

  /* The outputs the shell announced while connecting give the scale.  */
  nhshell_dispatch_main (shell);
  dpyinfo->scale120 = 0;
  dpyinfo->resx = dpyinfo->resy = 96.0;
  nhwl_drain_events (dpyinfo, NULL);
  if (dpyinfo->scale120 == 0)
    dpyinfo->scale120 = 120;
  dpyinfo->resx = dpyinfo->resy = 96.0 * dpyinfo->scale120 / 120.0;

  /* Set up scrolling increments.  */
  dpyinfo->scroll.x_per_char = 1;
  dpyinfo->scroll.y_per_line = 1;

  if (dpyinfo->connection >= 0)
    add_keyboard_wait_descriptor (dpyinfo->connection);

  unblock_input ();

  return dpyinfo;
}

/* Get rid of display DPYINFO, deleting all frames on it.  Adapted from
   pgtk_delete_display.  */
static void
nhwl_delete_display (struct nhwl_display_info *dpyinfo)
{
  struct terminal *t;

  /* Close all frames and delete the generic struct terminal for this
     display.  */
  for (t = terminal_list; t; t = t->next_terminal)
    if (t->type == output_nhwl && t->display_info.nhwl == dpyinfo)
      {
	delete_terminal (t);
	break;
      }

  if (x_display_list == dpyinfo)
    x_display_list = dpyinfo->next;
  else
    {
      struct nhwl_display_info *tail;

      for (tail = x_display_list; tail; tail = tail->next)
	if (tail->next == dpyinfo)
	  tail->next = tail->next->next;
    }

  xfree (dpyinfo);
}

/* This function is called when the last frame on a display is deleted.
   Adapted from pgtk_delete_terminal.  */
void
nhwl_delete_terminal (struct terminal *terminal)
{
  struct nhwl_display_info *dpyinfo = terminal->display_info.nhwl;

  /* Protect against recursive calls.  delete_frame in
     delete_terminal calls us back when it deletes our last frame.  */
  if (!terminal->name)
    return;

  block_input ();

  if (dpyinfo->shell)
    {
      image_destroy_all_bitmaps (dpyinfo);

      if (dpyinfo->connection >= 0)
	delete_keyboard_wait_descriptor (dpyinfo->connection);
      dpyinfo->connection = -1;

      nhshell_close (dpyinfo->shell);
      dpyinfo->shell = NULL;
    }

  nhwl_delete_display (dpyinfo);
  terminal->display_info.nhwl = NULL;
  unblock_input ();
}

void
mark_nhwlterm (void)
{
  struct nhwl_display_info *dpyinfo;

  for (dpyinfo = x_display_list; dpyinfo; dpyinfo = dpyinfo->next)
    {
      mark_object (dpyinfo->name_list_element);
      mark_object (dpyinfo->color_map);
    }
}

/* Adapted from pgtk_display_pixel_width/height: the output with the
   largest extent, in device pixels.  */
int
nhwl_display_pixel_width (struct nhwl_display_info *dpyinfo)
{
  int i, width = 0;

  for (i = 0; i < dpyinfo->n_monitors; i++)
    width = max (width, dpyinfo->monitors[i].w);
  return width ? width : 1920;
}

int
nhwl_display_pixel_height (struct nhwl_display_info *dpyinfo)
{
  int i, height = 0;

  for (i = 0; i < dpyinfo->n_monitors; i++)
    height = max (height, dpyinfo->monitors[i].h);
  return height ? height : 1080;
}

static int
nhwl_bench_cmp (const void *a, const void *b)
{
  double x = *(const double *) a, y = *(const double *) b;

  return (x > y) - (x < y);
}

static double
nhwl_bench_ms (struct timespec t)
{
  return t.tv_sec * 1e3 + t.tv_nsec / 1e6;
}

/* Sorted A of N values: the P-th percentile (nearest rank).  */
static double
nhwl_bench_pct (double *a, ptrdiff_t n, double p)
{
  ptrdiff_t k = (ptrdiff_t) ceil (p * n) - 1;

  return a[max (k, 0)];
}

DEFUN ("nhwl--bench-full-redraw", Fnhwl__bench_full_redraw,
       Snhwl__bench_full_redraw, 1, 1, 0,
       doc: /* Present N frames of the selected frame, each damaged whole.
Each frame waits for the previous one's frame callback.  Return
(:frames N :compose-p50 MS :compose-p95 MS :present-interval-p95 MS):
the time one present took (commit, buffer, compose, attach) and the time
between two presents.  The motion bench of the W0 budgets.  */)
  (Lisp_Object n)
{
  struct frame *f = SELECTED_FRAME ();
  struct nhwl_output *out;
  struct nhwl_display_info *dpyinfo;
  struct timespec ts;
  double *compose, *interval, prev = 0;
  ptrdiff_t frames, i, done = 0;

  CHECK_FIXNAT (n);
  if (!FRAME_NHWL_P (f))
    error ("Not a Nidhogg shell frame");
  frames = XFIXNAT (n);
  if (frames < 1 || frames > 100000)
    error ("N must be 1 to 100000");
  out = FRAME_NHWL_OUTPUT (f);
  dpyinfo = FRAME_DISPLAY_INFO (f);
  if (!out->glitnir_id || !out->configured)
    error ("The frame is not shown yet");

  compose = xnmalloc (frames, 2 * sizeof *compose);
  interval = compose + frames;
  for (i = 0; i < frames; i++)
    {
      double t0, t1;
      int waits = 0;
      bool committed;

      /* The previous present's frame callback (about one refresh): the
	 pacer is ready again.  */
      glitnir_damage_all (out->glitnir_id);
      nhwl_dirty (f);
      while (waits++ < 5000)
	{
	  int ready = 0;

	  if (nhshell_window_ready (dpyinfo->shell, out->window, &ready) < 0
	      || ready)
	    break;
	  poll (NULL, 0, 1);
	}
      clock_gettime (CLOCK_MONOTONIC, &ts);
      t0 = nhwl_bench_ms (ts);
      committed = nhwl_present (f);
      clock_gettime (CLOCK_MONOTONIC, &ts);
      t1 = nhwl_bench_ms (ts);
      if (!committed)
	break;
      compose[done] = t1 - t0;
      if (done > 0)
	interval[done - 1] = t1 - prev;
      prev = t1;
      done++;
    }
  if (done < 2)
    {
      xfree (compose);
      error ("The bench presented %"pD"d frames", done);
    }
  qsort (compose, done, sizeof *compose, nhwl_bench_cmp);
  qsort (interval, done - 1, sizeof *interval, nhwl_bench_cmp);
  Lisp_Object r
    = list (intern_c_string (":frames"), make_fixnum (done),
	    intern_c_string (":compose-p50"),
	    make_float (nhwl_bench_pct (compose, done, 0.5)),
	    intern_c_string (":compose-p95"),
	    make_float (nhwl_bench_pct (compose, done, 0.95)),
	    intern_c_string (":present-interval-p95"),
	    make_float (nhwl_bench_pct (interval, done - 1, 0.95)));
  xfree (compose);
  return r;
}

void
syms_of_nhwlterm (void)
{
  defsubr (&Snhwl__bench_full_redraw);

  DEFSYM (Qmodifier_value, "modifier-value");
  DEFSYM (Qalt, "alt");
  DEFSYM (Qhyper, "hyper");
  DEFSYM (Qmeta, "meta");
  DEFSYM (Qsuper, "super");
  DEFSYM (Qcontrol, "control");

  Fput (Qalt, Qmodifier_value, make_fixnum (alt_modifier));
  Fput (Qhyper, Qmodifier_value, make_fixnum (hyper_modifier));
  Fput (Qmeta, Qmodifier_value, make_fixnum (meta_modifier));
  Fput (Qsuper, Qmodifier_value, make_fixnum (super_modifier));
  Fput (Qcontrol, Qmodifier_value, make_fixnum (ctrl_modifier));

  DEFVAR_LISP ("nhwl-alt-modifier", Vnhwl_alt_modifier,
    doc: /* The Emacs modifier for the Alt key (xkb's Mod1).
One of `alt', `meta', `super', `hyper' or `control'.  */);
  Vnhwl_alt_modifier = Qmeta;

  DEFVAR_LISP ("nhwl-meta-modifier", Vnhwl_meta_modifier,
    doc: /* The Emacs modifier for xkb's Meta modifier.  */);
  Vnhwl_meta_modifier = Qmeta;

  DEFVAR_LISP ("nhwl-super-modifier", Vnhwl_super_modifier,
    doc: /* The Emacs modifier for the Super key.  */);
  Vnhwl_super_modifier = Qsuper;

  DEFVAR_LISP ("nhwl-hyper-modifier", Vnhwl_hyper_modifier,
    doc: /* The Emacs modifier for the Hyper key.  */);
  Vnhwl_hyper_modifier = Qhyper;

  DEFVAR_LISP ("nhwl-wait-for-configure-timeout",
	       Vnhwl_wait_for_configure_timeout,
    doc: /* Seconds to wait for the compositor's first configure of a frame.
If it is not a float there is no wait.  */);
  Vnhwl_wait_for_configure_timeout = make_float (2.0);

  DEFVAR_LISP ("pgtk-keysym-table", Vnhwl_keysym_table,
    doc: /* Hash table of character codes indexed by X keysym codes.  */);
  Vnhwl_keysym_table = make_hash_table (&hashtest_eql, 900, Weak_None);

  DEFVAR_LISP ("x-ctrl-keysym", Vx_ctrl_keysym,
    doc: /* SKIP: real doc in xterm.c.  */);
  Vx_ctrl_keysym = Qnil;
  DEFVAR_LISP ("x-alt-keysym", Vx_alt_keysym,
    doc: /* SKIP: real doc in xterm.c.  */);
  Vx_alt_keysym = Qnil;
  DEFVAR_LISP ("x-hyper-keysym", Vx_hyper_keysym,
    doc: /* SKIP: real doc in xterm.c.  */);
  Vx_hyper_keysym = Qnil;
  DEFVAR_LISP ("x-meta-keysym", Vx_meta_keysym,
    doc: /* SKIP: real doc in xterm.c.  */);
  Vx_meta_keysym = Qnil;
  DEFVAR_LISP ("x-super-keysym", Vx_super_keysym,
    doc: /* SKIP: real doc in xterm.c.  */);
  Vx_super_keysym = Qnil;

  DEFVAR_BOOL ("x-use-underline-position-properties",
	       x_use_underline_position_properties,
    doc: /* SKIP: real doc in xterm.c.  */);
  x_use_underline_position_properties = 1;

  DEFVAR_BOOL ("x-underline-at-descent-line",
	       x_underline_at_descent_line,
    doc: /* SKIP: real doc in xterm.c.  */);
  x_underline_at_descent_line = 0;

  DEFVAR_LISP ("x-toolkit-scroll-bars", Vx_toolkit_scroll_bars,
    doc: /* SKIP: real doc in xterm.c.  */);
  Vx_toolkit_scroll_bars = Qnil;

  /* Tell Emacs about this window system: `pgtk' at the Lisp level, so
     term/pgtk-win.el loads unchanged, and `nidhogg-wayland'.  */
  Fprovide (Qpgtk, Qnil);
  Fprovide (intern_c_string ("nidhogg-wayland"), Qnil);
}

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

/* This file implements Emacs's terminal hooks against the shell glue
   (nhglue.h) once; it never calls Wayland.  Functions adapted from
   pgtkterm.c name their origin in their comment; the upstream commit is
   a360712c9d272d950d8d8255ef74570f7e90b7d9 (GNU Emacs emacs-31.1), for
   every "Adapted from" note in this file.  The frame's pixel unit
   is the device pixel, so a scale change re-fonts the frame as a move
   between monitors does, and no drawing happens at a fractional scale.

   Every shell call runs under block_input: the input polling timer can
   enter nhwl_read_socket from a signal handler, and the shell is not
   re-entrant.  */

#include <config.h>

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
#include "nidhogg-history.h"

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

static void nhwl_connection_hook (struct nhwl_display_info *, Lisp_Object);
static void nhwl_reconnect_disarm (struct nhwl_display_info *);

/* The text of the protocol error that ended DPYINFO's connection, into
   BUF (CAP bytes); empty after a plain hangup, or when the glue is
   poisoned.  */
static void
nhwl_last_error (struct nhwl_display_info *dpyinfo, char *buf, size_t cap)
{
  size_t len = 0;

  buf[0] = 0;
  if (dpyinfo->glue && !dpyinfo->glue_broken
      && NHG (nhg_last_error (dpyinfo->glue, buf, cap, &len)) != NHG_OK)
    buf[0] = 0;
}

/* How deep F hangs when it is a shown popup frame of DPYINFO (1: the
   popup of a frame that has no parent), else 0.  */
static int
nhwl_lost_popup_depth (struct frame *f, struct nhwl_display_info *dpyinfo)
{
  int depth = 1;

  if (!FRAME_LIVE_P (f) || !nhwl_popup_frame_p (f)
      || FRAME_DISPLAY_INFO (f) != dpyinfo || !FRAME_VISIBLE_P (f))
    return 0;
  for (f = FRAME_PARENT_FRAME (f); f; f = FRAME_PARENT_FRAME (f))
    depth++;
  return depth;
}

/* A `delete-frame' event for each shown popup frame of DPYINFO, the
   deepest first, as a compositor dismisses them: a menu waiting in its
   loop reads them and returns nil.  Its frames are dead by then.  */
static void
nhwl_lost_popups (struct nhwl_display_info *dpyinfo)
{
  Lisp_Object tail, frame;
  struct input_event ie;
  int depth, deepest = 0;

  FOR_EACH_FRAME (tail, frame)
    {
      depth = nhwl_lost_popup_depth (XFRAME (frame), dpyinfo);
      if (depth > deepest)
	deepest = depth;
    }
  for (depth = deepest; depth > 0; depth--)
    FOR_EACH_FRAME (tail, frame)
      if (nhwl_lost_popup_depth (XFRAME (frame), dpyinfo) == depth)
	{
	  EVENT_INIT (ie);
	  ie.kind = DELETE_WINDOW_EVENT;
	  ie.arg = Qnil;
	  ie.frame_or_window = frame;
	  kbd_buffer_store_event (&ie);
	}
}

/* The compositor went away, and nothing waits for it to come back
   (reconnecting is off, failed or timed out), or the glue panicked.  The
   display's frames and its terminal are deleted without running Lisp,
   and the session goes on on its other frames (another display, a tty,
   the daemon's own frame).  With no frame left Emacs exits with status
   70, as xterm does: before the deletion, since delete_frame would
   otherwise kill Emacs from inside it.  WHY says what happened; the
   message adds the protocol error's text when there was one.  This is
   the one way a display goes when its compositor does.  Frees DPYINFO.  */
static void
nhwl_connection_lost (struct nhwl_display_info *dpyinfo, const char *why)
{
  struct terminal *t = dpyinfo->terminal, *p;
  Lisp_Object tail, frame, buf = Fcurrent_buffer ();
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
      char detail[512];

      nhwl_last_error (dpyinfo, detail, sizeof detail);
      if (detail[0])
	fprintf (stderr, "nidhogg-wayland: %s: %s\n", why, detail);
      else
	fprintf (stderr, "nidhogg-wayland: %s\n", why);
      shut_down_emacs (0, Qnil);
      exit (70);
    }

  /* Before the frames go: no reconnect tick or settle is due any more;
     the glue lets go of what it held for the connection (a poisoned glue
     is not asked); a menu waiting in its loop is told that its popups
     went; and Lisp hears `lost', from a timer, so after the deletion.
     Each frame's deletion then cancels its bell's timer and frees its
     backing, and with it its Glitnir root.  A selection or drop read
     that waited on the display has already returned nil: the shell
     ended it when the connection went.  */
  nhwl_reconnect_disarm (dpyinfo);
  dpyinfo->reconnecting = dpyinfo->reconnect_settling = false;
  if (dpyinfo->glue && !dpyinfo->glue_broken)
    nhwl_glue_rc (dpyinfo, NHG (nhg_give_up (dpyinfo->glue)));
  nhwl_lost_popups (dpyinfo);
  nhwl_connection_hook (dpyinfo, Qnhwl_lost);

  t->read_socket_hook = NULL;	/* delete_frame reads input: no re-entry */
  FOR_EACH_FRAME (tail, frame)	/* a parent's deletion kills its children */
    if (FRAME_LIVE_P (XFRAME (frame)) && FRAME_NHWL_P (XFRAME (frame))
	&& FRAME_DISPLAY_INFO (XFRAME (frame)) == dpyinfo)
      delete_frame (frame, Qnoelisp);
  Fset_buffer (buf);		/* the caller (call-process) may be inserting */
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

#ifdef NHWL_WAYLAND
/* Naglfar's backing primitives are in nhmacdraw.c.  */

/* Adapted from pgtk_begin_cr_clip: the frame's retained backing, device
   size, made or resized on use.  NHWL_CR_DEPTH counts the open calls.  */
static int nhwl_cr_depth;
cairo_t *
nhwl_begin_cr_clip (struct frame *f)
{
  nhwl_backing_resize (f);
  cairo_t *cr = FRAME_NHWL_OUTPUT (f)->cr_context;
  cairo_save (cr);
  /* Emacs draws in the native area, inside the title border: moved once,
     as a glyph string's fills and font drawing nest these calls.  */
  if (!nhwl_cr_depth++)
    cairo_translate (cr, FRAME_NHWL_OUTPUT (f)->csd.l, FRAME_NHWL_OUTPUT (f)->csd.t);
  return cr;
}

void
nhwl_end_cr_clip (struct frame *f)
{
  cairo_restore (FRAME_NHWL_OUTPUT (f)->cr_context);
  nhwl_cr_depth--;
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
#endif /* NHWL_WAYLAND */

/* Whether C is a child frame of F with a window.  */
static bool
nhwl_child_of (struct frame *c, struct frame *f)
{
  return (nhwl_child_p (c) && FRAME_PARENT_FRAME (c) == f
	  && FRAME_NHWL_OUTPUT (c)->window);
}

/* Whether F may show a buffer by itself: shown, and at the size its next
   redisplay draws (a size change_frame_size queued is drawn by that
   redisplay, which publishes again).  A child frame's surface is unmapped
   only while it is hidden (nhwl_child_push), so a shown one is
   mapped.  The glue asks the same of every ancestor.  */
static bool
nhwl_frame_drawable (struct frame *f)
{
  return FRAME_VISIBLE_P (f) && !f->new_size_p;
}

/* The surface list nhg_present takes, kept between publishes.  */
static NhgSurface *nhwl_surfaces;
static ptrdiff_t nhwl_nsurfaces, nhwl_surfaces_cap;

/* List F's surface after its parent's (PARENT: the parent's window, 0 for
   the root), then the child frames under it, each followed by its own.
   When PREPARE and F may show a buffer, F is brought up to date
   with what was drawn first: its backing's size, the title strip, a child
   frame's position and input region, the window geometry (all of which go
   with the tree's next commit; the glue sends them in nhwl_csd_draw's
   publish step), and its surface is flushed, because the glue reads its
   pixels.  The child frames under an undrawable frame are not prepared.  */
static void
nhwl_surface_add (struct frame *f, uint64_t parent, bool prepare)
{
  struct nhwl_output *out = FRAME_NHWL_OUTPUT (f);
  bool drawable = prepare && nhwl_frame_drawable (f);
  Lisp_Object tail, frame;

  if (drawable)
    {
      nhwl_backing_resize (f);
      /* The title strip; the window geometry, and a child frame's position
	 and input region (the glue sends them).  */
#ifdef NHWL_WAYLAND
      nhwl_csd_draw (f);
#endif
      nhwl_glitnir_ensure (f);
      if (nhwl_backing_p (out))
	nhwl_backing_flush (out);
    }
#ifdef ENABLE_CHECKING
  {
    /* Emacs's values; the native size is the one C asked for.  */
    struct frame *p = FRAME_PARENT_FRAME (f);
    NhgFrameFacts facts
      = { max (1, f->new_size_p ? f->new_width : FRAME_PIXEL_WIDTH (f)),
	  max (1, f->new_size_p ? f->new_height : FRAME_PIXEL_HEIGHT (f)),
	  FRAME_COLUMN_WIDTH (f), FRAME_LINE_HEIGHT (f),
	  p && FRAME_NHWL_P (p) ? FRAME_NHWL_OUTPUT (p)->window : 0,
	  ((FRAME_VISIBLE_P (f) ? NHG_FACT_VISIBLE : 0)
	   | (FRAME_ICONIFIED_P (f) ? NHG_FACT_ICONIFIED : 0)
	   | (out->csd.on ? NHG_FACT_CSD : 0)) };

    nhwl_glue_rc (FRAME_DISPLAY_INFO (f),
		  NHG (nhg_debug_check_frame (FRAME_NHG (f), out->window,
					      &facts)));
  }
#endif
  if (nhwl_nsurfaces == nhwl_surfaces_cap)
    nhwl_surfaces = xpalloc (nhwl_surfaces, &nhwl_surfaces_cap, 1, -1,
			     sizeof *nhwl_surfaces);
  nhwl_surfaces[nhwl_nsurfaces++]
    = (NhgSurface) { out->window, parent, out->glitnir_id,
		     ((drawable ? NHG_SURF_DRAWABLE : 0)
		      | (out->alpha_host ? NHG_SURF_ALPHA_HOST : 0)),
		     out->alpha_cut };

  /* The child frames (the glue puts them in stacking order).  */
  FOR_EACH_FRAME (tail, frame)
    if (nhwl_child_of (XFRAME (frame), f))
      nhwl_surface_add (XFRAME (frame), out->window, drawable);
}

/* nhwl-scroll-layers and F's alpha-background, as the glue's window
   layers take them (NHG_LAYERS_*).  */
uint32_t
nhwl_layer_flags (struct frame *f)
{
  uint32_t flags = 0;

  if (!NILP (Vnhwl_scroll_layers))
    flags |= NHG_LAYERS_ON;
  if (EQ (Vnhwl_scroll_layers, Qinteger_scales))
    flags |= NHG_LAYERS_INTEGER_SCALES;
  if (f->alpha_background >= 1.0)
    flags |= NHG_LAYERS_OPAQUE_BG;
  return flags;
}

/* The window list nhg_layers_sync takes, kept between publishes.  */
static NhgLayerWindow *nhwl_layer_windows;
static ptrdiff_t nhwl_nlayer_windows, nhwl_layer_windows_cap;

/* List WINDOW's leaf windows and its next siblings' (the root window's
   next sibling is the minibuffer window), each with the box the scroll
   passes (nhwl_scroll_run), its edges without the dividers and its card
   (the window parameter glitnir--card).  */
static void
nhwl_layer_windows_add (Lisp_Object window)
{
  for (; !NILP (window); window = XWINDOW (window)->next)
    {
      struct window *w = XWINDOW (window);
      int x, y, width, height;

      if (WINDOWP (w->contents))
	{
	  nhwl_layer_windows_add (w->contents);
	  continue;
	}
      window_box (w, ANY_AREA, &x, &y, &width, &height);
      if (nhwl_nlayer_windows == nhwl_layer_windows_cap)
	nhwl_layer_windows = xpalloc (nhwl_layer_windows,
				      &nhwl_layer_windows_cap, 1, -1,
				      sizeof *nhwl_layer_windows);
      NhgLayerWindow *l = &nhwl_layer_windows[nhwl_nlayer_windows++];
      *l = (NhgLayerWindow) { w->sequence_number, { x, y, width, height },
	{ WINDOW_LEFT_EDGE_X (w), WINDOW_TOP_EDGE_Y (w),
	  WINDOW_PIXEL_WIDTH (w) - WINDOW_RIGHT_DIVIDER_WIDTH (w),
	  WINDOW_PIXEL_HEIGHT (w) - WINDOW_BOTTOM_DIVIDER_WIDTH (w) } };
      nhwl_chrome_vector (Fwindow_parameter (window, Qglitnir__card), l->card);
    }
}

/* Before root R is published: the glue makes the windows of R with a
   layer of their own that are gone, resized or quiet part of the frame
   again, or all of them when R is no longer an opaque root, in this
   publish, and gives the windows with a card theirs.  Nothing to do while
   R has neither (its parameter glitnir--cards is nil).  */
static void
nhwl_layers_sync (struct frame *r)
{
  struct nhwl_output *out = FRAME_NHWL_OUTPUT (r);
  struct nhwl_display_info *dpyinfo = FRAME_DISPLAY_INFO (r);

  if (!out->glitnir_id
      || (NILP (get_frame_param (r, Qglitnir__cards))
	  && nhwl_glue_rc (dpyinfo, NHG (nhg_layers_count (dpyinfo->glue,
							   out->glitnir_id))) <= 0))
    return;
  nhwl_nlayer_windows = 0;
  nhwl_layer_windows_add (FRAME_ROOT_WINDOW (r));
  nhwl_glue_rc (dpyinfo,
		NHG (nhg_layers_sync (dpyinfo->glue, out->window,
				      out->glitnir_id, nhwl_layer_windows,
				      nhwl_nlayer_windows,
				      nhwl_layer_flags (r))));
}

/* The blocks nhg_blocks_sync takes, kept between publishes.  */
static NhgBlock *nhwl_blocks;
static ptrdiff_t nhwl_nblocks, nhwl_blocks_cap;

/* Position P of block slot in window W's buffer (a marker of that
   buffer, or an integer), or -1.  */
static ptrdiff_t
nhwl_block_pos (struct window *w, Lisp_Object p)
{
  if (MARKERP (p))
    return (XMARKER (p)->buffer == XBUFFER (w->contents)
	    ? marker_position (p) : -1);
  return FIXNUMP (p) ? XFIXNUM (p) : -1;
}

/* The slot I of block vector V as an integer, or DFLT.  */
static intmax_t
nhwl_block_int (Lisp_Object v, int i, intmax_t dflt)
{
  Lisp_Object x = AREF (v, i);
  return (FIXNUMP (x) ? XFIXNUM (x)
	  : FLOATP (x) ? (intmax_t) XFLOAT_DATA (x) : dflt);
}

/* The colour beneath block V of window W: its BENEATH-RGB, or for a
   `row' block without one (-1) the window's default background, which
   a buffer's face remapping changes.  */
static intmax_t
nhwl_block_beneath (struct window *w, Lisp_Object v)
{
  intmax_t under = nhwl_block_int (v, 8, 0);
  if (under >= 0)
    return under;
  struct frame *f = XFRAME (w->frame);
  struct face *def = FACE_FROM_ID_OR_NULL (f, lookup_basic_face (w, f, DEFAULT_FACE_ID));
  return def ? (intmax_t) (def->background & 0xffffff) : 0;
}

extern Lisp_Object Foverlays_in (Lisp_Object, Lisp_Object);

/* The after-strings of the overlays of W's buffer that end at END (an
   overlay holds its after-string at its end).  */
static Lisp_Object
nhwl_block_after_strings (struct window *w, ptrdiff_t end)
{
  specpdl_ref count = SPECPDL_INDEX ();
  Lisp_Object list = Qnil;

  record_unwind_current_buffer ();
  set_buffer_internal (XBUFFER (w->contents));
  /* Those that cover the character before END, and the empty ones and
     those that start at END (which end at END only when empty).  */
  for (int pass = 0; pass < 2; pass++)
    {
      Lisp_Object ovs
	= (pass == 0
	   ? (end > BEGV ? Foverlays_in (make_fixnum (end - 1), make_fixnum (end)) : Qnil)
	   : Foverlays_in (make_fixnum (end), make_fixnum (min (end + 1, Z))));
      for (; CONSP (ovs); ovs = XCDR (ovs))
	{
	  Lisp_Object ov = XCAR (ovs);
	  if (OVERLAY_END (ov) != end)
	    continue;
	  Lisp_Object str = Foverlay_get (ov, Qafter_string);
	  if (STRINGP (str))
	    list = Fcons (str, list);
	}
    }
  return unbind_to (count, list);
}

/* Whether ROW holds only text of an after-string in STRINGS (those of
   the overlays that end at END) displayed at END: it starts at END, a
   glyph of its text area comes from one of them and none from the
   buffer.  Its end position is not asked: every row of a multi-row
   after-string but the first may end past END (the iterator has stepped
   on from the string's newline).  A string row of another overlay
   starting at END (the next cell's header, a before-string) is not
   one.  Rows of an image or a space display of the string count: their
   glyphs' object is the string.  */
static bool
nhwl_block_tail_row_p (struct glyph_row *row, ptrdiff_t end, Lisp_Object strings)
{
  if (MATRIX_ROW_START_CHARPOS (row) != end)
    return false;
  bool from_string = false;
  struct glyph *g = row->glyphs[TEXT_AREA];
  for (struct glyph *e = g + row->used[TEXT_AREA]; g < e; g++)
    {
      if (BUFFERP (g->object))
	return false;
      if (STRINGP (g->object) && !NILP (Fmemq (g->object, strings)))
	from_string = true;
    }
  return from_string;
}

/* Whether ROW has a glyph of the string STR.  */
static bool
nhwl_block_row_has_string_p (struct glyph_row *row, Lisp_Object str)
{
  struct glyph *g = row->glyphs[TEXT_AREA];
  for (struct glyph *e = g + row->used[TEXT_AREA]; g < e; g++)
    if (EQ (g->object, str))
      return true;
  return false;
}

/* The fill of the `row' block of leaf window W (its parameter
   glitnir--blocks) over W's text row ROW, a row whose start is in the
   block's START..END: its FILL-RGB at FILL-OPACITY over its BENEATH-RGB,
   0xRRGGBB with bit 24 set; else 0 (no such block, a mode or header
   line, a frame Glitnir does not draw).  The first such block wins.
   Redisplay keeps it on the row (update_text_area), and the row's glyphs
   that show W's default background draw in it (glitnir_decor_row_bg,
   nhwl_clear_end_of_line): the block lies beneath its rows' glyphs.  */
unsigned int
glitnir_row_fill (struct window *w, struct glyph_row *row)
{
  if (row->mode_line_p || !BUFFERP (w->contents)
      || !nhwl_glitnir_active_p (XFRAME (w->frame)))
    return 0;
  Lisp_Object window;
  XSETWINDOW (window, w);
  Lisp_Object v = Fwindow_parameter (window, Qglitnir__blocks);
  if (!VECTORP (v))
    return 0;
  ptrdiff_t rs = MATRIX_ROW_START_CHARPOS (row);
  for (ptrdiff_t i = 0; i < ASIZE (v); i++)
    {
      Lisp_Object b = AREF (v, i);
      if (!VECTORP (b) || ASIZE (b) < 23 || !EQ (AREF (b, 0), make_fixnum (4)))
	continue;
      ptrdiff_t start = nhwl_block_pos (w, AREF (b, 1));
      ptrdiff_t end = nhwl_block_pos (w, AREF (b, 2));
      intmax_t rgb = nhwl_block_int (b, 16, -1);
      if (start < 0 || rs < start || rs >= end || rgb < 0)
	continue;
      Lisp_Object op = AREF (b, 17);
      double a = NUMBERP (op) ? min (max (XFLOATINT (op), 0.0), 1.0) : 1.0;
      intmax_t under = nhwl_block_beneath (w, b);
      unsigned int out = 0;
      for (int sh = 0; sh <= 16; sh += 8)
	out |= ((unsigned int) lround (a * ((rgb >> sh) & 0xff)
				       + (1 - a) * ((under >> sh) & 0xff))
		<< sh);
      return out | 1u << 24;
    }
  return 0;
}

/* List block V of leaf window W, whose text area is CLIP: the rows of
   W's current matrix holding its text, from its first row's top to its
   last row's bottom, from x0 to x1 of the text area.  V is
   [KIND START END X0 X1 RADIUS RING-RGB RING-OPACITY BENEATH-RGB GAP
   SHADOW-RGB SHADOW-OPACITY HALO-RGB HALO-OPACITY OUTER-RGB OUTER-OPACITY
   FILL-RGB FILL-OPACITY TOP-RGB TOP-OPACITY OVERLAY PAD-TOP PAD-BOTTOM]
   as glitnir--block-vector makes it (KIND 0 a ring, 1 a rule, 2 a bar, 3 an
   inner panel, 4 a row: Glitnir paints what lies outside its rounded
   rectangle, the rows paint its fill (glitnir_row_fill); START and END positions, END exclusive, or equal: the rows of
   the overlay strings displayed at that position, and no text row; OVERLAY an
   overlay, or nil: the block then holds the rows with a glyph of its
   after-string, START and END unread (a block of another kind takes
   only that overlay's after-string for the rows after its END);
   PAD-TOP and PAD-BOTTOM device px, an inner panel's rectangle shorter
   by them (the rows are the same); X0,
   X1, RADIUS and GAP device pixels; an RGB -1 for none).  A block
   whose text starts above the window's first row (scrolled out) is open
   at the top; one that goes on below its last row, at the bottom.  Rows
   of nothing but the after-string of an overlay ending at END (a notebook
   cell's output, of any number of rows) are inside: they start at END and
   show no buffer text, so a block starting at END leaves them out, and
   so are the rows of a before-string of the next block's overlay.  A
   block starting at START leaves out the rows of the after-strings of
   overlays ending at START: they are the previous block's, though a row
   of a multi-row string but the first ends at START + 1.  This is all C
   does: the glue plans the layers and Glitnir draws.  */
static void
nhwl_block_add (struct window *w, Lisp_Object v, NhgRect clip)
{
  struct glyph_matrix *m = w->current_matrix;

  if (!m || !VECTORP (v) || ASIZE (v) < 23)
    return;
  /* An inner block may name the overlay whose after-string it holds:
     its rows are those with a glyph of that string, wherever they
     start.  */
  Lisp_Object ov = AREF (v, 20);
  Lisp_Object own = OVERLAYP (ov) ? Foverlay_get (ov, Qafter_string) : Qnil;
  /* An inner block holds the rows of that string; another kind takes
     only that overlay's after-string for the rows after its END.  */
  bool by_string = STRINGP (own) && EQ (AREF (v, 0), make_fixnum (3));
  ptrdiff_t start = nhwl_block_pos (w, AREF (v, 1));
  ptrdiff_t end = nhwl_block_pos (w, AREF (v, 2));
  if (!by_string && (start < 0 || end < start))
    return;
  /* START = END: the rows of the overlay strings there and no text row.  */
  bool tail_only = end == start;
  Lisp_Object strings = (STRINGP (own) && !by_string
			 ? list1 (own) : Qt);	/* Qt: not made yet */
  Lisp_Object before = Qt;	/* the same for the rows at START */

  struct glyph_row *first = MATRIX_FIRST_TEXT_ROW (m);
  struct glyph_row *bottom = MATRIX_BOTTOM_TEXT_ROW (m, w);
  struct glyph_row *top = NULL, *last = NULL;
  bool stopped = false;
  for (struct glyph_row *row = first; row < bottom && row->enabled_p; row++)
    {
      if (by_string)
	{
	  if (nhwl_block_row_has_string_p (row, own))
	    {
	      if (!top)
		top = row;
	      last = row;
	    }
	  else if (top)
	    {
	      stopped = true;
	      break;
	    }
	  continue;
	}
      ptrdiff_t rs = MATRIX_ROW_START_CHARPOS (row);
      ptrdiff_t re = MATRIX_ROW_END_CHARPOS (row);
      if (rs == end && EQ (strings, Qt))
	strings = nhwl_block_after_strings (w, end);
      /* a row of the after-string of the overlays ending at START is the
	 previous block's, though its end is past START (a row of a
	 multi-row string but the first ends at START + 1); the block's
	 own header there is a before-string */
      if (!tail_only && rs == start)
	{
	  if (EQ (before, Qt))
	    before = nhwl_block_after_strings (w, start);
	  if (nhwl_block_tail_row_p (row, start, before))
	    continue;
	}
      if (rs > end || (rs == end && !nhwl_block_tail_row_p (row, end, strings)))
	{
	  stopped = true;
	  break;
	}
      if (tail_only
	  ? rs < start
	  : re <= start && !(row->ends_at_zv_p && re >= start))
	continue;
      if (!top)
	top = row;
      last = row;
      if (row->ends_at_zv_p)
	{
	  stopped = true;
	  break;
	}
    }
  if (!top)
    return;

  int x0 = clip_to_bounds (0, nhwl_block_int (v, 3, 0), clip.w);
  int x1 = clip_to_bounds (x0, nhwl_block_int (v, 4, clip.w), clip.w);
  int y0 = WINDOW_TO_FRAME_PIXEL_Y (w, top->y);
  int y1 = WINDOW_TO_FRAME_PIXEL_Y (w, last->y) + last->height;
  uint32_t flags = (EQ (AREF (v, 0), make_fixnum (1)) ? NHG_BLOCK_RULE
		    : EQ (AREF (v, 0), make_fixnum (2)) ? NHG_BLOCK_BAR
		    : EQ (AREF (v, 0), make_fixnum (3)) ? NHG_BLOCK_INNER
		    : EQ (AREF (v, 0), make_fixnum (4)) ? NHG_BLOCK_ROW : 0);
  if (by_string)
    {
      /* open where the string goes on past the rows shown */
      struct glyph *g = top->glyphs[TEXT_AREA], *e = g + top->used[TEXT_AREA];
      for (; g < e && !EQ (g->object, own); g++)
	;
      if (top == first && g < e && g->charpos > 0)
	flags |= NHG_BLOCK_OPEN_TOP;
      if (!stopped)
	flags |= NHG_BLOCK_OPEN_BOTTOM;
    }
  else
    {
      if (top == first && MATRIX_ROW_START_CHARPOS (top) > start)
	flags |= NHG_BLOCK_OPEN_TOP;
      if (!stopped && MATRIX_ROW_END_CHARPOS (last) < end)
	flags |= NHG_BLOCK_OPEN_BOTTOM;
    }

  /* An inner panel's padding: the drawn rectangle is shorter, the rows
     are not; none at a side the window cuts off.  */
  if (EQ (AREF (v, 0), make_fixnum (3)))
    {
      int pt = (flags & NHG_BLOCK_OPEN_TOP) ? 0 : max (0, (int) nhwl_block_int (v, 21, 0));
      int pb = (flags & NHG_BLOCK_OPEN_BOTTOM) ? 0 : max (0, (int) nhwl_block_int (v, 22, 0));
      pt = min (pt, y1 - y0);
      pb = min (pb, y1 - y0 - pt);
      y0 += pt;
      y1 -= pb;
    }
  if (nhwl_nblocks == nhwl_blocks_cap)
    nhwl_blocks = xpalloc (nhwl_blocks, &nhwl_blocks_cap, 1, -1,
			   sizeof *nhwl_blocks);
  Lisp_Object op = AREF (v, 7), sop = AREF (v, 11), hop = AREF (v, 13);
  Lisp_Object oop = AREF (v, 15), fop = AREF (v, 17), top_op = AREF (v, 19);
  nhwl_blocks[nhwl_nblocks++]
    = (NhgBlock) { w->sequence_number, clip,
		   { clip.x + x0, y0, x1 - x0, y1 - y0 },
		   flags, (int32_t) nhwl_block_int (v, 5, 0),
		   (int32_t) nhwl_block_int (v, 6, -1),
		   NUMBERP (op) ? (float) XFLOATINT (op) : 1.0f,
		   (int32_t) nhwl_block_beneath (w, v),
		   (int32_t) nhwl_block_int (v, 9, 0),
		   (int32_t) nhwl_block_int (v, 10, -1),
		   NUMBERP (sop) ? (float) XFLOATINT (sop) : 0.0f,
		   (int32_t) nhwl_block_int (v, 12, -1),
		   NUMBERP (hop) ? (float) XFLOATINT (hop) : 0.0f,
		   (int32_t) nhwl_block_int (v, 14, -1),
		   NUMBERP (oop) ? (float) XFLOATINT (oop) : 0.0f,
		   (int32_t) nhwl_block_int (v, 16, -1),
		   NUMBERP (fop) ? (float) XFLOATINT (fop) : 0.0f,
		   (int32_t) nhwl_block_int (v, 18, -1),
		   NUMBERP (top_op) ? (float) XFLOATINT (top_op) : 0.0f };
}

/* Add a filled bar RECT in RGB at OPACITY to the list, clipped to CLIP,
   for window W, its corners rounded by RADIUS device pixels (0: square).  */
static void
nhwl_bar_add (struct window *w, NhgRect clip, NhgRect rect, intmax_t rgb,
	      float opacity, int radius)
{
  if (rgb < 0 || rect.w <= 0 || rect.h <= 0)
    return;
  if (nhwl_nblocks == nhwl_blocks_cap)
    nhwl_blocks = xpalloc (nhwl_blocks, &nhwl_blocks_cap, 1, -1,
			   sizeof *nhwl_blocks);
  nhwl_blocks[nhwl_nblocks++]
    = (NhgBlock) { w->sequence_number, clip, rect,
		   NHG_BLOCK_BAR | NHG_BLOCK_STRIP, max (0, radius),
		   (int32_t) rgb, opacity, (int32_t) rgb };
}

/* The ruler vector's header: [WIDTH RIGHT HEIGHT BAND-RGB BAND-OPACITY
   FLAGS INSET RADIUS FROM LENGTH TOTAL], then its marks.  */
enum { NHWL_RULER_HEAD = 11 };
enum { NHWL_RULER_HIDE_SHORT = 1 };

/* List the overview ruler V of leaf window W, whose text area spans
   TOP to TOP + HEIGHT: a strip WIDTH wide at the window's right inner
   edge less RIGHT, standing for the whole of what the buffer shows.  It
   is the window's scroll bar: the band (the thumb) over the part the
   window shows goes first, INSET in from either side of the strip with
   corners of RADIUS, then each mark at its position's fraction, at
   least HEIGHT tall, inside the strip.  Fractions are of characters, as
   the scroll bar measures, so a scroll needs nothing of Lisp.  When
   TOTAL is positive the buffer holds rows FROM to FROM + LENGTH of a
   view of TOTAL rows (a list the hub sends a window of): the strip
   stands for the TOTAL rows and a position maps into the held rows by
   its character fraction of the buffer.  With FLAGS' hide-short bit,
   nothing is drawn while the window shows all there is.  V is the
   header above then MARK..., each MARK [POS X0 X1 RGB OPACITY HEIGHT
   RADIUS END] (HEIGHT 0: the header's; END nil or a position the mark
   runs down to), as glitnir--ruler-vector makes it.  */
/* The fraction of B's accessible part BEGV..BEGV+SPAN that POS stands
   at: by visible lines when B's text has a line model (Hlidskjalf; folded
   lines left out, so a mark keeps its place when the strip becomes the
   minimap), else by characters.  */
static double
nhwl_ruler_frac (struct buffer *b, ptrdiff_t begv, ptrdiff_t span,
		 ptrdiff_t pos)
{
#ifdef HAVE_NIDHOGG_HISTORY
  double f;
  if (nh_lines_ntracked && nh_lines_fraction (b, pos, &f))
    return f;
#endif
  return (double) (clip_to_bounds (begv, pos, begv + span) - begv) / span;
}

/* The part of leaf window W's buffer B shown: the current matrix's
   first and last text rows (the window's end is not kept valid between
   redisplays), from *START to *END.  True when a row ends the
   accessible text.  */
static bool
nhwl_shown (struct window *w, struct buffer *b, ptrdiff_t *start,
	    ptrdiff_t *end)
{
  *start = *end = marker_position (w->start);
  struct glyph_matrix *m = w->current_matrix;
  if (m)
    {
      struct glyph_row *bottom = MATRIX_BOTTOM_TEXT_ROW (m, w);
      for (struct glyph_row *row = MATRIX_FIRST_TEXT_ROW (m);
	   row < bottom && row->enabled_p; row++)
	{
	  if (row == MATRIX_FIRST_TEXT_ROW (m))
	    *start = MATRIX_ROW_START_CHARPOS (row);
	  *end = MATRIX_ROW_END_CHARPOS (row);
	  /* A row that reaches ZV ends the text too: in a window fitted to
	     its lines the final newline's empty line has no row.  */
	  if (row->ends_at_zv_p || *end >= BUF_ZV (b))
	    return true;
	}
    }
  return false;
}

static void
nhwl_ruler_add (struct window *w, Lisp_Object v, int top, int height)
{
  if (!VECTORP (v) || ASIZE (v) < NHWL_RULER_HEAD || height <= 0)
    return;
  struct buffer *b = XBUFFER (w->contents);
  int width = nhwl_block_int (v, 0, 0);
  int right = nhwl_block_int (v, 1, 0);
  int least = max (1, (int) nhwl_block_int (v, 2, 1));
  intmax_t flags = nhwl_block_int (v, 5, 0);
  int inset = max (0, (int) nhwl_block_int (v, 6, 0));
  int radius = max (0, (int) nhwl_block_int (v, 7, 0));
  intmax_t from = max (0, nhwl_block_int (v, 8, 0));
  intmax_t length = max (0, nhwl_block_int (v, 9, 0));
  intmax_t total = nhwl_block_int (v, 10, 0);
  int x1 = (WINDOW_RIGHT_EDGE_X (w) - WINDOW_RIGHT_DIVIDER_WIDTH (w)
	    - WINDOW_RIGHT_SCROLL_BAR_AREA_WIDTH (w) - right);
  if (width <= 0)
    return;
  NhgRect strip = { x1 - width, top, width, height };
  ptrdiff_t begv = BUF_BEGV (b), span = max (1, BUF_ZV (b) - begv);
  bool spanned = total > 0;
  if (spanned)
    from = min (from, total), length = min (length, total - from);
  /* The fraction of the whole a position stands at: of the buffer's
     visible lines or characters, or of the view's rows through the rows
     held.  */
#define NHWL_RULER_FRAC(pos) nhwl_ruler_frac (b, begv, span, pos)
#define NHWL_RULER_Y(pos)						\
  (top + (int) ((spanned							\
		 ? (from + NHWL_RULER_FRAC (pos) * length) / total	\
		 : NHWL_RULER_FRAC (pos))					\
		* height))

  Lisp_Object op = AREF (v, 4);
  ptrdiff_t start, end;
  bool at_zv = nhwl_shown (w, b, &start, &end);
  /* All of it shows: the first row starts the accessible text, a row
     ends it, and a view's window holds every row.  */
  if ((flags & NHWL_RULER_HIDE_SHORT)
      && start <= begv && at_zv
      && (!spanned || (from == 0 && length >= total)))
    return;
  int y0 = NHWL_RULER_Y (start);
  int y1 = max (y0 + least, NHWL_RULER_Y (end));
  int bw = max (1, width - 2 * inset);
  nhwl_bar_add (w, strip,
		(NhgRect) { strip.x + min (inset, (width - 1) / 2), y0, bw,
			    min (y1, top + height) - y0 },
		nhwl_block_int (v, 3, -1), NUMBERP (op) ? XFLOATINT (op) : 1.0f,
		radius);

  for (ptrdiff_t i = NHWL_RULER_HEAD; i < ASIZE (v); i++)
    {
      Lisp_Object m = AREF (v, i);
      if (!VECTORP (m) || ASIZE (m) < 5)
	continue;
      ptrdiff_t pos = nhwl_block_pos (w, AREF (m, 0));
      if (pos < 0)
	continue;
      int mx0 = clip_to_bounds (0, nhwl_block_int (m, 1, 0), width);
      int mx1 = clip_to_bounds (mx0, nhwl_block_int (m, 2, width), width);
      int mh = ASIZE (m) > 5 ? (int) nhwl_block_int (m, 5, 0) : 0;
      int mr = ASIZE (m) > 6 ? (int) nhwl_block_int (m, 6, 0) : 0;
      if (mh <= 0)
	mh = least;
      /* A mark with an END runs from its position to END's (a run of
	 changed lines), at least its height.  */
      ptrdiff_t endpos = ASIZE (m) > 7 ? nhwl_block_pos (w, AREF (m, 7)) : -1;
      if (endpos > pos)
	mh = max (mh, NHWL_RULER_Y (endpos) - NHWL_RULER_Y (pos) + mh);
      mh = min (mh, height);
      int y = min (NHWL_RULER_Y (pos), top + height - mh);
      Lisp_Object mop = AREF (m, 4);
      nhwl_bar_add (w, strip, (NhgRect) { strip.x + mx0, y, mx1 - mx0, mh },
		    nhwl_block_int (m, 3, -1),
		    NUMBERP (mop) ? XFLOATINT (mop) : 1.0f, mr);
    }
#undef NHWL_RULER_Y
#undef NHWL_RULER_FRAC
}

/* The edges nhg_edges_sync takes, kept between publishes.  */
static NhgEdge *nhwl_edges;
static ptrdiff_t nhwl_nedges, nhwl_edges_cap;

/* The minimaps nhg_minimap_sync takes, kept between publishes.  */
static NhgMinimap *nhwl_minimaps;
static ptrdiff_t nhwl_nminimaps, nhwl_minimaps_cap;
/* Their files' names, copied (a face lookup may move a Lisp string's
   data): each record's FILE is an offset into this until
   nhwl_minimap_files_fix makes it a pointer, just before the sync.  */
static char *nhwl_minimap_names;
static ptrdiff_t nhwl_minimap_names_len, nhwl_minimap_names_cap;
/* Their hunk labels, each TEXT an offset into the names and each
   minimap's HUNKS an offset into these, until nhwl_minimap_files_fix.  */
static NhgMinimapHunk *nhwl_minimap_hunks;
static ptrdiff_t nhwl_minimap_hunks_len, nhwl_minimap_hunks_cap;
/* The most hunk labels a minimap takes.  */
#define NHWL_MINIMAP_HUNKS_MAX 4096
/* The matrix rows that start a line, for the fold arrows: each
   minimap's ROWS an offset into these until nhwl_minimap_files_fix.  */
static NhgMinimapRow *nhwl_minimap_rows;
static ptrdiff_t nhwl_minimap_rows_len, nhwl_minimap_rows_cap;
/* The pins' names, each TEXT an offset into the names and each
   minimap's PINS an offset into these, until nhwl_minimap_files_fix.  */
static NhgMinimapName *nhwl_minimap_pins;
static ptrdiff_t nhwl_minimap_pins_len, nhwl_minimap_pins_cap;

#ifdef NHWL_WAYLAND
extern int32_t nhwl_minimap_label_render (void *, int32_t, const uint8_t *,
					  size_t, uint32_t, uint32_t,
					  uint32_t *, int32_t, int32_t,
					  int32_t *, int32_t *);
#endif

/* Copy LEN bytes at DATA to the minimap names; their offset.  */
static uintptr_t
nhwl_minimap_name_copy (const unsigned char *data, size_t len)
{
  if (nhwl_minimap_names_cap - nhwl_minimap_names_len < (ptrdiff_t) len)
    nhwl_minimap_names = xpalloc (nhwl_minimap_names, &nhwl_minimap_names_cap,
				  len, -1, 1);
  memcpy (nhwl_minimap_names + nhwl_minimap_names_len, data, len);
  uintptr_t at = nhwl_minimap_names_len;
  nhwl_minimap_names_len += len;
  return at;
}

/* Make each listed minimap's FILE and hunk offsets pointers into the
   copied names and hunks.  */
static void
nhwl_minimap_files_fix (void)
{
  for (ptrdiff_t i = 0; i < nhwl_minimap_hunks_len; i++)
    nhwl_minimap_hunks[i].text
      = (const uint8_t *) nhwl_minimap_names
	+ (uintptr_t) nhwl_minimap_hunks[i].text;
  for (ptrdiff_t i = 0; i < nhwl_minimap_pins_len; i++)
    nhwl_minimap_pins[i].text
      = (const uint8_t *) nhwl_minimap_names
	+ (uintptr_t) nhwl_minimap_pins[i].text;
  for (ptrdiff_t i = 0; i < nhwl_nminimaps; i++)
    {
      if (nhwl_minimaps[i].file_len)
	nhwl_minimaps[i].file
	  = (const uint8_t *) nhwl_minimap_names
	    + (uintptr_t) nhwl_minimaps[i].file;
      if (nhwl_minimaps[i].hunks_len)
	nhwl_minimaps[i].hunks
	  = nhwl_minimap_hunks + (uintptr_t) nhwl_minimaps[i].hunks;
      if (nhwl_minimaps[i].pill_format_len)
	nhwl_minimaps[i].pill_format
	  = (const uint8_t *) nhwl_minimap_names
	    + (uintptr_t) nhwl_minimaps[i].pill_format;
      if (nhwl_minimaps[i].number_len)
	nhwl_minimaps[i].number
	  = (const uint8_t *) nhwl_minimap_names
	    + (uintptr_t) nhwl_minimaps[i].number;
      if (nhwl_minimaps[i].heat_len)
	nhwl_minimaps[i].heat
	  = (const uint8_t *) nhwl_minimap_names
	    + (uintptr_t) nhwl_minimaps[i].heat;
      if (nhwl_minimaps[i].pins_len)
	nhwl_minimaps[i].pins
	  = nhwl_minimap_pins + (uintptr_t) nhwl_minimaps[i].pins;
      if (nhwl_minimaps[i].rows_len)
	nhwl_minimaps[i].rows
	  = nhwl_minimap_rows + (uintptr_t) nhwl_minimaps[i].rows;
    }
}

/* A string slot I of vector V copied to the minimap names: its offset,
   its length into *LEN (0: none).  */
static uintptr_t
nhwl_minimap_string (Lisp_Object v, ptrdiff_t i, size_t *len)
{
  *len = 0;
  if (ASIZE (v) <= i || !STRINGP (AREF (v, i)) || SBYTES (AREF (v, i)) == 0)
    return 0;
  Lisp_Object u = ENCODE_UTF_8 (AREF (v, i));
  *len = SBYTES (u);
  return nhwl_minimap_name_copy (SDATA (u), *len);
}

/* Copy the rows of leaf window W's current matrix that are a line's
   first screen row to the minimap rows: their offset, their count into
   *N (at most 512).  */
static uintptr_t
nhwl_minimap_line_rows (struct window *w, size_t *n)
{
  uintptr_t at = nhwl_minimap_rows_len;
  struct glyph_matrix *m = w->current_matrix;
  *n = 0;
  if (!m)
    return at;
  struct glyph_row *first = MATRIX_FIRST_TEXT_ROW (m);
  struct glyph_row *bottom = MATRIX_BOTTOM_TEXT_ROW (m, w);
  for (struct glyph_row *row = first;
       row < bottom && row->enabled_p && *n < 512; row++)
    {
      if (!row->displays_text_p || MATRIX_ROW_CONTINUATION_LINE_P (row))
	continue;
      if (nhwl_minimap_rows_len == nhwl_minimap_rows_cap)
	nhwl_minimap_rows = xpalloc (nhwl_minimap_rows, &nhwl_minimap_rows_cap,
				     1, -1, sizeof *nhwl_minimap_rows);
      nhwl_minimap_rows[nhwl_minimap_rows_len++] = (NhgMinimapRow) {
	.pos = MATRIX_ROW_START_CHARPOS (row),
	.y = WINDOW_TO_FRAME_PIXEL_Y (w, row->y),
	.h = row->height,
      };
      (*n)++;
    }
  return at;
}

/* List leaf window W's minimap V (the window parameter glitnir--minimap,
   [FLAGS WIDTH SCALE FILL-RGB FILL-OPACITY TEXT-RGB TEXT-OPACITY
   BAND-RGB BAND-OPACITY LABELS FILE LABEL-RGB SUB-RGB ACCENT-RGB MARK-RGB
   LABEL-FACE CLASS-RGB HUNK-RGB HUNKS PILL-RGB PILL-FORMAT NUMBER], HUNKS
   a vector of [LINE TEXT]) and its fold arrows A (glitnir--fold-arrows,
   [MODE SIZE INSET RGB OPACITY FOLDED-RGB FOLDED-OPACITY]) over its
   text area CLIP, when its buffer's text has a line model; the glue
   reads the rows from the model.  Without V, a record for the arrows
   alone.  R is its ruler (glitnir--ruler): the pointer resting on that
   strip opens the minimap.  */
static void
nhwl_minimap_add (struct window *w, Lisp_Object v, Lisp_Object a,
		  Lisp_Object r, NhgRect clip)
{
#ifdef HAVE_NIDHOGG_HISTORY
  bool arrows = VECTORP (a) && ASIZE (a) >= 7 && nhwl_block_int (a, 0, 0) > 0;
  if (!(VECTORP (v) && ASIZE (v) >= 9) && !arrows)
    return;
  if (!nh_lines_ntracked)
    return;
  if (!VECTORP (v) || ASIZE (v) < 9)
    v = make_vector (9, Qnil);
  struct buffer *b = XBUFFER (w->contents);
  uint64_t key = nh_lines_model (b);
  if (!key)
    return;
  ptrdiff_t start, end;
  nhwl_shown (w, b, &start, &end);
  NhgRect strip = { 0, 0, 0, 0 };
  if (VECTORP (r) && ASIZE (r) >= 2 && nhwl_block_int (r, 0, 0) > 0)
    {
      int width = nhwl_block_int (r, 0, 0);
      int x1 = (WINDOW_RIGHT_EDGE_X (w) - WINDOW_RIGHT_DIVIDER_WIDTH (w)
		- WINDOW_RIGHT_SCROLL_BAR_AREA_WIDTH (w)
		- nhwl_block_int (r, 1, 0));
      strip = (NhgRect) { x1 - width, clip.y, width, clip.h };
    }
  if (nhwl_nminimaps == nhwl_minimaps_cap)
    nhwl_minimaps = xpalloc (nhwl_minimaps, &nhwl_minimaps_cap, 1, -1,
			     sizeof *nhwl_minimaps);
  /* The labels: the face first (a lookup may realize it), then the
     file's name copied.  */
  struct frame *f = XFRAME (w->frame);
  uint32_t labels = 0;
  int32_t face = 0;
  uintptr_t file = 0;
  size_t file_len = 0;
  bool pills = ASIZE (v) > 19 && nhwl_block_int (v, 19, -1) >= 0;
  bool number = ASIZE (v) > 21 && STRINGP (AREF (v, 21));
  bool heat = ASIZE (v) > 22 && STRINGP (AREF (v, 22));
  bool pins = (ASIZE (v) > 28 && nhwl_block_int (v, 27, -1) >= 0
	       && nhwl_block_int (v, 28, -1) >= 0);
  if (ASIZE (v) >= 16
      && (nhwl_block_int (v, 9, 0) > 0 || pills || number || pins))
    {
      static bool render_set;
      if (!render_set)
	{
#ifdef NHWL_WAYLAND
	  nhg_minimap_set_label_render (nhwl_minimap_label_render);
#elif defined HAVE_NHMAC
	  nhg_minimap_set_label_render (nhmac_minimap_label_render);
#endif
	  render_set = true;
	}
      labels = max (0, nhwl_block_int (v, 9, 0));
      face = (SYMBOLP (AREF (v, 15)) && !NILP (AREF (v, 15))
	      ? lookup_named_face (NULL, f, AREF (v, 15), false) : -1);
      if (face < 0)
	face = DEFAULT_FACE_ID;
    }
  /* The file's name: the key of the hub's labels and heat in Sjá.  */
  if ((labels || heat) && STRINGP (AREF (v, 10)) && SBYTES (AREF (v, 10)) > 0)
    {
      Lisp_Object u = ENCODE_UTF_8 (AREF (v, 10));
      file_len = SBYTES (u);
      file = nhwl_minimap_name_copy (SDATA (u), file_len);
    }
  /* The hunk labels, with labels on: each [LINE TEXT] copied.  */
  uintptr_t hunks = nhwl_minimap_hunks_len;
  size_t hunks_len = 0;
  Lisp_Object hv = labels && ASIZE (v) > 18 ? AREF (v, 18) : Qnil;
  for (ptrdiff_t i = 0;
       VECTORP (hv) && i < ASIZE (hv) && hunks_len < NHWL_MINIMAP_HUNKS_MAX;
       i++)
    {
      Lisp_Object h = AREF (hv, i);
      if (!VECTORP (h) || ASIZE (h) < 2 || !FIXNATP (AREF (h, 0))
	  || !STRINGP (AREF (h, 1)))
	continue;
      Lisp_Object u = ENCODE_UTF_8 (AREF (h, 1));
      if (nhwl_minimap_hunks_len == nhwl_minimap_hunks_cap)
	nhwl_minimap_hunks = xpalloc (nhwl_minimap_hunks,
				      &nhwl_minimap_hunks_cap, 1, -1,
				      sizeof *nhwl_minimap_hunks);
      nhwl_minimap_hunks[nhwl_minimap_hunks_len++] = (NhgMinimapHunk) {
	.line = XFIXNAT (AREF (h, 0)),
	.text = (const uint8_t *) nhwl_minimap_name_copy (SDATA (u),
							  SBYTES (u)),
	.len = SBYTES (u),
      };
      hunks_len++;
    }
  /* The pins' names: each copied.  */
  uintptr_t pin_names = nhwl_minimap_pins_len;
  size_t pins_len = 0;
  Lisp_Object pv = pins && ASIZE (v) > 29 ? AREF (v, 29) : Qnil;
  for (ptrdiff_t i = 0;
       VECTORP (pv) && i < ASIZE (pv) && pins_len < NHWL_MINIMAP_HUNKS_MAX;
       i++)
    {
      Lisp_Object u = (STRINGP (AREF (pv, i)) ? ENCODE_UTF_8 (AREF (pv, i))
		       : empty_unibyte_string);
      if (nhwl_minimap_pins_len == nhwl_minimap_pins_cap)
	nhwl_minimap_pins = xpalloc (nhwl_minimap_pins,
				     &nhwl_minimap_pins_cap, 1, -1,
				     sizeof *nhwl_minimap_pins);
      nhwl_minimap_pins[nhwl_minimap_pins_len++] = (NhgMinimapName) {
	.text = (const uint8_t *) nhwl_minimap_name_copy (SDATA (u),
							  SBYTES (u)),
	.len = SBYTES (u),
      };
      pins_len++;
    }
  /* The heat layer's name.  */
  size_t heat_len;
  uintptr_t heat_at = nhwl_minimap_string (v, 22, &heat_len);
  /* The pill's format and the window's number.  */
  size_t pill_format_len, number_len;
  uintptr_t pill_format = nhwl_minimap_string (v, 20, &pill_format_len);
  uintptr_t number_at = nhwl_minimap_string (v, 21, &number_len);
  /* The fold arrows: the left fringe and the rows that start a line.  */
  NhgRect fringe = { 0, 0, 0, 0 };
  size_t rows_len = 0;
  uintptr_t rows = 0;
  if (arrows)
    {
      /* The fringe is next to the text, or outside the left margin.  */
      int fw = WINDOW_LEFT_FRINGE_WIDTH (w);
      int fx = (clip.x - fw
		- (WINDOW_HAS_FRINGES_OUTSIDE_MARGINS (w)
		   ? WINDOW_LEFT_MARGIN_WIDTH (w) : 0));
      fringe = (NhgRect) { fx, clip.y, fw, clip.h };
      rows = nhwl_minimap_line_rows (w, &rows_len);
    }
#define NHWL_MINIMAP_OPACITY(i) \
  (NUMBERP (AREF (v, i)) ? (float) XFLOATINT (AREF (v, i)) : 1.0f)
  nhwl_minimaps[nhwl_nminimaps++] = (NhgMinimap) {
    .seq = w->sequence_number,
    .clip = clip,
    .key = key,
    .start = start,
    .end = max (start, end),
    .begv = BUF_BEGV (b),
    .zv = BUF_ZV (b),
    .flags = (nhwl_block_int (v, 0, 0)
	      | (NILP (AREF (v, 0)) ? NHG_MINIMAP_NONE : 0)),
    .width = max (0, nhwl_block_int (v, 1, 0)),
    .scale = max (1, nhwl_block_int (v, 2, 2)),
    .fill_rgb = nhwl_block_int (v, 3, -1),
    .fill_opacity = NHWL_MINIMAP_OPACITY (4),
    .text_rgb = nhwl_block_int (v, 5, -1),
    .text_opacity = NHWL_MINIMAP_OPACITY (6),
    .band_rgb = nhwl_block_int (v, 7, -1),
    .band_opacity = NHWL_MINIMAP_OPACITY (8),
    .strip = strip,
    .labels = labels,
    .label_face = face,
    .host = f,
    .file = (const uint8_t *) file,
    .file_len = file_len,
    .label_rgb = labels ? nhwl_block_int (v, 11, -1) : -1,
    .sub_rgb = labels ? nhwl_block_int (v, 12, -1) : -1,
    .accent_rgb = ASIZE (v) > 13 ? nhwl_block_int (v, 13, -1) : -1,
    .mark_rgb = labels ? nhwl_block_int (v, 14, -1) : -1,
  };
#undef NHWL_MINIMAP_OPACITY
  /* CLASS-RGB: each display class's colour (nil or -1: the bars').  */
  Lisp_Object classes = ASIZE (v) > 16 ? AREF (v, 16) : Qnil;
  NhgMinimap *m = &nhwl_minimaps[nhwl_nminimaps - 1];
  for (int c = 0; c < 16; c++)
    m->class_rgb[c] = (VECTORP (classes) && c < ASIZE (classes)
		       ? nhwl_block_int (classes, c, -1) : -1);
  m->hunk_rgb = labels && ASIZE (v) > 17 ? nhwl_block_int (v, 17, -1) : -1;
  m->hunks = (const NhgMinimapHunk *) hunks;
  m->hunks_len = hunks_len;
  m->pill_rgb = pills ? nhwl_block_int (v, 19, -1) : -1;
  m->heat = (const uint8_t *) heat_at;
  m->heat_len = heat_len;
  if (heat_len)
    {
#define NHWL_MINIMAP_OPACITY(i) \
  (NUMBERP (AREF (v, i)) ? (float) XFLOATINT (AREF (v, i)) : 1.0f)
      m->heat_low_rgb = ASIZE (v) > 26 ? nhwl_block_int (v, 23, -1) : -1;
      m->heat_low_opacity = ASIZE (v) > 26 ? NHWL_MINIMAP_OPACITY (24) : 0;
      m->heat_high_rgb = ASIZE (v) > 26 ? nhwl_block_int (v, 25, -1) : -1;
      m->heat_high_opacity = ASIZE (v) > 26 ? NHWL_MINIMAP_OPACITY (26) : 0;
#undef NHWL_MINIMAP_OPACITY
    }
  else
    m->heat_low_rgb = m->heat_high_rgb = -1;
  m->trail_rgb = ASIZE (v) > 32 ? nhwl_block_int (v, 30, -1) : -1;
  m->trail_opacity = (ASIZE (v) > 32 && NUMBERP (AREF (v, 31))
		      ? XFLOATINT (AREF (v, 31)) : 0);
  m->trail_length = (ASIZE (v) > 32
		     ? min (64, max (0, nhwl_block_int (v, 32, 20))) : 0);
  m->pin_kind = pins ? nhwl_block_int (v, 27, -1) : -1;
  m->pin_rgb = pins ? nhwl_block_int (v, 28, -1) : -1;
  m->pins = (const NhgMinimapName *) pin_names;
  m->pins_len = pins_len;
  m->pill_format = (const uint8_t *) pill_format;
  m->pill_format_len = pill_format_len;
  m->number = (const uint8_t *) number_at;
  m->number_len = number_len;
  if (arrows)
    {
      m->arrows = nhwl_block_int (a, 0, 0);
      m->fringe = fringe;
      m->arrow_size = max (0, nhwl_block_int (a, 1, 0));
      m->arrow_inset = max (0, nhwl_block_int (a, 2, 0));
      m->arrow_rgb = nhwl_block_int (a, 3, -1);
      m->arrow_opacity = NUMBERP (AREF (a, 4)) ? XFLOATINT (AREF (a, 4)) : 1.0f;
      m->folded_rgb = nhwl_block_int (a, 5, -1);
      m->folded_opacity = NUMBERP (AREF (a, 6)) ? XFLOATINT (AREF (a, 6)) : 1.0f;
      m->rows = (const NhgMinimapRow *) rows;
      m->rows_len = rows_len;
    }
#endif
}

/* Where position POS of leaf window W's buffer is drawn, plus DX and
   DY: the left of its glyph and the top of its row in W's current
   matrix, frame pixels, into P.  A position not on the rows shown sits
   just outside the text area CLIP, one line above it (before the rows)
   or below it, at its column times the frame's column width plus DX:
   an edge to a card scrolled out leaves the window there.  False for a
   position outside the buffer's accessible part.  */
static bool
nhwl_edge_point (struct window *w, ptrdiff_t pos, int dx, int dy,
		 NhgRect clip, NhgPoint *p)
{
  struct buffer *b = XBUFFER (w->contents);
  struct frame *f = XFRAME (w->frame);
  struct glyph_matrix *m = w->current_matrix;
  bool above = pos < marker_position (w->start);

  if (pos < BUF_BEGV (b) || pos > BUF_ZV (b))
    return false;
  if (m)
    {
      struct glyph_row *first = MATRIX_FIRST_TEXT_ROW (m);
      struct glyph_row *bottom = MATRIX_BOTTOM_TEXT_ROW (m, w);
      if (first < bottom && first->enabled_p)
	above = pos < MATRIX_ROW_START_CHARPOS (first);
      for (struct glyph_row *row = first;
	   !above && row < bottom && row->enabled_p; row++)
	{
	  if (MATRIX_ROW_START_CHARPOS (row) > pos)
	    break;
	  if (MATRIX_ROW_END_CHARPOS (row) <= pos && !row->ends_at_zv_p)
	    continue;
	  int x = row->x;
	  struct glyph *g = row->glyphs[TEXT_AREA];
	  struct glyph *end = g + row->used[TEXT_AREA];
	  for (; g < end && !(BUFFERP (g->object) && g->charpos >= pos); g++)
	    x += g->pixel_width;
	  *p = (NhgPoint) { WINDOW_TEXT_TO_FRAME_PIXEL_X (w, x) + dx,
			    WINDOW_TO_FRAME_PIXEL_Y (w, row->y) + dy };
	  return true;
	}
    }
  /* Off the rows shown: its column, counted back to the line's start
     (at most 4096 characters).  */
  ptrdiff_t i = buf_charpos_to_bytepos (b, pos), begv = BUF_BEGV_BYTE (b);
  int col = 0;
  for (; i > begv && col < 4096; i--)
    {
      int c = BUF_FETCH_BYTE (b, i - 1);
      if (c == '\n')
	break;
      col += CHAR_HEAD_P (c);
    }
  *p = (NhgPoint) { clip.x + col * FRAME_COLUMN_WIDTH (f) + dx,
		    (above ? clip.y - FRAME_LINE_HEIGHT (f)
		     : clip.y + clip.h + FRAME_LINE_HEIGHT (f)) };
  return true;
}

/* Resolve edge V of leaf window W, whose text area is CLIP, into E.  V
   is [FLAGS RGB OPACITY NPOINTS POS DX DY...] as glitnir--edge-vector
   makes it (FLAGS NHG_EDGE_*, RGB 0xRRGGBB; each point a position of
   W's buffer, a marker or an integer, and pixels from its glyph's left
   and its row's top).  False for an edge of fewer than 2 points or
   more than NHG_EDGE_POINTS, with a point outside the buffer, or all
   of whose points are before the rows shown or all after them.  */
static bool
nhwl_edge_resolve (struct window *w, Lisp_Object v, NhgRect clip, NhgEdge *e)
{
  if (!VECTORP (v) || ASIZE (v) < 4)
    return false;
  intmax_t n = nhwl_block_int (v, 3, 0);
  if (n < 2 || n > NHG_EDGE_POINTS || ASIZE (v) < 4 + 3 * n)
    return false;
  Lisp_Object op = AREF (v, 2);
  *e = (NhgEdge) { .seq = w->sequence_number, .clip = clip,
		   .npoints = n, .flags = nhwl_block_int (v, 0, 0),
		   .rgb = nhwl_block_int (v, 1, -1),
		   .opacity = NUMBERP (op) ? XFLOATINT (op) : 1.0f };
  /* An edge whose points are all before the rows shown, or all after
     them, runs outside the text area: none of it is drawn, so it is
     not resolved (resolving a far position costs a walk back to its
     line's start).  */
  struct glyph_matrix *m = w->current_matrix;
  if (m)
    {
      struct glyph_row *first = MATRIX_FIRST_TEXT_ROW (m);
      struct glyph_row *bottom = MATRIX_BOTTOM_TEXT_ROW (m, w), *last = NULL;
      for (struct glyph_row *row = first; row < bottom && row->enabled_p; row++)
	last = row;
      if (last)
	{
	  ptrdiff_t start = MATRIX_ROW_START_CHARPOS (first);
	  ptrdiff_t end = (last->ends_at_zv_p ? PTRDIFF_MAX
			   : MATRIX_ROW_END_CHARPOS (last));
	  int before = 0, after = 0;
	  for (int i = 0; i < n; i++)
	    {
	      ptrdiff_t pos = nhwl_block_pos (w, AREF (v, 4 + 3 * i));
	      before += pos >= 0 && pos < start;
	      after += pos >= end;
	    }
	  if (before == n || after == n)
	    return false;
	}
    }
  for (int i = 0; i < n; i++)
    {
      ptrdiff_t pos = nhwl_block_pos (w, AREF (v, 4 + 3 * i));
      if (pos < 0
	  || !nhwl_edge_point (w, pos, nhwl_block_int (v, 5 + 3 * i, 0),
			       nhwl_block_int (v, 6 + 3 * i, 0), clip,
			       &e->points[i]))
	return false;
    }
  return true;
}

/* List the edges V (the window parameter glitnir--edges, a vector of
   edge vectors) of leaf window W, whose text area is CLIP.  */
static void
nhwl_edges_add (struct window *w, Lisp_Object v, NhgRect clip)
{
  if (!VECTORP (v))
    return;
  for (ptrdiff_t i = 0; i < ASIZE (v); i++)
    {
      if (nhwl_nedges == nhwl_edges_cap)
	nhwl_edges = xpalloc (nhwl_edges, &nhwl_edges_cap, 1, -1,
			      sizeof *nhwl_edges);
      if (nhwl_edge_resolve (w, AREF (v, i), clip, &nhwl_edges[nhwl_nedges]))
	nhwl_nedges++;
    }
}

/* The leaf window under WINDOW and its next siblings whose sequence
   number is SEQ, or nil.  */
static Lisp_Object
nhwl_window_of_seq (Lisp_Object window, EMACS_INT seq)
{
  for (; !NILP (window); window = XWINDOW (window)->next)
    {
      struct window *w = XWINDOW (window);
      if (WINDOWP (w->contents))
	{
	  Lisp_Object in = nhwl_window_of_seq (w->contents, seq);
	  if (!NILP (in))
	    return in;
	}
      else if (w->sequence_number == seq)
	return window;
    }
  return Qnil;
}

/* The glue's NHG_ACT_MINIMAP A for frame F: publish F's tree (a minimap's
   reveal or state changed: no redisplay), or hand Lisp the click, drag
   step or dwell as a `glitnir-minimap' event.  */
static int
nhwl_minimap_action (struct frame *f, const NhgAction *a,
		     struct input_event *hold_quit)
{
  if (a->code == NHG_MINIMAP_PUBLISH)
    {
      nhwl_publish (f);
      return 0;
    }
  Lisp_Object window = nhwl_window_of_seq (FRAME_ROOT_WINDOW (f),
					   (EMACS_INT) a->dy);
  if (NILP (window))
    return 0;
  Lisp_Object kind = (a->code == NHG_MINIMAP_CLICK ? Qnhwl_minimap_click
		      : a->code == NHG_MINIMAP_DRAG ? Qnhwl_minimap_drag
		      : a->code == NHG_MINIMAP_DWELL_MARKS
		      ? Qnhwl_minimap_dwell_marks
		      : a->code == NHG_MINIMAP_DWELL_LABEL
		      ? Qnhwl_minimap_dwell_label
		      : Qnhwl_minimap_dwell);
  struct input_event ie;
  EVENT_INIT (ie);
  ie.kind = NHWL_MINIMAP_EVENT;
  ie.arg = list3 (kind, window, make_fixnum ((EMACS_INT) a->dx));
  XSETFRAME (ie.frame_or_window, f);
  kbd_buffer_store_event_hold (&ie, hold_quit);
  return 1;
}

/* List the blocks of WINDOW's leaf windows and its next siblings' (the
   window parameter glitnir--blocks, a vector of block vectors), their
   overview rulers (glitnir--ruler) and their edges (glitnir--edges).  */
static void
nhwl_blocks_add (Lisp_Object window)
{
  for (; !NILP (window); window = XWINDOW (window)->next)
    {
      struct window *w = XWINDOW (window);

      if (WINDOWP (w->contents))
	{
	  nhwl_blocks_add (w->contents);
	  continue;
	}
      Lisp_Object v = Fwindow_parameter (window, Qglitnir__blocks);
      Lisp_Object r = Fwindow_parameter (window, Qglitnir__ruler);
      Lisp_Object e = Fwindow_parameter (window, Qglitnir__edges);
      Lisp_Object mm = Fwindow_parameter (window, Qglitnir__minimap);
      Lisp_Object fa = Fwindow_parameter (window, Qglitnir__fold_arrows);
      if ((!VECTORP (v) && !VECTORP (r) && !VECTORP (e) && !VECTORP (mm)
	   && !VECTORP (fa))
	  || !BUFFERP (w->contents))
	continue;
      int x, y, width, height;
      window_box (w, TEXT_AREA, &x, &y, &width, &height);
      NhgRect clip = { x, y, width, height };
      if (VECTORP (v))
	for (ptrdiff_t i = 0; i < ASIZE (v); i++)
	  nhwl_block_add (w, AREF (v, i), clip);
      nhwl_ruler_add (w, r, y, height);
      nhwl_edges_add (w, e, clip);
      nhwl_minimap_add (w, mm, fa, r, clip);
    }
}

DEFUN ("glitnir-minimap-flash", Fglitnir_minimap_flash,
       Sglitnir_minimap_flash, 2, 2, 0,
       doc: /* Light the row of POS in WINDOW's minimap for 400 ms (a jump's flash).
The row is drawn in the minimap's `:accent', fading on the compositor's
frame clock (with reduced motion: lit for 400 ms, then gone); no Lisp
runs meanwhile.  Return t, or nil when WINDOW has no minimap open.  */)
  (Lisp_Object window, Lisp_Object pos)
{
  struct window *w = decode_live_window (window);
  CHECK_FIXNUM_COERCE_MARKER (pos);
  struct frame *f = XFRAME (w->frame);
  if (!FRAME_NHWL_P (f))
    return Qnil;
  struct nhwl_output *out = FRAME_NHWL_OUTPUT (f);
  struct nhwl_display_info *dpyinfo = FRAME_DISPLAY_INFO (f);
  if (!out->glitnir_id || !dpyinfo->glue)
    return Qnil;
  int32_t rc = NHG (nhg_minimap_flash (dpyinfo->glue, out->glitnir_id,
				       w->sequence_number, XFIXNUM (pos)));
  return rc > 0 ? Qt : Qnil;
}

DEFUN ("glitnir-edges-resolved", Fglitnir_edges_resolved,
       Sglitnir_edges_resolved, 0, 1, 0,
       doc: /* The points of WINDOW's edges as C resolves them when its frame publishes.
WINDOW defaults to the selected window.  A list with an entry for each
edge of its parameter `glitnir--edges': a list of (X . Y), device
pixels from the origin of the window's text area, or nil for an edge
left out.  For tests.  */)
  (Lisp_Object window)
{
  struct window *w = decode_live_window (window);
  Lisp_Object out = Qnil;

  XSETWINDOW (window, w);
  Lisp_Object v = Fwindow_parameter (window, Qglitnir__edges);
  if (!VECTORP (v) || !BUFFERP (w->contents))
    return Qnil;
  int x, y, width, height;
  window_box (w, TEXT_AREA, &x, &y, &width, &height);
  NhgRect clip = { x, y, width, height };
  for (ptrdiff_t i = 0; i < ASIZE (v); i++)
    {
      NhgEdge e;
      Lisp_Object pts = Qnil;
      if (nhwl_edge_resolve (w, AREF (v, i), clip, &e))
	for (int j = e.npoints - 1; j >= 0; j--)
	  pts = Fcons (Fcons (make_fixnum (e.points[j].x - x),
			      make_fixnum (e.points[j].y - y)), pts);
      out = Fcons (pts, out);
    }
  return Fnreverse (out);
}

/* Before F's tree is published: F and each drawable child
   frame under it give the glue their windows' blocks, while the frame
   has some (its parameter glitnir--blocks) or block layers left.  Each
   frame is a Glitnir root of its own, so a child frame's blocks are in
   its own coordinates and follow its moves and scale.  */
static void
nhwl_blocks_sync (struct frame *f)
{
  struct nhwl_output *out = FRAME_NHWL_OUTPUT (f);
  struct nhwl_display_info *dpyinfo = FRAME_DISPLAY_INFO (f);
  Lisp_Object tail, frame;

  if (!nhwl_frame_drawable (f))
    return;
  if (out->glitnir_id
      && (!NILP (get_frame_param (f, Qglitnir__blocks))
	  || nhwl_glue_rc (dpyinfo, NHG (nhg_blocks_count (dpyinfo->glue,
							   out->glitnir_id))) > 0
	  || nhwl_glue_rc (dpyinfo, NHG (nhg_edges_count (dpyinfo->glue,
							  out->glitnir_id))) > 0
	  || nhwl_glue_rc (dpyinfo, NHG (nhg_minimap_count (dpyinfo->glue,
							    out->glitnir_id))) > 0))
    {
      nhwl_nblocks = nhwl_nedges = nhwl_nminimaps = 0;
      nhwl_minimap_names_len = nhwl_minimap_hunks_len = 0;
      nhwl_minimap_rows_len = nhwl_minimap_pins_len = 0;
      nhwl_blocks_add (FRAME_ROOT_WINDOW (f));
      nhwl_glue_rc (dpyinfo,
		    NHG (nhg_blocks_sync (dpyinfo->glue, out->window,
					  out->glitnir_id, nhwl_blocks,
					  nhwl_nblocks)));
      /* After the blocks: the edge layers go on top of them.  */
      nhwl_glue_rc (dpyinfo,
		    NHG (nhg_edges_sync (dpyinfo->glue, out->window,
					 out->glitnir_id, nhwl_edges,
					 nhwl_nedges)));
      /* After the edges: the minimaps cover the text's edge, above all.  */
      nhwl_minimap_files_fix ();
      nhwl_glue_rc (dpyinfo,
		    NHG (nhg_minimap_sync (dpyinfo->glue, out->window,
					   out->glitnir_id, nhwl_minimaps,
					   nhwl_nminimaps)));
    }
  FOR_EACH_FRAME (tail, frame)
    if (nhwl_child_of (XFRAME (frame), f))
      nhwl_blocks_sync (XFRAME (frame));
}

#ifdef HAVE_NHMAC
/* How long a published frame may wait for its commit while Lisp runs on
   after a redisplay without waiting (the shell commits at its next pump
   or wait; NS's Core Animation at its run loop's turn).  */
#define NHWL_PRESENT_DEADLINE_NS (8 * 1000 * 1000)

static void nhwl_present_deadline (struct atimer *);

static void
nhwl_present_deadline_arm (struct nhwl_display_info *dpyinfo)
{
  if (!dpyinfo->present_deadline)
    {
      dpyinfo->present_deadline
	= start_atimer (ATIMER_RELATIVE,
			make_timespec (0, NHWL_PRESENT_DEADLINE_NS),
			nhwl_present_deadline, dpyinfo);
      /* After the atimer's start, so the kick finds it due.  */
      nhmac_deadline_wake (NHWL_PRESENT_DEADLINE_NS);
    }
}

/* The deadline came: the shell commits what it holds.  Inside a
   redisplay the commit waits for the redisplay's own publish.  */
static void
nhwl_present_deadline (struct atimer *timer)
{
  struct nhwl_display_info *dpyinfo = timer->client_data;

  dpyinfo->present_deadline = NULL;
  if (!dpyinfo->glue)
    return;
  if (redisplaying_p)
    {
      nhwl_present_deadline_arm (dpyinfo);
      return;
    }
  block_input ();
  nhwl_glue_rc (dpyinfo, NHG (nhg_flush_presents (dpyinfo->glue)));
  unblock_input ();
}

#endif

/* A consistency point for F's tree (F, its root, a top-level, tip or popup
   frame, and the child frames under it, sync subsurfaces): Emacs is not
   redisplaying and buffer flips are not blocked, so every frame of the tree
   is drawn completely.  Bring the tree up to date and publish it; the shell
   presents it (on its thread, or at the root's FRAME_READY).  The glue
   publishes nothing when the root may not show a buffer, or when nothing
   was drawn.  The key time is the root's (keyboard focus is a top-level
   window's): a packet carries it, and a key read before a redisplay that
   drew nothing anywhere in the tree stops waiting for a paint to be timed
   by.  Called after every redisplay (the buffer flips unblocked) and after
   a draw outside one.  */
void
nhwl_publish (struct frame *f)
{
  struct frame *r = nhwl_tree_root (f);
  struct nhwl_output *out = FRAME_NHWL_OUTPUT (r);
  struct nhwl_display_info *dpyinfo = FRAME_DISPLAY_INFO (r);
  uint32_t flags = NHG_PRESENT_BACKSTOP;

  if (!dpyinfo->glue || !out->window || redisplaying_p
      || buffer_flipping_blocked_p ())
    return;
  if (out->t_key_ns && out->key_redisplay != redisplay_counter
      && !kbd_buffer_events_waiting ())
    flags |= NHG_PRESENT_KEY_SEEN;
  block_input ();
  nhwl_nsurfaces = 0;
  nhwl_surface_add (r, 0, true);
  nhwl_layers_sync (r);
  nhwl_blocks_sync (r);
  int32_t rc = nhwl_glue_rc (dpyinfo,
			     NHG (nhg_present (dpyinfo->glue, out->window,
					       nhwl_surfaces, nhwl_nsurfaces,
					       flags, out->t_key_ns)));
  if (rc > 0 && (rc & NHG_PRESENTED_KEY_DONE))
    out->t_key_ns = 0;
#ifdef HAVE_NHMAC
  if (rc > 0)
    nhwl_present_deadline_arm (dpyinfo);
#endif
  unblock_input ();
}

/* Root R's tree and every popup (the shell keeps those hanging from it),
   brought up to date for glitnir-snapshot, into *S; how many there are.  */
ptrdiff_t
nhwl_snapshot_list (struct frame *r, const NhgSurface **s)
{
  Lisp_Object tail, frame;

  nhwl_publish (r);
  nhwl_nsurfaces = 0;
  nhwl_surface_add (r, 0, true);
  FOR_EACH_FRAME (tail, frame)
    if (nhwl_popup_frame_p (XFRAME (frame))
	&& FRAME_NHWL_OUTPUT (XFRAME (frame))->window)
      nhwl_surface_add (XFRAME (frame), 0, true);
  *s = nhwl_surfaces;
  return nhwl_nsurfaces;
}

/* The pacer of root window ROOT's tree lets it present (inline mode): the
   shell presents what was published.  It needs nothing of Emacs, so it runs
   whatever Emacs is doing, drawn or not: a tree that owes a commit (a bare
   ack, surface state, the re-arm of a hidden window's pacer) gets it here.
   True when the root committed a buffer.  */
static bool
nhwl_present_ready (struct nhwl_display_info *dpyinfo, uint64_t root)
{
  int32_t rc;

  if (!dpyinfo->glue || !root)
    return false;
  rc = nhwl_glue_rc (dpyinfo,
		     NHG (nhg_present (dpyinfo->glue, root, NULL, 0,
				       NHG_PRESENT_READY, 0)));
  return rc > 0 && (rc & NHG_PRESENTED_COMMITTED);
}

#ifdef NHWL_WAYLAND
/* Adapted from pgtk_clear_area.  */
void
nhwl_clear_area (struct frame *f, int x, int y, int width, int height)
{
  cairo_t *cr;

  eassert (width > 0 && height > 0);

  cr = nhwl_begin_cr_clip (f);
  nhwl_damage (f, x, y, width, height);
  /* The label anchors under the clear go with it (PD).  */
  glitnir_decor_clear (f, x, y, width, height);
  nhwl_set_cr_source_with_color (f, FRAME_NHWL_OUTPUT (f)->background_color,
				 true);
  cairo_rectangle (cr, x, y, width, height);
  cairo_fill (cr);
  nhwl_end_cr_clip (f);
}
#endif /* NHWL_WAYLAND */

/* While nhwl_clear_end_of_line clears a text row's end: the row's
   glitnir_fill, which the cleared area shows in place of the
   background.  */
static unsigned int nhwl_row_fill;

static void
nhwl_clear_frame_area (struct frame *f, int x, int y, int width, int height)
{
  if (nhwl_row_fill)
    nhwl_fill_rectangle (f, nhwl_row_fill & 0xffffff, x, y, width, height,
			 true);
  else
    nhwl_clear_area (f, x, y, width, height);
}

/* gui_clear_end_of_line, in the fill of the `row' block over ROW
   (glitnir_row_fill) when it has one.  */
static void
nhwl_clear_end_of_line (struct window *w, struct glyph_row *row,
			enum glyph_row_area area, int to_x)
{
  nhwl_row_fill = area == TEXT_AREA ? row->glitnir_fill : 0;
  gui_clear_end_of_line (w, row, area, to_x);
  nhwl_row_fill = 0;
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

#ifdef NHWL_WAYLAND
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
#endif /* NHWL_WAYLAND */

/* Child frame F's border, WIDTH device pixels (0: none) in face
   colour RGB at alpha ALPHA, for its chrome: the rounded corners keep it
   as a ring, as Glitnir's pgtk path recorded it.  */
static void
nhwl_child_border (struct frame *f, int width, unsigned long rgb,
		   double alpha)
{
  uint32_t a, argb;

  if (!nhwl_child_p (f) || !FRAME_NHWL_OUTPUT (f)->window)
    return;
  alpha = min (max (alpha, 0.0), 1.0);
  a = lround (alpha * 255);
  argb = (a << 24 | ((rgb >> 16 & 0xff) * a / 255) << 16
	  | ((rgb >> 8 & 0xff) * a / 255) << 8 | (rgb & 0xff) * a / 255);
  nhwl_glue_rc (FRAME_DISPLAY_INFO (f),
		NHG (nhg_child_border (FRAME_NHG (f),
				       FRAME_NHWL_OUTPUT (f)->window, width,
				       width > 0 ? argb : 0)));
}

/* Adapted from pgtk_clear_under_internal_border.  */
void
nhwl_clear_under_internal_border (struct frame *f)
{
  if (FRAME_INTERNAL_BORDER_WIDTH (f) <= 0)
    nhwl_child_border (f, 0, 0, 0);
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
      nhwl_child_border (f, border, face ? face->background : 0,
			 face ? f->alpha_background : 0);

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

/* DIAGNOSTIC (dropped before the tag): with NHWL_CURSOR_TRACE set, each
   shape request goes to stderr with its frame, shape, the mouse
   highlight's point and the caller (as an offset from
   nhwl_define_frame_cursor, for addr2line): the first 300, then every
   1000th.  */
static void
nhwl_cursor_trace (struct frame *f, Emacs_Cursor cursor, void *caller)
{
  static int on = -1;
  static uintmax_t n;
  if (on < 0)
    on = getenv ("NHWL_CURSOR_TRACE") != NULL;
  if (!on || (++n > 300 && n % 1000))
    return;
  Mouse_HLInfo *hl = MOUSE_HL_INFO (f);
  struct timespec t = current_timespec ();
  fprintf (stderr,
	   "nhwl-cursor %ju %jd.%06ld frame %p %s shape %d at %d,%d"
	   " lit %d redisplaying %d caller %+td\n",
	   n, (intmax_t) t.tv_sec, t.tv_nsec / 1000, (void *) f,
	   STRINGP (f->name) ? SSDATA (f->name) : "?",
	   (int) ((intptr_t) cursor - 1), hl->mouse_face_mouse_x,
	   hl->mouse_face_mouse_y, !NILP (hl->mouse_face_window),
	   redisplaying_p,
	   (char *) caller - (char *) (void *) nhwl_cursor_trace);
}

/* Adapted from pgtk_define_frame_cursor.  */
static void
nhwl_define_frame_cursor (struct frame *f, Emacs_Cursor cursor)
{
  struct nhwl_output *out = FRAME_NHWL_OUTPUT (f);

  if (!f->pointer_invisible && out->current_cursor != cursor && out->window
      && cursor)
    {
      nhwl_cursor_trace (f, cursor, __builtin_return_address (0));
      block_input ();
      NHG (nhg_window_set_cursor (FRAME_NHG (f), out->window,
				  (uint32_t) ((intptr_t) cursor - 1)));
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
  nhwl_clear_end_of_line,
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

#ifdef HAVE_NHMAC
/* Whether F was asked for at a position (its `left' and `top', from the
   main screen's top-left), where the shell places windows itself.  */
static bool
nhwl_placed_p (struct frame *f)
{
  return ((f->size_hint_flags & (USPosition | PPosition))
	  && !(f->size_hint_flags & (XNegative | YNegative)));
}
#endif

/* The scale frame F starts at, times 120: for a top-level asked for at a
   position, the scale of the screen there (START120, set before its fonts
   open); else its parent's, else the selected frame's (where the user
   works, and where a new top-level opens beside it), else the newest the
   display heard.  */
static uint32_t
nhwl_start_scale (struct frame *f)
{
  struct frame *s = FRAME_PARENT_FRAME (f) ? FRAME_PARENT_FRAME (f) : SELECTED_FRAME ();

  if (!FRAME_PARENT_FRAME (f) && FRAME_NHWL_OUTPUT (f)->start120)
    return FRAME_NHWL_OUTPUT (f)->start120;
  if (s != f && FRAME_NHWL_P (s) && FRAME_DISPLAY_INFO (s) == FRAME_DISPLAY_INFO (f)
      && FRAME_NHWL_OUTPUT (s)->scale120)
    return FRAME_NHWL_OUTPUT (s)->scale120;
  return FRAME_DISPLAY_INFO (f)->scale120 ? FRAME_DISPLAY_INFO (f)->scale120 : 120;
}

/* The scale of frame F's window as a factor (the one it starts at until
   the compositor says).  */
static double
nhwl_frame_scale (struct frame *f)
{
  uint32_t s120 = FRAME_NHWL_OUTPUT (f)->scale120;

  return (s120 ? s120 : nhwl_start_scale (f)) / 120.0;
}

/* FRAME_RES of F: NHWL_DPI times its scale times its root frame's
   `nidhogg-text-zoom' parameter (nidhogg-scale.el sets it).  */
double
nhwl_frame_res (struct frame *f)
{
  Lisp_Object zoom = get_frame_param (nhwl_tree_root (f), Qnidhogg_text_zoom);

  return NHWL_DPI * nhwl_frame_scale (f) * (NUMBERP (zoom) ? XFLOATINT (zoom) : 1);
}

double
nhwl_frame_scale_factor (struct frame *f)
{
  return nhwl_frame_scale (f);
}

/* F's resolution changed from WAS: a config-changed event (nidhogg-scale
   . WAS) has nidhogg-scale.el re-open its fonts from their point sizes,
   and its images, made for the old device size, are made again at the
   new one (from the drain, not inside redisplay).  */
static void
nhwl_res_changed (struct frame *f, double was)
{
  struct input_event ev;

  EVENT_INIT (ev);
  ev.kind = CONFIG_CHANGED_EVENT;
  ev.arg = Fcons (Qnidhogg_scale, make_float (was));
  XSETFRAME (ev.frame_or_window, f);
  kbd_buffer_store_event (&ev);
  clear_image_cache (f, Qnil);
}

/* Logical size of DEV device pixels at scale S120/120, rounded the way
   the presenter rounds the viewport's destination.  */
int
nhwl_to_logical (int dev, uint32_t s120)
{
  int64_t s = max (1, (int64_t) s120);

  return (int) (((int64_t) dev * 120 + s / 2) / s);
}

/* F's native width and height as they are about to be: a size that
   change_frame_size delayed until the next redisplay counts, so a fit
   that follows (a scale, another configure, read in the same go) is
   compared with the size the frame will take, not the one it had.  */
static void
nhwl_frame_size_due (struct frame *f, int *w, int *h)
{
  *w = f->new_size_p && f->new_width >= 0 ? f->new_width : FRAME_PIXEL_WIDTH (f);
  *h = f->new_size_p && f->new_height >= 0 ? f->new_height : FRAME_PIXEL_HEIGHT (f);
}

/* F's facts the glue keeps for its window: visible, iconified, our own
   title bar.  */
static uint32_t
nhwl_window_facts (struct frame *f)
{
  return ((FRAME_VISIBLE_P (f) ? NHG_FACT_VISIBLE : 0)
	  | (FRAME_ICONIFIED_P (f) ? NHG_FACT_ICONIFIED : 0)
	  | (FRAME_NHWL_OUTPUT (f)->csd.on ? NHG_FACT_CSD : 0));
}

/* Make frame F's shell window from DESC (its NUL-terminated strings, the
   rest up to ANCHOR) through the glue, which registers it as a frame of
   kind FLAGS (NHG_FRAME_*) with the facts it starts from (the scale F's
   window starts at, F's native size, its visibility and title bar),
   filled in here; its id goes to *ID.  The glue's code, or 0.  */
int
nhwl_window_new (struct frame *f, NhgWindowDesc *desc, uint32_t flags,
		 uint64_t *id)
{
  struct nhwl_display_info *dpyinfo = FRAME_DISPLAY_INFO (f);
  struct nhwl_output *out = FRAME_NHWL_OUTPUT (f);
  int w = f->new_size_p ? f->new_width : FRAME_PIXEL_WIDTH (f);
  int h = f->new_size_p ? f->new_height : FRAME_PIXEL_HEIGHT (f);

  desc->title_len = desc->title ? strlen (desc->title) : 0;
  desc->app_id_len = desc->app_id ? strlen (desc->app_id) : 0;
  desc->session_name_len
    = desc->session_name ? strlen (desc->session_name) : 0;
  desc->role = flags;
  desc->facts = nhwl_window_facts (f);
  desc->native_w = max (1, w);
  desc->native_h = max (1, h);
  desc->scale120 = out->scale120;

  int rc;
#ifdef HAVE_NHMAC
  /* *ID a window already: it takes the place DESC describes instead,
     keeping its id and its picture (nhwl_set_parent_frame).  */
  if (*id)
    rc = nhwl_glue_rc (dpyinfo, NHG (nhg_window_reparent (dpyinfo->glue,
							   *id, desc)));
  else
#endif
    rc = nhwl_glue_rc (dpyinfo, NHG (nhg_window_new (dpyinfo->glue, desc,
						       id)));
  if (rc >= 0 && FRAME_COLUMN_WIDTH (f) > 0 && FRAME_LINE_HEIGHT (f) > 0)
    nhwl_glue_rc (dpyinfo, NHG (nhg_frame_units (dpyinfo->glue, *id,
						 FRAME_COLUMN_WIDTH (f),
						 FRAME_LINE_HEIGHT (f))));
  return rc;
}

/* The glue forgets WINDOW, whose shell window is gone or going.  A
   poisoned glue answers NHG_E_PANIC, which is ignored here: the display
   is going already.  */
void
nhwl_window_forget (struct nhwl_display_info *dpyinfo, uint64_t window)
{
  nhwl_glue_rc (dpyinfo, NHG (nhg_frame_forget (dpyinfo->glue, window)));
}

/* Destroy shell window WINDOW, and forget it in the glue.  Errors are
   ignored: the window is forgotten all the same, and a lost or poisoned
   display is going already.  */
void
nhwl_window_destroy (struct nhwl_display_info *dpyinfo, uint64_t window)
{
  nhwl_glue_rc (dpyinfo, NHG (nhg_window_destroy (dpyinfo->glue, window)));
}

/* Push F's visibility (ON: visible, not iconified) to the glue.  */
void
nhwl_window_visible (struct frame *f, bool on)
{
  uint64_t window = FRAME_NHWL_OUTPUT (f)->window;

  if (window)
    nhwl_glue_rc (FRAME_DISPLAY_INFO (f),
		  NHG (nhg_window_visible (FRAME_NHG (f), window, on)));
}

/* F's name in the compositor's session, or nil: no session restore, a
   replaced session, or a switch that is off.  Lisp decides (the first
   frame on a display is "initial", the others keep their `session-name'
   parameter or get a generated one) and opens the session at the
   display's first frame, before its window exists, so the window can join
   it before its first commit.  The name is kept in F's parameters, where
   frameset saves it.  Naglfar has no compositor session (nhwl-session.el
   is the Wayland shell's, and is not loaded there): no name, and no call
   into Lisp that would only signal void-function.  */
static Lisp_Object
nhwl_session_name (struct frame *f)
{
#ifdef HAVE_NHMAC
  return Qnil;
#else
  struct nhwl_display_info *dpyinfo = FRAME_DISPLAY_INFO (f);
  bool first = !dpyinfo->session_first_done;
  Lisp_Object frame, name;

  XSETFRAME (frame, f);
  dpyinfo->session_first_done = true;
  if (first)
    safe_calln (Qnhwl_session_start, frame);
  name = safe_calln (Qnhwl_session_name, frame, first ? Qt : Qnil);
  if (!STRINGP (name))
    return Qnil;
  store_in_alist (&f->param_alist, Qsession_name, name);
  return name;
#endif
}

/* Create frame F's shell window at its current pixel size.  Adapted from
   xg_create_frame_widgets' role in pgtk.  IN_PLACE: see nhwl_window_new.  */
void
nhwl_create_frame_window (struct frame *f, uint64_t in_place)
{
  struct nhwl_output *out = FRAME_NHWL_OUTPUT (f);
  struct nhwl_display_info *dpyinfo = FRAME_DISPLAY_INFO (f);
  NhgWindowDesc desc;
  Lisp_Object title;
  Lisp_Object encoded = Qnil;
  Lisp_Object encoded_name = Qnil;
  uint64_t id = in_place;
  int rc;
  /* A frame that had a window (its `parent-frame' changed) starts at its
     new parent's scale: its fonts were open at the old resolution.  */
  double was = out->scale120 ? nhwl_frame_res (f) : 0;

  title = !NILP (f->title) ? f->title : f->name;
  if (STRINGP (title))
    encoded = ENCODE_UTF_8 (title);

  memset (&desc, 0, sizeof desc);
  out->scale120 = nhwl_start_scale (f);
  if (was && was != nhwl_frame_res (f))
    nhwl_res_changed (f, was);
  /* Border widths are logical pixels (nhwlfns.c's nhwl_border_device),
     set before the frame had this scale: made before it had a window, or
     moved to a parent at another one.  */
  nhwl_scale_borders (f);
  desc.w = max (1, nhwl_to_logical (FRAME_PIXEL_WIDTH (f), out->scale120));
  desc.h = max (1, nhwl_to_logical (FRAME_PIXEL_HEIGHT (f), out->scale120));
  desc.title = STRINGP (encoded) ? SSDATA (encoded) : "";
  desc.app_id = "nidhogg";
  desc.flags = FRAME_UNDECORATED (f) ? NHG_WIN_UNDECORATED : 0;
  if (nhwl_child_p (f))
    {
      /* The surface starts its shadow margin before the native area; the
	 glue places it in the parent's surface (logical pixels).  */
      desc.kind = NHG_KIND_CHILD;
      desc.parent = FRAME_NHWL_OUTPUT (FRAME_PARENT_FRAME (f))->window;
      desc.x = f->left_pos - out->csd.l;
      desc.y = f->top_pos - out->csd.t;
    }
  else
    {
      Lisp_Object name = nhwl_session_name (f);

      if (STRINGP (name))
	{
	  encoded_name = ENCODE_UTF_8 (name);
	  desc.session_name = SSDATA (encoded_name);
	}
#ifdef HAVE_NHMAC
      /* It opens at its position, else beside the selected frame, whose
	 scale it starts at (nhwl_start_scale): its first layout is at the
	 scale it shows at, not at the main screen's.  */
      if (nhwl_placed_p (f))
	{
	  desc.flags |= NHG_WIN_PLACED;
	  desc.x = f->left_pos;
	  desc.y = f->top_pos;
	}
      else
	{
	  struct frame *sf = SELECTED_FRAME ();

	  while (FRAME_PARENT_FRAME (sf))
	    sf = FRAME_PARENT_FRAME (sf);
	  if (sf != f && FRAME_NHWL_P (sf)
	      && FRAME_DISPLAY_INFO (sf) == FRAME_DISPLAY_INFO (f))
	    desc.parent = FRAME_NHWL_OUTPUT (sf)->window;
	}
#endif
    }

  block_input ();
  rc = nhwl_window_new (f, &desc,
			nhwl_child_p (f) ? NHG_FRAME_CHILD : 0, &id);
  /* Before unblock_input: a drain there (a pending signal handled) finds
     F by its window.  */
  if (rc >= 0)
    out->window = id;
  unblock_input ();
  if (rc >= 0 && nhwl_test_drain_in_window_new)
    nhwl_drain_events (dpyinfo, NULL);
  if (rc < 0)
    error ("nidhogg-wayland: cannot create a window (%d)", rc);
  /* `alpha' was set before the window existed, or for an older window.  */
  out->alpha_cut = 0;
  out->alpha_host = false;
  nhwl_set_frame_alpha (f);
  nhwl_set_min_size (f);
}

/***********************************************************************
		 The client-side title bar (the glue's)
 ***********************************************************************/

/* The title bar's layout, hit tests, buttons and window geometry are the
   glue's (csd.rs, moved from nhwlcsd.c); C keeps a copy of the border
   (struct nhwl_csd) for its drawing, applies the title bar's actions and
   paints the strip (nhwldraw.c).  */

/* Lay F's border out again: the glue's title bar from the compositor's
   decoration answer, the toplevel's states and the scale, or a child
   frame's shadow margin from its chrome and the scale.  When it changes,
   the frame is drawn again (and the glue places a top-level's child frames
   again).  */
void
nhwl_csd_layout (struct frame *f)
{
  struct nhwl_output *out = FRAME_NHWL_OUTPUT (f);
  struct nhwl_display_info *dpyinfo = FRAME_DISPLAY_INFO (f);
  struct nhwl_csd *c = &out->csd;
  NhgCsdLayout lay;
  int rc;

  /* A popup frame has neither title bar nor margins.  */
  if (nhwl_popup_frame_p (f) || !out->window)
    return;
  rc = nhwl_glue_rc (dpyinfo,
		     NHG (nhg_csd_layout (dpyinfo->glue, out->window,
					  out->scale120,
					  (FRAME_UNDECORATED (f)
					   ? NHG_CSD_UNDECORATED : 0),
					  &lay)));
  if (rc < 0)
    return;
  /* Copied even when unchanged: the glue lays a window made anew out
     from nothing, so its "unchanged" may differ from this copy.  */
  c->on = lay.on != 0;
  c->strip = lay.strip;
  c->l = lay.l, c->t = lay.t, c->r = lay.r, c->b = lay.b;
  if (rc == 1)
    SET_FRAME_GARBAGED (f);
}

/* The title or the title bars' style changed: F is redisplayed, and the
   glue says at the publish whether the strip shows something new.  */
void
nhwl_csd_changed (struct frame *f)
{
  if (FRAME_NHWL_OUTPUT (f)->csd.on)
    fset_redisplay (f);
}

/* The native-area position of the surface point LX, LY, which the shell
   reports in device pixels already (logical times scale).  */
static void
nhwl_csd_native_pos (struct frame *f, double lx, double ly, int *x, int *y)
{
  struct nhwl_csd *c = &FRAME_NHWL_OUTPUT (f)->csd;

  *x = (int) lround (lx) - c->l;
  *y = (int) lround (ly) - c->t;
}

/* Apply a TITLE_BAR action A to F: the glue took a pointer event over F's
   title bar or margins (or the pointer left them).  */
static void
nhwl_title_bar_action (struct frame *f, const NhgAction *a)
{
  struct nhwl_output *out = FRAME_NHWL_OUTPUT (f);

  /* A pointer hidden while typing comes back with the mouse.  */
  if ((a->flags & (NHG_TB_POINTER | NHG_TB_LEFT)) && f->pointer_invisible)
    nhwl_toggle_invisible_pointer (f, false);
  /* A button's hover or press changed.  */
  if (a->flags & NHG_TB_REDRAW)
    nhwl_csd_changed (f);
  if (a->flags & NHG_TB_LEFT)
    {
      /* Emacs sets its own shape again at the next glyph.  */
      out->current_cursor = NULL;
      FRAME_DISPLAY_INFO (f)->last_mouse_glyph_frame = NULL;
    }
  if (a->flags & NHG_TB_POINTER)
    {
      /* The border's shape there; the event goes no further.  */
      if (out->current_cursor != nhwl_cursor_for_shape (a->code))
	{
	  nhwl_cursor_trace (f, nhwl_cursor_for_shape (a->code), NULL);
	  NHG (nhg_window_set_cursor (FRAME_NHG (f), out->window, a->code));
	  out->current_cursor = nhwl_cursor_for_shape (a->code);
	}
      clear_mouse_face (MOUSE_HL_INFO (f));
    }
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
      nhwl_glue_rc (FRAME_DISPLAY_INFO (f),
		    NHG (nhg_window_title (FRAME_NHG (f), out->window,
					   SSDATA (encoded),
					   SBYTES (encoded))));
      unblock_input ();
      nhwl_csd_changed (f);
    }
}

/* F's `undecorated' changed at run time (at creation the shell takes the
   flag with the window).  A top-level asks the compositor again (server
   decorations or none), and our title bar and margins come or go at once
   (nhwl_csd_layout).  A maximized, fullscreen or tiled window keeps the
   compositor's size, so its native area takes the title bar's rows or
   gives them back; a floating one keeps its native size, as on X11, and
   its surface grows or shrinks.  The redraw that follows presents it.  */
void
nhwl_set_undecorated_hint (struct frame *f, bool undecorated)
{
  struct nhwl_output *out = FRAME_NHWL_OUTPUT (f);
  int dw, dh;

  if (!out->window)
    return;
  block_input ();
  nhwl_glue_rc (FRAME_DISPLAY_INFO (f),
		NHG (nhg_window_undecorated (FRAME_NHG (f), out->window,
					     undecorated)));
  unblock_input ();
  nhwl_csd_layout (f);
  if (nhwl_glue_rc (FRAME_DISPLAY_INFO (f),
		    NHG (nhg_frame_fit (FRAME_NHG (f), out->window,
					out->scale120, out->logical_w,
					out->logical_h, out->states,
					FRAME_PIXEL_WIDTH (f),
					FRAME_PIXEL_HEIGHT (f),
					NHG_FIT_UNDECORATED, &dw, &dh)))
      == 1)
    change_frame_size (f, dw, dh, false, true, false);
  nhwl_set_min_size (f);
  SET_FRAME_GARBAGED (f);
}

/* Apply F's `alpha', by pgtk_set_frame_alpha's rule: the active value
   while F is the highlight frame, else the inactive one, never below
   `frame-alpha-lower-limit'.  The glue applies it (the compositor
   multiplies when it can; otherwise the shell multiplies the pixels, and
   the whole frame is damaged for it).  */
void
nhwl_set_frame_alpha (struct frame *f)
{
  struct nhwl_output *out = FRAME_NHWL_OUTPUT (f);
  double alpha = (FRAME_DISPLAY_INFO (f)->highlight_frame == f
		  ? f->alpha[0] : f->alpha[1]);
  double alpha_min = 1.0;
  uint32_t cut = 0;
  int rc;

  if (!out || !out->window)
    return;
  if (FLOATP (Vframe_alpha_lower_limit))
    alpha_min = XFLOAT_DATA (Vframe_alpha_lower_limit);
  else if (FIXNUMP (Vframe_alpha_lower_limit))
    alpha_min = XFIXNUM (Vframe_alpha_lower_limit) / 100.0;
  /* A negative alpha is none: opaque.  */
  if (alpha < 0.0 || alpha > 1.0)
    alpha = 1.0;
  else if (alpha < alpha_min && alpha_min <= 1.0)
    alpha = alpha_min;

  block_input ();
  rc = nhwl_glue_rc (FRAME_DISPLAY_INFO (f),
		     NHG (nhg_window_alpha (FRAME_NHG (f), out->window, alpha,
					    f->alpha_background, &cut)));
  unblock_input ();
  if (rc < 0)
    return;
  if (rc & NHG_ALPHA_DAMAGE)
    nhwl_damage_all (f);
  /* The surface list's copies.  */
  out->alpha_cut = cut;
  out->alpha_host = rc & NHG_ALPHA_HOST;
}

/* Frame F was just sized: give its internal tool bar the height its
   items need at F's new width now, rather than at the next redisplay.
   The tool bar wraps its items at the frame's width (GTK's tool bar is
   a widget outside the native area, one row at any width), and
   redisplay_tool_bar would change its height, and with it F's text or
   native height, after the size was read back.  *HEIGHT, F's native
   height, grows or shrinks with the tool bar, so the text area keeps the
   height asked for.  True when the tool bar's height changed (F is then
   to be sized again at *HEIGHT).  */
static bool
nhwl_tool_bar_fit (struct frame *f, int *height)
{
  int old = FRAME_TOOL_BAR_HEIGHT (f), new;
  Lisp_Object frame;

  if (old == 0 || NILP (Vauto_resize_tool_bars)
      || !WINDOWP (f->tool_bar_window))
    return false;
  XSETFRAME (frame, f);
  /* Measured at the tool bar window's width, which change_frame_size
     has just set.  */
  new = XFIXNUM (Ftool_bar_height (frame, Qt));
  if (new == 0 || new == old
      || (EQ (Vauto_resize_tool_bars, Qgrow_only) && new < old))
    return false;
  FRAME_TOOL_BAR_HEIGHT (f) = new;
  FRAME_TOOL_BAR_LINES (f)
    = (new + FRAME_LINE_HEIGHT (f) - 1) / FRAME_LINE_HEIGHT (f);
  store_frame_param (f, Qtool_bar_lines,
		     make_fixnum (FRAME_TOOL_BAR_LINES (f)));
  /* Redisplay counts the rows again, and finds the height they need.  */
  f->n_tool_bar_rows = 0;
  *height += new - old;
  return true;
}

/* Adapted from pgtk_set_window_size.  The window's size is the size of
   the buffer we present, so the frame takes the size it asks for at
   once (the compositor only suggests), within what a surface may be
   (the glue clamps it).  A child frame or an invisible frame has its new
   size before this returns, as xg_frame_set_char_size gives it them: a
   caller that sizes a child and then places it by its size (posframe,
   any popup fitted to its buffer) reads the new size, not the old one.
   A visible top-level's size waits for the next redisplay, as a
   configure's does.  A visible top-level that is maximized or
   fullscreen keeps the compositor's size, as a GTK window does:
   xdg-shell binds the client to that size (fullscreen: at most it), and
   leaving the state gives the floating size back.  The tool bar of a
   frame sized at once takes its new height at once too
   (nhwl_tool_bar_fit).  */
void
nhwl_set_window_size (struct frame *f, bool change_gravity, int pixelwidth,
		      int pixelheight)
{
  uint64_t window = FRAME_NHWL_OUTPUT (f)->window;

  if (FRAME_VISIBLE_P (f) && !FRAME_PARENT_FRAME (f)
      && (FRAME_NHWL_OUTPUT (f)->states
	  & (NHG_STATE_MAXIMIZED | NHG_STATE_FULLSCREEN)))
    return;

  bool delay = FRAME_VISIBLE_P (f) && !FRAME_PARENT_FRAME (f);

  NHG (nhg_frame_clamp (FRAME_NHG (f), window, &pixelwidth, &pixelheight));
  nhwl_popup_set_size (f, pixelwidth, pixelheight);
  change_frame_size (f, pixelwidth, pixelheight, false, delay, false);
  if (!delay && nhwl_tool_bar_fit (f, &pixelheight))
    {
      NHG (nhg_frame_clamp (FRAME_NHG (f), window, &pixelwidth, &pixelheight));
      nhwl_popup_set_size (f, pixelwidth, pixelheight);
      change_frame_size (f, pixelwidth, pixelheight, false, false, false);
    }
  SET_FRAME_GARBAGED (f);
  if (window)
    nhwl_glue_rc (FRAME_DISPLAY_INFO (f),
		  NHG (nhg_window_size (FRAME_NHG (f), window,
					max (1, pixelwidth),
					max (1, pixelheight))));
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
  /* A child frame has a position: in its parent.  A negative one counts
     from the parent's right or bottom edge (as pgtk_calc_absolute_position
     makes it, without the outer border a Wayland frame lacks).  */
  if (nhwl_child_p (f)
      && (f->size_hint_flags & (XNegative | YNegative)))
    {
      struct frame *p = FRAME_PARENT_FRAME (f);

      if (f->size_hint_flags & XNegative)
	f->left_pos += FRAME_PIXEL_WIDTH (p) - FRAME_PIXEL_WIDTH (f);
      if (f->size_hint_flags & YNegative)
	f->top_pos += FRAME_PIXEL_HEIGHT (p) - FRAME_PIXEL_HEIGHT (f);
      f->size_hint_flags &= ~ (XNegative | YNegative);
    }
#ifdef HAVE_NHMAC
  /* Naglfar's top-level has a position on the screen, as NS's has.  */
  if (!nhwl_child_p (f) && FRAME_NHWL_OUTPUT (f)->window)
    {
      block_input ();
      nhwl_glue_rc (FRAME_DISPLAY_INFO (f),
		    NHG (nhg_window_place (FRAME_NHG (f),
					   FRAME_NHWL_OUTPUT (f)->window,
					   xoff, yoff)));
      unblock_input ();
    }
#endif
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
    {
      /* The size of a wheel step in a finger record.  */
      nhwl_glue_rc (FRAME_DISPLAY_INFO (f),
		    NHG (nhg_frame_units (FRAME_NHG (f),
					  FRAME_NHWL_OUTPUT (f)->window,
					  max (1, FRAME_COLUMN_WIDTH (f)),
					  max (1, FRAME_LINE_HEIGHT (f)))));
      adjust_frame_size (f, FRAME_COLS (f) * FRAME_COLUMN_WIDTH (f),
			 FRAME_LINES (f) * FRAME_LINE_HEIGHT (f), 3,
			 false, Qfont);
      nhwl_set_min_size (f);
    }

  return font_object;
}

/* Tell the compositor F's smallest size (xdg_toplevel.set_min_size, with
   the next commit): one column and one line of text with the bars, as
   GTK's size hints say (xg_wm_set_size_hint), and our title bar (the glue
   gives a child frame's window none).  */
void
nhwl_set_min_size (struct frame *f)
{
  struct nhwl_output *out = FRAME_NHWL_OUTPUT (f);
  int w, h;

  if (!out->window || !FRAME_FONT (f))
    return;
  w = FRAME_TEXT_COLS_TO_PIXEL_WIDTH (f, 1);
  h = (FRAME_TEXT_LINES_TO_PIXEL_HEIGHT (f, 1) + FRAME_MENUBAR_HEIGHT (f)
       + FRAME_TOOLBAR_HEIGHT (f) + out->csd.strip);
#ifdef HAVE_NHMAC
  /* AppKit's minimum is the frame's own (frame-windows-min-size and the
     internal borders), so a drag of an edge stops where the frame does;
     one column was far below it (23 points against 70), and the window
     kept asking for sizes the frame would not take.  */
  {
    Lisp_Object frame, mw, mh;
    int border = 2 * FRAME_INTERNAL_BORDER_WIDTH (f);

    XSETFRAME (frame, f);
    mw = safe_calln (Qframe_windows_min_size, frame, Qt, Qnil, Qt);
    mh = safe_calln (Qframe_windows_min_size, frame, Qnil, Qnil, Qt);
    if (FIXNUMP (mw))
      w = max (w, XFIXNUM (mw) + border);
    if (FIXNUMP (mh))
      h = max (h, (XFIXNUM (mh) + border + FRAME_MENUBAR_HEIGHT (f)
		   + FRAME_TAB_BAR_HEIGHT (f) + FRAME_TOOLBAR_HEIGHT (f)));
  }
#endif
  block_input ();
  nhwl_glue_rc (FRAME_DISPLAY_INFO (f),
		NHG (nhg_window_min_size (FRAME_NHG (f), out->window, w, h)));
  unblock_input ();
}

/* Wait for the compositor's first configure of frame F, handling the
   events that arrive meanwhile: the glue waits up to 20 ms a round for
   something to drain.  Adapted in role from pgtk_wait_for_map_event.  */
void
nhwl_wait_for_configure (struct frame *f)
{
  struct nhwl_output *out = FRAME_NHWL_OUTPUT (f);
  struct nhwl_display_info *dpyinfo = FRAME_DISPLAY_INFO (f);
  double limit = FLOATP (Vnhwl_wait_for_configure_timeout)
    ? XFLOAT_DATA (Vnhwl_wait_for_configure_timeout) : 0;
  int waited_ms = 0;

  int rounds = 0;

  while (!out->configured && dpyinfo->glue && waited_ms < limit * 1000
	 && rounds++ < 4000)
    {
      int rc;

      block_input ();
      rc = nhwl_glue_rc (dpyinfo,
			 NHG (nhg_window_wait_configured (dpyinfo->glue,
							  out->window, 20)));
      unblock_input ();
      if (rc == 1)
	break;
      if (rc == NHG_E_TIMEOUT)
	waited_ms += 20;
      else if (rc < 0)
	break;
      nhwl_drain_events (dpyinfo, NULL);
    }
}

/* Adapted from pgtk_make_frame_visible.  The toplevel is mapped by the
   first buffer we commit after the compositor's first configure.  */
void
nhwl_make_frame_visible (struct frame *f)
{
  struct nhwl_output *out = FRAME_NHWL_OUTPUT (f);

  /* A popup frame's window is made now (nhwl_popup_show).  */
  if (nhwl_popup_frame_p (f))
    {
      nhwl_popup_show (f);
      return;
    }
  /* A shown frame made a top-level again waits for its first configure.  */
  if (!FRAME_VISIBLE_P (f) || !out->configured)
    {
      nhwl_child_push (f, true);
      if (!out->configured)
	nhwl_wait_for_configure (f);
      if (out->configured)
	{
	  if (f->want_fullscreen != FULLSCREEN_NONE)
	    nhwl_set_fullscreen_state (f);
	  SET_FRAME_VISIBLE (f, 1);
	  SET_FRAME_ICONIFIED (f, false);
	  nhwl_window_visible (f, true);
	  out->has_been_visible = true;
	  SET_FRAME_GARBAGED (f);
	}
    }
}

/* Adapted from pgtk_make_frame_invisible.  */
void
nhwl_make_frame_invisible (struct frame *f)
{
  /* A popup frame's window goes (nhwl_popup_hide).  */
  if (nhwl_popup_frame_p (f))
    {
      nhwl_popup_hide (f);
      return;
    }
  nhwl_child_focus_gone (f, false);
  nhwl_child_push (f, false);
  SET_FRAME_VISIBLE (f, 0);
  SET_FRAME_ICONIFIED (f, false);
  nhwl_window_visible (f, false);
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
      nhwl_glue_rc (dpyinfo,
		    NHG (nhg_window_iconify (dpyinfo->glue,
					     FRAME_NHWL_OUTPUT (f)->window)));
      unblock_input ();
    }
  SET_FRAME_VISIBLE (f, 0);
  SET_FRAME_ICONIFIED (f, true);
}

/* Adapted from set_fullscreen_state (pgtkterm.c, emacs-31.1): the glue
   asks the compositor (`fullwidth' and `fullheight' ask for maximized,
   and the configure then stores `maximized').  */
static void
nhwl_set_fullscreen_state (struct frame *f)
{
  struct nhwl_output *out = FRAME_NHWL_OUTPUT (f);
  uint32_t mode;

  if (!out->window)
    return;

  switch (f->want_fullscreen)
    {
    case FULLSCREEN_NONE:
      mode = NHG_FULLSCREEN_NONE;
      store_frame_param (f, Qfullscreen, Qnil);
      break;

    case FULLSCREEN_BOTH:
      mode = NHG_FULLSCREEN_BOTH;
      store_frame_param (f, Qfullscreen, Qfullboth);
      break;

    case FULLSCREEN_MAXIMIZED:
      mode = NHG_FULLSCREEN_MAXIMIZED;
      store_frame_param (f, Qfullscreen, Qmaximized);
      break;

    case FULLSCREEN_WIDTH:
      mode = NHG_FULLSCREEN_WIDTH;
      break;

    case FULLSCREEN_HEIGHT:
      mode = NHG_FULLSCREEN_HEIGHT;
      break;

    default:
      f->want_fullscreen = FULLSCREEN_NONE;
      return;
    }
  block_input ();
  nhwl_glue_rc (FRAME_DISPLAY_INFO (f),
		NHG (nhg_window_fullscreen (FRAME_NHG (f), out->window,
					    mode)));
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

#define CLEAR_IF_EQ(FIELD)	\
  do { if (f == dpyinfo->FIELD) dpyinfo->FIELD = 0; } while (false)

  /* F is never unhighlighted: delete_frame has already removed its
     windows.  Its tree's keys go to its parent.  */
  CLEAR_IF_EQ (highlight_frame);
  nhwl_child_focus_gone (f, true);
  free_frame_faces (f);

  if (f == dpyinfo->x_focus_frame)
    {
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

  if (out->window && dpyinfo->glue)
    {
      nhwl_glue_rc (dpyinfo, NHG (nhg_im_enable (dpyinfo->glue, out->window,
						 0)));
      nhwl_window_destroy (dpyinfo, out->window);
    }
  out->window = 0;

  if (out->bell_timer)
    cancel_atimer (out->bell_timer);
  out->bell_timer = NULL;
#ifdef NHWL_WAYLAND
  nhwl_cr_destroy_frame_context (f);
#endif
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
  if (dpyinfo->glue != NULL)
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
   serial), which the compositor may refuse.  Within a window, a child
   frame takes its top-level's keys (nhwl_child_focus).  */
void
nhwl_focus_frame (struct frame *f, bool noactivate)
{
  /* Focus on our own popup is its top-level's.  */
  if (nhwl_popup_frame_p (f))
    return;
  nhwl_child_focus (f);
  if (!noactivate)
    nhwl_activate_frame (f);
}

/* Ask the compositor to activate the top-level window of F.  */
void
nhwl_activate_frame (struct frame *f)
{
  struct nhwl_output *out = FRAME_NHWL_OUTPUT (nhwl_tree_root (f));

  if (!out->window)
    return;
  block_input ();
  NHG (nhg_window_activate (FRAME_NHG (f), out->window, NULL));
  unblock_input ();
}


/***********************************************************************
				 Focus
 ***********************************************************************/

/* Adapted from frame_highlight and frame_unhighlight (pgtkterm.c): the
   frame has no border to colour; the cursor changes, and `alpha' takes
   its active or inactive value.  */
static void
frame_highlight (struct frame *f)
{
  gui_update_cursor (f, true);
  nhwl_set_frame_alpha (f);
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
    frame_highlight (old_highlight);
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

static void nhwl_im_update (struct frame *);

/* TO, a frame of root R's tree, takes the keys of R's window from now on.
   While R's window has the keyboard, Emacs's focus moves with
   them, as on X11 when a child frame takes the input focus: a focus-out
   for the frame that had it, unless that frame is being deleted (GONE),
   and a focus-in for TO.  */
void
nhwl_focus_route (struct frame *r, struct frame *to, bool gone)
{
  struct nhwl_display_info *dpyinfo = FRAME_DISPLAY_INFO (r);
  struct frame *from = dpyinfo->x_focus_event_frame;
  union buffered_input_event ev;

  FRAME_NHWL_OUTPUT (r)->focus_window
    = to == r ? 0 : FRAME_NHWL_OUTPUT (to)->window;
  if (!from || from == to || nhwl_tree_root (from) != r)
    return;
  EVENT_INIT (ev.ie);
  ev.ie.kind = NO_EVENT;
  ev.ie.arg = Qnil;
  if (gone)
    dpyinfo->x_focus_event_frame = NULL;
  else
    nhwl_focus_changed (false, FOCUS_EXPLICIT, dpyinfo, from, &ev);
  if (ev.ie.kind != NO_EVENT)
    kbd_buffer_store_event (&ev.ie);
  EVENT_INIT (ev.ie);
  ev.ie.kind = NO_EVENT;
  ev.ie.arg = Qnil;
  nhwl_focus_changed (true, FOCUS_EXPLICIT, dpyinfo, to, &ev);
  if (ev.ie.kind != NO_EVENT)
    kbd_buffer_store_event (&ev.ie);
  nhwl_im_update (to);
}


/***********************************************************************
			     Child frames
 ***********************************************************************/

/* A child frame is a sync subsurface of its parent frame's window (shell
   kind NHG_KIND_CHILD), published and presented with its tree: the
   shell commits the child frames deepest first, then the root
   (nhwl_publish).  It has its own backing and its own Glitnir root, like
   any frame; the root's one layer carries the frame's chrome (rounded
   corners, shadow).  The glue holds the child's layout (child.rs): the
   room the shadow needs around the native area, which is the frame's
   border (so the backing, the damage and the pointer use the offset they
   have for a top-level's title bar), the surface's position in its
   parent's, its input region (the native area), its mapping and its
   stacking.  C pushes the frame's facts and redirects the keyboard
   focus.  */

uint64_t
nhwl_frame_window (struct frame *f)
{
  return FRAME_NHWL_P (f) && f->output_data.nhwl
    ? f->output_data.nhwl->window : 0;
}

struct frame *
nhwl_window_frame (uint64_t window)
{
  struct nhwl_display_info *d;
  struct nhwl_output *o;

  if (!window)
    return NULL;
  for (d = x_display_list; d; d = d->next)
    for (o = d->outputs; o; o = o->next_output)
      if (o->window == window)
	return o->frame;
  return NULL;
}

bool
nhwl_child_p (struct frame *f)
{
  /* A popup frame has a parent too, but is a tree of its own.  */
  return (FRAME_NHWL_P (f) && f->output_data.nhwl && FRAME_PARENT_FRAME (f)
	  && !f->output_data.nhwl->popup_frame);
}

/* The root of F's tree: the top-level (or tip) frame whose window's pacer
   presents F with it.  */
struct frame *
nhwl_tree_root (struct frame *f)
{
  while (nhwl_child_p (f))
    f = FRAME_PARENT_FRAME (f);
  return f;
}

/* Whether child frame F's surface stays mapped when what changed is not
   its visibility: shown, or iconified (only the visibility hooks map and
   unmap it).  */
static bool
nhwl_child_shown (struct frame *f)
{
  return FRAME_VISIBLE_P (f) || FRAME_ICONIFIED_P (f);
}

/* Push child frame F's facts to the glue: its parent, where its native
   area sits in the parent's, whether it is SHOWN (its surface mapped),
   whether it is no-accept-focus and whether it waits to be drawn again
   (garbaged: a frame resized and then moved, as posframe shows one), in
   which case the glue sends a new position with that picture rather
   than at once.  A surface mapped again is drawn again whole.  */
void
nhwl_child_push (struct frame *f, bool shown)
{
  struct nhwl_output *out = FRAME_NHWL_OUTPUT (f);
  uint32_t flags = ((shown ? NHG_CHILD_VISIBLE : 0)
		    | (FRAME_NO_ACCEPT_FOCUS (f) ? NHG_CHILD_NO_FOCUS : 0)
		    | (FRAME_GARBAGED_P (f) ? NHG_CHILD_REDRAW : 0));

  if (!nhwl_child_p (f) || !out->window)
    return;
  if (nhwl_glue_rc (FRAME_DISPLAY_INFO (f),
		    NHG (nhg_child_set (FRAME_NHG (f), out->window,
					nhwl_frame_window (FRAME_PARENT_FRAME (f)),
					f->left_pos, f->top_pos, flags)))
      == 1)
    nhwl_damage_all (f);
}

/* F's shell window exists (made, or made again under a new parent).  A
   subsurface has no configure: it is ready.  The glue gets its chrome, the
   margin that gives, and where it sits (and puts it above its
   siblings).  */
void
nhwl_child_created (struct frame *f)
{
  struct nhwl_output *out = FRAME_NHWL_OUTPUT (f);

  if (!nhwl_child_p (f))
    return;
  out->configured = true;
  if (!out->window)
    return;
  nhwl_glue_rc (FRAME_DISPLAY_INFO (f),
		NHG (nhg_child_chrome (FRAME_NHG (f), out->window,
				       out->chrome, NHG_CHROME_SLOTS)));
  nhwl_csd_layout (f);
  nhwl_child_push (f, nhwl_child_shown (f));
}

/* Child frame F moved (its left or top changed), or its
   `no-accept-focus' did: the glue gets its facts, and sends a new
   position with the tree's next commit.  */
void
nhwl_child_reposition (struct frame *f)
{
  nhwl_child_push (f, nhwl_child_shown (f));
}

/* The terminal's frame_raise_lower_hook: among a child's siblings.  A
   top-level cannot restack itself on Wayland; raising one asks the
   compositor to activate it.  */
void
nhwl_frame_raise_lower (struct frame *f, bool raise)
{
  uint64_t window;

  /* The compositor stacks popups.  */
  if (nhwl_popup_frame_p (f))
    return;
  if (!nhwl_child_p (f))
    {
      if (raise)
	nhwl_activate_frame (f);
      return;
    }
  /* The glue keeps the siblings' order and stacks F against its
     neighbour (stacking is the parent's state, so the parent and its
     ancestors owe a commit).  */
  window = FRAME_NHWL_OUTPUT (f)->window;
  if (window)
    nhwl_glue_rc (FRAME_DISPLAY_INFO (f),
		  NHG (nhg_child_raise (FRAME_NHG (f), window, raise,
					FRAME_Z_GROUP (f))));
}

/* Wayland gives the keyboard to top-levels only: the keys and
   input-method events of a top-level's window go to the frame of its tree
   that Emacs focused, as X11's focus redirection does for child frames.  */

/* The frame the keys of root R's window go to: the frame of R's tree
   Emacs focused, while it is shown; else R.  */
struct frame *
nhwl_focus_target (struct frame *r)
{
  struct frame *t = nhwl_window_frame (FRAME_NHWL_OUTPUT (r)->focus_window);

  return (t && t != r && nhwl_tree_root (t) == r && FRAME_VISIBLE_P (t)
	  ? t : r);
}

/* Emacs focuses F (x-focus-frame, or a click in it): F takes the keys of
   its tree, unless it is a `no-accept-focus' child.  */
void
nhwl_child_focus (struct frame *f)
{
  if (nhwl_child_p (f) && FRAME_NO_ACCEPT_FOCUS (f))
    return;
  nhwl_focus_route (nhwl_tree_root (f), f, false);
}

/* Child frame F is being hidden, or deleted (GONE): when it, or a frame
   under it, takes its tree's keys, they go to F's parent.  */
void
nhwl_child_focus_gone (struct frame *f, bool gone)
{
  struct frame *r, *t;

  if (!nhwl_child_p (f))
    return;
  r = nhwl_tree_root (f);
  t = nhwl_window_frame (FRAME_NHWL_OUTPUT (r)->focus_window);
  if (t && (t == f || frame_ancestor_p (f, t)))
    nhwl_focus_route (r, FRAME_PARENT_FRAME (f), gone);
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

  if (mods & NHG_MOD_SHIFT)
    mod |= shift_modifier;
  if (mods & NHG_MOD_CTRL)
    mod |= ctrl_modifier;
  if (mods & NHG_MOD_ALT)
    mod |= mod_alt;
  if (mods & NHG_MOD_META)
    mod |= mod_meta;
  if (mods & NHG_MOD_SUPER)
    mod |= mod_super;
  if (mods & NHG_MOD_HYPER)
    mod |= mod_hyper;
  return mod;
}

/* Convert the glue's KEY action A (a press, a repeat or a release) on
   frame F into an input event.  Adapted from key_press_event
   (pgtkterm.c): the shell has already run the keysym through xkb and
   compose, and the glue has classified it (keys.rs), so A carries the
   keysym, its class and character, and the text the key produced.  For
   a release, INEV holds the press's event.  Return true if INEV holds
   an event.  */
static bool
nhwl_key_event (struct frame *f, const NhgAction *a,
		union buffered_input_event *inev)
{
  uint32_t keysym = (uint32_t) a->code;
  Mouse_HLInfo *hlinfo = MOUSE_HL_INFO (f);
  Lisp_Object c;
  int modifiers;

  bool release = a->flags & NHG_KEY_RELEASE;

  /* If mouse-highlight is an integer, input clears out mouse
     highlighting.  A release is not input.  */
  if (!release && !hlinfo->mouse_face_hidden && FIXNUMP (Vmouse_highlight))
    {
      clear_mouse_face (hlinfo);
      hlinfo->mouse_face_hidden = true;
    }

  modifiers = nhwl_emacs_modifiers (a->mods);
  modifiers |= extra_keyboard_modifiers & (shift_modifier | ctrl_modifier
					   | meta_modifier | alt_modifier
					   | super_modifier | hyper_modifier);

  XSETFRAME (inev->ie.frame_or_window, f);
  inev->ie.modifiers = modifiers;
  inev->ie.timestamp = a->time_ms;
  /* The frame-time record's start: our clock, not the compositor's.  It
     is the root's, also for a key routed to a child frame.  */
  {
    struct nhwl_output *ro = FRAME_NHWL_OUTPUT (nhwl_tree_root (f));

    if (!release && !ro->t_key_ns)
      {
	ro->t_key_ns = nhwl_now_ns ();
	ro->key_redisplay = redisplay_counter;
      }
  }

  /* make_lispy_event turns characters into control characters: the
     keysym is the key's, not the control character's.  */

  /* A keysym that names a character, then the keysym table, then a
     function key or the text's character.  */
  if (a->flags & NHG_KEY_CHAR)
    goto character;
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

  if (a->flags & NHG_KEY_SYMBOL)
    {
      /* make_lispy_event will convert this to a symbolic key.  */
      inev->ie.kind = NON_ASCII_KEYSTROKE_EVENT;
      inev->ie.code = keysym;
      return true;
    }
  if (!(a->flags & NHG_KEY_TEXT))
    return false;
 character:
  inev->ie.kind = (a->x < 128 ? ASCII_KEYSTROKE_EVENT
		   : MULTIBYTE_CHAR_KEYSTROKE_EVENT);
  inev->ie.code = a->x;
  return true;
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

/* Apply a SCALE action: the shell reported a new preferred scale for
   frame F (the glue decided it is new).  The frame's resolution follows
   (nhwl_frame_res, nhwl_res_changed).  */
static void
nhwl_scale_changed (struct frame *f, uint32_t scale120)
{
  double was = nhwl_frame_res (f);
  /* The window keeps its size on the screen (its logical size, AppKit's
     points) and the text follows: the frame takes that logical size in
     device pixels at the new scale.  On macOS too: AppKit keeps the
     window's points but reports no new size, and a frame that kept its
     device pixels would be shown at the old scale's size in points.  */
  uint32_t old = FRAME_NHWL_OUTPUT (f)->scale120;
  int nw, nh;
  nhwl_frame_size_due (f, &nw, &nh);
  int lw = nhwl_to_logical (nw, old);
  int lh = nhwl_to_logical (nh, old);

  FRAME_NHWL_OUTPUT (f)->scale120 = FRAME_DISPLAY_INFO (f)->scale120 = scale120;
  FRAME_DISPLAY_INFO (f)->resx = FRAME_DISPLAY_INFO (f)->resy = NHWL_DPI * scale120 / 120.0;
  nhwl_res_changed (f, was);
  /* Faces hold device widths made at the scale (their boxes, xfaces.c):
     realized again.  */
  face_change = true;
  nhwl_csd_layout (f);
  /* Default fringes are 8 logical pixels (frame.c's
     default_fringe_width): their device width follows the scale.  */
  if (!FIXNUMP (get_frame_param (f, Qleft_fringe)))
    gui_set_left_fringe (f, Qnil, Qnil);
  if (!FIXNUMP (get_frame_param (f, Qright_fringe)))
    gui_set_right_fringe (f, Qnil, Qnil);
  /* Border widths are logical pixels too (nhwlfns.c's
     nhwl_border_device).  */
  nhwl_scale_borders (f);
  if (old && old != scale120)
    {
#ifdef NHWL_WAYLAND
      /* A maximized, fullscreen or tiled window has the compositor's size,
	 which nhwl_set_window_size leaves alone: it takes the last
	 configure's logical size at the new scale instead.  The compositor
	 sends that configure before the scale when an output changes scale
	 (mutter does), and it was fitted at the old one.  Read in one go
	 with the scale, its size is still pending: the fit compares with
	 it (nhwl_frame_size_due), or it would find the frame's old size,
	 equal to the new one, and keep the configure's.  */
      struct nhwl_output *out = FRAME_NHWL_OUTPUT (f);
      int dw, dh;

      if (out->states & (NHG_STATE_MAXIMIZED | NHG_STATE_FULLSCREEN
			 | NHG_STATE_TILED_L | NHG_STATE_TILED_R
			 | NHG_STATE_TILED_T | NHG_STATE_TILED_B))
	{
	  if (nhwl_glue_rc (FRAME_DISPLAY_INFO (f),
			    NHG (nhg_frame_fit (FRAME_NHG (f), out->window,
						scale120, out->logical_w,
						out->logical_h, out->states,
						nw, nh,
						NHG_FIT_UNDECORATED, &dw, &dh)))
	      == 1)
	    change_frame_size (f, dw, dh, false, true, false);
	}
      else
#endif
	nhwl_set_window_size (f, false, (int) (((int64_t) lw * scale120 + 60) / 120),
			      (int) (((int64_t) lh * scale120 + 60) / 120));
    }
  SET_FRAME_GARBAGED (f);
}

/* Apply a RESIZE action A to frame F: a configure, with the compositor's
   suggestion of a size A->w x A->h (0: ours) and the toplevel's states
   A->code.  The glue holds the configure and records its ack: a content
   ack when the frame's size changes here (the push below), and for the
   cases the glue decides itself (the first configure, a client-side title
   bar's state change).  The first configure on a connection (at
   creation, or after a reconnect) redraws the frame: its content ack
   goes with a packet published after it, and nothing else asks for a
   redisplay when the size stays.  */
static void
nhwl_configure (struct frame *f, const NhgAction *a)
{
  struct nhwl_output *out = FRAME_NHWL_OUTPUT (f);
  uint32_t states = a->code;
  Lisp_Object fs = Qnil;

  if (!out->configured)
    SET_FRAME_GARBAGED (f);
  out->logical_w = a->w;
  out->logical_h = a->h;
  out->configured = true;

  if (states & NHG_STATE_FULLSCREEN)
    fs = Qfullboth;
  else if (states & NHG_STATE_MAXIMIZED)
    fs = Qmaximized;
  if (a->flags & NHG_RESIZE_FULLSCREEN)
    store_frame_param (f, Qfullscreen, fs);
  out->states = states;
  nhwl_csd_layout (f);

  /* The compositor's size is a suggestion in logical pixels, for the window
     geometry (the native area, and the title strip when we draw one); the
     frame keeps its own size when it already is that size once rounded.
     The glue fits it and records the ack now, as a content ack.  */
  {
    int dw, dh, nw, nh;

    /* Against the size a configure read before this one (in the same go)
       left pending, not the one the frame still has.  */
    nhwl_frame_size_due (f, &nw, &nh);
    if (nhwl_glue_rc (FRAME_DISPLAY_INFO (f),
		      NHG (nhg_frame_fit (FRAME_NHG (f), out->window,
					  out->scale120, a->w, a->h, states,
					  nw, nh,
					  NHG_FIT_CONFIGURE, &dw, &dh)))
	== 1)
      {
	change_frame_size (f, dw, dh, false, true, false);
	SET_FRAME_GARBAGED (f);
      }
  }

  if (FRAME_VISIBLE_P (f))
    fset_redisplay (f);
}

/* Apply the glue's WHEEL action A of frame F, from one scroll record as
   pgtk's scroll_event turns GDK's (the glue adds the records up).  A stop
   is a TOUCH_END_EVENT (momentum); a finger or continuous record's own
   pixel delta goes to `pixel-scroll-precision-mode'; whole steps are one
   event with their number (`mwheel-coalesce-scroll-events') or one event
   each.  Return the number of events stored.  */
static int
nhwl_wheel_apply (struct frame *f, const NhgAction *a,
		  struct input_event *hold_quit)
{
  struct nhwl_display_info *d = FRAME_DISPLAY_INFO (f);
  double px = a->dx, py = a->dy;
  int n = a->w, i;
  union buffered_input_event inev;

  EVENT_INIT (inev.ie);
  inev.ie.arg = Qnil;
  inev.ie.modifiers = nhwl_emacs_modifiers (a->mods);
  XSETINT (inev.ie.x, (int) d->last_mouse_motion_x);
  XSETINT (inev.ie.y, (int) d->last_mouse_motion_y);
  XSETFRAME (inev.ie.frame_or_window, f);
  inev.ie.timestamp = a->time_ms;

  if (a->flags & NHG_WHEEL_STOP)
    {
      inev.ie.kind = TOUCH_END_EVENT;
      kbd_buffer_store_event_hold (&inev.ie, hold_quit);
      return 1;
    }
  inev.ie.kind = (a->code == NHG_WHEEL_LEFT || a->code == NHG_WHEEL_RIGHT
		  ? HORIZ_WHEEL_EVENT : WHEEL_EVENT);
  inev.ie.modifiers |= (a->code == NHG_WHEEL_UP || a->code == NHG_WHEEL_LEFT
			? up_modifier : down_modifier);

  if (a->flags & NHG_WHEEL_HIRES)
    {
      /* This record's pixel delta, for `pixel-scroll-precision', which
	 (as on X) takes a negative delta for wheel-down.  */
      inev.ie.arg = list3 (Qnil, make_float (-px), make_float (-py));
      kbd_buffer_store_event_hold (&inev.ie, hold_quit);
      return 1;
    }
  if (a->flags & NHG_WHEEL_COALESCED)
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

/***********************************************************************
			   Input methods
 ***********************************************************************/

/* `pgtk-use-im-context': whether input methods are on (they are by
   default).  */
static bool nhwl_im_on = true;

/* Text-input is on for F's window while F has the keyboard focus (and
   input methods are on); the shell sends the requests when the
   compositor has entered the text input.  */
static void
nhwl_im_enable (struct frame *f, bool on)
{
  struct nhwl_output *out = FRAME_NHWL_OUTPUT (nhwl_tree_root (f));
  struct nhwl_display_info *dpyinfo = FRAME_DISPLAY_INFO (f);

  if (dpyinfo->glue && out->window)
    nhwl_glue_rc (dpyinfo, NHG (nhg_im_enable (dpyinfo->glue, out->window,
					       on && nhwl_im_on)));
}

/* Top-level R lost the keyboard it had lent to a grabbing popup frame:
   the frame of its tree that Emacs focused gets its focus-out now.  */
void
nhwl_focus_lost (struct frame *r)
{
  struct nhwl_display_info *dpyinfo = FRAME_DISPLAY_INFO (r);
  struct frame *e = dpyinfo->x_focus_event_frame;
  union buffered_input_event ev;

  if (!e || nhwl_tree_root (e) != r)
    return;
  EVENT_INIT (ev.ie);
  ev.ie.kind = NO_EVENT;
  ev.ie.arg = Qnil;
  nhwl_focus_changed (false, FOCUS_EXPLICIT, dpyinfo, e, &ev);
  if (ev.ie.kind != NO_EVENT)
    kbd_buffer_store_event (&ev.ie);
  nhwl_im_enable (r, false);
}

/* Bytes of the line kept on each side of point for the surrounding
   text (the shell cuts the whole to 4000).  */
#define NHWL_IM_SPAN 2000

/* Whether C may be in the surrounding text: the line ends at a newline,
   and the protocol's string is UTF-8 with no NUL (a `.elc' file's
   ";ELC\037\0\0\0" killed the daemon), so a raw byte, a surrogate or a
   character past Unicode ends it too.  */
static bool
nhwl_im_char_p (int c)
{
  return (c != 0 && c != '\n' && c <= MAX_UNICODE_CHAR
	  && !char_surrogate_p (c));
}

/* Tell the input method where point is in F's selected window (the
   cursor rectangle, logical pixels in the surface of F's top-level, which
   has the keyboard) and what surrounds it (the current line).  The shell
   sends only what changed.  */
static void
nhwl_im_update (struct frame *f)
{
  struct nhwl_display_info *dpyinfo = FRAME_DISPLAY_INFO (f);
  struct nhwl_output *out = FRAME_NHWL_OUTPUT (nhwl_tree_root (f));
  uint32_t s120 = out->scale120 ? out->scale120 : 120;
  struct window *w;
  struct buffer *b;

  if (!nhwl_im_on || !dpyinfo->glue || !out->window
      || f != dpyinfo->x_focus_frame || !WINDOWP (f->selected_window))
    return;
  w = XWINDOW (f->selected_window);
  if (!BUFFERP (w->contents) || w->phys_cursor.vpos < 0)
    return;

  {
    /* Device pixels in F's native area; the glue places them in the
       root's surface.  */
    NhgRect r = { (WINDOW_TO_FRAME_PIXEL_X (w, w->phys_cursor.x)
		   + WINDOW_LEFT_FRINGE_WIDTH (w)
		   + WINDOW_LEFT_MARGIN_WIDTH (w)),
		  WINDOW_TO_FRAME_PIXEL_Y (w, w->phys_cursor.y),
		  w->phys_cursor_width, w->phys_cursor_height };

    nhwl_glue_rc (dpyinfo,
		  NHG (nhg_im_cursor (dpyinfo->glue,
				      FRAME_NHWL_OUTPUT (f)->window, s120,
				      r)));
  }

  b = XBUFFER (w->contents);
  {
    char text[2 * NHWL_IM_SPAN + 1];
    ptrdiff_t pt = clip_to_bounds (BUF_BEGV (b), window_point (w),
				   BUF_ZV (b));
    ptrdiff_t pt_b = buf_charpos_to_bytepos (b, pt);
    ptrdiff_t s = pt_b, e = pt_b, n = 0;

    if (BVAR (b, enable_multibyte_characters))
      {
	/* Whole characters, out to the first one on each side that
	   nhwl_im_char_p refuses.  */
	while (s > BUF_BEGV_BYTE (b))
	  {
	    ptrdiff_t p = s - buf_prev_char_len (b, s);
	    if (pt_b - p > NHWL_IM_SPAN
		|| !nhwl_im_char_p (BUF_FETCH_MULTIBYTE_CHAR (b, p)))
	      break;
	    s = p;
	  }
	while (e < BUF_ZV_BYTE (b))
	  {
	    ptrdiff_t q = e + buf_next_char_len (b, e);
	    if (q - pt_b > NHWL_IM_SPAN
		|| !nhwl_im_char_p (BUF_FETCH_MULTIBYTE_CHAR (b, e)))
	      break;
	    e = q;
	  }
	for (ptrdiff_t i = s; i < e; i++)
	  text[n++] = *BUF_BYTE_ADDRESS (b, i);
      }
    nhwl_glue_rc (dpyinfo,
		  NHG (nhg_im_surrounding (dpyinfo->glue, out->window, text, n,
					   n ? pt_b - s : 0,
					   n ? pt_b - s : 0)));
#ifdef HAVE_NHMAC
    /* NS's ranges for NSTextInputClient: characters from BEGV, the
       region when the mark is active.  */
    ptrdiff_t m = (!NILP (BVAR (b, mark_active))
		   && XMARKER (BVAR (b, mark))->buffer == b
		   ? marker_position (BVAR (b, mark)) : pt);
    nhwl_glue_rc (dpyinfo,
		  NHG (nhg_im_offsets (dpyinfo->glue, out->window,
				       buf_bytepos_to_charpos (b, s)
				       - BUF_BEGV (b),
				       pt - BUF_BEGV (b), m - BUF_BEGV (b))));
#endif
  }
}

/* The text of a PREEDIT or COMMIT_TEXT record as a Lisp string, from the
   glue's action A (which took it from the shell); nil if it could not be
   taken.  */
static Lisp_Object
nhwl_im_text (const NhgAction *a)
{
  if (!a->text)
    return Qnil;
  if (!a->len)
    return empty_unibyte_string;
  return make_string (a->text, a->len);
}

static void
nhwl_im_store_key (struct frame *f, int kind, int code,
		   struct input_event *hold_quit)
{
  struct input_event ie;

  EVENT_INIT (ie);
  ie.kind = kind;
  ie.code = code;
  XSETFRAME (ie.frame_or_window, f);
  ie.arg = Qnil;
  kbd_buffer_store_event_hold (&ie, hold_quit);
}

/* Insert committed text: its characters as keystrokes, in order.
   Return the number of events stored.  */
static int
nhwl_im_commit (struct frame *f, Lisp_Object str,
		struct input_event *hold_quit)
{
  ptrdiff_t i = 0, ib = 0;
  int n = 0;

  if (!STRINGP (str))
    return 0;
  while (i < SCHARS (str))
    {
      int c = fetch_string_char_advance (str, &i, &ib);

      nhwl_im_store_key (f, c < 128 ? ASCII_KEYSTROKE_EVENT
			 : MULTIBYTE_CHAR_KEYSTROKE_EVENT, c, hold_quit);
      n++;
    }
  return n;
}

/* Delete BEFORE bytes before and AFTER bytes after point in F's
   selected window: count the characters they cover and queue one
   special event, (nhwl-delete-surrounding (BEFORE AFTER FRAME)), whose
   Lisp handler does the deletion.  Return the number of events stored.  */
static int
nhwl_im_delete (struct frame *f, uint32_t before, uint32_t after,
		struct input_event *hold_quit)
{
  struct window *w;
  struct buffer *b;
  ptrdiff_t pt_b, i;
  int nb = 0, na = 0;
  struct input_event ie;

  if (!WINDOWP (f->selected_window))
    return 0;
  w = XWINDOW (f->selected_window);
  if (!BUFFERP (w->contents))
    return 0;
  b = XBUFFER (w->contents);
  pt_b = buf_charpos_to_bytepos (b, clip_to_bounds (BUF_BEGV (b),
						    window_point (w),
						    BUF_ZV (b)));
  /* Characters whose first byte lies in the span.  */
  for (i = max (BUF_BEGV_BYTE (b), pt_b - (ptrdiff_t) before); i < pt_b; i++)
    nb += CHAR_HEAD_P (*BUF_BYTE_ADDRESS (b, i));
  for (i = pt_b; i < min (BUF_ZV_BYTE (b), pt_b + (ptrdiff_t) after); i++)
    na += CHAR_HEAD_P (*BUF_BYTE_ADDRESS (b, i));

  if (!nb && !na)
    return 0;
  EVENT_INIT (ie);
  ie.kind = NHWL_DELETE_SURROUNDING_EVENT;
  XSETFRAME (ie.frame_or_window, f);
  ie.arg = list3 (make_fixnum (nb), make_fixnum (na), ie.frame_or_window);
  kbd_buffer_store_event_hold (&ie, hold_quit);
  return 1;
}

/* `pgtk-use-im-context'.  */
void
nhwl_use_im_context (bool on)
{
  struct nhwl_display_info *d;

  nhwl_im_on = on;
  for (d = x_display_list; d; d = d->next)
    if (d->x_focus_frame)
      nhwl_im_enable (d->x_focus_frame, true);
}

/* The frame a key or input-method event of F's window goes to: the frame
   of F's tree that Emacs focused; for no window, the focus frame.  */
static struct frame *
nhwl_key_frame (struct nhwl_display_info *dpyinfo, struct frame *f)
{
  return f ? nhwl_focus_target (nhwl_tree_root (f)) : dpyinfo->x_focus_frame;
}

/* The modifier key pressed alone so far (its keysym), or 0: its release
   with no other key or button between gives a lone-modifier event
   (`nidhogg-lone-modifier-events').  */
static uint32_t nhwl_lone_keysym;

/* The modifier (`alt', `control', `shift' or `super') of KEYSYM, a
   modifier key, or nil; *OWN gets the NHG_MOD_* bits it sets.  */
static Lisp_Object
nhwl_lone_modifier (uint32_t keysym, uint32_t *own)
{
  switch (keysym)
    {
    case 0xffe9: case 0xffea:	/* Alt_L, Alt_R */
    case 0xffe7: case 0xffe8:	/* Meta_L, Meta_R */
      *own = NHG_MOD_ALT | NHG_MOD_META;
      return Qalt;
    case 0xffe3: case 0xffe4:	/* Control_L, Control_R */
      *own = NHG_MOD_CTRL;
      return Qcontrol;
    case 0xffe1: case 0xffe2:	/* Shift_L, Shift_R */
      *own = NHG_MOD_SHIFT;
      return Qshift;
    case 0xffeb: case 0xffec:	/* Super_L, Super_R */
      *own = NHG_MOD_SUPER | NHG_MOD_HYPER;
      return Qsuper;
    default:
      return Qnil;
    }
}

/* Follow the KEY action A for lone modifiers: a modifier key pressed
   with no other modifier held starts one, any other press (or a
   repeat's press of another key) ends it, and the release of the key
   that started it gives `lone-alt' (`lone-control', `lone-shift',
   `lone-super') to F when `nidhogg-lone-modifier-events' names the
   modifier.  Return the number of events stored.  */
static int
nhwl_lone_note (struct frame *f, const NhgAction *a,
		struct input_event *hold_quit)
{
  uint32_t keysym = (uint32_t) a->code, own = 0;
  Lisp_Object mod = nhwl_lone_modifier (keysym, &own);
  struct input_event ie;

  if (a->flags & NHG_KEY_REPEAT)
    {
      if (keysym != nhwl_lone_keysym)
	nhwl_lone_keysym = 0;
      return 0;
    }
  if (!(a->flags & NHG_KEY_RELEASE))
    {
      /* The mask is the one before this key's own modifiers event.  */
      nhwl_lone_keysym = (!NILP (mod) && !(a->mods & ~own & ~NHG_MOD_CAPS)
			  ? keysym : 0);
      return 0;
    }
  if (keysym != nhwl_lone_keysym)
    {
      nhwl_lone_keysym = 0;
      return 0;
    }
  nhwl_lone_keysym = 0;
  if (!f || NILP (Fmemq (mod, Vnidhogg_lone_modifier_events)))
    return 0;
  EVENT_INIT (ie);
  ie.kind = NHWL_LONE_MODIFIER_EVENT;
  ie.arg = (EQ (mod, Qalt) ? Qlone_alt : EQ (mod, Qcontrol) ? Qlone_control
	    : EQ (mod, Qshift) ? Qlone_shift : Qlone_super);
  ie.timestamp = a->time_ms;
  XSETFRAME (ie.frame_or_window, f);
  kbd_buffer_store_event_hold (&ie, hold_quit);
  return 1;
}

/* Apply the glue's KEY action A, whose window is F's (NULL: none): the
   key goes to the frame of F's tree that Emacs focused.  A release
   becomes a `key-release' special event, only while
   `nidhogg-key-events' is non-nil; a repeat carries
   Qnidhogg_key_repeat in .arg for `nidhogg-last-key-repeat'.  Return
   the number of events stored.  */
static int
nhwl_key_apply (struct nhwl_display_info *dpyinfo, struct frame *f,
		const NhgAction *a, struct input_event *hold_quit)
{
  union buffered_input_event inev;
  bool release = a->flags & NHG_KEY_RELEASE;
  int lone;

  f = nhwl_key_frame (dpyinfo, f);
  lone = nhwl_lone_note (f, a, hold_quit);
  if (release && NILP (Vnidhogg_key_events))
    return lone;
  if (!f)
    return lone;
  EVENT_INIT (inev.ie);
  inev.ie.kind = NO_EVENT;
  inev.ie.arg = Qnil;
  if (!nhwl_key_event (f, a, &inev) || inev.ie.kind == NO_EVENT)
    return lone;
  if (release)
    {
      inev.ie.arg = Fcons (make_fixnum (inev.ie.kind),
			   make_fixnum (inev.ie.code));
      inev.ie.kind = NHWL_KEY_RELEASE_EVENT;
    }
  else if (a->flags & NHG_KEY_REPEAT)
    inev.ie.arg = Qnidhogg_key_repeat;
  kbd_buffer_store_event_hold (&inev.ie, hold_quit);
  return lone + 1;
}

/* Apply the glue's PREEDIT, COMMIT or IM_DELETE action A, whose window is
   F's (NULL: none): the input method's events go to the frame of F's tree
   that Emacs focused, as keys do.  Return the number of events stored.  */
static int
nhwl_im_apply (struct nhwl_display_info *dpyinfo, struct frame *f,
	       const NhgAction *a, struct input_event *hold_quit)
{
  f = nhwl_key_frame (dpyinfo, f);
  if (!f)
    return 0;
  switch (a->kind)
    {
    case NHG_ACT_PREEDIT:
      {
	/* pgtk's event: ((TEXT (ul . t))), nil when cleared.  */
	Lisp_Object str = nhwl_im_text (a);
	struct input_event ie;

	EVENT_INIT (ie);
	ie.kind = PREEDIT_TEXT_EVENT;
	ie.arg = (STRINGP (str) && SCHARS (str)
		  ? list1 (list2 (str, Fcons (intern_c_string ("ul"), Qt)))
		  : Qnil);
	XSETFRAME (ie.frame_or_window, f);
	kbd_buffer_store_event_hold (&ie, hold_quit);
	return 1;
      }

    case NHG_ACT_COMMIT:
      return nhwl_im_commit (f, nhwl_im_text (a), hold_quit);

    default:
      return nhwl_im_delete (f, a->x, a->y, hold_quit);
    }
}

/* Apply the glue's MOTION action A of frame F (an enter or a motion):
   the position is in device pixels in F's surface.  */
static void
nhwl_motion_apply (struct frame *f, const NhgAction *a)
{
  struct nhwl_display_info *d = FRAME_DISPLAY_INFO (f);
  Mouse_HLInfo *hlinfo = MOUSE_HL_INFO (f);
  int x, y;

  /* A pointer hidden while typing comes back with the mouse.  */
  if (f->pointer_invisible)
    nhwl_toggle_invisible_pointer (f, false);
  /* A motion over the title bar or the margins went to the glue
     (NHG_ACT_TITLE_BAR) and does not come here.  */
  nhwl_csd_native_pos (f, a->dx, a->dy, &x, &y);
  d->last_mouse_frame = f;
  previous_help_echo_string = help_echo_string;
  help_echo_string = Qnil;
  if (hlinfo->mouse_face_hidden)
    {
      hlinfo->mouse_face_hidden = false;
      clear_mouse_face (hlinfo);
    }
  if (!nhwl_note_mouse_movement (f, x, y, a->time_ms))
    help_echo_string = previous_help_echo_string;
  if (!NILP (help_echo_string) || !NILP (previous_help_echo_string))
    {
      Lisp_Object frame;

      XSETFRAME (frame, f);
      any_help_event_p = true;
      gen_help_event (help_echo_string, frame, help_echo_window,
		      help_echo_object, help_echo_pos);
    }
}

/* Apply the glue's LEAVE action of frame F.  */
static void
nhwl_leave_apply (struct frame *f)
{
  Mouse_HLInfo *hlinfo = MOUSE_HL_INFO (f);

  clear_mouse_face (hlinfo);
#ifdef HAVE_GLITNIR
  note_mouse_sensor_leave (f);
#endif
  if (FRAME_DISPLAY_INFO (f)->last_mouse_frame == f)
    FRAME_DISPLAY_INFO (f)->last_mouse_frame = NULL;
}

/* Apply the glue's CLICK action A of frame F, a button the title bar and
   the margins did not take: a mouse click event.  Return the number of
   events stored.  */
static int
nhwl_click_apply (struct nhwl_display_info *dpyinfo, struct frame *f,
		  const NhgAction *a, struct input_event *hold_quit)
{
  union buffered_input_event inev;
  int code = a->code, x, y;
  bool down = a->flags == NHG_BUTTON_PRESSED;

  if (down)
    nhwl_lone_keysym = 0;	/* a click between: no lone modifier */
  EVENT_INIT (inev.ie);
  inev.ie.kind = NO_EVENT;
  inev.ie.arg = Qnil;

  /* A press in a child frame, or in an ancestor of the frame with the
     focus, focuses the frame pressed (as x_construct_mouse_click's caller
     does on X11).  */
  if (down && !FRAME_NO_ACCEPT_FOCUS (f) && !gui_mouse_grabbed (dpyinfo))
    {
      struct frame *hf = dpyinfo->highlight_frame;

      if (FRAME_PARENT_FRAME (f) || (hf && frame_ancestor_p (f, hf)))
	nhwl_child_focus (f);
    }
  if (code < 0)
    return 0;
  x = FRAME_DISPLAY_INFO (f)->last_mouse_motion_x;
  y = FRAME_DISPLAY_INFO (f)->last_mouse_motion_y;
  /* A button in the tab bar carries the item it is on (pgtk's
     button_event, emacs-31.1); one on a disabled item is dropped.  */
  if (WINDOWP (f->tab_bar_window)
      && WINDOW_TOTAL_LINES (XWINDOW (f->tab_bar_window))
      && EQ (window_from_coordinates (f, x, y, 0, true, true, true),
	     f->tab_bar_window)
      && NILP (inev.ie.arg = handle_tab_bar_click
	       (f, x, y, down, nhwl_emacs_modifiers (a->mods))))
    return 0;
  inev.ie.kind = MOUSE_CLICK_EVENT;
  inev.ie.code = code;
  inev.ie.modifiers = (nhwl_emacs_modifiers (a->mods)
		       | (down ? down_modifier : up_modifier));
  XSETINT (inev.ie.x, x);
  XSETINT (inev.ie.y, y);
  XSETFRAME (inev.ie.frame_or_window, f);
  inev.ie.timestamp = a->time_ms;
  FRAME_DISPLAY_INFO (f)->last_mouse_frame = f;
  kbd_buffer_store_event_hold (&inev.ie, hold_quit);
  return 1;
}

/* Apply the glue's TOUCH action A of frame F, as pgtk's touch_event_cb
   builds the events (the glue tracks the points and picks the local
   ids): a begin or end carries the point and its position, an update the
   frame's live points as ((X Y ID) ...), and an end's modifiers are 1 when
   the compositor cancelled the point.  Positions are moved to the native
   area.  Return the number of events stored.  */
static int
nhwl_touch_apply (struct frame *f, const NhgAction *a,
		  struct input_event *hold_quit)
{
  struct input_event ie;
  int x, y;

  EVENT_INIT (ie);
  ie.arg = Qnil;
  XSETFRAME (ie.frame_or_window, f);
  ie.timestamp = a->time_ms;
  nhwl_csd_native_pos (f, a->x, a->y, &x, &y);
  switch (a->flags)
    {
    case NHG_TOUCH_BEGIN:
      ie.kind = TOUCHSCREEN_BEGIN_EVENT;
      XSETINT (ie.arg, a->code);
      XSETINT (ie.x, x);
      XSETINT (ie.y, y);
      break;

    case NHG_TOUCH_UPDATE:
      {
	Lisp_Object arg = Qnil;

	ie.kind = TOUCHSCREEN_UPDATE_EVENT;
	/* Oldest point first in the text, and in the list (as pgtk's).  */
	for (size_t i = a->len / sizeof (NhgTouchPoint); i-- > 0; )
	  {
	    NhgTouchPoint p;

	    memcpy (&p, a->text + i * sizeof p, sizeof p);
	    nhwl_csd_native_pos (f, p.x, p.y, &x, &y);
	    arg = Fcons (list3i (x, y, p.id), arg);
	  }
	ie.arg = arg;
	break;
      }

    default:
      ie.kind = TOUCHSCREEN_END_EVENT;
      ie.modifiers = a->flags == NHG_TOUCH_CANCEL;
      XSETINT (ie.arg, a->code);
      XSETINT (ie.x, x);
      XSETINT (ie.y, y);
      break;
    }
  kbd_buffer_store_event_hold (&ie, hold_quit);
  return 1;
}

/* Apply the glue's PINCH action A of frame F, as pgtk_handle_event builds
   the event: (DX DY SCALE ANGLE-DELTA) as floats, the pointer's position
   in the native area and the keyboard's modifiers.  Return the number of
   events stored.  */
static int
nhwl_pinch_apply (struct frame *f, const NhgAction *a,
		  struct input_event *hold_quit)
{
  struct input_event ie;
  double angle = 0;
  int x, y;

  if (a->len >= sizeof angle)
    memcpy (&angle, a->text, sizeof angle);
  nhwl_csd_native_pos (f, a->x, a->y, &x, &y);
  EVENT_INIT (ie);
  ie.kind = PINCH_EVENT;
  XSETFRAME (ie.frame_or_window, f);
  XSETINT (ie.x, x);
  XSETINT (ie.y, y);
  ie.arg = list4 (make_float (a->dx), make_float (a->dy),
		  make_float (a->scale), make_float (angle));
  ie.modifiers = nhwl_emacs_modifiers (a->mods);
  ie.timestamp = a->time_ms;
  kbd_buffer_store_event_hold (&ie, hold_quit);
  return 1;
}

/* Apply the glue's FOCUS_IN (IN) or FOCUS_OUT action for F's window: a
   top-level's keyboard focus is Emacs's focus on the frame of its tree
   that takes its keys.  Return the number of events stored.  */
static int
nhwl_focus_apply (struct nhwl_display_info *dpyinfo, struct frame *f,
		  bool in, struct input_event *hold_quit)
{
  union buffered_input_event inev;
  struct frame *r, *e, *t;

  /* The keyboard moving between a top-level and its own popup frames
     gives no action (the glue's popup.rs).  */
  r = nhwl_tree_root (f);
  e = dpyinfo->x_focus_event_frame;
  t = !in && e && nhwl_tree_root (e) == r ? e : nhwl_focus_target (r);
  EVENT_INIT (inev.ie);
  inev.ie.kind = NO_EVENT;
  inev.ie.arg = Qnil;
  nhwl_focus_changed (in, FOCUS_EXPLICIT, dpyinfo, t, &inev);
  nhwl_im_enable (r, in);
  if (in)
    nhwl_im_update (t);
  if (inev.ie.kind == NO_EVENT)
    return 0;
  kbd_buffer_store_event_hold (&inev.ie, hold_quit);
  return 1;
}

/* Apply the glue's drop-target action A (NHG_ACT_DND_*) of frame F (NULL:
   the window has no frame): a DRAG_N_DROP_EVENT in pgtk's forms, at the
   position in F's native area.  Return the number of events stored.  */
static int
nhwl_dnd_apply (struct nhwl_display_info *dpyinfo, struct frame *f,
		const NhgAction *a, struct input_event *hold_quit)
{
  union buffered_input_event inev;
  int x = 0, y = 0;

  if (!f)
    {
      /* Nothing will answer the drop: finish it, or the offer stays.  */
      if (a->kind == NHG_ACT_DND_DROP)
	nhwl_glue_rc (dpyinfo, NHG (nhg_dnd_finish (dpyinfo->glue, 0,
						    a->time_ms)));
      return 0;
    }
  if (a->kind != NHG_ACT_DND_LEAVE)
    nhwl_csd_native_pos (f, a->dx, a->dy, &x, &y);
  EVENT_INIT (inev.ie);
  inev.ie.kind = DRAG_N_DROP_EVENT;
  inev.ie.modifiers = 0;
  inev.ie.arg = nhwl_dnd_event_arg (a);
  inev.ie.timestamp = a->time_ms;
  XSETINT (inev.ie.x, x);
  XSETINT (inev.ie.y, y);
  XSETFRAME (inev.ie.frame_or_window, f);
  kbd_buffer_store_event_hold (&inev.ie, hold_quit);
  return 1;
}

/***********************************************************************
			 Reconnecting in place
 ***********************************************************************/

/* Frames survive a compositor restart: when the compositor goes (a
   hangup, not a protocol error) the glue connects to it again, and the
   frames come back with their contents.  The schedule and the decisions
   are the glue's (nidhogg/glue/src/reconnect.rs); C keeps the timer and
   the Lisp side.  Lisp hears of it through
   `nhwl-display-connection-functions', run from a timer.  When the
   compositor does not come back within `nhwl-reconnect-timeout', the
   display goes as without reconnecting (nhwl_connection_lost).  */

/* Reconnecting is wanted: `nhwl-reconnect' is non-nil (or not defined
   yet) and the environment has no NHWL_RECONNECT=0.  */
static bool
nhwl_reconnect_wanted (void)
{
  const char *env = getenv ("NHWL_RECONNECT");

  if (env && !strcmp (env, "0"))
    return false;
  return (NILP (Fboundp (Qnhwl_reconnect))
	  || !NILP (Fsymbol_value (Qnhwl_reconnect)));
}

/* `nhwl-reconnect-timeout' in milliseconds (10 s when not a number).  */
static uint32_t
nhwl_reconnect_timeout (void)
{
  double secs = (NUMBERP (Vnhwl_reconnect_timeout)
		 ? XFLOATINT (Vnhwl_reconnect_timeout) : 10.0);

  return secs <= 0 ? 0 : secs > 3600 ? 3600 * 1000 : (uint32_t) (secs * 1000);
}

/* Run `nhwl-display-connection-functions' with DPYINFO's name and STATE
   at the next timer check, after the hooks queued before it: never from
   here, where no Lisp may run.  */
static void
nhwl_connection_hook (struct nhwl_display_info *dpyinfo, Lisp_Object state)
{
  pending_funcalls
    = nconc2 (pending_funcalls,
	      list1 (list4 (Qrun_hook_with_args,
			    Qnhwl_display_connection_functions,
			    XCAR (dpyinfo->name_list_element), state)));
}

static void nhwl_reconnect_tick (struct atimer *);

/* Fire DPYINFO's reconnect timer MS milliseconds from now.  */
static void
nhwl_reconnect_arm (struct nhwl_display_info *dpyinfo, uint32_t ms)
{
  if (dpyinfo->reconnect_timer)
    cancel_atimer (dpyinfo->reconnect_timer);
  dpyinfo->reconnect_timer
    = start_atimer (ATIMER_RELATIVE,
		    make_timespec (ms / 1000, ms % 1000 * 1000 * 1000),
		    nhwl_reconnect_tick, dpyinfo);
}

/* Stop DPYINFO's reconnect timer (the display is going, or is back).  */
static void
nhwl_reconnect_disarm (struct nhwl_display_info *dpyinfo)
{
  if (dpyinfo->reconnect_timer)
    cancel_atimer (dpyinfo->reconnect_timer);
  dpyinfo->reconnect_timer = NULL;
}

/* The reconnect timer.  While reconnecting: one step of connecting again
   (at most 50 ms on this thread), then the next tick at the glue's
   backoff; when the glue gives up (the timeout passed, or the new
   display failed) the display goes at the next read.  On success the
   shell's descriptor wakes the drain, which takes RECONNECTED.  While
   settling it does nothing: firing wakes the event loop, whose read asks
   the glue what is due.  It never presents and never runs Lisp.  */
static void
nhwl_reconnect_tick (struct atimer *timer)
{
  struct nhwl_display_info *dpyinfo = timer->client_data;
  uint32_t next = 0;
  int32_t rc;

  dpyinfo->reconnect_timer = NULL;
  if (!dpyinfo->reconnecting || !dpyinfo->glue || dpyinfo->glue_broken)
    return;
  block_input ();
  rc = nhwl_glue_rc (dpyinfo,
		     NHG (nhg_reconnect_step (dpyinfo->glue,
					      dpyinfo->reconnect_timeout_ms,
					      &next)));
  if (rc == NHG_E_TIMEOUT)
    nhwl_reconnect_arm (dpyinfo, next);
  else if (rc < 0)
    dpyinfo->connection_lost = true;
  unblock_input ();
}

/* The compositor went and the shell reconnects (NHG_ACT_RECONNECTING A):
   start the timer, its first tick A->code ms from now, and tell Lisp.  A
   display without a frame left, or with reconnecting turned off since
   it opened, goes at once.  */
static void
nhwl_reconnecting (struct nhwl_display_info *dpyinfo, const NhgAction *a)
{
  Lisp_Object tail, frame;
  bool live = false;

  FOR_EACH_FRAME (tail, frame)
    {
      struct frame *f = XFRAME (frame);

      if (FRAME_NHWL_P (f) && FRAME_DISPLAY_INFO (f) == dpyinfo
	  && !FRAME_TOOLTIP_P (f))
	live = true;
    }
  if (!live || !nhwl_reconnect_wanted ())
    {
      dpyinfo->connection_lost = true;
      return;
    }
  dpyinfo->reconnecting = true;
  dpyinfo->reconnect_settling = false;
  dpyinfo->reconnect_timeout_ms = nhwl_reconnect_timeout ();
  nhwl_reconnect_arm (dpyinfo, a->code);
  nhwl_connection_hook (dpyinfo, Qnhwl_reconnecting);
}

/* The display is back (NHG_ACT_RECONNECTED): every frame, child frames
   included, is damaged whole, and every top-level waits for its first
   configure on the new connection, which redraws it (nhwl_configure), so
   the publish after that configure's ack carries the tree's whole
   picture; nothing presents here.  The reads ask the glue what is due
   from now on.  */
static void
nhwl_reconnected (struct nhwl_display_info *dpyinfo)
{
  struct nhwl_output *o;

  nhwl_reconnect_disarm (dpyinfo);
  dpyinfo->reconnecting = false;
  dpyinfo->reconnect_settling = true;
  for (o = dpyinfo->outputs; o; o = o->next_output)
    {
      if (o->window && o->glitnir_id)
	nhwl_damage_all (o->frame);
      if (o->window && !nhwl_child_p (o->frame)
	  && !nhwl_popup_frame_p (o->frame) && !FRAME_TOOLTIP_P (o->frame))
	o->configured = false;
    }
}

/* What is due after a reconnect (nhg_reconnect_settle): `reconnected'
   for Lisp, the monitors hook when the list changed, and the focus-out
   of the frame that had the keyboard when no keyboard enter came.
   Return the number of events stored.  */
static int
nhwl_reconnect_settle (struct nhwl_display_info *dpyinfo,
		       struct input_event *hold_quit)
{
  uint64_t window = 0;
  uint32_t next = 0;
  struct input_event ie;
  struct frame *f;
  int n = 0;
  int32_t rc;

  block_input ();
  rc = nhwl_glue_rc (dpyinfo, NHG (nhg_reconnect_settle (dpyinfo->glue,
							  &window, &next)));
  if (rc > 0 && (rc & NHG_SETTLE_REPORT))
    nhwl_connection_hook (dpyinfo, Qnhwl_reconnected);
  if (rc > 0 && (rc & NHG_SETTLE_MONITORS))
    {
      EVENT_INIT (ie);
      ie.kind = MONITORS_CHANGED_EVENT;
      XSETTERMINAL (ie.arg, dpyinfo->terminal);
      kbd_buffer_store_event_hold (&ie, hold_quit);
      n++;
    }
  if (rc > 0 && (rc & NHG_SETTLE_FOCUS_OUT)
      && (f = nhwl_window_to_frame (dpyinfo, window)))
    n += nhwl_focus_apply (dpyinfo, f, false, hold_quit);
  if (rc < 0 || next == 0)
    dpyinfo->reconnect_settling = false;
  else if (!dpyinfo->reconnect_timer)
    nhwl_reconnect_arm (dpyinfo, next);
  unblock_input ();
  return n;
}

/* Apply action A of the glue's event step, one whose kind is ported: the
   Emacs calls the glue's decision needs.  An action whose window maps to
   no frame is skipped.  Return the number of events stored (through
   HOLD_QUIT where Emacs gave one).  */
static int
nhwl_apply (struct nhwl_display_info *dpyinfo, const NhgAction *a,
	    struct input_event *hold_quit)
{
  struct frame *f;
  struct input_event ie;

  switch (a->kind)
    {
    case NHG_ACT_LOST:
      /* Handled when the drain is over.  */
      dpyinfo->connection_lost = true;
      return 0;

    case NHG_ACT_RECONNECTING:
      nhwl_reconnecting (dpyinfo, a);
      return 0;

    case NHG_ACT_RECONNECTED:
      nhwl_reconnected (dpyinfo);
      return 0;

    case NHG_ACT_MONITORS:
      if (a->window && (f = nhwl_window_to_frame (dpyinfo, a->window)))
	{
	  /* F moved to another monitor: Lisp runs move-frame-functions.  */
	  EVENT_INIT (ie);
	  ie.kind = MOVE_FRAME_EVENT;
	  XSETFRAME (ie.frame_or_window, f);
	  kbd_buffer_store_event_hold (&ie, hold_quit);
	  return 1;
	}
      /* The output list changed (the glue keeps it).  The first scale an
	 output reports is the display's, and its resolution's.  */
      if (a->code > 0 && dpyinfo->scale120 == 0)
	dpyinfo->scale120 = a->code;
      /* One event per change: Lisp runs display-monitors-changed-functions
	 for it.  */
      EVENT_INIT (ie);
      ie.kind = MONITORS_CHANGED_EVENT;
      XSETTERMINAL (ie.arg, dpyinfo->terminal);
      kbd_buffer_store_event_hold (&ie, hold_quit);
      return 1;

    case NHG_ACT_SESSION:
      /* The compositor answered the session request, or another client
	 took the session over: Lisp keeps the id and stops naming.  */
      {
	char buf[256];
	size_t len = 0;
	Lisp_Object state = (a->code == NHG_SESSION_CREATED
			     ? Qcreated
			     : a->code == NHG_SESSION_RESTORED
			     ? Qnhwl_session_restored : Qnhwl_session_replaced);
	Lisp_Object id = Qnil;

	if (a->code != NHG_SESSION_REPLACED
	    && nhwl_glue_rc (dpyinfo, NHG (nhg_session_id (dpyinfo->glue, buf,
							    sizeof buf, &len)))
	       >= 0)
	  id = make_string (buf, len);
	EVENT_INIT (ie);
	ie.kind = NHWL_SESSION_EVENT;
	ie.arg = Fcons (state, id);
	kbd_buffer_store_event_hold (&ie, hold_quit);
	return 1;
      }

#ifdef HAVE_NHMAC
    case NHG_ACT_MENU:
      return nhmac_menu_choice (a, hold_quit);
#endif

    case NHG_ACT_SELECTION_CLEAR:
      /* Another client took a selection we owned.  */
      EVENT_INIT (ie);
      nhwl_selection_clear_event (a->code, &ie);
      kbd_buffer_store_event_hold (&ie, hold_quit);
      return 1;

    case NHG_ACT_DRAG_ACTION:
    case NHG_ACT_DRAG_DROPPED:
    case NHG_ACT_DRAG_DONE:
      /* The glue keeps the drag's state; nhwl_drag_run polls it.  */
      return 0;
    }

  f = a->window ? nhwl_window_to_frame (dpyinfo, a->window) : NULL;
  if (a->kind >= NHG_ACT_DND_ENTER && a->kind <= NHG_ACT_DND_DROP)
    return nhwl_dnd_apply (dpyinfo, f, a, hold_quit);
  if (a->kind == NHG_ACT_KEY)
    return nhwl_key_apply (dpyinfo, f, a, hold_quit);
  if (a->kind == NHG_ACT_PREEDIT || a->kind == NHG_ACT_COMMIT
      || a->kind == NHG_ACT_IM_DELETE)
    return nhwl_im_apply (dpyinfo, f, a, hold_quit);
  if (!f)
    return 0;
  switch (a->kind)
    {
    case NHG_ACT_MOTION:
      nhwl_motion_apply (f, a);
      break;

    case NHG_ACT_LEAVE:
      nhwl_leave_apply (f);
      break;

    case NHG_ACT_CLICK:
      return nhwl_click_apply (dpyinfo, f, a, hold_quit);

    case NHG_ACT_WHEEL:
      return nhwl_wheel_apply (f, a, hold_quit);

    case NHG_ACT_TOUCH:
      return nhwl_touch_apply (f, a, hold_quit);

    case NHG_ACT_PINCH:
      return nhwl_pinch_apply (f, a, hold_quit);

    case NHG_ACT_RESIZE:
      nhwl_configure (f, a);
      break;

    case NHG_ACT_SCALE:
      nhwl_scale_changed (f, a->code);
      break;

#ifdef HAVE_NHMAC
    case NHG_ACT_MOVED:
      /* Where AppKit put the top-level, as nsterm.m's windowDidMove:
	 stores it: AppKit may constrain or nudge the place asked for, and
	 a move to a screen of another scale changes its device pixels.  */
      if (!nhwl_child_p (f))
	{
	  f->left_pos = a->x;
	  f->top_pos = a->y;
	  FRAME_NHWL_OUTPUT (f)->title_bar_height = a->h;
	}
      break;
#endif

    case NHG_ACT_READY:
      /* Whatever Emacs is doing, drawn or not: the pacer re-arms through
	 here.  */
      nhwl_present_ready (dpyinfo,
			  FRAME_NHWL_OUTPUT (nhwl_tree_root (f))->window);
#ifdef HAVE_NHMAC
      /* A held commit's slot freed: its pixels are read under the lock.  */
      nhwl_publish (f);
#endif
      break;

    case NHG_ACT_TITLE_BAR:
      nhwl_title_bar_action (f, a);
      break;

    case NHG_ACT_MINIMAP:
      return nhwl_minimap_action (f, a, hold_quit);

    case NHG_ACT_DELETE:
      /* A tip's popup goes at once; its frame when the tip's timer, the
	 next x-hide-tip or the next x-show-tip deletes it.  */
      if (a->flags & NHG_DEL_TIP)
	{
	  if (FRAME_TOOLTIP_P (f))
	    nhwl_tip_dismissed (f);
	  return 0;
	}
      /* A popup frame's (NHG_DEL_POPUP) stays until Lisp deletes or hides
	 it, on the `delete-frame' event.  */
      EVENT_INIT (ie);
      ie.kind = DELETE_WINDOW_EVENT;
      ie.arg = Qnil;
      XSETFRAME (ie.frame_or_window, f);
      kbd_buffer_store_event_hold (&ie, hold_quit);
      return 1;

    case NHG_ACT_FOCUS_IN:
    case NHG_ACT_FOCUS_OUT:
      return nhwl_focus_apply (dpyinfo, f, a->kind == NHG_ACT_FOCUS_IN,
			       hold_quit);

    default:
      break;
    }
  return 0;
}

/* The most events one drain takes; the rest wait in the shell's queue,
   whose descriptor stays readable.  */
#define NHWL_DRAIN_MAX 256

/* Drain DPYINFO's shell events into the keyboard buffer (through
   HOLD_QUIT where Emacs gave one); return the number of events stored.
   The glue takes one event per step and answers with its actions, and
   each is applied before the next step, so every decision sees what the
   earlier events changed.  Callers hold block_input.  */
int
nhwl_drain_events (struct nhwl_display_info *dpyinfo,
		   struct input_event *hold_quit)
{
  NhgNextIn in;
  NhgActionList list;
  int count = 0;
  int n = 0;
  int rc = 0;

  if (!dpyinfo->glue || dpyinfo->glue_broken)
    return 0;

  block_input ();
  while (n < NHWL_DRAIN_MAX)
    {
      size_t i;

      /* The facts the step reads, sampled again for each event.  */
      in.flags = ((kbd_buffer_events_waiting () ? NHG_IN_INPUT_WAITING : 0)
		  | (redisplaying_p ? NHG_IN_REDISPLAY : 0)
		  | (mwheel_coalesce_scroll_events
		     ? NHG_IN_COALESCE_WHEEL : 0));
      list.v = NULL;
      list.n = 0;
      rc = nhwl_glue_rc (dpyinfo,
			 NHG (nhg_next (dpyinfo->glue, &in, &list)));
      if (rc <= 0)
	break;
      n++;

      for (i = 0; i < list.n; i++)
	count += nhwl_apply (dpyinfo, &list.v[i], hold_quit);
    }

  /* Cut at the cap, the drain may end right after a configure it
     applied: record that ack now, taking no event, so that every
     configure applied is answered when the drain returns.  */
  if (rc > 0)
    {
      in.flags = NHG_IN_ACKS_ONLY;
      list.v = NULL;
      list.n = 0;
      rc = nhwl_glue_rc (dpyinfo,
			 NHG (nhg_next (dpyinfo->glue, &in, &list)));
    }

  if (rc == NHG_E_DISCONNECTED)
    dpyinfo->connection_lost = true;

  /* Flush what we sent while handling the events.  A broken glue is
     handled by the caller.  */
  if (rc != NHG_E_DISCONNECTED && !dpyinfo->glue_broken)
    NHG (nhg_dispatch_main (dpyinfo->glue));
  unblock_input ();
  return count;
}

/* Give up the drag of DPYINFO (PTR) if the glue still runs it: the
   unwind of nhwl_drag_run, on a quit, an error or its own end.  */
static void
nhwl_drag_unwind (void *ptr)
{
  struct nhwl_display_info *dpyinfo = ptr;

  if (!dpyinfo->glue || dpyinfo->glue_broken)
    return;
  block_input ();
  nhwl_glue_rc (dpyinfo, NHG (nhg_drag_cancel (dpyinfo->glue)));
  unblock_input ();
}

/* Run the drag out of a frame of DPYINFO that the glue has just started
   (x-begin-drag): a nested loop of drains, which present at FRAME_READY
   and store every other event for after the loop, and waits of 100 ms
   at most on the glue's descriptor.  It never prompts and never runs
   Lisp.  It ends:
   - at the glue's DONE;
   - when the compositor showed nothing of the drag within TIMEOUT
     seconds (it ignores a drag with a stale serial);
   - when the target it was dropped on did not finish within TIMEOUT;
   - at C-g, which gives the drag up and quits;
   - when the display is lost (the glue's DONE comes first).
   Return the action performed (NHG_DND_*; 0: none), and in *FRAME the
   frame to return for RETURN-FRAME, else NULL.  */
int
nhwl_drag_run (struct nhwl_display_info *dpyinfo, double timeout,
	       struct frame **frame)
{
  specpdl_ref count = SPECPDL_INDEX ();
  struct timespec limit = dtotimespec (timeout);
  struct timespec since = current_timespec ();
  bool dropped = false;
  int32_t phase = NHG_DRAG_PHASE_WAITING;
  uint32_t performed = 0;
  uint64_t win = 0;

  *frame = NULL;
  record_unwind_protect_ptr (nhwl_drag_unwind, dpyinfo);
  for (;;)
    {
      struct input_event hold;
      struct pollfd pfd;

      EVENT_INIT (hold);
      nhwl_drain_events (dpyinfo, &hold);
      if (hold.kind != NO_EVENT)
	Vquit_flag = Qt;
      if (dpyinfo->glue && !dpyinfo->glue_broken)
	{
	  block_input ();
	  phase = nhwl_glue_rc (dpyinfo, NHG (nhg_drag_state (dpyinfo->glue,
							       &performed,
							       &win)));
	  unblock_input ();
	}
      if (phase < 0 || phase == NHG_DRAG_PHASE_DONE || !NILP (Vquit_flag)
	  || dpyinfo->connection_lost || dpyinfo->glue_broken
	  || !dpyinfo->glue)
	break;
      /* The finish timeout counts from the drop.  */
      if (phase == NHG_DRAG_PHASE_DROPPED && !dropped)
	{
	  dropped = true;
	  since = current_timespec ();
	}
      if ((dropped || phase == NHG_DRAG_PHASE_WAITING)
	  && timespec_cmp (timespec_sub (current_timespec (), since),
			   limit) >= 0)
	break;
      pfd.fd = NHG (nhg_fd (dpyinfo->glue));
      pfd.events = POLLIN;
      pfd.revents = 0;
      poll (&pfd, 1, 100);
    }
  /* Gives a drag the glue still runs up: no DONE follows.  */
  unbind_to (count, Qnil);
  maybe_quit ();
  if (phase != NHG_DRAG_PHASE_DONE)
    return 0;
  *frame = win ? nhwl_window_to_frame (dpyinfo, win) : NULL;
  return performed;
}

/* The reports of the glue's panics, newest first, for
   (nhwl-glue-panics).  */
static Lisp_Object nhwl_glue_panic_list;

/* The glue poisoned DPYINFO's display (a panic inside it, or a call that
   found the display's state locked).  Its report goes to stderr and to
   (nhwl-glue-panics), and the display goes as if the compositor had: its
   frames are deleted, Emacs and a daemon stay.  No reconnect is tried,
   because a panic is a bug.  Frees DPYINFO.  */
static void
nhwl_glue_lost (struct nhwl_display_info *dpyinfo)
{
  Lisp_Object report = build_string ("(the glue kept no report)");
  size_t len = 0;

  if (NHG (nhg_panic_message (dpyinfo->glue, NULL, 0, &len)) == NHG_OK
      && len > 0)
    {
      char *buf = xmalloc (len + 1);

      if (NHG (nhg_panic_message (dpyinfo->glue, buf, len + 1, &len)) == NHG_OK)
	{
	  fprintf (stderr, "nidhogg-wayland: internal error in the shell glue:\n%s\n",
		   buf);
	  report = build_string_from_utf8 (buf);
	}
      xfree (buf);
    }
  nhwl_glue_panic_list = Fcons (report, nhwl_glue_panic_list);
  nhwl_connection_lost (dpyinfo, "internal error in the shell glue");
}

/* The terminal's read_socket_hook.  */
static int
nhwl_read_socket (struct terminal *terminal, struct input_event *hold_quit)
{
  struct nhwl_display_info *dpyinfo = terminal->display_info.nhwl;
  int n;

  if (!dpyinfo || !dpyinfo->glue)
    return 0;
  /* A call whose code went unchecked (the drawing calls) may have poisoned
     the glue: any call answers NHG_E_PANIC then, and nhg_fd is a cheap
     one.  */
  if (dpyinfo->glue_broken
      || NHG (nhg_fd (dpyinfo->glue)) == NHG_E_PANIC)
    {
      /* Frees DPYINFO, as below.  */
      nhwl_glue_lost (dpyinfo);
      return 0;
    }
  n = nhwl_drain_events (dpyinfo, hold_quit);
  if (dpyinfo->reconnect_settling && !dpyinfo->connection_lost
      && !dpyinfo->glue_broken)
    n += nhwl_reconnect_settle (dpyinfo, hold_quit);
  if (dpyinfo->connection_lost)
    {
      /* Frees DPYINFO and clears TERMINAL's pointer to it.  gobble_input
	 calls this hook again while it returns more than 0, so the events
	 this drain stored are not counted.  */
      nhwl_connection_lost (dpyinfo, "connection to the display lost");
      return 0;
    }
  return n;
}

/* No toolkit menu is ever active: menus are popup frames.  */
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

/* Publish what was drawn, at a consistency point (nhwl_publish asks).
   Adapted from pgtk_frame_up_to_date.  */
static void
nhwl_frame_up_to_date (struct frame *f)
{
  block_input ();
  FRAME_MOUSE_UPDATE (f);
  nhwl_im_update (f);
  nhwl_publish (f);
  unblock_input ();
}

/* The end of every redisplay, for every frame: the consistency point.
   A tree is published once, from its root.  */
static void
nhwl_buffer_flipping_unblocked_hook (struct frame *f)
{
  if (!nhwl_child_p (f))
    nhwl_publish (f);
}

/* The visible bell's 50 ms are up: the glue puts the inverted rows back
   (except what a draw replaced) and damages them.  The rows publish like
   any draw outside a redisplay; nothing presents here.  */
static void
nhwl_bell_restore (struct atimer *timer)
{
  struct frame *f = timer->client_data;
  struct nhwl_output *out = FRAME_NHWL_OUTPUT (f);
  int32_t rc = 0;

  out->bell_timer = NULL;
  if (!out->glitnir_id || !nhwl_backing_p (out))
    return;
  block_input ();
  nhwl_backing_flush (out);
  rc = nhwl_glue_rc (FRAME_DISPLAY_INFO (f),
		     NHG (nhg_bell_restore (FRAME_NHG (f), out->glitnir_id)));
  nhwl_backing_dirty (out);
  unblock_input ();
  if (rc > 0)
    nhwl_publish (f);
}

/* The visible bell: the glue inverts pgtk_flash's rows in the backing and
   damages them, and a timer puts them back.  */
static void
nhwl_flash (struct frame *f)
{
  struct nhwl_output *out = FRAME_NHWL_OUTPUT (f);
  NhgBellGeom g = { FRAME_PIXEL_WIDTH (f), FRAME_PIXEL_HEIGHT (f),
		    FRAME_LINE_HEIGHT (f), FRAME_INTERNAL_BORDER_WIDTH (f),
		    FRAME_TOP_MARGIN_HEIGHT (f), FRAME_BOTTOM_MARGIN_HEIGHT (f) };
  int32_t rc;

  if (!out->window || !FRAME_NHG (f))
    return;
  block_input ();
  nhwl_backing_resize (f);
  nhwl_glitnir_ensure (f);
  nhwl_backing_flush (out);
  rc = nhwl_glue_rc (FRAME_DISPLAY_INFO (f),
		     NHG (nhg_bell_flash (FRAME_NHG (f), out->glitnir_id, &g)));
  nhwl_backing_dirty (out);
  if (rc > 0)
    {
      if (out->bell_timer)
	cancel_atimer (out->bell_timer);
      out->bell_timer = start_atimer (ATIMER_RELATIVE,
				      make_timespec (0, 50 * 1000 * 1000),
				      nhwl_bell_restore, f);
    }
  unblock_input ();
  if (rc > 0)
    nhwl_publish (f);
}

static void
nhwl_ring_bell (struct frame *f)
{
  struct nhwl_output *out = FRAME_NHWL_OUTPUT (f);

  if (visible_bell)
    nhwl_flash (f);
  else if (out->window)
    nhwl_glue_rc (FRAME_DISPLAY_INFO (f),
		  NHG (nhg_bell (FRAME_NHG (f), out->window)));
}

static void
nhwl_toggle_invisible_pointer (struct frame *f, bool invisible)
{
  struct nhwl_output *out = FRAME_NHWL_OUTPUT (f);

  if (out->window)
    {
      block_input ();
      nhwl_glue_rc (FRAME_DISPLAY_INFO (f),
		    NHG (nhg_pointer_visible (FRAME_NHG (f), out->window,
					      !invisible,
					      out->current_cursor
					      ? (uint32_t) ((intptr_t)
							    out->current_cursor
							    - 1)
					      : NHG_CURSOR_DEFAULT)));
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
#ifdef HAVE_NHMAC
      /* The glue's picture of it goes too.  */
      NHG (nhg_image_forget ((uint8_t *) pixmap->data));
#endif
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
  terminal->set_frame_alpha_hook = nhwl_set_frame_alpha;
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
  NhgDisplay *glue;
  ptrdiff_t nbytes;

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

  glue = NHG (nhg_open (strlen (dpy_name) ? dpy_name : NULL, NULL, 0));
  if (!glue)
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
  dpyinfo->glue = glue;
  dpyinfo->connection = nhwl_glue_rc (dpyinfo, NHG (nhg_fd (glue)));
  {
    const char *env = getenv ("NHWL_PRESENTER");

    /* A thread that cannot start leaves the display inline.  */
    if (nhwl_presenter_thread && !(env && !strcmp (env, "0")))
      NHG (nhg_presenter_thread (glue, 1));
  }
  /* Frames survive a compositor restart unless that is turned off; a
     connection that cannot be made again (an inherited socket) stays
     without.  */
  if (nhwl_reconnect_wanted ())
    NHG (nhg_set_reconnect (glue, 1));
  /* The title bars' style Lisp gave so far.  */
  nhwl_csd_push_style (dpyinfo);
  NHG (nhg_caps (glue, &dpyinfo->caps));
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
#ifdef HAVE_NHMAC
	terminal->kboard = allocate_kboard (Qns);
#else
	terminal->kboard = allocate_kboard (Qpgtk);
#endif

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
  dpyinfo->invisible_cursor = nhwl_cursor_for_shape (NHG_CURSOR_HIDDEN);
  dpyinfo->vertical_scroll_bar_cursor
    = nhwl_cursor_for_shape (NHG_CURSOR_RESIZE_EW);
  dpyinfo->horizontal_scroll_bar_cursor
    = nhwl_cursor_for_shape (NHG_CURSOR_RESIZE_NS);

  reset_mouse_highlight (&dpyinfo->mouse_highlight);

  /* The X colour names.  */
  color_file = Fexpand_file_name (build_string ("rgb.txt"),
				  Vdata_directory);
  dpyinfo->color_map = Fx_load_color_file (color_file);
  if (NILP (dpyinfo->color_map))
    fatal ("Could not read %s.\n", SDATA (color_file));

  /* The outputs the shell announced while connecting give the scale.  */
  NHG (nhg_dispatch_main (glue));
  dpyinfo->scale120 = 0;
  nhwl_drain_events (dpyinfo, NULL);
  if (dpyinfo->scale120 == 0)
    dpyinfo->scale120 = 120;
  dpyinfo->resx = dpyinfo->resy = NHWL_DPI * dpyinfo->scale120 / 120.0;

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

  /* Its client data is DPYINFO, freed below.  */
  nhwl_reconnect_disarm (dpyinfo);
#ifdef HAVE_NHMAC
  if (dpyinfo->present_deadline)
    cancel_atimer (dpyinfo->present_deadline);
  dpyinfo->present_deadline = NULL;
  nhmac_deadline_wake (-1);
#endif

  if (dpyinfo->glue)
    {
      image_destroy_all_bitmaps (dpyinfo);

      if (dpyinfo->connection >= 0)
	delete_keyboard_wait_descriptor (dpyinfo->connection);
      dpyinfo->connection = -1;

      /* On a poisoned display this shuts the connection down and leaks
	 the rest.  */
      NHG (nhg_close (dpyinfo->glue));
      dpyinfo->glue = NULL;
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

DEFUN ("nhwl--bench-full-redraw", Fnhwl__bench_full_redraw,
       Snhwl__bench_full_redraw, 1, 1, 0,
       doc: /* Present N frames of the selected frame, each damaged whole.
Each frame waits for the previous one's frame callback.  Return
(:frames N :compose-p50 MS :compose-p95 MS :present-interval-p95 MS):
the time one present took on Emacs's thread (the publish, and inline the
commit, buffer, compose and attach) and the time between two presents.
The motion bench of the budgets table; it runs through the presenter, on
its thread or inline.  */)
  (Lisp_Object n)
{
  struct frame *f = SELECTED_FRAME ();
  struct nhwl_output *out;
  struct nhwl_display_info *dpyinfo;
  NhgBenchOut r;
  ptrdiff_t frames;

  CHECK_FIXNAT (n);
  if (!FRAME_NHWL_P (f))
    error ("Not a Nidhogg shell frame");
  frames = XFIXNAT (n);
  if (frames < 1 || frames > 100000)
    error ("N must be 1 to 100000");
  f = nhwl_tree_root (f);
  out = FRAME_NHWL_OUTPUT (f);
  dpyinfo = FRAME_DISPLAY_INFO (f);
  if (!out->glitnir_id || !out->configured || !dpyinfo->glue)
    error ("The frame is not shown yet");

  /* The tree, as nhwl_publish lists it; the glue damages, publishes and
     presents it, frame by frame.  */
  r.frames = 0;
  nhwl_nsurfaces = 0;
  nhwl_surface_add (f, 0, true);
  if (nhwl_glue_rc (dpyinfo,
		    NHG (nhg_bench_full_redraw (dpyinfo->glue, nhwl_surfaces,
						nhwl_nsurfaces, frames, &r)))
      < 0)
    error ("The bench presented %"pD"d frames", (ptrdiff_t) r.frames);
  return list (intern_c_string (":frames"), make_fixnum (r.frames),
	       intern_c_string (":compose-p50"), make_float (r.compose_p50),
	       intern_c_string (":compose-p95"), make_float (r.compose_p95),
	       intern_c_string (":present-interval-p95"),
	       make_float (r.interval_p95));
}

DEFUN ("nhwl-presenter-stats", Fnhwl_presenter_stats, Snhwl_presenter_stats,
       0, 1, 0,
       doc: /* Return the presenter's counters on DISPLAY since it opened.
DISPLAY is a display name, a frame or a terminal; nil means the selected
frame's.  A plist: `:packets' published, `:superseded' by a newer one
before they were presented, `:commits' with a buffer and `:bare-commits'
without (each counted once flushed to the compositor),
`:main-wait-max-ns' (the longest Emacs's thread waited for the
presenter), `:publish-copy-max-ns' (the longest copy into the front
buffers), `:front-trims' (front buffers dropped after the quiet period),
`:published' (the newest request's number, 0 before the first: each
packet, configure ack and piece of surface state recorded for a commit
takes the next one), `:presented' (the newest request that, with every
older one, has gone out to the compositor in a flushed commit or needs
none any more: below `:published' while a picture, an ack or recorded
state waits, whatever else was committed meanwhile; 0 after a
reconnection until the new compositor has every picture), `:shown'
(the same, once the compositor has also used each shown window's
newest commit: its frame callback is done; behind while a window the
compositor shows holds its callback back), `:thread-on' (1 when it presents on its own thread, 0 inline), and its
timing: `:presenter-frames' root buffers that showed published drawing,
`:presenter-late' of them committed more than a refresh after they were
ready, and `:ready-to-commit-max-ns'.  */)
  (Lisp_Object display)
{
  struct nhwl_display_info *dpyinfo = check_nhwl_display_info (display);
  NhgPresenterStats s = { 0 };

  if (nhwl_glue_rc (dpyinfo, NHG (nhg_presenter_stats (dpyinfo->glue, &s)))
      < 0)
    error ("The display is gone");
  return list (QCpackets, make_uint (s.counters.packets),
	       QCsuperseded, make_uint (s.counters.superseded),
	       QCcommits, make_uint (s.counters.commits),
	       QCbare_commits, make_uint (s.counters.bare_commits),
	       QCmain_wait_max_ns, make_uint (s.counters.main_wait_max_ns),
	       QCpublish_copy_max_ns,
	       make_uint (s.counters.publish_copy_max_ns),
	       QCfront_trims, make_uint (s.counters.front_trims),
	       QCpublished, make_uint (s.counters.published),
	       QCpresented, make_uint (s.counters.presented),
	       QCshown, make_uint (s.counters.shown),
	       QCthread_on, make_fixnum (s.counters.thread_on),
	       QCpresenter_frames, make_uint (s.frames),
	       QCpresenter_late, make_uint (s.late),
	       QCready_to_commit_max_ns, make_uint (s.ready_to_commit_max_ns));
}

DEFUN ("nhwl-glue-panics", Fnhwl_glue_panics, Snhwl_glue_panics, 0, 0, 0,
       doc: /* Return the reports of the shell glue's panics, newest first.
Each is a string: the panic's message and location, then a backtrace.  A
panic inside the glue drops the display it happened on; Emacs keeps
running.  */)
  (void)
{
  return Fcopy_sequence (nhwl_glue_panic_list);
}

DEFUN ("nhwl--scroll-layers", Fnhwl__scroll_layers, Snhwl__scroll_layers,
       0, 1, 0,
       doc: /* Return how many layers of their own FRAME's windows have.
FRAME defaults to the selected frame.  A window that is its own output
scrolls by moving its layer's view (`nhwl-scroll-layers'); a card is one
more.  For tests.  */)
  (Lisp_Object frame)
{
  struct frame *f = decode_live_frame (frame);
  struct nhwl_output *out;
  int32_t rc;

  if (!FRAME_NHWL_P (f))
    error ("Not a Nidhogg shell frame");
  out = FRAME_NHWL_OUTPUT (f);
  if (!out->glitnir_id || !FRAME_NHG (f))
    return make_fixnum (0);
  rc = nhwl_glue_rc (FRAME_DISPLAY_INFO (f),
		     NHG (nhg_layers_count (FRAME_NHG (f), out->glitnir_id)));
  return make_fixnum (max (rc, 0));
}

DEFUN ("nhwl--frame-states", Fnhwl__frame_states, Snhwl__frame_states,
       0, 1, 0,
       doc: /* Return the states of FRAME's last configure, a list.
FRAME defaults to the selected frame.  The list holds `fullscreen' and
`maximized' as the compositor last configured them: its answer, while
the `fullscreen' parameter says at once what the frame asked for.  For
tests.  */)
  (Lisp_Object frame)
{
  struct frame *f = decode_live_frame (frame);
  Lisp_Object states = Qnil;

  if (!FRAME_NHWL_P (f))
    error ("Not a Nidhogg shell frame");
  if (FRAME_NHWL_OUTPUT (f)->states & NHG_STATE_MAXIMIZED)
    states = Fcons (Qmaximized, states);
  if (FRAME_NHWL_OUTPUT (f)->states & NHG_STATE_FULLSCREEN)
    states = Fcons (Qfullscreen, states);
  return states;
}

DEFUN ("nhwl--hold-configures", Fnhwl__hold_configures,
       Snhwl__hold_configures, 1, 2, 0,
       doc: /* Leave the compositor's configures to FRAME's display unread if ON.
FRAME defaults to the selected frame.  While ON is non-nil the shell
reads no configure (nor anything else its main queue carries): what
the compositor sends waits, as a late answer would.  ON nil reads it
at the next read of input, in the order it came.  Return nil.

This is a test primitive: it stalls the shell's main queue, so a
frame that waits for the compositor waits on it.  A row that needs the
compositor's answer read after something the frame does holds it back.
Nothing outside tests may call it (drgnx never does).  */)
  (Lisp_Object on, Lisp_Object frame)
{
  struct frame *f = decode_live_frame (frame);
  struct nhwl_display_info *dpyinfo;
  int32_t rc;

  if (!FRAME_NHWL_P (f))
    error ("Not a Nidhogg shell frame");
  dpyinfo = FRAME_DISPLAY_INFO (f);
  rc = NHG (nhg_hold_main (dpyinfo->glue, !NILP (on)));
  if (nhwl_glue_rc (dpyinfo, rc) < 0)
    error ("The shell cannot hold its configures");
  return Qnil;
}

DEFUN ("nhwl--focus-frames", Fnhwl__focus_frames, Snhwl__focus_frames,
       0, 1, 0,
       doc: /* Return the frames FRAME's display holds for focus, a plist.
FRAME defaults to the selected frame.  `:focus' is the frame with the
keyboard focus, `:highlight' the one whose cursor is drawn active (the
focus frame, or the frame its focus is redirected to), `:event' the
frame of the last focus event; each a frame or nil.  For tests.  */)
  (Lisp_Object frame)
{
  struct frame *f = decode_live_frame (frame);
  struct nhwl_display_info *dpyinfo;
  Lisp_Object focus = Qnil, highlight = Qnil, event = Qnil;

  if (!FRAME_NHWL_P (f))
    error ("Not a Nidhogg shell frame");
  dpyinfo = FRAME_DISPLAY_INFO (f);
  if (dpyinfo->x_focus_frame)
    XSETFRAME (focus, dpyinfo->x_focus_frame);
  if (dpyinfo->highlight_frame)
    XSETFRAME (highlight, dpyinfo->highlight_frame);
  if (dpyinfo->x_focus_event_frame)
    XSETFRAME (event, dpyinfo->x_focus_event_frame);
  return listn (6, QCfocus, focus, QChighlight, highlight, QCevent, event);
}

#ifdef ENABLE_CHECKING
DEFUN ("nhwl--test-glue-panic", Fnhwl__test_glue_panic,
       Snhwl__test_glue_panic, 0, 1, 0,
       doc: /* Make the shell glue panic on FRAME's display.
FRAME defaults to the selected frame.  The display is dropped at the
next read of input.  Return the glue's code (-1 for a panic).  For
tests; only in builds with checking.  */)
  (Lisp_Object frame)
{
  struct frame *f = decode_live_frame (frame);
  struct nhwl_display_info *dpyinfo;
  int32_t rc;

  if (!FRAME_NHWL_P (f))
    error ("Not a Nidhogg shell frame");
  dpyinfo = FRAME_DISPLAY_INFO (f);
  rc = NHG (nhg_test_panic (dpyinfo->glue));
  return make_fixnum (nhwl_glue_rc (dpyinfo, rc));
}
#endif

void
syms_of_nhwlterm (void)
{
  defsubr (&Snhwl__bench_full_redraw);
  defsubr (&Sglitnir_edges_resolved);
  defsubr (&Sglitnir_minimap_flash);
  defsubr (&Snhwl_presenter_stats);
  defsubr (&Snhwl_glue_panics);
  defsubr (&Snhwl__scroll_layers);
  defsubr (&Snhwl__frame_states);
  defsubr (&Snhwl__focus_frames);
  defsubr (&Snhwl__hold_configures);
  DEFSYM (QCfocus, ":focus");
  DEFSYM (QChighlight, ":highlight");
  DEFSYM (QCevent, ":event");
  DEFSYM (Qnidhogg_scale, "nidhogg-scale");
  DEFSYM (Qnidhogg_text_zoom, "nidhogg-text-zoom");
#ifdef ENABLE_CHECKING
  defsubr (&Snhwl__test_glue_panic);
#endif
  nhwl_glue_panic_list = Qnil;
  staticpro (&nhwl_glue_panic_list);

  DEFVAR_LISP ("nidhogg-key-events", Vnidhogg_key_events,
    doc: /* Non-nil means key releases arrive as `key-release' events.
A release is the special event (key-release EVENT), EVENT the event its
press gave (with the modifiers held at the release); bind `key-release'
in `special-event-map' to handle it.  A release whose press gave no
event (a dead key, a key pressed before the frame had the keyboard)
gives none.  Nil, the default, drops releases.  */);
  Vnidhogg_key_events = Qnil;
  DEFVAR_LISP ("nidhogg-lone-modifier-events", Vnidhogg_lone_modifier_events,
    doc: /* Modifiers whose key pressed and released alone is an event.
A list of `alt', `control', `shift' and `super'.  A key of a modifier in
it (Alt or Meta, Control, Shift, Super or Hyper), pressed while no other
modifier is held and released with no other key or mouse button pressed
between, gives the event `lone-alt' (`lone-control', `lone-shift',
`lone-super'), which a keymap can bind like a function key.  Nil, the
default, gives none.  */);
  Vnidhogg_lone_modifier_events = Qnil;
  DEFVAR_LISP ("nidhogg-last-key-repeat", Vnidhogg_last_key_repeat,
    doc: /* Non-nil if the last key event read was a key repeat.
Set as each keystroke is read: t when the keyboard repeated a held key,
nil for a press (or a key that did not come from a Nidhogg frame).  */);
  Vnidhogg_last_key_repeat = Qnil;

  /* Reconnecting in place; nhwl-settings.el has the options.  */
  DEFSYM (Qnhwl_reconnect, "nhwl-reconnect");
  DEFVAR_LISP ("nhwl-reconnect-timeout", Vnhwl_reconnect_timeout,
    doc: /* Seconds to wait for a restarted compositor.  */);
  Vnhwl_reconnect_timeout = make_float (10.0);
  DEFSYM (Qnhwl_display_connection_functions,
	  "nhwl-display-connection-functions");
  DEFSYM (Qnhwl_reconnecting, "reconnecting");
  DEFSYM (Qnhwl_reconnected, "reconnected");
  /* The display is given up (nhwl_connection_lost).  */
  DEFSYM (Qnhwl_lost, "lost");

  DEFSYM (QCpackets, ":packets");
  DEFSYM (QCsuperseded, ":superseded");
  DEFSYM (QCcommits, ":commits");
  DEFSYM (QCbare_commits, ":bare-commits");
  DEFSYM (QCmain_wait_max_ns, ":main-wait-max-ns");
  DEFSYM (QCpublish_copy_max_ns, ":publish-copy-max-ns");
  DEFSYM (QCfront_trims, ":front-trims");
  DEFSYM (QCpublished, ":published");
  DEFSYM (QCpresented, ":presented");
  DEFSYM (QCshown, ":shown");
  DEFSYM (QCthread_on, ":thread-on");
  DEFSYM (QCpresenter_frames, ":presenter-frames");
  DEFSYM (QCpresenter_late, ":presenter-late");
  DEFSYM (QCready_to_commit_max_ns, ":ready-to-commit-max-ns");

  DEFVAR_BOOL ("nhwl-presenter-thread", nhwl_presenter_thread,
    doc: /* Non-nil means a thread of the shell presents each window.
It composes and commits a window's picture when the compositor is ready
for it, whatever Emacs is doing; Emacs's thread only publishes what it
drew.  Nil means Emacs presents on its own thread, when its event loop
learns the compositor is ready.  Read when a display opens; the
environment variable NHWL_PRESENTER=0 turns it off as well.  */);
  nhwl_presenter_thread = true;

  DEFSYM (Qinteger_scales, "integer-scales");
  DEFVAR_LISP ("nhwl-scroll-layers", Vnhwl_scroll_layers,
    doc: /* Non-nil means a window that scrolls whole gets a layer of its own.
The compositor then shows each scroll by moving the layer's view, and
uploads only the rows the scroll exposed instead of the whole window.
Only a top-level frame that is opaque (`alpha' and `alpha-background'
1) gets layers, at most four windows each; a window's layer goes after
2 seconds without such a scroll, and when the window is resized, split
or deleted, the frame's scale changes or it becomes translucent.  The
symbol `integer-scales' means only at a whole scale (1, 2, ...).  Nil
means never.  Takes effect at the next redisplay.  */);
  Vnhwl_scroll_layers = Qt;

  DEFSYM (Qmodifier_value, "modifier-value");
  DEFSYM (Qalt, "alt");
  DEFSYM (Qhyper, "hyper");
  DEFSYM (Qmeta, "meta");
  DEFSYM (Qsuper, "super");
  DEFSYM (Qcontrol, "control");
  DEFSYM (Qshift, "shift");
  DEFSYM (Qlone_alt, "lone-alt");
  DEFSYM (Qlone_control, "lone-control");
  DEFSYM (Qlone_shift, "lone-shift");
  DEFSYM (Qlone_super, "lone-super");

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

  DEFVAR_BOOL ("nhwl--test-drain-in-window-new", nhwl_test_drain_in_window_new,
    doc: /* Non-nil: drain the shell's events right after a frame's window is made.
As an `unblock_input' that handles a pending signal does there.  For tests.  */);
  nhwl_test_drain_in_window_new = false;

  DEFVAR_BOOL ("nhwl--test-fatal-in-handler", nhwl_test_fatal_in_handler,
    doc: /* Non-nil: a SIGTERM inside a shell glue call runs kill-emacs at once.
In the signal handler, inside the call, as before the call's end ran it.
For tests.  */);
  nhwl_test_fatal_in_handler = false;

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

#ifdef NHWL_WAYLAND
  /* Tell Emacs about this window system: `pgtk' at the Lisp level, so
     term/pgtk-win.el loads unchanged, and `nidhogg-wayland'.  */
  Fprovide (Qpgtk, Qnil);
  Fprovide (intern_c_string ("nidhogg-wayland"), Qnil);
#else
  syms_of_nhmacterm ();
#endif
}

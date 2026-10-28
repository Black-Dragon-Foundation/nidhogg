/* Popup frames for Emacs-drawn menus on Nidhogg's Wayland shell.

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

/* A menu level is a frame whose `nhwl-popup' parameter, given at
   creation with `parent-frame', makes its window an xdg popup of the
   parent's window: (X Y W H . PLIST), the rectangle it sits beside in
   device pixels of the parent's native area, and PLIST's `:gravity'
   (the side or corner it grows toward, `bottom-right' by default),
   `:adjust' (how the compositor keeps it on the output: `flip-x',
   `flip-y', `slide-x', `slide-y'; all by default) and `:grab' (it takes
   the keyboard and pointer, and a click elsewhere dismisses it).

   A popup frame is the root of its own tree (its pacer and its acks are
   its own) and is presented by nhwl_present_tree like any frame.  Its
   window exists while it is shown: made by `make-frame-visible' at the
   current anchor, destroyed by `make-frame-invisible', its popup frames
   first.  Keys and pointer events on the popup go to its frame; keyboard
   focus moving between a top-level and its own popups is no focus change
   for Emacs.  The compositor dismissing it is a `delete-frame' event.
   The menus themselves are Lisp (nhwl-menu.el), reached from
   `x-popup-menu' through nhwl_popup_menu.  */

#include <config.h>

#include "lisp.h"
#include "nhwlterm.h"
#include "blockinput.h"
#include "frame.h"

/* The `:gravity' names, NhShellAnchor's gravity numbers 1 to 8.  */
static const char *const gravity_names[] = {
  "top", "bottom", "left", "right", "top-left", "bottom-left", "top-right",
  "bottom-right"
};

/* The `:adjust' names and their NHSHELL_ADJUST_* bits.  */
static const struct { const char *name; uint32_t bit; } adjust_names[] = {
  { "flip-x", NHSHELL_ADJUST_FLIP_X }, { "flip-y", NHSHELL_ADJUST_FLIP_Y },
  { "slide-x", NHSHELL_ADJUST_SLIDE_X }, { "slide-y", NHSHELL_ADJUST_SLIDE_Y }
};

#define ADJUST_ALL (NHSHELL_ADJUST_FLIP_X | NHSHELL_ADJUST_FLIP_Y \
		    | NHSHELL_ADJUST_SLIDE_X | NHSHELL_ADJUST_SLIDE_Y)

bool
nhwl_popup_frame_p (struct frame *f)
{
  return (FRAME_NHWL_P (f) && f->output_data.nhwl
	  && f->output_data.nhwl->popup.on);
}

/* The top-level frame popup or child frame F hangs from.  */
static struct frame *
popup_owner (struct frame *f)
{
  while (nhwl_popup_frame_p (f) || nhwl_child_p (f))
    f = FRAME_PARENT_FRAME (f);
  return f;
}

static AVOID
popup_invalid (Lisp_Object value)
{
  xsignal2 (Qerror, build_string ("Invalid `nhwl-popup' parameter"), value);
}

/* Read the value V of `nhwl-popup' into P (its anchor, gravity, adjust
   and grab).  */
static void
popup_parse (Lisp_Object v, struct nhwl_popup *p)
{
  Lisp_Object tail = v, plist, g, adj;
  int xywh[4], i;
  size_t k;

  for (i = 0; i < 4; i++, tail = XCDR (tail))
    {
      if (!CONSP (tail) || !RANGED_FIXNUMP (-0x3fffffff, XCAR (tail), 0x3fffffff))
	popup_invalid (v);
      xywh[i] = XFIXNUM (XCAR (tail));
    }
  plist = tail;
  p->x = xywh[0], p->y = xywh[1], p->w = max (xywh[2], 0), p->h = max (xywh[3], 0);

  g = Fplist_get (plist, QCgravity, Qnil);
  p->gravity = NILP (g) ? 8 : 0;
  for (k = 0; SYMBOLP (g) && !NILP (g) && k < ARRAYELTS (gravity_names); k++)
    if (!strcmp (SSDATA (SYMBOL_NAME (g)), gravity_names[k]))
      p->gravity = k + 1;
  if (!p->gravity)
    popup_invalid (v);

  if (NILP (Fplist_member (plist, QCadjust, Qnil)))
    p->adjust = ADJUST_ALL;
  else
    {
      p->adjust = 0;
      for (adj = Fplist_get (plist, QCadjust, Qnil); CONSP (adj); adj = XCDR (adj))
	{
	  uint32_t bit = 0;

	  for (k = 0; SYMBOLP (XCAR (adj)) && k < ARRAYELTS (adjust_names); k++)
	    if (!strcmp (SSDATA (SYMBOL_NAME (XCAR (adj))), adjust_names[k].name))
	      bit = adjust_names[k].bit;
	  if (!bit)
	    popup_invalid (v);
	  p->adjust |= bit;
	}
    }
  p->grab = !NILP (Fplist_get (plist, QCgrab, Qnil));
}

/* Frame F is being made (x-create-frame, with PARMS): a popup frame when
   PARMS has `nhwl-popup'.  */
void
nhwl_popup_init (struct frame *f, Lisp_Object parms)
{
  Lisp_Object v = Fcdr (Fassq (Qnhwl_popup, parms));
  struct nhwl_popup *p = &FRAME_NHWL_OUTPUT (f)->popup;

  if (NILP (v))
    return;
  if (!FRAME_PARENT_FRAME (f))
    error ("nidhogg-wayland: a popup frame needs a `parent-frame'");
  popup_parse (v, p);
  p->on = true;
}

/* The anchor of popup frame F in logical pixels of its parent's surface
   (the parent's native area starts after its title bar and margins).  */
static void
popup_anchor (struct frame *f, NhShellAnchor *a)
{
  struct nhwl_popup *p = &FRAME_NHWL_OUTPUT (f)->popup;
  struct nhwl_output *po = FRAME_NHWL_OUTPUT (FRAME_PARENT_FRAME (f));
  uint32_t s = po->scale120 ? po->scale120 : 120;
  int x0 = nhwl_to_logical (po->csd.l + p->x, s);
  int y0 = nhwl_to_logical (po->csd.t + p->y, s);

  a->x = x0;
  a->y = y0;
  a->w = max (1, nhwl_to_logical (po->csd.l + p->x + p->w, s) - x0);
  a->h = max (1, nhwl_to_logical (po->csd.t + p->y + p->h, s) - y0);
  a->gravity = p->gravity;
  a->adjust = p->adjust;
}

/* Whether a shown popup frame hanging from top-level R holds a grab.  */
bool
nhwl_popup_grabbing (struct frame *r)
{
  struct nhwl_display_info *dpyinfo = FRAME_DISPLAY_INFO (r);
  struct nhwl_output *o;

  if (!dpyinfo->shell)
    return false;
  for (o = dpyinfo->outputs; o; o = o->next_output)
    if (o->popup.on && o->popup.grab && o->window
	&& FRAME_VISIBLE_P (o->frame) && popup_owner (o->frame) == r
	&& nhshell_window_grabbed (dpyinfo->shell, o->window) == 1)
      return true;
  return false;
}

/* Show popup frame F: a new popup of its parent's window, at the current
   anchor and F's size.  */
void
nhwl_popup_show (struct frame *f)
{
  struct nhwl_output *out = FRAME_NHWL_OUTPUT (f);
  struct nhwl_display_info *dpyinfo = FRAME_DISPLAY_INFO (f);
  struct frame *pf = FRAME_PARENT_FRAME (f);
  struct nhwl_output *po;
  NhShellWindowDesc desc;
  uint64_t id = 0;
  int rc;

  if (out->window || !dpyinfo->shell)
    return;
  if (!pf || !FRAME_LIVE_P (pf) || !FRAME_NHWL_P (pf)
      || !(po = FRAME_NHWL_OUTPUT (pf))->window)
    error ("nidhogg-wayland: the parent of a popup frame is not shown");

  /* A popup starts at its parent's scale, and at the size the frame is
     about to take.  */
  out->scale120 = po->scale120 ? po->scale120 : 120;
  memset (&desc, 0, sizeof desc);
  desc.w = max (1, nhwl_to_logical (f->new_size_p ? f->new_width
				    : FRAME_PIXEL_WIDTH (f), out->scale120));
  desc.h = max (1, nhwl_to_logical (f->new_size_p ? f->new_height
				    : FRAME_PIXEL_HEIGHT (f), out->scale120));
  desc.title = "";
  desc.app_id = "nidhogg";
  desc.flags = out->popup.grab ? NHSHELL_WIN_GRAB : 0;
  desc.kind = NHSHELL_KIND_POPUP;
  desc.parent = po->window;
  popup_anchor (f, &desc.anchor);

  block_input ();
  rc = nhshell_window_new (dpyinfo->shell, &desc, &id);
  unblock_input ();
  if (rc < 0)
    error ("nidhogg-wayland: cannot create a popup window (%d)", rc);
  out->window = id;
  out->popup.sent_w = desc.w;
  out->popup.sent_h = desc.h;
  /* `alpha' goes to the new window.  */
  out->alpha_cut = 0;
  out->alpha_host = false;
  nhwl_set_frame_alpha (f);

  SET_FRAME_VISIBLE (f, 1);
  SET_FRAME_ICONIFIED (f, false);
  out->has_been_visible = true;
  out->nhist = 0;
  nhwl_damage_all (f);
  SET_FRAME_GARBAGED (f);
}

/* Hide popup frame F: its window goes, after those of the popup frames
   hanging from it (the compositor wants the topmost popup destroyed
   first).  When F's top-level had lent its keyboard focus to a grabbing
   popup, and the keyboard is now on none of our windows, the top-level
   has lost the focus (the compositor dismissed the menu while focus went
   elsewhere).  */
void
nhwl_popup_hide (struct frame *f)
{
  struct nhwl_output *out = FRAME_NHWL_OUTPUT (f);
  struct nhwl_display_info *dpyinfo = FRAME_DISPLAY_INFO (f);
  struct nhwl_output *o;
  struct frame *r;

  for (o = dpyinfo->outputs; o; o = o->next_output)
    if (o->popup.on && o->window && FRAME_PARENT_FRAME (o->frame) == f)
      nhwl_popup_hide (o->frame);
  if (out->window && dpyinfo->shell)
    {
      block_input ();
      nhshell_window_destroy (dpyinfo->shell, out->window);
      unblock_input ();
    }
  out->window = 0;
  SET_FRAME_VISIBLE (f, 0);
  SET_FRAME_ICONIFIED (f, false);

  r = popup_owner (f);
  if (FRAME_NHWL_OUTPUT (r)->focus_lent && !dpyinfo->kbd_window
      && !nhwl_popup_grabbing (r))
    {
      FRAME_NHWL_OUTPUT (r)->focus_lent = false;
      nhwl_focus_lost (r);
    }
}

/* Popup frame F's size is now WIDTH x HEIGHT device pixels: the popup
   takes the new size, placed again by its anchor.  The whole frame is
   drawn again when the shell made the popup again (below xdg_popup 3).  */
void
nhwl_popup_set_size (struct frame *f, int width, int height)
{
  struct nhwl_output *out = FRAME_NHWL_OUTPUT (f);
  uint32_t s120 = out->scale120 ? out->scale120 : 120;
  int w, h, rc;

  if (!nhwl_popup_frame_p (f) || !out->window)
    return;
  w = max (1, nhwl_to_logical (width, s120));
  h = max (1, nhwl_to_logical (height, s120));
  if (w == out->popup.sent_w && h == out->popup.sent_h)
    return;
  block_input ();
  rc = nhshell_window_set_size (FRAME_NHWL_SHELL (f), out->window, w, h);
  unblock_input ();
  if (rc < 0)
    return;
  out->popup.sent_w = w;
  out->popup.sent_h = h;
  if (rc == 1)
    nhwl_damage_all (f);
}

/* F's parameters changed: a new `nhwl-popup' anchor position moves the
   shown popup (xdg_popup.reposition).  Its size, gravity, adjust and grab
   take effect when it is shown next.  */
void
nhwl_popup_params_changed (struct frame *f)
{
  struct nhwl_output *out;
  struct nhwl_popup n;
  NhShellAnchor a;
  Lisp_Object v;
  int rc;

  /* Not in the frame's parameters yet while x-create-frame runs.  */
  if (!nhwl_popup_frame_p (f) || NILP (v = get_frame_param (f, Qnhwl_popup)))
    return;
  out = FRAME_NHWL_OUTPUT (f);
  n = out->popup;
  popup_parse (v, &n);
  if (n.x == out->popup.x && n.y == out->popup.y && n.w == out->popup.w
      && n.h == out->popup.h && n.gravity == out->popup.gravity
      && n.adjust == out->popup.adjust && n.grab == out->popup.grab)
    return;
  out->popup = n;
  if (!out->window)
    return;
  popup_anchor (f, &a);
  block_input ();
  rc = nhshell_window_set_position (FRAME_NHWL_SHELL (f), out->window, a.x, a.y);
  unblock_input ();
  if (rc == 1)
    nhwl_damage_all (f);
}

/* A keyboard focus event (IN, or out) for F's window: true when Emacs
   sees no focus change.  A popup frame's focus is its top-level's; a
   top-level that loses the keyboard while a popup frame of its own holds
   a grab has lent it to that popup, and keeps Emacs's focus.  */
bool
nhwl_popup_focus_kept (struct frame *f, bool in)
{
  struct nhwl_output *ro;

  if (nhwl_popup_frame_p (f))
    return true;
  ro = FRAME_NHWL_OUTPUT (nhwl_tree_root (f));
  if (in)
    {
      ro->focus_lent = false;
      return false;
    }
  if (!nhwl_popup_grabbing (nhwl_tree_root (f)))
    return false;
  ro->focus_lent = true;
  return true;
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

void
syms_of_nhwlmenu (void)
{
  DEFSYM (Qnhwl_popup, "nhwl-popup");
  DEFSYM (Qnhwl_menu_popup, "nhwl-menu-popup");
  DEFSYM (QCgravity, ":gravity");
  DEFSYM (QCadjust, ":adjust");
  DEFSYM (QCgrab, ":grab");
}

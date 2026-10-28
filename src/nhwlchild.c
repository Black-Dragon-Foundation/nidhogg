/* Child frames of Nidhogg's own Wayland shell (HAVE_NHWL).
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

/* A child frame is a sync subsurface of its parent frame's window (shell
   kind NHSHELL_KIND_CHILD), presented with its tree: the root frame's
   pacer commits the child frames deepest first, then the root
   (nhwl_present_tree).  It has its own backing and its own Glitnir root,
   like any frame; the root's one layer carries the frame's chrome (rounded
   corners, shadow).  The shadow needs room around the native area: that
   margin is held in the output's CSD offsets (csd.l/t/r/b), so the backing,
   the damage and the pointer use the offset they already have for a
   top-level's title bar.  The surface is the native area plus the margin;
   its input region is the native area only.  */

#include <config.h>
#include <math.h>
#include "lisp.h"
#include "nhwlterm.h"
#include "blockinput.h"
#include "window.h"

#ifdef HAVE_NHWL

/* Stacking order counters: a raised child gets a number above all, a lowered
   one below all.  */
static int z_top, z_bottom;

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
  return FRAME_NHWL_P (f) && f->output_data.nhwl && FRAME_PARENT_FRAME (f);
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

/* Device pixels to logical, rounded to nearest (also for negatives).  */
static int
dev_to_log (int dev, uint32_t s120)
{
  return (int) lround (dev * 120.0 / (s120 ? s120 : 120));
}

/* The child's surface offset in its parent's surface, logical pixels: the
   frame's left/top are device pixels from the parent's native area, and the
   surface starts the shadow margin before the child's native area.  */
static void
child_surface_pos (struct frame *f, int *x, int *y)
{
  struct nhwl_output *out = FRAME_NHWL_OUTPUT (f);
  struct nhwl_output *po = FRAME_NHWL_OUTPUT (FRAME_PARENT_FRAME (f));

  *x = dev_to_log (po->csd.l + f->left_pos - out->csd.l, po->scale120);
  *y = dev_to_log (po->csd.t + f->top_pos - out->csd.t, po->scale120);
}

/* Fill DESC for F's window when F is a child frame.  */
void
nhwl_child_describe (struct frame *f, NhShellWindowDesc *desc)
{
  if (!nhwl_child_p (f))
    return;
  desc->kind = NHSHELL_KIND_CHILD;
  desc->parent = FRAME_NHWL_OUTPUT (FRAME_PARENT_FRAME (f))->window;
  child_surface_pos (f, &desc->x, &desc->y);
}

/* F's shell window exists.  A subsurface has no configure: it is ready.  */
void
nhwl_child_created (struct frame *f)
{
  struct nhwl_output *out = FRAME_NHWL_OUTPUT (f);

  if (!nhwl_child_p (f))
    return;
  out->configured = true;
  out->child.mapped = true;
  out->child.region_sent = false;
  out->child.z = ++z_top;
  /* The window was made at this position.  */
  child_surface_pos (f, &out->child.pos[0], &out->child.pos[1]);
  out->child.pos_sent = true;
  nhwl_child_layout (f);
}


/***********************************************************************
			      Chrome and margin
 ***********************************************************************/

static float
chrome_num (Lisp_Object v)
{
  return NUMBERP (v) ? (float) max (XFLOATINT (v), 0.0) : 0;
}

static Lisp_Object
chrome_key (Lisp_Object plist, const char *name)
{
  return Fplist_get (plist, intern_c_string (name), Qnil);
}

/* The room the shadow needs on each side, device pixels: the layer grown by
   the spread and the blur's reach (3 sigma, sigma = blur / 2; one pixel
   more for rounding), moved by the offset.  */
void
nhwl_child_layout (struct frame *f)
{
  struct nhwl_output *out = FRAME_NHWL_OUTPUT (f);
  struct nhwl_child *c = &out->child;
  struct nhwl_csd *d = &out->csd;
  double s = (out->scale120 ? out->scale120 : 120) / 120.0;
  int l = 0, t = 0, r = 0, b = 0;

  if (c->chrome && c->opacity > 0)
    {
      int pad = (int) ceil (1.5 * c->blur * s) + 1 + (int) lround (c->spread * s);
      int ox = (int) lround (c->ox * s), oy = (int) lround (c->oy * s);

      l = max (0, pad - ox);
      r = max (0, pad + ox);
      t = max (0, pad - oy);
      b = max (0, pad + oy);
    }
  if (d->l != l || d->t != t || d->r != r || d->b != b)
    {
      d->l = l, d->t = t, d->r = r, d->b = b;
      SET_FRAME_GARBAGED (f);
    }
}

/* Take the chrome from `glitnir--resolve' (called after the frame's
   parameters changed); adapted from glitnir_pgtk_params_changed.  */
void
nhwl_child_params_changed (struct frame *f)
{
  static bool busy;		/* the resolver changed parameters itself */
  struct nhwl_child n, *c;
  Lisp_Object frame, res;

  if (busy || !nhwl_child_p (f) || NILP (Ffboundp (Qglitnir__resolve)))
    return;
  c = &FRAME_NHWL_OUTPUT (f)->child;
  XSETFRAME (frame, f);
  busy = true;
  res = safe_calln (Qglitnir__resolve, frame);
  busy = false;

  memset (&n, 0, sizeof n);
  n.chrome = CONSP (res);
  if (n.chrome)
    {
      Lisp_Object sh = chrome_key (res, ":shadow");

      n.radius = chrome_num (chrome_key (res, ":radius"));
      if (CONSP (sh))
	{
	  Lisp_Object off = chrome_key (sh, ":offset");
	  Lisp_Object col = chrome_key (sh, ":color");
	  uint32_t rgb = 0;
	  int i;

	  n.blur = chrome_num (chrome_key (sh, ":blur"));
	  if (CONSP (off))
	    {
	      n.ox = NUMBERP (XCAR (off)) ? XFLOATINT (XCAR (off)) : 0;
	      n.oy = NUMBERP (XCDR (off)) ? XFLOATINT (XCDR (off)) : 0;
	    }
	  n.spread = NUMBERP (chrome_key (sh, ":spread"))
	    ? XFLOATINT (chrome_key (sh, ":spread")) : 0;
	  n.opacity = min (chrome_num (chrome_key (sh, ":opacity")), 1);
	  for (i = 0; i < 3; i++, col = CONSP (col) ? XCDR (col) : Qnil)
	    rgb = rgb << 8 | (CONSP (col) && FIXNATP (XCAR (col))
			      ? min (XFIXNAT (XCAR (col)), 255) : 0);
	  n.rgb = rgb;
	}
    }
  if (n.chrome == c->chrome && n.radius == c->radius && n.blur == c->blur
      && n.ox == c->ox && n.oy == c->oy && n.spread == c->spread
      && n.opacity == c->opacity && n.rgb == c->rgb)
    return;
  c->chrome = n.chrome;
  c->radius = n.radius, c->blur = n.blur, c->ox = n.ox, c->oy = n.oy;
  c->spread = n.spread, c->opacity = n.opacity, c->rgb = n.rgb;
  c->chrome_dirty = true;
  nhwl_child_layout (f);
  SET_FRAME_GARBAGED (f);
}

/* The layer's rectangle in the root: the native area inside the margin when
   the frame has chrome (RECT is left as it is, the whole root, when not).
   True when it, the chrome or the scale differ from what Glitnir holds, so
   the caller upserts the layer and applies the chrome.  */
bool
nhwl_child_layer_update (struct frame *f, GlitnirRect *rect)
{
  struct nhwl_output *out = FRAME_NHWL_OUTPUT (f);
  struct nhwl_child *c = &out->child;
  uint32_t s120 = out->scale120 ? out->scale120 : 120;
  bool changed;

  if (!nhwl_child_p (f))
    return false;
  if (c->chrome)
    *rect = (GlitnirRect) { out->csd.l, out->csd.t, FRAME_PIXEL_WIDTH (f),
			    FRAME_PIXEL_HEIGHT (f) };
  changed = (!c->layer_set || c->chrome_dirty || c->chrome_s120 != s120
	     || memcmp (&c->layer, rect, sizeof *rect) != 0);
  c->layer = *rect;
  c->layer_set = true;
  return changed;
}

/* Hand the chrome, in device pixels, to the root's layer.  */
void
nhwl_child_apply_chrome (struct frame *f)
{
  struct nhwl_output *out = FRAME_NHWL_OUTPUT (f);
  struct nhwl_child *c = &out->child;
  double s = (out->scale120 ? out->scale120 : 120) / 120.0;

  c->chrome_dirty = false;
  c->chrome_s120 = out->scale120 ? out->scale120 : 120;
  if (!c->chrome)
    {
      glitnir_layer_set_chrome (out->glitnir_id, out->glitnir_id, NULL);
      return;
    }
  GlitnirChrome g = {
    .radius_px = c->radius * s,
    .blur_px = c->blur * s,
    .offset_x = c->ox * s,
    .offset_y = c->oy * s,
    .spread_px = c->spread * s,
    .shadow_opacity = c->opacity,
    .shadow_argb = 0xff000000u | c->rgb,
  };
  glitnir_layer_set_chrome (out->glitnir_id, out->glitnir_id, &g);
}

/* The source of the root's layer starts at the layer's top-left, which is
   inside the backing when the frame has a margin.  */
void
nhwl_child_source (struct frame *f, GlitnirSource *s)
{
  struct nhwl_child *c = &FRAME_NHWL_OUTPUT (f)->child;

  if (!nhwl_child_p (f) || !c->chrome || !c->layer_set)
    return;
  s->pixels += (ptrdiff_t) c->layer.y * s->stride + (ptrdiff_t) c->layer.x * 4;
  s->width = min (c->layer.w, s->width - c->layer.x);
  s->height = min (c->layer.h, s->height - c->layer.y);
}


/***********************************************************************
		     Position, input region, visibility
 ***********************************************************************/

/* A child's position and stacking take effect with its parent's next commit
   (Wayland's rule): the shell marks the child and every ancestor owed, and
   the tree's pacer commits them, deepest first, bare when nothing else is
   drawn.  */
static void
send_position (struct frame *f)
{
  struct nhwl_output *out = FRAME_NHWL_OUTPUT (f);
  struct nhwl_child *c = &out->child;
  int x, y;

  child_surface_pos (f, &x, &y);
  if (c->pos_sent && c->pos[0] == x && c->pos[1] == y)
    return;
  if (nhshell_window_set_position (FRAME_NHWL_SHELL (f), out->window, x, y) >= 0)
    {
      c->pos[0] = x, c->pos[1] = y;
      c->pos_sent = true;
    }
}

/* Before a present: the position and the input region (the native area)
   the compositor holds are the frame's now.  Only changes are sent.  */
void
nhwl_child_sync (struct frame *f)
{
  struct nhwl_output *out = FRAME_NHWL_OUTPUT (f);
  struct nhwl_child *c = &out->child;
  uint32_t s120 = out->scale120;
  int reg[4] = { 0, 0, 0, 0 };

  if (!nhwl_child_p (f) || !out->window)
    return;
  send_position (f);
  if (out->csd.l || out->csd.t || out->csd.r || out->csd.b)
    {
      reg[0] = dev_to_log (out->csd.l, s120);
      reg[1] = dev_to_log (out->csd.t, s120);
      reg[2] = max (1, dev_to_log (FRAME_PIXEL_WIDTH (f), s120));
      reg[3] = max (1, dev_to_log (FRAME_PIXEL_HEIGHT (f), s120));
    }
  if (c->region_sent && !memcmp (reg, c->region, sizeof reg))
    return;
  if (!c->region_sent && !reg[2])
    {
      c->region_sent = true;	/* the whole surface is the default */
      return;
    }
  {
    GlitnirRect r = { reg[0], reg[1], reg[2], reg[3] };

    if (nhshell_window_set_input_region (FRAME_NHWL_SHELL (f), out->window,
					 &r, reg[2] ? 1 : 0) >= 0)
      {
	memcpy (c->region, reg, sizeof reg);
	c->region_sent = true;
      }
  }
}

/* The frame moved (left/top changed).  */
void
nhwl_child_reposition (struct frame *f)
{
  if (nhwl_child_p (f) && FRAME_NHWL_OUTPUT (f)->window)
    send_position (f);
}

/* F's native area moved inside its surface (its title bar or margins
   changed): its child frames follow.  F presents next, which commits them.  */
void
nhwl_child_parent_layout (struct frame *f)
{
  Lisp_Object tail, frame;

  FOR_EACH_FRAME (tail, frame)
    {
      struct frame *g = XFRAME (frame);

      if (nhwl_child_p (g) && FRAME_PARENT_FRAME (g) == f
	  && FRAME_NHWL_OUTPUT (g)->window)
	send_position (g);
    }
}

/* Show or hide the child's surface; showing repaints all of it.  */
void
nhwl_child_set_visible (struct frame *f, bool on)
{
  struct nhwl_output *out = FRAME_NHWL_OUTPUT (f);
  struct nhwl_child *c = &out->child;
  NhShell *sh = FRAME_NHWL_SHELL (f);

  if (!nhwl_child_p (f) || !out->window)
    return;
  if (on && !c->mapped)
    {
      nhshell_window_set_mapped (sh, out->window, 1);
      c->mapped = true;
      out->nhist = 0;
      nhwl_damage_all (f);
    }
  else if (!on && c->mapped)
    {
      nhshell_window_set_mapped (sh, out->window, 0);
      c->mapped = false;
    }
}


/***********************************************************************
			   Stacking, z-group
 ***********************************************************************/

/* The sibling of child frame F that is highest (TOP) or lowest, if any.  */
static struct frame *
extreme_sibling (struct frame *f, bool top)
{
  Lisp_Object tail, frame;
  struct frame *best = NULL;

  FOR_EACH_FRAME (tail, frame)
    {
      struct frame *g = XFRAME (frame);

      if (g == f || !nhwl_child_p (g) || !FRAME_NHWL_OUTPUT (g)->window
	  || FRAME_PARENT_FRAME (g) != FRAME_PARENT_FRAME (f))
	continue;
      if (!best
	  || (top ? FRAME_NHWL_OUTPUT (g)->child.z > FRAME_NHWL_OUTPUT (best)->child.z
	      : FRAME_NHWL_OUTPUT (g)->child.z < FRAME_NHWL_OUTPUT (best)->child.z))
	best = g;
    }
  return best;
}

/* The terminal's frame_raise_lower_hook: among a child's siblings.  A
   top-level cannot restack itself on Wayland; raising one asks the
   compositor to activate it.  */
void
nhwl_frame_raise_lower (struct frame *f, bool raise)
{
  struct nhwl_output *out;
  struct frame *sib;

  if (!nhwl_child_p (f))
    {
      if (raise)
	nhwl_activate_frame (f);
      return;
    }
  out = FRAME_NHWL_OUTPUT (f);
  if (!out->window)
    return;
  sib = extreme_sibling (f, raise);
  out->child.z = raise ? ++z_top : --z_bottom;
  /* The shell marks the parent and its ancestors owed (stacking is the
     parent's state).  */
  if (sib)
    nhshell_window_stack (FRAME_NHWL_SHELL (f), out->window,
			  FRAME_NHWL_OUTPUT (sib)->window, raise ? 1 : 0);
}

/* Adapted from pgtk_set_z_group: stored, and for a child frame acted on
   among its siblings.  */
void
nhwl_set_z_group (struct frame *f, Lisp_Object new_value, Lisp_Object old_value)
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
   (nhwl_child_focus).  */
void
nhwl_set_no_accept_focus (struct frame *f, Lisp_Object new_value,
			  Lisp_Object old_value)
{
  FRAME_NO_ACCEPT_FOCUS (f) = !NILP (new_value);
}


/***********************************************************************
			   Keyboard focus (W1-7)
 ***********************************************************************/

/* Wayland gives the keyboard to top-levels only: the keys and
   input-method events of a top-level's window go to the frame of its tree
   that Emacs focused, as X11's focus redirection does for child frames.  */

/* Where F's native area starts in its tree root's surface, device
   pixels.  The frames of a tree share one scale.  */
void
nhwl_native_origin (struct frame *f, int *x, int *y)
{
  if (!nhwl_child_p (f))
    {
      *x = FRAME_NHWL_OUTPUT (f)->csd.l;
      *y = FRAME_NHWL_OUTPUT (f)->csd.t;
      return;
    }
  nhwl_native_origin (FRAME_PARENT_FRAME (f), x, y);
  *x += f->left_pos;
  *y += f->top_pos;
}

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
				Reparenting
 ***********************************************************************/

/* Make F's window (and those of the child frames under it) again, under
   F's current parent.  The old window of F is gone (its dependents with
   it).  */
static void
create_tree (struct frame *f)
{
  struct nhwl_output *out = FRAME_NHWL_OUTPUT (f);
  Lisp_Object tail, frame;

  nhwl_create_frame_window (f);
  nhwl_child_created (f);
  if (FRAME_VISIBLE_P (f))
    {
      out->nhist = 0;
      nhwl_damage_all (f);
      SET_FRAME_GARBAGED (f);
    }
  FOR_EACH_FRAME (tail, frame)
    if (FRAME_NHWL_P (XFRAME (frame)) && FRAME_PARENT_FRAME (XFRAME (frame)) == f)
      create_tree (XFRAME (frame));
}

static void
forget_windows_under (struct frame *f)
{
  Lisp_Object tail, frame;

  FOR_EACH_FRAME (tail, frame)
    if (FRAME_NHWL_P (XFRAME (frame)) && FRAME_PARENT_FRAME (XFRAME (frame)) == f)
      {
	FRAME_NHWL_OUTPUT (XFRAME (frame))->window = 0;
	forget_windows_under (XFRAME (frame));
      }
}

/* The `parent-frame' parameter changed at run time.  A top-level cannot
   become a child frame or back in W1; a child changes parent by getting a
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
  if (!p || !FRAME_PARENT_FRAME (f))
    {
      store_frame_param (f, Qparent_frame, old_value);
      error ("nidhogg-wayland: a top-level frame cannot become a child frame, or back, in W1");
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
    nhshell_window_destroy (FRAME_NHWL_SHELL (f), out->window);
  out->window = 0;
  forget_windows_under (f);
  fset_parent_frame (f, new_value);
  create_tree (f);
  unblock_input ();
}

#endif /* HAVE_NHWL */

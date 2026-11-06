/* Glitnir (the compositor) on macOS: the NS side.

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

/* Lisp resolves, NS applies: lisp/glitnir.el produces a child frame's chrome
   vector (`glitnir--chrome-vector'), and this file keeps it with the frame's
   NS state and answers Glitnir's Lisp interface (nidhogg-glitnir.c) for the
   NS build.  Nothing is composed on the CPU here: the window server composes.
   Every call runs on the main thread.  With `glitnir-enable' nil, or on a
   frame without chrome, this file makes and changes nothing.

   The chrome slots are the glue's (nidhogg/glue/include/nhglue.h, which is
   header-only here: no glue code is linked into the NS build).  */

#include <config.h>

#ifdef HAVE_GLITNIR

#include <dlfcn.h>
#include <math.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>

#include "lisp.h"
#include "blockinput.h"
#include "nsterm.h"
#include "frame.h"

#include <nhglue.h>

#ifdef NS_IMPL_COCOA
#import <QuartzCore/QuartzCore.h>
#endif

verify (NHG_CHROME_SLOTS == 24);	/* the size of ns_output.glitnir_chrome */


/***********************************************************************
			   What is active
 ***********************************************************************/

/* Chrome needs macOS 10.15 (CGPreflightScreenCaptureAccess, cornerCurve);
   below it Glitnir is not active and the frame is stock.  GNUstep builds
   are stock.  */
static bool
ns_glitnir_os_ok (void)
{
#ifdef NS_IMPL_COCOA
  if (@available (macOS 10.15, *))
    return true;
#endif
  return false;
}

/* True when Glitnir's decorations draw on F: a frame of this display, not a
   tooltip, while `glitnir-enable' is on.  */
bool
ns_glitnir_active_p (struct frame *f)
{
  return (glitnir_enable && ns_glitnir_os_ok () && FRAME_NS_P (f)
	  && f->output_data.ns && !FRAME_TOOLTIP_P (f));
}


/***********************************************************************
			   The vector intake
 ***********************************************************************/

static void ns_glitnir_apply (struct frame *f);

/* Take the chrome from `glitnir--chrome-vector' (called after the frame's
   parameters changed, and once a frame is made), keep it with the frame's
   output data and apply it to the frame's window.  A frame that is not a
   child frame keeps no chrome (root frames ignore it), so one that stopped
   being a child frame loses its own.  The resolver may change parameters
   itself; the second entry is ignored.  */
void
ns_glitnir_params_changed (struct frame *f)
{
  static bool busy;
  Lisp_Object frame, v = Qnil;

  if (busy || !FRAME_NS_P (f) || !f->output_data.ns || FRAME_TOOLTIP_P (f))
    return;
  if (FRAME_PARENT_FRAME (f) && !NILP (Ffboundp (Qglitnir__chrome_vector)))
    {
      XSETFRAME (frame, f);
      busy = true;
      v = safe_calln (Qglitnir__chrome_vector, frame);
      busy = false;
    }

  float *out = f->output_data.ns->glitnir_chrome;
  memset (out, 0, NHG_CHROME_SLOTS * sizeof *out);
  if (VECTORP (v))
    for (ptrdiff_t i = 0; i < min (ASIZE (v), NHG_CHROME_SLOTS); i++)
      if (NUMBERP (AREF (v, i)))
	out[i] = XFLOATINT (AREF (v, i));
  ns_glitnir_apply (f);
}


/***********************************************************************
		     The pair hooks' main-thread time
 ***********************************************************************/

/* The time each pair hook spent on the main thread, the last
   NS_GLITNIR_SYNC_SAMPLES of them, in milliseconds (`glitnir-stats').  */
enum { NS_GLITNIR_SYNC_SAMPLES = 1024 };
static double ns_glitnir_sync_ms[NS_GLITNIR_SYNC_SAMPLES];
static uint64_t ns_glitnir_sync_count;

static uint64_t
ns_glitnir_now_ns (void)
{
  struct timespec ts;
  clock_gettime (CLOCK_MONOTONIC, &ts);
  return (uint64_t) ts.tv_sec * 1000000000 + ts.tv_nsec;
}

static void
ns_glitnir_note (uint64_t start_ns)
{
  ns_glitnir_sync_ms[ns_glitnir_sync_count++ % NS_GLITNIR_SYNC_SAMPLES]
    = (ns_glitnir_now_ns () - start_ns) / 1e6;
}

static int
ns_glitnir_cmp (const void *a, const void *b)
{
  double x = *(const double *) a, y = *(const double *) b;
  return (x > y) - (x < y);
}

/* (:count N :sync-ms (:p50 P :p95 P :max M)): the pair hooks counted since
   the last reset, and their times over the last samples.  RESET empties the
   record after reading it.  */
Lisp_Object
ns_glitnir_stats (bool reset)
{
  double v[NS_GLITNIR_SYNC_SAMPLES];
  size_t n = min (ns_glitnir_sync_count, NS_GLITNIR_SYNC_SAMPLES);
  double p50 = 0, p95 = 0, mx = 0;

  memcpy (v, ns_glitnir_sync_ms, n * sizeof *v);
  if (n > 0)
    {
      qsort (v, n, sizeof *v, ns_glitnir_cmp);
      p50 = v[(size_t) ceil (0.50 * n) - 1];
      p95 = v[(size_t) ceil (0.95 * n) - 1];
      mx = v[n - 1];
    }
  Lisp_Object result
    = list4 (intern_c_string (":count"), make_uint (ns_glitnir_sync_count),
	     intern_c_string (":sync-ms"),
	     listn (6, intern_c_string (":p50"), make_float (p50),
		    intern_c_string (":p95"), make_float (p95),
		    intern_c_string (":max"), make_float (mx)));
  if (reset)
    ns_glitnir_sync_count = 0;
  return result;
}


/***********************************************************************
			     Window cards
 ***********************************************************************/

/* A root frame's windows with a card (the window parameter
   `glitnir--card', which glitnir.el resolves from `corner-radius',
   `shadow' and `ring') are drawn by sublayers of the root's EmacsLayer,
   above what Emacs drew: per card, its four corners filled with the
   colour beneath (slot BENEATH_RGB), and its shadow and edge ring masked
   to the gap (the window area outside the windows, the corners of the
   cards in).  Every card layer is in one container layer, made with the
   first card and removed with the last.  The container keeps the key its
   layers were made from (the scale, the frame's size, each window's edges
   and card), so the layers change only when that changed: typing changes
   none.  */

#include "window.h"

/* How often a frame's card layers were made again or removed.  */
static intmax_t ns_glitnir_card_changes;

intmax_t
ns_glitnir_card_updates (void)
{
  return ns_glitnir_card_changes;
}

#ifdef NS_IMPL_COCOA

/* One window of the key: its edges without the dividers (points, the
   view's coordinates) and its card (all 0 for none).  No padding, so keys
   compare as bytes.  */
struct ns_glitnir_card_window
{
  int x, y, w, h;
  float card[NHG_CHROME_SLOTS];
};

/* Append WINDOW's leaf windows and its next siblings' to OUT (the root
   window's next sibling is the minibuffer window).  */
static void
ns_glitnir_card_windows (Lisp_Object window, NSMutableData *out)
{
  for (; !NILP (window); window = XWINDOW (window)->next)
    {
      struct window *w = XWINDOW (window);
      struct ns_glitnir_card_window e;

      if (WINDOWP (w->contents))
	{
	  ns_glitnir_card_windows (w->contents, out);
	  continue;
	}
      memset (&e, 0, sizeof e);
      e.x = WINDOW_LEFT_EDGE_X (w);
      e.y = WINDOW_TOP_EDGE_Y (w);
      e.w = WINDOW_PIXEL_WIDTH (w) - WINDOW_RIGHT_DIVIDER_WIDTH (w);
      e.h = WINDOW_PIXEL_HEIGHT (w) - WINDOW_BOTTOM_DIVIDER_WIDTH (w);
      Lisp_Object v = Fwindow_parameter (window, Qglitnir__card);
      if (VECTORP (v))
	for (ptrdiff_t i = 0; i < min (ASIZE (v), NHG_CHROME_SLOTS); i++)
	  if (NUMBERP (AREF (v, i)))
	    e.card[i] = XFLOATINT (AREF (v, i));
      [out appendBytes: &e length: sizeof e];
    }
}

/* Colour RGB (0xRRGGBB) at ALPHA, as Emacs makes its colours.  */
static CGColorRef
ns_glitnir_cg_color (float rgb, CGFloat alpha)
{
  unsigned long c = rgb > 0 && rgb <= 0xffffff ? (unsigned long) rgb : 0;
  return [[NSColor colorForEmacsRed: ((c >> 16) & 0xff) / 255.0
			      green: ((c >> 8) & 0xff) / 255.0
			       blue: (c & 0xff) / 255.0
			      alpha: alpha] CGColor];
}

/* Add rectangle R with corners of radius RADIUS (clamped to fit) to P.  */
static void
ns_glitnir_add_rounded (CGMutablePathRef p, CGRect r, CGFloat radius)
{
  radius = fmin (radius, fmin (CGRectGetWidth (r), CGRectGetHeight (r)) / 2);
  if (radius > 0)
    CGPathAddRoundedRect (p, NULL, r, radius, radius);
  else
    CGPathAddRect (p, NULL, r);
}

/* The card layers in HOST, or nil.  */
static CALayer *
ns_glitnir_cards_box (CALayer *host)
{
  for (CALayer *l in [host sublayers])
    if ([[l name] isEqualToString: @"glitnir-cards"])
      return l;
  return nil;
}

#endif /* NS_IMPL_COCOA */

/* After F was drawn (ns_update_end): bring the card layers of root frame
   F up to date.  Nothing to do while F has no card (its parameter
   `glitnir--cards' is nil) and no card layers.  */
void
ns_glitnir_cards_update (struct frame *f)
{
#ifdef NS_IMPL_COCOA
  if (FRAME_PARENT_FRAME (f) || !FRAME_NS_P (f) || !f->output_data.ns
      || !FRAME_NS_VIEW (f))
    return;
  EmacsView *view = FRAME_NS_VIEW (f);
  CALayer *host = [view layer];
  if (!host)
    return;
  CALayer *box = ns_glitnir_cards_box (host);
  bool want = (ns_glitnir_active_p (f)
	       && !NILP (get_frame_param (f, Qglitnir__cards)));
  if (!want && !box)
    return;

  @autoreleasepool
    {
      CGFloat scale = [[view window] backingScaleFactor];
      NSRect hb = [host bounds];
      NSMutableData *key = [NSMutableData data];
      CGFloat head[3] = { scale, NSWidth (hb), NSHeight (hb) };
      [key appendBytes: head length: sizeof head];
      ns_glitnir_card_windows (FRAME_ROOT_WINDOW (f), key);
      const struct ns_glitnir_card_window *ws
	= (const struct ns_glitnir_card_window *)
	    ((const char *) [key bytes] + sizeof head);
      ptrdiff_t n = ([key length] - sizeof head) / sizeof *ws;
      bool any = false;
      for (ptrdiff_t i = 0; i < n; i++)
	if (ws[i].card[NHG_CHROME_ON] != 0)
	  any = true;
      if (!want || !any)
	{
	  if (box)
	    {
	      [CATransaction begin];
	      [CATransaction setDisableActions: YES];
	      [box removeFromSuperlayer];
	      [CATransaction commit];
	      ns_glitnir_card_changes++;
	    }
	  return;
	}
      if (box && [[box valueForKey: @"glitnirKey"] isEqual: key])
	return;

      [CATransaction begin];
      [CATransaction setDisableActions: YES];
      if (!box)
	{
	  box = [CALayer layer];
	  [box setName: @"glitnir-cards"];
	  /* Above the layers of the view's subviews (the scroll bars).  */
	  [box setZPosition: 1];
	  [host addSublayer: box];
	}
      [box setFrame: NSRectToCGRect (hb)];
      [box setSublayers: nil];
      [box setValue: key forKey: @"glitnirKey"];

      /* The windows in the layer's coordinates, and which way is down.  */
      CGRect *rect = xmalloc (n * sizeof *rect);
      CGRect area = CGRectNull;
      for (ptrdiff_t i = 0; i < n; i++)
	{
	  NSRect r = [view convertRectToLayer:
			     NSMakeRect (ws[i].x, ws[i].y, ws[i].w, ws[i].h)];
	  rect[i] = CGRectOffset (NSRectToCGRect (r), -NSMinX (hb),
				  -NSMinY (hb));
	  if (!CGRectIsEmpty (rect[i]))
	    area = CGRectUnion (area, rect[i]);
	}
      CGFloat down = ([view convertPointToLayer: NSMakePoint (0, 1)].y
		      - [view convertPointToLayer: NSZeroPoint].y);
      CGFloat ring_w = fmax (1, round (scale)) / scale;
      NSMutableArray *chrome = [NSMutableArray array];

      for (ptrdiff_t i = 0; i < n; i++)
	{
	  const float *c = ws[i].card;
	  CGRect r = rect[i];
	  if (c[NHG_CHROME_ON] == 0 || CGRectIsEmpty (r))
	    continue;
	  CGFloat radius = fmin (c[NHG_CHROME_RADIUS],
				 fmin (CGRectGetWidth (r),
				       CGRectGetHeight (r)) / 2);

	  /* The corners: the rectangle less the rounded one.  */
	  if (radius > 0)
	    {
	      CAShapeLayer *corners = [CAShapeLayer layer];
	      CGMutablePathRef p = CGPathCreateMutable ();
	      CGPathAddRect (p, NULL, r);
	      CGPathAddRoundedRect (p, NULL, r, radius, radius);
	      [corners setFrame: [box bounds]];
	      [corners setPath: p];
	      [corners setFillRule: kCAFillRuleEvenOdd];
	      [corners setFillColor:
			 ns_glitnir_cg_color (c[NHG_CHROME_BENEATH_RGB], 1)];
	      CGPathRelease (p);
	      [box addSublayer: corners];
	    }

	  bool shadow = c[NHG_CHROME_OPACITY] > 0;
	  bool ring = (c[NHG_CHROME_RING_RGB] >= 0
		       && c[NHG_CHROME_RING_OPACITY] > 0);
	  if (!shadow && !ring)
	    continue;

	  /* The shadow and the ring, in the gap: the window area less the
	     windows, the cards' corners left in.  */
	  CALayer *layer = [CALayer layer];
	  [layer setFrame: [box bounds]];
	  if (shadow)
	    {
	      CGFloat spread = c[NHG_CHROME_SPREAD];
	      CGRect s = CGRectInset (r, -spread, -spread);
	      if (!CGRectIsEmpty (s))
		{
		  CGMutablePathRef sp = CGPathCreateMutable ();
		  ns_glitnir_add_rounded (sp, s, fmax (0, radius + spread));
		  [layer setShadowPath: sp];
		  CGPathRelease (sp);
		  [layer setShadowColor:
			   ns_glitnir_cg_color (c[NHG_CHROME_RGB], 1)];
		  [layer setShadowOpacity: c[NHG_CHROME_OPACITY]];
		  [layer setShadowRadius: c[NHG_CHROME_BLUR] / 2];
		  [layer setShadowOffset:
			   CGSizeMake (c[NHG_CHROME_OX],
				       c[NHG_CHROME_OY] * down)];
		}
	    }
	  if (ring)
	    {
	      /* One point just outside the card, whole device pixels.  */
	      CAShapeLayer *line = [CAShapeLayer layer];
	      CGMutablePathRef rp = CGPathCreateMutable ();
	      ns_glitnir_add_rounded (rp, CGRectInset (r, -ring_w / 2,
						       -ring_w / 2),
				      radius > 0 ? radius + ring_w / 2 : 0);
	      [line setFrame: [layer bounds]];
	      [line setPath: rp];
	      [line setFillColor: NULL];
	      [line setLineWidth: ring_w];
	      [line setStrokeColor:
		      ns_glitnir_cg_color (c[NHG_CHROME_RING_RGB],
					   c[NHG_CHROME_RING_OPACITY])];
	      CGPathRelease (rp);
	      [layer addSublayer: line];
	    }
	  CAShapeLayer *mask = [CAShapeLayer layer];
	  CGMutablePathRef mp = CGPathCreateMutable ();
	  CGPathAddRect (mp, NULL, area);
	  for (ptrdiff_t j = 0; j < n; j++)
	    if (!CGRectIsEmpty (rect[j]))
	      ns_glitnir_add_rounded (mp, rect[j],
				      (ws[j].card[NHG_CHROME_ON] != 0
				       ? ws[j].card[NHG_CHROME_RADIUS] : 0));
	  [mask setFrame: [layer bounds]];
	  [mask setPath: mp];
	  [mask setFillRule: kCAFillRuleEvenOdd];
	  CGPathRelease (mp);
	  [layer setMask: mask];
	  [layer setShouldRasterize: YES];
	  [layer setRasterizationScale: scale];
	  [chrome addObject: layer];
	}
      /* Every card's chrome over every card's corners, as Glitnir
	 composes a corner: the colour beneath, then the cards' shadows
	 and rings.  */
      for (CALayer *l in chrome)
	[box addSublayer: l];
      xfree (rect);
      [CATransaction commit];
      ns_glitnir_card_changes++;
    }
#endif
}


/***********************************************************************
		  Translucent background and backdrop blur
 ***********************************************************************/

/* Stock NS draws no `alpha-background'.  A chromed child frame whose
   `alpha-background' is translucent gets it here: its window is clear, as
   for corners, and Emacs's background fills (nsterm.m, the fills
   nhwldraw.c draws with the alpha) take their colour at that alpha, so
   what lies behind shows through.  The text and every other foreground
   stay opaque: a text pixel the glyph covers is the stock pixel.  With
   `backdrop-blur' too (slot BACKDROP, which glitnir.el sets only with a
   translucent background), an NSVisualEffectView in behindWindow mode
   below the EmacsView blurs what lies behind; the blur is the system's
   Popover material, so the slot's radius only turns it on.  When the
   platform asks for less transparency (Reduce transparency), such a
   frame is opaque: no effect view and the stock fills.  */

/* The colour of a background fill of F: COLOR, or COLOR at F's
   `alpha-background' while F's background is see-through.  RESPECT false
   (an internal border without `borders-respect-alpha-background') keeps
   COLOR.  */
unsigned long
ns_glitnir_bg (struct frame *f, unsigned long color, bool respect)
{
  if (!respect || !ns_glitnir_see_through_p (f))
    return color;
  double alpha = ((color >> 24) & 0xff) * f->alpha_background;
  return (color & 0xffffff) | ((unsigned long) lround (alpha) << 24);
}

/* True while F's background fills are see-through.  */
bool
ns_glitnir_see_through_p (struct frame *f)
{
  return (glitnir_enable && f->output_data.ns
	  && f->output_data.ns->glitnir_see_through);
}

#ifdef NS_IMPL_COCOA

/* True when the platform asks for less transparency, or
   `glitnir--ns-reduce-transparency' (for tests) is `on'; `off' is
   false.  */
static bool
ns_glitnir_reduce_transparency_p (void)
{
  Lisp_Object v = Vglitnir__ns_reduce_transparency;

  if (EQ (v, intern_c_string ("on")))
    return true;
  if (EQ (v, intern_c_string ("off")))
    return false;
  return [[NSWorkspace sharedWorkspace]
	   accessibilityDisplayShouldReduceTransparency];
}

/* The blur's effect view in CONTENT (a frame's window's content view),
   or nil.  It is the content view's, so it goes with the window.  */
static NSVisualEffectView *
ns_glitnir_effect_view (NSView *content)
{
  for (NSView *v in [content subviews])
    if ([v isKindOfClass: [NSVisualEffectView class]]
	&& [[v identifier] isEqualToString: @"glitnir-blur"])
      return (NSVisualEffectView *) v;
  return nil;
}

/* The effect view's mask: a rounded square of radius R (points) that
   stretches between its corners, so a resize needs no new image.  Its
   pixels are drawn at twice its size, for a 2x screen.  */
static NSImage *
ns_glitnir_blur_mask (CGFloat r)
{
  CGFloat side = 2 * r + 1;
  size_t px = (size_t) ceil (2 * side);
  CGColorSpaceRef cs = CGColorSpaceCreateWithName (kCGColorSpaceSRGB);
  CGContextRef ctx = CGBitmapContextCreate (NULL, px, px, 8, 0, cs,
					    kCGImageAlphaPremultipliedLast);
  NSImage *m = nil;

  CGColorSpaceRelease (cs);
  if (!ctx)
    return nil;
  CGContextScaleCTM (ctx, px / side, px / side);
  CGPathRef p = CGPathCreateWithRoundedRect (CGRectMake (0, 0, side, side),
					     r, r, NULL);
  CGContextAddPath (ctx, p);
  CGContextSetRGBFillColor (ctx, 0, 0, 0, 1);
  CGContextFillPath (ctx);
  CGPathRelease (p);
  CGImageRef img = CGBitmapContextCreateImage (ctx);
  CGContextRelease (ctx);
  if (img)
    {
      m = [[[NSImage alloc] initWithCGImage: img
				       size: NSMakeSize (side, side)]
	    autorelease];
      CGImageRelease (img);
      [m setCapInsets: NSEdgeInsetsMake (r, r, r, r)];
      [m setResizingMode: NSImageResizingModeStretch];
    }
  return m;
}

/* Apply F's translucent background and blur to its window W: ON when
   F's chrome is on, R its corner radius (points).  Return whether F's
   background is see-through; the window must then be clear.  F is drawn
   again when that changed.  */
static bool
ns_glitnir_backdrop (struct frame *f, NSWindow *w, bool on, CGFloat r)
{
  struct ns_output *o = f->output_data.ns;
  NSView *content = [w contentView];
  NSVisualEffectView *v = ns_glitnir_effect_view (content);
  bool see = on && f->alpha_background < 1;
  bool blur = see && o->glitnir_chrome[NHG_CHROME_BACKDROP] > 0;

  if (blur && ns_glitnir_reduce_transparency_p ())
    see = blur = false;
  if (!blur)
    [v removeFromSuperview];
  else
    {
      NSRect bounds = [content bounds];
      CGFloat k = fmax (0, fmin (r, fmin (NSWidth (bounds),
					  NSHeight (bounds)) / 2));
      if (!v)
	{
	  v = [[[NSVisualEffectView alloc] initWithFrame: bounds]
		autorelease];
	  [v setIdentifier: @"glitnir-blur"];
	  [v setBlendingMode: NSVisualEffectBlendingModeBehindWindow];
	  [v setMaterial: NSVisualEffectMaterialPopover];
	  [v setState: NSVisualEffectStateActive];
	  [v setAutoresizingMask: NSViewWidthSizable | NSViewHeightSizable];
	  [content addSubview: v positioned: NSWindowBelow relativeTo: nil];
	}
      [v setFrame: bounds];
      [v setMaskImage: k > 0 ? ns_glitnir_blur_mask (k) : nil];
    }
  if (see != o->glitnir_see_through)
    {
      o->glitnir_see_through = see;
      SET_FRAME_GARBAGED (f);
    }
  return see;
}

#endif /* NS_IMPL_COCOA */


#ifdef NS_IMPL_COCOA

/***********************************************************************
			Window-server coordinates
 ***********************************************************************/

/* Cocoa screen coordinates (origin at the main screen's bottom left) to
   CoreGraphics' global ones (origin at its top left).  */
static CGRect
ns_glitnir_cg_rect (NSRect r)
{
  CGFloat h = NSHeight ([[[NSScreen screens] objectAtIndex: 0] frame]);
  return CGRectMake (r.origin.x, h - r.origin.y - r.size.height,
		     r.size.width, r.size.height);
}

/* F's content (the EmacsView: no title bar, no toolbar) in screen
   coordinates.  */
static NSRect
ns_glitnir_content_rect (struct frame *f)
{
  NSView *view = (NSView *) FRAME_NS_VIEW (f);
  NSRect r = [view convertRect: [view bounds] toView: nil];
  return [[view window] convertRectToScreen: r];
}


/***********************************************************************
		       The shadow window and the pair
 ***********************************************************************/

/* A child frame's shadow window: a clear, click-through window that only
   draws the shadow of the frame's child window.  It is a sibling, a child
   window of the same parent, ordered directly below its child window.  The
   pair is kept by EmacsWindow (nsterm.m): the parent's -addChildWindow:
   attaches the shadow window first, its -removeChildWindow: detaches it,
   and the child's -orderOut:, frame, alpha and level setters bring it
   along.  Those overrides ignore a shadow window passed to them, so the
   re-stacking loops that walk -childWindows leave it to its child.  */
@interface GlitnirShadowWindow : NSWindow
{
@public
  /* The child frame, or NULL once it let the shadow go.  */
  struct frame *owner;
  /* Draws the shadow from shadowPath, masked to the ring outside the
     child window (so a translucent child does not show its own shadow).  */
  CALayer *shadow;
  CAShapeLayer *mask;
  /* In points: the room around the child window, the corner radius and
     the shadow's spread.  */
  CGFloat margin, radius, spread;
}
@end

@implementation GlitnirShadowWindow

- (instancetype) initForFrame: (struct frame *) f
{
  self = [super initWithContentRect: NSMakeRect (0, 0, 1, 1)
			  styleMask: NSWindowStyleMaskBorderless
			    backing: NSBackingStoreBuffered
			      defer: NO];
  if (!self)
    return nil;
  owner = f;
  [self setReleasedWhenClosed: NO];
  [self setOpaque: NO];
  [self setBackgroundColor: [NSColor clearColor]];
  [self setHasShadow: NO];
  [self setIgnoresMouseEvents: YES];
  [self setExcludedFromWindowsMenu: YES];
  [self setCollectionBehavior: (NSWindowCollectionBehaviorTransient
				| NSWindowCollectionBehaviorIgnoresCycle)];
  [self setAnimationBehavior: NSWindowAnimationBehaviorNone];
  [self setAccessibilityElement: NO];
  [self setTitle: @"glitnir-shadow"];

  /* Layer-hosting: the content view's layer is ours.  */
  NSView *view = [self contentView];
  [view setLayer: [CALayer layer]];
  [view setWantsLayer: YES];
  shadow = [[CALayer alloc] init];
  mask = [[CAShapeLayer alloc] init];
  [mask setFillRule: kCAFillRuleEvenOdd];
  [shadow setMask: mask];
  [[view layer] addSublayer: shadow];
  return self;
}

- (void) dealloc
{
  [shadow release];
  [mask release];
  [super dealloc];
}

- (BOOL) canBecomeKeyWindow
{
  return NO;
}

- (BOOL) canBecomeMainWindow
{
  return NO;
}

/* Where the shadow window belongs: its child window's frame grown by the
   margin; or R when it has no child window.  */
- (NSRect) placeFor: (NSRect) r
{
  NSWindow *child = (owner && FRAME_NS_VIEW (owner)
		     ? [(NSView *) FRAME_NS_VIEW (owner) window] : nil);
  return child ? NSInsetRect ([child frame], -margin, -margin) : r;
}

/* Every move goes where the child window is, whoever asks: when AppKit
   carries both windows of the pair with their parent, the shadow window
   ends up at its child's place in whichever order they move.  */
- (void) setFrame: (NSRect) r display: (BOOL) display
{
  [super setFrame: [self placeFor: r] display: display];
}

- (void) setFrameOrigin: (NSPoint) p
{
  NSRect r = { p, [self frame].size };
  [super setFrameOrigin: [self placeFor: r].origin];
}

@end

/* The live frame whose window W is now, or NULL.  A window that a frame
   replaced (`ns_set_undecorated') still has the frame's view as its
   delegate, but the view is in the new window.  */
static struct frame *
ns_glitnir_window_frame (NSWindow *w)
{
  id view = [w delegate];
  struct frame *f;

  if (![view isKindOfClass: [EmacsView class]])
    return NULL;
  f = ((EmacsView *) view)->emacsframe;
  if (!f || !FRAME_LIVE_P (f) || !FRAME_NS_P (f) || !f->output_data.ns
      || FRAME_NS_VIEW (f) != view || [view window] != w)
    return NULL;
  return f;
}

static GlitnirShadowWindow *
ns_glitnir_shadow_of (NSWindow *child)
{
  struct frame *f = ns_glitnir_window_frame (child);
  return f ? (GlitnirShadowWindow *) f->output_data.ns->glitnir_shadow : nil;
}

bool
ns_glitnir_shadow_p (NSWindow *w)
{
  return [w isKindOfClass: [GlitnirShadowWindow class]];
}

/* The corner radius CG accepts for a rectangle of SIZE.  */
static CGFloat
ns_glitnir_clamp_radius (CGFloat r, CGSize size)
{
  return max (0, min (r, min (size.width, size.height) / 2));
}

/* Take the shadow's look from the chrome vector C.  */
static void
ns_glitnir_shadow_spec (GlitnirShadowWindow *s, const float *c)
{
  CGFloat blur = max (0, c[NHG_CHROME_BLUR]);
  CGFloat ox = c[NHG_CHROME_OX], oy = c[NHG_CHROME_OY];
  uint32_t rgb = (c[NHG_CHROME_RGB] > 0 && c[NHG_CHROME_RGB] <= 0xffffff
		  ? (uint32_t) c[NHG_CHROME_RGB] : 0);

  s->radius = max (0, c[NHG_CHROME_RADIUS]);
  s->spread = c[NHG_CHROME_SPREAD];
  s->margin = max (0, ceil (1.5 * blur + fabs (ox) + fabs (oy) + s->spread));

  NSColor *color = [NSColor colorWithSRGBRed: ((rgb >> 16) & 0xff) / 255.0
				       green: ((rgb >> 8) & 0xff) / 255.0
					blue: (rgb & 0xff) / 255.0
				       alpha: 1];
  [s->shadow setShadowColor: [color CGColor]];
  [s->shadow setShadowOpacity: c[NHG_CHROME_OPACITY]];
  /* CSS's blur is twice the Gaussian's deviation; CA's y is up.  */
  [s->shadow setShadowRadius: blur / 2];
  [s->shadow setShadowOffset: CGSizeMake (ox, -oy)];
}

/* Put shadow window S at CHILD's frame grown by the margin, with CHILD's
   alpha and level, and its shadow and mask paths at CHILD's size.  */
static void
ns_glitnir_fit (GlitnirShadowWindow *s, NSWindow *child)
{
  NSRect cf = [child frame];
  CGFloat m = s->margin;
  NSRect sf = NSInsetRect (cf, -m, -m);
  CGRect bounds = CGRectMake (0, 0, NSWidth (sf), NSHeight (sf));
  CGRect inner = CGRectMake (m, m, NSWidth (cf), NSHeight (cf));
  CGRect grown = CGRectInset (inner, -s->spread, -s->spread);
  CGFloat r = ns_glitnir_clamp_radius (s->radius, inner.size);
  CGFloat gr = (s->radius > 0
		? ns_glitnir_clamp_radius (s->radius + s->spread, grown.size)
		: 0);
  CGFloat scale = [s backingScaleFactor];

  [CATransaction begin];
  [CATransaction setDisableActions: YES];
  [s setFrame: sf display: NO];
  [s->shadow setFrame: bounds];
  [s->shadow setContentsScale: scale];
  if (grown.size.width > 0 && grown.size.height > 0)
    {
      CGPathRef path = CGPathCreateWithRoundedRect (grown, gr, gr, NULL);
      [s->shadow setShadowPath: path];
      CGPathRelease (path);
    }
  else
    [s->shadow setShadowPath: NULL];
  [s->mask setFrame: bounds];
  [s->mask setContentsScale: scale];
  CGMutablePathRef ring = CGPathCreateMutable ();
  CGPathAddRect (ring, NULL, bounds);
  CGPathAddRoundedRect (ring, NULL, inner, r, r);
  [s->mask setPath: ring];
  CGPathRelease (ring);
  if ([s alphaValue] != [child alphaValue])
    [s setAlphaValue: [child alphaValue]];
  [CATransaction commit];
}

static NSInteger
ns_glitnir_back_to_front (id w1, id w2, void *context)
{
  NSInteger i1 = [w1 orderedIndex], i2 = [w2 orderedIndex];
  return i1 > i2 ? NSOrderedAscending : i1 < i2 ? NSOrderedDescending
    : NSOrderedSame;
}

/* Attach CHILD's shadow window directly below CHILD again: re-attach CHILD
   and each sibling above it, bottom to top.  The parent's -addChildWindow:
   brings each one's shadow window, and the siblings keep their order.  */
static void
ns_glitnir_reattach (NSWindow *child)
{
  NSWindow *parent = [child parentWindow];
  bool from = false;

  if (!parent)
    return;
  for (NSWindow *w in [[parent childWindows]
			sortedArrayUsingFunction: ns_glitnir_back_to_front
					 context: nil])
    {
      if (w == child)
	from = true;
      if (from && !ns_glitnir_shadow_p (w))
	{
	  [parent removeChildWindow: w];
	  [parent addChildWindow: w ordered: NSWindowAbove];
	}
    }
}

/* Detach shadow window S from the window it is a child of.  */
static void
ns_glitnir_detach (GlitnirShadowWindow *s)
{
  NSWindow *parent = [s parentWindow];

  if ([parent isKindOfClass: [EmacsWindow class]])
    [(EmacsWindow *) parent glitnirDetach: s];
  else if (parent)
    [parent removeChildWindow: s];
}

/* F's shadow window goes: detached, ordered out and closed.  */
static void
ns_glitnir_drop_shadow (struct frame *f)
{
  GlitnirShadowWindow *s = f->output_data.ns->glitnir_shadow;

  if (!s)
    return;
  f->output_data.ns->glitnir_shadow = NULL;
  s->owner = NULL;
  ns_glitnir_detach (s);
  [s orderOut: nil];
  [s close];
  [s release];
}

/* The parent's -addChildWindow: hook: CHILD's shadow window, detached
   from any parent and fitted to CHILD, for the parent to attach right
   before CHILD; or nil.  */
NSWindow *
ns_glitnir_shadow_to_attach (NSWindow *child)
{
  GlitnirShadowWindow *s = ns_glitnir_shadow_of (child);
  uint64_t t0;

  if (!s)
    return nil;
  t0 = ns_glitnir_now_ns ();
  ns_glitnir_detach (s);
  [s setLevel: [child level]];
  ns_glitnir_fit (s, child);
  ns_glitnir_note (t0);
  return s;
}

/* The parent's -removeChildWindow: hook: CHILD's shadow window is
   detached with it.  */
void
ns_glitnir_child_removed (NSWindow *child)
{
  GlitnirShadowWindow *s = ns_glitnir_shadow_of (child);
  uint64_t t0;

  if (!s)
    return;
  t0 = ns_glitnir_now_ns ();
  ns_glitnir_detach (s);
  ns_glitnir_note (t0);
}

/* The child's -orderOut: hook: its shadow window is detached and ordered
   out first, so that nothing shows it again without its child.  */
void
ns_glitnir_child_hidden (NSWindow *child)
{
  GlitnirShadowWindow *s = ns_glitnir_shadow_of (child);
  uint64_t t0;

  if (!s)
    return;
  t0 = ns_glitnir_now_ns ();
  ns_glitnir_detach (s);
  [s orderOut: nil];
  ns_glitnir_note (t0);
}

/* The child's frame and alpha setters' hook (and the scale's): the shadow
   window follows CHILD.  */
void
ns_glitnir_follow (NSWindow *child)
{
  GlitnirShadowWindow *s = ns_glitnir_shadow_of (child);
  uint64_t t0;

  if (!s)
    return;
  t0 = ns_glitnir_now_ns ();
  ns_glitnir_fit (s, child);
  ns_glitnir_note (t0);
}

/* The child's level setter's hook, before CHILD takes LEVEL: the shadow
   window takes it first, so that CHILD, going to its new level after it,
   is above it there.  */
void
ns_glitnir_level (NSWindow *child, NSInteger level)
{
  GlitnirShadowWindow *s = ns_glitnir_shadow_of (child);
  uint64_t t0;

  if (!s || [s level] == level)
    return;
  t0 = ns_glitnir_now_ns ();
  [s setLevel: level];
  ns_glitnir_note (t0);
}

/* The destroy hook (ns_destroy_window, before the window closes): F's
   shadow window goes.  */
void
ns_glitnir_frame_destroyed (struct frame *f)
{
  if (!FRAME_NS_P (f) || !f->output_data.ns)
    return;
  block_input ();
  ns_glitnir_drop_shadow (f);
  f->output_data.ns->glitnir_rounded = NULL;
  unblock_input ();
}

/* Round window W's content to radius R (points): a clear window whose
   content layer masks to the rounded rectangle, with a circular curve as
   on Linux.  */
static void
ns_glitnir_round (NSWindow *w, CGFloat r)
{
  NSView *content = [w contentView];
  CALayer *layer;

  [w setOpaque: NO];
  [w setBackgroundColor: [NSColor clearColor]];
  [content setWantsLayer: YES];
  /* AppKit drives masksToBounds from clipsToBounds on macOS 14 and later.  */
  if (@available (macOS 14, *))
    [content setClipsToBounds: YES];
  layer = [content layer];
  [layer setCornerRadius: r];
  [layer setMasksToBounds: YES];
  if (@available (macOS 10.15, *))
    [layer setCornerCurve: kCACornerCurveCircular];
  [w invalidateShadow];
}

/* Give F's window W back the stock values ns_glitnir_round changed.  */
static void
ns_glitnir_unround (struct frame *f, NSWindow *w)
{
  NSView *content = [w contentView];
  NSColor *bg = f->output_data.ns->background_color;
  CALayer *layer = [content layer];

  [layer setCornerRadius: 0];
  [layer setMasksToBounds: NO];
  if (@available (macOS 14, *))
    [content setClipsToBounds: NO];
  if (bg)
    {
      [w setBackgroundColor: bg];
      [w setOpaque: [bg alphaComponent] == (EmacsCGFloat) 1.0];
    }
  [w invalidateShadow];
}

/* Apply F's chrome vector to its window: the corners, and the shadow
   window, made, changed or dropped.  With Glitnir off for F, or no chrome,
   the window gets back what was changed and nothing new is made.  */
static void
ns_glitnir_apply (struct frame *f)
{
  struct ns_output *o = f->output_data.ns;
  NSWindow *w = [(NSView *) FRAME_NS_VIEW (f) window];
  const float *c = o->glitnir_chrome;

  if (!w)
    return;

  bool on = (ns_glitnir_active_p (f) && FRAME_PARENT_FRAME (f)
	     && c[NHG_CHROME_ON] != 0);
  CGFloat r = on ? max (0, c[NHG_CHROME_RADIUS]) : 0;
  bool shadow = on && c[NHG_CHROME_OPACITY] > 0;

  /* Off, and nothing to undo: a root frame, or a child frame without
     chrome, is not touched at all.  */
  if (!on && !o->glitnir_shadow && !o->glitnir_rounded)
    return;

  block_input ();
  @autoreleasepool
    {
      [CATransaction begin];
      [CATransaction setDisableActions: YES];
      /* A see-through background needs the clear window too.  */
      bool see = ns_glitnir_backdrop (f, w, on, r);
      if (r > 0 || see)
	{
	  ns_glitnir_round (w, r);
	  o->glitnir_rounded = w;
	}
      else
	{
	  /* A window the frame replaced is closed; the new one is stock.  */
	  if (o->glitnir_rounded == w)
	    ns_glitnir_unround (f, w);
	  o->glitnir_rounded = NULL;
	}

      GlitnirShadowWindow *s = o->glitnir_shadow;
      if (!shadow)
	ns_glitnir_drop_shadow (f);
      else
	{
	  if (!s)
	    {
	      s = [[GlitnirShadowWindow alloc] initForFrame: f];
	      o->glitnir_shadow = s;
	    }
	  ns_glitnir_shadow_spec (s, c);
	  [s setLevel: [w level]];
	  ns_glitnir_fit (s, w);
	  /* A shown child window gets its shadow window below it now; a
	     hidden one when it is shown (the parent's -addChildWindow:).  */
	  if ([w parentWindow] && [w isVisible]
	      && [s parentWindow] != [w parentWindow])
	    ns_glitnir_reattach (w);
	}
      [CATransaction commit];
    }
  unblock_input ();
}

/* Copy what F's window shows of its chrome to OUT, a chrome vector
   (NHG_CHROME_SLOTS floats, logical pixels), read back from its layers,
   and return true; or return false when Glitnir is off for F or the
   window shows no chrome.  */
bool
ns_glitnir_chrome (struct frame *f, float *out)
{
  if (!ns_glitnir_active_p (f))
    return false;

  struct ns_output *o = f->output_data.ns;
  NSWindow *w = [(NSView *) FRAME_NS_VIEW (f) window];
  GlitnirShadowWindow *s = o->glitnir_shadow;
  bool rounded = w && o->glitnir_rounded == w;

  if (!rounded && !s)
    return false;
  memset (out, 0, NHG_CHROME_SLOTS * sizeof *out);
  out[NHG_CHROME_ON] = 1;
  if (rounded)
    out[NHG_CHROME_RADIUS] = [[[w contentView] layer] cornerRadius];
  if (s)
    {
      CALayer *l = s->shadow;
      CGFloat inner = NSWidth ([s frame]) - 2 * s->margin;
      CGPathRef path = [l shadowPath];
      CGColorRef color = [l shadowColor];

      out[NHG_CHROME_BLUR] = 2 * [l shadowRadius];
      out[NHG_CHROME_OX] = [l shadowOffset].width;
      out[NHG_CHROME_OY] = -[l shadowOffset].height;
      if (path)
	out[NHG_CHROME_SPREAD]
	  = (CGRectGetWidth (CGPathGetPathBoundingBox (path)) - inner) / 2;
      out[NHG_CHROME_OPACITY] = [l shadowOpacity];
      if (color && CGColorGetNumberOfComponents (color) >= 3)
	{
	  const CGFloat *k = CGColorGetComponents (color);
	  out[NHG_CHROME_RGB] = (((uint32_t) lround (k[0] * 255) << 16)
				 | ((uint32_t) lround (k[1] * 255) << 8)
				 | (uint32_t) lround (k[2] * 255));
	}
    }
  return true;
}


/***********************************************************************
				Capture
 ***********************************************************************/

/* CGWindowListCreateImage is obsolete in the macOS 15 SDK but still in the
   library, so it is looked up at run time.  The last argument is the
   CGWindowImageOption: best resolution (device pixels).  */
typedef CGImageRef (*ns_glitnir_capture_fn) (CGRect, CGWindowListOption,
					     CGWindowID, uint32_t);
enum { NS_GLITNIR_IMAGE_BEST_RESOLUTION = 1 << 3 };

/* False when the process may not capture the screen (Screen Recording).
   This never asks: CGRequestScreenCaptureAccess would show a prompt.  */
static bool
ns_glitnir_may_capture (void)
{
  typedef bool (*preflight_fn) (void);
  preflight_fn preflight
    = (preflight_fn) dlsym (RTLD_DEFAULT, "CGPreflightScreenCaptureAccess");
  return !preflight || preflight ();
}

/* The picture of F's root's content rectangle as the window server shows it:
   the root's window and every window above it (child frames and their
   shadows), in device pixels.  Raw: (WIDTH HEIGHT BYTES), BYTES ARGB32 in the
   machine's byte order, premultiplied, rows tightly packed; else a PNG string.
   Signals `glitnir-no-capture' when the process may not capture.  */
Lisp_Object
ns_glitnir_snapshot (struct frame *f, bool png)
{
  struct frame *r = root_frame (f);
  Lisp_Object result = Qnil;

  if (!FRAME_NS_P (r) || !r->output_data.ns || !FRAME_NS_VIEW (r))
    error ("glitnir-snapshot: no window");
  if (!ns_glitnir_may_capture ())
    xsignal1 (Qglitnir_no_capture,
	      build_string ("Screen Recording is not allowed for this process"));
  ns_glitnir_capture_fn capture
    = (ns_glitnir_capture_fn) dlsym (RTLD_DEFAULT, "CGWindowListCreateImage");
  if (!capture)
    xsignal1 (Qglitnir_no_capture,
	      build_string ("CGWindowListCreateImage is not available"));

  bool failed = false;
  block_input ();
  @autoreleasepool
    {
      NSWindow *window = [(NSView *) FRAME_NS_VIEW (r) window];
      [CATransaction flush];
      CGImageRef img
	= capture (ns_glitnir_cg_rect (ns_glitnir_content_rect (r)),
		   kCGWindowListOptionOnScreenAboveWindow
		   | kCGWindowListOptionIncludingWindow,
		   (CGWindowID) [window windowNumber],
		   NS_GLITNIR_IMAGE_BEST_RESOLUTION);
      if (!img)
	failed = true;
      else
	{
	  size_t w = CGImageGetWidth (img), h = CGImageGetHeight (img);
	  CGColorSpaceRef cs = CGColorSpaceCreateWithName (kCGColorSpaceSRGB);
	  CGContextRef ctx
	    = CGBitmapContextCreate (NULL, w, h, 8, w * 4, cs,
				     kCGImageAlphaPremultipliedFirst
				     | kCGBitmapByteOrder32Little);
	  CGColorSpaceRelease (cs);
	  if (!ctx)
	    failed = true;
	  else
	    {
	      CGContextSetBlendMode (ctx, kCGBlendModeCopy);
	      CGContextDrawImage (ctx, CGRectMake (0, 0, w, h), img);
	      if (png)
		{
		  CGImageRef out = CGBitmapContextCreateImage (ctx);
		  NSBitmapImageRep *rep
		    = [[[NSBitmapImageRep alloc] initWithCGImage: out]
			autorelease];
		  NSData *data
		    = [rep representationUsingType: NSBitmapImageFileTypePNG
					properties: @{}];
		  if (data)
		    result = make_unibyte_string ([data bytes], [data length]);
		  else
		    failed = true;
		  CGImageRelease (out);
		}
	      else
		result = list3 (make_fixnum (w), make_fixnum (h),
				make_unibyte_string (CGBitmapContextGetData (ctx),
						     w * 4 * h));
	      CGContextRelease (ctx);
	    }
	  CGImageRelease (img);
	}
    }
  unblock_input ();
  if (failed)
    error ("glitnir-snapshot: the capture failed");
  return result;
}


/***********************************************************************
		   The window server's view of the children
 ***********************************************************************/

/* One child frame of the root in the window server's list: its window's
   and its shadow window's place in the list (front to back, -1 when not on
   screen), bounds and alpha.  */
struct ns_glitnir_pair_entry
{
  struct frame *frame;		/* NULL: a shadow window without a frame */
  CFIndex child_at, shadow_at;
  CGRect child, shadow;
  double child_alpha, shadow_alpha;
};

static Lisp_Object
ns_glitnir_bounds (CGRect b, CGRect origin)
{
  return list4 (make_float (b.origin.x - origin.origin.x),
		make_float (b.origin.y - origin.origin.y),
		make_float (b.size.width), make_float (b.size.height));
}

/* The entry of E (N entries) for frame C, made when there is none and room
   is left; NULL else.  A shadow window without a frame (C NULL) gets an
   entry of its own.  */
static struct ns_glitnir_pair_entry *
ns_glitnir_pair_entry (struct ns_glitnir_pair_entry *e, int *n, int size,
		       struct frame *c)
{
  for (int i = 0; c && i < *n; i++)
    if (e[i].frame == c)
      return &e[i];
  if (*n == size)
    return NULL;
  e[*n] = (struct ns_glitnir_pair_entry) { c, -1, -1 };
  return &e[(*n)++];
}

/* The child frames of F's root as the window server lists them, bottom to
   top: (:frame F :child (X Y W H) :alpha A :shadow (X Y W H) :shadow-alpha
   A :below BOOL), bounds in points from the root content's top left.  A
   child frame is listed while its window or its shadow window is on
   screen; the one not on screen has nil bounds and alpha.  :below is
   non-nil when the shadow window is directly below the child's in the
   window server's order.  A shadow window on screen whose frame let it go
   is listed with :frame nil.  Needs no permission: only the window list,
   not pictures.  */
Lisp_Object
ns_glitnir_pairs (struct frame *f)
{
  struct frame *r = root_frame (f);
  Lisp_Object result = Qnil;
  struct ns_glitnir_pair_entry e[64];
  int n = 0;

  if (!FRAME_NS_P (r) || !r->output_data.ns || !FRAME_NS_VIEW (r))
    return Qnil;

  block_input ();
  CGRect origin = ns_glitnir_cg_rect (ns_glitnir_content_rect (r));
  CFArrayRef list
    = CGWindowListCopyWindowInfo (kCGWindowListOptionOnScreenOnly,
				  kCGNullWindowID);
  if (list)
    {
      for (CFIndex i = 0; i < CFArrayGetCount (list); i++)
	{
	  NSDictionary *info = (NSDictionary *) CFArrayGetValueAtIndex (list, i);
	  CGWindowID number
	    = [[info objectForKey: (NSString *) kCGWindowNumber] unsignedIntValue];
	  NSWindow *w = [NSApp windowWithWindowNumber: number];
	  struct ns_glitnir_pair_entry *p = NULL;
	  bool is_shadow = w && ns_glitnir_shadow_p (w);
	  struct frame *c = (is_shadow ? ((GlitnirShadowWindow *) w)->owner
			     : w ? ns_glitnir_window_frame (w) : NULL);
	  CGRect b;

	  if (c && (!FRAME_PARENT_FRAME (c) || root_frame (c) != r))
	    continue;
	  if (!c && !is_shadow)
	    continue;
	  if (!CGRectMakeWithDictionaryRepresentation
	      ((CFDictionaryRef) [info objectForKey:
					 (NSString *) kCGWindowBounds], &b))
	    continue;
	  p = ns_glitnir_pair_entry (e, &n, ARRAYELTS (e), c);
	  if (!p)
	    break;
	  double alpha
	    = [[info objectForKey: (NSString *) kCGWindowAlpha] doubleValue];
	  if (is_shadow)
	    p->shadow_at = i, p->shadow = b, p->shadow_alpha = alpha;
	  else
	    p->child_at = i, p->child = b, p->child_alpha = alpha;
	}
      CFRelease (list);
    }

  /* The list runs front to back: consing each entry, in that order, on the
     front leaves the bottom one first.  */
  for (int i = n - 1; i >= 0; i--)
    for (int j = 0; j < i; j++)
      {
	CFIndex a = e[j].child_at >= 0 ? e[j].child_at : e[j].shadow_at;
	CFIndex b = e[j + 1].child_at >= 0 ? e[j + 1].child_at : e[j + 1].shadow_at;
	if (a > b)
	  {
	    struct ns_glitnir_pair_entry t = e[j];
	    e[j] = e[j + 1], e[j + 1] = t;
	  }
      }
  for (int i = 0; i < n; i++)
    {
      Lisp_Object frame = Qnil;
      bool child = e[i].child_at >= 0, shadow = e[i].shadow_at >= 0;

      if (e[i].frame)
	XSETFRAME (frame, e[i].frame);
      result
	= Fcons (list (intern_c_string (":frame"), frame,
		       intern_c_string (":child"),
		       child ? ns_glitnir_bounds (e[i].child, origin) : Qnil,
		       intern_c_string (":alpha"),
		       child ? make_float (e[i].child_alpha) : Qnil,
		       intern_c_string (":shadow"),
		       shadow ? ns_glitnir_bounds (e[i].shadow, origin) : Qnil,
		       intern_c_string (":shadow-alpha"),
		       shadow ? make_float (e[i].shadow_alpha) : Qnil,
		       intern_c_string (":below"),
		       (child && shadow && e[i].shadow_at == e[i].child_at + 1
			? Qt : Qnil)),
		 result);
    }
  unblock_input ();
  return result;
}

#else /* ! NS_IMPL_COCOA */

Lisp_Object
ns_glitnir_snapshot (struct frame *f, bool png)
{
  error ("glitnir-snapshot: no image surface");
}

Lisp_Object
ns_glitnir_pairs (struct frame *f)
{
  return Qnil;
}

/* GNUstep builds are stock: no chrome, no shadow windows.  */

static void
ns_glitnir_apply (struct frame *f)
{
}

bool
ns_glitnir_chrome (struct frame *f, float *out)
{
  return false;
}

bool
ns_glitnir_shadow_p (NSWindow *w)
{
  return false;
}

NSWindow *
ns_glitnir_shadow_to_attach (NSWindow *child)
{
  return nil;
}

void
ns_glitnir_child_removed (NSWindow *child)
{
}

void
ns_glitnir_child_hidden (NSWindow *child)
{
}

void
ns_glitnir_follow (NSWindow *child)
{
}

void
ns_glitnir_level (NSWindow *child, NSInteger level)
{
}

void
ns_glitnir_frame_destroyed (struct frame *f)
{
}

#endif /* ! NS_IMPL_COCOA */

#endif /* HAVE_GLITNIR */

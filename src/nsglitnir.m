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
#include <string.h>

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

/* Take the chrome from `glitnir--chrome-vector' (called after the frame's
   parameters changed, and once a frame is made) and keep it with the frame's
   output data.  A frame that is not a child frame keeps nothing: root frames
   ignore chrome.  The resolver may change parameters itself; the second
   entry is ignored.  */
void
ns_glitnir_params_changed (struct frame *f)
{
  static bool busy;
  Lisp_Object frame, v;

  if (busy || !FRAME_NS_P (f) || !f->output_data.ns || FRAME_TOOLTIP_P (f)
      || !FRAME_PARENT_FRAME (f) || NILP (Ffboundp (Qglitnir__chrome_vector)))
    return;
  XSETFRAME (frame, f);
  busy = true;
  v = safe_calln (Qglitnir__chrome_vector, frame);
  busy = false;

  float *out = f->output_data.ns->glitnir_chrome;
  memset (out, 0, NHG_CHROME_SLOTS * sizeof *out);
  if (VECTORP (v))
    for (ptrdiff_t i = 0; i < min (ASIZE (v), NHG_CHROME_SLOTS); i++)
      if (NUMBERP (AREF (v, i)))
	out[i] = XFLOATINT (AREF (v, i));
}

/* Copy F's chrome vector (NHG_CHROME_SLOTS floats, logical pixels) to OUT and
   return true, or return false when Glitnir is off for F or F has no chrome.  */
bool
ns_glitnir_chrome (struct frame *f, float *out)
{
  if (!ns_glitnir_active_p (f)
      || f->output_data.ns->glitnir_chrome[NHG_CHROME_ON] == 0)
    return false;
  memcpy (out, f->output_data.ns->glitnir_chrome,
	  NHG_CHROME_SLOTS * sizeof *out);
  return true;
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

/* The visible child frames of F's root as the window server lists them,
   bottom to top: (:frame F :child (X Y W H) :alpha A :shadow (X Y W H)
   :shadow-alpha A :below BOOL), bounds in points from the root content's
   top left.  No shadow window is made yet, so :shadow is nil.  Needs no
   permission: only the window list, not pictures.  */
Lisp_Object
ns_glitnir_pairs (struct frame *f)
{
  struct frame *r = root_frame (f);
  Lisp_Object result = Qnil;

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
	  Lisp_Object tail, frame_obj;

	  FOR_EACH_FRAME (tail, frame_obj)
	    {
	      struct frame *c = XFRAME (frame_obj);
	      CGRect b;

	      if (!FRAME_NS_P (c) || !c->output_data.ns || !FRAME_NS_VIEW (c)
		  || !FRAME_PARENT_FRAME (c) || root_frame (c) != r
		  || (CGWindowID) [[(NSView *) FRAME_NS_VIEW (c) window]
				    windowNumber] != number)
		continue;
	      if (!CGRectMakeWithDictionaryRepresentation
		  ((CFDictionaryRef) [info objectForKey:
					     (NSString *) kCGWindowBounds], &b))
		continue;
	      double alpha
		= [[info objectForKey: (NSString *) kCGWindowAlpha] doubleValue];
	      /* The list runs front to back: consing each entry on the front
		 leaves the bottom one first.  */
	      result
		= Fcons (list (intern_c_string (":frame"), frame_obj,
			       intern_c_string (":child"),
			       list4 (make_float (b.origin.x - origin.origin.x),
				      make_float (b.origin.y - origin.origin.y),
				      make_float (b.size.width),
				      make_float (b.size.height)),
			       intern_c_string (":alpha"), make_float (alpha),
			       intern_c_string (":shadow"), Qnil,
			       intern_c_string (":shadow-alpha"), Qnil,
			       intern_c_string (":below"), Qnil),
			 result);
	      break;
	    }
	}
      CFRelease (list);
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

#endif /* ! NS_IMPL_COCOA */

#endif /* HAVE_GLITNIR */

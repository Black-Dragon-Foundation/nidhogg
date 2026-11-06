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

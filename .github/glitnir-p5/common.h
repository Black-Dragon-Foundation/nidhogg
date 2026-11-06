// Glitnir P5 spike: what every probe shares.  The windows mimic Nidhogg's NS backend (src/nsterm.m): a child frame is
// a borderless, resizable NSWindow attached with -addChildWindow:ordered:NSWindowAbove, hidden with -orderOut: (which
// detaches it) and raised by detaching and re-attaching it; its content is an NSView subview whose layer shows a
// CPU-drawn IOSurface (EmacsLayer).  Results go to $OUT/results.tsv (probe, key, value), screenshots to $OUT/shots.
#import <AppKit/AppKit.h>
#import <QuartzCore/QuartzCore.h>
#import <IOSurface/IOSurface.h>
#include <math.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

// ---- results ----
void P_init(const char *probe);
void R(NSString *key, NSString *fmt, ...) NS_FORMAT_FUNCTION(2, 3);
NSString *P_out(NSString *rel);         // $OUT/rel, directories made

// ---- time and the run loop ----
uint64_t P_now_ns(void);
void P_spin(double secs);               // run the event loop (window server events, CA commits) for secs
void P_frame_wait(uint64_t start_ns, int frame, double hz);  // pace frame `frame` of a loop started at start_ns
double P_cpu_self(void);                // this process's user+system CPU seconds
double P_cpu_ws(void);                  // WindowServer's CPU seconds (ps), -1 if unknown
NSString *P_stats(double *v, int n);    // "p50 … p95 … max … (n)" of v (sorted in place), in the values' unit

// ---- screens ----
// A screen whose backing scale is `scale` (1 or 2).  For 2 it tries, in order, an existing Retina screen, a HiDPI
// virtual display (CGVirtualDisplay) and a HiDPI mode of the main display; records which (key "screen.<scale>x").
NSScreen *P_screen(int scale);

// ---- Emacs-like content ----
// pattern: 0 text lines, 1 text over vertical stripes (for blur), 2 a solid colour.  With `cards` set, bg is the gap
// and each card (a rect in view points, origin bottom-left) is filled with cardBg and gets its own text.
@interface ELView : NSView
@property (nonatomic, strong) NSColor *bg, *fg, *cardBg;
@property (nonatomic, copy) NSArray<NSValue *> *cards;
@property (nonatomic) int pattern;
@property (nonatomic, copy) NSString *label;
@property (nonatomic) int generation;   // changes the text, so a repaint is a real change
- (void)paint;                          // CPU-draw into a fresh IOSurface and make it the layer's contents
@end

@interface ELWindow : NSWindow          // EmacsWindow's -orderFront: for child windows
@property (nonatomic, strong) ELView *el;
@property (nonatomic, weak) NSWindow *frameParent;
@end

// A root frame: titled window, content an ELView.  `r` is the content rect in global Cocoa coordinates.
ELWindow *P_root(NSRect r, BOOL dark, int pattern);
// A child frame of `parent` (nsterm.m's undecorated child): `r` in global Cocoa coordinates, shown and attached.
ELWindow *P_child(ELWindow *parent, NSRect r, BOOL dark, int pattern);
// Nidhogg's paths: ns_make_frame_visible / _invisible, ns_raise_frame, ns_set_offset, ns_set_window_size.
void P_show(ELWindow *child, ELWindow *parent);
void P_hide(ELWindow *child);
void P_raise(ELWindow *child);
void P_move(NSWindow *w, NSPoint origin);
void P_resize(NSWindow *w, NSRect frame);
// The look: transparent window, rounded masked content layer (corner-radius), continuous curve.
void P_round(NSWindow *w, CGFloat radius);
void P_appearance(BOOL dark);

// ---- the window server's view (no permission needed for bounds, order, alpha) ----
// One snapshot of the front-to-back on-screen list: for each of the n ids its index (-1: not on screen), bounds (CG
// global coordinates, top-left origin) and alpha.  Any output array may be NULL.
void P_ws(const CGWindowID *ids, int n, int *idx, CGRect *bounds, double *alpha);

// ---- captures ----
// Each returns a +1 CGImage or NULL; the rect is global Cocoa coordinates (points).
CGImageRef P_cap_cg_rect(NSRect r);                    // CGWindowListCreateImage, on-screen composite
CGImageRef P_cap_cg_window(CGWindowID wid, BOOL shadow); // CGWindowListCreateImage, one window
CGImageRef P_cap_sck_rect(NSRect r, NSString **err);   // ScreenCaptureKit (macOS 14+), display filter
CGImageRef P_cap_sck_window(CGWindowID wid, NSString **err); // ScreenCaptureKit, desktop-independent window
CGImageRef P_cap_cli_rect(NSRect r, NSString *path);   // screencapture -x -R (writes path, then loads it)
CGImageRef P_cap_cli_window(CGWindowID wid, NSString *path); // screencapture -x -o -l
CGImageRef P_cap_layer(CALayer *l, CGFloat scale);     // in-process renderInContext (no window server effects)
// The best on-screen capture that works here (decided once, key "capture.method"), saved as shots/<name>.png.
BOOL P_shot(NSString *name, NSRect r);
BOOL P_shot_window(NSString *name, NSWindow *w, BOOL shadow);
BOOL P_save_png(CGImageRef img, NSString *path);

// Pixels as 8-bit sRGB RGBA, premultiplied; caller frees.
uint8_t *P_rgba(CGImageRef img, size_t *w, size_t *h);
// Mean absolute Laplacian of the luminance in a sub-rectangle (pixels, top-left origin): high = sharp detail.
double P_detail(const uint8_t *px, size_t w, size_t x0, size_t y0, size_t rw, size_t rh);
// Mean and max absolute difference of two equal-size RGBA images over a sub-rectangle.
void P_diff(const uint8_t *a, const uint8_t *b, size_t w, size_t x0, size_t y0, size_t rw, size_t rh,
            double *mean, int *max);

// Colours and the shadow look.
NSColor *P_rgb(unsigned hex, CGFloat alpha);
typedef struct { const char *name; CGFloat ox, oy, blur, opacity; } PShadow;  // CSS-style: oy down, blur = 2 sigma
extern const PShadow P_MOCKUP, P_DEFAULT;   // the mockup's 0 24px 60px at 0.6; drgnx-look's (0 . 6) blur 16 at 0.35
void P_shadow_layer(CALayer *l, PShadow s);  // shadowColor/Opacity/Radius (= sigma)/Offset on l
CGPathRef P_ring_path(CGRect outer, CGRect inner, CGFloat radius);  // +1: outer minus rounded inner (even-odd)

// Option (b): a clear, click-through window whose layer draws shadow `spec` through shadowPath around a rounded rect,
// masked to the ring outside it.  -fitTo: sizes it to a popup frame (global Cocoa rect) outset by `margin`.
@interface PShadowWin : NSObject
@property (nonatomic, strong) NSWindow *win;
@property (nonatomic, strong) CALayer *layer;
@property (nonatomic, strong) CAShapeLayer *mask;
@property (nonatomic) CGFloat margin, radius;
@property (nonatomic) PShadow spec;
- (instancetype)initWithSpec:(PShadow)spec radius:(CGFloat)radius;
- (void)fitTo:(NSRect)popupFrame;
@end

// Cubic-bezier easing as in glitnir.el's curves: y at progress x.
double P_ease(double x1, double y1, double x2, double y2, double x);

// A rect helper: global Cocoa rect on screen s at (x, y) from its visible frame's top-left, size w × h.
NSRect P_at(NSScreen *s, CGFloat x, CGFloat y, CGFloat w, CGFloat h);

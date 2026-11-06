// Glitnir P5 spike: the shared part (see common.h).
#import "common.h"
#import <ImageIO/ImageIO.h>
#import <CoreText/CoreText.h>
#import <ScreenCaptureKit/ScreenCaptureKit.h>
#include <dlfcn.h>
#include <stdarg.h>
#include <stdlib.h>
#include <sys/resource.h>
#include <time.h>

// ---------------------------------------------------------------- results

static NSString *g_probe, *g_out;
static FILE *g_res;

NSString *P_out(NSString *rel)
{
  NSString *p = [g_out stringByAppendingPathComponent:rel];
  [[NSFileManager defaultManager] createDirectoryAtPath:[p stringByDeletingLastPathComponent]
                            withIntermediateDirectories:YES attributes:nil error:nil];
  return p;
}

void P_init(const char *probe)
{
  g_probe = @(probe);
  const char *o = getenv("OUT");
  g_out = o ? @(o) : @"out";
  g_res = fopen([P_out(@"results.tsv") fileSystemRepresentation], "a");
  if (!g_res)
    g_res = stderr;
  setvbuf(g_res, NULL, _IOLBF, 0);   // a crash keeps every line written so far
  [NSApplication sharedApplication];
  [NSApp setActivationPolicy:NSApplicationActivationPolicyRegular];
  [NSApp finishLaunching];
  [NSApp activateIgnoringOtherApps:YES];
  P_spin(0.3);
  R(@"start", @"pid %d, %lu screen(s)", getpid(), (unsigned long)[[NSScreen screens] count]);
}

void R(NSString *key, NSString *fmt, ...)
{
  va_list ap;
  va_start(ap, fmt);
  NSString *v = [[NSString alloc] initWithFormat:fmt arguments:ap];
  va_end(ap);
  v = [[v stringByReplacingOccurrencesOfString:@"\t" withString:@" "]
          stringByReplacingOccurrencesOfString:@"\n" withString:@" "];
  NSString *line = [NSString stringWithFormat:@"%@\t%@\t%@\n", g_probe, key, v];
  fputs(line.UTF8String, g_res);
  fputs(line.UTF8String, stdout);
  fflush(stdout);
}

// ---------------------------------------------------------------- time

uint64_t P_now_ns(void) { return clock_gettime_nsec_np(CLOCK_UPTIME_RAW); }

void P_spin(double secs)
{
  NSDate *end = [NSDate dateWithTimeIntervalSinceNow:secs];
  do
    {
      NSEvent *e = [NSApp nextEventMatchingMask:NSEventMaskAny untilDate:end
                                         inMode:NSDefaultRunLoopMode dequeue:YES];
      if (e)
        [NSApp sendEvent:e];
    }
  while ([end timeIntervalSinceNow] > 0);
}

void P_frame_wait(uint64_t start_ns, int frame, double hz)
{
  uint64_t target = start_ns + (uint64_t)(frame * 1e9 / hz);
  for (;;)
    {
      uint64_t now = P_now_ns();
      if (now >= target)
        break;
      double left = (target - now) / 1e9;
      P_spin(left < 0.002 ? left : 0.002);
    }
}

double P_cpu_self(void)
{
  struct rusage u;
  getrusage(RUSAGE_SELF, &u);
  return u.ru_utime.tv_sec + u.ru_utime.tv_usec / 1e6 + u.ru_stime.tv_sec + u.ru_stime.tv_usec / 1e6;
}

double P_cpu_ws(void)
{
  // ps's TIME is [[dd-]hh:]mm:ss.cc; WindowServer runs as another user, ps still shows its time.
  FILE *p = popen("ps -axo time=,comm= | awk '/WindowServer$/ {print $1; exit}'", "r");
  if (!p)
    return -1;
  char buf[64] = {0};
  char *got = fgets(buf, sizeof buf, p);
  pclose(p);
  if (!got || !buf[0])
    return -1;
  char *s = strchr(buf, '-');
  s = s ? s + 1 : buf;
  double v = 0;
  for (char *tok = strtok(s, ":"); tok; tok = strtok(NULL, ":"))
    v = v * 60 + atof(tok);
  return v;
}

static int cmp_d(const void *a, const void *b)
{
  double x = *(const double *)a, y = *(const double *)b;
  return x < y ? -1 : x > y;
}

NSString *P_stats(double *v, int n)
{
  if (n <= 0)
    return @"(no samples)";
  qsort(v, n, sizeof *v, cmp_d);
  return [NSString stringWithFormat:@"p50 %.3f p95 %.3f max %.3f (n %d)", v[n / 2], v[(int)(n * 0.95)], v[n - 1], n];
}

// ---------------------------------------------------------------- screens

// CGVirtualDisplay is CoreGraphics's private virtual-display class (as Chromium's and DeskPad's use of it); declared
// here, never linked by name: the instances come from NSClassFromString.
@interface CGVirtualDisplayDescriptor : NSObject
@property (nonatomic) unsigned int vendorID, productID, serialNum, maxPixelsWide, maxPixelsHigh;
@property (nonatomic, strong) NSString *name;
@property (nonatomic) CGSize sizeInMillimeters;
@property (nonatomic, strong) dispatch_queue_t queue;
@end
@interface CGVirtualDisplayMode : NSObject
- (instancetype)initWithWidth:(NSUInteger)w height:(NSUInteger)h refreshRate:(CGFloat)r;
@end
@interface CGVirtualDisplaySettings : NSObject
@property (nonatomic, strong) NSArray *modes;
@property (nonatomic) unsigned int hiDPI;
@end
@interface CGVirtualDisplay : NSObject
- (instancetype)initWithDescriptor:(CGVirtualDisplayDescriptor *)d;
- (BOOL)applySettings:(CGVirtualDisplaySettings *)s;
@property (readonly, nonatomic) CGDirectDisplayID displayID;
@end

static NSMutableArray *g_virtual;   // keeps the virtual displays alive for the process

static CGDirectDisplayID screen_id(NSScreen *s)
{
  return [s.deviceDescription[@"NSScreenNumber"] unsignedIntValue];
}

static NSScreen *screen_with_scale(int scale)
{
  for (NSScreen *s in [NSScreen screens])
    if ((int)s.backingScaleFactor == scale)
      return s;
  return nil;
}

static NSString *screens_desc(void)
{
  NSMutableString *m = [NSMutableString string];
  for (NSScreen *s in [NSScreen screens])
    [m appendFormat:@"[%u %@ x%.0f] ", screen_id(s), NSStringFromRect(s.frame), s.backingScaleFactor];
  return m;
}

static NSScreen *wait_for_scale(int scale, double secs)
{
  uint64_t end = P_now_ns() + (uint64_t)(secs * 1e9);
  NSScreen *s;
  while (!(s = screen_with_scale(scale)) && P_now_ns() < end)
    P_spin(0.1);
  return s;
}

// A HiDPI mode (pixel width twice the point width) on display d, set for this app only (reverted when it exits).
static BOOL hidpi_mode(CGDirectDisplayID d)
{
  NSDictionary *opt = @{(__bridge NSString *)kCGDisplayShowDuplicateLowResolutionModes: @YES};
  CFArrayRef modes = CGDisplayCopyAllDisplayModes(d, (__bridge CFDictionaryRef)opt);
  if (!modes)
    return NO;
  CGDisplayModeRef best = NULL;
  size_t bw = 0;
  NSMutableString *list = [NSMutableString string];
  for (CFIndex i = 0; i < CFArrayGetCount(modes); i++)
    {
      CGDisplayModeRef m = (CGDisplayModeRef)CFArrayGetValueAtIndex(modes, i);
      size_t w = CGDisplayModeGetWidth(m), pw = CGDisplayModeGetPixelWidth(m);
      [list appendFormat:@"%zux%zu/%zux%zu ", w, CGDisplayModeGetHeight(m), pw, CGDisplayModeGetPixelHeight(m)];
      if (pw == 2 * w && w >= 900 && w > bw)
        best = m, bw = w;
    }
  R(@"screen.modes", @"display %u: %@", d, list);
  BOOL ok = NO;
  if (best)
    {
      CGDisplayConfigRef cfg;
      CGError e = CGBeginDisplayConfiguration(&cfg);
      if (e == kCGErrorSuccess)
        e = CGConfigureDisplayWithDisplayMode(cfg, d, best, NULL);
      if (e == kCGErrorSuccess)
        e = CGCompleteDisplayConfiguration(cfg, kCGConfigureForAppOnly);
      R(@"screen.mode-set", @"display %u to %zu pt wide HiDPI: CGError %d", d, bw, (int)e);
      ok = e == kCGErrorSuccess;
    }
  CFRelease(modes);
  return ok;
}

static CGDirectDisplayID make_virtual(BOOL hidpi)
{
  Class D = NSClassFromString(@"CGVirtualDisplayDescriptor"), M = NSClassFromString(@"CGVirtualDisplayMode"),
        S = NSClassFromString(@"CGVirtualDisplaySettings"), V = NSClassFromString(@"CGVirtualDisplay");
  if (!D || !M || !S || !V)
    {
      R(@"screen.virtual", @"CGVirtualDisplay classes missing");
      return 0;
    }
  CGVirtualDisplayDescriptor *d = [[D alloc] init];
  d.queue = dispatch_get_main_queue();
  d.name = hidpi ? @"p5 virtual 2x" : @"p5 virtual 1x";
  d.maxPixelsWide = 2560;
  d.maxPixelsHigh = 1600;
  d.sizeInMillimeters = CGSizeMake(286, 179);   // ~227 dpi at 2560 px: a Retina panel
  d.vendorID = 0x5035;
  d.productID = hidpi ? 2 : 1;
  d.serialNum = hidpi ? 2 : 1;
  CGVirtualDisplay *v = [[V alloc] initWithDescriptor:d];
  if (!v)
    {
      R(@"screen.virtual", @"initWithDescriptor gave nil (hidpi %d)", hidpi);
      return 0;
    }
  CGVirtualDisplaySettings *s = [[S alloc] init];
  s.hiDPI = hidpi ? 1 : 0;
  // modes in points (DeskPad's use); if they turn out to be pixels, P_screen picks the larger HiDPI mode
  s.modes = hidpi ? @[[[M alloc] initWithWidth:1280 height:800 refreshRate:60],
                      [[M alloc] initWithWidth:2560 height:1600 refreshRate:60]]
                  : @[[[M alloc] initWithWidth:1440 height:900 refreshRate:60]];
  BOOL ok = [v applySettings:s];
  R(@"screen.virtual", @"display %u, applySettings %d (hidpi %d)", v.displayID, ok, hidpi);
  if (!g_virtual)
    g_virtual = [NSMutableArray array];
  [g_virtual addObject:v];
  return ok ? v.displayID : 0;
}

static NSScreen *wait_for_id(CGDirectDisplayID did, double secs)
{
  uint64_t end = P_now_ns() + (uint64_t)(secs * 1e9);
  for (;;)
    {
      for (NSScreen *s in [NSScreen screens])
        if (screen_id(s) == did)
          return s;
      if (P_now_ns() >= end)
        return nil;
      P_spin(0.1);
    }
}

static BOOL big_enough(NSScreen *s)
{
  return s.visibleFrame.size.width >= 1240 && s.visibleFrame.size.height >= 720;   // the probes' largest scene
}

NSScreen *P_screen(int scale)
{
  static BOOL logged, tried[3];
  static CGDirectDisplayID chosen[3];
  if (scale < 1 || scale > 2)
    return nil;
  if (!logged)
    {
      logged = YES;
      R(@"screen.initial", @"%@", screens_desc());
    }
  NSString *key = [NSString stringWithFormat:@"screen.%dx", scale];
  if (chosen[scale])
    {
      for (NSScreen *s in [NSScreen screens])
        if (screen_id(s) == chosen[scale] && (int)s.backingScaleFactor == scale)
          return s;
    }
  NSScreen *s = screen_with_scale(scale);
  if (s && big_enough(s))
    {
      R(key, @"display %u %@", screen_id(s), NSStringFromRect(s.frame));
      chosen[scale] = screen_id(s);
      return s;
    }
  if (tried[scale])
    return s;
  tried[scale] = YES;
  NSScreen *small = s;
  CGDirectDisplayID vd = make_virtual(scale == 2);
  NSScreen *v = vd ? wait_for_id(vd, 3) : nil;
  if (v && scale == 2 && (int)v.backingScaleFactor == 2 && !big_enough(v))
    {
      R(@"screen.virtual-small", @"%@: trying a larger HiDPI mode", NSStringFromRect(v.frame));
      hidpi_mode(vd);
      P_spin(1);
      v = wait_for_id(vd, 2);
    }
  if (v && (int)v.backingScaleFactor == scale)
    s = v, R(key, @"virtual display %u %@", screen_id(s), NSStringFromRect(s.frame));
  else if (scale == 2 && v && hidpi_mode(vd) && (s = wait_for_scale(2, 3)))
    R(key, @"virtual display %u + HiDPI mode %@", screen_id(s), NSStringFromRect(s.frame));
  else if (small)
    s = small, R(key, @"display %u %@ (smaller than the scenes want)", screen_id(s), NSStringFromRect(s.frame));
  else if (scale == 2 && hidpi_mode(CGMainDisplayID()) && (s = wait_for_scale(2, 3)))
    R(key, @"main display in a HiDPI mode %@", NSStringFromRect(s.frame));
  else
    s = nil, R(key, @"none: %@", screens_desc());
  if (s)
    chosen[scale] = screen_id(s);
  return s;
}

NSRect P_at(NSScreen *s, CGFloat x, CGFloat y, CGFloat w, CGFloat h)
{
  NSRect vf = s.visibleFrame;
  return NSMakeRect(vf.origin.x + x, vf.origin.y + vf.size.height - y - h, w, h);
}

NSColor *P_rgb(unsigned hex, CGFloat alpha)
{
  return [NSColor colorWithSRGBRed:((hex >> 16) & 255) / 255.0 green:((hex >> 8) & 255) / 255.0
                              blue:(hex & 255) / 255.0 alpha:alpha];
}

// ---------------------------------------------------------------- Emacs-like content

static NSArray<NSString *> *sample_lines(NSString *label, int gen)
{
  return @[label ?: @"",
           [NSString stringWithFormat:@"(defun glitnir-p5-probe (frame) ; generation %d", gen],
           @"  \"Corners, shadow, motion and blur on NS.\"",
           @"  (let ((chrome (glitnir--chrome-vector frame)))",
           @"    (when (aref chrome 0) (message \"on: %S\" chrome))))",
           @"iIl1|O0 ,.;: ——— ≠≤≥ λ → ✓ abcdefghijklmnopqrstuvwxyz",
           @"ABCDEFGHIJKLMNOPQRSTUVWXYZ 0123456789 {}[]()<>/\\",
           @"The quick brown fox jumps over the lazy dog.",
           @"corfu  eldoc-box  posframe  which-key  transient"];
}

static void draw_text(CGContextRef c, NSArray<NSString *> *lines, CGRect r, NSColor *fg)
{
  CTFontRef font = CTFontCreateWithName(CFSTR("Menlo"), 13, NULL);
  NSDictionary *attrs = @{(__bridge NSString *)kCTFontAttributeName: (__bridge id)font,
                          (__bridge NSString *)kCTForegroundColorAttributeName: (__bridge id)fg.CGColor};
  CGContextSaveGState(c);
  CGContextClipToRect(c, r);
  CGFloat y = r.origin.y + r.size.height - 20;
  for (NSString *s in lines)
    {
      if (y < r.origin.y)
        break;
      NSAttributedString *as = [[NSAttributedString alloc] initWithString:s attributes:attrs];
      CTLineRef l = CTLineCreateWithAttributedString((__bridge CFAttributedStringRef)as);
      CGContextSetTextPosition(c, r.origin.x + 10, y);
      CTLineDraw(l, c);
      CFRelease(l);
      y -= 17;
    }
  CGContextRestoreGState(c);
  CFRelease(font);
}

@implementation ELView
{
  IOSurfaceRef _surf;
}

- (instancetype)initWithFrame:(NSRect)r
{
  if ((self = [super initWithFrame:r]))
    {
      self.wantsLayer = YES;
      self.layerContentsRedrawPolicy = NSViewLayerContentsRedrawNever;
      _bg = NSColor.whiteColor;
      _fg = NSColor.blackColor;
      _cardBg = NSColor.whiteColor;
    }
  return self;
}

- (void)dealloc
{
  if (_surf)
    CFRelease(_surf);
}

- (CALayer *)makeBackingLayer
{
  CALayer *l = [CALayer layer];
  l.contentsGravity = kCAGravityBottomLeft;
  return l;
}

- (BOOL)wantsUpdateLayer { return YES; }
- (void)updateLayer {}

- (void)viewDidChangeBackingProperties
{
  [super viewDidChangeBackingProperties];
  [self paint];
}

- (void)setFrameSize:(NSSize)s
{
  BOOL changed = !NSEqualSizes(s, self.frame.size);
  [super setFrameSize:s];
  if (changed && self.window)
    [self paint];
}

- (void)paint
{
  CGFloat scale = self.window ? self.window.backingScaleFactor : 1;
  NSSize sz = self.bounds.size;
  int w = (int)ceil(sz.width * scale), h = (int)ceil(sz.height * scale);
  if (w <= 0 || h <= 0)
    return;
  int bpr = (int)IOSurfaceAlignProperty(kIOSurfaceBytesPerRow, (size_t)w * 4);
  // As EmacsLayer: BGRA, 4 bytes per element, drawn on the CPU through a CGBitmapContext.
  IOSurfaceRef s = IOSurfaceCreate((__bridge CFDictionaryRef)@{
      (__bridge NSString *)kIOSurfaceWidth: @(w), (__bridge NSString *)kIOSurfaceHeight: @(h),
      (__bridge NSString *)kIOSurfaceBytesPerRow: @(bpr), (__bridge NSString *)kIOSurfaceBytesPerElement: @4,
      (__bridge NSString *)kIOSurfacePixelFormat: @((unsigned)'BGRA')});
  if (!s)
    return;
  IOSurfaceLock(s, 0, NULL);
  CGColorSpaceRef cs = CGColorSpaceCreateWithName(kCGColorSpaceSRGB);
  CGContextRef c = CGBitmapContextCreate(IOSurfaceGetBaseAddress(s), w, h, 8, IOSurfaceGetBytesPerRow(s), cs,
                                         kCGImageAlphaPremultipliedFirst | kCGBitmapByteOrder32Little);
  CGContextScaleCTM(c, scale, scale);
  CGRect all = CGRectMake(0, 0, sz.width, sz.height);
  CGContextClearRect(c, all);
  CGContextSetFillColorWithColor(c, self.bg.CGColor);
  CGContextFillRect(c, all);
  if (self.pattern == 1)
    {
      CGContextSetFillColorWithColor(c, [self.fg colorWithAlphaComponent:0.85].CGColor);
      for (CGFloat x = 0; x < sz.width; x += 8)
        CGContextFillRect(c, CGRectMake(x, 0, 4, sz.height));
    }
  if (self.cards)
    {
      int i = 0;
      for (NSValue *v in self.cards)
        {
          CGRect cr = NSRectToCGRect(v.rectValue);
          CGContextSetFillColorWithColor(c, self.cardBg.CGColor);
          CGContextFillRect(c, cr);
          draw_text(c, sample_lines([NSString stringWithFormat:@"window card %d", ++i], self.generation), cr,
                    self.fg);
        }
    }
  else if (self.pattern != 2)
    draw_text(c, sample_lines(self.label, self.generation), all, self.fg);
  CGContextFlush(c);
  CGContextRelease(c);
  CGColorSpaceRelease(cs);
  IOSurfaceUnlock(s, 0, NULL);
  self.layer.contentsScale = scale;
  self.layer.contents = (__bridge id)s;
  if (_surf)
    CFRelease(_surf);
  _surf = s;
}
@end

@implementation ELWindow
- (BOOL)canBecomeKeyWindow { return YES; }

// nsterm.m: "the only way to reorder child frames is by removing them from the parent and then reattaching them".
- (void)orderFront:(id)sender
{
  NSWindow *parent = self.parentWindow;
  if (parent)
    {
      [parent removeChildWindow:self];
      [parent addChildWindow:self ordered:NSWindowAbove];
    }
  else
    [super orderFront:sender];
}
@end

static ELView *add_el(NSWindow *w, BOOL dark, int pattern, BOOL child)
{
  ELView *v = [[ELView alloc] initWithFrame:w.contentView.bounds];
  v.autoresizingMask = NSViewWidthSizable | NSViewHeightSizable;
  v.bg = child ? P_rgb(dark ? 0x2b2d31 : 0xffffff, 1) : P_rgb(dark ? 0x1e1f22 : 0xeeeeea, 1);
  v.fg = P_rgb(dark ? 0xdcdcdc : 0x1d1d1f, 1);
  v.cardBg = P_rgb(dark ? 0x2b2d31 : 0xffffff, 1);
  v.pattern = pattern;
  v.label = child ? @"child frame" : @"root frame";
  w.contentView.wantsLayer = YES;
  [w.contentView addSubview:v];
  w.backgroundColor = v.bg;
  return v;
}

ELWindow *P_root(NSRect r, BOOL dark, int pattern)
{
  ELWindow *w = [[ELWindow alloc]
      initWithContentRect:NSMakeRect(0, 0, r.size.width, r.size.height)
                styleMask:NSWindowStyleMaskTitled | NSWindowStyleMaskResizable | NSWindowStyleMaskClosable
                          | NSWindowStyleMaskMiniaturizable
                  backing:NSBackingStoreBuffered defer:NO];
  w.releasedWhenClosed = NO;
  w.title = @"p5 root frame";
  [w setFrame:[w frameRectForContentRect:r] display:NO];
  w.el = add_el(w, dark, pattern, NO);
  [w orderFrontRegardless];
  P_spin(0.05);
  [w.el paint];
  return w;
}

ELWindow *P_child(ELWindow *parent, NSRect r, BOOL dark, int pattern)
{
  // nsterm.m's undecorated child frame: borderless + resizable, buffered, deferred, no shadow, floating subrole
  ELWindow *w = [[ELWindow alloc] initWithContentRect:NSMakeRect(0, 0, r.size.width, r.size.height)
                                            styleMask:NSWindowStyleMaskBorderless | NSWindowStyleMaskResizable
                                              backing:NSBackingStoreBuffered defer:YES];
  w.releasedWhenClosed = NO;
  [w setFrame:r display:NO];
  w.el = add_el(w, dark, pattern, YES);
  w.hasShadow = NO;
  [w setAccessibilitySubrole:NSAccessibilityFloatingWindowSubrole];
  P_show(w, parent);
  [w.el paint];
  return w;
}

void P_show(ELWindow *c, ELWindow *p)
{
  // ns_make_frame_visible: ns_raise_frame (-orderFront:), then setParentChildRelationships re-attaches the child
  c.frameParent = p;
  [c orderFront:nil];
  if (!c.parentWindow)
    [p addChildWindow:c ordered:NSWindowAbove];
}

void P_hide(ELWindow *c) { [c orderOut:nil]; }   // ns_make_frame_invisible; detaches the child (Apple's docs)
void P_raise(ELWindow *c) { [c orderFront:nil]; } // ns_raise_frame
void P_move(NSWindow *w, NSPoint o) { [w setFrameOrigin:o]; }            // ns_set_offset
void P_resize(NSWindow *w, NSRect f) { [w setFrame:f display:NO]; }      // ns_set_window_size

void P_round(NSWindow *w, CGFloat radius)
{
  w.opaque = NO;
  w.backgroundColor = NSColor.clearColor;
  NSView *cv = w.contentView;
  cv.wantsLayer = YES;
  cv.clipsToBounds = YES;   // AppKit drives masksToBounds from it on macOS 14+
  cv.layer.cornerRadius = radius;
  cv.layer.masksToBounds = YES;
  cv.layer.cornerCurve = kCACornerCurveContinuous;
  [w invalidateShadow];
}

void P_appearance(BOOL dark)
{
  NSApp.appearance = [NSAppearance appearanceNamed:dark ? NSAppearanceNameDarkAqua : NSAppearanceNameAqua];
}

const PShadow P_MOCKUP = {"mockup", 0, 24, 60, 0.6};
const PShadow P_DEFAULT = {"default", 0, 6, 16, 0.35};

void P_shadow_layer(CALayer *l, PShadow s)
{
  CGColorRef black = CGColorCreateSRGB(0, 0, 0, 1);
  l.shadowColor = black;
  CGColorRelease(black);
  l.shadowOpacity = (float)s.opacity;
  l.shadowRadius = s.blur / 2;               // CSS blur is twice the Gaussian sigma
  l.shadowOffset = CGSizeMake(s.ox, -s.oy);  // CA's y is up (unflipped layers)
}

CGPathRef P_ring_path(CGRect outer, CGRect inner, CGFloat radius)
{
  CGMutablePathRef p = CGPathCreateMutable();
  CGPathAddRect(p, NULL, outer);
  CGPathAddRoundedRect(p, NULL, inner, radius, radius);
  return p;
}

double P_ease(double x1, double y1, double x2, double y2, double x)
{
  if (x <= 0)
    return 0;
  if (x >= 1)
    return 1;
  double lo = 0, hi = 1, t = x;
  for (int i = 0; i < 60; i++)
    {
      t = (lo + hi) / 2;
      double bx = 3 * (1 - t) * (1 - t) * t * x1 + 3 * (1 - t) * t * t * x2 + t * t * t;
      if (bx < x)
        lo = t;
      else
        hi = t;
    }
  return 3 * (1 - t) * (1 - t) * t * y1 + 3 * (1 - t) * t * t * y2 + t * t * t;
}

// ---------------------------------------------------------------- the shadow window (option b)

@implementation PShadowWin
- (instancetype)initWithSpec:(PShadow)spec radius:(CGFloat)radius
{
  if (!(self = [super init]))
    return nil;
  _spec = spec;
  _radius = radius;
  _margin = ceil(1.5 * spec.blur + fabs(spec.oy) + fabs(spec.ox));   // 3 sigma plus the offset
  _win = [[NSWindow alloc] initWithContentRect:NSMakeRect(0, 0, 100, 100) styleMask:NSWindowStyleMaskBorderless
                                       backing:NSBackingStoreBuffered defer:NO];
  _win.releasedWhenClosed = NO;
  _win.opaque = NO;
  _win.backgroundColor = NSColor.clearColor;
  _win.hasShadow = NO;
  _win.ignoresMouseEvents = YES;
  NSView *v = _win.contentView;
  v.layer = [CALayer layer];   // layer-hosting: the view's layer is ours
  v.wantsLayer = YES;
  _layer = [CALayer layer];
  P_shadow_layer(_layer, spec);
  _mask = [CAShapeLayer layer];
  _mask.fillRule = kCAFillRuleEvenOdd;
  _layer.mask = _mask;
  [v.layer addSublayer:_layer];
  return self;
}

- (void)fitTo:(NSRect)pf
{
  CGFloat m = self.margin;
  NSRect f = NSInsetRect(pf, -m, -m);
  [CATransaction begin];
  [CATransaction setDisableActions:YES];
  [self.win setFrame:f display:NO];
  CGRect b = CGRectMake(0, 0, f.size.width, f.size.height);
  CGRect inner = CGRectMake(m, m, pf.size.width, pf.size.height);
  self.layer.frame = b;
  CGPathRef p = CGPathCreateWithRoundedRect(inner, self.radius, self.radius, NULL);
  self.layer.shadowPath = p;
  CGPathRelease(p);
  self.mask.frame = b;
  CGPathRef ring = P_ring_path(b, inner, self.radius);
  self.mask.path = ring;
  CGPathRelease(ring);
  [CATransaction commit];
}
@end


// ---------------------------------------------------------------- the window server's view

void P_ws(const CGWindowID *ids, int n, int *idx, CGRect *bounds, double *alpha)
{
  for (int i = 0; i < n; i++)
    {
      if (idx)
        idx[i] = -1;
      if (bounds)
        bounds[i] = CGRectNull;
      if (alpha)
        alpha[i] = -1;
    }
  CFArrayRef l = CGWindowListCopyWindowInfo(kCGWindowListOptionOnScreenOnly, kCGNullWindowID);
  if (!l)
    return;
  int k = 0;
  for (NSDictionary *d in (__bridge NSArray *)l)
    {
      CGWindowID wid = [d[(__bridge NSString *)kCGWindowNumber] unsignedIntValue];
      for (int i = 0; i < n; i++)
        if (ids[i] == wid)
          {
            if (idx)
              idx[i] = k;
            if (bounds)
              CGRectMakeWithDictionaryRepresentation(
                  (__bridge CFDictionaryRef)d[(__bridge NSString *)kCGWindowBounds], &bounds[i]);
            if (alpha)
              alpha[i] = [d[(__bridge NSString *)kCGWindowAlpha] doubleValue];
          }
      k++;
    }
  CFRelease(l);
}

// ---------------------------------------------------------------- captures

// CGWindowListCreateImage is obsoleted in the macOS 15 SDK, so it is looked up at run time.
typedef CGImageRef (*cgwlci_fn)(CGRect, CGWindowListOption, CGWindowID, uint32_t);
enum { IMG_IGNORE_FRAMING = 1 << 0, IMG_BEST = 1 << 3 };

static cgwlci_fn cgwlci(void)
{
  static cgwlci_fn f;
  static BOOL done;
  if (!done)
    {
      done = YES;
      f = (cgwlci_fn)dlsym(RTLD_DEFAULT, "CGWindowListCreateImage");
    }
  return f;
}

static CGRect to_cg(NSRect r)
{
  CGFloat H = [[NSScreen screens][0] frame].size.height;
  return CGRectMake(r.origin.x, H - r.origin.y - r.size.height, r.size.width, r.size.height);
}

CGImageRef P_cap_cg_rect(NSRect r)
{
  cgwlci_fn f = cgwlci();
  return f ? f(to_cg(r), kCGWindowListOptionOnScreenOnly, kCGNullWindowID, IMG_BEST) : NULL;
}

CGImageRef P_cap_cg_window(CGWindowID wid, BOOL shadow)
{
  cgwlci_fn f = cgwlci();
  return f ? f(CGRectNull, kCGWindowListOptionIncludingWindow, wid, IMG_BEST | (shadow ? 0 : IMG_IGNORE_FRAMING))
           : NULL;
}

// Set by a completion handler on another queue; the main thread spins its run loop until it is set.
@interface PFlag : NSObject
@property (atomic) BOOL done;
@end
@implementation PFlag
@end

static BOOL wait_flag(PFlag *flag, double secs)
{
  uint64_t end = P_now_ns() + (uint64_t)(secs * 1e9);
  while (!flag.done && P_now_ns() < end)
    P_spin(0.01);
  return flag.done;
}

static SCShareableContent *sck_content(NSString **err)
{
  __block SCShareableContent *content = nil;
  __block NSError *e1 = nil;
  PFlag *done = [PFlag new];
  [SCShareableContent getShareableContentExcludingDesktopWindows:NO onScreenWindowsOnly:YES
                                               completionHandler:^(SCShareableContent *c, NSError *e) {
                                                 content = c;
                                                 e1 = e;
                                                 done.done = YES;
                                               }];
  if (!wait_flag(done, 10))
    *err = @"getShareableContent: no answer in 10 s";
  else if (!content)
    *err = [NSString stringWithFormat:@"getShareableContent: %@ %ld %@", e1.domain, (long)e1.code,
                                      e1.localizedDescription];
  return content;
}

static CGImageRef sck_shot(SCContentFilter *f, SCStreamConfiguration *cfg, NSString **err)
{
  __block CGImageRef img = NULL;
  __block NSError *e2 = nil;
  PFlag *done = [PFlag new];
  [SCScreenshotManager captureImageWithFilter:f configuration:cfg
                            completionHandler:^(CGImageRef i, NSError *e) {
                              if (i)
                                img = CGImageRetain(i);
                              e2 = e;
                              done.done = YES;
                            }];
  if (!wait_flag(done, 10))
    *err = @"captureImage: no answer in 10 s";
  else if (!img)
    *err = [NSString stringWithFormat:@"captureImage: %@ %ld %@", e2.domain, (long)e2.code, e2.localizedDescription];
  return img;
}

CGImageRef P_cap_sck_rect(NSRect r, NSString **err)
{
  __autoreleasing NSString *dummy = nil;
  if (!err)
    err = &dummy;
  SCShareableContent *content = sck_content(err);
  if (!content)
    return NULL;
  CGRect cg = to_cg(r);
  SCDisplay *disp = nil;
  for (SCDisplay *d in content.displays)
    if (CGRectContainsRect(d.frame, cg))
      disp = d;
  if (!disp)
    disp = content.displays.firstObject;
  if (!disp)
    {
      *err = @"no SCDisplay";
      return NULL;
    }
  SCContentFilter *f = [[SCContentFilter alloc] initWithDisplay:disp excludingWindows:@[]];
  SCStreamConfiguration *cfg = [[SCStreamConfiguration alloc] init];
  CGFloat scale = f.pointPixelScale;
  cfg.sourceRect = CGRectOffset(cg, -disp.frame.origin.x, -disp.frame.origin.y);
  cfg.width = (size_t)(cg.size.width * scale);
  cfg.height = (size_t)(cg.size.height * scale);
  cfg.showsCursor = NO;
  return sck_shot(f, cfg, err);
}

CGImageRef P_cap_sck_window(CGWindowID wid, NSString **err)
{
  __autoreleasing NSString *dummy = nil;
  if (!err)
    err = &dummy;
  SCShareableContent *content = sck_content(err);
  if (!content)
    return NULL;
  SCWindow *win = nil;
  for (SCWindow *w in content.windows)
    if (w.windowID == wid)
      win = w;
  if (!win)
    {
      *err = [NSString stringWithFormat:@"window %u not among %lu shareable windows", wid,
                                        (unsigned long)content.windows.count];
      return NULL;
    }
  SCContentFilter *f = [[SCContentFilter alloc] initWithDesktopIndependentWindow:win];
  SCStreamConfiguration *cfg = [[SCStreamConfiguration alloc] init];
  CGFloat scale = f.pointPixelScale;
  cfg.width = (size_t)(win.frame.size.width * scale);
  cfg.height = (size_t)(win.frame.size.height * scale);
  cfg.showsCursor = NO;
  return sck_shot(f, cfg, err);
}

static CGImageRef load_png(NSString *path)
{
  CGImageSourceRef src = CGImageSourceCreateWithURL((__bridge CFURLRef)[NSURL fileURLWithPath:path], NULL);
  if (!src)
    return NULL;
  CGImageRef img = CGImageSourceGetCount(src) ? CGImageSourceCreateImageAtIndex(src, 0, NULL) : NULL;
  CFRelease(src);
  return img;
}

CGImageRef P_cap_cli_rect(NSRect r, NSString *path)
{
  CGRect cg = to_cg(r);
  [[NSFileManager defaultManager] removeItemAtPath:path error:nil];
  NSString *cmd = [NSString stringWithFormat:@"screencapture -x -R%d,%d,%d,%d '%@' >/dev/null 2>&1",
                                             (int)cg.origin.x, (int)cg.origin.y, (int)cg.size.width,
                                             (int)cg.size.height, path];
  int rc = system(cmd.UTF8String);
  return rc == 0 ? load_png(path) : NULL;
}

CGImageRef P_cap_cli_window(CGWindowID wid, NSString *path)
{
  [[NSFileManager defaultManager] removeItemAtPath:path error:nil];
  NSString *cmd = [NSString stringWithFormat:@"screencapture -x -o -l%u '%@' >/dev/null 2>&1", wid, path];
  int rc = system(cmd.UTF8String);
  return rc == 0 ? load_png(path) : NULL;
}

CGImageRef P_cap_layer(CALayer *l, CGFloat scale)
{
  size_t w = (size_t)ceil(l.bounds.size.width * scale), h = (size_t)ceil(l.bounds.size.height * scale);
  if (!w || !h)
    return NULL;
  CGColorSpaceRef cs = CGColorSpaceCreateWithName(kCGColorSpaceSRGB);
  CGContextRef c = CGBitmapContextCreate(NULL, w, h, 8, w * 4, cs,
                                         kCGImageAlphaPremultipliedLast | kCGBitmapByteOrder32Big);
  CGContextScaleCTM(c, scale, scale);
  [l renderInContext:c];
  CGImageRef img = CGBitmapContextCreateImage(c);
  CGContextRelease(c);
  CGColorSpaceRelease(cs);
  return img;
}

BOOL P_save_png(CGImageRef img, NSString *path)
{
  if (!img)
    return NO;
  CGImageDestinationRef d = CGImageDestinationCreateWithURL((__bridge CFURLRef)[NSURL fileURLWithPath:path],
                                                            CFSTR("public.png"), 1, NULL);
  if (!d)
    return NO;
  CGImageDestinationAddImage(d, img, NULL);
  BOOL ok = CGImageDestinationFinalize(d);
  CFRelease(d);
  return ok;
}

// The capture route for screenshots, decided on the first shot: CG, then ScreenCaptureKit, then screencapture.
static int g_method = -1;
static const char *g_method_names[] = {"CGWindowListCreateImage", "ScreenCaptureKit", "screencapture"};

static CGImageRef cap_rect(int m, NSRect r, NSString *tmp, NSString **err)
{
  switch (m)
    {
    case 0: return P_cap_cg_rect(r);
    case 1: return P_cap_sck_rect(r, err);
    default: return P_cap_cli_rect(r, tmp);
    }
}

BOOL P_shot(NSString *name, NSRect r)
{
  P_spin(0.05);
  [CATransaction flush];
  NSString *path = P_out([NSString stringWithFormat:@"shots/%@.png", name]);
  NSString *tmp = P_out(@"shots/.tmp.png");
  CGImageRef img = NULL;
  if (g_method < 0)
    {
      for (int m = 0; m < 3 && !img; m++)
        {
          NSString *err = nil;
          img = cap_rect(m, r, tmp, &err);
          if (img)
            g_method = m;
          else
            R(@"capture.tried", @"%s: failed %@", g_method_names[m], err ?: @"");
        }
      R(@"capture.method", @"%s", g_method < 0 ? "none" : g_method_names[g_method]);
    }
  else
    img = cap_rect(g_method, r, tmp, NULL);
  BOOL ok = P_save_png(img, path);
  R([@"shot." stringByAppendingString:name], @"%@ %zux%zu px for %.0fx%.0f pt", ok ? @"saved" : @"FAILED",
    img ? CGImageGetWidth(img) : 0, img ? CGImageGetHeight(img) : 0, r.size.width, r.size.height);
  if (img)
    CGImageRelease(img);
  return ok;
}

BOOL P_shot_window(NSString *name, NSWindow *w, BOOL shadow)
{
  P_spin(0.05);
  [CATransaction flush];
  NSString *path = P_out([NSString stringWithFormat:@"shots/%@.png", name]);
  CGImageRef img = P_cap_cg_window((CGWindowID)w.windowNumber, shadow);
  if (!img)
    img = P_cap_cli_window((CGWindowID)w.windowNumber, path);
  BOOL ok = P_save_png(img, path);
  R([@"shot." stringByAppendingString:name], @"%@ %zux%zu px", ok ? @"saved" : @"FAILED",
    img ? CGImageGetWidth(img) : 0, img ? CGImageGetHeight(img) : 0);
  if (img)
    CGImageRelease(img);
  return ok;
}

// ---------------------------------------------------------------- pixels

uint8_t *P_rgba(CGImageRef img, size_t *w, size_t *h)
{
  *w = CGImageGetWidth(img);
  *h = CGImageGetHeight(img);
  uint8_t *px = calloc(*w * *h, 4);
  CGColorSpaceRef cs = CGColorSpaceCreateWithName(kCGColorSpaceSRGB);
  CGContextRef c = CGBitmapContextCreate(px, *w, *h, 8, *w * 4, cs,
                                         kCGImageAlphaPremultipliedLast | kCGBitmapByteOrder32Big);
  CGContextDrawImage(c, CGRectMake(0, 0, *w, *h), img);
  CGContextRelease(c);
  CGColorSpaceRelease(cs);
  return px;   // row 0 is the image's top row
}

static double lum(const uint8_t *p) { return 0.2126 * p[0] + 0.7152 * p[1] + 0.0722 * p[2]; }

double P_detail(const uint8_t *px, size_t w, size_t x0, size_t y0, size_t rw, size_t rh)
{
  double sum = 0;
  long n = 0;
  for (size_t y = y0 + 1; y + 1 < y0 + rh; y++)
    for (size_t x = x0 + 1; x + 1 < x0 + rw; x++)
      {
        const uint8_t *c = px + (y * w + x) * 4;
        double l = 4 * lum(c) - lum(c - 4) - lum(c + 4) - lum(c - w * 4) - lum(c + w * 4);
        sum += fabs(l);
        n++;
      }
  return n ? sum / n : 0;
}

void P_diff(const uint8_t *a, const uint8_t *b, size_t w, size_t x0, size_t y0, size_t rw, size_t rh,
            double *mean, int *max)
{
  double sum = 0;
  long n = 0;
  int mx = 0;
  for (size_t y = y0; y < y0 + rh; y++)
    for (size_t x = x0; x < x0 + rw; x++)
      for (int k = 0; k < 3; k++)
        {
          int d = abs((int)a[(y * w + x) * 4 + k] - (int)b[(y * w + x) * 4 + k]);
          sum += d;
          n++;
          if (d > mx)
            mx = d;
        }
  *mean = n ? sum / n : 0;
  *max = mx;
}

// Q6: can a CI runner capture windows, and which routes work without the Screen Recording (TCC) permission?
// Our own window is magenta; a second process (this binary with --foreign) shows a cyan one.  Each route captures
// both; a route that returns an image but not the right colours is "blank" (the OS hides what it may not show).
#import "common.h"
#include <dlfcn.h>

static NSString *verdict(CGImageRef img, NSRect cap, NSRect own, NSRect foreign, NSString **own_v, NSString **for_v)
{
  if (!img)
    {
      *own_v = *for_v = @"no image";
      return @"no image";
    }
  size_t w, h;
  uint8_t *px = P_rgba(img, &w, &h);
  double sx = w / cap.size.width, sy = h / cap.size.height;
  NSString *(^probe)(NSRect, unsigned) = ^NSString *(NSRect r, unsigned want) {
    if (NSIsEmptyRect(r))
      return @"n/a";
    // centre of r in image pixels (image row 0 is the top)
    size_t x = (size_t)((NSMidX(r) - cap.origin.x) * sx);
    size_t y = (size_t)((NSMaxY(cap) - NSMidY(r)) * sy);
    if (x >= w || y >= h)
      return @"outside";
    const uint8_t *p = px + (y * w + x) * 4;
    int r0 = (want >> 16) & 255, g0 = (want >> 8) & 255, b0 = want & 255;
    BOOL ok = abs(p[0] - r0) < 40 && abs(p[1] - g0) < 40 && abs(p[2] - b0) < 40;
    return [NSString stringWithFormat:@"%@ (rgb %d,%d,%d)", ok ? @"ok" : @"blank", p[0], p[1], p[2]];
  };
  *own_v = probe(own, 0xff00ff);
  *for_v = probe(foreign, 0x00ffff);
  free(px);
  return [NSString stringWithFormat:@"%zux%zu px", w, h];
}

static void report(NSString *route, CGImageRef img, double ms, NSString *err, NSRect cap, NSRect own, NSRect foreign)
{
  NSString *o, *f;
  NSString *size = verdict(img, cap, own, foreign, &o, &f);
  R([@"route." stringByAppendingString:route], @"%@; own %@; foreign %@; %.1f ms%@", size, o, f, ms,
    err ? [@"; " stringByAppendingString:err] : @"");
  if (img)
    {
      P_save_png(img, P_out([NSString stringWithFormat:@"shots/p6-%@.png", route]));
      CGImageRelease(img);
    }
}

static int foreign_main(int argc, char **argv)
{
  // --foreign x y w h (global Cocoa coordinates)
  P_init("p6-foreign");
  NSRect r = NSMakeRect(atof(argv[2]), atof(argv[3]), atof(argv[4]), atof(argv[5]));
  NSWindow *w = [[NSWindow alloc] initWithContentRect:NSMakeRect(0, 0, r.size.width, r.size.height)
                                            styleMask:NSWindowStyleMaskBorderless backing:NSBackingStoreBuffered
                                                defer:NO];
  w.releasedWhenClosed = NO;
  [w setFrame:r display:NO];
  w.backgroundColor = P_rgb(0x00ffff, 1);
  w.level = NSFloatingWindowLevel;
  [w orderFrontRegardless];
  R(@"window", @"%ld at %@", (long)w.windowNumber, NSStringFromRect(r));
  P_spin(60);   // the parent kills it when done
  return 0;
}

int main(int argc, char **argv)
{
  @autoreleasepool
    {
      if (argc >= 6 && !strcmp(argv[1], "--foreign"))
        return foreign_main(argc, argv);
      P_init("p6-capture");
      R(@"os", @"%@", [[NSProcessInfo processInfo] operatingSystemVersionString]);
      R(@"tcc.preflight", @"CGPreflightScreenCaptureAccess = %d", CGPreflightScreenCaptureAccess());
      R(@"cgwlci", @"CGWindowListCreateImage %@", dlsym(RTLD_DEFAULT, "CGWindowListCreateImage") ? @"present" : @"absent");

      NSScreen *s = P_screen(1) ?: [NSScreen screens][0];
      ELWindow *own = P_root(P_at(s, 60, 60, 300, 200), NO, 2);
      own.el.bg = P_rgb(0xff00ff, 1);
      own.backgroundColor = own.el.bg;
      [own.el paint];
      own.level = NSFloatingWindowLevel;
      NSRect ownr = [own contentRectForFrameRect:own.frame];
      NSRect forr = NSOffsetRect(ownr, 360, 0);

      NSTask *t = [[NSTask alloc] init];
      t.executableURL = [NSURL fileURLWithPath:[[NSBundle mainBundle] executablePath]];
      t.arguments = @[@"--foreign", @(forr.origin.x).stringValue, @(forr.origin.y).stringValue,
                      @(forr.size.width).stringValue, @(forr.size.height).stringValue];
      NSError *err = nil;
      if (![t launchAndReturnError:&err])
        {
          R(@"foreign", @"launch failed: %@", err);
          forr = NSZeroRect;
        }
      P_spin(2.0);
      NSRect cap = NSInsetRect(NSUnionRect(ownr, NSIsEmptyRect(forr) ? ownr : forr), -20, -20);

      // whole-region routes
      uint64_t t0 = P_now_ns();
      CGImageRef img = P_cap_cg_rect(cap);
      report(@"cg-rect", img, (P_now_ns() - t0) / 1e6, nil, cap, ownr, forr);

      NSString *e = nil;
      t0 = P_now_ns();
      img = P_cap_sck_rect(cap, &e);
      report(@"sck-rect", img, (P_now_ns() - t0) / 1e6, e, cap, ownr, forr);

      t0 = P_now_ns();
      img = P_cap_cli_rect(cap, P_out(@"shots/p6-cli-rect-raw.png"));
      report(@"cli-rect", img, (P_now_ns() - t0) / 1e6, nil, cap, ownr, forr);

      // one-window routes (own, then the foreign window by its number from the window list)
      CGWindowID fid = 0;
      CFArrayRef l = CGWindowListCopyWindowInfo(kCGWindowListOptionOnScreenOnly, kCGNullWindowID);
      for (NSDictionary *d in (__bridge NSArray *)l)
        if ([d[(__bridge NSString *)kCGWindowOwnerPID] intValue] == t.processIdentifier)
          fid = [d[(__bridge NSString *)kCGWindowNumber] unsignedIntValue];
      if (l)
        CFRelease(l);
      R(@"foreign.window", @"%u", fid);
      // the window-list names need the permission too: record whether we see the other process's window name
      l = CGWindowListCopyWindowInfo(kCGWindowListOptionOnScreenOnly, kCGNullWindowID);
      long named = 0, total = 0;
      for (NSDictionary *d in (__bridge NSArray *)l)
        {
          total++;
          if ([d[(__bridge NSString *)kCGWindowName] length])
            named++;
        }
      if (l)
        CFRelease(l);
      R(@"window-list.names", @"%ld of %ld on-screen windows carry a name", named, total);

      CGWindowID wids[2] = {(CGWindowID)own.windowNumber, fid};
      NSString *who[2] = {@"own", @"foreign"};
      NSRect rects[2] = {ownr, forr};
      for (int i = 0; i < 2; i++)
        {
          if (!wids[i])
            continue;
          NSRect none = NSZeroRect;
          NSRect *own_r = i == 0 ? &rects[0] : &none, *for_r = i == 1 ? &rects[1] : &none;
          t0 = P_now_ns();
          img = P_cap_cg_window(wids[i], NO);
          report([@"cg-window-" stringByAppendingString:who[i]], img, (P_now_ns() - t0) / 1e6, nil, rects[i], *own_r,
                 *for_r);
          e = nil;
          t0 = P_now_ns();
          img = P_cap_sck_window(wids[i], &e);
          report([@"sck-window-" stringByAppendingString:who[i]], img, (P_now_ns() - t0) / 1e6, e, rects[i], *own_r,
                 *for_r);
          t0 = P_now_ns();
          img = P_cap_cli_window(wids[i], P_out([NSString stringWithFormat:@"shots/p6-cli-window-%@-raw.png", who[i]]));
          report([@"cli-window-" stringByAppendingString:who[i]], img, (P_now_ns() - t0) / 1e6, nil, rects[i], *own_r,
                 *for_r);
        }

      // in-process: renderInContext of our own layer tree (IOSurface contents included?)
      t0 = P_now_ns();
      img = P_cap_layer(own.contentView.layer, own.backingScaleFactor);
      NSRect local = NSMakeRect(0, 0, ownr.size.width, ownr.size.height);
      report(@"layer-render", img, (P_now_ns() - t0) / 1e6, nil, local, local, NSZeroRect);

      [t terminate];
      [own close];
      R(@"done", @"ok");
    }
  return 0;
}

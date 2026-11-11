// A virtual display for the macOS GUI lane (nidhogg/test/nhmac-gui.sh), test-only.
//
//   mac-display --scale 1|2 --ready FILE
//
// The hosted macOS runner has one 1024x768 display at scale 1 and no way to get a Retina one.  This makes a display
// through CoreGraphics' private CGVirtualDisplay class (the one Chromium's tests and DeskPad use): 1440x900 at scale 1,
// or 1280x800 points HiDPI (2560x1600 pixels) at scale 2.  When the display is there it writes
// "LEFT TOP WIDTH HEIGHT SCALE" to FILE (the display's bounds in points, in CoreGraphics' global coordinates, which
// are the `left' and `top' of an Emacs frame on it), then runs until it is killed: the display lives as long as this
// process does.  Exit 1, with the reason on stderr, when it cannot make the display.
//
// Built by nhmac-gui.sh with: clang -fobjc-arc -framework AppKit -framework CoreGraphics -o mac-display mac-display.m

#import <AppKit/AppKit.h>
#import <CoreGraphics/CoreGraphics.h>

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

static void spin(double secs)
{
  // AppKit learns of a new display only when the application's event loop runs (a bare run loop does not)
  NSDate *end = [NSDate dateWithTimeIntervalSinceNow:secs];
  do
    {
      NSEvent *e = [NSApp nextEventMatchingMask:NSEventMaskAny untilDate:end inMode:NSDefaultRunLoopMode
                                        dequeue:YES];
      if (e)
        [NSApp sendEvent:e];
    }
  while ([end timeIntervalSinceNow] > 0);
}

static NSScreen *screen_of(CGDirectDisplayID did)
{
  for (NSScreen *s in [NSScreen screens])
    if ([s.deviceDescription[@"NSScreenNumber"] unsignedIntValue] == did)
      return s;
  return nil;
}

int main(int argc, char **argv)
{
  @autoreleasepool
    {
      int scale = 1;
      const char *ready = NULL;
      for (int i = 1; i + 1 < argc; i += 2)
        {
          if (!strcmp(argv[i], "--scale"))
            scale = atoi(argv[i + 1]);
          else if (!strcmp(argv[i], "--ready"))
            ready = argv[i + 1];
        }
      if ((scale != 1 && scale != 2) || !ready)
        {
          fprintf(stderr, "usage: mac-display --scale 1|2 --ready FILE\n");
          return 2;
        }
      [NSApplication sharedApplication];
      [NSApp setActivationPolicy:NSApplicationActivationPolicyAccessory];
      [NSApp finishLaunching];

      Class D = NSClassFromString(@"CGVirtualDisplayDescriptor"), M = NSClassFromString(@"CGVirtualDisplayMode"),
            S = NSClassFromString(@"CGVirtualDisplaySettings"), V = NSClassFromString(@"CGVirtualDisplay");
      if (!D || !M || !S || !V)
        {
          fprintf(stderr, "mac-display: CGVirtualDisplay is missing\n");
          return 1;
        }
      CGVirtualDisplayDescriptor *d = [[D alloc] init];
      d.queue = dispatch_get_main_queue();
      d.name = scale == 2 ? @"nidhogg test 2x" : @"nidhogg test 1x";
      d.maxPixelsWide = 2560;
      d.maxPixelsHigh = 1600;
      d.sizeInMillimeters = CGSizeMake(286, 179);
      d.vendorID = 0x4e48;
      d.productID = scale;
      d.serialNum = scale;
      CGVirtualDisplay *v = [[V alloc] initWithDescriptor:d];
      if (!v)
        {
          fprintf(stderr, "mac-display: no virtual display\n");
          return 1;
        }
      CGVirtualDisplaySettings *s = [[S alloc] init];
      s.hiDPI = scale == 2 ? 1 : 0;
      s.modes = scale == 2 ? @[[[M alloc] initWithWidth:1280 height:800 refreshRate:60],
                               [[M alloc] initWithWidth:2560 height:1600 refreshRate:60]]
                           : @[[[M alloc] initWithWidth:1440 height:900 refreshRate:60]];
      if (![v applySettings:s])
        {
          fprintf(stderr, "mac-display: applySettings failed\n");
          return 1;
        }
      CGDirectDisplayID did = v.displayID;
      NSScreen *screen = nil;
      for (int i = 0; i < 100 && !(screen = screen_of(did)); i++)
        spin(0.1);
      if (!screen || (int)screen.backingScaleFactor != scale)
        {
          fprintf(stderr, "mac-display: display %u came up %s at scale %.0f, wanted %d\n", did,
                  screen ? "" : "not", screen ? screen.backingScaleFactor : 0.0, scale);
          return 1;
        }
      CGRect b = CGDisplayBounds(did);
      NSString *line = [NSString stringWithFormat:@"%d %d %d %d %d\n", (int)b.origin.x, (int)b.origin.y,
                                                  (int)b.size.width, (int)b.size.height, scale];
      NSString *tmp = [NSString stringWithFormat:@"%s.tmp", ready];
      [line writeToFile:tmp atomically:NO encoding:NSUTF8StringEncoding error:nil];
      rename(tmp.UTF8String, ready);
      for (;;)
        spin(1);
    }
}

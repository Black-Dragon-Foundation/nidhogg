// Q4: backdrop-blur.  Over a root frame of hard vertical stripes:
//   A  a child frame with an NSVisualEffectView in behindWindow mode under its translucent content, rounded by the
//      content layer's mask only, overhanging the root's right edge (so part of it lies over the desktop);
//   B  the same with the effect view's maskImage set to the rounded rect too (does the layer mask clip the blur?);
//   C  a second behindWindow child overlapping A (two blurs on top of each other);
//   K1, K2  withinWindow effect views in the root (window cards), overlapping each other;
//   F  a CALayer with backgroundFilters (CIGaussianBlur), the R37 fallback.
// "detail" is the mean absolute Laplacian of the captured luminance: stripes score high, a blur low.  Then the
// cost: a blurred popup moving at 120 Hz over changing content, and cards over a root repainting every frame.
#import "common.h"
#import <CoreImage/CoreImage.h>

static NSVisualEffectView *effect(NSRect frame, NSVisualEffectBlendingMode mode)
{
  NSVisualEffectView *v = [[NSVisualEffectView alloc] initWithFrame:frame];
  v.blendingMode = mode;
  v.material = NSVisualEffectMaterialPopover;
  v.state = NSVisualEffectStateActive;
  return v;
}

static NSImage *rounded_mask(CGFloat radius)
{
  NSImage *m = [NSImage imageWithSize:NSMakeSize(2 * radius + 1, 2 * radius + 1) flipped:NO
                       drawingHandler:^BOOL(NSRect r) {
                         [[NSColor blackColor] set];
                         [[NSBezierPath bezierPathWithRoundedRect:r xRadius:radius yRadius:radius] fill];
                         return YES;
                       }];
  m.capInsets = NSEdgeInsetsMake(radius, radius, radius, radius);
  m.resizingMode = NSImageResizingModeStretch;
  return m;
}

static ELWindow *blur_child(ELWindow *root, NSRect r, BOOL dark, int pattern, BOOL mask_image)
{
  ELWindow *pop = P_child(root, r, dark, pattern);
  P_round(pop, 10);
  NSVisualEffectView *v = effect(pop.contentView.bounds, NSVisualEffectBlendingModeBehindWindow);
  v.autoresizingMask = NSViewWidthSizable | NSViewHeightSizable;
  if (mask_image)
    v.maskImage = rounded_mask(10);
  [pop.contentView addSubview:v positioned:NSWindowBelow relativeTo:pop.el];
  pop.el.bg = [pop.el.bg colorWithAlphaComponent:0.35];   // alpha-background
  [pop.el paint];
  return pop;
}

static double region_detail(NSRect r)
{
  CGImageRef img = P_cap_cg_rect(r);
  if (!img)
    img = P_cap_cli_rect(r, P_out(@"shots/.p4.png"));
  if (!img)
    return -1;
  size_t w, h;
  uint8_t *px = P_rgba(img, &w, &h);
  double d = P_detail(px, w, 0, 0, w, h);
  free(px);
  CGImageRelease(img);
  return d;
}

static NSRect strip(NSRect pop, CGFloat x0, CGFloat x1)   // a text-free strip near the popup's bottom, global coords
{
  return NSMakeRect(x0, pop.origin.y + 8, x1 - x0, 40);
}

static void scene(NSScreen *s, int scale, BOOL dark)
{
  P_appearance(dark);
  NSString *tag = [NSString stringWithFormat:@"%dx-%@", scale, dark ? @"dark" : @"light"];
  ELWindow *root = P_root(P_at(s, 20, 20, 1000, 600), dark, 1);
  NSRect rc = [root contentRectForFrameRect:root.frame];
  CGFloat X = rc.origin.x, Y = rc.origin.y;
  NSRect rA = NSMakeRect(X + 820, Y + 300, 360, 220), rB = NSMakeRect(X + 520, Y + 40, 280, 220),
         rC = NSMakeRect(X + 700, Y + 380, 300, 160);
  NSRect k1 = NSMakeRect(40, 330, 260, 180), k2 = NSMakeRect(200, 260, 260, 180), kf = NSMakeRect(40, 40, 260, 180);
  NSRect (^g)(NSRect) = ^NSRect(NSRect l) { return NSOffsetRect(l, X, Y); };
  NSRect k12 = NSIntersectionRect(g(k1), g(k2));
  NSDictionary<NSString *, NSValue *> *regions = @{
    @"A-over-root": [NSValue valueWithRect:strip(rA, rA.origin.x + 20, NSMaxX(rc) - 10)],
    @"A-over-desktop": [NSValue valueWithRect:strip(rA, NSMaxX(root.frame) + 10, NSMaxX(rA) - 20)],
    @"B-mask-image": [NSValue valueWithRect:strip(rB, rB.origin.x + 20, NSMaxX(rB) - 20)],
    @"C-over-A": [NSValue valueWithRect:NSInsetRect(NSIntersectionRect(rA, rC), 6, 6)],
    @"K1-only": [NSValue valueWithRect:NSInsetRect(NSMakeRect(X + 50, Y + 450, 140, 50), 0, 0)],
    @"K1-K2-overlap": [NSValue valueWithRect:NSInsetRect(k12, 8, 8)],
    @"F-backgroundFilters": [NSValue valueWithRect:NSInsetRect(g(kf), 12, 12)],
  };
  NSMutableDictionary *before = [NSMutableDictionary dictionary];
  P_spin(0.3);
  for (NSString *k in regions)
    before[k] = @(region_detail(regions[k].rectValue));

  ELWindow *A = blur_child(root, rA, dark, 0, NO);
  A.el.label = @"A: behindWindow, layer mask";
  [A.el paint];
  ELWindow *B = blur_child(root, rB, dark, 0, YES);
  B.el.label = @"B: behindWindow + maskImage";
  [B.el paint];
  ELWindow *C = blur_child(root, rC, dark, 2, NO);

  NSVisualEffectView *c1 = effect(k1, NSVisualEffectBlendingModeWithinWindow);
  NSVisualEffectView *c2 = effect(k2, NSVisualEffectBlendingModeWithinWindow);
  for (NSVisualEffectView *c in @[c1, c2])
    {
      c.wantsLayer = YES;
      c.layer.cornerRadius = 10;
      c.layer.masksToBounds = YES;
      [root.contentView addSubview:c positioned:NSWindowAbove relativeTo:nil];
    }
  root.contentView.layerUsesCoreImageFilters = YES;
  CALayer *f = [CALayer layer];
  f.frame = NSRectToCGRect(kf);
  f.cornerRadius = 10;
  f.masksToBounds = YES;
  CIFilter *gauss = [CIFilter filterWithName:@"CIGaussianBlur" withInputParameters:@{kCIInputRadiusKey: @10}];
  f.backgroundFilters = gauss ? @[gauss] : @[];
  [root.contentView.layer addSublayer:f];
  P_spin(0.8);

  for (NSString *k in [regions.allKeys sortedArrayUsingSelector:@selector(compare:)])
    {
      double after = region_detail(regions[k].rectValue), b = [before[k] doubleValue];
      R([NSString stringWithFormat:@"%@.%@", tag, k], @"detail before %.2f, with the effect %.2f (%@)", b, after,
        b > 0 && after >= 0 && after < 0.35 * b ? @"BLURRED" : @"not blurred");
    }
  P_shot([@"p4-scene-" stringByAppendingString:tag], NSInsetRect(NSUnionRect(root.frame, rA), -20, -20));
  P_shot([@"p4-A-overhang-" stringByAppendingString:tag], NSInsetRect(rA, -20, -20));
  P_shot([@"p4-B-mask-image-" stringByAppendingString:tag], NSInsetRect(rB, -20, -20));
  P_shot([@"p4-cards-" stringByAppendingString:tag], NSInsetRect(NSUnionRect(g(k1), g(k2)), -20, -20));
  P_shot([@"p4-F-backgroundFilters-" stringByAppendingString:tag], NSInsetRect(g(kf), -20, -20));

  if (!dark)
    {
      const double hz = 120;
      // cost 1: A moving at 120 Hz while the root repaints every 4th frame, against an opaque popup without blur
      for (int blur = 0; blur <= 1; blur++)
        {
          ELWindow *p = blur ? A : P_child(root, rA, dark, 0);
          if (!blur)
            {
              [A orderOut:nil];
              [B orderOut:nil];
              [C orderOut:nil];
            }
          else
            {
              P_show(A, root);
              [p setFrame:rA display:NO];
            }
          P_spin(0.3);
          int n = 240;
          double c0 = P_cpu_self(), w0 = P_cpu_ws();
          uint64_t start = P_now_ns();
          for (int i = 0; i < n; i++)
            {
              P_frame_wait(start, i, hz);
              double a = i * 2 * M_PI / 120;
              P_move(p, NSMakePoint(rA.origin.x - 300 + 100 * cos(a), rA.origin.y - 100 + 100 * sin(a)));
              if (i % 4 == 0)
                {
                  root.el.generation = i;
                  [root.el paint];
                }
              [CATransaction flush];
            }
          double secs = (P_now_ns() - start) / 1e9;
          R([NSString stringWithFormat:@"%@.cost.move-%@", tag, blur ? @"blurred-popup" : @"plain-popup"],
            @"per frame: self %.3f ms, WindowServer %.3f ms CPU (%d frames in %.2f s)",
            (P_cpu_self() - c0) * 1e3 / n, (P_cpu_ws() - w0) * 1e3 / n, n, secs);
          if (!blur)
            [p close];
        }
      [A orderOut:nil];
      // cost 2: the root repainting every frame (typing, scrolling) with the withinWindow cards and F, then without
      for (int cards = 1; cards >= 0; cards--)
        {
          c1.hidden = c2.hidden = !cards;
          f.hidden = !cards;
          P_spin(0.2);
          int n = 240;
          double c0 = P_cpu_self(), w0 = P_cpu_ws();
          uint64_t start = P_now_ns();
          for (int i = 0; i < n; i++)
            {
              P_frame_wait(start, i, hz);
              root.el.generation = i;
              [root.el paint];
              [CATransaction flush];
            }
          R([NSString stringWithFormat:@"%@.cost.repaint-%@", tag, cards ? @"with-cards" : @"without-cards"],
            @"per frame: self %.3f ms, WindowServer %.3f ms CPU", (P_cpu_self() - c0) * 1e3 / n,
            (P_cpu_ws() - w0) * 1e3 / n);
        }
    }
  [A close];
  [B close];
  [C close];
  [root close];
  P_spin(0.2);
}

int main(void)
{
  @autoreleasepool
    {
      P_init("p4-blur");
      for (int scale = 1; scale <= 2; scale++)
        {
          NSScreen *s = P_screen(scale);
          if (!s)
            {
              R([NSString stringWithFormat:@"%dx", scale], @"skipped: no screen at this scale");
              continue;
            }
          scene(s, scale, NO);
          scene(s, scale, YES);
        }
      R(@"done", @"ok");
    }
  return 0;
}

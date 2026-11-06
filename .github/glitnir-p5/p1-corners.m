// Q1: corner-radius.  A stock child frame (opaque, square: the control) beside the same child made transparent with a
// rounded, masked, continuous-curve content layer (the test).  Both show the same IOSurface text, so their window
// captures must match pixel for pixel away from the corners if rounding keeps the text as sharp as stock.
#import "common.h"

static void compare(NSString *tag, ELWindow *ctl, ELWindow *tst, CGFloat radius)
{
  CGImageRef a = P_cap_cg_window((CGWindowID)ctl.windowNumber, NO);
  CGImageRef b = P_cap_cg_window((CGWindowID)tst.windowNumber, NO);
  NSString *route = @"CGWindowListCreateImage";
  if (!a || !b)
    {
      if (a)
        CGImageRelease(a);
      if (b)
        CGImageRelease(b);
      a = P_cap_cli_window((CGWindowID)ctl.windowNumber, P_out(@"shots/.p1-a.png"));
      b = P_cap_cli_window((CGWindowID)tst.windowNumber, P_out(@"shots/.p1-b.png"));
      route = @"screencapture -l";
    }
  if (!a || !b)
    {
      R([tag stringByAppendingString:@".sharp"], @"no window capture route worked");
      if (a)
        CGImageRelease(a);
      if (b)
        CGImageRelease(b);
      return;
    }
  size_t aw, ah, bw, bh;
  uint8_t *pa = P_rgba(a, &aw, &ah), *pb = P_rgba(b, &bw, &bh);
  double px_per_pt = aw / ctl.frame.size.width;
  R([tag stringByAppendingString:@".capture"], @"%@: control %zux%zu, rounded %zux%zu px (%.2f px/pt)", route, aw,
    ah, bw, bh, px_per_pt);
  if (aw == bw && ah == bh)
    {
      size_t m = (size_t)ceil((radius + 2) * px_per_pt);   // stay clear of the corners
      double mean;
      int max;
      P_diff(pa, pb, aw, m, m, aw - 2 * m, ah - 2 * m, &mean, &max);
      double da = P_detail(pa, aw, m, m, aw - 2 * m, ah - 2 * m), db = P_detail(pb, bw, m, m, bw - 2 * m, bh - 2 * m);
      R([tag stringByAppendingString:@".sharp"], @"%@: text area differs from stock by mean %.3f max %d (0-255); "
                                                 @"detail stock %.2f rounded %.2f",
        mean < 0.5 && max <= 16 ? @"SAME AS STOCK" : @"DIFFERENT", mean, max, da, db);
      // corner pixel: transparent in the rounded window, opaque in the stock one
      R([tag stringByAppendingString:@".corner-alpha"], @"rounded (1,1) a=%d centre a=%d; stock (1,1) a=%d",
        pb[(1 * bw + 1) * 4 + 3], pb[((bh / 2) * bw + bw / 2) * 4 + 3], pa[(1 * aw + 1) * 4 + 3]);
      // where the corner arc is: first opaque pixel along the top row and along the diagonal
      size_t x = 0;
      while (x < bw / 2 && pb[x * 4 + 3] < 128)
        x++;
      size_t d = 0;
      while (d < bh / 2 && pb[(d * bw + d) * 4 + 3] < 128)
        d++;
      R([tag stringByAppendingString:@".corner-shape"],
        @"top row opaque from x=%zu px, diagonal from %zu px (radius %.0f pt = %.0f px; a circle gives %.0f, a "
        @"continuous curve more)",
        x, d, radius, radius * px_per_pt, radius * px_per_pt * (1 - M_SQRT1_2));
    }
  free(pa);
  free(pb);
  CGImageRelease(a);
  CGImageRelease(b);
}

int main(void)
{
  @autoreleasepool
    {
      P_init("p1-corners");
      const CGFloat radius = 10;
      for (int scale = 1; scale <= 2; scale++)
        {
          NSScreen *s = P_screen(scale);
          if (!s)
            {
              R([NSString stringWithFormat:@"%dx", scale], @"skipped: no screen at this scale");
              continue;
            }
          for (int dark = 0; dark <= 1; dark++)
            {
              P_appearance(dark);
              NSString *tag = [NSString stringWithFormat:@"%dx-%@", scale, dark ? @"dark" : @"light"];
              ELWindow *root = P_root(P_at(s, 30, 30, 900, 560), dark, 0);
              NSRect rc = [root contentRectForFrameRect:root.frame];
              NSRect r1 = NSMakeRect(rc.origin.x + 40, rc.origin.y + 160, 380, 220);
              NSRect r2 = NSMakeRect(rc.origin.x + 480, rc.origin.y + 160, 380, 220);
              ELWindow *ctl = P_child(root, r1, dark, 0);
              ELWindow *tst = P_child(root, r2, dark, 0);
              P_round(tst, radius);
              [ctl.el paint];
              [tst.el paint];
              P_spin(0.5);
              P_shot([@"p1-corners-" stringByAppendingString:tag], NSInsetRect(NSUnionRect(r1, r2), -30, -30));
              P_shot([@"p1-corner-zoom-" stringByAppendingString:tag],
                     NSMakeRect(NSMinX(r2) - 8, NSMaxY(r2) - 32, 40, 40));
              P_shot_window([@"p1-rounded-window-" stringByAppendingString:tag], tst, NO);
              // on screen: the root's colour must show through the rounded corner
              CGImageRef img = P_cap_cg_rect(NSMakeRect(NSMinX(r2), NSMaxY(r2) - 4, 4, 4));
              if (img)
                {
                  size_t w, h;
                  uint8_t *px = P_rgba(img, &w, &h);
                  R([tag stringByAppendingString:@".corner-on-screen"], @"top-left pixel rgb %d,%d,%d (root bg %@)",
                    px[0], px[1], px[2], dark ? @"30,31,34" : @"238,238,234");
                  free(px);
                  CGImageRelease(img);
                }
              compare(tag, ctl, tst, radius);
              [tst close];
              [ctl close];
              [root close];
              P_spin(0.2);
            }
        }
      // The in-process route at 2x whatever the screens are: the rounded layer tree rendered by renderInContext.
      P_appearance(NO);
      NSScreen *s1 = [NSScreen screens][0];
      ELWindow *root = P_root(P_at(s1, 30, 30, 500, 360), NO, 0);
      NSRect rc = [root contentRectForFrameRect:root.frame];
      ELWindow *tst = P_child(root, NSMakeRect(rc.origin.x + 40, rc.origin.y + 60, 380, 220), NO, 0);
      P_round(tst, radius);
      P_spin(0.3);
      CGImageRef img = P_cap_layer(tst.contentView.layer, 2);
      R(@"offscreen-2x", @"renderInContext at 2x: %@", P_save_png(img, P_out(@"shots/p1-offscreen-2x.png"))
                                                            ? @"saved shots/p1-offscreen-2x.png"
                                                            : @"failed");
      if (img)
        CGImageRelease(img);
      [tst close];
      [root close];
      R(@"done", @"ok");
    }
  return 0;
}

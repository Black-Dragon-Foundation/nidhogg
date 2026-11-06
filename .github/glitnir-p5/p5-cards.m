// Q5: window cards (P4).  Two Emacs windows side by side in one root frame, drawn by the frame into its one IOSurface
// (gap colour around them); the chrome is sublayers of the root view's layer above that content: per card a corner
// mask (the gap colour filling the rect minus the rounded rect), a shadow layer with shadowPath masked to the gap (so
// it never darkens the card itself) and a 1 px ring.  Then the cost: the divider moving for 120 frames, content
// repainted and paths updated in one transaction, against the repaint alone.
#import "common.h"

@interface Card : NSObject
@property (nonatomic, strong) CAShapeLayer *corners, *mask, *ring;
@property (nonatomic, strong) CALayer *shadow;
@end
@implementation Card
@end

static Card *card_layers(CALayer *host, NSColor *gap, PShadow spec, BOOL dark)
{
  Card *c = [Card new];
  c.corners = [CAShapeLayer layer];
  c.corners.fillRule = kCAFillRuleEvenOdd;
  c.corners.fillColor = gap.CGColor;
  c.shadow = [CALayer layer];
  P_shadow_layer(c.shadow, spec);
  c.mask = [CAShapeLayer layer];
  c.mask.fillRule = kCAFillRuleEvenOdd;
  c.shadow.mask = c.mask;
  c.ring = [CAShapeLayer layer];
  c.ring.fillColor = NULL;
  c.ring.lineWidth = 1;
  c.ring.strokeColor = (dark ? P_rgb(0xffffff, 0.10) : P_rgb(0x000000, 0.10)).CGColor;
  for (CALayer *l in @[c.corners, c.shadow, c.ring])
    [host addSublayer:l];
  return c;
}

static void card_fit(Card *c, CGRect bounds, CGRect r, CGFloat radius)
{
  CGPathRef corners = P_ring_path(r, r, radius);   // rect minus rounded rect: the four corners
  c.corners.frame = bounds;
  c.corners.path = corners;
  CGPathRelease(corners);
  c.shadow.frame = bounds;
  CGPathRef sp = CGPathCreateWithRoundedRect(r, radius, radius, NULL);
  c.shadow.shadowPath = sp;
  c.mask.frame = bounds;
  CGPathRef gapmask = P_ring_path(bounds, r, radius);
  c.mask.path = gapmask;
  CGPathRelease(gapmask);
  c.ring.frame = bounds;
  CGPathRef rp = CGPathCreateWithRoundedRect(CGRectInset(r, 0.5, 0.5), radius - 0.5, radius - 0.5, NULL);
  c.ring.path = rp;
  CGPathRelease(rp);
  CGPathRelease(sp);
}

static void layout(ELWindow *root, NSArray<Card *> *cards, CGFloat divider)
{
  NSSize sz = root.el.bounds.size;
  CGFloat gap = 12;
  NSRect a = NSMakeRect(gap, gap, divider - 1.5 * gap, sz.height - 2 * gap);
  NSRect b = NSMakeRect(divider + gap / 2, gap, sz.width - divider - 1.5 * gap, sz.height - 2 * gap);
  [CATransaction begin];
  [CATransaction setDisableActions:YES];
  root.el.cards = @[[NSValue valueWithRect:a], [NSValue valueWithRect:b]];
  [root.el paint];
  if (cards.count == 2)
    {
      CGRect bounds = NSRectToCGRect(root.contentView.bounds);
      card_fit(cards[0], bounds, NSRectToCGRect(a), 10);
      card_fit(cards[1], bounds, NSRectToCGRect(b), 10);
    }
  [CATransaction commit];
}

static void scene(NSScreen *s, int scale, BOOL dark)
{
  P_appearance(dark);
  NSString *tag = [NSString stringWithFormat:@"%dx-%@", scale, dark ? @"dark" : @"light"];
  ELWindow *root = P_root(P_at(s, 20, 20, 960, 560), dark, 0);
  NSColor *gap = P_rgb(dark ? 0x17181a : 0xe4e4e0, 1);
  root.el.bg = gap;
  root.backgroundColor = gap;
  for (int look = 0; look < 2; look++)
    {
      PShadow spec = look ? P_MOCKUP : P_DEFAULT;
      NSMutableArray<Card *> *cards = [NSMutableArray array];
      for (int i = 0; i < 2; i++)
        [cards addObject:card_layers(root.contentView.layer, gap, spec, dark)];
      layout(root, cards, 480);
      P_spin(0.4);
      P_shot([NSString stringWithFormat:@"p5-cards-%s-%@", spec.name, tag],
             [root contentRectForFrameRect:root.frame]);
      NSRect rc = [root contentRectForFrameRect:root.frame];
      P_shot([NSString stringWithFormat:@"p5-card-corner-%s-%@", spec.name, tag],
             NSMakeRect(rc.origin.x + 440, NSMaxY(rc) - 70, 80, 70));
      if (!dark && look == 0)
        {
          // cost: the divider sweeping 120 frames at 120 Hz with the chrome, then without
          for (int chrome = 1; chrome >= 0; chrome--)
            {
              for (Card *c in cards)
                c.corners.hidden = c.shadow.hidden = c.ring.hidden = !chrome;
              int n = 120;
              double *ms = calloc(n, sizeof *ms);
              double c0 = P_cpu_self(), w0 = P_cpu_ws();
              uint64_t start = P_now_ns();
              for (int i = 0; i < n; i++)
                {
                  P_frame_wait(start, i, 120);
                  uint64_t t0 = P_now_ns();
                  root.el.generation = i;
                  layout(root, chrome ? (NSArray<Card *> *)cards : @[], 480 + 200 * sin(i * 2 * M_PI / 60));
                  [CATransaction flush];
                  ms[i] = (P_now_ns() - t0) / 1e6;
                }
              R([NSString stringWithFormat:@"%@.cost.%@", tag, chrome ? @"with-chrome" : @"repaint-only"],
                @"main %@ ms; per frame self %.3f ms, WindowServer %.3f ms CPU", P_stats(ms, n),
                (P_cpu_self() - c0) * 1e3 / n, (P_cpu_ws() - w0) * 1e3 / n);
              free(ms);
            }
          for (Card *c in cards)
            c.corners.hidden = c.shadow.hidden = c.ring.hidden = NO;
          layout(root, cards, 480);
        }
      for (Card *c in cards)
        for (CALayer *l in @[c.corners, c.shadow, c.ring])
          [l removeFromSuperlayer];
    }
  // a child frame over a card: its own shadow window must fall over the card chrome, not under it
  NSRect rc = [root contentRectForFrameRect:root.frame];
  NSMutableArray<Card *> *cards = [NSMutableArray array];
  for (int i = 0; i < 2; i++)
    [cards addObject:card_layers(root.contentView.layer, gap, P_DEFAULT, dark)];
  layout(root, cards, 480);
  ELWindow *pop = P_child(root, NSMakeRect(rc.origin.x + 380, rc.origin.y + 220, 300, 160), dark, 0);
  P_round(pop, 10);
  PShadowWin *sw = [[PShadowWin alloc] initWithSpec:P_DEFAULT radius:10];
  [sw fitTo:pop.frame];
  [pop addChildWindow:sw.win ordered:NSWindowBelow];
  P_spin(0.4);
  P_shot([@"p5-cards-with-popup-" stringByAppendingString:tag], rc);
  [sw.win close];
  [pop close];
  [root close];
  P_spin(0.2);
}

int main(void)
{
  @autoreleasepool
    {
      P_init("p5-cards");
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

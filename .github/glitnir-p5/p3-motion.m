// Q3: alpha and transitions with glitnir.el's curves: show over 0.12 s with ease-out, hide over 0.08 s with ease-in,
// pop from scale 0.96 (motion.rs's POP_SCALE).  Route 1: the window's alphaValue through NSAnimationContext (what
// nsterm.m's `alpha' parameter already sets), read back as the window server's own alpha every millisecond, with the
// shadow window (option b) animated in the same group to see whether the pair stays in step.  Route 2: the content
// layer's opacity and transform through CABasicAnimation, read from the presentation layer.  Each is compared with
// the cubic bezier; the cost is this process's and WindowServer's CPU over repeated show/hide cycles.
#import "common.h"
#include <pthread.h>
#include <stdatomic.h>

#define MAXS 4000
static struct
{
  CGWindowID ids[2];
  int n;
  _Atomic int run;
  int count;
  uint64_t t[MAXS];
  double a[2][MAXS];
} S;

static void *alpha_loop(void *arg)
{
  (void)arg;
  while (atomic_load(&S.run) && S.count < MAXS)
    @autoreleasepool
    {
      double a[2];
      P_ws(S.ids, S.n, NULL, NULL, a);
      S.t[S.count] = P_now_ns();
      S.a[0][S.count] = a[0];
      S.a[1][S.count] = S.n > 1 ? a[1] : -1;
      S.count++;
      usleep(1000);
    }
  return NULL;
}

static pthread_t sample_start(CGWindowID a, CGWindowID b)
{
  S.ids[0] = a;
  S.ids[1] = b;
  S.n = b ? 2 : 1;
  S.count = 0;
  atomic_store(&S.run, 1);
  pthread_t t;
  pthread_create(&t, NULL, alpha_loop, NULL);
  return t;
}

static void sample_stop(pthread_t t)
{
  atomic_store(&S.run, 0);
  pthread_join(t, NULL);
}

typedef struct { double x1, y1, x2, y2; } Curve;
static const Curve EASE_OUT = {0, 0, 0.58, 1}, EASE_IN = {0.42, 0, 1, 1};

// Best lag (0-60 ms) and the max/mean error of samples (t, v) against from + (to - from) * ease((t - t0 - lag) / d).
static NSString *fit(const uint64_t *t, const double *v, int n, uint64_t t0, double d, double from, double to, Curve c)
{
  double best_max = 1e9, best_mean = 0;
  int best_lag = 0, used = 0;
  for (int lag = 0; lag <= 60; lag++)
    {
      double mx = 0, sum = 0;
      int k = 0;
      for (int i = 0; i < n; i++)
        {
          if (v[i] < 0)
            continue;
          double x = ((double)t[i] - (double)t0) / 1e9 - lag / 1e3;
          if (x < -0.02 || x > d + 0.05)
            continue;
          double want = from + (to - from) * P_ease(c.x1, c.y1, c.x2, c.y2, x / d);
          double e = fabs(v[i] - want);
          mx = e > mx ? e : mx;
          sum += e;
          k++;
        }
      if (k && mx < best_max)
        best_max = mx, best_mean = sum / k, best_lag = lag, used = k;
    }
  return [NSString stringWithFormat:@"lag %d ms, error max %.3f mean %.3f over %d samples", best_lag, best_max,
                                    best_mean, used];
}

static double max_jump(const double *v, int n)
{
  double mx = 0;
  for (int i = 1; i < n; i++)
    if (v[i] >= 0 && v[i - 1] >= 0 && fabs(v[i] - v[i - 1]) > mx)
      mx = fabs(v[i] - v[i - 1]);
  return mx;
}

static NSString *distinct_steps(const double *v, int n)
{
  int steps = 0;
  for (int i = 1; i < n; i++)
    if (v[i] != v[i - 1])
      steps++;
  return [NSString stringWithFormat:@"%d value changes", steps];
}

static void fade(NSWindow *w, NSWindow *w2, double to, double secs, Curve c)
{
  [NSAnimationContext runAnimationGroup:^(NSAnimationContext *ctx) {
    ctx.duration = secs;
    ctx.timingFunction = [CAMediaTimingFunction functionWithControlPoints:c.x1 :c.y1 :c.x2 :c.y2];
    w.animator.alphaValue = to;
    if (w2)
      w2.animator.alphaValue = to;
  } completionHandler:nil];
}

static void window_alpha(NSScreen *s, NSString *tag)
{
  ELWindow *root = P_root(P_at(s, 20, 20, 900, 560), NO, 0);
  NSRect rc = [root contentRectForFrameRect:root.frame];
  ELWindow *pop = P_child(root, NSMakeRect(rc.origin.x + 200, rc.origin.y + 200, 360, 220), NO, 0);
  P_round(pop, 10);
  PShadowWin *sw = [[PShadowWin alloc] initWithSpec:P_MOCKUP radius:10];
  [sw fitTo:pop.frame];
  [pop addChildWindow:sw.win ordered:NSWindowBelow];
  pop.alphaValue = 0;
  sw.win.alphaValue = 0;
  P_spin(0.3);

  // show: 0 -> 1 over 0.12 s, ease-out
  pthread_t th = sample_start((CGWindowID)pop.windowNumber, (CGWindowID)sw.win.windowNumber);
  P_spin(0.02);
  uint64_t t0 = P_now_ns();
  fade(pop, sw.win, 1, 0.12, EASE_OUT);
  uint64_t call = P_now_ns() - t0;
  P_spin(0.3);
  sample_stop(th);
  R([tag stringByAppendingString:@".window-alpha.show"], @"%@; %@; start call %.3f ms",
    fit(S.t, S.a[0], S.count, t0, 0.12, 0, 1, EASE_OUT), distinct_steps(S.a[0], S.count), call / 1e6);
  double dmax = 0;
  for (int i = 0; i < S.count; i++)
    if (S.a[1][i] >= 0 && fabs(S.a[0][i] - S.a[1][i]) > dmax)
      dmax = fabs(S.a[0][i] - S.a[1][i]);
  R([tag stringByAppendingString:@".window-alpha.shadow-in-step"], @"max |popup - shadow window| alpha %.3f", dmax);

  // hide: 1 -> 0 over 0.08 s, ease-in
  th = sample_start((CGWindowID)pop.windowNumber, 0);
  P_spin(0.02);
  t0 = P_now_ns();
  fade(pop, sw.win, 0, 0.08, EASE_IN);
  P_spin(0.3);
  sample_stop(th);
  R([tag stringByAppendingString:@".window-alpha.hide"], @"%@",
    fit(S.t, S.a[0], S.count, t0, 0.08, 1, 0, EASE_IN));

  // a show 40 ms into a hide: it must continue from where the hide got to, with no jump
  pop.alphaValue = 1;
  sw.win.alphaValue = 1;
  P_spin(0.2);
  th = sample_start((CGWindowID)pop.windowNumber, 0);
  P_spin(0.02);
  fade(pop, sw.win, 0, 0.08, EASE_IN);
  P_spin(0.04);
  fade(pop, sw.win, 1, 0.12, EASE_OUT);
  P_spin(0.3);
  sample_stop(th);
  double lo = 1;
  for (int i = 0; i < S.count; i++)
    if (S.a[0][i] >= 0 && S.a[0][i] < lo)
      lo = S.a[0][i];
  R([tag stringByAppendingString:@".window-alpha.interrupted"],
    @"lowest alpha %.3f, largest step between 1 ms samples %.3f, final %.3f", lo, max_jump(S.a[0], S.count),
    S.count ? S.a[0][S.count - 1] : -1);

  // the look at mid-fade (a still, for the record)
  pop.alphaValue = 0.5;
  sw.win.alphaValue = 0.5;
  P_spin(0.2);
  P_shot([@"p3-mid-fade-" stringByAppendingString:tag], NSInsetRect(pop.frame, -60, -110));

  // cost: 30 show/hide cycles
  pop.alphaValue = 0;
  sw.win.alphaValue = 0;
  P_spin(0.2);
  double c0 = P_cpu_self(), w0 = P_cpu_ws();
  uint64_t start = P_now_ns();
  for (int i = 0; i < 30; i++)
    {
      fade(pop, sw.win, 1, 0.12, EASE_OUT);
      P_spin(0.15);
      fade(pop, sw.win, 0, 0.08, EASE_IN);
      P_spin(0.11);
    }
  double secs = (P_now_ns() - start) / 1e9;
  R([tag stringByAppendingString:@".window-alpha.cost"],
    @"30 cycles in %.2f s: self %.1f ms CPU, WindowServer %.1f ms CPU (%.2f / %.2f ms per 8.33 ms frame)", secs,
    (P_cpu_self() - c0) * 1e3, (P_cpu_ws() - w0) * 1e3, (P_cpu_self() - c0) * 1e3 / (secs * 120),
    (P_cpu_ws() - w0) * 1e3 / (secs * 120));
  [sw.win close];
  [pop close];
  [root close];
  P_spin(0.2);
}

static void add_pop(CALayer *l, BOOL show, double secs, Curve c)
{
  CGSize sz = l.bounds.size;
  CGFloat s0 = 0.96;
  CATransform3D small = CATransform3DMakeScale(s0, s0, 1);   // about the centre (the layer's anchor is 0,0)
  small.m41 = sz.width / 2 * (1 - s0);
  small.m42 = sz.height / 2 * (1 - s0);
  CABasicAnimation *o = [CABasicAnimation animationWithKeyPath:@"opacity"];
  o.fromValue = @(show ? 0.0 : 1.0);
  o.toValue = @(show ? 1.0 : 0.0);
  CABasicAnimation *t = [CABasicAnimation animationWithKeyPath:@"transform"];
  t.fromValue = [NSValue valueWithCATransform3D:show ? small : CATransform3DIdentity];
  t.toValue = [NSValue valueWithCATransform3D:CATransform3DIdentity];
  CAAnimationGroup *g = [CAAnimationGroup animation];
  g.animations = show ? @[o, t] : @[o];   // the hide is a fade (glitnir.el's `pop' pops in, fades out)
  g.duration = secs;
  g.timingFunction = [CAMediaTimingFunction functionWithControlPoints:c.x1 :c.y1 :c.x2 :c.y2];
  [CATransaction begin];
  [CATransaction setDisableActions:YES];
  l.opacity = show ? 1 : 0;
  l.transform = CATransform3DIdentity;
  [l addAnimation:g forKey:@"p5-pop"];
  [CATransaction commit];
}

static void layer_pop(NSScreen *s, NSString *tag)
{
  ELWindow *root = P_root(P_at(s, 20, 20, 900, 560), NO, 0);
  NSRect rc = [root contentRectForFrameRect:root.frame];
  ELWindow *pop = P_child(root, NSMakeRect(rc.origin.x + 200, rc.origin.y + 200, 360, 220), NO, 0);
  P_round(pop, 10);
  pop.hasShadow = YES;
  CALayer *l = pop.contentView.layer;
  P_spin(0.3);

  int n = 0;
  uint64_t *t = calloc(MAXS, sizeof *t);
  double *op = calloc(MAXS, sizeof *op), *sc = calloc(MAXS, sizeof *sc);
  uint64_t t0 = P_now_ns();
  add_pop(l, YES, 0.12, EASE_OUT);
  uint64_t call = P_now_ns() - t0;
  while (P_now_ns() - t0 < 250000000ull && n < MAXS)
    {
      P_spin(0.001);
      CALayer *p = l.presentationLayer;
      t[n] = P_now_ns();
      op[n] = p ? p.opacity : -1;
      sc[n] = p ? p.transform.m11 : -1;
      n++;
    }
  R([tag stringByAppendingString:@".layer-pop.opacity"], @"%@; start call %.3f ms",
    fit(t, op, n, t0, 0.12, 0, 1, EASE_OUT), call / 1e6);
  R([tag stringByAppendingString:@".layer-pop.scale"], @"%@ (scale 0.96 -> 1; the error is in scale units)",
    fit(t, sc, n, t0, 0.12, 0.96, 1, EASE_OUT));
  P_spin(0.2);
  R([tag stringByAppendingString:@".layer-pop.end-state"], @"model opacity %.2f scale %.3f; AppKit kept them: %@",
    l.opacity, l.transform.m11, l.opacity == 1 && l.transform.m11 == 1 ? @"yes" : @"NO");

  // the look 40 % into the pop (a still)
  double e = P_ease(EASE_OUT.x1, EASE_OUT.y1, EASE_OUT.x2, EASE_OUT.y2, 0.4);
  CGFloat s1 = 0.96 + 0.04 * e;
  CATransform3D m = CATransform3DMakeScale(s1, s1, 1);
  m.m41 = l.bounds.size.width / 2 * (1 - s1);
  m.m42 = l.bounds.size.height / 2 * (1 - s1);
  [CATransaction begin];
  [CATransaction setDisableActions:YES];
  l.opacity = e;
  l.transform = m;
  [CATransaction commit];
  [pop invalidateShadow];
  P_spin(0.2);
  P_shot([@"p3-mid-pop-" stringByAppendingString:tag], NSInsetRect(pop.frame, -60, -110));
  [CATransaction begin];
  [CATransaction setDisableActions:YES];
  l.opacity = 1;
  l.transform = CATransform3DIdentity;
  [CATransaction commit];

  // cost: 30 pop-in/fade-out cycles
  double c0 = P_cpu_self(), w0 = P_cpu_ws();
  uint64_t start = P_now_ns();
  for (int i = 0; i < 30; i++)
    {
      add_pop(l, YES, 0.12, EASE_OUT);
      P_spin(0.15);
      add_pop(l, NO, 0.08, EASE_IN);
      P_spin(0.11);
    }
  double secs = (P_now_ns() - start) / 1e9;
  R([tag stringByAppendingString:@".layer-pop.cost"],
    @"30 cycles in %.2f s: self %.1f ms CPU, WindowServer %.1f ms CPU (%.2f / %.2f ms per 8.33 ms frame)", secs,
    (P_cpu_self() - c0) * 1e3, (P_cpu_ws() - w0) * 1e3, (P_cpu_self() - c0) * 1e3 / (secs * 120),
    (P_cpu_ws() - w0) * 1e3 / (secs * 120));
  free(t);
  free(op);
  free(sc);
  [pop close];
  [root close];
  P_spin(0.2);
}

int main(void)
{
  @autoreleasepool
    {
      P_init("p3-motion");
      R(@"reduce-motion", @"accessibilityDisplayShouldReduceMotion = %d",
        [[NSWorkspace sharedWorkspace] accessibilityDisplayShouldReduceMotion]);
      P_appearance(NO);
      for (int scale = 1; scale <= 2; scale++)
        {
          NSScreen *s = P_screen(scale);
          if (!s)
            {
              R([NSString stringWithFormat:@"%dx", scale], @"skipped: no screen at this scale");
              continue;
            }
          NSString *tag = [NSString stringWithFormat:@"%dx", scale];
          window_alpha(s, tag);
          layer_pop(s, tag);
        }
      R(@"done", @"ok");
    }
  return 0;
}

// Q2: the shadow.  (a) the system shadow: hasShadow YES on the rounded undecorated child; (b) a shadow window under
// the child, a clear, click-through window whose layer draws the CSS-like shadow through shadowPath, masked to the
// ring outside the child (so a translucent child does not show its own shadow through itself).  (b) is placed two
// ways: b1 a child window of the popup ordered below it (AppKit moves it with the popup), b2 a sibling child of the
// root frame that Nidhogg would move itself.  Screenshots side by side on light and dark; then, per placement, what
// move, resize, raise and hide/show do to the pair (the window server's own bounds and order, sampled on a thread
// every millisecond), and the cost per frame against no shadow and the system shadow.
#import "common.h"
#include <pthread.h>
#include <stdatomic.h>

static const CGFloat RADIUS = 10;

// ---------------------------------------------------------------- the sampler thread

typedef struct
{
  CGWindowID pop, sh;
  CGFloat m;
  _Atomic int run;
  long samples, missing, bad_pos, bad_size, bad_order;
} Sampler;

static void *sample_loop(void *arg)
{
  Sampler *s = arg;
  while (atomic_load(&s->run))
    @autoreleasepool
    {
      CGWindowID ids[2] = {s->pop, s->sh};
      int idx[2];
      CGRect b[2];
      P_ws(ids, 2, idx, b, NULL);
      if (idx[0] < 0 || idx[1] < 0)
        s->missing++;
      else
        {
          s->samples++;
          if (fabs(b[1].origin.x - (b[0].origin.x - s->m)) > 0.5 || fabs(b[1].origin.y - (b[0].origin.y - s->m)) > 0.5)
            s->bad_pos++;
          if (fabs(b[1].size.width - (b[0].size.width + 2 * s->m)) > 0.5
              || fabs(b[1].size.height - (b[0].size.height + 2 * s->m)) > 0.5)
            s->bad_size++;
          if (idx[1] != idx[0] + 1)
            s->bad_order++;
        }
      usleep(1000);
    }
  return NULL;
}

static pthread_t sampler_start(Sampler *s, ELWindow *pop, PShadowWin *sw)
{
  memset(s, 0, sizeof *s);
  s->pop = (CGWindowID)pop.windowNumber;
  s->sh = (CGWindowID)sw.win.windowNumber;
  s->m = sw.margin;
  atomic_store(&s->run, 1);
  pthread_t t;
  pthread_create(&t, NULL, sample_loop, s);
  return t;
}

static void sampler_stop(Sampler *s, pthread_t t, NSString *key)
{
  atomic_store(&s->run, 0);
  pthread_join(t, NULL);
  R(key, @"%ld samples: position off in %ld, size off in %ld, not directly below in %ld; one missing in %ld",
    s->samples, s->bad_pos, s->bad_size, s->bad_order, s->missing);
}

// The pair as the window server has it now: "ok" or what is wrong.
static NSString *pair_state(ELWindow *pop, PShadowWin *sw)
{
  CGWindowID ids[2] = {(CGWindowID)pop.windowNumber, (CGWindowID)sw.win.windowNumber};
  int idx[2];
  CGRect b[2];
  P_ws(ids, 2, idx, b, NULL);
  if (idx[0] < 0 && idx[1] < 0)
    return @"both hidden";
  if (idx[0] < 0)
    return @"SHADOW LEFT ON SCREEN";
  if (idx[1] < 0)
    return @"SHADOW MISSING";
  CGFloat m = sw.margin;
  if (fabs(b[1].origin.x - (b[0].origin.x - m)) > 0.5 || fabs(b[1].origin.y - (b[0].origin.y - m)) > 0.5)
    return @"position off";
  if (fabs(b[1].size.width - (b[0].size.width + 2 * m)) > 0.5)
    return @"size off";
  if (idx[1] != idx[0] + 1)
    return [NSString stringWithFormat:@"order off (popup %d, shadow %d)", idx[0], idx[1]];
  return @"ok";
}

// ---------------------------------------------------------------- setups

typedef enum { MODE_NONE, MODE_SYSTEM, MODE_B1, MODE_B2 } Mode;
static NSString *mode_name(Mode m)
{
  return @[@"none", @"a-system", @"b1-child-of-popup", @"b2-sibling"][m];
}

static PShadowWin *attach(Mode mode, ELWindow *pop, ELWindow *root, PShadow spec)
{
  P_round(pop, RADIUS);
  if (mode == MODE_SYSTEM)
    {
      pop.hasShadow = YES;
      [pop invalidateShadow];
    }
  if (mode != MODE_B1 && mode != MODE_B2)
    return nil;
  PShadowWin *sw = [[PShadowWin alloc] initWithSpec:spec radius:RADIUS];
  [sw fitTo:pop.frame];
  if (mode == MODE_B1)
    [pop addChildWindow:sw.win ordered:NSWindowBelow];
  else
    {
      // the shadow first, then the popup re-attached above it
      [root removeChildWindow:pop];
      [root addChildWindow:sw.win ordered:NSWindowAbove];
      [root addChildWindow:pop ordered:NSWindowAbove];
    }
  return sw;
}

// What Nidhogg's NS glue would do around each path, per placement.
static void do_move(Mode mode, ELWindow *pop, PShadowWin *sw, NSPoint o)
{
  P_move(pop, o);
  if (mode == MODE_B2)
    [sw fitTo:pop.frame];
}

static void do_resize(Mode mode, ELWindow *pop, PShadowWin *sw, NSRect f)
{
  P_resize(pop, f);
  if (mode == MODE_SYSTEM)
    [pop invalidateShadow];
  if (sw)
    [sw fitTo:pop.frame];
}

static void do_raise(Mode mode, ELWindow *pop, PShadowWin *sw, ELWindow *root)
{
  if (mode == MODE_B2)
    {
      [root removeChildWindow:sw.win];
      [root addChildWindow:sw.win ordered:NSWindowAbove];
    }
  P_raise(pop);
}

static void do_hide(Mode mode, ELWindow *pop, PShadowWin *sw, BOOL fix)
{
  P_hide(pop);
  if (fix && sw)
    [sw.win orderOut:nil];
  else if (mode == MODE_B2)
    [sw.win orderOut:nil];   // b2 always needs it: nothing else hides a sibling
}

static void do_show(Mode mode, ELWindow *pop, PShadowWin *sw, ELWindow *root, BOOL fix)
{
  if (mode == MODE_B2)
    {
      [sw fitTo:pop.frame];
      [root addChildWindow:sw.win ordered:NSWindowAbove];
    }
  P_show(pop, root);
  if (fix && mode == MODE_B1)
    {
      [sw fitTo:pop.frame];
      [pop addChildWindow:sw.win ordered:NSWindowBelow];
    }
}

// ---------------------------------------------------------------- screenshots

static void shots(NSScreen *s, int scale)
{
  for (int dark = 0; dark <= 1; dark++)
    {
      P_appearance(dark);
      NSString *tag = [NSString stringWithFormat:@"%dx-%@", scale, dark ? @"dark" : @"light"];
      ELWindow *root = P_root(P_at(s, 20, 20, 1180, 640), dark, 0);
      NSRect rc = [root contentRectForFrameRect:root.frame];
      Mode modes[3] = {MODE_SYSTEM, MODE_B1, MODE_B1};
      PShadow specs[3] = {P_DEFAULT, P_MOCKUP, P_DEFAULT};
      NSString *names[3] = {@"a-system", @"b-mockup", @"b-default"};
      NSMutableArray *keep = [NSMutableArray array];
      NSRect all = NSZeroRect;
      for (int i = 0; i < 3; i++)
        {
          NSRect r = NSMakeRect(rc.origin.x + 60 + i * 380, rc.origin.y + 300, 300, 190);
          ELWindow *pop = P_child(root, r, dark, 0);
          pop.el.label = names[i];
          [pop.el paint];
          PShadowWin *sw = attach(modes[i], pop, root, specs[i]);
          [keep addObject:pop];
          if (sw)
            [keep addObject:sw];
          all = NSIsEmptyRect(all) ? r : NSUnionRect(all, r);
        }
      P_spin(0.6);
      P_shot([@"p2-side-by-side-" stringByAppendingString:tag], NSInsetRect(all, -60, -110));
      for (int i = 0; i < 3; i++)
        {
          NSRect r = NSMakeRect(rc.origin.x + 60 + i * 380, rc.origin.y + 300, 300, 190);
          P_shot([NSString stringWithFormat:@"p2-%@-%@", names[i], tag], NSInsetRect(r, -60, -110));
        }
      for (id o in keep)
        if ([o isKindOfClass:[PShadowWin class]])
          [((PShadowWin *)o).win close];
        else
          [(NSWindow *)o close];
      [keep removeAllObjects];

      // translucent popups (alpha-background 0.75) over the root's text: does either shadow show through?
      NSRect r1 = NSMakeRect(rc.origin.x + 60, rc.origin.y + 300, 300, 190);
      NSRect r2 = NSMakeRect(rc.origin.x + 440, rc.origin.y + 300, 300, 190);
      ELWindow *p1 = P_child(root, r1, dark, 0), *p2 = P_child(root, r2, dark, 0);
      p1.el.bg = [p1.el.bg colorWithAlphaComponent:0.75];
      p2.el.bg = [p2.el.bg colorWithAlphaComponent:0.75];
      p1.el.label = @"a-system, translucent";
      p2.el.label = @"b-mockup, translucent";
      [p1.el paint];
      [p2.el paint];
      attach(MODE_SYSTEM, p1, root, P_DEFAULT);
      PShadowWin *sw2 = attach(MODE_B1, p2, root, P_MOCKUP);
      P_spin(0.6);
      P_shot([@"p2-translucent-" stringByAppendingString:tag], NSInsetRect(NSUnionRect(r1, r2), -60, -110));
      [sw2.win close];
      [p1 close];
      [p2 close];
      [root close];
      P_spin(0.2);
    }
}

// ---------------------------------------------------------------- sync and cost

static void sync_and_cost(NSScreen *s, int scale, Mode mode)
{
  P_appearance(NO);
  NSString *tag = [NSString stringWithFormat:@"%dx.%@", scale, mode_name(mode)];
  ELWindow *root = P_root(P_at(s, 20, 20, 1000, 640), NO, 0);
  NSRect rc = [root contentRectForFrameRect:root.frame];
  NSRect r0 = NSMakeRect(rc.origin.x + 300, rc.origin.y + 200, 360, 220);
  ELWindow *pop = P_child(root, r0, NO, 0);
  PShadowWin *sw = attach(mode, pop, root, P_MOCKUP);
  P_spin(0.4);
  Sampler smp;
  pthread_t th;
  const double hz = 120;

  // move: 240 frames at 120 Hz along a circle of radius 120 pt (corfu following point, a posframe being dragged)
  {
    int n = 240;
    double *ms = calloc(n, sizeof *ms);
    if (sw)
      th = sampler_start(&smp, pop, sw);
    double c0 = P_cpu_self(), w0 = P_cpu_ws();
    uint64_t start = P_now_ns();
    for (int i = 0; i < n; i++)
      {
        P_frame_wait(start, i, hz);
        uint64_t t0 = P_now_ns();
        double a = i * 2 * M_PI / 120;
        do_move(mode, pop, sw, NSMakePoint(r0.origin.x + 120 * cos(a), r0.origin.y + 120 * sin(a)));
        [CATransaction flush];
        ms[i] = (P_now_ns() - t0) / 1e6;
      }
    double secs = (P_now_ns() - start) / 1e9, c1 = P_cpu_self(), w1 = P_cpu_ws();
    P_spin(0.1);
    if (sw)
      sampler_stop(&smp, th, [tag stringByAppendingString:@".move.sync"]);
    R([tag stringByAppendingString:@".move.main-ms"], @"%@", P_stats(ms, n));
    R([tag stringByAppendingString:@".move.cpu-per-frame-ms"], @"self %.3f, WindowServer %.3f (over %.2f s)",
      (c1 - c0) * 1e3 / n, w0 < 0 ? -1 : (w1 - w0) * 1e3 / n, secs);
    if (sw)
      R([tag stringByAppendingString:@".move.settled"], @"%@", pair_state(pop, sw));
    free(ms);
  }

  // resize: 120 frames, the popup's size oscillating (corfu's candidate list growing and shrinking), repainted
  {
    int n = 120;
    double *ms = calloc(n, sizeof *ms);
    if (sw)
      th = sampler_start(&smp, pop, sw);
    double c0 = P_cpu_self(), w0 = P_cpu_ws();
    uint64_t start = P_now_ns();
    for (int i = 0; i < n; i++)
      {
        P_frame_wait(start, i, hz);
        uint64_t t0 = P_now_ns();
        double a = i * 2 * M_PI / 60;
        NSRect f = NSMakeRect(r0.origin.x, r0.origin.y, 300 + 120 * (0.5 + 0.5 * sin(a)),
                              180 + 80 * (0.5 + 0.5 * cos(a)));
        pop.el.generation = i;
        do_resize(mode, pop, sw, f);
        [CATransaction flush];
        ms[i] = (P_now_ns() - t0) / 1e6;
      }
    double c1 = P_cpu_self(), w1 = P_cpu_ws();
    P_spin(0.1);
    if (sw)
      sampler_stop(&smp, th, [tag stringByAppendingString:@".resize.sync"]);
    R([tag stringByAppendingString:@".resize.main-ms"], @"%@", P_stats(ms, n));
    R([tag stringByAppendingString:@".resize.cpu-per-frame-ms"], @"self %.3f, WindowServer %.3f",
      (c1 - c0) * 1e3 / n, w0 < 0 ? -1 : (w1 - w0) * 1e3 / n);
    if (sw)
      R([tag stringByAppendingString:@".resize.settled"], @"%@", pair_state(pop, sw));
    free(ms);
  }

  if (sw)
    {
      // raise: a second popup overlapping; raise each in turn; the shadow must stay directly under its popup
      ELWindow *other = P_child(root, NSOffsetRect(pop.frame, 120, -60), NO, 0);
      other.el.label = @"another child frame";
      [other.el paint];
      int now_ok = 0, later_ok = 0, n = 40;
      NSString *first_bad = nil;
      for (int i = 0; i < n; i++)
        {
          if (i % 2)
            P_raise(other);
          else
            do_raise(mode, pop, sw, root);
          P_spin(0);
          NSString *st = pair_state(pop, sw);
          now_ok += [st isEqual:@"ok"];
          P_spin(0.05);
          NSString *st2 = pair_state(pop, sw);
          later_ok += [st2 isEqual:@"ok"];
          if (![st2 isEqual:@"ok"] && !first_bad)
            first_bad = st2;
        }
      R([tag stringByAppendingString:@".raise"], @"%d/%d right at once, %d/%d after 50 ms%@", now_ok, n, later_ok, n,
        first_bad ? [@"; first wrong: " stringByAppendingString:first_bad] : @"");
      P_shot([@"p2-after-raises-" stringByAppendingString:tag],
             NSInsetRect(NSUnionRect(pop.frame, other.frame), -60, -110));
      [other close];

      // hide and show as Nidhogg does today (fix NO), then with the glue's extra step (fix YES)
      for (int fix = 0; fix <= 1; fix++)
        {
          int hide_ok = 0, show_ok = 0, cycles = 20;
          NSString *hide_bad = nil, *show_bad = nil;
          for (int i = 0; i < cycles; i++)
            {
              do_hide(mode, pop, sw, fix);
              P_spin(0.05);
              NSString *h = pair_state(pop, sw);
              if ([h isEqual:@"both hidden"])
                hide_ok++;
              else if (!hide_bad)
                hide_bad = h;
              do_show(mode, pop, sw, root, fix);
              P_spin(0.05);
              NSString *v = pair_state(pop, sw);
              if ([v isEqual:@"ok"])
                show_ok++;
              else if (!show_bad)
                show_bad = v;
            }
          R([tag stringByAppendingString:fix ? @".hide-show.with-glue" : @".hide-show.as-today"],
            @"hidden together %d/%d%@; shown together %d/%d%@", hide_ok, cycles,
            hide_bad ? [@" (" stringByAppendingFormat:@"%@)", hide_bad] : @"", show_ok, cycles,
            show_bad ? [@" (" stringByAppendingFormat:@"%@)", show_bad] : @"");
          // leave it shown and attached for the next pass
          if (![pair_state(pop, sw) isEqual:@"ok"])
            do_show(mode, pop, sw, root, YES);
        }

      // the parent frame moves (a root frame dragged): child windows follow; does the shadow?
      NSPoint ro = root.frame.origin;
      [root setFrameOrigin:NSMakePoint(ro.x + 40, ro.y - 30)];
      P_spin(0.1);
      R([tag stringByAppendingString:@".root-move"], @"%@", pair_state(pop, sw));
      [sw.win close];
    }
  [pop close];
  [root close];
  P_spin(0.2);
}

int main(void)
{
  @autoreleasepool
    {
      P_init("p2-shadow");
      R(@"specs", @"mockup: offset (%.0f, %.0f) blur %.0f opacity %.2f; default: offset (%.0f, %.0f) blur %.0f "
                  @"opacity %.2f; CA shadowRadius = blur / 2",
        P_MOCKUP.ox, P_MOCKUP.oy, P_MOCKUP.blur, P_MOCKUP.opacity, P_DEFAULT.ox, P_DEFAULT.oy, P_DEFAULT.blur,
        P_DEFAULT.opacity);
      for (int scale = 1; scale <= 2; scale++)
        {
          NSScreen *s = P_screen(scale);
          if (!s)
            {
              R([NSString stringWithFormat:@"%dx", scale], @"skipped: no screen at this scale");
              continue;
            }
          shots(s, scale);
          for (Mode m = MODE_NONE; m <= MODE_B2; m++)
            sync_and_cost(s, scale, m);
        }
      R(@"done", @"ok");
    }
  return 0;
}

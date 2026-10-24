/* glitnir.h -- the C ABI of the Glitnir compositor library.

   Every function returns a status code.  Ids and numbers cross the boundary, never pointers to the library's
   memory.  A panic inside the library is caught at the boundary and reported as GLITNIR_E_PANIC.  */

#ifndef GLITNIR_H
#define GLITNIR_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

#define GLITNIR_OK        0
#define GLITNIR_E_PANIC  -1
#define GLITNIR_E_ARG    -2
#define GLITNIR_E_STATE  -3

/* Write the ABI version (1) to *OUT.  GLITNIR_E_ARG if OUT is NULL.  */
int32_t glitnir_abi_version (uint32_t *out);

/* Panic inside the guard; always returns GLITNIR_E_PANIC.  For tests.  */
int32_t glitnir_debug_panic (void);

/* One paint, as the glue measures it (monotonic nanoseconds; 0 = not measured).  */
typedef struct
{
  uint64_t t_update_ns, t_commit_ns, t_freeze_ns, t_draw_start_ns, t_draw_end_ns;
  uint64_t t_compose_start_ns, t_compose_end_ns;
  int64_t clock_frame_us;
  int64_t presented_us;		/* -1: not known (GTK3 binds no wp_presentation) */
  uint64_t damage_px, scroll_px, painted_px;
  uint32_t layers, ticks, late_ticks, violations;
  uint8_t backfill;		/* 0, 1, or 2 unknown */
  uint8_t scale_mismatch;
} GlitnirStatsEntry;

typedef struct { double p50, p95, max; } GlitnirPercentiles;

/* The last 1024 entries' percentiles (ms), and totals since the last reset.  */
typedef struct
{
  uint64_t count;
  GlitnirPercentiles compose_ms, draw_ms, latency_ms, freeze_ms;
  uint64_t damage_px_p50, painted_px_p50;
  double backfill_rate;		/* -1: no entry knows (backfill 2 everywhere) */
  uint64_t ticks, late_ticks, violations, scale_mismatch;
} GlitnirStatsSummary;

/* Append ENTRY to ROOT's ring (allocated at the first record).  GLITNIR_E_ARG if ENTRY is NULL.  */
int32_t glitnir_stats_record (uint64_t root, const GlitnirStatsEntry *entry);

/* Write ROOT's summary to OUT (zeros for an unknown root); empty the ring when RESET is non-zero.  */
int32_t glitnir_stats_summary (uint64_t root, int32_t reset, GlitnirStatsSummary *out);

/* Drop ROOT's ring.  */
int32_t glitnir_stats_forget (uint64_t root);

/* A rectangle in device pixels; empty when w <= 0 or h <= 0.  */
typedef struct { int32_t x, y, w, h; } GlitnirRect;

/* Damage: what changed on a frame since its damage was last taken.  A rectangle already contained in the damage adds
   nothing.  Whole-frame damage reads back as one rectangle {0, 0, INT32_MAX, INT32_MAX}: the reader clips it to the
   frame.  */

/* Add the N rectangles at RECTS to FRAME's damage.  GLITNIR_E_ARG if RECTS is NULL and N is not 0.  */
int32_t glitnir_damage_add_n (uint64_t frame, const GlitnirRect *rects, size_t n);

/* Damage the whole of FRAME.  */
int32_t glitnir_damage_all (uint64_t frame);

/* WINDOW's pixels in AREA moved vertically by DY on FRAME (the host copied them); EXPOSED is the part of AREA the move
   left stale.  Both rectangles are damaged.  */
int32_t glitnir_scroll (uint64_t frame, uint64_t window, GlitnirRect area, int32_t dy, GlitnirRect exposed);

/* Take FRAME's damage: *OUT points to *N rectangles, valid until FRAME's next glitnir_damage_pending or
   glitnir_damage_forget; FRAME then has no damage.  GLITNIR_E_ARG if OUT or N is NULL.  */
int32_t glitnir_damage_pending (uint64_t frame, const GlitnirRect **out, size_t *n);

/* Drop FRAME's damage (the frame is gone).  */
int32_t glitnir_damage_forget (uint64_t frame);

/* What a commit hands back.  */
typedef struct
{
  const GlitnirRect *rects;
  size_t n;
} GlitnirCommitOut;

/* Commit ROOT at a consistency point (no drawing in progress on ROOT or its children): OUT->rects points to OUT->n
   rectangles, ROOT's damage in ROOT's device pixels (the host moves its child frames' damage into ROOT first), valid
   until ROOT's next commit or glitnir_damage_forget; ROOT then has no damage.  GLITNIR_E_ARG if OUT is NULL.  */
int32_t glitnir_commit (uint64_t root, GlitnirCommitOut *out);

#ifdef __cplusplus
}
#endif

#endif /* GLITNIR_H */

/* glitnir.h -- the C ABI of the Glitnir compositor library.

   Every function returns a status code.  Ids and numbers cross the boundary, never pointers to the library's
   memory.  A panic inside the library is caught at the boundary and reported as GLITNIR_E_PANIC.  */

#ifndef GLITNIR_H
#define GLITNIR_H

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
  double backfill_rate;
  uint64_t ticks, late_ticks, violations, scale_mismatch;
} GlitnirStatsSummary;

/* Append ENTRY to ROOT's ring (allocated at the first record).  GLITNIR_E_ARG if ENTRY is NULL.  */
int32_t glitnir_stats_record (uint64_t root, const GlitnirStatsEntry *entry);

/* Write ROOT's summary to OUT (zeros for an unknown root); empty the ring when RESET is non-zero.  */
int32_t glitnir_stats_summary (uint64_t root, int32_t reset, GlitnirStatsSummary *out);

/* Drop ROOT's ring.  */
int32_t glitnir_stats_forget (uint64_t root);

#ifdef __cplusplus
}
#endif

#endif /* GLITNIR_H */

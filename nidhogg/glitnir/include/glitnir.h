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

/* Append ENTRY to ROOT's ring (allocated at the first record).  An entry whose t_compose_start_ns and
   t_compose_end_ns are both 0 gets the time ROOT's composes took since its last record, ending at t_draw_end_ns; one
   whose layers is 0, the layers they drew.  GLITNIR_E_ARG if ENTRY is NULL.  */
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
   left stale.  Both rectangles are damaged, and a scene with a window layer WINDOW damages that layer's rectangle at
   the next commit.  */
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
   rectangles in ROOT's device pixels, valid until ROOT's next commit or glitnir_damage_forget.
   Without a scene (glitnir_root_new): ROOT's own damage (the host moves its child frames' damage into ROOT first).
   With one, the scene damage, clipped to ROOT's frame layer:
   - each frame and child-node layer's damage (the damage of the frame whose id is the layer's), clipped to the layer
     and moved to its place; a hidden layer's damage is dropped, a removed layer's is left alone;
   - the rectangle of each window layer scrolled (glitnir_scroll);
   - the old and the new bounds (shadow included) of every layer that moved, resized, appeared, disappeared, changed
     chrome or changed place in the stacking, against the last commit, so a hide and a show between two commits
     cancel; every layer's when ROOT's frame layer changed scale.
   The layers' damage is taken.  GLITNIR_E_ARG if OUT is NULL.  */
int32_t glitnir_commit (uint64_t root, GlitnirCommitOut *out);

/* The scene: one root's layers as a tree, in ROOT's device pixels.  Ids are unique within a root, whatever their kind;
   a frame or child-node layer's id is its frame's (the id its damage is recorded under).  Parent 0 is the top level
   (ROOT's frame layer).  Under each layer, its windows paint first, then its child nodes bottom to top.  Every call but
   glitnir_root_new and glitnir_root_drop returns GLITNIR_E_STATE when ROOT has no scene.  */

#define GLITNIR_LAYER_FRAME  1	/* a frame's surface (ROOT's own) */
#define GLITNIR_LAYER_WINDOW 2	/* a window's rectangle on its frame's surface */
#define GLITNIR_LAYER_CHILD  3	/* a child frame's surface; its windows and nested child frames under it */

typedef struct
{
  uint64_t id, parent;
  uint32_t kind;		/* GLITNIR_LAYER_* */
  GlitnirRect rect;		/* root device px */
  double scale;
  uint8_t visible;		/* 0: hidden, with everything under it */
} GlitnirLayerDesc;

/* A layer's chrome, device px; colours premultiplied ARGB in Cairo's order (CAIRO_FORMAT_ARGB32).  The shadow
   (shadow_opacity > 0) is the layer grown by spread_px, blurred with sigma = blur_px / 2, moved by the offset.  */
typedef struct
{
  float radius_px, blur_px, offset_x, offset_y, spread_px, shadow_opacity;
  uint32_t shadow_argb;
  float border_px;
  uint32_t border_argb;
} GlitnirChrome;

/* Give ROOT a scene (an existing one is kept).  */
int32_t glitnir_root_new (uint64_t root);

/* Drop ROOT's scene; its commits return its own damage again.  */
int32_t glitnir_root_drop (uint64_t root);

/* Add or change the layer DESC->id.  A new layer, or one under a new parent, goes on top of its parent's children;
   its chrome is kept.  GLITNIR_E_ARG if DESC is NULL, the kind is unknown, the id is 0, the scale is not positive, or
   the parent is the layer or under it.  */
int32_t glitnir_layer_upsert (uint64_t root, const GlitnirLayerDesc *desc);

/* Remove layer ID and every layer under it (an unknown id: no change).  */
int32_t glitnir_layer_remove (uint64_t root, uint64_t id);

/* Set layer ID's chrome; NULL: none.  GLITNIR_E_ARG for an unknown id.  */
int32_t glitnir_layer_set_chrome (uint64_t root, uint64_t id, const GlitnirChrome *chrome);

/* Raise (lower) layer ID to the top (bottom) of its parent's children.  GLITNIR_E_ARG for an unknown id.  */
int32_t glitnir_stack_raise (uint64_t root, uint64_t id);
int32_t glitnir_stack_lower (uint64_t root, uint64_t id);

/* Move layer ID, with everything under it, to the top of PARENT's children (0: the top level).  GLITNIR_E_ARG for an
   unknown id or a PARENT that is ID or under it.  */
int32_t glitnir_stack_reparent (uint64_t root, uint64_t id, uint64_t parent);

/* ROOT's child-node layers, hidden ones included, bottom to top, depth first (a nested child right above its parent,
   below the parent's upper siblings): *IDS points to *N ids, valid until ROOT's next glitnir_stack_list or
   glitnir_root_drop.  GLITNIR_E_ARG if IDS or N is NULL.  */
int32_t glitnir_stack_list (uint64_t root, const uint64_t **ids, size_t *n);

/* Compose.  Pixels are premultiplied ARGB32 (CAIRO_FORMAT_ARGB32), HEIGHT rows of STRIDE bytes (whole pixels, at least
   WIDTH), 4-byte aligned, borrowed for the call only.  A source is the surface of the frame whose id is LAYER, its
   first pixel at that layer's top-left; it must not overlap the target.  */
typedef struct
{
  uint64_t layer;
  const uint8_t *pixels;
  int32_t stride, width, height;
  double scale;
} GlitnirSource;

typedef struct
{
  uint8_t *pixels;
  int32_t stride, width, height;
  int32_t origin_x, origin_y;	/* the first pixel's place, root device px */
  double scale;
} GlitnirTarget;

/* Compose ROOT's committed scene (as its last commit left it) into TARGET within the NCLIP rectangles at CLIP (root
   device px; overlaps are composed once), bottom to top: a frame or child-node layer without chrome is copied from
   its source; a chromed layer's shadow goes over what lies below, then the layer is copied but for its corner
   squares, which go through the corner mask, with the border redrawn along the rounded path (the straight edges stay
   the source's).  Window layers are their frame's pixels and add nothing.  A layer without a source is not drawn;
   target pixels no layer covers keep their value.  The time taken and the layers drawn go to ROOT's next
   glitnir_stats_record.
   GLITNIR_E_STATE, with nothing drawn, when ROOT has no scene or a source's scale differs from TARGET's (the host
   then draws ROOT the stock way).  GLITNIR_E_ARG if TARGET is NULL, CLIP or SOURCES is NULL with a non-zero count,
   or a size, stride or pixel pointer is not as above.  */
int32_t glitnir_compose (uint64_t root, const GlitnirTarget *target, const GlitnirRect *clip, size_t nclip,
			 const GlitnirSource *sources, size_t nsrc);

#ifdef __cplusplus
}
#endif

#endif /* GLITNIR_H */

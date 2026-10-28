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
  /* From t_update_ns to the end of the paint, over the entries with an update time only.  */
  GlitnirPercentiles update_ms;
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

/* A rectangle in logical units.  */
typedef struct { double x, y, w, h; } GlitnirRectF;

/* Snapping and stable rounding.  SCALE120 is the fractional scale times 120.  Stable rounding is mutter's: an origin is
   roundf (POS * s), an extent roundf ((POS + SIZE) * s) - roundf (POS * s), in float, half away from zero.  */

/* The device origin of a surface at logical POS.  */
int32_t glitnir_stable_origin (int32_t pos, uint32_t scale120);

/* The device extent of the logical span POS .. POS + SIZE.  */
int32_t glitnir_stable_extent (int32_t pos, int32_t size, uint32_t scale120);

/* The device rectangle of LOGICAL, relative to the device origin of a surface at logical (ORIGIN_X, ORIGIN_Y).  */
GlitnirRect glitnir_stable_rect (GlitnirRect logical, int32_t origin_x, int32_t origin_y, uint32_t scale120);

/* Move a logical rectangle's edges to round (e * SCALE) / SCALE; one under 2 device px wide or high becomes
   max (1, floor (size * SCALE)) device px at round (pos * SCALE).  An integer SCALE returns the input.  */
GlitnirRectF glitnir_snap_rect (double x, double y, double w, double h, double scale);

/* A line width of W logical px at SCALE: under 2 device px, max (1, floor (W * SCALE)) of them; else the nearest whole
   count.  An integer SCALE returns W.  */
double glitnir_snap_line (double w, double scale);

/* Resample the W x H mask at SRC (A1 in cairo's bit order when A1 is not 0, else A8; rows SRC_STRIDE bytes apart) to A8
   on the device grid at SCALE120 into OUT, which holds glitnir_stable_origin (W) x glitnir_stable_origin (H) pixels
   (rows OUT_STRIDE bytes apart).  Logical cell I covers device px [stable_origin (I), stable_origin (I + 1)), nearest,
   so every device pixel of an A1 mask is fully on or off.  GLITNIR_E_ARG for a NULL pointer, an empty size or a stride
   too small.  */
int32_t glitnir_mask_to_device (const uint8_t *src, int32_t src_stride, int32_t w, int32_t h, int32_t a1,
				uint32_t scale120, uint8_t *out, int32_t out_stride);

/* The rounded corners of a decorated box (PD) into the ARGB32 picture ARGB (rows STRIDE bytes apart, 4-byte aligned):
   in each corner of CORNERS_MASK (1 top left, 2 top right, 4 bottom right, 8 bottom left) of RUN (device px), BENEATH
   outside the arc of radius RADIUS * SCALE and the ring of width RING_W * SCALE along it, rounded to device px (the
   radius clamped to half the shorter side).  FILL 0 composes OVER the pixels (the box's inside stays); otherwise the
   corner squares are replaced, FILL inside the ring.  Colours premultiplied 0xAARRGGBB; tiles cached per (radius,
   scale, colours).  The straight edges are the caller's, and RUN must lie in the picture (the caller clips).
   GLITNIR_E_ARG for a NULL or misaligned ARGB, a bad stride, a negative origin, an empty RUN or a bad number.  */
int32_t glitnir_decor_corners (uint8_t *argb, int32_t stride, GlitnirRect run, double radius, uint32_t fill,
			       uint32_t ring, uint32_t beneath, double ring_w, uint32_t corners_mask, double scale);

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

/* Decorations (labels).  An anchor states something about a window layer's pixels: the first glyph of a labelled run.
   Coordinates are device px in the window layer (its top-left is 0, 0); the sprite is a name tag the host rendered
   once, at the layer's device scale, and cached with glitnir_decor_sprite_put.  Every call but the sprite ones is
   GLITNIR_E_STATE for a ROOT without a scene and GLITNIR_E_ARG for an unknown LAYER.  */
typedef struct
{
  GlitnirRect glyph;		/* the anchor glyph */
  GlitnirRect clip;		/* the label is drawn only inside this (and the layer) */
  uint64_t sprite;		/* the sprite's key */
  int32_t row_top;		/* the window's first text row's top: a glyph with y <= row_top gets its label below */
} GlitnirAnchor;

/* Record A on LAYER, replacing the anchors whose glyph it overlaps.  GLITNIR_E_ARG if A is NULL.  */
int32_t glitnir_decor_put (uint64_t root, uint64_t layer, const GlitnirAnchor *a);

/* Drop LAYER's anchors whose glyph meets EXTENT (a glyph string drew over it).  */
int32_t glitnir_decor_drop_extent (uint64_t root, uint64_t layer, GlitnirRect extent);

/* Drop LAYER's anchors whose glyph lies wholly inside R (a clear covered it).  */
int32_t glitnir_decor_drop_rect (uint64_t root, uint64_t layer, GlitnirRect r);

/* Drop LAYER's anchors; LAYER 0: every layer of ROOT.  */
int32_t glitnir_decor_drop_all (uint64_t root, uint64_t layer);

/* LAYER's pixels in BAND moved vertically by DY: the anchors whose glyph lies in BAND move with them, and those that
   would end outside BAND or the layer are dropped.  Call it with glitnir_scroll.  */
int32_t glitnir_decor_scroll (uint64_t root, uint64_t layer, GlitnirRect band, int32_t dy);

/* Cache the sprite KEY: premultiplied ARGB32, H rows of STRIDE bytes (whole pixels, at least W), 4-byte aligned, at
   most 4096 on a side; replaces an earlier sprite of that key.  The cache keeps the 64 most recently used.
   GLITNIR_E_ARG for a bad size, stride or pointer.  */
int32_t glitnir_decor_sprite_put (uint64_t key, const uint8_t *argb, int32_t stride, int32_t w, int32_t h);

/* 1 if KEY is cached (and counts as a use), else 0.  */
int32_t glitnir_decor_sprite_has (uint64_t key);

/* Labels.  At commit the scene places a label for each anchor whose sprite is cached: above the anchor glyph, below it
   when glyph.y <= row_top, shifted right past an earlier label on the same row, and one row further from the text (up
   for a label above, down for one below) when the shift would take it out of the layer.  A label that appears,
   moves, changes sprite or goes adds its old and new sprite bounds to the commit's damage, and compose draws the
   sprites over the window layer's pixels, clipped to the anchor's clip.  */

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
   the source's).  Window layers are their frame's pixels and add nothing but their labels' sprites.  A layer without a source is not drawn;
   target pixels no layer covers keep their value.  The time taken and the layers drawn go to ROOT's next
   glitnir_stats_record.
   GLITNIR_E_STATE, with nothing drawn, when ROOT has no scene or a source's scale differs from TARGET's (the host
   then draws ROOT the stock way).  GLITNIR_E_ARG if TARGET is NULL, CLIP or SOURCES is NULL with a non-zero count,
   or a size, stride or pixel pointer is not as above.  */
int32_t glitnir_compose (uint64_t root, const GlitnirTarget *target, const GlitnirRect *clip, size_t nclip,
			 const GlitnirSource *sources, size_t nsrc);

/* Image probes on an ARGB32 picture (native byte order; STRIDE bytes per row, at least 4 * W; the buffer holds
   STRIDE * H bytes, the last row possibly stopping after 4 * W).  Luminance is that of the sRGB-encoded values, 0 to 1.
   All return GLITNIR_E_ARG for a NULL pointer, a non-positive size, a stride under 4 * W, or a region that is empty or
   not inside the picture.  */

/* R9's edge metric over REGION: the monotonic luminance runs along every row and column.  SPREAD_PX is the mean 10-90 %
   rise distance (linearly interpolated) of the RUNS runs whose amplitude is >= 0.5; CONTRAST the mean amplitude of
   the runs >= 0.2.  */
typedef struct { double spread_px, contrast; uint32_t runs; } GlitnirEdgeStats;
int32_t glitnir_probe_edges (const uint8_t *argb, int32_t stride, int32_t w, int32_t h, GlitnirRect region,
			     GlitnirEdgeStats *out);

/* A rule inside BAND: the darkness (1 - luminance, averaged along the rule) profile across the band, along x when
   VERTICAL is non-zero, else along y; the runs above min + 0.02 are measured and the heaviest is reported: WIDTH
   its pixels, PEAK its largest excess over the minimum, MASS the sum of those excesses.  */
typedef struct { double width, peak, mass; } GlitnirRuleStats;
int32_t glitnir_probe_rule (const uint8_t *argb, int32_t stride, int32_t w, int32_t h, GlitnirRect band,
			    int32_t vertical, GlitnirRuleStats *out);

/* Write to COUNT the number of pixels of FILL, inset by INSET on every side, whose luminance differs from that of
   the colour FILL_ARGB (0xAARRGGBB) by more than 1/255.  */
int32_t glitnir_probe_seams (const uint8_t *argb, int32_t stride, int32_t w, int32_t h, GlitnirRect fill,
			     uint32_t fill_argb, uint32_t inset, uint32_t *count);

#ifdef __cplusplus
}
#endif

#endif /* GLITNIR_H */

/* nhglue.h -- the C ABI of Nidhogg's shell glue (crate nhglue, libnhglue.a).

   The glue holds the native shell's state and makes the decisions that need no live Emacs state; the C hooks in
   src/nhwl*.c touch Emacs and call in here.  Calls go one way: C calls nhg_*, the glue answers with plain records and
   never calls back into Emacs (nidhogg/glue/nm-check.sh checks the archive for it).  Call everything on the Lisp
   thread that holds Emacs's global lock.  Every call returns NHG_OK (or a count, where said) or a negative NHG_E_*
   code; a panic inside the glue is caught at the boundary and reported as NHG_E_PANIC.  libnhglue.a also carries every
   nhshell_* and glitnir_* symbol: it is the one Rust library Emacs links.

   The guard.  Every call runs in one: with less than 256 KiB of stack left it returns NHG_E_NOMEM; a display whose
   state is locked on entry (a call that re-entered, or one a longjmp abandoned) is poisoned; a panic poisons it too.
   A poisoned display answers NHG_E_PANIC to every call but nhg_panic_message and nhg_close, and nhg_close then shuts
   its connection down and leaks it.  C makes every call through NHG (src/nhwlterm.h), which clears
   waiting_for_input around it, so that SIGINT's longjmp never crosses a Rust frame.

   Pixels: a frame's backing belongs to the glue, and C draws on it through a Cairo surface over the same memory
   (cairo_image_surface_create_for_data).  C calls cairo_surface_flush before a call that "reads pixels" and
   cairo_surface_mark_dirty after one that "writes pixels"; each call's comment says which, or neither.  */

#ifndef NHGLUE_H
#define NHGLUE_H

#include <stddef.h>
#include <stdint.h>

#include "nhshell.h"

#ifdef __cplusplus
extern "C" {
#endif

#define NHG_ABI_VERSION      1

/* The codes equal NHSHELL_E_* where they mean the same.  */
#define NHG_OK               0
#define NHG_E_PANIC         -1   /* a panic inside the glue */
#define NHG_E_ARG           -2
#define NHG_E_STATE         -3
#define NHG_E_UNSUPPORTED   -4
#define NHG_E_DISCONNECTED  -5
#define NHG_E_NOMEM         -6
#define NHG_E_TIMEOUT       -7
#define NHG_E_TOO_SMALL     -8
#define NHG_E_THREAD        -9   /* called from a thread the backend refuses */

/* One display: a shell connection and the glue's state for it.  */
typedef struct NhgDisplay NhgDisplay;

/* Write the ABI version (NHG_ABI_VERSION) to *OUT.  NHG_E_ARG if OUT is NULL.  */
int32_t nhg_abi_version (uint32_t *out);

/* Connect to DISPLAY (NULL: the environment's) as nhshell_open does.  NULL on failure, with a NUL-terminated reason
   in ERR (CAP bytes; ERR may be NULL when CAP is 0).  */
NhgDisplay *nhg_open (const char *display, char *err, size_t cap);

/* Close the connection and free D.  NULL is accepted.  D is not used again.  On a poisoned display it touches nothing
   behind the display's lock: it shuts the connection's socket down (the compositor destroys our windows), stops the
   shell's threads, waits at most 100 ms for each and leaks the rest.  */
int32_t nhg_close (NhgDisplay *d);

/* The descriptor that is readable when events are queued (>= 0), or a negative code.  */
int32_t nhg_fd (NhgDisplay *d);

/* The shell handle inside D, for the nhshell_* calls not yet ported; valid until nhg_close.  NULL if D is NULL.  */
NhShell *nhg_shell (NhgDisplay *d);

/* ---- Backings, damage and scroll ----------------------------------------------------------------------------------

   FRAME is a frame's key: its Glitnir root id, which nhg_layer_ensure gives it (a shell window id is not stable for
   a frame: popup, tip and child frames get a new window each time they are shown).  Rectangles are device pixels;
   damage and scroll take them with the native area's origin, and the glue adds the border the last
   nhg_layer_ensure was given.  */

/* A rectangle in device pixels; empty when w <= 0 or h <= 0.  Layout equals GlitnirRect.  */
typedef struct { int32_t x, y, w, h; } NhgRect;

#define NHG_GEOM_LAYER_CHANGED  1   /* the layer's rectangle or chrome changed (a child frame's) */
#define NHG_GEOM_FRESH          2   /* the backing starts over in BG_ARGB, keeping nothing */

/* A frame's geometry as C reads it before a call.  The backing is the native area plus the border L, T, R, B (the
   title strip and the margins), at least 1 x 1.  */
typedef struct
{
  int32_t native_w, native_h;   /* the frame's pixel size */
  int32_t l, t, r, b;
  uint32_t bg_argb;             /* a fresh backing's colour, premultiplied ARGB as Cairo stores it */
  uint32_t scale120;            /* the window's scale times 120 (0: 120) */
  uint32_t flags;               /* NHG_GEOM_* */
  NhgRect layer;                /* the Glitnir layer in the backing */
} NhgFrameGeom;

/* A backing's pixels, borrowed: ARGB32, STRIDE = 4 * W.  GEN counts the backing's creations.  */
typedef struct { uint8_t *pixels; int32_t stride, w, h; uint64_t gen; } NhgBacking;

/* A frame's damage committed but not yet presented, borrowed, and its area.  */
typedef struct { const NhgRect *rects; size_t n; uint64_t damage_px; } NhgPending;

/* Make the frame's Glitnir layer match G; *FRAME 0 gets a key and a scene.  1 when the layer was made or changed (the
   frame is damaged whole and its pending damage dropped), 0 when it matched.  Neither reads nor writes pixels.  */
int32_t nhg_layer_ensure (NhgDisplay *d, uint64_t *frame, const NhgFrameGeom *g);

/* Make the frame's backing G's size and describe it in OUT.  A resize keeps the pixels where the old and new sizes
   overlap and leaves the rest transparent; a new backing, or one with NHG_GEOM_FRESH, is all BG_ARGB.  OUT->pixels
   stays valid until the next nhg_backing or nhg_backing_forget of the frame.  NHG_E_STATE before nhg_layer_ensure.
   Reads and writes pixels.  */
int32_t nhg_backing (NhgDisplay *d, uint64_t frame, const NhgFrameGeom *g, NhgBacking *out);

/* Damage N rectangles of the frame (native-area origin).  Returns how many were not empty.  Neither.  */
int32_t nhg_damage (NhgDisplay *d, uint64_t frame, const NhgRect *r, size_t n);

/* Damage the whole frame, its border included.  Neither.  */
int32_t nhg_damage_all (NhgDisplay *d, uint64_t frame);

/* Emacs window W_SEQ's rows RUN (native-area origin: its columns, the run's first row and height) move to TO_Y in
   the backing, and Glitnir damages the moved area and the rows it left stale.  Writes pixels.  */
int32_t nhg_scroll (NhgDisplay *d, uint64_t frame, uint64_t w_seq, NhgRect run, int32_t to_y);

/* Take Glitnir's committed damage for the frame into its pending list (past 32 rectangles the list collapses to its
   bounding box).  Returns how many rectangles it took.  Neither.  */
int32_t nhg_commit (NhgDisplay *d, uint64_t frame);

/* The frame's pending damage in OUT, valid until the next call that changes it; none for an unknown frame.
   Neither.  */
int32_t nhg_pending (NhgDisplay *d, uint64_t frame, NhgPending *out);

/* The frame's pending damage was presented: the list and its area start again.  Neither.  */
int32_t nhg_presented (NhgDisplay *d, uint64_t frame);

/* The frame's backing in OUT, borrowed until the next call on the frame.  NHG_E_STATE without one.  Reads pixels.  */
int32_t nhg_snapshot (NhgDisplay *d, uint64_t frame, NhgBacking *out);

/* The frame is gone: its Glitnir scene, damage and statistics, and its backing.  FRAME 0 is accepted.  Neither.  */
int32_t nhg_backing_forget (NhgDisplay *d, uint64_t frame);

/* Multiply every pixel of a W x H premultiplied ARGB32 buffer (rows STRIDE bytes apart, 4-byte aligned) by the alpha
   1 - CUT / UINT32_MAX.  Writes the buffer's pixels (a shell buffer, not a backing).  */
int32_t nhg_alpha_multiply (NhgDisplay *d, uint8_t *pixels, int32_t stride, int32_t w, int32_t h, uint32_t cut);

/* ---- The window registry ------------------------------------------------------------------------------------------

   Each shell window C makes, with its kind, keyed by the window's id; a frame's pixel state above is keyed by its
   Glitnir root id instead, because a popup, tip or child frame gets a new window each time it is shown.  C registers
   each window it makes and forgets it when the window goes (until windows are made through the glue).  A forgotten
   window's calls are refused until it is registered again (the shell's ids are reused).  Register: NHG_E_STATE if
   registered, NHG_E_ARG for window 0 or an unknown flag; forget: NHG_E_STATE if not registered.  Neither reads nor
   writes pixels, nor do the calls below.  */
#define NHG_FRAME_CHILD      1
#define NHG_FRAME_TIP        2
#define NHG_FRAME_POPUP      4
int32_t nhg_frame_register (NhgDisplay *d, uint64_t window, uint32_t flags);
int32_t nhg_frame_forget (NhgDisplay *d, uint64_t window);

/* The Emacs facts a frame's decisions read, as C pushes them (sizes in device pixels; PARENT the parent's window, 0
   for none).  */
#define NHG_FACT_VISIBLE     1
#define NHG_FACT_ICONIFIED   2
typedef struct
{
  int32_t native_w, native_h, col_w, line_h;
  uint64_t parent;
  uint32_t flags;		/* NHG_FACT_* */
} NhgFrameFacts;

/* The first panic's report on D (message, location, backtrace): up to CAP - 1 bytes and a NUL into BUF, its full
   length into *LEN (0: no panic).  Works on a poisoned display.  */
int32_t nhg_panic_message (NhgDisplay *d, char *buf, size_t cap, size_t *len);

/* Test hooks: only in a library built with the feature test-hooks (configure's --enable-checking, so C calls them
   under ENABLE_CHECKING).  nhg_debug_check_frame compares the facts C pushed for WINDOW with Emacs's values F, and a
   mismatch poisons D (a debug assertion).  nhg_test_panic panics inside the glue.  */
int32_t nhg_debug_check_frame (NhgDisplay *d, uint64_t window, const NhgFrameFacts *f);
int32_t nhg_test_panic (NhgDisplay *d);

#ifdef __cplusplus
}
#endif

#endif /* NHGLUE_H */

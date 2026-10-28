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

/* Make the frame's Glitnir layer match G; *FRAME 0 gets a key and a scene.  1 when the layer was made or changed (the
   frame is damaged whole), 0 when it matched.  Neither reads nor writes pixels.  */
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

/* The frame's backing in OUT, borrowed until the next call on the frame.  NHG_E_STATE without one.  Reads pixels.  */
int32_t nhg_snapshot (NhgDisplay *d, uint64_t frame, NhgBacking *out);

/* The frame is gone: its Glitnir scene, damage and statistics, and its backing.  FRAME 0 is accepted.  Neither.  */
int32_t nhg_backing_forget (NhgDisplay *d, uint64_t frame);

/* ---- Presenting ---------------------------------------------------------------------------------------------------

   The shell presents (it commits, composing from what was published); the glue decides when a tree's picture is
   complete.  A tree is a root window (a top-level, tip or popup frame's) and the child frames' windows under it.  C
   lists the tree's surfaces bottom to top, the root first and each parent before its children: the window, its
   parent's window (0 for the root), the frame's key, and the flags.  NHG_SURF_DRAWABLE: the frame may show a buffer
   by itself (FRAME_VISIBLE_P and not new_size_p, and a child frame's window mapped); a surface shows one only when
   its ancestors may too, and an undrawable one keeps its last buffer.  NHG_SURF_ALPHA_HOST: the compositor cannot
   apply the frame's alpha, so the shell multiplies the pixels by 1 - ALPHA_CUT / UINT32_MAX.  */
#define NHG_SURF_DRAWABLE    1
#define NHG_SURF_ALPHA_HOST  2
typedef struct
{
  uint64_t window, parent, frame;
  uint32_t flags;		/* NHG_SURF_* */
  uint32_t alpha_cut;
} NhgSurface;

/* nhg_present's FLAGS.  BACKSTOP: a consistency point (not redisplaying, buffer flips not blocked; C has brought
   every drawable frame up to date): the tree is published when its root may show a buffer, with the root's key time
   T_KEY_NS (0: none).  READY: the root's FRAME_READY: in inline mode the shell presents the tree, drawn or not (a
   tree that owes a commit gets it).  KEY_SEEN: T_KEY_NS was read before the redisplay that just ended.  */
#define NHG_PRESENT_BACKSTOP 1
#define NHG_PRESENT_READY    2
#define NHG_PRESENT_KEY_SEEN 4
/* nhg_present's result bits.  PUBLISHED: a packet was queued.  COMMITTED: the root committed a buffer (inline).
   KEY_DONE: the key time is done (a packet carries it, or a redisplay after it drew nothing): C clears it.  */
#define NHG_PRESENTED_PUBLISHED 1
#define NHG_PRESENTED_COMMITTED 2
#define NHG_PRESENTED_KEY_DONE  4

/* Publish and present the tree of root window ROOT: S holds its N surfaces (NULL only when N is 0, which READY alone
   allows).  Returns NHG_PRESENTED_* bits, NHG_E_ARG for a list that is not ROOT's tree.  Reads pixels: C flushes
   every listed frame's surface first.  */
int32_t nhg_present (NhgDisplay *d, uint64_t root, const NhgSurface *s, size_t n, uint32_t flags, uint64_t t_key_ns);

/* Present on the shell's own thread (ON nonzero; FRAME_READY no longer comes) or inline, at each root's FRAME_READY
   (the default at open).  A thread that cannot start leaves the display inline (NHG_E_NOMEM).  Neither.  */
int32_t nhg_presenter_thread (NhgDisplay *d, int32_t on);

/* The presenter's counters since open, and its timing: FRAMES root buffers that showed published drawing, LATE of
   them committed more than a refresh after they were ready, and the longest wait from ready to commit.  Neither.  */
typedef struct
{
  NhShellPresenterStats counters;
  uint64_t frames, late, ready_to_commit_max_ns;
} NhgPresenterStats;
int32_t nhg_presenter_stats (NhgDisplay *d, NhgPresenterStats *out);

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
#define NHG_FACT_CSD         4   /* the frame draws its own title bar */
typedef struct
{
  int32_t native_w, native_h, col_w, line_h;
  uint64_t parent;
  uint32_t flags;		/* NHG_FACT_* */
} NhgFrameFacts;

/* Pushes, keyed by the window (C seeds them each time it makes a window for a frame).  NHG_E_STATE for a window not
   registered.  The native size as Emacs has it (the size C last asked for, device pixels): C pushes it when it makes
   the window, when applying a RESIZE action changes the frame's size, and when it resizes the frame itself; a
   configure being applied then gets a content ack.  NHG_E_ARG for an empty size.  */
int32_t nhg_frame_resized (NhgDisplay *d, uint64_t window, int32_t native_w, int32_t native_h);
/* The scale (times 120) C gave the window when it made it.  */
int32_t nhg_frame_scale (NhgDisplay *d, uint64_t window, uint32_t scale120);
/* Whether the frame draws its own title bar (ON non-zero), after each layout of it.  */
int32_t nhg_frame_csd (NhgDisplay *d, uint64_t window, int32_t on);

/* ---- The event step ------------------------------------------------------------------------------------------------

   read_socket_hook loops on nhg_next, which takes exactly one shell event and answers with its actions; C applies
   them (the Emacs calls) before it asks again, so each decision sees what the earlier events changed.  Before each
   call C samples NHG_IN_* again.  An action whose window maps to no frame is skipped.  Neither reads nor writes
   pixels, nor does nhg_raw_event.  */

#define NHG_IN_INPUT_WAITING   1   /* kbd_buffer_events_waiting () */
#define NHG_IN_COALESCE_WHEEL  2   /* mwheel-coalesce-scroll-events */
typedef struct { uint32_t flags; } NhgNextIn;

/* Kinds 1-31 are the port's, 32-63 W2's and W3's.  */
#define NHG_ACT_KEY              1
#define NHG_ACT_CLICK            2
#define NHG_ACT_WHEEL            3
#define NHG_ACT_MOTION           4
#define NHG_ACT_LEAVE            5
#define NHG_ACT_RESIZE           6   /* W, H the suggestion (logical; 0: the frame's own), CODE the NHSHELL_STATE_*,
				        FLAGS NHG_RESIZE_*; C pushes the frame's new size if it changes it */
#define NHG_ACT_SCALE            7   /* CODE the scale times 120, SCALE the factor */
#define NHG_ACT_GARBAGE          8
#define NHG_ACT_DELETE           9   /* the compositor asks to close the window */
#define NHG_ACT_FOCUS_IN        10
#define NHG_ACT_FOCUS_OUT       11
#define NHG_ACT_VISIBLE         12
#define NHG_ACT_ICONIFIED       13
#define NHG_ACT_READY           14   /* the window may present: C runs the present step, drawn or not */
#define NHG_ACT_MONITORS        15
#define NHG_ACT_LOST            16   /* the connection is gone */
#define NHG_ACT_PREEDIT         17
#define NHG_ACT_COMMIT          18
#define NHG_ACT_IM_DELETE       19
#define NHG_ACT_IM_SURROUNDING  20
#define NHG_ACT_SELECTION_CLEAR 21
/* Transitional: an event whose kind is not ported yet, handled on C's old path (nhg_raw_event gives it); a PREEDIT's
   or COMMIT_TEXT's text comes as the action's text (null: it could not be taken), and MODS is the keyboard's
   NHSHELL_MOD_* mask.  */
#define NHG_ACT_RAW             31

#define NHG_RESIZE_FULLSCREEN    1   /* fullscreen or maximized changed: C stores the fullscreen parameter */

typedef struct
{
  uint32_t kind;		/* NHG_ACT_* */
  uint64_t window;		/* the shell window the event belongs to (0: none) */
  uint64_t time_ms;
  int32_t x, y, w, h, code;	/* per kind */
  uint32_t mods, flags;
  double dx, dy, scale;
  const char *text;		/* UTF-8, NUL-terminated after LEN bytes; NULL: none */
  size_t len;
} NhgAction;
typedef struct { const NhgAction *v; size_t n; } NhgActionList;

/* Take exactly one shell event: 1 when it took one (OUT holds its 0..n actions), 0 when none is queued (OUT empty).
   The actions and their texts stay valid until the next nhg_next on D; no other call frees them.  The acks of the
   configures applied since the last call are recorded first.  IN may be NULL (no flags).  */
int32_t nhg_next (NhgDisplay *d, const NhgNextIn *in, NhgActionList *out);

/* The shell event behind the last NHG_ACT_RAW action into OUT; NHG_E_STATE when the last event gave none.  */
int32_t nhg_raw_event (NhgDisplay *d, NhShellEvent *out);

/* The first panic's report on D (message, location, backtrace): up to CAP - 1 bytes and a NUL into BUF, its full
   length into *LEN (0: no panic).  Works on a poisoned display.  */
int32_t nhg_panic_message (NhgDisplay *d, char *buf, size_t cap, size_t *len);

/* Test hooks: only in a library built with the feature test-hooks (configure's --enable-checking, so C calls them
   under ENABLE_CHECKING).  nhg_debug_check_frame compares the facts C pushed for WINDOW with Emacs's values F, and a
   mismatch poisons D (a debug assertion).  nhg_test_panic panics inside the glue.  */
int32_t nhg_debug_check_frame (NhgDisplay *d, uint64_t window, const NhgFrameFacts *f);
int32_t nhg_test_panic (NhgDisplay *d);

/* ---- The keyboard --------------------------------------------------------------------------------------------------

   A KEY action is a press or a repeat: WINDOW the window with the keyboard (C sends the key to the frame of its tree
   that Emacs focused, or to the focus frame when no frame has that window), CODE the keysym, TEXT its UTF-8 (possibly
   empty), MODS the NHSHELL_MOD_* mask, FLAGS NHG_KEY_*.  A repeat taken by a call with NHG_IN_INPUT_WAITING set gives
   no action.  FOCUS_IN and FOCUS_OUT name the window whose keyboard focus changed; C resolves Emacs's focus frame
   and its highlighting.  Neither reads nor writes pixels, nor does nhg_keyboard_window.  */

#define NHG_KEY_REPEAT           1   /* the key is a repeat */

/* The window that has the keyboard focus, as the last focus event said, into *OUT (0: none of ours).  */
int32_t nhg_keyboard_window (NhgDisplay *d, uint64_t *out);

#ifdef __cplusplus
}
#endif

#endif /* NHGLUE_H */

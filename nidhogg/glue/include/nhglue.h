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
#define NHG_E_RECONNECTING  -10  /* the display is reconnecting: the call sent nothing */

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

#define NHG_GEOM_LAYER_CHANGED  1   /* the layer's rectangle or chrome changed (the glue sets it for a child frame) */
#define NHG_GEOM_FRESH          2   /* the backing starts over in BG_ARGB, keeping nothing */

/* A frame's geometry as C reads it before a call.  The backing is the native area plus the border L, T, R, B (the
   title strip and the margins), at least 1 x 1.  WINDOW is the frame's shell window (0: none): for a child frame,
   nhg_layer_ensure puts the layer inside the shadow margin and hands the layer the child's chrome (child.rs).  */
typedef struct
{
  int32_t native_w, native_h;   /* the frame's pixel size */
  int32_t l, t, r, b;
  uint32_t bg_argb;             /* a fresh backing's colour, premultiplied ARGB as Cairo stores it */
  uint32_t scale120;            /* the window's scale times 120 (0: 120) */
  uint32_t flags;               /* NHG_GEOM_* */
  NhgRect layer;                /* the Glitnir layer in the backing */
  uint64_t window;
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
   Glitnir root id instead, because a popup, tip or child frame gets a new window each time it is shown.  Windows are
   made and destroyed through the glue (nhg_window_new, nhg_window_destroy), which registers and forgets them; C
   forgets a window the shell dropped with its parent's (nhg_frame_forget).  A forgotten
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
/* Whether the frame draws its own title bar (ON non-zero).  C no longer calls it: nhg_csd_layout sets the fact; the
   tests use it.  */
int32_t nhg_frame_csd (NhgDisplay *d, uint64_t window, int32_t on);

/* ---- The event step ------------------------------------------------------------------------------------------------

   read_socket_hook loops on nhg_next, which takes exactly one shell event and answers with its actions; C applies
   them (the Emacs calls) before it asks again, so each decision sees what the earlier events changed.  Before each
   call C samples NHG_IN_* again.  An action whose window maps to no frame is skipped.  Neither reads nor writes
   pixels.  */

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
#define NHG_ACT_DELETE           9   /* the compositor asks to close the window, or dismissed its popup (FLAGS
					NHG_DEL_TIP, NHG_DEL_POPUP) */
#define NHG_ACT_FOCUS_IN        10
#define NHG_ACT_FOCUS_OUT       11
#define NHG_ACT_VISIBLE         12
#define NHG_ACT_ICONIFIED       13
#define NHG_ACT_READY           14   /* the window may present: C runs the present step, drawn or not */
#define NHG_ACT_MONITORS        15   /* CODE the added or changed output's scale times 120 (0: one went) */
#define NHG_ACT_LOST            16   /* the connection is gone */
#define NHG_ACT_PREEDIT         17
#define NHG_ACT_COMMIT          18
#define NHG_ACT_IM_DELETE       19
#define NHG_ACT_IM_SURROUNDING  20
#define NHG_ACT_SELECTION_CLEAR 21
#define NHG_ACT_TITLE_BAR       22   /* a pointer event went through the title bar: FLAGS NHG_TB_*, CODE the pointer
					shape (NHSHELL_CURSOR_*) with NHG_TB_POINTER; the event's own action (MOTION,
					LEAVE, CLICK), if any, comes after it */

#define NHG_RESIZE_FULLSCREEN    1   /* fullscreen or maximized changed: C stores the fullscreen parameter */

/* MOTION (an enter is one): DX, DY the pointer's position in device pixels in the window's surface.  LEAVE: the
   window.  CLICK: CODE the evdev button, FLAGS the NHSHELL_BUTTON_* state, DX, DY the position, MODS the keyboard's.
   WHEEL: CODE NHG_WHEEL_UP, _DOWN, _LEFT or _RIGHT (as pgtk's buttons 4 to 7), W the whole steps, DX, DY the
   record's pixel delta in device pixels, MODS the keyboard's, FLAGS NHG_WHEEL_*: none (W events, one step each),
   HIRES (one event with the pixel delta, no steps), STOP (a gesture ended; momentum begins) or COALESCED (W steps in
   one event).  PREEDIT and COMMIT: TEXT, LEN (NULL: it could not be taken; empty: the preedit is cleared), WINDOW as
   the event came, even one that is not a frame, or 0; C sends them to the frame of its tree that Emacs focused.
   IM_DELETE: X bytes before point, Y after.  */
#define NHG_WHEEL_HIRES          1
#define NHG_WHEEL_STOP           2
#define NHG_WHEEL_COALESCED      4
#define NHG_WHEEL_UP             4
#define NHG_WHEEL_DOWN           5
#define NHG_WHEEL_LEFT           6
#define NHG_WHEEL_RIGHT          7

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

/* The first panic's report on D (message, location, backtrace): up to CAP - 1 bytes and a NUL into BUF, its full
   length into *LEN (0: no panic).  Works on a poisoned display.  */
int32_t nhg_panic_message (NhgDisplay *d, char *buf, size_t cap, size_t *len);

/* ---- The title bar -------------------------------------------------------------------------------------------------

   The client-side title bar's layout, hit tests, buttons' state and window geometry are the glue's; C paints the
   strip from the draw list nhg_csd_paint gives, with the colours and the font it resolves itself.  Pointer events
   over the border are taken in the event step: a press moves, resizes, maximizes, minimizes or shows the window menu
   through the shell, and a click on close gives an NHG_ACT_DELETE.  Device pixels of the backing.  Neither reads nor
   writes pixels.  */

#define NHG_CSD_CLOSE        1   /* button kinds */
#define NHG_CSD_MAX          2
#define NHG_CSD_MIN          3
#define NHG_CSD_MENU         4
#define NHG_CSD_MAX_BUTTONS  8
#define NHG_BTN_HOVER        1   /* a button's state: under the pointer */
#define NHG_BTN_PRESS        2   /* pressed (armed) and under the pointer */
#define NHG_CSD_UNDECORATED  1   /* nhg_csd_layout flags: the frame is undecorated */
#define NHG_TB_REDRAW        1   /* NHG_ACT_TITLE_BAR flags: the strip's look changed (C redisplays the frame) */
#define NHG_TB_POINTER       2   /* the pointer is over the border: C clears the mouse face, shows CODE's shape */
#define NHG_TB_LEFT          4   /* the pointer left the border for the native area: C forgets its pointer shape */

/* The native area's offsets in the backing (T includes the strip), the strip's height (0: none), and ON whether the
   frame draws its own title bar.  */
typedef struct { int32_t l, t, r, b, strip; uint32_t on; } NhgCsdLayout;

/* The strip's draw list: the strip, the box the title is centred in, whether the window is active, and the buttons
   (NHG_CSD_* kind, NHG_BTN_* state, cell).  */
typedef struct
{
  NhgRect strip, title;
  uint32_t active, nbuttons;
  struct { uint32_t kind, state; NhgRect r; } button[NHG_CSD_MAX_BUTTONS];
} NhgCsdPaint;

/* The title bars' style from Lisp: each side's buttons in order (other codes are ignored); every strip is painted
   again.  */
int32_t nhg_csd_style (NhgDisplay *d, const uint32_t *left, size_t nleft, const uint32_t *right, size_t nright);
/* Lay WINDOW's border out again from the compositor's decoration answer, the toplevel's states, SCALE120 (0: 120)
   and FLAGS: 1 when it changed (C garbages the frame), 0 when not, the layout in *OUT (OUT may be NULL).  Sets the
   frame's CSD fact.  A child frame's border is its shadow margin instead, from its chrome and SCALE120 (no strip).
   When a top-level's border changes, its child frames are placed again.  */
int32_t nhg_csd_layout (NhgDisplay *d, uint64_t window, uint32_t scale120, uint32_t flags, NhgCsdLayout *out);
/* WINDOW's offsets around its native area: OUT gets L, T, R, B (a child frame's: its shadow margin).  */
int32_t nhg_frame_offsets (NhgDisplay *d, uint64_t window, int32_t out[4]);
/* The publish step of a drawable WINDOW, with its native size, before nhg_present: a child frame's position and input
   region go to the shell if they changed, the window geometry if it changed, then 1 with the draw list in *OUT when C
   must paint the strip and damage the border, 0 when the backing already shows it.  LOOK hashes what C paints from
   and the glue does not see (the title, the colours, the font, the backing's generation).  */
int32_t nhg_csd_paint (NhgDisplay *d, uint64_t window, int32_t native_w, int32_t native_h, uint64_t look,
		       NhgCsdPaint *out);
/* WINDOW's strip as laid out for that native size (nhwl--title-bar): 1 with it in *OUT, 0 when it draws none.  */
int32_t nhg_csd_title_bar (NhgDisplay *d, uint64_t window, int32_t native_w, int32_t native_h, NhgCsdPaint *out);

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
   and its highlighting.  A popup frame's focus, and the focus a top-level lends a grabbing popup frame of its own,
   give none (nhg_popup_hide says when a lent focus is lost).  */

#define NHG_KEY_REPEAT           1   /* the key is a repeat */

/* ---- Selections, display queries, the bench, the bell and the pointer -------------------------------------------
   None of these reads or writes pixels except nhg_bench_full_redraw, which reads them.  KIND is NHSHELL_SEL_CLIPBOARD
   or NHSHELL_SEL_PRIMARY.  A buffer a call lends stays valid until the next call of the same entry point.  */

/* Offer N types as selection KIND: MIMES[i] with the LENS[i] bytes at DATA[i], copied.  NHG_E_STATE without keyboard
   focus; the glue then owns nothing.  */
int32_t nhg_selection_set (NhgDisplay *d, uint32_t kind, const char *const *mimes, const uint8_t *const *data,
                           const size_t *lens, size_t n);

/* Give selection KIND up: 1 when this process owned it, 0 when it did not.  */
int32_t nhg_selection_disown (NhgDisplay *d, uint32_t kind);

/* 1 while this process's source is the current selection KIND, else 0.  */
int32_t nhg_selection_owned (NhgDisplay *d, uint32_t kind);

/* The number of MIME types of selection KIND (0: no selection).  *OUT and *LEN get the types, each followed by a NUL,
   and their byte count; either may be NULL.  */
int32_t nhg_selection_types (NhgDisplay *d, uint32_t kind, const char **out, size_t *len);

/* Selection KIND as MIME into *OUT, *LEN: our own bytes at once, another client's within TIMEOUT_MS
   (NHG_E_TIMEOUT).  NHG_E_DISCONNECTED within 100 ms of the connection's loss.  NHG_E_STATE without a selection or
   without that type.  */
int32_t nhg_selection_read (NhgDisplay *d, uint32_t kind, const char *mime, uint32_t timeout_ms, const uint8_t **out,
                            size_t *len);

/* A change of selection KIND that took it from us comes out of nhg_next as a SELECTION_CLEAR action (code: KIND; the
   ownership is cleared).  */

/* One output, as the shell reports it: X and Y in logical pixels, W and H the mode in device pixels.  */
typedef struct
{
  uint64_t id;
  int32_t x, y, w, h, mm_w, mm_h;
  uint32_t scale120;
  char name[32];
} NhgMonitor;

/* The outputs come from nhg_next (a MONITORS action says the list changed); nhg_monitors gives the list.  */

/* The display as x-display-* reads it: its extent in device pixels (1920 x 1080 with no output) and in millimetres,
   the first output's scale times 120 (0: none yet) and the shell's capabilities.  */
typedef struct
{
  int32_t w, h, mm_w, mm_h;
  uint32_t scale120;
  NhShellCaps caps;
} NhgDisplayInfo;
int32_t nhg_display_info (NhgDisplay *d, NhgDisplayInfo *out);

/* Present FRAMES frames (1 to 100000) of the tree of the N surfaces S (as nhg_present takes them: bottom to top, the
   root first), each damaged whole and presented through the presenter, and time them: the frames that committed, the
   median and 95th percentile of one present in milliseconds (the publish, and the try that committed), and the 95th
   percentile of the time between two commits.  NHG_E_STATE when the root is not registered, has no backing, or fewer
   than two frames committed (OUT->frames says how many).  Waits at most 5 s for each frame.  Reads pixels.  */
typedef struct { uint64_t frames; double compose_p50, compose_p95, interval_p95; } NhgBenchOut;
int32_t nhg_bench_full_redraw (NhgDisplay *d, const NhgSurface *s, size_t n, uint32_t frames, NhgBenchOut *out);

/* Ring the system bell on WINDOW's surface (0: none), at most once per 20 ms.  NHG_E_UNSUPPORTED without the
   protocol.  */
int32_t nhg_bell (NhgDisplay *d, uint64_t window);

/* Show (ON) the pointer over WINDOW as SHAPE (NHSHELL_CURSOR_*), or hide it.  */
int32_t nhg_pointer_visible (NhgDisplay *d, uint64_t window, int32_t on, uint32_t shape);

/* ---- Wheel units, the input method and outputs ---------------------------------------------------------------------
   None of these reads or writes pixels.  */

/* Push: WINDOW's column width and line height in device pixels (FRAME_COLUMN_WIDTH, FRAME_LINE_HEIGHT), the size of a
   wheel step in a finger record.  Call it when the window is made and when its font changes; a checking build compares
   it with Emacs's values.  NHG_E_ARG for a size under 1, NHG_E_STATE for an unknown window.  */
int32_t nhg_frame_units (NhgDisplay *d, uint64_t window, int32_t col_w, int32_t line_h);

/* Text input for WINDOW on or off (NHG_E_UNSUPPORTED without text-input-v3); the cursor rectangle R in logical pixels
   of its surface; the LEN bytes of UTF-8 at TEXT around point, with point at byte CURSOR and the mark at byte ANCHOR of
   them (at most 4000 bytes go to the input method, cut around point).  The compositor is told only what changed.  */
int32_t nhg_im_enable (NhgDisplay *d, uint64_t window, int32_t on);
int32_t nhg_im_cursor (NhgDisplay *d, uint64_t window, NhgRect r);
int32_t nhg_im_surrounding (NhgDisplay *d, uint64_t window, const char *text, size_t len, int32_t cursor,
                            int32_t anchor);

/* Copy up to CAP outputs into OUT; their number goes to *N (may be NULL) and is returned.  At most 16 are known.  */
int32_t nhg_monitors (NhgDisplay *d, NhgMonitor *out, size_t cap, size_t *n);

/* Copy the ids of the outputs WINDOW is on, up to CAP, into OUT; their number goes to *N (may be NULL) and is
   returned.  NHG_E_STATE for an unknown window.  */
int32_t nhg_frame_outputs (NhgDisplay *d, uint64_t window, uint64_t *out, size_t cap, size_t *n);

/* ---- Window operations and tips ------------------------------------------------------------------------------------

   WINDOW is a shell window id; a window the glue does not know (or forgot) gives NHG_E_STATE.  Every window C makes
   goes through nhg_window_new (or nhg_tip_show), which registers it with the facts its frame starts from, so a tip,
   popup or child frame's new window (each show makes one) starts from Emacs's values, and its alpha starts over.
   Texts are UTF-8 with their lengths, valid for the call.  Neither reads nor writes pixels, nor do the calls below.  */

/* Layout equals NhShellAnchor: a rectangle in the parent's surface (logical pixels), the gravity, NHSHELL_ADJUST_*.  */
typedef struct { int32_t x, y, w, h; uint32_t gravity, adjust; } NhgAnchor;

typedef struct
{
  int32_t w, h;                 /* logical pixels */
  const char *title; size_t title_len;
  const char *app_id; size_t app_id_len;
  const char *session_name; size_t session_name_len;   /* top-levels; empty: none */
  uint32_t flags;               /* NHSHELL_WIN_* */
  uint32_t kind;                /* NHSHELL_KIND_* */
  uint64_t parent;              /* children and popups */
  int32_t x, y;                 /* children: the offset in the parent (logical) */
  NhgAnchor anchor;             /* popups */
  uint32_t role;                /* NHG_FRAME_* */
  uint32_t facts;               /* NHG_FACT_VISIBLE, NHG_FACT_ICONIFIED, NHG_FACT_CSD: Emacs's, now */
  int32_t native_w, native_h;   /* the frame's native size as Emacs has it (device pixels) */
  uint32_t scale120;            /* the scale C gives the window (times 120) */
} NhgWindowDesc;

/* Make the window W describes and register it (it replaces nhg_frame_register for new windows), seeded with W's facts;
   its id into *OUT.  A shell error is its code.  */
int32_t nhg_window_new (NhgDisplay *d, const NhgWindowDesc *w, uint64_t *out);
/* The title, LEN bytes (up to a NUL).  */
int32_t nhg_window_title (NhgDisplay *d, uint64_t window, const char *title, size_t len);
/* `undecorated' changed at run time (ON non-zero): a top-level asks the compositor again; a child has nothing to ask.  */
int32_t nhg_window_undecorated (NhgDisplay *d, uint64_t window, int32_t on);

/* nhg_window_alpha's result bits.  DAMAGE: C damages the whole frame.  HOST: the compositor cannot apply the alpha,
   so the shell multiplies the pixels (NHG_SURF_ALPHA_HOST, with *CUT as the surface's ALPHA_CUT).  */
#define NHG_ALPHA_DAMAGE 1
#define NHG_ALPHA_HOST   2
/* Apply the frame's effective alpha ALPHA (0 to 1: C chose the active or inactive value and applied
   frame-alpha-lower-limit) and its alpha-background ALPHA_BG (C paints that into the backing).  NHG_ALPHA_* bits; the
   alpha's cut (what it takes off opaque) into *CUT (may be NULL).  */
int32_t nhg_window_alpha (NhgDisplay *d, uint64_t window, double alpha, double alpha_bg, uint32_t *cut);
/* Push: C resized the frame itself to W x H device pixels (as nhg_frame_resized: a configure being applied gets a
   content ack).  */
int32_t nhg_window_size (NhgDisplay *d, uint64_t window, int32_t w, int32_t h);
/* The frame's smallest size, W x H device pixels (a child's window has none).  */
int32_t nhg_window_min_size (NhgDisplay *d, uint64_t window, int32_t w, int32_t h);
/* Push: Emacs made the frame visible (ON non-zero) or invisible, not iconified.  Checked (nhg_debug_check_frame).  */
int32_t nhg_window_visible (NhgDisplay *d, uint64_t window, int32_t on);
/* Iconify: the compositor minimizes the window; a push too (iconified, not visible).  */
int32_t nhg_window_iconify (NhgDisplay *d, uint64_t window);

/* nhg_window_fullscreen's modes (Emacs's fullscreen_type values; WIDTH and HEIGHT ask for maximized).  */
#define NHG_FULLSCREEN_NONE      0
#define NHG_FULLSCREEN_WIDTH     1
#define NHG_FULLSCREEN_HEIGHT    2
#define NHG_FULLSCREEN_BOTH      3
#define NHG_FULLSCREEN_MAXIMIZED 4
int32_t nhg_window_fullscreen (NhgDisplay *d, uint64_t window, uint32_t mode);
/* Destroy the window and forget its frame (forgotten even when the shell refuses, after a lost connection: C ignores
   the code while it tears down).  */
int32_t nhg_window_destroy (NhgDisplay *d, uint64_t window);
/* One round of the bounded wait for a top-level's first configure: 1 when it is configured; else the work due on this
   thread is done and the shell's descriptor waited on for at most TIMEOUT_MS: 0 when something is queued (C drains,
   then calls again), NHG_E_TIMEOUT when nothing came.  */
int32_t nhg_window_wait_configured (NhgDisplay *d, uint64_t window, uint32_t timeout_ms);

/* A tip on PARENT's window at ANCHOR_DEV (device pixels in the parent's surface: the pointer, or the tip's left and
   top) moved by DX, DY.  *WINDOW 0: make the tip's popup, NATIVE_W x NATIVE_H device pixels at the parent's scale, a
   visible tip frame, its id into *WINDOW, and return 1 (draw it whole).  Otherwise only the offsets changed: the popup
   moves with a bare commit, and 1 says the whole tip must be drawn again (below xdg_wm_base 3 the popup is made
   again), 0 that it need not.  */
int32_t nhg_tip_show (NhgDisplay *d, uint64_t *window, uint64_t parent, NhgRect anchor_dev, int32_t dx, int32_t dy,
		      int32_t native_w, int32_t native_h);
/* The tip's popup goes and its frame is forgotten; NHG_E_STATE for a window that is not a tip's.  */
int32_t nhg_tip_hide (NhgDisplay *d, uint64_t window);
/* DELETE flags: the compositor dismissed a tip's popup; C calls nhg_tip_hide and hides the frame (Lisp deletes it).  */
#define NHG_DEL_TIP 1

/* ---- Child frames --------------------------------------------------------------------------------------------------

   A child frame's window (registered with NHG_FRAME_CHILD) is a sync subsurface of its parent frame's: its position,
   stacking, input region and mapping take effect with the tree's commit.  The glue holds the child's layout: the
   shadow margin its chrome needs (its offsets, nhg_frame_offsets), its surface's position in the parent's surface,
   its input region (the native area, without the margin), whether it is mapped, and its stacking.  C pushes the
   child's facts when they change and again for a window made anew; the keyboard focus redirect stays C.  None of
   these reads or writes pixels.  */

#define NHG_CHILD_VISIBLE    1   /* nhg_child_set flags: the child is shown (its surface is mapped) */
#define NHG_CHILD_NO_FOCUS   2   /* the frame is no-accept-focus */

/* The chrome glitnir--resolve gives a child frame, in logical pixels: ON 0 for none (the rest 0 then), the corner
   radius, the shadow's blur, offset (OX, OY), spread and opacity (0 to 1), and its colour RGB (0xRRGGBB).  */
typedef struct
{
  uint32_t on;
  float radius, blur, ox, oy, spread, opacity;
  uint32_t rgb;
} NhgChrome;

/* Push: WINDOW is a child of PARENT's window, its native area at X, Y (device pixels of the parent's native area:
   the frame's left and top), with FLAGS NHG_CHILD_*.  The first push after the window is made records where the
   window was made; a later one sends the position if it changed, and maps or unmaps the surface as
   NHG_CHILD_VISIBLE says.  1 when the surface was mapped again (C damages the frame whole), else 0.  NHG_E_STATE for a
   window that is not a registered child, or an unknown parent.  */
int32_t nhg_child_set (NhgDisplay *d, uint64_t window, uint64_t parent, int32_t x, int32_t y, uint32_t flags);

/* WINDOW's chrome as glitnir--resolve gave it now: 1 when it changed (C lays the frame out again, nhg_csd_layout,
   and garbages it), 0 when not.  The layer takes it at the next nhg_layer_ensure.  */
int32_t nhg_child_chrome (NhgDisplay *d, uint64_t window, const NhgChrome *c);

/* WINDOW was raised or lowered among its siblings: SIBLINGS holds the parent's child windows bottom to top as C now
   stacks them, WINDOW among them, and Z_GROUP is the frame's z-group (Emacs's enum z_group: 0 none, 1 above, 2
   below, 3 above-suspended; C ordered SIBLINGS, the glue keeps the value).
   The glue puts WINDOW right above the sibling below it, or below the one above it when it is the lowest; the
   parent and its ancestors owe a commit.  NHG_E_ARG when WINDOW is not in SIBLINGS.  */
int32_t nhg_child_stack (NhgDisplay *d, uint64_t window, const uint64_t *siblings_bottom_to_top, size_t n,
                         int32_t z_group);

/* ---- Menu popups ---------------------------------------------------------------------------------------------------

   A popup frame's window (registered with NHG_FRAME_POPUP) is an xdg popup of its parent frame's window, made when
   the frame is shown and destroyed when it is hidden; the frame is the root of its own tree.  C decodes the frame's
   `nhwl-popup' parameter (X Y W H . PLIST): ANCHOR_DEV is the rectangle in device pixels of the parent's native area,
   GRAVITY NhgAnchor's (1 to 8), ADJUST NHSHELL_ADJUST_*, FLAGS NHG_POPUP_*.  The glue places it in the parent's
   surface (after its border) at the parent's scale.  Size, gravity, adjust and grab apply at the next show; a new
   parameter moves the shown popup.  A grab needs a recent press's serial: without one the popup grabs nothing.  The
   compositor dismissing it is a DELETE action with NHG_DEL_POPUP: C stores a delete-frame event, and the frame stays
   until Lisp deletes or hides it.  Keyboard focus moving between a top-level and its own popup frames gives no
   FOCUS_IN or FOCUS_OUT.  None of these reads or writes pixels.  */

#define NHG_POPUP_GRAB 1   /* the popup takes the keyboard and pointer; a click elsewhere dismisses it */
#define NHG_DEL_POPUP  2   /* DELETE flags: the compositor dismissed a popup frame's popup */

/* Show a popup frame: a new popup of PARENT's window at ANCHOR_DEV, NATIVE_W x NATIVE_H device pixels at the parent's
   scale, hanging from top-level OWNER (whose keyboard focus is kept while the popup grabs); registered as a visible
   popup frame, its id into *WINDOW.  A shell error is its code.  */
int32_t nhg_popup_show (NhgDisplay *d, uint64_t *window, uint64_t parent, uint64_t owner, NhgRect anchor_dev,
                        uint32_t gravity, uint32_t adjust, uint32_t flags, int32_t native_w, int32_t native_h);
/* The popup frame's parameter changed: 0 when it did not; otherwise the shown popup moves to the new anchor
   (xdg_popup.reposition), and 1 says the whole frame must be drawn again (below xdg_wm_base 3 the popup is made
   again).  */
int32_t nhg_popup_place (NhgDisplay *d, uint64_t window, NhgRect anchor_dev, uint32_t gravity, uint32_t adjust,
                         uint32_t flags);
/* Push: the popup frame's size is now W x H device pixels; the popup takes it, placed again by its anchor.  1 when
   the whole frame must be drawn again (below xdg_wm_base 3), else 0.  */
int32_t nhg_popup_size (NhgDisplay *d, uint64_t window, int32_t w, int32_t h);
/* Hide a popup frame: WINDOW (0: none) goes and is forgotten (also when the shell refuses).  1 when top-level OWNER
   had lent its keyboard focus to a grabbing popup, none of its popups grabs now, and the keyboard is on none of our
   windows: C sends Emacs the focus-out (nhwl_focus_lost).  NHG_E_STATE for a window that is not a popup frame's.  */
int32_t nhg_popup_hide (NhgDisplay *d, uint64_t window, uint64_t owner);

/* ---- Window layers -------------------------------------------------------------------------------------------------
   An Emacs window that scrolls whole gets a surface of its own (the shell's content layer), whose viewport moves with
   the scroll, so the compositor uploads only the rows the scroll exposed.  The glue decides: a window's first
   full-window scroll (its run moves every row of the window but the exposed ones) on an opaque root (a top-level,
   alpha 1 applied by the compositor, alpha-background 1, wp_viewporter present, a scale nhwl-scroll-layers allows)
   promotes it, at most four per frame; two seconds without a full-window scroll, a resize, split or delete of the
   window, a scale change, or the frame no longer an opaque root demotes it.  The backing still moves its rows.
   WINDOW is the frame's shell window, FRAME its key; FLAGS are NHG_LAYERS_*.  */

#define NHG_LAYERS_ON              1   /* nhwl-scroll-layers is not nil */
#define NHG_LAYERS_INTEGER_SCALES  2   /* it is `integer-scales': only at a whole scale */
#define NHG_LAYERS_OPAQUE_BG       4   /* the frame's alpha-background is 1 */

/* One Emacs window of a frame: its sequence_number and window_box (w, ANY_AREA), with the native area's origin.  */
typedef struct
{
  uint64_t seq;
  NhgRect rect;
} NhgLayerWindow;

/* nhg_scroll for window W_SEQ at RECT (as NhgLayerWindow's): a full-window scroll on an opaque root makes the window
   its own output first; a changed RECT or a frame no longer eligible makes it part of the frame again.  1 when the
   window is its own output now, else 0.  */
int32_t nhg_scroll_window (NhgDisplay *d, uint64_t window, uint64_t frame, uint64_t w_seq, NhgRect rect, NhgRect run,
                           int32_t to_y, uint32_t flags);

/* FRAME is about to be published and its windows are the N at WINDOWS: own outputs that are gone, resized or idle,
   or all of them when the frame is no longer an opaque root, become part of the frame again (in this publish).
   Returns how many stay own outputs.  */
int32_t nhg_layers_sync (NhgDisplay *d, uint64_t window, uint64_t frame, const NhgLayerWindow *windows, size_t n,
                         uint32_t flags);

/* How many of FRAME's windows are their own outputs.  */
int32_t nhg_layers_count (NhgDisplay *d, uint64_t frame);

/* ---- The visible bell, idle inhibition, export handles and activation tokens ------------------------------------ */

/* The frame facts the visible bell's rows come from (pgtk_flash's): the native area, the line height, the internal
   border and the top and bottom margins, in device pixels.  */
typedef struct
{
  int32_t width, height, line_h, border, top_margin, bottom_margin;
} NhgBellGeom;

/* The visible bell: invert the rows G names (the top and bottom line of a tall frame, all of a short one) in the
   backing of the frame with key KEY, keep what they held and damage them.  Returns the rows inverted.  A flash still
   showing is put back first.  NHG_E_STATE for a frame without a backing.  Writes pixels: flush the surface before and
   mark it dirty after.  */
int32_t nhg_bell_flash (NhgDisplay *d, uint64_t key, const NhgBellGeom *g);

/* Put back what nhg_bell_flash inverted, except the pixels a draw has replaced since (damage and scroll release
   them), and damage the rows.  Returns the rows written, 0 when none were left.  Writes pixels: flush before, mark
   dirty after.  C calls it from its 50 ms atimer, and cancels the timer when the frame goes.  */
int32_t nhg_bell_restore (NhgDisplay *d, uint64_t key);

/* Hold (ON non-zero) or release an idle inhibitor on WINDOW's surface.  1 when that changed it, 0 when it was so
   already.  NHG_E_UNSUPPORTED without the protocol.  */
int32_t nhg_inhibit_idle (NhgDisplay *d, uint64_t window, int32_t on);

/* The xdg-foreign handle of WINDOW's top-level (a child frame's root), without a scheme prefix, exported at the first
   call and kept until the window goes; waits at most TIMEOUT_MS.  NUL-terminated into BUF (CAP bytes), the length
   into *LEN (may be NULL).  NHG_E_TIMEOUT, NHG_E_UNSUPPORTED without the exporter, NHG_E_TOO_SMALL.  */
int32_t nhg_window_export (NhgDisplay *d, uint64_t window, uint32_t timeout_ms, char *buf, size_t cap, size_t *len);

/* A fresh activation token made on WINDOW's top-level with the newest input serial, within TIMEOUT_MS, into BUF as
   above.  NHG_E_STATE before any input, NHG_E_TIMEOUT, NHG_E_UNSUPPORTED, NHG_E_TOO_SMALL.  */
int32_t nhg_activation_token (NhgDisplay *d, uint64_t window, uint32_t timeout_ms, char *buf, size_t cap,
                              size_t *len);

/* ---- Session restore ---------------------------------------------------------------------------------------------- */

/* nhg_session_open reasons: a fresh launch, a recovery after the compositor restarted, a restore of a saved session.  */
#define NHG_SESSION_LAUNCH   0
#define NHG_SESSION_RECOVER  1
#define NHG_SESSION_RESTORE  2

/* NHG_ACT_SESSION: the compositor answered nhg_session_open or took the session over.  CODE: */
#define NHG_ACT_SESSION       41
#define NHG_SESSION_CREATED   0   /* a new session: nhg_session_id gives its id */
#define NHG_SESSION_RESTORED  1   /* the saved one */
#define NHG_SESSION_REPLACED  2   /* another client has the session: names are ignored from now on */

/* Open the compositor's session for REASON.  ID (ID_LEN bytes; NULL or empty: none) is the saved session's id, else a
   new session is asked for.  An open session is dropped first.  NHG_ACT_SESSION answers.  NHG_E_UNSUPPORTED without
   xdg_session_manager_v1.  A window whose NhgWindowDesc has a session_name joins the session before its first commit
   (see nhg_window_new).  */
int32_t nhg_session_open (NhgDisplay *d, const char *id, size_t id_len, uint32_t reason);

/* The open session's id, NUL-terminated into BUF (CAP bytes), the length into *LEN (may be NULL).  NHG_E_STATE before
   the compositor has answered, NHG_E_TOO_SMALL.  */
int32_t nhg_session_id (NhgDisplay *d, char *buf, size_t cap, size_t *len);

/* Take WINDOW out of the session: the compositor forgets its name.  1 when it was named, 0 when not (no session, a
   replaced one, an unnamed or unknown window).  */
int32_t nhg_session_remove (NhgDisplay *d, uint64_t window);

/* ---- The drop target -------------------------------------------------------------------------------------------
   A drag from another client comes out of nhg_next as NHG_ACT_DND_* actions for the window under the pointer: DX, DY
   the position in device pixels, TIME_MS the compositor's drag time (hand it back to nhg_dnd_status and
   nhg_dnd_finish), CODE the action (NHSHELL_DND_*; 0: none), the source's preferred one for ENTER and MOTION, the
   chosen one for DROP.  LEAVE has the window only.  The offer is read as selection NHSHELL_SEL_DND (2) with
   nhg_selection_types and nhg_selection_read, from the enter until the leave or the finish.  The shell sends no LEAVE
   after a DROP.  */
#define NHG_ACT_DND_ENTER       32
#define NHG_ACT_DND_MOTION      33
#define NHG_ACT_DND_LEAVE       34
#define NHG_ACT_DND_DROP        35

#define NHG_SEL_DND              2   /* the offer's KIND for nhg_selection_types, _owned and _read (NHSHELL_SEL_DND) */
#define NHG_DND_COPY             1   /* the actions (NHSHELL_DND_*) */
#define NHG_DND_MOVE             2
#define NHG_DND_ASK              4

/* Register the N types a drop on WINDOW takes, replacing its list (none: any).  NHG_E_STATE for a window that is not a
   frame.  */
int32_t nhg_dnd_register (NhgDisplay *d, uint64_t window, const char *const *mimes, size_t n);

/* Accept the current drag for ACTION (NHSHELL_DND_*, one bit; 0 accepts nothing) with the type last read from the offer
   (else its first).  1 when the shell was told, 0 for a call older than the drag's enter or without a drag.  */
int32_t nhg_dnd_status (NhgDisplay *d, uint32_t action, uint32_t time_ms);

/* Finish the drop: SUCCESS non-zero when its action was performed.  The offer is destroyed either way.  1 when the
   source was told, 0 for a stale call, without a dropped offer, or with nothing to tell the source.  Call it after every
   DROP, whatever happened to the data.  */
int32_t nhg_dnd_finish (NhgDisplay *d, int32_t success, uint32_t time_ms);

/* Touch and pinch.  NHG_ACT_TOUCH: CODE the point's local id (the glue counts them; they identify a point to Lisp),
   FLAGS NHG_TOUCH_*, X, Y the point's position in device pixels in the window's surface (a BEGIN, an END, a CANCEL).
   An UPDATE (sent only when the point moved by a whole pixel) has no position: its TEXT holds the window's live points,
   oldest first, as LEN / sizeof (NhgTouchPoint) NhgTouchPoint values (copy them out; the text is not aligned).  The
   glue forgets a forgotten window's points without an action.
   NHG_ACT_PINCH (begin and update; the end gives none): DX, DY this update's motion of the fingers' centre in device
   pixels, SCALE the cumulative scale since the begin (1.0 at the begin), X, Y the pointer's position, MODS the
   keyboard's, TEXT an unaligned double, this update's angle in radians, clockwise.  */
#define NHG_ACT_TOUCH         39
#define NHG_ACT_PINCH         40
#define NHG_TOUCH_BEGIN       0
#define NHG_TOUCH_UPDATE      1
#define NHG_TOUCH_END         2
#define NHG_TOUCH_CANCEL      3
typedef struct { int32_t x, y, id; } NhgTouchPoint;

/* ---- Reconnecting in place ---------------------------------------------------------------------------------------
   With nhg_set_reconnect on, a compositor that goes away (a hangup, not a protocol error) gives NHG_ACT_RECONNECTING
   instead of NHG_ACT_LOST: C starts its timer, whose first tick comes CODE milliseconds later, and each tick calls
   nhg_reconnect_step.  NHG_ACT_RECONNECTED (no window) says the display is back: C damages every frame whole, and asks
   nhg_reconnect_settle at each read until it says nothing is pending.  Window ids do not change.  */
#define NHG_ACT_RECONNECTING  42
#define NHG_ACT_RECONNECTED   43
#define NHG_SETTLE_REPORT     1   /* every shown frame presented again, or 2 s passed: run `reconnected' */
#define NHG_SETTLE_MONITORS   2   /* the monitor list differs from before the loss (ids aside) */
#define NHG_SETTLE_FOCUS_OUT  4   /* no keyboard enter 1 s after the report: *WINDOW loses the focus */

/* Reconnect in place when the display goes (ON non-zero; off at open).  NHG_E_UNSUPPORTED when the connection cannot
   be made again (an inherited socket).  */
int32_t nhg_set_reconnect (NhgDisplay *d, int32_t on);

/* One tick of the reconnect timer: a step of at most 50 ms, given up TIMEOUT_MS after the loss.  NHG_OK once connected
   (NHG_ACT_RECONNECTED follows from nhg_next) or when not reconnecting; NHG_E_TIMEOUT with the next tick's delay in
   *NEXT_MS (50, 100, 200, 400, 800 ms, then every second, never past the timeout); NHG_E_DISCONNECTED when the glue
   gave up: drop the display.  */
int32_t nhg_reconnect_step (NhgDisplay *d, uint32_t timeout_ms, uint32_t *next_ms);

/* After NHG_ACT_RECONNECTED: what is due now (NHG_SETTLE_* bits), *WINDOW the window for NHG_SETTLE_FOCUS_OUT, and
   *NEXT_MS when to ask again (0: nothing is pending).  */
int32_t nhg_reconnect_settle (NhgDisplay *d, uint64_t *window, uint32_t *next_ms);

/* ---- The drag source (x-begin-drag) ----------------------------------------------------------------------------
   nhg_drag_start drags N types out of WINDOW (MIMES[i] with the LENS[i] bytes at DATA[i], copied), offering ACTIONS
   (NHG_DND_*), our own windows preferring PREFERRED (one of them), with FLAGS NHG_DRAG_*.  It needs the newest button
   press or touch down whose release has not come: NHG_E_STATE without one (x-begin-drag returns nil at once) or for a
   window that is not a frame.  While the drag runs, nhg_next answers with:
   - NHG_ACT_DRAG_ACTION: the compositor took the drag, or the action it chose with the target changed (CODE; 0: none);
   - NHG_ACT_DRAG_DROPPED: dropped on another client's window, which has to finish it (wait nhwl-dnd-finish-timeout);
   - NHG_ACT_DRAG_DONE, once: the drag is over, CODE the action performed (0: none), and with FLAGS NHG_DRAG_FRAME
     WINDOW the frame to return (RETURN-FRAME; the drag was given up).
   A button release or touch up that reaches us ends the drag with no action: the compositor did not take it.  A drop on
   our own window comes as NHG_ACT_DND_ENTER and NHG_ACT_DND_DROP for pgtk-dnd.el (the glue answered and finished the
   offer, which reads the drag's own bytes), with DONE for its action.  nhg_drag_cancel gives the drag up (no DONE).  */
#define NHG_ACT_DRAG_ACTION     36
#define NHG_ACT_DRAG_DROPPED    37
#define NHG_ACT_DRAG_DONE       38
#define NHG_DRAG_ALLOW_CURRENT   1   /* ALLOW-CURRENT-FRAME: drops on WINDOW are taken */
#define NHG_DRAG_RETURN_FRAME    2   /* RETURN-FRAME: give up for our frame under the pointer once it left WINDOW */
#define NHG_DRAG_RETURN_NOW      4   /* RETURN-FRAME `now': give up for the first of our frames under the pointer */
#define NHG_DRAG_FRAME           1   /* DONE's FLAGS: WINDOW is the frame to return */

int32_t nhg_drag_start (NhgDisplay *d, uint64_t window, const char *const *mimes, const uint8_t *const *data,
                        const size_t *lens, size_t n, uint32_t actions, uint32_t preferred, uint32_t flags);

/* Give our drag up: its source goes, and no NHG_ACT_DRAG_DONE follows.  1 when it ran, else 0.  */
int32_t nhg_drag_cancel (NhgDisplay *d);

/* ---- Giving the display up ---------------------------------------------------------------------------------------
   When the compositor went and nothing waits for it (reconnecting is off, failed or timed out), C calls nhg_give_up
   before it deletes the display's frames: reconnecting stops, a drag over us ends unfinished, and the touch points and
   owed acks go.  Each frame's deletion frees its backing; nhg_close frees what is left.  */
int32_t nhg_give_up (NhgDisplay *d);

/* Why the connection ended: a protocol error's text (INTERFACE@ID: error CODE: MESSAGE), up to CAP - 1 bytes and a
   NUL into BUF, its full length into *LEN (0: the connection lives, or ended with a plain hangup).  */
int32_t nhg_last_error (NhgDisplay *d, char *buf, size_t cap, size_t *len);

#ifdef __cplusplus
}
#endif

#endif /* NHGLUE_H */

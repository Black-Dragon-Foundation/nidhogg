/* nhshell.h -- the C ABI of Nidhogg's native shell layer (crate nhshell, linked through nidhogg/glue's libnhglue.a).

   One shell API for every windowing backend: connection, capabilities, windows, events and the presenter.  Every call
   returns 0 (or a count, where said) or a negative NHSHELL_E_* code; a panic inside the library is caught at the
   boundary and reported as NHSHELL_E_PANIC.  Call everything from one thread (the host's main thread).  Sizes are
   logical pixels; names with DEV are device pixels.  libnhglue.a also carries every glitnir_* symbol.  */

#ifndef NHSHELL_H
#define NHSHELL_H

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

#include "glitnir.h"

#ifdef __cplusplus
extern "C" {
#endif

#define NHSHELL_OK                 0
#define NHSHELL_E_PANIC           -1
#define NHSHELL_E_ARG             -2
#define NHSHELL_E_STATE           -3   /* no such window, no buffer begun */
#define NHSHELL_E_UNSUPPORTED     -4   /* no backend, or a protocol it needs is absent */
#define NHSHELL_E_DISCONNECTED    -5   /* the connection to the display is gone */
#define NHSHELL_E_NOMEM           -6
#define NHSHELL_E_TOO_SMALL       -8   /* nhshell_take_text: the buffer is short; *LEN is the size, the text stays */

/* Write the ABI version (1) to *OUT.  NHSHELL_E_ARG if OUT is NULL.  */
int32_t nhshell_abi_version (uint32_t *out);

typedef struct NhShell NhShell;

/* Capabilities: one protocol version per optional feature, 0 when absent.  A missing protocol makes its feature do
   nothing.  */
typedef struct
{
  uint32_t compositor, subcompositor, seat, xdg_wm_base, xdg_decoration, fractional_scale, viewporter, cursor_shape,
    text_input_v3, primary_selection, data_device, presentation, fifo, commit_timing, alpha_modifier, activation,
    pointer_warp, system_bell, session_management, background_effect, pointer_gestures, idle_inhibit,
    single_pixel_buffer, dmabuf, xdg_foreign;
  bool ssd;        /* server-side decorations were granted, after negotiation */
  bool force_csd;  /* client-side decorations are forced (a test switch) */
} NhShellCaps;

/* The shell's events are Rust records (crate nhshell, event.rs), which the glue reads; the values below are the ones
   C still uses.  */
#define NHSHELL_SESSION_CREATED  0u
#define NHSHELL_SESSION_RESTORED 1u
#define NHSHELL_SESSION_REPLACED 2u
#define NHSHELL_TOUCH_DOWN   0u
#define NHSHELL_TOUCH_MOTION 1u
#define NHSHELL_TOUCH_UP     2u
#define NHSHELL_TOUCH_CANCEL 3u
#define NHSHELL_PINCH_BEGIN  0u
#define NHSHELL_PINCH_UPDATE 1u
#define NHSHELL_PINCH_END    2u

/* key modifiers */
#define NHSHELL_MOD_SHIFT (1u << 0)
#define NHSHELL_MOD_CTRL  (1u << 1)
#define NHSHELL_MOD_ALT   (1u << 2)
#define NHSHELL_MOD_SUPER (1u << 3)
#define NHSHELL_MOD_HYPER (1u << 4)
#define NHSHELL_MOD_META  (1u << 5)
#define NHSHELL_MOD_CAPS  (1u << 6)

/* a configure's states */
#define NHSHELL_STATE_MAXIMIZED  (1u << 0)
#define NHSHELL_STATE_FULLSCREEN (1u << 1)
#define NHSHELL_STATE_ACTIVATED  (1u << 2)
#define NHSHELL_STATE_RESIZING   (1u << 3)
#define NHSHELL_STATE_TILED_L    (1u << 4)
#define NHSHELL_STATE_TILED_R    (1u << 5)
#define NHSHELL_STATE_TILED_T    (1u << 6)
#define NHSHELL_STATE_TILED_B    (1u << 7)
#define NHSHELL_STATE_SUSPENDED  (1u << 8)

/* a button's state */
#define NHSHELL_BUTTON_RELEASED 0
#define NHSHELL_BUTTON_PRESSED  1

/* a scroll's source */
#define NHSHELL_SCROLL_WHEEL      0
#define NHSHELL_SCROLL_FINGER     1
#define NHSHELL_SCROLL_CONTINUOUS 2

/* Window flags and interactive-resize edges (OR the edges for a corner).  */
#define NHSHELL_WIN_UNDECORATED (1u << 0)
#define NHSHELL_WIN_FULLSCREEN  (1u << 1)
#define NHSHELL_WIN_MAXIMIZED   (1u << 2)
/* A popup that takes the keyboard and pointer (a menu): it grabs with the newest button press, key press or touch
   down serial, unless a drag is in progress, there is no serial, or it would not become the topmost grab (its parent
   must be a top-level window while no popup holds a grab, or the popup holding the topmost grab).  Without the grab
   it still works by keys through its parent's focus, but an outside click does not dismiss it.  */
#define NHSHELL_WIN_GRAB        (1u << 3)
/* With a session name: ask the session to restore the window (before its first commit) instead of just adding it.  */
#define NHSHELL_WIN_RESTORE     (1u << 4)

#define NHSHELL_EDGE_TOP    1u
#define NHSHELL_EDGE_BOTTOM 2u
#define NHSHELL_EDGE_LEFT   4u
#define NHSHELL_EDGE_RIGHT  8u

/* Pointer shapes for nhshell_window_set_cursor.  */
enum
{
  NHSHELL_CURSOR_DEFAULT, NHSHELL_CURSOR_TEXT, NHSHELL_CURSOR_POINTER, NHSHELL_CURSOR_CROSSHAIR,
  NHSHELL_CURSOR_WAIT, NHSHELL_CURSOR_HELP, NHSHELL_CURSOR_MOVE, NHSHELL_CURSOR_NOT_ALLOWED,
  NHSHELL_CURSOR_RESIZE_NS, NHSHELL_CURSOR_RESIZE_EW, NHSHELL_CURSOR_RESIZE_NWSE, NHSHELL_CURSOR_RESIZE_NESW,
  NHSHELL_CURSOR_HIDDEN
};

/* Window kinds: a child is a sync subsurface of its parent, presented with its tree (what it shows, and its position,
   stacking, input region and unmap, take effect once every ancestor has committed after it, the root last); a popup
   is placed by the compositor beside an anchor rectangle, and is a tree of its own.  */
#define NHSHELL_KIND_TOPLEVEL 0u
#define NHSHELL_KIND_CHILD    1u
#define NHSHELL_KIND_POPUP    2u

#define NHSHELL_ADJUST_SLIDE_X (1u << 0)
#define NHSHELL_ADJUST_SLIDE_Y (1u << 1)
#define NHSHELL_ADJUST_FLIP_X  (1u << 2)
#define NHSHELL_ADJUST_FLIP_Y  (1u << 3)

/* The rectangle (logical pixels in the parent) a popup sits beside.  GRAVITY is the side or corner it grows toward:
   0 none, 1 top, 2 bottom, 3 left, 4 right, 5 top-left, 6 bottom-left, 7 top-right, 8 bottom-right; the popup's
   matching corner touches the same corner of the rectangle (8: below and right of it).  ADJUST: NHSHELL_ADJUST_*.  */
typedef struct
{
  int32_t x, y, w, h;
  uint32_t gravity;
  uint32_t adjust;
} NhShellAnchor;

typedef struct
{
  int32_t w, h;                    /* logical pixels */
  const char *title, *app_id;      /* NULL: "" */
  uint32_t flags;                  /* NHSHELL_WIN_* (a popup: NHSHELL_WIN_GRAB only) */
  uint32_t kind;                  /* NHSHELL_KIND_*; 0 for a top-level window (the fields below are for the others) */
  uint64_t parent;                 /* children and popups: the parent window */
  int32_t x, y;                    /* children: offset in the parent, logical pixels */
  NhShellAnchor anchor;            /* popups: where to put it (x, y are ignored) */
  const char *session_name;        /* top-levels: the session name (NULL: none); see nhshell_session_open */
} NhShellWindowDesc;

/* A pixel buffer of the shell's presenter (its layout is the Rust side's).  AGE is how many presents ago these pixels
   were last shown (0: contents undefined, draw everything).  */
typedef struct
{
  uint8_t *pixels;
  int32_t w, h, stride;
  uint32_t age;
} NhShellBuffer;

/* Connection.  open connects to DISPLAY (NULL: the environment's); NHSHELL_E_UNSUPPORTED if no backend can.  close
   frees the handle (NULL accepted).  wake_fd returns a descriptor (>= 0) that is readable when events are queued.  */
int32_t nhshell_open (const char *display, NhShell **out);
int32_t nhshell_close (NhShell *sh);
int32_t nhshell_wake_fd (NhShell *sh);
int32_t nhshell_caps (NhShell *sh, NhShellCaps *out);

/* dispatch_main runs the work that must happen on the host's thread (configure and surface events) and flushes.  The
   events themselves are read through the glue.  */
int32_t nhshell_dispatch_main (NhShell *sh);

/* Windows.  A window is a 64-bit id.  move, resize and show_menu use the latest input serial.  */
int32_t nhshell_window_new (NhShell *sh, const NhShellWindowDesc *desc, uint64_t *out);
int32_t nhshell_window_destroy (NhShell *sh, uint64_t window);
int32_t nhshell_window_set_title (NhShell *sh, uint64_t window, const char *title);
int32_t nhshell_window_set_app_id (NhShell *sh, uint64_t window, const char *app_id);
int32_t nhshell_window_set_min_size (NhShell *sh, uint64_t window, int32_t w, int32_t h);
int32_t nhshell_window_set_fullscreen (NhShell *sh, uint64_t window, int32_t on);
int32_t nhshell_window_set_maximized (NhShell *sh, uint64_t window, int32_t on);
int32_t nhshell_window_minimize (NhShell *sh, uint64_t window);
/* A top-level's `undecorated' changed at run time: ON asks again for server-side decorations, off for none (the
   compositor answers with a configure; nothing when client-side decorations are forced).  */
int32_t nhshell_window_set_decorated (NhShell *sh, uint64_t window, int32_t on);
/* The window's opacity (wp_alpha_modifier_v1): FACTOR multiplies its pixels' alpha, UINT32_MAX is opaque.  The window
   owes a commit (a child's ancestors too).  NHSHELL_E_UNSUPPORTED without the protocol: the host multiplies.  */
int32_t nhshell_window_set_alpha (NhShell *sh, uint64_t window, uint32_t factor);
int32_t nhshell_window_move (NhShell *sh, uint64_t window);
int32_t nhshell_window_resize (NhShell *sh, uint64_t window, uint32_t edges);
int32_t nhshell_window_show_menu (NhShell *sh, uint64_t window, int32_t x, int32_t y);
/* The window geometry (surface-local logical pixels): where the window sits inside the surface, for a client that
   draws its own decorations and resize margins.  Takes effect with the next commit that carries a buffer.  */
int32_t nhshell_window_set_geometry (NhShell *sh, uint64_t window, int32_t x, int32_t y, int32_t w, int32_t h);
/* Records a configure's ack; it goes out right before the commit that carries it, exactly once (the newest serial
   wins).  FLAGS NHSHELL_ACK_CONTENT says the ack changes what the window shows (a new size, the first configure, a
   client-side state change): it is sent only with a buffer, never with a bare commit, and stays marked until a
   buffer carries it.  Without the flag a bare commit may carry it.  */
#define NHSHELL_ACK_CONTENT 1u
int32_t nhshell_window_ack_configure (NhShell *sh, uint64_t window, uint32_t serial, uint32_t flags);
int32_t nhshell_window_set_cursor (NhShell *sh, uint64_t window, uint32_t shape);
/* The Xcursor theme (NAME, logical SIZE) shown where the compositor lacks wp_cursor_shape_v1; a cursor already shown
   from the theme is redrawn.  NHSHELL_CURSOR_HIDDEN shows no cursor.  */
int32_t nhshell_set_cursor_theme (NhShell *sh, const char *name, uint32_t size);
/* Ask the compositor to activate the window (xdg-activation).  TOKEN is the activation token to use (emacsclient's
   XDG_ACTIVATION_TOKEN); NULL requests one with the latest input serial and the window's surface, and activates when
   it arrives (NHSHELL_E_STATE before any input; NHSHELL_E_UNSUPPORTED without xdg_activation_v1).  */
int32_t nhshell_window_activate (NhShell *sh, uint64_t window, const char *token);
/* A fresh activation token made with the newest input serial and WINDOW's surface, copied to BUF (CAP bytes, no NUL)
   with its length in *LEN.  Waits at most TIMEOUT_MS (NHSHELL_E_TIMEOUT); NHSHELL_E_STATE before any input;
   NHSHELL_E_UNSUPPORTED without xdg_activation_v1; NHSHELL_E_TOO_SMALL when CAP is short (*LEN is the size).  Queued
   input records stay queued while it waits.  */
int32_t nhshell_activation_token (NhShell *sh, uint64_t window, uint32_t timeout_ms, char *buf, size_t cap,
                                  size_t *len);

/* The system bell on WINDOW's surface (0: no surface), at most once per 20 ms; extra rings are dropped.
   NHSHELL_E_UNSUPPORTED without xdg_system_bell_v1.  The visible bell is the host's drawing.  */
int32_t nhshell_bell (NhShell *sh, uint64_t window);
/* ON non-zero: hold an idle inhibitor on WINDOW's surface (one per window); zero: release it.  NHSHELL_E_UNSUPPORTED
   without zwp_idle_inhibit_manager_v1.  */
int32_t nhshell_window_inhibit_idle (NhShell *sh, uint64_t window, int32_t on);
/* WINDOW's xdg-foreign handle (the zxdg_exported_v2 string, without a "wayland:" prefix) into BUF, as above.  It is
   exported at the first call and cached until the window is destroyed.  NHSHELL_E_UNSUPPORTED without
   zxdg_exporter_v2.  */
int32_t nhshell_window_export (NhShell *sh, uint64_t window, uint32_t timeout_ms, char *buf, size_t cap, size_t *len);

/* Sessions (xdg_session_manager_v1; NHSHELL_E_UNSUPPORTED without it).  open: ID NULL for a new session, REASON
   NHSHELL_SESSION_LAUNCH, _RECOVER or _RESTORE (0, 1, 2); an open session is dropped first.  A session record
   answers, and session_id then gives the id (NHSHELL_E_STATE before).  A window with a session_name in its descriptor
   joins the session before its first commit, as a restore with NHSHELL_WIN_RESTORE; a name already held by a live
   window is NHSHELL_E_STATE.  With no session, or a REPLACED one, names are ignored.  session_remove takes WINDOW out of
   the session (NHSHELL_E_STATE for an unnamed window).  */
#define NHSHELL_SESSION_LAUNCH  0u
#define NHSHELL_SESSION_RECOVER 1u
#define NHSHELL_SESSION_RESTORE 2u
int32_t nhshell_session_open (NhShell *sh, const char *id, uint32_t reason);
int32_t nhshell_session_id (NhShell *sh, char *buf, size_t cap, size_t *len);
int32_t nhshell_window_session_remove (NhShell *sh, uint64_t window);

/* Children and popups.  set_position: a child moves with its parent's next commit; a popup's anchor rectangle moves to
   (X, Y) (xdg_popup.reposition).  stack: put a child above or below SIBLING (0: the parent), with the parent's next
   commit.  set_position returns 1 when the whole window must be redrawn (a popup made again below xdg_popup v3),
   0 otherwise; a child's position, stacking, input region and unmap make each ancestor up to the root owe a commit
   (all but stacking the child too), and go out with the tree's next commits.  set_input_region: the part that takes
   input, N rectangles in logical pixels (N 0:
   the whole surface).
   set_mapped: off shows nothing (a null buffer); on lets the next present show the window.  Pointer events over a child
   or popup carry its id.  destroy of a window also destroys its children and popups.  */
int32_t nhshell_window_set_position (NhShell *sh, uint64_t window, int32_t x, int32_t y);
int32_t nhshell_window_stack (NhShell *sh, uint64_t window, uint64_t sibling, int32_t above);
int32_t nhshell_window_set_input_region (NhShell *sh, uint64_t window, const GlitnirRect *rects, size_t n);
int32_t nhshell_window_set_mapped (NhShell *sh, uint64_t window, int32_t on);
/* Popups that grab.  grabbed returns 1 while popup WINDOW holds a grab (asked with NHSHELL_WIN_GRAB, granted, not
   dismissed by the compositor), else 0.  Destroying a popup first destroys its popup descendants and every grabbing
   popup above it, topmost first.  set_size gives a popup a new size (logical pixels), placed again by its anchor; it
   returns 1 when the whole window must be redrawn (a popup made again below xdg_popup v3), 0 otherwise, and the next
   configure asks for a buffer of the new size.  Below v3, set_size and set_position refuse (NHSHELL_E_STATE) a popup
   that has popups of its own.  NHSHELL_E_STATE for a window that is not a popup.  Key, pointer and focus events on a
   popup carry its id.  */
int32_t nhshell_window_grabbed (NhShell *sh, uint64_t window);
int32_t nhshell_window_set_size (NhShell *sh, uint64_t window, int32_t w, int32_t h);

/* The window's preferred scale times 120 (120 until the compositor says), and whether the compositor decorates it
   (*SSD 1 or 0; caps.ssd is the latest answer for any window).  */
int32_t nhshell_window_scale (NhShell *sh, uint64_t window, uint32_t *scale120);
int32_t nhshell_window_ssd (NhShell *sh, uint64_t window, int32_t *ssd);

/* Publishing: the shell presents.  One pacer per tree (a root and its child windows): a frame-ready record comes for
   roots only, when the pacer lets the tree present (frame callbacks pace, a content ack never waits, a starved tree
   waits for a buffer release, and a commit owed for surface state or a re-arm wakes it too).
   At a consistency point the host publishes a tree (a root and its child windows): one NhShellSurfaceFrame per
   drawable surface, the root first, then its child windows deepest first.  publish commits each surface's Glitnir
   scene (GLITNIR_ROOT), copies the damaged pixels of its backing (PIXELS: H rows of STRIDE bytes, W x H device pixels,
   premultiplied ARGB32, read during the call only) and queues one packet for the tree.  It returns 1 when a packet
   was queued (damage, or a recorded content ack that waits for a picture drawn after it), 0 when there was nothing.
   (SRC_X, SRC_Y) is where the surface's layer starts in its backing (a child frame's shadow margin).  Flags:
   NHSHELL_SF_DRAWABLE, the surface may show a buffer (without it, its damage waits in Glitnir); NHSHELL_SF_ALPHA_HOST,
   the shell multiplies the surface by 1 - ALPHA_CUT / UINT32_MAX (no wp_alpha_modifier_v1).  T_KEY_NS is the
   CLOCK_MONOTONIC time of the key the picture answers (0: none); the earliest one not yet shown is kept.
   present_tree presents ROOT's tree by the present path's rule at its FRAME_READY: buffers when a packet left damage
   or a content ack (eligible once a packet was published after it) waits, the child windows deepest first and the
   root last; else bare commits where one is owed; else nothing.  It returns 1 when the root committed a buffer, 0
   otherwise, NHSHELL_E_STATE in thread mode.  set_presenter_thread: ON presents on the shell's own thread, off
   (the default at open) inline, when the host calls present_tree.  presenter_stats fills *OUT.  */
#define NHSHELL_SF_DRAWABLE   1u
#define NHSHELL_SF_ALPHA_HOST 2u
typedef struct
{
  uint64_t window, glitnir_root;
  const uint8_t *pixels;
  int32_t w, h, stride, src_x, src_y;
  uint32_t flags, alpha_cut;
} NhShellSurfaceFrame;
typedef struct
{
  uint64_t packets, superseded, commits, bare_commits, main_wait_max_ns, publish_copy_max_ns, front_trims;
  uint32_t thread_on;
} NhShellPresenterStats;
int32_t nhshell_publish (NhShell *sh, uint64_t root, const NhShellSurfaceFrame *s, size_t n, uint64_t t_key_ns);
int32_t nhshell_present_tree (NhShell *sh, uint64_t root);
int32_t nhshell_set_presenter_thread (NhShell *sh, int32_t on);
int32_t nhshell_presenter_stats (NhShell *sh, NhShellPresenterStats *out);

/* Selections.  KIND is NHSHELL_SEL_CLIPBOARD or NHSHELL_SEL_PRIMARY.  own copies N types (MIMES[i] with the LENS[i]
   bytes at DATA[i]); other clients' pastes are answered from the copy by the shell's own thread.  It needs keyboard
   focus (NHSHELL_E_STATE without).  owned returns 1 while our source is the current selection, else 0.  types writes
   the current MIME types, each followed by a NUL, to BUF when CAP holds them all, sets *LEN to the bytes needed and
   returns the count.  read returns our own bytes at once, or reads another client's until it finishes or TIMEOUT_MS
   passes (NHSHELL_E_TIMEOUT); NHSHELL_E_STATE when there is no selection or no such type.  Free *OUT with
   nhshell_free_bytes.  */
#define NHSHELL_E_TIMEOUT         -7
#define NHSHELL_SEL_CLIPBOARD 0u
#define NHSHELL_SEL_PRIMARY   1u
int32_t nhshell_selection_own (NhShell *sh, uint32_t kind, const char *const *mimes, const uint8_t *const *data,
                               const size_t *lens, size_t n);
int32_t nhshell_selection_disown (NhShell *sh, uint32_t kind);
int32_t nhshell_selection_owned (NhShell *sh, uint32_t kind);
int32_t nhshell_selection_types (NhShell *sh, uint32_t kind, char *buf, size_t cap, size_t *len);
int32_t nhshell_selection_read (NhShell *sh, uint32_t kind, const char *mime, uint32_t timeout_ms, uint8_t **out,
                                size_t *len);
void nhshell_free_bytes (uint8_t *p);
/* Text input (text-input-v3).  enable turns it on or off for WINDOW; the state survives the window losing and
   regaining text-input focus.  cursor is the cursor rectangle in the window (logical pixels), sent to the input method
   only when it changed.  surrounding is the text around the cursor (UTF-8, LEN bytes; CURSOR and ANCHOR byte offsets),
   cut to 4000 bytes around the cursor on a character boundary.  take_text copies the text of a PREEDIT or COMMIT_TEXT
   record into BUF (CAP bytes, no NUL) and stores its length in *LEN; the id is single-use.  NHSHELL_E_UNSUPPORTED
   without text-input-v3.  */
int32_t nhshell_text_input_enable (NhShell *sh, uint64_t window, int32_t on);
int32_t nhshell_text_input_cursor (NhShell *sh, uint64_t window, int32_t x, int32_t y, int32_t w, int32_t h);
int32_t nhshell_text_input_surrounding (NhShell *sh, uint64_t window, const char *utf8, size_t len, int32_t cursor,
                                        int32_t anchor);
int32_t nhshell_take_text (NhShell *sh, uint32_t id, char *buf, size_t cap, size_t *len);

#ifdef __cplusplus
}
#endif

#endif /* NHSHELL_H */

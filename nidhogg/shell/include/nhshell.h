/* nhshell.h -- the C ABI of Nidhogg's native shell layer (crate nhshell, libnhshell.a).

   One shell API for every windowing backend: connection, capabilities, windows, events and the presenter.  Every call
   returns 0 (or a count, where said) or a negative NHSHELL_E_* code; a panic inside the library is caught at the
   boundary and reported as NHSHELL_E_PANIC.  Call everything from one thread (the host's main thread).  Sizes are
   logical pixels; names with DEV are device pixels.  libnhshell.a also carries every glitnir_* symbol.  */

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
    single_pixel_buffer, dmabuf;
  bool ssd;        /* server-side decorations were granted, after negotiation */
  bool force_csd;  /* client-side decorations are forced (a test switch) */
} NhShellCaps;

/* Event kinds.  KIND says which member of NhShellEvent.u is valid; CLOSE, POINTER_LEAVE, DISCONNECTED and FRAME_READY
   have none.  MODIFIERS uses u.key.mods; POINTER_ENTER, POINTER_MOTION and POINTER_LEAVE use u.pointer.x and y;
   BUTTON uses u.pointer.button and state; OUTPUT_REMOVE uses u.output.id.  Positions are logical pixels; TIME_NS is
   the compositor's clock.  */
enum
{
  NHSHELL_EV_KEY = 1, NHSHELL_EV_MODIFIERS, NHSHELL_EV_POINTER_ENTER, NHSHELL_EV_POINTER_LEAVE,
  NHSHELL_EV_POINTER_MOTION, NHSHELL_EV_BUTTON, NHSHELL_EV_SCROLL, NHSHELL_EV_CONFIGURE, NHSHELL_EV_CLOSE,
  NHSHELL_EV_SCALE, NHSHELL_EV_FOCUS, NHSHELL_EV_FRAME_DONE, NHSHELL_EV_OUTPUT_ADD, NHSHELL_EV_OUTPUT_REMOVE,
  NHSHELL_EV_DISCONNECTED,
  NHSHELL_EV_POPUP_DONE = 16,      /* the compositor dismissed a popup; no payload */
  /* the window's pacer lets the host present what it drew (see nhshell_window_dirty); 16-20 belong to popups and
     children */
  NHSHELL_EV_FRAME_READY = 21
};

/* u.key.mods */
#define NHSHELL_MOD_SHIFT (1u << 0)
#define NHSHELL_MOD_CTRL  (1u << 1)
#define NHSHELL_MOD_ALT   (1u << 2)
#define NHSHELL_MOD_SUPER (1u << 3)
#define NHSHELL_MOD_HYPER (1u << 4)
#define NHSHELL_MOD_META  (1u << 5)
#define NHSHELL_MOD_CAPS  (1u << 6)

/* u.configure.states */
#define NHSHELL_STATE_MAXIMIZED  (1u << 0)
#define NHSHELL_STATE_FULLSCREEN (1u << 1)
#define NHSHELL_STATE_ACTIVATED  (1u << 2)
#define NHSHELL_STATE_RESIZING   (1u << 3)
#define NHSHELL_STATE_TILED_L    (1u << 4)
#define NHSHELL_STATE_TILED_R    (1u << 5)
#define NHSHELL_STATE_TILED_T    (1u << 6)
#define NHSHELL_STATE_TILED_B    (1u << 7)
#define NHSHELL_STATE_SUSPENDED  (1u << 8)

/* u.pointer.state */
#define NHSHELL_BUTTON_RELEASED 0
#define NHSHELL_BUTTON_PRESSED  1

/* u.scroll.source */
#define NHSHELL_SCROLL_WHEEL      0
#define NHSHELL_SCROLL_FINGER     1
#define NHSHELL_SCROLL_CONTINUOUS 2

typedef struct
{
  uint32_t kind;
  uint64_t window;
  uint64_t time_ns;
  union
  {
    struct
    {
      uint32_t keycode;     /* evdev scancode */
      uint32_t keysym;
      uint32_t mods;        /* NHSHELL_MOD_* after this event */
      uint8_t pressed, repeat;
      char utf8[16];        /* the text the key produced, NUL-terminated; "" for none */
    } key;
    struct
    {
      double x, y;
      uint32_t button;      /* evdev button code (BTN_LEFT = 0x110) */
      uint32_t state;
    } pointer;
    struct
    {
      double dx, dy;
      int32_t v120x, v120y; /* wheel steps, 120 per notch; 0 when the source reports none */
      uint32_t source;
      uint8_t stop;
    } scroll;
    struct
    {
      int32_t w, h;         /* the size asked for; 0: your choice */
      uint32_t states;
      uint32_t serial;      /* pass to nhshell_window_ack_configure after committing a buffer of that size */
    } configure;
    struct { uint32_t scale120; } scale;  /* the preferred scale times 120 */
    struct { uint8_t in; } focus;
    struct
    {
      uint64_t presented_ns;  /* 0: no presentation feedback */
      uint32_t refresh_ns;
    } frame_done;
    struct
    {
      uint64_t id;
      int32_t x, y, w, h, mm_w, mm_h;
      uint32_t scale120;
      char name[32];
    } output;
  } u;
} NhShellEvent;

/* Window flags and interactive-resize edges (OR the edges for a corner).  */
#define NHSHELL_WIN_UNDECORATED (1u << 0)
#define NHSHELL_WIN_FULLSCREEN  (1u << 1)
#define NHSHELL_WIN_MAXIMIZED   (1u << 2)

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

/* Window kinds: a child is a subsurface of its parent (moves and stacks with the parent's next commit); a popup is
   placed by the compositor beside an anchor rectangle.  */
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
  uint32_t flags;                  /* NHSHELL_WIN_* */
  uint32_t kind;                   /* NHSHELL_KIND_*; 0 for a top-level window (the fields below are for the others) */
  uint64_t parent;                 /* children and popups: the parent window */
  int32_t x, y;                    /* children: offset in the parent, logical pixels */
  NhShellAnchor anchor;            /* popups: where to put it (x, y are ignored) */
} NhShellWindowDesc;

/* A pixel buffer to draw into until the next nhshell_present_commit.  AGE is how many presents ago these pixels were
   last shown (0: contents undefined, draw everything).  */
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

/* Events.  next_event returns 1 with an event in *OUT, 0 when the queue is empty; when the wake descriptor is readable
   the host calls it until it returns 0 (which also runs pending main-thread work and clears the descriptor).  After
   the connection is lost it yields one DISCONNECTED event, then returns NHSHELL_E_DISCONNECTED.  dispatch_main runs
   the work that must happen on the host's thread (configure and surface events) and flushes.  */
int32_t nhshell_next_event (NhShell *sh, NhShellEvent *out);
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
int32_t nhshell_window_move (NhShell *sh, uint64_t window);
int32_t nhshell_window_resize (NhShell *sh, uint64_t window, uint32_t edges);
int32_t nhshell_window_show_menu (NhShell *sh, uint64_t window, int32_t x, int32_t y);
/* The window geometry (surface-local logical pixels): where the window sits inside the surface, for a client that
   draws its own decorations and resize margins.  Takes effect with the next commit.  */
int32_t nhshell_window_set_geometry (NhShell *sh, uint64_t window, int32_t x, int32_t y, int32_t w, int32_t h);
/* Acks a configure at once; the next present_commit (which is made in any pacer state) carries the buffer.  */
int32_t nhshell_window_ack_configure (NhShell *sh, uint64_t window, uint32_t serial);
int32_t nhshell_window_set_cursor (NhShell *sh, uint64_t window, uint32_t shape);

/* Children and popups.  set_position: a child moves with its parent's next commit; a popup's anchor rectangle moves to
   (X, Y) (xdg_popup.reposition).  stack: put a child above or below SIBLING (0: the parent), with the parent's next
   commit.  set_input_region: the part that takes input, N rectangles in logical pixels (N 0: the whole surface).
   set_mapped: off shows nothing (a null buffer); on lets the next present show the window.  Pointer events over a child
   or popup carry its id.  destroy of a window also destroys its children and popups.  */
int32_t nhshell_window_set_position (NhShell *sh, uint64_t window, int32_t x, int32_t y);
int32_t nhshell_window_stack (NhShell *sh, uint64_t window, uint64_t sibling, int32_t above);
int32_t nhshell_window_set_input_region (NhShell *sh, uint64_t window, const GlitnirRect *rects, size_t n);
int32_t nhshell_window_set_mapped (NhShell *sh, uint64_t window, int32_t on);

/* The window's preferred scale times 120 (120 until the compositor says), and whether the compositor decorates it
   (*SSD 1 or 0; caps.ssd is the latest answer for any window).  */
int32_t nhshell_window_scale (NhShell *sh, uint64_t window, uint32_t *scale120);
int32_t nhshell_window_ssd (NhShell *sh, uint64_t window, int32_t *ssd);

/* The presenter.  begin returns a buffer of DEV_W x DEV_H device pixels in *OUT; commit shows it, with DAMAGE (N
   rectangles in device pixels; NULL only when N is 0, which presents with no damage).  */
/* Pacing.  window_dirty says the host drew something; NHSHELL_EV_FRAME_READY follows when the window's pacer lets it
   present (frame callbacks pace, an ack never waits, a starved window waits for a buffer release).  window_ready
   asks the same: *READY is 1 when the host may begin a frame now.  */
int32_t nhshell_window_dirty (NhShell *sh, uint64_t window);
int32_t nhshell_window_ready (NhShell *sh, uint64_t window, int32_t *ready);
int32_t nhshell_present_begin (NhShell *sh, uint64_t window, int32_t dev_w, int32_t dev_h, NhShellBuffer *out);
int32_t nhshell_present_commit (NhShell *sh, uint64_t window, const GlitnirRect *damage, size_t n);

#ifdef __cplusplus
}
#endif

#endif /* NHSHELL_H */

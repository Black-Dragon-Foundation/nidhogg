/* Definitions and headers for Nidhogg's own Wayland shell (HAVE_NHWL).
   Copyright (C) 1989, 1993, 2005, 2008-2026 Free Software Foundation,
   Inc.

This file is part of GNU Emacs.

GNU Emacs is free software: you can redistribute it and/or modify
it under the terms of the GNU General Public License as published by
the Free Software Foundation, either version 3 of the License, or (at
your option) any later version.

GNU Emacs is distributed in the hope that it will be useful,
but WITHOUT ANY WARRANTY; without even the implied warranty of
MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
GNU General Public License for more details.

You should have received a copy of the GNU General Public License
along with GNU Emacs.  If not, see <https://www.gnu.org/licenses/>.  */

/* The glue between Emacs's terminal and redisplay interfaces and the shell
   API (nidhogg/shell/include/nhshell.h).  The shell owns the Wayland
   connection and the reader thread; nothing here calls Wayland.  The frame's
   pixel unit is the device pixel: logical sizes appear only at the protocol.
   Adapted from pgtkterm.h, without GDK.  */

#ifndef _NHWLTERM_H_
#define _NHWLTERM_H_

#include "dispextern.h"
#include "frame.h"
#include "character.h"
#include "font.h"
#include "sysselect.h"

#ifdef HAVE_NHWL

#include <cairo.h>

#include "nhshell.h"

/* Adapted from pgtk_bitmap_record (image.c uses this one under the same
   field names).  */
struct nhwl_bitmap_record
{
  char *file;
  int refcount;
  int height, width, depth;
  cairo_pattern_t *pattern;
};

#define RGB_TO_ULONG(r, g, b) (((r) << 16) | ((g) << 8) | (b))
#define ARGB_TO_ULONG(a, r, g, b) (((a) << 24) | ((r) << 16) | ((g) << 8) | (b))

#define ALPHA_FROM_ULONG(color) ((color) >> 24)
#define RED_FROM_ULONG(color)	(((color) >> 16) & 0xff)
#define GREEN_FROM_ULONG(color) (((color) >> 8) & 0xff)
#define BLUE_FROM_ULONG(color)	((color) & 0xff)

/* Scroll bars are not drawn by W0 (the parameter is ignored); the type is
   here because shared code names it.  */
struct scroll_bar
{
  union vectorlike_header header;
  Lisp_Object window;
  Lisp_Object next, prev;
  Window x_window;
  int top, left, width, height;
  int start, end;
  int dragging;
  bool horizontal;
};

#define NHWL_MAX_OUTPUTS 16

/* One monitor, as the shell reported it (logical pixels, like the protocol).  */
struct nhwl_monitor
{
  uint64_t id;
  int x, y, w, h, mm_w, mm_h;
  uint32_t scale120;
  char name[32];
};

struct nhwl_display_info
{
  /* Chain of all nhwl_display_info structures.  */
  struct nhwl_display_info *next;

  /* The generic display parameters corresponding to this display.  */
  struct terminal *terminal;

  /* The shell connection (NULL once the display is gone) and the
     descriptor that is readable when it has events for us.  */
  NhShell *shell;
  int connection;
  NhShellCaps caps;

  /* This is a cons cell of the form (NAME . FONT-LIST-CACHE).  */
  Lisp_Object name_list_element;

  /* The X colour table (etc/rgb.txt) as an alist (NAME . #RRGGBB int).  */
  Lisp_Object color_map;

  /* Number of frames that are on this display.  */
  int reference_count;

  /* Logical identifier of this display.  */
  unsigned x_id;

  /* Default name for all frames on this display.  */
  char *x_id_name;

  /* The number of fonts loaded. */
  int n_fonts;

  /* Minimum width over all characters in all fonts in font_table.  */
  int smallest_char_width;

  /* Minimum font height over all fonts in font_table.  */
  int smallest_font_height;

  struct nhwl_bitmap_record *bitmaps;
  ptrdiff_t bitmaps_size;
  ptrdiff_t bitmaps_last;

  /* DPI resolution of this display: 96 times the preferred scale, so that
     fonts are opened at the device size.  */
  double resx, resy;

  /* The preferred scale of the display, times 120: the newest value any
     window or monitor reported.  */
  uint32_t scale120;

  /* Mask of things that cause the mouse to be grabbed */
  int grabbed;

  int n_planes;
  int color_p;

  /* Emacs bitmap-id of the default icon bitmap for this frame.
     Or -1 if none has been allocated yet.  */
  ptrdiff_t icon_bitmap_id;

  Window root_window;

  /* Xism */
  XrmDatabase rdb;

  /* Cursors for the shared code: the shape's NHSHELL_CURSOR_* plus one.  */
  Emacs_Cursor vertical_scroll_bar_cursor;
  Emacs_Cursor horizontal_scroll_bar_cursor;
  Emacs_Cursor invisible_cursor;

  /* Information about the range of text currently shown in
     mouse-face.  */
  Mouse_HLInfo mouse_highlight;

  struct frame *highlight_frame;
  struct frame *x_focus_frame;

  /* The last frame mentioned in a focus event.  */
  struct frame *x_focus_event_frame;

  /* The frame where the mouse was last time we reported a mouse event.  */
  struct frame *last_mouse_frame;

  /* The frame where the mouse was last time we reported a mouse motion.  */
  struct frame *last_mouse_motion_frame;

  /* Position where the mouse was last time we reported a motion.  */
  int last_mouse_motion_x;
  int last_mouse_motion_y;

  /* Where the mouse was last time we reported a mouse position.  */
  XRectangle last_mouse_glyph;
  struct frame *last_mouse_glyph_frame;

  /* Time of last mouse movement.  */
  Time last_mouse_movement_time;

  /* The scroll bar in which the last motion event occurred.  */
  void *last_mouse_scroll_bar;

  /* The modifier mask (NHSHELL_MOD_*) after the newest key event.  */
  uint32_t mods;

  /* Accumulated wheel motion that has not made a step yet.  */
  struct
  {
    double acc_x, acc_y;
    double x_per_char, y_per_line;
    /* Whose fractions acc_x and acc_y are (compared, never used).  */
    struct frame *frame;
    uint32_t source;
  } scroll;

  /* The monitors the shell reported (w and h in device pixels).  */
  struct nhwl_monitor monitors[NHWL_MAX_OUTPUTS];
  int n_monitors;

  /* The frames of this display, from the creation of their shell window
     to their deletion: the compositor's first events for a frame come
     before it is on Vframe_list.  */
  struct nhwl_output *outputs;

  /* The shell reported DISCONNECTED.  */
  bool connection_lost;
};

/* This is a chain of structures for all the displays currently in use.  */
extern struct nhwl_display_info *x_display_list;

#define NHWL_PENDING_MAX 32	/* damage rectangles kept per present */
#define NHWL_HIST 4		/* presents whose damage repairs an old buffer */

/* The bare client-side title bar (nhwlcsd.c): device pixels, in the backing.  The backing is the native area plus
   this border: L, T, R and B are the native area's offsets (T includes the strip); M is the resize margin and STRIP
   the title strip's height now in effect (0 when hidden).  */
struct nhwl_csd
{
  bool on;			/* the compositor offers no server decorations */
  int m, strip, l, t, r, b;
  /* What the border was last drawn for, and that backing's generation.  */
  struct { int w, h, l, t; uint32_t title; bool active; unsigned long fg, bg; uint64_t gen; } sig;
  bool sig_valid;
  /* The window geometry last sent (logical), if any.  */
  int geom[4];
  bool geom_sent;
  /* The pointer in the surface (device px), the region a consumed press began in, and the last strip click.  */
  double px, py;
  int down;
  bool grab, in_border;
  uint64_t click_ns;
};

/* A child frame's state (nhwlchild.c).  The chrome is in logical pixels as `glitnir--resolve' gave it; its margin
   (the room the shadow needs around the native area) is held in the output's CSD offsets, so the backing, the damage
   and the pointer all use the native-area offset they already have.  */
struct nhwl_child
{
  bool chrome, chrome_dirty;
  float radius, blur, ox, oy, spread, opacity;
  uint32_t rgb;
  int z;			/* stacking order among siblings: larger is higher */
  uint32_t chrome_s120;		/* the scale the chrome was last handed over at */
  bool layer_set;
  GlitnirRect layer;		/* the Glitnir layer's rectangle as last handed over */
  bool pos_sent;
  int pos[2];			/* the surface offset in the parent, logical, as last sent */
  bool region_sent;
  int region[4];		/* the input region, logical, as last sent */
  bool mapped;			/* the shell window is shown (not set_mapped off) */
};

struct nhwl_output
{
  struct frame *frame;
  struct nhwl_output *next_output;

  unsigned long foreground_color;
  unsigned long background_color;
  void *toolbar;

  /* The shell's window id (0: none yet) and what the shell last told us
     about it.  */
  uint64_t window;
  Window parent_desc;		/* always the display's root (0) */
  uint32_t scale120;		/* the window's preferred scale times 120 */
  int logical_w, logical_h;	/* the compositor's last suggestion, logical px */
  uint32_t states;		/* NHSHELL_STATE_* of the last configure */
  bool configured;		/* a configure has been seen (the toplevel is mapped) */
  uint32_t serial;

  /* The retained backing: the frame's pixels, device size.  */
  cairo_surface_t *cr_surface;
  cairo_t *cr_context;
  int cr_width, cr_height;
  uint64_t cr_gen;		/* counts the backing's creations */
  struct nhwl_csd csd;
  struct nhwl_child child;

  /* Glitnir's scene of this frame: its id (0: none yet), the size and scale
     it was told, the damage committed but not yet presented, and the
     damage of the last presents (to repair an older buffer).  */
  uint64_t glitnir_id;
  int glitnir_w, glitnir_h;
  uint32_t glitnir_s120;
  GlitnirRect pending[NHWL_PENDING_MAX];
  int npending;
  GlitnirRect hist[NHWL_HIST][NHWL_PENDING_MAX];
  int nhistn[NHWL_HIST];
  int nhist;
  /* The frame-time record's entry in the making: the monotonic time of the
     key event the next present shows (0: none), the commit's, and the
     committed and composed areas.  */
  uint64_t t_key_ns, t_commit_ns, damage_px, painted_px;

  /* Cursors (the shell's pointer shape plus one)  */
  Emacs_Cursor current_cursor;
  Emacs_Cursor text_cursor;
  Emacs_Cursor nontext_cursor;
  Emacs_Cursor modeline_cursor;
  Emacs_Cursor hand_cursor;
  Emacs_Cursor hourglass_cursor;
  Emacs_Cursor horizontal_drag_cursor;
  Emacs_Cursor vertical_drag_cursor;
  Emacs_Cursor left_edge_cursor;
  Emacs_Cursor top_left_corner_cursor;
  Emacs_Cursor top_edge_cursor;
  Emacs_Cursor top_right_corner_cursor;
  Emacs_Cursor right_edge_cursor;
  Emacs_Cursor bottom_right_corner_cursor;
  Emacs_Cursor bottom_edge_cursor;
  Emacs_Cursor bottom_left_corner_cursor;
  Emacs_Cursor current_pointer;

  unsigned long border_pixel;
  Emacs_GC cursor_xgcv;

  ptrdiff_t icon_bitmap;

  struct font *font;
  int baseline_offset;

  /* If a fontset is specified for this frame instead of font, this
     value contains an ID of the fontset, else -1.  */
  int fontset;

  unsigned long mouse_color;
  unsigned long cursor_color;
  unsigned long cursor_foreground_color;

  /* The size of the extra width currently allotted for vertical
     scroll bars, in pixels.  */
  int vertical_scroll_bar_extra;

  /* Heights of external bars: always 0 (the menu and tool bars are
     drawn by Emacs).  */
  int menubar_height;
  int toolbar_top_height, toolbar_bottom_height;
  int toolbar_left_width, toolbar_right_width;

  /* This is the Emacs structure for the display this frame is on.  */
  struct nhwl_display_info *display_info;

  int has_been_visible;

  /* Relief GCs, colors etc.  */
  struct relief
  {
    Emacs_GC xgcv;
    unsigned long pixel;
  }
  black_relief, white_relief;

  /* The background for which the above relief GCs were set up.  */
  unsigned long relief_background;
  bool_bf relief_background_valid_p : 1;

  /* Keep track of focus.  */
  int focus_state;
};

/* Satisfy term.c.  */
struct x_output
{
  int unused;
};

enum
{
  FOCUS_NONE = 0,
  FOCUS_IMPLICIT = 1,
  FOCUS_EXPLICIT = 2
};

#define FRAME_NHWL_OUTPUT(f)	  ((f)->output_data.nhwl)
#define FRAME_NHWL_DISPLAY_INFO(f) (FRAME_NHWL_OUTPUT (f)->display_info)

/* The accessor names shared code uses on every window system.  */
#define FRAME_X_OUTPUT(f)         FRAME_NHWL_OUTPUT (f)
#define FRAME_OUTPUT_DATA(f)      FRAME_NHWL_OUTPUT (f)

#define FRAME_DISPLAY_INFO(f)     (FRAME_X_OUTPUT (f)->display_info)
#define FRAME_FOREGROUND_COLOR(f) (FRAME_X_OUTPUT (f)->foreground_color)
#define FRAME_BACKGROUND_COLOR(f) (FRAME_X_OUTPUT (f)->background_color)
#define FRAME_CURSOR_COLOR(f)     (FRAME_X_OUTPUT (f)->cursor_color)
#define FRAME_POINTER_TYPE(f)     (FRAME_X_OUTPUT (f)->current_pointer)
#define FRAME_FONT(f)             (FRAME_X_OUTPUT (f)->font)
#define FRAME_X_WINDOW(f)         ((Window) FRAME_X_OUTPUT (f)->window)
#define FRAME_NATIVE_WINDOW(f)    FRAME_X_WINDOW (f)

/* Turning a lisp vector value into a pointer to a struct scroll_bar.  */
#define XSCROLL_BAR(vec) ((struct scroll_bar *) XVECTOR (vec))

#define FRAME_DEFAULT_FACE(f) FACE_FROM_ID_OR_NULL (f, DEFAULT_FACE_ID)
#define FRAME_MENUBAR_HEIGHT(f) (FRAME_X_OUTPUT (f)->menubar_height)
#define FRAME_TOOLBAR_TOP_HEIGHT(f) ((f)->output_data.nhwl->toolbar_top_height)
#define FRAME_TOOLBAR_BOTTOM_HEIGHT(f) \
  ((f)->output_data.nhwl->toolbar_bottom_height)
#define FRAME_TOOLBAR_HEIGHT(f) \
  (FRAME_TOOLBAR_TOP_HEIGHT (f) + FRAME_TOOLBAR_BOTTOM_HEIGHT (f))
#define FRAME_TOOLBAR_LEFT_WIDTH(f) ((f)->output_data.nhwl->toolbar_left_width)
#define FRAME_TOOLBAR_RIGHT_WIDTH(f) ((f)->output_data.nhwl->toolbar_right_width)
#define FRAME_TOOLBAR_WIDTH(f) \
  (FRAME_TOOLBAR_LEFT_WIDTH (f) + FRAME_TOOLBAR_RIGHT_WIDTH (f))

#define FRAME_FONTSET(f) (FRAME_X_OUTPUT (f)->fontset)

#define FRAME_BASELINE_OFFSET(f) (FRAME_X_OUTPUT (f)->baseline_offset)
#define BLACK_PIX_DEFAULT(f) 0x000000
#define WHITE_PIX_DEFAULT(f) 0xFFFFFF

/* First position where characters can be shown (instead of scrollbar, if
   it is on left. */
#define FIRST_CHAR_POSITION(f)				\
  (! (FRAME_HAS_VERTICAL_SCROLL_BARS_ON_LEFT (f)) ? 0	\
   : FRAME_SCROLL_BAR_COLS (f))

/* The shell's window (a 64-bit id) of frame F, and its display's shell.  */
#define FRAME_NHWL_WINDOW(f) (FRAME_NHWL_OUTPUT (f)->window)
#define FRAME_NHWL_SHELL(f) (FRAME_DISPLAY_INFO (f)->shell)

/* Nidhogg's own parameters that map to nothing outside the shell: the
   modifier names for the shell's modifier bits.  */

/* nhwlterm.c */
extern struct nhwl_display_info *nhwl_term_init (Lisp_Object);
extern void nhwl_term_shutdown (int);
extern void nhwl_delete_terminal (struct terminal *);
extern void nhwl_free_frame_resources (struct frame *);
extern void nhwl_destroy_window (struct frame *);
extern void nhwl_make_frame_visible (struct frame *);
extern void nhwl_make_frame_invisible (struct frame *);
extern void nhwl_iconify_frame (struct frame *);
extern void nhwl_focus_frame (struct frame *, bool);
extern Lisp_Object nhwl_get_focus_frame (struct frame *);
extern void nhwl_frame_rehighlight (struct nhwl_display_info *);
extern void nhwl_clear_frame (struct frame *);
extern void nhwl_clear_area (struct frame *, int, int, int, int);
extern void nhwl_clear_under_internal_border (struct frame *);
extern bool nhwl_defined_color (struct frame *, const char *, Emacs_Color *,
				bool, bool);
extern int nhwl_parse_color (struct frame *, const char *, Emacs_Color *);
extern void nhwl_query_color (struct frame *, Emacs_Color *);
extern void nhwl_query_colors (struct frame *, Emacs_Color *, int);
extern void nhwl_implicitly_set_name (struct frame *, Lisp_Object, Lisp_Object);
extern int nhwl_display_pixel_height (struct nhwl_display_info *);
extern int nhwl_display_pixel_width (struct nhwl_display_info *);
extern void nhwl_set_window_size (struct frame *, bool, int, int);
extern void nhwl_set_scroll_bar_default_width (struct frame *);
extern void nhwl_set_scroll_bar_default_height (struct frame *);
extern void nhwl_default_font_parameter (struct frame *, Lisp_Object);
extern double nhwl_frame_scale_factor (struct frame *);
extern const char *nhwl_get_defaults_value (const char *);
extern const char *nhwl_get_string_resource (XrmDatabase, const char *,
					     const char *);
extern void nhwl_create_frame_window (struct frame *);
extern void nhwl_set_title_hint (struct frame *);
extern void nhwl_set_undecorated_hint (struct frame *, bool);
extern void nhwl_wait_for_configure (struct frame *);
extern struct nhwl_display_info *check_nhwl_display_info (Lisp_Object);
extern struct nhwl_display_info *nhwl_display_info_for_name (Lisp_Object);
extern void mark_nhwlterm (void);
extern int nhwl_drain_events (struct nhwl_display_info *,
			      struct input_event *);

/* The backing (nhwlterm.c; nhwldraw.c draws on it).  */
extern void nhwl_cr_update_surface_desired_size (struct frame *, int, int,
						 bool);
extern cairo_t *nhwl_begin_cr_clip (struct frame *);
extern void nhwl_end_cr_clip (struct frame *);
extern void nhwl_set_cr_source_with_gc_foreground (struct frame *, Emacs_GC *,
						   bool);
extern void nhwl_set_cr_source_with_gc_background (struct frame *, Emacs_GC *,
						   bool);
extern void nhwl_set_cr_source_with_color (struct frame *, unsigned long,
					   bool);
extern bool nhwl_present (struct frame *);
extern void nhwl_dirty (struct frame *);

/* nhwldraw.c: drawing, the backing and Glitnir.  */
extern void nhwl_scroll_run (struct window *, struct run *);
extern void nhwl_draw_fringe_bitmap (struct window *, struct glyph_row *,
				     struct draw_fringe_bitmap_params *);
extern void nhwl_define_fringe_bitmap (int, unsigned short *, int, int);
extern void nhwl_destroy_fringe_bitmap (int);
extern void nhwl_draw_glyph_string (struct glyph_string *);
extern void nhwl_draw_window_cursor (struct window *, struct glyph_row *, int,
				     int, enum text_cursor_kinds, int, bool,
				     bool);
extern void nhwl_draw_vertical_window_border (struct window *, int, int, int);
extern void nhwl_draw_window_divider (struct window *, int, int, int, int);
extern void nhwl_fill_background_by_face (struct frame *, struct face *, int,
					  int, int, int);
extern void nhwl_damage (struct frame *, int, int, int, int);
extern void nhwl_damage_internal_border (struct frame *);
extern void nhwl_backing_resize (struct frame *);
extern uint64_t nhwl_now_ns (void);
extern void nhwl_glitnir_ensure (struct frame *);
extern void nhwl_glitnir_forget (struct frame *);
extern void nhwl_glitnir_commit (struct frame *);
extern void nhwl_glitnir_compose (struct frame *, const NhShellBuffer *);
extern void nhwl_glitnir_presented (struct frame *);
extern void nhwl_glitnir_scroll (struct frame *, struct window *, int, int,
				 int, int, int);
extern void nhwl_damage_glyphs (struct glyph_string *);
extern void nhwl_damage_cursor (struct window *, struct glyph_row *, bool);
extern void nhwl_damage_fringe (struct window *,
				struct draw_fringe_bitmap_params *);
extern cairo_surface_t *nhwl_snapshot (struct frame *);

/* nhwlcsd.c: the bare client-side title bar.  */
extern void nhwl_csd_layout (struct frame *);
extern void nhwl_csd_draw (struct frame *);
extern void nhwl_csd_send_geometry (struct frame *);
extern void nhwl_csd_changed (struct frame *);
extern void nhwl_csd_geometry_size (struct frame *, int *, int *);
extern void nhwl_csd_native_size (struct frame *, int, int, int *, int *);
extern void nhwl_csd_native_pos (struct frame *, double, double, int *, int *);
extern int nhwl_to_logical (int, uint32_t);
extern int nhwl_to_device (int, uint32_t);
extern bool nhwl_csd_pointer (struct frame *, double, double);
extern bool nhwl_csd_button (struct frame *, const NhShellEvent *, bool *);

/* nhwlchild.c: child frames (subsurfaces of the parent frame's window).  */
extern uint64_t nhwl_frame_window (struct frame *);
extern struct frame *nhwl_window_frame (uint64_t);
extern bool nhwl_child_p (struct frame *);
extern void nhwl_tip_dismissed (struct frame *);
extern void nhwl_child_describe (struct frame *, NhShellWindowDesc *);
extern void nhwl_child_created (struct frame *);
extern void nhwl_child_layout (struct frame *);
extern void nhwl_child_sync (struct frame *);
extern void nhwl_child_set_visible (struct frame *, bool);
extern void nhwl_child_reposition (struct frame *);
extern void nhwl_child_parent_layout (struct frame *);
extern void nhwl_child_params_changed (struct frame *);
extern bool nhwl_child_layer_update (struct frame *, GlitnirRect *);
extern void nhwl_child_apply_chrome (struct frame *);
extern void nhwl_child_source (struct frame *, GlitnirSource *);
extern void nhwl_frame_raise_lower (struct frame *, bool);
extern void nhwl_set_parent_frame (struct frame *, Lisp_Object, Lisp_Object);
extern void nhwl_set_z_group (struct frame *, Lisp_Object, Lisp_Object);
extern void nhwl_set_no_accept_focus (struct frame *, Lisp_Object,
				      Lisp_Object);

/* The names ftcrfont.c and the other Cairo code shared with pgtk use.  */
#define FRAME_CR_CONTEXT(f) (FRAME_NHWL_OUTPUT (f)->cr_context)
#define pgtk_begin_cr_clip nhwl_begin_cr_clip
#define pgtk_end_cr_clip nhwl_end_cr_clip
#define pgtk_set_cr_source_with_gc_foreground nhwl_set_cr_source_with_gc_foreground
#define pgtk_set_cr_source_with_gc_background nhwl_set_cr_source_with_gc_background
#define pgtk_set_cr_source_with_color nhwl_set_cr_source_with_color

/* nhwlfns.c */
extern void syms_of_nhwlterm (void);
extern void syms_of_nhwlfns (void);
extern void syms_of_nhwlselect (void);
extern bool nhwl_selection_changed (struct nhwl_display_info *, uint32_t,
				    struct input_event *);
extern void nhwl_handle_selection_clear (struct input_event *);
extern frame_parm_handler nhwl_frame_parm_handlers[];
extern Emacs_Cursor nhwl_cursor_for_shape (uint32_t);

#endif /* HAVE_NHWL */
#endif /* _NHWLTERM_H_ */

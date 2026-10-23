/* Nidhogg's worker threads: the shared layout and every cross-file prototype.

Copyright (C) 2026 Black Dragon Foundation contributors.

This file is part of Nidhogg.

Nidhogg is free software: you can redistribute it and/or modify
it under the terms of the GNU General Public License as published by
the Free Software Foundation, either version 3 of the License, or (at
your option) any later version.

Nidhogg is distributed in the hope that it will be useful,
but WITHOUT ANY WARRANTY; without even the implied warranty of
MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
GNU General Public License for more details.

You should have received a copy of the GNU General Public License
along with Nidhogg.  If not, see <https://www.gnu.org/licenses/>.  */

/* README.md, "Work off the main thread", is the user-facing description.

   This header is the contract between the seven nidhogg-*.c files of
   the workers (nidhogg-workers.c, -treesit-bg.c, -io.c, -decode.c,
   -json-tape.c, -image-bg.c, -blk.c).  Each later task owns one file
   and implements the prototypes below that name it; a change here is a
   change for all of them.  The owner of each group is in brackets.

   With NH_WK_HARNESS defined (nidhogg/test/workers-harness.sh) the
   files compile without Lisp: everything that names a Lisp type is
   under #ifndef NH_WK_HARNESS.  */

#ifndef NIDHOGG_WORKERS_H
#define NIDHOGG_WORKERS_H

#ifdef HAVE_NIDHOGG_WORKERS

#include <stdalign.h>
#include <stdatomic.h>
#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>
#include <stdlib.h>
#include <pthread.h>

#ifndef NH_WK_HARNESS
# include "lisp.h"
#endif

/* The worker threads (the thread names are nh-io, nh-parse, nh-img,
   nh-blk and nh-save).  */
enum nh_wk_worker
{
  NH_WK_IO,
  NH_WK_PARSE,
  NH_WK_IMG,
  NH_WK_BLK,
  NH_WK_SAVE,
  NH_WK_N
};

/* The classes of nh-blk jobs; each runs on at most two threads at once.  */
enum nh_wk_class
{
  NH_WK_CL_DIR,
  NH_WK_CL_FILE,
  NH_WK_CL_DNS,
  NH_WK_CL_N
};

/* The points where a worker parks while a test arms them.  */
enum nh_wk_pause
{
  NH_WK_P_NONE,
  NH_WK_P_AFTER_SNAPSHOT,
  NH_WK_P_BEFORE_DONE,
  NH_WK_P_AFTER_READ,
  NH_WK_P_BEFORE_TOKENIZE,
  NH_WK_P_BEFORE_DECODE,
  NH_WK_P_IN_POLL,
  NH_WK_P_AFTER_HEADER,
  NH_WK_P_BEFORE_WRITE,
  NH_WK_P_IN_LOOKUP,
  NH_WK_P_N
};

/* A job's `state'.  */
enum nh_wk_job_state
{
  NH_WK_J_QUEUED,
  NH_WK_J_RUNNING,
  NH_WK_J_DONE,
  NH_WK_J_CANCELLED
};

/* A unit of work.  The submitter holds one reference and the worker
   one; `free' runs when the count reaches zero.  `run' executes on the
   worker, `finish' on the main thread at landing.  */
struct nh_wk_job
{
  _Atomic int refs;
  _Atomic int state;		/* enum nh_wk_job_state */
  _Atomic bool cancel;
  _Atomic uint64_t progress, total;
  enum nh_wk_worker w;
  int cls;			/* enum nh_wk_class, for nh-blk */
  bool speculative;
  void (*run) (struct nh_wk_job *);
  void (*finish) (struct nh_wk_job *);
  void (*free) (struct nh_wk_job *);
  struct nh_wk_job *next;
  pthread_t runner;
  uint64_t submitted_ns;
};

/* A wake channel: an eventfd (both fds the same) or a pipe.  */
struct nh_wk_wake
{
  int rfd, wfd;
};

/* Set when the process is going down (nh_wk_fatal_begin) [4].  */
extern _Atomic bool nh_wk_dead;
/* Set while nidhogg-workers--mode is `inline' [3].  */
extern bool nh_wk_inline;

/* No `eassert' on a worker thread: it would run Lisp-side machinery.  */
#define NH_WK_ASSERT(cond) \
  do { if (!(cond)) abort (); } while (0)

/* Forward declarations of the types the prototypes below name.  */
struct Lisp_Process;
struct frame;
struct image;
struct buffer;
struct addrinfo;
struct nh_dec_state;
struct nh_tape;
struct nh_mat;

/* The runtime, nidhogg-workers.c [3].  */
extern bool nh_wk_submit (struct nh_wk_job *);
extern int nh_wk_wait (struct nh_wk_job *, int64_t timeout_ns, int phase,
		       int reason);
extern void nh_wk_cancel (struct nh_wk_job *);
extern void nh_wk_job_ref (struct nh_wk_job *);
extern void nh_wk_job_unref (struct nh_wk_job *);
extern void nh_wk_complete (struct nh_wk_job *);
extern void nh_wk_pause_point (enum nh_wk_pause);
extern int nh_wk_wake_open (struct nh_wk_wake *);
extern void nh_wk_wake_signal (struct nh_wk_wake *);
extern void nh_wk_wake_clear (struct nh_wk_wake *);
extern void nh_wk_wake_close (struct nh_wk_wake *);
extern bool nh_wk_fd_budget_ok (int extra);
extern bool nh_wk_on_worker (void);
extern void nh_wk_set_on_worker (bool);
extern _Atomic ptrdiff_t nh_wk_worker_probes;

/* The counters of nidhogg-workers-stats, from any thread [3].  Why a job
   took today's path, or a wait could not raise an idle thread
   (`:fallbacks'); a later reason is a new entry here and a new name in
   nidhogg-workers.c.  */
enum nh_wk_fallback
{
  NH_WK_FB_DUMPING,
  NH_WK_FB_DEAD,
  NH_WK_FB_THREAD,		/* thread-create */
  NH_WK_FB_FD_BUDGET,
  NH_WK_FB_BOOST_DENIED,
  NH_WK_FB_N
};
/* A worker's other counters: `:queued-bytes' (a gauge: add and
   subtract), `:backpressure', `:slices'.  */
enum nh_wk_stat
{
  NH_WK_ST_QUEUED_BYTES,
  NH_WK_ST_BACKPRESSURE,
  NH_WK_ST_SLICES,
  NH_WK_ST_N
};
extern void nh_wk_fallback (enum nh_wk_worker, enum nh_wk_fallback);
extern void nh_wk_stat_add (enum nh_wk_worker, enum nh_wk_stat, int64_t);
/* NS spent in PHASE (an NH_WK_PH_*; REASON an NH_WK_RS_* for
   worker-wait), for `:phase-ms' (PD-3).  nh_wk_wait counts its own.  */
extern void nh_wk_phase_ns_add (int phase, int reason, int64_t ns);
/* A deferral (`:deferrals'): AREA 0 tree-sitter, 1 images.  */
extern void nh_wk_deferral_add (int area);

/* The dead flag, from shut_down_emacs [4].  */
extern void nh_wk_fatal_begin (void);

/* The redisplay budget [5].  */
extern int64_t nh_wk_budget_take (int64_t want_ns);
extern void nh_wk_budget_reset (void);

/* Tree-sitter, nidhogg-treesit-bg.c [9, 10, 11, 12].  */
extern void nh_ts_alloc_install (void);
extern void nh_ts_parser_freed (void *parser_c);
extern void nh_ts_record_edit (void *parser_c, const void *ts_input_edit,
			       ptrdiff_t start_byte, ptrdiff_t old_end_byte,
			       ptrdiff_t new_end_byte);
extern void nh_ts_select_entry (void);

/* The reader, nidhogg-io.c [16, 17, 18, 19, 20].  */
extern bool nh_io_handover (struct Lisp_Process *);
extern void nh_io_handback (struct Lisp_Process *, int reason);
extern int nh_io_select_fd (struct Lisp_Process *);
extern void nh_io_pause (int infd);
extern void nh_io_resume (int infd);
extern ptrdiff_t nh_io_read (struct Lisp_Process *, int channel,
			     bool *handled);
extern void nh_io_drain (struct Lisp_Process *);
extern void nh_io_coding_changed (struct Lisp_Process *);
extern int nh_io_tls_boot (struct Lisp_Process *, void *gnutls_state);

/* Decoding, nidhogg-decode.c [18].  */
extern ptrdiff_t nh_dec_chunk (int system, const unsigned char *in,
			       ptrdiff_t n, struct nh_dec_state *,
			       unsigned char *out, ptrdiff_t *nchars,
			       bool last);

/* The JSON tape, nidhogg-json-tape.c [22, 23].  */
extern struct nh_tape *nh_json_tokenize (const unsigned char *, size_t,
					 int mode, const _Atomic bool *stop);
extern bool nh_json_materialize (struct nh_mat *, int64_t budget_ns);

/* Images, nidhogg-image-bg.c [27, 28, 29].  */
extern bool nh_img_lookup (struct frame *, struct image *, ptrdiff_t id);
/* The synchronous loaders' entry (image.c): true when it handled the load,
   with the loader's result in *OK; false: run the loader as before.  */
extern bool nh_img_sync_load (struct frame *, struct image *, int fmt,
			      bool *ok);

/* The header parsers and frame-count scans (Task 27), Lisp-free.  The
   formats are the loaders' four.  A parser answers 1 when it parsed,
   0 when the bytes are not a (complete enough) file of that format and
   -1 when it ran out of the bytes it was given (a longer prefix may
   do).  Sizes are the native size in pixels, never zero.  */
enum nh_img_format
{
  NH_IMG_F_NONE,
  NH_IMG_F_PNG,
  NH_IMG_F_JPEG,
  NH_IMG_F_GIF,
  NH_IMG_F_WEBP
};

/* One extension block of a GIF frame, as giflib's DGifSlurp keeps it.  */
struct nh_img_gif_ext
{
  int function, len;
  uint8_t bytes[255];
};

#define NH_IMG_GIF_EXT_MAX 64

/* What nh_img_gif_scan and nh_img_webp_scan found for frame IDX of
   the file: the screen (canvas) size, the number of frames and, for
   GIF, frame IDX's extension blocks (EXTS, at most NH_IMG_GIF_EXT_MAX,
   else the scan fails) and, for WebP, whether it is animated and frame
   IDX's duration in milliseconds.  */
struct nh_img_scan
{
  int width, height, frames, nexts;
  bool animated;
  int duration_ms;
  struct nh_img_gif_ext exts[NH_IMG_GIF_EXT_MAX];
};

extern int nh_img_parse_header (int fmt, const uint8_t *, size_t,
				int *width, int *height);
extern int nh_img_gif_scan (const uint8_t *, size_t, int idx,
			    struct nh_img_scan *);
extern int nh_img_webp_scan (const uint8_t *, size_t, int idx,
			     struct nh_img_scan *);

/* Auto-save and name lookups, nidhogg-blk.c [34, 35].  */
extern bool nh_save_auto_save_1 (struct buffer *);
extern bool nh_save_keyboard_triggered;
extern bool nh_dns_start (struct Lisp_Process *, const char *host,
			  const char *service, const struct addrinfo *hints);

#ifndef NH_WK_HARNESS

extern bool nh_ts_publish (Lisp_Object parser);
extern void nh_ts_maybe_start (Lisp_Object parser, int start_point);
extern Lisp_Object nh_img_size (Lisp_Object spec, Lisp_Object frame,
				bool pixels, bool *answered);
extern Lisp_Object nh_img_metadata (Lisp_Object spec, Lisp_Object frame,
				    bool *answered);
/* The hooks in image.c the header answers use.  */
extern int nh_img_spec_format (Lisp_Object spec);
extern bool nh_img_size_ok (struct frame *, int width, int height);
extern bool nh_img_displayed_size (struct frame *, Lisp_Object spec,
				   int native_w, int native_h,
				   int *width, int *height);
extern bool nh_save_name_in_flight (Lisp_Object name);

/* The `syms_of' part of each file, called from syms_of_nidhogg_workers.  */
extern void syms_of_nidhogg_workers (void);
extern void nh_ts_syms (void);
extern void nh_io_syms (void);
extern void nh_dec_syms (void);
extern void nh_json_syms (void);
extern void nh_img_syms (void);
extern void nh_blk_syms (void);

#endif /* not NH_WK_HARNESS */

#endif /* HAVE_NIDHOGG_WORKERS */

#endif /* NIDHOGG_WORKERS_H */

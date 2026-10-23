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
struct coding_system;
struct nh_tape;
struct nh_mat;

/* The runtime, nidhogg-workers.c [3].  */
extern bool nh_wk_submit (struct nh_wk_job *);
extern int nh_wk_wait (struct nh_wk_job *, int64_t timeout_ns, int phase,
		       int reason);
extern void nh_wk_wait_begins (void);
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

/* Condition variables whose timed waits use CLOCK_MONOTONIC deadlines
   (ns).  macOS has no pthread_condattr_setclock: there the wait is
   relative, for the time left until DEADLINE.  */
extern void nh_wk_cond_init (pthread_cond_t *);
extern int nh_wk_cond_wait_until (pthread_cond_t *, pthread_mutex_t *,
				  uint64_t);
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

/* Signal context [4] (§4 item 11).  A signal handler that may reach
   worker code runs between nh_wk_signal_enter and nh_wk_signal_leave
   (process.c's SIGCHLD handler).  nh_wk_signal_context is true there,
   and on every thread but a worker once nh_wk_dead is set (the fatal
   path): then nothing takes a worker mutex or waits.  Every worker
   mutex is taken through nh_wk_lock, which counts a lock taken in signal
   context and aborts, rather than deadlock on a mutex the interrupted
   code holds.  */
extern void nh_wk_signal_enter (void);
extern void nh_wk_signal_leave (void);
extern bool nh_wk_signal_context (void);
extern void nh_wk_lock (pthread_mutex_t *);

/* The redisplay budget [5].  */
extern int64_t nh_wk_budget_take (int64_t want_ns);
extern void nh_wk_budget_reset (void);

/* Tree-sitter, nidhogg-treesit-bg.c [9, 10, 11, 12].  */
extern void nh_ts_alloc_install (void);
extern void nh_ts_parser_freed (void *parser_c);
/* treesit.c applied a TSInputEdit to the parser's tree, or (with
   TS_INPUT_EDIT NULL) a parser without a tree saw a change in buffer
   bytes: log it for the parser's job, if any [11].  */
extern void nh_ts_record_edit (void *parser_c, const void *ts_input_edit,
			       ptrdiff_t start_byte, ptrdiff_t old_end_byte,
			       ptrdiff_t new_end_byte);
/* wait_reading_process_output with READ_KBD != 0: start jobs for the
   parsers edited since and not demanded (timers, filters) [12].  */
extern void nh_ts_select_entry (void);
/* Kill-buffer: cancel the parse jobs of the parsers of that buffer.  */
extern void nh_ts_buffer_killed (struct buffer *);

/* The reader, nidhogg-io.c [16, 17, 18, 19, 20, 24].  A hand-back's
   reasons.  */
enum nh_io_reason
{
  NH_IO_CLOSE,			/* the process goes down: its fds close next */
  NH_IO_TLS,
  NH_IO_READER_OFF,
  NH_IO_STATUS
};
extern bool nh_io_handover (struct Lisp_Process *);
extern void nh_io_handback (struct Lisp_Process *, int reason);
extern int nh_io_select_fd (struct Lisp_Process *);
/* After a hand-back other than NH_IO_CLOSE: true once the bytes the
   reader had read are delivered; then nh_io_release frees the handle
   (the caller first takes the wake fd out of the select set).  */
extern bool nh_io_done (struct Lisp_Process *);
extern void nh_io_release (struct Lisp_Process *);
/* Flow control [17].  nh_io_pause is async-signal-safe and returns the
   fd the select set holds for INFD (INFD when no reader owns it).  */
extern int nh_io_pause (int infd);
extern void nh_io_resume (int infd);
/* read_process_output's reader branch: one delivery's worth, up to SIZE
   bytes, into BUF, as the read it replaces [19]; *HANDLED false means
   read the fd as today.  */
extern ptrdiff_t nh_io_read (struct Lisp_Process *, int channel, char *buf,
			     ptrdiff_t size, bool *handled);
/* Before a drain [17]: true when the caller must hand the process back
   (NH_IO_STATUS) so its last bytes are read on the main thread.  */
extern bool nh_io_drain (struct Lisp_Process *);
/* Task 18.  setup_process_coding_systems set up CODING, P's decoding
   struct, again: the coding generation moves on.  */
extern void nh_io_coding_changed (struct Lisp_Process *,
				  struct coding_system *);
/* The main thread decoded P's output with CODING, whose detection may
   have settled it on a stateless system.  */
extern void nh_io_coding_settled (struct Lisp_Process *,
				  struct coding_system *);
/* A filter's delivery of the NBYTES at CHARS (P's decoding carryover,
   then the bytes nh_io_read delivered): true when nh-io's decode of
   exactly those bytes is current, with CODING's dst_object, produced
   counts and carryover set as decode_coding_c_string would set them.  */
extern bool nh_io_decoded (struct Lisp_Process *, struct coding_system *,
			   const char *chars, ptrdiff_t nbytes);
/* Delivery [19].  The default filter's insertion: true when nh-io's
   decodes of the NREAD bytes at BUF went into the current buffer at
   point as decode_coding_c_string would insert them (point before the
   text, CODING's counts and carryover set).  */
extern bool nh_io_insert (struct Lisp_Process *, struct coding_system *,
			  const char *buf, ptrdiff_t nread);
/* Around a delivery the reader made: the `deliver' phase and its ns.  */
struct nh_io_deliver
{
  int depth;
  int64_t t0, filter_ns;
};
extern void nh_io_deliver_begin (bool reader, struct nh_io_deliver *);
extern void nh_io_deliver_end (struct nh_io_deliver *);
/* Around a filter call: its ns, and the estimate of its cost per byte
   that caps the deliveries to it (NBYTES 0: it was not called).  */
extern int64_t nh_io_filter_begin (struct Lisp_Process *);
extern void nh_io_filter_end (struct Lisp_Process *, int64_t t0,
			      ptrdiff_t nbytes);
/* Task 20, Fgnutls_boot's hooks: with GNUTLS_STATE NULL (before any
   GnuTLS call) the reader hands the process back for good and keeps
   the bytes it had not delivered for GnuTLS; with the new session, the
   session gets a pull function that returns those bytes first.  */
extern int nh_io_tls_boot (struct Lisp_Process *, void *gnutls_state);
/* The bytes of the session GNUTLS_STATE that its pull function has yet
   to return (emacs_gnutls_record_check_pending).  */
extern ptrdiff_t nh_io_tls_pending (void *gnutls_state);
/* For the crash rows (Task 4's --test-crash on nh-io).  */
extern bool nh_io_test_crash (void);
/* Task 4's kill_buffer_processes hook: under nh_wk_dead, close a
   reader-owned process's fds directly.  */
extern void nh_io_dead_close (struct Lisp_Process *);

/* Decoding, nidhogg-decode.c [18].  The stateless systems (§8.2) nh-io
   decodes a process's output with; NH_DEC_NONE leaves it to the main
   thread.  The UTF-8 ones make Emacs's multibyte form (invalid bytes as
   raw-byte characters), the raw-text ones (raw-text-*, no-conversion,
   binary) unibyte bytes.  */
enum nh_dec_system
{
  NH_DEC_NONE,
  NH_DEC_UTF8_UNIX,
  NH_DEC_UTF8_DOS,
  NH_DEC_UTF8_MAC,
  NH_DEC_RAW_UNIX,
  NH_DEC_RAW_DOS,
  NH_DEC_RAW_MAC,
  NH_DEC_N
};

#define NH_DEC_MULTIBYTE(sys) \
  ((sys) == NH_DEC_UTF8_UNIX || (sys) == NH_DEC_UTF8_DOS \
   || (sys) == NH_DEC_UTF8_MAC)
/* The most bytes a decode of N bytes writes.  */
#define NH_DEC_OUT_MAX(n) (2 * (n))

/* What a chunk left undecoded for the next one: an incomplete sequence
   (at most 4 bytes) or, -dos, a final CR.  */
enum { NH_DEC_CARRY = 8 };
struct nh_dec_state
{
  int n;
  unsigned char carry[NH_DEC_CARRY];
};

/* Decode STATE's carry and then the N bytes at IN with SYSTEM, as the
   process path decodes one read (one decode_coding call); with LAST,
   flush an incomplete tail as CODING_MODE_LAST_BLOCK does.  Write at
   most NH_DEC_OUT_MAX (carry + N) bytes at OUT, their character count
   in *NCHARS; return the byte count, or -1 when out of memory.  */
extern ptrdiff_t nh_dec_chunk (int system, const unsigned char *in,
			       ptrdiff_t n, struct nh_dec_state *,
			       unsigned char *out, ptrdiff_t *nchars,
			       bool last);
/* The same on the N bytes at S with no carry: *CONSUMED is how many
   were decoded (the rest is the carry).  */
extern ptrdiff_t nh_dec_run (int system, const unsigned char *s,
			     ptrdiff_t n, bool last, unsigned char *out,
			     ptrdiff_t *nchars, ptrdiff_t *consumed);

/* The JSON tape, nidhogg-json-tape.c [22, 23].  The tokenizer's modes:
   `json-parse-string' (trailing content is an error) and
   `json-parse-buffer' (one value, the rest is left).  */
enum
{
  NH_JSON_MODE_STRING,
  NH_JSON_MODE_BUFFER
};
extern struct nh_tape *nh_json_tokenize (const unsigned char *, size_t,
					 int mode, const _Atomic bool *stop);
extern bool nh_json_materialize (struct nh_mat *, int64_t budget_ns);

/* Images, nidhogg-image-bg.c [27, 28, 29].  */
/* Task 29: the image cache's hooks (image.c, xdisp.c).  lookup_image on
   a miss: true when the entry was handled here (decoded within the
   redisplay's budget, or made pending), false to load it as before.  On
   a hit (a Lisp caller may need the pixels of a pending entry), in
   prepare_image_for_display (true: a pending entry, not to be loaded),
   in free_image, and at the top of redisplay_internal (the landing).  */
extern bool nh_img_lookup (struct frame *, struct image *);
extern void nh_img_hit (struct frame *, struct image *);
extern bool nh_img_pending_p (struct frame *, struct image *);
extern void nh_img_freed (struct frame *, struct image *);
extern void nh_img_redisplay_begin (void);
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

/* Name lookups, nidhogg-blk.c [35].  */
extern bool nh_dns_start (struct Lisp_Process *, const char *host,
			  const char *service, const struct addrinfo *hints);

/* File blocks, nidhogg-decode.c and nidhogg-blk.c [33].  The scan is
   Lisp-free: what a worker learns about the bytes it read, so the main
   thread can skip its own scans when it inserts them.  EOL is
   NH_DEC_EOL_NONE with no CR or LF, _LF with LFs only, _CRLF when every
   LF follows a CR and every CR precedes an LF, _CR with CRs only, else
   _MIXED.  UTF8 means strictly valid UTF-8 (no surrogates, no overlong
   forms, at most U+10FFFF; ASCII is valid), NCHARS then its character
   count.  BOM: starts with EF BB BF.  */
enum nh_dec_eol
{
  NH_DEC_EOL_NONE,
  NH_DEC_EOL_LF,
  NH_DEC_EOL_CRLF,
  NH_DEC_EOL_CR,
  NH_DEC_EOL_MIXED
};

struct nh_dec_scan
{
  bool ascii, utf8, has_null, has_esc, bom;
  int eol;			/* enum nh_dec_eol */
  size_t nchars;
};

extern void nh_dec_scan (const unsigned char *, size_t, struct nh_dec_scan *);

/* The auto-save encoder, nidhogg-decode.c [34]: N bytes of Emacs's
   internal representation (a buffer's text, multibyte or unibyte) as
   `utf-8-emacs-unix' encodes them: raw-byte characters become their
   byte, everything else is copied.  OUT may be IN (it never runs
   ahead); return the bytes written.  Lisp-free.  */
extern size_t nh_dec_autosave_encode (const unsigned char *in, size_t n,
				      unsigned char *out);

#ifndef NH_WK_HARNESS

extern void nh_blk_file_syms (void);
/* A demand that must reparse (treesit_ensure_parsed, after the
   narrowing sync) [10, 11, 12]: NH_TS_TREE when *TREE (now the
   caller's) is the parse of the current text (a finished job's, a 1 ms
   attempt's, or the result of waiting for a nearly done job);
   NH_TS_SYNC when the synchronous parse must run (any job is then
   dropped); NH_TS_NOT_READY only with MAY_DEFER, when redisplay is
   fontifying the parser's buffer and a job now parses the text (the
   caller parses nothing and leaves need_reparse set).  */
enum { NH_TS_SYNC, NH_TS_TREE, NH_TS_NOT_READY };
extern int nh_ts_demand (Lisp_Object parser, bool may_defer, void **tree);
/* The start points (§7.3 item 1) [12].  */
enum { NH_TS_START_DEMAND, NH_TS_START_CREATE, NH_TS_START_SELECT };
extern void nh_ts_maybe_start (Lisp_Object parser, int start_point);
/* NODE's (treesit-subtree-stat) if a worker computed it, else nil.  */
extern Lisp_Object nh_ts_subtree_stat_cached (Lisp_Object node);
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
/* Task 29.  The iterator's lookup_image calls (xdisp.c): with
   redisplaying_p, a miss may defer.  */
struct it;
extern ptrdiff_t nh_img_redisplay_lookup (struct frame *, Lisp_Object spec,
					  int face_id, struct it *);
/* Task 30: the stand-in rule (image-bg.c decides, image.c draws), and
   `:nidhogg-data-encoding base64'.  */
extern bool nh_img_stand_in_for (struct frame *, struct image *);
extern bool nh_img_stand_in (struct frame *, struct image *, struct image *);
extern bool nh_img_b64_spec (Lisp_Object spec);
/* What image.c does for the pending entries (Cairo only; elsewhere
   nh_img_job_inputs refuses).  FMT is an enum nh_img_fmt.  */
struct nh_img_params;
struct nh_img_out;
extern bool nh_img_job_inputs (struct frame *, struct image *, int fmt,
			       struct nh_img_params *, uint8_t **data,
			       size_t *len, char **file);
extern bool nh_img_load_result (struct frame *, struct image *, int fmt,
				struct nh_img_out *, int rc);
extern void nh_img_land (struct frame *, struct image *, int fmt,
			 struct nh_img_out *, int rc);
extern bool nh_img_prepare (struct frame *, struct image *);
/* The watchdog's `image' phase around a synchronous decode (and its ns,
   PD-3): the depth to hand to nh_img_phase_end.  */
extern int nh_img_phase_begin (int64_t *t0);
extern void nh_img_phase_end (int depth, int64_t t0);
/* process.c, for nidhogg-set-process-reader: hand PROC back for good
   (ON false), or let the reader take it again if it is being read.  */
extern void nh_io_proc_reader (Lisp_Object proc, bool on);
/* Before a select over MASK with TIMEOUT: when TIMEOUT is zero (what is
   there now: status_notify's check, a zero-timeout wait), output the
   reader is reading, or has yet to read, for a process MASK selects
   lands on its wake fd first (bounded).  */
extern void nh_io_settle (fd_set *mask, struct timespec timeout);
/* Framing [24].  A framed delivery left its batch for
   read_and_dispose_of_process_output: nh_io_batch_begin hands it over
   (FN nil: the process's buffer is dead, the batch is dropped) and
   enters the `filter' phase; nh_io_batch_end leaves it.  */
struct nh_io_call
{
  Lisp_Object fn, batch;
  int depth;
  int64_t t0;
};
extern bool nh_io_batch_begin (struct Lisp_Process *, struct nh_io_call *);
extern void nh_io_batch_end (struct nh_io_call *);
/* `set-process-filter', `set-process-coding-system' and the reader's
   switch-off turn PROC's framing off: its pending bytes go to the
   filter as raw output.  */
extern void nh_io_unframe (Lisp_Object proc);
/* The materializer for the framed branch, nidhogg-json-tape.c [24]:
   OPTIONS is a plist of json-parse-string's keywords; nh_json_options
   checks one and returns it complete, in a fixed order.  SRC stays the
   caller's until nh_json_mat_free.  */
extern Lisp_Object nh_json_options (Lisp_Object options);
extern struct nh_mat *nh_json_mat_start (const struct nh_tape *,
					 const unsigned char *src,
					 size_t nsrc, Lisp_Object options);
extern Lisp_Object nh_json_mat_value (struct nh_mat *);
extern bool nh_json_mat_failed (struct nh_mat *);
extern void nh_json_mat_free (struct nh_mat *);
/* The stateless system CODING decodes with, or NH_DEC_NONE [18].  */
extern int nh_dec_classify (struct coding_system *);
extern Lisp_Object nh_dec_system_name (int);

/* Auto-save writes on nh-save, nidhogg-blk.c [34] (§8.6).  The keyboard
   loop's two auto-saves call nh_save_keyboard_auto_save instead of
   do-auto-save.  auto_save_1 calls nh_save_auto_save_1 (current buffer,
   mode bits) before it writes: true when the write was queued (or
   skipped as hung), false when it must write synchronously, as it does
   whenever it runs in signal context or on the fatal path.
   nh_save_due is do-auto-save's extra due test: a synchronous auto-save
   writes a buffer whose job has not landed.  delete-file-internal and
   rename-file report names, kill-buffer settles the buffer's job.
   nh_save_write_sync (fileio.c) is auto_save_1 for the current buffer,
   as do-auto-save runs it.  */
extern void nh_save_keyboard_auto_save (Lisp_Object no_message);
extern bool nh_save_auto_save_1 (struct buffer *, int mode);
extern bool nh_save_due (struct buffer *);
extern void nh_save_file_deleted (Lisp_Object name);
extern void nh_save_file_renamed (Lisp_Object from, Lisp_Object to);
extern void nh_save_settle (struct buffer *);
extern void nh_save_write_sync (void);

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

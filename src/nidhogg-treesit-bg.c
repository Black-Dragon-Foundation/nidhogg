/* Nidhogg's worker threads: tree-sitter parsing off the main thread.

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

/* Task 9: the allocator.  Task 10: parse jobs over snapshots, publish
   and ownership.  Task 11: the edit log and replay.  Task 12: the start
   points and the wait rule.  Task 13: deferral at redisplay.
   README.md, "Work off the main thread", is the user-facing
   description; the design's §7.1 and §7.3 to §7.5 the contract.

   A job (struct nh_ts_job, one per parser, the latest snapshot wins)
   holds a snapshot taken on the main thread: the visible bytes of the
   buffer's text, a copy of the parser's edited tree (ts_tree_copy: the
   subtrees are shared and copied on write), the ranges a demand would
   set now and the parser's edit generation.  nh-parse parses it with a
   tree-sitter parser of its own for that language, then computes the
   subtree statistics.  Nothing the worker touches belongs to Lisp.

   Ownership.  The parser holds one reference (Lisp_TS_Parser.nh_job),
   the worker another while the job is queued or running.  Deleting the
   parser, killing its buffer or GC freeing it cancels the job and drops
   the parser's reference; whichever side lets go last frees the job,
   on either thread (every block is malloc's or tree-sitter's, whose
   allocator picks per thread, and both free with free).

   Publish.  At a demand (treesit_ensure_parsed, after the narrowing
   sync) a done job whose language, visible range, ranges and edit
   generation equal the parser's gives its tree to today's tail
   (timestamp, changed ranges, notifiers, the old tree freed); any other
   job is dropped and the synchronous parse runs.  So notifiers run only
   at demand calls, and a node stays valid until the publish.

   The edit log (§7.3 item 5, §7.4).  While a parser has a job, every
   edit treesit.c applies to the parser's tree (treesit_record_change_1,
   treesit_sync_visible_region) is logged exactly as applied; a parser
   without a tree logs the buffer bytes of the change, relative to the
   snapshot's BEGV, and the publish turns them into tree edits the way
   treesit_record_change_1 would have (B3).  A parser that tracks line
   and column drops a tree-less job at its first edit (the tree edits
   would need the snapshot's line and column, MC-19).  Publish replays
   the log onto the job's tree and, if the log is not empty, reparses
   it once on the main thread (the watchdog's `treesit' phase) with the
   parser's current ranges: the same grouping a synchronous parse at
   the snapshot would have had.  Only the main thread touches the log.
   A start drops a job whose snapshot is older than the newest edit
   (the latest snapshot wins); a demand replays it.

   Start points (§7.3 item 1, MC-18), never inside insdel or a
   primitive.  DEMAND: while redisplay fontifies the parser's buffer
   (`nidhogg--redisplay-fontifying', Task 13 binds it) the demand of
   treesit-parser-changed-regions (treesit--pre-redisplay's) first
   parses on the main thread for at most 1 ms; a finished attempt
   publishes as today, a cancelled one is reset (never resumed) and a
   job takes the parse, the demand answering "not ready".  CREATE: after
   the major mode, a job for each parser that never parsed
   (nidhogg-workers-treesit.el).  SELECT: wait_reading_process_output
   with READ_KBD != 0 starts jobs for the parsers edited since it last
   ran (timers, filters) that no demand parsed.

   The wait rule (§7.5).  A demand that needs a tree while a job runs
   cancels it and parses on the main thread when it has not started,
   or when its snapshot is older than the newest edit and it has parsed
   less than half its input; otherwise it waits (`treesit-wait', the
   job raised as PD-15 says) for at most the synchronous estimate (the
   last full parse's ns per byte times the input), then publishes, or
   cancels and parses.

   Deferral (§7.1, Task 13).  With `nidhogg-treesit-defer' on,
   handle_fontified_prop binds `nidhogg--redisplay-fontifying' while
   redisplay (only redisplay: redisplaying_p) runs
   fontification-functions.  nidhogg-workers-treesit.el's wrappers make
   the demand above, wait for a job within the redisplay's budget
   (nidhogg-workers--treesit-await) and, if it is still running, leave
   the region's faces as they are and record it.  A job started with
   the switch on lands through the drainer (nh_ts_job_finish), which
   calls the Lisp landing and asks for a redisplay; the landing marks the
   recorded regions unfontified, and the redisplay's demand publishes.  */

#ifndef NH_WK_HARNESS
# include <config.h>
#endif

#ifdef HAVE_NIDHOGG_WORKERS

#include "nidhogg-workers.h"

#if !defined NH_WK_HARNESS && defined HAVE_TREE_SITTER

# include <errno.h>
# include <sched.h>
# include <signal.h>
# include <string.h>
# include <time.h>
# include "dynlib.h"
# include "buffer.h"
# include "coding.h"
# include "keyboard.h"
# include "treesit.h"
# include "nidhogg-workers-phase.h"

/* Tree-sitter's allocator (MC-15).  A worker never calls xmalloc (it
   probes the profiler, and memory_full would longjmp), so each wrapper
   asks which thread it runs on.  Blocks cross threads freely: both
   sides free with libc's free.  A failed allocation on a worker is
   fatal.  */

static bool nh_ts_alloc_installed;

static void *
nh_ts_malloc (size_t n)
{
  if (!nh_wk_on_worker ())
    return xmalloc (n);
  void *p = malloc (n);
  if (!p && n)
    abort ();
  return p;
}

static void *
nh_ts_calloc (size_t n, size_t size)
{
  if (!nh_wk_on_worker ())
    return xzalloc (n * size);
  void *p = calloc (n, size);
  if (!p && n && size)
    abort ();
  return p;
}

static void *
nh_ts_realloc (void *block, size_t n)
{
  if (!nh_wk_on_worker ())
    return xrealloc (block, n);
  void *p = realloc (block, n);
  if (!p && n)
    abort ();
  return p;
}

static void
nh_ts_free (void *block)
{
  if (!nh_wk_on_worker ())
    xfree (block);
  else
    free (block);
}

void
nh_ts_alloc_install (void)
{
  ts_set_allocator (nh_ts_malloc, nh_ts_calloc, nh_ts_realloc, nh_ts_free);
  nh_ts_alloc_installed = true;
}

/* Test primitive: parse TEXT with the grammar in LIBRARY on a raw
   thread flagged as a worker, 20 times.  */

struct nh_ts_alloc_test
{
  const char *library, *symbol, *text;
  ptrdiff_t len;
  ptrdiff_t nodes;
  bool ok;
};

static void *
nh_ts_alloc_test_thread (void *arg)
{
  struct nh_ts_alloc_test *t = arg;
  nh_wk_set_on_worker (true);
  dynlib_handle_ptr h = dynlib_open (t->library);
  if (h)
    {
      const TSLanguage *(*fn) (void)
	= (const TSLanguage *(*) (void)) dynlib_func (h, t->symbol);
      TSParser *p = ts_parser_new ();
      if (fn && p && ts_parser_set_language (p, fn ()))
	{
	  t->ok = true;
	  for (int i = 0; i < 20; i++)
	    {
	      TSTree *tree = ts_parser_parse_string (p, NULL, t->text,
						     t->len);
	      if (!tree)
		{
		  t->ok = false;
		  break;
		}
	      t->nodes += ts_node_child_count (ts_tree_root_node (tree));
	      ts_tree_delete (tree);
	    }
	}
      if (p)
	ts_parser_delete (p);
      /* The handle stays open, as treesit.c leaves every grammar it
	 loads: dynlib.c has dynlib_close only in native-comp builds.  */
    }
  return NULL;
}

DEFUN ("nidhogg-workers--test-ts-alloc", Fnidhogg_workers__test_ts_alloc,
       Snidhogg_workers__test_ts_alloc, 3, 3, 0,
       doc: /* Parse TEXT with the grammar in LIBRARY (symbol SYMBOL) on a worker-flagged thread.
Tree-sitter must already be initialized.  Return (NODES . PROBES), PROBES being
how many times the thread reached `malloc_probe'.  Test only.  */)
  (Lisp_Object library, Lisp_Object symbol, Lisp_Object text)
{
  CHECK_STRING (library);
  CHECK_STRING (symbol);
  CHECK_STRING (text);
  if (!nh_ts_alloc_installed)
    error ("Tree-sitter's allocator is not installed yet");
  library = ENCODE_FILE (library);
  struct nh_ts_alloc_test t
    = { SSDATA (library), SSDATA (symbol), SSDATA (text), SBYTES (text), 0,
	false };
  nh_wk_worker_probes = 0;
  sigset_t all, old;
  sigfillset (&all);
  pthread_sigmask (SIG_BLOCK, &all, &old);
  pthread_t th;
  int err = pthread_create (&th, NULL, nh_ts_alloc_test_thread, &t);
  pthread_sigmask (SIG_SETMASK, &old, NULL);
  if (err)
    error ("Cannot create a thread");
  pthread_join (th, NULL);
  if (!t.ok)
    error ("The worker parse failed");
  return Fcons (make_fixnum (t.nodes), make_fixnum (nh_wk_worker_probes));
}

/* Parse jobs (Task 10).  */

/* ts_parser_parse_with_options (tree-sitter 0.25, ABI 15) cancels
   through a progress callback; older libraries take no jobs.  */
#if TREE_SITTER_LANGUAGE_VERSION >= 15
# define NH_TS_JOBS true
#else
# define NH_TS_JOBS false
#endif

/* An edit made after the snapshot (main thread only).  */
struct nh_ts_edit
{
  TSInputEdit edit;		/* as applied to the parser's tree */
  /* A tree-less parser's change: buffer bytes from the snapshot's BEGV
     (B3).  */
  ptrdiff_t start, old_end, new_end;
};

/* The longest log; one more edit drops the job.  */
enum { NH_TS_LOG_MAX = 1 << 16 };

struct nh_ts_job
{
  struct nh_wk_job job;
  const TSLanguage *lang;
  char *text;			/* the visible bytes (malloc) */
  uint32_t len;
  TSRange *ranges;		/* malloc */
  uint32_t nranges;
  TSTree *old;			/* the parser's tree, copied; the worker frees it */
  TSTree *result;		/* the worker's parse; NULL if cancelled */
  ptrdiff_t gen, visible_beg, visible_end;
  ptrdiff_t stat[3];		/* (treesit-subtree-stat) of the result's root */
  /* The edit log: its entries (malloc), how many, the room, and the
     parser's edit generation it brings the job's tree to.  */
  struct nh_ts_edit *log;
  ptrdiff_t nlog, log_room, log_gen;
  bool tree_less;		/* the parser had no tree at the snapshot */
  int policy;			/* the runner's sched_getscheduler, or -1 */
};

/* The counters of `nidhogg-workers--test-ts-stats'.  */
enum
{
  NH_TS_C_STARTED,
  NH_TS_C_REFUSED,
  NH_TS_C_PUBLISHED,
  NH_TS_C_DISCARDED,
  NH_TS_C_STAT_CACHED,
  NH_TS_C_REPLAYED,
  NH_TS_C_REPLAYED_EDITS,
  NH_TS_C_SUPERSEDED,
  NH_TS_C_PARSES,
  NH_TS_C_ATTEMPTS,
  NH_TS_C_CANCELS,
  NH_TS_C_START_DEMAND,
  NH_TS_C_START_CREATE,
  NH_TS_C_START_SELECT,
  NH_TS_C_WAITED,
  NH_TS_C_ABANDONED,
  NH_TS_C_N
};
static _Atomic int64_t nh_ts_count[NH_TS_C_N];

/* The demand's synchronous attempt (§7.3 item 1), and the longest wait
   for a nearly done job whatever the estimate says.  */
# define NH_TS_ATTEMPT_NS INT64_C (1000000)
# define NH_TS_WAIT_MAX_NS INT64_C (2000000000)
/* The synchronous estimate (§7.5): ns per KiB of the last full parse
   of at least NH_TS_ESTIMATE_MIN bytes (a worker's, any language); 0
   until one, when NH_TS_DEFAULT_NS_PER_KIB (100 ns a byte) stands in.  */
static _Atomic int64_t nh_ts_ns_per_kib;
enum { NH_TS_ESTIMATE_MIN = 4096, NH_TS_DEFAULT_NS_PER_KIB = 100 * 1024 };

/* Jobs allocated and not yet freed; the condition variable is
   broadcast whenever one is freed.  */
static pthread_mutex_t nh_ts_live_m = PTHREAD_MUTEX_INITIALIZER;
static pthread_cond_t nh_ts_live_cv;
static ptrdiff_t nh_ts_live;
static pthread_once_t nh_ts_once = PTHREAD_ONCE_INIT;

static void
nh_ts_init_once (void)
{
  nh_wk_cond_init (&nh_ts_live_cv);
}

static int64_t
nh_ts_now (void)
{
  struct timespec ts;
  clock_gettime (CLOCK_MONOTONIC, &ts);
  return (int64_t) ts.tv_sec * 1000000000 + ts.tv_nsec;
}

static void
nh_ts_counted (int c)
{
  atomic_fetch_add_explicit (&nh_ts_count[c], 1, memory_order_relaxed);
}

/* The worker's side.  */

#if NH_TS_JOBS

/* Each thread's tree-sitter parsers, one per language (a TSLanguage is
   immutable and shared; a TSParser is not).  */
enum { NH_TS_THREAD_PARSERS = 8 };
static _Thread_local struct
{
  const TSLanguage *lang;
  TSParser *parser;
} nh_ts_tparsers[NH_TS_THREAD_PARSERS];
static _Thread_local int nh_ts_tparser_next;

static TSParser *
nh_ts_thread_parser (const TSLanguage *lang)
{
  for (int i = 0; i < NH_TS_THREAD_PARSERS; i++)
    if (nh_ts_tparsers[i].parser && nh_ts_tparsers[i].lang == lang)
      return nh_ts_tparsers[i].parser;
  int i = nh_ts_tparser_next++ % NH_TS_THREAD_PARSERS;
  if (nh_ts_tparsers[i].parser)
    ts_parser_delete (nh_ts_tparsers[i].parser);
  nh_ts_tparsers[i].parser = NULL;
  TSParser *p = ts_parser_new ();
  if (!ts_parser_set_language (p, lang))
    {
      ts_parser_delete (p);
      return NULL;
    }
  nh_ts_tparsers[i].lang = lang;
  nh_ts_tparsers[i].parser = p;
  return p;
}

/* The snapshot in one chunk.  */
static const char *
nh_ts_read (void *payload, uint32_t byte_index, TSPoint position,
	    uint32_t *bytes_read)
{
  struct nh_ts_job *t = payload;
  (void) position;
  if (byte_index >= t->len)
    {
      *bytes_read = 0;
      return "";
    }
  *bytes_read = t->len - byte_index;
  return t->text + byte_index;
}

static bool
nh_ts_progress (TSParseState *state)
{
  struct nh_ts_job *t = state->payload;
  atomic_store_explicit (&t->job.progress, state->current_byte_offset,
			 memory_order_relaxed);
  return atomic_load (&t->job.cancel) || atomic_load (&nh_wk_dead);
}

#endif /* NH_TS_JOBS */

/* Ftreesit_subtree_stat's walk, without Lisp: (MAX-DEPTH MAX-WIDTH
   COUNT) of the subtree of NODE into OUT.  */
static void
nh_ts_stat_walk (TSNode node, ptrdiff_t out[3])
{
  TSTreeCursor cursor = ts_tree_cursor_new (node);
  ptrdiff_t max_depth = 1, max_width = 0, count = 0, depth = 0;
  for (;;)
    {
      count++;
      while (ts_tree_cursor_goto_first_child (&cursor))
	{
	  depth++;
	  count++;
	  ptrdiff_t width = 1;
	  while (ts_tree_cursor_goto_next_sibling (&cursor))
	    width++;
	  max_width = max (max_width, width);
	  ts_tree_cursor_goto_parent (&cursor);
	  ts_tree_cursor_goto_first_child (&cursor);
	}
      max_depth = max (max_depth, depth);
      while (!ts_tree_cursor_goto_next_sibling (&cursor))
	{
	  if (ts_tree_cursor_goto_parent (&cursor))
	    depth--;
	  else
	    {
	      ts_tree_cursor_delete (&cursor);
	      out[0] = max_depth;
	      out[1] = max_width;
	      out[2] = count;
	      return;
	    }
	}
    }
}

static void
nh_ts_job_run (struct nh_wk_job *job)
{
  struct nh_ts_job *t = (struct nh_ts_job *) job;
  TSTree *tree = NULL;

#ifdef __linux__
  t->policy = sched_getscheduler (0);
#endif
  atomic_store (&job->total, t->len);
  nh_wk_pause_point (NH_WK_P_AFTER_SNAPSHOT);
#if NH_TS_JOBS
  TSParser *p = (atomic_load (&job->cancel) ? NULL
		 : nh_ts_thread_parser (t->lang));
  if (p && ts_parser_set_included_ranges (p, t->ranges, t->nranges))
    {
      nh_ts_counted (NH_TS_C_PARSES);
      TSInput input = { .payload = t, .read = nh_ts_read,
			.encoding = TSInputEncodingUTF8 };
      TSParseOptions opts = { .payload = t,
			      .progress_callback = nh_ts_progress };
      bool full = !t->old;
      int64_t t0 = nh_ts_now ();
      tree = ts_parser_parse_with_options (p, t->old, input, opts);
      /* A cancelled parse is never resumed (§7.2).  */
      if (!tree)
	ts_parser_reset (p);
      else if (full && t->len >= NH_TS_ESTIMATE_MIN)
	atomic_store_explicit (&nh_ts_ns_per_kib,
			       (nh_ts_now () - t0) * 1024 / t->len,
			       memory_order_relaxed);
    }
#endif
  if (t->old)
    ts_tree_delete (t->old);
  t->old = NULL;
  if (tree)
    nh_ts_stat_walk (ts_tree_root_node (tree), t->stat);
  t->result = tree;
  nh_wk_pause_point (NH_WK_P_BEFORE_DONE);
}

/* The last reference is gone (either thread).  */
static void
nh_ts_job_free (struct nh_wk_job *job)
{
  struct nh_ts_job *t = (struct nh_ts_job *) job;
  if (t->old)
    ts_tree_delete (t->old);
  if (t->result)
    ts_tree_delete (t->result);
  free (t->text);
  free (t->ranges);
  free (t->log);
  free (t);
  nh_wk_lock (&nh_ts_live_m);
  nh_ts_live--;
  pthread_cond_broadcast (&nh_ts_live_cv);
  pthread_mutex_unlock (&nh_ts_live_m);
}

/* The main thread's side.  */

static bool
nh_ts_switch_on (void)
{
  Lisp_Object v = find_symbol_value (Qnidhogg_workers_treesit);
  return !BASE_EQ (v, Qunbound) && !NILP (v);
}

/* `nidhogg-treesit-defer' (Task 13).  */
static bool
nh_ts_defer_on (void)
{
  Lisp_Object v = find_symbol_value (Qnidhogg_treesit_defer);
  return !BASE_EQ (v, Qunbound) && !NILP (v);
}

/* The drainer (PD-6), main thread: a job started with deferral on has
   landed.  The Lisp landing marks the regions redisplay deferred
   unfontified once their buffer has no job left running; then redisplay
   runs, after this timer check, and its demand publishes the tree.  */
static void
nh_ts_job_finish (struct nh_wk_job *job)
{
  (void) job;
  if (!NILP (Ffboundp (Qnidhogg_workers_treesit__land)))
    safe_calln (Qnidhogg_workers_treesit__land);
  /* A wait for input redisplays after a timer check that ran something
     (process.c, keyboard.c).  */
  timers_run++;
}

static TSRange
nh_ts_range (struct buffer *b, ptrdiff_t beg, ptrdiff_t end)
{
  ptrdiff_t begv_byte = BUF_BEGV_BYTE (b);
  return (TSRange) {
    .start_byte = (uint32_t) (buf_charpos_to_bytepos (b, beg) - begv_byte),
    .end_byte = (uint32_t) (buf_charpos_to_bytepos (b, end) - begv_byte)
  };
}

/* The ranges a demand on P would parse with now, in a malloc'd array
   (NULL if out of memory), their number in *N.  With a tree, that is
   what treesit_sync_visible_region computes from last_set_ranges (it
   clips them to the visible region, in place: this leaves them
   untouched); without one, or with no ranges set, the parser's own.  */
static TSRange *
nh_ts_demand_ranges (struct Lisp_TS_Parser *p, struct buffer *b,
		     uint32_t *n)
{
  Lisp_Object r = p->last_set_ranges;
  if (!p->tree || NILP (r))
    {
      uint32_t len;
      const TSRange *cur = ts_parser_included_ranges (p->parser, &len);
      TSRange *out = malloc ((len ? len : 1) * sizeof *out);
      if (out && len)
	memcpy (out, cur, len * sizeof *out);
      *n = len;
      return out;
    }
  ptrdiff_t begv = BUF_BEGV (b), zv = BUF_ZV (b);
  TSRange *out = malloc ((list_length (r) + 1) * sizeof *out);
  if (!out)
    return NULL;
  uint32_t k = 0;
  FOR_EACH_TAIL_SAFE (r)
    {
      Lisp_Object range = XCAR (r);
      ptrdiff_t beg = XFIXNUM (XCAR (range)), end = XFIXNUM (XCDR (range));
      if (end <= begv)
	continue;
      if (beg >= zv)
	break;
      out[k++] = nh_ts_range (b, max (beg, begv), min (end, zv));
    }
  /* The sync's "no range overlaps" case.  */
  if (k == 0)
    out[k++] = nh_ts_range (b, begv, begv);
  *n = k;
  return out;
}

/* Would a demand on P now parse exactly what T parsed?  */
static bool
nh_ts_job_current (struct Lisp_TS_Parser *p, struct nh_ts_job *t,
		   struct buffer *b)
{
  if (t->gen != p->nh_gen || t->lang != ts_parser_language (p->parser)
      || t->visible_beg != BUF_BEGV_BYTE (b)
      || t->visible_end != BUF_ZV_BYTE (b))
    return false;
  uint32_t n;
  TSRange *r = nh_ts_demand_ranges (p, b, &n);
  bool same = r && n == t->nranges;
  for (uint32_t i = 0; same && i < n; i++)
    same = (r[i].start_byte == t->ranges[i].start_byte
	    && r[i].end_byte == t->ranges[i].end_byte);
  free (r);
  return same;
}

static bool
nh_ts_job_done (struct nh_ts_job *t)
{
  return (atomic_load (&t->job.state) == NH_WK_J_DONE
	  && !atomic_load (&t->job.cancel) && t->result);
}

/* Has T's worker let go of it (done, or cancelled)?  */
static bool
nh_ts_job_finished (struct nh_ts_job *t)
{
  int st = atomic_load (&t->job.state);
  return st == NH_WK_J_DONE || st == NH_WK_J_CANCELLED;
}

/* §7.5: is T's snapshot older than P's newest edit, with less than half
   its input parsed (or its parse not begun)?  */
static bool
nh_ts_job_behind (struct Lisp_TS_Parser *p, struct nh_ts_job *t)
{
  uint64_t total = atomic_load (&t->job.total);
  uint64_t done = atomic_load (&t->job.progress);
  return t->gen < p->nh_gen && (total == 0 || 2 * done < total);
}

/* The synchronous estimate of parsing LEN bytes, in ns, within the
   attempt's 1 ms and NH_TS_WAIT_MAX_NS.  */
static int64_t
nh_ts_estimate (uint32_t len)
{
  int64_t per = atomic_load_explicit (&nh_ts_ns_per_kib,
				      memory_order_relaxed);
  if (per <= 0)
    per = NH_TS_DEFAULT_NS_PER_KIB;
  int64_t ns = per * len / 1024;
  return max (NH_TS_ATTEMPT_NS, min (ns, NH_TS_WAIT_MAX_NS));
}

/* Cancel P's job and drop the parser's reference.  */
static void
nh_ts_drop (struct Lisp_TS_Parser *p)
{
  struct nh_ts_job *t = p->nh_job;
  if (!t)
    return;
  p->nh_job = NULL;
  nh_wk_cancel (&t->job);
  nh_wk_job_unref (&t->job);
}

/* Is the view the parser's tree has the buffer's visible region?  A
   parser without a tree has none, so a narrowed buffer counts as a
   temporary narrowing then.  */
static bool
nh_ts_view_is_visible (struct Lisp_TS_Parser *p, struct buffer *b)
{
  if (p->tree)
    return (p->visible_beg == BUF_BEGV_BYTE (b)
	    && p->visible_end == BUF_ZV_BYTE (b));
  return BUF_BEGV (b) == BUF_BEG (b) && BUF_ZV (b) == BUF_Z (b);
}

/* Start a job for PARSER over a snapshot, unless one for the current
   text runs or is done.  Return true if such a job exists now.  Never
   touches the parser's visible range, ranges or tree.  FROM_DEMAND:
   called by treesit_ensure_parsed itself, after its narrowing sync and
   before it parses (any other caller inside a reparse is refused).  */
static bool
nh_ts_start (Lisp_Object parser, bool from_demand)
{
  struct Lisp_TS_Parser *p = XTS_PARSER (parser);
  struct buffer *b = XBUFFER (p->buffer);

  if (!NH_TS_JOBS || !nh_ts_switch_on () || p->deleted || !BUFFER_LIVE_P (b)
      || (p->within_reparse && !from_demand) || !p->need_reparse
      || !nh_ts_view_is_visible (p, b)
      || BUF_Z_BYTE (b) - BUF_BEG_BYTE (b) > UINT32_MAX)
    {
      nh_ts_counted (NH_TS_C_REFUSED);
      return false;
    }
  struct nh_ts_job *cur = p->nh_job;
  if (cur && !atomic_load (&cur->job.cancel)
      && atomic_load (&cur->job.state) != NH_WK_J_CANCELLED
      && nh_ts_job_current (p, cur, b))
    return true;
  /* The latest snapshot wins (MC-6): a job over older text, even a done
     one whose log a demand could replay, gives way.  */
  if (cur)
    nh_ts_counted (NH_TS_C_SUPERSEDED);
  nh_ts_drop (p);

  int64_t t0 = nh_ts_now ();
  int depth = NH_WK_PHASE_PUSH (NH_WK_PH_TREESIT, NH_WK_RS_SNAPSHOT);
  ptrdiff_t beg = BUF_BEGV_BYTE (b), end = BUF_ZV_BYTE (b);
  ptrdiff_t gpt = BUF_GPT_BYTE (b), len = end - beg;
  struct nh_ts_job *t = calloc (1, sizeof *t);
  char *text = malloc (len ? len : 1);
  uint32_t nranges = 0;
  TSRange *ranges = nh_ts_demand_ranges (p, b, &nranges);
  if (!t || !text || !ranges)
    {
      free (t);
      free (text);
      free (ranges);
      NH_WK_PHASE_POP (depth);
      nh_ts_counted (NH_TS_C_REFUSED);
      return false;
    }
  /* The text around the gap (an indirect buffer's is its base's).  */
  if (beg < gpt)
    memcpy (text, BUF_BYTE_ADDRESS (b, beg), min (end, gpt) - beg);
  if (end > gpt)
    {
      ptrdiff_t from = max (beg, gpt);
      memcpy (text + (from - beg), BUF_BYTE_ADDRESS (b, from), end - from);
    }
  t->text = text;
  t->len = len;
  t->ranges = ranges;
  t->nranges = nranges;
  t->lang = ts_parser_language (p->parser);
  t->old = p->tree ? ts_tree_copy (p->tree) : NULL;
  t->gen = t->log_gen = p->nh_gen;
  t->tree_less = !p->tree;
  t->policy = -1;
  t->visible_beg = beg;
  t->visible_end = end;
  t->job.w = NH_WK_PARSE;
  t->job.speculative = true;
  t->job.run = nh_ts_job_run;
  /* Only a job redisplay may have deferred colour for lands (Task 13):
     the drainer holds the worker's reference until it runs.  */
  t->job.finish = nh_ts_defer_on () ? nh_ts_job_finish : NULL;
  t->job.free = nh_ts_job_free;
  atomic_store (&t->job.refs, 1);	/* the parser's */
  NH_WK_PHASE_POP (depth);
  nh_wk_phase_ns_add (NH_WK_PH_TREESIT, NH_WK_RS_SNAPSHOT, nh_ts_now () - t0);

  pthread_once (&nh_ts_once, nh_ts_init_once);
  nh_wk_lock (&nh_ts_live_m);
  nh_ts_live++;
  pthread_mutex_unlock (&nh_ts_live_m);
  if (!nh_wk_submit (&t->job))
    {
      nh_wk_job_unref (&t->job);
      nh_ts_counted (NH_TS_C_REFUSED);
      return false;
    }
  p->nh_job = t;
  nh_ts_counted (NH_TS_C_STARTED);
  return true;
}

/* The parsers edited since the select entry last ran (Task 12): main
   thread or a Lisp thread, under the global lock.  A parser leaves the
   list when it is deleted or freed; past NH_TS_DIRTY_MAX of them the
   select entry looks at every buffer's parsers instead.  */
enum { NH_TS_DIRTY_MAX = 32 };
static struct Lisp_TS_Parser *nh_ts_dirty[NH_TS_DIRTY_MAX];
static int nh_ts_ndirty;
static bool nh_ts_dirty_all;

static void
nh_ts_mark_dirty (struct Lisp_TS_Parser *p)
{
  for (int i = nh_ts_ndirty - 1; i >= 0; i--)
    if (nh_ts_dirty[i] == p)
      return;
  if (nh_ts_ndirty < NH_TS_DIRTY_MAX)
    nh_ts_dirty[nh_ts_ndirty++] = p;
  else
    nh_ts_dirty_all = true;
}

static void
nh_ts_unmark_dirty (struct Lisp_TS_Parser *p)
{
  for (int i = 0; i < nh_ts_ndirty; i++)
    if (nh_ts_dirty[i] == p)
      {
	nh_ts_dirty[i] = nh_ts_dirty[--nh_ts_ndirty];
	return;
      }
}

/* The edit log (Task 11).  */

static bool
nh_ts_log_push (struct nh_ts_job *t, const struct nh_ts_edit *e)
{
  if (t->nlog == t->log_room)
    {
      if (t->log_room >= NH_TS_LOG_MAX)
	return false;
      ptrdiff_t room = t->log_room ? 2 * t->log_room : 16;
      /* malloc's, not xmalloc's: the worker may free the job.  */
      struct nh_ts_edit *log = realloc (t->log, room * sizeof *log);
      if (!log)
	return false;
      t->log = log;
      t->log_room = room;
    }
  t->log[t->nlog++] = *e;
  return true;
}

/* treesit.c applied TS_INPUT_EDIT (a TSInputEdit) to PARSER_C's tree,
   or, with TS_INPUT_EDIT NULL, recorded a change from START_BYTE to
   OLD_END_BYTE (now NEW_END_BYTE) in a parser without a tree.  Called
   from insdel: no Lisp, no signal.  */
void
nh_ts_record_edit (void *parser_c, const void *ts_input_edit,
		   ptrdiff_t start_byte, ptrdiff_t old_end_byte,
		   ptrdiff_t new_end_byte)
{
  struct Lisp_TS_Parser *p = parser_c;
  /* The select entry's candidates (no Lisp: a pointer in an array).  */
  nh_ts_mark_dirty (p);
  struct nh_ts_job *t = p->nh_job;
  if (!t)
    return;
  struct nh_ts_edit e = { .start = start_byte - t->visible_beg,
			  .old_end = old_end_byte - t->visible_beg,
			  .new_end = new_end_byte - t->visible_beg };
  if (ts_input_edit)
    e.edit = *(const TSInputEdit *) ts_input_edit;
  bool tree_less = !ts_input_edit;
  /* A job whose tree and the parser's disagree on having one cannot be
     replayed; nor can a tree-less job of a parser that tracks line and
     column (MC-19), or a log out of room.  */
  if (tree_less != t->tree_less
      || (tree_less && p->visi_beg_linecol.bytepos != 0)
      || !nh_ts_log_push (t, &e))
    {
      nh_ts_drop (p);
      nh_ts_counted (NH_TS_C_DISCARDED);
      return;
    }
  t->log_gen = p->nh_gen;
}

/* Apply T's log to TREE, a tree of T's snapshot.  A tree-less job's
   entries become the edits treesit_record_change_1 would have made,
   clipped to the view the job's tree has, which must end up being the
   buffer B's visible region.  Return false if the log cannot apply.  */
static bool
nh_ts_replay (struct nh_ts_job *t, TSTree *tree, struct buffer *b)
{
  /* The tree-less view, in bytes from the snapshot's BEGV.  */
  ptrdiff_t vb = 0, ve = t->visible_end - t->visible_beg;
  for (ptrdiff_t i = 0; i < t->nlog; i++)
    {
      struct nh_ts_edit *e = &t->log[i];
      if (t->tree_less)
	{
	  ptrdiff_t s = min (ve, max (vb, e->start)) - vb;
	  ptrdiff_t oe = min (ve, max (vb, e->old_end)) - vb;
	  ptrdiff_t ne = max (vb, e->new_end) - vb;
	  ptrdiff_t delta = (e->old_end > e->new_end
			     ? min (vb, e->new_end) - min (vb, e->old_end)
			     : e->old_end < vb ? e->new_end - e->old_end : 0);
	  if (s < 0 || s > oe || s > ne || oe > UINT32_MAX || ne > UINT32_MAX)
	    return false;
	  vb += delta;
	  ve += delta + (ne - oe);
	  /* treesit.c's TREESIT_TS_POINT_1_0.  */
	  TSPoint none = { 1, 0 };
	  e->edit = (TSInputEdit) { (uint32_t) s, (uint32_t) oe,
				    (uint32_t) ne, none, none, none };
	}
      ts_tree_edit (tree, &e->edit);
    }
  return (!t->tree_less
	  || (vb + t->visible_beg == BUF_BEGV_BYTE (b)
	      && ve + t->visible_beg == BUF_ZV_BYTE (b)));
}

/* At a demand that must reparse: true when P's finished job's tree (in
   *TREE, now the caller's) is the parse of the current text.  A
   finished job that cannot give one is dropped; a job still queued or
   running stays (nh_ts_demand decides).  */
static bool
nh_ts_publish (Lisp_Object parser, void **tree)
{
  struct Lisp_TS_Parser *p = XTS_PARSER (parser);
  struct nh_ts_job *t = p->nh_job;
  if (!t || !nh_ts_job_finished (t))
    return false;
  struct buffer *b = XBUFFER (p->buffer);
  /* With edits since the snapshot, the log must hold every one of them
     (the reparse below takes the current ranges); without, the job
     must have parsed exactly what a demand would.  */
  bool replay = t->nlog > 0;
  if (!nh_ts_job_done (t) || t->log_gen != p->nh_gen
      || (replay ? t->lang != ts_parser_language (p->parser)
	  : !nh_ts_job_current (p, t, b)))
    {
      nh_ts_drop (p);
      nh_ts_counted (NH_TS_C_DISCARDED);
      return false;
    }
  TSTree *result = t->result;
  t->result = NULL;
  if (replay)
    {
      /* One parse on this thread, from the job's tree with the edits
	 replayed: the caller has synced the visible region, so the
	 parser's input reads what a synchronous parse would.  */
      int64_t t0 = nh_ts_now ();
      int depth = NH_WK_PHASE_PUSH (NH_WK_PH_TREESIT, 0);
      TSTree *reparsed = (nh_ts_replay (t, result, b)
			  ? ts_parser_parse (p->parser, result, p->input)
			  : NULL);
      ts_tree_delete (result);
      NH_WK_PHASE_POP (depth);
      nh_wk_phase_ns_add (NH_WK_PH_TREESIT, 0, nh_ts_now () - t0);
      if (!reparsed)
	{
	  nh_ts_drop (p);
	  nh_ts_counted (NH_TS_C_DISCARDED);
	  return false;
	}
      atomic_fetch_add_explicit (&nh_ts_count[NH_TS_C_REPLAYED_EDITS],
				 t->nlog, memory_order_relaxed);
      nh_ts_counted (NH_TS_C_REPLAYED);
      result = reparsed;
      /* The worker's statistics are of the tree before the edits.  */
      p->nh_stat_timestamp = -1;
    }
  else
    {
      memcpy (p->nh_stat, t->stat, sizeof p->nh_stat);
      /* The caller's tail increments the timestamp next.  */
      p->nh_stat_timestamp = p->timestamp + 1;
    }
  *tree = result;
  p->nh_job = NULL;
  nh_wk_job_unref (&t->job);
  nh_ts_counted (NH_TS_C_PUBLISHED);
  return true;
}

/* Start points and the wait rule (Task 12).  */

void
nh_ts_maybe_start (Lisp_Object parser, int start_point)
{
  static const int counter[] = {
    [NH_TS_START_DEMAND] = NH_TS_C_START_DEMAND,
    [NH_TS_START_CREATE] = NH_TS_C_START_CREATE,
    [NH_TS_START_SELECT] = NH_TS_C_START_SELECT
  };
  int64_t started = atomic_load (&nh_ts_count[NH_TS_C_STARTED]);
  nh_ts_start (parser, start_point == NH_TS_START_DEMAND);
  if (atomic_load (&nh_ts_count[NH_TS_C_STARTED]) != started)
    nh_ts_counted (counter[start_point]);
}

#if NH_TS_JOBS

static bool
nh_ts_attempt_progress (TSParseState *state)
{
  const int64_t *deadline = state->payload;
  return nh_ts_now () >= *deadline;
}

#endif

/* The demand's attempt (§7.3 item 1): P's parse on this thread, as
   treesit_ensure_parsed would run it, cancelled after 1 ms.  A
   cancelled parse is reset, never resumed (§7.2).  Return the tree, or
   NULL if cancelled.  */
static TSTree *
nh_ts_attempt (struct Lisp_TS_Parser *p)
{
#if NH_TS_JOBS
  nh_ts_counted (NH_TS_C_ATTEMPTS);
  int64_t t0 = nh_ts_now ();
  int64_t deadline = t0 + NH_TS_ATTEMPT_NS;
  int depth = NH_WK_PHASE_PUSH (NH_WK_PH_TREESIT, 0);
  TSParseOptions opts = { .payload = &deadline,
			  .progress_callback = nh_ts_attempt_progress };
  TSTree *tree = ts_parser_parse_with_options (p->parser, p->tree, p->input,
					       opts);
  if (!tree)
    {
      ts_parser_reset (p->parser);
      nh_ts_counted (NH_TS_C_CANCELS);
    }
  NH_WK_PHASE_POP (depth);
  nh_wk_phase_ns_add (NH_WK_PH_TREESIT, 0, nh_ts_now () - t0);
  return tree;
#else
  (void) p;
  return NULL;
#endif
}

/* §7.5: a demand that needs a tree now, while P's job is queued or
   running.  Cancel a job that has not started, or that is over older
   text and short of half its input, and let the synchronous parse run;
   wait for any other within the synchronous estimate (nh_wk_wait raises
   it, PD-15), then publish it, or cancel it and parse.  */
static int
nh_ts_wait_or_cancel (Lisp_Object parser, void **tree)
{
  struct Lisp_TS_Parser *p = XTS_PARSER (parser);
  struct nh_ts_job *t = p->nh_job;
  if (atomic_load (&t->job.state) != NH_WK_J_QUEUED
      && !nh_ts_job_behind (p, t))
    {
      nh_ts_counted (NH_TS_C_WAITED);
      if (nh_wk_wait (&t->job, nh_ts_estimate (t->len),
		      NH_WK_PH_TREESIT_WAIT, 0) == 1
	  && nh_ts_publish (parser, tree))
	return NH_TS_TREE;
    }
  /* nh_ts_publish dropped a finished job it could not publish.  */
  if (p->nh_job)
    {
      nh_ts_drop (p);
      nh_ts_counted (NH_TS_C_ABANDONED);
    }
  return NH_TS_SYNC;
}

int
nh_ts_demand (Lisp_Object parser, bool may_defer, void **tree)
{
  struct Lisp_TS_Parser *p = XTS_PARSER (parser);
  if (nh_ts_publish (parser, tree))
    return NH_TS_TREE;
  /* The demand path (review S2): only while redisplay fontifies this
     parser's buffer, and only for the demand that may answer "not
     ready" (treesit-parser-changed-regions).  */
  bool defer = (NH_TS_JOBS && may_defer
		&& EQ (Vnidhogg__redisplay_fontifying, p->buffer)
		&& nh_ts_switch_on ());
  if (p->nh_job && !defer)
    return nh_ts_wait_or_cancel (parser, tree);
  if (p->nh_job)
    {
      /* The job carries on (its log replays the newer edits at the
	 publish) unless it is behind: then the latest snapshot wins.  */
      if (!nh_ts_job_behind (p, p->nh_job))
	return NH_TS_NOT_READY;
      nh_ts_drop (p);
      nh_ts_counted (NH_TS_C_ABANDONED);
    }
  if (!defer)
    return NH_TS_SYNC;
  TSTree *quick = nh_ts_attempt (p);
  if (quick)
    {
      *tree = quick;
      return NH_TS_TREE;
    }
  nh_ts_maybe_start (parser, NH_TS_START_DEMAND);
  /* An inline job is done already.  */
  if (nh_ts_publish (parser, tree))
    return NH_TS_TREE;
  /* No job (no thread, the switch...): today's parse, nothing defers.  */
  return p->nh_job ? NH_TS_NOT_READY : NH_TS_SYNC;
}

/* The select entry's start for one edited parser.  */
static void
nh_ts_select_one (struct Lisp_TS_Parser *p)
{
  if (!p->deleted && !p->nh_job && p->need_reparse
      && BUFFER_LIVE_P (XBUFFER (p->buffer)))
    nh_ts_maybe_start (make_lisp_ptr (p, Lisp_Vectorlike),
		       NH_TS_START_SELECT);
}

void
nh_ts_select_entry (void)
{
  if (nh_ts_ndirty == 0 && !nh_ts_dirty_all)
    return;
  struct Lisp_TS_Parser *dirty[NH_TS_DIRTY_MAX];
  int n = nh_ts_ndirty;
  bool all = nh_ts_dirty_all;
  memcpy (dirty, nh_ts_dirty, n * sizeof *dirty);
  nh_ts_ndirty = 0;
  nh_ts_dirty_all = false;
  if (!NH_TS_JOBS || !nh_ts_switch_on () || nh_wk_signal_context ())
    return;
  /* No Lisp runs and nothing conses below, so no GC frees a parser in
     DIRTY meanwhile.  */
  for (int i = 0; i < n; i++)
    nh_ts_select_one (dirty[i]);
  if (all)
    {
      Lisp_Object tail, buf;
      FOR_EACH_LIVE_BUFFER (tail, buf)
	{
	  /* An indirect buffer's parsers are on its base buffer's list.  */
	  if (XBUFFER (buf)->base_buffer)
	    continue;
	  Lisp_Object parsers = BVAR (XBUFFER (buf), ts_parser_list);
	  FOR_EACH_TAIL_SAFE (parsers)
	    if (TS_PARSERP (XCAR (parsers)))
	      nh_ts_select_one (XTS_PARSER (XCAR (parsers)));
	}
    }
}

Lisp_Object
nh_ts_subtree_stat_cached (Lisp_Object node)
{
  struct Lisp_TS_Node *n = XTS_NODE (node);
  struct Lisp_TS_Parser *p = XTS_PARSER (n->parser);
  if (p->nh_stat_timestamp != p->timestamp || n->timestamp != p->timestamp
      || !p->tree || !ts_node_eq (n->node, ts_tree_root_node (p->tree)))
    return Qnil;
  nh_ts_counted (NH_TS_C_STAT_CACHED);
  return list3 (make_fixnum (p->nh_stat[0]), make_fixnum (p->nh_stat[1]),
		make_fixnum (p->nh_stat[2]));
}

void
nh_ts_parser_freed (void *parser_c)
{
  nh_ts_unmark_dirty (parser_c);
  nh_ts_drop (parser_c);
}

void
nh_ts_buffer_killed (struct buffer *b)
{
  /* An indirect buffer's parsers are on its base buffer's list.  */
  struct buffer *base = b->base_buffer ? b->base_buffer : b;
  Lisp_Object tail = BVAR (base, ts_parser_list);
  FOR_EACH_TAIL_SAFE (tail)
    if (TS_PARSERP (XCAR (tail))
	&& XBUFFER (XTS_PARSER (XCAR (tail))->buffer) == b)
      nh_ts_drop (XTS_PARSER (XCAR (tail)));
}

static struct Lisp_TS_Parser *
nh_ts_check_parser (Lisp_Object parser)
{
  CHECK_TS_PARSER (parser);
  if (XTS_PARSER (parser)->deleted)
    xsignal1 (Qtreesit_parser_deleted, parser);
  return XTS_PARSER (parser);
}

DEFUN ("nidhogg-treesit-parser-ready-p", Fnidhogg_treesit_parser_ready_p,
       Snidhogg_treesit_parser_ready_p, 1, 1, 0,
       doc: /* Return t if PARSER's tree can be had without parsing on this thread.
That is: no reparse is needed, or a worker has finished parsing the
current text (the next demand publishes its tree).  */)
  (Lisp_Object parser)
{
  struct Lisp_TS_Parser *p = nh_ts_check_parser (parser);
  struct buffer *b = XBUFFER (p->buffer);
  if (!BUFFER_LIVE_P (b))
    return Qnil;
  if (p->tree && !p->need_reparse && nh_ts_view_is_visible (p, b))
    return Qt;
  struct nh_ts_job *t = p->nh_job;
  return t && nh_ts_job_done (t) && nh_ts_job_current (p, t, b) ? Qt : Qnil;
}

DEFUN ("nidhogg-workers--treesit-start-created",
       Fnidhogg_workers__treesit_start_created,
       Snidhogg_workers__treesit_start_created, 0, 0, 0,
       doc: /* Start a parse job for each parser of this buffer that never parsed.
The create start point: `nidhogg-workers-treesit--start-created' calls
it on `after-change-major-mode-hook', so the first parse of a file
overlaps the rest of `find-file'.  Return how many jobs started.  */)
  (void)
{
  struct buffer *b = current_buffer;
  struct buffer *base = b->base_buffer ? b->base_buffer : b;
  int64_t started = atomic_load (&nh_ts_count[NH_TS_C_START_CREATE]);
  Lisp_Object tail = BVAR (base, ts_parser_list);
  FOR_EACH_TAIL_SAFE (tail)
    if (TS_PARSERP (XCAR (tail)))
      {
	struct Lisp_TS_Parser *p = XTS_PARSER (XCAR (tail));
	if (XBUFFER (p->buffer) == b && !p->deleted && !p->tree
	    && !p->nh_job)
	  nh_ts_maybe_start (XCAR (tail), NH_TS_START_CREATE);
      }
  return make_int (atomic_load (&nh_ts_count[NH_TS_C_START_CREATE])
		   - started);
}

/* Deferral at redisplay (Task 13).  */

DEFUN ("nidhogg-workers--treesit-await", Fnidhogg_workers__treesit_await,
       Snidhogg_workers__treesit_await, 1, 2, 0,
       doc: /* Wait for PARSER's parse job within the redisplay's wait budget.
Return t when nothing runs for PARSER any more (no job, or its job
finished, in time or before), so a demand would not wait; nil when its
job still runs once the grant (the synchronous estimate, at most what
is left of `nidhogg-redisplay-wait-budget' in this redisplay) is spent.
With COUNT non-nil, a nil answer counts a tree-sitter deferral
\(`:deferrals' of `nidhogg-workers-stats').  Internal: the redisplay
wrappers of nidhogg-workers-treesit.el call it.  */)
  (Lisp_Object parser, Lisp_Object count)
{
  struct Lisp_TS_Parser *p = nh_ts_check_parser (parser);
  struct nh_ts_job *t = p->nh_job;
  if (!t || nh_ts_job_finished (t))
    return Qt;
  /* No Lisp runs during the wait: T stays the parser's job.  */
  int64_t grant = nh_wk_budget_take (nh_ts_estimate (t->len));
  if (grant > 0)
    nh_wk_wait (&t->job, grant, NH_WK_PH_TREESIT_WAIT, 0);
  if (nh_ts_job_finished (t))
    return Qt;
  if (!NILP (count))
    nh_wk_deferral_add (0);
  return Qnil;
}

DEFUN ("nidhogg-workers--treesit-busy-p", Fnidhogg_workers__treesit_busy_p,
       Snidhogg_workers__treesit_busy_p, 1, 1, 0,
       doc: /* Return t if a parser of BUFFER has a parse job queued or running.
Internal: the landing of deferred regions waits until none has.  */)
  (Lisp_Object buffer)
{
  CHECK_BUFFER (buffer);
  struct buffer *b = XBUFFER (buffer);
  if (!BUFFER_LIVE_P (b))
    return Qnil;
  /* An indirect buffer's parsers are on its base buffer's list.  */
  struct buffer *base = b->base_buffer ? b->base_buffer : b;
  Lisp_Object tail = BVAR (base, ts_parser_list);
  FOR_EACH_TAIL_SAFE (tail)
    if (TS_PARSERP (XCAR (tail)))
      {
	struct Lisp_TS_Parser *p = XTS_PARSER (XCAR (tail));
	if (XBUFFER (p->buffer) == b && p->nh_job
	    && !nh_ts_job_finished (p->nh_job))
	  return Qt;
      }
  return Qnil;
}

/* Test primitives.  */

DEFUN ("nidhogg-workers--test-ts-start",Fnidhogg_workers__test_ts_start,
       Snidhogg_workers__test_ts_start, 1, 1, 0,
       doc: /* Start a parse job for PARSER over a snapshot; for tests.
Return t if a job for the current text is now queued, running or done,
nil if none could start (the switch `nidhogg-workers-treesit' is off,
nothing to parse, a temporary narrowing...).  */)
  (Lisp_Object parser)
{
  nh_ts_check_parser (parser);
  return nh_ts_start (parser, false) ? Qt : Qnil;
}

DEFUN ("nidhogg-workers--test-ts-select-entry",
       Fnidhogg_workers__test_ts_select_entry,
       Snidhogg_workers__test_ts_select_entry, 0, 0, 0,
       doc: /* Run tree-sitter's select-entry start point; for tests.
What `wait_reading_process_output' does when it waits for keyboard
input, which a batch Emacs never does.  */)
  (void)
{
  nh_ts_select_entry ();
  return Qnil;
}

DEFUN ("nidhogg-workers--test-ts-wait", Fnidhogg_workers__test_ts_wait,
       Snidhogg_workers__test_ts_wait, 2, 2, 0,
       doc: /* Wait at most MS milliseconds for PARSER's job; for tests.
Return 1 when it is done, 0 at the deadline, -1 if it was cancelled, nil
if PARSER has no job.  */)
  (Lisp_Object parser, Lisp_Object ms)
{
  struct Lisp_TS_Parser *p = nh_ts_check_parser (parser);
  CHECK_FIXNAT (ms);
  struct nh_ts_job *t = p->nh_job;
  if (!t)
    return Qnil;
  return make_fixnum (nh_wk_wait (&t->job, XFIXNAT (ms) * INT64_C (1000000),
				  NH_WK_PH_TREESIT_WAIT, 0));
}

DEFUN ("nidhogg-workers--test-ts-info", Fnidhogg_workers__test_ts_info,
       Snidhogg_workers__test_ts_info, 1, 1, 0,
       doc: /* Return PARSER's worker state; for tests.
A plist: `:job' (nil, `queued', `running', `done' or `cancelled'),
`:gen' (the edit generation), `:visible' (the byte range its tree
sees, (BEG . END)), `:ranges' (its `last_set_ranges' itself, for `eq'),
`:log' (the edits logged since the job's snapshot, or nil) and `:policy'
\(the scheduling policy of the thread that ran the job: `other', `idle',
`batch', or nil if it has not run or the system does not say).  */)
  (Lisp_Object parser)
{
  static const char *const states[] = {
    "queued", "running", "done", "cancelled"
  };
  struct Lisp_TS_Parser *p = nh_ts_check_parser (parser);
  struct nh_ts_job *t = p->nh_job;
  Lisp_Object job = (t ? intern_c_string (states[atomic_load (&t->job.state)])
		     : Qnil);
  Lisp_Object policy = Qnil;
#ifdef __linux__
  if (t && t->policy == SCHED_OTHER)
    policy = intern_c_string ("other");
  else if (t && t->policy == SCHED_IDLE)
    policy = intern_c_string ("idle");
  else if (t && t->policy == SCHED_BATCH)
    policy = intern_c_string ("batch");
#endif
  return listn (12, intern_c_string (":policy"), policy,
		intern_c_string (":log"), t ? make_int (t->nlog) : Qnil,
		intern_c_string (":job"), job,
		intern_c_string (":gen"), make_int (p->nh_gen),
		intern_c_string (":visible"),
		Fcons (make_int (p->visible_beg), make_int (p->visible_end)),
		intern_c_string (":ranges"), p->last_set_ranges);
}

DEFUN ("nidhogg-workers--test-ts-stats", Fnidhogg_workers__test_ts_stats,
       Snidhogg_workers__test_ts_stats, 0, 0, 0,
       doc: /* Return the tree-sitter jobs' counters; for tests.
A plist: `:started', `:refused', `:published', `:discarded' (dropped at a
demand: not done, or ranges changed with no edit since the snapshot; or
at an edit the log cannot replay), `:stat-cached' (subtree statistics
answered from a publish), `:replayed' (publishes that replayed edits and
reparsed), `:replayed-edits' (the edits they replayed), `:superseded'
(jobs a start replaced with a newer snapshot), `:parses' (worker parses
begun; a job cancelled before its parse begins none), `:attempts' (the
demand's 1 ms attempts), `:cancels' (attempts cancelled and reset),
`:started-demand', `:started-create', `:started-select' (jobs started at
each start point), `:waited' (demands that waited for a job),
`:abandoned' (demands that cancelled a queued or running job: not
started, behind, or not done within the estimate) and `:live' (jobs not
yet freed).  */)
  (void)
{
  nh_wk_lock (&nh_ts_live_m);
  ptrdiff_t live = nh_ts_live;
  pthread_mutex_unlock (&nh_ts_live_m);
  return listn (34,
		intern_c_string (":started"),
		make_int (atomic_load (&nh_ts_count[NH_TS_C_STARTED])),
		intern_c_string (":refused"),
		make_int (atomic_load (&nh_ts_count[NH_TS_C_REFUSED])),
		intern_c_string (":published"),
		make_int (atomic_load (&nh_ts_count[NH_TS_C_PUBLISHED])),
		intern_c_string (":discarded"),
		make_int (atomic_load (&nh_ts_count[NH_TS_C_DISCARDED])),
		intern_c_string (":stat-cached"),
		make_int (atomic_load (&nh_ts_count[NH_TS_C_STAT_CACHED])),
		intern_c_string (":replayed"),
		make_int (atomic_load (&nh_ts_count[NH_TS_C_REPLAYED])),
		intern_c_string (":replayed-edits"),
		make_int (atomic_load (&nh_ts_count[NH_TS_C_REPLAYED_EDITS])),
		intern_c_string (":superseded"),
		make_int (atomic_load (&nh_ts_count[NH_TS_C_SUPERSEDED])),
		intern_c_string (":parses"),
		make_int (atomic_load (&nh_ts_count[NH_TS_C_PARSES])),
		intern_c_string (":attempts"),
		make_int (atomic_load (&nh_ts_count[NH_TS_C_ATTEMPTS])),
		intern_c_string (":cancels"),
		make_int (atomic_load (&nh_ts_count[NH_TS_C_CANCELS])),
		intern_c_string (":started-demand"),
		make_int (atomic_load (&nh_ts_count[NH_TS_C_START_DEMAND])),
		intern_c_string (":started-create"),
		make_int (atomic_load (&nh_ts_count[NH_TS_C_START_CREATE])),
		intern_c_string (":started-select"),
		make_int (atomic_load (&nh_ts_count[NH_TS_C_START_SELECT])),
		intern_c_string (":waited"),
		make_int (atomic_load (&nh_ts_count[NH_TS_C_WAITED])),
		intern_c_string (":abandoned"),
		make_int (atomic_load (&nh_ts_count[NH_TS_C_ABANDONED])),
		intern_c_string (":live"), make_int (live));
}

DEFUN ("nidhogg-workers--test-ts-await-idle",
       Fnidhogg_workers__test_ts_await_idle,
       Snidhogg_workers__test_ts_await_idle, 1, 1, 0,
       doc: /* Wait at most MS milliseconds until every tree-sitter job is freed; for tests.
Return t if none is left.  A bounded condition wait, not a sleep.  Jobs
started with `nidhogg-treesit-defer' on are freed when they land, so
this first lands nh-parse's finished jobs, as
`nidhogg-workers--test-await-landed' does.  */)
  (Lisp_Object ms)
{
  CHECK_FIXNAT (ms);
  pthread_once (&nh_ts_once, nh_ts_init_once);
  int64_t deadline = nh_ts_now () + XFIXNAT (ms) * INT64_C (1000000);
  Fnidhogg_workers__test_await_landed (intern_c_string ("nh-parse"), ms);
  nh_wk_lock (&nh_ts_live_m);
  while (nh_ts_live > 0 && nh_ts_now () < deadline)
    {
      nh_wk_cond_wait_until (&nh_ts_live_cv, &nh_ts_live_m, deadline);
    }
  bool idle = nh_ts_live == 0;
  pthread_mutex_unlock (&nh_ts_live_m);
  return idle ? Qt : Qnil;
}

#else /* NH_WK_HARNESS or no tree-sitter */

void
nh_ts_alloc_install (void)
{
}

void
nh_ts_parser_freed (void *parser_c)
{
  (void) parser_c;
}

void
nh_ts_buffer_killed (struct buffer *b)
{
  (void) b;
}

void
nh_ts_record_edit (void *parser_c, const void *ts_input_edit,
		   ptrdiff_t start_byte, ptrdiff_t old_end_byte,
		   ptrdiff_t new_end_byte)
{
  (void) parser_c;
  (void) ts_input_edit;
  (void) start_byte;
  (void) old_end_byte;
  (void) new_end_byte;
}

void
nh_ts_select_entry (void)
{
}

#endif /* NH_WK_HARNESS or no tree-sitter */

#ifndef NH_WK_HARNESS

#ifndef HAVE_TREE_SITTER

int
nh_ts_demand (Lisp_Object parser, bool may_defer, void **tree)
{
  (void) parser;
  (void) may_defer;
  (void) tree;
  return NH_TS_SYNC;
}

void
nh_ts_maybe_start (Lisp_Object parser, int start_point)
{
  (void) parser;
  (void) start_point;
}

Lisp_Object
nh_ts_subtree_stat_cached (Lisp_Object node)
{
  (void) node;
  return Qnil;
}

#endif /* not HAVE_TREE_SITTER */

void
nh_ts_redisplay_fontifying (bool redisplaying)
{
#ifdef HAVE_TREE_SITTER
  if (redisplaying && nh_ts_defer_on ())
    specbind (Qnidhogg__redisplay_fontifying, Fcurrent_buffer ());
#else
  (void) redisplaying;
#endif
}

void
nh_ts_syms (void)
{
  DEFVAR_LISP ("nidhogg--redisplay-fontifying", Vnidhogg__redisplay_fontifying,
	       doc: /* The buffer a redisplay is fontifying, or nil.
Redisplay binds it around `fontification-functions' while
`nidhogg-treesit-defer' is on.  */);
  Vnidhogg__redisplay_fontifying = Qnil;
  DEFSYM (Qnidhogg__redisplay_fontifying, "nidhogg--redisplay-fontifying");
#ifdef HAVE_TREE_SITTER
  DEFSYM (Qnidhogg_workers_treesit, "nidhogg-workers-treesit");
  DEFSYM (Qnidhogg_treesit_defer, "nidhogg-treesit-defer");
  DEFSYM (Qnidhogg_workers_treesit__land, "nidhogg-workers-treesit--land");
  defsubr (&Snidhogg_workers__treesit_await);
  defsubr (&Snidhogg_workers__treesit_busy_p);
  defsubr (&Snidhogg_workers__test_ts_alloc);
  defsubr (&Snidhogg_treesit_parser_ready_p);
  defsubr (&Snidhogg_workers__test_ts_start);
  defsubr (&Snidhogg_workers__test_ts_wait);
  defsubr (&Snidhogg_workers__test_ts_info);
  defsubr (&Snidhogg_workers__test_ts_stats);
  defsubr (&Snidhogg_workers__test_ts_await_idle);
  defsubr (&Snidhogg_workers__treesit_start_created);
  defsubr (&Snidhogg_workers__test_ts_select_entry);
#endif
}

#endif /* not NH_WK_HARNESS */

#endif /* HAVE_NIDHOGG_WORKERS */

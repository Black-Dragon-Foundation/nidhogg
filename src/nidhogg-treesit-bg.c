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
   and ownership.  Tasks 11 to 13 fill the rest (the edit log, the start
   points, deferral).  README.md, "Work off the main thread", is the
   user-facing description; the design's §7.3 to §7.5 the contract.

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
   at demand calls, and a node stays valid until the publish.  */

#ifndef NH_WK_HARNESS
# include <config.h>
#endif

#ifdef HAVE_NIDHOGG_WORKERS

#include "nidhogg-workers.h"

#if !defined NH_WK_HARNESS && defined HAVE_TREE_SITTER

# include <errno.h>
# include <signal.h>
# include <string.h>
# include <time.h>
# include "dynlib.h"
# include "buffer.h"
# include "coding.h"
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
      dynlib_close (h);
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
};

/* The counters of `nidhogg-workers--test-ts-stats'.  */
enum
{
  NH_TS_C_STARTED,
  NH_TS_C_REFUSED,
  NH_TS_C_PUBLISHED,
  NH_TS_C_DISCARDED,
  NH_TS_C_STAT_CACHED,
  NH_TS_C_N
};
static _Atomic int64_t nh_ts_count[NH_TS_C_N];

/* Jobs allocated and not yet freed; the condition variable is
   broadcast whenever one is freed.  */
static pthread_mutex_t nh_ts_live_m = PTHREAD_MUTEX_INITIALIZER;
static pthread_cond_t nh_ts_live_cv;
static ptrdiff_t nh_ts_live;
static pthread_once_t nh_ts_once = PTHREAD_ONCE_INIT;

static void
nh_ts_init_once (void)
{
  pthread_condattr_t attr;
  pthread_condattr_init (&attr);
  pthread_condattr_setclock (&attr, CLOCK_MONOTONIC);
  pthread_cond_init (&nh_ts_live_cv, &attr);
  pthread_condattr_destroy (&attr);
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

  atomic_store (&job->total, t->len);
  nh_wk_pause_point (NH_WK_P_AFTER_SNAPSHOT);
#if NH_TS_JOBS
  TSParser *p = (atomic_load (&job->cancel) ? NULL
		 : nh_ts_thread_parser (t->lang));
  if (p && ts_parser_set_included_ranges (p, t->ranges, t->nranges))
    {
      TSInput input = { .payload = t, .read = nh_ts_read,
			.encoding = TSInputEncodingUTF8 };
      TSParseOptions opts = { .payload = t,
			      .progress_callback = nh_ts_progress };
      tree = ts_parser_parse_with_options (p, t->old, input, opts);
      /* A cancelled parse is never resumed (§7.2).  */
      if (!tree)
	ts_parser_reset (p);
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
   touches the parser's visible range, ranges or tree.  */
static bool
nh_ts_start (Lisp_Object parser)
{
  struct Lisp_TS_Parser *p = XTS_PARSER (parser);
  struct buffer *b = XBUFFER (p->buffer);

  if (!NH_TS_JOBS || !nh_ts_switch_on () || p->deleted || !BUFFER_LIVE_P (b)
      || p->within_reparse || !p->need_reparse
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
  /* The latest snapshot wins (MC-6).  */
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
  t->gen = p->nh_gen;
  t->visible_beg = beg;
  t->visible_end = end;
  t->job.w = NH_WK_PARSE;
  t->job.speculative = true;
  t->job.run = nh_ts_job_run;
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

bool
nh_ts_publish (Lisp_Object parser, void **tree)
{
  struct Lisp_TS_Parser *p = XTS_PARSER (parser);
  struct nh_ts_job *t = p->nh_job;
  if (!t)
    return false;
  if (!nh_ts_job_done (t) || !nh_ts_job_current (p, t, XBUFFER (p->buffer)))
    {
      nh_ts_drop (p);
      nh_ts_counted (NH_TS_C_DISCARDED);
      return false;
    }
  *tree = t->result;
  t->result = NULL;
  memcpy (p->nh_stat, t->stat, sizeof p->nh_stat);
  /* The caller's tail increments the timestamp next.  */
  p->nh_stat_timestamp = p->timestamp + 1;
  p->nh_job = NULL;
  nh_wk_job_unref (&t->job);
  nh_ts_counted (NH_TS_C_PUBLISHED);
  return true;
}

void
nh_ts_maybe_start (Lisp_Object parser, int start_point)
{
  (void) start_point;
  nh_ts_start (parser);
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

/* Test primitives.  */

DEFUN ("nidhogg-workers--test-ts-start", Fnidhogg_workers__test_ts_start,
       Snidhogg_workers__test_ts_start, 1, 1, 0,
       doc: /* Start a parse job for PARSER over a snapshot; for tests.
Return t if a job for the current text is now queued, running or done,
nil if none could start (the switch `nidhogg-workers-treesit' is off,
nothing to parse, a temporary narrowing...).  */)
  (Lisp_Object parser)
{
  nh_ts_check_parser (parser);
  return nh_ts_start (parser) ? Qt : Qnil;
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
sees, (BEG . END)) and `:ranges' (its `last_set_ranges' itself, for `eq').  */)
  (Lisp_Object parser)
{
  static const char *const states[] = {
    "queued", "running", "done", "cancelled"
  };
  struct Lisp_TS_Parser *p = nh_ts_check_parser (parser);
  struct nh_ts_job *t = p->nh_job;
  Lisp_Object job = (t ? intern_c_string (states[atomic_load (&t->job.state)])
		     : Qnil);
  return listn (8, intern_c_string (":job"), job,
		intern_c_string (":gen"), make_int (p->nh_gen),
		intern_c_string (":visible"),
		Fcons (make_int (p->visible_beg), make_int (p->visible_end)),
		intern_c_string (":ranges"), p->last_set_ranges);
}

DEFUN ("nidhogg-workers--test-ts-stats", Fnidhogg_workers__test_ts_stats,
       Snidhogg_workers__test_ts_stats, 0, 0, 0,
       doc: /* Return the tree-sitter jobs' counters; for tests.
A plist: `:started', `:refused', `:published', `:discarded' (dropped at a
demand: stale, not done, or ranges changed), `:stat-cached' (subtree
statistics answered from a publish) and `:live' (jobs not yet freed).  */)
  (void)
{
  nh_wk_lock (&nh_ts_live_m);
  ptrdiff_t live = nh_ts_live;
  pthread_mutex_unlock (&nh_ts_live_m);
  return listn (12,
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
		intern_c_string (":live"), make_int (live));
}

DEFUN ("nidhogg-workers--test-ts-await-idle",
       Fnidhogg_workers__test_ts_await_idle,
       Snidhogg_workers__test_ts_await_idle, 1, 1, 0,
       doc: /* Wait at most MS milliseconds until every tree-sitter job is freed; for tests.
Return t if none is left.  A bounded condition wait, not a sleep.  */)
  (Lisp_Object ms)
{
  CHECK_FIXNAT (ms);
  pthread_once (&nh_ts_once, nh_ts_init_once);
  int64_t deadline = nh_ts_now () + XFIXNAT (ms) * INT64_C (1000000);
  nh_wk_lock (&nh_ts_live_m);
  while (nh_ts_live > 0 && nh_ts_now () < deadline)
    {
      struct timespec ts = { .tv_sec = deadline / 1000000000,
			     .tv_nsec = deadline % 1000000000 };
      pthread_cond_timedwait (&nh_ts_live_cv, &nh_ts_live_m, &ts);
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

#endif /* NH_WK_HARNESS or no tree-sitter */

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

#ifndef NH_WK_HARNESS

#ifndef HAVE_TREE_SITTER

bool
nh_ts_publish (Lisp_Object parser, void **tree)
{
  (void) parser;
  (void) tree;
  return false;
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
nh_ts_syms (void)
{
  DEFVAR_LISP ("nidhogg--redisplay-fontifying", Vnidhogg__redisplay_fontifying,
	       doc: /* The buffer a redisplay is fontifying, or nil.  */);
  Vnidhogg__redisplay_fontifying = Qnil;
#ifdef HAVE_TREE_SITTER
  DEFSYM (Qnidhogg_workers_treesit, "nidhogg-workers-treesit");
  defsubr (&Snidhogg_workers__test_ts_alloc);
  defsubr (&Snidhogg_treesit_parser_ready_p);
  defsubr (&Snidhogg_workers__test_ts_start);
  defsubr (&Snidhogg_workers__test_ts_wait);
  defsubr (&Snidhogg_workers__test_ts_info);
  defsubr (&Snidhogg_workers__test_ts_stats);
  defsubr (&Snidhogg_workers__test_ts_await_idle);
#endif
}

#endif /* not NH_WK_HARNESS */

#endif /* HAVE_NIDHOGG_WORKERS */

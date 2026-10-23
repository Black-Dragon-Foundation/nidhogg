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

/* Stubs (Task 2); Tasks 9 to 13 fill them.  */

#ifndef NH_WK_HARNESS
# include <config.h>
#endif

#ifdef HAVE_NIDHOGG_WORKERS

#include "nidhogg-workers.h"

#if !defined NH_WK_HARNESS && defined HAVE_TREE_SITTER

# include <signal.h>
# include "dynlib.h"
# include "coding.h"
# include "treesit.h"

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

#else /* NH_WK_HARNESS or no tree-sitter */

void
nh_ts_alloc_install (void)
{
}

#endif /* NH_WK_HARNESS or no tree-sitter */

void
nh_ts_parser_freed (void *parser_c)
{
  (void) parser_c;
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

#ifndef NH_WK_HARNESS

bool
nh_ts_publish (Lisp_Object parser)
{
  (void) parser;
  return false;
}

void
nh_ts_maybe_start (Lisp_Object parser, int start_point)
{
  (void) parser;
  (void) start_point;
}

void
nh_ts_syms (void)
{
  DEFVAR_LISP ("nidhogg--redisplay-fontifying", Vnidhogg__redisplay_fontifying,
	       doc: /* The buffer a redisplay is fontifying, or nil.  */);
  Vnidhogg__redisplay_fontifying = Qnil;
#ifdef HAVE_TREE_SITTER
  defsubr (&Snidhogg_workers__test_ts_alloc);
#endif
}

#endif /* not NH_WK_HARNESS */

#endif /* HAVE_NIDHOGG_WORKERS */

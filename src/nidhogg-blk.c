/* Nidhogg's worker threads: directory reads, file blocks, auto-save, DNS.

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

/* Stubs (Task 2); Tasks 32 to 35 fill them.  */

#ifndef NH_WK_HARNESS
# include <config.h>
#endif

#ifdef HAVE_NIDHOGG_WORKERS

#include "nidhogg-workers.h"

#ifndef NH_WK_HARNESS

/* ===== Directory reads (Task 32) =====================================

   `nidhogg-directory-files-async' reads a directory on an nh-blk thread
   (class `dir') into plain C data: for every entry its name, the fields
   `file-attributes' reports (lstat, and readlink for a symbolic link;
   user and group names through getpwuid_r and getgrgid_r with a
   per-job cache), and, for sorted results, the entries of each
   directory ordered by bytes.  The main thread builds the Lisp objects
   (a copy of dired.c's builder, `file_attributes'): it decodes the
   names with `file-name-coding-system', applies MATCH, and passes
   batches of (NAME . ATTRIBUTES) to the callback from the drainer.

   Names in recursive results are relative to the directory; the
   entries `.' and `..' are listed only for the directory itself.  A
   subdirectory that cannot be read is skipped.  The entry cap counts
   entries read, before MATCH.  A directory whose base name is in SKIP
   is listed but not descended into.  Order: sorted results are each
   directory's entries in `file-attributes-lessp' order, directories in
   breadth-first order; NOSORT results come in the reverse of the
   order `readdir' gave, as `directory-files-and-attributes' returns
   them (recursive ones reverse the whole breadth-first sequence).  */

#include <dirent.h>
#include <errno.h>
#include <fcntl.h>
#include <grp.h>
#include <pwd.h>
#include <string.h>
#include <sys/resource.h>
#include <unistd.h>
#include <sys/stat.h>
#include <filemode.h>
#include <stat-time.h>

#include "coding.h"
#include "systime.h"

enum { NH_DIR_BATCH = 1000 };

/* Failed operations, for nh_dir_job.err_op.  */
enum { NH_DIR_E_OPEN = 1, NH_DIR_E_READ, NH_DIR_E_ATTR, NH_DIR_E_LINK };

struct nh_dir_idc
{
  uint64_t id;
  char *name;			/* NULL: no such id */
};

struct nh_dir_idcache
{
  struct nh_dir_idc *v;
  size_t n, cap;
};

struct nh_dir_ent
{
  char *name;			/* relative to the directory read */
  uint32_t base;		/* offset of the last component */
  uint32_t group;		/* the directory it was read from */
  unsigned char kind;		/* 0 other, 1 directory, 2 symbolic link */
  char *link;
  nlink_t nlink;
  uid_t uid;
  gid_t gid;
  const char *uname, *gname;	/* in the job's caches; NULL: use the id */
  struct timespec at, mt, ct;
  off_t size;
  char modes[12];
  ino_t ino;
  dev_t dev;
};

struct nh_dir_job
{
  struct nh_wk_job job;
  /* Inputs (copies).  */
  char *root;
  bool nosort, recursive, id_string, sync;
  ptrdiff_t max_depth, max_entries;	/* -1: none */
  char **skip;
  size_t nskip;
  /* Worker output.  */
  struct nh_dir_ent *ents;
  size_t n, cap;
  bool all_utf8;
  int err_op, err;
  char *err_path;
  struct nh_dir_idcache users, groups;
  /* Main thread only.  */
  size_t pos;			/* NOSORT: entries not yet delivered */
  bool finished;
  Lisp_Object holder;		/* (CALLBACK MATCH DIRFILENAME), rooted */
};

static Lisp_Object nh_dir_roots;

static void *
nh_dir_alloc (size_t n)
{
  void *p = malloc (n ? n : 1);
  NH_WK_ASSERT (p);
  return p;
}

static char *
nh_dir_strdup (const char *s)
{
  size_t n = strlen (s) + 1;
  return memcpy (nh_dir_alloc (n), s, n);
}

/* Is S, a NUL-terminated name, strict UTF-8?  */
static bool
nh_dir_utf8 (const unsigned char *s)
{
  while (*s)
    {
      unsigned c = *s, cp, min;
      int n;
      if (c < 0x80)
	{
	  s++;
	  continue;
	}
      if (c >= 0xc2 && c <= 0xdf)
	n = 1, cp = c & 0x1f, min = 0x80;
      else if ((c & 0xf0) == 0xe0)
	n = 2, cp = c & 0x0f, min = 0x800;
      else if (c >= 0xf0 && c <= 0xf4)
	n = 3, cp = c & 0x07, min = 0x10000;
      else
	return false;
      for (int k = 1; k <= n; k++)
	{
	  if ((s[k] & 0xc0) != 0x80)
	    return false;
	  cp = (cp << 6) | (s[k] & 0x3f);
	}
      if (cp < min || cp > 0x10ffff || (cp >= 0xd800 && cp <= 0xdfff))
	return false;
      s += n + 1;
    }
  return true;
}

/* The name of user or group ID, NULL if it has none.  */
static const char *
nh_dir_idname (struct nh_dir_idcache *c, bool group, uint64_t id)
{
  for (size_t i = 0; i < c->n; i++)
    if (c->v[i].id == id)
      return c->v[i].name;

  char *name = NULL, *buf = NULL;
  size_t size = 1024;
  for (;;)
    {
      buf = realloc (buf, size);
      NH_WK_ASSERT (buf);
      int rc;
      if (group)
	{
	  struct group g, *r = NULL;
	  rc = getgrgid_r (id, &g, buf, size, &r);
	  if (rc == 0 && r)
	    name = nh_dir_strdup (r->gr_name);
	}
      else
	{
	  struct passwd p, *r = NULL;
	  rc = getpwuid_r (id, &p, buf, size, &r);
	  if (rc == 0 && r)
	    name = nh_dir_strdup (r->pw_name);
	}
      if (rc == ERANGE && size < (1 << 20))
	{
	  size *= 4;
	  continue;
	}
      break;
    }
  free (buf);

  if (c->n == c->cap)
    {
      c->cap = c->cap ? 2 * c->cap : 16;
      c->v = realloc (c->v, c->cap * sizeof *c->v);
      NH_WK_ASSERT (c->v);
    }
  c->v[c->n++] = (struct nh_dir_idc) { id, name };
  return name;
}

static void
nh_dir_idcache_free (struct nh_dir_idcache *c)
{
  for (size_t i = 0; i < c->n; i++)
    free (c->v[i].name);
  free (c->v);
}

static void
nh_dir_fail (struct nh_dir_job *j, int op, int err, const char *path)
{
  j->err_op = op;
  j->err = err;
  free (j->err_path);
  j->err_path = nh_dir_strdup (path);
}

static struct nh_dir_ent *
nh_dir_push (struct nh_dir_job *j)
{
  if (j->n == j->cap)
    {
      j->cap = j->cap ? 2 * j->cap : 256;
      j->ents = realloc (j->ents, j->cap * sizeof *j->ents);
      NH_WK_ASSERT (j->ents);
    }
  struct nh_dir_ent *e = &j->ents[j->n++];
  memset (e, 0, sizeof *e);
  return e;
}

static int
nh_dir_cmp (const void *a, const void *b)
{
  const struct nh_dir_ent *x = a, *y = b;
  return strcmp (x->name + x->base, y->name + y->base);
}

struct nh_dir_todo
{
  char *rel;
  ptrdiff_t depth;
};

/* Read the directory REL (relative to the root; NULL for the root) as
   GROUP.  Return false after an error that ends the job.  Subdirectories
   to read go onto *Q.  */
static bool
nh_dir_one (struct nh_dir_job *j, const char *rel, ptrdiff_t depth,
	    uint32_t group, struct nh_dir_todo **q, size_t *nq, size_t *capq,
	    bool *capped)
{
  size_t rl = rel ? strlen (rel) : 0;
  char *path = nh_dir_alloc (strlen (j->root) + 1 + rl + 1);
  strcpy (path, j->root);
  if (rel)
    {
      strcat (path, "/");
      strcat (path, rel);
    }
  bool top = group == 0;
  int fd = -1;
  DIR *d = NULL;
  int err = 0;

  if (!nh_wk_fd_budget_ok (1))
    err = EMFILE;
  else
    {
      fd = open (path, O_RDONLY | O_DIRECTORY | O_CLOEXEC);
      if (fd < 0)
	err = errno;
      else if (!(d = fdopendir (fd)))
	{
	  err = errno;
	  close (fd);
	}
    }
  if (!d)
    {
      if (top)
	nh_dir_fail (j, NH_DIR_E_OPEN, err, path);
      free (path);
      return !top;
    }
  free (path);

  size_t first = j->n;
  bool ok = true;
  for (;;)
    {
      errno = 0;
      struct dirent *dp = readdir (d);
      if (!dp)
	{
	  if (errno == EAGAIN || errno == EINTR)
	    continue;
	  if (errno)
	    {
	      if (top)
		nh_dir_fail (j, NH_DIR_E_READ, errno, j->root);
	      ok = !top;
	    }
	  break;
	}
      const char *nm = dp->d_name;
      bool dots = strcmp (nm, ".") == 0 || strcmp (nm, "..") == 0;
      if (dots && !top)
	continue;
      struct stat s;
      if (fstatat (fd, nm, &s, AT_SYMLINK_NOFOLLOW) != 0)
	{
	  /* Gone, or in a subdirectory (one that can be listed but not
	     searched is unreadable, and unreadable subdirectories are
	     skipped): no entry.  */
	  if (errno == ENOENT || errno == ENOTDIR || !top)
	    continue;
	  nh_dir_fail (j, NH_DIR_E_ATTR, errno, nm);
	  ok = false;
	  break;
	}
      char *link = NULL;
      if (S_ISLNK (s.st_mode))
	{
	  size_t size = 256;
	  for (;;)
	    {
	      link = realloc (link, size);
	      NH_WK_ASSERT (link);
	      ssize_t r = readlinkat (fd, nm, link, size);
	      if (r < 0)
		{
		  int e = errno;
		  free (link);
		  link = NULL;
		  if (top && e != EINVAL && e != ENOENT && e != ENOTDIR)
		    {
		      nh_dir_fail (j, NH_DIR_E_LINK, e, nm);
		      ok = false;
		    }
		  break;
		}
	      if ((size_t) r < size)
		{
		  link[r] = 0;
		  break;
		}
	      size *= 2;
	    }
	  if (!link)
	    {
	      if (!ok)
		break;
	      continue;		/* it vanished: no entry */
	    }
	}

      size_t nl = strlen (nm);
      struct nh_dir_ent *e = nh_dir_push (j);
      e->base = top ? 0 : rl + 1;
      e->name = nh_dir_alloc (e->base + nl + 1);
      if (!top)
	{
	  memcpy (e->name, rel, rl);
	  e->name[rl] = '/';
	}
      memcpy (e->name + e->base, nm, nl + 1);
      e->group = group;
      e->kind = S_ISLNK (s.st_mode) ? 2 : S_ISDIR (s.st_mode);
      e->link = link;
      e->nlink = s.st_nlink;
      e->uid = s.st_uid;
      e->gid = s.st_gid;
      if (j->id_string)
	{
	  e->uname = nh_dir_idname (&j->users, false, s.st_uid);
	  e->gname = nh_dir_idname (&j->groups, true, s.st_gid);
	}
      e->at = get_stat_atime (&s);
      e->mt = get_stat_mtime (&s);
      e->ct = get_stat_ctime (&s);
      e->size = s.st_size;
      filemodestring (&s, e->modes);
      e->ino = s.st_ino;
      e->dev = s.st_dev;
      if (!nh_dir_utf8 ((const unsigned char *) nm))
	j->all_utf8 = false;

      if (j->max_entries >= 0 && (ptrdiff_t) j->n >= j->max_entries)
	{
	  *capped = true;
	  break;
	}
    }
  closedir (d);
  if (!j->nosort && j->n > first)
    qsort (j->ents + first, j->n - first, sizeof *j->ents, nh_dir_cmp);

  /* Subdirectories queue in the order the entries are delivered (sorted,
     or readdir's), so the walk is breadth first in that order.  */
  if (j->recursive && (j->max_depth < 0 || depth + 1 <= j->max_depth))
    for (size_t k = first; k < j->n; k++)
      {
	struct nh_dir_ent *e = &j->ents[k];
	const char *nm = e->name + e->base;
	if (e->kind != 1 || strcmp (nm, ".") == 0 || strcmp (nm, "..") == 0)
	  continue;
	bool skipped = false;
	for (size_t i = 0; i < j->nskip; i++)
	  if (strcmp (j->skip[i], nm) == 0)
	    skipped = true;
	if (skipped)
	  continue;
	if (*nq == *capq)
	  {
	    *capq = *capq ? 2 * *capq : 16;
	    *q = realloc (*q, *capq * sizeof **q);
	    NH_WK_ASSERT (*q);
	  }
	(*q)[(*nq)++] = (struct nh_dir_todo) { nh_dir_strdup (e->name),
					       depth + 1 };
      }
  return ok;
}

static void
nh_dir_run (struct nh_wk_job *wj)
{
  struct nh_dir_job *j = (struct nh_dir_job *) wj;
  struct nh_dir_todo *q = NULL;
  size_t nq = 0, capq = 0, head = 0;
  bool capped = false;
  uint32_t group = 0;

  j->all_utf8 = true;
  if (j->max_entries == 0)
    capped = true;
  else if (nh_dir_one (j, NULL, 0, group++, &q, &nq, &capq, &capped))
    while (!capped && head < nq)
      {
	if (atomic_load (&wj->cancel))
	  break;
	nh_dir_one (j, q[head].rel, q[head].depth, group++, &q, &nq, &capq,
		    &capped);
	head++;
      }
  for (size_t i = 0; i < nq; i++)
    free (q[i].rel);
  free (q);
  j->pos = j->n;
  nh_wk_pause_point (NH_WK_P_BEFORE_DONE);
}

static void
nh_dir_free (struct nh_wk_job *wj)
{
  struct nh_dir_job *j = (struct nh_dir_job *) wj;
  for (size_t i = 0; i < j->n; i++)
    {
      free (j->ents[i].name);
      free (j->ents[i].link);
    }
  free (j->ents);
  for (size_t i = 0; i < j->nskip; i++)
    free (j->skip[i]);
  free (j->skip);
  free (j->root);
  free (j->err_path);
  nh_dir_idcache_free (&j->users);
  nh_dir_idcache_free (&j->groups);
  free (j);
}

/* Main thread: Lisp objects.  */

static Lisp_Object
nh_dir_attrs (struct nh_dir_ent *e)
{
  Lisp_Object type = (e->kind == 1 ? Qt
		      : e->kind == 2
		      ? DECODE_FILE (build_unibyte_string (e->link))
		      : Qnil);
  return CALLN (Flist,
		type,
		make_fixnum (e->nlink),
		(e->uname
		 ? DECODE_SYSTEM (build_unibyte_string (e->uname))
		 : INT_TO_INTEGER (e->uid)),
		(e->gname
		 ? DECODE_SYSTEM (build_unibyte_string (e->gname))
		 : INT_TO_INTEGER (e->gid)),
		make_lisp_time (e->at),
		make_lisp_time (e->mt),
		make_lisp_time (e->ct),
		INT_TO_INTEGER (sizeof (e->size) == 4
				? e->size & 0xffffffffu : e->size),
		make_string (e->modes, 10),
		Qt,
		INT_TO_INTEGER (e->ino),
		INT_TO_INTEGER (e->dev));
}

/* (NAME . ATTRIBUTES) for E, or nil if MATCH rejects its name.  */
static Lisp_Object
nh_dir_entry (struct nh_dir_job *j, struct nh_dir_ent *e)
{
  Lisp_Object match = XCAR (XCDR (j->holder));
  const char *b = e->name + e->base;
  Lisp_Object base = DECODE_FILE (make_unibyte_string (b, strlen (b)));
  if (!NILP (match) && fast_string_match_internal (match, base, Qnil) < 0)
    return Qnil;
  Lisp_Object name = base;
  if (e->base)
    name = DECODE_FILE (make_unibyte_string (e->name, strlen (e->name)));
  return Fcons (name, nh_dir_attrs (e));
}

static Lisp_Object
nh_dir_report (Lisp_Object arg)
{
  int op = XFIXNUM (XCAR (arg));
  Lisp_Object path = XCAR (XCDR (arg));
  int err = XFIXNUM (XCAR (XCDR (XCDR (arg))));
  report_file_errno (op == NH_DIR_E_OPEN ? "Opening directory"
		     : op == NH_DIR_E_READ ? "Reading directory"
		     : op == NH_DIR_E_ATTR ? "Getting attributes"
		     : "Reading symbolic link",
		     path, err);
}

/* The error `directory-files' would have signalled.  */
static Lisp_Object
nh_dir_error (struct nh_dir_job *j)
{
  Lisp_Object path = XCAR (XCDR (XCDR (j->holder)));
  if (j->err_op != NH_DIR_E_OPEN && j->err_op != NH_DIR_E_READ)
    path = DECODE_FILE (build_unibyte_string (j->err_path));
  return internal_condition_case_1
    (nh_dir_report,
     list3 (make_fixnum (j->err_op), path, make_fixnum (j->err)),
     Qerror, Fidentity);
}

/* Is a byte sort the order `file-attributes-lessp' gives these names?  */
static bool
nh_dir_byte_order_ok (struct nh_dir_job *j)
{
  if (!j->all_utf8)
    return false;
  Lisp_Object c = Vfile_name_coding_system;
  if (NILP (c))
    c = Vdefault_file_name_coding_system;
  return (SYMBOLP (c) && !NILP (c)
	  && EQ (calln (intern_c_string ("coding-system-base"), c), Qutf_8));
}

static void
nh_dir_finish (struct nh_wk_job *wj)
{
  struct nh_dir_job *j = (struct nh_dir_job *) wj;
  Lisp_Object holder = j->holder, cb = XCAR (holder);
  Lisp_Object batch = Qnil, error = Qnil;
  bool done = true;

  if (j->err)
    error = nh_dir_error (j);
  else if (!j->nosort)
    {
      bool lisp_sort = !nh_dir_byte_order_ok (j);
      Lisp_Object acc = Qnil;
      for (size_t i = 0; i < j->n; )
	{
	  uint32_t g = j->ents[i].group;
	  Lisp_Object list = Qnil;
	  for (; i < j->n && j->ents[i].group == g; i++)
	    {
	      Lisp_Object x = nh_dir_entry (j, &j->ents[i]);
	      if (!NILP (x))
		list = Fcons (x, list);
	    }
	  list = Fnreverse (list);
	  if (lisp_sort)
	    list = CALLN (Fsort, list, Qfile_attributes_lessp);
	  for (; CONSP (list); list = XCDR (list))
	    acc = Fcons (XCAR (list), acc);
	}
      batch = Fnreverse (acc);
    }
  else
    {
      size_t scanned = 0;
      while (j->pos > 0 && scanned < NH_DIR_BATCH)
	{
	  Lisp_Object x = nh_dir_entry (j, &j->ents[--j->pos]);
	  scanned++;
	  if (!NILP (x))
	    batch = Fcons (x, batch);
	}
      batch = Fnreverse (batch);
      done = j->pos == 0;
      if (!done && !j->sync)
	{
	  /* The next batch lands behind whatever else has landed.  */
	  nh_wk_job_ref (wj);
	  nh_wk_complete (wj);
	}
    }
  if (done)
    {
      j->finished = true;
      nh_dir_roots = Fdelq (holder, nh_dir_roots);
    }
  if (NILP (batch) && !done && NILP (error))
    return;
  calln (cb, batch, done ? Qt : Qnil, error);
}

static void
nh_dir_unwind_sync (void *p)
{
  nh_wk_job_unref (p);
}

DEFUN ("nidhogg-directory-files-async", Fnidhogg_directory_files_async,
       Snidhogg_directory_files_async, 2, MANY, 0,
       doc: /* Read directory DIR on a worker thread and call CALLBACK with the entries.
CALLBACK is called as (CALLBACK BATCH DONE ERROR) from the main loop, in
order.  BATCH is a list of (NAME . ATTRIBUTES), as
`directory-files-and-attributes' returns them; DONE is t on the last
call; ERROR is nil, or, with BATCH nil, the error `directory-files'
would have signalled.
Keywords:
:match REGEXP     only names (the last component) matching REGEXP.
:nosort           deliver in batches, in the order of
                  `directory-files-and-attributes' with NOSORT;
                  otherwise one sorted batch.
:id-format        as for `file-attributes'.
:recursive        read subdirectories too, breadth first; names are
                  relative to DIR; unreadable subdirectories are skipped.
:max-depth N      how many levels of subdirectories to read.
:max-entries N    stop after N entries have been read.
:skip NAMES       list of directory names to list but not descend into.
Signal `nidhogg-not-local' for a name that has a file name handler.
usage: (nidhogg-directory-files-async DIR CALLBACK &key match nosort id-format recursive max-depth max-entries skip)  */)
  (ptrdiff_t nargs, Lisp_Object *args)
{
  Lisp_Object dir = Fexpand_file_name (args[0], Qnil);
  Lisp_Object callback = args[1];
  Lisp_Object match = Qnil, id_format = Qnil, skip = Qnil;
  Lisp_Object max_depth = Qnil, max_entries = Qnil;
  bool nosort = false, recursive = false;

  for (ptrdiff_t i = 2; i < nargs; i += 2)
    {
      const char *k = SYMBOLP (args[i]) ? SSDATA (SYMBOL_NAME (args[i])) : "";
      Lisp_Object v = i + 1 < nargs ? args[i + 1] : Qnil;
      if (strcmp (k, ":match") == 0)
	match = v;
      else if (strcmp (k, ":nosort") == 0)
	nosort = !NILP (v);
      else if (strcmp (k, ":id-format") == 0)
	id_format = v;
      else if (strcmp (k, ":recursive") == 0)
	recursive = !NILP (v);
      else if (strcmp (k, ":max-depth") == 0)
	max_depth = v;
      else if (strcmp (k, ":max-entries") == 0)
	max_entries = v;
      else if (strcmp (k, ":skip") == 0)
	skip = v;
      else
	xsignal2 (Qerror, build_string ("Unknown keyword"), args[i]);
    }
  if (!NILP (match))
    CHECK_STRING (match);
  if (!NILP (max_depth))
    CHECK_FIXNAT (max_depth);
  if (!NILP (max_entries))
    CHECK_FIXNAT (max_entries);
  CHECK_LIST (skip);

  if (!NILP (Ffind_file_name_handler (dir, Qdirectory_files_and_attributes)))
    xsignal1 (Qnidhogg_not_local, dir);

  Lisp_Object dirfilename = Fdirectory_file_name (dir);
  Lisp_Object encoded = ENCODE_FILE (dirfilename);

  struct nh_dir_job *j = nh_dir_alloc (sizeof *j);
  memset (j, 0, sizeof *j);
  j->job.w = NH_WK_BLK;
  j->job.cls = NH_WK_CL_DIR;
  j->job.run = nh_dir_run;
  j->job.finish = nh_dir_finish;
  j->job.free = nh_dir_free;
  atomic_store (&j->job.refs, 1);
  j->root = nh_dir_strdup (SSDATA (encoded));
  j->nosort = nosort;
  j->recursive = recursive;
  j->id_string = !(NILP (id_format) || EQ (id_format, Qinteger));
  j->max_depth = NILP (max_depth) ? -1 : XFIXNAT (max_depth);
  j->max_entries = NILP (max_entries) ? -1 : XFIXNAT (max_entries);
  for (Lisp_Object s = skip; CONSP (s); s = XCDR (s))
    {
      CHECK_STRING (XCAR (s));
      j->skip = realloc (j->skip, (j->nskip + 1) * sizeof *j->skip);
      NH_WK_ASSERT (j->skip);
      j->skip[j->nskip++] = nh_dir_strdup (SSDATA (ENCODE_FILE (XCAR (s))));
    }
  j->holder = list3 (callback, match, dirfilename);
  nh_dir_roots = Fcons (j->holder, nh_dir_roots);

  if (nh_wk_submit (&j->job))
    {
      nh_wk_job_unref (&j->job);
      return Qt;
    }
  /* Today's path: read here and deliver everything now.  */
  j->sync = true;
  specpdl_ref count = SPECPDL_INDEX ();
  record_unwind_protect_ptr (nh_dir_unwind_sync, j);
  nh_dir_run (&j->job);
  while (!j->finished)
    nh_dir_finish (&j->job);
  return unbind_to (count, Qt);
}

static void
nh_dir_syms (void)
{
  DEFSYM (Qnidhogg_not_local, "nidhogg-not-local");
  Fput (Qnidhogg_not_local, Qerror_conditions,
	list2 (Qnidhogg_not_local, Qerror));
  Fput (Qnidhogg_not_local, Qerror_message,
	build_string ("Not a local file name"));
  nh_dir_roots = Qnil;
  staticpro (&nh_dir_roots);
  defsubr (&Snidhogg_directory_files_async);
}

/* ===== End of directory reads ======================================= */

static void nh_save_syms (void);
static void nh_dns_syms (void);

void
nh_blk_syms (void)
{
  nh_dir_syms ();
  nh_save_syms ();
  nh_dns_syms ();
}

#endif /* not NH_WK_HARNESS */


#ifndef NH_WK_HARNESS

/* ==== File blocks (Task 33): nidhogg-read-file-async and
   nidhogg-insert-file-block, on nh-blk class `file' ===================

   A worker reads the byte range of a regular file and scans it
   (nh_dec_scan); the main thread gets a `user-ptr' block that owns the
   bytes.  nidhogg-insert-file-block chooses the coding on the main
   thread exactly as insert-file-contents does, and inserts the bytes
   either straight from the worker's counts (fast path) or through
   decode_coding_gap.  */

#include <errno.h>
#include <fcntl.h>
#include <string.h>
#include <sys/stat.h>
#include <unistd.h>

#include "buffer.h"
#include "coding.h"
#include "composite.h"
#include "region-cache.h"

struct nh_blk_block
{
  unsigned char *bytes;
  size_t n;
  struct nh_dec_scan scan;
  bool regular, has_beg, has_end;
  off_t beg, end;
  char *name;			/* the expanded name, as Lisp bytes */
  ptrdiff_t name_nchars, name_nbytes;
  bool name_multibyte;
};

static void
nh_blk_block_free (void *p)
{
  struct nh_blk_block *b = p;
  nh_wk_stat_add (NH_WK_BLK, NH_WK_ST_QUEUED_BYTES, -(int64_t) b->n);
  free (b->bytes);
  free (b->name);
  free (b);
}

struct nh_blk_file_job
{
  struct nh_wk_job job;
  char *path;			/* encoded */
  struct nh_blk_block *blk;	/* result */
  EMACS_INT cb_id;
  int err;			/* errno, or 0 */
  const char *err_op;		/* its operation, or a file-error message */
};

static void
nh_blk_file_fail (struct nh_blk_file_job *j, const char *op, int err)
{
  j->err_op = op;
  j->err = err;
}

static void
nh_blk_file_run (struct nh_wk_job *job)
{
  struct nh_blk_file_job *j = (struct nh_blk_file_job *) job;
  struct nh_blk_block *b = j->blk;
  int fd = open (j->path, O_RDONLY | O_CLOEXEC | O_NONBLOCK);
  if (fd < 0)
    {
      nh_blk_file_fail (j, "Opening input file", errno);
      return;
    }
  struct stat st;
  if (fstat (fd, &st) < 0)
    {
      nh_blk_file_fail (j, "Input file status", errno);
      close (fd);
      return;
    }
  if (S_ISDIR (st.st_mode))
    {
      nh_blk_file_fail (j, "Read error", EISDIR);
      close (fd);
      return;
    }
  if (!S_ISREG (st.st_mode))
    {
      nh_blk_file_fail (j, "not a regular file", 0);
      close (fd);
      return;
    }
  b->regular = true;
  off_t beg = b->has_beg ? b->beg : 0;
  off_t end = b->has_end ? b->end : INT64_MAX;
  if (end < beg)
    end = beg;
  off_t likely = st.st_size < end ? st.st_size : end;
  size_t cap = (likely > beg ? likely - beg : 0) + 1, n = 0;
  unsigned char *buf = malloc (cap);
  if (!buf)
    {
      nh_blk_file_fail (j, "Read error", ENOMEM);
      close (fd);
      return;
    }
  off_t pos = beg;
  while (pos < end && !atomic_load (&job->cancel))
    {
      if (cap - n < 2)
	{
	  size_t ncap = cap + (cap >> 1) + 1024 * 1024;
	  unsigned char *nb = realloc (buf, ncap);
	  if (!nb)
	    {
	      nh_blk_file_fail (j, "Read error", ENOMEM);
	      break;
	    }
	  buf = nb;
	  cap = ncap;
	}
      size_t want = cap - n;
      if (want > 1024 * 1024)
	want = 1024 * 1024;
      if ((off_t) want > end - pos)
	want = end - pos;
      ssize_t r = pread (fd, buf + n, want, pos);
      if (r < 0)
	{
	  if (errno == EINTR)
	    continue;
	  nh_blk_file_fail (j, "Read error", errno);
	  break;
	}
      if (r == 0)
	break;
      n += r;
      pos += r;
    }
  close (fd);
  if (j->err_op)
    {
      free (buf);
      return;
    }
  nh_wk_pause_point (NH_WK_P_AFTER_READ);
  b->bytes = buf;
  b->n = n;
  nh_wk_stat_add (NH_WK_BLK, NH_WK_ST_QUEUED_BYTES, (int64_t) n);
  nh_dec_scan (buf, n, &b->scan);
}

static void
nh_blk_file_free (struct nh_wk_job *job)
{
  struct nh_blk_file_job *j = (struct nh_blk_file_job *) job;
  if (j->blk)
    nh_blk_block_free (j->blk);
  free (j->path);
  free (j);
}

/* Pending callbacks, (ID . CALLBACK), so the GC sees them.  */
static Lisp_Object Vnh_blk_callbacks;
static EMACS_INT nh_blk_next_id;
static Lisp_Object Qnh_blk_QCbom;
static Lisp_Object Qnh_blk_work_name;

static Lisp_Object
nh_blk_name_string (struct nh_blk_block *b)
{
  return make_specified_string (b->name, b->name_nchars, b->name_nbytes,
				b->name_multibyte);
}

static void
nh_blk_file_finish (struct nh_wk_job *job)
{
  struct nh_blk_file_job *j = (struct nh_blk_file_job *) job;
  Lisp_Object cb = Qnil, tail, prev = Qnil;
  for (tail = Vnh_blk_callbacks; CONSP (tail); prev = tail, tail = XCDR (tail))
    if (XFIXNUM (XCAR (XCAR (tail))) == j->cb_id)
      {
	cb = XCDR (XCAR (tail));
	if (NILP (prev))
	  Vnh_blk_callbacks = XCDR (tail);
	else
	  XSETCDR (prev, XCDR (tail));
	break;
      }
  if (NILP (cb))
    return;
  Lisp_Object block = Qnil, err = Qnil;
  if (j->err_op)
    {
      Lisp_Object name = nh_blk_name_string (j->blk);
      if (j->err)
	err = get_file_errno_data (j->err_op, name, j->err);
      else
	err = Fcons (Qfile_error, list2 (build_string (j->err_op), name));
    }
  else
    {
      block = make_user_ptr (nh_blk_block_free, j->blk);
      j->blk = NULL;
    }
  calln (cb, block, err);
}

DEFUN ("nidhogg-read-file-async", Fnidhogg_read_file_async,
       Snidhogg_read_file_async, 2, MANY, 0,
       doc: /* Read FILE on a worker thread; call CALLBACK with a file block.
CALLBACK is called as (CALLBACK BLOCK ERROR) at a timer check: BLOCK is
a file block for `nidhogg-insert-file-block', or nil with ERROR the error
data `insert-file-contents' would have signaled (the file must be a
regular file).  Keywords: `:beg' and `:end', byte offsets of the range
to read.  Return an integer handle, or nil when the read was not
started (the workers are off, dumping or out of file descriptors); the
caller then reads synchronously.  A remote FILE signals
`nidhogg-not-local'.
usage: (nidhogg-read-file-async FILE CALLBACK &key BEG END)  */)
  (ptrdiff_t nargs, Lisp_Object *args)
{
  Lisp_Object file = args[0], cb = args[1], beg = Qnil, end = Qnil;
  for (ptrdiff_t i = 2; i + 1 < nargs; i += 2)
    {
      const char *k = SYMBOLP (args[i]) ? SSDATA (SYMBOL_NAME (args[i])) : "";
      if (strcmp (k, ":beg") == 0)
	beg = args[i + 1];
      else if (strcmp (k, ":end") == 0)
	end = args[i + 1];
      else
	xsignal2 (Qerror, build_string ("Unknown property"), args[i]);
    }
  CHECK_STRING (file);
  if (!NILP (beg))
    CHECK_FIXNAT (beg);
  if (!NILP (end))
    CHECK_FIXNAT (end);
  file = Fexpand_file_name (file, Qnil);
  if (!NILP (Ffind_file_name_handler (file, Qinsert_file_contents)))
    xsignal1 (Qnidhogg_not_local, file);

  Lisp_Object enc = ENCODE_FILE (file);
  struct nh_blk_file_job *j = calloc (1, sizeof *j);
  struct nh_blk_block *b = calloc (1, sizeof *b);
  if (!j || !b)
    {
      free (j);
      free (b);
      memory_full (sizeof *j);
    }
  j->blk = b;
  j->path = strdup (SSDATA (enc));
  b->name = malloc (SBYTES (file) + 1);
  if (!j->path || !b->name)
    {
      nh_blk_file_free (&j->job);
      memory_full (SBYTES (file));
    }
  memcpy (b->name, SSDATA (file), SBYTES (file));
  b->name_nchars = SCHARS (file);
  b->name_nbytes = SBYTES (file);
  b->name_multibyte = STRING_MULTIBYTE (file);
  if (!NILP (beg))
    {
      b->has_beg = true;
      b->beg = XFIXNAT (beg);
    }
  if (!NILP (end))
    {
      b->has_end = true;
      b->end = XFIXNAT (end);
    }
  j->cb_id = ++nh_blk_next_id;
  j->job.w = NH_WK_BLK;
  j->job.cls = NH_WK_CL_FILE;
  j->job.run = nh_blk_file_run;
  j->job.finish = nh_blk_file_finish;
  j->job.free = nh_blk_file_free;
  atomic_store (&j->job.refs, 1);
  Vnh_blk_callbacks = Fcons (Fcons (make_int (j->cb_id), cb),
			     Vnh_blk_callbacks);
  EMACS_INT id = j->cb_id;
  if (!nh_wk_submit (&j->job))
    {
      Vnh_blk_callbacks = XCDR (Vnh_blk_callbacks);
      nh_wk_job_unref (&j->job);
      return Qnil;
    }
  nh_wk_job_unref (&j->job);	/* the submitter's */
  return make_int (id);
}

/* ---- Inserting a block ------------------------------------------- */

static struct nh_blk_block *
nh_blk_block_arg (Lisp_Object x)
{
  if (!USER_PTRP (x) || XUSER_PTR (x)->finalizer != nh_blk_block_free
      || !XUSER_PTR (x)->p)
    wrong_type_argument (Qnh_blk_file_block_p, x);
  return XUSER_PTR (x)->p;
}

/* Make the erased scratch buffer current (MULTIBYTE or not); the
   caller unbinds.  */
static void
nh_blk_work_buffer (bool multibyte)
{
  record_unwind_current_buffer ();
  Lisp_Object w = Fget_buffer_create (Qnh_blk_work_name, Qt);
  struct buffer *buf = XBUFFER (w);
  delete_all_overlays (buf);
  bset_directory (buf, BVAR (current_buffer, directory));
  bset_read_only (buf, Qnil);
  bset_filename (buf, Qnil);
  bset_undo_list (buf, Qt);
  set_buffer_internal (buf);
  Ferase_buffer ();
  bset_enable_multibyte_characters (buf, multibyte ? Qt : Qnil);
}

/* set-auto-coding over the block's head and tail, as
   insert-file-contents reads them for a non-empty buffer: the first
   1 KiB and the last 3 KiB (all of a file of 4 KiB or less).  */
static Lisp_Object
nh_blk_auto_coding (struct nh_blk_block *b, Lisp_Object name)
{
  unsigned char text[4096];
  size_t nread;
  if (b->n <= 4096)
    {
      memcpy (text, b->bytes, b->n);
      nread = b->n;
    }
  else
    {
      size_t tailbeg = b->n - 3072 > 4096 ? b->n - 3072 : 4096;
      memcpy (text, b->bytes, 1024);
      memcpy (text + 1024, b->bytes + tailbeg, b->n - tailbeg);
      nread = 1024 + (b->n - tailbeg);
    }
  specpdl_ref count = SPECPDL_INDEX ();
  nh_blk_work_buffer (false);
  insert_1_both ((char *) text, nread, nread, 0, 0, 0);
  TEMP_SET_PT_BOTH (BEG, BEG_BYTE);
  Lisp_Object val = calln (Vset_auto_coding_function, name,
			   make_fixnum (nread));
  return unbind_to (count, val);
}

/* The coding system decode_coding_gap would report for the block under
   CODING_SYSTEM: run it on a sample with the same detectable features
   (non-ASCII, a line feed) in a scratch buffer.  */
static Lisp_Object
nh_blk_decided_name (Lisp_Object coding_system, bool nonascii, bool lf)
{
  unsigned char sample[4];
  int len = 0;
  if (nonascii)
    {
      sample[len++] = 0xC3;
      sample[len++] = 0xA9;
    }
  else
    sample[len++] = 'a';
  if (lf)
    sample[len++] = '\n';
  specpdl_ref count = SPECPDL_INDEX ();
  nh_blk_work_buffer (true);
  move_gap_both (BEG, BEG_BYTE);
  if (GAP_SIZE < len + 64)
    make_gap (len + 64 - GAP_SIZE);
  memcpy (GAP_END_ADDR - len, sample, len);
  struct coding_system c;
  setup_coding_system (coding_system, &c);
  c.dst_multibyte = true;
  decode_coding_gap (&c, len);
  Lisp_Object name = CODING_ID_NAME (c.id);
  unbind_to (count, Qnil);
  return name;
}

/* True if detection under `undecided' would pick UTF-8 for valid
   UTF-8 text without a signature.  */
static bool
nh_blk_utf8_first (void)
{
  struct coding_system c;
  setup_coding_system (preferred_coding_system (), &c);
  Lisp_Object attrs = CODING_ID_ATTRS (c.id);
  if (!EQ (CODING_ATTR_TYPE (attrs), Qutf_8))
    return false;
  Lisp_Object bom = plist_get (CODING_ATTR_PLIST (attrs), Qnh_blk_QCbom);
  return NILP (bom) || CONSP (bom);
}

DEFUN ("nidhogg-insert-file-block", Fnidhogg_insert_file_block,
       Snidhogg_insert_file_block, 1, 1, 0,
       doc: /* Insert the file block BLOCK after point, like `insert-file-contents'.
The coding system is chosen here, on the main thread, the way
`insert-file-contents' chooses it (`coding-system-for-read', the
`coding:' cookies, `file-coding-system-alist'), and
`last-coding-system-used' is set.  Valid UTF-8 without a null byte
under `undecided' (when UTF-8 has the top priority) and ASCII-only text
under an ASCII-compatible coding are inserted with the counts the
worker found; everything else is decoded.  Return (FILE CHARS) as
`insert-file-contents' does.  */)
  (Lisp_Object block)
{
  struct nh_blk_block *b = nh_blk_block_arg (block);
  specpdl_ref count = SPECPDL_INDEX ();
  Lisp_Object name = nh_blk_name_string (b);
  Lisp_Object beg = b->has_beg ? make_fixnum (b->beg) : Qnil;
  Lisp_Object end = b->has_end ? make_fixnum (b->end) : Qnil;
  Lisp_Object coding_system = Qnil, old_undo, p, insval;
  struct coding_system coding;
  bool multibyte = !NILP (BVAR (current_buffer, enable_multibyte_characters));
  ptrdiff_t inserted = 0;

  if (!NILP (BVAR (current_buffer, read_only)))
    Fbarf_if_buffer_read_only (Qnil);

  if (EQ (Vcoding_system_for_read, Qauto_save_coding))
    coding_system = coding_inherit_eol_type (Qutf_8_emacs, Qunix);
  else
    {
      if (!NILP (Vcoding_system_for_read))
	coding_system = Vcoding_system_for_read;
      else
	{
	  if (b->regular && b->n > 0 && !NILP (Vset_auto_coding_function))
	    coding_system = nh_blk_auto_coding (b, name);
	  if (NILP (coding_system))
	    {
	      coding_system = CALLN (Ffind_operation_coding_system,
				     Qinsert_file_contents, name, Qnil,
				     beg, end, Qnil);
	      if (CONSP (coding_system))
		coding_system = XCAR (coding_system);
	    }
	}
      if (NILP (coding_system))
	coding_system = Qundecided;
      else
	CHECK_CODING_SYSTEM (coding_system);
      if (!multibyte)
	coding_system = raw_text_coding_system (coding_system);
    }
  setup_coding_system (coding_system, &coding);

  /* The fast path: the worker's counts stand in for the scans.  */
  bool fast = false;
  if (b->n > 0 && multibyte && CODING_MAY_REQUIRE_DECODING (&coding)
      && b->scan.eol <= NH_DEC_EOL_LF && !b->scan.has_null
      && !b->scan.has_esc)
    {
      Lisp_Object attrs = CODING_ID_ATTRS (coding.id);
      if (!NILP (CODING_ATTR_ASCII_COMPAT (attrs))
	  && NILP (CODING_ATTR_POST_READ (attrs))
	  && NILP (CODING_ATTR_DECODE_TBL (attrs))
	  && NILP (CODING_ATTR_TRANS_TBL (attrs)))
	fast = (b->scan.ascii
		|| (b->scan.utf8 && !b->scan.bom
		    && EQ (CODING_ATTR_TYPE (attrs), Qundecided)
		    && nh_blk_utf8_first ()));
    }

  if (b->n > 0)
    {
      if (BUF_BYTES_MAX - Z_BYTE < (ptrdiff_t) b->n)
	buffer_overflow ();
      prepare_to_modify_buffer (PT, PT, NULL);
    }
  move_gap_both (PT, PT_BYTE);
  if ((size_t) GAP_SIZE <= b->n)
    make_gap (b->n - GAP_SIZE + 1);
  if (b->n > 0)
    memcpy (GPT_ADDR, b->bytes, b->n);
  inserted = b->n;
  if (inserted > 0)
    Fset (Qdeactivate_mark, Qt);

  coding.dst_multibyte = multibyte;
  if (fast)
    {
      Vnh_blk_insert_path = Qnh_blk_fast;
      ptrdiff_t chars = b->scan.ascii ? (ptrdiff_t) b->n
				      : (ptrdiff_t) b->scan.nchars;
      insert_from_gap (chars, b->n, false, false);
      inserted = chars;
      coding_system
	= nh_blk_decided_name (coding_system, !b->scan.ascii,
			       b->scan.eol == NH_DEC_EOL_LF);
    }
  else
    {
      Vnh_blk_insert_path = Qnh_blk_decode;
      if (CODING_MAY_REQUIRE_DECODING (&coding)
	  && (inserted > 0 || CODING_REQUIRE_FLUSHING (&coding)))
	{
	  memmove (GAP_END_ADDR - inserted, GPT_ADDR, inserted);
	  decode_coding_gap (&coding, inserted);
	  inserted = coding.produced_char;
	  coding_system = CODING_ID_NAME (coding.id);
	}
      else if (inserted > 0)
	{
	  eassert (NILP (BVAR (current_buffer, enable_multibyte_characters)));
	  insert_from_gap_1 (inserted, inserted, false);
	  invalidate_buffer_caches (current_buffer, PT, PT + inserted);
	  adjust_after_insert (PT, PT_BYTE, PT + inserted, PT_BYTE + inserted,
			       inserted);
	}
    }

  if (inserted > 0 && b->n > 0)
    {
      signal_after_change (PT, 0, inserted);
      update_compositions (PT, PT, CHECK_BORDER);
    }

  Vlast_coding_system_used = coding_system;

  if (!NILP (Ffboundp (Qafter_insert_file_set_coding)))
    {
      insval = calln (Qafter_insert_file_set_coding, make_fixnum (inserted),
		      Qnil);
      if (!NILP (insval))
	{
	  if (!RANGED_FIXNUMP (0, insval, ZV - PT))
	    wrong_type_argument (Qinserted_chars, insval);
	  inserted = XFIXNAT (insval);
	}
    }

  if (inserted > 0 && !NILP (Ffboundp (Qformat_decode)))
    {
      specpdl_ref count1 = SPECPDL_INDEX ();
      ptrdiff_t old_inserted = inserted;
      specbind (Qinhibit_point_motion_hooks, Qt);
      specbind (Qinhibit_modification_hooks, Qt);
      old_undo = BVAR (current_buffer, undo_list);
      bset_undo_list (current_buffer, Qt);

      insval = calln (Qformat_decode, Qnil, make_fixnum (inserted), Qnil);
      if (!RANGED_FIXNUMP (0, insval, ZV - PT))
	wrong_type_argument (Qinserted_chars, insval);
      inserted = XFIXNAT (insval);

      p = Vafter_insert_file_functions;
      FOR_EACH_TAIL (p)
	{
	  insval = calln (XCAR (p), make_fixnum (inserted));
	  if (!NILP (insval))
	    {
	      if (!RANGED_FIXNUMP (0, insval, ZV - PT))
		wrong_type_argument (Qinserted_chars, insval);
	      inserted = XFIXNAT (insval);
	    }
	}

      bset_undo_list (current_buffer, old_undo);
      if (CONSP (old_undo) && inserted != old_inserted)
	{
	  Lisp_Object tem = XCAR (old_undo);
	  if (CONSP (tem) && FIXNUMP (XCAR (tem)) && FIXNUMP (XCDR (tem))
	      && XFIXNUM (XCDR (tem)) == PT + old_inserted)
	    XSETCDR (tem, make_fixnum (PT + inserted));
	}
      unbind_to (count1, Qnil);
    }

  if (current_buffer->base_buffer && current_buffer->base_buffer->newline_cache)
    invalidate_region_cache (current_buffer->base_buffer,
			     current_buffer->base_buffer->newline_cache,
			     PT - BEG, Z - PT - inserted);
  else if (current_buffer->newline_cache)
    invalidate_region_cache (current_buffer, current_buffer->newline_cache,
			     PT - BEG, Z - PT - inserted);

  return unbind_to (count, list2 (name, make_fixnum (inserted)));
}

void
nh_blk_file_syms (void)
{
  DEFSYM (Qnh_blk_file_block_p, "nidhogg-file-block-p");
  DEFSYM (Qnh_blk_fast, "fast");
  DEFSYM (Qnh_blk_decode, "decode");
  Qnh_blk_QCbom = intern_c_string (":bom");
  staticpro (&Qnh_blk_QCbom);
  Qnh_blk_work_name = build_string (" *nidhogg-blk-work*");
  staticpro (&Qnh_blk_work_name);
  staticpro (&Vnh_blk_callbacks);
  Vnh_blk_callbacks = Qnil;
  DEFVAR_LISP ("nidhogg-blk--insert-path", Vnh_blk_insert_path,
	       doc: /* `fast' or `decode': how the last `nidhogg-insert-file-block' inserted.
Internal, for tests.  */);
  Vnh_blk_insert_path = Qnil;
  defsubr (&Snidhogg_read_file_async);
  defsubr (&Snidhogg_insert_file_block);
}

#endif /* not NH_WK_HARNESS */


#ifndef NH_WK_HARNESS

/* ==== Auto-save writes (Task 34): keyboard-triggered auto-saves on
   nh-save ===========================================================

   The keyboard loop's do-auto-save (after `auto-save-interval' events
   or `auto-save-timeout' idle seconds) runs with
   nh_save_keyboard_triggered set.  There, with `nidhogg-workers-autosave'
   on, auto_save_1 hands a buffer on the plain path (its auto-save file
   is not the visited file; no annotation functions, formats, selective
   display, encoding translation or file name handler) to
   nh_save_auto_save_1: it copies the text around the gap and queues a
   job on nh-save, and do-auto-save goes on as before (hooks, counters,
   messages and `recent-auto-save-p' unchanged).  The job encodes the
   text as `utf-8-emacs-unix' does (nh_dec_autosave_encode) into a new
   file in the auto-save file's directory, created with the auto-save
   mode bits: an unnamed O_TMPFILE where the file system has them, else
   `.#NAME#.nh-PID-N' (a leftover of a dead Emacs is removed by the next
   such write there).  It never writes the auto-save file's name.

   The landing (the drainer, main thread) puts the file in place with
   one check and one rename (after linkat of an unnamed file), to the
   buffer's *current* `buffer-auto-save-file-name'.  The job is dropped
   instead, and its file removed, when it was superseded: a synchronous
   write of the buffer (any of them, below), `delete-file' of the name,
   `rename-file' of it to anything but the buffer's current name, a save
   of the buffer past the snapshot (BUF_SAVE_MODIFF moved and the
   visited file's modtime changed: `set-buffer-modified-p' alone, as an
   undo through the unmodified state does, writes nothing and leaves the
   synchronous auto-save file in place), a newer snapshot, or the
   buffer's death.  A write error is reported at the landing with
   auto_save_error's warning.

   Synchronous as today: do-auto-save from Lisp, kill-emacs, the fatal
   path and the tty emergency escape (signal context), every buffer off
   the plain path.  Any of them writes a buffer whose job has not landed
   (nh_save_due: the counters already describe the snapshot) and drops
   the job; reading the jobs needs no lock (main-thread state), so this
   works in a signal handler, which only reads the list and sets
   `dropped'.  A keyboard-triggered auto-save of a buffer whose job still
   runs is written synchronously, with the warning; a job running over
   60 s sets `auto_save_failure_time' (the buffer is then skipped for 20
   minutes) and the auto-save is not written.  kill-buffer settles the
   job before it asks about the auto-save file: it lands a finished job,
   or writes a running one's auto-save now.

   References: the list of jobs holds one (dropped when the job's finish
   runs), the worker one, so a job is freed on the main thread after its
   landing; the free closes the unnamed file and removes the named one if
   it was not put in place.  */

#include <dirent.h>
#include <errno.h>
#include <fcntl.h>
#include <signal.h>
#include <stdio.h>
#include <string.h>
#include <time.h>
#include <unistd.h>

#include "buffer.h"
#include "coding.h"
#include "frame.h"
#include "systime.h"
#include "nidhogg-workers-phase.h"

#define NH_SAVE_HUNG_NS (INT64_C (60) * 1000000000)

/* A buffer's pre-faulted snapshot memory (§8.4): the text of a save
   of a buffer of at least NH_SAVE_ARENA_MIN bytes stays allocated when
   its job lands, and the next save copies into it without page faults.
   A save finds it free (not BUSY: no job is using it) and big enough,
   else it mallocs.  Freed when the buffer is killed or auto-save goes
   off; a busy one then goes when its job is freed (DEAD).  */
enum { NH_SAVE_ARENA_MIN = 4 * 1024 * 1024 };
struct nh_save_arena
{
  struct buffer *buf;		/* the key; never dereferenced */
  unsigned char *mem;
  size_t cap;
  bool busy, dead;
  struct nh_save_arena *next;
};
static pthread_mutex_t nh_save_arena_m = PTHREAD_MUTEX_INITIALIZER;
static struct nh_save_arena *nh_save_arenas;
/* The bytes of pre-faulted snapshot memory kept for later saves.  */
static _Atomic int64_t nh_save_arena_bytes;

/* B's arena, under nh_save_arena_m.  */
static struct nh_save_arena *
nh_save_arena_find (struct buffer *b)
{
  for (struct nh_save_arena *a = nh_save_arenas; a; a = a->next)
    if (a->buf == b)
      return a;
  return NULL;
}

static void
nh_save_arena_unlink (struct nh_save_arena *a)
{
  for (struct nh_save_arena **p = &nh_save_arenas; *p; p = &(*p)->next)
    if (*p == a)
      {
	*p = a->next;
	break;
      }
  atomic_fetch_sub (&nh_save_arena_bytes, (int64_t) a->cap);
  free (a->mem);
  free (a);
}

/* `nidhogg-workers--test-fatal-under-lock' with `arena': the main
   thread dies holding the arena mutex.  */
void
nh_save_test_arena_hold (void)
{
  pthread_mutex_lock (&nh_save_arena_m);
}

/* Free B's arena (a busy one when its job is freed).  */
static void
nh_save_arena_free (struct buffer *b)
{
  pthread_mutex_lock (&nh_save_arena_m);
  struct nh_save_arena *a = nh_save_arena_find (b);
  if (a)
    {
      if (a->busy)
	a->dead = true;
      else
	nh_save_arena_unlink (a);
    }
  pthread_mutex_unlock (&nh_save_arena_m);
}

/* A text of N bytes for a snapshot of B: B's free arena if it holds N,
   else malloc, with an eighth to spare for a big text (the pages
   untouched cost nothing) so that typing between saves fits the arena
   it becomes.  *ARENA is the arena used, or NULL; *CAP the size.  */
static unsigned char *
nh_save_arena_take (struct buffer *b, size_t n, struct nh_save_arena **arena,
		    size_t *cap)
{
  unsigned char *text = NULL;
  *arena = NULL;
  *cap = n;
  pthread_mutex_lock (&nh_save_arena_m);
  struct nh_save_arena *a = nh_save_arena_find (b);
  if (a && !a->busy && !a->dead && a->cap >= n)
    {
      a->busy = true;
      *arena = a;
      *cap = a->cap;
      text = a->mem;
    }
  pthread_mutex_unlock (&nh_save_arena_m);
  if (text)
    return text;
  if (n >= NH_SAVE_ARENA_MIN)
    *cap = n + n / 8;
  return malloc (*cap ? *cap : 1);
}

struct nh_save_job
{
  struct nh_wk_job job;
  /* Inputs (copies).  */
  unsigned char *text;		/* the snapshot; encoded in place */
  size_t nbytes;
  size_t cap;			/* the allocation TEXT is in */
  struct nh_save_arena *arena;	/* the arena TEXT is, or NULL (malloc) */
  char *target;			/* the encoded auto-save name at the snapshot */
  int mode;
  bool unnamed;			/* try O_TMPFILE first */
  uintmax_t serial;		/* N of the temporary names */
  /* Set by the main thread (also in a signal handler), read by the
     worker and the landing.  */
  _Atomic bool dropped;
  /* Worker output.  */
  int fd;			/* the unnamed file, open; else -1 */
  char *tmp;			/* the named file, or NULL */
  const char *err_op;		/* NULL, or the failed operation */
  int err;
  /* Main thread only.  */
  struct buffer *buf;		/* XCAR (holder) */
  Lisp_Object holder;		/* (BUFFER . NAME), in nh_save_roots */
  modiff_count modiff;		/* BUF_MODIFF at the snapshot */
  struct timespec modtime;	/* the buffer's modtime at the snapshot */
  struct timespec started;	/* CLOCK_MONOTONIC, at the snapshot */
  bool landed;
  /* A later auto-save skipped the buffer for this hung job, and
     do-auto-save then counted the buffer's whole text as saved.  */
  bool skipped_over;
  struct nh_save_job *next;
};

/* The jobs whose finish has not run, newest first.  Main thread only;
   a signal handler on the main thread reads it (every change is one
   pointer store, so it sees the list before or after) and sets
   `dropped'.  */
static struct nh_save_job *nh_save_jobs;
static Lisp_Object nh_save_roots;
static uintmax_t nh_save_serial;

/* J's job is over: keep a big text as its buffer's arena
   (J's own pages are already faulted), unless one as big is there.  */
static void
nh_save_arena_adopt (struct nh_save_job *j)
{
  if (!j->text || j->arena || j->nbytes < NH_SAVE_ARENA_MIN)
    return;
  pthread_mutex_lock (&nh_save_arena_m);
  struct nh_save_arena *a = nh_save_arena_find (j->buf);
  if (a && (a->busy || a->dead || a->cap >= j->cap))
    {
      pthread_mutex_unlock (&nh_save_arena_m);
      return;
    }
  if (a)
    nh_save_arena_unlink (a);
  a = calloc (1, sizeof *a);
  if (a)
    {
      *a = (struct nh_save_arena) { .buf = j->buf, .mem = j->text,
				    .cap = j->cap, .next = nh_save_arenas };
      nh_save_arenas = a;
      atomic_fetch_add (&nh_save_arena_bytes, (int64_t) a->cap);
      nh_wk_stat_add (NH_WK_SAVE, NH_WK_ST_QUEUED_BYTES,
		      -(int64_t) j->nbytes);
      j->text = NULL;
    }
  pthread_mutex_unlock (&nh_save_arena_m);
}

/* The keyboard loop's do-auto-save is running, at this eval depth (an
   auto-save-hook function calling do-auto-save is deeper: Lisp).  */
static bool nh_save_keyboard_triggered;
static intmax_t nh_save_kbd_depth;

static int64_t
nh_save_age_ns (struct nh_save_job *j)
{
  struct timespec now;
  clock_gettime (CLOCK_MONOTONIC, &now);
  return ((int64_t) (now.tv_sec - j->started.tv_sec) * 1000000000
	  + (now.tv_nsec - j->started.tv_nsec));
}

/* ---- The worker ----------------------------------------------------- */

/* The bytes of BASE the temporary names keep: at most 200, so that
   `.#BASE#.nh-PID-SERIAL' fits a 255-byte name whatever the auto-save
   name's length, cut at a UTF-8 character's start (macOS refuses a
   name that is not UTF-8).  */
static int
nh_save_base_len (const char *base)
{
  size_t n = strlen (base);
  if (n <= 200)
    return n;
  n = 200;
  while (n > 0 && ((unsigned char) base[n] & 0xc0) == 0x80)
    n--;
  return n;
}

/* `DIR/.#BASE#.nh-PID-SERIAL' for TARGET = DIR/BASE.  */
static char *
nh_save_tmpname (const char *target, uintmax_t serial)
{
  const char *slash = strrchr (target, '/');
  int dirlen = slash ? slash - target + 1 : 0;
  size_t size = strlen (target) + 64;
  char *s = malloc (size);
  NH_WK_ASSERT (s);
  snprintf (s, size, "%.*s.#%.*s#.nh-%jd-%ju", dirlen, target,
	    nh_save_base_len (target + dirlen), target + dirlen,
	    (intmax_t) getpid (), serial);
  return s;
}

static char *
nh_save_dirname (const char *target)
{
  const char *slash = strrchr (target, '/');
  if (!slash)
    {
      char *s = strdup (".");
      NH_WK_ASSERT (s);
      return s;
    }
  size_t n = slash == target ? 1 : slash - target;
  char *s = malloc (n + 1);
  NH_WK_ASSERT (s);
  memcpy (s, target, n);
  s[n] = 0;
  return s;
}

/* Remove the `.#BASE#.nh-PID-N' files of TARGET whose Emacs is gone.  */
static void
nh_save_clean_leftovers (const char *target)
{
  char *dir = nh_save_dirname (target);
  const char *slash = strrchr (target, '/');
  const char *base = slash ? slash + 1 : target;
  int blen = nh_save_base_len (base);
  size_t plen = blen + sizeof ".##.nh-" - 1;
  char *prefix = malloc (plen + 1);
  NH_WK_ASSERT (prefix);
  snprintf (prefix, plen + 1, ".#%.*s#.nh-", blen, base);
  int fd = open (dir, O_RDONLY | O_DIRECTORY | O_CLOEXEC);
  DIR *d = fd < 0 ? NULL : fdopendir (fd);
  if (!d && fd >= 0)
    close (fd);
  for (struct dirent *e; d && (e = readdir (d)); )
    {
      if (strncmp (e->d_name, prefix, plen) != 0)
	continue;
      char *end;
      long pid = strtol (e->d_name + plen, &end, 10);
      if (end != e->d_name + plen && *end == '-' && pid > 0
	  && pid != getpid () && kill (pid, 0) != 0 && errno == ESRCH)
	unlinkat (dirfd (d), e->d_name, 0);
    }
  if (d)
    closedir (d);
  free (prefix);
  free (dir);
}

static void
nh_save_fail (struct nh_save_job *j, const char *op, int err)
{
  j->err_op = op;
  j->err = err;
}

static void
nh_save_write (struct nh_save_job *j)
{
  size_t n = nh_dec_autosave_encode (j->text, j->nbytes, j->text);
  if (!nh_wk_fd_budget_ok (2))
    {
      nh_save_fail (j, "Opening output file", EMFILE);
      return;
    }

  int fd = -1;
#ifdef O_TMPFILE
  if (j->unnamed)
    {
      char *dir = nh_save_dirname (j->target);
      fd = open (dir, O_TMPFILE | O_WRONLY | O_CLOEXEC, j->mode);
      int e = errno;
      free (dir);
      /* No O_TMPFILE on this file system: the named file.  */
      if (fd < 0 && e != EOPNOTSUPP && e != EISDIR && e != EINVAL)
	{
	  nh_save_fail (j, "Opening output file", e);
	  return;
	}
    }
#endif
  if (fd < 0)
    {
      nh_save_clean_leftovers (j->target);
      j->tmp = nh_save_tmpname (j->target, j->serial);
      fd = open (j->tmp, O_WRONLY | O_CREAT | O_EXCL | O_CLOEXEC, j->mode);
      if (fd < 0)
	{
	  int e = errno;
	  free (j->tmp);
	  j->tmp = NULL;
	  nh_save_fail (j, "Opening output file", e);
	  return;
	}
    }

  int werr = 0;
  for (size_t done = 0; done < n; )
    {
      size_t want = n - done < (1u << 30) ? n - done : (1u << 30);
      ssize_t r = write (fd, j->text + done, want);
      if (r < 0)
	{
	  if (errno == EINTR)
	    continue;
	  werr = errno;
	  break;
	}
      done += r;
    }
  /* A named file is complete once closed; NFS reports late errors at
     the close.  The unnamed one stays open for linkat.  */
  if (j->tmp)
    {
      if (close (fd) != 0 && !werr)
	werr = errno;
      fd = -1;
    }
  if (werr)
    {
      if (fd >= 0)
	close (fd);
      if (j->tmp)
	{
	  unlink (j->tmp);
	  free (j->tmp);
	  j->tmp = NULL;
	}
      nh_save_fail (j, "Write error", werr);
      return;
    }
  j->fd = fd;
}

static void
nh_save_free_text (struct nh_save_job *j)
{
  if (j->text)
    {
      if (j->arena)
	{
	  /* The memory stays the buffer's, unless it was freed
	     meanwhile.  */
	  pthread_mutex_lock (&nh_save_arena_m);
	  j->arena->busy = false;
	  if (j->arena->dead)
	    nh_save_arena_unlink (j->arena);
	  pthread_mutex_unlock (&nh_save_arena_m);
	}
      else
	free (j->text);
      j->text = NULL;
      nh_wk_stat_add (NH_WK_SAVE, NH_WK_ST_QUEUED_BYTES, -(int64_t) j->nbytes);
    }
}

static void
nh_save_run (struct nh_wk_job *wj)
{
  struct nh_save_job *j = (struct nh_save_job *) wj;
  nh_wk_pause_point (NH_WK_P_BEFORE_WRITE);
  if (!atomic_load (&j->dropped))
    nh_save_write (j);
  nh_save_arena_adopt (j);
  nh_save_free_text (j);
}

/* Lisp-free: it may run wherever the last reference goes.  */
static void
nh_save_free (struct nh_wk_job *wj)
{
  struct nh_save_job *j = (struct nh_save_job *) wj;
  if (j->fd >= 0)
    close (j->fd);
  if (j->tmp)
    {
      unlink (j->tmp);
      free (j->tmp);
    }
  nh_save_free_text (j);
  free (j->target);
  free (j);
}

/* ---- The main thread ------------------------------------------------ */

/* May this auto-save queue jobs?  Never in a signal handler or on the
   fatal path, whatever the flag says.  */
static bool
nh_save_async_context (void)
{
  return (nh_save_keyboard_triggered && lisp_eval_depth == nh_save_kbd_depth
	  && !nh_wk_signal_context () && !fatal_error_in_progress);
}

static bool
nh_save_switch_on (void)
{
  Lisp_Object v = find_symbol_value (Qnidhogg_workers_autosave);
  return !BASE_EQ (v, Qunbound) && !NILP (v);
}

/* auto_save_error's warning, for buffer B (it is static in fileio.c).  */
static void
nh_save_warn (struct buffer *b, Lisp_Object error_val)
{
  specpdl_ref count = SPECPDL_INDEX ();
  record_unwind_current_buffer ();
  set_buffer_internal (b);
  ring_bell (XFRAME (selected_frame));
  AUTO_STRING (format, "Auto-saving %s: %s");
  Lisp_Object msg = CALLN (Fformat, format, BVAR (current_buffer, name),
			   Ferror_message_string (error_val));
  calln (Qdisplay_warning, Qauto_save, msg, QCerror);
  unbind_to (count, Qnil);
}

/* B's auto-save name now, expanded in B's directory, or nil.  */
static Lisp_Object
nh_save_name (struct buffer *b)
{
  Lisp_Object name = BVAR (b, auto_save_file_name);
  return STRINGP (name) ? Fexpand_file_name (name, BVAR (b, directory)) : Qnil;
}

/* The plain path (§8.6): what write-region would do for B's auto-save
   is a plain write of the encoded text.  B is the current buffer.  */
static bool
nh_save_plain_path (struct buffer *b)
{
  Lisp_Object name = BVAR (b, auto_save_file_name);
  Lisp_Object format = BVAR (b, auto_save_file_format);
  if (EQ (format, Qt))
    format = BVAR (b, file_format);
  return (STRINGP (name)
	  && NILP (Fstring_equal (BVAR (b, filename), name))
	  && NILP (Vauto_save_visited_file_name)
	  && NILP (Vwrite_region_annotate_functions)
	  && NILP (Vwrite_region_post_annotation_function)
	  && NILP (format)
	  && !EQ (BVAR (b, selective_display), Qt)
	  && (NILP (Venable_character_translation)
	      || NILP (Vstandard_translation_table_for_encode))
	  && NILP (Ffind_file_name_handler (nh_save_name (b), Qwrite_region)));
}

/* Drop every job of B: a synchronous write supersedes them.  No lock,
   no allocation: safe in a signal handler.  A finished job's named
   file goes at once (unlink is async-signal-safe; the free unlinks it
   again, harmlessly): kill-emacs and the fatal path end the process
   before the job is freed, and the named file, unlike an unnamed
   O_TMPFILE one, would outlive it.  */
static void
nh_save_drop_buffer (struct buffer *b)
{
  for (struct nh_save_job *j = nh_save_jobs; j; j = j->next)
    if (j->buf == b && !j->landed)
      {
	atomic_store (&j->dropped, true);
	if (atomic_load (&j->job.state) == NH_WK_J_DONE && j->tmp)
	  unlink (j->tmp);
      }
}

/* B's job that has not finished running, or NULL.  */
static struct nh_save_job *
nh_save_running (struct buffer *b)
{
  for (struct nh_save_job *j = nh_save_jobs; j; j = j->next)
    if (j->buf == b && !j->landed)
      {
	int st = atomic_load (&j->job.state);
	if (st == NH_WK_J_QUEUED || st == NH_WK_J_RUNNING)
	  return j;
      }
  return NULL;
}

/* The jobs whose finish has not run, each with a reference, for a walk
   that runs Lisp (a job may land and leave the list meanwhile).  */
struct nh_save_held
{
  struct nh_save_job **v;
  ptrdiff_t n;
};

static void
nh_save_hold (struct nh_save_held *h)
{
  ptrdiff_t k = 0;
  for (struct nh_save_job *j = nh_save_jobs; j; j = j->next)
    k++;
  h->v = xnmalloc (k ? k : 1, sizeof *h->v);
  h->n = 0;
  for (struct nh_save_job *j = nh_save_jobs; j; j = j->next)
    {
      nh_wk_job_ref (&j->job);
      h->v[h->n++] = j;
    }
}

/* The unwind that ends such a walk.  */
static void
nh_save_release (void *arg)
{
  struct nh_save_held *h = arg;
  for (ptrdiff_t i = 0; i < h->n; i++)
    nh_wk_job_unref (&h->v[i]->job);
  xfree (h->v);
}

static void
nh_save_unref_unwind (void *job)
{
  nh_wk_job_unref (job);
}

/* Undo the counters' claim that the snapshot is on disk: the next
   auto-save of B writes it (a landing that could not rename).  */
static void
nh_save_redue (struct buffer *b, struct nh_save_job *j)
{
  if (BUF_AUTOSAVE_MODIFF (b) >= j->modiff)
    BUF_AUTOSAVE_MODIFF (b) = j->modiff - 1;
}

/* Put the job's file in place as TARGET; return 0 or an errno.  */
static int
nh_save_put (struct nh_save_job *j, const char *target)
{
  if (j->tmp)
    {
      if (rename (j->tmp, target) != 0)
	return errno;
      free (j->tmp);
      j->tmp = NULL;
      return 0;
    }
#ifdef O_TMPFILE
  if (j->fd < 0)
    return EBADF;
  char proc[64];
  snprintf (proc, sizeof proc, "/proc/self/fd/%d", j->fd);
  char *tmp = nh_save_tmpname (target, j->serial);
  int err = 0;
  if (linkat (AT_FDCWD, proc, AT_FDCWD, tmp, AT_SYMLINK_FOLLOW) != 0)
    err = errno;
  else if (rename (tmp, target) != 0)
    {
      err = errno;
      unlink (tmp);
    }
  free (tmp);
  return err;
#else
  return EBADF;
#endif
}

/* The landing: put J's file in place, unless J was superseded.  */
static void
nh_save_land (struct nh_save_job *j)
{
  Lisp_Object holder = j->holder; /* on the stack: the GC sees it */
  struct buffer *b = XBUFFER (XCAR (holder));

  j->landed = true;
  if (!BUFFER_LIVE_P (b))
    {
      /* Killed while the job ran: it may have left an arena.  */
      nh_save_arena_free (b);
      return;
    }
  /* As a write that took over 60 s does today.  */
  if (nh_save_age_ns (j) > NH_SAVE_HUNG_NS)
    b->auto_save_failure_time = current_timespec ().tv_sec;
  /* At most the snapshot's text is saved: the next auto-save after the
     20 minutes writes what the skipped one did not.  */
  if (j->skipped_over && BUF_AUTOSAVE_MODIFF (b) > j->modiff)
    BUF_AUTOSAVE_MODIFF (b) = j->modiff;
  if (atomic_load (&j->dropped)
      || (BUF_SAVE_MODIFF (b) >= j->modiff
	  && timespec_cmp (b->modtime, j->modtime) != 0)
      || !STRINGP (BVAR (b, auto_save_file_name)))
    return;
  if (j->err_op)
    {
      nh_save_warn (b, get_file_errno_data (j->err_op, XCDR (holder), j->err));
      return;
    }
  Lisp_Object target = nh_save_name (b);
  /* The name changed under the job to one the plain path never writes
     (the visited file itself, or auto-saving there): renaming the
     snapshot onto it would replace the user's file with older text.
     The next auto-save writes, the way the buffer asks for now.  */
  if (!NILP (Ffind_file_name_handler (target, Qwrite_region))
      || !NILP (Vauto_save_visited_file_name)
      || (STRINGP (BVAR (b, filename))
	  && !NILP (Fstring_equal (Fexpand_file_name (BVAR (b, filename), Qnil),
				   target))))
    {
      nh_save_redue (b, j);
      return;
    }
  int err = nh_save_put (j, SSDATA (ENCODE_FILE (target)));
  if (err == EXDEV)
    nh_save_redue (b, j);
  else if (err)
    nh_save_warn (b, get_file_errno_data ("Opening output file", target, err));
}

/* J's finish has run: it leaves the list and drops the list's
   reference (the drainer still holds the worker's).  */
static void
nh_save_forget (struct nh_save_job *j)
{
  for (struct nh_save_job **p = &nh_save_jobs; *p; p = &(*p)->next)
    if (*p == j)
      {
	*p = j->next;
	break;
      }
  nh_save_roots = Fdelq (j->holder, nh_save_roots);
  nh_wk_job_unref (&j->job);
}

/* The drainer's reference keeps J alive through the landing.  */
static void
nh_save_finish (struct nh_wk_job *wj)
{
  struct nh_save_job *j = (struct nh_save_job *) wj;
  nh_save_forget (j);
  if (!j->landed)
    nh_save_land (j);
}

/* The snapshot's copy, timed and its minor faults counted (this
   thread's), for `nidhogg-workers--test-save-snapshot'.  */
static _Atomic int64_t nh_save_snap_ns, nh_save_snap_faults;

static int64_t
nh_save_thread_faults (void)
{
  struct rusage ru;
#ifdef RUSAGE_THREAD
  if (getrusage (RUSAGE_THREAD, &ru) != 0)
    return 0;
#else
  if (getrusage (RUSAGE_SELF, &ru) != 0)
    return 0;
#endif
  return ru.ru_minflt;
}

/* Copy B's text (the current buffer) and queue its write.  */
static bool
nh_save_queue (struct buffer *b, int mode)
{
  ptrdiff_t n1 = BUF_GPT_BYTE (b) - BEG_BYTE;
  ptrdiff_t n2 = BUF_Z_BYTE (b) - BUF_GPT_BYTE (b);
  Lisp_Object name = nh_save_name (b);
  Lisp_Object encoded = ENCODE_FILE (name);
  struct nh_save_job *j = calloc (1, sizeof *j);
  struct nh_save_arena *arena;
  size_t cap;
  unsigned char *text = nh_save_arena_take (b, n1 + n2, &arena, &cap);
  char *target = strdup (SSDATA (encoded));
  if (!j || !text || !target)
    {
      free (j);
      if (arena)
	{
	  pthread_mutex_lock (&nh_save_arena_m);
	  arena->busy = false;
	  pthread_mutex_unlock (&nh_save_arena_m);
	}
      else
	free (text);
      free (target);
      return false;
    }
  j->arena = arena;
  j->cap = cap;
  struct timespec c0, c1;
  int64_t f0 = nh_save_thread_faults ();
  int depth = NH_WK_PHASE_PUSH (NH_WK_PH_WORKER_WAIT, NH_WK_RS_SNAPSHOT);
  clock_gettime (CLOCK_MONOTONIC, &c0);
  memcpy (text, BUF_BEG_ADDR (b), n1);
  memcpy (text + n1, BUF_GAP_END_ADDR (b), n2);
  clock_gettime (CLOCK_MONOTONIC, &c1);
  NH_WK_PHASE_POP (depth);
  atomic_store (&nh_save_snap_ns, (int64_t) (c1.tv_sec - c0.tv_sec)
		* 1000000000 + (c1.tv_nsec - c0.tv_nsec));
  atomic_store (&nh_save_snap_faults, nh_save_thread_faults () - f0);
  j->text = text;
  j->nbytes = n1 + n2;
  nh_wk_stat_add (NH_WK_SAVE, NH_WK_ST_QUEUED_BYTES, (int64_t) j->nbytes);
  j->target = target;
  j->mode = mode;
  j->unnamed = nh_save_unnamed;
  j->serial = ++nh_save_serial;
  j->fd = -1;
  j->buf = b;
  j->modiff = BUF_MODIFF (b);
  j->modtime = b->modtime;
  clock_gettime (CLOCK_MONOTONIC, &j->started);
  j->job.w = NH_WK_SAVE;
  j->job.run = nh_save_run;
  j->job.finish = nh_save_finish;
  j->job.free = nh_save_free;
  atomic_store (&j->job.refs, 1);	/* the list's */
  Lisp_Object buffer;
  XSETBUFFER (buffer, b);
  j->holder = Fcons (buffer, name);
  nh_save_roots = Fcons (j->holder, nh_save_roots);
  if (!nh_wk_submit (&j->job))
    {
      nh_save_roots = XCDR (nh_save_roots);
      nh_wk_job_unref (&j->job);
      return false;
    }
  /* Two stores, in this order: a signal handler sees the old list or
     the new one.  */
  j->next = nh_save_jobs;
  nh_save_jobs = j;
  return true;
}

bool
nh_save_auto_save_1 (struct buffer *b, int mode)
{
  if (!nh_save_async_context ()
      || !nh_save_switch_on () || !nh_save_plain_path (b))
    {
      nh_save_drop_buffer (b);
      /* Not in a signal handler or on the fatal path (an emergency
	 auto-save): the arena's mutex may be held by the code it
	 interrupted, and free may be where it crashed.  */
      if (!nh_wk_signal_context () && !fatal_error_in_progress)
	nh_save_arena_free (b);
      return false;
    }
  struct nh_save_job *prev = nh_save_running (b);
  if (prev)
    {
      bool live = !atomic_load (&prev->dropped);
      if (live && nh_save_age_ns (prev) > NH_SAVE_HUNG_NS)
	{
	  /* As a 60 s write does today: skipped for 20 minutes.  But
	     unlike that write, nothing past the job's snapshot is on its
	     way: the landing takes back do-auto-save's claim that it is.  */
	  b->auto_save_failure_time = current_timespec ().tv_sec;
	  prev->skipped_over = true;
	  nh_save_warn (b, list2 (Qerror,
				  build_string ("The previous auto-save has"
						" not finished in 60 seconds")));
	  return true;
	}
      nh_save_drop_buffer (b);
      if (live)
	nh_save_warn (b, list2 (Qerror,
				build_string ("The previous auto-save is"
					      " still being written")));
      return false;
    }
  /* A finished job not landed yet: the newer snapshot replaces it.  */
  nh_save_drop_buffer (b);
  return nh_save_queue (b, mode);
}

bool
nh_save_due (struct buffer *b)
{
  if (!nh_save_jobs || nh_save_async_context ())
    return false;
  for (struct nh_save_job *j = nh_save_jobs; j; j = j->next)
    if (j->buf == b && !j->landed && !atomic_load (&j->dropped))
      return true;
  return false;
}

static void
nh_save_kbd_unwind (void)
{
  nh_save_keyboard_triggered = false;
}

void
nh_save_keyboard_auto_save (Lisp_Object no_message)
{
  specpdl_ref count = SPECPDL_INDEX ();
  record_unwind_protect_void (nh_save_kbd_unwind);
  nh_save_keyboard_triggered = true;
  nh_save_kbd_depth = lisp_eval_depth;
  Fdo_auto_save (no_message, Qnil);
  unbind_to (count, Qnil);
  /* A buffer whose auto-saving is off never reaches auto_save_1 to
     free its snapshot memory: free it here.  */
  if (atomic_load (&nh_save_arena_bytes) > 0)
    {
      Lisp_Object tail, buf;
      FOR_EACH_LIVE_BUFFER (tail, buf)
	if (!STRINGP (BVAR (XBUFFER (buf), auto_save_file_name)))
	  nh_save_arena_free (XBUFFER (buf));
    }
}

/* Drop the jobs whose current auto-save name is FROM, unless they keep
   that name (TO is it); TO nil: a deletion.  */
static void
nh_save_name_gone (Lisp_Object from, Lisp_Object to)
{
  if (!nh_save_jobs)
    return;
  struct nh_save_held h;
  specpdl_ref count = SPECPDL_INDEX ();
  nh_save_hold (&h);
  record_unwind_protect_ptr (nh_save_release, &h);
  for (ptrdiff_t i = 0; i < h.n; i++)
    {
      struct nh_save_job *j = h.v[i];
      if (j->landed || atomic_load (&j->dropped) || !BUFFER_LIVE_P (j->buf))
	continue;
      Lisp_Object cur = nh_save_name (j->buf);
      if (STRINGP (cur) && !NILP (Fstring_equal (cur, from))
	  && (NILP (to) || NILP (Fstring_equal (cur, to))))
	atomic_store (&j->dropped, true);
    }
  unbind_to (count, Qnil);
}

void
nh_save_file_deleted (Lisp_Object name)
{
  nh_save_name_gone (name, Qnil);
}

void
nh_save_file_renamed (Lisp_Object from, Lisp_Object to)
{
  nh_save_name_gone (from, to);
}

void
nh_save_settle (struct buffer *b)
{
  bool write = false;
 again:
  for (struct nh_save_job *j = nh_save_jobs; j; j = j->next)
    if (j->buf == b && !j->landed && !atomic_load (&j->dropped))
      {
	if (atomic_load (&j->job.state) == NH_WK_J_DONE)
	  {
	    /* The landing may run Lisp: hold J, then walk again.  */
	    specpdl_ref count = SPECPDL_INDEX ();
	    nh_wk_job_ref (&j->job);
	    record_unwind_protect_ptr (nh_save_unref_unwind, &j->job);
	    nh_save_land (j);
	    unbind_to (count, Qnil);
	    goto again;
	  }
	write = true;
      }
  /* A running job of a buffer that has nothing to auto-save any more
     (saved since, or auto-saving turned off) is dropped, not replaced
     by a write: that write would leave an auto-save file of a saved
     buffer, or warn about a missing name.  */
  if (write && (BUF_SAVE_MODIFF (b) >= BUF_MODIFF (b)
		|| !STRINGP (BVAR (b, auto_save_file_name))))
    {
      nh_save_drop_buffer (b);
      write = false;
    }
  if (write)
    {
      /* auto_save_1 drops the running job: this write supersedes it.  */
      specpdl_ref count = SPECPDL_INDEX ();
      record_unwind_current_buffer ();
      set_buffer_internal (b);
      nh_save_write_sync ();
      unbind_to (count, Qnil);
    }
  /* The buffer is being killed: its snapshot memory goes.  */
  nh_save_arena_free (b);
}

/* ---- Test primitives ------------------------------------------------ */

DEFUN ("nidhogg-workers--test-keyboard-auto-save",
       Fnidhogg_workers__test_keyboard_auto_save,
       Snidhogg_workers__test_keyboard_auto_save, 0, 1, 0,
       doc: /* Auto-save as the keyboard loop does after typing; for tests.
NO-MESSAGE as for `do-auto-save'.  This is the call `read_char' makes
after `auto-save-interval' events or `auto-save-timeout' idle seconds.  */)
  (Lisp_Object no_message)
{
  nh_save_keyboard_auto_save (no_message);
  return Qnil;
}

DEFUN ("nidhogg-workers--test-save-await", Fnidhogg_workers__test_save_await,
       Snidhogg_workers__test_save_await, 1, 1, 0,
       doc: /* Wait at most MS ms for the auto-save jobs to finish, then land them; for tests.
Return t if every job has landed, nil if one is still running (a job
parked at a pause point).  Bounded condition waits, not sleeps.  */)
  (Lisp_Object ms)
{
  CHECK_FIXNAT (ms);
  struct timespec t0;
  clock_gettime (CLOCK_MONOTONIC, &t0);
  int64_t deadline = ((int64_t) t0.tv_sec * 1000000000 + t0.tv_nsec
		      + (int64_t) XFIXNAT (ms) * 1000000);
  struct nh_save_held h;
  specpdl_ref count = SPECPDL_INDEX ();
  nh_save_hold (&h);
  record_unwind_protect_ptr (nh_save_release, &h);
  for (ptrdiff_t i = 0; i < h.n; i++)
    {
      struct timespec now;
      clock_gettime (CLOCK_MONOTONIC, &now);
      int64_t left = deadline - ((int64_t) now.tv_sec * 1000000000
				 + now.tv_nsec);
      nh_wk_wait (&h.v[i]->job, left > 0 ? left : 0, NH_WK_PH_WORKER_WAIT, 0);
    }
  unbind_to (count, Qnil);
  Fnidhogg_workers__drain ();
  for (struct nh_save_job *j = nh_save_jobs; j; j = j->next)
    if (!j->landed)
      return Qnil;
  return Qt;
}

DEFUN ("nidhogg-workers--test-save-jobs", Fnidhogg_workers__test_save_jobs,
       Snidhogg_workers__test_save_jobs, 0, 0, 0,
       doc: /* Return the auto-save jobs not landed, newest first; for tests.
A list of (BUFFER STATE), STATE `queued', `running', `done' or
`dropped'.  */)
  (void)
{
  static const char *const states[] = { "queued", "running", "done", "done" };
  Lisp_Object r = Qnil;
  for (struct nh_save_job *j = nh_save_jobs; j; j = j->next)
    if (!j->landed)
      r = Fcons (list2 (XCAR (j->holder),
			intern_c_string (atomic_load (&j->dropped) ? "dropped"
					 : states[atomic_load (&j->job.state)])),
		 r);
  return Fnreverse (r);
}

DEFUN ("nidhogg-workers--test-save-age", Fnidhogg_workers__test_save_age,
       Snidhogg_workers__test_save_age, 1, 1, 0,
       doc: /* Make every auto-save job not landed SECONDS older; for tests.  */)
  (Lisp_Object seconds)
{
  CHECK_FIXNAT (seconds);
  for (struct nh_save_job *j = nh_save_jobs; j; j = j->next)
    j->started.tv_sec -= XFIXNAT (seconds);
  return Qnil;
}

DEFUN ("nidhogg-workers--test-save-snapshot",
       Fnidhogg_workers__test_save_snapshot,
       Snidhogg_workers__test_save_snapshot, 0, 0, 0,
       doc: /* The last auto-save snapshot's copy; for tests.
A plist: `:snapshot-ns' and `:snapshot-faults' (this thread's minor
page faults during the copy), `:arena-bytes' (pre-faulted snapshot
memory kept for later saves).  */)
  (void)
{
  return list (intern_c_string (":snapshot-ns"),
	       make_int (atomic_load (&nh_save_snap_ns)),
	       intern_c_string (":snapshot-faults"),
	       make_int (atomic_load (&nh_save_snap_faults)),
	       intern_c_string (":arena-bytes"),
	       make_int (atomic_load (&nh_save_arena_bytes)));
}

DEFUN ("nidhogg-workers--test-copy-bench", Fnidhogg_workers__test_copy_bench,
       Snidhogg_workers__test_copy_bench, 1, 1, 0,
       doc: /* Copy BYTES into fresh memory, then into the same memory; for tests.
Return (NS1 FAULTS1 NS2 FAULTS2): the time and this thread's minor
faults of each copy.  */)
  (Lisp_Object bytes)
{
  CHECK_FIXNAT (bytes);
  size_t n = XFIXNAT (bytes);
  unsigned char *src = malloc (n ? n : 1), *dst = malloc (n ? n : 1);
  if (!src || !dst)
    {
      free (src);
      free (dst);
      memory_full (n);
    }
  memset (src, 'x', n);
  /* The copies are observed (the compiler may not drop them).  */
  volatile unsigned char sink = 0;
  int64_t ns[2], faults[2];
  for (int i = 0; i < 2; i++)
    {
      struct timespec c0, c1;
      int64_t f0 = nh_save_thread_faults ();
      clock_gettime (CLOCK_MONOTONIC, &c0);
      memcpy (dst, src, n);
      __asm__ volatile ("" : : "r" (dst) : "memory");
      clock_gettime (CLOCK_MONOTONIC, &c1);
      faults[i] = nh_save_thread_faults () - f0;
      sink += dst[n / 2];
      ns[i] = (int64_t) (c1.tv_sec - c0.tv_sec) * 1000000000
	+ (c1.tv_nsec - c0.tv_nsec);
    }
  (void) sink;
  free (src);
  free (dst);
  return list4 (make_int (ns[0]), make_int (faults[0]),
		make_int (ns[1]), make_int (faults[1]));
}

DEFUN ("nidhogg-workers--test-save-encode", Fnidhogg_workers__test_save_encode,
       Snidhogg_workers__test_save_encode, 1, 1, 0,
       doc: /* Return STRING's internal bytes as the auto-save job encodes them; for tests.
The value is a unibyte string.  For a multibyte STRING it equals
`encode-coding-string' with `utf-8-emacs-unix'; for a unibyte one, what
`write-region' writes from a unibyte buffer with that coding.  */)
  (Lisp_Object string)
{
  CHECK_STRING (string);
  ptrdiff_t n = SBYTES (string);
  unsigned char *buf = xmalloc (n ? n : 1);
  memcpy (buf, SDATA (string), n);
  size_t m = nh_dec_autosave_encode (buf, n, buf);
  Lisp_Object r = make_unibyte_string ((char *) buf, m);
  xfree (buf);
  return r;
}

static void
nh_save_syms (void)
{
  DEFSYM (Qnidhogg_workers_autosave, "nidhogg-workers-autosave");
  nh_save_roots = Qnil;
  staticpro (&nh_save_roots);
  DEFVAR_BOOL ("nidhogg-workers--save-unnamed", nh_save_unnamed,
	       doc: /* Non-nil: an auto-save job writes an unnamed O_TMPFILE where it can.
Nil: always the named `.#NAME#.nh-PID-N' file.  Internal, for tests.  */);
  nh_save_unnamed = true;
  defsubr (&Snidhogg_workers__test_keyboard_auto_save);
  defsubr (&Snidhogg_workers__test_save_await);
  defsubr (&Snidhogg_workers__test_save_jobs);
  defsubr (&Snidhogg_workers__test_save_age);
  defsubr (&Snidhogg_workers__test_save_encode);
  defsubr (&Snidhogg_workers__test_save_snapshot);
  defsubr (&Snidhogg_workers__test_copy_bench);
}

/* ==== End of auto-save writes ======================================== */

#endif /* not NH_WK_HARNESS */


#ifndef NH_WK_HARNESS

/* ==== Host-name lookups (Task 35): make-network-process `:nowait' on
   nh-blk class `dns' ==================================================

   (§8.7, MC-27) The lookup glibc's getaddrinfo_a made, on every
   platform: getaddrinfo with the same hints, on an nh-blk thread.  The
   process owns the job (its nh_dns_job); the wait loop polls it where it
   polls getaddrinfo_a's request and hands the addresses to the same
   postponed connect, or sets the status glibc's path sets.  A running
   getaddrinfo cannot be cancelled: a delete-process during the lookup
   drops the job and returns at once; a lookup still parked or queued
   is skipped, and one already in getaddrinfo finishes on its thread,
   which frees what it found.  The finish does nothing: it is there so
   the landing wakes the select (which also comes back every 100 ms
   while a lookup runs, as for getaddrinfo_a).  */

#include <sys/types.h>
#include <sys/socket.h>
#include <netdb.h>

struct nh_dns_job
{
  struct nh_wk_job job;
  char *host, *service;		/* one allocation */
  struct addrinfo hints;
  /* Worker output, read once the state is NH_WK_J_DONE.  */
  int err;
  struct addrinfo *res;
};

/* Counters for `nidhogg-workers--test-dns-info'.  */
static _Atomic int64_t nh_dns_started, nh_dns_dropped, nh_dns_live;

static void
nh_dns_run (struct nh_wk_job *wj)
{
  struct nh_dns_job *j = (struct nh_dns_job *) wj;
  nh_wk_pause_point (NH_WK_P_IN_LOOKUP);
  if (atomic_load (&wj->cancel))
    {
      j->err = EAI_FAIL;
      return;
    }
  j->err = getaddrinfo (j->host, j->service, &j->hints, &j->res);
  if (j->err)
    j->res = NULL;
}

static void
nh_dns_finish (struct nh_wk_job *wj)
{
  (void) wj;
}

static void
nh_dns_free (struct nh_wk_job *wj)
{
  struct nh_dns_job *j = (struct nh_dns_job *) wj;
  if (j->res)
    freeaddrinfo (j->res);
  free (j->host);
  free (j);
  atomic_fetch_sub (&nh_dns_live, 1);
}

static bool
nh_dns_switch_on (void)
{
  Lisp_Object v = find_symbol_value (Qnidhogg_workers_dns);
  return !BASE_EQ (v, Qunbound) && !NILP (v);
}

struct nh_dns_job *
nh_dns_start (const char *host, const char *service, int family,
	      int socktype)
{
  if (!nh_dns_switch_on ())
    return NULL;
  size_t hl = strlen (host), sl = strlen (service);
  struct nh_dns_job *j = calloc (1, sizeof *j);
  char *s = malloc (hl + 1 + sl + 1);
  if (!j || !s)
    {
      free (j);
      free (s);
      memory_full (sizeof *j + hl + sl + 2);
    }
  memcpy (s, host, hl + 1);
  memcpy (s + hl + 1, service, sl + 1);
  j->host = s;
  j->service = s + hl + 1;
  j->hints.ai_family = family;
  j->hints.ai_socktype = socktype;
  j->job.w = NH_WK_BLK;
  j->job.cls = NH_WK_CL_DNS;
  j->job.run = nh_dns_run;
  j->job.finish = nh_dns_finish;
  j->job.free = nh_dns_free;
  atomic_store (&j->job.refs, 1);
  atomic_fetch_add (&nh_dns_live, 1);
  if (!nh_wk_submit (&j->job))
    {
      nh_wk_job_unref (&j->job);
      return NULL;
    }
  atomic_fetch_add (&nh_dns_started, 1);
  /* The submitter's reference is the process's now.  */
  return j;
}

bool
nh_dns_poll (struct nh_dns_job *j, struct addrinfo **res)
{
  int st = atomic_load (&j->job.state);
  if (st == NH_WK_J_QUEUED || st == NH_WK_J_RUNNING)
    return true;
  *res = st == NH_WK_J_DONE && j->err == 0 ? j->res : NULL;
  if (*res)
    j->res = NULL;
  return false;
}

const char *
nh_dns_host (struct nh_dns_job *j)
{
  return j->host;
}

void
nh_dns_drop (struct nh_dns_job *j)
{
  int st = atomic_load (&j->job.state);
  if (st == NH_WK_J_QUEUED || st == NH_WK_J_RUNNING)
    {
      atomic_fetch_add (&nh_dns_dropped, 1);
      nh_wk_cancel (&j->job);
    }
  nh_wk_job_unref (&j->job);
}

DEFUN ("nidhogg-workers--test-dns-info", Fnidhogg_workers__test_dns_info,
       Snidhogg_workers__test_dns_info, 0, 0, 0,
       doc: /* Return the counters of the `:nowait' lookups on nh-blk; for tests.
A plist: `:started' (jobs submitted), `:dropped' (jobs a process let go
of before they ended) and `:live' (jobs not yet freed).  */)
  (void)
{
  return list (intern_c_string (":started"),
	       make_int (atomic_load (&nh_dns_started)),
	       intern_c_string (":dropped"),
	       make_int (atomic_load (&nh_dns_dropped)),
	       intern_c_string (":live"),
	       make_int (atomic_load (&nh_dns_live)));
}

static void
nh_dns_syms (void)
{
  DEFSYM (Qnidhogg_workers_dns, "nidhogg-workers-dns");
  DEFVAR_BOOL ("nidhogg-workers--test-glibc-dns-off", nh_dns_glibc_off,
	       doc: /* Non-nil: `:nowait' lookups take the path of a platform without getaddrinfo_a.
With `nidhogg-workers-dns' off (or a job that could not start) the
lookup is then synchronous, as on macOS, Android and musl.  No effect
where Emacs has no getaddrinfo_a.  Internal, for tests.  */);
  nh_dns_glibc_off = false;
  defsubr (&Snidhogg_workers__test_dns_info);
}

/* ==== End of host-name lookups ======================================= */

#endif /* not NH_WK_HARNESS */

#endif /* HAVE_NIDHOGG_WORKERS */

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

bool nh_save_keyboard_triggered;

bool
nh_save_auto_save_1 (struct buffer *b)
{
  (void) b;
  return false;
}

bool
nh_dns_start (struct Lisp_Process *p, const char *host, const char *service,
	      const struct addrinfo *hints)
{
  (void) p;
  (void) host;
  (void) service;
  (void) hints;
  return false;
}

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
	  if (errno == ENOENT || errno == ENOTDIR)
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
		  if (e != EINVAL && e != ENOENT && e != ENOTDIR)
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

bool
nh_save_name_in_flight (Lisp_Object name)
{
  (void) name;
  return false;
}

void
nh_blk_syms (void)
{
  nh_dir_syms ();
}

#endif /* not NH_WK_HARNESS */

#endif /* HAVE_NIDHOGG_WORKERS */

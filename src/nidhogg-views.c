/* Lisp door of Sjá, Nidhogg's native view model (nidhogg/sja): one generic call.

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

/* Sjá lives in the Rust library Glitnir carries.  This file passes an op
   and its arguments (nil, fixnums, strings) to sja_call and builds the
   Lisp value from the flat answer Sjá returns.  It knows no op: the
   verbs are Rust's, the named functions are defsubsts in
   lisp/nidhogg-views.el.  Sjá never calls Emacs.  */

#include <config.h>

#ifdef HAVE_GLITNIR

#include "lisp.h"
#include "coding.h"
#include "sja.h"

enum { SJA_MAX_ARGS = 16 };

/* The short strings answers gave, kept so the same bytes come back as the
   same string: a view's ids recur in every answer (a class step over
   6,400 rows answered 6,400 fresh strings a key, ~460 KB, enough for a GC
   pause to land on a keystroke).  A vector of SJA_STRINGS slots in sets of
   SJA_WAYS by the bytes' hash, the newest first; nil until the first
   answer.  A string Lisp changed since (its bytes, or text properties put
   on it) is not given again.  Bounded: SJA_STRINGS strings of at most
   SJA_STRING_MAX bytes.  */
enum { SJA_STRINGS = 1 << 15, SJA_WAYS = 4, SJA_STRING_MAX = 128 };
static Lisp_Object sja_strings;

/* The string of the LEN bytes of UTF-8 at P.  */
static Lisp_Object
sja_string (const unsigned char *p, size_t len)
{
  if (len > SJA_STRING_MAX)
    return make_string_from_utf8 ((const char *) p, len);
  if (NILP (sja_strings))
    sja_strings = make_nil_vector (SJA_STRINGS);
  /* The hash mixed (splitmix64's finalizer): its low bits come from a
     short string's first bytes alone, so ids that share a prefix would
     share a set.  */
  uint64_t z = hash_char_array ((const char *) p, len);
  z = (z ^ (z >> 30)) * UINT64_C (0xbf58476d1ce4e5b9);
  z = (z ^ (z >> 27)) * UINT64_C (0x94d049bb133111eb);
  z ^= z >> 31;
  ptrdiff_t i = z & (SJA_STRINGS - SJA_WAYS);
  ptrdiff_t j = i;
  Lisp_Object s = Qnil;
  for (; j < i + SJA_WAYS; j++)
    {
      Lisp_Object c = AREF (sja_strings, j);
      if (STRINGP (c) && SBYTES (c) == len && !string_intervals (c)
	  && memcmp (SDATA (c), p, len) == 0)
	{
	  s = c;
	  break;
	}
    }
  if (NILP (s))
    {
      s = make_string_from_utf8 ((const char *) p, len);
      j = i + SJA_WAYS - 1;	/* the oldest goes */
    }
  for (; j > i; j--)
    ASET (sja_strings, j, AREF (sja_strings, j - 1));
  ASET (sja_strings, i, s);
  return s;
}

/* The value of the answer tokens T[*AT..N), advancing *AT.  */
static Lisp_Object
sja_value (const sja_tok *t, size_t n, size_t *at)
{
  if (*at >= n)
    return Qnil;
  const sja_tok *k = &t[(*at)++];
  switch (k->tag)
    {
    case SJA_INT:
      return make_int (k->int_);
    case SJA_STR:
      return sja_string (k->ptr, k->len);
    case SJA_LIST:
    case SJA_DOTTED:
    case SJA_VECTOR:
      {
	ptrdiff_t m = k->int_;
	if (m < 0 || (size_t) m > n - *at)
	  return Qnil;
	if (k->tag == SJA_VECTOR)
	  {
	    Lisp_Object v = make_nil_vector (m);
	    for (ptrdiff_t i = 0; i < m; i++)
	      ASET (v, i, sja_value (t, n, at));
	    return v;
	  }
	Lisp_Object head = Qnil, tail = Qnil;
	for (ptrdiff_t i = 0; i < m; i++)
	  {
	    Lisp_Object x = sja_value (t, n, at);
	    if (k->tag == SJA_DOTTED && i == m - 1 && CONSP (tail))
	      XSETCDR (tail, x);
	    else
	      {
		Lisp_Object c = list1 (x);
		if (NILP (head))
		  head = c;
		else
		  XSETCDR (tail, c);
		tail = c;
	      }
	  }
	return head;
      }
    default:
      return Qnil;
    }
}

DEFUN ("nidhogg-view--call", Fnidhogg_view__call, Snidhogg_view__call, 1, MANY, 0,
       doc: /* Call Sjá, the native view model: OP, a fixnum, on ARGS.
Each of ARGS is nil, a fixnum or a string.  Return Sjá's answer: nil, an
integer, a string, or a list, dotted list or vector of those.  Bad
arguments signal `args-out-of-range'; a refused call signals `error'.
Use the functions of `nidhogg-views', not this one.
usage: (nidhogg-view--call OP &rest ARGS)  */)
  (ptrdiff_t nargs, Lisp_Object *args)
{
  CHECK_FIXNUM (args[0]);
  ptrdiff_t n = nargs - 1;
  if (n > SJA_MAX_ARGS)
    xsignal2 (Qargs_out_of_range, args[0], make_fixnum (n));
  sja_arg a[SJA_MAX_ARGS];
  for (ptrdiff_t i = 0; i < n; i++)
    {
      Lisp_Object x = args[i + 1];
      a[i] = (sja_arg) { .tag = SJA_NIL };
      if (FIXNUMP (x))
	a[i] = (sja_arg) { .tag = SJA_INT, .int_ = XFIXNUM (x) };
      else if (STRINGP (x))
	a[i] = (sja_arg) { .tag = SJA_STR, .ptr = SDATA (x), .len = SBYTES (x) };
      else if (!NILP (x))
	xsignal2 (Qargs_out_of_range, args[0], x);
    }
  /* No Lisp allocation until the answer is read: the strings stay put.  */
  const sja_tok *out = NULL;
  size_t nout = 0, at = 0;
  int32_t rc = sja_call (XFIXNUM (args[0]), a, n, &out, &nout);
  if (rc == SJA_E_ARG)
    xsignal2 (Qargs_out_of_range, args[0], Flist (n, args + 1));
  if (rc != SJA_OK)
    error ("nidhogg-view--call: Sjá refused the call (%d)", (int) rc);
  return sja_value (out, nout, &at);
}

/* Push LEN bytes at S, a hub's whole `view-model' line, to Sjá (the op
   `nidhogg-view-push' calls), for the framed reader (:raw-to-sja): the
   line never becomes a Lisp string.  Sjá parses it during the call and
   keeps nothing that points into S.  Return its answer as
   `nidhogg-view--call' would; on a refused call nil, with Sjá's code in
   *RC (else 0).  */
Lisp_Object
nidhogg_view_push_bytes (const unsigned char *s, size_t len, int *rc)
{
  sja_arg a = { .tag = SJA_STR, .ptr = s, .len = len };
  const sja_tok *out = NULL;
  size_t nout = 0, at = 0;
  *rc = sja_call (SJA_PUSH, &a, 1, &out, &nout);
  return *rc == SJA_OK ? sja_value (out, nout, &at) : Qnil;
}

void
syms_of_nidhogg_views (void)
{
  defsubr (&Snidhogg_view__call);
  sja_strings = Qnil;
  staticpro (&sja_strings);
}

#endif /* HAVE_GLITNIR */

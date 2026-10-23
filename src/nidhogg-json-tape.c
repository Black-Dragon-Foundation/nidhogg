/* Nidhogg's worker threads: the JSON tokenizer, tape and materializer.

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

/* The tokenizer (nh_json_tokenize) reads bytes and writes a tape; it
   never touches Lisp, so a worker can run it.  It follows json.c's
   rules byte for byte, including the byte offset at which each error is
   signalled, so the materializer can signal exactly what
   `json-parse-string' and `json-parse-buffer' signal.  The
   materializer (nh_json_materialize) runs on the main thread and builds
   what json_parse_value builds, in slices that GC may run between (its
   partial state is a rooted Lisp vector).  json.c is not edited.

   The tape is one malloc block:

     struct nh_tape | entries (16 bytes each) | key table | string arena

   A document is a pre-order list of entries; a container is its
   entry, its children, then an END entry.  Strings (and keys, and the
   digits of integers too large for 64 bits) are offsets into the arena;
   each distinct object key is stored once and named by its index in the
   key table.  A failed parse is a tape with no entries whose `err' and
   `err_off' say what json.c would signal and where.  */

#ifdef NH_WK_HARNESS
# ifndef _GNU_SOURCE
#  define _GNU_SOURCE 1
# endif
#else
# include <config.h>
#endif

#ifdef HAVE_NIDHOGG_WORKERS

#include <errno.h>
#include <locale.h>
#include <math.h>
#include <string.h>
#ifdef __APPLE__
# include <xlocale.h>
#endif

#include "nidhogg-workers.h"

#define NH_JSON_MAX_DEPTH 10000
#define NH_JSON_STOP_STRIDE 65536

/* The modes of nh_json_tokenize, NH_JSON_MODE_STRING and _BUFFER, are
   in nidhogg-workers.h (nidhogg-io.c's framings use them too).  */

enum nh_json_type
{
  NH_JSON_T_OBJECT = 1,	/* a = members; KEY, value pairs follow, then END.  */
  NH_JSON_T_ARRAY,	/* a = elements; values follow, then END.  */
  NH_JSON_T_STRING,	/* a = bytes, v.s.b = chars, v.s.c = arena offset.  */
  NH_JSON_T_KEY,	/* a = bytes, v.s.b = chars, v.s.c = key table index.  */
  NH_JSON_T_INTEGER,	/* v.u = magnitude, neg = sign.  */
  NH_JSON_T_BIGNUM,	/* a = bytes, v.s.c = arena offset of "[-]digits\0".  */
  NH_JSON_T_FLOAT,	/* v.d.  */
  NH_JSON_T_TRUE,
  NH_JSON_T_FALSE,
  NH_JSON_T_NULL,
  NH_JSON_T_END
};

/* The error to signal, in json.c's names.  */
enum nh_json_err
{
  NH_JSON_E_NONE,
  NH_JSON_E_PARSE,		/* json-parse-error */
  NH_JSON_E_EOF,		/* json-end-of-file */
  NH_JSON_E_TRAILING,		/* json-trailing-content */
  NH_JSON_E_TOO_DEEP,		/* json-object-too-deep */
  NH_JSON_E_UTF8,		/* json-utf8-decode-error */
  NH_JSON_E_SURROGATE,		/* json-invalid-surrogate-error */
  NH_JSON_E_RANGE,		/* json-number-out-of-range-error */
  NH_JSON_E_ESCAPE,		/* json-escape-sequence-error */
  NH_JSON_E_GENERIC		/* json-error */
};

struct nh_json_ent
{
  uint8_t type;
  uint8_t neg;
  uint16_t pad;
  uint32_t a;
  union
  {
    uint64_t u;
    double d;
    struct { uint32_t b, c; } s;
  } v;
};

_Static_assert (sizeof (struct nh_json_ent) == 16, "tape entries are 16 bytes");

struct nh_json_key
{
  uint32_t off, len, nchars;
};

struct nh_tape
{
  int err;			/* enum nh_json_err; NONE for a value.  */
  size_t err_off;		/* Byte offset where json.c signals.  */
  size_t consumed;		/* Bytes the value (and its whitespace) took.  */
  size_t n_ent, n_keys, arena_len;
  struct nh_json_ent *ents;
  struct nh_json_key *keys;
  unsigned char *arena;
};

struct nh_json_frame
{
  bool is_obj;
  uint32_t ent;
  uint32_t count;
};

struct nh_json_tk
{
  const unsigned char *begin, *cur, *end, *next_check;
  const _Atomic bool *stop;
  int err;
  bool oom, stopped;
  int depth;
  struct nh_json_ent *ents;
  size_t n_ent, cap_ent;
  unsigned char *arena;
  size_t n_ar, cap_ar;
  struct nh_json_key *keys;
  size_t n_keys, cap_keys;
  uint32_t *htab;		/* key index + 1, 0 = empty.  */
  size_t hcap;
  struct nh_json_frame *frames;
  size_t n_fr, cap_fr;
  char *numbuf;
  size_t cap_num;
};

/* The same table as json.c's json_plain_char.  */
static inline bool
nh_json_plain (int c)
{
  return c >= 0x20 && c < 0x80 && c != '"' && c != '\\';
}

static locale_t nh_json_c_locale;
static pthread_once_t nh_json_locale_once = PTHREAD_ONCE_INIT;

static void
nh_json_locale_init (void)
{
  nh_json_c_locale = newlocale (LC_ALL_MASK, "C", (locale_t) 0);
}

static bool
nh_json_grow (void **ptr, size_t *cap, size_t need, size_t elt)
{
  if (need <= *cap)
    return true;
  size_t ncap = *cap < 16 ? 16 : *cap;
  while (ncap < need)
    {
      if (ncap > SIZE_MAX / 2)
	return false;
      ncap *= 2;
    }
  if (ncap > SIZE_MAX / elt)
    return false;
  void *n = realloc (*ptr, ncap * elt);
  if (!n)
    return false;
  *ptr = n;
  *cap = ncap;
  return true;
}

#define NH_JSON_FAIL(p, e) \
  do { (p)->err = (e); return false; } while (0)

static void
nh_json_next_check (struct nh_json_tk *p)
{
  size_t rem = p->end - p->cur;
  p->next_check = rem > NH_JSON_STOP_STRIDE ? p->cur + NH_JSON_STOP_STRIDE
					    : p->end;
}

/* json_input_get: the next byte, or json-end-of-file.  Every 64 KB
   the stop flag is looked at.  */
static bool
tk_get (struct nh_json_tk *p, int *c)
{
  if (p->cur >= p->end)
    NH_JSON_FAIL (p, NH_JSON_E_EOF);
  if (p->cur >= p->next_check)
    {
      if (p->stop && atomic_load_explicit (p->stop, memory_order_relaxed))
	{
	  p->stopped = true;
	  return false;
	}
      nh_json_next_check (p);
    }
  *c = *p->cur++;
  return true;
}

/* json_input_get_if_possible: -1 at the end, without moving.  */
static int
tk_get_opt (struct nh_json_tk *p)
{
  return p->cur < p->end ? *p->cur++ : -1;
}

static bool
nh_json_ws (int c)
{
  return c == 0x20 || c == 0x09 || c == 0x0d || c == 0x0a;
}

static bool
tk_skip_ws (struct nh_json_tk *p, int *c)
{
  for (;;)
    {
      if (!tk_get (p, c))
	return false;
      if (!nh_json_ws (*c))
	return true;
    }
}

static bool
tk_ent (struct nh_json_tk *p, int type, int neg, uint32_t a, uint64_t u)
{
  if (p->n_ent >= UINT32_MAX
      || !nh_json_grow ((void **) &p->ents, &p->cap_ent, p->n_ent + 1,
			sizeof *p->ents))
    {
      p->oom = true;
      return false;
    }
  struct nh_json_ent *e = &p->ents[p->n_ent++];
  e->type = type;
  e->neg = neg;
  e->pad = 0;
  e->a = a;
  e->v.u = u;
  return true;
}

static bool
tk_ar_room (struct nh_json_tk *p, size_t n)
{
  if (n > UINT32_MAX - 8 - p->n_ar
      || !nh_json_grow ((void **) &p->arena, &p->cap_ar, p->n_ar + n, 1))
    {
      p->oom = true;
      return false;
    }
  return true;
}

static bool
tk_ar_put (struct nh_json_tk *p, unsigned char c)
{
  if (!tk_ar_room (p, 1))
    return false;
  p->arena[p->n_ar++] = c;
  return true;
}

static bool
tk_ar_put_n (struct nh_json_tk *p, const unsigned char *s, size_t n)
{
  if (!tk_ar_room (p, n))
    return false;
  memcpy (p->arena + p->n_ar, s, n);
  p->n_ar += n;
  return true;
}

static int
tk_hex (int c)
{
  if (c >= '0' && c <= '9')
    return c - '0';
  if (c >= 'A' && c <= 'F')
    return c - 'A' + 10;
  if (c >= 'a' && c <= 'f')
    return c - 'a' + 10;
  return -1;
}

static bool
tk_unicode (struct nh_json_tk *p, int *num)
{
  int v = 0;
  for (int i = 0; i < 4; i++)
    {
      int c;
      if (!tk_get (p, &c))
	return false;
      int h = tk_hex (c);
      if (h < 0)
	NH_JSON_FAIL (p, NH_JSON_E_ESCAPE);
      v = v << 4 | h;
    }
  *num = v;
  return true;
}

/* json_parse_string: decode a string into the arena (the opening quote
   is consumed).  */
static bool
tk_string (struct nh_json_tk *p, size_t *start, size_t *nbytes,
	   size_t *nchars)
{
  *start = p->n_ar;
  size_t chars_delta = 0;
  for (;;)
    {
      const unsigned char *run = p->cur;
      while (p->cur < p->end && p->cur < p->next_check
	     && nh_json_plain (*p->cur))
	p->cur++;
      if (p->cur > run && !tk_ar_put_n (p, run, p->cur - run))
	return false;

      int c;
      if (!tk_get (p, &c))
	return false;
      if (nh_json_plain (c))
	{
	  if (!tk_ar_put (p, c))
	    return false;
	  continue;
	}
      if (c == '"')
	{
	  *nbytes = p->n_ar - *start;
	  *nchars = *nbytes - chars_delta;
	  return true;
	}
      if (c & 0x80)
	{
	  /* Strict UTF-8, checked in json.c's order.  */
	  if (!tk_ar_put (p, c))
	    return false;
	  int c1;
	  if (!tk_get (p, &c1))
	    return false;
	  if ((c1 & 0xc0) != 0x80)
	    NH_JSON_FAIL (p, NH_JSON_E_UTF8);
	  if (!tk_ar_put (p, c1))
	    return false;
	  if (c <= 0xc1)
	    NH_JSON_FAIL (p, NH_JSON_E_UTF8);
	  else if (c <= 0xdf)
	    chars_delta += 1;
	  else if (c <= 0xef)
	    {
	      int c2;
	      if (!tk_get (p, &c2))
		return false;
	      if ((c2 & 0xc0) != 0x80)
		NH_JSON_FAIL (p, NH_JSON_E_UTF8);
	      int v = ((c & 0x0f) << 12) + ((c1 & 0x3f) << 6) + (c2 & 0x3f);
	      if (v < 0x800 || (v >= 0xd800 && v <= 0xdfff))
		NH_JSON_FAIL (p, NH_JSON_E_UTF8);
	      if (!tk_ar_put (p, c2))
		return false;
	      chars_delta += 2;
	    }
	  else if (c <= 0xf7)
	    {
	      int c2, c3;
	      if (!tk_get (p, &c2) || !tk_get (p, &c3))
		return false;
	      if ((c2 & 0xc0) != 0x80 || (c3 & 0xc0) != 0x80)
		NH_JSON_FAIL (p, NH_JSON_E_UTF8);
	      int v = (((c & 0x07) << 18) + ((c1 & 0x3f) << 12)
		       + ((c2 & 0x3f) << 6) + (c3 & 0x3f));
	      if (v < 0x10000 || v > 0x10ffff)
		NH_JSON_FAIL (p, NH_JSON_E_UTF8);
	      if (!tk_ar_put (p, c2) || !tk_ar_put (p, c3))
		return false;
	      chars_delta += 3;
	    }
	  else
	    NH_JSON_FAIL (p, NH_JSON_E_UTF8);
	}
      else if (c == '\\')
	{
	  if (!tk_get (p, &c))
	    return false;
	  unsigned char out = 0;
	  switch (c)
	    {
	    case '"': out = '"'; break;
	    case '\\': out = '\\'; break;
	    case '/': out = '/'; break;
	    case 'b': out = '\b'; break;
	    case 'f': out = '\f'; break;
	    case 'n': out = '\n'; break;
	    case 'r': out = '\r'; break;
	    case 't': out = '\t'; break;
	    case 'u':
	      {
		int num;
		if (!tk_unicode (p, &num))
		  return false;
		if (num >= 0xd800 && num < 0xdc00)
		  {
		    int b1, b2, num2;
		    if (!tk_get (p, &b1))
		      return false;
		    if (b1 != '\\')
		      NH_JSON_FAIL (p, NH_JSON_E_SURROGATE);
		    if (!tk_get (p, &b2))
		      return false;
		    if (b2 != 'u')
		      NH_JSON_FAIL (p, NH_JSON_E_SURROGATE);
		    if (!tk_unicode (p, &num2))
		      return false;
		    if (num2 < 0xdc00 || num2 >= 0xe000)
		      NH_JSON_FAIL (p, NH_JSON_E_SURROGATE);
		    num = 0x10000 + ((num - 0xd800) << 10 | (num2 - 0xdc00));
		  }
		else if (num >= 0xdc00 && num < 0xe000)
		  NH_JSON_FAIL (p, NH_JSON_E_SURROGATE);

		unsigned char u[4];
		int nu;
		if (num < 0x80)
		  {
		    u[0] = num;
		    nu = 1;
		  }
		else if (num < 0x800)
		  {
		    u[0] = 0xc0 | num >> 6;
		    u[1] = 0x80 | (num & 0x3f);
		    nu = 2;
		  }
		else if (num < 0x10000)
		  {
		    u[0] = 0xe0 | num >> 12;
		    u[1] = 0x80 | ((num >> 6) & 0x3f);
		    u[2] = 0x80 | (num & 0x3f);
		    nu = 3;
		  }
		else
		  {
		    u[0] = 0xf0 | num >> 18;
		    u[1] = 0x80 | ((num >> 12) & 0x3f);
		    u[2] = 0x80 | ((num >> 6) & 0x3f);
		    u[3] = 0x80 | (num & 0x3f);
		    nu = 4;
		  }
		if (!tk_ar_put_n (p, u, nu))
		  return false;
		chars_delta += nu - 1;
		continue;
	      }
	    default:
	      NH_JSON_FAIL (p, NH_JSON_E_ESCAPE);
	    }
	  if (!tk_ar_put (p, out))
	    return false;
	}
      else
	NH_JSON_FAIL (p, NH_JSON_E_PARSE);
    }
}

static uint32_t
nh_json_hash (const unsigned char *s, size_t n)
{
  uint32_t h = 2166136261u;
  for (size_t i = 0; i < n; i++)
    h = (h ^ s[i]) * 16777619u;
  return h;
}

/* Store the key just decoded at ARENA[START, START+NBYTES) once: if it
   is already in the key table the arena is rewound.  Return its index.  */
static bool
tk_key (struct nh_json_tk *p, size_t start, size_t nbytes, size_t nchars,
	uint32_t *id)
{
  if ((p->n_keys + 1) * 2 > p->hcap)
    {
      size_t ncap = p->hcap ? p->hcap * 2 : 64;
      uint32_t *nt = calloc (ncap, sizeof *nt);
      if (!nt)
	{
	  p->oom = true;
	  return false;
	}
      for (size_t k = 0; k < p->n_keys; k++)
	{
	  size_t i = nh_json_hash (p->arena + p->keys[k].off,
				   p->keys[k].len) & (ncap - 1);
	  while (nt[i])
	    i = (i + 1) & (ncap - 1);
	  nt[i] = k + 1;
	}
      free (p->htab);
      p->htab = nt;
      p->hcap = ncap;
    }
  size_t i = nh_json_hash (p->arena + start, nbytes) & (p->hcap - 1);
  while (p->htab[i])
    {
      const struct nh_json_key *k = &p->keys[p->htab[i] - 1];
      /* An empty key may come before the arena exists (NULL):
	 memcmp must not see it, even for 0 bytes.  */
      if (k->len == nbytes
	  && (nbytes == 0
	      || memcmp (p->arena + k->off, p->arena + start, nbytes) == 0))
	{
	  *id = p->htab[i] - 1;
	  p->n_ar = start;
	  return true;
	}
      i = (i + 1) & (p->hcap - 1);
    }
  if (!nh_json_grow ((void **) &p->keys, &p->cap_keys, p->n_keys + 1,
		     sizeof *p->keys))
    {
      p->oom = true;
      return false;
    }
  *id = p->n_keys++;
  p->keys[*id] = (struct nh_json_key) { start, nbytes, nchars };
  p->htab[i] = *id + 1;
  return true;
}

static bool
tk_emit_string (struct nh_json_tk *p, bool key)
{
  size_t start, nbytes, nchars;
  if (!tk_string (p, &start, &nbytes, &nchars))
    return false;
  uint32_t third = start;
  if (key && !tk_key (p, start, nbytes, nchars, &third))
    return false;
  if (!tk_ent (p, key ? NH_JSON_T_KEY : NH_JSON_T_STRING, 0, nbytes, 0))
    return false;
  struct nh_json_ent *e = &p->ents[p->n_ent - 1];
  e->v.s.b = nchars;
  e->v.s.c = third;
  return true;
}

static bool
tk_emit_int (struct nh_json_tk *p, const unsigned char *start, bool neg,
	     bool overflow, uint64_t integer)
{
  if (!overflow && (!neg || integer <= (uint64_t) INT64_MAX + 1))
    return tk_ent (p, NH_JSON_T_INTEGER, neg, 0, integer);
  size_t n = p->cur - start;
  size_t off = p->n_ar;
  if (!tk_ar_put_n (p, start, n) || !tk_ar_put (p, 0))
    return false;
  if (!tk_ent (p, NH_JSON_T_BIGNUM, 0, n, 0))
    return false;
  p->ents[p->n_ent - 1].v.s.c = off;
  return true;
}

static bool
tk_emit_float (struct nh_json_tk *p, const unsigned char *start)
{
  size_t n = p->cur - start;
  if (!nh_json_grow ((void **) &p->numbuf, &p->cap_num, n + 1, 1))
    {
      p->oom = true;
      return false;
    }
  memcpy (p->numbuf, start, n);
  p->numbuf[n] = 0;
  pthread_once (&nh_json_locale_once, nh_json_locale_init);
  if (!nh_json_c_locale)
    {
      p->oom = true;
      return false;
    }
  errno = 0;
  char *e;
  double value = strtod_l (p->numbuf, &e, nh_json_c_locale);
  if (errno != 0 && (value == HUGE_VAL || value == -HUGE_VAL))
    NH_JSON_FAIL (p, NH_JSON_E_RANGE);
  if (e != p->numbuf + n)
    NH_JSON_FAIL (p, NH_JSON_E_GENERIC);
  if (!tk_ent (p, NH_JSON_T_FLOAT, 0, 0, 0))
    return false;
  p->ents[p->n_ent - 1].v.d = value;
  return true;
}

static inline bool
nh_json_digit (int c)
{
  return c >= '0' && c <= '9';
}

/* json_parse_number: C, the first byte, is consumed.  */
static bool
tk_number (struct nh_json_tk *p, int c)
{
  const unsigned char *start = p->cur - 1;
  bool neg = false;
  if (c == '-')
    {
      neg = true;
      if (!tk_get (p, &c))
	return false;
    }
  if (!nh_json_digit (c))
    NH_JSON_FAIL (p, NH_JSON_E_PARSE);

  uint64_t integer = c - '0';
  bool overflow = false;
  if (integer == 0)
    {
      if (p->cur >= p->end)
	return tk_emit_int (p, start, neg, overflow, integer);
      if (!tk_get (p, &c))
	return false;
    }
  else
    for (;;)
      {
	if (p->cur >= p->end)
	  return tk_emit_int (p, start, neg, overflow, integer);
	if (!tk_get (p, &c))
	  return false;
	if (!nh_json_digit (c))
	  break;
	overflow |= __builtin_mul_overflow (integer, 10, &integer);
	overflow |= __builtin_add_overflow (integer, c - '0', &integer);
      }

  bool is_float = false;
  if (c == '.')
    {
      is_float = true;
      if (!tk_get (p, &c))
	return false;
      if (!nh_json_digit (c))
	NH_JSON_FAIL (p, NH_JSON_E_PARSE);
      for (;;)
	{
	  if (p->cur >= p->end)
	    return tk_emit_float (p, start);
	  if (!tk_get (p, &c))
	    return false;
	  if (!nh_json_digit (c))
	    break;
	}
    }
  if (c == 'e' || c == 'E')
    {
      is_float = true;
      if (!tk_get (p, &c))
	return false;
      if (c == '-' || c == '+')
	{
	  if (!tk_get (p, &c))
	    return false;
	}
      if (!nh_json_digit (c))
	NH_JSON_FAIL (p, NH_JSON_E_PARSE);
      for (;;)
	{
	  if (p->cur >= p->end)
	    return tk_emit_float (p, start);
	  if (!tk_get (p, &c))
	    return false;
	  if (!nh_json_digit (c))
	    break;
	}
    }
  p->cur--;			/* put back the byte after the number */
  return is_float ? tk_emit_float (p, start)
		  : tk_emit_int (p, start, neg, overflow, integer);
}

static bool
nh_json_token_char (int c)
{
  return ((c >= 'A' && c <= 'Z') || (c >= 'a' && c <= 'z')
	  || (c >= '0' && c <= '9') || c == '-');
}

/* The literals: json.c consumes a byte before comparing it.  */
static bool
tk_literal (struct nh_json_tk *p, const char *rest, int type)
{
  for (; *rest; rest++)
    if (tk_get_opt (p) != (unsigned char) *rest)
      NH_JSON_FAIL (p, NH_JSON_E_PARSE);
  int c2 = tk_get_opt (p);
  if (nh_json_token_char (c2))
    NH_JSON_FAIL (p, NH_JSON_E_PARSE);
  if (c2 >= 0)
    p->cur--;
  return tk_ent (p, type, 0, 0, 0);
}

static bool
tk_push (struct nh_json_tk *p, bool is_obj, size_t ent)
{
  if (!nh_json_grow ((void **) &p->frames, &p->cap_fr, p->n_fr + 1,
		     sizeof *p->frames))
    {
      p->oom = true;
      return false;
    }
  p->frames[p->n_fr++] = (struct nh_json_frame) { is_obj, ent, 0 };
  return true;
}

/* json_parse, without recursion (depth 10,000 must not need the
   worker's 1 MB stack).  */
static bool
tk_value (struct nh_json_tk *p)
{
  int c;
  if (!tk_skip_ws (p, &c))
    return false;

  for (;;)
    {
    value:
      switch (c)
	{
	case '{':
	case '[':
	  {
	    bool obj = c == '{';
	    if (!tk_skip_ws (p, &c))
	      return false;
	    if (c == (obj ? '}' : ']'))
	      {
		if (!tk_ent (p, obj ? NH_JSON_T_OBJECT : NH_JSON_T_ARRAY, 0,
			     0, 0)
		    || !tk_ent (p, NH_JSON_T_END, 0, 0, 0))
		  return false;
		goto after_value;
	      }
	    if (--p->depth < 0)
	      NH_JSON_FAIL (p, NH_JSON_E_TOO_DEEP);
	    if (!tk_push (p, obj, p->n_ent)
		|| !tk_ent (p, obj ? NH_JSON_T_OBJECT : NH_JSON_T_ARRAY, 0,
			    0, 0))
	      return false;
	    if (obj)
	      goto member;
	    goto value;
	  }
	case '"':
	  if (!tk_emit_string (p, false))
	    return false;
	  break;
	case '0': case '1': case '2': case '3': case '4':
	case '5': case '6': case '7': case '8': case '9':
	case '-':
	  if (!tk_number (p, c))
	    return false;
	  break;
	case 't':
	  if (!tk_literal (p, "rue", NH_JSON_T_TRUE))
	    return false;
	  break;
	case 'f':
	  if (!tk_literal (p, "alse", NH_JSON_T_FALSE))
	    return false;
	  break;
	case 'n':
	  if (!tk_literal (p, "ull", NH_JSON_T_NULL))
	    return false;
	  break;
	default:
	  NH_JSON_FAIL (p, NH_JSON_E_PARSE);
	}

    after_value:
      for (;;)
	{
	  if (p->n_fr == 0)
	    return true;
	  struct nh_json_frame *f = &p->frames[p->n_fr - 1];
	  f->count++;
	  if (!tk_skip_ws (p, &c))
	    return false;
	  if (c == (f->is_obj ? '}' : ']'))
	    {
	      p->depth++;
	      p->ents[f->ent].a = f->count;
	      p->n_fr--;
	      if (!tk_ent (p, NH_JSON_T_END, 0, 0, 0))
		return false;
	      continue;		/* the container was a value of its parent */
	    }
	  if (c != ',')
	    NH_JSON_FAIL (p, NH_JSON_E_PARSE);
	  if (!tk_skip_ws (p, &c))
	    return false;
	  if (f->is_obj)
	    goto member;
	  goto value;
	}

    member:
      if (c != '"')
	NH_JSON_FAIL (p, NH_JSON_E_PARSE);
      if (!tk_emit_string (p, true))
	return false;
      if (!tk_skip_ws (p, &c))
	return false;
      if (c != ':')
	NH_JSON_FAIL (p, NH_JSON_E_PARSE);
      if (!tk_skip_ws (p, &c))
	return false;
    }
}

static void
tk_free (struct nh_json_tk *p)
{
  free (p->ents);
  free (p->arena);
  free (p->keys);
  free (p->htab);
  free (p->frames);
  free (p->numbuf);
}

#define NH_JSON_ALIGN16(n) (((n) + 15) & ~(size_t) 15)

/* Tokenize BYTES[0, N) in MODE.  The result is one malloc block (free
   it with free) holding either the document or the error json.c would
   signal.  NULL when STOP became true (checked every 64 KB) or memory
   ran out.  */
struct nh_tape *
nh_json_tokenize (const unsigned char *bytes, size_t n, int mode,
		  const _Atomic bool *stop)
{
  struct nh_json_tk p;
  memset (&p, 0, sizeof p);
  p.begin = p.cur = bytes;
  p.end = bytes + n;
  p.stop = stop;
  p.depth = NH_JSON_MAX_DEPTH;
  nh_json_next_check (&p);
  if (stop && atomic_load_explicit (stop, memory_order_relaxed))
    return NULL;

  bool ok = tk_value (&p);
  if (ok && mode == NH_JSON_MODE_STRING)
    {
      int c = -1;
      while (p.cur < p.end && nh_json_ws (c = *p.cur++))
	c = -1;
      if (c >= 0)
	{
	  p.err = NH_JSON_E_TRAILING;
	  ok = false;
	}
    }
  if (p.oom || p.stopped)
    {
      tk_free (&p);
      return NULL;
    }

  size_t ents_sz = ok ? p.n_ent * sizeof *p.ents : 0;
  size_t keys_sz = ok ? NH_JSON_ALIGN16 (p.n_keys * sizeof *p.keys) : 0;
  size_t arena_sz = ok ? p.n_ar : 0;
  size_t hdr = NH_JSON_ALIGN16 (sizeof (struct nh_tape));
  struct nh_tape *t = malloc (hdr + ents_sz + keys_sz + arena_sz);
  if (!t)
    {
      tk_free (&p);
      return NULL;
    }
  memset (t, 0, sizeof *t);
  unsigned char *base = (unsigned char *) t;
  if (ok)
    {
      t->n_ent = p.n_ent;
      t->n_keys = p.n_keys;
      t->arena_len = p.n_ar;
      t->ents = (struct nh_json_ent *) (base + hdr);
      t->keys = (struct nh_json_key *) (base + hdr + ents_sz);
      t->arena = base + hdr + ents_sz + keys_sz;
      memcpy (t->ents, p.ents, ents_sz);
      if (p.n_keys)
	memcpy (t->keys, p.keys, p.n_keys * sizeof *p.keys);
      if (p.n_ar)
	memcpy (t->arena, p.arena, p.n_ar);
      t->consumed = p.cur - p.begin;
    }
  else
    {
      t->err = p.err;
      t->err_off = p.cur - p.begin;
    }
  tk_free (&p);
  return t;
}

#ifndef NH_WK_HARNESS

#include <time.h>

#include "buffer.h"
#include "nidhogg-workers-phase.h"

/* What the materializer needs besides the tape: json.c's options, and
   the source bytes, for the line and position of an error.  */
struct nh_mat_conf
{
  enum { NH_MAT_OBJ_HASH, NH_MAT_OBJ_ALIST, NH_MAT_OBJ_PLIST } object_type;
  enum { NH_MAT_ARR_ARRAY, NH_MAT_ARR_LIST } array_type;
  Lisp_Object null_object;
  Lisp_Object false_object;
};

/* A materialization in progress.  The struct lives in C memory, which
   GC does not scan, so every Lisp object it needs between slices is in
   STATE, a vector on nh_json_in_flight (staticpro'd):

     RESULT	the value, once the document's last entry is built;
     WORK	the elements collected so far for every open container,
		one after the other, the innermost last (TOP are used);
     KEYS	the interned key of each key-table index, or 0 until the
		first occurrence interns it (alist and plist only);
     NULL, FALSE  the :null-object and :false-object.

   A container is built when its END entry is reached, from its slice
   of WORK, so nothing partial is ever a Lisp object Lisp can see.  */
enum
{
  NH_MAT_S_RESULT,
  NH_MAT_S_WORK,
  NH_MAT_S_KEYS,
  NH_MAT_S_NULL,
  NH_MAT_S_FALSE,
  NH_MAT_S_N
};

struct nh_mat_open
{
  size_t ent;			/* The container's entry.  */
  ptrdiff_t start;		/* Where its elements start in WORK.  */
};

struct nh_mat
{
  const struct nh_tape *tape;
  struct nh_mat_conf conf;
  size_t next;			/* The next entry to build.  */
  Lisp_Object state;		/* The vector above, or nil once unlinked.  */
  ptrdiff_t top;		/* Slots of WORK in use.  */
  struct nh_mat_open *open;	/* The open containers, innermost last.  */
  ptrdiff_t n_open, cap_open;
  bool in_slice, failed, done;
  int phase_depth;
  const unsigned char *src;
  size_t nsrc;
  bool src_multibyte;
};

/* Every materialization between nh_mat_init and nh_mat_release (or a
   failed slice): a list of their STATE vectors.  */
static Lisp_Object nh_json_in_flight;

static uint64_t
nh_json_now (void)
{
  struct timespec ts;
  clock_gettime (CLOCK_MONOTONIC, &ts);
  return (uint64_t) ts.tv_sec * 1000000000 + ts.tv_nsec;
}

/* Start materializing TAPE with CONF into M; the options' objects are
   rooted from here on.  */
static void
nh_mat_init (struct nh_mat *m, const struct nh_tape *tape,
	     const struct nh_mat_conf *conf)
{
  m->tape = tape;
  m->conf = *conf;
  m->next = 0;
  m->state = Qnil;
  m->top = 0;
  m->open = NULL;
  m->n_open = m->cap_open = 0;
  m->in_slice = m->failed = m->done = false;
  m->phase_depth = -1;
  Lisp_Object s = make_nil_vector (NH_MAT_S_N);
  ASET (s, NH_MAT_S_WORK, make_nil_vector (16));
  ASET (s, NH_MAT_S_KEYS,
	make_vector (conf->object_type == NH_MAT_OBJ_HASH ? 0 : tape->n_keys,
		     make_fixnum (0)));
  ASET (s, NH_MAT_S_NULL, conf->null_object);
  ASET (s, NH_MAT_S_FALSE, conf->false_object);
  m->state = s;
  nh_json_in_flight = Fcons (s, nh_json_in_flight);
}

static void
nh_mat_unlink (struct nh_mat *m)
{
  if (!NILP (m->state))
    {
      nh_json_in_flight = Fdelq (m->state, nh_json_in_flight);
      m->state = Qnil;
    }
}

/* Drop M's Lisp state and its stack.  The tape stays the caller's.  */
static void
nh_mat_release (struct nh_mat *m)
{
  nh_mat_unlink (m);
  xfree (m->open);
  m->open = NULL;
  m->n_open = m->cap_open = 0;
}

/* The value, once nh_json_materialize has answered true.  */
static Lisp_Object
nh_mat_result (struct nh_mat *m)
{
  return AREF (m->state, NH_MAT_S_RESULT);
}

static Lisp_Object
nh_mat_error_symbol (int err)
{
  switch (err)
    {
    case NH_JSON_E_PARSE: return Qjson_parse_error;
    case NH_JSON_E_EOF: return Qjson_end_of_file;
    case NH_JSON_E_TRAILING: return Qjson_trailing_content;
    case NH_JSON_E_TOO_DEEP: return Qjson_object_too_deep;
    case NH_JSON_E_UTF8: return Qjson_utf8_decode_error;
    case NH_JSON_E_SURROGATE: return Qjson_invalid_surrogate_error;
    case NH_JSON_E_RANGE: return Qjson_number_out_of_range;
    case NH_JSON_E_ESCAPE: return Qjson_escape_sequence_error;
    default: return Qjson_error;
    }
}

/* Signal what json.c signals: (ERROR LINE nil POSITION), the line
   counted from 1 (deprecated there), the position in characters for a
   multibyte source and in bytes otherwise.  */
static AVOID
nh_mat_signal (struct nh_mat *m)
{
  size_t off = m->tape->err_off;
  eassert (off <= m->nsrc);
  ptrdiff_t line = 1, pos = 0;
  for (size_t i = 0; i < off; i++)
    {
      line += m->src[i] == '\n';
      pos += !m->src_multibyte || (m->src[i] & 0xc0) != 0x80;
    }
  xsignal3 (nh_mat_error_symbol (m->tape->err), INT_TO_INTEGER (line), Qnil,
	    INT_TO_INTEGER (pos));
}

/* The key of entry E: a fresh string for hash tables (as json.c), else
   the symbol, interned once per message (the key table's index caches
   it in KEYS).  */
static Lisp_Object
nh_mat_key (struct nh_mat *m, const struct nh_json_ent *e)
{
  const struct nh_tape *t = m->tape;
  const char *s = (const char *) t->arena + t->keys[e->v.s.c].off;
  ptrdiff_t nbytes = e->a, nchars = e->v.s.b;
  if (m->conf.object_type == NH_MAT_OBJ_HASH)
    return make_multibyte_string (s, nchars, nbytes);
  Lisp_Object keys = AREF (m->state, NH_MAT_S_KEYS);
  Lisp_Object sym = AREF (keys, e->v.s.c);
  if (!FIXNUMP (sym))
    return sym;
  nidhogg_workers__test_intern_count++;
  if (m->conf.object_type == NH_MAT_OBJ_ALIST)
    sym = intern_c_multibyte (s, nchars, nbytes);
  else
    {
      USE_SAFE_ALLOCA;
      char *buf = SAFE_ALLOCA (nbytes + 1);
      buf[0] = ':';
      memcpy (buf + 1, s, nbytes);
      sym = intern_c_multibyte (buf, nchars + 1, nbytes + 1);
      SAFE_FREE ();
    }
  ASET (keys, e->v.s.c, sym);
  return sym;
}

/* A built value: the document's (RESULT) or the innermost open
   container's next element (WORK).  */
static void
nh_mat_put (struct nh_mat *m, Lisp_Object v)
{
  if (m->n_open == 0)
    {
      ASET (m->state, NH_MAT_S_RESULT, v);
      return;
    }
  Lisp_Object work = AREF (m->state, NH_MAT_S_WORK);
  if (m->top == ASIZE (work))
    {
      work = larger_vector (work, 1, -1);
      ASET (m->state, NH_MAT_S_WORK, work);
    }
  ASET (work, m->top++, v);
}

/* The innermost container is complete: build it from its elements, as
   json_parse_array and json_parse_object do, and clear their slots.  */
static Lisp_Object
nh_mat_close (struct nh_mat *m)
{
  struct nh_mat_open o = m->open[--m->n_open];
  const struct nh_json_ent *e = &m->tape->ents[o.ent];
  Lisp_Object work = AREF (m->state, NH_MAT_S_WORK);
  Lisp_Object *w = XVECTOR (work)->contents + o.start;
  ptrdiff_t n = m->top - o.start;
  Lisp_Object r = Qnil;
  if (e->type == NH_JSON_T_ARRAY)
    r = (m->conf.array_type == NH_MAT_ARR_ARRAY ? Fvector (n, w)
	 : Flist (n, w));
  else
    switch (m->conf.object_type)
      {
      case NH_MAT_OBJ_HASH:
	{
	  r = make_hash_table (&hashtest_equal, n / 2, Weak_None);
	  struct Lisp_Hash_Table *h = XHASH_TABLE (r);
	  for (ptrdiff_t i = 0; i < n; i += 2)
	    {
	      hash_hash_t hash;
	      ptrdiff_t idx = hash_find_get_hash (h, w[i], &hash);
	      if (idx < 0)
		hash_put (h, w[i], w[i + 1], hash);
	      else
		set_hash_value_slot (h, idx, w[i + 1]);
	    }
	  break;
	}
      case NH_MAT_OBJ_ALIST:
	for (ptrdiff_t i = n - 2; i >= 0; i -= 2)
	  r = Fcons (Fcons (w[i], w[i + 1]), r);
	break;
      case NH_MAT_OBJ_PLIST:
	r = Flist (n, w);
	break;
      }
  memclear (w, n * word_size);
  m->top = o.start;
  return r;
}

/* Build entries from M->next until the document is complete (true) or,
   at a check every `nidhogg-workers--test-slice-entries' entries, the
   slice has run BUDGET_NS or longer (false).  */
static bool
nh_mat_slice (struct nh_mat *m, int64_t budget_ns)
{
  const struct nh_tape *t = m->tape;
  EMACS_INT stride = max (1, nidhogg_workers__test_slice_entries);
  EMACS_INT left = stride;
  uint64_t t0 = nh_json_now ();
  while (m->next < t->n_ent)
    {
      if ((EMACS_INT) m->next == nidhogg_workers__test_json_fail_at)
	xsignal (Qnil, Vmemory_signal_data);
      const struct nh_json_ent *e = &t->ents[m->next];
      Lisp_Object v = Qnil;
      switch (e->type)
	{
	case NH_JSON_T_ARRAY:
	case NH_JSON_T_OBJECT:
	  if (m->n_open == m->cap_open)
	    m->open = xpalloc (m->open, &m->cap_open, 1, -1, sizeof *m->open);
	  m->open[m->n_open++] = (struct nh_mat_open) { m->next, m->top };
	  break;
	case NH_JSON_T_END:
	  v = nh_mat_close (m);
	  break;
	case NH_JSON_T_KEY:
	  v = nh_mat_key (m, e);
	  break;
	case NH_JSON_T_STRING:
	  v = make_multibyte_string ((const char *) t->arena + e->v.s.c,
				     e->v.s.b, e->a);
	  break;
	case NH_JSON_T_INTEGER:
	  v = (e->neg ? INT_TO_INTEGER ((intmax_t) -(uintmax_t) e->v.u)
	       : INT_TO_INTEGER ((uintmax_t) e->v.u));
	  break;
	case NH_JSON_T_BIGNUM:
	  {
	    ptrdiff_t len;
	    v = string_to_number ((const char *) t->arena + e->v.s.c, 10, &len);
	    eassert (len == e->a);
	    break;
	  }
	case NH_JSON_T_FLOAT:
	  v = make_float (e->v.d);
	  break;
	case NH_JSON_T_TRUE:
	  v = Qt;
	  break;
	case NH_JSON_T_FALSE:
	  v = AREF (m->state, NH_MAT_S_FALSE);
	  break;
	case NH_JSON_T_NULL:
	  v = AREF (m->state, NH_MAT_S_NULL);
	  break;
	default:
	  emacs_abort ();
	}
      if (e->type != NH_JSON_T_ARRAY && e->type != NH_JSON_T_OBJECT)
	nh_mat_put (m, v);
      m->next++;
      if (--left == 0)
	{
	  left = stride;
	  if (m->next < t->n_ent && budget_ns >= 0
	      && nh_json_now () - t0 >= (uint64_t) budget_ns)
	    return false;
	}
    }
  return true;
}

/* A slice left by a signal or throw (a `memory-full' among them): the
   message becomes an error record and its partial state is dropped.  */
static void
nh_mat_slice_unwind (void *p)
{
  struct nh_mat *m = p;
  if (!m->in_slice)
    return;
  m->in_slice = false;
  m->failed = true;
  NH_WK_PHASE_POP (m->phase_depth);
  nh_mat_unlink (m);
}

/* Continue building M's value for about BUDGET_NS (negative: no limit),
   in the phase `json'; true once it is complete (nh_mat_result), false
   when it yielded.  GC may run between calls.  Signal the tape's error,
   or `json-out-of-memory' after a slice that was left by a signal.  */
bool
nh_json_materialize (struct nh_mat *m, int64_t budget_ns)
{
  if (m->tape->err != NH_JSON_E_NONE)
    nh_mat_signal (m);
  if (m->failed)
    xsignal0 (Qjson_out_of_memory);
  if (m->done)
    return true;
  specpdl_ref count = SPECPDL_INDEX ();
  record_unwind_protect_ptr (nh_mat_slice_unwind, m);
  m->phase_depth = NH_WK_PHASE_PUSH (NH_WK_PH_JSON, 0);
  m->in_slice = true;
  m->done = nh_mat_slice (m, budget_ns);
  m->in_slice = false;
  NH_WK_PHASE_POP (m->phase_depth);
  unbind_to (count, Qnil);
  return m->done;
}

static void
nh_mat_parse_args (ptrdiff_t nargs, Lisp_Object *args,
		   struct nh_mat_conf *conf)
{
  if ((nargs % 2) != 0)
    wrong_type_argument (Qplistp, Flist (nargs, args));

  /* From the back, so keyword values appearing first take precedence
     (as in json.c).  */
  for (ptrdiff_t i = nargs; i > 0; i -= 2)
    {
      Lisp_Object key = args[i - 2];
      Lisp_Object value = args[i - 1];
      if (EQ (key, QCobject_type))
	{
	  if (EQ (value, Qhash_table))
	    conf->object_type = NH_MAT_OBJ_HASH;
	  else if (EQ (value, Qalist))
	    conf->object_type = NH_MAT_OBJ_ALIST;
	  else if (EQ (value, Qplist))
	    conf->object_type = NH_MAT_OBJ_PLIST;
	  else
	    wrong_choice (list3 (Qhash_table, Qalist, Qplist), value);
	}
      else if (EQ (key, QCarray_type))
	{
	  if (EQ (value, Qarray))
	    conf->array_type = NH_MAT_ARR_ARRAY;
	  else if (EQ (value, Qlist))
	    conf->array_type = NH_MAT_ARR_LIST;
	  else
	    wrong_choice (list2 (Qarray, Qlist), value);
	}
      else if (EQ (key, QCnull_object))
	conf->null_object = value;
      else if (EQ (key, QCfalse_object))
	conf->false_object = value;
      else
	wrong_choice (list4 (QCobject_type, QCarray_type, QCnull_object,
			     QCfalse_object),
		      value);
    }
}

/* The framed branch's options (Task 24): CONF from OPTIONS, a plist of
   json-parse-string's keywords, checked as json-parse-string checks
   them.  */
static void
nh_mat_conf_of (Lisp_Object options, struct nh_mat_conf *conf)
{
  *conf = (struct nh_mat_conf) { NH_MAT_OBJ_HASH, NH_MAT_ARR_ARRAY, QCnull,
				 QCfalse };
  ptrdiff_t n = list_length (options);
  USE_SAFE_ALLOCA;
  Lisp_Object *args;
  SAFE_ALLOCA_LISP (args, n);
  for (ptrdiff_t i = 0; i < n; i++, options = XCDR (options))
    args[i] = XCAR (options);
  nh_mat_parse_args (n, args, conf);
  SAFE_FREE ();
}

Lisp_Object
nh_json_options (Lisp_Object options)
{
  struct nh_mat_conf conf;
  nh_mat_conf_of (options, &conf);
  return listn (8, QCobject_type,
		(conf.object_type == NH_MAT_OBJ_HASH ? Qhash_table
		 : conf.object_type == NH_MAT_OBJ_ALIST ? Qalist : Qplist),
		QCarray_type,
		conf.array_type == NH_MAT_ARR_ARRAY ? Qarray : Qlist,
		QCnull_object, conf.null_object,
		QCfalse_object, conf.false_object);
}

/* A materialization of TAPE, the JSON at SRC[0, NSRC) (unibyte), in
   C memory: free it with nh_json_mat_free.  */
struct nh_mat *
nh_json_mat_start (const struct nh_tape *tape, const unsigned char *src,
		   size_t nsrc, Lisp_Object options)
{
  struct nh_mat_conf conf;
  nh_mat_conf_of (options, &conf);
  struct nh_mat *m = xmalloc (sizeof *m);
  nh_mat_init (m, tape, &conf);
  m->src = src;
  m->nsrc = nsrc;
  m->src_multibyte = false;
  return m;
}

Lisp_Object
nh_json_mat_value (struct nh_mat *m)
{
  return nh_mat_result (m);
}

/* Whether a slice of M was left by a signal (a `memory-full'): the
   message is then an error, `json-out-of-memory'.  */
bool
nh_json_mat_failed (struct nh_mat *m)
{
  return m->failed;
}

void
nh_json_mat_free (struct nh_mat *m)
{
  nh_mat_release (m);
  xfree (m);
}

static void
nh_json_free_cleanup (void *p)
{
  free (p);
}

static void
nh_mat_release_cleanup (void *p)
{
  nh_mat_release (p);
}

/* The test primitives' start: check ARGS (STRING MODE &rest OPTIONS)
   into CONF and MODE and tokenize STRING.  The tape is the caller's.  */
static struct nh_tape *
nh_json_test_tokenize (ptrdiff_t nargs, Lisp_Object *args,
		       struct nh_mat_conf *conf, int *mode)
{
  Lisp_Object string = args[0];
  CHECK_STRING (string);
  if (EQ (args[1], Qstring))
    *mode = NH_JSON_MODE_STRING;
  else if (EQ (args[1], Qbuffer))
    *mode = NH_JSON_MODE_BUFFER;
  else
    wrong_choice (list2 (Qstring, Qbuffer), args[1]);
  *conf = (struct nh_mat_conf) { NH_MAT_OBJ_HASH, NH_MAT_ARR_ARRAY, QCnull,
				 QCfalse };
  nh_mat_parse_args (nargs - 2, args + 2, conf);
  struct nh_tape *tape = nh_json_tokenize (SDATA (string), SBYTES (string),
					   *mode, NULL);
  if (!tape)
    xsignal0 (Qjson_out_of_memory);
  return tape;
}

static void
nh_mat_set_src (struct nh_mat *m, Lisp_Object string)
{
  m->src = SDATA (string);
  m->nsrc = SBYTES (string);
  m->src_multibyte = STRING_MULTIBYTE (string);
}

DEFUN ("nidhogg-workers--test-tokenize", Fnidhogg_workers__test_tokenize,
       Snidhogg_workers__test_tokenize, 2, MANY, 0,
       doc: /* Parse STRING through the JSON tape; for tests.
MODE is `string' (as `json-parse-string') or `buffer' (as
`json-parse-buffer' on the bytes of STRING, a single value from the
start with trailing bytes allowed).  OPTIONS are `json-parse-string's
keywords.  Return the value, or signal what the tape's error says; in
`buffer' mode return (VALUE . BYTES-CONSUMED).
usage: (nidhogg-workers--test-tokenize STRING MODE &rest OPTIONS)  */)
  (ptrdiff_t nargs, Lisp_Object *args)
{
  specpdl_ref count = SPECPDL_INDEX ();
  struct nh_mat_conf conf;
  int mode;
  struct nh_tape *tape = nh_json_test_tokenize (nargs, args, &conf, &mode);
  record_unwind_protect_ptr (nh_json_free_cleanup, tape);
  struct nh_mat m;
  nh_mat_init (&m, tape, &conf);
  nh_mat_set_src (&m, args[0]);
  record_unwind_protect_ptr (nh_mat_release_cleanup, &m);
  nh_json_materialize (&m, -1);
  Lisp_Object result = nh_mat_result (&m);
  if (mode == NH_JSON_MODE_BUFFER)
    result = Fcons (result, make_fixnum (tape->consumed));
  return unbind_to (count, result);
}

/* The one sliced materialization the tests drive, and its tape.  */
static struct nh_mat nh_json_test_mat;
static bool nh_json_test_live;
static struct nh_tape *nh_json_test_tape;

static void
nh_json_test_drop (void)
{
  if (nh_json_test_live)
    nh_mat_release (&nh_json_test_mat);
  nh_json_test_live = false;
  free (nh_json_test_tape);
  nh_json_test_tape = NULL;
}

DEFUN ("nidhogg-workers--test-json-start", Fnidhogg_workers__test_json_start,
       Snidhogg_workers__test_json_start, 2, MANY, 0,
       doc: /* Start a sliced materialization of STRING; for tests.
The arguments are those of `nidhogg-workers--test-tokenize'.  An earlier
one is dropped.  Signal the tape's error, if it has one; else build
nothing yet: `nidhogg-workers--test-json-step' does.
usage: (nidhogg-workers--test-json-start STRING MODE &rest OPTIONS)  */)
  (ptrdiff_t nargs, Lisp_Object *args)
{
  nh_json_test_drop ();
  struct nh_mat_conf conf;
  int mode;
  nh_json_test_tape = nh_json_test_tokenize (nargs, args, &conf, &mode);
  if (nh_json_test_tape->err != NH_JSON_E_NONE)
    {
      struct nh_mat em = { .tape = nh_json_test_tape };
      nh_mat_set_src (&em, args[0]);
      nh_mat_signal (&em);
    }
  nh_mat_init (&nh_json_test_mat, nh_json_test_tape, &conf);
  nh_json_test_live = true;
  return Qnil;
}

DEFUN ("nidhogg-workers--test-json-step", Fnidhogg_workers__test_json_step,
       Snidhogg_workers__test_json_step, 1, 1, 0,
       doc: /* Run one slice of the test materialization; for tests.
BUDGET-NS is the slice's budget (negative: none).  Return nil when the
slice yielded, or (VALUE) when the document is complete (the
materialization is then dropped).  */)
  (Lisp_Object budget_ns)
{
  CHECK_FIXNUM (budget_ns);
  if (!nh_json_test_live)
    error ("No test materialization");
  if (!nh_json_materialize (&nh_json_test_mat, XFIXNUM (budget_ns)))
    return Qnil;
  Lisp_Object value = nh_mat_result (&nh_json_test_mat);
  nh_json_test_drop ();
  return list1 (value);
}

DEFUN ("nidhogg-workers--test-json-in-flight",
       Fnidhogg_workers__test_json_in_flight,
       Snidhogg_workers__test_json_in_flight, 0, 0, 0,
       doc: /* The number of JSON materializations in flight; for tests.  */)
  (void)
{
  return make_fixnum (list_length (nh_json_in_flight));
}

DEFUN ("nidhogg-workers--test-set-numeric-locale",
       Fnidhogg_workers__test_set_numeric_locale,
       Snidhogg_workers__test_set_numeric_locale, 1, 1, 0,
       doc: /* Set the C library's LC_NUMERIC to NAME; for tests.
Return the locale now in force, or nil if NAME is not available.  With
NAME nil, only report the current one.  */)
  (Lisp_Object name)
{
  const char *r;
  if (NILP (name))
    r = setlocale (LC_NUMERIC, NULL);
  else
    {
      CHECK_STRING (name);
      r = setlocale (LC_NUMERIC, SSDATA (name));
    }
  return r ? build_string (r) : Qnil;
}

void
nh_json_syms (void)
{
  defsubr (&Snidhogg_workers__test_tokenize);
  defsubr (&Snidhogg_workers__test_set_numeric_locale);
  defsubr (&Snidhogg_workers__test_json_start);
  defsubr (&Snidhogg_workers__test_json_step);
  defsubr (&Snidhogg_workers__test_json_in_flight);

  nh_json_in_flight = Qnil;
  staticpro (&nh_json_in_flight);

  DEFVAR_INT ("nidhogg-workers--test-slice-entries",
	      nidhogg_workers__test_slice_entries,
	      doc: /* How many tape entries a JSON slice builds between clock checks.
4,096; tests lower it (1 with a budget of 0 is one entry per slice).  */);
  nidhogg_workers__test_slice_entries = 4096;

  DEFVAR_INT ("nidhogg-workers--test-intern-count",
	      nidhogg_workers__test_intern_count,
	      doc: /* Test only: how many keys the JSON materializer has interned.
Tests set it to 0 and read it after a parse.  */);
  nidhogg_workers__test_intern_count = 0;

  DEFVAR_INT ("nidhogg-workers--test-json-fail-at",
	      nidhogg_workers__test_json_fail_at,
	      doc: /* Test only: the tape entry at which a JSON slice signals `memory-full'.
-1, the default, is none.  The signal is what `memory_full' signals.  */);
  nidhogg_workers__test_json_fail_at = -1;
}

#endif /* not NH_WK_HARNESS */

#endif /* HAVE_NIDHOGG_WORKERS */

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
   what json_parse_value builds.  json.c is not edited.

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

/* Modes of nh_json_tokenize.  */
enum
{
  NH_JSON_MODE_STRING,	/* json-parse-string: trailing content is an error.  */
  NH_JSON_MODE_BUFFER	/* json-parse-buffer: one value, the rest is left.  */
};

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
      if (k->len == nbytes
	  && memcmp (p->arena + k->off, p->arena + start, nbytes) == 0)
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

#include "buffer.h"

/* What the materializer needs besides the tape: json.c's options, and
   the source bytes, for the line and position of an error.  */
struct nh_mat_conf
{
  enum { NH_MAT_OBJ_HASH, NH_MAT_OBJ_ALIST, NH_MAT_OBJ_PLIST } object_type;
  enum { NH_MAT_ARR_ARRAY, NH_MAT_ARR_LIST } array_type;
  Lisp_Object null_object;
  Lisp_Object false_object;
};

struct nh_mat
{
  const struct nh_tape *tape;
  struct nh_mat_conf conf;
  size_t next;			/* The next entry to build.  */
  Lisp_Object result;
  const unsigned char *src;
  size_t nsrc;
  bool src_multibyte;
};

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

static Lisp_Object
nh_mat_key (struct nh_mat *m, const struct nh_json_ent *e, bool intern,
	    bool colon)
{
  const struct nh_tape *t = m->tape;
  const char *s = (const char *) t->arena + t->keys[e->v.s.c].off;
  ptrdiff_t nbytes = e->a, nchars = e->v.s.b;
  if (!intern)
    return make_multibyte_string (s, nchars, nbytes);
  if (!colon)
    return intern_c_multibyte (s, nchars, nbytes);
  USE_SAFE_ALLOCA;
  char *buf = SAFE_ALLOCA (nbytes + 1);
  buf[0] = ':';
  memcpy (buf + 1, s, nbytes);
  Lisp_Object sym = intern_c_multibyte (buf, nchars + 1, nbytes + 1);
  SAFE_FREE ();
  return sym;
}

/* Build the value at M->next, as json_parse_value does.  It recurses
   like json.c does (and holds every Lisp object in a local, which the
   conservative stack scan sees); Task 23 slices it.  */
static Lisp_Object
nh_mat_value (struct nh_mat *m)
{
  const struct nh_tape *t = m->tape;
  const struct nh_json_ent *e = &t->ents[m->next++];
  switch (e->type)
    {
    case NH_JSON_T_STRING:
      return make_multibyte_string ((const char *) t->arena + e->v.s.c,
				    e->v.s.b, e->a);
    case NH_JSON_T_INTEGER:
      if (e->neg)
	return INT_TO_INTEGER ((intmax_t) -(uintmax_t) e->v.u);
      return INT_TO_INTEGER ((uintmax_t) e->v.u);
    case NH_JSON_T_BIGNUM:
      {
	ptrdiff_t len;
	Lisp_Object r = string_to_number ((const char *) t->arena + e->v.s.c,
					  10, &len);
	eassert (len == e->a);
	return r;
      }
    case NH_JSON_T_FLOAT:
      return make_float (e->v.d);
    case NH_JSON_T_TRUE:
      return Qt;
    case NH_JSON_T_FALSE:
      return m->conf.false_object;
    case NH_JSON_T_NULL:
      return m->conf.null_object;
    case NH_JSON_T_ARRAY:
      {
	ptrdiff_t n = e->a;
	Lisp_Object result = Qnil;
	if (m->conf.array_type == NH_MAT_ARR_ARRAY)
	  {
	    result = make_vector (n, Qnil);
	    for (ptrdiff_t i = 0; i < n; i++)
	      {
		Lisp_Object x = nh_mat_value (m);
		ASET (result, i, x);
	      }
	  }
	else
	  {
	    Lisp_Object *cdr = &result;
	    for (ptrdiff_t i = 0; i < n; i++)
	      {
		Lisp_Object nc = Fcons (nh_mat_value (m), Qnil);
		*cdr = nc;
		cdr = xcdr_addr (nc);
	      }
	  }
	m->next++;		/* END */
	return result;
      }
    case NH_JSON_T_OBJECT:
      {
	ptrdiff_t n = e->a;
	Lisp_Object result = Qnil;
	if (m->conf.object_type == NH_MAT_OBJ_HASH)
	  {
	    result = make_hash_table (&hashtest_equal, n, Weak_None);
	    struct Lisp_Hash_Table *h = XHASH_TABLE (result);
	    for (ptrdiff_t i = 0; i < n; i++)
	      {
		Lisp_Object key = nh_mat_key (m, &t->ents[m->next++], false,
					      false);
		Lisp_Object value = nh_mat_value (m);
		hash_hash_t hash;
		ptrdiff_t idx = hash_find_get_hash (h, key, &hash);
		if (idx < 0)
		  hash_put (h, key, value, hash);
		else
		  set_hash_value_slot (h, idx, value);
	      }
	  }
	else
	  {
	    bool plist = m->conf.object_type == NH_MAT_OBJ_PLIST;
	    Lisp_Object *cdr = &result;
	    for (ptrdiff_t i = 0; i < n; i++)
	      {
		Lisp_Object key = nh_mat_key (m, &t->ents[m->next++], true,
					      plist);
		Lisp_Object value = nh_mat_value (m);
		Lisp_Object nc;
		if (plist)
		  {
		    nc = Fcons (key, Qnil);
		    *cdr = nc;
		    cdr = xcdr_addr (nc);
		    nc = Fcons (value, Qnil);
		  }
		else
		  nc = Fcons (Fcons (key, value), Qnil);
		*cdr = nc;
		cdr = xcdr_addr (nc);
	      }
	  }
	m->next++;		/* END */
	return result;
      }
    default:
      emacs_abort ();
    }
}

/* Build M's value into M->result, or signal the tape's error.  BUDGET_NS
   is ignored until Task 23 slices the work; the answer is always true
   (done).  */
bool
nh_json_materialize (struct nh_mat *m, int64_t budget_ns)
{
  (void) budget_ns;
  if (m->tape->err != NH_JSON_E_NONE)
    nh_mat_signal (m);
  m->next = 0;
  m->result = nh_mat_value (m);
  return true;
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

static void
nh_json_free_cleanup (void *p)
{
  free (p);
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
  Lisp_Object string = args[0];
  CHECK_STRING (string);
  int mode;
  if (EQ (args[1], Qstring))
    mode = NH_JSON_MODE_STRING;
  else if (EQ (args[1], Qbuffer))
    mode = NH_JSON_MODE_BUFFER;
  else
    wrong_choice (list2 (Qstring, Qbuffer), args[1]);

  struct nh_mat m;
  m.conf = (struct nh_mat_conf) { NH_MAT_OBJ_HASH, NH_MAT_ARR_ARRAY, QCnull,
				  QCfalse };
  nh_mat_parse_args (nargs - 2, args + 2, &m.conf);
  m.src = SDATA (string);
  m.nsrc = SBYTES (string);
  m.src_multibyte = STRING_MULTIBYTE (string);

  struct nh_tape *tape = nh_json_tokenize (m.src, m.nsrc, mode, NULL);
  if (!tape)
    xsignal0 (Qjson_out_of_memory);
  record_unwind_protect_ptr (nh_json_free_cleanup, tape);
  m.tape = tape;
  m.result = Qnil;
  nh_json_materialize (&m, 0);
  Lisp_Object result = m.result;
  if (mode == NH_JSON_MODE_BUFFER)
    result = Fcons (result, make_fixnum (tape->consumed));
  return unbind_to (count, result);
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
}

#endif /* not NH_WK_HARNESS */

#endif /* HAVE_NIDHOGG_WORKERS */

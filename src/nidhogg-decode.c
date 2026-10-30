/* Nidhogg's worker threads: stateless decoders and the auto-save encoder.

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

/* Stubs (Task 2); Tasks 18, 33 and 34 fill them.  */

#ifndef NH_WK_HARNESS
# include <config.h>
#endif

#ifdef HAVE_NIDHOGG_WORKERS

#include <string.h>

#include "nidhogg-workers.h"

#ifndef NH_WK_HARNESS
# include "coding.h"
#endif

/* ---- Task 18: the stateless decoders ------------------------------- */

/* The process path decodes each read as one decode_coding call
   (coding.c) on the bytes the previous read left over followed by the
   new ones.  What follows copies that call for the systems of enum
   nh_dec_system, quirks included, so that a chunk decoded here is byte
   for byte what the main thread would make of the same bytes:
   decode_coding_utf_8 on a C-string (unibyte) source,
   decode_coding_raw_text, produce_chars into a multibyte (UTF-8) or
   unibyte (raw text) destination with no translation table, the
   CODING_MODE_LAST_BLOCK flush and decode_eol.  Two quirks matter for
   -dos: a byte read after a CR and found invalid is dropped
   (decode_coding_utf_8 restarts after it), and so the result depends on
   where decode_coding's character buffer fills, which is copied too.  */

/* decode_coding's character buffer: min (bytes + MAX_CHARBUF_EXTRA_SIZE,
   MAX_CHARBUF_SIZE) characters per decoder call.  */
enum { NH_DEC_CHARBUF = 0x4000, NH_DEC_CHARBUF_EXTRA = 16 };

/* Character C at O in Emacs's multibyte form (char_string): up to 0x3FFF7F
   as UTF-8 extended to five bytes, a raw byte (0x3FFF80...) as two.  */
static unsigned char *
nh_dec_put (unsigned char *o, int c)
{
  if (c < 0x80)
    *o++ = c;
  else if (c <= 0x7FF)
    {
      *o++ = 0xC0 | (c >> 6);
      *o++ = 0x80 | (c & 0x3F);
    }
  else if (c <= 0xFFFF)
    {
      *o++ = 0xE0 | (c >> 12);
      *o++ = 0x80 | ((c >> 6) & 0x3F);
      *o++ = 0x80 | (c & 0x3F);
    }
  else if (c <= 0x1FFFFF)
    {
      *o++ = 0xF0 | (c >> 18);
      *o++ = 0x80 | ((c >> 12) & 0x3F);
      *o++ = 0x80 | ((c >> 6) & 0x3F);
      *o++ = 0x80 | (c & 0x3F);
    }
  else if (c <= 0x3FFF7F)
    {
      *o++ = 0xF8;
      *o++ = 0x80 | ((c >> 18) & 0x0F);
      *o++ = 0x80 | ((c >> 12) & 0x3F);
      *o++ = 0x80 | ((c >> 6) & 0x3F);
      *o++ = 0x80 | (c & 0x3F);
    }
  else
    {
      int b = c - 0x3FFF00;
      *o++ = 0xC0 | ((b >> 6) & 1);
      *o++ = 0x80 | (b & 0x3F);
    }
  return o;
}

/* The character byte B decodes to when it stands alone (BYTE8_TO_CHAR).  */
static int
nh_dec_byte8 (int b)
{
  return b < 0x80 ? b : 0x3FFF00 + b;
}

/* coding.c's ONE_MORE_BYTE for a unibyte source.  */
#define NH_DEC_MORE(c)				\
  do {						\
    if (src == end)				\
      {						\
	if (base < src)				\
	  short_src = true;			\
	goto no_more;				\
      }						\
    (c) = *src++;				\
  } while (0)

/* One call of decode_coding_utf_8 on S[*POS..N) with room for CAP
   characters, written at *OP and counted in *NCHARS; *POS becomes what
   it consumed.  Return false if the source ran out inside a sequence
   (CODING_RESULT_INSUFFICIENT_SRC).  */
static bool
nh_dec_utf8_call (const unsigned char *s, ptrdiff_t n, ptrdiff_t *pos,
		  bool dos, ptrdiff_t cap, unsigned char **op,
		  ptrdiff_t *nchars)
{
  const unsigned char *src = s + *pos, *end = s + n, *base = src;
  unsigned char *o = *op;
  ptrdiff_t used = 0;
  int after_cr = -1;
  bool short_src = false;

  for (;;)
    {
      int c, c1, c2, c3, c4, c5;

      base = src;
      if (used >= cap)
	{
	  if (after_cr >= 0)
	    base--;
	  break;
	}
      if (after_cr >= 0)
	c1 = after_cr, after_cr = -1;
      else
	NH_DEC_MORE (c1);
      if (c1 < 0x80)
	{
	  if (dos && c1 == '\r')
	    NH_DEC_MORE (after_cr);
	  c = c1;
	}
      else
	{
	  NH_DEC_MORE (c2);
	  if ((c2 & 0xC0) != 0x80)
	    goto invalid;
	  if ((c1 & 0xE0) == 0xC0)
	    {
	      c = ((c1 & 0x1F) << 6) | (c2 & 0x3F);
	      if (c < 128)
		goto invalid;
	    }
	  else
	    {
	      NH_DEC_MORE (c3);
	      if ((c3 & 0xC0) != 0x80)
		goto invalid;
	      if ((c1 & 0xF0) == 0xE0)
		{
		  c = (((c1 & 0xF) << 12)
		       | ((c2 & 0x3F) << 6) | (c3 & 0x3F));
		  if (c < 0x800 || (c >= 0xd800 && c < 0xe000))
		    goto invalid;
		}
	      else
		{
		  NH_DEC_MORE (c4);
		  if ((c4 & 0xC0) != 0x80)
		    goto invalid;
		  if ((c1 & 0xF8) == 0xF0)
		    {
		      c = (((c1 & 0x7) << 18) | ((c2 & 0x3F) << 12)
			   | ((c3 & 0x3F) << 6) | (c4 & 0x3F));
		      if (c < 0x10000)
			goto invalid;
		    }
		  else
		    {
		      NH_DEC_MORE (c5);
		      if ((c5 & 0xC0) != 0x80)
			goto invalid;
		      if ((c1 & 0xFC) != 0xF8)
			goto invalid;
		      c = (((c1 & 0x3) << 24) | ((c2 & 0x3F) << 18)
			   | ((c3 & 0x3F) << 12) | ((c4 & 0x3F) << 6)
			   | (c5 & 0x3F));
		      if (c > 0x3FFFFF || c < 0x200000)
			goto invalid;
		    }
		}
	    }
	}
      o = nh_dec_put (o, c);
      used++;
      continue;

    invalid:
      /* From BASE again: after a CR (-dos) that is past the byte read
	 after it, which is lost, as in decode_coding_utf_8.  */
      src = base;
      NH_DEC_MORE (c);
      o = nh_dec_put (o, nh_dec_byte8 (c));
      used++;
    }

 no_more:
  *pos = base - s;
  *op = o;
  *nchars += used;
  return !short_src;
}

#undef NH_DEC_MORE

/* decode_coding with decode_coding_utf_8, then the flush.  */
static ptrdiff_t
nh_dec_utf8 (const unsigned char *s, ptrdiff_t n, bool dos, bool last,
	     unsigned char *out, ptrdiff_t *nchars, ptrdiff_t *consumed)
{
  ptrdiff_t cap = (n < NH_DEC_CHARBUF - NH_DEC_CHARBUF_EXTRA
		   ? n + NH_DEC_CHARBUF_EXTRA : NH_DEC_CHARBUF);
  ptrdiff_t pos = 0;
  unsigned char *o = out;

  while (pos < n && nh_dec_utf8_call (s, n, &pos, dos, cap, &o, nchars))
    ;
  if (last)
    for (; pos < n; pos++, ++*nchars)
      o = nh_dec_put (o, nh_dec_byte8 (s[pos]));
  *consumed = pos;
  return o - out;
}

/* decode_coding with decode_coding_raw_text, then the flush: -dos keeps
   a final CR back.  */
static ptrdiff_t
nh_dec_raw (const unsigned char *s, ptrdiff_t n, bool dos, bool last,
	    unsigned char *out, ptrdiff_t *nchars, ptrdiff_t *consumed)
{
  ptrdiff_t m = dos && n > 0 && s[n - 1] == '\r' ? n - 1 : n;
  if (m > 0)
    memcpy (out, s, m);
  if (last && m < n)
    out[m++] = '\r';
  *nchars += m;
  *consumed = last ? n : m;
  return m;
}

/* decode_eol on the LEN bytes at OUT: CR LF to LF (-dos), CR to LF
   (-mac).  A CR or LF is never part of a multibyte sequence, so bytes
   are characters here.  */
static ptrdiff_t
nh_dec_eol (int eol, unsigned char *out, ptrdiff_t len, ptrdiff_t *nchars)
{
  if (eol == NH_DEC_EOL_CR)
    {
      for (ptrdiff_t i = 0; i < len; i++)
	if (out[i] == '\r')
	  out[i] = '\n';
    }
  else if (eol == NH_DEC_EOL_CRLF)
    {
      ptrdiff_t j = 0;
      for (ptrdiff_t i = 0; i < len; i++)
	if (out[i] == '\r' && i + 1 < len && out[i + 1] == '\n')
	  --*nchars;
	else
	  out[j++] = out[i];
      len = j;
    }
  return len;
}

ptrdiff_t
nh_dec_run (int system, const unsigned char *s, ptrdiff_t n, bool last,
	    unsigned char *out, ptrdiff_t *nchars, ptrdiff_t *consumed)
{
  NH_WK_ASSERT (system > NH_DEC_NONE && system < NH_DEC_N);
  int eol = (system == NH_DEC_UTF8_DOS || system == NH_DEC_RAW_DOS
	     ? NH_DEC_EOL_CRLF
	     : system == NH_DEC_UTF8_MAC || system == NH_DEC_RAW_MAC
	     ? NH_DEC_EOL_CR : NH_DEC_EOL_LF);
  bool dos = eol == NH_DEC_EOL_CRLF;
  *nchars = 0;
  ptrdiff_t len = (NH_DEC_MULTIBYTE (system)
		   ? nh_dec_utf8 (s, n, dos, last, out, nchars, consumed)
		   : nh_dec_raw (s, n, dos, last, out, nchars, consumed));
  return nh_dec_eol (eol, out, len, nchars);
}

ptrdiff_t
nh_dec_chunk (int system, const unsigned char *in, ptrdiff_t n,
	      struct nh_dec_state *st, unsigned char *out, ptrdiff_t *nchars,
	      bool last)
{
  const unsigned char *s = in;
  unsigned char *joined = NULL;
  ptrdiff_t total = st->n + n, consumed;

  if (st->n > 0)
    {
      joined = malloc (total);
      if (!joined)
	return -1;
      memcpy (joined, st->carry, st->n);
      if (n > 0)
	memcpy (joined + st->n, in, n);
      s = joined;
    }
  ptrdiff_t len = nh_dec_run (system, s, total, last, out, nchars,
			      &consumed);
  st->n = total - consumed;
  NH_WK_ASSERT (st->n <= NH_DEC_CARRY);
  if (st->n > 0)
    memcpy (st->carry, s + consumed, st->n);
  free (joined);
  return len;
}

/* ---- Task 33: the speculative check of a file block ---------------- */

/* Scan N bytes at P once: ASCII only; strictly valid UTF-8 and its
   character count; the line-ending kind; a null byte; an ESC; a BOM.
   Lisp-free, so nh-blk runs it right after the read.  */
void
nh_dec_scan (const unsigned char *p, size_t n, struct nh_dec_scan *s)
{
  size_t conts = 0, lf = 0, cr = 0, crlf = 0;
  bool ascii = true, utf8 = true;

  memset (s, 0, sizeof *s);
  s->bom = n >= 3 && p[0] == 0xEF && p[1] == 0xBB && p[2] == 0xBF;
  for (size_t i = 0; i < n; )
    {
      unsigned char c = p[i];
      if (c < 0x80)
	{
	  if (c == 0)
	    s->has_null = true;
	  else if (c == 0x1B)
	    s->has_esc = true;
	  else if (c == '\n')
	    {
	      lf++;
	      if (i > 0 && p[i - 1] == '\r')
		crlf++;
	    }
	  else if (c == '\r')
	    cr++;
	  i++;
	  continue;
	}
      ascii = false;
      if (!utf8)
	{
	  i++;
	  continue;
	}
      size_t len;
      unsigned char lo = 0x80, hi = 0xBF;
      if (c >= 0xC2 && c <= 0xDF)
	len = 2;
      else if (c >= 0xE0 && c <= 0xEF)
	{
	  len = 3;
	  if (c == 0xE0)
	    lo = 0xA0;
	  else if (c == 0xED)
	    hi = 0x9F;
	}
      else if (c >= 0xF0 && c <= 0xF4)
	{
	  len = 4;
	  if (c == 0xF0)
	    lo = 0x90;
	  else if (c == 0xF4)
	    hi = 0x8F;
	}
      else
	{
	  utf8 = false;
	  i++;
	  continue;
	}
      bool ok = n - i >= len && p[i + 1] >= lo && p[i + 1] <= hi;
      for (size_t k = 2; ok && k < len; k++)
	ok = (p[i + k] & 0xC0) == 0x80;
      if (!ok)
	{
	  utf8 = false;
	  i++;
	  continue;
	}
      conts += len - 1;
      i += len;
    }
  s->ascii = ascii;
  s->utf8 = utf8;
  s->nchars = utf8 ? n - conts : 0;
  if (lf == 0 && cr == 0)
    s->eol = NH_DEC_EOL_NONE;
  else if (cr == 0)
    s->eol = NH_DEC_EOL_LF;
  else if (lf == 0)
    s->eol = NH_DEC_EOL_CR;
  else if (cr == lf && lf == crlf)
    s->eol = NH_DEC_EOL_CRLF;
  else
    s->eol = NH_DEC_EOL_MIXED;
}

/* ---- Task 34: the auto-save encoder --------------------------------- */

/* The length of the character at P in Emacs's internal representation,
   or 0 if P does not start one: a copy of character.h's
   multibyte_length with CHECK and ALLOW_8BIT true, the test that
   consume_chars applies to a buffer's text when it encodes.  */
static int
nh_dec_mb_length (const unsigned char *p, const unsigned char *pend)
{
  if (p < pend)
    {
      unsigned char c = p[0];
      if (c < 0x80)
	return 1;
      if (p + 1 < pend)
	{
	  unsigned char d = p[1];
	  int w = ((d & 0xC0) << 2) + c;
	  if (0x2C0 <= w && w <= 0x2DF)
	    return 2;
	  if (p + 2 < pend)
	    {
	      unsigned char e = p[2];
	      w += (e & 0xC0) << 4;
	      int w1 = w | ((d & 0x20) >> 2);
	      if (0xAE1 <= w1 && w1 <= 0xAEF)
		return 3;
	      if (p + 3 < pend)
		{
		  unsigned char f = p[3];
		  w += (f & 0xC0) << 6;
		  int w2 = w | ((d & 0x30) >> 3);
		  if (0x2AF1 <= w2 && w2 <= 0x2AF7)
		    return 4;
		  if (p + 4 < pend)
		    {
		      int_fast64_t lw = w + ((p[4] & 0xC0) << 8),
			w3 = (lw << 24) + (d << 16) + (e << 8) + f;
		      if (0xAAF8888080 <= w3 && w3 <= 0xAAF88FBFBD)
			return 5;
		    }
		}
	    }
	}
    }
  return 0;
}

/* encode_coding_utf_8's rule for `utf-8-emacs-unix' (no BOM, no EOL
   conversion, no translation table): a raw-byte character (internally
   C0 or C1 and one continuation byte) is written as its byte; any other
   character is written as its internal bytes; a byte that starts no
   character (a unibyte buffer's) is a raw byte, written as itself.  */
size_t
nh_dec_autosave_encode (const unsigned char *in, size_t n, unsigned char *out)
{
  const unsigned char *end = in + n;
  size_t o = 0;
  for (size_t i = 0; i < n; )
    {
      unsigned char c = in[i];
      if (c < 0x80)
	{
	  /* A run of ASCII.  */
	  size_t j = i + 1;
	  while (j < n && in[j] < 0x80)
	    j++;
	  if (out + o != in + i)
	    memmove (out + o, in + i, j - i);
	  o += j - i;
	  i = j;
	  continue;
	}
      int len = nh_dec_mb_length (in + i, end);
      if (len == 2 && c < 0xC2)
	{
	  out[o++] = 0x80 | ((c & 1) << 6) | (in[i + 1] & 0x3F);
	  i += 2;
	}
      else if (len > 0)
	{
	  if (out + o != in + i)
	    memmove (out + o, in + i, len);
	  o += len;
	  i += len;
	}
      else
	out[o++] = in[i++];
    }
  return o;
}

#ifndef NH_WK_HARNESS

/* Task 18: which stateless system CODING decodes with.  A settled end
   of line only (an undecided one is detected on the main thread, which
   then settles the struct); no BOM, post-read conversion or translation
   table of the system's own; not utf-8-emacs (§8.2).  The caller checks
   the global translation table and inhibit-eol-conversion at
   delivery.  */
int
nh_dec_classify (struct coding_system *coding)
{
  Lisp_Object attrs = CODING_ID_ATTRS (coding->id);
  Lisp_Object eol = CODING_ID_EOL_TYPE (coding->id);
  Lisp_Object type = CODING_ATTR_TYPE (attrs);
  bool unibyte = !NILP (CODING_ATTR_FOR_UNIBYTE (attrs));
  int base;

  if (!NILP (CODING_ATTR_POST_READ (attrs))
      || !NILP (CODING_ATTR_DECODE_TBL (attrs)))
    return NH_DEC_NONE;
  if (EQ (type, Qutf_8) && !unibyte
      && NILP (AREF (attrs, coding_attr_utf_bom))
      && !EQ (CODING_ATTR_BASE_NAME (attrs), Qutf_8_emacs))
    base = NH_DEC_UTF8_UNIX;
  else if (EQ (type, Qraw_text) && unibyte)
    base = NH_DEC_RAW_UNIX;
  else
    return NH_DEC_NONE;
  return (EQ (eol, Qunix) ? base
	  : EQ (eol, Qdos) ? base + 1
	  : EQ (eol, Qmac) ? base + 2
	  : NH_DEC_NONE);
}

static const char *const nh_dec_system_names[NH_DEC_N] = {
  NULL, "utf-8-unix", "utf-8-dos", "utf-8-mac",
  "raw-text-unix", "raw-text-dos", "raw-text-mac"
};

/* The name of SYS for the test primitives, or nil for NH_DEC_NONE.  */
Lisp_Object
nh_dec_system_name (int sys)
{
  return (sys > NH_DEC_NONE && sys < NH_DEC_N
	  ? intern_c_string (nh_dec_system_names[sys]) : Qnil);
}

/* One step of --test-decode: S (nil: the end of the stream) decoded by
   nh-io's decoder with SYS and *ST, or with ORACLE by CODING as
   read_process_output decodes a read, CARRY and *NCARRY being its
   p->decoding_carryover.  Return (STRING . NCHARS).  */
static Lisp_Object
nh_dec_test_step (Lisp_Object s, int sys, struct nh_dec_state *st,
		  struct coding_system *coding, unsigned char *carry,
		  int *ncarry)
{
  ptrdiff_t n = 0;
  if (!NILP (s))
    {
      CHECK_STRING (s);
      if (STRING_MULTIBYTE (s))
	error ("Chunks must be unibyte strings");
      n = SBYTES (s);
    }
  Lisp_Object text;
  ptrdiff_t nchars;
  USE_SAFE_ALLOCA;
  if (sys != NH_DEC_NONE)
    {
      unsigned char *out = SAFE_ALLOCA (NH_DEC_OUT_MAX (n + NH_DEC_CARRY) + 1);
      ptrdiff_t len = nh_dec_chunk (sys, n ? SDATA (s) : NULL, n, st, out,
				    &nchars, NILP (s));
      if (len < 0)
	memory_full (n);
      text = (NH_DEC_MULTIBYTE (sys)
	      ? make_multibyte_string ((char *) out, nchars, len)
	      : make_unibyte_string ((char *) out, len));
    }
  else
    {
      unsigned char *buf = SAFE_ALLOCA (*ncarry + n + 1);
      memcpy (buf, carry, *ncarry);
      if (n)
	memcpy (buf + *ncarry, SDATA (s), n);
      if (NILP (s))
	coding->mode |= CODING_MODE_LAST_BLOCK;
      decode_coding_c_string (coding, buf, *ncarry + n, Qt);
      text = coding->dst_object;
      nchars = SCHARS (text);
      *ncarry = coding->carryover_bytes;
      memcpy (carry, coding->carryover, *ncarry);
    }
  SAFE_FREE ();
  return Fcons (text, make_int (nchars));
}

DEFUN ("nidhogg-workers--test-decode", Fnidhogg_workers__test_decode,
       Snidhogg_workers__test_decode, 2, 3, 0,
       doc: /* Decode CHUNKS as a process's output under CODING-SYSTEM; for tests.
CHUNKS is a list of unibyte strings, read in turn.  Return a list with
one (STRING . NCHARS) per chunk and a last one for the end of the
stream (the flush of an incomplete tail): STRING is what a filter would
get, NCHARS the decoder's character count.  With ORACLE nil, nh-io's
decoder makes them, and the value is nil unless CODING-SYSTEM is one it
decodes; with ORACLE non-nil, the main thread's decoder does, as
`read_process_output' does with the reader off.  */)
  (Lisp_Object coding_system, Lisp_Object chunks, Lisp_Object oracle)
{
  struct coding_system coding;
  struct nh_dec_state st = { 0 };
  unsigned char carry[sizeof coding.carryover];
  int ncarry = 0;

  CHECK_LIST (chunks);
  setup_coding_system (coding_system, &coding);
  int sys = NILP (oracle) ? nh_dec_classify (&coding) : NH_DEC_NONE;
  if (NILP (oracle) && sys == NH_DEC_NONE)
    return Qnil;
  Lisp_Object result = Qnil;
  for (Lisp_Object tail = chunks; CONSP (tail); tail = XCDR (tail))
    result = Fcons (nh_dec_test_step (XCAR (tail), sys, &st, &coding,
				      carry, &ncarry),
		    result);
  result = Fcons (nh_dec_test_step (Qnil, sys, &st, &coding, carry, &ncarry),
		  result);
  return Fnreverse (result);
}

void
nh_dec_syms (void)
{
  defsubr (&Snidhogg_workers__test_decode);
}

#endif /* not NH_WK_HARNESS */

#endif /* HAVE_NIDHOGG_WORKERS */

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

ptrdiff_t
nh_dec_chunk (int system, const unsigned char *in, ptrdiff_t n,
	      struct nh_dec_state *st, unsigned char *out, ptrdiff_t *nchars,
	      bool last)
{
  (void) system;
  (void) in;
  (void) n;
  (void) st;
  (void) out;
  (void) last;
  *nchars = 0;
  return 0;
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

#ifndef NH_WK_HARNESS

void
nh_dec_syms (void)
{
}

#endif /* not NH_WK_HARNESS */

#endif /* HAVE_NIDHOGG_WORKERS */

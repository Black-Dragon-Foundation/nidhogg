/* Fuzz target `decode' (Task 18): nh-io's stateless decoders.  Lisp-free.
   Built by nidhogg/test/workers-harness.sh --fuzz, with libFuzzer when
   the compiler has it and with fuzz/driver.c otherwise.  Whatever the
   bytes and wherever a stream is cut: the decoder stays inside its
   input and output, the carry stays small, the character count is the
   output's, the output is Emacs's multibyte form (UTF-8) or unibyte
   bytes (raw text), and decoding in pieces gives the whole decode
   (except utf-8-dos, where Emacs's own decoder drops a byte read after a CR
   and found invalid, so its result depends on the cuts).  */

#define NH_WK_HARNESS 1
#define HAVE_NIDHOGG_WORKERS 1
#include "../../../../src/nidhogg-workers.h"

#include <assert.h>
#include <string.h>

/* Characters in the multibyte LEN bytes at P; abort unless every one is
   well formed: a lead byte (C0 or C1 for a raw byte, up to F8) and its
   continuation bytes.  */
static ptrdiff_t
count_multibyte (const unsigned char *p, ptrdiff_t len)
{
  ptrdiff_t chars = 0;
  for (ptrdiff_t i = 0; i < len; chars++)
    {
      int c = p[i], k;
      if (c < 0x80)
	k = 1;
      else if ((c & 0xE0) == 0xC0)
	k = 2;
      else if ((c & 0xF0) == 0xE0)
	k = 3;
      else if ((c & 0xF8) == 0xF0)
	k = 4;
      else
	{
	  assert (c == 0xF8);
	  k = 5;
	}
      assert (i + k <= len);
      for (int j = 1; j < k; j++)
	assert ((p[i + j] & 0xC0) == 0x80);
      i += k;
    }
  return chars;
}

static void
check_output (int sys, const unsigned char *out, ptrdiff_t len,
	      ptrdiff_t nchars, ptrdiff_t in)
{
  assert (0 <= len && len <= NH_DEC_OUT_MAX (in));
  if (NH_DEC_MULTIBYTE (sys))
    assert (count_multibyte (out, len) == nchars);
  else
    assert (nchars == len);
}

int
LLVMFuzzerTestOneInput (const uint8_t *data, size_t size)
{
  /* Decode from an exactly sized heap copy, so a read past the end is
     the sanitizer's to catch.  */
  unsigned char *p = malloc (size ? size : 1);
  unsigned char *whole = malloc (NH_DEC_OUT_MAX (size) + 1);
  unsigned char *pieces = malloc (NH_DEC_OUT_MAX (size + NH_DEC_CARRY) + 1);
  unsigned char *out = malloc (NH_DEC_OUT_MAX (size + NH_DEC_CARRY) + 1);
  if (!p || !whole || !pieces || !out)
    abort ();
  memcpy (p, data, size);
  /* The cuts: every STEP bytes, STEP from the first byte.  */
  size_t step = size ? 1 + p[0] % 7 : 1;

  for (int sys = NH_DEC_NONE + 1; sys < NH_DEC_N; sys++)
    {
      struct nh_dec_state st = { 0 };
      ptrdiff_t nchars, wchars, pchars = 0, plen = 0;
      ptrdiff_t wlen = nh_dec_chunk (sys, p, size, &st, whole, &wchars, true);
      check_output (sys, whole, wlen, wchars, size);
      assert (st.n == 0);

      for (size_t at = 0; at < size; at += step)
	{
	  size_t n = size - at < step ? size - at : step;
	  /* Each piece from its own exact copy.  */
	  unsigned char *piece = malloc (n);
	  if (!piece)
	    abort ();
	  memcpy (piece, p + at, n);
	  int carry = st.n;
	  ptrdiff_t len = nh_dec_chunk (sys, piece, n, &st, out, &nchars,
					false);
	  free (piece);
	  check_output (sys, out, len, nchars, carry + n);
	  assert (0 <= st.n && st.n <= 4);
	  memcpy (pieces + plen, out, len);
	  plen += len;
	  pchars += nchars;
	}
      ptrdiff_t len = nh_dec_chunk (sys, NULL, 0, &st, out, &nchars, true);
      check_output (sys, out, len, nchars, NH_DEC_CARRY);
      assert (st.n == 0);
      memcpy (pieces + plen, out, len);
      plen += len;
      pchars += nchars;

      if (sys != NH_DEC_UTF8_DOS)
	assert (plen == wlen && pchars == wchars
		&& memcmp (pieces, whole, wlen) == 0);
    }
  free (p);
  free (whole);
  free (pieces);
  free (out);
  return 0;
}

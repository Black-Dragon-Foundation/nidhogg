/* Fuzz target `cores' (Task 28): the image decoder cores on untrusted bytes.

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

/* Built by nidhogg/test/workers-harness.sh --fuzz SECONDS cores, with
   -DNH_WK_HARNESS and the four libraries.  With libFuzzer (clang) the
   entry point is LLVMFuzzerTestOneInput; with gcc the driver below mutates
   the seed files given on the command line (flips, truncations, splices)
   for the time given in NH_FUZZ_SECONDS, under ASan and UBSan.  Input
   layout: byte 0 the format, byte 1 the frame index (0xff: none), the
   rest the file.  Limits keep each run small.  */

#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>

#include "nidhogg-image-bg.h"

int
LLVMFuzzerTestOneInput (const uint8_t *data, size_t size)
{
  if (size < 3)
    return 0;
  struct nh_img_params p;
  memset (&p, 0, sizeof p);
  int fmt = data[0] % NH_IMG_FMT_N;
  p.frame_index = data[1] == 0xff ? -1 : data[1] % 16;
  p.key = p.frame_index >= 0 ? nh_img_hash (data + 2, size - 2) : 0;
  p.limit_w = p.limit_h = 512;
  p.bg_set = data[0] & 0x80;
  p.bg_r = 0x1234;
  p.bg_g = 0x8000;
  p.bg_b = 0xfedc;
  p.frame_bg_pixel = 0x00ffffff;
  p.gif_bg_set = data[0] & 0x40;
  p.gif_bg_pixel = 0x00102030;
  struct nh_img_out out;
  memset (&out, 0, sizeof out);
  nh_img_decode (fmt, data + 2, size - 2, &p, &out);
  nh_img_out_free (&out);
  return 0;
}

#ifdef NH_FUZZ_DRIVER

static uint64_t rng = 0x9e3779b97f4a7c15ull;

static uint64_t
next (void)
{
  rng ^= rng << 13;
  rng ^= rng >> 7;
  rng ^= rng << 17;
  return rng;
}

int
main (int argc, char **argv)
{
  const char *secs = getenv ("NH_FUZZ_SECONDS");
  time_t end = time (NULL) + (secs ? atoi (secs) : 10);
  enum { MAXSEED = 64, MAXLEN = 1 << 20 };
  uint8_t *seed[MAXSEED];
  size_t slen[MAXSEED];
  int n = 0;
  for (int i = 1; i < argc && n < MAXSEED; i++)
    {
      FILE *f = fopen (argv[i], "rb");
      if (!f)
	continue;
      uint8_t *b = malloc (MAXLEN);
      size_t len = fread (b + 2, 1, MAXLEN - 2, f) + 2;
      fclose (f);
      const char *dot = strrchr (argv[i], '.');
      b[0] = !dot ? 0 : !strcmp (dot, ".png") ? NH_IMG_PNG
	: !strcmp (dot, ".jpg") ? NH_IMG_JPEG : !strcmp (dot, ".gif")
	? NH_IMG_GIF : NH_IMG_WEBP;
      b[1] = 0xff;
      seed[n] = b;
      slen[n++] = len;
    }
  if (!n)
    return 2;
  long runs = 0;
  uint8_t *buf = malloc (MAXLEN);
  while (time (NULL) < end)
    for (int k = 0; k < 200; k++, runs++)
      {
	int s = next () % n;
	size_t len = slen[s];
	memcpy (buf, seed[s], len);
	int muts = 1 + next () % 8;
	while (muts--)
	  switch (next () % 5)
	    {
	    case 0: buf[next () % len] ^= 1 << (next () % 8); break;
	    case 1: buf[next () % len] = next (); break;
	    case 2: len = 3 + next () % (len - 2); break;
	    case 3: buf[1] = next () % 20 < 3 ? 0xff : next () % 4; break;
	    default:
	      {
		size_t a = next () % len, b2 = next () % len;
		buf[a] = buf[b2];
	      }
	    }
	LLVMFuzzerTestOneInput (buf, len);
      }
  printf ("cores: %ld runs, no crash\n", runs);
  return 0;
}

#endif

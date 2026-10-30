/* A stand-in for libFuzzer's driver, for compilers without it (gcc): runs
   LLVMFuzzerTestOneInput on mutations of the seed files for a number of
   seconds.  The random seed is fixed, so a failing run repeats.
   Usage: driver SECONDS SEED-FILE...  */

#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>

extern int LLVMFuzzerTestOneInput (const uint8_t *, size_t);

static uint64_t rng = 0x9e3779b97f4a7c15u;

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
  if (argc < 3)
    return 2;
  double budget = atof (argv[1]);
  int nseeds = argc - 2;
  uint8_t **seed = calloc (nseeds, sizeof *seed);
  size_t *len = calloc (nseeds, sizeof *len);
  for (int i = 0; i < nseeds; i++)
    {
      FILE *f = fopen (argv[2 + i], "rb");
      if (!f)
	return 2;
      fseek (f, 0, SEEK_END);
      len[i] = ftell (f);
      fseek (f, 0, SEEK_SET);
      seed[i] = malloc (len[i] + 1);
      if (fread (seed[i], 1, len[i], f) != len[i])
	return 2;
      fclose (f);
      LLVMFuzzerTestOneInput (seed[i], len[i]);
    }
  struct timespec t0, t;
  clock_gettime (CLOCK_MONOTONIC, &t0);
  unsigned long runs = 0;
  uint8_t *buf = malloc (1 << 16);
  do
    {
      for (int k = 0; k < 256; k++, runs++)
	{
	  int s = next () % nseeds;
	  size_t n = len[s] < (1 << 16) ? len[s] : (1 << 16);
	  memcpy (buf, seed[s], n);
	  int edits = 1 + next () % 4;
	  for (int e = 0; e < edits && n; e++)
	    {
	      size_t pos = next () % (n < 64 ? n : (next () % 3 ? 64 : n));
	      switch (next () % 5)
		{
		case 0: buf[pos] ^= 1 << (next () % 8); break;
		case 1: buf[pos] = next (); break;
		case 2: n = pos; break;               /* truncate */
		case 3: buf[pos] = 0xff; break;
		default:                              /* a field to an edge value */
		  buf[pos] = (uint8_t[]) { 0, 1, 0x7f, 0x80, 0xfe }[next () % 5];
		}
	    }
	  LLVMFuzzerTestOneInput (buf, n);
	}
      clock_gettime (CLOCK_MONOTONIC, &t);
    }
  while ((t.tv_sec - t0.tv_sec) + (t.tv_nsec - t0.tv_nsec) / 1e9 < budget);
  printf ("fuzz: %lu runs\n", runs);
  for (int i = 0; i < nseeds; i++)
    free (seed[i]);
  free (seed);
  free (len);
  free (buf);
  return 0;
}

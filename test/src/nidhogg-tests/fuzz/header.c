/* Fuzz target `header' (Task 27): the four image header parsers and the
   GIF and WebP frame-count scans.  Lisp-free.  Built by
   nidhogg/test/workers-harness.sh --fuzz, with libFuzzer when the compiler
   has it and with fuzz/driver.c otherwise; the seeds are the images under
   fixtures/images.  Whatever the bytes, a parser must stay inside them and
   an answer must be a real size.  */

#define NH_WK_HARNESS 1
#define HAVE_NIDHOGG_WORKERS 1
#include "../../../../src/nidhogg-workers.h"

#include <assert.h>
#include <string.h>

int
LLVMFuzzerTestOneInput (const uint8_t *data, size_t size)
{
  /* Parse from an exactly sized heap copy, so a read past the end is
     the sanitizer's to catch.  */
  uint8_t *p = malloc (size ? size : 1);
  if (!p)
    return 0;
  memcpy (p, data, size);

  for (int fmt = NH_IMG_F_PNG; fmt <= NH_IMG_F_WEBP; fmt++)
    {
      int w = 0, h = 0;
      if (nh_img_parse_header (fmt, p, size, &w, &h) == 1)
	assert (w > 0 && h > 0);
      /* A prefix is never an error to parse.  */
      for (size_t n = 0; n < size && n < 70; n += 3)
	nh_img_parse_header (fmt, p, n, &w, &h);
    }
  for (int idx = 0; idx < 3; idx++)
    {
      struct nh_img_scan sc;
      if (nh_img_gif_scan (p, size, idx, &sc) == 1)
	assert (sc.width > 0 && sc.height > 0 && sc.frames > 0
		&& sc.nexts >= 0 && sc.nexts <= NH_IMG_GIF_EXT_MAX);
      if (nh_img_webp_scan (p, size, idx, &sc) == 1)
	assert (sc.width > 0 && sc.height > 0 && sc.frames >= 0
		&& sc.duration_ms >= 0);
    }
  free (p);
  return 0;
}

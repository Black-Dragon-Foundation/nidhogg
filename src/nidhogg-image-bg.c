/* Nidhogg's worker threads: image headers and decoding (nh-img).

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

/* Header parsers and header answers (Task 27); the decoder cores and
   the pending entries are Tasks 28 and 29.  Nothing here decodes: the
   parsers read the bytes of a file or a :data string and say how big
   the image is and, for GIF and WebP, how many frames it has.  An
   image they cannot parse answers nothing, and the caller keeps
   today's synchronous path.  The parsers are Lisp-free (the harness
   fuzzes them); the part that reads specs and builds Lisp objects is
   under #ifndef NH_WK_HARNESS.  */

#ifndef NH_WK_HARNESS
# include <config.h>
#endif

#ifdef HAVE_NIDHOGG_WORKERS

#include "nidhogg-workers.h"

#include <limits.h>
#include <string.h>

bool
nh_img_lookup (struct frame *f, struct image *img, ptrdiff_t id)
{
  (void) f;
  (void) img;
  (void) id;
  return false;
}

bool
nh_img_sync_load (struct frame *f, struct image *img, int fmt)
{
  (void) f;
  (void) img;
  (void) fmt;
  return false;
}


/* The header parsers.  */

static uint32_t
rd_be32 (const uint8_t *p)
{
  return (uint32_t) p[0] << 24 | (uint32_t) p[1] << 16
	 | (uint32_t) p[2] << 8 | p[3];
}

static unsigned
rd_be16 (const uint8_t *p)
{
  return (unsigned) p[0] << 8 | p[1];
}

static unsigned
rd_le16 (const uint8_t *p)
{
  return (unsigned) p[1] << 8 | p[0];
}

static unsigned
rd_le24 (const uint8_t *p)
{
  return (unsigned) p[2] << 16 | (unsigned) p[1] << 8 | p[0];
}

static uint32_t
rd_le32 (const uint8_t *p)
{
  return (uint32_t) p[3] << 24 | rd_le24 (p);
}

static uint32_t
crc32_png (const uint8_t *p, size_t n)
{
  uint32_t c = 0xffffffffu;
  for (size_t i = 0; i < n; i++)
    {
      c ^= p[i];
      for (int k = 0; k < 8; k++)
	c = (c >> 1) ^ (0xedb88320u & -(c & 1));
    }
  return ~c;
}

/* PNG: the signature and IHDR, whose CRC libpng checks too.  */
static int
parse_png (const uint8_t *p, size_t n, int *w, int *h)
{
  static const uint8_t sig[8] = { 0x89, 'P', 'N', 'G', '\r', '\n', 0x1a, '\n' };
  if (n < 33)
    return -1;
  if (memcmp (p, sig, 8) != 0 || rd_be32 (p + 8) != 13
      || memcmp (p + 12, "IHDR", 4) != 0
      || crc32_png (p + 12, 17) != rd_be32 (p + 29))
    return 0;
  uint32_t width = rd_be32 (p + 16), height = rd_be32 (p + 20);
  int depth = p[24], type = p[25];
  if (width == 0 || height == 0 || width > INT_MAX || height > INT_MAX
      || p[26] != 0 || p[27] != 0 || p[28] > 1)
    return 0;
  bool ok;
  switch (type)
    {
    case 0: ok = depth == 1 || depth == 2 || depth == 4 || depth == 8
		 || depth == 16; break;
    case 3: ok = depth == 1 || depth == 2 || depth == 4 || depth == 8; break;
    case 2: case 4: case 6: ok = depth == 8 || depth == 16; break;
    default: ok = false;
    }
  if (!ok)
    return 0;
  *w = width;
  *h = height;
  return 1;
}

/* JPEG: walk the marker segments to the first SOF (baseline, extended
   or progressive, 8 bits).  */
static int
parse_jpeg (const uint8_t *p, size_t n, int *w, int *h)
{
  if (n < 2)
    return -1;
  if (p[0] != 0xff || p[1] != 0xd8)
    return 0;
  size_t i = 2;
  for (;;)
    {
      if (i >= n)
	return -1;
      if (p[i] != 0xff)
	return 0;
      while (i < n && p[i] == 0xff)
	i++;
      if (i >= n)
	return -1;
      int m = p[i++];
      if (m == 0x01 || (m >= 0xd0 && m <= 0xd8))
	continue;
      if (m == 0x00 || m == 0xd9 || m == 0xda)
	return 0;
      if (i + 2 > n)
	return -1;
      size_t len = rd_be16 (p + i);
      if (len < 2)
	return 0;
      if (m >= 0xc0 && m <= 0xc2)
	{
	  if (len < 8)
	    return 0;
	  if (i + 8 > n)
	    return -1;
	  unsigned height = rd_be16 (p + i + 3), width = rd_be16 (p + i + 5);
	  if (p[i + 2] != 8 || width == 0 || height == 0 || p[i + 7] == 0)
	    return 0;
	  *w = width;
	  *h = height;
	  return 1;
	}
      i += len;
    }
}

/* GIF: the logical screen descriptor.  */
static int
parse_gif (const uint8_t *p, size_t n, int *w, int *h)
{
  if (n < 13)
    return -1;
  if (memcmp (p, "GIF8", 4) != 0 || (p[4] != '7' && p[4] != '9')
      || p[5] != 'a')
    return 0;
  unsigned width = rd_le16 (p + 6), height = rd_le16 (p + 8);
  if (width == 0 || height == 0)
    return 0;
  *w = width;
  *h = height;
  return 1;
}

/* The size of a VP8 (lossy) or VP8L (lossless) bitstream chunk.  */
static bool
webp_stream_size (const uint8_t *fcc, const uint8_t *c, size_t len,
		  int *w, int *h)
{
  if (memcmp (fcc, "VP8 ", 4) == 0)
    {
      if (len < 10 || (c[0] & 1) != 0 || c[3] != 0x9d || c[4] != 0x01
	  || c[5] != 0x2a)
	return false;
      *w = rd_le16 (c + 6) & 0x3fff;
      *h = rd_le16 (c + 8) & 0x3fff;
    }
  else if (memcmp (fcc, "VP8L", 4) == 0)
    {
      if (len < 5 || c[0] != 0x2f)
	return false;
      uint32_t bits = rd_le32 (c + 1);
      *w = (bits & 0x3fff) + 1;
      *h = ((bits >> 14) & 0x3fff) + 1;
    }
  else
    return false;
  return *w > 0 && *h > 0;
}

/* WebP: the RIFF header and the first chunk: a bitstream's own size or
   VP8X's canvas.  */
static int
parse_webp (const uint8_t *p, size_t n, int *w, int *h)
{
  if (n < 30)
    return -1;
  if (memcmp (p, "RIFF", 4) != 0 || memcmp (p + 8, "WEBP", 4) != 0)
    return 0;
  const uint8_t *c = p + 20;
  size_t len = rd_le32 (p + 16);
  if (memcmp (p + 12, "VP8X", 4) == 0)
    {
      if (len < 10)
	return 0;
      uint32_t width = rd_le24 (c + 4) + 1u, height = rd_le24 (c + 7) + 1u;
      if (width > INT_MAX || height > INT_MAX)
	return 0;
      *w = width;
      *h = height;
      return 1;
    }
  if (len > n - 20)
    len = n - 20;
  return webp_stream_size (p + 12, c, len, w, h) ? 1 : 0;
}

/* The native size of the image in P[0..N) of format FMT.  */
int
nh_img_parse_header (int fmt, const uint8_t *p, size_t n, int *w, int *h)
{
  switch (fmt)
    {
    case NH_IMG_F_PNG: return parse_png (p, n, w, h);
    case NH_IMG_F_JPEG: return parse_jpeg (p, n, w, h);
    case NH_IMG_F_GIF: return parse_gif (p, n, w, h);
    case NH_IMG_F_WEBP: return parse_webp (p, n, w, h);
    default: return 0;
    }
}


/* The frame-count scans.  They need the whole file.  */

/* GIF, as giflib's DGifSlurp reads it: extension blocks collect and
   attach to the next image; the first sub-block of an extension carries
   its function code and the continuation ones the code 0.  Trailing
   extensions belong to no frame.  The LZW data is skipped, not
   decoded.  */
int
nh_img_gif_scan (const uint8_t *p, size_t n, int idx, struct nh_img_scan *sc)
{
  int w, h;
  if (parse_gif (p, n, &w, &h) != 1 || idx < 0)
    return 0;
  memset (sc, 0, sizeof *sc);
  sc->width = w;
  sc->height = h;
  struct nh_img_gif_ext pend[NH_IMG_GIF_EXT_MAX];
  int npend = 0;
  size_t pos = 13;
  if (p[10] & 0x80)
    pos += 3u << ((p[10] & 7) + 1);
  for (;;)
    {
      if (pos >= n)
	return 0;
      int b = p[pos++];
      if (b == 0x3b)
	break;
      if (b == 0x2c)
	{
	  if (pos + 9 > n)
	    return 0;
	  unsigned left = rd_le16 (p + pos), top = rd_le16 (p + pos + 2);
	  unsigned iw = rd_le16 (p + pos + 4), ih = rd_le16 (p + pos + 6);
	  int packed = p[pos + 8];
	  pos += 9;
	  if (packed & 0x80)
	    pos += 3u << ((packed & 7) + 1);
	  if (pos >= n || p[pos++] > 8)
	    return 0;
	  for (;;)
	    {
	      if (pos >= n)
		return 0;
	      size_t len = p[pos++];
	      if (len == 0)
		break;
	      pos += len;
	      if (pos > n)
		return 0;
	    }
	  if (top + ih > (unsigned) h || left + iw > (unsigned) w)
	    return 0;
	  if (sc->frames == idx)
	    {
	      memcpy (sc->exts, pend, npend * sizeof *pend);
	      sc->nexts = npend;
	    }
	  sc->frames++;
	  npend = 0;
	}
      else if (b == 0x21)
	{
	  if (pos >= n)
	    return 0;
	  int fn = p[pos++];
	  bool first = true;
	  for (;;)
	    {
	      if (pos >= n)
		return 0;
	      size_t len = p[pos++];
	      if (len == 0)
		break;
	      if (pos + len > n || npend == NH_IMG_GIF_EXT_MAX)
		return 0;
	      pend[npend].function = first ? fn : 0;
	      pend[npend].len = len;
	      memcpy (pend[npend].bytes, p + pos, len);
	      npend++;
	      first = false;
	      pos += len;
	    }
	}
      else
	return 0;
    }
  return sc->frames > 0;
}

/* WebP: a still image has the size of its bitstream; VP8X gives the
   canvas, and the animated kind counts its ANMF chunks and keeps the
   duration of frame IDX (the last frame when IDX is past the end, as
   webp_load clamps it).  */
int
nh_img_webp_scan (const uint8_t *p, size_t n, int idx, struct nh_img_scan *sc)
{
  if (n < 30 || idx < 0 || memcmp (p, "RIFF", 4) != 0
      || memcmp (p + 8, "WEBP", 4) != 0)
    return 0;
  uint32_t riff = rd_le32 (p + 4);
  if (riff < 12 || riff > n - 8)
    return 0;
  n = (size_t) riff + 8;
  memset (sc, 0, sizeof *sc);
  size_t pos = 12;
  bool x = false, seen_stream = false;
  int last_dur = 0, idx_dur = 0;
  while (pos + 8 <= n)
    {
      const uint8_t *fcc = p + pos;
      size_t len = rd_le32 (p + pos + 4), pay = pos + 8;
      if (len > n - pay)
	return 0;
      const uint8_t *c = p + pay;
      if (pos == 12 && memcmp (fcc, "VP8X", 4) == 0)
	{
	  if (len < 10)
	    return 0;
	  uint32_t cw = rd_le24 (c + 4) + 1u, ch = rd_le24 (c + 7) + 1u;
	  if (cw > INT_MAX || ch > INT_MAX)
	    return 0;
	  sc->width = cw;
	  sc->height = ch;
	  sc->animated = (c[0] & 2) != 0;
	  x = true;
	}
      else if (!x)
	{
	  if (!webp_stream_size (fcc, c, len, &sc->width, &sc->height))
	    return 0;
	  return 1;
	}
      else if (memcmp (fcc, "ANMF", 4) == 0)
	{
	  if (!sc->animated || len < 16)
	    return 0;
	  last_dur = rd_le24 (c + 12);
	  if (sc->frames == idx)
	    idx_dur = last_dur;
	  sc->frames++;
	}
      else if (!sc->animated && !seen_stream)
	{
	  int w, h;
	  if (webp_stream_size (fcc, c, len, &w, &h))
	    {
	      if (w != sc->width || h != sc->height)
		return 0;
	      seen_stream = true;
	    }
	}
      pos = pay + len + (len & 1);
    }
  if (!x)
    return 0;
  if (sc->animated)
    {
      if (sc->frames == 0)
	return 0;
      sc->duration_ms = idx < sc->frames ? idx_dur : last_dur;
      return 1;
    }
  return seen_stream;
}


#ifndef NH_WK_HARNESS

#include "frame.h"
#include "dispextern.h"
#include "coding.h"
#include "sysstdio.h"

#include <fcntl.h>
#include <sys/stat.h>

/* Files bigger than this are not scanned for frames, and the JPEG
   header is looked for in at most the first megabyte.  */
enum { NH_IMG_SCAN_MAX = 64 << 20, NH_IMG_JPEG_MAX = 1 << 20 };

static bool
nh_img_on (void)
{
  Lisp_Object v = find_symbol_value (Qnidhogg_workers_images);
  return !BASE_EQ (v, Qunbound) && !NILP (v);
}

/* The bytes of an image spec: the :data string (not copied, so nothing
   may allocate Lisp objects while it is in use) or a prefix of the
   :file.  */
struct nh_img_src
{
  Lisp_Object data;
  uint8_t *buf;
  size_t n, size;
};

static const uint8_t *
nh_img_ptr (const struct nh_img_src *s)
{
  return STRINGP (s->data) ? SDATA (s->data) : s->buf;
}

/* Fill S from SPEC: all of a :data string, or the first CAP bytes of the
   file (a file bigger than CAP is refused when WHOLE).  A file's buffer
   is freed by the caller's unbind_to.  */
static bool
nh_img_open (Lisp_Object spec, size_t cap, bool whole, struct nh_img_src *s)
{
  s->data = image_spec_value (spec, QCdata, NULL);
  s->buf = NULL;
  if (!NILP (s->data))
    {
      if (!STRINGP (s->data))
	return false;
      s->n = s->size = SBYTES (s->data);
      return true;
    }
  Lisp_Object file = image_find_image_file (image_spec_value (spec, QCfile,
							       NULL));
  if (!STRINGP (file))
    return false;
  int fd = emacs_open (SSDATA (ENCODE_FILE (file)), O_RDONLY, 0);
  if (fd < 0)
    return false;
  struct stat st;
  bool ok = sys_fstat (fd, &st) == 0 && S_ISREG (st.st_mode)
	    && !(whole && st.st_size > (off_t) cap);
  if (ok)
    {
      s->size = st.st_size;
      s->n = min ((size_t) st.st_size, cap);
      s->buf = xmalloc (s->n + 1);
      record_unwind_protect_ptr (xfree, s->buf);
      size_t got = 0;
      while (got < s->n)
	{
	  ssize_t r = emacs_read (fd, s->buf + got, s->n - got);
	  if (r <= 0)
	    break;
	  got += r;
	}
      ok = got == s->n;
    }
  emacs_close (fd);
  return ok;
}

/* The native size of SPEC's image (of format FMT) from its bytes; with
   SCAN also the frame data of frame IDX in *SC.  The whole file is read
   when SCAN, else only what the header needs.  */
static bool
nh_img_probe (Lisp_Object spec, int fmt, bool scan, int idx,
	      struct nh_img_scan *sc, int *w, int *h)
{
  specpdl_ref count = SPECPDL_INDEX ();
  struct nh_img_src s;
  bool ok = false;
  if (scan)
    {
      if (nh_img_open (spec, NH_IMG_SCAN_MAX, true, &s))
	{
	  int r = (fmt == NH_IMG_F_GIF
		   ? nh_img_gif_scan (nh_img_ptr (&s), s.n, idx, sc)
		   : nh_img_webp_scan (nh_img_ptr (&s), s.n, idx, sc));
	  ok = r == 1 && (fmt != NH_IMG_F_GIF || idx < sc->frames);
	  *w = sc->width;
	  *h = sc->height;
	}
    }
  else
    {
      for (int stage = 0; stage < 2; stage++)
	{
	  size_t cap = stage == 0 ? (fmt == NH_IMG_F_JPEG ? 65536 : 64)
				  : NH_IMG_JPEG_MAX;
	  if (!nh_img_open (spec, cap, false, &s))
	    break;
	  int r = nh_img_parse_header (fmt, nh_img_ptr (&s), s.n, w, h);
	  ok = r == 1;
	  if (r != -1 || fmt != NH_IMG_F_JPEG || s.size <= s.n)
	    break;
	}
    }
  unbind_to (count, Qnil);
  return ok;
}

static int
nh_img_index (Lisp_Object spec)
{
  Lisp_Object index = image_spec_value (spec, QCindex, NULL);
  return FIXNUMP (index) ? min (XFIXNAT (index), INT_MAX) : 0;
}

/* image-size from the header (F is NULL only in tests, which then ask
   for pixels).  */
static Lisp_Object
nh_img_size_1 (Lisp_Object spec, struct frame *f, bool pixels, bool *answered)
{
  *answered = false;
  if (!nh_img_on () || !valid_image_p (spec) || (!f && !pixels))
    return Qnil;
  int fmt = nh_img_spec_format (spec);
  if (fmt == NH_IMG_F_NONE)
    return Qnil;
  /* A frame index is checked against the frames, as the loader does.  */
  bool scan = (fmt == NH_IMG_F_GIF || fmt == NH_IMG_F_WEBP)
	      && !NILP (image_spec_value (spec, QCindex, NULL));
  struct nh_img_scan sc;
  int w, h, dw, dh;
  if (!nh_img_probe (spec, fmt, scan, nh_img_index (spec), &sc, &w, &h)
      || !nh_img_size_ok (f, w, h)
      || !nh_img_displayed_size (f, spec, w, h, &dw, &dh))
    return Qnil;
  *answered = true;
  if (pixels)
    return Fcons (make_fixnum (dw), make_fixnum (dh));
  return Fcons (make_float ((double) dw / FRAME_COLUMN_WIDTH (f)),
		make_float ((double) dh / FRAME_LINE_HEIGHT (f)));
}

Lisp_Object
nh_img_size (Lisp_Object spec, Lisp_Object frame, bool pixels, bool *answered)
{
  *answered = false;
  if (!nh_img_on () || !valid_image_p (spec))
    return Qnil;
  return nh_img_size_1 (spec, decode_window_system_frame (frame), pixels,
			answered);
}

/* image-metadata from the frame scan: what the loaders record.  PNG
   and JPEG record none.  */
static Lisp_Object
nh_img_metadata_1 (Lisp_Object spec, struct frame *f, bool *answered)
{
  *answered = false;
  int fmt = nh_img_spec_format (spec);
  if (fmt == NH_IMG_F_NONE)
    return Qnil;
  struct nh_img_scan sc;
  int w, h, idx = nh_img_index (spec);
  bool scan = fmt == NH_IMG_F_GIF || fmt == NH_IMG_F_WEBP;
  if (!nh_img_probe (spec, fmt, scan, idx, &sc, &w, &h))
    return Qnil;
  if (!scan)
    {
      *answered = true;
      return Qnil;
    }
  if (!nh_img_size_ok (f, w, h))
    return Qnil;
  *answered = true;
  Lisp_Object data = Qnil;
  if (fmt == NH_IMG_F_WEBP)
    {
      if (!sc.animated)
	return Qnil;
      return Fcons (Qdelay,
		    Fcons (sc.duration_ms ? make_float (sc.duration_ms / 1000.0)
			   : Qt,
			   Fcons (Qcount,
				  Fcons (make_fixnum (sc.frames), Qnil))));
    }
  if (sc.nexts > 0)
    {
      int delay = 0;
      for (int i = 0; i < sc.nexts; i++)
	{
	  struct nh_img_gif_ext *e = &sc.exts[i];
	  data = Fcons (make_fixnum (e->function),
			Fcons (make_unibyte_string ((char *) e->bytes, e->len),
			       data));
	  if (e->function == 0xf9 && e->len == 4)
	    delay = e->bytes[2] << CHAR_BIT | e->bytes[1];
	}
      data = list2 (Qextension_data, data);
      data = Fcons (Qdelay, Fcons (delay ? make_float (delay / 100.0) : Qt,
				   data));
    }
  if (sc.frames > 1)
    data = Fcons (Qcount, Fcons (make_fixnum (sc.frames), data));
  return data;
}

Lisp_Object
nh_img_metadata (Lisp_Object spec, Lisp_Object frame, bool *answered)
{
  *answered = false;
  if (!nh_img_on () || !valid_image_p (spec))
    return Qnil;
  return nh_img_metadata_1 (spec, decode_window_system_frame (frame),
			    answered);
}

DEFUN ("nidhogg-workers--test-image-answer", Fnidhogg_workers__test_image_answer,
       Snidhogg_workers__test_image_answer, 2, 2, 0,
       doc: /* Test hook: the header answer for image SPEC, without a frame.
KIND is `size' (the size in pixels) or `metadata'.  Value is nil when the
header path would not answer, else (ANSWER) with the answer.  The switch
`nidhogg-workers-images' must be on.  */)
  (Lisp_Object kind, Lisp_Object spec)
{
  bool answered = false;
  Lisp_Object v;
  if (!nh_img_on () || !valid_image_p (spec))
    return Qnil;
  if (EQ (kind, Qsize))
    v = nh_img_size_1 (spec, NULL, true, &answered);
  else if (EQ (kind, Qmetadata))
    v = nh_img_metadata_1 (spec, NULL, &answered);
  else
    error ("Unknown kind");
  return answered ? list1 (v) : Qnil;
}

void
nh_img_syms (void)
{
  DEFSYM (Qnidhogg_workers_images, "nidhogg-workers-images");
  DEFSYM (Qsize, "size");
  defsubr (&Snidhogg_workers__test_image_answer);
}

#endif /* not NH_WK_HARNESS */

#endif /* HAVE_NIDHOGG_WORKERS */

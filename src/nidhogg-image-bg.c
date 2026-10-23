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

/* Header parsers and header answers (Task 27): the parsers read the bytes
   of a file or a :data string and say how big the image is and, for GIF
   and WebP, how many frames it has.  An image they cannot parse answers
   nothing, and the caller keeps today's synchronous path.  The parsers
   are Lisp-free (the harness fuzzes them); the part that reads specs and
   builds Lisp objects is under #ifndef NH_WK_HARNESS.

   The Lisp-free decoder cores of PNG, JPEG, GIF and WebP (Task 28) and the
   animation cache they share.  Each core is a copy of the decode path of the
   loader of the same name in image.c, with libc allocation and error codes
   in place of `image_error'; the oracle (workers/image-oracle) holds the
   copies equal to the loaders.

   Pending entries and landing (Task 29, design §8.3 items 1-4, 7-9).
   While redisplay looks an image up (the iterator's calls, with
   redisplaying_p), a cache miss on a PNG, JPEG, GIF or WebP whose header
   answers queues a decode on nh-img with copies of everything the loader
   reads, and waits for it within the redisplay's wait budget.  Past the
   budget the entry stays pending: it has the header's size, so layout is
   final, and no pixels, so it draws as the face's background.  The job's
   finish (the drainer, PD-6) only marks the entry landed and asks for a
   redisplay; at the top of the next redisplay_internal every landed entry
   is finished as a synchronous load would have finished it, and only the
   windows whose current matrix shows it are redrawn.  Results address an
   entry by (cache, id, generation): free_image, which every eviction,
   clear-image-cache, image-flush and frame deletion goes through, drops
   the entry's record and cancels its job, so a stale result is dropped.
   A Lisp caller that needs the pixels of a pending entry (a lookup
   outside redisplay) waits for the job or loads synchronously.  The
   records and the landing are the main thread's (the global lock
   holder's); the job is plain C.  Task 30 adds the stand-in rule.  */

#ifndef NH_WK_HARNESS
# include <config.h>
# include <stat-time.h>
#endif

#ifdef HAVE_NIDHOGG_WORKERS

#include <errno.h>
#include <fcntl.h>
#include <limits.h>
#include <pthread.h>
#include <setjmp.h>
#include <signal.h>
#include <stdio.h>
#include <string.h>
#include <sys/stat.h>
#include <time.h>
#include <unistd.h>

#include "nidhogg-workers.h"
#include "nidhogg-image-bg.h"
# ifndef NH_WK_HARNESS
#  include "coding.h"
# else
/* The fuzz targets link this file without the runtime, and run no
   signal handler.  */
#  define nh_wk_lock pthread_mutex_lock
# endif

#ifdef HAVE_PNG
# include <png.h>
#endif
#ifdef HAVE_JPEG
# ifdef HAVE_STDLIB_H
#  undef HAVE_STDLIB_H
# endif
# include <jpeglib.h>
# include <jerror.h>
#endif
#ifdef HAVE_GIF
# include <gif_lib.h>
#endif
#ifdef HAVE_WEBP
# include <webp/decode.h>
# include <webp/demux.h>
#endif

/* The harness builds without the decoder libraries, where the cores'
   helpers go unused.  */
#ifndef MAYBE_UNUSED
# define MAYBE_UNUSED __attribute__ ((unused))
#endif

/* Pixels the cores refuse whatever the limits say (512 MiB of words).  */
#define NH_IMG_MAX_PIXELS ((int64_t) 1 << 27)

static _Atomic int64_t nh_img_decodes;

int64_t
nh_img_decode_count (bool reset)
{
  return reset ? atomic_exchange (&nh_img_decodes, 0)
	       : atomic_load (&nh_img_decodes);
}

#define NH_IMG_COUNT() atomic_fetch_add (&nh_img_decodes, 1)

/* FNV-1a, for the identity of the data of an animation.  */
uint64_t
nh_img_hash_combine (uint64_t h, const void *p, size_t n)
{
  const uint8_t *b = p;
  for (size_t i = 0; i < n; i++)
    {
      h ^= b[i];
      h *= 1099511628211ull;
    }
  return h;
}

uint64_t
nh_img_hash (const uint8_t *data, size_t len)
{
  uint64_t h = nh_img_hash_combine (14695981039346656037ull, data, len);
  return h ? h : 1;
}

uint64_t
nh_img_file_key (const char *path)
{
  struct stat st;
  if (stat (path, &st) != 0)
    return 0;
  uint64_t h = nh_img_hash_combine (14695981039346656037ull, path,
				    strlen (path));
#ifdef NH_WK_HARNESS
  /* The harness builds in the Linux container only.  */
  struct timespec mtime = st.st_mtim;
#else
  /* gnulib's: macOS names the field st_mtimespec.  */
  struct timespec mtime = get_stat_mtime (&st);
#endif
  h = nh_img_hash_combine (h, &mtime, sizeof mtime);
  h = nh_img_hash_combine (h, &st.st_size, sizeof st.st_size);
  return h ? h : 1;
}

void
nh_img_out_free (struct nh_img_out *out)
{
  free (out->pixels);
  free (out->mask);
  while (out->ext)
    {
      struct nh_img_ext *e = out->ext;
      out->ext = e->next;
      free (e->bytes);
      free (e);
    }
  out->pixels = NULL;
  out->mask = NULL;
}

static int
nh_img_fail (struct nh_img_out *out, int code)
{
  out->err = code;
  return code;
}

MAYBE_UNUSED static bool
nh_core_size_ok (const struct nh_img_params *p, int64_t w, int64_t h)
{
  if (w <= 0 || h <= 0 || w > INT_MAX || h > INT_MAX)
    return false;
  if (p->limit_w >= 0 && w > p->limit_w)
    return false;
  if (p->limit_h >= 0 && h > p->limit_h)
    return false;
  return w * h <= NH_IMG_MAX_PIXELS;
}

/* The encoded bytes: DATA, or the contents of PARAMS->file.  */
struct nh_img_in
{
  const uint8_t *p;
  size_t len;
  uint8_t *owned;
};

MAYBE_UNUSED static int
nh_img_input (const uint8_t *data, size_t len, const struct nh_img_params *p,
	      struct nh_img_in *in, struct nh_img_out *out)
{
  in->owned = NULL;
  if (data)
    {
      in->p = data;
      in->len = len;
      return 0;
    }
  if (!p->file)
    return nh_img_fail (out, NH_IMG_E_OPEN);
  int fd = open (p->file, O_RDONLY | O_CLOEXEC);
  if (fd < 0)
    return nh_img_fail (out, NH_IMG_E_OPEN);
  struct stat st;
  size_t cap = fstat (fd, &st) == 0 && st.st_size > 0 ? st.st_size : 4096;
  uint8_t *buf = malloc (cap);
  size_t n = 0;
  while (buf)
    {
      if (n == cap)
	{
	  if (cap > SIZE_MAX / 2)
	    {
	      free (buf);
	      buf = NULL;
	      break;
	    }
	  cap *= 2;
	  uint8_t *nb = realloc (buf, cap);
	  if (!nb)
	    {
	      free (buf);
	      buf = NULL;
	      break;
	    }
	  buf = nb;
	}
      ssize_t r = read (fd, buf + n, cap - n);
      if (r < 0 && errno == EINTR)
	continue;
      if (r < 0)
	{
	  free (buf);
	  buf = NULL;
	  break;
	}
      if (r == 0)
	break;
      n += r;
    }
  close (fd);
  if (!buf)
    return nh_img_fail (out, NH_IMG_E_OPEN);
  in->owned = buf;
  in->p = buf;
  in->len = n;
  return 0;
}

MAYBE_UNUSED static void
nh_img_in_free (struct nh_img_in *in)
{
  free (in->owned);
  in->owned = NULL;
}


/* The animation cache: per image (by the identity of its data) the state
   that lets frame N be decoded from frame N-1.  An entry is taken out of
   the list while a decode uses it, so two threads never share one; a
   second decode of the same image meanwhile starts from frame 0.  */

struct nh_img_centry
{
  struct nh_img_centry *next;
  uint64_t key;
  int fmt;
  size_t bytes;
  int pos;			/* the last frame decoded, -1 if none */
#ifdef HAVE_GIF
  GifFileType *gif;
  uint32_t *canvas;
  uint32_t frame_bg;
  bool bg_set;
  uint32_t bg;
#endif
#ifdef HAVE_WEBP
  WebPAnimDecoder *dec;
  uint8_t *contents;
  uint8_t *last;		/* RGBA of frame POS */
  int width, height, frames;
  int timestamp;		/* of frame POS */
  int duration;			/* of frame POS */
  bool has_alpha;
#endif
};

#define NH_IMG_CACHE_MAX_ENTRIES 8
#define NH_IMG_CACHE_MAX_BYTES ((size_t) 256 << 20)

static pthread_mutex_t nh_img_cache_lock = PTHREAD_MUTEX_INITIALIZER;
static struct nh_img_centry *nh_img_cache;

#ifdef HAVE_GIF
static void
nh_gif_close (GifFileType *gif)
{
  int err;
# if GIFLIB_MAJOR + (GIFLIB_MINOR >= 1) > 5
  DGifCloseFile (gif, &err);
# else
  DGifCloseFile (gif);
  (void) err;
# endif
}
#endif

static void
nh_img_centry_free (struct nh_img_centry *e)
{
  if (!e)
    return;
#ifdef HAVE_GIF
  if (e->fmt == NH_IMG_GIF)
    {
      if (e->gif)
	nh_gif_close (e->gif);
      free (e->canvas);
    }
#endif
#ifdef HAVE_WEBP
  if (e->fmt == NH_IMG_WEBP)
    {
      if (e->dec)
	WebPAnimDecoderDelete (e->dec);
      free (e->contents);
      free (e->last);
    }
#endif
  free (e);
}

MAYBE_UNUSED static struct nh_img_centry *
nh_img_cache_take (int fmt, uint64_t key)
{
  struct nh_img_centry *e = NULL, **pp;
  nh_wk_lock (&nh_img_cache_lock);
  for (pp = &nh_img_cache; *pp; pp = &(*pp)->next)
    if ((*pp)->key == key && (*pp)->fmt == fmt)
      {
	e = *pp;
	*pp = e->next;
	e->next = NULL;
	break;
      }
  pthread_mutex_unlock (&nh_img_cache_lock);
  return e;
}

MAYBE_UNUSED static void
nh_img_cache_put (struct nh_img_centry *e)
{
  struct nh_img_centry *drop = NULL;
  nh_wk_lock (&nh_img_cache_lock);
  for (struct nh_img_centry *q = nh_img_cache; q; q = q->next)
    if (q->key == e->key && q->fmt == e->fmt)
      {
	drop = e;		/* another decode stored it meanwhile */
	e = NULL;
	break;
      }
  if (e)
    {
      e->next = nh_img_cache;
      nh_img_cache = e;
    }
  int n = 0;
  size_t bytes = 0;
  struct nh_img_centry **pp = &nh_img_cache;
  while (*pp)
    {
      n++;
      bytes += (*pp)->bytes;
      if (n > NH_IMG_CACHE_MAX_ENTRIES || bytes > NH_IMG_CACHE_MAX_BYTES)
	{
	  struct nh_img_centry *victim = *pp;
	  *pp = victim->next;
	  victim->next = drop;
	  drop = victim;
	  n--;
	  bytes -= victim->bytes;
	  continue;
	}
      pp = &(*pp)->next;
    }
  pthread_mutex_unlock (&nh_img_cache_lock);
  while (drop)
    {
      struct nh_img_centry *next = drop->next;
      nh_img_centry_free (drop);
      drop = next;
    }
}

void
nh_img_cache_clear (void)
{
  nh_wk_lock (&nh_img_cache_lock);
  struct nh_img_centry *all = nh_img_cache;
  nh_img_cache = NULL;
  pthread_mutex_unlock (&nh_img_cache_lock);
  while (all)
    {
      struct nh_img_centry *next = all->next;
      nh_img_centry_free (all);
      all = next;
    }
}

int
nh_img_cache_entries (void)
{
  int n = 0;
  nh_wk_lock (&nh_img_cache_lock);
  for (struct nh_img_centry *q = nh_img_cache; q; q = q->next)
    n++;
  pthread_mutex_unlock (&nh_img_cache_lock);
  return n;
}


/* PNG (png_load_body).  */

#ifdef HAVE_PNG

struct nh_png_ctx
{
  png_structp png;
  png_infop info, end;
  uint8_t *rows_mem;
  png_bytep *rows;
  struct nh_img_out *out;
  const uint8_t *bytes;
  size_t len, pos;
};

static void
nh_png_error (png_structp png, png_const_charp msg)
{
  struct nh_png_ctx *c = png_get_error_ptr (png);
  snprintf (c->out->errmsg, sizeof c->out->errmsg, "%s", msg);
  png_longjmp (png, 1);
}

static void
nh_png_warning (png_structp png, png_const_charp msg)
{
  struct nh_png_ctx *c = png_get_error_ptr (png);
  if (c->out->nwarn < 4)
    snprintf (c->out->warn[c->out->nwarn++], sizeof c->out->warn[0], "%s",
	      msg);
}

static void
nh_png_read (png_structp png, png_bytep data, size_t length)
{
  struct nh_png_ctx *c = png_get_io_ptr (png);
  if (length > c->len - c->pos)
    png_error (png, "Read error");
  memcpy (data, c->bytes + c->pos, length);
  c->pos += length;
}

static int
nh_png_decode (const uint8_t *data, size_t len,
	       const struct nh_img_params *p, struct nh_img_out *out)
{
  struct nh_img_in in;
  struct nh_png_ctx ctx;
  struct nh_png_ctx *volatile c = &ctx;
  memset (&ctx, 0, sizeof ctx);
  ctx.out = out;
  if (nh_img_input (data, len, p, &in, out))
    return out->err;
  if (in.len < 8 || png_sig_cmp (in.p, 0, 8))
    {
      nh_img_in_free (&in);
      return nh_img_fail (out, NH_IMG_E_NOT_IMAGE);
    }
  ctx.bytes = in.p + 8;
  ctx.len = in.len - 8;

  ctx.png = png_create_read_struct (PNG_LIBPNG_VER_STRING, &ctx,
				    nh_png_error, nh_png_warning);
  if (ctx.png)
    {
      ctx.info = png_create_info_struct (ctx.png);
      ctx.end = png_create_info_struct (ctx.png);
    }
  if (!(ctx.info && ctx.end))
    {
      png_destroy_read_struct (&ctx.png, &ctx.info, &ctx.end);
      nh_img_in_free (&in);
      return nh_img_fail (out, NH_IMG_E_SILENT);
    }

  if (setjmp (png_jmpbuf (ctx.png)))
    {
      if (!c->out->err)
	c->out->err = NH_IMG_E_LIB;
      goto done;
    }

  png_set_read_fn (ctx.png, &ctx, nh_png_read);
  png_set_sig_bytes (ctx.png, 8);
  png_read_info (ctx.png, ctx.info);
  png_uint_32 width, height;
  int bit_depth, color_type, interlace_type;
  png_get_IHDR (ctx.png, ctx.info, &width, &height, &bit_depth, &color_type,
		&interlace_type, NULL, NULL);
  if (!nh_core_size_ok (p, width, height))
    {
      out->err = NH_IMG_E_SIZE;
      goto done;
    }
  out->width = width;
  out->height = height;
  out->pixels = malloc ((size_t) width * height * sizeof *out->pixels);
  if (!out->pixels)
    {
      out->err = NH_IMG_E_NOMEM;
      goto done;
    }

  bool transparent_p = false;
# ifdef PNG_tRNS_SUPPORTED
  png_bytep trans_alpha;
  int num_trans;
  if (png_get_tRNS (ctx.png, ctx.info, &trans_alpha, &num_trans, NULL))
    {
      transparent_p = true;
      if (trans_alpha)
	for (int i = 0; i < num_trans; i++)
	  if (0 < trans_alpha[i] && trans_alpha[i] < 255)
	    {
	      transparent_p = false;
	      break;
	    }
    }
# endif

  if (bit_depth == 16)
    png_set_strip_16 (ctx.png);
  png_set_expand (ctx.png);
  if (color_type == PNG_COLOR_TYPE_GRAY
      || color_type == PNG_COLOR_TYPE_GRAY_ALPHA)
    png_set_gray_to_rgb (ctx.png);

  if (!transparent_p && p->bg_set)
    {
      int shift = bit_depth == 16 ? 0 : 8;
      png_color_16 bg = { 0 };
      bg.red = p->bg_r >> shift;
      bg.green = p->bg_g >> shift;
      bg.blue = p->bg_b >> shift;
      png_set_background (ctx.png, &bg, PNG_BACKGROUND_GAMMA_SCREEN, 0, 1.0);
    }

  png_set_interlace_handling (ctx.png);
  png_read_update_info (ctx.png, ctx.info);

  int channels = png_get_channels (ctx.png, ctx.info);
  if (channels != 3 && channels != 4)
    {
      snprintf (out->errmsg, sizeof out->errmsg, "Unsupported channels");
      out->err = NH_IMG_E_LIB;
      goto done;
    }
  size_t row_bytes = png_get_rowbytes (ctx.png, ctx.info);
  if (row_bytes > SIZE_MAX / height)
    {
      out->err = NH_IMG_E_NOMEM;
      goto done;
    }
  ctx.rows_mem = malloc (row_bytes * height);
  ctx.rows = malloc (height * sizeof *ctx.rows);
  if (!ctx.rows_mem || !ctx.rows)
    {
      out->err = NH_IMG_E_NOMEM;
      goto done;
    }
  for (png_uint_32 i = 0; i < height; i++)
    ctx.rows[i] = ctx.rows_mem + (size_t) i * row_bytes;

  png_read_image (ctx.png, ctx.rows);
  png_read_end (ctx.png, ctx.info);

  if (channels == 4 && transparent_p)
    {
      out->mask = malloc ((size_t) width * height);
      if (!out->mask)
	{
	  out->err = NH_IMG_E_NOMEM;
	  goto done;
	}
    }

  for (png_uint_32 y = 0; y < height; y++)
    {
      const uint8_t *q = ctx.rows[y];
      for (png_uint_32 x = 0; x < width; x++)
	{
	  uint32_t r = *q++, g = *q++, b = *q++;
	  out->pixels[(size_t) y * width + x] = (r << 16) | (g << 8) | b;
	  if (channels == 4)
	    {
	      if (out->mask)
		out->mask[(size_t) y * width + x] = *q > 0 ? 255 : 0;
	      q++;
	    }
	}
    }

  png_color_16p bkgd;
  if (png_get_bKGD (ctx.png, ctx.info, &bkgd))
    {
      out->bkgd_valid = true;
      out->bkgd[0] = bkgd->red;
      out->bkgd[1] = bkgd->green;
      out->bkgd[2] = bkgd->blue;
    }
  NH_IMG_COUNT ();

 done:
  c = &ctx;
  png_destroy_read_struct (&c->png, &c->info, &c->end);
  free (c->rows_mem);
  free (c->rows);
  nh_img_in_free (&in);
  if (out->err)
    {
      free (out->pixels);
      free (out->mask);
      out->pixels = NULL;
      out->mask = NULL;
    }
  return out->err;
}

#endif /* HAVE_PNG */


/* JPEG (jpeg_load_body).  */

#ifdef HAVE_JPEG

struct nh_jpeg_err
{
  struct jpeg_error_mgr pub;
  jmp_buf jb;
  struct nh_img_out *out;
};

struct nh_jpeg_src
{
  struct jpeg_source_mgr mgr;
  bool lenient;		/* a file: read in chunks as the stdio source does */
  bool finished;
  JOCTET eoi[2];
  JOCTET *buf;		/* the file source's 8192-byte buffer */
  const uint8_t *rest;	/* what the file source has not handed out yet */
  size_t left;
};

static void
nh_jpeg_exit (j_common_ptr cinfo)
{
  struct nh_jpeg_err *e = (struct nh_jpeg_err *) cinfo->err;
  char buf[JMSG_LENGTH_MAX];
  cinfo->err->format_message (cinfo, buf);
  snprintf (e->out->errmsg, sizeof e->out->errmsg, "%s", buf);
  longjmp (e->jb, 1);
}

static void
nh_jpeg_init_source (j_decompress_ptr cinfo)
{
}

static void
nh_jpeg_term_source (j_decompress_ptr cinfo)
{
}

static boolean
nh_jpeg_fill (j_decompress_ptr cinfo)
{
  struct nh_jpeg_src *s = (struct nh_jpeg_src *) cinfo->src;
  s->finished = true;
  s->eoi[0] = (JOCTET) 0xFF;
  s->eoi[1] = (JOCTET) JPEG_EOI;
  s->mgr.next_input_byte = s->eoi;
  s->mgr.bytes_in_buffer = 2;
  return 1;
}

/* The file source of image.c's jpeg_file_src, over the bytes already
   read: 8192-byte chunks, the fake EOI once, and then (as the stdio
   source does) no refill, so a decoder that reads on past the end sees
   the stale buffer.  The oracle holds this equal to the loader.  */
#define NH_JPEG_CHUNK 8192

static boolean
nh_jpeg_fill_file (j_decompress_ptr cinfo)
{
  struct nh_jpeg_src *s = (struct nh_jpeg_src *) cinfo->src;
  if (!s->finished)
    {
      size_t n = s->left < NH_JPEG_CHUNK ? s->left : NH_JPEG_CHUNK;
      if (n > 0)
	{
	  memcpy (s->buf, s->rest, n);
	  s->rest += n;
	  s->left -= n;
	  s->mgr.bytes_in_buffer = n;
	}
      else
	{
	  WARNMS (cinfo, JWRN_JPEG_EOF);
	  s->finished = true;
	  s->buf[0] = (JOCTET) 0xFF;
	  s->buf[1] = (JOCTET) JPEG_EOI;
	  s->mgr.bytes_in_buffer = 2;
	}
      s->mgr.next_input_byte = s->buf;
    }
  return 1;
}

static void
nh_jpeg_skip_file (j_decompress_ptr cinfo, long num_bytes)
{
  struct nh_jpeg_src *s = (struct nh_jpeg_src *) cinfo->src;
  while (num_bytes > 0 && !s->finished)
    {
      if ((size_t) num_bytes <= s->mgr.bytes_in_buffer)
	{
	  s->mgr.bytes_in_buffer -= num_bytes;
	  s->mgr.next_input_byte += num_bytes;
	  break;
	}
      num_bytes -= s->mgr.bytes_in_buffer;
      s->mgr.bytes_in_buffer = 0;
      s->mgr.next_input_byte = NULL;
      nh_jpeg_fill_file (cinfo);
    }
}

static void
nh_jpeg_skip (j_decompress_ptr cinfo, long num_bytes)
{
  struct nh_jpeg_src *s = (struct nh_jpeg_src *) cinfo->src;
  if (num_bytes <= 0)
    return;
  if ((size_t) num_bytes > s->mgr.bytes_in_buffer)
    ERREXIT (cinfo, JERR_INPUT_EOF);
  s->mgr.bytes_in_buffer -= num_bytes;
  s->mgr.next_input_byte += num_bytes;
}

static int
nh_jpeg_decode (const uint8_t *data, size_t len,
		const struct nh_img_params *p, struct nh_img_out *out)
{
  struct nh_img_in in;
  struct nh_jpeg_err err;
  struct nh_jpeg_err *volatile ev = &err;
  struct nh_img_in *volatile inv = &in;
  struct jpeg_decompress_struct cinfo;
  struct jpeg_decompress_struct *volatile cv = &cinfo;

  if (nh_img_input (data, len, p, &in, out))
    return out->err;
  memset (&cinfo, 0, sizeof cinfo);
  err.out = out;
  cinfo.err = jpeg_std_error (&err.pub);
  err.pub.error_exit = nh_jpeg_exit;
  if (setjmp (err.jb))
    {
      if (!ev->out->err)
	ev->out->err = NH_IMG_E_LIB;
      goto done;
    }

  jpeg_CreateDecompress (&cinfo, JPEG_LIB_VERSION, sizeof cinfo);
  {
    struct nh_jpeg_src *s
      = cinfo.mem->alloc_small ((j_common_ptr) &cinfo, JPOOL_PERMANENT,
				sizeof *s);
    cinfo.src = &s->mgr;
    s->lenient = data == NULL;
    s->finished = false;
    s->mgr.init_source = nh_jpeg_init_source;
    s->mgr.resync_to_restart = jpeg_resync_to_restart;
    s->mgr.term_source = nh_jpeg_term_source;
    if (s->lenient)
      {
	s->buf = cinfo.mem->alloc_small ((j_common_ptr) &cinfo,
					 JPOOL_PERMANENT, NH_JPEG_CHUNK);
	s->rest = in.p;
	s->left = in.len;
	s->mgr.fill_input_buffer = nh_jpeg_fill_file;
	s->mgr.skip_input_data = nh_jpeg_skip_file;
	s->mgr.bytes_in_buffer = 0;
	s->mgr.next_input_byte = NULL;
      }
    else
      {
	s->mgr.fill_input_buffer = nh_jpeg_fill;
	s->mgr.skip_input_data = nh_jpeg_skip;
	s->mgr.bytes_in_buffer = in.len;
	s->mgr.next_input_byte = in.p;
      }
  }

  jpeg_read_header (&cinfo, 1);
  jpeg_start_decompress (&cinfo);
  int width = cinfo.output_width, height = cinfo.output_height;
  int ncomp = cinfo.output_components;
  int ir, ig, ib;
  if (ncomp > 2)
    ir = 0, ig = 1, ib = 2;
  else if (ncomp > 1)
    ir = 0, ig = 1, ib = 0;
  else
    ir = 0, ig = 0, ib = 0;
  if (!nh_core_size_ok (p, width, height))
    {
      out->err = NH_IMG_E_SIZE;
      goto done;
    }
  out->width = width;
  out->height = height;
  out->pixels = malloc ((size_t) width * height * sizeof *out->pixels);
  if (!out->pixels)
    {
      out->err = NH_IMG_E_NOMEM;
      goto done;
    }
  JSAMPARRAY buffer
    = cinfo.mem->alloc_sarray ((j_common_ptr) &cinfo, JPOOL_IMAGE,
			       (JDIMENSION) width * ncomp, 1);
  for (int y = 0; y < height; y++)
    {
      jpeg_read_scanlines (&cinfo, buffer, 1);
      for (int x = 0; x < width; x++)
	{
	  int off = x * ncomp;
	  uint32_t r = buffer[0][off + ir], g = buffer[0][off + ig],
	    b = buffer[0][off + ib];
	  out->pixels[(size_t) y * width + x] = (r << 16) | (g << 8) | b;
	}
    }
  jpeg_finish_decompress (&cinfo);
  NH_IMG_COUNT ();

 done:
  if (cv->mem)
    jpeg_destroy_decompress (cv);
  nh_img_in_free (inv);
  if (out->err)
    {
      free (out->pixels);
      free (out->mask);
      out->pixels = NULL;
      out->mask = NULL;
    }
  return out->err;
}

#endif /* HAVE_JPEG */


/* GIF (gif_load).  Needs giflib 5.  */

#if defined HAVE_GIF && GIFLIB_MAJOR >= 5

struct nh_gif_src
{
  const uint8_t *bytes;
  size_t len, index;
};

static int
nh_gif_read (GifFileType *file, GifByteType *buf, int len)
{
  struct nh_gif_src *s = file->UserData;
  if (len < 0 || (size_t) len > s->len - s->index)
    return -1;
  memcpy (buf, s->bytes + s->index, len);
  s->index += len;
  return len;
}

static void
nh_gif_errmsg (struct nh_img_out *out, int code)
{
# if GIFLIB_MAJOR + (GIFLIB_MINOR >= 1) > 5
  const char *s = GifErrorString (code);
  if (s)
    snprintf (out->errmsg, sizeof out->errmsg, "%s", s);
# else
  (void) code;
# endif
}

static int
nh_gif_decode (const uint8_t *data, size_t len,
	       const struct nh_img_params *p, struct nh_img_out *out)
{
  struct nh_img_centry *e = NULL;
  GifFileType *gif = NULL;
  uint32_t *canvas = NULL;
  bool want_cache = p->frame_index >= 0 && p->key != 0;
  int idx = p->frame_index >= 0 ? p->frame_index : 0;
  int pos = -1, width, height, gif_err = 0;
  size_t data_bytes = 0;

  if (want_cache && (e = nh_img_cache_take (NH_IMG_GIF, p->key)))
    {
      gif = e->gif;
      canvas = e->canvas;
      pos = e->pos;
      data_bytes = e->bytes - (size_t) gif->SWidth * gif->SHeight * 4;
      if (e->frame_bg != p->frame_bg_pixel || e->bg_set != p->gif_bg_set
	  || (e->bg_set && e->bg != p->gif_bg_pixel))
	pos = -1;
    }
  if (!gif)
    {
      struct nh_img_in in;
      struct nh_gif_src src;
      if (nh_img_input (data, len, p, &in, out))
	return out->err;
      data_bytes = in.len;
      src.bytes = in.p;
      src.len = in.len;
      src.index = 0;
      gif = DGifOpen (&src, nh_gif_read, &gif_err);
      if (!gif)
	{
	  nh_gif_errmsg (out, gif_err);
	  nh_img_in_free (&in);
	  return nh_img_fail (out, NH_IMG_E_GIF_OPEN);
	}
      if (!nh_core_size_ok (p, gif->SWidth, gif->SHeight))
	{
	  nh_gif_close (gif);
	  nh_img_in_free (&in);
	  return nh_img_fail (out, NH_IMG_E_SIZE);
	}
      int rc = DGifSlurp (gif);
      if (rc == GIF_ERROR || gif->ImageCount <= 0)
	{
	  nh_gif_errmsg (out, gif->Error);
	  nh_gif_close (gif);
	  nh_img_in_free (&in);
	  return nh_img_fail (out, NH_IMG_E_GIF_READ);
	}
      nh_img_in_free (&in);
      width = gif->SWidth;
      height = gif->SHeight;
      for (int j = 0; j < gif->ImageCount; j++)
	{
	  struct SavedImage *s = gif->SavedImages + j;
	  if (s->ImageDesc.Width < 0 || s->ImageDesc.Height < 0
	      || s->ImageDesc.Top < 0 || s->ImageDesc.Left < 0
	      || s->ImageDesc.Top + s->ImageDesc.Height > height
	      || s->ImageDesc.Left + s->ImageDesc.Width > width)
	    {
	      nh_gif_close (gif);
	      return nh_img_fail (out, NH_IMG_E_GIF_SUBIMAGE);
	    }
	}
    }
  width = gif->SWidth;
  height = gif->SHeight;

  int err = 0;
  if (idx < 0 || idx >= gif->ImageCount)
    {
      out->err_arg = idx;
      err = NH_IMG_E_GIF_INDEX;
    }
  else if (!nh_core_size_ok (p, width, height))
    err = NH_IMG_E_SIZE;
  else if (!canvas
	   && !(canvas = malloc ((size_t) width * height * sizeof *canvas)))
    err = NH_IMG_E_NOMEM;
  if (err)
    {
      nh_gif_close (gif);
      free (canvas);
      nh_img_centry_free (e ? (e->gif = NULL, e->canvas = NULL, e) : NULL);
      return nh_img_fail (out, err);
    }

  int top = gif->SavedImages[0].ImageDesc.Top;
  int left = gif->SavedImages[0].ImageDesc.Left;
  int bot = top + gif->SavedImages[0].ImageDesc.Height;
  int right = left + gif->SavedImages[0].ImageDesc.Width;
  uint32_t frame_bg = p->frame_bg_pixel;

  /* As gif_load: the part outside the first subimage is the frame
     background, refilled on every call, cached canvas or not.  */
  for (int y = 0; y < top; y++)
    for (int x = 0; x < width; x++)
      canvas[x + (size_t) y * width] = frame_bg;
  for (int y = bot; y < height; y++)
    for (int x = 0; x < width; x++)
      canvas[x + (size_t) y * width] = frame_bg;
  for (int y = top; y < bot; y++)
    {
      for (int x = 0; x < left; x++)
	canvas[x + (size_t) y * width] = frame_bg;
      for (int x = right; x < width; x++)
	canvas[x + (size_t) y * width] = frame_bg;
    }

  int start = (e && pos >= 0 && pos == idx - 1) ? idx : 0;
  for (int j = start; j <= idx; j++)
    {
      struct SavedImage *sub = gif->SavedImages + j;
      unsigned char *raster = (unsigned char *) sub->RasterBits;
      int sw = sub->ImageDesc.Width, sh = sub->ImageDesc.Height;
      int st = sub->ImageDesc.Top, sl = sub->ImageDesc.Left;
      GraphicsControlBlock gcb;
      DGifSavedExtensionToGCB (gif, j, &gcb);
      int disposal = gcb.DisposalMode;
      int transparency = gcb.TransparentColor;
      if (j == 0)
	disposal = DISPOSE_BACKGROUND;
      if (disposal == DISPOSAL_UNSPECIFIED)
	disposal = DISPOSE_DO_NOT;
      if (disposal == DISPOSE_PREVIOUS)
	disposal = DISPOSE_DO_NOT;
      ColorMapObject *map = sub->ImageDesc.ColorMap;
      if (!map)
	map = gif->SColorMap;
      uint32_t colors[256] = { 0, };
      if (map)
	for (int i = 0; i < map->ColorCount && i < 256; i++)
	  {
	    if (transparency == i)
	      colors[i] = p->gif_bg_set ? p->gif_bg_pixel : frame_bg;
	    else
	      colors[i] = ((uint32_t) map->Colors[i].Red << 16)
		| ((uint32_t) map->Colors[i].Green << 8)
		| map->Colors[i].Blue;
	  }
      for (int y = 0; y < sh; y++)
	for (int x = 0; x < sw; x++)
	  {
	    int c = raster[(size_t) y * sw + x];
	    if (transparency != c || disposal != DISPOSE_DO_NOT)
	      canvas[x + sl + (size_t) (y + st) * width] = colors[c];
	  }
      NH_IMG_COUNT ();
    }

  out->width = width;
  out->height = height;
  out->has_corners = true;
  out->corners[0] = top;
  out->corners[1] = left;
  out->corners[2] = bot;
  out->corners[3] = right;
  out->frame_count = gif->ImageCount;
  struct SavedImage *cur = gif->SavedImages + idx;
  struct nh_img_ext **tail = &out->ext;
  for (int i = 0; i < cur->ExtensionBlockCount; i++)
    {
      ExtensionBlock *b = cur->ExtensionBlocks + i;
      struct nh_img_ext *x = calloc (1, sizeof *x);
      if (!x)
	break;
      x->function = b->Function;
      x->len = b->ByteCount;
      x->bytes = malloc (b->ByteCount ? b->ByteCount : 1);
      if (!x->bytes)
	{
	  free (x);
	  break;
	}
      memcpy (x->bytes, b->Bytes, b->ByteCount);
      *tail = x;
      tail = &x->next;
      out->has_delay = true;
      if (b->Function == 249 && b->ByteCount == 4)
	out->delay = (b->Bytes[2] << 8) | b->Bytes[1];
    }

  if (want_cache)
    {
      out->pixels = malloc ((size_t) width * height * sizeof *out->pixels);
      if (out->pixels)
	memcpy (out->pixels, canvas, (size_t) width * height * 4);
      if (!e)
	{
	  e = calloc (1, sizeof *e);
	  if (e)
	    {
	      e->fmt = NH_IMG_GIF;
	      e->key = p->key;
	    }
	}
      if (e)
	{
	  e->gif = gif;
	  e->canvas = canvas;
	  e->pos = idx;
	  e->frame_bg = frame_bg;
	  e->bg_set = p->gif_bg_set;
	  e->bg = p->gif_bg_pixel;
	  e->bytes = (size_t) width * height * 4 + data_bytes;
	  nh_img_cache_put (e);
	}
      else
	{
	  nh_gif_close (gif);
	  free (canvas);
	}
    }
  else
    {
      out->pixels = canvas;
      nh_gif_close (gif);
    }
  if (!out->pixels)
    {
      nh_img_out_free (out);
      return nh_img_fail (out, NH_IMG_E_NOMEM);
    }
  return 0;
}

#endif /* HAVE_GIF && GIFLIB_MAJOR >= 5 */


/* WebP (webp_load).  */

#ifdef HAVE_WEBP

static int
nh_webp_decode (const uint8_t *data, size_t len,
		const struct nh_img_params *p, struct nh_img_out *out)
{
  struct nh_img_centry *e = NULL;
  bool want_cache = p->frame_index >= 0 && p->key != 0;
  WebPBitstreamFeatures features;
  memset (&features, 0, sizeof features);
  uint8_t *decoded_cpy = NULL;
  const uint8_t *decoded = NULL;
  int width = -1, height = -1, frames = -1, duration = -1;
  int err = 0;

  if (want_cache)
    e = nh_img_cache_take (NH_IMG_WEBP, p->key);

  if (e)
    {
      features.has_alpha = e->has_alpha;
      features.has_animation = 1;
    }
  else
    {
      struct nh_img_in in;
      if (nh_img_input (data, len, p, &in, out))
	return out->err;
      if (WebPGetFeatures (in.p, in.len, &features) != VP8_STATUS_OK)
	{
	  nh_img_in_free (&in);
	  return nh_img_fail (out, NH_IMG_E_WEBP_HEADERS);
	}
      if (!nh_core_size_ok (p, features.width, features.height))
	{
	  nh_img_in_free (&in);
	  return nh_img_fail (out, NH_IMG_E_SIZE);
	}
      if (features.has_animation)
	{
	  e = calloc (1, sizeof *e);
	  if (!e)
	    {
	      nh_img_in_free (&in);
	      return nh_img_fail (out, NH_IMG_E_NOMEM);
	    }
	  e->fmt = NH_IMG_WEBP;
	  e->key = p->key;
	  e->pos = -1;
	  e->has_alpha = features.has_alpha;
	  if (in.owned)
	    {
	      e->contents = in.owned;
	      in.owned = NULL;
	    }
	  else
	    {
	      e->contents = malloc (in.len ? in.len : 1);
	      if (e->contents)
		memcpy (e->contents, in.p, in.len);
	    }
	  if (!e->contents)
	    err = NH_IMG_E_NOMEM;
	  else
	    {
	      WebPData wd = { .bytes = e->contents, .size = in.len };
	      e->dec = WebPAnimDecoderNew (&wd, NULL);
	      WebPAnimInfo info;
	      if (!e->dec)
		err = NH_IMG_E_WEBP_PARSE;
	      else if (!WebPAnimDecoderGetInfo (e->dec, &info))
		err = NH_IMG_E_WEBP_INFO;
	      else
		{
		  e->width = info.canvas_width;
		  e->height = info.canvas_height;
		  e->frames = info.frame_count;
		  e->bytes = in.len + (size_t) e->width * e->height * 8;
		  if (!nh_core_size_ok (p, e->width, e->height))
		    err = NH_IMG_E_SIZE;
		}
	    }
	  nh_img_in_free (&in);
	  if (err)
	    {
	      nh_img_centry_free (e);
	      return nh_img_fail (out, err);
	    }
	}
      else
	{
	  decoded_cpy = features.has_alpha
	    ? WebPDecodeRGBA (in.p, in.len, &width, &height)
	    : WebPDecodeRGB (in.p, in.len, &width, &height);
	  decoded = decoded_cpy;
	  nh_img_in_free (&in);
	  if (decoded)
	    NH_IMG_COUNT ();
	}
    }

  bool alpha_blend;
  if (e)
    {
      width = e->width;
      height = e->height;
      frames = e->frames;
      int idx = p->frame_index >= 0 ? (p->frame_index < frames
				       ? p->frame_index : frames) : 0;
      if (e->pos == idx && e->last)
	{
	  decoded = e->last;
	  duration = e->duration;
	}
      else
	{
	  if (e->pos >= idx)
	    {
	      WebPAnimDecoderReset (e->dec);
	      e->timestamp = 0;
	      e->pos = -1;
	    }
	  uint8_t *frame = NULL;
	  while (e->pos < idx && WebPAnimDecoderHasMoreFrames (e->dec))
	    {
	      int ts;
	      if (!WebPAnimDecoderGetNext (e->dec, &frame, &ts))
		{
		  out->err_arg = e->pos + 1;
		  nh_img_centry_free (e);
		  return nh_img_fail (out, NH_IMG_E_WEBP_FRAME);
		}
	      NH_IMG_COUNT ();
	      duration = ts - e->timestamp;
	      e->timestamp = ts;
	      e->pos++;
	      decoded = frame;
	    }
	  if (decoded && want_cache)
	    {
	      size_t n = (size_t) width * height * 4;
	      if (!e->last)
		e->last = malloc (n);
	      if (e->last)
		{
		  memcpy (e->last, decoded, n);
		  decoded = e->last;
		}
	      e->duration = duration;
	    }
	}
      alpha_blend = true;
    }
  else
    alpha_blend = features.has_alpha;

  if (!decoded)
    {
      if (e)
	nh_img_centry_free (e);
      WebPFree (decoded_cpy);
      return nh_img_fail (out, NH_IMG_E_WEBP_DECODE);
    }
  if (!nh_core_size_ok (p, width, height))
    {
      if (e)
	nh_img_centry_free (e);
      WebPFree (decoded_cpy);
      return nh_img_fail (out, NH_IMG_E_SIZE);
    }
  out->pixels = malloc ((size_t) width * height * sizeof *out->pixels);
  if (!out->pixels)
    {
      if (e)
	nh_img_centry_free (e);
      WebPFree (decoded_cpy);
      return nh_img_fail (out, NH_IMG_E_NOMEM);
    }
  int bgr = p->bg_set ? p->bg_r >> 8 : 0;
  int bgg = p->bg_set ? p->bg_g >> 8 : 0;
  int bgb = p->bg_set ? p->bg_b >> 8 : 0;
  const uint8_t *q = decoded;
  for (int y = 0; y < height; y++)
    for (int x = 0; x < width; x++)
      {
	int r, g, b;
	if (alpha_blend)
	  {
	    float a = (float) q[3] / UINT8_MAX;
	    r = (int) (a * q[0] + (1 - a) * bgr);
	    g = (int) (a * q[1] + (1 - a) * bgg);
	    b = (int) (a * q[2] + (1 - a) * bgb);
	    q += 4;
	  }
	else
	  {
	    r = *q++;
	    g = *q++;
	    b = *q++;
	  }
	out->pixels[(size_t) y * width + x]
	  = ((uint32_t) r << 16) | ((uint32_t) g << 8) | (uint32_t) b;
      }
  out->width = width;
  out->height = height;
  out->has_corners = true;
  out->corners[0] = 0;
  out->corners[1] = 0;
  out->corners[2] = height;
  out->corners[3] = width;
  if (features.has_animation)
    {
      out->animated = true;
      out->frame_count = frames;
      out->has_delay = true;
      out->delay = duration;
    }
  WebPFree (decoded_cpy);
  if (e)
    {
      if (want_cache)
	nh_img_cache_put (e);
      else
	nh_img_centry_free (e);
    }
  return 0;
}

#endif /* HAVE_WEBP */


/* The table (PD-11).  */

#ifdef HAVE_PNG
static const struct nh_img_decoder nh_img_c_png = { nh_png_decode };
#endif
#ifdef HAVE_JPEG
static const struct nh_img_decoder nh_img_c_jpeg = { nh_jpeg_decode };
#endif
#if defined HAVE_GIF && GIFLIB_MAJOR >= 5
static const struct nh_img_decoder nh_img_c_gif = { nh_gif_decode };
#endif
#ifdef HAVE_WEBP
static const struct nh_img_decoder nh_img_c_webp = { nh_webp_decode };
#endif

const struct nh_img_decoder *nh_img_decoders[NH_IMG_FMT_N] = {
#ifdef HAVE_PNG
  [NH_IMG_PNG] = &nh_img_c_png,
#endif
#ifdef HAVE_JPEG
  [NH_IMG_JPEG] = &nh_img_c_jpeg,
#endif
#if defined HAVE_GIF && GIFLIB_MAJOR >= 5
  [NH_IMG_GIF] = &nh_img_c_gif,
#endif
#ifdef HAVE_WEBP
  [NH_IMG_WEBP] = &nh_img_c_webp,
#endif
};

int
nh_img_decode (int fmt, const uint8_t *data, size_t len,
	       const struct nh_img_params *params, struct nh_img_out *out)
{
  if (fmt < 0 || fmt >= NH_IMG_FMT_N || !nh_img_decoders[fmt])
    return nh_img_fail (out, NH_IMG_E_UNSUPPORTED);
  return nh_img_decoders[fmt]->decode (data, len, params, out);
}


/* The synchronous loaders' entry, nh_img_sync_load, is in image.c, which
   owns the frame and image internals it needs; so are the parts of the
   pending entries that touch them (Task 29, at the end of this file).  */

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
#include "window.h"
#include "keyboard.h"
#include "nidhogg-workers-phase.h"

#include <fcntl.h>
#include <sys/stat.h>

/* The header path below, like the pending entries further down, works
   on image.c's specs and frames, and image.c is built only with a
   window system: a tty-only build has no images, so it has neither.
   The decoder cores and their test primitives stay.  */
#ifdef HAVE_WINDOW_SYSTEM

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

#endif /* HAVE_WINDOW_SYSTEM */

/* Test primitives.  */

static const char *const nh_img_err_names[NH_IMG_E_N] = {
  "ok", "unsupported", "open", "not-image", "lib", "size", "silent", "nomem",
  "gif-open", "gif-read", "gif-subimage", "gif-index", "webp-headers",
  "webp-parse", "webp-info", "webp-frame", "webp-decode"
};

struct nh_img_tjob
{
  int fmt;
  uint8_t *data;
  size_t len;
  char *file;
  struct nh_img_params params;
  struct nh_img_out out;
  pthread_t thread;
  pthread_mutex_t lock;
  pthread_cond_t cond;
  bool go;
};

#define NH_IMG_TJOBS 8
static struct nh_img_tjob *nh_img_tjobs[NH_IMG_TJOBS];

static void *
nh_img_tjob_run (void *arg)
{
  struct nh_img_tjob *j = arg;
  nh_wk_lock (&j->lock);
  while (!j->go)
    pthread_cond_wait (&j->cond, &j->lock);
  pthread_mutex_unlock (&j->lock);
  nh_img_decode (j->fmt, j->data, j->len, &j->params, &j->out);
  return NULL;
}

static Lisp_Object
nh_img_plist_get (Lisp_Object plist, const char *key)
{
  for (; CONSP (plist) && CONSP (XCDR (plist)); plist = XCDR (XCDR (plist)))
    if (SYMBOLP (XCAR (plist))
	&& strcmp (SSDATA (SYMBOL_NAME (XCAR (plist))), key) == 0)
      return XCAR (XCDR (plist));
  return Qnil;
}

static Lisp_Object
nh_img_out_lisp (struct nh_img_out *out)
{
  Lisp_Object r = Qnil;
  if (out->err)
    {
      r = list2 (intern_c_string (":error"),
		 intern_c_string (nh_img_err_names[out->err]));
      r = nconc2 (r, list2 (intern_c_string (":message"),
			    build_string (out->errmsg)));
      r = nconc2 (r, list2 (intern_c_string (":arg"),
			    make_fixnum (out->err_arg)));
      return r;
    }
  size_t n = (size_t) out->width * out->height;
  r = list2 (intern_c_string (":width"), make_fixnum (out->width));
  r = nconc2 (r, list2 (intern_c_string (":height"), make_fixnum (out->height)));
  r = nconc2 (r, list2 (intern_c_string (":pixels"),
			make_unibyte_string ((char *) out->pixels, n * 4)));
  r = nconc2 (r, list2 (intern_c_string (":mask"),
			out->mask ? make_unibyte_string ((char *) out->mask, n)
			: Qnil));
  r = nconc2 (r, list2 (intern_c_string (":frames"),
			make_fixnum (out->frame_count)));
  r = nconc2 (r, list2 (intern_c_string (":delay"),
			out->has_delay ? make_fixnum (out->delay) : Qnil));
  r = nconc2 (r, list2 (intern_c_string (":corners"),
			out->has_corners
			? list4 (make_fixnum (out->corners[0]),
				 make_fixnum (out->corners[1]),
				 make_fixnum (out->corners[2]),
				 make_fixnum (out->corners[3]))
			: Qnil));
  r = nconc2 (r, list2 (intern_c_string (":bkgd"),
			out->bkgd_valid
			? list3 (make_fixnum (out->bkgd[0]),
				 make_fixnum (out->bkgd[1]),
				 make_fixnum (out->bkgd[2]))
			: Qnil));
  Lisp_Object warns = Qnil;
  for (int i = out->nwarn - 1; i >= 0; i--)
    warns = Fcons (build_string (out->warn[i]), warns);
  r = nconc2 (r, list2 (intern_c_string (":warnings"), warns));
  return r;
}

/* Fill TJ from the Lisp SPEC and PARAMS; the data is copied.  */
static struct nh_img_tjob *
nh_img_tjob_make (Lisp_Object spec, Lisp_Object params)
{
  CHECK_CONS (spec);
  Lisp_Object plist = XCDR (spec);
  Lisp_Object type = nh_img_plist_get (plist, ":type");
  Lisp_Object file = nh_img_plist_get (plist, ":file");
  Lisp_Object dat = nh_img_plist_get (plist, ":data");
  Lisp_Object index = nh_img_plist_get (plist, ":index");
  CHECK_SYMBOL (type);
  const char *tn = SSDATA (SYMBOL_NAME (type));
  int fmt = strcmp (tn, "png") == 0 ? NH_IMG_PNG
    : strcmp (tn, "jpeg") == 0 ? NH_IMG_JPEG
    : strcmp (tn, "gif") == 0 ? NH_IMG_GIF
    : strcmp (tn, "webp") == 0 ? NH_IMG_WEBP : -1;
  if (fmt < 0)
    error ("Unsupported image type");
  struct nh_img_tjob *j = calloc (1, sizeof *j);
  if (!j)
    memory_full (sizeof *j);
  j->fmt = fmt;
  pthread_mutex_init (&j->lock, NULL);
  pthread_cond_init (&j->cond, NULL);
  struct nh_img_params *q = &j->params;
  q->frame_index = FIXNATP (index) ? XFIXNAT (index) : -1;
  q->limit_w = q->limit_h = -1;
  if (STRINGP (dat))
    {
      j->len = SBYTES (dat);
      j->data = malloc (j->len ? j->len : 1);
      if (!j->data)
	memory_full (j->len);
      memcpy (j->data, SDATA (dat), j->len);
      if (q->frame_index >= 0)
	q->key = nh_img_hash (j->data, j->len);
    }
  else
    {
      CHECK_STRING (file);
      Lisp_Object f = ENCODE_FILE (Fexpand_file_name (file, Qnil));
      j->file = strdup (SSDATA (f));
      if (!j->file)
	memory_full (0);
      q->file = j->file;
      if (q->frame_index >= 0)
	q->key = nh_img_file_key (j->file);
    }
  Lisp_Object v;
  if (!NILP (v = nh_img_plist_get (params, ":bg")) && CONSP (v))
    {
      q->bg_set = true;
      q->bg_r = XFIXNUM (XCAR (v));
      q->bg_g = XFIXNUM (XCAR (XCDR (v)));
      q->bg_b = XFIXNUM (XCAR (XCDR (XCDR (v))));
    }
  if (FIXNUMP (v = nh_img_plist_get (params, ":frame-bg")))
    q->frame_bg_pixel = XFIXNUM (v);
  if (FIXNUMP (v = nh_img_plist_get (params, ":gif-bg")))
    {
      q->gif_bg_set = true;
      q->gif_bg_pixel = XFIXNUM (v);
    }
  if (FIXNUMP (v = nh_img_plist_get (params, ":limit")))
    q->limit_w = q->limit_h = XFIXNUM (v);
  return j;
}

static void
nh_img_tjob_free (struct nh_img_tjob *j)
{
  nh_img_out_free (&j->out);
  free (j->data);
  free (j->file);
  pthread_mutex_destroy (&j->lock);
  pthread_cond_destroy (&j->cond);
  free (j);
}

DEFUN ("nidhogg-workers--test-image-decode", Fnidhogg_workers__test_image_decode,
       Snidhogg_workers__test_image_decode, 1, 3, 0,
       doc: /* Decode image SPEC through the Lisp-free core (test primitive).
SPEC is (image :type TYPE :file F or :data D [:index N]).  PARAMS is a
plist of :bg (R G B, 16 bits each), :frame-bg PIXEL, :gif-bg PIXEL and
:limit PIXELS.  Value: a plist with :width, :height, :pixels (32-bit words
0x00RRGGBB, little endian), :mask, :frames, :delay, :corners, :bkgd and
:warnings, or with :error, :message and :arg.  The data is copied first.
With MODE `thread' the copy is decoded by a separate thread, held until
`nidhogg-workers--test-image-decode-finish', and the value is its handle.  */)
  (Lisp_Object spec, Lisp_Object params, Lisp_Object mode)
{
  struct nh_img_tjob *j = nh_img_tjob_make (spec, params);
  if (!EQ (mode, intern_c_string ("thread")))
    {
      nh_img_decode (j->fmt, j->data, j->len, &j->params, &j->out);
      Lisp_Object r = nh_img_out_lisp (&j->out);
      nh_img_tjob_free (j);
      return r;
    }
  int slot = 0;
  while (slot < NH_IMG_TJOBS && nh_img_tjobs[slot])
    slot++;
  if (slot == NH_IMG_TJOBS)
    {
      nh_img_tjob_free (j);
      error ("Too many pending test decodes");
    }
  /* The worker contract: the asynchronous signals blocked in the thread.  */
  sigset_t all, old;
  sigfillset (&all);
  sigdelset (&all, SIGSEGV);
  sigdelset (&all, SIGBUS);
  sigdelset (&all, SIGILL);
  sigdelset (&all, SIGFPE);
  sigdelset (&all, SIGABRT);
  sigdelset (&all, SIGTRAP);
  pthread_sigmask (SIG_BLOCK, &all, &old);
  int rc = pthread_create (&j->thread, NULL, nh_img_tjob_run, j);
  pthread_sigmask (SIG_SETMASK, &old, NULL);
  if (rc)
    {
      nh_img_tjob_free (j);
      error ("Cannot start the test decode thread");
    }
  nh_img_tjobs[slot] = j;
  return make_fixnum (slot);
}

DEFUN ("nidhogg-workers--test-image-decode-finish",
       Fnidhogg_workers__test_image_decode_finish,
       Snidhogg_workers__test_image_decode_finish, 1, 1, 0,
       doc: /* Release the held decode HANDLE, wait for it and return its result.  */)
  (Lisp_Object handle)
{
  CHECK_FIXNAT (handle);
  EMACS_INT slot = XFIXNAT (handle);
  if (slot >= NH_IMG_TJOBS || !nh_img_tjobs[slot])
    error ("No such test decode");
  struct nh_img_tjob *j = nh_img_tjobs[slot];
  nh_img_tjobs[slot] = NULL;
  nh_wk_lock (&j->lock);
  j->go = true;
  pthread_cond_broadcast (&j->cond);
  pthread_mutex_unlock (&j->lock);
  pthread_join (j->thread, NULL);
  Lisp_Object r = nh_img_out_lisp (&j->out);
  nh_img_tjob_free (j);
  return r;
}

DEFUN ("nidhogg-workers--test-decode-count", Fnidhogg_workers__test_decode_count,
       Snidhogg_workers__test_decode_count, 0, 1, 0,
       doc: /* Number of image frames the cores decoded or composed so far.
With non-nil RESET, also reset the count.  */)
  (Lisp_Object reset)
{
  return make_int (nh_img_decode_count (!NILP (reset)));
}

DEFUN ("nidhogg-workers--test-image-cache-clear",
       Fnidhogg_workers__test_image_cache_clear,
       Snidhogg_workers__test_image_cache_clear, 0, 0, 0,
       doc: /* Empty the animation cache of the image cores.  */)
  (void)
{
  nh_img_cache_clear ();
  return Qnil;
}


/* Pending entries and landing (Task 29); see the commentary at the top.  */

static int64_t
nh_img_now (void)
{
  struct timespec ts;
  clock_gettime (CLOCK_MONOTONIC, &ts);
  return (int64_t) ts.tv_sec * 1000000000 + ts.tv_nsec;
}

int
nh_img_phase_begin (int64_t *t0)
{
  *t0 = nh_img_now ();
  return NH_WK_PHASE_PUSH (NH_WK_PH_IMAGE, 0);
}

void
nh_img_phase_end (int depth, int64_t t0)
{
  NH_WK_PHASE_POP (depth);
  nh_wk_phase_ns_add (NH_WK_PH_IMAGE, 0, nh_img_now () - t0);
}

#ifdef HAVE_WINDOW_SYSTEM

/* A redisplay wait takes the budget in slices, so that a decode that ends
   early leaves the rest of the budget to the next image.  */
#define NH_IMG_SLICE_NS INT64_C (1000000)
/* How long a caller that needs the pixels of a pending entry waits before
   it loads them itself.  */
#define NH_IMG_PIXELS_WAIT_NS INT64_C (1000000000)

struct nh_img_job
{
  struct nh_wk_job job;
  int fmt;			/* enum nh_img_fmt */
  uint8_t *data;		/* a copy of :data, or NULL */
  size_t len;
  char *file;			/* the encoded file name, or NULL */
  struct nh_img_params params;
  struct nh_img_out out;
  int rc;
  /* The entry, as (cache, id, generation).  CACHE is only compared.  */
  const void *cache;
  ptrdiff_t id;
  uint64_t gen;
  bool consumed;		/* main thread: the result was taken */
};

/* A pending entry; main thread (lock holder) only.  */
struct nh_img_pend
{
  struct nh_img_pend *next;
  const void *cache;
  ptrdiff_t id;
  uint64_t gen;
  struct nh_img_job *j;		/* a reference */
  bool landed;			/* decoded: lands at the next redisplay */
  bool synthetic;		/* a test entry: no image behind it */
};

static struct nh_img_pend *nh_img_pends;
static uint64_t nh_img_gen;
static bool nh_img_any_landed;
/* True while redisplay looks an image up (nh_img_redisplay_lookup).  */
static bool nh_img_in_redisplay;
/* The windows the last landing marked (tests).  */
static Lisp_Object nh_img_marked;
/* The hashes of the specs whose decode failed after a deferral: they load
   synchronously from then on (a failed image is loaded again at every
   lookup, as today) rather than decode and land at every redisplay.  */
enum { NH_IMG_FAILED_N = 32 };
static EMACS_UINT nh_img_failed[NH_IMG_FAILED_N];
static int nh_img_failed_used;
/* Counters (tests).  */
static _Atomic int nh_img_live;
static EMACS_INT nh_img_n_landed, nh_img_n_dropped, nh_img_n_deferred;
/* The cache the test entries name.  */
static char nh_img_test_cache;

static bool
nh_img_defer_on (void)
{
  Lisp_Object v = find_symbol_value (Qnidhogg_image_defer);
  return !BASE_EQ (v, Qunbound) && !NILP (v);
}

static int
nh_img_core_format (int hfmt)
{
  switch (hfmt)
    {
    case NH_IMG_F_PNG:
      return NH_IMG_PNG;
    case NH_IMG_F_JPEG:
      return NH_IMG_JPEG;
    case NH_IMG_F_GIF:
      return NH_IMG_GIF;
    case NH_IMG_F_WEBP:
      return NH_IMG_WEBP;
    default:
      return -1;
    }
}

static bool
nh_img_failed_p (EMACS_UINT hash)
{
  int n = min (nh_img_failed_used, NH_IMG_FAILED_N);
  for (int i = 0; i < n; i++)
    if (nh_img_failed[i] == hash)
      return true;
  return false;
}

static void
nh_img_failed_add (EMACS_UINT hash)
{
  if (!nh_img_failed_p (hash))
    nh_img_failed[nh_img_failed_used++ % NH_IMG_FAILED_N] = hash;
  if (nh_img_failed_used >= 2 * NH_IMG_FAILED_N)
    nh_img_failed_used -= NH_IMG_FAILED_N;
}

/* The job, on nh-img.  */
static void
nh_img_job_run (struct nh_wk_job *job)
{
  struct nh_img_job *j = (struct nh_img_job *) job;
  nh_wk_pause_point (NH_WK_P_AFTER_HEADER);
  if (atomic_load (&job->cancel))
    {
      j->rc = nh_img_fail (&j->out, NH_IMG_E_SILENT);
      return;
    }
  if (j->params.frame_index >= 0)
    j->params.key = (j->data ? nh_img_hash (j->data, j->len)
		     : nh_img_file_key (j->file));
  j->rc = nh_img_decode (j->fmt, j->data, j->len, &j->params, &j->out);
}

/* The last reference is gone (either thread).  */
static void
nh_img_job_free (struct nh_wk_job *job)
{
  struct nh_img_job *j = (struct nh_img_job *) job;
  nh_img_out_free (&j->out);
  free (j->data);
  free (j->file);
  free (j);
  atomic_fetch_sub (&nh_img_live, 1);
}

static struct nh_img_pend *
nh_img_pend_find (const void *cache, ptrdiff_t id)
{
  for (struct nh_img_pend *r = nh_img_pends; r; r = r->next)
    if (r->cache == cache && r->id == id)
      return r;
  return NULL;
}

static void
nh_img_pend_unlink (struct nh_img_pend *r)
{
  for (struct nh_img_pend **pp = &nh_img_pends; *pp; pp = &(*pp)->next)
    if (*pp == r)
      {
	*pp = r->next;
	r->next = NULL;
	return;
      }
}

/* Drop R (unlinked) and the reference it holds.  */
static void
nh_img_pend_free (struct nh_img_pend *r)
{
  r->j->consumed = true;
  nh_wk_job_unref (&r->j->job);
  free (r);
}

/* R's entry is gone: its job is cancelled and its result dropped.  */
static void
nh_img_pend_forget (struct nh_img_pend *r)
{
  nh_img_pend_unlink (r);
  nh_wk_cancel (&r->j->job);
  nh_img_n_dropped++;
  nh_img_pend_free (r);
}

/* The drainer (PD-6): the job is done.  Its entry, if it is still there,
   lands at the next redisplay.  */
static void
nh_img_job_finish (struct nh_wk_job *job)
{
  struct nh_img_job *j = (struct nh_img_job *) job;
  if (j->consumed)
    return;
  struct nh_img_pend *r = nh_img_pend_find (j->cache, j->id);
  if (!r || r->gen != j->gen)
    {
      nh_img_n_dropped++;
      return;
    }
  r->landed = true;
  nh_img_any_landed = true;
  /* Ask for a redisplay: a wait for input redisplays after a timer check
     that ran something (process.c, keyboard.c).  */
  timers_run++;
}

static void
nh_img_job_init (struct nh_img_job *j, const void *cache, ptrdiff_t id)
{
  j->job.w = NH_WK_IMG;
  j->job.speculative = true;
  j->job.run = nh_img_job_run;
  j->job.finish = nh_img_job_finish;
  j->job.free = nh_img_job_free;
  atomic_store (&j->job.refs, 1);	/* the submitter's */
  j->cache = cache;
  j->id = id;
  j->gen = ++nh_img_gen;
  atomic_fetch_add (&nh_img_live, 1);
}

/* free_image: IMG's record goes, whatever freed it.  */
void
nh_img_freed (struct frame *f, struct image *img)
{
  if (!nh_img_pends)
    return;
  struct nh_img_pend *r = nh_img_pend_find (FRAME_IMAGE_CACHE (f), img->id);
  if (r)
    nh_img_pend_forget (r);
}

bool
nh_img_pending_p (struct frame *f, struct image *img)
{
  return nh_img_pends && nh_img_pend_find (FRAME_IMAGE_CACHE (f), img->id);
}

/* lookup_image's miss: see the commentary at the top.  */
bool
nh_img_lookup (struct frame *f, struct image *img)
{
  if (!nh_img_in_redisplay || !nh_img_on () || !nh_img_defer_on ()
      || nh_img_failed_p (img->hash))
    return false;
  Lisp_Object spec = img->spec;
  int hfmt = nh_img_spec_format (spec);
  int fmt = nh_img_core_format (hfmt);
  if (fmt < 0)
    return false;

  /* The header, here: the native size, refused where the loader would
     report an error.  Then the job's copies.  */
  int64_t t0;
  int depth = nh_img_phase_begin (&t0);
  bool scan = ((hfmt == NH_IMG_F_GIF || hfmt == NH_IMG_F_WEBP)
	       && !NILP (image_spec_value (spec, QCindex, NULL)));
  struct nh_img_scan sc;
  int w = 0, h = 0, dw, dh;
  bool ok = (nh_img_probe (spec, hfmt, scan, nh_img_index (spec), &sc, &w, &h)
	     && nh_img_size_ok (f, w, h)
	     && nh_img_displayed_size (f, spec, w, h, &dw, &dh));
  struct nh_img_job *j = ok ? calloc (1, sizeof *j) : NULL;
  if (j && !nh_img_job_inputs (f, img, fmt, &j->params, &j->data, &j->len,
			       &j->file))
    {
      free (j);
      j = NULL;
    }
  nh_img_phase_end (depth, t0);
  if (!j)
    return false;
  j->fmt = fmt;
  nh_img_job_init (j, FRAME_IMAGE_CACHE (f), img->id);
  if (!nh_wk_submit (&j->job))
    {
      nh_wk_job_unref (&j->job);
      return false;
    }

  /* Wait within the redisplay's budget (§8.3 item 4).  */
  int r = 0;
  while (atomic_load (&j->job.state) != NH_WK_J_DONE)
    {
      int64_t grant = nh_wk_budget_take (NH_IMG_SLICE_NS);
      if (grant <= 0)
	break;
      r = nh_wk_wait (&j->job, grant, NH_WK_PH_WORKER_WAIT, NH_WK_RS_DISPLAY);
      if (r != 0)
	break;
    }
  int st = atomic_load (&j->job.state);
  if (st == NH_WK_J_DONE)
    {
      /* In time: this redisplay shows it.  */
      j->consumed = true;
      depth = nh_img_phase_begin (&t0);
      img->load_failed_p = !nh_img_load_result (f, img, fmt, &j->out, j->rc);
      nh_img_phase_end (depth, t0);
      nh_wk_job_unref (&j->job);
      return true;
    }
  struct nh_img_pend *p = (st == NH_WK_J_CANCELLED || r < 0 ? NULL
			   : calloc (1, sizeof *p));
  if (!p)
    {
      nh_wk_cancel (&j->job);
      j->consumed = true;
      nh_wk_job_unref (&j->job);
      return false;
    }

  /* Pending: the header's size, no pixels.  lookup_image goes on as after
     a load, so the entry gets its final size.  */
  p->cache = j->cache;
  p->id = j->id;
  p->gen = j->gen;
  p->j = j;			/* the submitter's reference */
  p->next = nh_img_pends;
  nh_img_pends = p;
  img->width = w;
  img->height = h;
  img->load_failed_p = false;
  nh_img_n_deferred++;
  nh_wk_deferral_add (1);
  return true;
}

static bool
nh_img_window_shows (struct window *w, ptrdiff_t id)
{
  struct glyph_matrix *m = w->current_matrix;
  if (!m)
    return false;
  for (int i = 0; i < m->nrows; i++)
    {
      struct glyph_row *row = MATRIX_ROW (m, i);
      if (!row->enabled_p)
	continue;
      for (int a = LEFT_MARGIN_AREA; a < LAST_AREA; a++)
	for (struct glyph *g = row->glyphs[a], *e = g + row->used[a]; g < e; g++)
	  if (g->type == IMAGE_GLYPH && g->u.img_id == id)
	    return true;
    }
  return false;
}

static void
nh_img_mark_window (Lisp_Object window, ptrdiff_t id)
{
  struct window *w = XWINDOW (window);
  if (!nh_img_window_shows (w, id))
    return;
  /* The rows are unchanged (same image, same size), so only a cleared
     current matrix makes the update draw them again.  */
  clear_glyph_matrix (w->current_matrix);
  wset_redisplay (w);
  redisplay_other_windows ();
  if (NILP (Fmemq (window, nh_img_marked)))
    nh_img_marked = Fcons (window, nh_img_marked);
}

static void
nh_img_mark_tree (Lisp_Object window, ptrdiff_t id)
{
  while (WINDOWP (window))
    {
      struct window *w = XWINDOW (window);
      if (WINDOWP (w->contents))
	nh_img_mark_tree (w->contents, id);
      else
	nh_img_mark_window (window, id);
      window = w->next;
    }
}

/* Mark for redisplay the windows that show entry ID of CACHE, on every
   frame that shares the cache, and no other (§8.3 item 7).  */
static void
nh_img_mark (const void *cache, ptrdiff_t id)
{
  Lisp_Object tail, frame;
  FOR_EACH_FRAME (tail, frame)
    {
      struct frame *f = XFRAME (frame);
      if (!FRAME_WINDOW_P (f) || FRAME_IMAGE_CACHE (f) != cache)
	continue;
      /* The root window's siblings: the minibuffer window.  */
      nh_img_mark_tree (FRAME_ROOT_WINDOW (f), id);
      if (WINDOWP (f->tab_bar_window))
	nh_img_mark_window (f->tab_bar_window, id);
#ifndef HAVE_EXT_TOOL_BAR
      if (WINDOWP (f->tool_bar_window))
	nh_img_mark_window (f->tool_bar_window, id);
#endif
    }
}

static struct frame *
nh_img_cache_frame (const void *cache)
{
  Lisp_Object tail, frame;
  FOR_EACH_FRAME (tail, frame)
    {
      struct frame *f = XFRAME (frame);
      if (FRAME_WINDOW_P (f) && FRAME_IMAGE_CACHE (f) == cache)
	return f;
    }
  return NULL;
}

/* Land R (unlinked) into IMG on F: from its job's result, or with SYNC
   (the job could not finish) by the loader.  */
static void
nh_img_land_pend (struct frame *f, struct image *img, struct nh_img_pend *r,
		  bool sync)
{
  struct nh_img_job *j = r->j;
  ptrdiff_t id = r->id;
  const void *cache = r->cache;
  j->consumed = true;
  int64_t t0;
  int depth = nh_img_phase_begin (&t0);
  nh_img_land (f, img, j->fmt, sync ? NULL : &j->out, j->rc);
  nh_img_phase_end (depth, t0);
  nh_img_pend_free (r);
  if (img->load_failed_p)
    nh_img_failed_add (img->hash);
  nh_img_n_landed++;
  nh_img_mark (cache, id);
}

/* The top of redisplay_internal: every landed entry is finished at once.  */
void
nh_img_redisplay_begin (void)
{
  if (!nh_img_any_landed)
    return;
  nh_img_any_landed = false;
  nh_img_marked = Qnil;
  /* Take them out first: a landing logs errors, which runs Lisp.  */
  struct nh_img_pend *landed = NULL, **pp = &nh_img_pends;
  while (*pp)
    {
      struct nh_img_pend *r = *pp;
      if (r->landed)
	{
	  *pp = r->next;
	  r->next = landed;
	  landed = r;
	}
      else
	pp = &r->next;
    }
  while (landed)
    {
      struct nh_img_pend *r = landed;
      landed = r->next;
      r->next = NULL;
      struct frame *f = r->synthetic ? NULL : nh_img_cache_frame (r->cache);
      struct image *img = f ? IMAGE_OPT_FROM_ID (f, r->id) : NULL;
      if (img && !img->pixmap && !img->load_failed_p)
	nh_img_land_pend (f, img, r, false);
      else
	{
	  if (r->synthetic)
	    nh_img_n_landed++;
	  else
	    nh_img_n_dropped++;
	  nh_img_pend_free (r);
	}
    }
}

/* lookup_image's hit, outside redisplay: a Lisp caller (or a display
   function such as `window-text-pixel-size') may need the pixels of a
   pending entry, so it waits for the job (§8.3 item 9) or, past the
   bound, loads it as today.  */
void
nh_img_hit (struct frame *f, struct image *img)
{
  if (nh_img_in_redisplay || !nh_img_pends || img->pixmap
      || img->load_failed_p)
    return;
  struct nh_img_pend *r = nh_img_pend_find (FRAME_IMAGE_CACHE (f), img->id);
  if (!r)
    return;
  bool sync = false;
  if (!r->landed
      && nh_wk_wait (&r->j->job, NH_IMG_PIXELS_WAIT_NS, NH_WK_PH_WORKER_WAIT,
		     NH_WK_RS_IMAGE_PIXELS) != 1)
    {
      nh_wk_cancel (&r->j->job);
      sync = true;
    }
  nh_img_pend_unlink (r);
  nh_img_land_pend (f, img, r, sync);
}

/* The iterator's lookup_image calls (§8.3 item 1): only while
   redisplaying may a miss defer.  */
static void
nh_img_redisplay_restore (int old)
{
  nh_img_in_redisplay = old;
}

ptrdiff_t
nh_img_redisplay_lookup (struct frame *f, Lisp_Object spec, int face_id)
{
  if (!redisplaying_p)
    return lookup_image (f, spec, face_id);
  specpdl_ref count = SPECPDL_INDEX ();
  record_unwind_protect_int (nh_img_redisplay_restore, nh_img_in_redisplay);
  nh_img_in_redisplay = true;
  ptrdiff_t id = lookup_image (f, spec, face_id);
  unbind_to (count, Qnil);
  return id;
}

DEFUN ("nidhogg-workers--test-image-info", Fnidhogg_workers__test_image_info,
       Snidhogg_workers__test_image_info, 0, 0, 0,
       doc: /* The pending image entries and their counters; for tests.
A plist: `:pending' (entries without pixels, landed ones included),
`:landed-waiting' (decoded, landing at the next redisplay), `:live'
\(decode jobs not freed yet), and since startup `:deferred' (entries made
pending), `:landed' and `:dropped' (results whose entry was gone).  */)
  (void)
{
  EMACS_INT pending = 0, waiting = 0;
  for (struct nh_img_pend *r = nh_img_pends; r; r = r->next)
    {
      pending++;
      waiting += r->landed;
    }
  return listn (12,
		intern_c_string (":pending"), make_int (pending),
		intern_c_string (":landed-waiting"), make_int (waiting),
		intern_c_string (":live"), make_int (atomic_load (&nh_img_live)),
		intern_c_string (":deferred"), make_int (nh_img_n_deferred),
		intern_c_string (":landed"), make_int (nh_img_n_landed),
		intern_c_string (":dropped"), make_int (nh_img_n_dropped));
}

DEFUN ("nidhogg-workers--test-marked-windows",
       Fnidhogg_workers__test_marked_windows,
       Snidhogg_workers__test_marked_windows, 0, 0, 0,
       doc: /* The windows the last image landing marked for redisplay; for tests.  */)
  (void)
{
  return Fcopy_sequence (nh_img_marked);
}

DEFUN ("nidhogg-workers--deferrable-p", Fnidhogg_workers__deferrable_p,
       Snidhogg_workers__deferrable_p, 1, 1, 0,
       doc: /* Non-nil if images of FORMAT may decode on nh-img in this build.
FORMAT is an image type symbol such as `png'.  */)
  (Lisp_Object format)
{
#ifdef USE_CAIRO
  int fmt = nh_img_core_format (nh_img_spec_format (list3 (Qimage, QCtype,
							   format)));
  return fmt >= 0 && nh_img_decoders[fmt] ? Qt : Qnil;
#else
  return Qnil;
#endif
}

DEFUN ("nidhogg-workers--test-image-job", Fnidhogg_workers__test_image_job,
       Snidhogg_workers__test_image_job, 2, 3, 0,
       doc: /* Decode image SPEC on nh-img for the test entry ID; for tests.
SPEC and PARAMS are as for `nidhogg-workers--test-image-decode'.  The
entry has no image behind it; otherwise its job is a display job: it
passes the pause point `after-header', lands at the next redisplay (or
`nidhogg-workers--test-image-land') and is dropped if the entry is
forgotten first (`nidhogg-workers--test-image-forget', what free_image
does).  Return ID, or nil if the submit was refused.  */)
  (Lisp_Object spec, Lisp_Object id, Lisp_Object params)
{
  CHECK_FIXNAT (id);
  if (nh_img_pend_find (&nh_img_test_cache, XFIXNAT (id)))
    error ("The test entry exists");
  struct nh_img_tjob *t = nh_img_tjob_make (spec, params);
  struct nh_img_job *j = calloc (1, sizeof *j);
  struct nh_img_pend *p = calloc (1, sizeof *p);
  if (!j || !p)
    {
      free (j);
      free (p);
      nh_img_tjob_free (t);
      memory_full (sizeof *j);
    }
  j->fmt = t->fmt;
  j->data = t->data;
  j->len = t->len;
  j->file = t->file;
  j->params = t->params;
  j->params.file = j->file;
  t->data = NULL;
  t->file = NULL;
  nh_img_tjob_free (t);
  nh_img_job_init (j, &nh_img_test_cache, XFIXNAT (id));
  if (!nh_wk_submit (&j->job))
    {
      nh_wk_job_unref (&j->job);
      free (p);
      return Qnil;
    }
  p->cache = j->cache;
  p->id = j->id;
  p->gen = j->gen;
  p->j = j;
  p->synthetic = true;
  p->next = nh_img_pends;
  nh_img_pends = p;
  return id;
}

DEFUN ("nidhogg-workers--test-image-forget", Fnidhogg_workers__test_image_forget,
       Snidhogg_workers__test_image_forget, 1, 1, 0,
       doc: /* Forget the test entry ID as free_image forgets an entry; for tests.
Return t if it was pending.  */)
  (Lisp_Object id)
{
  CHECK_FIXNAT (id);
  struct nh_img_pend *r = nh_img_pend_find (&nh_img_test_cache, XFIXNAT (id));
  if (!r)
    return Qnil;
  nh_img_pend_forget (r);
  return Qt;
}

DEFUN ("nidhogg-workers--test-image-land", Fnidhogg_workers__test_image_land,
       Snidhogg_workers__test_image_land, 0, 0, 0,
       doc: /* Land the decoded image entries as a redisplay does; for tests.
Return how many landed.  */)
  (void)
{
  EMACS_INT before = nh_img_n_landed;
  nh_img_redisplay_begin ();
  return make_int (nh_img_n_landed - before);
}

#else /* not HAVE_WINDOW_SYSTEM */

void
nh_img_redisplay_begin (void)
{
}

#endif /* not HAVE_WINDOW_SYSTEM */

extern void nh_img_image_syms (void);

void
nh_img_syms (void)
{
  defsubr (&Snidhogg_workers__test_image_decode);
  defsubr (&Snidhogg_workers__test_image_decode_finish);
  defsubr (&Snidhogg_workers__test_decode_count);
  defsubr (&Snidhogg_workers__test_image_cache_clear);
#ifdef HAVE_WINDOW_SYSTEM
  DEFSYM (Qnidhogg_workers_images, "nidhogg-workers-images");
  DEFSYM (Qsize, "size");
  defsubr (&Snidhogg_workers__test_image_answer);
  DEFSYM (Qnidhogg_image_defer, "nidhogg-image-defer");
  staticpro (&nh_img_marked);
  nh_img_marked = Qnil;
  defsubr (&Snidhogg_workers__test_image_info);
  defsubr (&Snidhogg_workers__test_marked_windows);
  defsubr (&Snidhogg_workers__deferrable_p);
  defsubr (&Snidhogg_workers__test_image_job);
  defsubr (&Snidhogg_workers__test_image_forget);
  defsubr (&Snidhogg_workers__test_image_land);
  nh_img_image_syms ();
#endif
}

#endif /* not NH_WK_HARNESS */

#endif /* HAVE_NIDHOGG_WORKERS */

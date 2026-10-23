/* Nidhogg's worker threads: the image decoder table (nh-img).

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

/* The decoder cores (PD-11).  A core turns bytes into pixels without Lisp,
   Emacs allocation or Emacs error handling, so the same code runs on the
   main thread (the synchronous loaders, under the flag), on nh-img and in
   the Lisp-free harness.  Everything that depends on the frame, the face or
   the spec is resolved on the main thread into `struct nh_img_params'.  The
   table `nh_img_decoders' is the only thing OT7 touches: it replaces the
   entries of PNG, GIF and WebP at init and keeps the C cores to switch back.

   The output follows what the loaders build on a Cairo frame: one 32-bit
   word 0x00RRGGBB per pixel and, only where the loaders make a mask, an
   8-bit mask (255 draw, 0 retain).  */

#ifndef NIDHOGG_IMAGE_BG_H
#define NIDHOGG_IMAGE_BG_H

#ifdef HAVE_NIDHOGG_WORKERS

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

enum nh_img_fmt
{
  NH_IMG_PNG,
  NH_IMG_JPEG,
  NH_IMG_GIF,
  NH_IMG_WEBP,
  NH_IMG_FMT_N
};

/* What went wrong.  The main thread maps a code and its operands to the
   `image_error' text the loaders log (nh_img_report in image.c).  */
enum nh_img_err
{
  NH_IMG_OK = 0,
  NH_IMG_E_UNSUPPORTED,	/* no core for this format in this build */
  NH_IMG_E_OPEN,	/* the file cannot be opened or read */
  NH_IMG_E_NOT_IMAGE,	/* PNG: the signature is wrong */
  NH_IMG_E_LIB,		/* the library failed: out->errmsg */
  NH_IMG_E_SIZE,	/* beyond the limits of the params (max-image-size) */
  NH_IMG_E_SILENT,	/* a failure the loaders do not report */
  NH_IMG_E_NOMEM,
  NH_IMG_E_GIF_OPEN,	/* DGifOpen: out->errmsg or empty */
  NH_IMG_E_GIF_READ,	/* DGifSlurp: out->errmsg or empty */
  NH_IMG_E_GIF_SUBIMAGE, /* a subimage does not fit */
  NH_IMG_E_GIF_INDEX,	/* :index is out of range: out->err_arg */
  NH_IMG_E_WEBP_HEADERS,
  NH_IMG_E_WEBP_PARSE,
  NH_IMG_E_WEBP_INFO,
  NH_IMG_E_WEBP_FRAME,	/* out->err_arg is the frame number */
  NH_IMG_E_WEBP_DECODE,
  NH_IMG_E_N
};

/* Everything the loaders read from the frame, the face and the spec.  */
struct nh_img_params
{
  int frame_index;	/* :index; -1 when the spec has none */
  uint64_t key;		/* identity of the encoded data for the animation
			   cache (GIF, WebP); 0: do not cache */
  const char *file;	/* the encoded file name, read by the core when
			   DATA is NULL */
  int64_t limit_w, limit_h; /* `max-image-size' as pixels; negative: none */
  bool bg_set;		/* a colour a real alpha channel composites over */
  uint16_t bg_r, bg_g, bg_b;	/* 16 bits per channel (PNG, WebP) */
  uint32_t frame_bg_pixel;	/* GIF: the frame background, as a pixel */
  bool gif_bg_set;		/* GIF: the spec's :background */
  uint32_t gif_bg_pixel;
  uint32_t face_fg, face_bg;	/* reserved for decoders that need the
				   face; the C cores do not read them */
};

struct nh_img_ext		/* a GIF extension block of the frame */
{
  struct nh_img_ext *next;
  int function;
  size_t len;
  uint8_t *bytes;
};

struct nh_img_out
{
  uint32_t *pixels;	/* width * height words, malloc'd */
  uint8_t *mask;	/* width * height bytes, malloc'd, or NULL */
  int width, height;
  bool has_corners;	/* GIF and WebP set img->corners */
  int corners[4];	/* top, left, bottom, right */
  bool animated;	/* WebP: the file has an animation */
  int frame_count;	/* GIF: ImageCount; WebP animation: frames; else 0 */
  bool has_delay;
  int delay;		/* GIF: hundredths of a second; WebP: milliseconds */
  struct nh_img_ext *ext;	/* GIF extension blocks of the frame, in order */
  bool bkgd_valid;	/* PNG bKGD chunk */
  uint16_t bkgd[3];
  int nwarn;		/* PNG warnings, to log as "PNG warning: %s" */
  char warn[4][96];
  int err;		/* enum nh_img_err */
  int err_arg;
  char errmsg[128];
};

struct nh_img_decoder
{
  /* Decode DATA (LEN bytes), or the file PARAMS->file when DATA is NULL,
     into OUT, which the caller has zeroed.  Value: 0 and the pixels, or an
     error code (also in OUT->err).  Callable from any thread.  */
  int (*decode) (const uint8_t *data, size_t len,
		 const struct nh_img_params *params, struct nh_img_out *out);
};

/* Indexed by `enum nh_img_fmt'; an entry is NULL when the build lacks the
   library.  */
extern const struct nh_img_decoder *nh_img_decoders[NH_IMG_FMT_N];

extern int nh_img_decode (int fmt, const uint8_t *data, size_t len,
			  const struct nh_img_params *params,
			  struct nh_img_out *out);
extern void nh_img_out_free (struct nh_img_out *out);
extern uint64_t nh_img_hash (const uint8_t *data, size_t len);
extern uint64_t nh_img_file_key (const char *path);
extern uint64_t nh_img_hash_combine (uint64_t h, const void *p, size_t n);

/* Base64 for `:nidhogg-data-encoding' (Task 30); see the definition.  */
extern bool nh_img_b64_decode (const uint8_t *in, size_t n, uint8_t *out,
			       size_t cap, size_t *got, bool *complete);

/* The animation cache of the cores.  Counters for the tests: frames
   composed or decoded since the last reset.  */
extern int64_t nh_img_decode_count (bool reset);
extern void nh_img_cache_clear (void);
extern int nh_img_cache_entries (void);

/* The entry of a synchronous loader in image.c (nidhogg-workers.h declares
   nh_img_sync_load): decode through the core and return its result.  */
#define NH_IMG_SYNC_LOAD(f, img, fmt)				\
  do {								\
    bool nh_ok_;						\
    if (nh_img_sync_load (f, img, fmt, &nh_ok_))		\
      return nh_ok_;						\
  } while (0)

#endif /* HAVE_NIDHOGG_WORKERS */

#endif /* NIDHOGG_IMAGE_BG_H */

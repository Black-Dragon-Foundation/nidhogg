/* Glitnir's image decoders (Rust) in the image worker's decoder table.

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

/* The image worker (nidhogg-image-bg.c) decodes through the table
   `nh_img_decoders', which holds the C decoder cores.  At load time this
   file puts its own entry in front of each core it has a Rust decoder
   for; the entry decodes in Rust when the format is in
   `nidhogg-workers-image-rust' and otherwise calls the core it replaced,
   so the switch takes effect at the next decode, on every thread.

   An entry runs on nh-img or on the main thread (the synchronous
   loaders), never touches Lisp, and calls the crate's stateless
   `glitnir_img_decode' directly: not through the glue's guard, whose
   state is the main thread's, and under no lock.  The crate's result
   comes from calloc and becomes the core's result as it is; a panic in
   the crate is a decode error.  An image the crate hands back
   (GLITNIR_IMG_E_HOST: a PNG libpng composites through gamma tables)
   goes to the C core with the bytes already read.  */

#include <config.h>

#if defined HAVE_GLITNIR && defined HAVE_NIDHOGG_WORKERS

#include <errno.h>
#include <fcntl.h>
#include <stdatomic.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <unistd.h>

#include "lisp.h"
#include "nidhogg-image-bg.h"
#include "glitnir.h"

/* The formats decoded in Rust when nothing says otherwise: a bit per
   `enum nh_img_fmt'.  The variable's default is built from it, so the
   dumped value and the mask agree.  */
#define GLITNIR_IMG_RUST_DEFAULT 0u

/* The largest image file read (encoded bytes), as the cores' limit.  */
#define GLITNIR_IMG_FILE_MAX ((off_t) 1 << 30)

/* The formats `nidhogg-workers-image-rust' names, as a bit mask; kept
   by its watcher, read by the entries on any thread.  */
static atomic_uint glitnir_img_rust = GLITNIR_IMG_RUST_DEFAULT;

/* The C cores the entries replaced.  */
static const struct nh_img_decoder *glitnir_img_core[NH_IMG_FMT_N];

/* Decodes done in Rust, and images handed back to the C core.  */
static atomic_llong glitnir_img_decoded, glitnir_img_handed_back;

/* The crate's format for each table index; -1: none.  */
static const int glitnir_img_format[NH_IMG_FMT_N] = {
  [NH_IMG_PNG] = GLITNIR_IMG_PNG,
  [NH_IMG_JPEG] = -1,
  [NH_IMG_GIF] = -1,
  [NH_IMG_WEBP] = -1,
};

static const char *const glitnir_img_names[NH_IMG_FMT_N] = {
  [NH_IMG_PNG] = "png", [NH_IMG_JPEG] = "jpeg",
  [NH_IMG_GIF] = "gif", [NH_IMG_WEBP] = "webp",
};

/* The encoded bytes: DATA, or the contents of P->file (only a regular
   file, at most GLITNIR_IMG_FILE_MAX bytes, read up to the size it had
   when opened: a name that leads to a FIFO or /dev/zero must not grow
   the buffer).  *OWNED is what to free.  0, or NH_IMG_E_OPEN.  */
static int
glitnir_img_input (const uint8_t *data, size_t len,
		   const struct nh_img_params *p, const uint8_t **bytes,
		   size_t *n, uint8_t **owned)
{
  *owned = NULL;
  if (data)
    {
      *bytes = data;
      *n = len;
      return 0;
    }
  if (!p->file)
    return NH_IMG_E_OPEN;
  int fd = open (p->file, O_RDONLY | O_CLOEXEC);
  if (fd < 0)
    return NH_IMG_E_OPEN;
  struct stat st;
  if (fstat (fd, &st) != 0 || !S_ISREG (st.st_mode)
      || st.st_size > GLITNIR_IMG_FILE_MAX)
    {
      close (fd);
      return NH_IMG_E_OPEN;
    }
  size_t cap = st.st_size > 0 ? st.st_size : 1;
  uint8_t *buf = malloc (cap);
  size_t got = 0;
  while (buf && got < cap)
    {
      ssize_t r = read (fd, buf + got, cap - got);
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
      got += r;
    }
  close (fd);
  if (!buf)
    return NH_IMG_E_OPEN;
  *owned = buf;
  *bytes = buf;
  *n = got;
  return 0;
}

/* Decode in Rust, or hand back to the C core.  */
static int
glitnir_img_rust_decode (int fmt, const uint8_t *data, size_t len,
			 const struct nh_img_params *p, struct nh_img_out *out)
{
  const uint8_t *bytes;
  size_t n;
  uint8_t *owned;
  int e = glitnir_img_input (data, len, p, &bytes, &n, &owned);
  if (e)
    {
      out->err = e;
      return e;
    }
  GlitnirImgParams gp = {
    .limit_w = p->limit_w, .limit_h = p->limit_h,
    .bg_set = p->bg_set, .bg_r = p->bg_r, .bg_g = p->bg_g, .bg_b = p->bg_b,
  };
  GlitnirImgOut go;
  int32_t r = glitnir_img_decode (glitnir_img_format[fmt], bytes, n, &gp, &go);
  if (r == GLITNIR_IMG_E_HOST)
    {
      atomic_fetch_add (&glitnir_img_handed_back, 1);
      int c = glitnir_img_core[fmt]->decode (bytes, n, p, out);
      free (owned);
      return c;
    }
  free (owned);
  switch (r)
    {
    case GLITNIR_OK:
      atomic_fetch_add (&glitnir_img_decoded, 1);
      out->pixels = go.pixels;
      out->mask = go.mask;
      out->width = go.width;
      out->height = go.height;
      out->bkgd_valid = go.bkgd_valid;
      memcpy (out->bkgd, go.bkgd, sizeof out->bkgd);
      out->err = NH_IMG_OK;
      return NH_IMG_OK;
    case GLITNIR_IMG_E_NOT_IMAGE:
      out->err = NH_IMG_E_NOT_IMAGE;
      break;
    case GLITNIR_IMG_E_SIZE:
      out->err = NH_IMG_E_SIZE;
      break;
    case GLITNIR_IMG_E_NOMEM:
      out->err = NH_IMG_E_NOMEM;
      break;
    case GLITNIR_IMG_E_DECODE:
      out->err = NH_IMG_E_LIB;
      snprintf (out->errmsg, sizeof out->errmsg, "%s", go.errmsg);
      break;
    case GLITNIR_E_PANIC:
      out->err = NH_IMG_E_LIB;
      snprintf (out->errmsg, sizeof out->errmsg, "decoder panic");
      break;
    default:
      out->err = NH_IMG_E_UNSUPPORTED;
      break;
    }
  glitnir_img_free (&go);
  return out->err;
}

static int
glitnir_img_entry (int fmt, const uint8_t *data, size_t len,
		   const struct nh_img_params *p, struct nh_img_out *out)
{
  if (atomic_load_explicit (&glitnir_img_rust, memory_order_relaxed)
      & (1u << fmt))
    return glitnir_img_rust_decode (fmt, data, len, p, out);
  return glitnir_img_core[fmt]->decode (data, len, p, out);
}

static int
glitnir_img_png (const uint8_t *data, size_t len,
		 const struct nh_img_params *p, struct nh_img_out *out)
{
  return glitnir_img_entry (NH_IMG_PNG, data, len, p, out);
}

static const struct nh_img_decoder glitnir_img_entries[NH_IMG_FMT_N] = {
  [NH_IMG_PNG] = { glitnir_img_png },
};

/* Put the entries in front of the cores, at load time: the table is
   static data that the dump does not carry, so this runs in every
   session (and in temacs).  */
static void __attribute__ ((constructor))
glitnir_img_install (void)
{
  for (int fmt = 0; fmt < NH_IMG_FMT_N; fmt++)
    if (glitnir_img_format[fmt] >= 0 && nh_img_decoders[fmt])
      {
	glitnir_img_core[fmt] = nh_img_decoders[fmt];
	nh_img_decoders[fmt] = &glitnir_img_entries[fmt];
      }
}

/* The mask of the formats in LIST that have an entry.  */
static unsigned
glitnir_img_mask (Lisp_Object list)
{
  unsigned mask = 0;
  FOR_EACH_TAIL_SAFE (list)
    for (int fmt = 0; fmt < NH_IMG_FMT_N; fmt++)
      if (glitnir_img_core[fmt] && SYMBOLP (XCAR (list))
	  && !strcmp (SSDATA (SYMBOL_NAME (XCAR (list))),
		      glitnir_img_names[fmt]))
	mask |= 1u << fmt;
  return mask;
}

static Lisp_Object
watch_nidhogg_workers_image_rust (Lisp_Object symbol, Lisp_Object newval,
				  Lisp_Object operation, Lisp_Object where)
{
  (void) symbol;
  /* The decoders are global: a buffer-local value means nothing.  */
  if (NILP (where))
    atomic_store (&glitnir_img_rust,
		  EQ (operation, Qmakunbound) ? 0 : glitnir_img_mask (newval));
  return Qnil;
}

DEFUN ("nidhogg-workers--test-image-rust", Fnidhogg_workers__test_image_rust,
       Snidhogg_workers__test_image_rust, 0, 1, 0,
       doc: /* Glitnir's image decoders, for tests.
Value: (:formats FORMATS :decoded N :handed-back M), FORMATS the image
types that have a Rust decoder in this build, N the images decoded in
Rust and M those the Rust decoder handed back to the C core, since the
last reset.  Non-nil RESET resets the counts after reading them.  */)
  (Lisp_Object reset)
{
  Lisp_Object formats = Qnil;
  for (int fmt = NH_IMG_FMT_N - 1; fmt >= 0; fmt--)
    if (glitnir_img_core[fmt])
      formats = Fcons (intern_c_string (glitnir_img_names[fmt]), formats);
  long long decoded = (NILP (reset) ? atomic_load (&glitnir_img_decoded)
		       : atomic_exchange (&glitnir_img_decoded, 0));
  long long back = (NILP (reset) ? atomic_load (&glitnir_img_handed_back)
		    : atomic_exchange (&glitnir_img_handed_back, 0));
  return listn (6, intern_c_string (":formats"), formats,
		intern_c_string (":decoded"), make_int (decoded),
		intern_c_string (":handed-back"), make_int (back));
}

void
syms_of_glitnir_images (void)
{
  DEFSYM (Qnidhogg_workers_image_rust, "nidhogg-workers-image-rust");

  DEFVAR_LISP ("nidhogg-workers-image-rust", Vnidhogg_workers_image_rust,
	       doc: /* The image types the image worker decodes in Rust.
A list of image types: each one in it that Glitnir has a decoder for
\(`png', with the `png' crate) is decoded in Rust, the others by the C
libraries, as before.  Both give the same pixels; the Rust decoders keep
the C libraries out of the path of untrusted image data.  A PNG whose
transparency libpng would composite through gamma tables (one with a
gAMA, sRGB, iCCP or cICP chunk) is decoded by libpng either way.  Error
messages are the Rust decoders' own.  Takes effect at the next decode;
only the global value counts.  */);
  Lisp_Object dflt = Qnil;
  for (int fmt = NH_IMG_FMT_N - 1; fmt >= 0; fmt--)
    if (GLITNIR_IMG_RUST_DEFAULT & (1u << fmt))
      dflt = Fcons (intern_c_string (glitnir_img_names[fmt]), dflt);
  Vnidhogg_workers_image_rust = dflt;

  static union Aligned_Lisp_Subr Swatch_nidhogg_workers_image_rust =
     {{{ PSEUDOVECTOR_FLAG | (PVEC_SUBR << PSEUDOVECTOR_AREA_BITS) },
       { .a4 = watch_nidhogg_workers_image_rust },
       4, 4, "watch_nidhogg_workers_image_rust", {0}, lisp_h_Qnil}};
  Lisp_Object watcher;
  XSETSUBR (watcher, &Swatch_nidhogg_workers_image_rust.s);
  Fadd_variable_watcher (Qnidhogg_workers_image_rust, watcher);

  defsubr (&Snidhogg_workers__test_image_rust);
}

#endif /* HAVE_GLITNIR && HAVE_NIDHOGG_WORKERS */

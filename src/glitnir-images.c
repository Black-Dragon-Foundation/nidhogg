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
   goes to the C core with the bytes already read.

   Animations (GIF, WebP): the crate keeps nothing between calls, so the
   state that lets frame N be decoded from frame N-1 (the canvas, the
   frames read) comes back with each decode the host may continue, and
   this file keeps it in a small cache of its own, as the cores keep
   theirs: by the identity of the data, taken out while a decode uses it,
   at most 8 entries and 256 MB.  */

#include <config.h>

#if defined HAVE_GLITNIR && defined HAVE_NIDHOGG_WORKERS

#include <errno.h>
#include <fcntl.h>
#include <pthread.h>
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

/* Decodes done in Rust, images handed back to the C core, and decodes
   that continued from a kept animation state.  */
static atomic_llong glitnir_img_decoded, glitnir_img_handed_back;
static atomic_llong glitnir_img_continued;

/* The crate's format for each table index; -1: none.  */
static const int glitnir_img_format[NH_IMG_FMT_N] = {
  [NH_IMG_PNG] = GLITNIR_IMG_PNG,
  [NH_IMG_JPEG] = -1,
  [NH_IMG_GIF] = GLITNIR_IMG_GIF,
  [NH_IMG_WEBP] = GLITNIR_IMG_WEBP,
};

static const char *const glitnir_img_names[NH_IMG_FMT_N] = {
  [NH_IMG_PNG] = "png", [NH_IMG_JPEG] = "jpeg",
  [NH_IMG_GIF] = "gif", [NH_IMG_WEBP] = "webp",
};

/* The animation states, by format and key.  */

struct glitnir_img_centry
{
  struct glitnir_img_centry *next;
  int fmt;
  uint64_t key;
  void *anim;
  size_t bytes;
};

#define GLITNIR_IMG_CACHE_MAX_ENTRIES 8
#define GLITNIR_IMG_CACHE_MAX_BYTES ((size_t) 256 << 20)

static pthread_mutex_t glitnir_img_cache_lock = PTHREAD_MUTEX_INITIALIZER;
static struct glitnir_img_centry *glitnir_img_cache;

/* Take FMT's state for KEY out of the cache, or NULL.  */
static void *
glitnir_img_cache_take (int fmt, uint64_t key)
{
  struct glitnir_img_centry *e = NULL;
  pthread_mutex_lock (&glitnir_img_cache_lock);
  for (struct glitnir_img_centry **pp = &glitnir_img_cache; *pp;
       pp = &(*pp)->next)
    if ((*pp)->fmt == fmt && (*pp)->key == key)
      {
	e = *pp;
	*pp = e->next;
	break;
      }
  pthread_mutex_unlock (&glitnir_img_cache_lock);
  if (!e)
    return NULL;
  void *anim = e->anim;
  free (e);
  return anim;
}

static void
glitnir_img_centries_free (struct glitnir_img_centry *e)
{
  while (e)
    {
      struct glitnir_img_centry *next = e->next;
      glitnir_img_anim_free (e->anim);
      free (e);
      e = next;
    }
}

/* Keep ANIM (BYTES of memory) as FMT's state for KEY, the newest entry;
   the oldest ones go beyond the bounds.  */
static void
glitnir_img_cache_put (int fmt, uint64_t key, void *anim, size_t bytes)
{
  struct glitnir_img_centry *e = malloc (sizeof *e), *drop = NULL;
  if (!e)
    {
      glitnir_img_anim_free (anim);
      return;
    }
  *e = (struct glitnir_img_centry) { .fmt = fmt, .key = key, .anim = anim,
				     .bytes = bytes };
  pthread_mutex_lock (&glitnir_img_cache_lock);
  for (struct glitnir_img_centry *q = glitnir_img_cache; q; q = q->next)
    if (q->fmt == fmt && q->key == key)
      {
	/* another decode stored it meanwhile */
	drop = e;
	e = NULL;
	break;
      }
  if (e)
    {
      e->next = glitnir_img_cache;
      glitnir_img_cache = e;
    }
  int n = 0;
  size_t total = 0;
  struct glitnir_img_centry **pp = &glitnir_img_cache;
  while (*pp)
    {
      n++;
      total += (*pp)->bytes;
      if (n > GLITNIR_IMG_CACHE_MAX_ENTRIES
	  || total > GLITNIR_IMG_CACHE_MAX_BYTES)
	{
	  struct glitnir_img_centry *victim = *pp;
	  *pp = victim->next;
	  victim->next = drop;
	  drop = victim;
	  n--;
	  total -= victim->bytes;
	  continue;
	}
      pp = &(*pp)->next;
    }
  pthread_mutex_unlock (&glitnir_img_cache_lock);
  glitnir_img_centries_free (drop);
}

static void
glitnir_img_cache_clear (void)
{
  pthread_mutex_lock (&glitnir_img_cache_lock);
  struct glitnir_img_centry *all = glitnir_img_cache;
  glitnir_img_cache = NULL;
  pthread_mutex_unlock (&glitnir_img_cache_lock);
  glitnir_img_centries_free (all);
}

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

/* The core's error for the crate's error R in format FMT.  */
static int
glitnir_img_error (int fmt, int32_t r)
{
  switch (r)
    {
    case GLITNIR_IMG_E_NOT_IMAGE:
      return (fmt == NH_IMG_GIF ? NH_IMG_E_GIF_OPEN
	      : fmt == NH_IMG_WEBP ? NH_IMG_E_WEBP_HEADERS : NH_IMG_E_NOT_IMAGE);
    case GLITNIR_IMG_E_DECODE:
    case GLITNIR_E_PANIC:
      return (fmt == NH_IMG_GIF ? NH_IMG_E_GIF_READ
	      : fmt == NH_IMG_WEBP ? NH_IMG_E_WEBP_DECODE : NH_IMG_E_LIB);
    case GLITNIR_IMG_E_SIZE:
      return NH_IMG_E_SIZE;
    case GLITNIR_IMG_E_NOMEM:
      return NH_IMG_E_NOMEM;
    case GLITNIR_IMG_E_SUBIMAGE:
      return NH_IMG_E_GIF_SUBIMAGE;
    case GLITNIR_IMG_E_INDEX:
      return NH_IMG_E_GIF_INDEX;
    case GLITNIR_IMG_E_FRAME:
      return NH_IMG_E_WEBP_FRAME;
    case GLITNIR_IMG_E_PARSE:
      return NH_IMG_E_WEBP_PARSE;
    default:
      return NH_IMG_E_UNSUPPORTED;
    }
}

/* GO's extension blocks (function, length, bytes; GIF) as OUT's list.
   As the core: an allocation failure ends the list.  */
static void
glitnir_img_ext (const GlitnirImgOut *go, struct nh_img_out *out)
{
  struct nh_img_ext **tail = &out->ext;
  for (size_t at = 0; go->ext && at + 2 <= go->ext_len;)
    {
      int len = go->ext[at + 1];
      if (at + 2 + len > go->ext_len)
	break;
      struct nh_img_ext *x = calloc (1, sizeof *x);
      if (!x)
	break;
      x->function = go->ext[at];
      x->len = len;
      x->bytes = malloc (len ? len : 1);
      if (!x->bytes)
	{
	  free (x);
	  break;
	}
      memcpy (x->bytes, go->ext + at + 2, len);
      *tail = x;
      tail = &x->next;
      at += 2 + len;
    }
}

/* Decode in Rust, or hand back to the C core.  */
static int
glitnir_img_rust_decode (int fmt, const uint8_t *data, size_t len,
			 const struct nh_img_params *p, struct nh_img_out *out)
{
  /* As the cores: an animation's state is kept when a frame is asked
     for and the data has an identity.  */
  bool keep = fmt != NH_IMG_PNG && p->frame_index >= 0 && p->key != 0;
  void *anim = keep ? glitnir_img_cache_take (fmt, p->key) : NULL;
  const uint8_t *bytes = NULL;
  size_t n = 0;
  uint8_t *owned = NULL;
  if (anim)
    atomic_fetch_add (&glitnir_img_continued, 1);
  else
    {
      int e = glitnir_img_input (data, len, p, &bytes, &n, &owned);
      if (e)
	{
	  out->err = e;
	  return e;
	}
    }
  GlitnirImgParams gp = {
    .limit_w = p->limit_w, .limit_h = p->limit_h,
    .bg_set = p->bg_set, .bg_r = p->bg_r, .bg_g = p->bg_g, .bg_b = p->bg_b,
    .frame_index = p->frame_index, .frame_bg = p->frame_bg_pixel,
    .gif_bg_set = p->gif_bg_set, .gif_bg = p->gif_bg_pixel,
    .keep = keep, .anim = anim,
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
  if (r != GLITNIR_OK)
    {
      out->err = glitnir_img_error (fmt, r);
      out->err_arg = go.err_arg;
      if (r == GLITNIR_E_PANIC)
	snprintf (out->errmsg, sizeof out->errmsg, "decoder panic");
      else
	snprintf (out->errmsg, sizeof out->errmsg, "%s", go.errmsg);
      glitnir_img_free (&go);
      return out->err;
    }
  atomic_fetch_add (&glitnir_img_decoded, 1);
  out->pixels = go.pixels;
  out->mask = go.mask;
  out->width = go.width;
  out->height = go.height;
  out->bkgd_valid = go.bkgd_valid;
  memcpy (out->bkgd, go.bkgd, sizeof out->bkgd);
  out->frame_count = go.frame_count;
  out->animated = go.animated;
  out->has_delay = go.has_delay;
  out->delay = go.delay;
  out->has_corners = go.has_corners;
  memcpy (out->corners, go.corners, sizeof out->corners);
  glitnir_img_ext (&go, out);
  free (go.ext);
  if (go.anim)
    glitnir_img_cache_put (fmt, p->key, go.anim, go.anim_bytes);
  out->err = NH_IMG_OK;
  return NH_IMG_OK;
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

static int
glitnir_img_gif (const uint8_t *data, size_t len,
		 const struct nh_img_params *p, struct nh_img_out *out)
{
  return glitnir_img_entry (NH_IMG_GIF, data, len, p, out);
}

static int
glitnir_img_webp (const uint8_t *data, size_t len,
		  const struct nh_img_params *p, struct nh_img_out *out)
{
  return glitnir_img_entry (NH_IMG_WEBP, data, len, p, out);
}

static const struct nh_img_decoder glitnir_img_entries[NH_IMG_FMT_N] = {
  [NH_IMG_PNG] = { glitnir_img_png },
  [NH_IMG_GIF] = { glitnir_img_gif },
  [NH_IMG_WEBP] = { glitnir_img_webp },
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
Value: (:formats FORMATS :decoded N :handed-back M :continued C), FORMATS
the image types that have a Rust decoder in this build, N the images
decoded in Rust, M those the Rust decoder handed back to the C core and
C the decodes that continued from a kept animation state, since the last
reset.  Non-nil RESET resets the counts after reading them and empties
the kept animation states.  */)
  (Lisp_Object reset)
{
  Lisp_Object formats = Qnil;
  for (int fmt = NH_IMG_FMT_N - 1; fmt >= 0; fmt--)
    if (glitnir_img_core[fmt])
      formats = Fcons (intern_c_string (glitnir_img_names[fmt]), formats);
  atomic_llong *counts[] = { &glitnir_img_decoded, &glitnir_img_handed_back,
			     &glitnir_img_continued };
  long long v[3];
  for (int i = 0; i < 3; i++)
    v[i] = NILP (reset) ? atomic_load (counts[i]) : atomic_exchange (counts[i], 0);
  if (!NILP (reset))
    glitnir_img_cache_clear ();
  return listn (8, intern_c_string (":formats"), formats,
		intern_c_string (":decoded"), make_int (v[0]),
		intern_c_string (":handed-back"), make_int (v[1]),
		intern_c_string (":continued"), make_int (v[2]));
}

void
syms_of_glitnir_images (void)
{
  DEFSYM (Qnidhogg_workers_image_rust, "nidhogg-workers-image-rust");

  DEFVAR_LISP ("nidhogg-workers-image-rust", Vnidhogg_workers_image_rust,
	       doc: /* The image types the image worker decodes in Rust.
A list of image types: each one in it that Glitnir has a decoder for
\(`png', `gif' and `webp', with the `png', `gif' and `image-webp'
crates) is decoded in Rust, the others by the C libraries, as before.  Both give the same pixels; the Rust decoders keep
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

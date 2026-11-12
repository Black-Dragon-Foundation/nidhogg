/* Naglfar's font driver (HAVE_NHMAC): Core Text through the glue.

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

/* A struct font_driver with macfont's entries (macfont.m), its work in
   the glue: the matching (glue/src/fontspec.rs) lists Core Text's
   faces, and a font's metrics and glyphs come rounded as macfont_open
   and macfont_glyph_extents round them (glue/src/font.rs).  C turns
   specs, entities and font objects, which are Lisp objects, into the
   glue's records and back.  A character's glyph and a glyph's metrics
   are read from the font's glyph table (glue/src/glyphs.rs) without a
   call; a miss fills a page.  Text is shaped by Core Text in the glue
   (nhg_font_shape, macfont_shape's rules), and runs are drawn as
   records (nhmac_glyph_run).  */

#include <config.h>

#include "lisp.h"
#include "blockinput.h"
#include "nhwlterm.h"
#include "frame.h"
#include "keyboard.h"
#include "character.h"
#include "composite.h"
#include "font.h"

struct nhmac_font_info
{
  struct font font;
  uint64_t handle;
  /* The glue's glyph table, read without a call; fixed while open.  */
  const NhgFontTable *table;
};

static Lisp_Object nhmac_font_cache;

/* The glue's id of NAME, a symbol or nil (0).  */
static uint32_t
nhmac_name_id (Lisp_Object name)
{
  uint32_t id = 0;

  if (SYMBOLP (name) && !NILP (name))
    {
      Lisp_Object s = SYMBOL_NAME (name);
      NHG (nhg_font_id (SSDATA (s), SBYTES (s), &id));
    }
  return id;
}

/* The symbol of the glue's name ID (nil for 0).  */
static Lisp_Object
nhmac_id_name (uint32_t id)
{
  char buf[256];
  size_t len = 0;

  if (id == 0 || NHG (nhg_font_name (id, buf, sizeof buf, &len)) != NHG_OK)
    return Qnil;
  return font_intern_prop (buf, len, true);
}

/* SPEC, a font spec or entity, as the glue's record.  */
static void
nhmac_spec_record (Lisp_Object spec, NhgFontSpec *r)
{
  Lisp_Object extra = AREF (spec, FONT_EXTRA_INDEX), v;

  memset (r, 0, sizeof *r);
  r->family = nhmac_name_id (AREF (spec, FONT_FAMILY_INDEX));
  r->foundry = nhmac_name_id (AREF (spec, FONT_FOUNDRY_INDEX));
  r->registry = nhmac_name_id (AREF (spec, FONT_REGISTRY_INDEX));
  r->weight = FIXNUMP (AREF (spec, FONT_WEIGHT_INDEX))
    ? FONT_WEIGHT_NUMERIC (spec) : -1;
  r->slant = FIXNUMP (AREF (spec, FONT_SLANT_INDEX))
    ? FONT_SLANT_NUMERIC (spec) : -1;
  r->width = FIXNUMP (AREF (spec, FONT_WIDTH_INDEX))
    ? FONT_WIDTH_NUMERIC (spec) : -1;
  r->spacing = FIXNUMP (AREF (spec, FONT_SPACING_INDEX))
    ? XFIXNUM (AREF (spec, FONT_SPACING_INDEX)) : -1;
  r->size = FIXNUMP (AREF (spec, FONT_SIZE_INDEX))
    ? XFIXNUM (AREF (spec, FONT_SIZE_INDEX)) : 0;
  /* :otf names a script by its OTF tag, which macfont takes as the
     spec's script (macfont_get_open_type_spec); :script comes first.  */
  v = assq_no_quit (QCscript, extra);
  if (CONSP (v))
    r->script = nhmac_name_id (XCDR (v));
  else
    {
      v = assq_no_quit (QCotf, extra);
      if (CONSP (v) && CONSP (XCDR (v)) && SYMBOLP (XCAR (XCDR (v)))
	  && !NILP (XCAR (XCDR (v))))
	{
	  Lisp_Object s = assq_no_quit (XCAR (XCDR (v)), Votf_script_alist);

	  if (CONSP (s) && SYMBOLP (XCDR (s)))
	    r->script = nhmac_name_id (XCDR (s));
	}
    }
  v = assq_no_quit (QClang, extra);
  if (CONSP (v))
    r->language = nhmac_name_id (XCDR (v));
}

/* The glue's entity E as a font entity of this driver.  The face's id
   and what is synthesised go in the extra slot.  */
static Lisp_Object
nhmac_entity (const NhgFontEntity *e)
{
  Lisp_Object entity = font_make_entity ();

  ASET (entity, FONT_TYPE_INDEX, Qnaglfar);
  ASET (entity, FONT_FOUNDRY_INDEX, nhmac_id_name (e->spec.foundry));
  ASET (entity, FONT_FAMILY_INDEX, nhmac_id_name (e->spec.family));
  ASET (entity, FONT_ADSTYLE_INDEX, Qnil);
  ASET (entity, FONT_REGISTRY_INDEX, Qiso10646_1);
  FONT_SET_STYLE (entity, FONT_WEIGHT_INDEX, make_fixnum (e->spec.weight));
  FONT_SET_STYLE (entity, FONT_SLANT_INDEX, make_fixnum (e->spec.slant));
  FONT_SET_STYLE (entity, FONT_WIDTH_INDEX, make_fixnum (e->spec.width));
  ASET (entity, FONT_SPACING_INDEX, make_fixnum (e->spec.spacing));
  ASET (entity, FONT_SIZE_INDEX, make_fixnum (0));
  ASET (entity, FONT_AVGWIDTH_INDEX, make_fixnum (0));
  ASET (entity, FONT_EXTRA_INDEX,
	list1 (Fcons (QCfont_entity,
		      Fcons (make_uint (e->font_id),
			     make_fixnum (e->synth)))));
  return entity;
}

/* Install the glue's font source with script-representative-chars,
   flattened: per script its name id, a count (NHG_FONT_ANY_CHAR set
   for a vector: any one character will do), then the characters.  */
static void
nhmac_font_start (void)
{
  ptrdiff_t n = 0, i = 0;
  Lisp_Object t;
  uint32_t *flat;

  for (t = Vscript_representative_chars; CONSP (t); t = XCDR (t))
    if (CONSP (XCAR (t)))
      n += 2 + (VECTORP (XCDR (XCAR (t))) ? ASIZE (XCDR (XCAR (t)))
		: XFIXNUM (Fsafe_length (XCDR (XCAR (t)))));
  flat = xmalloc (max (n, 1) * sizeof *flat);
  for (t = Vscript_representative_chars; CONSP (t); t = XCDR (t))
    if (CONSP (XCAR (t)) && SYMBOLP (XCAR (XCAR (t))))
      {
	Lisp_Object chars = XCDR (XCAR (t));
	ptrdiff_t at = i;
	uint32_t k = 0;

	flat[i++] = nhmac_name_id (XCAR (XCAR (t)));
	i++;
	if (VECTORP (chars))
	  {
	    for (ptrdiff_t j = 0; j < ASIZE (chars); j++)
	      if (RANGED_FIXNUMP (0, AREF (chars, j), MAX_UNICODE_CHAR))
		flat[i++] = XFIXNAT (AREF (chars, j)), k++;
	    k |= NHG_FONT_ANY_CHAR;
	  }
	else
	  for (; CONSP (chars); chars = XCDR (chars))
	    if (CHARACTERP (XCAR (chars)))
	      flat[i++] = XFIXNAT (XCAR (chars)), k++;
	flat[at + 1] = k;
      }
  NHG (nhg_font_start (flat, i));
  xfree (flat);
}

/* The fonts of SPEC: every one (ALL) or the nearest.  */
static Lisp_Object
nhmac_query (Lisp_Object spec, bool all)
{
  static bool started;
  NhgFontSpec r;
  NhgFontEntity stack[256], *out = stack;
  size_t n = 0;
  int32_t rc;
  Lisp_Object list = Qnil;

  if (!started)
    {
      nhmac_font_start ();
      started = true;
    }
  nhmac_spec_record (spec, &r);
  block_input ();
  rc = all ? NHG (nhg_font_list (&r, out, ARRAYELTS (stack), &n))
    : NHG (nhg_font_match (&r, out, 1, &n));
  /* More fonts than the stack holds (a spec with no family or script
     fits every face): ask again with room for them all.  */
  if (all && rc == NHG_OK && n > ARRAYELTS (stack))
    {
      size_t cap = n;

      out = xnmalloc (cap, sizeof *out);
      rc = NHG (nhg_font_list (&r, out, cap, &n));
      n = min (n, cap);
    }
  unblock_input ();
  if (rc == NHG_OK)
    for (size_t i = min (n, all ? n : 1); i > 0; i--)
      list = Fcons (nhmac_entity (&out[i - 1]), list);
  if (out != stack)
    xfree (out);
  return list;
}

static Lisp_Object
nhmacfont_get_cache (struct frame *f)
{
  return nhmac_font_cache;
}

static Lisp_Object
nhmacfont_list (struct frame *f, Lisp_Object spec)
{
  return nhmac_query (spec, true);
}

static Lisp_Object
nhmacfont_match (struct frame *f, Lisp_Object spec)
{
  Lisp_Object l = nhmac_query (spec, false);
  return CONSP (l) ? XCAR (l) : Qnil;
}

/* Whether FONT has C (macfont_has_char): an entity's face answers from
   its character set without opening it; an object is left to
   encode_char (-1).  */
static int
nhmacfont_has_char (Lisp_Object font, int c)
{
  Lisp_Object val;
  int32_t has = -1;

  if (!FONT_ENTITY_P (font))
    return -1;
  val = assq_no_quit (QCfont_entity, AREF (font, FONT_EXTRA_INDEX));
  if (!CONSP (val) || !CONSP (XCDR (val)))
    return -1;
  block_input ();
  if (NHG (nhg_font_has_char (XFIXNUM (XCAR (XCDR (val))), c, &has)) != NHG_OK)
    has = -1;
  unblock_input ();
  return has;
}

static Lisp_Object
nhmacfont_open (struct frame *f, Lisp_Object entity, int pixel_size)
{
  Lisp_Object val, font_object;
  struct nhmac_font_info *info;
  struct font *font;
  NhgFontMetrics m;
  uint32_t flags = 0;
  int size;

  val = assq_no_quit (QCfont_entity, AREF (entity, FONT_EXTRA_INDEX));
  if (!CONSP (val) || !CONSP (XCDR (val)))
    return Qnil;
  size = XFIXNUM (AREF (entity, FONT_SIZE_INDEX));
  if (size == 0)
    size = pixel_size;
  if (size == 0)
    size = FRAME_FONT (f) ? FRAME_FONT (f)->pixel_size : 12;
  /* What the entity synthesises, and :minspace (macfont_open).  */
  if (FIXNUMP (XCDR (XCDR (val))))
    flags = XFIXNUM (XCDR (XCDR (val))) & (NHG_FONT_SYNTH_ITALIC
					    | NHG_FONT_SYNTH_BOLD
					    | NHG_FONT_SYNTH_MONO);
  if (!NILP (Fcdr (assq_no_quit (QCminspace,
				 AREF (entity, FONT_EXTRA_INDEX)))))
    flags |= NHG_FONT_MINSPACE;

  block_input ();
  if (NHG (nhg_font_open (XFIXNUM (XCAR (XCDR (val))), size, flags, &m))
      != NHG_OK)
    {
      unblock_input ();
      return Qnil;
    }
  font_object = font_make_object (VECSIZE (struct nhmac_font_info),
				  entity, size);
  ASET (font_object, FONT_TYPE_INDEX, Qnaglfar);
  info = (struct nhmac_font_info *) XFONT_OBJECT (font_object);
  font = &info->font;
  info->handle = m.handle;
  if (NHG (nhg_font_table (m.handle, &info->table)) != NHG_OK)
    {
      NHG (nhg_font_close (m.handle));
      unblock_input ();
      return Qnil;
    }
  font->driver = &nhmacfont_driver;
  font->pixel_size = m.pixel_size;
  font->encoding_charset = font->repertory_charset = -1;
  font->ascent = m.ascent;
  font->descent = m.descent;
  font->height = m.height;
  font->space_width = m.space_width;
  font->average_width = m.average_width;
  font->min_width = font->max_width = m.space_width;
  font->underline_position = m.underline_position;
  font->underline_thickness = m.underline_thickness;
  font->baseline_offset = 0;
  font->relative_compose = 0;
  font->default_ascent = 0;
  font->vertical_centering = 0;
  font->props[FONT_NAME_INDEX] = Ffont_xlfd_name (font_object, Qnil, Qt);
  unblock_input ();
  return font_object;
}

static void
nhmacfont_close (struct font *font)
{
  struct nhmac_font_info *info = (struct nhmac_font_info *) font;

  if (font_data_structures_may_be_ill_formed () || !info->handle)
    return;
  NHG (nhg_font_close (info->handle));
  info->handle = 0;
}

/* Character C's glyph, from the font's table; a page not read yet is
   filled first.  */
static unsigned
nhmacfont_encode_char (struct font *font, int c)
{
  struct nhmac_font_info *info = (struct nhmac_font_info *) font;
  unsigned page = (unsigned) c / NHG_FONT_PAGE;
  uint32_t glyph;

  if (c < 0 || page >= NHG_FONT_CHAR_PAGES)
    return FONT_INVALID_CODE;
  if (!info->table->chars[page])
    NHG (nhg_font_fill (info->handle, page));
  if (!info->table->chars[page])
    return FONT_INVALID_CODE;
  glyph = info->table->chars[page]->glyph[c % NHG_FONT_PAGE];
  return glyph == NHG_GLYPH_NONE ? FONT_INVALID_CODE : glyph;
}

/* Glyph G's metrics from the font's table (NULL: no such glyph).  */
static const NhgGlyphMetrics *
nhmac_glyph_metrics (struct nhmac_font_info *info, unsigned g)
{
  unsigned page = g / NHG_FONT_PAGE;

  if (page >= NHG_FONT_GLYPH_PAGES)
    return NULL;
  if (!info->table->glyphs[page])
    NHG (nhg_font_fill (info->handle, page | NHG_FONT_FILL_GLYPHS));
  if (!info->table->glyphs[page])
    return NULL;
  return &info->table->glyphs[page]->metrics[g % NHG_FONT_PAGE];
}

static void
nhmacfont_text_extents (struct font *font, const unsigned int *code,
			int nglyphs, struct font_metrics *metrics)
{
  struct nhmac_font_info *info = (struct nhmac_font_info *) font;
  int total = 0;

  if (metrics)
    memset (metrics, 0, sizeof *metrics);
  for (int i = 0; i < nglyphs; i++)
    {
      const NhgGlyphMetrics *m = nhmac_glyph_metrics (info, code[i]);
      NhgGlyphMetrics g = m ? *m
	: (NhgGlyphMetrics) { 0, font->space_width, font->space_width,
			      font->ascent, font->descent };

      if (metrics)
	{
	  if (total + g.lbearing < metrics->lbearing)
	    metrics->lbearing = total + g.lbearing;
	  if (total + g.rbearing > metrics->rbearing)
	    metrics->rbearing = total + g.rbearing;
	  if (g.ascent > metrics->ascent)
	    metrics->ascent = g.ascent;
	  if (g.descent > metrics->descent)
	    metrics->descent = g.descent;
	}
      total += g.width;
    }
  if (metrics)
    metrics->width = total;
}

static int
nhmacfont_draw (struct glyph_string *s, int from, int to, int x, int y,
		bool with_background)
{
  nhmac_glyph_run (s, ((struct nhmac_font_info *) s->font)->handle, from, to,
		   x, y, with_background);
  return to - from;
}

/* LGSTRING shaped by Core Text (macfont_shape): each glyph's cluster,
   character, code, metrics and adjustment from the glue's records.  */
static Lisp_Object
nhmacfont_shape (Lisp_Object lgstring, Lisp_Object direction)
{
  struct font *font = CHECK_FONT_GET_OBJECT (LGSTRING_FONT (lgstring));
  struct nhmac_font_info *info = (struct nhmac_font_info *) font;
  ptrdiff_t glyph_len = LGSTRING_GLYPH_LEN (lgstring), len;
  uint32_t *chars;
  NhgShaped *out;
  size_t used = 0;
  USE_SAFE_ALLOCA;

  for (len = 0; len < glyph_len; len++)
    if (NILP (LGSTRING_GLYPH (lgstring, len)))
      break;
  SAFE_NALLOCA (chars, 1, len);
  SAFE_NALLOCA (out, 1, glyph_len);
  for (ptrdiff_t i = 0; i < len; i++)
    chars[i] = LGLYPH_CHAR (LGSTRING_GLYPH (lgstring, i));
  block_input ();
  if (NHG (nhg_font_shape (info->handle, chars, len, out, glyph_len, &used))
      != NHG_OK)
    used = 0;
  unblock_input ();
  for (size_t i = 0; i < used; i++)
    {
      Lisp_Object lglyph = LGSTRING_GLYPH (lgstring, i);
      const NhgShaped *g = &out[i];

      if (NILP (lglyph))
	{
	  lglyph = LGLYPH_NEW ();
	  LGSTRING_SET_GLYPH (lgstring, i, lglyph);
	}
      LGLYPH_SET_FROM (lglyph, g->from);
      LGLYPH_SET_TO (lglyph, g->to);
      LGLYPH_SET_CHAR (lglyph, g->c);
      LGLYPH_SET_CODE (lglyph, g->code);
      LGLYPH_SET_WIDTH (lglyph, g->m.width);
      LGLYPH_SET_LBEARING (lglyph, g->m.lbearing);
      LGLYPH_SET_RBEARING (lglyph, g->m.rbearing);
      LGLYPH_SET_ASCENT (lglyph, g->m.ascent);
      LGLYPH_SET_DESCENT (lglyph, g->m.descent);
      if (g->adjust)
	LGLYPH_SET_ADJUSTMENT (lglyph, CALLN (Fvector, make_fixnum (g->xoff),
					      make_fixnum (g->yoff),
					      make_fixnum (g->wadjust)));
    }
  SAFE_FREE ();
  return used ? make_fixnum (used) : Qnil;
}

/* The glyphs of C's 256 variation sequences (macfont_variation_glyphs),
   from the font's cmap format-14 table; how many the font has.  */
static int
nhmacfont_variation_glyphs (struct font *font, int c, unsigned variations[256])
{
  uint32_t v[256], n = 0;

  block_input ();
  if (NHG (nhg_font_variation_glyphs (((struct nhmac_font_info *) font)->handle,
				      c, v, &n)) != NHG_OK)
    n = 0;
  unblock_input ();
  for (int i = 0; i < 256; i++)
    variations[i] = n ? v[i] : 0;
  return n;
}

struct font_driver const nhmacfont_driver =
  {
    .type = LISPSYM_INITIALLY (Qnaglfar),
    .case_sensitive = false,
    .get_cache = nhmacfont_get_cache,
    .list = nhmacfont_list,
    .match = nhmacfont_match,
    .has_char = nhmacfont_has_char,
    .open_font = nhmacfont_open,
    .close_font = nhmacfont_close,
    .encode_char = nhmacfont_encode_char,
    .text_extents = nhmacfont_text_extents,
    .draw = nhmacfont_draw,
    .shape = nhmacfont_shape,
    .get_variation_glyphs = nhmacfont_variation_glyphs,
  };

void
syms_of_nhmacfont (void)
{
  DEFSYM (Qnaglfar, "naglfar");
  DEFSYM (QCminspace, ":minspace");
  nhmac_font_cache = list1 (Qnil);
  staticpro (&nhmac_font_cache);
}

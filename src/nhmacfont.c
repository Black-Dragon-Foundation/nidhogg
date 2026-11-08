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
   glue's records and back.  For now one family's worth: ASCII, no
   shaping, glyph metrics one glyph at a time; runs are drawn as
   records (nhmac_glyph_run).  */

#include <config.h>

#include "lisp.h"
#include "blockinput.h"
#include "nhwlterm.h"
#include "frame.h"
#include "keyboard.h"
#include "character.h"
#include "font.h"

struct nhmac_font_info
{
  struct font font;
  uint64_t handle;
  bool mono;
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
  v = assq_no_quit (QCscript, extra);
  if (CONSP (v))
    r->script = nhmac_name_id (XCDR (v));
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

/* The fonts of SPEC: every one (ALL) or the nearest.  */
static Lisp_Object
nhmac_query (Lisp_Object spec, bool all)
{
  static bool started;
  NhgFontSpec r;
  NhgFontEntity out[256];
  size_t n = 0;
  int32_t rc;
  Lisp_Object list = Qnil;

  if (!started)
    {
      NHG (nhg_font_start ());
      started = true;
    }
  nhmac_spec_record (spec, &r);
  block_input ();
  rc = all ? NHG (nhg_font_list (&r, out, ARRAYELTS (out), &n))
    : NHG (nhg_font_match (&r, out, 1, &n));
  unblock_input ();
  if (rc != NHG_OK)
    return Qnil;
  for (size_t i = n; i > 0; i--)
    list = Fcons (nhmac_entity (&out[i - 1]), list);
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

static Lisp_Object
nhmacfont_open (struct frame *f, Lisp_Object entity, int pixel_size)
{
  Lisp_Object val, font_object;
  struct nhmac_font_info *info;
  struct font *font;
  NhgFontMetrics m;
  int size;

  val = assq_no_quit (QCfont_entity, AREF (entity, FONT_EXTRA_INDEX));
  if (!CONSP (val) || !CONSP (XCDR (val)))
    return Qnil;
  size = XFIXNUM (AREF (entity, FONT_SIZE_INDEX));
  if (size == 0)
    size = pixel_size;
  if (size == 0)
    size = FRAME_FONT (f) ? FRAME_FONT (f)->pixel_size : 12;

  block_input ();
  if (NHG (nhg_font_open (XFIXNUM (XCAR (XCDR (val))), size, &m)) != NHG_OK)
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
  info->mono = m.mono != 0;
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

static unsigned
nhmacfont_encode_char (struct font *font, int c)
{
  struct nhmac_font_info *info = (struct nhmac_font_info *) font;
  uint32_t glyph;

  if (NHG (nhg_font_glyph (info->handle, c, &glyph)) != NHG_OK)
    return FONT_INVALID_CODE;
  return glyph;
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
      NhgGlyphMetrics g;

      if (NHG (nhg_font_glyph_metrics (info->handle, code[i], info->mono,
				       font->space_width, &g)) != NHG_OK)
	g = (NhgGlyphMetrics) { 0, font->space_width, font->space_width,
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

struct font_driver const nhmacfont_driver =
  {
    .type = LISPSYM_INITIALLY (Qnaglfar),
    .case_sensitive = false,
    .get_cache = nhmacfont_get_cache,
    .list = nhmacfont_list,
    .match = nhmacfont_match,
    .open_font = nhmacfont_open,
    .close_font = nhmacfont_close,
    .encode_char = nhmacfont_encode_char,
    .text_extents = nhmacfont_text_extents,
    .draw = nhmacfont_draw,
  };

void
syms_of_nhmacfont (void)
{
  DEFSYM (Qnaglfar, "naglfar");
  nhmac_font_cache = list1 (Qnil);
  staticpro (&nhmac_font_cache);
}

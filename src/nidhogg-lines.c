/* The line model's change hook and Lisp interface (Hlidskjalf).

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

/* A buffer text with a line model (nidhogg/hlidskjalf: its lines,
   marks, folds, hidden lines and trail) is fed from the edit history's
   change sites (nidhogg-history.h): after each change the hook sends
   the model the new text of the whole lines the change touched.  The
   table keeps Z and Z_BYTE as the last change left them, as the
   history's does: a change whose sizes do not add up, or one the model
   cannot apply, builds the lines again from the text.  A text of
   `nidhogg-lines-sync-limit' bytes or more is built on a thread of its
   own from a copy; the hook's edits are kept and replayed meanwhile.
   An indirect buffer shares its base's text, so the table is keyed by
   the text.  */

#include <config.h>

#ifdef HAVE_NIDHOGG_HISTORY

#include <string.h>

#include "lisp.h"
#include "buffer.h"
#include "coding.h"
#include "window.h"
#include "nidhogg-history.h"

#include <hlidskjalf.h>
#include <glitnir.h>

struct nh_lines_text
{
  struct buffer_text *text;
  ptrdiff_t z, z_byte;
};

int nh_lines_ntracked;
static struct nh_lines_text *nh_lines_texts;
static ptrdiff_t nh_lines_size;
static uintmax_t nh_lines_rebuilds;

static uint64_t
nh_lines_key (struct buffer_text *text)
{
  return (uintptr_t) text;
}

static struct nh_lines_text *
nh_lines_find (struct buffer_text *text)
{
  for (int i = 0; i < nh_lines_ntracked; i++)
    if (nh_lines_texts[i].text == text)
      return &nh_lines_texts[i];
  return NULL;
}

/* The char-width-table the line model's character widths were last
   taken from, and those widths: ranges (first, last, width) of the
   characters past ASCII whose width is not 1.  */
static Lisp_Object nh_lines_widths_of;
static uint32_t *nh_lines_widths;
static ptrdiff_t nh_lines_widths_len, nh_lines_widths_cap;

static void
nh_lines_width_range (Lisp_Object arg, Lisp_Object key, Lisp_Object val)
{
  if (!FIXNUMP (val))
    return;
  int w = sanitize_char_width (XFIXNUM (val));
  EMACS_INT from = CONSP (key) ? XFIXNUM (XCAR (key)) : XFIXNUM (key);
  EMACS_INT to = CONSP (key) ? XFIXNUM (XCDR (key)) : XFIXNUM (key);
  if (w == 1 || to < 0x80)
    return;
  if (nh_lines_widths_cap - nh_lines_widths_len < 3)
    nh_lines_widths = xpalloc (nh_lines_widths, &nh_lines_widths_cap, 3, -1,
			       sizeof *nh_lines_widths);
  nh_lines_widths[nh_lines_widths_len++] = max (from, 0x80);
  nh_lines_widths[nh_lines_widths_len++] = to;
  nh_lines_widths[nh_lines_widths_len++] = w;
}

/* Send `char-width-table' to the line model when it holds another table
   than the one sent last, or when FORCE (the table changed in place):
   the models built with the old widths are built again when next
   asked for their widest line.  */
static void
nh_lines_widths_sync (bool force)
{
  if (!force && EQ (Vchar_width_table, nh_lines_widths_of))
    return;
  nh_lines_widths_of = Vchar_width_table;
  nh_lines_widths_len = 0;
  if (CHAR_TABLE_P (Vchar_width_table))
    map_char_table (nh_lines_width_range, Qnil, Vchar_width_table, Qnil);
  hlid_set_widths (nh_lines_widths, nh_lines_widths_len / 3);
}

/* Build B's lines from its whole text (BEG..Z, narrowing ignored), on a
   thread of their own when the text is large and BACKGROUND.  */
static void
nh_lines_build (struct buffer *b, struct nh_lines_text *t, bool background)
{
  ptrdiff_t gpt = BUF_GPT_BYTE (b), z = BUF_Z_BYTE (b);
  const unsigned char *p1 = BUF_BEG_ADDR (b);
  size_t n1 = gpt - BEG_BYTE;
  const unsigned char *p2 = BUF_GAP_END_ADDR (b);
  size_t n2 = z - gpt;
  bool bg = background && FIXNATP (Vnidhogg_lines_sync_limit)
	    && n1 + n2 >= XFIXNAT (Vnidhogg_lines_sync_limit);
  nh_lines_widths_sync (false);
  if (hlid_build (nh_lines_key (b->text),
		  !NILP (BVAR (b, enable_multibyte_characters)),
		  SANE_TAB_WIDTH (b),
		  n1 ? p1 : NULL, n1, n2 ? p2 : NULL, n2, bg)
      != HLID_OK)
    emacs_abort ();
  t->z = BUF_Z (b);
  t->z_byte = BUF_Z_BYTE (b);
  nh_lines_rebuilds++;
}

/* The byte after the last newline before BYTE in B (BEG_BYTE when none).  */
static ptrdiff_t
nh_lines_bol (struct buffer *b, ptrdiff_t byte)
{
  ptrdiff_t gpt = BUF_GPT_BYTE (b);
  if (byte > gpt)
    {
      const unsigned char *base = BUF_BYTE_ADDRESS (b, gpt);
      const unsigned char *nl = memrchr (base, '\n', byte - gpt);
      if (nl)
	return gpt + (nl - base) + 1;
      byte = gpt;
    }
  const unsigned char *base = BUF_BEG_ADDR (b);
  const unsigned char *nl = memrchr (base, '\n', byte - BEG_BYTE);
  return nl ? BEG_BYTE + (nl - base) + 1 : BEG_BYTE;
}

/* The byte after the first newline at or after BYTE in B (Z_BYTE when none).  */
static ptrdiff_t
nh_lines_eol (struct buffer *b, ptrdiff_t byte)
{
  ptrdiff_t gpt = BUF_GPT_BYTE (b), z = BUF_Z_BYTE (b);
  if (byte < gpt)
    {
      const unsigned char *base = BUF_BYTE_ADDRESS (b, byte);
      const unsigned char *nl = memchr (base, '\n', gpt - byte);
      if (nl)
	return byte + (nl - base) + 1;
      byte = gpt;
    }
  if (byte >= z)
    return z;
  const unsigned char *base = BUF_BYTE_ADDRESS (b, byte);
  const unsigned char *nl = memchr (base, '\n', z - byte);
  return nl ? byte + (nl - base) + 1 : z;
}

void
nh_lines_change (struct buffer *b,
		 ptrdiff_t start, ptrdiff_t start_byte,
		 ptrdiff_t old_end, ptrdiff_t old_end_byte,
		 ptrdiff_t new_end, ptrdiff_t new_end_byte)
{
  struct nh_lines_text *t = nh_lines_find (b->text);
  if (!t)
    return;
  ptrdiff_t z = BUF_Z (b), z_byte = BUF_Z_BYTE (b);
  bool ok = (BEG <= start && start <= old_end && start <= new_end
	     && old_end <= t->z && new_end <= z
	     && z - t->z == new_end - old_end
	     && z_byte - t->z_byte == ((new_end_byte - start_byte)
				       - (old_end_byte - start_byte)));
  if (ok)
    {
      /* The new text of the whole lines the change touched, in two
	 parts when the gap is inside it.  */
      ptrdiff_t ls = nh_lines_bol (b, start_byte);
      ptrdiff_t le = nh_lines_eol (b, new_end_byte);
      ptrdiff_t gpt = BUF_GPT_BYTE (b);
      const unsigned char *p1 = BUF_BYTE_ADDRESS (b, ls), *p2 = NULL;
      size_t n1 = le - ls, n2 = 0;
      if (ls < gpt && gpt < le)
	{
	  n1 = gpt - ls;
	  p2 = BUF_BYTE_ADDRESS (b, gpt);
	  n2 = le - gpt;
	}
      ok = hlid_edit (nh_lines_key (b->text), start, old_end, new_end,
		      n1 ? p1 : NULL, n1, p2, n2) == HLID_OK;
      t->z = z;
      t->z_byte = z_byte;
    }
  if (!ok)
    nh_lines_build (b, t, true);
}

void
nh_lines_resync (struct buffer *b)
{
  struct nh_lines_text *t = nh_lines_find (b->text);
  if (t)
    nh_lines_build (b, t, true);
}

static void
nh_lines_untrack (struct nh_lines_text *t)
{
  hlid_drop (nh_lines_key (t->text));
  *t = nh_lines_texts[--nh_lines_ntracked];
}

void
nh_lines_forget (struct buffer *b)
{
  struct nh_lines_text *t = nh_lines_find (b->text);
  if (t)
    nh_lines_untrack (t);
}

/* B's model key, after a stale model was built again; 0 when B's text has no model.  */
uint64_t
nh_lines_model (struct buffer *b)
{
  struct nh_lines_text *t = BUFFER_LIVE_P (b) ? nh_lines_find (b->text) : NULL;
  if (!t)
    return 0;
  hlid_stats s;
  if (hlid_describe (nh_lines_key (b->text), &s) == HLID_OK && s.state == 2)
    nh_lines_build (b, t, true);
  return nh_lines_key (b->text);
}

/* The visible-line fraction of POS in B's accessible part, for the scroll
   bar; false when B has no model or its lines are being built.  */
bool
nh_lines_fraction (struct buffer *b, ptrdiff_t pos, double *out)
{
  struct nh_lines_text *t = nh_lines_find (b->text);
  if (!t)
    return false;
  int r = hlid_fraction (nh_lines_key (b->text), pos, BUF_BEGV (b),
			 BUF_ZV (b), out);
  if (r == HLID_STALE)
    nh_lines_build (b, t, true);
  return r == HLID_OK;
}

/* The columns of the widest visible line in B's accessible part into
   *COLS (the horizontal strip's measure); false when B has no model or
   its lines are being built.  Lines built with another tab width or
   other character widths are built again first.  */
bool
nh_lines_widest (struct buffer *b, uint32_t *cols)
{
  struct nh_lines_text *t = nh_lines_find (b->text);
  if (!t)
    return false;
  nh_lines_widths_sync (false);
  uint64_t key = nh_lines_key (b->text);
  int r = hlid_widest (key, (uintptr_t) b, BUF_BEGV (b), BUF_ZV (b),
		       SANE_TAB_WIDTH (b), cols);
  if (r == HLID_STALE)
    {
      nh_lines_build (b, t, true);
      r = hlid_widest (key, (uintptr_t) b, BUF_BEGV (b), BUF_ZV (b),
		       SANE_TAB_WIDTH (b), cols);
    }
  return r == HLID_OK;
}

/* Whether POS's visible line in B is folded (a fold starts on it, the
   lines after it hidden), for the terminal's fold margin; false when B
   has no model.  */
bool
nh_lines_folded_at (struct buffer *b, ptrdiff_t pos)
{
  struct nh_lines_text *t = nh_lines_find (b->text);
  return t && hlid_folded_at (nh_lines_key (b->text), pos) == HLID_OK;
}

/* The display width in columns of POS's line in B into *COLS, for the
   horizontal strip's rows cut while their line fits; false when B has
   no model or it is not built.  */
bool
nh_lines_cols_at (struct buffer *b, ptrdiff_t pos, uint32_t *cols)
{
  struct nh_lines_text *t = nh_lines_find (b->text);
  return t && hlid_cols_at (nh_lines_key (b->text), pos, cols) == HLID_OK;
}

static struct buffer *
nh_lines_buffer (Lisp_Object buffer)
{
  return decode_buffer (buffer);
}

static uint64_t
nh_lines_key_of (Lisp_Object buffer)
{
  uint64_t key = nh_lines_model (nh_lines_buffer (buffer));
  if (!key)
    error ("No line model for this buffer (nidhogg-lines-enable)");
  return key;
}

static uint64_t
nh_lines_pos (Lisp_Object pos)
{
  return fix_position (pos);
}

static uint8_t
nh_lines_byte (Lisp_Object x)
{
  if (NILP (x))
    return 0;
  CHECK_FIXNAT (x);
  return min (XFIXNAT (x), 255);
}

DEFUN ("nidhogg-lines-enable", Fnidhogg_lines_enable,
       Snidhogg_lines_enable, 0, 2, 0,
       doc: /* Keep a line model of BUFFER's text (the current buffer by default).
The model holds the text's lines, the marks set with
`nidhogg-lines-set-marks', the folds set with `nidhogg-lines-set-folds',
the hidden ranges and the trail; the scroll bar measures visible lines
through it.  A text of `nidhogg-lines-sync-limit' bytes or more is read
on a thread of its own: its line answers are nil until that finishes,
unless WAIT is non-nil.  An indirect buffer shares its base buffer's
text, so enabling either enables both.  Return t.  */)
  (Lisp_Object buffer, Lisp_Object wait)
{
  struct buffer *b = nh_lines_buffer (buffer);
  if (!BUFFER_LIVE_P (b))
    error ("Selecting deleted buffer");
  struct nh_lines_text *t = nh_lines_find (b->text);
  if (!t)
    {
      if (nh_lines_ntracked == nh_lines_size)
	nh_lines_texts = xpalloc (nh_lines_texts, &nh_lines_size, 1,
				  INT_MAX, sizeof *nh_lines_texts);
      t = &nh_lines_texts[nh_lines_ntracked++];
      t->text = b->text;
      nh_lines_build (b, t, true);
    }
  if (!NILP (wait))
    hlid_wait (nh_lines_key (b->text));
  return Qt;
}

DEFUN ("nidhogg-lines-disable", Fnidhogg_lines_disable,
       Snidhogg_lines_disable, 0, 1, 0,
       doc: /* Drop the line model of BUFFER's text (the current buffer by default).  */)
  (Lisp_Object buffer)
{
  struct buffer *b = nh_lines_buffer (buffer);
  struct nh_lines_text *t = nh_lines_find (b->text);
  if (t)
    nh_lines_untrack (t);
  return Qnil;
}

DEFUN ("nidhogg-lines-line", Fnidhogg_lines_line, Snidhogg_lines_line,
       2, 2, 0,
       doc: /* The 0-based line of position POS in BUFFER's text, narrowing ignored.
nil while the model's lines are being built.  */)
  (Lisp_Object buffer, Lisp_Object pos)
{
  uint64_t key = nh_lines_key_of (buffer), l;
  return hlid_line_of (key, nh_lines_pos (pos), &l) == HLID_OK
	 ? make_uint (l) : Qnil;
}

/* The display class of highlight capture or face NAME (LEN bytes), by
   its leading name (`keyword' for `keyword.return'), or by an Emacs face
   name (`font-lock-keyword-face'): 1 keyword, 2 function, 3 type, 4
   string, 5 comment, 6 constant, 7 variable, 8 property, 9 operator, 10
   punctuation, 11 added, 12 removed, 13 heading, 14 link, 15 special; 0
   (text, nothing drawn over the line) for another.  The line model's
   class table (design section 7.2).  */
int
nh_lines_class_of (const char *name, ptrdiff_t len)
{
  static const struct { const char *name; int class; } table[] = {
    /* the longer names of a prefix first */
    { "string.special.url", 14 }, { "string.escape", 15 },
    { "string.regexp", 15 }, { "string.special", 15 },
    { "markup.heading", 13 }, { "markup.link", 14 }, { "markup.raw", 4 },
    { "diff.plus", 11 }, { "diff.minus", 12 }, { "diff.delta", 15 },
    { "keyword", 1 }, { "conditional", 1 }, { "repeat", 1 },
    { "exception", 1 }, { "include", 1 }, { "preproc", 1 },
    { "function", 2 }, { "method", 2 },
    { "type", 3 }, { "constructor", 3 }, { "module", 3 },
    { "namespace", 3 },
    { "string", 4 }, { "character", 4 },
    { "comment", 5 },
    { "constant", 6 }, { "number", 6 }, { "float", 6 }, { "boolean", 6 },
    { "variable", 7 }, { "parameter", 7 },
    { "property", 8 }, { "field", 8 }, { "attribute", 8 },
    { "operator", 9 }, { "punctuation", 10 },
    { "label", 15 }, { "tag", 15 }, { "escape", 15 },
    /* Emacs's faces, `font-lock-' and `-face' taken off */
    { "builtin", 1 }, { "preprocessor", 1 }, { "negation-char", 1 },
    { "function-name", 2 }, { "function-call", 2 },
    { "doc", 5 }, { "comment-delimiter", 5 },
    { "variable-name", 7 }, { "variable-use", 7 },
    { "property-name", 8 }, { "property-use", 8 },
    { "bracket", 10 }, { "delimiter", 10 }, { "misc-punctuation", 10 },
    { "regexp", 15 }, { "warning", 15 },
  };
  if (len > 10 && !memcmp (name, "font-lock-", 10))
    {
      name += 10;
      len -= 10;
      if (len > 5 && !memcmp (name + len - 5, "-face", 5))
	len -= 5;
    }
  for (size_t i = 0; i < sizeof table / sizeof *table; i++)
    {
      ptrdiff_t k = strlen (table[i].name);
      if (len >= k && !memcmp (name, table[i].name, k)
	  && (len == k || name[k] == '.'))
	return table[i].class;
    }
  return 0;
}

/* The class of text property face value FACE: a face's, or the first
   face's of a list of them; 0 for none, an anonymous face or another.  */
static int
nh_lines_face_class (Lisp_Object face)
{
  if (CONSP (face))
    face = XCAR (face);
  if (!SYMBOLP (face) || NILP (face) || EQ (face, Qt)
      || SREF (SYMBOL_NAME (face), 0) == ':')
    return 0;
  Lisp_Object name = SYMBOL_NAME (face);
  return nh_lines_class_of (SSDATA (name), SBYTES (name));
}

/* The face text properties of the current buffer's text from BEG to END,
   widened to whole lines, as the class runs of their lines in model KEY
   (design section 7.2: buffers the tree-sitter highlights do not
   colour).  */
static void
nh_lines_faces (uint64_t key, ptrdiff_t beg, ptrdiff_t end)
{
  ptrdiff_t bb = CHAR_TO_BYTE (beg), eb = CHAR_TO_BYTE (end);
  while (bb > BEGV_BYTE && FETCH_BYTE (bb - 1) != '\n')
    bb--;
  while (eb < ZV_BYTE && FETCH_BYTE (eb) != '\n')
    eb++;
  beg = BYTE_TO_CHAR (bb);
  end = BYTE_TO_CHAR (eb);
  hlid_span *v = NULL;
  ptrdiff_t n = 0, cap = 0;
  for (ptrdiff_t p = beg; p < end;)
    {
      Lisp_Object next = Fnext_single_property_change (make_fixnum (p), Qface,
						       Qnil, make_fixnum (end));
      ptrdiff_t q = FIXNUMP (next) ? XFIXNUM (next) : end;
      int class = nh_lines_face_class (Fget_text_property (make_fixnum (p),
							    Qface, Qnil));
      if (class)
	{
	  if (n == cap)
	    v = xpalloc (v, &cap, 1, -1, sizeof *v);
	  v[n++] = (hlid_span) { .beg = p, .end = q, .class_ = class };
	}
      p = max (q, p + 1);
    }
  hlid_replace_classes (key, beg, end, v, n);
  xfree (v);
}

/* After `fontification-functions' ran at POS in B (current): the faces
   of the fontified run around POS, within 4096 characters of it, as the
   class runs of their lines.  Nothing for a buffer without a line model
   or whose tree-sitter highlights colour it
   (`nidhogg-treesit-highlight-query').  */
void
nh_lines_take_faces (struct buffer *b, ptrdiff_t pos)
{
  if (!nh_lines_ntracked || b != current_buffer || pos < BEGV || pos >= ZV)
    return;
#if HAVE_TREE_SITTER
  if (!NILP (Fbuffer_local_value (Qnidhogg_treesit_highlight_query,
				  Fcurrent_buffer ())))
    return;
#endif
  uint64_t key = nh_lines_model (b);
  if (!key || NILP (Fget_text_property (make_fixnum (pos), Qfontified, Qnil)))
    return;
  Lisp_Object lo = make_fixnum (max (BEGV, pos - 4096));
  Lisp_Object hi = make_fixnum (min (ZV, pos + 4096));
  Lisp_Object b0 = Fprevious_single_property_change (make_fixnum (pos + 1),
						     Qfontified, Qnil, lo);
  Lisp_Object e0 = Fnext_single_property_change (make_fixnum (pos),
						 Qfontified, Qnil, hi);
  nh_lines_faces (key, FIXNUMP (b0) ? XFIXNUM (b0) : XFIXNUM (lo),
		  FIXNUMP (e0) ? XFIXNUM (e0) : XFIXNUM (hi));
}

DEFUN ("nidhogg-lines-take-faces", Fnidhogg_lines_take_faces,
       Snidhogg_lines_take_faces, 0, 3, 0,
       doc: /* Take the face text properties of BUFFER's text from BEG to END as class runs.
The lines from BEG to END (default: the accessible part) get the classes
of their faces, as redisplay gives text it fontifies: for text fontified
otherwise, such as by `font-lock-ensure'.  A face's class comes from its
name (`font-lock-keyword-face' is a keyword, and so on); other faces draw
as text.  Returns nil.  */)
  (Lisp_Object buffer, Lisp_Object beg, Lisp_Object end)
{
  struct buffer *b = nh_lines_buffer (buffer);
  uint64_t key = nh_lines_model (b);
  if (!key)
    return Qnil;
  specpdl_ref count = SPECPDL_INDEX ();
  record_unwind_current_buffer ();
  set_buffer_internal (b);
  ptrdiff_t from = NILP (beg) ? BEGV : clip_to_bounds (BEGV, fix_position (beg), ZV);
  ptrdiff_t to = NILP (end) ? ZV : clip_to_bounds (from, fix_position (end), ZV);
  nh_lines_faces (key, from, to);
  return unbind_to (count, Qnil);
}

DEFUN ("nidhogg-lines-take-hub-folds", Fnidhogg_lines_take_hub_folds,
       Snidhogg_lines_take_hub_folds, 2, 2, 0,
       doc: /* Take the hub's fold ranges of FILE into BUFFER's line model.
FILE is the name the hub keys the file's minimap layers by (its true
name); its `folds' layer, which the hub sends while a connection watches
the file (`minimap/watch'), becomes the model's hub fold source (1),
once per layer version.  Returns t when the model holds the ranges of
the layer's latest version, nil when there is no such layer yet, the
buffer's tree-sitter parser folds it (source 0), or the lines are being
built.  */)
  (Lisp_Object buffer, Lisp_Object file)
{
  CHECK_STRING (file);
  uint64_t key = nh_lines_model (nh_lines_buffer (buffer));
  if (!key)
    return Qnil;
  Lisp_Object u = ENCODE_UTF_8 (file);
  return (glitnir_minimap_take_folds (key, SDATA (u), SBYTES (u)) == 1
	  ? Qt : Qnil);
}

DEFUN ("nidhogg-lines-classes", Fnidhogg_lines_classes,
       Snidhogg_lines_classes, 2, 2, 0,
       doc: /* The class runs of 0-based LINE in BUFFER's text, as (COL LEN CLASS).
COL counts characters from the line's start, CLASS is the display class
(1 keyword ... 15 special, as `nidhogg-treesit-highlight-query' says).
nil while the model's lines are being built, or past the last line.  */)
  (Lisp_Object buffer, Lisp_Object line)
{
  uint64_t key = nh_lines_key_of (buffer);
  CHECK_FIXNAT (line);
  hlid_run r[HLID_RUNS];
  size_t n;
  if (hlid_line_runs (key, XFIXNAT (line), r, &n) != HLID_OK)
    return Qnil;
  Lisp_Object l = Qnil;
  while (n-- > 0)
    l = Fcons (list3 (make_fixnum (r[n].col), make_fixnum (r[n].len),
		      make_fixnum (r[n].class_)), l);
  return l;
}

DEFUN ("nidhogg-lines-line-start", Fnidhogg_lines_line_start,
       Snidhogg_lines_line_start, 2, 2, 0,
       doc: /* The position 0-based LINE of BUFFER's text starts at.
Past the last line, the end of the text.  nil while the lines are being built.  */)
  (Lisp_Object buffer, Lisp_Object line)
{
  CHECK_FIXNAT (line);
  uint64_t key = nh_lines_key_of (buffer), p;
  return hlid_pos_of (key, XFIXNAT (line), &p) == HLID_OK
	 ? make_uint (p) : Qnil;
}

DEFUN ("nidhogg-lines-fraction", Fnidhogg_lines_fraction,
       Snidhogg_lines_fraction, 2, 2, 0,
       doc: /* Where POS sits in BUFFER's accessible part, from 0.0 to 1.0, by visible lines.
Lines hidden with `nidhogg-lines-set-hidden' do not count.  nil while
the lines are being built.  This is the scroll bar's measure.  */)
  (Lisp_Object buffer, Lisp_Object pos)
{
  struct buffer *b = nh_lines_buffer (buffer);
  uint64_t key = nh_lines_key_of (buffer);
  double f;
  return hlid_fraction (key, nh_lines_pos (pos), BUF_BEGV (b), BUF_ZV (b), &f)
	 == HLID_OK ? make_float (f) : Qnil;
}

DEFUN ("nidhogg-lines-widest", Fnidhogg_lines_widest,
       Snidhogg_lines_widest, 0, 2, 0,
       doc: /* The columns of the widest visible line in BUFFER's accessible part.
Lines hidden with `nidhogg-lines-set-hidden' do not count.  A tab reaches
the next `tab-width' stop, another control character takes 2 columns and
any other character its `char-width-table' width.  nil while the lines
are being built.  The model takes `char-width-table' again when the
variable holds another table, or with REFRESH non-nil (after the table
was changed in place), and builds the lines again when the widths or
`tab-width' changed.  This is the horizontal strip's measure.  */)
  (Lisp_Object buffer, Lisp_Object refresh)
{
  struct buffer *b = nh_lines_buffer (buffer);
  nh_lines_key_of (buffer);
  if (!NILP (refresh))
    nh_lines_widths_sync (true);
  uint32_t cols;
  return nh_lines_widest (b, &cols) ? make_fixnum (cols) : Qnil;
}

DEFUN ("nidhogg-lines-set-marks", Fnidhogg_lines_set_marks,
       Snidhogg_lines_set_marks, 3, 3, 0,
       doc: /* Replace the marks of KIND in BUFFER's line model with MARKS.
KIND is a number from 0 to 63.  Each of MARKS is a vector [POS END LANE
CLASS PRIORITY]: END (or nil) ends the run of text the mark covers; LANE,
CLASS and PRIORITY are numbers from 0 to 255 (nil is 0) that the owner
names.  The model shifts the marks with every edit.  */)
  (Lisp_Object buffer, Lisp_Object kind, Lisp_Object marks)
{
  uint64_t key = nh_lines_key_of (buffer);
  CHECK_FIXNAT (kind);
  ptrdiff_t n = list_length (marks), i = 0;
  USE_SAFE_ALLOCA;
  hlid_mark *v;
  SAFE_NALLOCA (v, 1, max (n, 1));
  for (Lisp_Object l = marks; CONSP (l); l = XCDR (l))
    {
      Lisp_Object m = XCAR (l);
      CHECK_VECTOR (m);
      if (ASIZE (m) < 1)
	xsignal1 (Qargs_out_of_range, m);
      Lisp_Object get[5];
      for (int k = 0; k < 5; k++)
	get[k] = k < ASIZE (m) ? AREF (m, k) : Qnil;
      v[i++] = (hlid_mark) {
	.pos = nh_lines_pos (get[0]),
	.end = NILP (get[1]) ? 0 : nh_lines_pos (get[1]),
	.lane = nh_lines_byte (get[2]),
	.class_ = nh_lines_byte (get[3]),
	.prio = nh_lines_byte (get[4]),
      };
    }
  hlid_set_marks (key, XFIXNAT (kind), v, i);
  SAFE_FREE ();
  return Qnil;
}

static uint64_t
nh_lines_kinds (Lisp_Object kinds)
{
  if (NILP (kinds))
    return UINT64_MAX;
  uint64_t mask = 0;
  for (Lisp_Object l = kinds; CONSP (l); l = XCDR (l))
    {
      CHECK_FIXNAT (XCAR (l));
      mask |= UINT64_C (1) << (XFIXNAT (XCAR (l)) & 63);
    }
  return mask;
}

DEFUN ("nidhogg-lines-next-mark", Fnidhogg_lines_next_mark,
       Snidhogg_lines_next_mark, 2, 4, 0,
       doc: /* The position of the first mark after POS in BUFFER's line model.
With DIR negative, the last mark before POS instead.  KINDS is a list of
the kinds to consider; nil means all.  nil when there is none.  */)
  (Lisp_Object buffer, Lisp_Object pos, Lisp_Object kinds, Lisp_Object dir)
{
  uint64_t key = nh_lines_key_of (buffer), out;
  int d = FIXNUMP (dir) && XFIXNUM (dir) < 0 ? -1 : 1;
  return hlid_next_mark (key, nh_lines_pos (pos), nh_lines_kinds (kinds), d,
			 &out) == HLID_OK ? make_uint (out) : Qnil;
}

DEFUN ("nidhogg-lines-counts", Fnidhogg_lines_counts,
       Snidhogg_lines_counts, 3, 3, 0,
       doc: /* The marks of each kind at positions from BEG to before END in BUFFER's model.
A vector of 64 counts, indexed by kind.  */)
  (Lisp_Object buffer, Lisp_Object beg, Lisp_Object end)
{
  uint64_t key = nh_lines_key_of (buffer);
  uint32_t c[64];
  hlid_counts (key, nh_lines_pos (beg), nh_lines_pos (end), c);
  Lisp_Object v = make_nil_vector (64);
  for (int i = 0; i < 64; i++)
    ASET (v, i, make_fixnum (c[i]));
  return v;
}

DEFUN ("nidhogg-lines-set-folds", Fnidhogg_lines_set_folds,
       Snidhogg_lines_set_folds, 3, 4, 0,
       doc: /* Replace SOURCE's fold ranges in BUFFER's line model with FOLDS.
SOURCE is a number from 0 to 255 the owner names (the tree-sitter
parser, the hub, headings, indentation...).  Each of FOLDS is a list
\(BEG END KIND), KIND a number from 0 to 255.  At most MAX ranges are kept
over all sources (the smallest go first); nil means 20000.  The model
sets each range's depth and shifts the ranges with every edit.  */)
  (Lisp_Object buffer, Lisp_Object source, Lisp_Object folds, Lisp_Object max)
{
  uint64_t key = nh_lines_key_of (buffer);
  CHECK_FIXNAT (source);
  ptrdiff_t n = list_length (folds), i = 0;
  USE_SAFE_ALLOCA;
  hlid_fold *v;
  SAFE_NALLOCA (v, 1, max (n, 1));
  for (Lisp_Object l = folds; CONSP (l); l = XCDR (l))
    {
      Lisp_Object f = XCAR (l);
      v[i++] = (hlid_fold) {
	.beg = nh_lines_pos (Fcar (f)),
	.end = nh_lines_pos (Fcar (Fcdr (f))),
	.kind = nh_lines_byte (Fcar (Fcdr (Fcdr (f)))),
      };
    }
  hlid_set_folds (key, nh_lines_byte (source), v, i,
		  FIXNATP (max) ? XFIXNAT (max) : 20000);
  SAFE_FREE ();
  return Qnil;
}

static Lisp_Object
nh_lines_fold_list (hlid_fold *f)
{
  return list5 (make_uint (f->beg), make_uint (f->end), make_fixnum (f->kind),
		make_fixnum (f->depth), make_fixnum (f->source));
}

DEFUN ("nidhogg-lines-folds", Fnidhogg_lines_folds, Snidhogg_lines_folds,
       1, 3, 0,
       doc: /* The fold ranges of BUFFER's line model that overlap BEG..END.
nil BEG and END mean the whole text.  Each is (BEG END KIND DEPTH
SOURCE), in order of BEG, an outer range before the ranges it holds.  */)
  (Lisp_Object buffer, Lisp_Object beg, Lisp_Object end)
{
  uint64_t key = nh_lines_key_of (buffer);
  uint64_t b = NILP (beg) ? 0 : nh_lines_pos (beg);
  uint64_t e = NILP (end) ? UINT64_MAX : nh_lines_pos (end);
  size_t n = 0;
  hlid_folds (key, b, e, NULL, 0, &n);
  USE_SAFE_ALLOCA;
  hlid_fold *v;
  SAFE_NALLOCA (v, 1, max (n, 1));
  hlid_folds (key, b, e, v, n, &n);
  Lisp_Object out = Qnil;
  for (size_t i = n; i > 0; i--)
    out = Fcons (nh_lines_fold_list (&v[i - 1]), out);
  SAFE_FREE ();
  return out;
}

DEFUN ("nidhogg-lines-fold-at", Fnidhogg_lines_fold_at,
       Snidhogg_lines_fold_at, 2, 2, 0,
       doc: /* The fold range to toggle at POS in BUFFER: (BEG END KIND DEPTH SOURCE), or nil.
The largest range that starts on POS's line, else the innermost one
that holds POS.  */)
  (Lisp_Object buffer, Lisp_Object pos)
{
  uint64_t key = nh_lines_key_of (buffer);
  hlid_fold f;
  return hlid_fold_at (key, nh_lines_pos (pos), &f) == HLID_OK
	 ? nh_lines_fold_list (&f) : Qnil;
}

DEFUN ("nidhogg-lines-set-hidden", Fnidhogg_lines_set_hidden,
       Snidhogg_lines_set_hidden, 2, 2, 0,
       doc: /* Tell BUFFER's line model which ranges are hidden: RANGES, a list of (BEG . END) or (BEG END NAME).
A line whose start lies after BEG and at or before END is hidden: the
scroll bar and the minimap leave it out.  NAME, a string or nil, is a
named fold's name: the minimap's fold pill on the range's first line
shows it instead of the count of lines hidden.  */)
  (Lisp_Object buffer, Lisp_Object ranges)
{
  uint64_t key = nh_lines_key_of (buffer);
  ptrdiff_t n = list_length (ranges), i = 0;
  USE_SAFE_ALLOCA;
  uint64_t *v;
  const uint8_t **names;
  size_t *lens;
  SAFE_NALLOCA (v, 2, max (n, 1));
  SAFE_NALLOCA (names, 1, max (n, 1));
  SAFE_NALLOCA (lens, 1, max (n, 1));
  /* The names encoded first: their data must not move before the call.  */
  Lisp_Object encoded = Qnil;
  for (Lisp_Object l = ranges; CONSP (l); l = XCDR (l))
    {
      Lisp_Object r = XCAR (l);
      CHECK_CONS (r);
      Lisp_Object name = Qnil;
      if (CONSP (XCDR (r)))
	{
	  v[i++] = nh_lines_pos (XCAR (r));
	  v[i++] = nh_lines_pos (XCAR (XCDR (r)));
	  name = CONSP (XCDR (XCDR (r))) ? XCAR (XCDR (XCDR (r))) : Qnil;
	  if (!NILP (name))
	    CHECK_STRING (name);
	}
      else
	{
	  v[i++] = nh_lines_pos (XCAR (r));
	  v[i++] = nh_lines_pos (XCDR (r));
	}
      encoded = Fcons (NILP (name) ? Qnil : ENCODE_UTF_8 (name), encoded);
    }
  encoded = Fnreverse (encoded);
  ptrdiff_t k = 0;
  for (Lisp_Object e = encoded; CONSP (e); e = XCDR (e), k++)
    {
      names[k] = STRINGP (XCAR (e)) ? SDATA (XCAR (e)) : NULL;
      lens[k] = STRINGP (XCAR (e)) ? SBYTES (XCAR (e)) : 0;
    }
  hlid_set_hidden_named (key, v, names, lens, i / 2);
  SAFE_FREE ();
  return Qnil;
}

DEFUN ("nidhogg-lines-trail-note", Fnidhogg_lines_trail_note,
       Snidhogg_lines_trail_note, 2, 2, 0,
       doc: /* Note a visit to POS on BUFFER's trail (the last 64 visited lines).  */)
  (Lisp_Object buffer, Lisp_Object pos)
{
  hlid_trail_note (nh_lines_key_of (buffer), nh_lines_pos (pos));
  return Qnil;
}

DEFUN ("nidhogg-lines-trail", Fnidhogg_lines_trail, Snidhogg_lines_trail,
       1, 1, 0,
       doc: /* BUFFER's trail: the positions visited, newest first, one per line.  */)
  (Lisp_Object buffer)
{
  uint64_t key = nh_lines_key_of (buffer), v[64];
  size_t n = 0;
  hlid_trail (key, v, 64, &n);
  Lisp_Object out = Qnil;
  for (size_t i = min (n, 64); i > 0; i--)
    out = Fcons (make_uint (v[i - 1]), out);
  return out;
}

DEFUN ("nidhogg-lines-trail-clear", Fnidhogg_lines_trail_clear,
       Snidhogg_lines_trail_clear, 1, 1, 0,
       doc: /* Forget BUFFER's trail.  */)
  (Lisp_Object buffer)
{
  hlid_trail_clear (nh_lines_key_of (buffer));
  return Qnil;
}

/* Step B's search (model KEY) for up to 4 ms: its answer as
   `nidhogg-lines-search-step' gives it.  New hits redisplay B.  */
static Lisp_Object
nh_lines_search_answer (struct buffer *b, uint64_t key)
{
  ptrdiff_t gpt = BUF_GPT_BYTE (b), z = BUF_Z_BYTE (b);
  const unsigned char *p1 = BUF_BEG_ADDR (b);
  size_t n1 = gpt - BEG_BYTE;
  const unsigned char *p2 = BUF_GAP_END_ADDR (b);
  size_t n2 = z - gpt;
  int32_t state = 0;
  uint64_t count = 0;
  if (hlid_search_step (key, n1 ? p1 : NULL, n1, n2 ? p2 : NULL, n2,
			4 * 1000 * 1000, &state, &count) != HLID_OK
      || state == 0)
    return Qnil;
  if (count > 0 || state == 2)
    bset_redisplay (b);
  return (state == 1 ? Qrunning : state == 3 ? Qfallback
	  : make_uint (count));
}

DEFUN ("nidhogg-lines-search", Fnidhogg_lines_search,
       Snidhogg_lines_search, 2, 4, 0,
       doc: /* Search BUFFER's whole text for QUERY, for the minimap (design X1).
QUERY is an Emacs regexp, or with LITERAL plain text; CASE-FOLD folds
case (ASCII only: a query with other letters is not taken).  The scan
runs in slices of 4 ms: this call makes the first, and
`nidhogg-lines-search-step' the next ones.  The answer is that of
`nidhogg-lines-search-step', or nil when the query is outside the subset
the line model carries (syntax classes such as \\w or \\_<, back
references, a negated bracket, anything that matches a newline): Lisp
searches it itself.  QUERY nil ends the search.  Empty matches are not
hits (`how-many' counts them).  An edit starts the scan over.  */)
  (Lisp_Object buffer, Lisp_Object query, Lisp_Object literal,
   Lisp_Object case_fold)
{
  struct buffer *b = nh_lines_buffer (buffer);
  uint64_t key = nh_lines_key_of (buffer);
  if (NILP (query))
    {
      hlid_search (key, NULL, 0, 0);
      bset_redisplay (b);
      return Qnil;
    }
  CHECK_STRING (query);
  uint32_t flags = (NILP (literal) ? 0 : 1) | (NILP (case_fold) ? 0 : 2);
  if (hlid_search (key, SDATA (query), SBYTES (query), flags) != HLID_OK)
    return Qnil;
  return nh_lines_search_answer (b, key);
}

DEFUN ("nidhogg-lines-search-step", Fnidhogg_lines_search_step,
       Snidhogg_lines_search_step, 1, 1, 0,
       doc: /* Scan on for BUFFER's search for up to 4 ms.
Return `running' while there is more to scan, the number of hits when
the scan is done, `fallback' when the text holds what the line model
cannot read (Lisp searches it), or nil with no search.  */)
  (Lisp_Object buffer)
{
  return nh_lines_search_answer (nh_lines_buffer (buffer),
				 nh_lines_key_of (buffer));
}

DEFUN ("nidhogg-lines-search-hits", Fnidhogg_lines_search_hits,
       Snidhogg_lines_search_hits, 1, 3, 0,
       doc: /* BUFFER's search hits starting from BEG to before END, as (POS . END).
BEG and END default to the text's ends; at most 65536 are given.  */)
  (Lisp_Object buffer, Lisp_Object beg, Lisp_Object end)
{
  uint64_t key = nh_lines_key_of (buffer);
  uint64_t a = NILP (beg) ? 0 : nh_lines_pos (beg);
  uint64_t z = NILP (end) ? UINT64_MAX : nh_lines_pos (end);
  size_t n = 0;
  hlid_search_hits (key, a, z, NULL, 0, &n);
  n = min (n, 65536);
  uint64_t *v = xmalloc (max (n, 1) * 2 * sizeof *v);
  size_t got = 0;
  hlid_search_hits (key, a, z, v, n, &got);
  Lisp_Object out = Qnil;
  for (size_t i = min (n, got); i > 0; i--)
    out = Fcons (Fcons (make_uint (v[2 * i - 2]), make_uint (v[2 * i - 1])),
		 out);
  xfree (v);
  return out;
}

DEFUN ("nidhogg-lines-search-current", Fnidhogg_lines_search_current,
       Snidhogg_lines_search_current, 2, 2, 0,
       doc: /* Make the search hit at POS in BUFFER the current one (nil: none).
The minimap draws it with a ring.  Return how many hits start before
POS (so the hit at POS is that plus one of the count).  */)
  (Lisp_Object buffer, Lisp_Object pos)
{
  uint64_t key = nh_lines_key_of (buffer), index = 0;
  hlid_search_current (key, NILP (pos) ? 0 : nh_lines_pos (pos), &index);
  bset_redisplay (nh_lines_buffer (buffer));
  return make_uint (index);
}

DEFUN ("nidhogg-lines-set-section-regexp", Fnidhogg_lines_set_section_regexp,
       Snidhogg_lines_set_section_regexp, 2, 2, 0,
       doc: /* Find MARK sections in BUFFER's text with REGEXP, nil for none.
REGEXP is in Rust's regex syntax, not Emacs's, matched against each line
(the minimap draws a section's name: the group named `name', else the
first group, else the whole match, trimmed).  The model's lines are
built again to find them, and kept with every edit after.  BUFFER's text
must have a model (`nidhogg-lines-enable').  Signals `invalid-regexp'
when REGEXP does not compile; the expression before it stays.  */)
  (Lisp_Object buffer, Lisp_Object regexp)
{
  struct buffer *b = nh_lines_buffer (buffer);
  struct nh_lines_text *t = nh_lines_find (b->text);
  if (!t)
    error ("No line model for this buffer (nidhogg-lines-enable)");
  Lisp_Object re = Qnil;
  if (!NILP (regexp))
    {
      CHECK_STRING (regexp);
      re = ENCODE_UTF_8 (regexp);
    }
  if (hlid_set_sections (nh_lines_key (b->text),
			 NILP (re) ? NULL : SDATA (re),
			 NILP (re) ? 0 : SBYTES (re)) != HLID_OK)
    xsignal1 (Qinvalid_regexp, regexp);
  nh_lines_build (b, t, true);
  return Qnil;
}

DEFUN ("nidhogg-lines-set-fold-sources", Fnidhogg_lines_set_fold_sources,
       Snidhogg_lines_set_fold_sources, 3, 3, 0,
       doc: /* Which fold ranges BUFFER's line model finds itself.
INDENT non-nil: by indentation, from a line to the last following line
indented deeper (source 4, kind 0).  HEADING-REGEXP, a string in Rust's
regex syntax or nil for none: by headings, each line it matches running
to the line before the next heading of its level or above (source 2,
kind 8); its first group is the heading's marks, their length the level
\(Markdown: "^(#+)\\s").  Each range runs from its first line's start to
its last line's end, so that line stays shown when it is folded.  The
ranges come with `nidhogg-lines-folds' beside the owner's, found again
after each change.  The model's lines are built again to find the
headings.  BUFFER's text must have a model (`nidhogg-lines-enable').
Signals `invalid-regexp' when HEADING-REGEXP does not compile; nothing
changes then.  */)
  (Lisp_Object buffer, Lisp_Object indent, Lisp_Object heading_regexp)
{
  struct buffer *b = nh_lines_buffer (buffer);
  struct nh_lines_text *t = nh_lines_find (b->text);
  if (!t)
    error ("No line model for this buffer (nidhogg-lines-enable)");
  Lisp_Object re = Qnil;
  if (!NILP (heading_regexp))
    {
      CHECK_STRING (heading_regexp);
      re = ENCODE_UTF_8 (heading_regexp);
    }
  if (hlid_set_derived_folds (nh_lines_key (b->text), !NILP (indent),
			      NILP (re) ? NULL : SDATA (re),
			      NILP (re) ? 0 : SBYTES (re)) != HLID_OK)
    xsignal1 (Qinvalid_regexp, heading_regexp);
  if (!NILP (re))
    nh_lines_build (b, t, true);
  return Qnil;
}

DEFUN ("nidhogg-lines-sections", Fnidhogg_lines_sections,
       Snidhogg_lines_sections, 0, 3, 0,
       doc: /* The MARK sections of BUFFER's text, as (POS . NAME) in order.
POS is the start of the section's line; only lines starting in BEG..END
count, by default the whole text.  nil while the lines are being built or
with no expression (`nidhogg-lines-set-section-regexp').  */)
  (Lisp_Object buffer, Lisp_Object beg, Lisp_Object end)
{
  uint64_t key = nh_lines_key_of (buffer);
  uint64_t b = NILP (beg) ? 0 : nh_lines_pos (beg);
  uint64_t e = NILP (end) ? UINT64_MAX : nh_lines_pos (end);
  Lisp_Object out = Qnil;
  uint64_t pos;
  const uint8_t *name;
  size_t len;
  for (size_t i = 0;
       hlid_section (key, b, e, i, &pos, &name, &len) == HLID_OK; i++)
    out = Fcons (Fcons (make_uint (pos),
			make_string_from_utf8 ((const char *) name, len)),
		 out);
  return Fnreverse (out);
}

DEFUN ("nidhogg-lines-describe", Fnidhogg_lines_describe,
       Snidhogg_lines_describe, 0, 1, 0,
       doc: /* The line model of BUFFER's text as a plist, or nil when it has none.
:lines and :chars (nil while the lines are being built), :chunks, the
line index's chunks; :marks, :folds and :hidden, the marks, fold ranges
and hidden ranges held; :bytes, the memory they take; :edits, the
changes applied; :state, `ready', `building' or `stale'; :rebuilds, the
builds made for every text since Emacs started.  */)
  (Lisp_Object buffer)
{
  struct buffer *b = nh_lines_buffer (buffer);
  uint64_t key = nh_lines_model (b);
  hlid_stats s;
  if (!key || hlid_describe (key, &s) != HLID_OK)
    return Qnil;
  bool ready = s.state == 0;
  const char *state = (s.state == 0 ? "ready"
		       : s.state == 1 ? "building" : "stale");
  return list (intern_c_string (":lines"), ready ? make_uint (s.lines) : Qnil,
	       intern_c_string (":chars"), ready ? make_uint (s.chars) : Qnil,
	       intern_c_string (":chunks"), make_uint (s.chunks),
	       intern_c_string (":marks"), make_uint (s.marks),
	       intern_c_string (":folds"), make_uint (s.folds),
	       intern_c_string (":hidden"), make_uint (s.hidden),
	       intern_c_string (":bytes"), make_uint (s.heap_bytes),
	       intern_c_string (":edits"), make_uint (s.edits),
	       intern_c_string (":state"), intern_c_string (state),
	       intern_c_string (":rebuilds"), make_uint (nh_lines_rebuilds));
}

void
syms_of_nidhogg_lines (void)
{
  DEFVAR_LISP ("nidhogg-lines-sync-limit", Vnidhogg_lines_sync_limit,
	       doc: /* A text of this many bytes or more builds its line model on a thread of its own.
nil builds every model at once, on the main thread.  */);
  Vnidhogg_lines_sync_limit = make_fixnum (1024 * 1024);

  defsubr (&Snidhogg_lines_enable);
  defsubr (&Snidhogg_lines_widest);
  nh_lines_widths_of = Qnil;
  staticpro (&nh_lines_widths_of);
  defsubr (&Snidhogg_lines_disable);
  defsubr (&Snidhogg_lines_line);
  defsubr (&Snidhogg_lines_classes);
  defsubr (&Snidhogg_lines_take_hub_folds);
  defsubr (&Snidhogg_lines_take_faces);
  defsubr (&Snidhogg_lines_line_start);
  defsubr (&Snidhogg_lines_fraction);
  defsubr (&Snidhogg_lines_set_marks);
  defsubr (&Snidhogg_lines_next_mark);
  defsubr (&Snidhogg_lines_counts);
  defsubr (&Snidhogg_lines_set_folds);
  defsubr (&Snidhogg_lines_folds);
  defsubr (&Snidhogg_lines_fold_at);
  defsubr (&Snidhogg_lines_set_hidden);
  defsubr (&Snidhogg_lines_trail_note);
  defsubr (&Snidhogg_lines_trail);
  defsubr (&Snidhogg_lines_trail_clear);
  defsubr (&Snidhogg_lines_search);
  defsubr (&Snidhogg_lines_search_step);
  defsubr (&Snidhogg_lines_search_hits);
  defsubr (&Snidhogg_lines_search_current);
  DEFSYM (Qrunning, "running");
  DEFSYM (Qfallback, "fallback");
  defsubr (&Snidhogg_lines_set_section_regexp);
  defsubr (&Snidhogg_lines_sections);
  defsubr (&Snidhogg_lines_set_fold_sources);
  defsubr (&Snidhogg_lines_describe);
  Fprovide (intern_c_string ("nidhogg-lines"), Qnil);
}

#endif /* HAVE_NIDHOGG_HISTORY */

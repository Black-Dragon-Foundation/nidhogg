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
#include "nidhogg-history.h"

#include <hlidskjalf.h>

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
  Lisp_Object tw = BVAR (b, tab_width);
  bool bg = background && FIXNATP (Vnidhogg_lines_sync_limit)
	    && n1 + n2 >= XFIXNAT (Vnidhogg_lines_sync_limit);
  if (hlid_build (nh_lines_key (b->text),
		  !NILP (BVAR (b, enable_multibyte_characters)),
		  FIXNATP (tw) ? min (XFIXNAT (tw), 1000) : 8,
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
       doc: /* Tell BUFFER's line model which ranges are hidden: RANGES, a list of (BEG . END).
A line whose start lies after BEG and at or before END is hidden: the
scroll bar and the minimap leave it out.  */)
  (Lisp_Object buffer, Lisp_Object ranges)
{
  uint64_t key = nh_lines_key_of (buffer);
  ptrdiff_t n = list_length (ranges), i = 0;
  USE_SAFE_ALLOCA;
  uint64_t *v;
  SAFE_NALLOCA (v, 2, max (n, 1));
  for (Lisp_Object l = ranges; CONSP (l); l = XCDR (l))
    {
      Lisp_Object r = XCAR (l);
      CHECK_CONS (r);
      v[i++] = nh_lines_pos (XCAR (r));
      v[i++] = nh_lines_pos (XCDR (r));
    }
  hlid_set_hidden (key, v, i / 2);
  SAFE_FREE ();
  return Qnil;
}

DEFUN ("nidhogg-lines-trail-note", Fnidhogg_lines_trail_note,
       Snidhogg_lines_trail_note, 2, 2, 0,
       doc: /* Note a visit to POS on BUFFER's trail (the last 20 visited lines).  */)
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
  uint64_t key = nh_lines_key_of (buffer), v[32];
  size_t n = 0;
  hlid_trail (key, v, 32, &n);
  Lisp_Object out = Qnil;
  for (size_t i = min (n, 32); i > 0; i--)
    out = Fcons (make_uint (v[i - 1]), out);
  return out;
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
  defsubr (&Snidhogg_lines_disable);
  defsubr (&Snidhogg_lines_line);
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
  defsubr (&Snidhogg_lines_set_section_regexp);
  defsubr (&Snidhogg_lines_sections);
  defsubr (&Snidhogg_lines_describe);
  Fprovide (intern_c_string ("nidhogg-lines"), Qnil);
}

#endif /* HAVE_NIDHOGG_HISTORY */

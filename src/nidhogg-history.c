/* The edit history's change hook: tracked texts and the Lisp interface.

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

/* Every place a buffer's text changes calls NH_HISTORY_CHANGE (the ten
   places tree-sitter is told of a change: insdel.c, casefiddle.c,
   editfns.c), right after the change and before any Lisp runs.  With
   no text tracked that is one test.  With some tracked, the text is
   looked up in a small table here; a tracked one's change goes to its
   ring in Glitnir as (start, old length, the new text, time, origin).

   The table keeps Z and Z_BYTE as they were after the last change
   recorded: a change whose sizes do not add up to the text's growth
   (a change the hook did not see) marks the ring for a resync rather
   than record a wrong change.  An indirect buffer shares its base's
   text, so the table is keyed by the text and a change is recorded
   once.  */

#include <config.h>

#ifdef HAVE_NIDHOGG_HISTORY

#include <stdlib.h>
#include <time.h>

#include "lisp.h"
#include "buffer.h"
#include "nidhogg-history.h"
#include "nidhogg-testclock.h"

#include <glitnir.h>

struct nh_history_text
{
  struct buffer_text *text;
  /* The text's Z and Z_BYTE after the last change recorded.  */
  ptrdiff_t z, z_byte;
  bool multibyte;
};

int nh_history_ntracked;
static struct nh_history_text *nh_history_texts;
static ptrdiff_t nh_history_size;

/* The origins the rings name: index 0 is nil (the user); the table
   starts again whenever every ring is empty.  */
static Lisp_Object nh_history_origins;
static ptrdiff_t nh_history_norigins = 1;
static ptrdiff_t nh_history_origin_cached;

static uintmax_t nh_history_lookups, nh_history_changes;
static uintmax_t nh_history_dropped, nh_history_hook_ns;

static uint64_t
nh_history_key (struct buffer_text *text)
{
  return (uintptr_t) text;
}

static struct nh_history_text *
nh_history_find (struct buffer_text *text)
{
  for (int i = 0; i < nh_history_ntracked; i++)
    if (nh_history_texts[i].text == text)
      return &nh_history_texts[i];
  return NULL;
}

static void
nh_history_untrack (struct nh_history_text *t)
{
  glitnir_history_track (nh_history_key (t->text), 0);
  *t = nh_history_texts[--nh_history_ntracked];
}

/* ORIGIN's index in the origin table, added when new.  A string is
   compared by its text and kept as a copy; anything else by `eq'.  */
static uint32_t
nh_history_origin_index (Lisp_Object origin)
{
  if (NILP (origin))
    return 0;
  if (!VECTORP (nh_history_origins))
    {
      nh_history_origins = make_nil_vector (16);
      nh_history_norigins = 1;
      nh_history_origin_cached = 0;
    }
  ptrdiff_t c = nh_history_origin_cached;
  if (c > 0 && EQ (AREF (nh_history_origins, c), origin))
    return c;
  for (ptrdiff_t i = 1; i < nh_history_norigins; i++)
    {
      Lisp_Object o = AREF (nh_history_origins, i);
      if (EQ (o, origin)
	  || (STRINGP (o) && STRINGP (origin)
	      && !NILP (Fstring_equal (o, origin))))
	{
	  nh_history_origin_cached = i;
	  return i;
	}
    }
  if (nh_history_norigins == ASIZE (nh_history_origins))
    nh_history_origins = larger_vector (nh_history_origins, 1, -1);
  ASET (nh_history_origins, nh_history_norigins,
	STRINGP (origin) ? Fcopy_sequence (origin) : origin);
  nh_history_origin_cached = nh_history_norigins;
  return nh_history_norigins++;
}

static uint64_t
nh_history_mono_ns (void)
{
  struct timespec t;
  clock_gettime (CLOCK_MONOTONIC, &t);
  return t.tv_sec * UINT64_C (1000000000) + t.tv_nsec;
}

void
nh_history_change (struct buffer *b,
		   ptrdiff_t start, ptrdiff_t start_byte,
		   ptrdiff_t old_end, ptrdiff_t old_end_byte,
		   ptrdiff_t new_end, ptrdiff_t new_end_byte)
{
  nh_history_lookups++;
  struct nh_history_text *t = nh_history_find (b->text);
  if (!t)
    return;
  uint64_t t0 = nh_history_mono_ns ();
  ptrdiff_t z = BUF_Z (b), z_byte = BUF_Z_BYTE (b);
  ptrdiff_t old_len = old_end - start, nchars = new_end - start;
  ptrdiff_t nbytes = new_end_byte - start_byte;
  bool ok = (BEG <= start && 0 <= old_len && 0 <= nchars
	     && old_end <= t->z && new_end <= z
	     && z - t->z == nchars - old_len
	     && z_byte - t->z_byte == nbytes - (old_end_byte - start_byte));
  t->z = z;
  t->z_byte = z_byte;
  uint64_t key = nh_history_key (b->text);
  if (ok)
    {
      /* The new text, in two parts when the gap is inside it.  */
      ptrdiff_t gpt = BUF_GPT_BYTE (b);
      const unsigned char *p1 = BUF_BYTE_ADDRESS (b, start_byte);
      const unsigned char *p2 = NULL;
      size_t n1 = nbytes, n2 = 0;
      if (start_byte < gpt && gpt < new_end_byte)
	{
	  n1 = gpt - start_byte;
	  p2 = BUF_BYTE_ADDRESS (b, gpt);
	  n2 = new_end_byte - gpt;
	}
      struct timespec now = nh_tc_now ();
      GlitnirHistoryHead h = {
	.start = start,
	.old_len = old_len,
	.nchars = nchars,
	.nbytes = nbytes,
	.time = now.tv_sec + now.tv_nsec / 1e9,
	.origin = nh_history_origin_index (Vnidhogg_history_origin),
      };
      ok = glitnir_history_push (key, &h, n1 ? p1 : NULL, n1, p2, n2)
	   == GLITNIR_OK;
    }
  else
    glitnir_history_resync (key);
  if (ok)
    nh_history_changes++;
  else
    nh_history_dropped++;
  nh_history_hook_ns += nh_history_mono_ns () - t0;
}

void
nh_history_resync (struct buffer *b)
{
  struct nh_history_text *t = nh_history_find (b->text);
  if (!t)
    return;
  glitnir_history_resync (nh_history_key (t->text));
  t->z = BUF_Z (b);
  t->z_byte = BUF_Z_BYTE (b);
  t->multibyte = !NILP (BVAR (b, enable_multibyte_characters));
}

void
nh_history_forget (struct buffer *b)
{
  struct nh_history_text *t = nh_history_find (b->text);
  if (t)
    nh_history_untrack (t);
}

static struct buffer *
nh_history_buffer (Lisp_Object buffer)
{
  CHECK_BUFFER (buffer);
  return XBUFFER (buffer);
}

DEFUN ("nidhogg-history-track", Fnidhogg_history_track,
       Snidhogg_history_track, 2, 2, 0,
       doc: /* Record changes to BUFFER's text if ON is non-nil; stop if nil.
Return non-nil while the text is tracked.  An indirect buffer shares its
base buffer's text, so tracking either tracks both, and stopping either
stops both.  Changes are taken with `nidhogg-history-drain'.  */)
  (Lisp_Object buffer, Lisp_Object on)
{
  struct buffer *b = nh_history_buffer (buffer);
  struct nh_history_text *t = nh_history_find (b->text);
  if (NILP (on))
    {
      if (t)
	nh_history_untrack (t);
      return Qnil;
    }
  if (!BUFFER_LIVE_P (b))
    error ("Selecting deleted buffer");
  if (t)
    return Qt;
  if (glitnir_history_track (nh_history_key (b->text), 1) != GLITNIR_OK)
    error ("The edit history cannot track this buffer");
  if (nh_history_ntracked == nh_history_size)
    nh_history_texts = xpalloc (nh_history_texts, &nh_history_size, 1,
				INT_MAX, sizeof *nh_history_texts);
  nh_history_texts[nh_history_ntracked++] = (struct nh_history_text) {
    .text = b->text,
    .z = BUF_Z (b),
    .z_byte = BUF_Z_BYTE (b),
    .multibyte = !NILP (BVAR (b, enable_multibyte_characters)),
  };
  return Qt;
}

DEFUN ("nidhogg-history-drain", Fnidhogg_history_drain,
       Snidhogg_history_drain, 1, 1, 0,
       doc: /* Take the changes recorded in BUFFER's text since the last drain.
Each is (START OLD-LEN INSERTED TIME ORIGIN), oldest first: the
OLD-LEN characters from position START (1-based, ignoring narrowing)
were replaced by the string INSERTED at TIME (seconds, a float), with
`nidhogg-history-origin' bound to ORIGIN.  Applied in order to the text
the buffer had when it was tracked (or at the last `resync'), they give
its text now.  The symbol `resync' instead means the changes since the
last drain were not kept (the text was replaced as a whole, or too much
changed at once): read the whole text again.  nil when BUFFER is not
tracked or nothing changed.  */)
  (Lisp_Object buffer)
{
  struct buffer *b = nh_history_buffer (buffer);
  struct nh_history_text *t
    = BUFFER_LIVE_P (b) ? nh_history_find (b->text) : NULL;
  if (!t)
    return Qnil;
  uint64_t key = nh_history_key (t->text);
  Lisp_Object out = Qnil;
  GlitnirHistoryHead h;
  int32_t r;
  while ((r = glitnir_history_peek (key, &h)) == GLITNIR_OK)
    {
      Lisp_Object s = (t->multibyte
		       ? make_uninit_multibyte_string (h.nchars, h.nbytes)
		       : make_uninit_string (h.nbytes));
      if (glitnir_history_pop (key, SDATA (s), h.nbytes) != GLITNIR_OK)
	emacs_abort ();
      Lisp_Object origin = (0 < h.origin && h.origin < nh_history_norigins
			    ? AREF (nh_history_origins, h.origin) : Qnil);
      out = Fcons (list5 (make_int (h.start), make_int (h.old_len), s,
			  make_float (h.time), origin),
		   out);
    }
  if (r == GLITNIR_HISTORY_RESYNC)
    {
      glitnir_history_clear (key);
      out = Qresync;
    }
  else
    out = Fnreverse (out);
  uint64_t pending = 0;
  glitnir_history_pending (&pending);
  if (!pending && VECTORP (nh_history_origins))
    {
      for (ptrdiff_t i = 1; i < nh_history_norigins; i++)
	ASET (nh_history_origins, i, Qnil);
      nh_history_norigins = 1;
      nh_history_origin_cached = 0;
    }
  return out;
}

DEFUN ("nidhogg-history-pending-p", Fnidhogg_history_pending_p,
       Snidhogg_history_pending_p, 0, 0, 0,
       doc: /* Return non-nil if a tracked text has changes or a `resync' to drain.  */)
  (void)
{
  uint64_t n = 0;
  if (nh_history_ntracked)
    glitnir_history_pending (&n);
  return n ? Qt : Qnil;
}

DEFUN ("nidhogg-history-stats", Fnidhogg_history_stats,
       Snidhogg_history_stats, 0, 0, 0,
       doc: /* Return the change hook's counters as a plist.
:tracked, the texts tracked; :lookups, how often a change site looked
for its text (only while some text is tracked); :changes, the changes
recorded; :dropped, those not kept (a ring marked for a resync);
:hook-ns, the time spent recording them, in nanoseconds; :pending, the
texts with something to drain.  */)
  (void)
{
  uint64_t pending = 0;
  glitnir_history_pending (&pending);
  return list (intern_c_string (":tracked"), make_int (nh_history_ntracked),
	       intern_c_string (":lookups"), make_uint (nh_history_lookups),
	       intern_c_string (":changes"), make_uint (nh_history_changes),
	       intern_c_string (":dropped"), make_uint (nh_history_dropped),
	       intern_c_string (":hook-ns"), make_uint (nh_history_hook_ns),
	       intern_c_string (":pending"), make_uint (pending));
}

void
syms_of_nidhogg_history (void)
{
  DEFSYM (Qresync, "resync");
  staticpro (&nh_history_origins);
  nh_history_origins = Qnil;

  DEFVAR_LISP ("nidhogg-history-origin", Vnidhogg_history_origin,
	       doc: /* Who is changing tracked buffer text: nil is the user.
Bind it around a change made on someone else's behalf (an agent's
write, a merge, a revert, a reload from disk); each change recorded
for `nidhogg-history-drain' carries the value it had then.  A string
or a symbol.  */);
  Vnidhogg_history_origin = Qnil;

  defsubr (&Snidhogg_history_track);
  defsubr (&Snidhogg_history_drain);
  defsubr (&Snidhogg_history_pending_p);
  defsubr (&Snidhogg_history_stats);
  Fprovide (intern_c_string ("nidhogg-history"), Qnil);
}

#endif /* HAVE_NIDHOGG_HISTORY */

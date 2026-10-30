/* Fuzz target `framers' (Task 24): nh-io's framers.  Lisp-free.

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

/* Built by nidhogg/test/workers-harness.sh --fuzz with -DHAVE_NIDHOGG_WORKERS
   -DNH_WK_HARNESS -I src, linked with nidhogg-workers.c and
   nidhogg-decode.c (libFuzzer, or fuzz/driver.c).  It includes
   nidhogg-io.c for its static framers and nidhogg-json-tape.c for the
   tape's layout.  Every input is framed by each
   framing (the first byte picks the options): read whole, and read in
   pieces cut where a generator seeded by the input says.  Both reads
   give the same messages (wire bytes, message bytes, tape or decode),
   the same rest and the same outcome, and the messages' wire bytes and
   the rest are the input, in order.  */

#include "nidhogg-json-tape.c"
#include "nidhogg-io.c"

#include <assert.h>

struct run
{
  struct nh_io_framer f;
  struct nh_io_msg *msgs;
  int rc;
};

/* Frame DATA[0, SIZE) cut at the ends in CUTS (NCUTS of them, rising),
   then the end of the stream, into R.  */
static void
frame (struct run *r, const struct nh_io_fconf *c, const uint8_t *data,
       size_t size, const size_t *cuts, int ncuts)
{
  *r = (struct run) { .f = { .c = *c } };
  struct nh_io_msg **tail = &r->msgs;
  size_t at = 0;
  for (int i = 0; i <= ncuts; i++)
    {
      size_t end = i < ncuts ? cuts[i] : size;
      bool ok = nh_io_framer_add (&r->f, data + at, end - at);
      assert (ok);
      at = end;
      if (r->rc == 0)
	r->rc = nh_io_frame (&r->f, false, NULL, &tail);
    }
  if (r->rc == 0)
    r->rc = nh_io_frame (&r->f, true, NULL, &tail);
  assert (r->rc == 0 || r->rc == -1);
}

static void
check_same (const struct run *a, const struct run *b, const uint8_t *data,
	    size_t size)
{
  assert (a->rc == b->rc);
  size_t at = 0;
  const struct nh_io_msg *m = a->msgs, *n = b->msgs;
  for (; m && n; m = m->next, n = n->next)
    {
      assert (m->len == n->len && m->raw_off == n->raw_off
	      && m->raw_len == n->raw_len);
      assert (m->raw_off + m->raw_len <= m->len);
      assert (at + m->len <= size);
      assert (memcmp (m->wire, data + at, m->len) == 0);
      assert (memcmp (n->wire, data + at, n->len) == 0);
      at += m->len;
      assert (!m->tape == !n->tape);
      if (m->tape)
	assert (m->tape->err == n->tape->err && m->tape->n_ent == n->tape->n_ent
		&& m->tape->arena_len == n->tape->arena_len);
      assert (!m->dec == !n->dec);
      if (m->dec)
	assert (m->dlen == n->dlen && m->nchars == n->nchars
		&& memcmp (m->dec, n->dec, m->dlen) == 0);
    }
  assert (!m && !n);
  size_t rest = a->f.len - a->f.start;
  assert (rest == b->f.len - b->f.start);
  assert (at + rest == size);
  if (rest)
    {
      assert (memcmp (a->f.buf + a->f.start, data + at, rest) == 0);
      assert (memcmp (b->f.buf + b->f.start, data + at, rest) == 0);
    }
}

static void
run_free (struct run *r)
{
  free (r->f.buf);
  nh_io_msgs_free (r->msgs);
}

int
LLVMFuzzerTestOneInput (const uint8_t *data, size_t size)
{
  if (size == 0)
    return 0;
  unsigned cfg = data[0];
  /* The cuts: a generator seeded by the input.  */
  uint64_t rng = 0x9e3779b97f4a7c15u ^ size;
  for (size_t i = 0; i < size && i < 64; i++)
    rng = (rng ^ data[i]) * 0x100000001b3u;
  enum { MAXCUTS = 64 };
  size_t cuts[MAXCUTS];
  int ncuts = 0;
  for (size_t at = 0; ncuts < MAXCUTS; )
    {
      rng ^= rng << 13;
      rng ^= rng >> 7;
      rng ^= rng << 17;
      /* Mostly small pieces, some of one byte.  */
      at += 1 + rng % (rng & 0x100 ? 3 : 97);
      if (at >= size)
	break;
      cuts[ncuts++] = at;
    }

  for (int kind = NH_IO_FR_CL; kind <= NH_IO_FR_RECORDS; kind++)
    {
      struct nh_io_fconf c = {
	.kind = kind,
	.parse = cfg & 1 ? NH_JSON_MODE_BUFFER : NH_JSON_MODE_STRING,
	.sep = cfg & 2 ? '\n' : 0,
	.sys = cfg & 4 ? NH_DEC_UTF8_UNIX : cfg & 8 ? NH_DEC_RAW_DOS
	: NH_DEC_NONE,
	.max = cfg & 16 ? 40 : (size_t) 1 << 20,
      };
      if (kind != NH_IO_FR_RECORDS)
	c.sys = NH_DEC_NONE;
      struct run whole, cut;
      frame (&whole, &c, data, size, NULL, 0);
      frame (&cut, &c, data, size, cuts, ncuts);
      check_same (&whole, &cut, data, size);
      run_free (&whole);
      run_free (&cut);
    }
  return 0;
}

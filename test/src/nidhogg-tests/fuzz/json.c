/* libFuzzer target `json' (Task 22): the tokenizer in both modes.

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

/* Built with -DHAVE_NIDHOGG_WORKERS -DNH_WK_HARNESS -I src and
   -fsanitize=fuzzer,address,undefined.  It includes the tokenizer's
   file so that it can walk the tape; every tape that comes back must be
   well formed: containers balanced with the counts they claim, strings
   and keys inside the arena, key indices inside the key table.  */

#include "nidhogg-json-tape.c"

#include <assert.h>

static size_t
check_value (const struct nh_tape *t, size_t i)
{
  assert (i < t->n_ent);
  const struct nh_json_ent *e = &t->ents[i++];
  switch (e->type)
    {
    case NH_JSON_T_STRING:
      assert ((size_t) e->v.s.c + e->a <= t->arena_len);
      assert (e->v.s.b <= e->a);
      break;
    case NH_JSON_T_BIGNUM:
      assert ((size_t) e->v.s.c + e->a + 1 <= t->arena_len);
      assert (t->arena[e->v.s.c + e->a] == 0);
      break;
    case NH_JSON_T_INTEGER:
    case NH_JSON_T_FLOAT:
    case NH_JSON_T_TRUE:
    case NH_JSON_T_FALSE:
    case NH_JSON_T_NULL:
      break;
    case NH_JSON_T_ARRAY:
      for (uint32_t k = 0; k < e->a; k++)
	i = check_value (t, i);
      assert (i < t->n_ent && t->ents[i++].type == NH_JSON_T_END);
      break;
    case NH_JSON_T_OBJECT:
      for (uint32_t k = 0; k < e->a; k++)
	{
	  assert (i < t->n_ent && t->ents[i].type == NH_JSON_T_KEY);
	  const struct nh_json_ent *key = &t->ents[i++];
	  assert (key->v.s.c < t->n_keys);
	  const struct nh_json_key *kt = &t->keys[key->v.s.c];
	  assert (kt->len == key->a && kt->nchars == key->v.s.b);
	  assert ((size_t) kt->off + kt->len <= t->arena_len);
	  i = check_value (t, i);
	}
      assert (i < t->n_ent && t->ents[i++].type == NH_JSON_T_END);
      break;
    default:
      assert (0);
    }
  return i;
}

int
LLVMFuzzerTestOneInput (const uint8_t *data, size_t size)
{
  for (int mode = NH_JSON_MODE_STRING; mode <= NH_JSON_MODE_BUFFER; mode++)
    {
      struct nh_tape *t = nh_json_tokenize (data, size, mode, NULL);
      assert (t);
      if (t->err == NH_JSON_E_NONE)
	{
	  assert (check_value (t, 0) == t->n_ent);
	  assert (t->consumed <= size);
	}
      else
	{
	  assert (t->err_off <= size);
	  assert (t->n_ent == 0);
	}
      free (t);
    }
  return 0;
}

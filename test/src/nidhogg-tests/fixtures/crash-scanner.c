/* The `crash' test grammar of workers/worker-crash-forwarded-grammar
   (workers-fatal-tests.el, §17.3 S10).  grammars.sh builds it from
   tree-sitter-python's parser and scanner with the language renamed
   `crash' and the external scanner's scan function wrapped by this one:
   when the scanner is entered with a backquote as the next character
   (never valid Python), it writes through a null pointer, as a broken
   external scanner would.  Otherwise it is Python.

   Copyright (C) 2026 Black Dragon Foundation contributors

   This file is part of Nidhogg.

   Nidhogg is free software: you can redistribute it and/or modify
   it under the terms of the GNU General Public License as published by
   the Free Software Foundation, either version 3 of the License, or
   (at your option) any later version.

   Nidhogg is distributed in the hope that it will be useful,
   but WITHOUT ANY WARRANTY; without even the implied warranty of
   MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
   GNU General Public License for more details.

   You should have received a copy of the GNU General Public License
   along with Nidhogg.  If not, see <https://www.gnu.org/licenses/>.  */

#include <stdbool.h>
#include "tree_sitter/parser.h"

/* tree-sitter-python's scan function (scanner.c, renamed).  */
bool nh_crash_real_scan (void *, TSLexer *, const bool *);

/* The scan function parser.c names (renamed from Python's).  */
bool
nh_crash_scan (void *payload, TSLexer *lexer, const bool *valid)
{
  if (lexer->lookahead == '`')
    *(volatile int *) 0 = 1;
  return nh_crash_real_scan (payload, lexer, valid);
}

/* Lisp interface of Glitnir (the compositor), Nidhogg's Rust static library.

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

#include <config.h>

#ifdef HAVE_GLITNIR

#include "lisp.h"
#include "frame.h"

DEFUN ("glitnir-active-p", Fglitnir_active_p, Sglitnir_active_p, 0, 1, 0,
       doc: /* Return non-nil if Glitnir (the compositor) draws FRAME.
FRAME defaults to the selected frame.  Only graphical frames of the pgtk
build can be drawn by Glitnir; batch and terminal frames never are.  */)
  (Lisp_Object frame)
{
  struct frame *f = decode_live_frame (frame);
#ifdef HAVE_PGTK
  if (!FRAME_PGTK_P (f))
    return Qnil;
#else
  (void) f;
#endif
  /* Not drawn yet: the compositor takes over a root in a later change.  */
  return Qnil;
}

void
syms_of_nidhogg_glitnir (void)
{
  defsubr (&Sglitnir_active_p);

  DEFVAR_BOOL ("glitnir-enable", glitnir_enable,
	       doc: /* Non-nil means Glitnir (the compositor) may draw graphical frames.
Setting it to nil makes Glitnir hand every frame back to the stock drawing.  */);
  glitnir_enable = true;

  Fprovide (intern_c_string ("glitnir"), Qnil);
}

#endif /* HAVE_GLITNIR */

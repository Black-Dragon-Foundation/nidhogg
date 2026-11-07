/* The macOS half of Nidhogg's own shell (Naglfar, HAVE_NHMAC): the two
   generic calls that carry Lisp's platform requests to the shell, and the
   names Lisp sees.  The shared C (nhwlterm.c, nhwlfns.c) does the rest;
   AppKit lives in nidhogg/shell/src/macos, behind the glue.

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

#include "lisp.h"
#include "nhwlterm.h"
#include "frame.h"
#include "keyboard.h"

/* The display a platform request goes to: the selected frame's, else
   the first.  */
static struct nhwl_display_info *
nhmac_display (void)
{
  struct frame *f = SELECTED_FRAME ();
  struct nhwl_display_info *dpyinfo
    = FRAME_NHWL_P (f) ? FRAME_DISPLAY_INFO (f) : x_display_list;

  if (!dpyinfo || !dpyinfo->glue)
    error ("No Naglfar display");
  return dpyinfo;
}

/* ARGS as one readable text, a list: a frame of the shell becomes its
   window's id, which is how the shell knows it.  */
static Lisp_Object
nhmac_args_text (ptrdiff_t nargs, Lisp_Object *args)
{
  Lisp_Object list = Qnil;

  for (ptrdiff_t i = nargs - 1; i >= 0; i--)
    {
      Lisp_Object a = args[i];
      if (FRAMEP (a) && FRAME_NHWL_P (XFRAME (a)))
	a = make_uint (FRAME_NHWL_OUTPUT (XFRAME (a))->window);
      list = Fcons (a, list);
    }
  return Fprin1_to_string (list, Qt, Qnil);
}

DEFUN ("nhmac--call", Fnhmac__call, Snhmac__call, 1, MANY, 0,
       doc: /* Ask the macOS shell to do WHAT, a symbol, with ARGS.
ARGS are passed as one readable list, a frame as its shell window.  An
unknown WHAT is an error.  For nhmac-win.el.
usage: (nhmac--call WHAT &rest ARGS)  */)
  (ptrdiff_t nargs, Lisp_Object *args)
{
  struct nhwl_display_info *dpyinfo = nhmac_display ();
  Lisp_Object text;
  int32_t rc;

  CHECK_SYMBOL (args[0]);
  text = nhmac_args_text (nargs - 1, args + 1);
  rc = NHG (nhg_app_call (dpyinfo->glue, SSDATA (SYMBOL_NAME (args[0])),
			  SSDATA (text)));
  if (nhwl_glue_rc (dpyinfo, rc) < 0)
    error ("nhmac--call %s: %d", SSDATA (SYMBOL_NAME (args[0])), rc);
  return Qnil;
}

DEFUN ("nhmac--query", Fnhmac__query, Snhmac__query, 1, MANY, 0,
       doc: /* Ask the macOS shell WHAT, a symbol, with ARGS; return its answer.
ARGS are passed as `nhmac--call' passes them; the answer is a Lisp form.
A query may run a modal session.  For nhmac-win.el.
usage: (nhmac--query WHAT &rest ARGS)  */)
  (ptrdiff_t nargs, Lisp_Object *args)
{
  struct nhwl_display_info *dpyinfo = nhmac_display ();
  char buf[16384];
  size_t len = 0;
  Lisp_Object text;
  int32_t rc;

  CHECK_SYMBOL (args[0]);
  text = nhmac_args_text (nargs - 1, args + 1);
  rc = NHG (nhg_app_query (dpyinfo->glue, SSDATA (SYMBOL_NAME (args[0])),
			   SSDATA (text), buf, sizeof buf, &len));
  if (nhwl_glue_rc (dpyinfo, rc) < 0)
    error ("nhmac--query %s: %d", SSDATA (SYMBOL_NAME (args[0])), rc);
  if (len >= sizeof buf)
    error ("nhmac--query %s: the answer is too long", SSDATA (SYMBOL_NAME (args[0])));
  return Fcar (Fread_from_string (make_string (buf, len), Qnil, Qnil));
}

/* The menu bar: built at idle by the native menu task; until then the
   frame shows none.  */
void
set_frame_menubar (struct frame *f, bool deep_p)
{
}

void
syms_of_nhmacterm (void)
{
  defsubr (&Snhmac__call);
  defsubr (&Snhmac__query);
  syms_of_nhmacfont ();

  /* `window-system' reads `ns', so ns-win.el and packages take their
     macOS branches; `nidhogg-macos' and `naglfar' name this shell.  */
  Fprovide (Qns, Qnil);
  Fprovide (intern_c_string ("nidhogg-macos"), Qnil);
  Fprovide (intern_c_string ("naglfar"), Qnil);
}

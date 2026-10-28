/* Clipboard and primary selection for Nidhogg's Wayland shell.

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

/* The five selection primitives of pgtk-win.el on top of the shell's
   selection calls.  Text is converted when a selection is owned (the
   shell's reader thread answers other clients' pastes from the copy);
   other targets are not offered.  Reads wait at most
   `nhwl-selection-timeout' seconds.  */

#include <config.h>

#include "lisp.h"
#include "nhwlterm.h"
#include "blockinput.h"
#include "coding.h"
#include "keyboard.h"
#include "termhooks.h"

/* Which selections this process owns, to tell "we lost it" from "we
   gave it up" when the shell reports a change.  */
static bool nhwl_sel_ours[2];

/* The MIME types we offer for text, in the order of preference.  */
static const char *const nhwl_text_mimes[] = {
  "text/plain;charset=utf-8", "UTF8_STRING", "text/plain", "STRING", "TEXT",
};
#define N_TEXT_MIMES ARRAYELTS (nhwl_text_mimes)

/* The shell's selection kind for SEL, or -1 when there is none.  NIL
   means PRIMARY, as in pgtk.  */
static int
nhwl_selection_kind (Lisp_Object sel)
{
  if (NILP (sel) || EQ (sel, QPRIMARY))
    return NHSHELL_SEL_PRIMARY;
  if (EQ (sel, QCLIPBOARD))
    return NHSHELL_SEL_CLIPBOARD;
  return -1;
}

static bool
nhwl_text_target_p (Lisp_Object target)
{
  return (EQ (target, QUTF8_STRING) || EQ (target, QSTRING)
	  || EQ (target, QTEXT) || EQ (target, QCOMPOUND_TEXT)
	  || EQ (target, Qtext_plain) || EQ (target, Qtext_plain_utf8));
}

/* The current MIME types of KIND as a Lisp list of strings.  */
static Lisp_Object
nhwl_offer_types (struct nhwl_display_info *dpyinfo, int kind)
{
  for (int attempt = 0; attempt < 3; attempt++)
    {
      size_t need = 0;
      int n = nhshell_selection_types (dpyinfo->shell, kind, NULL, 0, &need);
      if (n <= 0)
	return Qnil;
      char *buf = xmalloc (need);
      size_t got = 0;
      n = nhshell_selection_types (dpyinfo->shell, kind, buf, need, &got);
      Lisp_Object list = Qnil;
      if (n > 0 && got <= need)
	for (size_t i = 0; i < got; i += strlen (buf + i) + 1)
	  list = Fcons (build_string (buf + i), list);
      xfree (buf);
      if (!NILP (list))
	return Fnreverse (list);
    }
  return Qnil;
}

DEFUN ("pgtk-own-selection-internal", Fpgtk_own_selection_internal,
       Spgtk_own_selection_internal, 2, 3, 0,
       doc: /* Assert an X selection of type SELECTION and value VALUE.
Only strings are offered, as text.  Return VALUE, or nil when the
compositor refuses (no keyboard focus).  */)
  (Lisp_Object selection, Lisp_Object value, Lisp_Object frame)
{
  int kind = nhwl_selection_kind (selection);
  if (kind < 0 || !x_display_list || !STRINGP (value))
    return Qnil;

  Lisp_Object bytes = code_convert_string_norecord (value, Qutf_8, true);
  const uint8_t *data[N_TEXT_MIMES];
  size_t lens[N_TEXT_MIMES];
  for (int i = 0; i < N_TEXT_MIMES; i++)
    {
      data[i] = (const uint8_t *) SDATA (bytes);
      lens[i] = SBYTES (bytes);
    }
  block_input ();
  int rc = nhshell_selection_own (x_display_list->shell, kind,
				  nhwl_text_mimes, data, lens, N_TEXT_MIMES);
  unblock_input ();
  if (rc < 0)
    return Qnil;
  nhwl_sel_ours[kind] = true;
  return value;
}

DEFUN ("pgtk-disown-selection-internal", Fpgtk_disown_selection_internal,
       Spgtk_disown_selection_internal, 1, 2, 0,
       doc: /* If we own the selection SELECTION, disown it.  */)
  (Lisp_Object selection, Lisp_Object terminal)
{
  int kind = nhwl_selection_kind (selection);
  if (kind < 0 || !x_display_list || !nhwl_sel_ours[kind])
    return Qnil;
  nhwl_sel_ours[kind] = false;
  block_input ();
  nhshell_selection_disown (x_display_list->shell, kind);
  unblock_input ();
  return Qt;
}

DEFUN ("pgtk-selection-owner-p", Fpgtk_selection_owner_p,
       Spgtk_selection_owner_p, 0, 2, 0,
       doc: /* Whether the current Emacs process owns the selection SELECTION.  */)
  (Lisp_Object selection, Lisp_Object terminal)
{
  int kind = nhwl_selection_kind (selection);
  if (kind < 0 || !x_display_list)
    return Qnil;
  return nhshell_selection_owned (x_display_list->shell, kind) > 0 ? Qt : Qnil;
}

DEFUN ("pgtk-selection-exists-p", Fpgtk_selection_exists_p,
       Spgtk_selection_exists_p, 0, 2, 0,
       doc: /* Whether there is an owner for the given selection.  */)
  (Lisp_Object selection, Lisp_Object terminal)
{
  int kind = nhwl_selection_kind (selection);
  if (kind < 0 || !x_display_list)
    return Qnil;
  return nhshell_selection_types (x_display_list->shell, kind, NULL, 0, NULL)
	 > 0 ? Qt : Qnil;
}

DEFUN ("pgtk-get-selection-internal", Fpgtk_get_selection_internal,
       Spgtk_get_selection_internal, 2, 3, 0,
       doc: /* Return text selected from some X window.
SELECTION-SYMBOL is CLIPBOARD, PRIMARY or SECONDARY (nil), TARGET-TYPE the
target: TARGETS gives a vector of the offered types, a text target gives
the text, any other the bytes of the type of that name.  Waits at most
`nhwl-selection-timeout' seconds; nil when nothing arrives.  */)
  (Lisp_Object selection_symbol, Lisp_Object target_type,
   Lisp_Object terminal)
{
  int kind = nhwl_selection_kind (selection_symbol);
  if (kind < 0 || !x_display_list)
    return Qnil;
  CHECK_SYMBOL (target_type);

  Lisp_Object types = nhwl_offer_types (x_display_list, kind);
  if (NILP (types))
    return Qnil;

  if (EQ (target_type, QTARGETS))
    {
      ptrdiff_t n = list_length (types), i = 0;
      Lisp_Object vec = make_nil_vector (n);
      for (; CONSP (types); types = XCDR (types), i++)
	ASET (vec, i, Fintern (XCAR (types), Qnil));
      return vec;
    }

  bool text = nhwl_text_target_p (target_type);
  Lisp_Object mime = Qnil;
  Lisp_Object name = SYMBOL_NAME (target_type);
  if (!NILP (Fmember (name, types)))
    mime = name;
  else if (text)
    for (int i = 0; i < N_TEXT_MIMES && NILP (mime); i++)
      {
	Lisp_Object m = build_string (nhwl_text_mimes[i]);
	if (!NILP (Fmember (m, types)))
	  mime = m;
      }
  if (NILP (mime))
    return Qnil;

  double secs = 1.0;
  if (!NILP (Fboundp (Qnhwl_selection_timeout))
      && NUMBERP (Fsymbol_value (Qnhwl_selection_timeout)))
    secs = XFLOATINT (Fsymbol_value (Qnhwl_selection_timeout));
  uint32_t ms = secs <= 0 ? 0 : secs > 60 ? 60000 : (uint32_t) (secs * 1000);

  uint8_t *out = NULL;
  size_t len = 0;
  int rc = nhshell_selection_read (x_display_list->shell, kind,
				   SSDATA (mime), ms, &out, &len);
  if (rc < 0)
    return Qnil;

  Lisp_Object str = make_unibyte_string ((char *) out, len);
  nhshell_free_bytes (out);
  if (text)
    return code_convert_string_norecord (str, Qutf_8, false);
  /* Raw bytes: gui-get-selection decodes them from the property.  */
  Fput_text_property (make_fixnum (0), make_fixnum (len),
		      intern ("foreign-selection"), target_type, str);
  return str;
}

/* The shell says selection KIND changed.  Fill IE and return true when
   it was ours and another client took it.  */
bool
nhwl_selection_changed (struct nhwl_display_info *dpyinfo, uint32_t kind,
			struct input_event *ie)
{
  if (kind > NHSHELL_SEL_PRIMARY || !nhwl_sel_ours[kind]
      || nhshell_selection_owned (dpyinfo->shell, kind) > 0)
    return false;
  nhwl_sel_ours[kind] = false;
  ie->kind = SELECTION_CLEAR_EVENT;
  ie->arg = kind == NHSHELL_SEL_PRIMARY ? QPRIMARY : QCLIPBOARD;
  return true;
}

/* Called by the command loop for a SELECTION_CLEAR_EVENT.  */
void
nhwl_handle_selection_clear (struct input_event *ie)
{
  CALLN (Frun_hook_with_args, Qpgtk_lost_selection_functions, ie->arg);
  redisplay_preserve_echo_area (20);
}

void
syms_of_nhwlselect (void)
{
  DEFSYM (QCLIPBOARD, "CLIPBOARD");
  DEFSYM (Qnhwl_selection_timeout, "nhwl-selection-timeout");
  DEFSYM (Qpgtk_lost_selection_functions, "pgtk-lost-selection-functions");
  DEFSYM (QUTF8_STRING, "UTF8_STRING");
  DEFSYM (QTARGETS, "TARGETS");
  DEFSYM (QTEXT, "TEXT");
  DEFSYM (QSTRING, "STRING");
  DEFSYM (QCOMPOUND_TEXT, "COMPOUND_TEXT");
  DEFSYM (Qtext_plain, "text/plain");
  DEFSYM (Qtext_plain_utf8, "text/plain;charset=utf-8");

  DEFVAR_LISP ("pgtk-lost-selection-functions", Vpgtk_lost_selection_functions,
    doc: /* A list of functions to be called when Emacs loses a selection.
Each is called with the selection (a symbol) as its argument.  */);
  Vpgtk_lost_selection_functions = Qnil;

  defsubr (&Spgtk_own_selection_internal);
  defsubr (&Spgtk_disown_selection_internal);
  defsubr (&Spgtk_selection_owner_p);
  defsubr (&Spgtk_selection_exists_p);
  defsubr (&Spgtk_get_selection_internal);
}

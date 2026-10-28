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
  /* The offer of the drag over our windows (nhwl_dnd_event_arg).  */
  if (EQ (sel, Qnhwl_dnd_selection))
    return NHG_SEL_DND;
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
  const char *buf = NULL;
  size_t got = 0;
  Lisp_Object list = Qnil;

  if (nhwl_glue_rc (dpyinfo, NHG (nhg_selection_types (dpyinfo->glue, kind,
						       &buf, &got))) <= 0)
    return Qnil;
  /* BUF is the glue's, valid until its next call; the list is built
     before any.  */
  for (size_t i = 0; i < got; i += strlen (buf + i) + 1)
    list = Fcons (build_string (buf + i), list);
  return Fnreverse (list);
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
  int rc = nhwl_glue_rc (x_display_list,
			 NHG (nhg_selection_set (x_display_list->glue, kind,
						 nhwl_text_mimes, data, lens,
						 N_TEXT_MIMES)));
  unblock_input ();
  if (rc < 0)
    return Qnil;
  return value;
}

DEFUN ("pgtk-disown-selection-internal", Fpgtk_disown_selection_internal,
       Spgtk_disown_selection_internal, 1, 2, 0,
       doc: /* If we own the selection SELECTION, disown it.  */)
  (Lisp_Object selection, Lisp_Object terminal)
{
  int kind = nhwl_selection_kind (selection);
  if (kind < 0 || !x_display_list)
    return Qnil;
  block_input ();
  int rc = nhwl_glue_rc (x_display_list,
			 NHG (nhg_selection_disown (x_display_list->glue,
						    kind)));
  unblock_input ();
  return rc > 0 ? Qt : Qnil;
}

DEFUN ("pgtk-selection-owner-p", Fpgtk_selection_owner_p,
       Spgtk_selection_owner_p, 0, 2, 0,
       doc: /* Whether the current Emacs process owns the selection SELECTION.  */)
  (Lisp_Object selection, Lisp_Object terminal)
{
  int kind = nhwl_selection_kind (selection);
  if (kind < 0 || !x_display_list)
    return Qnil;
  return nhwl_glue_rc (x_display_list,
		       NHG (nhg_selection_owned (x_display_list->glue, kind)))
	 > 0 ? Qt : Qnil;
}

DEFUN ("pgtk-selection-exists-p", Fpgtk_selection_exists_p,
       Spgtk_selection_exists_p, 0, 2, 0,
       doc: /* Whether there is an owner for the given selection.  */)
  (Lisp_Object selection, Lisp_Object terminal)
{
  int kind = nhwl_selection_kind (selection);
  if (kind < 0 || !x_display_list)
    return Qnil;
  return nhwl_glue_rc (x_display_list,
		       NHG (nhg_selection_types (x_display_list->glue, kind,
						 NULL, NULL)))
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

  const uint8_t *out = NULL;
  size_t len = 0;
  int rc = nhwl_glue_rc (x_display_list,
			 NHG (nhg_selection_read (x_display_list->glue, kind,
						  SSDATA (mime), ms, &out,
						  &len)));
  if (rc < 0)
    return Qnil;

  /* OUT is the glue's, valid until its next call; the string copies it.  */
  Lisp_Object str = make_unibyte_string ((const char *) out, len);
  if (text)
    return code_convert_string_norecord (str, Qutf_8, false);
  /* Raw bytes: gui-get-selection decodes them from the property.  */
  Fput_text_property (make_fixnum (0), make_fixnum (len),
		      intern ("foreign-selection"), target_type, str);
  return str;
}

/* The glue says another client took selection KIND from us (a
   SELECTION_CLEAR action): fill IE with the event.  */
void
nhwl_selection_clear_event (uint32_t kind, struct input_event *ie)
{
  ie->kind = SELECTION_CLEAR_EVENT;
  ie->arg = kind == NHSHELL_SEL_PRIMARY ? QPRIMARY : QCLIPBOARD;
}

/* Called by the command loop for a SELECTION_CLEAR_EVENT.  */
void
nhwl_handle_selection_clear (struct input_event *ie)
{
  CALLN (Frun_hook_with_args, Qpgtk_lost_selection_functions, ie->arg);
  redisplay_preserve_echo_area (20);
}

/* Drag and drop onto our windows, with pgtk-dnd.el's semantics.  The glue
   (dnd.rs) reports the drag; nhwl_apply makes a DRAG_N_DROP_EVENT of each
   action, and pgtk-dnd.el answers with the two functions below.  */

/* The action symbol of the glue's action code CODE (NHG_DND_*).  */
static Lisp_Object
nhwl_dnd_action_symbol (int code)
{
  switch (code)
    {
    case NHG_DND_COPY:
      return Qnhwl_dnd_copy;
    case NHG_DND_MOVE:
      return Qnhwl_dnd_move;
    case NHG_DND_ASK:
      return Qnhwl_dnd_ask;
    default:
      return Qnil;
    }
}

/* The glue's action code of the Lisp action ACT.  Wayland has no link or
   private action: they are copies.  */
static uint32_t
nhwl_dnd_action_code (Lisp_Object act)
{
  if (NILP (act))
    return 0;
  if (EQ (act, Qnhwl_dnd_copy) || EQ (act, Qnhwl_dnd_link)
      || EQ (act, Qnhwl_dnd_private))
    return NHG_DND_COPY;
  if (EQ (act, Qnhwl_dnd_move))
    return NHG_DND_MOVE;
  if (EQ (act, Qnhwl_dnd_ask))
    return NHG_DND_ASK;
  signal_error ("Invalid drag action", act);
}

/* The argument of the DRAG_N_DROP_EVENT for the glue's DND action A: motion
   (and the enter) (lambda XdndSelection TIME ACTION), drop
   (quote XdndSelection TIME), leave nil.  */
Lisp_Object
nhwl_dnd_event_arg (const NhgAction *a)
{
  Lisp_Object time = make_uint (a->time_ms);

  switch (a->kind)
    {
    case NHG_ACT_DND_ENTER:
    case NHG_ACT_DND_MOTION:
      return list4 (Qlambda, Qnhwl_dnd_selection, time,
		    nhwl_dnd_action_symbol (a->code));
    case NHG_ACT_DND_DROP:
      return list3 (Qquote, Qnhwl_dnd_selection, time);
    default:
      return Qnil;
    }
}

DEFUN ("pgtk-register-dnd-targets", Fpgtk_register_dnd_targets,
       Spgtk_register_dnd_targets, 2, 2, 0,
       doc: /* Register TARGETS on FRAME.
TARGETS should be a list of strings describing data types (selection
targets) that can be dropped on top of FRAME.  A drag that offers none
of them is refused.  */)
  (Lisp_Object frame, Lisp_Object targets)
{
  struct frame *f = decode_window_system_frame (frame);
  struct nhwl_output *out = FRAME_NHWL_OUTPUT (f);
  ptrdiff_t n = 0;
  USE_SAFE_ALLOCA;

  CHECK_LIST (targets);
  ptrdiff_t length = list_length (targets);
  const char **mimes;
  SAFE_NALLOCA (mimes, 1, length);
  Lisp_Object tem = targets;
  FOR_EACH_TAIL (tem)
    {
      CHECK_STRING (XCAR (tem));
      SAFE_ALLOCA_STRING (mimes[n], XCAR (tem));
      n++;
    }
  CHECK_LIST_END (tem, targets);

  if (out && out->window && FRAME_NHG (f))
    nhwl_glue_rc (FRAME_DISPLAY_INFO (f),
		  NHG (nhg_dnd_register (FRAME_NHG (f), out->window, mimes,
					 n)));
  SAFE_FREE ();
  return Qnil;
}

DEFUN ("pgtk-drop-finish", Fpgtk_drop_finish, Spgtk_drop_finish, 3, 3, 0,
       doc: /* Finish the drag-n-drop event that happened at TIMESTAMP.
SUCCESS is whether or not the drop was successful, i.e. the action
chosen in the last call to `pgtk-update-drop-status' was performed.
TIMESTAMP is the time associated with the drag-n-drop event that is
being finished.  A call with an older TIMESTAMP than the current drag's
is ignored.  The source is told only when an action was set.  DELETE is
ignored: a move is the source's job on Wayland.  */)
  (Lisp_Object success, Lisp_Object timestamp, Lisp_Object delete)
{
  uint32_t time;

  if (!x_display_list)
    return Qnil;
  CONS_TO_INTEGER (timestamp, uint32_t, time);
  nhwl_glue_rc (x_display_list,
		NHG (nhg_dnd_finish (x_display_list->glue, !NILP (success),
				     time)));
  return Qnil;
}

DEFUN ("pgtk-update-drop-status", Fpgtk_update_drop_status,
       Spgtk_update_drop_status, 2, 2, 0,
       doc: /* Update the status of the current drag-and-drop operation.
ACTION is the action the drop source should take: copy, move, ask, or
nil to accept nothing.  TIMESTAMP is the same as in `pgtk-drop-finish'.  */)
  (Lisp_Object action, Lisp_Object timestamp)
{
  uint32_t time;

  if (!x_display_list)
    return Qnil;
  CONS_TO_INTEGER (timestamp, uint32_t, time);
  nhwl_glue_rc (x_display_list,
		NHG (nhg_dnd_status (x_display_list->glue,
				     nhwl_dnd_action_code (action), time)));
  return Qnil;
}

void
syms_of_nhwlselect (void)
{
  DEFSYM (Qnhwl_dnd_selection, "XdndSelection");
  DEFSYM (Qnhwl_dnd_copy, "copy");
  DEFSYM (Qnhwl_dnd_move, "move");
  DEFSYM (Qnhwl_dnd_ask, "ask");
  DEFSYM (Qnhwl_dnd_link, "link");
  DEFSYM (Qnhwl_dnd_private, "private");
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
  defsubr (&Spgtk_register_dnd_targets);
  defsubr (&Spgtk_drop_finish);
  defsubr (&Spgtk_update_drop_status);
}

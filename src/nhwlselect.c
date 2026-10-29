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

/* The XdndSelection value Lisp set (`gui-set-selection'), which
   x-begin-drag converts when a drag starts; no other client sees it
   before.  The drag's bytes stay in the shell until the next drag.  */
static Lisp_Object nhwl_xdnd_value;

/* The shell's selection kind for SEL, or -1 when there is none.  NIL
   means PRIMARY, as in pgtk.  */
static int
nhwl_selection_kind (Lisp_Object sel)
{
  if (NILP (sel) || EQ (sel, QPRIMARY))
    return NHG_SEL_PRIMARY;
  if (EQ (sel, QCLIPBOARD))
    return NHG_SEL_CLIPBOARD;
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
compositor refuses (no keyboard focus).  The value of `XdndSelection'
is kept for `x-begin-drag', which converts it.  */)
  (Lisp_Object selection, Lisp_Object value, Lisp_Object frame)
{
  if (EQ (selection, Qnhwl_dnd_selection))
    return nhwl_xdnd_value = value;
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
  if (EQ (selection, Qnhwl_dnd_selection))
    {
      Lisp_Object owned = NILP (nhwl_xdnd_value) ? Qnil : Qt;
      nhwl_xdnd_value = Qnil;
      return owned;
    }
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
  if (EQ (selection, Qnhwl_dnd_selection))
    return NILP (nhwl_xdnd_value) ? Qnil : Qt;
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
  ie->arg = kind == NHG_SEL_PRIMARY ? QPRIMARY : QCLIPBOARD;
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

/* Dragging out of our frames (x-begin-drag), with X's arguments and
   results.  The XdndSelection value is converted for every target when
   the drag starts; the glue (dnd.rs) runs the drag, and nhwl_drag_run
   (nhwlterm.c) waits for its end.  */

/* x-begin-drag's ACTION as the actions offered (NHG_DND_*) and the one
   our own frames prefer.  Wayland has no link or private action: they
   are copies.  A move also offers copy, so a target that only copies
   still takes the drop (the result says which it did).  A list asks.  */
static void
nhwl_drag_actions (Lisp_Object action, uint32_t *offered,
		   uint32_t *preferred)
{
  *offered = *preferred = NHG_DND_COPY;
  if (EQ (action, Qnhwl_xdnd_move))
    {
      *offered = NHG_DND_MOVE | NHG_DND_COPY;
      *preferred = NHG_DND_MOVE;
    }
  else if (EQ (action, Qnhwl_xdnd_ask))
    {
      *offered = NHG_DND_ASK | NHG_DND_COPY | NHG_DND_MOVE;
      *preferred = NHG_DND_ASK;
    }
  else if (CONSP (action))
    {
      Lisp_Object tail = action;

      *offered = *preferred = NHG_DND_ASK;
      FOR_EACH_TAIL (tail)
	{
	  Lisp_Object item = XCAR (tail);

	  CHECK_CONS (item);
	  CHECK_SYMBOL (XCAR (item));
	  CHECK_STRING (XCDR (item));
	  *offered |= (EQ (XCAR (item), Qnhwl_xdnd_move) ? NHG_DND_MOVE
		       : NHG_DND_COPY);
	}
      CHECK_LIST_END (tail, action);
    }
  /* Any other symbol is one of X's own protocols' (such as XDS),
     which Wayland does not have: a copy.  */
  else if (!SYMBOLP (action))
    signal_error ("Invalid drag-and-drop action", action);
}

/* x-begin-drag's result for the action the target performed (NHG_DND_*;
   0: none).  */
static Lisp_Object
nhwl_drag_result (int action)
{
  switch (action)
    {
    case NHG_DND_COPY:
      return Qnhwl_xdnd_copy;
    case NHG_DND_MOVE:
      return Qnhwl_xdnd_move;
    case NHG_DND_ASK:
      return Qnhwl_xdnd_private;
    default:
      return Qnil;
    }
}

static Lisp_Object
nhwl_drag_convert_failed (Lisp_Object err, ptrdiff_t nargs,
			  Lisp_Object *args)
{
  return Qnil;
}

/* The XdndSelection value VALUE converted for TARGET (a symbol) as X
   converts a selection it owns, through `selection-converter-alist': a
   unibyte string, or nil when no converter makes a string (or it
   signals).  A string VALUE's text property named TARGET stands for it.  */
static Lisp_Object
nhwl_drag_convert (Lisp_Object value, Lisp_Object target)
{
  Lisp_Object alist = (NILP (Fboundp (Qnhwl_selection_converter_alist))
		       ? Qnil
		       : Fsymbol_value (Qnhwl_selection_converter_alist));
  Lisp_Object handler = CDR_SAFE (Fassq (target, alist));

  /* (AVAILABLE-P . CONVERTER): X's local conversion skips the test.  */
  if (CONSP (handler))
    handler = XCDR (handler);
  if (NILP (handler))
    return Qnil;
  if (STRINGP (value))
    {
      Lisp_Object alt = Fget_text_property (make_fixnum (0), target, value);

      if (!NILP (alt))
	value = alt;
    }

  Lisp_Object args[] = { handler, Qnhwl_dnd_selection, target, value };
  Lisp_Object r = internal_condition_case_n (Ffuncall, ARRAYELTS (args),
					     args, Qerror,
					     nhwl_drag_convert_failed);
  /* (TYPE . DATA) */
  if (CONSP (r) && SYMBOLP (XCAR (r)))
    r = XCDR (r);
  if (!STRINGP (r))
    return Qnil;
  if (STRING_MULTIBYTE (r))
    r = code_convert_string_norecord (r, Qutf_8, true);
  return r;
}

/* The bytes VALUE offers for TARGETS (a list of strings), as a list of
   (MIME . BYTES) in TARGETS' order: every target that converts, plus
   text/plain;charset=utf-8 when a text target converted and TARGETS
   lacks it.  */
static Lisp_Object
nhwl_drag_items (Lisp_Object value, Lisp_Object targets)
{
  Lisp_Object items = Qnil;
  Lisp_Object utf8 = SYMBOL_NAME (Qtext_plain_utf8);
  bool text = false;

  for (Lisp_Object tail = targets; CONSP (tail); tail = XCDR (tail))
    {
      Lisp_Object mime = XCAR (tail);
      Lisp_Object target = Fintern (mime, Qnil);

      if (!NILP (Fassoc (mime, items, Qnil)))
	continue;
      Lisp_Object bytes = nhwl_drag_convert (value, target);
      if (NILP (bytes))
	continue;
      items = Fcons (Fcons (mime, bytes), items);
      text |= nhwl_text_target_p (target);
    }
  if (text && NILP (Fassoc (utf8, items, Qnil)))
    {
      Lisp_Object bytes = nhwl_drag_convert (value, Qtext_plain_utf8);

      if (!NILP (bytes))
	items = Fcons (Fcons (utf8, bytes), items);
    }
  return Fnreverse (items);
}

/* `nhwl-dnd-finish-timeout' in seconds, from 0 to 60.  */
static double
nhwl_dnd_finish_secs (void)
{
  double secs = (NUMBERP (Vnhwl_dnd_finish_timeout)
		 ? XFLOATINT (Vnhwl_dnd_finish_timeout) : 5.0);
  return secs < 0 ? 0 : secs > 60 ? 60 : secs;
}

DEFUN ("x-begin-drag", Fx_begin_drag, Sx_begin_drag, 1, 6, 0,
       doc: /* Begin dragging contents on FRAME, with targets TARGETS.
TARGETS is a list of strings, which defines the selection targets that
will be available to the drop target.  Block until the mouse buttons
are released, then return the action chosen by the target, or `nil' if
the drop was not accepted by the drop target.  Dragging starts when the
mouse is pressed on FRAME, and the contents of the selection
`XdndSelection' will be sent to the client underneath the mouse
pointer (the drop target) when the mouse button is released.  Each
target is converted when the drag starts, through
`selection-converter-alist'; the ones that convert are offered, and
text is also offered as text/plain;charset=utf-8.

ACTION is a symbol which tells the target what it should do, and can
be one of the following:

 - `XdndActionCopy', which means to copy the contents from the drag
   source (FRAME) to the drop target.

 - `XdndActionMove', which means to first take the contents of
   `XdndSelection', and to delete whatever was saved into that
   selection afterwards.  A target that only copies may still copy.

`XdndActionPrivate' is also a valid return value, and means that the
drop target chose to perform an unspecified or unknown action (on
Wayland, the target asked the user and did not say what it did).

The source is also expected to cooperate with the target to perform
the action chosen by the target.  For example, callers should delete
the buffer text that was dragged if `XdndActionMove' is returned.

`XdndActionLink' and other actions are copies here: Wayland has only
copy, move and ask.

If RETURN-FRAME is non-nil, this function will return the frame if the
mouse pointer moves onto an Emacs frame, after first moving out of
FRAME.  If RETURN-FRAME is the symbol `now', any frame underneath the
mouse pointer will be returned immediately.  The drag is given up
then.

If ACTION is a list and not nil, its elements are assumed to be a cons
of (ITEM . STRING), where ITEM is the name of an action, and STRING is
a string describing ITEM to the user.  The drop target is expected to
prompt the user to choose between any of the actions in the list.

If ACTION is not specified or nil, `XdndActionCopy' is used
instead.

If ALLOW-CURRENT-FRAME is nil, a drop on FRAME itself is refused.
Otherwise it is taken: a drop on one of our own frames is handled by
`pgtk-dnd.el' after this function returns.

FOLLOW-TOOLTIP is ignored.

When the target was dropped on but does not finish within
`nhwl-dnd-finish-timeout' seconds, the drag is given up and nil is
returned, so a move never deletes text the target may not have taken.
C-g gives the drag up and quits.

This function returns nil immediately if no mouse button is held
down.  It should only be called when it is known that mouse buttons
are being held down, such as immediately after a `down-mouse-1' (or
similar) event.  */)
  (Lisp_Object targets, Lisp_Object action, Lisp_Object frame,
   Lisp_Object return_frame, Lisp_Object allow_current_frame,
   Lisp_Object follow_tooltip)
{
  struct frame *f = decode_window_system_frame (frame);
  struct nhwl_output *out = FRAME_NHWL_OUTPUT (f);
  uint32_t offered, preferred, flags = 0;
  struct frame *to = NULL;
  Lisp_Object items, tail;
  ptrdiff_t n, i = 0;
  int rc, done;
  USE_SAFE_ALLOCA;

  CHECK_LIST (targets);
  tail = targets;
  FOR_EACH_TAIL (tail)
    CHECK_STRING (XCAR (tail));
  CHECK_LIST_END (tail, targets);
  nhwl_drag_actions (action, &offered, &preferred);
  if (!NILP (allow_current_frame))
    flags |= NHG_DRAG_ALLOW_CURRENT;
  if (EQ (return_frame, Qnhwl_now))
    flags |= NHG_DRAG_RETURN_NOW;
  else if (!NILP (return_frame))
    flags |= NHG_DRAG_RETURN_FRAME;

  if (!out || !out->window || !FRAME_NHG (f) || NILP (nhwl_xdnd_value))
    return Qnil;
  items = nhwl_drag_items (nhwl_xdnd_value, targets);
  n = list_length (items);
  if (n == 0)
    return Qnil;

  const char **mimes;
  const uint8_t **data;
  size_t *lens;
  SAFE_NALLOCA (mimes, 1, n);
  SAFE_NALLOCA (data, 1, n);
  SAFE_NALLOCA (lens, 1, n);
  for (tail = items; CONSP (tail); tail = XCDR (tail), i++)
    {
      mimes[i] = SSDATA (XCAR (XCAR (tail)));
      data[i] = SDATA (XCDR (XCAR (tail)));
      lens[i] = SBYTES (XCDR (XCAR (tail)));
    }
  block_input ();
  rc = nhwl_glue_rc (FRAME_DISPLAY_INFO (f),
		     NHG (nhg_drag_start (FRAME_NHG (f), out->window, mimes,
					  data, lens, n, offered, preferred,
					  flags)));
  unblock_input ();
  SAFE_FREE ();
  /* NHG_E_STATE: no button is held, and X returns nil at once too.  */
  if (rc < 0)
    return Qnil;

  done = nhwl_drag_run (FRAME_DISPLAY_INFO (f), nhwl_dnd_finish_secs (), &to);
  if (to)
    {
      Lisp_Object value;

      XSETFRAME (value, to);
      return value;
    }
  return nhwl_drag_result (done);
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

  DEFSYM (Qnhwl_xdnd_copy, "XdndActionCopy");
  DEFSYM (Qnhwl_xdnd_move, "XdndActionMove");
  DEFSYM (Qnhwl_xdnd_ask, "XdndActionAsk");
  DEFSYM (Qnhwl_xdnd_private, "XdndActionPrivate");
  DEFSYM (Qnhwl_now, "now");
  DEFSYM (Qnhwl_selection_converter_alist, "selection-converter-alist");
  nhwl_xdnd_value = Qnil;
  staticpro (&nhwl_xdnd_value);
  DEFVAR_LISP ("nhwl-dnd-finish-timeout", Vnhwl_dnd_finish_timeout,
    doc: /* Seconds `x-begin-drag' waits for a target to finish a drop.
After this long without a word from the drag, or after the drop without
the target's finish, the drag is given up and `x-begin-drag' returns
nil.  At most 60.  */);
  Vnhwl_dnd_finish_timeout = make_float (5.0);
  defsubr (&Sx_begin_drag);

  defsubr (&Spgtk_own_selection_internal);
  defsubr (&Spgtk_disown_selection_internal);
  defsubr (&Spgtk_selection_owner_p);
  defsubr (&Spgtk_selection_exists_p);
  defsubr (&Spgtk_get_selection_internal);
  defsubr (&Spgtk_register_dnd_targets);
  defsubr (&Spgtk_drop_finish);
  defsubr (&Spgtk_update_drop_status);
}

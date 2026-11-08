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
#include "sysselect.h"
#include "thread.h"
#include "nidhogg-testclock.h"
#ifdef HAVE_NIDHOGG_WATCHDOG
#include "nidhogg-watchdog.h"
#endif

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

/* The non-main threads that want the global lock (thread.c), and the
   waiters a condition's wake is on its way to (nidhogg-testclock.c's
   marks): while it is above 0 the main wait does not park.  */
_Atomic int nhmac_lock_waiters;

/* The main wait (process.c), NS's ns_select in the glue's terms.  On
   the main thread with a non-zero timeout: pump AppKit's queued events
   first, so keys reach kbd_buffer before process output; yield once, as
   NS does, so another Lisp thread can take the lock; then wait in
   nhg_wait with the global lock held, its run loop serving AppKit and
   one kqueue holding the fds.  waiting_for_input is cleared around the
   wait and restored by assignment: set_waiting_for_input could throw a
   C-g from here, and a C-g during the wait is a key event, or quit-flag,
   which the caller reads after the select.  The wait makes the
   watchdog's calls really_call_select would have made.  Anything else
   (another thread, a zero timeout, no display, an auto-advancing test
   clock, whose rounds only really_call_select runs) is a plain
   thread_select.  */
int
nhmac_select (int nfds, fd_set *rfds, fd_set *wfds, fd_set *efds,
	      struct timespec *timeout, sigset_t *sigmask)
{
  struct nhwl_display_info *dpyinfo = x_display_list;
  bool zero = timeout && timeout->tv_sec == 0 && timeout->tv_nsec == 0;

  if (zero || nh_tc_auto || !dpyinfo || !dpyinfo->glue
      || dpyinfo->glue_broken || !main_thread_p (current_thread))
    return thread_select (pselect, nfds, rfds, wfds, efds, timeout,
			  sigmask);

  static bool told;
  if (!told)
    {
      NHG (nhg_set_lock_waiters (&nhmac_lock_waiters));
      told = true;
    }
  struct timespec end
    = timeout ? timespec_add (current_timespec (), *timeout) : invalid_timespec ();
  nhwl_glue_rc (dpyinfo, NHG (nhg_dispatch_main (dpyinfo->glue)));
  struct timespec tick = { 0, 1 };
  thread_select (pselect, 0, NULL, NULL, NULL, &tick, sigmask);

  /* While Lisp threads want the lock, never park: pump, then yield the
     lock in a select on the caller's own fds, backing off from 1 ns
     through 1 us, doubling to 1 ms, and return for anything the caller
     must see.  */
  long backoff = 1;
  while (atomic_load (&nhmac_lock_waiters) > 0)
    {
      nhwl_glue_rc (dpyinfo, NHG (nhg_dispatch_main (dpyinfo->glue)));
      struct timespec t = make_timespec (0, backoff);
      if (timespec_valid_p (end))
	{
	  struct timespec left = timespec_sub (end, current_timespec ());
	  if (timespec_sign (left) <= 0)
	    goto nothing;
	  if (timespec_cmp (left, t) < 0)
	    t = left;
	}
      fd_set r, w;
      if (rfds)
	r = *rfds;
      if (wfds)
	w = *wfds;
      int n = thread_select (pselect, nfds, rfds ? &r : NULL,
			     wfds ? &w : NULL, NULL, &t, sigmask);
      if (n != 0)
	{
	  if (rfds)
	    *rfds = r;
	  if (wfds)
	    *wfds = w;
	  if (efds)
	    FD_ZERO (efds);
	  return n;
	}
      if (detect_input_pending () || !NILP (Vquit_flag) || pending_signals)
	goto nothing;
      backoff = backoff == 1 ? 1000 : min (backoff * 2, 1000000);
    }

  int64_t ns = -1;
  if (timespec_valid_p (end))
    {
      struct timespec left = timespec_sub (end, current_timespec ());
      if (timespec_sign (left) <= 0)
	goto nothing;
      if (left.tv_sec < INT64_MAX / 1000000000 - 1)
	ns = left.tv_sec * (int64_t) 1000000000 + left.tv_nsec;
    }
  bool waiting = waiting_for_input;
  waiting_for_input = false;
#ifdef HAVE_NIDHOGG_WATCHDOG
  nh_wd_wait_enter (current_thread, timeout);
#endif
  int rc = NHG (nhg_wait (dpyinfo->glue, nfds, rfds, wfds, ns));
#ifdef HAVE_NIDHOGG_WATCHDOG
  nh_wd_wait_leave (current_thread);
#endif
  waiting_for_input = waiting;
  if (rc < 0)
    {
      /* The sets are untouched: the glue writes them only on success.  */
      nhwl_glue_rc (dpyinfo, rc);
      return thread_select (pselect, nfds, rfds, wfds, efds, timeout,
			    sigmask);
    }
  if (efds)
    FD_ZERO (efds);
  return rc;

 nothing:
  if (rfds)
    FD_ZERO (rfds);
  if (wfds)
    FD_ZERO (wfds);
  if (efds)
    FD_ZERO (efds);
  return 0;
}

/* Startup: the locale, before Lisp exists (main calls this first).  */
void
nhmac_startup (void)
{
  NHG (nhg_startup ());
}

/* A configured path placed in the app bundle (NS_SELF_CONTAINED builds
   keep them relative).  The answer is read at once or kept by the
   caller, and there are few calls, so a small ring holds them.  */
const char *
ns_relocate (const char *epath)
{
  static char ring[4][4096];
  static int next;
  char *out = ring[next++ % 4];
  size_t len = 0;

  if (NHG (nhg_relocate (epath, out, sizeof ring[0], &len)) != NHG_OK
      || len == 0)
    return epath;
  return out;
}

/* A choice from the native menu bar.  Lisp keeps the key paths of the
   tree it last sent (`nhmac-menu-paths', by the item's place in the tree)
   and the tree's generation (`nhmac-menu-generation'); the choice becomes
   the menu-bar events of the item's path, as find_and_call_menu_selection
   makes them, and one from an older tree is dropped.  Returns how many
   events it stored.  */
int
nhmac_menu_choice (const NhgAction *a, struct input_event *hold_quit)
{
  Lisp_Object gen = find_symbol_value (intern_c_string ("nhmac-menu-generation"));
  /* Generation 0 is the application's own events (quit, open files,
     reopen): their paths are fixed.  */
  Lisp_Object paths
    = find_symbol_value (intern_c_string (a->flags == 0 ? "nhmac-app-paths"
					  : "nhmac-menu-paths"));
  Lisp_Object path, frame;
  struct input_event buf;
  int stored = 0;

  if ((a->flags != 0 && (!FIXNUMP (gen) || XFIXNUM (gen) != a->flags))
      || !CONSP (paths))
    return 0;
  path = Fcdr (Fassq (make_fixnum (a->code), paths));
  if (!CONSP (path))
    return 0;
  XSETFRAME (frame, SELECTED_FRAME ());
  EVENT_INIT (buf);
  buf.kind = MENU_BAR_EVENT;
  buf.frame_or_window = frame;
  buf.arg = frame;
  kbd_buffer_store_event_hold (&buf, hold_quit);
  stored++;
  for (; CONSP (path); path = XCDR (path), stored++)
    {
      buf.arg = XCAR (path);
      kbd_buffer_store_event_hold (&buf, hold_quit);
    }
  return stored;
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

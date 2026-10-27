/* Nidhogg's stall watchdog: the GLib poll wrapper and the dialog depth.

   Pgtk only (the body is under HAVE_PGTK).

   A menu or a dialog the user is looking at runs inside a command, in
   a GLib main loop that blocks in the GLib poll function.  The
   wrapper installed here tells the watchdog that this wait is the
   user's (`nh_wd_wait_enter_modal'), so it does not count as a stall;
   a Lisp timer that runs inside it is ordinary busy time, as the span
   ends and restarts around each blocking poll.  `xg_select' selects
   by itself and never reaches the wrapper.

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

#ifdef HAVE_NIDHOGG_WATCHDOG

#ifdef HAVE_PGTK

#include <gtk/gtk.h>

#include "lisp.h"
#include "dispextern.h"
#include "pgtkterm.h"
#include "thread.h"
#include "nidhogg-watchdog.h"

int nh_wd_dialog_depth;

static GPollFunc nh_wd_real_poll;

static gint
nh_wd_gtk_poll (GPollFD *ufds, guint nfds, gint timeout)
{
  /* Only the main thread's blocking polls inside a menu or a dialog;
     anything else is just the real poll.  */
  if (timeout == 0 || !main_thread_p (current_thread)
      || !(popup_activated () || nh_wd_dialog_depth > 0))
    return nh_wd_real_poll (ufds, nfds, timeout);

  struct timespec ts, *tsp = NULL;
  if (timeout > 0)
    {
      ts = make_timespec (timeout / 1000, (timeout % 1000) * 1000000L);
      tsp = &ts;
    }
  nh_wd_wait_enter_modal (tsp);
  gint r = nh_wd_real_poll (ufds, nfds, timeout);
  nh_wd_wait_leave (current_thread);
  return r;
}

/* Wrap the default context's poll function, once.  */
void
nh_wd_gtk_install (void)
{
  if (nh_wd_real_poll)
    return;
  GMainContext *ctx = g_main_context_default ();
  nh_wd_real_poll = g_main_context_get_poll_func (ctx);
  g_main_context_set_poll_func (ctx, nh_wd_gtk_poll);
}

/* Test-only: a GLib timeout that ends the active menu or dialog's loop,
   as if the user had dismissed it.  */
static gboolean
nh_wd_gtk_dismiss (gpointer data)
{
  (void) data;
  if (popup_activated ())
    pgtk_menu_set_in_use (false);
  /* A GtkDialog run by `xg_dialog_run' (file and font choosers).  */
  GList *tops = gtk_window_list_toplevels ();
  for (GList *l = tops; l; l = l->next)
    if (GTK_IS_DIALOG (l->data) && gtk_widget_get_visible (l->data))
      gtk_dialog_response (GTK_DIALOG (l->data), GTK_RESPONSE_CANCEL);
  g_list_free (tops);
  return G_SOURCE_REMOVE;
}

static Lisp_Object
nh_wd_gtk_dismiss_after (Lisp_Object ms)
{
  CHECK_FIXNAT (ms);
  g_timeout_add (min (XFIXNAT (ms), G_MAXINT), nh_wd_gtk_dismiss, NULL);
  return Qnil;
}

DEFUN ("nidhogg-watchdog--test-popdown-after",
       Fnidhogg_watchdog__test_popdown_after,
       Snidhogg_watchdog__test_popdown_after, 1, 1, 0,
       doc: /* Pop down the active popup menu after MS milliseconds (a GLib timeout).
Test-only.  */)
  (Lisp_Object ms)
{
  return nh_wd_gtk_dismiss_after (ms);
}

DEFUN ("nidhogg-watchdog--test-dialog-response-after",
       Fnidhogg_watchdog__test_dialog_response_after,
       Snidhogg_watchdog__test_dialog_response_after, 1, 1, 0,
       doc: /* Close the active dialog after MS milliseconds without a choice (a GLib timeout).
Test-only.  */)
  (Lisp_Object ms)
{
  return nh_wd_gtk_dismiss_after (ms);
}

void
nh_wd_gtk_syms (void)
{
  defsubr (&Snidhogg_watchdog__test_popdown_after);
  defsubr (&Snidhogg_watchdog__test_dialog_response_after);
}

#endif /* HAVE_PGTK */

#endif /* HAVE_NIDHOGG_WATCHDOG */

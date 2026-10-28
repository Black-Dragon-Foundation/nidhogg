/* Nidhogg's stall watchdog: phases, felt input and typing commands.

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

/* The phase stack lives here (its inline push and pop are in
   nidhogg-watchdog-phase.h), with the per-span input accounting:

   - felt: the first keyboard, button or wheel event stored in a span
     (kbd_buffer_store_buffered_event, on whichever thread holds the
     global lock) marks the span felt and keeps its arrival: the event's
     own timestamp when it is a pgtk event whose timestamp reads as
     CLOCK_MONOTONIC milliseconds (as measured on mutter and KWin: 32
     bits, accepted within the last 2 s), else the store time.
     A span that ran a command is felt too.  These words are relaxed
     atomics (struct nh_wd_input): the main thread resets them in
     wait_leave without the lock.
   - commands: command_loop_1 counts each command (the edges take the
     delta per span); a second command in one span means its key was
     queued behind the first (`:input-queued').
   - typing: a command in the typing set (`--set-typing-commands')
     opens a measure from its event's arrival (the arrival recorded when
     the event was stored, found again by its kbd_buffer slot when
     kbd_buffer_get_event takes it), closed when the next input event
     is taken or at the next responsive wait, whichever comes first; it
     goes into the `typing' histogram.  This state is the main thread's
     only (command_loop_1 on a Lisp thread is not counted).  */

#include <config.h>

#ifdef HAVE_NIDHOGG_WATCHDOG

#include <string.h>

#include "nidhogg-watchdog.h"
#include "nidhogg-watchdog-phase.h"
#include "termhooks.h"
#include "frame.h"
#include "window.h"

struct nh_wd_phases nh_wd_phases;
_Thread_local bool nh_wd_phase_thread;
int nh_wd_redisplay_depth = NH_WD_PHASE_NONE;

/* An event's timestamp is accepted as its arrival when it is at most
   this old (ms); else the store time is used.  */
enum { NH_WD_TIMESTAMP_WINDOW_MS = 2000 };

/* The main thread's per-span state, reset when a span opens.  */
static uint32_t span_commands;	  /* commands started in it */
static uint64_t span_cmd_arrival; /* the first command's input arrival */
static uint64_t taken_arrival;	  /* the last input event taken, until a
				     command starts; 0 none */
static bool typing_open;	  /* a typing command is being measured */
static uint64_t typing_from;	  /* its input's arrival */

/* The typing commands, compared by EQ; each slot is staticpro'd.  */
enum { NH_WD_TYPING_MAX = 16 };
static Lisp_Object typing_commands[NH_WD_TYPING_MAX];
static int typing_n;

/* The arrival of the last input events copied into kbd_buffer, by their
   slot (newest at ARRIVALS_NEXT - 1).  Written and read with the global
   lock held.  STORED_ARRIVAL is the arrival nh_wd_input_stored worked
   out for the event being stored.  */
enum { NH_WD_ARRIVALS = 32 };
static struct
{
  const struct input_event *slot;
  uint64_t ns;
} arrivals[NH_WD_ARRIVALS];
static unsigned arrivals_next;
static uint64_t stored_arrival;


/* Phases.  */

/* Whether CATCH is H or older than H: reachable from H by ->next.  Walk
   from both ends at once, so the cost is the distance between the two,
   not the depth of the handler chain.  */
static bool
handler_reaches (struct handler *h, struct handler *catch)
{
  for (struct handler *a = h, *b = catch; a || b; )
    {
      if (a == catch)
	return true;
      if (b == h)
	return false;
      if (a)
	a = a->next;
      if (b)
	b = b->next;
    }
  return false;
}

/* unwind_to_catch is about to leave for CATCH: pop every phase pushed
   at or inside it.  Pushes nest, so they are popped from the top down
   while CATCH is reachable from the handler a push recorded.  */
void
nh_wd_unwind_phases (struct handler *catch)
{
  if (!nh_wd_phase_thread)
    return;
  int d = nh_wd_phases.depth;
  if (d > NH_WD_PHASE_DEPTH)
    d = NH_WD_PHASE_DEPTH;
  while (d > 0 && handler_reaches (nh_wd_phases.slot[d - 1].h, catch))
    d--;
  nh_wd_phase_pop_to (d);
}

void
nh_wd_test_phase_hit (int ph)
{
  Lisp_Object want = Vnidhogg_watchdog__test_sample_on_phase;
  if (SYMBOLP (want) && 0 <= ph && ph < NH_WD_PH_N
      && strcmp (SSDATA (SYMBOL_NAME (want)), nh_wd_phase_names[ph]) == 0)
    Vnidhogg_watchdog__test_last_sample = Fnidhogg_watchdog__test_sample ();
}


/* Input.  */

static bool
input_kind_p (int kind)
{
  switch (kind)
    {
    case ASCII_KEYSTROKE_EVENT:
    case MULTIBYTE_CHAR_KEYSTROKE_EVENT:
    case NON_ASCII_KEYSTROKE_EVENT:
    case MOUSE_CLICK_EVENT:
    case WHEEL_EVENT:
    case HORIZ_WHEEL_EVENT:
      return true;
    default:
      return false;
    }
}

/* The arrival of an event stored at NOW with timestamp TS_MS (32-bit
   CLOCK_MONOTONIC milliseconds): NOW minus its age, when the age is
   within the window.  */
static uint64_t
arrival_from_timestamp (uint64_t ts_ms, uint64_t now)
{
  uint32_t age = (uint32_t) (now / 1000000) - (uint32_t) ts_ms;
  if (age > NH_WD_TIMESTAMP_WINDOW_MS)
    return now;
  uint64_t a = (uint64_t) age * 1000000;
  return a < now ? now - a : now;
}

static bool
event_on_pgtk_p (const struct input_event *ev)
{
  Lisp_Object fw = ev->frame_or_window;
  if (WINDOWP (fw))
    fw = WINDOW_FRAME (XWINDOW (fw));
  return FRAMEP (fw) && FRAME_PGTK_P (XFRAME (fw));
}

/* Input arrived at ARRIVAL: the first in the span marks it felt.  */
static void
input_note (uint64_t arrival)
{
  uint32_t unfelt = 0;
  if (__atomic_compare_exchange_n (&nh_wd_input.felt, &unfelt, 1, false,
				   __ATOMIC_RELAXED, __ATOMIC_RELAXED))
    __atomic_store_n (&nh_wd_input.arrival_ns, arrival, __ATOMIC_RELAXED);
}

void
nh_wd_input_stored (const struct input_event *ev)
{
  if (!nh_wd_on || !input_kind_p (ev->kind))
    return;
  uint64_t now = nh_wd_now_ns ();
  stored_arrival = (ev->timestamp != 0 && event_on_pgtk_p (ev)
		    ? arrival_from_timestamp (ev->timestamp, now) : now);
  input_note (stored_arrival);
}

void
nh_wd_input_buffered (const struct input_event *slot)
{
  if (!nh_wd_on || !input_kind_p (slot->kind))
    return;
  arrivals[arrivals_next].slot = slot;
  arrivals[arrivals_next].ns = stored_arrival;
  arrivals_next = (arrivals_next + 1) % NH_WD_ARRIVALS;
}

/* A typing measure ends at NOW.  */
static void
typing_close (uint64_t now)
{
  if (!typing_open)
    return;
  typing_open = false;
  nh_wd_hist_add (NH_WD_H_TYPING, now > typing_from ? now - typing_from : 0);
}

void
nh_wd_input_taken (const struct input_event *slot)
{
  if (!nh_wd_on || !nh_wd_phase_thread || !input_kind_p (slot->kind))
    return;
  uint64_t now = nh_wd_now_ns ();
  typing_close (now);
  /* Its arrival, from the newest record of its slot; unknown (more
     than NH_WD_ARRIVALS events behind) is the take time, a lower
     bound of the wait.  */
  taken_arrival = now;
  for (unsigned i = 1; i <= NH_WD_ARRIVALS; i++)
    {
      unsigned k = (arrivals_next + NH_WD_ARRIVALS - i) % NH_WD_ARRIVALS;
      if (arrivals[k].slot == slot)
	{
	  taken_arrival = arrivals[k].ns;
	  arrivals[k].slot = NULL;
	  break;
	}
    }
}

void
nh_wd_command_start (Lisp_Object command)
{
  if (!nh_wd_on || !nh_wd_phase_thread)
    return;
  uint64_t now = nh_wd_now_ns ();
  __atomic_fetch_add (&nh_wd_input.commands, 1, __ATOMIC_RELAXED);
  if (span_commands++ > 0)
    __atomic_store_n (&nh_wd_input.queued, 1, __ATOMIC_RELAXED);
  typing_close (now);
  uint64_t arrival = taken_arrival ? taken_arrival : now;
  taken_arrival = 0;
  if (!span_cmd_arrival)
    span_cmd_arrival = arrival;
  for (int i = 0; i < typing_n; i++)
    if (EQ (command, typing_commands[i]))
      {
	typing_open = true;
	typing_from = arrival;
	break;
      }
}

/* A span opens at NOW (main thread; from wait_leave, without the global
   lock).  */
void
nh_wd_input_span_open (uint64_t now)
{
  nh_wd_phase_thread = true;
  __atomic_store_n (&nh_wd_input.felt, 0, __ATOMIC_RELAXED);
  __atomic_store_n (&nh_wd_input.arrival_ns, 0, __ATOMIC_RELAXED);
  __atomic_store_n (&nh_wd_input.queued, 0, __ATOMIC_RELAXED);
  (void) now;
  span_commands = 0;
  span_cmd_arrival = 0;
  taken_arrival = 0;
  typing_open = false;
}

/* The span ends at NOW (a responsive wait): close the typing measure
   and, when it is a stall ST, fill its input fields.  */
void
nh_wd_input_span_close (struct nh_wd_stall *st, uint64_t now)
{
  typing_close (now);
  if (!st)
    return;
  bool felt = __atomic_load_n (&nh_wd_input.felt, __ATOMIC_RELAXED) != 0;
  uint64_t arrival = (felt
		      ? __atomic_load_n (&nh_wd_input.arrival_ns,
					 __ATOMIC_RELAXED)
		      : 0);
  if (span_cmd_arrival && (!arrival || span_cmd_arrival < arrival))
    arrival = span_cmd_arrival;
  st->felt = felt || st->kind;
  st->input_queued = __atomic_load_n (&nh_wd_input.queued,
				      __ATOMIC_RELAXED) != 0;
  st->input_wait_ns = arrival && arrival < now ? now - arrival : 0;
}


/* Lisp.  */

DEFUN ("nidhogg-watchdog--set-typing-commands",
       Fnidhogg_watchdog__set_typing_commands,
       Snidhogg_watchdog__set_typing_commands, 1, 1, 0,
       doc: /* Set the commands whose latency the `typing' histogram measures.
COMMANDS is a list of at most 16 symbols, compared with `eq'.  Return
their number.  */)
  (Lisp_Object commands)
{
  ptrdiff_t n = list_length (commands);
  if (n > NH_WD_TYPING_MAX)
    args_out_of_range (commands, make_fixnum (NH_WD_TYPING_MAX));
  for (Lisp_Object tail = commands; CONSP (tail); tail = XCDR (tail))
    CHECK_SYMBOL (XCAR (tail));
  int i = 0;
  for (Lisp_Object tail = commands; CONSP (tail); tail = XCDR (tail))
    typing_commands[i++] = XCAR (tail);
  typing_n = i;
  for (; i < NH_WD_TYPING_MAX; i++)
    typing_commands[i] = Qnil;
  return make_fixnum (typing_n);
}

DEFUN ("nidhogg-watchdog--test-phase", Fnidhogg_watchdog__test_phase,
       Snidhogg_watchdog__test_phase, 0, 0, 0,
       doc: /* Test only: the phase on top of the phase stack, a symbol.  */)
  (void)
{
  uint8_t phase, reason;
  nh_wd_phase_top (&phase, &reason);
  return intern_c_string (nh_wd_phase_names[phase < NH_WD_PH_N
					     ? phase : NH_WD_PH_LISP]);
}

DEFUN ("nidhogg-watchdog--test-input-stored",
       Fnidhogg_watchdog__test_input_stored,
       Snidhogg_watchdog__test_input_stored, 1, 2, 0,
       doc: /* Test only: input of KIND is stored now.
KIND is `key', `button' or `wheel' (input), or any other symbol (an
event that is not input).  TIMESTAMP-MS, when non-nil, is the event's
own timestamp in CLOCK_MONOTONIC milliseconds, as a calibrated
compositor gives it.  Return nil.  */)
  (Lisp_Object kind, Lisp_Object timestamp_ms)
{
  CHECK_SYMBOL (kind);
  const char *k = SSDATA (SYMBOL_NAME (kind));
  bool input = (strcmp (k, "key") == 0 || strcmp (k, "button") == 0
		|| strcmp (k, "wheel") == 0);
  uint64_t now = nh_wd_now_ns ();
  uint64_t arrival = now;
  if (!NILP (timestamp_ms))
    {
      CHECK_FIXNAT (timestamp_ms);
      arrival = arrival_from_timestamp (XFIXNAT (timestamp_ms), now);
    }
  if (nh_wd_on && input)
    input_note (arrival);
  return Qnil;
}

DEFUN ("nidhogg-watchdog--test-command", Fnidhogg_watchdog__test_command,
       Snidhogg_watchdog__test_command, 1, 1, 0,
       doc: /* Test only: a command COMMAND starts, as in the command loop.
Return nil.  */)
  (Lisp_Object command)
{
  nh_wd_command_start (command);
  return Qnil;
}

void
nh_wd_input_syms (void)
{
  for (int i = 0; i < NH_WD_TYPING_MAX; i++)
    {
      typing_commands[i] = Qnil;
      staticpro (&typing_commands[i]);
    }

  DEFVAR_LISP ("nidhogg-watchdog--test-sample-on-phase",
	       Vnidhogg_watchdog__test_sample_on_phase,
	       doc: /* Test only: a phase whose push takes a sample.
When this is a phase symbol (such as `regex'), every push of that phase
takes a synchronous sample into `nidhogg-watchdog--test-last-sample'.  */);
  Vnidhogg_watchdog__test_sample_on_phase = Qnil;

  DEFVAR_LISP ("nidhogg-watchdog--test-last-sample",
	       Vnidhogg_watchdog__test_last_sample,
	       doc: /* Test only: the sample the last sampling phase push took.
See `nidhogg-watchdog--test-sample-on-phase'.  */);
  Vnidhogg_watchdog__test_last_sample = Qnil;

  defsubr (&Snidhogg_watchdog__set_typing_commands);
  defsubr (&Snidhogg_watchdog__test_phase);
  defsubr (&Snidhogg_watchdog__test_input_stored);
  defsubr (&Snidhogg_watchdog__test_command);
}

#endif /* HAVE_NIDHOGG_WATCHDOG */

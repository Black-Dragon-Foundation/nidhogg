/* Nidhogg's stall watchdog: the park protocol's steps.

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

/* The monitor thread sleeps on a condition variable while Emacs
   idles, and the main thread wakes it when a busy span starts:

     main thread, busy edge:  span = s + 1; waiting = 0;
                              if (parked) { lock m; signal; unlock m }
     monitor, park:           lock m; parked = 1;
                              while (waiting && span == last) cond_wait;
                              parked = 0; unlock m

   Every store and load of the three words is at NH_WD_PARK_ORDER,
   seq_cst: a Dekker pair.  At least one side sees the other's store:
   either the monitor sees the new span and does not wait, or the main
   thread sees parked == 1 and signals under the mutex, which the
   monitor cannot miss, because it checks under that mutex before it
   waits.  At a weaker order both stores can sit in their store buffers
   while both loads read the old values: the monitor waits through a
   busy span (a lost wake-up).

   Each step below makes one access to one word, so the same code runs
   three ways: run to the end on the shared words (the monitor and the
   main thread, nidhogg-watchdog-monitor.c, and the real-thread harness,
   nidhogg/test/watchdog-park-test.sh), or one step at a time on the
   interleaving model at the end of this file, whose stores go through a
   store buffer per side.  A change of NH_WD_PARK_ORDER, or of the
   steps' order, reaches all three.

   This header needs nothing from Emacs (the harness compiles it
   alone).  */

#ifndef NIDHOGG_WATCHDOG_PARK_H
#define NIDHOGG_WATCHDOG_PARK_H

#include <stdbool.h>
#include <stdint.h>

/* The memory order of every access to span, waiting and parked.  The
   harness overrides it with -D for its mutation runs.  */
#ifndef NH_WD_PARK_ORDER
# define NH_WD_PARK_ORDER __ATOMIC_SEQ_CST
#endif

/* The three words, and the two sides.  */
enum nh_wd_park_word
{
  NH_WD_PARK_SPAN,
  NH_WD_PARK_WAITING,
  NH_WD_PARK_PARKED
};

enum nh_wd_park_side
{
  NH_WD_PARK_MAIN,
  NH_WD_PARK_MONITOR
};

/* What a step returns besides the next pc (which is >= 0).  */
enum
{
  NH_WD_PARK_DONE = -1,		/* main: the monitor was not parked */
  NH_WD_PARK_SIGNAL = -2,	/* main: it was; signal under the mutex */
  NH_WD_PARK_WAIT = -3,		/* monitor: wait, then resume at pc 1 */
  NH_WD_PARK_GO = -4		/* monitor: a span to look at */
};

/* One side's store buffer (the model only): stores not yet visible to
   the other side, oldest first.  */
struct nh_wd_park_buffer
{
  int n;
  struct
  {
    int word;
    uint64_t value;
  } e[8];
};

/* The memory the steps access.  BUF null: the real words, with
   __atomic_* at NH_WD_PARK_ORDER.  BUF non-null: the model's words,
   and BUF[side] is each side's store buffer.  */
struct nh_wd_park_mem
{
  uint64_t *span;
  uint32_t *waiting;
  uint32_t *parked;
  struct nh_wd_park_buffer *buf;
};

/* The model's plain access to a word of memory.  */
static inline uint64_t
nh_wd_park_peek (struct nh_wd_park_mem *m, int word)
{
  return (word == NH_WD_PARK_SPAN ? *m->span
	  : word == NH_WD_PARK_WAITING ? *m->waiting : *m->parked);
}

static inline void
nh_wd_park_poke (struct nh_wd_park_mem *m, int word, uint64_t v)
{
  if (word == NH_WD_PARK_SPAN)
    *m->span = v;
  else if (word == NH_WD_PARK_WAITING)
    *m->waiting = v;
  else
    *m->parked = v;
}

/* The model: make SIDE's oldest buffered store visible.  */
static inline void
nh_wd_park_flush_one (struct nh_wd_park_mem *m, int side)
{
  struct nh_wd_park_buffer *b = &m->buf[side];
  nh_wd_park_poke (m, b->e[0].word, b->e[0].value);
  for (int i = 1; i < b->n; i++)
    b->e[i - 1] = b->e[i];
  b->n--;
}

/* The model: make all of SIDE's stores visible (a full barrier: a
   seq_cst store, or a mutex operation).  */
static inline void
nh_wd_park_drain (struct nh_wd_park_mem *m, int side)
{
  while (m->buf[side].n)
    nh_wd_park_flush_one (m, side);
}

/* Store V into WORD for SIDE.  The model: a seq_cst store is visible
   before the side's next load (it drains the buffer and writes
   through, as the x86 mapping's locked instruction does); any weaker
   store waits in the buffer.  */
static inline void
nh_wd_park_store (struct nh_wd_park_mem *m, int side, int word, uint64_t v)
{
  if (!m->buf)
    switch (word)
      {
      case NH_WD_PARK_SPAN:
	__atomic_store_n (m->span, v, NH_WD_PARK_ORDER);
	break;
      case NH_WD_PARK_WAITING:
	__atomic_store_n (m->waiting, (uint32_t) v, NH_WD_PARK_ORDER);
	break;
      default:
	__atomic_store_n (m->parked, (uint32_t) v, NH_WD_PARK_ORDER);
	break;
      }
  else if (NH_WD_PARK_ORDER == __ATOMIC_SEQ_CST)
    {
      nh_wd_park_drain (m, side);
      nh_wd_park_poke (m, word, v);
    }
  else
    {
      struct nh_wd_park_buffer *b = &m->buf[side];
      b->e[b->n].word = word;
      b->e[b->n].value = v;
      b->n++;
    }
}

/* Load WORD for SIDE.  The model: the side's own newest buffered store
   of WORD, else memory.  */
static inline uint64_t
nh_wd_park_load (struct nh_wd_park_mem *m, int side, int word)
{
  if (!m->buf)
    switch (word)
      {
      case NH_WD_PARK_SPAN:
	return __atomic_load_n (m->span, NH_WD_PARK_ORDER);
      case NH_WD_PARK_WAITING:
	return __atomic_load_n (m->waiting, NH_WD_PARK_ORDER);
      default:
	return __atomic_load_n (m->parked, NH_WD_PARK_ORDER);
      }
  struct nh_wd_park_buffer *b = &m->buf[side];
  for (int i = b->n - 1; i >= 0; i--)
    if (b->e[i].word == word)
      return b->e[i].value;
  return nh_wd_park_peek (m, word);
}

/* The main thread's busy edge into span SPAN, step PC (from 0): store
   span, store waiting = 0, then load parked.  Returns the next pc, or
   NH_WD_PARK_DONE, or NH_WD_PARK_SIGNAL (the caller signals the
   condition variable under the mutex).  */
static inline int
nh_wd_park_busy_step (struct nh_wd_park_mem *m, int pc, uint64_t span)
{
  switch (pc)
    {
    case 0:
      nh_wd_park_store (m, NH_WD_PARK_MAIN, NH_WD_PARK_SPAN, span);
      return 1;
    case 1:
      nh_wd_park_store (m, NH_WD_PARK_MAIN, NH_WD_PARK_WAITING, 0);
      return 2;
    default:
      return (nh_wd_park_load (m, NH_WD_PARK_MAIN, NH_WD_PARK_PARKED)
	      ? NH_WD_PARK_SIGNAL : NH_WD_PARK_DONE);
    }
}

/* The monitor's park, with the mutex held, step PC: 0 stores parked =
   1; 1 loads waiting; 2 loads span.  Returns the next pc, NH_WD_PARK_GO
   (a busy span, or a span other than LAST) or NH_WD_PARK_WAIT (wait on
   the condition variable, then resume at pc 1).  BUSY_ENDS false makes
   a busy span count as idle (the test clock): then only a new span
   ends the park.  */
static inline int
nh_wd_park_step (struct nh_wd_park_mem *m, int pc, uint64_t last,
		 bool busy_ends)
{
  switch (pc)
    {
    case 0:
      nh_wd_park_store (m, NH_WD_PARK_MONITOR, NH_WD_PARK_PARKED, 1);
      return 1;
    case 1:
      return (busy_ends
	      && !nh_wd_park_load (m, NH_WD_PARK_MONITOR, NH_WD_PARK_WAITING)
	      ? NH_WD_PARK_GO : 2);
    default:
      return (nh_wd_park_load (m, NH_WD_PARK_MONITOR, NH_WD_PARK_SPAN) != last
	      ? NH_WD_PARK_GO : NH_WD_PARK_WAIT);
    }
}

/* The monitor leaves the park (still holding the mutex).  */
static inline void
nh_wd_park_leave (struct nh_wd_park_mem *m)
{
  nh_wd_park_store (m, NH_WD_PARK_MONITOR, NH_WD_PARK_PARKED, 0);
}

/* The main thread's busy edge, run to the end.  True: signal.  */
static inline bool
nh_wd_park_busy (struct nh_wd_park_mem *m, uint64_t span)
{
  int pc = 0;
  do
    pc = nh_wd_park_busy_step (m, pc, span);
  while (pc >= 0);
  return pc == NH_WD_PARK_SIGNAL;
}

/* The monitor's park check from PC (0 on entering the park, 1 after a
   wait), run to the end.  True: wait.  */
static inline bool
nh_wd_park_must_wait (struct nh_wd_park_mem *m, int pc, uint64_t last,
		      bool busy_ends)
{
  do
    pc = nh_wd_park_step (m, pc, last, busy_ends);
  while (pc >= 0);
  return pc == NH_WD_PARK_WAIT;
}

/* The interleaving model.  The monitor parks against span LAST while
   Emacs waits; the main thread starts span LAST + 1.  Every
   interleaving of their steps, the mutex and the condition variable
   (a wait releases the mutex and blocks in one step; a signal takes
   the mutex, wakes a blocked waiter and releases it, in one step), and
   the flushes of each side's store buffer is run to its end.  An end
   with the monitor still blocked is a lost wake-up.  Mutex operations
   and the wait drain the side's buffer (they are full barriers).  */

enum
{
  NH_WD_PARK_M_LOCK = -10,	/* monitor: about to take the mutex */
  NH_WD_PARK_M_BLOCKED = -11,	/* monitor: waiting on the condition variable */
  NH_WD_PARK_M_RELOCK = -12,	/* monitor: woken, retaking the mutex */
  NH_WD_PARK_M_LEAVE = -13,	/* monitor: leaving the park */
  NH_WD_PARK_M_END = -14	/* either side: finished */
};

struct nh_wd_park_model
{
  uint64_t span;
  uint32_t waiting, parked;
  struct nh_wd_park_buffer buf[2];
  int main_pc, monitor_pc;
  bool locked;
};

static inline struct nh_wd_park_mem
nh_wd_park_model_mem (struct nh_wd_park_model *s)
{
  return (struct nh_wd_park_mem) { &s->span, &s->waiting, &s->parked,
				   s->buf };
}

static inline void
nh_wd_park_model_explore (const struct nh_wd_park_model *s, uint64_t last,
			  long *cases, long *lost)
{
  bool moved = false;
  struct nh_wd_park_model t;
  struct nh_wd_park_mem m;

  /* The main thread.  */
  if (s->main_pc >= 0)
    {
      t = *s;
      m = nh_wd_park_model_mem (&t);
      t.main_pc = nh_wd_park_busy_step (&m, t.main_pc, last + 1);
      if (t.main_pc == NH_WD_PARK_DONE)
	t.main_pc = NH_WD_PARK_M_END;
      nh_wd_park_model_explore (&t, last, cases, lost);
      moved = true;
    }
  else if (s->main_pc == NH_WD_PARK_SIGNAL && !s->locked)
    {
      t = *s;
      m = nh_wd_park_model_mem (&t);
      nh_wd_park_drain (&m, NH_WD_PARK_MAIN);
      if (t.monitor_pc == NH_WD_PARK_M_BLOCKED)
	t.monitor_pc = NH_WD_PARK_M_RELOCK;
      t.main_pc = NH_WD_PARK_M_END;
      nh_wd_park_model_explore (&t, last, cases, lost);
      moved = true;
    }

  /* The monitor.  */
  if ((s->monitor_pc == NH_WD_PARK_M_LOCK
       || s->monitor_pc == NH_WD_PARK_M_RELOCK)
      && !s->locked)
    {
      t = *s;
      m = nh_wd_park_model_mem (&t);
      nh_wd_park_drain (&m, NH_WD_PARK_MONITOR);
      t.locked = true;
      t.monitor_pc = s->monitor_pc == NH_WD_PARK_M_LOCK ? 0 : 1;
      nh_wd_park_model_explore (&t, last, cases, lost);
      moved = true;
    }
  else if (s->monitor_pc == NH_WD_PARK_M_LEAVE)
    {
      t = *s;
      m = nh_wd_park_model_mem (&t);
      nh_wd_park_leave (&m);
      nh_wd_park_drain (&m, NH_WD_PARK_MONITOR);
      t.locked = false;
      t.monitor_pc = NH_WD_PARK_M_END;
      nh_wd_park_model_explore (&t, last, cases, lost);
      moved = true;
    }
  else if (s->monitor_pc >= 0)
    {
      t = *s;
      m = nh_wd_park_model_mem (&t);
      int pc = nh_wd_park_step (&m, t.monitor_pc, last, true);
      if (pc == NH_WD_PARK_WAIT)
	{
	  nh_wd_park_drain (&m, NH_WD_PARK_MONITOR);
	  t.locked = false;
	  pc = NH_WD_PARK_M_BLOCKED;
	}
      else if (pc == NH_WD_PARK_GO)
	pc = NH_WD_PARK_M_LEAVE;
      t.monitor_pc = pc;
      nh_wd_park_model_explore (&t, last, cases, lost);
      moved = true;
    }

  /* A buffered store becomes visible.  */
  for (int side = NH_WD_PARK_MAIN; side <= NH_WD_PARK_MONITOR; side++)
    if (s->buf[side].n)
      {
	t = *s;
	m = nh_wd_park_model_mem (&t);
	nh_wd_park_flush_one (&m, side);
	nh_wd_park_model_explore (&t, last, cases, lost);
	moved = true;
      }

  if (!moved)
    {
      (*cases)++;
      if (s->monitor_pc == NH_WD_PARK_M_BLOCKED)
	(*lost)++;
    }
}

/* Run the model: *CASES interleavings, *LOST of them lost wake-ups.  */
static inline void
nh_wd_park_model_run (long *cases, long *lost)
{
  struct nh_wd_park_model s = {
    .span = 5, .waiting = 1, .parked = 0,
    .main_pc = 0, .monitor_pc = NH_WD_PARK_M_LOCK, .locked = false
  };
  *cases = *lost = 0;
  nh_wd_park_model_explore (&s, 5, cases, lost);
}

#endif /* NIDHOGG_WATCHDOG_PARK_H */

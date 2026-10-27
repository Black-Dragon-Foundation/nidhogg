/* Nidhogg's stall watchdog: the monitor thread, the park protocol and
   the hang files.

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

/* The main thread counts every busy span and turns each one at least
   as long as the threshold T into a record itself (nidhogg-watchdog.c);
   the monitor only adds evidence.  It is a plain pthread with every
   signal blocked, takes only nh_wd.m, and never touches Lisp, the
   specpdl or anyone else's locks:

     park while Emacs waits: no wake-ups at all while idle; the main
       thread signals it when a busy span starts (nidhogg-watchdog-park.h);
     for busy span s since t0: sleep until t0 + T/2 (most spans end
       first); if s is still busy, arm: note the main thread's CPU clock
       and the lateness; then every P = max (T/10, 1 ms) (1 ms while deep
       mode is pending) raise a request for s and send the main thread
       NIDHOGG_WATCHDOG_SIGNAL, whose handler takes a sample and clears
       the request (nidhogg-watchdog-sample.c); a request still pending
       after 2 P is missed; past the hang threshold, names mode and the
       hang file (below).

   The main thread counts, so a monitor that never runs (a starved
   thread) costs samples, never a record.  While `--test-clock' is set
   the monitor never arms: it treats every span as idle.

   The main thread's CPU clock is read here (about 0.5 us, a real
   system call) and once more when a stall closes, never on an edge.  */

#include <config.h>

#ifdef HAVE_NIDHOGG_WATCHDOG

#include <errno.h>
#ifdef __linux__
# include <sched.h>
# include <sys/syscall.h>
# include <unistd.h>

# include <fcntl.h>
# include <stdio.h>
# include <stdlib.h>
# include <string.h>
#endif

#include "nidhogg-watchdog.h"
#include "nidhogg-watchdog-park.h"
#include "nidhogg-watchdog-phase.h"

enum
{
  NH_WD_MS = 1000000		/* ns */
};

/* A request: the pending bit and the low 31 bits of its span.  */
#define NH_WD_PENDING (UINT32_C (1) << 31)
#define NH_WD_SPAN_BITS UINT32_C (0x7fffffff)

/* The shared words as the park protocol's memory.  */
static struct nh_wd_park_mem
nh_wd_park_real (void)
{
  return (struct nh_wd_park_mem) { &nh_wd.span, &nh_wd.waiting,
				   &nh_wd.parked, NULL };
}

/* The main thread's side.  */

void
nh_wd_publish_waiting (uint64_t now)
{
  (void) now;
  __atomic_store_n (&nh_wd.waiting, 1, NH_WD_PARK_ORDER);
  /* From here on the handler counts any request as stale; drop the
     one in flight.  */
  __atomic_store_n (&nh_wd.request, 0, __ATOMIC_SEQ_CST);
}

void
nh_wd_publish_busy (uint64_t now)
{
  struct nh_wd_park_mem mem = nh_wd_park_real ();
  /* busy_since before span: the monitor reads span, busy_since, span.  */
  __atomic_store_n (&nh_wd.busy_since_ns, now, NH_WD_PARK_ORDER);
  if (nh_wd_park_busy (&mem, __atomic_load_n (&nh_wd.span,
					       __ATOMIC_RELAXED) + 1))
    {
      pthread_mutex_lock (&nh_wd.m);
      pthread_cond_signal (&nh_wd.cv);
      pthread_mutex_unlock (&nh_wd.m);
    }
}

#ifdef __linux__

static pthread_t nh_wd_mon_thread;
static bool nh_wd_mon_running;	/* start and stop only */
static bool nh_wd_mon_stop;	/* under nh_wd.m */
static pid_t nh_wd_mon_tid;	/* the monitor's task, for stop */
static clockid_t nh_wd_main_cpu; /* the main thread's CPU clock */
/* Requests of armed_span that the handler never took (atomic).  */
static uint32_t nh_wd_mon_missed;

static void nh_wd_hang_tick (uint64_t, uint64_t, uint64_t);
static void nh_wd_hang_end (void);

static uint64_t
nh_wd_clock_ns (clockid_t clock)
{
  struct timespec ts;
  if (clock_gettime (clock, &ts) != 0)
    return 0;
  return (uint64_t) ts.tv_sec * 1000000000 + ts.tv_nsec;
}

/* With nh_wd.m held: wait on the condition variable until DEADLINE
   (CLOCK_MONOTONIC ns), a stop, or a spurious wake-up.  */
static void
nh_wd_mon_sleep_until (uint64_t deadline)
{
  struct timespec ts = { .tv_sec = deadline / 1000000000,
			 .tv_nsec = deadline % 1000000000 };
  pthread_cond_timedwait (&nh_wd.cv, &nh_wd.m, &ts);
}

/* Is span S still busy?  */
static bool
nh_wd_mon_busy (uint64_t s)
{
  return (__atomic_load_n (&nh_wd.span, __ATOMIC_SEQ_CST) == s
	  && !__atomic_load_n (&nh_wd.waiting, __ATOMIC_SEQ_CST));
}

/* A return from a wait that found no busy span.  */
static void
nh_wd_mon_woke_idle (void)
{
  __atomic_add_fetch (&nh_wd.monitor_wakeups, 1, __ATOMIC_RELAXED);
}

/* The sampling period: the configured one, else max (T/10, 1 ms);
   1 ms while deep mode is pending.  */
static uint64_t
nh_wd_mon_period (void)
{
  if (__atomic_load_n (&nh_wd.deep_left, __ATOMIC_RELAXED) > 0)
    return NH_WD_MS;
  uint64_t p = __atomic_load_n (&nh_wd.period_ns, __ATOMIC_RELAXED);
  if (!p)
    p = __atomic_load_n (&nh_wd.threshold_ns, __ATOMIC_RELAXED) / 10;
  return p < NH_WD_MS ? NH_WD_MS : p;
}

/* Gather evidence for busy span S, busy since T0, until it ends or the
   monitor stops.  Called and returns with nh_wd.m held.  */
static void
nh_wd_mon_watch (uint64_t s, uint64_t t0)
{
  uint64_t deadline
    = t0 + __atomic_load_n (&nh_wd.threshold_ns, __ATOMIC_RELAXED) / 2;
  uint64_t now = nh_wd_clock_ns (CLOCK_MONOTONIC);

  /* Most spans end before T/2.  */
  while (now < deadline)
    {
      nh_wd_mon_sleep_until (deadline);
      if (nh_wd_mon_stop)
	return;
      if (!nh_wd_mon_busy (s))
	{
	  nh_wd_mon_woke_idle ();
	  return;
	}
      now = nh_wd_clock_ns (CLOCK_MONOTONIC);
    }

  /* A stall candidate: arm.  The values first, then armed_span with a
     release store; the main thread's span close reads armed_span with
     acquire and uses the values only when it names its own span.  */
  __atomic_store_n (&nh_wd_mon_missed, 0, __ATOMIC_RELAXED);
  __atomic_store_n (&nh_wd.late_ns, now - deadline, __ATOMIC_RELAXED);
  __atomic_store_n (&nh_wd.cpu_armed_ns, nh_wd_clock_ns (nh_wd_main_cpu),
		    __ATOMIC_RELAXED);
  __atomic_store_n (&nh_wd.mode, 0, __ATOMIC_RELAXED);
  __atomic_store_n (&nh_wd.armed_span, s, __ATOMIC_RELEASE);

  uint32_t tag = ((uint32_t) s & NH_WD_SPAN_BITS) | NH_WD_PENDING;
  uint64_t raised = 0, next = now;
  for (;;)
    {
      uint64_t period = nh_wd_mon_period ();
      uint64_t hang = __atomic_load_n (&nh_wd.hang_ns, __ATOMIC_RELAXED);
      if (hang && now - t0 >= hang)
	__atomic_store_n (&nh_wd.mode, 1, __ATOMIC_RELAXED);
      nh_wd_hang_tick (s, t0, now);

      uint32_t req = __atomic_load_n (&nh_wd.request, __ATOMIC_ACQUIRE);
      if (req == tag)
	{
	  /* The last request is still pending.  */
	  if (now - raised >= 2 * period)
	    {
	      __atomic_add_fetch (&nh_wd.missed, 1, __ATOMIC_RELAXED);
	      __atomic_add_fetch (&nh_wd_mon_missed, 1, __ATOMIC_RELAXED);
	      __atomic_add_fetch (&nh_wd.requests, 1, __ATOMIC_RELAXED);
	      raised = now;
	      pthread_kill (nh_wd.main_id, NIDHOGG_WATCHDOG_SIGNAL);
	    }
	}
      /* Raise it from the value just seen, after one more look at the
	 span: a raise that loses to publish_waiting's clear fails, and
	 the handler drops one that wins just before the clear (the span
	 is waiting by then).  */
      else if (nh_wd_mon_busy (s)
	       && __atomic_compare_exchange_n (&nh_wd.request, &req, tag,
					       false, __ATOMIC_SEQ_CST,
					       __ATOMIC_SEQ_CST))
	{
	  __atomic_add_fetch (&nh_wd.requests, 1, __ATOMIC_RELAXED);
	  raised = now;
	  pthread_kill (nh_wd.main_id, NIDHOGG_WATCHDOG_SIGNAL);
	}

      next += period;
      if (next <= now)
	next = now + period;
      nh_wd_mon_sleep_until (next);
      if (nh_wd_mon_stop)
	return;
      if (!nh_wd_mon_busy (s))
	{
	  nh_wd_mon_woke_idle ();
	  return;
	}
      now = nh_wd_clock_ns (CLOCK_MONOTONIC);
    }
}

/* Hang files.  The monitor owns the file: past
   the hang threshold it writes HANG-FILE (nh_wd_hang_file, from
   `--start') through a temporary file, open/write/fsync/rename, first
   once a names-mode copy is in (or 2 s into the hang) and then every
   2 s, and deletes it when the span has ended, before it parks.  The
   main thread never touches the file, so a delete cannot race a rename
   and bring back a finished hang.  The identity (pid, the process start
   time, the boot id) is read once at start, on the main thread; the
   monitor never touches Lisp.

   All of this state is the monitor's, or under nh_wd.m.  */

enum
{
  NH_WD_HANG_EVERY = 2000 * NH_WD_MS,
  NH_WD_HANG_STACKS = 4
};

static char *nh_wd_hang_tmp;	/* HANG-FILE.tmp */
static int nh_wd_hang_dir = -1;	/* HANG-FILE's directory, for fsync */
static char nh_wd_hang_ident[160]; /* pid, proc-start and boot-id lines */
static bool nh_wd_hang_written;	/* HANG-FILE is ours and exists */
/* The session stops inside a hang (`kill-emacs' from SIGTERM or SIGHUP
   runs while the main thread is busy): the watch's end keeps
   HANG-FILE, for the next start's `:killed' record.  */
static bool nh_wd_hang_keep;
static uint64_t nh_wd_hang_since, nh_wd_hang_next;
static uint32_t nh_wd_hang_seen; /* the names_next copied last */
/* The latest names-mode copies, newest at HEAD - 1.  */
static struct nh_wd_names nh_wd_hang_stacks[NH_WD_HANG_STACKS];
static int nh_wd_hang_head, nh_wd_hang_nstacks;
static char nh_wd_hang_buf[NH_WD_HANG_STACKS * 32 * 65 + 512];

/* Tests (`--test-hold-rename'), under nh_wd.m: HOLD holds the next
   write between fsync and rename, HELD says one is held; WRITES counts
   the finished writes, WATCHING is the span the monitor watches (0:
   none).  An idle wait may hold several short busy spans the monitor
   never watched, so "ended" is "no older span watched", not a count.  */
static pthread_cond_t nh_wd_test_cv = PTHREAD_COND_INITIALIZER;
static bool nh_wd_test_hold, nh_wd_test_held;
static uint64_t nh_wd_test_writes, nh_wd_mon_watching;

/* Read up to SIZE - 1 bytes of FILE into BUF, NUL ended; false if it
   cannot be read.  */
static bool
nh_wd_read_small (const char *file, char *buf, size_t size)
{
  int fd = open (file, O_RDONLY | O_CLOEXEC);
  if (fd < 0)
    return false;
  ssize_t n = read (fd, buf, size - 1);
  close (fd);
  if (n < 0)
    return false;
  buf[n] = 0;
  return true;
}

/* At monitor start (main thread): this process's identity, and the
   temporary file's name.  */
static void
nh_wd_hang_start (void)
{
  char stat[1024], boot[64] = "";
  unsigned long long start = 0;
  if (nh_wd_read_small ("/proc/self/stat", stat, sizeof stat))
    {
      /* Field 22, counted after the command's closing parenthesis
	 (the command may hold spaces).  */
      char *p = strrchr (stat, ')');
      for (int field = 2; p && *p; p++)
	if (*p == ' ' && ++field == 22)
	  {
	    start = strtoull (p + 1, NULL, 10);
	    break;
	  }
    }
  if (nh_wd_read_small ("/proc/sys/kernel/random/boot_id", boot, sizeof boot))
    boot[strcspn (boot, "\n")] = 0;
  snprintf (nh_wd_hang_ident, sizeof nh_wd_hang_ident,
	    "pid %ld\nproc-start %llu\nboot-id %s\n",
	    (long) getpid (), start, boot);

  xfree (nh_wd_hang_tmp);
  nh_wd_hang_tmp = NULL;
  if (nh_wd_hang_dir >= 0)
    close (nh_wd_hang_dir);
  nh_wd_hang_dir = -1;
  nh_wd_hang_keep = false;
  if (nh_wd_hang_file)
    {
      size_t len = strlen (nh_wd_hang_file);
      nh_wd_hang_tmp = xmalloc (len + sizeof ".tmp");
      memcpy (nh_wd_hang_tmp, nh_wd_hang_file, len);
      memcpy (nh_wd_hang_tmp + len, ".tmp", sizeof ".tmp");

      /* The directory, opened once: a rename is durable only once the
	 directory is synced too (a hard freeze often ends in a reset).  */
      char *slash = strrchr (nh_wd_hang_tmp, '/');
      if (slash)
	{
	  char *dir = xstrdup (nh_wd_hang_tmp);
	  dir[slash > nh_wd_hang_tmp ? slash - nh_wd_hang_tmp : 1] = 0;
	  nh_wd_hang_dir = open (dir, O_RDONLY | O_DIRECTORY | O_CLOEXEC);
	  xfree (dir);
	}
    }
}

/* After the monitor stopped (main thread).  */
static void
nh_wd_hang_stop (void)
{
  if (nh_wd_hang_dir >= 0)
    close (nh_wd_hang_dir);
  nh_wd_hang_dir = -1;
  xfree (nh_wd_hang_tmp);
  nh_wd_hang_tmp = NULL;
  nh_wd_test_hold = nh_wd_test_held = false;
}

/* Copy the latest names-mode slot, if it is new and belongs to the
   span busy since T0.  The handler fills a slot, then publishes
   names_next with a release store; a names_next that changed while the
   slot was copied means the copy may be torn, and it is dropped (a
   seqlock).  */
static void
nh_wd_hang_copy_names (uint64_t t0)
{
  for (int tries = 0; tries < 3; tries++)
    {
      uint32_t k = __atomic_load_n (&nh_wd_cur.names_next, __ATOMIC_ACQUIRE);
      if (k == 0 || k == nh_wd_hang_seen)
	return;
      struct nh_wd_names *dst = &nh_wd_hang_stacks[nh_wd_hang_head];
      memcpy (dst, &nh_wd_cur.names[(k - 1) % 4], sizeof *dst);
      __atomic_thread_fence (__ATOMIC_ACQUIRE);
      if (__atomic_load_n (&nh_wd_cur.names_next, __ATOMIC_RELAXED) != k)
	continue;
      if (dst->t_ns < t0)
	return;			/* an older span's */
      nh_wd_hang_seen = k;
      nh_wd_hang_head = (nh_wd_hang_head + 1) % NH_WD_HANG_STACKS;
      if (nh_wd_hang_nstacks < NH_WD_HANG_STACKS)
	nh_wd_hang_nstacks++;
      return;
    }
}

/* Append the names of ST (the innermost, which is empty, left out) as
   one `stack' line to B at *N, at most SIZE bytes in all.  A tab or a
   control character in a name becomes `?'.  */
static void
nh_wd_hang_put_stack (char *b, size_t *n, size_t size,
		      const struct nh_wd_names *st)
{
  int m = st->n < 32 ? st->n : 32;
  if (*n + 6 >= size)
    return;
  memcpy (b + *n, "stack", 5);
  *n += 5;
  for (int i = 1; i < m; i++)
    {
      const char *name = st->name[i];
      if (!name[0] || *n + 66 >= size)
	continue;
      b[(*n)++] = '\t';
      for (int j = 0; j < 63 && name[j]; j++)
	b[(*n)++] = (unsigned char) name[j] < ' ' ? '?' : name[j];
    }
  b[(*n)++] = '\n';
}

/* Write HANG-FILE for span S, busy since T0, at NOW.  */
static void
nh_wd_hang_write (uint64_t s, uint64_t t0, uint64_t now)
{
  uint64_t real = nh_wd_clock_ns (CLOCK_REALTIME);
  uint8_t phase, reason;
  /* The main thread's phase stack, read from here: a racy but harmless
     look at a few bytes.  */
  nh_wd_phase_top (&phase, &reason);
  char *b = nh_wd_hang_buf;
  size_t size = sizeof nh_wd_hang_buf;
  int len = snprintf (b, size,
		      "nidhogg-watchdog-hang 1\n%sstart %.6f\nduration-ms %.1f\n"
		      "phase %s\n",
		      nh_wd_hang_ident, (real - (now - t0)) / 1e9,
		      (now - t0) / 1e6,
		      phase < NH_WD_PH_N ? nh_wd_phase_names[phase] : "lisp");
  if (len < 0 || (size_t) len >= size)
    return;
  size_t n = len;
  for (int i = 1; i <= nh_wd_hang_nstacks; i++)
    nh_wd_hang_put_stack (b, &n, size,
			  &nh_wd_hang_stacks[(nh_wd_hang_head - i
					      + NH_WD_HANG_STACKS)
					     % NH_WD_HANG_STACKS]);

  int fd = open (nh_wd_hang_tmp, O_WRONLY | O_CREAT | O_TRUNC | O_CLOEXEC,
		 0600);
  if (fd < 0)
    return;
  bool ok = true;
  for (size_t off = 0; ok && off < n; )
    {
      ssize_t w = write (fd, b + off, n - off);
      if (w > 0)
	off += w;
      else if (w < 0 && errno != EINTR)
	ok = false;
    }
  ok = fsync (fd) == 0 && ok;
  ok = close (fd) == 0 && ok;

  /* Tests: hold here, between fsync and rename.  */
  if (nh_wd_test_hold)
    {
      nh_wd_test_held = true;
      pthread_cond_broadcast (&nh_wd_test_cv);
      while (nh_wd_test_hold && !nh_wd_mon_stop)
	pthread_cond_wait (&nh_wd_test_cv, &nh_wd.m);
      nh_wd_test_held = false;
    }

  /* Still the hang's span (under nh_wd.m)?  A span that ended meanwhile
     keeps no file.  */
  if (ok && !nh_wd_mon_stop && nh_wd_mon_busy (s)
      && rename (nh_wd_hang_tmp, nh_wd_hang_file) == 0)
    {
      /* The hang's first rename makes the directory entry: sync it.  */
      if (!nh_wd_hang_written && nh_wd_hang_dir >= 0)
	fsync (nh_wd_hang_dir);
      nh_wd_hang_written = true;
    }
  else
    unlink (nh_wd_hang_tmp);
  nh_wd_test_writes++;
  pthread_cond_broadcast (&nh_wd_test_cv);
}

/* In the watch of span S, busy since T0, at NOW: past the hang
   threshold (names mode), copy the names and write when due.  */
static void
nh_wd_hang_tick (uint64_t s, uint64_t t0, uint64_t now)
{
  if (!nh_wd_hang_tmp || __atomic_load_n (&nh_wd.mode, __ATOMIC_RELAXED) != 1)
    return;
  if (!nh_wd_hang_since)
    nh_wd_hang_since = now;
  nh_wd_hang_copy_names (t0);
  if (nh_wd_hang_next
      ? now >= nh_wd_hang_next
      : nh_wd_hang_nstacks > 0 || now - nh_wd_hang_since >= NH_WD_HANG_EVERY)
    {
      nh_wd_hang_write (s, t0, now);
      nh_wd_hang_next = now + NH_WD_HANG_EVERY;
    }
}

/* The watch of a span ended (the span ended, or the monitor stops):
   delete its hang file, before parking, unless the session stops
   inside the hang.  */
static void
nh_wd_hang_end (void)
{
  if (nh_wd_hang_written && !nh_wd_hang_keep)
    unlink (nh_wd_hang_file);
  nh_wd_hang_written = false;
  nh_wd_hang_since = nh_wd_hang_next = 0;
  nh_wd_hang_seen = 0;
  nh_wd_hang_head = nh_wd_hang_nstacks = 0;
  nh_wd_mon_watching = 0;
  pthread_cond_broadcast (&nh_wd_test_cv);
}

/* Tests, main thread, with nh_wd.m held: wait (busy: no responsive
   wait) on the test condition variable until PRED (ARG), at most 30 s.
   Return PRED (ARG).  */
static bool
nh_wd_test_wait (bool (*pred) (uint64_t), uint64_t arg)
{
  struct timespec deadline;
  clock_gettime (CLOCK_REALTIME, &deadline);
  deadline.tv_sec += 30;
  while (!pred (arg))
    if (!nh_wd_mon_running
	|| (pthread_cond_timedwait (&nh_wd_test_cv, &nh_wd.m, &deadline)
	    == ETIMEDOUT))
      return pred (arg);
  return true;
}

static bool
nh_wd_test_held_p (uint64_t arg)
{
  (void) arg;
  return nh_wd_test_held;
}

static bool
nh_wd_test_written_p (uint64_t writes)
{
  return nh_wd_test_writes > writes;
}

static bool
nh_wd_test_ended_p (uint64_t span)
{
  return nh_wd_mon_watching == 0 || nh_wd_mon_watching >= span;
}

static Lisp_Object
nh_wd_test_hold_rename (Lisp_Object arg)
{
  bool ok = false;
  pthread_mutex_lock (&nh_wd.m);
  if (EQ (arg, QCheld))
    ok = nh_wd_test_wait (nh_wd_test_held_p, 0);
  else if (EQ (arg, QCended))
    ok = nh_wd_test_wait (nh_wd_test_ended_p,
			  __atomic_load_n (&nh_wd.span, __ATOMIC_RELAXED));
  else if (!NILP (arg))
    nh_wd_test_hold = true;
  else
    {
      nh_wd_test_hold = false;
      if (nh_wd_test_held)
	{
	  uint64_t writes = nh_wd_test_writes;
	  pthread_cond_broadcast (&nh_wd_test_cv);
	  ok = nh_wd_test_wait (nh_wd_test_written_p, writes);
	}
    }
  pthread_mutex_unlock (&nh_wd.m);
  return ok ? Qt : Qnil;
}

static void *
nh_wd_monitor_main (void *arg)
{
  struct nh_wd_park_mem mem = nh_wd_park_real ();
  uint64_t last = __atomic_load_n (&nh_wd.span, __ATOMIC_SEQ_CST);
  uint64_t cpu_counted = 0;
  (void) arg;

  nh_wd_mon_tid = syscall (SYS_gettid);

  pthread_mutex_lock (&nh_wd.m);
  while (!nh_wd_mon_stop)
    {
      /* Park until a busy span (or, on the test clock, a new span).  */
      bool busy_ends = !nh_wd_test_clock_p ();
      if (nh_wd_park_must_wait (&mem, 0, last, busy_ends))
	{
	  /* Its own CPU time, added at each park (`--stats' may reset
	     the counter).  */
	  uint64_t cpu = nh_wd_clock_ns (CLOCK_THREAD_CPUTIME_ID);
	  __atomic_add_fetch (&nh_wd.monitor_cpu_ns, cpu - cpu_counted,
			      __ATOMIC_RELAXED);
	  cpu_counted = cpu;
	  bool wait = true;
	  while (wait && !nh_wd_mon_stop)
	    {
	      pthread_cond_wait (&nh_wd.cv, &nh_wd.m);
	      if (__atomic_load_n (&nh_wd.waiting, __ATOMIC_SEQ_CST))
		nh_wd_mon_woke_idle ();
	      busy_ends = !nh_wd_test_clock_p ();
	      wait = nh_wd_park_must_wait (&mem, 1, last, busy_ends);
	    }
	}
      nh_wd_park_leave (&mem);
      if (nh_wd_mon_stop)
	break;

      /* Span s, busy since t0: busy_since is stored before span.  */
      uint64_t s = __atomic_load_n (&nh_wd.span, __ATOMIC_SEQ_CST);
      uint64_t t0 = __atomic_load_n (&nh_wd.busy_since_ns, __ATOMIC_SEQ_CST);
      if (__atomic_load_n (&nh_wd.span, __ATOMIC_SEQ_CST) != s)
	continue;
      last = s;
      if (busy_ends && nh_wd_mon_busy (s))
	{
	  nh_wd_mon_watching = s;
	  nh_wd_mon_watch (s, t0);
	  nh_wd_hang_end ();
	}
    }
  pthread_mutex_unlock (&nh_wd.m);
  return NULL;
}

/* Start the monitor thread (from the main thread).  Return 0, or an
   errno value.  With the test flag no_monitor, start nothing.  */
int
nh_wd_monitor_start (void)
{
  if (!main_thread_p (current_thread))
    return EINVAL;
  nh_wd.main_id = pthread_self ();
  if (nh_wd.no_monitor || nh_wd_mon_running)
    return 0;
  int err = pthread_getcpuclockid (nh_wd.main_id, &nh_wd_main_cpu);
  if (err)
    return err;
  __atomic_store_n (&nh_wd.armed_span, UINT64_MAX, __ATOMIC_RELAXED);
  nh_wd_hang_start ();

  /* The waits are timed on CLOCK_MONOTONIC.  Nobody waits on the
     condition variable while the monitor is not running.  */
  pthread_condattr_t attr;
  pthread_condattr_init (&attr);
  pthread_condattr_setclock (&attr, CLOCK_MONOTONIC);
  pthread_cond_destroy (&nh_wd.cv);
  pthread_cond_init (&nh_wd.cv, &attr);
  pthread_condattr_destroy (&attr);

  /* The thread inherits a mask with every signal blocked.  */
  sigset_t all, old;
  sigfillset (&all);
  nh_wd_mon_stop = false;
  pthread_sigmask (SIG_SETMASK, &all, &old);
  err = pthread_create (&nh_wd_mon_thread, NULL, nh_wd_monitor_main, NULL);
  pthread_sigmask (SIG_SETMASK, &old, NULL);
  if (err)
    return err;
  /* Named here, not by the thread itself, so the name is there when
     this returns.  */
  pthread_setname_np (nh_wd_mon_thread, "nh-watchdog");
  nh_wd_mon_running = true;
  return 0;
}

/* Stop the monitor thread and wait for it; safe when it is not
   running.  */
void
nh_wd_monitor_stop (void)
{
  if (!nh_wd_mon_running)
    return;
  pthread_mutex_lock (&nh_wd.m);
  nh_wd_mon_stop = true;
  pthread_cond_broadcast (&nh_wd.cv);
  pthread_cond_broadcast (&nh_wd_test_cv);
  pthread_mutex_unlock (&nh_wd.m);
  pthread_join (nh_wd_mon_thread, NULL);
  nh_wd_mon_running = false;
  nh_wd_hang_stop ();
  /* pthread_join returns once the thread's tid is cleared, a moment
     before the kernel drops the task from /proc/self/task; wait for
     that (a few yields), so no caller still sees it.  */
  for (int i = 0;
       i < 10000 && syscall (SYS_tgkill, getpid (), nh_wd_mon_tid, 0) == 0;
       i++)
    sched_yield ();
}

/* Main thread, from `--stop' before it publishes waiting: the busy span
   is a hang, so the watch's end keeps its hang file.  The flag is set
   under nh_wd.m before the span ends, and the watch's end runs under
   nh_wd.m after it sees the span end, so it always sees the flag.  */
void
nh_wd_monitor_keep_hang (void)
{
  if (!nh_wd_mon_running)
    return;
  pthread_mutex_lock (&nh_wd.m);
  nh_wd_hang_keep = true;
  pthread_mutex_unlock (&nh_wd.m);
}

/* The monitor's part of a stall that closes now (main thread, after
   nh_wd_publish_waiting): the lateness, the main thread's CPU time
   since the monitor armed, and the missed requests, when the monitor
   armed for this span.  If it never got to the span, the lateness is
   at least the part of the stall past T/2.  */
void
nh_wd_monitor_span_close (struct nh_wd_stall *st)
{
  uint64_t s = __atomic_load_n (&nh_wd.span, __ATOMIC_RELAXED);
  /* A hang by the main thread's own clock, monitor or not.  */
  uint64_t hang = __atomic_load_n (&nh_wd.hang_ns, __ATOMIC_RELAXED);
  st->hang = hang && st->wall_ns >= hang;
  if (__atomic_load_n (&nh_wd.armed_span, __ATOMIC_ACQUIRE) == s)
    {
      st->late_ns = __atomic_load_n (&nh_wd.late_ns, __ATOMIC_RELAXED);
      st->cpu_ns = (nh_wd_clock_ns (CLOCK_THREAD_CPUTIME_ID)
		    - __atomic_load_n (&nh_wd.cpu_armed_ns, __ATOMIC_RELAXED));
      st->missed = __atomic_load_n (&nh_wd_mon_missed, __ATOMIC_RELAXED);
    }
  else if (nh_wd_mon_running
	   && !nh_wd_test_clock_p ())
    {
      uint64_t half = __atomic_load_n (&nh_wd.threshold_ns,
				       __ATOMIC_RELAXED) / 2;
      st->late_ns = st->wall_ns > half ? st->wall_ns - half : 0;
    }
}

#else  /* !__linux__ */

/* Not on this platform yet (macOS needs a run-loop observer).  */

int
nh_wd_monitor_start (void)
{
  return ENOSYS;
}

void
nh_wd_monitor_stop (void)
{
}

void
nh_wd_monitor_keep_hang (void)
{
}

void
nh_wd_monitor_span_close (struct nh_wd_stall *st)
{
  (void) st;
}

#endif /* !__linux__ */

DEFUN ("nidhogg-watchdog--test-interleavings",
       Fnidhogg_watchdog__test_interleavings,
       Snidhogg_watchdog__test_interleavings, 0, 0, 0,
       doc: /* Run the park protocol's interleaving model; for tests.
Every interleaving of the main thread's busy edge against the monitor's
park (the steps of nidhogg-watchdog-park.h, each side's stores going
through its own store buffer) is run to its end.  Return (:cases N
:lost K): N interleavings, K of them ending with the monitor still
waiting through the busy span (a lost wake-up).  */)
  (void)
{
  long cases, lost;
  nh_wd_park_model_run (&cases, &lost);
  return list4 (intern_c_string (":cases"), make_fixnum (cases),
		intern_c_string (":lost"), make_fixnum (lost));
}

DEFUN ("nidhogg-watchdog--test-hold-rename",
       Fnidhogg_watchdog__test_hold_rename,
       Snidhogg_watchdog__test_hold_rename, 1, 1, 0,
       doc: /* Test only: hold the monitor's hang-file write before its rename.
HOLD t: the next write waits between `fsync' and `rename'.  nil: release
it, and when one was held, wait until that write has finished (renamed,
or, its span ended, its temporary file removed).  `:held': wait until a
write is held.  `:ended': wait until the monitor watches no span before
the current one (its watch of an older span ended, and that span's hang
file is deleted).
The waits keep the main thread busy (no responsive wait) and give up
after 30 s.  Return t when a wait's condition was met, else nil.  */)
  (Lisp_Object hold)
{
#ifdef __linux__
  return nh_wd_test_hold_rename (hold);
#else
  (void) hold;
  return Qnil;
#endif
}

void
nh_wd_monitor_syms (void)
{
  defsubr (&Snidhogg_watchdog__test_interleavings);
  DEFSYM (QCheld, ":held");
  DEFSYM (QCended, ":ended");
  defsubr (&Snidhogg_watchdog__test_hold_rename);
}

#endif /* HAVE_NIDHOGG_WATCHDOG */

/* Nidhogg's worker threads: threads, queues, jobs, waits, wake fds.

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

/* The runtime (Task 3).  README.md, "Work off the main thread", is the
   user-facing description; the design's §4 is the contract.

   Workers.  Each worker (nh-io, nh-parse, nh-img, nh-blk, nh-save) has
   one mutex and up to two pools of threads: a normal-priority pool and,
   for the speculative workers (nh-parse, nh-img, nh-blk's prefetch), an
   idle-priority pool (SCHED_IDLE).  Each pool has a FIFO and a condition
   variable its idle threads wait on.  The mutex is held only for pointer
   swaps and counts.  Threads start lazily, at the first job of their
   pool, with every asynchronous signal blocked and the synchronous fatal
   ones (SIGSEGV, SIGBUS, SIGILL, SIGFPE, SIGABRT, SIGTRAP) left to Emacs's
   crash forwarding; they are named after their worker, detached, with a
   1 MB stack, and never exit (except an nh-blk thread that was started to
   grow the pool and found nothing to do).

   nh-blk grows: one thread, then one more whenever every thread has been
   busy for 50 ms and a job waits, up to four.  A class (directory reads,
   file reads, lookups) runs on at most two threads at once, and takes a
   thread only if that leaves one for every other class with nothing
   running, so two hung classes never starve the third.

   Priorities (PD-15).  A wait on a speculative job that has not started
   moves it to the worker's normal-priority pool.  A wait on one running
   at idle priority tries to raise its thread (pthread_setschedparam):
   unprivileged, that fails, is never tried again, and each such wait
   counts `boost-denied' and waits within its caller's bound.

   Completions.  A job that finishes (its `run' returned, or a worker
   with its own loop called nh_wk_complete) is marked done under the done
   mutex, which wakes every waiter, and, if it has a `finish', appended
   to the completion FIFO.  The producer writes the completion fd only
   when the FIFO goes from empty to non-empty.  The fd's callback (no
   Lisp) clears the fd, THEN re-checks the FIFO: if it holds a job and no
   drainer is pending, it pushes one drainer entry onto pending_funcalls.
   The drainer runs every `finish' in completion order at the next timer
   check (PD-6).  Clearing after the re-check would lose a wake-up: a job
   appended between the two would have its write cleared.

   Waits.  nh_wk_wait loops on the done condition variable against an
   absolute CLOCK_MONOTONIC deadline, re-checking the job's state after
   every return (spurious wake-ups, signals), and never goes through
   really_call_select.  It works on whichever thread holds the global
   lock; its phase goes onto the watchdog's stack only on the main
   thread (nidhogg-workers-phase.h).

   Inline mode (`nidhogg-workers--mode' = `inline') runs each job's `run'
   inside nh_wk_submit and its `finish' at the next timer check.

   With NH_WK_HARNESS (nidhogg/test/workers-harness.sh) the Lisp half is
   left out; the harness drives the queues and the wake protocol under
   ThreadSanitizer.  */

#ifndef NH_WK_HARNESS
# include <config.h>
#elif !defined _GNU_SOURCE
# define _GNU_SOURCE		/* pthread_setname_np, SCHED_IDLE */
#endif

#ifdef HAVE_NIDHOGG_WORKERS

#include <errno.h>
#include <fcntl.h>
#include <poll.h>
#include <sched.h>
#include <signal.h>
#include <stdio.h>
#include <string.h>
#include <time.h>
#include <unistd.h>
#include <sys/select.h>
#ifdef __linux__
# include <sys/eventfd.h>
# include <sys/syscall.h>
#endif

#include "nidhogg-workers.h"
#ifndef NH_WK_HARNESS
# include "nidhogg-workers-phase.h"
# include "process.h"
#else
# define NH_WK_PHASE_PUSH(ph, reason) ((void) (ph), (void) (reason), 0)
# define NH_WK_PHASE_POP(depth) ((void) (depth))
#endif

_Atomic bool nh_wk_dead;
bool nh_wk_inline;

enum
{
  NH_WK_STACK = 1024 * 1024,
  NH_WK_FD_MARGIN = 64,		/* no worker fd within this of FD_SETSIZE */
  NH_WK_CLASS_MAX = 2,		/* nh-blk threads one class may hold */
  NH_WK_BLK_MAX = 4,
  NH_WK_WAIT_BUCKETS = 9	/* < 1, 2, 4 ... 128 ms, and the rest */
};

#define NH_WK_MS INT64_C (1000000)
#define NH_WK_BLK_GROW_NS (50 * NH_WK_MS)

enum { NH_WK_POOL_NORMAL, NH_WK_POOL_IDLE, NH_WK_POOLS };

static const char *const nh_wk_names[NH_WK_N] = {
  "nh-io", "nh-parse", "nh-img", "nh-blk", "nh-save"
};

/* Used by the Lisp half and the harness's drivers only.  */
#define NH_WK_TEST_ONLY __attribute__ ((unused))

static NH_WK_TEST_ONLY const char *const nh_wk_fallback_names[NH_WK_FB_N] = {
  "dumping", "dead", "thread-create", "fd-budget", "boost-denied"
};

static NH_WK_TEST_ONLY const char *const nh_wk_pause_names[NH_WK_P_N] = {
  "none", "after-snapshot", "before-done", "after-read", "before-tokenize",
  "before-decode", "in-poll", "after-header", "before-write", "in-lookup"
};

static NH_WK_TEST_ONLY const char *const nh_wk_class_names[NH_WK_CL_N] = {
  "dir", "file", "dns"
};

/* 1 + the worker this thread belongs to; 0 off the workers.  */
static _Thread_local int nh_wk_self;

struct nh_wk_pool
{
  pthread_cond_t cv;		/* idle threads and a standby wait here */
  struct nh_wk_job *head, *tail;
  int threads, max, idle, running;
  bool standby;			/* a grown thread waits out the delay */
  uint64_t grow_delay_ns, last_dispatch_ns;
};

struct nh_wk_wst
{
  pthread_mutex_t m;
  struct nh_wk_pool pool[NH_WK_POOLS];
  bool class_caps;		/* nh-blk: per-class limits */
  int class_running[NH_WK_CL_N];
  int queued;			/* under m */
  _Atomic int64_t jobs, cancels, pauses, longest_ns;
  _Atomic int64_t stat[NH_WK_ST_N];
  _Atomic int64_t fallbacks[NH_WK_FB_N];
  _Atomic int64_t wait_hist[NH_WK_WAIT_BUCKETS];
};

static struct nh_wk_wst nh_wk_w[NH_WK_N];

/* The done mutex and condition variable: every waiter waits on it, and
   every change of a job to DONE or CANCELLED broadcasts it.  */
static pthread_mutex_t nh_wk_done_m = PTHREAD_MUTEX_INITIALIZER;
static pthread_cond_t nh_wk_done_cv;

/* Pause points (tests): the armed points, one bit each, and how many
   workers are parked at each.  */
static pthread_mutex_t nh_wk_pause_m = PTHREAD_MUTEX_INITIALIZER;
static pthread_cond_t nh_wk_pause_cv;
static _Atomic unsigned nh_wk_armed;
static int nh_wk_parked[NH_WK_P_N];

/* Phase ns (PD-3): deliver, json, filter, treesit, treesit-wait, image,
   then worker-wait by reason: display, image-pixels, detach, drain.  */
enum { NH_WK_PHASE_SLOTS = 10 };
static NH_WK_TEST_ONLY _Atomic int64_t nh_wk_phase_ns[NH_WK_PHASE_SLOTS];
static _Atomic int64_t nh_wk_deferrals[2];

/* The boost: 0 untried, 1 works, 2 denied (never tried again).  */
static _Atomic int nh_wk_boost_state;

static pthread_once_t nh_wk_once = PTHREAD_ONCE_INIT;

static uint64_t
nh_wk_now (void)
{
  struct timespec ts;
  clock_gettime (CLOCK_MONOTONIC, &ts);
  return (uint64_t) ts.tv_sec * 1000000000 + ts.tv_nsec;
}

static struct timespec
nh_wk_abs (uint64_t ns)
{
  return (struct timespec) { .tv_sec = ns / 1000000000,
			     .tv_nsec = ns % 1000000000 };
}

static void
nh_wk_cond_init (pthread_cond_t *cv)
{
  pthread_condattr_t attr;
  pthread_condattr_init (&attr);
  pthread_condattr_setclock (&attr, CLOCK_MONOTONIC);
  pthread_cond_init (cv, &attr);
  pthread_condattr_destroy (&attr);
}

static void
nh_wk_init_once (void)
{
  long ncpu = sysconf (_SC_NPROCESSORS_ONLN);
  int img = ncpu / 2 < 1 ? 1 : ncpu / 2 > 2 ? 2 : ncpu / 2;
  static const int normal_max[NH_WK_N] = { 1, 1, 1, NH_WK_BLK_MAX, 1 };
  int idle_max[NH_WK_N] = { 0, 1, img, 1, 0 };

  nh_wk_cond_init (&nh_wk_done_cv);
  nh_wk_cond_init (&nh_wk_pause_cv);
  for (int w = 0; w < NH_WK_N; w++)
    {
      struct nh_wk_wst *ws = &nh_wk_w[w];
      pthread_mutex_init (&ws->m, NULL);
      for (int p = 0; p < NH_WK_POOLS; p++)
	nh_wk_cond_init (&ws->pool[p].cv);
      ws->pool[NH_WK_POOL_NORMAL].max = normal_max[w];
      ws->pool[NH_WK_POOL_IDLE].max = idle_max[w];
    }
  nh_wk_w[NH_WK_BLK].class_caps = true;
  nh_wk_w[NH_WK_BLK].pool[NH_WK_POOL_NORMAL].grow_delay_ns
    = NH_WK_BLK_GROW_NS;
}

static void
nh_wk_init (void)
{
  pthread_once (&nh_wk_once, nh_wk_init_once);
}

/* Counters.  */

static void
nh_wk_count (_Atomic int64_t *c, int64_t n)
{
  atomic_fetch_add_explicit (c, n, memory_order_relaxed);
}

void
nh_wk_fallback (enum nh_wk_worker w, enum nh_wk_fallback reason)
{
  nh_wk_count (&nh_wk_w[w].fallbacks[reason], 1);
}

void
nh_wk_stat_add (enum nh_wk_worker w, enum nh_wk_stat s, int64_t n)
{
  nh_wk_count (&nh_wk_w[w].stat[s], n);
}

void
nh_wk_deferral_add (int area)
{
  if (0 <= area && area < 2)
    nh_wk_count (&nh_wk_deferrals[area], 1);
}

#ifndef NH_WK_HARNESS
static int
nh_wk_phase_slot (int ph, int reason)
{
  if (ph == NH_WK_PH_DELIVER)
    return 0;
  if (ph == NH_WK_PH_JSON)
    return 1;
  if (ph == NH_WK_PH_FILTER)
    return 2;
  if (ph == NH_WK_PH_TREESIT)
    return 3;
  if (ph == NH_WK_PH_TREESIT_WAIT)
    return 4;
  if (ph == NH_WK_PH_IMAGE)
    return 5;
  if (ph == NH_WK_PH_WORKER_WAIT)
    {
      if (reason == NH_WK_RS_DISPLAY)
	return 6;
      if (reason == NH_WK_RS_IMAGE_PIXELS)
	return 7;
      if (reason == NH_WK_RS_DETACH)
	return 8;
      if (reason == NH_WK_RS_DRAIN)
	return 9;
    }
  return -1;
}
#endif

void
nh_wk_phase_ns_add (int phase, int reason, int64_t ns)
{
#ifndef NH_WK_HARNESS
  int slot = nh_wk_phase_slot (phase, reason);
  if (slot >= 0)
    nh_wk_count (&nh_wk_phase_ns[slot], ns);
#else
  (void) phase;
  (void) reason;
  (void) ns;
#endif
}

static void
nh_wk_wait_counted (int w, int phase, int reason, uint64_t ns)
{
  int b = 0;
  for (uint64_t edge = NH_WK_MS; b < NH_WK_WAIT_BUCKETS - 1 && ns >= edge;
       edge *= 2)
    b++;
  nh_wk_count (&nh_wk_w[w].wait_hist[b], 1);
  nh_wk_phase_ns_add (phase, reason, ns);
}

/* Wake fds.  */

#ifdef __linux__
enum { NH_WK_WAKE_FDS = 1 };
#else
enum { NH_WK_WAKE_FDS = 2 };
#endif

bool
nh_wk_fd_budget_ok (int extra)
{
  /* The descriptors the next EXTRA opens would get: the lowest free
     ones.  Take them, look at the highest, give them back.  */
  int fds[16], n = 0, top = -1;
  bool ok = true;
  if (extra > 16)
    extra = 16;
  while (n < extra)
    {
      int fd = open ("/dev/null", O_RDONLY | O_CLOEXEC);
      if (fd < 0)
	{
	  ok = false;
	  break;
	}
      fds[n++] = fd;
      if (fd > top)
	top = fd;
    }
  while (n > 0)
    close (fds[--n]);
  return ok && top < FD_SETSIZE - NH_WK_FD_MARGIN;
}

int
nh_wk_wake_open (struct nh_wk_wake *w)
{
  w->rfd = w->wfd = -1;
  if (!nh_wk_fd_budget_ok (NH_WK_WAKE_FDS))
    return -1;
#ifdef __linux__
  int fd = eventfd (0, EFD_CLOEXEC | EFD_NONBLOCK);
  if (fd < 0)
    return -1;
  w->rfd = w->wfd = fd;
#else
  int fds[2];
# ifndef NH_WK_HARNESS
  if (emacs_pipe (fds) != 0)
    return -1;
# else
  if (pipe (fds) != 0)
    return -1;
  fcntl (fds[0], F_SETFD, FD_CLOEXEC);
  fcntl (fds[1], F_SETFD, FD_CLOEXEC);
# endif
  fcntl (fds[0], F_SETFL, fcntl (fds[0], F_GETFL) | O_NONBLOCK);
  fcntl (fds[1], F_SETFL, fcntl (fds[1], F_GETFL) | O_NONBLOCK);
  w->rfd = fds[0];
  w->wfd = fds[1];
#endif
  if (w->rfd >= FD_SETSIZE - NH_WK_FD_MARGIN
      || w->wfd >= FD_SETSIZE - NH_WK_FD_MARGIN)
    {
      nh_wk_wake_close (w);
      return -1;
    }
  return 0;
}

/* Async-signal-safe: one write, errno kept.  A full eventfd or pipe is
   already readable, so EAGAIN is fine.  */
void
nh_wk_wake_signal (struct nh_wk_wake *w)
{
  int err = errno;
  if (w->wfd >= 0)
    {
#ifdef __linux__
      uint64_t one = 1;
      if (write (w->wfd, &one, sizeof one) < 0)
	{
	}
#else
      char c = 0;
      if (write (w->wfd, &c, 1) < 0)
	{
	}
#endif
    }
  errno = err;
}

void
nh_wk_wake_clear (struct nh_wk_wake *w)
{
  if (w->rfd < 0)
    return;
#ifdef __linux__
  uint64_t v;
  if (read (w->rfd, &v, sizeof v) < 0)
    {
    }
#else
  char buf[64];
  while (read (w->rfd, buf, sizeof buf) > 0)
    ;
#endif
}

void
nh_wk_wake_close (struct nh_wk_wake *w)
{
  if (w->rfd >= 0)
    close (w->rfd);
  if (w->wfd >= 0 && w->wfd != w->rfd)
    close (w->wfd);
  w->rfd = w->wfd = -1;
}

static bool
nh_wk_wake_readable (struct nh_wk_wake *w)
{
  struct pollfd pfd = { .fd = w->rfd, .events = POLLIN };
  return poll (&pfd, 1, 0) > 0 && (pfd.revents & POLLIN);
}

/* A completion queue: a FIFO of jobs and the wake fd that says it went
   from empty to non-empty.  Producers: any thread.  Consumer: the
   thread that holds the Lisp lock (the harness's consumer thread).  */
struct nh_wk_cq
{
  pthread_mutex_t m;
  struct nh_wk_job *head, *tail;
  struct nh_wk_wake wake;
  /* The consumer's: the FIFO is non-empty after a clear.  */
  void (*arm) (struct nh_wk_cq *);
  /* Tests: runs between the clear and the re-check.  */
  void (*gap) (void);
};

/* Append J; write the fd if the FIFO was empty.  */
static void
nh_wk_cq_push (struct nh_wk_cq *q, struct nh_wk_job *j)
{
  j->next = NULL;
  pthread_mutex_lock (&q->m);
  bool was_empty = !q->head;
  if (q->tail)
    q->tail->next = j;
  else
    q->head = j;
  q->tail = j;
  pthread_mutex_unlock (&q->m);
  if (was_empty)
    nh_wk_wake_signal (&q->wake);
}

static struct nh_wk_job *
nh_wk_cq_pop (struct nh_wk_cq *q)
{
  pthread_mutex_lock (&q->m);
  struct nh_wk_job *j = q->head;
  if (j)
    {
      q->head = j->next;
      if (!q->head)
	q->tail = NULL;
      j->next = NULL;
    }
  pthread_mutex_unlock (&q->m);
  return j;
}

static bool
nh_wk_cq_nonempty (struct nh_wk_cq *q)
{
  pthread_mutex_lock (&q->m);
  bool r = q->head != NULL;
  pthread_mutex_unlock (&q->m);
  return r;
}

/* The fd is readable: clear it, then re-check the FIFO (in this order:
   see the commentary at the top).  */
static void
nh_wk_cq_ready (struct nh_wk_cq *q)
{
  nh_wk_wake_clear (&q->wake);
  if (q->gap)
    q->gap ();
  if (nh_wk_cq_nonempty (q))
    q->arm (q);
}

static void nh_wk_arm_drainer (struct nh_wk_cq *);

static struct nh_wk_cq nh_wk_done_q = {
  .m = PTHREAD_MUTEX_INITIALIZER, .wake = { -1, -1 },
  .arm = nh_wk_arm_drainer
};

/* Jobs.  */

void
nh_wk_job_ref (struct nh_wk_job *job)
{
  atomic_fetch_add (&job->refs, 1);
}

void
nh_wk_job_unref (struct nh_wk_job *job)
{
  if (atomic_fetch_sub (&job->refs, 1) == 1 && job->free)
    job->free (job);
}

static void
nh_wk_set_state (struct nh_wk_job *job, int state)
{
  pthread_mutex_lock (&nh_wk_done_m);
  atomic_store (&job->state, state);
  pthread_cond_broadcast (&nh_wk_done_cv);
  pthread_mutex_unlock (&nh_wk_done_m);
}

/* The job's run is over (or, for a worker with its own loop, its result
   is ready): mark it, wake its waiters, queue its finish.  The worker's
   reference moves to the completion FIFO, or is dropped.  */
void
nh_wk_complete (struct nh_wk_job *job)
{
  bool cancelled = atomic_load (&job->cancel);
  nh_wk_set_state (job, cancelled ? NH_WK_J_CANCELLED : NH_WK_J_DONE);
  if (!cancelled && job->finish && !atomic_load (&nh_wk_dead))
    nh_wk_cq_push (&nh_wk_done_q, job);
  else
    nh_wk_job_unref (job);
}

bool
nh_wk_on_worker (void)
{
  return nh_wk_self != 0;
}

/* Pools (all under the worker's mutex).  */

/* May JOB start on POOL now?  */
static bool
nh_wk_class_ok (struct nh_wk_wst *ws, int pool, struct nh_wk_job *job)
{
  if (!ws->class_caps || pool != NH_WK_POOL_NORMAL)
    return true;
  int cls = job->cls;
  if (ws->class_running[cls] >= NH_WK_CLASS_MAX)
    return false;
  int total = 0, empty_others = 0;
  for (int c = 0; c < NH_WK_CL_N; c++)
    {
      total += ws->class_running[c];
      if (c != cls && ws->class_running[c] == 0)
	empty_others++;
    }
  return total + 1 + empty_others <= ws->pool[pool].max;
}

static bool
nh_wk_pool_unlink (struct nh_wk_pool *p, struct nh_wk_job *job)
{
  struct nh_wk_job *prev = NULL;
  for (struct nh_wk_job *j = p->head; j; prev = j, j = j->next)
    if (j == job)
      {
	if (prev)
	  prev->next = j->next;
	else
	  p->head = j->next;
	if (p->tail == j)
	  p->tail = prev;
	j->next = NULL;
	return true;
      }
  return false;
}

static void
nh_wk_pool_append (struct nh_wk_pool *p, struct nh_wk_job *job, bool front)
{
  if (front)
    {
      job->next = p->head;
      p->head = job;
      if (!p->tail)
	p->tail = job;
    }
  else
    {
      job->next = NULL;
      if (p->tail)
	p->tail->next = job;
      else
	p->head = job;
      p->tail = job;
    }
}

static bool
nh_wk_has_startable (struct nh_wk_wst *ws, int pool)
{
  for (struct nh_wk_job *j = ws->pool[pool].head; j; j = j->next)
    if (nh_wk_class_ok (ws, pool, j))
      return true;
  return false;
}

static struct nh_wk_job *
nh_wk_pick (struct nh_wk_wst *ws, int pool)
{
  for (struct nh_wk_job *j = ws->pool[pool].head; j; j = j->next)
    if (nh_wk_class_ok (ws, pool, j))
      {
	nh_wk_pool_unlink (&ws->pool[pool], j);
	return j;
      }
  return NULL;
}

/* Should POOL get one more thread?  If so, count it now; *STANDBY says
   it must first wait out the pool's growth delay.  */
static bool
nh_wk_need_thread (struct nh_wk_wst *ws, int pool, bool *standby)
{
  struct nh_wk_pool *p = &ws->pool[pool];
  if (p->idle > 0 || p->standby || p->threads >= p->max
      || !nh_wk_has_startable (ws, pool))
    return false;
  *standby = p->grow_delay_ns > 0 && p->threads > 0;
  p->standby = *standby;
  p->threads++;
  return true;
}

/* Threads.  */

static void *nh_wk_thread_main (void *);

/* The signals a worker blocks: all but the synchronous fatal ones.  */
static void
nh_wk_worker_sigmask (sigset_t *set)
{
  static const int fatal[] = { SIGSEGV, SIGBUS, SIGILL, SIGFPE, SIGABRT,
			       SIGTRAP };
  sigfillset (set);
  for (size_t i = 0; i < sizeof fatal / sizeof *fatal; i++)
    sigdelset (set, fatal[i]);
}

/* Start the thread nh_wk_need_thread counted.  On failure uncount it.  */
static bool
nh_wk_start_thread (int w, int pool, bool standby)
{
  pthread_attr_t attr;
  pthread_t t;
  sigset_t set, old;
  intptr_t arg = (w << 2) | (pool << 1) | standby;

  pthread_attr_init (&attr);
  pthread_attr_setdetachstate (&attr, PTHREAD_CREATE_DETACHED);
  pthread_attr_setstacksize (&attr, NH_WK_STACK);
  nh_wk_worker_sigmask (&set);
  pthread_sigmask (SIG_SETMASK, &set, &old);
  int err = pthread_create (&t, &attr, nh_wk_thread_main, (void *) arg);
  pthread_sigmask (SIG_SETMASK, &old, NULL);
  pthread_attr_destroy (&attr);
  if (!err)
    return true;

  struct nh_wk_wst *ws = &nh_wk_w[w];
  pthread_mutex_lock (&ws->m);
  ws->pool[pool].threads--;
  if (standby)
    ws->pool[pool].standby = false;
  pthread_mutex_unlock (&ws->m);
  nh_wk_fallback (w, NH_WK_FB_THREAD);
  return false;
}

/* A grown thread first waits until every other thread of its pool has
   been busy for the growth delay.  Return false if it is not needed
   after all (it then leaves).  Called and returns with the mutex.  */
static bool
nh_wk_standby (struct nh_wk_wst *ws, int pool)
{
  struct nh_wk_pool *p = &ws->pool[pool];
  for (;;)
    {
      if (atomic_load (&nh_wk_dead) || p->idle > 0
	  || !nh_wk_has_startable (ws, pool))
	{
	  p->standby = false;
	  return false;
	}
      uint64_t at = p->last_dispatch_ns + p->grow_delay_ns;
      if (p->running == p->threads - 1 && nh_wk_now () >= at)
	{
	  p->standby = false;
	  return true;
	}
      struct timespec ts = nh_wk_abs (at > nh_wk_now () ? at
				      : nh_wk_now () + NH_WK_MS);
      pthread_cond_timedwait (&p->cv, &ws->m, &ts);
    }
}

static void
nh_wk_set_name (int w)
{
#if defined __APPLE__
  pthread_setname_np (nh_wk_names[w]);
#elif defined __linux__
  pthread_setname_np (pthread_self (), nh_wk_names[w]);
#else
  (void) w;
#endif
}

static void
nh_wk_set_idle_priority (void)
{
#ifdef SCHED_IDLE
  struct sched_param sp = { 0 };
  pthread_setschedparam (pthread_self (), SCHED_IDLE, &sp);
#endif
}

static void *
nh_wk_thread_main (void *arg)
{
  intptr_t a = (intptr_t) arg;
  int w = a >> 2, pool = (a >> 1) & 1;
  bool standby = a & 1;
  struct nh_wk_wst *ws = &nh_wk_w[w];
  struct nh_wk_pool *p = &ws->pool[pool];

  nh_wk_self = w + 1;
  nh_wk_set_name (w);
  if (pool == NH_WK_POOL_IDLE)
    nh_wk_set_idle_priority ();

  pthread_mutex_lock (&ws->m);
  if (standby && !nh_wk_standby (ws, pool))
    {
      p->threads--;
      pthread_mutex_unlock (&ws->m);
      return NULL;
    }
  for (;;)
    {
      struct nh_wk_job *j = atomic_load (&nh_wk_dead) ? NULL
	: nh_wk_pick (ws, pool);
      if (!j)
	{
	  p->idle++;
	  pthread_cond_wait (&p->cv, &ws->m);
	  p->idle--;
	  continue;
	}
      j->runner = pthread_self ();
      atomic_store (&j->state, NH_WK_J_RUNNING);
      ws->queued--;
      p->running++;
      if (ws->class_caps && pool == NH_WK_POOL_NORMAL)
	ws->class_running[j->cls]++;
      p->last_dispatch_ns = nh_wk_now ();
      bool more_standby = false;
      bool more = nh_wk_need_thread (ws, pool, &more_standby);
      pthread_mutex_unlock (&ws->m);

      if (more)
	nh_wk_start_thread (w, pool, more_standby);
      uint64_t t0 = nh_wk_now ();
      j->run (j);
      int64_t ns = nh_wk_now () - t0;

      pthread_mutex_lock (&ws->m);
      p->running--;
      if (ws->class_caps && pool == NH_WK_POOL_NORMAL)
	ws->class_running[j->cls]--;
      if (ns > atomic_load_explicit (&ws->longest_ns, memory_order_relaxed))
	atomic_store_explicit (&ws->longest_ns, ns, memory_order_relaxed);
      pthread_mutex_unlock (&ws->m);
      nh_wk_complete (j);
      pthread_mutex_lock (&ws->m);
    }
}

/* Submit, wait, cancel.  */

static void nh_wk_arm_inline (void);

#ifndef NH_WK_HARNESS
static void nh_wk_done_ready (int fd, void *data);
#endif

/* Open the completion fd (and, in Emacs, select on it).  */
static bool
nh_wk_done_open (void)
{
  if (nh_wk_done_q.wake.rfd >= 0)
    return true;
  if (nh_wk_wake_open (&nh_wk_done_q.wake) < 0)
    return false;
#ifndef NH_WK_HARNESS
  add_non_keyboard_read_fd (nh_wk_done_q.wake.rfd, nh_wk_done_ready, NULL);
#endif
  return true;
}

bool
nh_wk_submit (struct nh_wk_job *job)
{
  NH_WK_ASSERT (0 <= job->w && job->w < NH_WK_N);
  NH_WK_ASSERT (!nh_wk_on_worker ());
  int w = job->w;
  struct nh_wk_wst *ws = &nh_wk_w[w];
  nh_wk_init ();
  if (ws->class_caps)
    NH_WK_ASSERT (0 <= job->cls && job->cls < NH_WK_CL_N);

  if (atomic_load (&nh_wk_dead))
    {
      nh_wk_fallback (w, NH_WK_FB_DEAD);
      return false;
    }
#ifndef NH_WK_HARNESS
  /* Never while dumping.  A --daemon's fork happens in main, before any
     Lisp runs, so no job can come before it.  */
  if (will_dump_p ())
    {
      nh_wk_fallback (w, NH_WK_FB_DUMPING);
      return false;
    }
#endif
  if (!nh_wk_inline && !nh_wk_done_open ())
    {
      nh_wk_fallback (w, NH_WK_FB_FD_BUDGET);
      return false;
    }

  nh_wk_job_ref (job);		/* the worker's */
  job->next = NULL;
  job->submitted_ns = nh_wk_now ();
  atomic_store (&job->state, NH_WK_J_QUEUED);
  nh_wk_count (&ws->jobs, 1);

  if (nh_wk_inline)
    {
      job->runner = pthread_self ();
      atomic_store (&job->state, NH_WK_J_RUNNING);
      job->run (job);
      nh_wk_complete (job);
      if (job->finish)
	nh_wk_arm_inline ();
      return true;
    }

  int pool = (job->speculative && ws->pool[NH_WK_POOL_IDLE].max > 0
	      ? NH_WK_POOL_IDLE : NH_WK_POOL_NORMAL);
  struct nh_wk_pool *p = &ws->pool[pool];
  bool standby;
  pthread_mutex_lock (&ws->m);
  nh_wk_pool_append (p, job, false);
  ws->queued++;
  bool start = nh_wk_need_thread (ws, pool, &standby);
  pthread_cond_broadcast (&p->cv);
  pthread_mutex_unlock (&ws->m);

  if (start && !nh_wk_start_thread (w, pool, standby))
    {
      /* With no thread at all the job would never run.  */
      pthread_mutex_lock (&ws->m);
      bool orphan = p->threads == 0 && nh_wk_pool_unlink (p, job);
      if (orphan)
	ws->queued--;
      pthread_mutex_unlock (&ws->m);
      if (orphan)
	{
	  atomic_store (&job->state, NH_WK_J_CANCELLED);
	  atomic_fetch_sub (&job->refs, 1);
	  return false;
	}
    }
  return true;
}

/* Try to raise the idle-priority thread T for a wait.  */
static bool
nh_wk_boost (int w, pthread_t t)
{
#ifdef SCHED_IDLE
  if (atomic_load (&nh_wk_boost_state) != 2)
    {
      struct sched_param sp = { 0 };
      if (pthread_setschedparam (t, SCHED_OTHER, &sp) == 0)
	{
	  atomic_store (&nh_wk_boost_state, 1);
	  return true;
	}
      atomic_store (&nh_wk_boost_state, 2);
    }
#else
  (void) t;
#endif
  nh_wk_fallback (w, NH_WK_FB_BOOST_DENIED);
  return false;
}

static void
nh_wk_unboost (pthread_t t)
{
#ifdef SCHED_IDLE
  struct sched_param sp = { 0 };
  pthread_setschedparam (t, SCHED_IDLE, &sp);
#else
  (void) t;
#endif
}

/* A wait on JOB begins: move it to the normal-priority pool if it has
   not started on an idle one; return true (and its thread in *T) if
   its idle-priority thread was raised.  */
static bool
nh_wk_promote (struct nh_wk_job *job, pthread_t *t)
{
  int w = job->w;
  struct nh_wk_wst *ws = &nh_wk_w[w];
  bool start = false, standby = false, running = false;

  pthread_mutex_lock (&ws->m);
  if (job->speculative)
    {
      int st = atomic_load (&job->state);
      if (st == NH_WK_J_QUEUED
	  && nh_wk_pool_unlink (&ws->pool[NH_WK_POOL_IDLE], job))
	{
	  job->speculative = false;
	  nh_wk_pool_append (&ws->pool[NH_WK_POOL_NORMAL], job, true);
	  start = nh_wk_need_thread (ws, NH_WK_POOL_NORMAL, &standby);
	  pthread_cond_broadcast (&ws->pool[NH_WK_POOL_NORMAL].cv);
	}
      else if (st == NH_WK_J_RUNNING)
	{
	  running = true;
	  *t = job->runner;
	}
    }
  pthread_mutex_unlock (&ws->m);

  if (start)
    nh_wk_start_thread (w, NH_WK_POOL_NORMAL, standby);
  return running && nh_wk_boost (w, *t);
}

/* Tests: the pause point a wait releases when it begins (-1: none).  */
static _Atomic int nh_wk_release_on_wait = -1;
static void nh_wk_release (int p);

int
nh_wk_wait (struct nh_wk_job *job, int64_t timeout_ns, int phase, int reason)
{
  int st = atomic_load (&job->state);
  if (st == NH_WK_J_DONE)
    return 1;
  if (st == NH_WK_J_CANCELLED || atomic_load (&nh_wk_dead))
    return -1;
  NH_WK_ASSERT (!nh_wk_on_worker ());

  uint64_t t0 = nh_wk_now ();
  uint64_t deadline = t0 + (timeout_ns > 0 ? timeout_ns : 0);
  int depth = NH_WK_PHASE_PUSH (phase, reason);
  pthread_t runner = pthread_self ();
  bool boosted = nh_wk_promote (job, &runner);
  int rel = atomic_exchange (&nh_wk_release_on_wait, -1);
  if (rel >= 0)
    nh_wk_release (rel);

  pthread_mutex_lock (&nh_wk_done_m);
  for (;;)
    {
      st = atomic_load (&job->state);
      if (st == NH_WK_J_DONE || st == NH_WK_J_CANCELLED
	  || atomic_load (&nh_wk_dead))
	break;
      uint64_t now = nh_wk_now ();
      if (now >= deadline)
	break;
      /* Returns on a broadcast, the deadline, or for no reason at all
	 (a signal may cause that): the loop decides.  */
      struct timespec ts = nh_wk_abs (deadline);
      pthread_cond_timedwait (&nh_wk_done_cv, &nh_wk_done_m, &ts);
    }
  pthread_mutex_unlock (&nh_wk_done_m);

  if (boosted)
    nh_wk_unboost (runner);
  NH_WK_PHASE_POP (depth);
  nh_wk_wait_counted (job->w, phase, reason, nh_wk_now () - t0);
  return (st == NH_WK_J_DONE ? 1
	  : st == NH_WK_J_CANCELLED || atomic_load (&nh_wk_dead) ? -1 : 0);
}

void
nh_wk_cancel (struct nh_wk_job *job)
{
  int w = job->w;
  struct nh_wk_wst *ws = &nh_wk_w[w];
  bool unlinked = false, live;

  atomic_store (&job->cancel, true);
  if (atomic_load (&nh_wk_dead))
    return;
  pthread_mutex_lock (&ws->m);
  int st = atomic_load (&job->state);
  live = st == NH_WK_J_QUEUED || st == NH_WK_J_RUNNING;
  if (st == NH_WK_J_QUEUED)
    for (int p = 0; p < NH_WK_POOLS && !unlinked; p++)
      unlinked = nh_wk_pool_unlink (&ws->pool[p], job);
  if (unlinked)
    ws->queued--;
  pthread_mutex_unlock (&ws->m);
  if (live)
    nh_wk_count (&ws->cancels, 1);
  if (unlinked)
    {
      nh_wk_set_state (job, NH_WK_J_CANCELLED);
      nh_wk_job_unref (job);
    }
}

/* Pause points.  */

void
nh_wk_pause_point (enum nh_wk_pause p)
{
  unsigned bit = 1u << p;
  if (!(atomic_load (&nh_wk_armed) & bit) || !nh_wk_on_worker ())
    return;
  nh_wk_count (&nh_wk_w[nh_wk_self - 1].pauses, 1);
  pthread_mutex_lock (&nh_wk_pause_m);
  nh_wk_parked[p]++;
  pthread_cond_broadcast (&nh_wk_pause_cv);
  while ((atomic_load (&nh_wk_armed) & bit) && !atomic_load (&nh_wk_dead))
    pthread_cond_wait (&nh_wk_pause_cv, &nh_wk_pause_m);
  nh_wk_parked[p]--;
  pthread_cond_broadcast (&nh_wk_pause_cv);
  pthread_mutex_unlock (&nh_wk_pause_m);
}

static NH_WK_TEST_ONLY void
nh_wk_arm_pause (int p)
{
  nh_wk_init ();
  atomic_fetch_or (&nh_wk_armed, 1u << p);
}

/* Release P, or every point if P < 0.  */
static void
nh_wk_release (int p)
{
  nh_wk_init ();
  pthread_mutex_lock (&nh_wk_pause_m);
  atomic_fetch_and (&nh_wk_armed, p < 0 ? 0u : ~(1u << p));
  pthread_cond_broadcast (&nh_wk_pause_cv);
  pthread_mutex_unlock (&nh_wk_pause_m);
}

/* Wait at most MS until N workers are parked at P; return how many
   are, or -1 if fewer.  */
static NH_WK_TEST_ONLY int
nh_wk_await_paused (int p, int64_t ms, int n)
{
  nh_wk_init ();
  uint64_t deadline = nh_wk_now () + ms * NH_WK_MS;
  pthread_mutex_lock (&nh_wk_pause_m);
  while (nh_wk_parked[p] < n && nh_wk_now () < deadline)
    {
      struct timespec ts = nh_wk_abs (deadline);
      pthread_cond_timedwait (&nh_wk_pause_cv, &nh_wk_pause_m, &ts);
    }
  int r = nh_wk_parked[p];
  pthread_mutex_unlock (&nh_wk_pause_m);
  return r >= n ? r : -1;
}

/* The wake protocol's checks (tests and the harness).  */

static _Atomic bool nh_wk_race_armed;

static void
nh_wk_race_arm (struct nh_wk_cq *q)
{
  (void) q;
  atomic_store (&nh_wk_race_armed, true);
}

static struct nh_wk_cq nh_wk_race_q = {
  .m = PTHREAD_MUTEX_INITIALIZER, .wake = { -1, -1 },
  .arm = nh_wk_race_arm
};

static void
nh_wk_race_reset (void)
{
  while (nh_wk_cq_pop (&nh_wk_race_q))
    ;
  nh_wk_wake_clear (&nh_wk_race_q.wake);
  atomic_store (&nh_wk_race_armed, false);
}

/* The fd follows the FIFO: readable after a push, cleared by the
   callback, which arms the consumer only while the FIFO holds a job.  */
static NH_WK_TEST_ONLY bool
nh_wk_race_level (void)
{
  struct nh_wk_job a = { 0 }, b = { 0 };
  bool ok = !nh_wk_wake_readable (&nh_wk_race_q.wake);
  nh_wk_cq_push (&nh_wk_race_q, &a);
  ok &= nh_wk_wake_readable (&nh_wk_race_q.wake);
  nh_wk_cq_push (&nh_wk_race_q, &b);
  ok &= nh_wk_wake_readable (&nh_wk_race_q.wake);
  nh_wk_cq_ready (&nh_wk_race_q);
  ok &= !nh_wk_wake_readable (&nh_wk_race_q.wake);
  ok &= atomic_exchange (&nh_wk_race_armed, false);
  ok &= nh_wk_cq_pop (&nh_wk_race_q) == &a;
  ok &= nh_wk_cq_pop (&nh_wk_race_q) == &b;
  nh_wk_cq_ready (&nh_wk_race_q);
  ok &= !nh_wk_wake_readable (&nh_wk_race_q.wake);
  ok &= !atomic_load (&nh_wk_race_armed);
  nh_wk_race_reset ();
  return ok;
}

/* The gap race: a helper appends a job exactly between the callback's
   clear and its re-check.  */
static pthread_mutex_t nh_wk_gap_m = PTHREAD_MUTEX_INITIALIZER;
static pthread_cond_t nh_wk_gap_cv;
static int nh_wk_gap_go;		/* 1 push, -1 stop; under gap_m */
static bool nh_wk_gap_done;
static struct nh_wk_job nh_wk_gap_job;

static void *
nh_wk_gap_helper (void *arg)
{
  (void) arg;
  pthread_mutex_lock (&nh_wk_gap_m);
  for (;;)
    {
      while (nh_wk_gap_go == 0)
	pthread_cond_wait (&nh_wk_gap_cv, &nh_wk_gap_m);
      if (nh_wk_gap_go < 0)
	break;
      nh_wk_gap_go = 0;
      pthread_mutex_unlock (&nh_wk_gap_m);
      nh_wk_cq_push (&nh_wk_race_q, &nh_wk_gap_job);
      pthread_mutex_lock (&nh_wk_gap_m);
      nh_wk_gap_done = true;
      pthread_cond_broadcast (&nh_wk_gap_cv);
    }
  pthread_mutex_unlock (&nh_wk_gap_m);
  return NULL;
}

static void
nh_wk_gap_hook (void)
{
  pthread_mutex_lock (&nh_wk_gap_m);
  nh_wk_gap_done = false;
  nh_wk_gap_go = 1;
  pthread_cond_broadcast (&nh_wk_gap_cv);
  while (!nh_wk_gap_done)
    pthread_cond_wait (&nh_wk_gap_cv, &nh_wk_gap_m);
  pthread_mutex_unlock (&nh_wk_gap_m);
}

/* N gap races; return how many lost the wake-up (-1: no helper).  */
static NH_WK_TEST_ONLY int
nh_wk_race_gap (int n)
{
  pthread_t helper;
  struct nh_wk_job a = { 0 };
  int lost = 0;
  sigset_t set, old;

  nh_wk_gap_go = 0;
  sigfillset (&set);
  pthread_sigmask (SIG_SETMASK, &set, &old);
  int err = pthread_create (&helper, NULL, nh_wk_gap_helper, NULL);
  pthread_sigmask (SIG_SETMASK, &old, NULL);
  if (err)
    return -1;
  for (int i = 0; i < n; i++)
    {
      /* A job lands and is drained; its write is still in the fd.  */
      nh_wk_cq_push (&nh_wk_race_q, &a);
      nh_wk_cq_pop (&nh_wk_race_q);
      atomic_store (&nh_wk_race_armed, false);
      nh_wk_race_q.gap = nh_wk_gap_hook;
      nh_wk_cq_ready (&nh_wk_race_q);
      nh_wk_race_q.gap = NULL;
      if (nh_wk_cq_nonempty (&nh_wk_race_q)
	  && !nh_wk_wake_readable (&nh_wk_race_q.wake)
	  && !atomic_load (&nh_wk_race_armed))
	lost++;
      nh_wk_race_reset ();
    }
  pthread_mutex_lock (&nh_wk_gap_m);
  nh_wk_gap_go = -1;
  pthread_cond_broadcast (&nh_wk_gap_cv);
  pthread_mutex_unlock (&nh_wk_gap_m);
  pthread_join (helper, NULL);
  return lost;
}

/* Free-running: PRODUCERS threads push N jobs in all; this thread
   consumes them through the fd.  A wake-up that never comes shows as a
   1 s poll timeout with jobs queued and nothing armed (then counted
   and recovered).  Return the lost wake-ups (-1: no producer).  */
struct nh_wk_stress
{
  struct nh_wk_job *jobs;
  int from, to;
};

static void *
nh_wk_stress_producer (void *arg)
{
  struct nh_wk_stress *s = arg;
  for (int i = s->from; i < s->to; i++)
    {
      nh_wk_cq_push (&nh_wk_race_q, &s->jobs[i]);
      if (i % 7 == 0)
	sched_yield ();
    }
  return NULL;
}

static NH_WK_TEST_ONLY int
nh_wk_race_stress (int n, int producers)
{
  enum { MAXP = 4 };
  pthread_t t[MAXP];
  struct nh_wk_stress s[MAXP];
  struct nh_wk_job *jobs = calloc (n, sizeof *jobs);
  int started = 0, consumed = 0, lost = 0;
  sigset_t set, old;

  if (!jobs)
    return -1;
  if (producers > MAXP)
    producers = MAXP;
  sigfillset (&set);
  pthread_sigmask (SIG_SETMASK, &set, &old);
  for (int i = 0; i < producers; i++)
    {
      s[i] = (struct nh_wk_stress) { jobs, n * i / producers,
				     n * (i + 1) / producers };
      if (pthread_create (&t[i], NULL, nh_wk_stress_producer, &s[i]) == 0)
	started++;
      else
	{
	  consumed += s[i].to - s[i].from;
	  s[i].jobs = NULL;
	}
    }
  pthread_sigmask (SIG_SETMASK, &old, NULL);
  if (!started)
    {
      free (jobs);
      return -1;
    }
  while (consumed < n)
    {
      struct pollfd pfd = { .fd = nh_wk_race_q.wake.rfd, .events = POLLIN };
      if (poll (&pfd, 1, 1000) > 0)
	nh_wk_cq_ready (&nh_wk_race_q);
      else if (nh_wk_cq_nonempty (&nh_wk_race_q)
	       && !atomic_load (&nh_wk_race_armed))
	{
	  lost++;
	  atomic_store (&nh_wk_race_armed, true);
	}
      if (atomic_exchange (&nh_wk_race_armed, false))
	while (nh_wk_cq_pop (&nh_wk_race_q))
	  consumed++;
    }
  for (int i = 0; i < producers; i++)
    if (s[i].jobs)
      pthread_join (t[i], NULL);
  nh_wk_race_reset ();
  free (jobs);
  return lost;
}

static NH_WK_TEST_ONLY bool
nh_wk_race_open (void)
{
  nh_wk_init ();
  if (nh_wk_race_q.wake.rfd >= 0)
    return true;
  nh_wk_cond_init (&nh_wk_gap_cv);
  return nh_wk_wake_open (&nh_wk_race_q.wake) == 0;
}

/* The drainer.  */

#ifdef NH_WK_HARNESS

/* The harness's consumer drains when this is set.  */
static _Atomic bool nh_wk_harness_armed;

static void
nh_wk_arm_drainer (struct nh_wk_cq *q)
{
  (void) q;
  atomic_store (&nh_wk_harness_armed, true);
}

static void
nh_wk_arm_inline (void)
{
  atomic_store (&nh_wk_harness_armed, true);
}

#else /* not NH_WK_HARNESS */

/* A drainer entry is on pending_funcalls (lock holder only).  */
static bool nh_wk_drainer_pending;
/* The job whose finish runs now, for the drainer's unwind.  */
static struct nh_wk_job *nh_wk_draining;

static void
nh_wk_arm_drainer (struct nh_wk_cq *q)
{
  (void) q;
  if (nh_wk_drainer_pending)
    return;
  nh_wk_drainer_pending = true;
  pending_funcalls = Fcons (list1 (Qnidhogg_workers__drain),
			    pending_funcalls);
}

static void
nh_wk_arm_inline (void)
{
  nh_wk_arm_drainer (&nh_wk_done_q);
}

/* The completion fd is readable (an add_read_fd callback: no Lisp).  */
static void
nh_wk_done_ready (int fd, void *data)
{
  (void) fd;
  (void) data;
  nh_wk_cq_ready (&nh_wk_done_q);
}

/* A finish that exits non-locally leaves the rest for another drainer.  */
static void
nh_wk_drain_unwind (void)
{
  struct nh_wk_job *j = nh_wk_draining;
  nh_wk_draining = NULL;
  if (j)
    nh_wk_job_unref (j);
  if (nh_wk_cq_nonempty (&nh_wk_done_q))
    nh_wk_arm_drainer (&nh_wk_done_q);
}

DEFUN ("nidhogg-workers--drain", Fnidhogg_workers__drain,
       Snidhogg_workers__drain, 0, 0, 0,
       doc: /* Run the finish functions of the jobs that have landed.
They run in completion order.  Internal: the completion fd's callback
puts this on `pending_funcalls', so it runs at the next timer check.  */)
  (void)
{
  specpdl_ref count = SPECPDL_INDEX ();
  struct nh_wk_job *j;

  nh_wk_drainer_pending = false;
  record_unwind_protect_void (nh_wk_drain_unwind);
  while ((j = nh_wk_cq_pop (&nh_wk_done_q)))
    {
      nh_wk_draining = j;
      if (!atomic_load (&j->cancel) && !atomic_load (&nh_wk_dead))
	j->finish (j);
      nh_wk_draining = NULL;
      nh_wk_job_unref (j);
    }
  return unbind_to (count, Qnil);
}

/* Statistics.  */

static Lisp_Object
nh_wk_kw (const char *name)
{
  return intern_c_string (name);
}

static Lisp_Object
nh_wk_ms (int64_t ns)
{
  return make_float (ns / 1e6);
}

static Lisp_Object
nh_wk_worker_stats (int w, bool reset)
{
  struct nh_wk_wst *ws = &nh_wk_w[w];
  int queued, running = 0;
  pthread_mutex_lock (&ws->m);
  queued = ws->queued;
  for (int p = 0; p < NH_WK_POOLS; p++)
    running += ws->pool[p].running;
  pthread_mutex_unlock (&ws->m);
  Lisp_Object hist = make_nil_vector (NH_WK_WAIT_BUCKETS);
  for (int b = 0; b < NH_WK_WAIT_BUCKETS; b++)
    ASET (hist, b, make_int (reset ? atomic_exchange (&ws->wait_hist[b], 0)
			     : atomic_load (&ws->wait_hist[b])));
  Lisp_Object fallbacks = Qnil;
  for (int r = NH_WK_FB_N - 1; r >= 0; r--)
    {
      int64_t n = (reset ? atomic_exchange (&ws->fallbacks[r], 0)
		   : atomic_load (&ws->fallbacks[r]));
      if (n)
	fallbacks = Fcons (Fcons (intern_c_string (nh_wk_fallback_names[r]),
				  make_int (n)),
			   fallbacks);
    }
#define NH_WK_TAKE(c) make_int (reset ? atomic_exchange (&(c), 0)	\
				 : atomic_load (&(c)))
  Lisp_Object jobs = NH_WK_TAKE (ws->jobs);
  Lisp_Object cancels = NH_WK_TAKE (ws->cancels);
  Lisp_Object pauses = NH_WK_TAKE (ws->pauses);
  Lisp_Object backpressure = NH_WK_TAKE (ws->stat[NH_WK_ST_BACKPRESSURE]);
  Lisp_Object slices = NH_WK_TAKE (ws->stat[NH_WK_ST_SLICES]);
#undef NH_WK_TAKE
  Lisp_Object longest = nh_wk_ms (reset ? atomic_exchange (&ws->longest_ns, 0)
				  : atomic_load (&ws->longest_ns));
  return listn (22,
		nh_wk_kw (":jobs"), jobs,
		nh_wk_kw (":queued"), make_int (queued),
		nh_wk_kw (":queued-bytes"),
		make_int (atomic_load (&ws->stat[NH_WK_ST_QUEUED_BYTES])),
		nh_wk_kw (":running"), make_int (running),
		nh_wk_kw (":wait-ms"), hist,
		nh_wk_kw (":fallbacks"), fallbacks,
		nh_wk_kw (":cancels"), cancels,
		nh_wk_kw (":pauses"), pauses,
		nh_wk_kw (":backpressure"), backpressure,
		nh_wk_kw (":slices"), slices,
		nh_wk_kw (":longest-ms"), longest);
}

DEFUN ("nidhogg-workers-stats", Fnidhogg_workers_stats,
       Snidhogg_workers_stats, 0, 1, 0,
       doc: /* Return the worker threads' counters; with RESET, zero them.
The value is a plist: for each worker (`:nh-io', `:nh-parse', `:nh-img',
`:nh-blk', `:nh-save') a plist of `:jobs' (submitted), `:queued' and
`:running' (now), `:queued-bytes', `:wait-ms' (a vector: how many waits
on its jobs took under 1, 2, 4 ... 128 ms, and longer), `:fallbacks'
\(an alist of reasons a job took today's path, or a wait could not raise
an idle thread: dumping, dead, thread-create, fd-budget, boost-denied),
`:cancels', `:pauses' (test pause points hit), `:backpressure',
`:slices' and `:longest-ms' (the longest job); then `:phase-ms', the
milliseconds spent in each phase (`worker-wait' by reason), `:blk-threads'
\(nh-blk's normal-priority threads) and `:deferrals'.  RESET zeroes the
cumulative counters after reading them, not `:queued', `:running',
`:queued-bytes' or `:blk-threads'.  */)
  (Lisp_Object reset)
{
  bool r = !NILP (reset);
  static const char *const phases[] = {
    "deliver", "json", "filter", "treesit", "treesit-wait", "image"
  };
  static const char *const reasons[] = {
    "display", "image-pixels", "detach", "drain"
  };
  int ph = 6;

  nh_wk_init ();
  Lisp_Object waits = Qnil;
  for (int i = 3; i >= 0; i--)
    waits = Fcons (intern_c_string (reasons[i]),
		   Fcons (nh_wk_ms (r ? atomic_exchange (&nh_wk_phase_ns[ph + i], 0)
				    : atomic_load (&nh_wk_phase_ns[ph + i])),
			  waits));
  Lisp_Object phase_ms = list2 (intern_c_string ("worker-wait"), waits);
  for (int i = ph - 1; i >= 0; i--)
    phase_ms = Fcons (intern_c_string (phases[i]),
		      Fcons (nh_wk_ms (r ? atomic_exchange (&nh_wk_phase_ns[i], 0)
				       : atomic_load (&nh_wk_phase_ns[i])),
			     phase_ms));

  struct nh_wk_wst *blk = &nh_wk_w[NH_WK_BLK];
  pthread_mutex_lock (&blk->m);
  int blk_threads = blk->pool[NH_WK_POOL_NORMAL].threads;
  pthread_mutex_unlock (&blk->m);

  Lisp_Object deferrals
    = list4 (intern_c_string ("treesit"),
	     make_int (r ? atomic_exchange (&nh_wk_deferrals[0], 0)
		       : atomic_load (&nh_wk_deferrals[0])),
	     intern_c_string ("image"),
	     make_int (r ? atomic_exchange (&nh_wk_deferrals[1], 0)
		       : atomic_load (&nh_wk_deferrals[1])));

  Lisp_Object tail = listn (6, nh_wk_kw (":phase-ms"), phase_ms,
			    nh_wk_kw (":blk-threads"), make_int (blk_threads),
			    nh_wk_kw (":deferrals"), deferrals);
  for (int w = NH_WK_N - 1; w >= 0; w--)
    {
      char key[16];
      snprintf (key, sizeof key, ":%s", nh_wk_names[w]);
      tail = Fcons (nh_wk_kw (key), Fcons (nh_wk_worker_stats (w, r), tail));
    }
  return tail;
}

/* Test primitives (PD-5).  */

static int
nh_wk_name_index (Lisp_Object sym, const char *const *names, int n,
		  const char *what)
{
  CHECK_SYMBOL (sym);
  const char *s = SSDATA (SYMBOL_NAME (sym));
  for (int i = 0; i < n; i++)
    if (strcmp (s, names[i]) == 0)
      return i;
  error ("Unknown %s: %s", what, s);
}

static int
nh_wk_point_arg (Lisp_Object point)
{
  int p = nh_wk_name_index (point, nh_wk_pause_names, NH_WK_P_N,
			    "pause point");
  if (p == NH_WK_P_NONE)
    error ("Unknown pause point: none");
  return p;
}

/* A test job: records the thread it ran on, parks at its pause point,
   runs MS milliseconds, and on landing pushes its id onto
   `nidhogg-workers--test-finished'.  */
struct nh_wk_test_job
{
  struct nh_wk_job job;
  int pause, ms;
  EMACS_INT id, user_id;
  int policy;			/* sched_getscheduler, or -1 */
  long tid;
  char comm[16];
};

enum { NH_WK_TEST_JOBS = 256 };
static struct nh_wk_test_job *nh_wk_test_jobs[NH_WK_TEST_JOBS];
static EMACS_INT nh_wk_test_next_id;

static void
nh_wk_test_run (struct nh_wk_job *job)
{
  struct nh_wk_test_job *t = (struct nh_wk_test_job *) job;
#ifdef __linux__
  t->tid = syscall (SYS_gettid);
  t->policy = sched_getscheduler (0);
  pthread_getname_np (pthread_self (), t->comm, sizeof t->comm);
#else
  t->policy = -1;
#endif
  if (t->pause != NH_WK_P_NONE)
    nh_wk_pause_point (t->pause);
  for (int i = 0; i < t->ms && !atomic_load (&job->cancel); i++)
    {
      struct timespec ms = { 0, NH_WK_MS };
      nanosleep (&ms, NULL);
    }
}

static void
nh_wk_test_finish (struct nh_wk_job *job)
{
  struct nh_wk_test_job *t = (struct nh_wk_test_job *) job;
  Vnidhogg_workers__test_finished
    = Fcons (make_int (t->user_id), Vnidhogg_workers__test_finished);
}

static void
nh_wk_test_free (struct nh_wk_job *job)
{
  free (job);
}

static struct nh_wk_test_job *
nh_wk_test_lookup (Lisp_Object id)
{
  CHECK_FIXNUM (id);
  struct nh_wk_test_job *t = nh_wk_test_jobs[XFIXNUM (id) % NH_WK_TEST_JOBS];
  if (!t || t->id != XFIXNUM (id))
    error ("No such test job: %"pI"d", XFIXNUM (id));
  return t;
}

DEFUN ("nidhogg-workers--test-job", Fnidhogg_workers__test_job,
       Snidhogg_workers__test_job, 2, MANY, 0,
       doc: /* Submit a test job to WORKER that runs MS milliseconds; for tests.
WORKER is `nh-io', `nh-parse', `nh-img', `nh-blk' or `nh-save'.  PROPS:
`:class' (`dir', `file' or `dns'; nh-blk's class, default `file'),
`:pause' (a pause point the job passes first), `:speculative' (non-nil:
an idle-priority job), `:id' (the integer the landing pushes onto
`nidhogg-workers--test-finished'; default the job's handle).  Return
the job's handle, an integer, or nil if the submit was refused.
usage: (nidhogg-workers--test-job WORKER MS &rest PROPS)  */)
  (ptrdiff_t nargs, Lisp_Object *args)
{
  int w = nh_wk_name_index (args[0], nh_wk_names, NH_WK_N, "worker");
  CHECK_FIXNAT (args[1]);
  struct nh_wk_test_job *t = calloc (1, sizeof *t);
  if (!t)
    memory_full (sizeof *t);
  t->ms = XFIXNAT (args[1]);
  t->id = t->user_id = ++nh_wk_test_next_id;
  t->policy = -1;
  t->job.w = w;
  t->job.cls = NH_WK_CL_FILE;
  t->job.run = nh_wk_test_run;
  t->job.finish = nh_wk_test_finish;
  t->job.free = nh_wk_test_free;
  atomic_store (&t->job.refs, 1);
  for (ptrdiff_t i = 2; i + 1 < nargs; i += 2)
    {
      const char *k = SYMBOLP (args[i]) ? SSDATA (SYMBOL_NAME (args[i])) : "";
      if (strcmp (k, ":class") == 0)
	t->job.cls = nh_wk_name_index (args[i + 1], nh_wk_class_names,
				       NH_WK_CL_N, "class");
      else if (strcmp (k, ":pause") == 0)
	t->pause = nh_wk_point_arg (args[i + 1]);
      else if (strcmp (k, ":speculative") == 0)
	t->job.speculative = !NILP (args[i + 1]);
      else if (strcmp (k, ":id") == 0)
	{
	  CHECK_FIXNUM (args[i + 1]);
	  t->user_id = XFIXNUM (args[i + 1]);
	}
      else
	{
	  free (t);
	  xsignal2 (Qerror, build_string ("Unknown property"), args[i]);
	}
    }
  if (!nh_wk_submit (&t->job))
    {
      nh_wk_job_unref (&t->job);
      return Qnil;
    }
  int slot = t->id % NH_WK_TEST_JOBS;
  if (nh_wk_test_jobs[slot])
    nh_wk_job_unref (&nh_wk_test_jobs[slot]->job);
  nh_wk_test_jobs[slot] = t;
  return make_int (t->id);
}

DEFUN ("nidhogg-workers--test-wait", Fnidhogg_workers__test_wait,
       Snidhogg_workers__test_wait, 2, 4, 0,
       doc: /* Wait at most MS milliseconds for the test job ID; for tests.
Return 1 when it is done, 0 at the deadline, -1 if it was cancelled.
PHASE (default `worker-wait') and REASON name the wait's watchdog phase:
PHASE is `worker-wait' or `treesit-wait', REASON nil or one of `display',
`image-pixels', `detach', `drain'.  */)
  (Lisp_Object id, Lisp_Object ms, Lisp_Object phase, Lisp_Object reason)
{
  struct nh_wk_test_job *t = nh_wk_test_lookup (id);
  CHECK_FIXNAT (ms);
  static const char *const reasons[] = {
    "display", "image-pixels", "detach", "drain"
  };
  static const int reason_ids[] = {
    NH_WK_RS_DISPLAY, NH_WK_RS_IMAGE_PIXELS, NH_WK_RS_DETACH, NH_WK_RS_DRAIN
  };
  int ph = NH_WK_PH_WORKER_WAIT, rs = 0;
  if (!NILP (phase))
    CHECK_SYMBOL (phase);
  if (!NILP (phase) && strcmp (SSDATA (SYMBOL_NAME (phase)), "treesit-wait") == 0)
    ph = NH_WK_PH_TREESIT_WAIT;
  if (!NILP (reason))
    rs = reason_ids[nh_wk_name_index (reason, reasons, 4, "reason")];
  return make_int (nh_wk_wait (&t->job, XFIXNAT (ms) * NH_WK_MS, ph, rs));
}

DEFUN ("nidhogg-workers--test-job-info", Fnidhogg_workers__test_job_info,
       Snidhogg_workers__test_job_info, 1, 1, 0,
       doc: /* Return what the test job ID recorded; for tests.
A plist: `:state' (`queued', `running', `done' or `cancelled'), and,
once it ran, `:comm' (its thread's name), `:policy' (`other', `idle',
`batch' or nil) and `:tid'.  */)
  (Lisp_Object id)
{
  static const char *const states[] = {
    "queued", "running", "done", "cancelled"
  };
  struct nh_wk_test_job *t = nh_wk_test_lookup (id);
  int st = atomic_load (&t->job.state);
  Lisp_Object policy = Qnil;
#ifdef __linux__
  if (t->policy == SCHED_OTHER)
    policy = intern_c_string ("other");
  else if (t->policy == SCHED_IDLE)
    policy = intern_c_string ("idle");
  else if (t->policy == SCHED_BATCH)
    policy = intern_c_string ("batch");
#endif
  bool ran = st != NH_WK_J_QUEUED && t->comm[0];
  return listn (8, nh_wk_kw (":state"), intern_c_string (states[st]),
		nh_wk_kw (":comm"), ran ? build_string (t->comm) : Qnil,
		nh_wk_kw (":policy"), policy,
		nh_wk_kw (":tid"), ran ? make_int (t->tid) : Qnil);
}

DEFUN ("nidhogg-workers--test-cancel", Fnidhogg_workers__test_cancel,
       Snidhogg_workers__test_cancel, 1, 1, 0,
       doc: /* Cancel the test job ID; for tests.  */)
  (Lisp_Object id)
{
  nh_wk_cancel (&nh_wk_test_lookup (id)->job);
  return Qnil;
}

DEFUN ("nidhogg-workers--pause-at", Fnidhogg_workers__pause_at,
       Snidhogg_workers__pause_at, 1, 1, 0,
       doc: /* Arm the pause point POINT: a worker that reaches it parks; for tests.
POINT is one of `after-snapshot', `before-done', `after-read',
`before-tokenize', `before-decode', `in-poll', `after-header',
`before-write', `in-lookup'.  `nidhogg-workers--release' un-parks.  */)
  (Lisp_Object point)
{
  nh_wk_arm_pause (nh_wk_point_arg (point));
  return Qnil;
}

DEFUN ("nidhogg-workers--release", Fnidhogg_workers__release,
       Snidhogg_workers__release, 0, 1, 0,
       doc: /* Disarm the pause point POINT and un-park its workers; for tests.
With POINT nil, every point.  */)
  (Lisp_Object point)
{
  nh_wk_release (NILP (point) ? -1 : nh_wk_point_arg (point));
  return Qnil;
}

DEFUN ("nidhogg-workers--test-await-paused",
       Fnidhogg_workers__test_await_paused,
       Snidhogg_workers__test_await_paused, 2, 3, 0,
       doc: /* Wait at most MS milliseconds until N workers park at POINT; for tests.
N defaults to 1.  Return how many are parked there, or nil if fewer than
N.  A bounded condition wait, not a sleep.  */)
  (Lisp_Object point, Lisp_Object ms, Lisp_Object n)
{
  CHECK_FIXNAT (ms);
  int want = NILP (n) ? 1 : (CHECK_FIXNAT (n), XFIXNAT (n));
  int r = nh_wk_await_paused (nh_wk_point_arg (point), XFIXNAT (ms), want);
  return r < 0 ? Qnil : make_int (r);
}

DEFUN ("nidhogg-workers--test-release-on-wait",
       Fnidhogg_workers__test_release_on_wait,
       Snidhogg_workers__test_release_on_wait, 1, 1, 0,
       doc: /* Make the next wait on a job release POINT when it begins; for tests.
POINT nil cancels.  For a wait whose thread holds the global lock, so
nothing else in Lisp could release the job.  */)
  (Lisp_Object point)
{
  atomic_store (&nh_wk_release_on_wait,
		NILP (point) ? -1 : nh_wk_point_arg (point));
  return Qnil;
}

/* The signal storm (tests): SIGURG and SIGCHLD to one thread.  */
struct nh_wk_storm
{
  pthread_t target;
  int n, point;
};
static _Atomic int nh_wk_storm_sent;

static void *
nh_wk_storm_main (void *arg)
{
  struct nh_wk_storm *s = arg;
  for (int i = 0; i < s->n; i++)
    {
      struct timespec ms = { 0, NH_WK_MS };
      pthread_kill (s->target, SIGURG);
      pthread_kill (s->target, SIGCHLD);
      atomic_fetch_add (&nh_wk_storm_sent, 1);
      nanosleep (&ms, NULL);
    }
  if (s->point >= 0)
    nh_wk_release (s->point);
  free (s);
  return NULL;
}

DEFUN ("nidhogg-workers--test-signal-storm",
       Fnidhogg_workers__test_signal_storm,
       Snidhogg_workers__test_signal_storm, 2, 2, 0,
       doc: /* Send this thread SIGURG and SIGCHLD every ms, N times; for tests.
A helper thread sends them, then releases the pause point POINT (unless
nil).  `nidhogg-workers--test-storm-sent' counts the pairs sent.  */)
  (Lisp_Object n, Lisp_Object point)
{
  CHECK_FIXNAT (n);
  int p = NILP (point) ? -1 : nh_wk_point_arg (point);
  struct nh_wk_storm *s = malloc (sizeof *s);
  if (!s)
    memory_full (sizeof *s);
  *s = (struct nh_wk_storm) { pthread_self (), XFIXNAT (n), p };
  atomic_store (&nh_wk_storm_sent, 0);
  pthread_attr_t attr;
  pthread_t t;
  sigset_t all, old;
  pthread_attr_init (&attr);
  pthread_attr_setdetachstate (&attr, PTHREAD_CREATE_DETACHED);
  sigfillset (&all);
  pthread_sigmask (SIG_SETMASK, &all, &old);
  int err = pthread_create (&t, &attr, nh_wk_storm_main, s);
  pthread_sigmask (SIG_SETMASK, &old, NULL);
  pthread_attr_destroy (&attr);
  if (err)
    {
      free (s);
      error ("Cannot start the signal storm");
    }
  return Qt;
}

DEFUN ("nidhogg-workers--test-storm-sent", Fnidhogg_workers__test_storm_sent,
       Snidhogg_workers__test_storm_sent, 0, 0, 0,
       doc: /* Return how many signal pairs the last storm sent; for tests.  */)
  (void)
{
  return make_int (atomic_load (&nh_wk_storm_sent));
}

DEFUN ("nidhogg-workers--test-wake-race", Fnidhogg_workers__test_wake_race,
       Snidhogg_workers__test_wake_race, 1, 1, 0,
       doc: /* Check the completion channel's wake protocol N times; for tests.
Return (:level OK :lost K :stress-lost K): OK if the fd follows the
queue (readable after a push, cleared by the callback, which arms the
drainer only while a job is queued); K the wake-ups lost when a producer
appends between the callback's clear and its re-check (N races), and
when two producers push N jobs freely.  */)
  (Lisp_Object n)
{
  CHECK_FIXNAT (n);
  if (!nh_wk_race_open ())
    error ("Cannot open a wake fd");
  bool level = nh_wk_race_level ();
  int lost = nh_wk_race_gap (XFIXNAT (n));
  int stress = nh_wk_race_stress (XFIXNAT (n), 2);
  return listn (6, nh_wk_kw (":level"), level ? Qt : Qnil,
		nh_wk_kw (":lost"), make_int (lost),
		nh_wk_kw (":stress-lost"), make_int (stress));
}

/* The descriptors `--test-fd-reserve' holds.  */
static int *nh_wk_reserved;
static int nh_wk_nreserved;

DEFUN ("nidhogg-workers--test-fd-reserve", Fnidhogg_workers__test_fd_reserve,
       Snidhogg_workers__test_fd_reserve, 1, 1, 0,
       doc: /* Fill every free descriptor below FD_SETSIZE - MARGIN; for tests.
With MARGIN nil, give them back.  Return how many are held.  */)
  (Lisp_Object margin)
{
  if (NILP (margin))
    {
      while (nh_wk_nreserved > 0)
	close (nh_wk_reserved[--nh_wk_nreserved]);
      free (nh_wk_reserved);
      nh_wk_reserved = NULL;
      return make_int (0);
    }
  CHECK_FIXNAT (margin);
  if (!nh_wk_reserved)
    nh_wk_reserved = malloc (FD_SETSIZE * sizeof *nh_wk_reserved);
  if (!nh_wk_reserved)
    memory_full (FD_SETSIZE * sizeof *nh_wk_reserved);
  while (nh_wk_nreserved < FD_SETSIZE)
    {
      int fd = open ("/dev/null", O_RDONLY | O_CLOEXEC);
      if (fd < 0)
	break;
      if (fd >= FD_SETSIZE - XFIXNAT (margin))
	{
	  close (fd);
	  break;
	}
      nh_wk_reserved[nh_wk_nreserved++] = fd;
    }
  return make_int (nh_wk_nreserved);
}

DEFUN ("nidhogg-workers--test-wake-open", Fnidhogg_workers__test_wake_open,
       Snidhogg_workers__test_wake_open, 0, 0, 0,
       doc: /* Return t if a wake fd can be opened now (it is closed again); for tests.  */)
  (void)
{
  struct nh_wk_wake w;
  if (nh_wk_wake_open (&w) < 0)
    return Qnil;
  nh_wk_wake_close (&w);
  return Qt;
}

/* `nidhogg-workers--mode' sets nh_wk_inline (a variable watcher).  */
static Lisp_Object
watch_nidhogg_workers_mode (Lisp_Object symbol, Lisp_Object newval,
			    Lisp_Object operation, Lisp_Object where)
{
  (void) symbol;
  (void) operation;
  if (NILP (where))
    nh_wk_inline = EQ (newval, Qinline);
  return Qnil;
}

static void
nh_wk_syms (void)
{
  DEFSYM (Qnidhogg_workers__drain, "nidhogg-workers--drain");
  DEFSYM (Qinline, "inline");
  DEFSYM (Qthreads, "threads");
  DEFSYM (Qnidhogg_workers__mode, "nidhogg-workers--mode");

  DEFVAR_LISP ("nidhogg-workers--mode", Vnidhogg_workers__mode,
	       doc: /* How jobs run: `threads' (on the worker threads) or `inline'.
In `inline' mode every job runs inside its submit, on the thread that
submitted it, and lands at the next timer check: the same code paths
with no scheduling, for tests.  */);
  Vnidhogg_workers__mode = Qthreads;

  DEFVAR_LISP ("nidhogg-workers--test-finished",
	       Vnidhogg_workers__test_finished,
	       doc: /* The ids of the test jobs that landed, the latest first; for tests.  */);
  Vnidhogg_workers__test_finished = Qnil;

  static union Aligned_Lisp_Subr Swatch_nidhogg_workers_mode =
     {{{ PSEUDOVECTOR_FLAG | (PVEC_SUBR << PSEUDOVECTOR_AREA_BITS) },
       { .a4 = watch_nidhogg_workers_mode },
       4, 4, "watch_nidhogg_workers_mode", {0}, lisp_h_Qnil}};
  Lisp_Object watcher;
  XSETSUBR (watcher, &Swatch_nidhogg_workers_mode.s);
  Fadd_variable_watcher (Qnidhogg_workers__mode, watcher);

  defsubr (&Snidhogg_workers__drain);
  defsubr (&Snidhogg_workers_stats);
  defsubr (&Snidhogg_workers__test_job);
  defsubr (&Snidhogg_workers__test_wait);
  defsubr (&Snidhogg_workers__test_job_info);
  defsubr (&Snidhogg_workers__test_cancel);
  defsubr (&Snidhogg_workers__pause_at);
  defsubr (&Snidhogg_workers__release);
  defsubr (&Snidhogg_workers__test_await_paused);
  defsubr (&Snidhogg_workers__test_release_on_wait);
  defsubr (&Snidhogg_workers__test_signal_storm);
  defsubr (&Snidhogg_workers__test_storm_sent);
  defsubr (&Snidhogg_workers__test_wake_race);
  defsubr (&Snidhogg_workers__test_fd_reserve);
  defsubr (&Snidhogg_workers__test_wake_open);
}

#endif /* not NH_WK_HARNESS */

/* Tasks 4 and 5.  */

void
nh_wk_fatal_begin (void)
{
}

int64_t
nh_wk_budget_take (int64_t want_ns)
{
  return want_ns;
}

void
nh_wk_budget_reset (void)
{
}

#ifndef NH_WK_HARNESS

void
syms_of_nidhogg_workers (void)
{
  nh_wk_syms ();
  nh_ts_syms ();
  nh_io_syms ();
  nh_dec_syms ();
  nh_json_syms ();
  nh_img_syms ();
  nh_blk_syms ();

  Fprovide (intern_c_string ("nidhogg-workers"), Qnil);
}

#endif /* not NH_WK_HARNESS */

#endif /* HAVE_NIDHOGG_WORKERS */

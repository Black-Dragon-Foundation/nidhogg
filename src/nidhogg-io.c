/* Nidhogg's worker threads: the process reader (nh-io).

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

/* Fd ownership (Task 16; the design's §5.3 and §8.2).  Tasks 17 to 20
   and 24 add flow control, drains, decoding, coalescing, TLS and
   framing.

   Hand-over.  When an eligible process's real fd would enter the
   select set (add_process_read_fd), nh_io_handover gives nh-io its own
   copy of the fd (F_DUPFD_CLOEXEC: the same open file description) and
   opens a wake fd for the process; the select set holds the wake fd
   and chan_process names the process there (process.c).  The real fd
   stays p->infd and stays in p->open_fd[], so writes, ioctls,
   setsockopt and process-send-eof are unchanged.  The handle is found
   by the real fd's number (nh_io_tab, main thread; nh_io_live, the
   lock-free view for signal context, published last and cleared first).

   Reading.  nh-io polls its command fd and every copy, reads each
   readable copy into the process's FIFO of chunks (one read of the
   process's readmax each), and writes the wake fd when the FIFO stops
   being empty (or EOF or an error is recorded).  The main thread's
   round-robin sees the wake fd readable and calls read_process_output,
   whose reader branch (nh_io_read) takes one delivery's worth of
   chunks into today's decode and filter path.  When it finds the FIFO
   empty it clears the wake fd, under the same mutex nh-io appends
   under, and returns EAGAIN: the
   wake fd is readable exactly while there is something to deliver, so
   the round-robin never spins (fd audit, finding 13).  EOF and errors
   come back as read would return them, once the FIFO is empty.  When
   the process decodes with a stateless system, nh-io decodes each chunk
   as it reads it (nidhogg-decode.c) and a filter gets that decode if it
   is still current (nh_io_decoded, Task 18).

   Hand-back is synchronous.  The main thread sends "detach" on the
   command fd and waits (worker-wait, reason detach) for the
   acknowledgement; nh-io looks at its commands at every poll return and
   between reads, drops the handle, closes its copy, then acknowledges.
   So a real fd number the main thread closes afterwards can never be
   read by nh-io for another owner, and the child sees EOF once Emacs
   closes.  Bytes nh-io read but the main thread did not deliver stay on
   the handle: a process that goes on (not closing) keeps its wake fd
   selected until they are delivered, then reads its real fd again.  A
   wait that times out (nh-io stuck, which only a test pause point can
   do) leaves the handle to nh-io, which frees it when it gets to the
   command; once the workers are dead (the fatal path) nothing waits and
   both fds close directly.

   Flow control and caps (Task 17).  Where today's code takes a process's
   fd out of the select set (delete_read_fd: `set-process-filter' t,
   `stop-process' on a connection, the SIGCHLD handler on exit), the
   reader is paused too: nh_io_pause sets the handle's atomic flag and
   writes one byte to the command fd, no mutex, no malloc, errno kept,
   so it runs in signal context; nh-io stops reading that copy after the
   read in progress.  add_process_read_fd resumes it.  Each handle's
   FIFO holds at most max (4 × readmax, 256 KB): at the cap nh-io stops
   reading the copy (the kernel's back-pressure, as today) and parks at
   the `after-read' pause point when a test armed it; the delivery that
   takes it below the cap wakes nh-io.

   Drains (Task 17).  Before status_notify's drain and a non-running
   wait_proc's, an ending process (exited, signalled, closed, failed)
   is handed back (reason status, worker-wait phase reason drain) and
   is never handed over again: the main thread delivers the FIFO, then
   reads the real fd to EAGAIN or EOF (a grandchild holding the pipe
   open cannot make it wait), all before the sentinel.

   Delivery (Task 19; the design's §8.2 "Delivery", S2).  One delivery
   per channel per select round, as today's one read, but it joins what
   is queued: the first chunk (or as much of it as fits), then whole
   chunks while they fit.  It never waits for more.  What fits is
   read-process-output-max for the default filter, which is C; for any
   other filter also the process's estimated 4 ms of filter work (a
   moving average of the filter's ns per byte over its calls), and
   before there is an estimate a pty's read (4 KB) or a pipe's or
   socket's (64 KB).  Chunks taken whole and decoded in one run keep
   their decodes for the delivery: a filter's string is their
   concatenation (nh_io_decoded), and the default filter inserts them
   into the buffer with today's bookkeeping (nh_io_insert), so the main
   thread decodes nothing.  The rest of the C delivery runs in the
   watchdog's `deliver' phase, the filter in its `filter' phase, and
   both count their ns.  Adaptive read buffering does not apply.

   STARTTLS (Task 20, MC-32).  `gnutls-boot' gives GnuTLS the real fd,
   which GnuTLS then reads itself, so a reader-owned process is handed
   back at the start of Fgnutls_boot, before any GnuTLS call, and is
   never handed over again: a per-fd mark, since `gnutls-deinit' clears
   gnutls_p (fd audit finding 9).  Bytes nh-io had read but not delivered
   arrived after the plaintext Lisp last saw; today they would still be
   in the kernel, the start of what GnuTLS reads.  They go to GnuTLS,
   never to the filter (that would be STARTTLS response injection): the
   session gets a pull function that returns them first and then reads
   the fd, and TLS's pending check counts them (finding 10).

   Inline mode (`nidhogg-workers--mode') has no thread: the select set
   holds the copy itself and nh_io_read reads it, so the hand-over, the
   select mapping and the close order run without scheduling.

   The core (handles, the loop, the handshake) is Lisp-free; the
   harness's ThreadSanitizer driver (fuzz/tsan-io.c) runs it.  */

#ifndef NH_WK_HARNESS
# include <config.h>
#elif !defined _GNU_SOURCE
# define _GNU_SOURCE		/* F_DUPFD_CLOEXEC on older libcs */
#endif

#ifdef HAVE_NIDHOGG_WORKERS

#include <errno.h>
#include <fcntl.h>
#include <poll.h>
#include <string.h>
#include <unistd.h>
#include <sys/select.h>
#include <sys/socket.h>

#include "nidhogg-workers.h"

#ifndef NH_WK_HARNESS
# include <time.h>
# include "nidhogg-workers-phase.h"
# include "buffer.h"
# include "character.h"
# include "coding.h"
# include "composite.h"
# include "process.h"
# define NH_IO_PH NH_WK_PH_WORKER_WAIT
# define NH_IO_RS NH_WK_RS_DETACH
# define NH_IO_RS_DRAIN NH_WK_RS_DRAIN
#else
# define NH_IO_PH 0
# define NH_IO_RS 0
# define NH_IO_RS_DRAIN 0
#endif

/* A FIFO's cap: at least this, else 4 reads' worth (MC-22).  */
#define NH_IO_CAP_MIN (256 * 1024)

/* How long a hand-back waits for nh-io: one 64 KB step is microseconds,
   so only a stuck nh-io reaches this.  */
#define NH_IO_DETACH_NS (INT64_C (1000) * 1000000)

/* The fds a reader-owned process holds: its copy and its wake fd (a
   pipe, two fds, where there is no eventfd).  */
#ifdef __linux__
enum { NH_IO_FDS = 2 };
#else
enum { NH_IO_FDS = 3 };
#endif

/* Why a hand-over was refused (`:refused' of --test-io-info).  */
enum nh_io_refusal
{
  NH_IO_RF_NONE,
  NH_IO_RF_OFF,			/* nidhogg-workers-process-reader is nil */
  NH_IO_RF_SWITCH,		/* the fd kind's sub-switch is nil */
  NH_IO_RF_TLS,
  NH_IO_RF_DATAGRAM,
  NH_IO_RF_SERVER,
  NH_IO_RF_SERIAL,
  NH_IO_RF_FILTER_T,
  NH_IO_RF_CONNECTING,
  NH_IO_RF_OPT_OUT,
  NH_IO_RF_BUDGET,
  NH_IO_RF_THREAD,
  NH_IO_RF_ENDED,		/* handed back for its last drain */
  NH_IO_RF_N
};

/* A chunk (Task 18): DATA holds the decoder's carry-in (NCIN bytes) and
   then the bytes read, so OFF starts at NCIN and LEN counts both.  When
   SYS is not NH_DEC_NONE, DEC (in the same block, after DATA) is their
   decode, DLEN bytes and NCHARS characters, which left the NCOUT bytes
   at COUT undecoded; GEN is the coding generation it was decoded
   under.  */
struct nh_io_chunk
{
  struct nh_io_chunk *next;
  ptrdiff_t len, off;
  int ncin, ncout, sys;
  uint64_t gen;
  unsigned char *dec;
  ptrdiff_t dlen, nchars;
  unsigned char cout[NH_DEC_CARRY];
  unsigned char data[];
};

enum nh_io_op { NH_IO_OP_ADD, NH_IO_OP_DETACH };

struct nh_io_h;

struct nh_io_cmd
{
  int op;
  struct nh_io_h *h;
  struct nh_io_cmd *next;
};

/* One reader-owned process.  */
struct nh_io_h
{
  int64_t id;			/* for the test log */
  int copy;			/* nh-io's copy of the real fd */
  int sel;			/* in the select set: the wake fd, or the copy */
  struct nh_wk_wake wake;
  bool inl;			/* inline mode: no thread */
  ptrdiff_t readmax;
  ptrdiff_t cap;		/* the FIFO's cap in bytes */

  /* Paused by delete_read_fd (any thread, signal context included).  */
  _Atomic bool paused;
  /* nh-io's: set before it reads the copy, cleared once what it read is
     queued and the wake fd written (nh_io_settle).  */
  _Atomic bool reading;
  /* The FIFO is at its cap: set and cleared under nh_io_m, read by nh-io
     without it.  */
  _Atomic bool capped;

  /* Under nh_io_m (nh-io reads it without, to size its next read).  */
  _Atomic ptrdiff_t queued;
  /* Under nh_io_m.  */
  struct nh_io_chunk *head, *tail;
  bool eof;
  int err;

  /* The main thread's: the hand-back is done.  */
  bool detached;
  /* nh-io's: it read EOF or an error, and polls the copy no more.  */
  bool io_end;

  struct nh_io_cmd add, detach;
  /* The detach handshake: nh-io completes it.  Two references, the main
     thread's and nh-io's, so whichever is last frees it.  */
  struct nh_wk_job *ack;
  /* Set by whichever comes first: nh-io acknowledging, or the main
     thread giving up after a timed-out wait.  */
  _Atomic bool gone;

  /* Decoding (Task 18).  The main thread's coding generation and the
     system to decode with, as GEN << 4 | SYS (0, raw, until the first
     nh_io_coding_changed: the constructors set the coding up after the
     hand-over, fd audit finding 1).  */
  _Atomic uint64_t coding;
  /* The reader's (nh-io, or the main thread inline): the system of its
     last chunk and what that chunk left undecoded.  */
  int dsys;
  struct nh_dec_state dst;
  /* The main thread's: the chunks nh_io_take just delivered whole, in
     one run of decodes (a list through `next'), kept for nh_io_decoded
     and nh_io_insert; and how many deliveries used those decodes
     (`:decoded') or decoded the bytes again (`:redecoded').  */
  struct nh_io_chunk *last;
  int64_t ndecoded, nredecoded;

  /* Delivery (Task 19), the main thread's: the filter's estimated ns per
     byte, once COST_KNOWN; COST_FIXED when a test fixed it.  */
  double cost;
  bool cost_known, cost_fixed;
};

#define NH_IO_SYS(coding) ((int) ((coding) & 0xF))
#define NH_IO_GEN(coding) ((coding) >> 4)

/* nh-io's mutex: the command list and every handle's FIFO.  */
static pthread_mutex_t nh_io_m = PTHREAD_MUTEX_INITIALIZER;
static struct nh_io_cmd *nh_io_cmd_head, *nh_io_cmd_tail;
static struct nh_wk_wake nh_io_cmdw = { -1, -1 };
static _Atomic bool nh_io_cmd_pending;
static _Atomic bool nh_io_crash;

/* nh-io's loop: one job that never ends.  */
static struct nh_wk_job nh_io_loop;
static bool nh_io_running;

/* The test log (--test-io-log): a ring of the latest events.  */
enum nh_io_ev
{
  NH_IO_EV_HANDOVER, NH_IO_EV_READ, NH_IO_EV_EOF, NH_IO_EV_ERROR,
  NH_IO_EV_ACK, NH_IO_EV_CLOSE, NH_IO_EV_TIMEOUT, NH_IO_EV_DIRECT,
  NH_IO_EV_N
};

struct nh_io_logent
{
  int ev, fd;
  int64_t id;
  ptrdiff_t n;
};

enum { NH_IO_LOG = 4096 };
static pthread_mutex_t nh_io_log_m = PTHREAD_MUTEX_INITIALIZER;
static _Atomic bool nh_io_logging;
static struct nh_io_logent nh_io_log[NH_IO_LOG];
static int64_t nh_io_nlog;

static void
nh_io_note (int ev, int64_t id, int fd, ptrdiff_t n)
{
  if (!atomic_load (&nh_io_logging))
    return;
  pthread_mutex_lock (&nh_io_log_m);
  nh_io_log[nh_io_nlog++ % NH_IO_LOG]
    = (struct nh_io_logent) { .ev = ev, .fd = fd, .id = id, .n = n };
  pthread_mutex_unlock (&nh_io_log_m);
}

static void
nh_io_chunks_free (struct nh_io_chunk *c)
{
  for (struct nh_io_chunk *next; c; c = next)
    {
      next = c->next;
      free (c);
    }
}

static void
nh_io_free (struct nh_io_h *h)
{
  nh_io_chunks_free (h->head);
  nh_io_chunks_free (h->last);
  nh_wk_stat_add (NH_WK_IO, NH_WK_ST_QUEUED_BYTES, -h->queued);
  if (!h->inl)
    nh_wk_wake_close (&h->wake);
  if (h->copy >= 0)
    close (h->copy);
  free (h);
}

/* Make nh-io look at its handles again (a pause, a resume, a FIFO back
   under its cap).  Async-signal-safe: an atomic store and one write.  */
static void
nh_io_poke (void)
{
  atomic_store (&nh_io_cmd_pending, true);
  nh_wk_wake_signal (&nh_io_cmdw);
}

/* nh-io.  */

static _Noreturn void
nh_io_park (void)
{
  for (;;)
    poll (NULL, 0, -1);
}

static void
nh_io_crash_now (void)
{
  int *volatile p = NULL;
  *p = 0;
}

/* Take the command list and carry it out.  POLLED[0..NP) are the
   handles of the poll in progress; a detached one is forgotten there.  */
static void
nh_io_commands (struct nh_io_h ***set, int *n, int *cap,
		struct nh_io_h **polled, int np)
{
  if (!atomic_exchange (&nh_io_cmd_pending, false))
    return;
  nh_wk_wake_clear (&nh_io_cmdw);
  if (atomic_load (&nh_io_crash))
    nh_io_crash_now ();
  nh_wk_lock (&nh_io_m);
  struct nh_io_cmd *c = nh_io_cmd_head;
  nh_io_cmd_head = nh_io_cmd_tail = NULL;
  pthread_mutex_unlock (&nh_io_m);

  while (c)
    {
      /* C lives in its handle, which the main thread may free as soon
	 as the acknowledgement is out.  */
      struct nh_io_cmd *next = c->next;
      struct nh_io_h *h = c->h;
      if (atomic_load (&nh_wk_dead))
	nh_io_park ();
      if (c->op == NH_IO_OP_ADD)
	{
	  if (*n == *cap)
	    {
	      int ncap = *cap ? 2 * *cap : 16;
	      struct nh_io_h **s = realloc (*set, ncap * sizeof *s);
	      NH_WK_ASSERT (s);
	      *set = s;
	      *cap = ncap;
	    }
	  (*set)[(*n)++] = h;
	}
      else
	{
	  for (int i = 0; i < *n; i++)
	    if ((*set)[i] == h)
	      {
		(*set)[i] = (*set)[--*n];
		break;
	      }
	  for (int k = 0; k < np; k++)
	    if (polled[k] == h)
	      polled[k] = NULL;
	  close (h->copy);
	  h->copy = -1;
	  nh_io_note (NH_IO_EV_ACK, h->id, -1, 0);
	  struct nh_wk_job *ack = h->ack;
	  if (atomic_exchange (&h->gone, true))
	    {
	      /* The main thread gave up waiting: the handle is ours.  */
	      nh_wk_job_unref (ack);
	      nh_io_free (h);
	    }
	  else
	    nh_wk_complete (ack);
	}
      c = next;
    }
}

/* A chunk of the R bytes at BUF, which H's reader (nh-io, or the main
   thread inline) decodes first when H's system is a stateless one; NULL
   when out of memory.  */
static struct nh_io_chunk *
nh_io_chunk_new (struct nh_io_h *h, const unsigned char *buf, ptrdiff_t r)
{
  uint64_t coding = atomic_load (&h->coding);
  int sys = NH_IO_SYS (coding);
  /* A test may change the coding here: the chunk is then decoded under
     a stale generation.  */
  nh_wk_pause_point (NH_WK_P_BEFORE_DECODE);
  if (sys != h->dsys)
    {
      h->dsys = sys;
      h->dst.n = 0;
    }
  int ncin = sys != NH_DEC_NONE ? h->dst.n : 0;
  ptrdiff_t total = ncin + r;
  struct nh_io_chunk *c
    = malloc (sizeof *c + total
	      + (sys != NH_DEC_NONE ? NH_DEC_OUT_MAX (total) : 0));
  if (!c)
    return NULL;
  *c = (struct nh_io_chunk) { .len = total, .off = ncin, .ncin = ncin,
			      .gen = NH_IO_GEN (coding) };
  memcpy (c->data, h->dst.carry, ncin);
  memcpy (c->data + ncin, buf, r);
  if (sys != NH_DEC_NONE)
    {
      ptrdiff_t consumed;
      c->sys = sys;
      c->dec = c->data + total;
      c->dlen = nh_dec_run (sys, c->data, total, false, c->dec, &c->nchars,
			    &consumed);
      c->ncout = total - consumed;
      NH_WK_ASSERT (c->ncout <= NH_DEC_CARRY);
      memcpy (c->cout, c->data + consumed, c->ncout);
      memcpy (h->dst.carry, c->cout, c->ncout);
      h->dst.n = c->ncout;
    }
  return c;
}

/* Read H's copy once into its FIFO, never past its cap.  */
static void
nh_io_read_one (struct nh_io_h *h, unsigned char **buf, ptrdiff_t *size)
{
  /* Only the main thread lowers QUEUED meanwhile: WANT can only be
     smaller than the room there is.  */
  ptrdiff_t want = h->cap - atomic_load (&h->queued);
  if (want > h->readmax)
    want = h->readmax;
  if (want <= 0)
    return;
  if (*size < h->readmax)
    {
      free (*buf);
      *buf = malloc (h->readmax);
      NH_WK_ASSERT (*buf);
      *size = h->readmax;
    }
  ssize_t r;
  atomic_store (&h->reading, true);
  do
    r = read (h->copy, *buf, want);
  while (r < 0 && errno == EINTR);
  if (r < 0 && (errno == EAGAIN || errno == EWOULDBLOCK))
    {
      atomic_store (&h->reading, false);
      return;
    }
  int err = errno;
  struct nh_io_chunk *c = NULL;
  if (r > 0)
    {
      c = nh_io_chunk_new (h, *buf, r);
      if (!c)
	{
	  r = -1;
	  err = ENOMEM;
	}
    }
  nh_io_note (r > 0 ? NH_IO_EV_READ : r == 0 ? NH_IO_EV_EOF : NH_IO_EV_ERROR,
	      h->id, h->copy, r);

  nh_wk_lock (&nh_io_m);
  /* The wake fd is readable exactly while the FIFO holds a chunk or EOF
     or an error is recorded (nh_io_take clears it under this mutex).  */
  bool was_quiet = !h->head && !h->eof && !h->err;
  bool capped = false;
  if (c)
    {
      if (h->tail)
	h->tail->next = c;
      else
	h->head = c;
      h->tail = c;
      if ((h->queued += r) >= h->cap)
	{
	  atomic_store (&h->capped, true);
	  capped = true;
	}
    }
  else if (r == 0)
    h->eof = true;
  else
    h->err = err;
  if (was_quiet)
    nh_wk_wake_signal (&h->wake);
  pthread_mutex_unlock (&nh_io_m);
  atomic_store (&h->reading, false);

  if (c)
    nh_wk_stat_add (NH_WK_IO, NH_WK_ST_QUEUED_BYTES, r);
  else
    h->io_end = true;
  if (capped)
    {
      /* nh-io polls this copy no more until a delivery wakes it.  */
      nh_wk_stat_add (NH_WK_IO, NH_WK_ST_BACKPRESSURE, 1);
      nh_wk_pause_point (NH_WK_P_AFTER_READ);
    }
}

/* Whether nh-io reads H's copy now.  */
static bool
nh_io_reading (struct nh_io_h *h)
{
  return (!h->io_end && !atomic_load (&h->paused)
	  && !atomic_load (&h->capped));
}

static void
nh_io_main (struct nh_wk_job *job)
{
  (void) job;
  struct nh_io_h **set = NULL, **polled = NULL;
  struct pollfd *pfd = NULL;
  int n = 0, cap = 0, pcap = 0;
  unsigned char *buf = NULL;
  ptrdiff_t bufsize = 0;

  for (;;)
    {
      if (pcap < n + 1)
	{
	  pcap = 2 * (n + 1);
	  pfd = realloc (pfd, pcap * sizeof *pfd);
	  polled = realloc (polled, pcap * sizeof *polled);
	  NH_WK_ASSERT (pfd && polled);
	}
      int np = 0;
      pfd[np] = (struct pollfd) { .fd = nh_io_cmdw.rfd, .events = POLLIN };
      polled[np++] = NULL;
      for (int i = 0; i < n; i++)
	if (nh_io_reading (set[i]))
	  {
	    pfd[np] = (struct pollfd) { .fd = set[i]->copy, .events = POLLIN };
	    polled[np++] = set[i];
	  }

      int r = poll (pfd, np, -1);
      if (atomic_load (&nh_wk_dead))
	nh_io_park ();
      if (r < 0)
	continue;
      nh_wk_pause_point (NH_WK_P_IN_POLL);
      if (atomic_load (&nh_wk_dead))
	nh_io_park ();

      /* Commands first, then between reads: a detach waits for at most
	 one read.  */
      nh_io_commands (&set, &n, &cap, polled, np);
      for (int k = 1; k < np; k++)
	{
	  struct nh_io_h *h = polled[k];
	  if (!h || !(pfd[k].revents & (POLLIN | POLLHUP | POLLERR | POLLNVAL))
	      /* Paused since the poll: no read after the one in progress.  */
	      || atomic_load (&h->paused))
	    continue;
	  if (atomic_load (&nh_wk_dead))
	    nh_io_park ();
	  nh_io_read_one (h, &buf, &bufsize);
	  nh_io_commands (&set, &n, &cap, polled, np);
	}
    }
}

/* Start nh-io once.  */
static bool
nh_io_start (void)
{
  if (nh_io_running)
    return true;
  if (nh_io_cmdw.rfd < 0 && nh_wk_wake_open (&nh_io_cmdw) < 0)
    return false;
  memset (&nh_io_loop, 0, sizeof nh_io_loop);
  atomic_store (&nh_io_loop.refs, 1);
  nh_io_loop.w = NH_WK_IO;
  nh_io_loop.run = nh_io_main;
  if (!nh_wk_submit (&nh_io_loop))
    return false;
  nh_io_running = true;
  return true;
}

static void
nh_io_command (struct nh_io_cmd *c)
{
  c->next = NULL;
  nh_wk_lock (&nh_io_m);
  if (nh_io_cmd_tail)
    nh_io_cmd_tail->next = c;
  else
    nh_io_cmd_head = c;
  nh_io_cmd_tail = c;
  pthread_mutex_unlock (&nh_io_m);
  atomic_store (&nh_io_cmd_pending, true);
  nh_wk_wake_signal (&nh_io_cmdw);
}

/* The main thread's half of the core.  The harness's plain compile
   uses none of it (its TSan driver does).  */
#define NH_IO_CORE static __attribute__ ((unused))

/* Handle ID for the real fd INFD (inline: read on the main thread), or
   NULL with *WHY set.  */
NH_IO_CORE struct nh_io_h *
nh_io_attach (int64_t id, int infd, ptrdiff_t readmax, bool inl, int *why)
{
  *why = NH_IO_RF_BUDGET;
  if (!nh_wk_fd_budget_ok (NH_IO_FDS))
    {
      nh_wk_fallback (NH_WK_IO, NH_WK_FB_FD_BUDGET);
      return NULL;
    }
  if (!inl && !nh_io_start ())
    {
      *why = NH_IO_RF_THREAD;
      return NULL;
    }
  struct nh_io_h *h = calloc (1, sizeof *h);
  if (!h)
    return NULL;
  h->id = id;
  h->inl = inl;
  h->readmax = readmax;
  h->cap = readmax < NH_IO_CAP_MIN / 4 ? NH_IO_CAP_MIN : 4 * readmax;
  h->wake = (struct nh_wk_wake) { -1, -1 };
  h->copy = fcntl (infd, F_DUPFD_CLOEXEC, 0);
  if (h->copy < 0 || (!inl && nh_wk_wake_open (&h->wake) < 0))
    {
      nh_io_free (h);
      return NULL;
    }
  h->sel = inl ? h->copy : h->wake.rfd;
  h->add = (struct nh_io_cmd) { .op = NH_IO_OP_ADD, .h = h };
  h->detach = (struct nh_io_cmd) { .op = NH_IO_OP_DETACH, .h = h };
  nh_io_note (NH_IO_EV_HANDOVER, id, infd, 0);
  if (!inl)
    nh_io_command (&h->add);
  return h;
}

static void
nh_io_job_free (struct nh_wk_job *job)
{
  free (job);
}

/* Close H's fds directly, without nh-io (the workers are dead).  */
static void
nh_io_direct (struct nh_io_h *h)
{
  nh_io_note (NH_IO_EV_DIRECT, h->id, h->copy, 0);
  if (h->copy >= 0)
    close (h->copy);
  h->copy = -1;
  nh_wk_wake_close (&h->wake);
}

/* The synchronous detach.  Return true when H is the main thread's
   again (nh-io has dropped it and closed its copy), false when it is
   not (a timed-out wait left it to nh-io; or the workers are dead and
   its fds were closed directly, and it is abandoned).  RS is the
   worker-wait phase's reason.  */
static bool
nh_io_detach_1 (struct nh_io_h *h, int rs)
{
  if (h->detached)
    return true;
  h->detached = true;
  if (h->inl)
    {
      close (h->copy);
      h->copy = -1;
      return true;
    }
  if (atomic_load (&nh_wk_dead))
    {
      nh_io_direct (h);
      return false;
    }
  struct nh_wk_job *ack = calloc (1, sizeof *ack);
  NH_WK_ASSERT (ack);
  atomic_store (&ack->refs, 2);
  ack->w = NH_WK_IO;
  ack->free = nh_io_job_free;
  h->ack = ack;
  nh_io_command (&h->detach);
  int r;
  while ((r = nh_wk_wait (ack, NH_IO_DETACH_NS, NH_IO_PH, rs)) == 0)
    if (!atomic_exchange (&h->gone, true))
      {
	/* nh-io has not got to it: it frees H when it does.  */
	nh_io_note (NH_IO_EV_TIMEOUT, h->id, -1, 0);
	nh_wk_job_unref (ack);
	return false;
      }
  if (r < 0)
    {
      nh_io_direct (h);
      return false;
    }
  nh_wk_job_unref (ack);
  return true;
}

NH_IO_CORE bool
nh_io_detach (struct nh_io_h *h)
{
  return nh_io_detach_1 (h, NH_IO_RS);
}

/* Whether the chunks C (a list) were all decoded, each going on from
   the carry the one before left: then their decodes, joined, are the
   decode of their bytes joined (nh_io_take keeps them only so).  */
static bool
nh_io_run_ok (struct nh_io_chunk *c)
{
  for (; c; c = c->next)
    {
      struct nh_io_chunk *d = c->next;
      if (!c->dec
	  || (d && (d->sys != c->sys || d->gen != c->gen
		    || d->ncin != c->ncout
		    || memcmp (d->data, c->cout, c->ncout) != 0)))
	return false;
    }
  return true;
}

/* Copy up to SIZE bytes of H's queued chunks into BUF (Task 19): the
   first chunk, or as much of it as fits, then whole chunks while they
   fit; never more than is queued now.  Return the count; 0 at EOF; -1
   with errno set to the read error, or to EAGAIN when nothing is queued
   (the wake fd is cleared then).  When every chunk taken was taken
   whole and decoded in one run, they stay in h->last for nh_io_decoded
   and nh_io_insert: a decode is any use only for bytes delivered in one
   piece.  */
NH_IO_CORE ptrdiff_t
nh_io_take (struct nh_io_h *h, char *buf, ptrdiff_t size)
{
  ptrdiff_t n = 0;
  int err = EAGAIN;
  struct nh_io_chunk *taken = NULL, **tail = &taken;
  bool whole = true;
  nh_io_chunks_free (h->last);
  h->last = NULL;
  nh_wk_lock (&nh_io_m);
  for (struct nh_io_chunk *c; (c = h->head) && n < size; )
    {
      ptrdiff_t avail = c->len - c->off;
      /* After the first chunk, whole chunks only.  */
      if (n > 0 && avail > size - n)
	break;
      ptrdiff_t k = avail < size - n ? avail : size - n;
      if (c->off != c->ncin || k < avail)
	whole = false;
      memcpy (buf + n, c->data + c->off, k);
      c->off += k;
      h->queued -= k;
      n += k;
      if (c->off < c->len)
	break;
      h->head = c->next;
      if (!h->head)
	h->tail = NULL;
      c->next = NULL;
      *tail = c;
      tail = &c->next;
    }
  if (n == 0)
    {
      if (h->eof)
	err = 0;
      else if (h->err)
	err = h->err;
    }
  if (!h->head && !h->eof && !h->err && !h->inl)
    nh_wk_wake_clear (&h->wake);
  bool uncapped = (n > 0 && atomic_load (&h->capped)
		   && h->queued < h->cap);
  if (uncapped)
    atomic_store (&h->capped, false);
  pthread_mutex_unlock (&nh_io_m);
  if (uncapped)
    nh_io_poke ();
  if (taken && whole && nh_io_run_ok (taken))
    h->last = taken;
  else
    nh_io_chunks_free (taken);
  if (n > 0)
    {
      nh_wk_stat_add (NH_WK_IO, NH_WK_ST_QUEUED_BYTES, -n);
      return n;
    }
  if (err == 0)
    return 0;
  errno = err;
  return -1;
}

/* Inline: read the copy, as read_process_output reads the real fd.  */
NH_IO_CORE ptrdiff_t
nh_io_take_inline (struct nh_io_h *h, char *buf, ptrdiff_t size)
{
  ssize_t r;
  nh_io_chunks_free (h->last);
  h->last = NULL;
  do
    r = read (h->copy, buf, size);
  while (r < 0 && errno == EINTR);
  if (r >= 0)
    nh_io_note (r > 0 ? NH_IO_EV_READ : NH_IO_EV_EOF, h->id, h->copy, r);
  /* Decode here what nh-io would decode (Task 18), so that inline mode
     takes the same delivery path.  */
  if (r > 0)
    {
      int err = errno;
      struct nh_io_chunk *c = nh_io_chunk_new (h, (unsigned char *) buf, r);
      if (c && c->dec)
	h->last = c;
      else
	free (c);
      errno = err;
    }
  return r;
}

/* For Task 4's crash rows: nh-io dereferences NULL at its next look at
   its commands.  Return false if nh-io is not running.  */
bool
nh_io_test_crash (void)
{
  if (!nh_io_running)
    return false;
  atomic_store (&nh_io_crash, true);
  atomic_store (&nh_io_cmd_pending, true);
  nh_wk_wake_signal (&nh_io_cmdw);
  return true;
}

#ifndef NH_WK_HARNESS

/* The process side.  Handles by real fd: nh_io_tab is the main
   thread's; nh_io_live is what signal context may read (Task 17's
   pause), published last at hand-over and cleared first at hand-back.
   The opt-out, the "ended" mark of a drained process and the last
   refusal are per real fd too (struct Lisp_Process gets no field),
   cleared when the fd closes.  */
static struct nh_io_h *nh_io_tab[FD_SETSIZE];
static struct nh_io_h *_Atomic nh_io_live[FD_SETSIZE];
static bool nh_io_optout[FD_SETSIZE];
static bool nh_io_ended[FD_SETSIZE];
static unsigned char nh_io_refused[FD_SETSIZE];
static int64_t nh_io_next_id;

#ifdef HAVE_GNUTLS
/* STARTTLS, per real fd, cleared when the fd closes: the "never hand
   over again" mark, the bytes reclaimed for GnuTLS (the main thread's;
   GnuTLS reads only there), whether a pull function was installed, and
   the counts for --test-io-info.  */
struct nh_io_tls_left
{
  gnutls_session_t session;	/* whose pull function reads them */
  ptrdiff_t len, off;
  int err;			/* nh-io's read error after the bytes */
  unsigned char bytes[];
};
static bool nh_io_tls[FD_SETSIZE];
static struct nh_io_tls_left *nh_io_tls_left[FD_SETSIZE];
static bool nh_io_tls_pull_on[FD_SETSIZE];
static int64_t nh_io_tls_reclaimed[FD_SETSIZE], nh_io_tls_pulled[FD_SETSIZE];
#endif

static const char *const nh_io_refusal_names[NH_IO_RF_N] = {
  "none", "off", "switch", "tls", "datagram", "server", "serial", "filter-t",
  "connecting", "opt-out", "fd-budget", "thread", "ended"
};

static struct nh_io_h *
nh_io_get (struct Lisp_Process *p)
{
  return 0 <= p->infd && p->infd < FD_SETSIZE ? nh_io_tab[p->infd] : NULL;
}

static bool
nh_io_switch (Lisp_Object sym)
{
  Lisp_Object v = find_symbol_value (sym);
  return !BASE_EQ (v, Qunbound) && !NILP (v);
}

/* Why P may not be handed over now (an enum nh_io_refusal), or 0.  */
static int
nh_io_eligible (struct Lisp_Process *p)
{
  Lisp_Object kind;
  if (!nh_io_switch (Qnidhogg_workers_process_reader))
    return NH_IO_RF_OFF;
  if (nh_io_ended[p->infd])
    return NH_IO_RF_ENDED;
#ifdef HAVE_GNUTLS
  if (!NILP (p->gnutls_boot_parameters) || p->gnutls_p || nh_io_tls[p->infd])
    return NH_IO_RF_TLS;
#endif
  if (EQ (p->type, Qnetwork))
    {
      if (p->socktype == SOCK_DGRAM)
	return NH_IO_RF_DATAGRAM;
      if (p->is_server || EQ (p->status, Qlisten))
	return NH_IO_RF_SERVER;
      if (EQ (p->status, Qconnect)
	  || (CONSP (p->status) && EQ (XCAR (p->status), Qconnect)))
	return NH_IO_RF_CONNECTING;
      kind = Qnidhogg_workers_reader_sockets;
    }
  else if (EQ (p->type, Qreal) && p->pty_out)
    kind = Qnidhogg_workers_reader_ptys;
  else if (EQ (p->type, Qreal) || EQ (p->type, Qpipe))
    kind = Qnidhogg_workers_reader_pipes;
  else
    return NH_IO_RF_SERIAL;
  if (EQ (p->filter, Qt))
    return NH_IO_RF_FILTER_T;
  if (nh_io_optout[p->infd])
    return NH_IO_RF_OPT_OUT;
  if (!nh_io_switch (kind))
    return NH_IO_RF_SWITCH;
  return 0;
}

bool
nh_io_handover (struct Lisp_Process *p)
{
  int fd = p->infd;
  if (fd < 0 || fd >= FD_SETSIZE)
    return false;
  if (nh_io_tab[fd])
    return !nh_io_tab[fd]->detached;
  int why = nh_io_eligible (p);
  struct nh_io_h *h = why ? NULL : nh_io_attach (nh_io_next_id + 1, fd,
						  p->readmax, nh_wk_inline,
						  &why);
  if (!h)
    {
      nh_io_refused[fd] = why;
      return false;
    }
  nh_io_next_id++;
  nh_io_refused[fd] = 0;
  nh_io_tab[fd] = h;
  atomic_store (&nh_io_live[fd], h);
  return true;
}

int
nh_io_select_fd (struct Lisp_Process *p)
{
  struct nh_io_h *h = nh_io_get (p);
  return h ? h->sel : -1;
}

#ifdef HAVE_GNUTLS
/* A STARTTLS boot handed H (the real fd FD's) back: move the bytes it
   read and did not deliver, and a read error after them, to FD's
   reclaimed bytes for GnuTLS.  H's FIFO is left empty, so the hand-back
   goes on to the real fd at once, and the filter never sees them.
   (When the hand-back's wait timed out, nh-io kept H and its bytes:
   only a test's pause point can make that happen.)  */
static void
nh_io_tls_reclaim (struct nh_io_h *h, int fd)
{
  nh_wk_lock (&nh_io_m);
  ptrdiff_t n = 0;
  for (struct nh_io_chunk *c = h->head; c; c = c->next)
    n += c->len - c->off;
  struct nh_io_tls_left *old = nh_io_tls_left[fd];
  ptrdiff_t had = old ? old->len - old->off : 0;
  if (n > 0 || h->err)
    {
      struct nh_io_tls_left *l = malloc (sizeof *l + had + n);
      NH_WK_ASSERT (l);
      *l = (struct nh_io_tls_left) { .len = had + n,
				     .err = h->err ? h->err : old ? old->err : 0 };
      if (old)
	memcpy (l->bytes, old->bytes + old->off, had);
      ptrdiff_t at = had;
      for (struct nh_io_chunk *c = h->head, *next; c; c = next)
	{
	  next = c->next;
	  /* The raw bytes: DATA starts with the decoder's carry-in.  */
	  memcpy (l->bytes + at, c->data + c->off, c->len - c->off);
	  at += c->len - c->off;
	  free (c);
	}
      free (old);
      nh_io_tls_left[fd] = l;
      nh_io_tls_reclaimed[fd] += n;
    }
  h->head = h->tail = NULL;
  h->queued = 0;
  h->eof = false;
  h->err = 0;
  pthread_mutex_unlock (&nh_io_m);
  nh_wk_stat_add (NH_WK_IO, NH_WK_ST_QUEUED_BYTES, -n);
}

/* GnuTLS's reads for a session that has reclaimed bytes: those first,
   then the fd, as GnuTLS's own pull reads it.  */
static ssize_t
nh_io_tls_pull (gnutls_transport_ptr_t ptr, void *data, size_t size)
{
  int fd = (intptr_t) ptr;
  struct nh_io_tls_left *l
    = 0 <= fd && fd < FD_SETSIZE ? nh_io_tls_left[fd] : NULL;
  if (l)
    {
      if (l->off < l->len)
	{
	  ptrdiff_t n = l->len - l->off;
	  if ((size_t) n > size)
	    n = size;
	  memcpy (data, l->bytes + l->off, n);
	  l->off += n;
	  nh_io_tls_pulled[fd] += n;
	  return n;
	}
      int err = l->err;
      nh_io_tls_left[fd] = NULL;
      free (l);
      if (err)
	{
	  errno = err;
	  return -1;
	}
    }
  return recv (fd, data, size, 0);
}

# ifdef HAVE_GNUTLS3
/* Whether a read would find something within MS milliseconds.  */
static int
nh_io_tls_pull_timeout (gnutls_transport_ptr_t ptr, unsigned int ms)
{
  int fd = (intptr_t) ptr;
  struct nh_io_tls_left *l
    = 0 <= fd && fd < FD_SETSIZE ? nh_io_tls_left[fd] : NULL;
  if (l && (l->off < l->len || l->err))
    return 1;
  struct pollfd pfd = { .fd = fd, .events = POLLIN };
  /* GNUTLS_INDEFINITE_TIMEOUT is UINT_MAX.  */
  return poll (&pfd, 1, ms > INT_MAX ? -1 : (int) ms);
}
# endif
#endif

void
nh_io_handback (struct Lisp_Process *p, int reason)
{
  int fd = p->infd;
  if (fd < 0 || fd >= FD_SETSIZE)
    return;
  if (reason == NH_IO_CLOSE)
    {
      nh_io_optout[fd] = false;
      nh_io_ended[fd] = false;
      nh_io_refused[fd] = 0;
#ifdef HAVE_GNUTLS
      nh_io_tls[fd] = false;
      free (nh_io_tls_left[fd]);
      nh_io_tls_left[fd] = NULL;
      nh_io_tls_pull_on[fd] = false;
      nh_io_tls_reclaimed[fd] = nh_io_tls_pulled[fd] = 0;
#endif
    }
  struct nh_io_h *h = nh_io_tab[fd];
  if (!h)
    return;
  atomic_store (&nh_io_live[fd], NULL);
  int64_t id = h->id;
  if (!nh_io_detach_1 (h, (reason == NH_IO_STATUS
			   ? NH_IO_RS_DRAIN : NH_IO_RS)))
    {
      nh_io_tab[fd] = NULL;
      h = NULL;
    }
#ifdef HAVE_GNUTLS
  else if (reason != NH_IO_CLOSE && nh_io_tls[fd])
    nh_io_tls_reclaim (h, fd);
#endif
  if (reason == NH_IO_CLOSE)
    {
      /* The real fd closes next (deactivate_process).  */
      nh_io_note (NH_IO_EV_CLOSE, id, fd, 0);
      if (h)
	{
	  nh_io_tab[fd] = NULL;
	  nh_io_free (h);
	}
    }
}

bool
nh_io_done (struct Lisp_Process *p)
{
  struct nh_io_h *h = nh_io_get (p);
  if (!h)
    return true;
  if (!h->detached)
    return false;
  if (atomic_load (&nh_wk_dead))
    return true;
  nh_wk_lock (&nh_io_m);
  bool done = !h->head && !h->eof && !h->err;
  pthread_mutex_unlock (&nh_io_m);
  return done;
}

/* The fatal path (Task 4's kill_buffer_processes hook): once the workers
   are dead, a reader-owned process's fds all close here, with no
   handshake and no lock, and P forgets them so nothing closes them
   twice.  The handle is abandoned: nh-io may still hold it.  */
void
nh_io_dead_close (struct Lisp_Process *p)
{
  struct nh_io_h *h = nh_io_get (p);
  if (!h || !atomic_load (&nh_wk_dead))
    return;
  atomic_store (&nh_io_live[p->infd], NULL);
  nh_io_tab[p->infd] = NULL;
  h->detached = true;
  nh_io_direct (h);
  for (int i = 0; i < PROCESS_OPEN_FDS; i++)
    if (p->open_fd[i] >= 0)
      {
	emacs_close (p->open_fd[i]);
	p->open_fd[i] = -1;
      }
  p->infd = p->outfd = -1;
}

void
nh_io_release (struct Lisp_Process *p)
{
  struct nh_io_h *h = nh_io_get (p);
  if (!h)
    return;
  NH_WK_ASSERT (h->detached);
  nh_io_tab[p->infd] = NULL;
  nh_io_free (h);
}

/* Delivery (Task 19).  A filter's work per delivery: 4 ms (MC-3).  */
#define NH_IO_FILTER_NS 4e6
/* Before the filter's cost is known: a Linux pty master read returns at
   most N_TTY_BUF_SIZE; a pipe's or a socket's, 64 KB.  */
enum { NH_IO_PTY_READ = 4096, NH_IO_PIPE_READ = 64 * 1024 };
/* The least a delivery to a filter takes, however costly its estimate:
   a filter whose cost is per call, not per byte, is not cut to a byte
   at a time.  */
enum { NH_IO_DELIVER_MIN = 512 };

static int64_t
nh_io_now (void)
{
  struct timespec ts;
  clock_gettime (CLOCK_MONOTONIC, &ts);
  return (int64_t) ts.tv_sec * 1000000000 + ts.tv_nsec;
}

/* The most one delivery to P takes, SIZE being the read it replaces
   (read-process-output-max).  The default filter is C: SIZE.  Any other
   filter: also its estimated 4 ms of work, at least NH_IO_DELIVER_MIN
   bytes; before an estimate, the fd kind's natural read.  */
static ptrdiff_t
nh_io_limit (struct Lisp_Process *p, struct nh_io_h *h, ptrdiff_t size)
{
  if (fast_read_process_output
      && EQ (p->filter, Qinternal_default_process_filter))
    return size;
  double lim = (!h->cost_known ? (p->pty_out ? NH_IO_PTY_READ
				  : NH_IO_PIPE_READ)
		: h->cost > 0 ? NH_IO_FILTER_NS / h->cost
		: size);
  if (lim < NH_IO_DELIVER_MIN)
    lim = NH_IO_DELIVER_MIN;
  return lim < size ? (ptrdiff_t) lim : size;
}

ptrdiff_t
nh_io_read (struct Lisp_Process *p, int channel, char *buf, ptrdiff_t size,
	    bool *handled)
{
  struct nh_io_h *h = nh_io_get (p);
  *handled = h && (channel == h->sel || channel == p->infd);
  if (!*handled)
    return 0;
  size = nh_io_limit (p, h, size);
  if (h->inl)
    {
      if (h->copy >= 0)
	return nh_io_take_inline (h, buf, size);
      *handled = false;
      return 0;
    }
  /* The fatal path takes no lock (nh-io may hold it forever).  */
  if (atomic_load (&nh_wk_dead))
    {
      errno = EAGAIN;
      return -1;
    }
  /* (nh_io_take forgets the last delivery's decode too.)  */
  ptrdiff_t n = nh_io_take (h, buf, size);
  /* A drain (CHANNEL is the real fd) of a handed-back process reads the
     real fd once the handle's bytes are delivered.  */
  if (n < 0 && errno == EAGAIN && h->detached && channel == p->infd)
    *handled = false;
  return n;
}

/* Pause the reader of the process whose real fd is INFD: nh-io stops
   reading it after the read in progress.  Return the fd the select set
   holds for it (its wake fd, or inline its copy), or INFD when no
   reader owns it.  Async-signal-safe (the SIGCHLD handler's
   delete_read_fd, §4 item 11): the lock-free table, an atomic flag and
   one write to the command fd, errno kept; no mutex, no malloc.  The
   handler runs on the main thread, which frees a handle only after
   clearing its nh_io_live entry.  */
int
nh_io_pause (int infd)
{
  if (infd < 0 || infd >= FD_SETSIZE)
    return infd;
  struct nh_io_h *h = atomic_load (&nh_io_live[infd]);
  if (!h)
    return infd;
  if (!atomic_exchange (&h->paused, true) && !h->inl)
    nh_io_poke ();
  return h->sel;
}

/* Undo nh_io_pause (add_process_read_fd, main thread).  */
void
nh_io_resume (int infd)
{
  struct nh_io_h *h = 0 <= infd && infd < FD_SETSIZE ? nh_io_tab[infd] : NULL;
  if (h && !h->detached && atomic_exchange (&h->paused, false) && !h->inl)
    nh_io_poke ();
}

/* Whether poll finds one of the N fds at PFD readable within MS (a
   signal may cut a wait short: the caller loops).  */
static bool
nh_io_poll_any (struct pollfd *pfd, int n, int ms)
{
  int r;
  do
    r = poll (pfd, n, ms);
  while (r < 0 && errno == EINTR && ms == 0);
  return r > 0;
}

/* Before a select over MASK with TIMEOUT in wait_reading_process_output.
   A zero timeout asks what output is there now, which today is what
   the kernel holds: `accept-process-output' with a zero timeout gets it,
   and status_notify runs sentinels only when such a select finds none
   (so a process's output, and another's that came before it exited, a
   `:stderr' pipe's EOF, reach filters first).  A reader-owned process's
   wake fd is readable only once nh-io has read: while nh-io reads, or
   has yet to read, output of a process whose wake fd MASK holds (it
   does while the process is read: not paused, not handed back), wait
   for it to land on a wake fd, so that the select finds it as today's
   finds it in the kernel.  Bounded: only an nh-io held at a test's
   pause point reaches the bound.  */
void
nh_io_settle (fd_set *mask, struct timespec timeout)
{
  if (timeout.tv_sec != 0 || timeout.tv_nsec != 0
      || atomic_load (&nh_wk_dead))
    return;
  struct pollfd real[FD_SETSIZE], wake[FD_SETSIZE];
  struct nh_io_h *hs[FD_SETSIZE];
  int n = 0;
  for (int fd = 0; fd < FD_SETSIZE; fd++)
    {
      struct nh_io_h *h = nh_io_tab[fd];
      if (h && !h->inl && !h->detached && FD_ISSET (h->sel, mask))
	{
	  real[n] = (struct pollfd) { .fd = fd, .events = POLLIN };
	  wake[n] = (struct pollfd) { .fd = h->sel, .events = POLLIN };
	  hs[n++] = h;
	}
    }
  if (n == 0)
    return;
  int depth = -1;
  bool waiting = false;
  struct timespec t;
  clock_gettime (CLOCK_MONOTONIC, &t);
  int64_t deadline = (t.tv_sec * INT64_C (1000000000) + t.tv_nsec
		      + NH_IO_DETACH_NS);
  /* A wake fd readable: the select finds output, as today's would.
     Else nothing in the kernel and no read in progress (checked in that
     order: a read that ended in between wrote its wake fd): nothing is
     on its way.  */
  while (!nh_io_poll_any (wake, n, 0))
    {
      bool pending = nh_io_poll_any (real, n, 0);
      for (int i = 0; !pending && i < n; i++)
	pending = atomic_load (&hs[i]->reading);
      if (!pending)
	break;
      clock_gettime (CLOCK_MONOTONIC, &t);
      int64_t left = deadline - (t.tv_sec * INT64_C (1000000000) + t.tv_nsec);
      if (left <= 0)
	break;
      if (!waiting)
	{
	  waiting = true;
	  depth = NH_WK_PHASE_PUSH (NH_IO_PH, NH_IO_RS_DRAIN);
	  nh_wk_wait_begins ();
	}
      nh_io_poll_any (wake, n, (int) (left / 1000000) + 1);
    }
  if (waiting)
    NH_WK_PHASE_POP (depth);
}

/* Whether P is going down: its status says so, or (a subprocess) the
   SIGCHLD handler saw it exit.  */
static bool
nh_io_ending (struct Lisp_Process *p)
{
  if (EQ (p->type, Qreal) && !p->alive)
    return true;
  Lisp_Object s = CONSP (p->status) ? XCAR (p->status) : p->status;
  return (EQ (s, Qexit) || EQ (s, Qsignal) || EQ (s, Qfailed)
	  || EQ (s, Qclosed));
}

/* Before the drains (status_notify's, a non-running wait_proc's): true
   when P's reader must hand it back (reason NH_IO_STATUS) first, so the
   main thread delivers the FIFO and then reads the real fd itself, in
   order, before the sentinel.  P is then never handed over again.  */
bool
nh_io_drain (struct Lisp_Process *p)
{
  struct nh_io_h *h = nh_io_get (p);
  if (!h || h->detached || !nh_io_ending (p))
    return false;
  nh_io_ended[p->infd] = true;
  return true;
}

/* Decoding (Task 18).  The main thread's generation and system go to
   the reader in one atomic word; each chunk records the generation it
   was decoded under, and its delivery uses that decode only while the
   generation, the system and the bytes are still what the main thread
   would decode.  Otherwise (a stale generation, a translation table,
   inhibit-eol-conversion, a carryover the reader did not have, a
   partial delivery) today's decode runs on the raw bytes the chunk
   kept, with the real fd's struct.  */

static void
nh_io_set_coding (struct nh_io_h *h, int sys)
{
  uint64_t gen = NH_IO_GEN (atomic_load (&h->coding)) + 1;
  atomic_store (&h->coding, gen << 4 | sys);
}

void
nh_io_coding_changed (struct Lisp_Process *p, struct coding_system *coding)
{
  struct nh_io_h *h = nh_io_get (p);
  if (h)
    nh_io_set_coding (h, nh_dec_classify (coding));
}

void
nh_io_coding_settled (struct Lisp_Process *p, struct coding_system *coding)
{
  struct nh_io_h *h = nh_io_get (p);
  if (!h)
    return;
  int sys = nh_dec_classify (coding);
  if (sys != NH_IO_SYS (atomic_load (&h->coding)))
    nh_io_set_coding (h, sys);
}

/* The chunks of H's last take (a list, now the caller's to free) when
   their decodes, joined, are what the main thread would make of the
   NBYTES at CHARS (its carryover, then the bytes delivered) with CODING;
   else NULL.  Their *DLEN bytes hold *NCHARS characters.  */
static struct nh_io_chunk *
nh_io_usable (struct nh_io_h *h, struct coding_system *coding,
	      const char *chars, ptrdiff_t nbytes, ptrdiff_t *dlen,
	      ptrdiff_t *nchars)
{
  if (!h || !h->last)
    return NULL;
  struct nh_io_chunk *c = h->last;
  h->last = NULL;
  /* nh_io_take kept one run: one generation, one system, each chunk
     going on from the one before.  */
  ptrdiff_t len = c->ncin;
  *dlen = *nchars = 0;
  for (struct nh_io_chunk *k = c; k; k = k->next)
    {
      len += k->len - k->ncin;
      *dlen += k->dlen;
      *nchars += k->nchars;
    }
  if (c->gen == NH_IO_GEN (atomic_load (&h->coding))
      && c->sys == nh_dec_classify (coding)
      && !inhibit_eol_conversion
      && (NILP (Venable_character_translation)
	  || NILP (Vstandard_translation_table_for_decode))
      && nbytes == len
      && memcmp (chars, c->data, c->ncin) == 0)
    return c;
  h->nredecoded++;
  nh_io_chunks_free (c);
  return NULL;
}

/* CODING's carryover is what the last of the chunks C left undecoded,
   and P's delivery used their decodes.  Free C.  */
static void
nh_io_used (struct nh_io_h *h, struct coding_system *coding,
	    struct nh_io_chunk *c)
{
  struct nh_io_chunk *last = c;
  while (last->next)
    last = last->next;
  coding->carryover_bytes = last->ncout;
  memcpy (coding->carryover, last->cout, last->ncout);
  h->ndecoded++;
  nh_io_chunks_free (c);
}

bool
nh_io_decoded (struct Lisp_Process *p, struct coding_system *coding,
	       const char *chars, ptrdiff_t nbytes)
{
  struct nh_io_h *h = nh_io_get (p);
  ptrdiff_t dlen, nchars;
  struct nh_io_chunk *c = nh_io_usable (h, coding, chars, nbytes, &dlen,
					&nchars);
  if (!c)
    return false;
  Lisp_Object s = (NH_DEC_MULTIBYTE (c->sys)
		   ? make_uninit_multibyte_string (nchars, dlen)
		   : make_uninit_string (dlen));
  unsigned char *d = SDATA (s);
  for (struct nh_io_chunk *k = c; k; k = k->next)
    {
      memcpy (d, k->dec, k->dlen);
      d += k->dlen;
    }
  coding->dst_object = s;
  coding->produced = dlen;
  coding->produced_char = nchars;
  nh_io_used (h, coding, c);
  return true;
}

/* The default filter's insertion (Task 19), from
   read_and_insert_process_output where it would decode the NREAD bytes
   at BUF into the current buffer, P's, at point: true when it inserted
   nh-io's decodes there instead, as that decode inserts (before
   markers, no text properties, compositions updated, point left before
   the text), with CODING's produced counts and carryover set as it
   sets them.  A unibyte system's bytes from 0x80 become raw-byte
   characters in a multibyte buffer; a UTF-8 decode is no use in a
   unibyte buffer.  */
bool
nh_io_insert (struct Lisp_Process *p, struct coding_system *coding,
	      const char *buf, ptrdiff_t nread)
{
  struct nh_io_h *h = nh_io_get (p);
  ptrdiff_t dlen, nchars;
  struct nh_io_chunk *c = nh_io_usable (h, coding, buf, nread, &dlen,
					&nchars);
  if (!c)
    return false;
  bool multibyte = !NILP (BVAR (current_buffer, enable_multibyte_characters));
  bool utf8 = NH_DEC_MULTIBYTE (c->sys);
  if (!multibyte && utf8)
    {
      h->nredecoded++;
      nh_io_chunks_free (c);
      return false;
    }
  bool widen = multibyte && !utf8;
  ptrdiff_t ibytes = 0;
  for (struct nh_io_chunk *k = c; k; k = k->next)
    ibytes += widen ? count_size_as_multibyte (k->dec, k->dlen) : k->dlen;
  ptrdiff_t ichars = !multibyte ? ibytes : utf8 ? nchars : dlen;
  USE_SAFE_ALLOCA;
  unsigned char *text = c->dec;
  if (widen || c->next)
    {
      unsigned char *d = text = SAFE_ALLOCA (ibytes);
      for (struct nh_io_chunk *k = c; k; k = k->next)
	if (widen)
	  d += str_to_multibyte (d, k->dec, k->dlen);
	else
	  {
	    memcpy (d, k->dec, k->dlen);
	    d += k->dlen;
	  }
    }
  ptrdiff_t pt = PT, pt_byte = PT_BYTE;
  insert_1_both ((char *) text, ichars, ibytes, false, false, true);
  update_compositions (pt, pt + ichars, CHECK_ALL);
  TEMP_SET_PT_BOTH (pt, pt_byte);
  SAFE_FREE ();
  XSETBUFFER (coding->dst_object, current_buffer);
  coding->produced = ibytes;
  coding->produced_char = ichars;
  nh_io_used (h, coding, c);
  return true;
}

/* The ns of the filter calls nh_io_filter_end counted, for the
   deliveries around them.  */
static int64_t nh_io_filter_ns;

/* Around read_and_dispose_of_process_output, for a delivery the reader
   made (READER): the `deliver' phase, and its ns without the filter's
   (a delivery nested in a filter counts in both).  */
void
nh_io_deliver_begin (bool reader, struct nh_io_deliver *d)
{
  *d = (struct nh_io_deliver) { .depth = -1 };
  if (!reader)
    return;
  d->t0 = nh_io_now ();
  d->filter_ns = nh_io_filter_ns;
  d->depth = NH_WK_PHASE_PUSH (NH_WK_PH_DELIVER, 0);
}

void
nh_io_deliver_end (struct nh_io_deliver *d)
{
  if (!d->t0)
    return;
  NH_WK_PHASE_POP (d->depth);
  int64_t ns = (nh_io_now () - d->t0) - (nh_io_filter_ns - d->filter_ns);
  if (ns > 0)
    nh_wk_phase_ns_add (NH_WK_PH_DELIVER, 0, ns);
}

/* Around a filter call with P's output: the filter's cost.  The start
   for nh_io_filter_end, 0 when the reader does not own P.  */
int64_t
nh_io_filter_begin (struct Lisp_Process *p)
{
  return nh_io_get (p) ? nh_io_now () : 0;
}

/* The filter called at T0 got NBYTES bytes (0: it was not called).
   Count its ns, and fold its ns per byte into P's estimate (a moving
   average, a quarter for the newest call) unless a test fixed it.  P's
   filter may have deleted P or handed it back meanwhile.  */
void
nh_io_filter_end (struct Lisp_Process *p, int64_t t0, ptrdiff_t nbytes)
{
  if (!t0 || nbytes <= 0)
    return;
  int64_t ns = nh_io_now () - t0;
  nh_io_filter_ns += ns;
  nh_wk_phase_ns_add (NH_WK_PH_FILTER, 0, ns);
  struct nh_io_h *h = nh_io_get (p);
  if (!h || h->cost_fixed)
    return;
  double per = (double) ns / nbytes;
  h->cost = h->cost_known ? h->cost + (per - h->cost) / 4 : per;
  h->cost_known = true;
}

/* Fgnutls_boot's two hooks (§8.2 "A STARTTLS upgrade", MC-32).  With
   GNUTLS_STATE NULL, before any GnuTLS call: mark P's fd never to be
   handed over again and, if the reader owns P, hand it back now
   (synchronously, the wake fd replaced by the real fd in the select
   set); the bytes the reader had not delivered are reclaimed for
   GnuTLS.  Return how many bytes are reclaimed.  With the session
   GNUTLS_STATE, once gnutls_init made it: when bytes are reclaimed,
   give the session the pull function that returns them before it reads
   the fd; return 1 if it did, else 0.  */
int
nh_io_tls_boot (struct Lisp_Process *p, void *gnutls_state)
{
#ifdef HAVE_GNUTLS
  int fd = p->infd;
  if (fd < 0 || fd >= FD_SETSIZE)
    return 0;
  if (!gnutls_state)
    {
      nh_io_tls[fd] = true;
      if (nh_io_tab[fd])
	{
	  Lisp_Object proc;
	  XSETPROCESS (proc, p);
	  nh_io_proc_reader (proc, false);
	}
      struct nh_io_tls_left *l = nh_io_tls_left[fd];
      return l ? l->len - l->off : 0;
    }
  struct nh_io_tls_left *l = nh_io_tls_left[fd];
  if (!l)
    return 0;
  l->session = gnutls_state;
  gnutls_transport_set_pull_function (gnutls_state, nh_io_tls_pull);
# ifdef HAVE_GNUTLS3
  gnutls_transport_set_pull_timeout_function (gnutls_state,
					      nh_io_tls_pull_timeout);
# endif
  nh_io_tls_pull_on[fd] = true;
  return 1;
#else
  (void) p;
  (void) gnutls_state;
  return 0;
#endif
}

/* emacs_gnutls_record_check_pending: the reclaimed bytes GNUTLS_STATE's
   pull function has yet to return, which the fd's readability cannot
   show (fd audit finding 10).  */
ptrdiff_t
nh_io_tls_pending (void *gnutls_state)
{
#ifdef HAVE_GNUTLS
  gnutls_transport_ptr_t recv_ptr, send_ptr;
  gnutls_transport_get_ptr2 (gnutls_state, &recv_ptr, &send_ptr);
  int fd = (intptr_t) recv_ptr;
  struct nh_io_tls_left *l
    = 0 <= fd && fd < FD_SETSIZE ? nh_io_tls_left[fd] : NULL;
  /* A read error after them counts too: the pull returns it.  */
  return (l && l->session == gnutls_state
	  ? l->len - l->off + (l->err != 0) : 0);
#else
  (void) gnutls_state;
  return 0;
#endif
}

DEFUN ("nidhogg-set-process-reader", Fnidhogg_set_process_reader,
       Snidhogg_set_process_reader, 2, 2, 0,
       doc: /* Say whether the reader thread may own PROCESS's output.
With FLAG nil, PROCESS's output is read on the main thread from now on,
as without the reader: the reader hands it back, after delivering what
it had read.  With FLAG non-nil (every process starts so), the reader
may own it again, when PROCESS is eligible and its kind of process is
switched on (`nidhogg-workers-process-reader' and its sub-switches).
Return FLAG.  */)
  (Lisp_Object process, Lisp_Object flag)
{
  CHECK_PROCESS (process);
  struct Lisp_Process *p = XPROCESS (process);
  if (0 <= p->infd && p->infd < FD_SETSIZE)
    {
      nh_io_optout[p->infd] = NILP (flag);
      nh_io_proc_reader (process, !NILP (flag));
    }
  return flag;
}

DEFUN ("nidhogg-workers--test-io-info", Fnidhogg_workers__test_io_info,
       Snidhogg_workers__test_io_info, 1, 1, 0,
       doc: /* Return what the reader knows of PROCESS; for tests.
A plist: `:owned' (non-nil while the reader owns PROCESS's output),
`:mode' (`threads' or `inline'), `:select' (the fd the select set holds
for it), `:copy' (the reader's copy of its fd), `:wake' (its wake fd;
nil inline), `:fds' (how many fds the reader holds for it), `:queued'
\(bytes read and not yet delivered), `:cap' (the most `:queued' may
reach), `:paused' (non-nil while paused, as by `set-process-filter' t),
`:refused' (why the last hand-over was refused: `off', `switch', `tls',
`datagram', `server', `serial', `filter-t', `connecting', `opt-out',
`fd-budget', `thread', `ended' (handed back for its last drain); nil if
none was), `:coding-generation' (how often its decoding was set up since
the hand-over), `:decoder' (the stateless system the reader decodes
with, such as `utf-8-unix' or `raw-text-dos'; nil: the main thread
decodes), `:decoded' (deliveries that used the reader's decodes: a
filter's string, or the default filter's insertion), `:redecoded'
\(deliveries whose decode was stale, so the main thread decoded the
bytes again) and `:filter-cost' (the filter's estimated nanoseconds per
byte, a float; nil until a filter call measured it).  For a process the
reader does not own, in a build with GnuTLS, also: `:tls-reclaimed' (bytes the
reader had read and not delivered when `gnutls-boot' took the fd back,
given to GnuTLS instead of the filter), `:tls-pulled' (how many of
them GnuTLS has read), `:tls-left' (how many it has yet to read) and
`:tls-pull' (non-nil if the session got the pull function that returns
them).  A `tls' refusal also means `gnutls-boot' ran on PROCESS.  */)
  (Lisp_Object process)
{
  CHECK_PROCESS (process);
  struct Lisp_Process *p = XPROCESS (process);
  struct nh_io_h *h = nh_io_get (p);
  int why = 0 <= p->infd && p->infd < FD_SETSIZE ? nh_io_refused[p->infd] : 0;
  Lisp_Object refused = why ? intern_c_string (nh_io_refusal_names[why]) : Qnil;
#ifdef HAVE_GNUTLS
  if (!h && 0 <= p->infd && p->infd < FD_SETSIZE)
    {
      int fd = p->infd;
      struct nh_io_tls_left *l = nh_io_tls_left[fd];
      return listn (12, QCowned, Qnil, intern_c_string (":refused"), refused,
		    intern_c_string (":tls-reclaimed"),
		    make_int (nh_io_tls_reclaimed[fd]),
		    intern_c_string (":tls-pulled"),
		    make_int (nh_io_tls_pulled[fd]),
		    intern_c_string (":tls-pull"),
		    nh_io_tls_pull_on[fd] ? Qt : Qnil,
		    intern_c_string (":tls-left"),
		    make_int (l ? l->len - l->off : 0));
    }
#endif
  if (!h)
    return list4 (QCowned, Qnil, intern_c_string (":refused"), refused);
  nh_wk_lock (&nh_io_m);
  ptrdiff_t queued = h->queued;
  pthread_mutex_unlock (&nh_io_m);
  int fds = (h->copy >= 0) + (h->wake.rfd >= 0)
    + (h->wake.wfd >= 0 && h->wake.wfd != h->wake.rfd);
  uint64_t coding = atomic_load (&h->coding);
  return listn (30, QCowned, h->detached ? Qnil : Qt,
		intern_c_string (":mode"), h->inl ? Qinline : Qthreads,
		intern_c_string (":select"), make_int (h->sel),
		intern_c_string (":copy"), h->copy >= 0 ? make_int (h->copy) : Qnil,
		intern_c_string (":wake"),
		h->wake.rfd >= 0 ? make_int (h->wake.rfd) : Qnil,
		intern_c_string (":fds"), make_int (fds),
		intern_c_string (":queued"), make_int (queued),
		intern_c_string (":cap"), make_int (h->cap),
		intern_c_string (":paused"),
		atomic_load (&h->paused) ? Qt : Qnil,
		intern_c_string (":refused"), refused,
		intern_c_string (":coding-generation"),
		make_uint (NH_IO_GEN (coding)),
		intern_c_string (":decoder"),
		nh_dec_system_name (NH_IO_SYS (coding)),
		intern_c_string (":decoded"), make_int (h->ndecoded),
		intern_c_string (":redecoded"), make_int (h->nredecoded),
		intern_c_string (":filter-cost"),
		h->cost_known ? make_float (h->cost) : Qnil);
}

DEFUN ("nidhogg-workers--test-filter-cost", Fnidhogg_workers__test_filter_cost,
       Snidhogg_workers__test_filter_cost, 2, 2, 0,
       doc: /* Fix the estimated cost of PROCESS's filter; for tests.
NS is nanoseconds per byte, a number: from now on a delivery to a filter
of PROCESS takes at most what NS makes 4 ms of the filter's work (at
least 512 bytes), and filter calls no longer move the estimate.  With
NS nil, the estimate is unknown again and the next filter call measures
it.  Return non-nil if the reader owns PROCESS (else nothing changes).  */)
  (Lisp_Object process, Lisp_Object ns)
{
  CHECK_PROCESS (process);
  if (!NILP (ns))
    CHECK_NUMBER (ns);
  struct nh_io_h *h = nh_io_get (XPROCESS (process));
  if (!h)
    return Qnil;
  h->cost_fixed = h->cost_known = !NILP (ns);
  h->cost = NILP (ns) ? 0 : XFLOATINT (ns);
  return Qt;
}

DEFUN ("nidhogg-workers--test-io-log", Fnidhogg_workers__test_io_log,
       Snidhogg_workers__test_io_log, 1, 1, 0,
       doc: /* Start or stop the reader's event log; for tests.
With ON non-nil, empty the log and start it, and return nil.  With ON
nil, stop it and return its events, oldest first (the latest 4096 at
most): each a list (EVENT ID FD N).  EVENT is `handover' (FD the real
fd), `read', `eof' or `error' (by nh-io, or inline by the main thread,
from the copy FD; N bytes), `ack' (nh-io dropped the handle and closed
its copy), `close' (the real fd FD closes next), `timeout' (a hand-back
gave up waiting) or `direct' (fds closed without nh-io).  ID names the
handle: every hand-over makes a new one.  */)
  (Lisp_Object on)
{
  static const char *const names[NH_IO_EV_N] = {
    "handover", "read", "eof", "error", "ack", "close", "timeout", "direct"
  };
  pthread_mutex_lock (&nh_io_log_m);
  Lisp_Object events = Qnil;
  if (NILP (on))
    {
      atomic_store (&nh_io_logging, false);
      int64_t first = nh_io_nlog > NH_IO_LOG ? nh_io_nlog - NH_IO_LOG : 0;
      for (int64_t i = nh_io_nlog - 1; i >= first; i--)
	{
	  struct nh_io_logent *e = &nh_io_log[i % NH_IO_LOG];
	  events = Fcons (list4 (intern_c_string (names[e->ev]), make_int (e->id),
				 make_int (e->fd), make_int (e->n)),
			  events);
	}
    }
  else
    {
      nh_io_nlog = 0;
      atomic_store (&nh_io_logging, true);
    }
  pthread_mutex_unlock (&nh_io_log_m);
  return events;
}

DEFUN ("nidhogg-workers--test-io-crash", Fnidhogg_workers__test_io_crash,
       Snidhogg_workers__test_io_crash, 0, 0, 0,
       doc: /* Make nh-io dereference NULL; for the fatal-path tests.
Return nil if nh-io is not running (no process was handed over in
`threads' mode yet).  */)
  (void)
{
  return nh_io_test_crash () ? Qt : Qnil;
}

DEFUN ("nidhogg-workers--test-io-hold-lock", Fnidhogg_workers__test_io_hold_lock,
       Snidhogg_workers__test_io_hold_lock, 1, 1, 0,
       doc: /* With FLAG non-nil, take nh-io's queue mutex and keep it; nil gives it back.
For the fatal-path tests: nh-io blocks on its next append, and nothing
on the fatal path may take the mutex.  */)
  (Lisp_Object flag)
{
  if (NILP (flag))
    pthread_mutex_unlock (&nh_io_m);
  else
    nh_wk_lock (&nh_io_m);
  return flag;
}

void
nh_io_syms (void)
{
  DEFSYM (Qnidhogg_workers_process_reader, "nidhogg-workers-process-reader");
  DEFSYM (Qnidhogg_workers_reader_pipes, "nidhogg-workers-reader-pipes");
  DEFSYM (Qnidhogg_workers_reader_sockets, "nidhogg-workers-reader-sockets");
  DEFSYM (Qnidhogg_workers_reader_ptys, "nidhogg-workers-reader-ptys");
  DEFSYM (QCowned, ":owned");
  defsubr (&Snidhogg_set_process_reader);
  defsubr (&Snidhogg_workers__test_io_info);
  defsubr (&Snidhogg_workers__test_io_log);
  defsubr (&Snidhogg_workers__test_io_crash);
  defsubr (&Snidhogg_workers__test_io_hold_lock);
  defsubr (&Snidhogg_workers__test_filter_cost);
}

#endif /* not NH_WK_HARNESS */

#endif /* HAVE_NIDHOGG_WORKERS */

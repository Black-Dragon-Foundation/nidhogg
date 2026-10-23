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

   Framing (Task 24; the design's §5.1-§5.4 and §8.4).  A reader-owned
   process with a framing (`nidhogg-set-process-framing') is read the
   same way, but the reader also frames: it splits the bytes into
   messages (content-length, lines or records), tokenizes each JSON
   message into a tape (nidhogg-json-tape.c) or decodes each record, and
   queues the messages instead of chunks; the wake fd works as for
   chunks.  The framer's state (the bytes after the last message) is the
   reader's: nh-io's, or the main thread's inline and once handed back.
   At most 64 MB of complete messages queue (the message in progress
   may grow to :max-message; past it, a framing error).  The main thread
   materializes queued messages for up to 4 ms (a message may take
   several deliveries: its materialization waits, rooted, in
   h->cur_mat), then leaves the batch for
   read_and_dispose_of_process_output, which calls the batch function
   under a filter's protections.  The drains (the real fd) and inline
   mode deliver every complete message at once.  Turning framing on and
   off are commands nh-io carries out between reads, and the main thread
   waits for them (the stop flag ends a tokenize within 64 KB): on, the
   queued chunks become the framer's bytes; off, the queued messages and
   the framer's bytes become chunks again, in order, and go to the
   filter as raw output.  A framing error does the same from nh-io's
   side for the bytes after the last good message.

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

/* nh_io_fr_read's answer when a framing error turned framing off: the
   delivery reads raw (Task 24).  */
#define NH_IO_RAW PTRDIFF_MIN

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

/* Framing (Task 24).  */
enum nh_io_fkind
{
  NH_IO_FR_NONE,
  NH_IO_FR_CL,			/* content-length */
  NH_IO_FR_LINES,
  NH_IO_FR_RECORDS
};

/* The most bytes of complete messages a framed process queues.  */
#define NH_IO_FR_CAP ((ptrdiff_t) 64 * 1024 * 1024)
/* A content-length header's most bytes.  */
enum { NH_IO_FR_HEADER_MAX = 8192 };

struct nh_io_fconf
{
  int kind;			/* enum nh_io_fkind */
  int parse;			/* NH_JSON_MODE_*: the JSON framings */
  int sep;			/* records: 0 or '\n' */
  int sys;			/* records: a stateless system, or NH_DEC_NONE */
  size_t max;			/* :max-message */
};

/* A framer: the bytes after the last message are BUF[START, LEN).
   SCAN is where the next look for a line end or separator starts; PRE
   is where the message in progress starts (`lines': after the empty
   lines it skipped) or, for a content-length header, where its line in
   progress starts; HDR and BODY are a content-length message's header
   and body sizes once its header is complete (HDR > 0); all relative
   to START.  */
struct nh_io_framer
{
  struct nh_io_fconf c;
  unsigned char *buf;
  size_t start, len, size;
  size_t scan, pre, hdr, body;
  bool have_len;
};

/* A complete message.  WIRE (LEN bytes) holds every byte the stream
   gave for it (the empty lines skipped before it, its header, its
   terminator); RAW_OFF and RAW_LEN are the message itself.  TAPE: the
   JSON framings' tokens.  DEC: a record's decode with :coding (MB: a
   multibyte one).  Each a malloc block.  */
struct nh_io_msg
{
  struct nh_io_msg *next;
  size_t len, raw_off, raw_len;
  unsigned char *wire;
  struct nh_tape *tape;
  unsigned char *dec;
  ptrdiff_t dlen, nchars;
  bool mb;
};

enum nh_io_op
{
  NH_IO_OP_ADD, NH_IO_OP_DETACH, NH_IO_OP_FRAME, NH_IO_OP_UNFRAME
};

struct nh_io_h;

struct nh_io_cmd
{
  int op;
  struct nh_io_h *h;
  struct nh_io_cmd *next;
  /* FRAME and UNFRAME (malloc'd; nh-io frees them): the framing, and
     the main thread's wait.  */
  struct nh_io_fconf conf;
  struct nh_wk_job *ack;
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

  /* Framing (Task 24).  The reader's framer; the raw cap to go back to.  */
  struct nh_io_framer fr;
  ptrdiff_t rawcap;
  /* Under nh_io_m: the complete messages, oldest first (QUEUED counts
     their wire bytes), and whether a framing error turned the reader's
     framing off (chunks follow the messages then).  */
  struct nh_io_msg *mhead, *mtail;
  bool fr_failed;
  /* Set by the main thread before a detach or a framing switch-off: a
     tokenize in progress stops (within 64 KB) and its bytes stay.  */
  _Atomic bool stop;
  /* The main thread's: framed for Lisp; the message being materialized
     (taken from the queue) and its materialization.  */
  bool m_framed;
  struct nh_io_msg *cur;
  struct nh_mat *cur_mat;
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
  NH_IO_EV_STOPPED, NH_IO_EV_N
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
nh_io_msg_free (struct nh_io_msg *m)
{
  if (m)
    {
      free (m->wire);
      free (m->dec);
      free (m->tape);
      free (m);
    }
}

static void
nh_io_msgs_free (struct nh_io_msg *m)
{
  for (struct nh_io_msg *next; m; m = next)
    {
      next = m->next;
      nh_io_msg_free (m);
    }
}

/* Empty F, keeping its framing.  */
static void
nh_io_framer_reset (struct nh_io_framer *f)
{
  free (f->buf);
  *f = (struct nh_io_framer) { .c = f->c };
}

static void
nh_io_free (struct nh_io_h *h)
{
  nh_io_chunks_free (h->head);
  nh_io_chunks_free (h->last);
  nh_io_msgs_free (h->mhead);
  nh_io_msg_free (h->cur);
  free (h->fr.buf);
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

static void nh_io_fr_on (struct nh_io_h *, const struct nh_io_fconf *);
static void nh_io_fr_off (struct nh_io_h *);

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
      else if (c->op == NH_IO_OP_FRAME || c->op == NH_IO_OP_UNFRAME)
	{
	  /* Task 24: the main thread waits for these.  */
	  if (c->op == NH_IO_OP_FRAME)
	    nh_io_fr_on (h, &c->conf);
	  else
	    nh_io_fr_off (h);
	  struct nh_wk_job *ack = c->ack;
	  free (c);
	  /* (Drops nh-io's reference.)  */
	  nh_wk_complete (ack);
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

/* Framing (Task 24), the reader's half.  */

/* A chunk of the N bytes at S that the filter gets as raw output (the
   main thread decodes them).  */
static struct nh_io_chunk *
nh_io_raw_chunk (const unsigned char *s, size_t n)
{
  struct nh_io_chunk *c = malloc (sizeof *c + n);
  NH_WK_ASSERT (c);
  *c = (struct nh_io_chunk) { .len = n };
  memcpy (c->data, s, n);
  return c;
}

/* Append the N bytes at S to F's; false when out of memory.  */
static bool
nh_io_framer_add (struct nh_io_framer *f, const unsigned char *s, size_t n)
{
  if (n == 0)
    return true;
  if (f->len + n > f->size && f->start > 0)
    {
      memmove (f->buf, f->buf + f->start, f->len - f->start);
      f->len -= f->start;
      f->start = 0;
    }
  if (f->len + n > f->size)
    {
      size_t size = f->size ? f->size : 4096;
      while (size < f->len + n)
	size *= 2;
      unsigned char *b = realloc (f->buf, size);
      if (!b)
	return false;
      f->buf = b;
      f->size = size;
    }
  memcpy (f->buf + f->len, s, n);
  f->len += n;
  return true;
}

/* A content-length header line, the N bytes at S without its CRLF: 1
   a Content-Length (its value in *LEN), 0 another header, -1 not a
   header.  Names are case-insensitive; the value may have spaces or
   tabs around it.  */
static int
nh_io_cl_line (const unsigned char *s, size_t n, size_t *len)
{
  static const char name[] = "content-length";
  size_t i = 0;
  while (i < n && s[i] != ':' && 0x20 < s[i] && s[i] < 0x7f)
    i++;
  if (i == 0 || i == n || s[i] != ':')
    return -1;
  size_t nlen = i;
  for (size_t k = nlen + 1; k < n; k++)
    if (s[k] == '\r' || s[k] == '\n')
      return -1;
  if (nlen != sizeof name - 1)
    return 0;
  for (size_t k = 0; k < nlen; k++)
    if ((s[k] | 0x20) != name[k])
      return 0;
  for (i = nlen + 1; i < n && (s[i] == ' ' || s[i] == '\t'); i++)
    ;
  if (i == n || s[i] < '0' || '9' < s[i])
    return -1;
  size_t v = 0;
  for (; i < n && '0' <= s[i] && s[i] <= '9'; i++)
    {
      if (v > (SIZE_MAX - 9) / 10)
	return -1;
      v = 10 * v + (s[i] - '0');
    }
  for (; i < n && (s[i] == ' ' || s[i] == '\t'); i++)
    ;
  if (i != n)
    return -1;
  *len = v;
  return 1;
}

/* F's next message: its WIRE bytes from F's start, the message itself
   the RAW_LEN bytes at RAW_OFF in them, tokenized (stopping when *STOP
   turns true) or decoded, appended at **TAIL.  0; or, with nothing
   emitted, 1 when STOP stopped the tokenize, -2 when out of memory.  */
static int
nh_io_emit (struct nh_io_framer *f, size_t wire, size_t raw_off,
	    size_t raw_len, const _Atomic bool *stop,
	    struct nh_io_msg ***tail)
{
  const unsigned char *s = f->buf + f->start;
  struct nh_io_msg *m = malloc (sizeof *m);
  if (!m)
    return -2;
  *m = (struct nh_io_msg) { .len = wire, .raw_off = raw_off,
			    .raw_len = raw_len };
  if (f->c.kind != NH_IO_FR_RECORDS)
    {
      /* A test stops nh-io here (S16: a detach during a large
	 message).  */
      nh_wk_pause_point (NH_WK_P_BEFORE_TOKENIZE);
      m->tape = nh_json_tokenize (s + raw_off, raw_len, f->c.parse, stop);
      if (!m->tape)
	{
	  free (m);
	  return stop && atomic_load (stop) ? 1 : -2;
	}
    }
  else if (f->c.sys != NH_DEC_NONE)
    {
      ptrdiff_t consumed;
      m->dec = malloc (NH_DEC_OUT_MAX (raw_len) + 1);
      m->dlen = (m->dec
		 ? nh_dec_run (f->c.sys, s + raw_off, raw_len, true, m->dec,
			       &m->nchars, &consumed)
		 : -1);
      if (m->dlen < 0)
	{
	  nh_io_msg_free (m);
	  return -2;
	}
      m->mb = NH_DEC_MULTIBYTE (f->c.sys);
    }
  if (f->start == 0 && wire == f->len)
    {
      /* The message is all F has: it takes F's buffer.  */
      m->wire = f->buf;
      f->buf = NULL;
      f->len = f->size = 0;
    }
  else
    {
      m->wire = malloc (wire);
      if (!m->wire)
	{
	  nh_io_msg_free (m);
	  return -2;
	}
      memcpy (m->wire, s, wire);
      f->start += wire;
    }
  **tail = m;
  *tail = &m->next;
  return 0;
}

/* The framers' steps: the next message from F's start (F has bytes
   there).  2 when one was emitted; 0 when the bytes are a message in
   progress, or skipped empty lines; else nh_io_emit's 1 or -2, or -1 a
   framing error.  EOF: no more bytes come.  */

static int
nh_io_step_cl (struct nh_io_framer *f, bool eof, const _Atomic bool *stop,
	       struct nh_io_msg ***tail)
{
  const unsigned char *s = f->buf + f->start;
  size_t avail = f->len - f->start;
  while (!f->hdr)
    {
      const unsigned char *nl = (f->scan < avail
				 ? memchr (s + f->scan, '\n', avail - f->scan)
				 : NULL);
      if (!nl)
	{
	  f->scan = avail;
	  return (avail > NH_IO_FR_HEADER_MAX || eof) ? -1 : 0;
	}
      size_t end = nl - s;
      if (end >= NH_IO_FR_HEADER_MAX || end == f->pre || s[end - 1] != '\r')
	return -1;
      size_t line = f->pre, n = end - 1 - line;
      f->pre = f->scan = end + 1;
      size_t len = 0;
      int k = n ? nh_io_cl_line (s + line, n, &len) : 0;
      if (k < 0 || (k > 0 && (f->have_len || len > f->c.max)))
	return -1;
      if (k > 0)
	{
	  f->body = len;
	  f->have_len = true;
	}
      else if (n == 0)
	{
	  /* The empty line: the header is complete.  */
	  if (!f->have_len)
	    return -1;
	  f->hdr = end + 1;
	}
    }
  if (avail - f->hdr < f->body)
    return eof ? -1 : 0;
  int r = nh_io_emit (f, f->hdr + f->body, f->hdr, f->body, stop, tail);
  if (r != 0)
    return r;
  f->scan = f->pre = f->hdr = f->body = 0;
  f->have_len = false;
  return 2;
}

static int
nh_io_step_lines (struct nh_io_framer *f, bool eof,
		  const _Atomic bool *stop, struct nh_io_msg ***tail)
{
  const unsigned char *s = f->buf + f->start;
  size_t avail = f->len - f->start;
  for (;;)
    {
      const unsigned char *nl = (f->scan < avail
				 ? memchr (s + f->scan, '\n', avail - f->scan)
				 : NULL);
      size_t end = nl ? (size_t) (nl - s) : avail;
      if (!nl)
	{
	  f->scan = avail;
	  /* The line in progress may still end in a \r that is dropped.  */
	  if (avail - f->pre > f->c.max + 1)
	    return -1;
	  /* At EOF, a last line without its newline is a message.  */
	  if (!eof || avail == f->pre)
	    return 0;
	}
      size_t cend = end > f->pre && s[end - 1] == '\r' ? end - 1 : end;
      size_t wire = nl ? end + 1 : end;
      if (cend == f->pre)
	{
	  /* An empty line: its bytes go with the next message's.  */
	  f->pre = f->scan = wire;
	  if (!nl)
	    return 0;
	  continue;
	}
      if (cend - f->pre > f->c.max)
	return -1;
      int r = nh_io_emit (f, wire, f->pre, cend - f->pre, stop, tail);
      if (r != 0)
	return r;
      f->pre = f->scan = 0;
      return 2;
    }
}

static int
nh_io_step_records (struct nh_io_framer *f, bool eof,
		    const _Atomic bool *stop, struct nh_io_msg ***tail)
{
  const unsigned char *s = f->buf + f->start;
  size_t avail = f->len - f->start;
  const unsigned char *sep = (f->scan < avail
			      ? memchr (s + f->scan, f->c.sep, avail - f->scan)
			      : NULL);
  size_t end = sep ? (size_t) (sep - s) : avail;
  if (!sep)
    {
      f->scan = avail;
      if (avail > f->c.max)
	return -1;
      /* At EOF, a last record without its separator is a record.  */
      if (!eof)
	return 0;
    }
  if (end > f->c.max)
    return -1;
  int r = nh_io_emit (f, sep ? end + 1 : end, 0, end, stop, tail);
  if (r != 0)
    return r;
  f->scan = 0;
  return 2;
}

/* Frame F's bytes: emit every complete message at **TAIL.  0 when what
   is left is a message in progress (or nothing), 1 when STOP stopped a
   tokenize, -1 a framing error (the bytes from F's start are no
   message), -2 out of memory.  EOF: no more bytes come.  */
static int
nh_io_frame (struct nh_io_framer *f, bool eof, const _Atomic bool *stop,
	     struct nh_io_msg ***tail)
{
  int r = 2;
  while (r == 2 && f->len > f->start)
    r = (f->c.kind == NH_IO_FR_CL ? nh_io_step_cl (f, eof, stop, tail)
	 : f->c.kind == NH_IO_FR_LINES ? nh_io_step_lines (f, eof, stop, tail)
	 : nh_io_step_records (f, eof, stop, tail));
  return r == 2 ? 0 : r;
}

/* H's reader queues what its framer makes of the N bytes at S it read
   (EV 1; N 0: of the bytes it has), or after EOF (EV 0) or the read
   error ERR (EV -1).  STOP: nh-io's stop flag (NULL on the main
   thread).  A framing error turns the reader's framing off: the bytes
   after the last message are queued as raw chunks.  */
static void
nh_io_fr_run (struct nh_io_h *h, const unsigned char *s, ptrdiff_t n,
	      int ev, int err, const _Atomic bool *stop)
{
  struct nh_io_framer *f = &h->fr;
  struct nh_io_msg *msgs = NULL, **tail = &msgs, *mlast = NULL;
  struct nh_io_chunk *raw = NULL, *rlast = NULL;
  int rc = -1;
  if (n > 0 && !nh_io_framer_add (f, s, n))
    raw = nh_io_raw_chunk (s, n);
  else
    rc = nh_io_frame (f, ev == 0, stop, &tail);
  if (rc == 1)
    nh_io_note (NH_IO_EV_STOPPED, h->id, h->copy, f->len - f->start);
  else if (rc < 0)
    {
      if (f->len > f->start)
	{
	  struct nh_io_chunk *c = nh_io_raw_chunk (f->buf + f->start,
						   f->len - f->start);
	  c->next = raw;
	  raw = c;
	}
      nh_io_framer_reset (f);
      f->c.kind = NH_IO_FR_NONE;
    }
  ptrdiff_t add = 0;
  for (struct nh_io_msg *m = msgs; m; m = m->next)
    {
      add += m->len;
      mlast = m;
    }
  for (struct nh_io_chunk *c = raw; c; c = c->next)
    {
      add += c->len;
      rlast = c;
    }

  nh_wk_lock (&nh_io_m);
  bool was_quiet = (!h->head && !h->mhead && !h->eof && !h->err
		    && !h->fr_failed);
  if (msgs)
    {
      if (h->mtail)
	h->mtail->next = msgs;
      else
	h->mhead = msgs;
      h->mtail = mlast;
    }
  if (raw)
    {
      if (h->tail)
	h->tail->next = raw;
      else
	h->head = raw;
      h->tail = rlast;
    }
  if (rc < 0)
    h->fr_failed = true;
  if (ev == 0)
    h->eof = true;
  else if (ev < 0)
    h->err = err;
  bool capped = add > 0 && (h->queued += add) >= h->cap;
  if (capped)
    atomic_store (&h->capped, true);
  if (was_quiet && (add > 0 || rc < 0 || ev <= 0) && !h->inl)
    nh_wk_wake_signal (&h->wake);
  pthread_mutex_unlock (&nh_io_m);

  if (add > 0)
    nh_wk_stat_add (NH_WK_IO, NH_WK_ST_QUEUED_BYTES, add);
  if (ev <= 0)
    h->io_end = true;
  if (capped)
    {
      nh_wk_stat_add (NH_WK_IO, NH_WK_ST_BACKPRESSURE, 1);
      nh_wk_pause_point (NH_WK_P_AFTER_READ);
    }
}

/* Framing on with CONF, by H's reader: the queued chunks' bytes come
   first in the framer.  */
static void
nh_io_fr_on (struct nh_io_h *h, const struct nh_io_fconf *conf)
{
  nh_wk_lock (&nh_io_m);
  struct nh_io_chunk *c = h->head;
  h->head = h->tail = NULL;
  ptrdiff_t n = 0;
  for (struct nh_io_chunk *k = c; k; k = k->next)
    n += k->len - k->off;
  h->queued -= n;
  h->fr_failed = false;
  h->cap = NH_IO_FR_CAP;
  if (h->queued < h->cap)
    atomic_store (&h->capped, false);
  bool eof = h->eof;
  pthread_mutex_unlock (&nh_io_m);
  nh_wk_stat_add (NH_WK_IO, NH_WK_ST_QUEUED_BYTES, -n);
  atomic_store (&h->stop, false);
  nh_io_framer_reset (&h->fr);
  h->fr.c = *conf;
  for (struct nh_io_chunk *k = c; k; k = k->next)
    {
      bool ok = nh_io_framer_add (&h->fr, k->data + k->off, k->len - k->off);
      NH_WK_ASSERT (ok);
    }
  nh_io_chunks_free (c);
  nh_io_fr_run (h, NULL, 0, eof ? 0 : 1, 0, NULL);
}

/* Framing off, by H's reader: the queued messages, the chunks after
   them (a framing error's) and the framer's bytes become the queue's
   raw chunks, in that order.  */
static void
nh_io_fr_off (struct nh_io_h *h)
{
  struct nh_io_framer *f = &h->fr;
  size_t k = f->len - f->start;
  struct nh_io_chunk *part = k ? nh_io_raw_chunk (f->buf + f->start, k) : NULL;
  nh_io_framer_reset (f);
  f->c.kind = NH_IO_FR_NONE;
  /* The decoder starts again: the main thread decodes these chunks.  */
  h->dsys = NH_DEC_N;
  h->dst.n = 0;

  nh_wk_lock (&nh_io_m);
  struct nh_io_chunk *first = NULL, *last = NULL;
  for (struct nh_io_msg *m = h->mhead, *next; m; m = next)
    {
      next = m->next;
      struct nh_io_chunk *c = nh_io_raw_chunk (m->wire, m->len);
      if (last)
	last->next = c;
      else
	first = c;
      last = c;
      nh_io_msg_free (m);
    }
  h->mhead = h->mtail = NULL;
  if (first)
    {
      last->next = h->head;
      if (!h->head)
	h->tail = last;
      h->head = first;
    }
  if (part)
    {
      if (h->tail)
	h->tail->next = part;
      else
	h->head = part;
      h->tail = part;
      h->queued += k;
    }
  h->fr_failed = false;
  h->cap = h->rawcap;
  atomic_store (&h->capped, h->queued >= h->cap);
  if (h->head && !h->inl)
    nh_wk_wake_signal (&h->wake);
  pthread_mutex_unlock (&nh_io_m);
  if (k)
    nh_wk_stat_add (NH_WK_IO, NH_WK_ST_QUEUED_BYTES, k);
  atomic_store (&h->stop, false);
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
  if (h->fr.c.kind != NH_IO_FR_NONE)
    {
      /* Framed (Task 24).  */
      nh_io_note (r > 0 ? NH_IO_EV_READ : r == 0 ? NH_IO_EV_EOF
		  : NH_IO_EV_ERROR, h->id, h->copy, r);
      nh_io_fr_run (h, *buf, r > 0 ? r : 0, r > 0 ? 1 : r == 0 ? 0 : -1, err,
		    &h->stop);
      return;
    }
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
  bool was_quiet = !h->head && !h->mhead && !h->eof && !h->err;
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
  h->cap = h->rawcap = (readmax < NH_IO_CAP_MIN / 4 ? NH_IO_CAP_MIN
			: 4 * readmax);
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
  /* A tokenize in progress stops within 64 KB (Task 24): its message's
     bytes stay in the framer, which is the main thread's from the
     acknowledgement on.  */
  atomic_store (&h->stop, true);
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

/* Turn H's framing on (OP NH_IO_OP_FRAME, with CONF) or off
   (NH_IO_OP_UNFRAME) on its reader's side (Task 24): directly when the
   main thread is the reader (inline, or handed back), else by a command
   nh-io carries out between reads, which this waits for (worker-wait,
   reason detach).  Switching off stops a tokenize in progress within
   64 KB.  */
NH_IO_CORE void
nh_io_sync (struct nh_io_h *h, int op, const struct nh_io_fconf *conf)
{
  if (h->inl || h->detached)
    {
      if (op == NH_IO_OP_FRAME)
	nh_io_fr_on (h, conf);
      else
	nh_io_fr_off (h);
      return;
    }
  if (atomic_load (&nh_wk_dead))
    return;
  struct nh_io_cmd *c = calloc (1, sizeof *c);
  struct nh_wk_job *ack = calloc (1, sizeof *ack);
  NH_WK_ASSERT (c && ack);
  atomic_store (&ack->refs, 2);
  ack->w = NH_WK_IO;
  ack->free = nh_io_job_free;
  *c = (struct nh_io_cmd) { .op = op, .h = h, .ack = ack };
  if (conf)
    c->conf = *conf;
  if (op == NH_IO_OP_UNFRAME)
    atomic_store (&h->stop, true);
  nh_io_command (c);
  nh_wk_wait (ack, NH_IO_DETACH_NS, NH_IO_PH, NH_IO_RS);
  nh_wk_job_unref (ack);
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
  if (!h->head && !h->mhead && !h->eof && !h->err && !h->inl)
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

/* Framing (Task 24).  A framed process's configuration, the vector
   [FRAMING FN OPTIONS], is on its plist under this uninterned symbol.  */
static Lisp_Object nh_io_key;

/* The materialization of H's current message, if any, is dropped (H is
   about to go).  */
static void
nh_io_fr_forget (struct nh_io_h *h)
{
  if (h->cur_mat)
    nh_json_mat_free (h->cur_mat);
  h->cur_mat = NULL;
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
  if (reason == NH_IO_CLOSE)
    {
      /* The framing goes with the process (Task 24).  */
      nh_io_fr_forget (h);
      if (h->m_framed)
	p->plist = plist_put (p->plist, nh_io_key, Qnil);
    }
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
  /* A framed process keeps its handle (its framer and queue) until it
     closes or its framing goes off (Task 24).  */
  if (h->m_framed)
    return false;
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

/* Framing (Task 24), the main thread's half.  The batch a framed
   delivery leaves for read_and_dispose_of_process_output: (PROCESS FN
   . BATCH), or nil.  */
static Lisp_Object nh_io_pending;

static Lisp_Object
nh_io_mat_step (Lisp_Object arg)
{
  return (nh_json_materialize (xmint_pointer (XCAR (arg)),
			       XFIXNUM (XCDR (arg)))
	  ? Qt : Qnil);
}

static Lisp_Object
nh_io_mat_error (Lisp_Object err)
{
  return Fcons (Qerror, err);
}

/* M's batch entry (MESSAGE RAW ERROR) in *ENTRY, its JSON built in
   *MAT (started here and freed once the entry is made) with the json
   OPTIONS for at most LEFT ns (negative: no limit): false when the
   materialization yielded first.  ERROR is what json-parse-* signals
   for RAW (`json-out-of-memory' after a slice a signal left).  */
static bool
nh_io_msg_entry (struct nh_io_msg *m, struct nh_mat **mat, Lisp_Object opts,
		 int64_t left, Lisp_Object *entry)
{
  const unsigned char *raw = m->wire + m->raw_off;
  Lisp_Object value = Qnil, error = Qnil;
  if (m->tape)
    {
      if (!*mat)
	*mat = nh_json_mat_start (m->tape, raw, m->raw_len, opts);
      Lisp_Object r
	= internal_condition_case_1 (nh_io_mat_step,
				     Fcons (make_mint_ptr (*mat),
					    make_fixnum (left)),
				     Qerror, nh_io_mat_error);
      if (NILP (r))
	return false;
      if (CONSP (r))
	error = (nh_json_mat_failed (*mat) ? list1 (Qjson_out_of_memory)
		 : XCDR (r));
      else
	value = nh_json_mat_value (*mat);
      nh_json_mat_free (*mat);
      *mat = NULL;
    }
  Lisp_Object bytes = make_unibyte_string ((const char *) raw, m->raw_len);
  if (!m->tape)
    value = (!m->dec ? bytes
	     : m->mb ? make_multibyte_string ((const char *) m->dec,
					      m->nchars, m->dlen)
	     : make_unibyte_string ((const char *) m->dec, m->dlen));
  *entry = list3 (value, bytes, error);
  return true;
}

/* Clear H's wake fd if nothing is left to deliver.  */
static void
nh_io_fr_quiet (struct nh_io_h *h)
{
  if (h->inl || h->cur)
    return;
  nh_wk_lock (&nh_io_m);
  if (!h->head && !h->mhead && !h->eof && !h->err && !h->fr_failed)
    nh_wk_wake_clear (&h->wake);
  pthread_mutex_unlock (&nh_io_m);
}

/* Take H's queued messages into a batch for P's framing CONF, for up to
   4 ms of materialization (UNSLICED: all of them), leave it in
   nh_io_pending and return its wire bytes; -1 with EAGAIN when the
   current message's materialization yielded first (it goes on at the
   next delivery: the wake fd stays readable).  */
static ptrdiff_t
nh_io_fr_take (struct Lisp_Process *p, struct nh_io_h *h, Lisp_Object conf,
	       bool unsliced)
{
  Lisp_Object opts = AREF (conf, 2);
  Lisp_Object b = Vnidhogg_workers__test_deliver_budget;
  int64_t budget = (unsliced ? -1 : FIXNATP (b) ? XFIXNAT (b)
		    : (int64_t) NH_IO_FILTER_NS);
  int64_t t0 = nh_io_now ();
  int depth = NH_WK_PHASE_PUSH (NH_WK_PH_DELIVER, 0);
  Lisp_Object batch = Qnil;
  ptrdiff_t wire = 0;
  for (;;)
    {
      if (!h->cur)
	{
	  nh_wk_lock (&nh_io_m);
	  struct nh_io_msg *m = h->mhead;
	  bool uncapped = false;
	  if (m)
	    {
	      h->mhead = m->next;
	      if (!h->mhead)
		h->mtail = NULL;
	      h->queued -= m->len;
	      uncapped = atomic_load (&h->capped) && h->queued < h->cap;
	      if (uncapped)
		atomic_store (&h->capped, false);
	    }
	  pthread_mutex_unlock (&nh_io_m);
	  if (!m)
	    break;
	  if (uncapped && !h->inl)
	    nh_io_poke ();
	  nh_wk_stat_add (NH_WK_IO, NH_WK_ST_QUEUED_BYTES, -m->len);
	  m->next = NULL;
	  h->cur = m;
	}
      int64_t left = budget - (nh_io_now () - t0);
      Lisp_Object entry;
      if (!nh_io_msg_entry (h->cur, &h->cur_mat, opts,
			    budget < 0 ? -1 : left > 0 ? left : 0, &entry))
	break;
      batch = Fcons (entry, batch);
      wire += h->cur->len;
      nh_io_msg_free (h->cur);
      h->cur = NULL;
      if (budget >= 0 && nh_io_now () - t0 >= budget)
	break;
    }
  NH_WK_PHASE_POP (depth);
  nh_io_fr_quiet (h);
  if (NILP (batch))
    {
      errno = EAGAIN;
      return -1;
    }
  nh_io_pending = Fcons (make_lisp_proc (p),
			 Fcons (AREF (conf, 1), Fnreverse (batch)));
  return wire;
}

/* The main thread reads FD (inline the copy, in a drain the real fd)
   once for H's framer: true when it read bytes, EOF or an error.  */
static bool
nh_io_fr_fill (struct nh_io_h *h, int fd, char *buf, ptrdiff_t size)
{
  if (atomic_load (&h->queued) >= h->cap)
    return false;
  ssize_t r;
  do
    r = read (fd, buf, size);
  while (r < 0 && errno == EINTR);
  if (r < 0 && (errno == EAGAIN || errno == EWOULDBLOCK))
    return false;
  int err = errno;
  nh_io_note (r > 0 ? NH_IO_EV_READ : r == 0 ? NH_IO_EV_EOF : NH_IO_EV_ERROR,
	      h->id, fd, r);
  nh_io_fr_run (h, (unsigned char *) buf, r > 0 ? r : 0,
		r > 0 ? 1 : r == 0 ? 0 : -1, err, NULL);
  return true;
}

/* nh_io_read for P, framed with CONF: a batch's wire bytes (the batch
   waits in nh_io_pending), or what nh_io_read returns with nothing
   delivered; NH_IO_RAW when a framing error turned framing off.  In a
   drain (CHANNEL is the real fd) and inline, the main thread reads the
   fd itself and delivers every complete message at once.  */
static ptrdiff_t
nh_io_fr_read (struct Lisp_Process *p, struct nh_io_h *h, Lisp_Object conf,
	       int channel, char *buf, ptrdiff_t size)
{
  bool unsliced = channel == p->infd || h->inl;
  int fd = (h->detached ? (channel == p->infd ? p->infd : -1)
	    : h->inl ? h->copy : -1);
  /* Handed back, the framer is the main thread's: frame what nh-io left
     (a tokenize the hand-back stopped).  */
  if (h->detached && h->fr.c.kind != NH_IO_FR_NONE && h->fr.len > h->fr.start)
    nh_io_fr_run (h, NULL, 0, 1, 0, NULL);
  for (;;)
    {
      nh_wk_lock (&nh_io_m);
      bool msgs = h->mhead, failed = h->fr_failed, eof = h->eof;
      int err = h->err;
      pthread_mutex_unlock (&nh_io_m);
      if (h->cur || msgs)
	return nh_io_fr_take (p, h, conf, unsliced);
      if (failed)
	{
	  /* The rest is raw output.  */
	  h->m_framed = false;
	  p->plist = plist_put (p->plist, nh_io_key, Qnil);
	  return NH_IO_RAW;
	}
      if (fd >= 0 && !eof && !err && nh_io_fr_fill (h, fd, buf, size))
	continue;
      if (eof)
	return 0;
      if (err)
	{
	  errno = err;
	  return -1;
	}
      nh_io_fr_quiet (h);
      errno = EAGAIN;
      return -1;
    }
}

/* The current message of H, taken from the queue, goes back to its head
   as a raw chunk (framing went off).  */
static void
nh_io_fr_cur_back (struct nh_io_h *h)
{
  nh_io_fr_forget (h);
  struct nh_io_msg *m = h->cur;
  if (!m)
    return;
  h->cur = NULL;
  struct nh_io_chunk *c = nh_io_raw_chunk (m->wire, m->len);
  nh_wk_lock (&nh_io_m);
  c->next = h->head;
  h->head = c;
  if (!h->tail)
    h->tail = c;
  h->queued += m->len;
  if (!h->inl)
    nh_wk_wake_signal (&h->wake);
  pthread_mutex_unlock (&nh_io_m);
  nh_wk_stat_add (NH_WK_IO, NH_WK_ST_QUEUED_BYTES, m->len);
  nh_io_msg_free (m);
}

void
nh_io_unframe (Lisp_Object proc)
{
  struct Lisp_Process *p = XPROCESS (proc);
  if (!NILP (plist_get (p->plist, nh_io_key)))
    p->plist = plist_put (p->plist, nh_io_key, Qnil);
  struct nh_io_h *h = nh_io_get (p);
  if (!h || !h->m_framed)
    return;
  h->m_framed = false;
  nh_io_sync (h, NH_IO_OP_UNFRAME, NULL);
  nh_io_fr_cur_back (h);
}

ptrdiff_t
nh_io_read (struct Lisp_Process *p, int channel, char *buf, ptrdiff_t size,
	    bool *handled)
{
  struct nh_io_h *h = nh_io_get (p);
  *handled = h && (channel == h->sel || channel == p->infd);
  if (!*handled)
    return 0;
  /* The fatal path takes no lock (nh-io may hold it forever).  */
  if (!h->inl && atomic_load (&nh_wk_dead))
    {
      errno = EAGAIN;
      return -1;
    }
  if (h->m_framed)
    {
      /* Framed (Task 24).  A configuration a plist change dropped turns
	 framing off.  */
      Lisp_Object conf = plist_get (p->plist, nh_io_key);
      if (!VECTORP (conf))
	nh_io_unframe (make_lisp_proc (p));
      else
	{
	  ptrdiff_t n = nh_io_fr_read (p, h, conf, channel, buf, size);
	  if (n != NH_IO_RAW)
	    return n;
	}
    }
  size = nh_io_limit (p, h, size);
  if (h->inl)
    {
      /* Bytes framing left as raw output come first.  */
      if (h->head || h->eof || h->err)
	return nh_io_take (h, buf, size);
      if (h->copy >= 0)
	return nh_io_take_inline (h, buf, size);
      *handled = false;
      return 0;
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

/* Task 24: P's framed delivery left a batch.  Hand it to C (FN nil:
   P's buffer is dead, the batch is dropped) and enter the `filter'
   phase.  */
bool
nh_io_batch_begin (struct Lisp_Process *p, struct nh_io_call *c)
{
  Lisp_Object pend = nh_io_pending;
  if (NILP (pend))
    return false;
  nh_io_pending = Qnil;
  if (XPROCESS (XCAR (pend)) != p)
    return false;
  *c = (struct nh_io_call) { .fn = XCAR (XCDR (pend)),
			     .batch = XCDR (XCDR (pend)), .depth = -1 };
  if (BUFFERP (p->buffer) && !BUFFER_LIVE_P (XBUFFER (p->buffer)))
    c->fn = Qnil;
  if (!NILP (c->fn))
    {
      c->t0 = nh_io_now ();
      c->depth = NH_WK_PHASE_PUSH (NH_WK_PH_FILTER, 0);
    }
  return true;
}

/* The batch function returned: its ns count as a filter's (not the
   delivery's).  */
void
nh_io_batch_end (struct nh_io_call *c)
{
  if (!c->t0)
    return;
  NH_WK_PHASE_POP (c->depth);
  int64_t ns = nh_io_now () - c->t0;
  nh_io_filter_ns += ns;
  nh_wk_phase_ns_add (NH_WK_PH_FILTER, 0, ns);
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
bytes again), `:filter-cost' (the filter's estimated nanoseconds per
byte, a float; nil until a filter call measured it) and `:messages'
\(framed: complete messages queued; their bytes count in `:queued').
For a process the reader does not own, in a build with GnuTLS, also:
`:tls-reclaimed' (bytes the reader had read and not delivered when
`gnutls-boot' took the fd back, given to GnuTLS instead of the filter),
`:tls-pulled' (how many of them GnuTLS has read), `:tls-left' (how
many it has yet to read) and `:tls-pull' (non-nil if the session got
the pull function that returns them).  A `tls' refusal also means
`gnutls-boot' ran on PROCESS.  */)
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
  intmax_t nmsgs = 0;
  for (struct nh_io_msg *m = h->mhead; m; m = m->next)
    nmsgs++;
  pthread_mutex_unlock (&nh_io_m);
  int fds = (h->copy >= 0) + (h->wake.rfd >= 0)
    + (h->wake.wfd >= 0 && h->wake.wfd != h->wake.rfd);
  uint64_t coding = atomic_load (&h->coding);
  return listn (32, QCowned, h->detached ? Qnil : Qt,
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
		h->cost_known ? make_float (h->cost) : Qnil,
		intern_c_string (":messages"), make_int (nmsgs));
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

/* FRAMING and its ARGS (NARGS of them) for PROCESS (nil for the test
   primitive): the reader's CONF, the batch function *FN and the JSON
   options *OPTS (nil for records).  */
static void
nh_io_fr_args (Lisp_Object process, Lisp_Object framing, ptrdiff_t nargs,
	       Lisp_Object *args, struct nh_io_fconf *conf, Lisp_Object *fn,
	       Lisp_Object *opts)
{
  *conf = (struct nh_io_fconf) { .parse = NH_JSON_MODE_STRING, .sep = 0,
				 .sys = NH_DEC_NONE,
				 .max = (size_t) 256 * 1024 * 1024 };
  if (EQ (framing, Qcontent_length))
    conf->kind = NH_IO_FR_CL;
  else if (EQ (framing, Qlines))
    conf->kind = NH_IO_FR_LINES;
  else if (EQ (framing, Qrecords))
    conf->kind = NH_IO_FR_RECORDS;
  else
    wrong_choice (list3 (Qcontent_length, Qlines, Qrecords), framing);
  if (nargs % 2)
    wrong_type_argument (Qplistp, Flist (nargs, args));
  Lisp_Object json = Qnil, coding = Qnil;
  *fn = Qnil;
  for (ptrdiff_t i = 0; i < nargs; i += 2)
    {
      Lisp_Object key = args[i], v = args[i + 1];
      if (EQ (key, QCbatch_function))
	*fn = v;
      else if (EQ (key, QCmax_message))
	{
	  CHECK_FIXNAT (v);
	  if (XFIXNAT (v) == 0)
	    args_out_of_range (key, v);
	  conf->max = XFIXNAT (v);
	}
      else if (conf->kind == NH_IO_FR_RECORDS && EQ (key, QCseparator))
	{
	  if (!BASE_EQ (v, make_fixnum (0)) && !BASE_EQ (v, make_fixnum ('\n')))
	    wrong_choice (list2 (make_fixnum (0), make_fixnum ('\n')), v);
	  conf->sep = XFIXNUM (v);
	}
      else if (conf->kind == NH_IO_FR_RECORDS && EQ (key, QCcoding))
	coding = v;
      else if (conf->kind != NH_IO_FR_RECORDS && EQ (key, QCparse))
	{
	  if (EQ (v, Qstring))
	    conf->parse = NH_JSON_MODE_STRING;
	  else if (EQ (v, Qbuffer))
	    conf->parse = NH_JSON_MODE_BUFFER;
	  else
	    wrong_choice (list2 (Qstring, Qbuffer), v);
	}
      else if (conf->kind != NH_IO_FR_RECORDS)
	json = Fcons (v, Fcons (key, json));
      else
	xsignal2 (Qerror, build_string ("Not a records framing keyword"), key);
    }
  *opts = (conf->kind == NH_IO_FR_RECORDS ? Qnil
	   : nh_json_options (Fnreverse (json)));
  if (!NILP (coding))
    {
      CHECK_CODING_SYSTEM (coding);
      struct coding_system cs;
      setup_coding_system (coding, &cs);
      conf->sys = nh_dec_classify (&cs);
      if (conf->sys == NH_DEC_NONE)
	xsignal3 (Qnidhogg_framing_unsupported, process, Qcoding, coding);
    }
}

DEFUN ("nidhogg-set-process-framing", Fnidhogg_set_process_framing,
       Snidhogg_set_process_framing, 2, MANY, 0,
       doc: /* Deliver PROCESS's output as messages, framed by FRAMING.
FRAMING is `content-length' (LSP and DAP: headers, an empty line, then
as many bytes as the Content-Length header says; header names in any
case and order, at most 8 KB of header), `lines' (newline-delimited
JSON: a trailing CR is dropped, empty lines are skipped) or `records'
\(no JSON: the output split at a separator).  With FRAMING nil, framing
goes off: every byte not yet delivered goes to PROCESS's filter as raw
output, in order, and output flows to the filter as before.

ARGS are keywords and values:
`:batch-function' FN (required): FN is called with PROCESS and a batch,
  a list of (MESSAGE RAW ERROR) in arrival order, as a filter is called
  (`inhibit-quit' bound, match data saved, errors reported as a
  filter's).  RAW is the message's bytes, a unibyte string.  MESSAGE is
  what `json-parse-string' (or `json-parse-buffer') returns for them,
  and ERROR nil; or MESSAGE is nil and ERROR the error it signals.  A
  batch for a process whose buffer is dead is dropped.
`:parse' `string' (the default) or `buffer': `json-parse-buffer's
  semantics instead (one value, trailing bytes allowed).
`:object-type', `:array-type', `:null-object', `:false-object': as for
  `json-parse-string'.
`:max-message' BYTES: a longer message is a framing error (default
  256 MB).
For `records': `:separator' 0 (the default) or ?\n, and `:coding' a
  coding system without state, such as `utf-8-unix', that decodes each
  record (else MESSAGE is RAW).  MESSAGE is the record (without its
  separator) and ERROR nil.

A framing error, a `set-process-filter' (also through `add-function')
and a `set-process-coding-system' turn framing off as FRAMING nil does.
PROCESS must be one the reader thread reads (see
`nidhogg-set-process-reader'), decoding with `binary' (`records': any
coding system without state); else this signals
`nidhogg-framing-unsupported' with PROCESS and the reason.  Setting a
new framing reframes the bytes not yet delivered.  Return FRAMING.
usage: (nidhogg-set-process-framing PROCESS FRAMING &rest ARGS)  */)
  (ptrdiff_t nargs, Lisp_Object *args)
{
  Lisp_Object process = args[0], framing = args[1];
  CHECK_PROCESS (process);
  struct Lisp_Process *p = XPROCESS (process);
  if (NILP (framing))
    {
      nh_io_unframe (process);
      return Qnil;
    }
  struct nh_io_fconf conf;
  Lisp_Object fn, opts;
  nh_io_fr_args (process, framing, nargs - 2, args + 2, &conf, &fn, &opts);
  if (!FUNCTIONP (fn))
    wrong_type_argument (Qfunctionp, fn);
  struct nh_io_h *h = nh_io_get (p);
  const char *why = NULL;
  if (!h || h->detached)
    {
      int r = (h ? NH_IO_RF_ENDED
	       : 0 <= p->infd && p->infd < FD_SETSIZE ? nh_io_refused[p->infd]
	       : 0);
      why = r ? nh_io_refusal_names[r] : "not-read";
    }
  else
    {
      int sys = NH_IO_SYS (atomic_load (&h->coding));
      if (conf.kind == NH_IO_FR_RECORDS ? sys == NH_DEC_NONE
	  : sys != NH_DEC_RAW_UNIX)
	why = "coding";
    }
  if (why)
    xsignal2 (Qnidhogg_framing_unsupported, process, intern_c_string (why));
  /* A framing in place goes off first: its bytes are reframed.  */
  nh_io_unframe (process);
  p->plist = plist_put (p->plist, nh_io_key,
			CALLN (Fvector, framing, fn, opts));
  h->m_framed = true;
  nh_io_sync (h, NH_IO_OP_FRAME, &conf);
  return framing;
}

DEFUN ("nidhogg-process-framing", Fnidhogg_process_framing,
       Snidhogg_process_framing, 1, 1, 0,
       doc: /* Return PROCESS's framing (see `nidhogg-set-process-framing'), or nil.  */)
  (Lisp_Object process)
{
  CHECK_PROCESS (process);
  struct nh_io_h *h = nh_io_get (XPROCESS (process));
  Lisp_Object conf = plist_get (XPROCESS (process)->plist, nh_io_key);
  return h && h->m_framed && VECTORP (conf) ? AREF (conf, 0) : Qnil;
}

/* The test framer's memory, freed on any exit.  */
struct nh_io_test_frame
{
  struct nh_io_framer f;
  struct nh_io_msg *msgs;
  struct nh_mat *mat;
};

static void
nh_io_test_frame_free (void *arg)
{
  struct nh_io_test_frame *t = arg;
  free (t->f.buf);
  nh_io_msgs_free (t->msgs);
  if (t->mat)
    nh_json_mat_free (t->mat);
}

DEFUN ("nidhogg-workers--test-frame", Fnidhogg_workers__test_frame,
       Snidhogg_workers__test_frame, 3, MANY, 0,
       doc: /* Frame CHUNKS as the reader frames a process's reads; for tests.
CHUNKS is a list of strings, read one after the other; with EOF
non-nil the stream ends after them.  FRAMING and ARGS are those of
`nidhogg-set-process-framing' (`:batch-function' is ignored).  Return
\(MESSAGES REST ERROR): MESSAGES the batch entries (MESSAGE RAW ERROR)
of every complete message, REST the bytes after the last one (a unibyte
string) and ERROR non-nil after a framing error (REST then holds every
byte from the error on).
usage: (nidhogg-workers--test-frame CHUNKS EOF FRAMING &rest ARGS)  */)
  (ptrdiff_t nargs, Lisp_Object *args)
{
  Lisp_Object chunks = args[0], fn, opts;
  struct nh_io_test_frame t = { 0 };
  nh_io_fr_args (Qnil, args[2], nargs - 3, args + 3, &t.f.c, &fn, &opts);
  specpdl_ref count = SPECPDL_INDEX ();
  record_unwind_protect_ptr (nh_io_test_frame_free, &t);
  struct nh_io_msg **tail = &t.msgs;
  int rc = 0;
  for (Lisp_Object l = chunks; CONSP (l); l = XCDR (l))
    {
      Lisp_Object s = XCAR (l);
      CHECK_STRING (s);
      if (!nh_io_framer_add (&t.f, SDATA (s), SBYTES (s)))
	memory_full (SBYTES (s));
      /* After a framing error the reader frames no more.  */
      if (rc == 0)
	rc = nh_io_frame (&t.f, false, NULL, &tail);
    }
  if (rc == 0 && !NILP (args[1]))
    rc = nh_io_frame (&t.f, true, NULL, &tail);
  if (rc == -2)
    memory_full (0);
  Lisp_Object entries = Qnil;
  for (struct nh_io_msg *m = t.msgs; m; m = m->next)
    {
      Lisp_Object entry;
      nh_io_msg_entry (m, &t.mat, opts, -1, &entry);
      entries = Fcons (entry, entries);
    }
  Lisp_Object rest = (t.f.len > t.f.start
		      ? make_unibyte_string ((const char *) t.f.buf + t.f.start,
					     t.f.len - t.f.start)
		      : empty_unibyte_string);
  return unbind_to (count, list3 (Fnreverse (entries), rest,
				  rc < 0 ? Qt : Qnil));
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
gave up waiting), `direct' (fds closed without nh-io) or `stopped' (a
hand-back or framing switch-off stopped a tokenize; N bytes after the
last message stay for the main thread).  ID names the handle: every
hand-over makes a new one.  */)
  (Lisp_Object on)
{
  static const char *const names[NH_IO_EV_N] = {
    "handover", "read", "eof", "error", "ack", "close", "timeout", "direct",
    "stopped"
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

  /* Task 24.  */
  DEFSYM (Qnidhogg_framing_unsupported, "nidhogg-framing-unsupported");
  define_error (Qnidhogg_framing_unsupported,
		"Process output cannot be framed", Qerror);
  DEFSYM (Qcontent_length, "content-length");
  DEFSYM (Qlines, "lines");
  DEFSYM (Qrecords, "records");
  DEFSYM (QCbatch_function, ":batch-function");
  DEFSYM (QCparse, ":parse");
  DEFSYM (QCmax_message, ":max-message");
  DEFSYM (QCseparator, ":separator");
  nh_io_key = Fmake_symbol (build_string ("nidhogg-framing"));
  staticpro (&nh_io_key);
  nh_io_pending = Qnil;
  staticpro (&nh_io_pending);
  DEFVAR_LISP ("nidhogg-workers--test-deliver-budget",
	       Vnidhogg_workers__test_deliver_budget,
	       doc: /* Test only: a framed delivery's materialization budget, in nanoseconds.
nil, the default, is 4 ms; 0 makes each delivery one message (with
`nidhogg-workers--test-slice-entries' 1, a JSON message takes one
delivery per value).  */);
  Vnidhogg_workers__test_deliver_budget = Qnil;
  defsubr (&Snidhogg_set_process_framing);
  defsubr (&Snidhogg_process_framing);
  defsubr (&Snidhogg_workers__test_frame);

  defsubr (&Snidhogg_set_process_reader);
  defsubr (&Snidhogg_workers__test_io_info);
  defsubr (&Snidhogg_workers__test_io_log);
  defsubr (&Snidhogg_workers__test_io_crash);
  defsubr (&Snidhogg_workers__test_io_hold_lock);
  defsubr (&Snidhogg_workers__test_filter_cost);
}

#endif /* not NH_WK_HARNESS */

#endif /* HAVE_NIDHOGG_WORKERS */

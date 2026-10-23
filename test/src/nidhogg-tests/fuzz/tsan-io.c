/* The reader's fd ownership under ThreadSanitizer (workers-harness.sh
   --tsan io): pipes handed to nh-io while writer threads fill them, the
   main thread taking chunks through the wake protocol and handing each
   pipe back at a random point, then reading the real fd itself.  Every
   byte must arrive once and in order across the hand-back, and the wake
   fd must never stay quiet while bytes wait.  Exit 0 = all rounds whole
   (and, under TSan, no race report).

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

#include "nidhogg-workers.c"
#include "nidhogg-decode.c"
/* The framers' tokenizer (Task 24).  */
#include "nidhogg-json-tape.c"
#include "nidhogg-io.c"

enum { ROUNDS = 200, PIPES = 2, TOTAL = 256 * 1024, READMAX = 4096 };

static unsigned char
pattern (long i)
{
  return (unsigned char) (i * 7 % 251);
}

struct writer
{
  int fd;
  unsigned seed;
};

static void *
writer_main (void *arg)
{
  struct writer *w = arg;
  unsigned char buf[8192];
  long done = 0;
  while (done < TOTAL)
    {
      long n = 1 + rand_r (&w->seed) % sizeof buf;
      if (n > TOTAL - done)
	n = TOTAL - done;
      for (long i = 0; i < n; i++)
	buf[i] = pattern (done + i);
      ssize_t r = write (w->fd, buf, n);
      if (r < 0)
	{
	  if (errno == EINTR)
	    continue;
	  break;
	}
      done += r;
    }
  close (w->fd);
  return NULL;
}

struct reader
{
  int rfd;
  struct nh_io_h *h;
  long got, detach_at;
  bool detached, ended;
};

/* Take what is there for R; false on a wrong byte or an error.  */
static bool
step (struct reader *r)
{
  char buf[READMAX];
  for (;;)
    {
      if (!r->detached && r->got >= r->detach_at)
	{
	  if (!nh_io_detach (r->h))
	    {
	      fprintf (stderr, "io: hand-back timed out\n");
	      return false;
	    }
	  r->detached = true;
	}
      ptrdiff_t n = nh_io_take (r->h, buf, sizeof buf);
      if (n < 0 && errno == EAGAIN && r->detached)
	n = read (r->rfd, buf, sizeof buf);
      if (n == 0)
	{
	  r->ended = true;
	  return true;
	}
      if (n < 0)
	return errno == EAGAIN || (fprintf (stderr, "io: read error %d\n",
					     errno), false);
      for (ptrdiff_t i = 0; i < n; i++)
	if ((unsigned char) buf[i] != pattern (r->got + i))
	  {
	    fprintf (stderr, "io: byte %ld wrong\n", r->got + i);
	    return false;
	  }
      r->got += n;
    }
}

static bool
round_once (unsigned seed)
{
  struct reader rd[PIPES];
  struct writer wr[PIPES];
  pthread_t wt[PIPES];
  bool ok = true;

  for (int i = 0; i < PIPES; i++)
    {
      int fds[2];
      if (pipe (fds) != 0)
	return false;
      fcntl (fds[0], F_SETFL, O_NONBLOCK);
      int why;
      rd[i] = (struct reader) { .rfd = fds[0],
				.detach_at = rand_r (&seed) % (TOTAL + 1) };
      rd[i].h = nh_io_attach (i + 1, fds[0], READMAX, false, &why);
      if (!rd[i].h)
	{
	  fprintf (stderr, "io: attach refused (%d)\n", why);
	  return false;
	}
      wr[i] = (struct writer) { .fd = fds[1], .seed = seed + i };
      pthread_create (&wt[i], NULL, writer_main, &wr[i]);
    }

  for (;;)
    {
      struct pollfd pfd[PIPES];
      int np = 0;
      for (int i = 0; i < PIPES; i++)
	if (!rd[i].ended)
	  pfd[np++] = (struct pollfd) { .fd = rd[i].detached ? rd[i].rfd
					: rd[i].h->sel, .events = POLLIN };
      if (np == 0)
	break;
      /* A quiet wake fd with bytes waiting is a lost wake-up.  */
      if (poll (pfd, np, 5000) == 0)
	{
	  fprintf (stderr, "io: no wake-up in 5 s\n");
	  ok = false;
	  break;
	}
      for (int i = 0; i < PIPES && ok; i++)
	if (!rd[i].ended)
	  ok = step (&rd[i]);
      if (!ok)
	break;
    }

  for (int i = 0; i < PIPES; i++)
    {
      if (ok && rd[i].got != TOTAL)
	{
	  fprintf (stderr, "io: %ld bytes of %d\n", rd[i].got, TOTAL);
	  ok = false;
	}
      if (!rd[i].detached && !nh_io_detach (rd[i].h))
	ok = false;
      else
	nh_io_free (rd[i].h);
      close (rd[i].rfd);
      pthread_join (wt[i], NULL);
    }
  return ok;
}

int
main (void)
{
  for (unsigned r = 0; r < ROUNDS; r++)
    if (!round_once (r * 7919 + 1))
      {
	printf ("io: round %u failed\n", r);
	return 1;
      }
  printf ("io: %d rounds of %d pipes, every byte once and in order\n",
	  ROUNDS, PIPES);
  return 0;
}

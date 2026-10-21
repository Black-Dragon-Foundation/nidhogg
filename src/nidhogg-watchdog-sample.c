/* Nidhogg's stall watchdog: the signal handler, the raw ring and `--take'.

   Task 9 wrote the handler, the guards, the specpdl walk, the thinning
   and the validation; Task 18 adds names mode.

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

/* The handler's rules (the design's §4.3; the plan's Global
   Constraints, "The handler"):

   - It saves errno, and returns at once (counting `stale') off the main
     thread, without a pending request, with a request for another span,
     while the main thread waits, or while the span is being closed.
   - It calls nothing but clock_gettime, allocates nothing, takes no
     lock, never runs Lisp, never signals and never longjmps.  Its
     shared words are lock-free __atomic_* operations.
   - It dereferences nothing but the main thread's specpdl slots, with
     two exceptions: a hook runner's first argument, read only for
     frames below the innermost, only when NARGS >= 1, and tested with
     the tag-only BARE_SYMBOL_P; and (Task 18) symbol names below the
     innermost frame when !gc_in_progress.  The innermost slot may be
     half-written (an entry is written with plain stores, then
     specpdl_ptr moves), so its words are copied raw and never followed.
   - The words it copies are never Lisp objects: nothing here is a GC
     root, and nh_wd_sample_to_lisp turns a word into an object only
     after validating it, on the main thread, outside the handler.

   Ordering: the handler interrupts the main thread, so only the
   compiler reorders what it sees; the main thread's writes it reads are
   bracketed by __atomic_signal_fence (the specpdl growth bracket in
   nidhogg-watchdog.h, the phase push in nidhogg-watchdog-phase.h, and
   nh_wd_sample_close's `closed' flag below).  */

#include <config.h>

#ifdef HAVE_NIDHOGG_WATCHDOG

#include <errno.h>
#include <stdlib.h>

#include "nidhogg-watchdog.h"
#include "nidhogg-watchdog-phase.h"
#include "pdumper.h"
#include "syssignal.h"
#ifdef HAVE_NATIVE_COMP
# include "comp.h"
#endif

/* The request word: bit 31 pending, the low 31 bits the span.  */
#define NH_WD_REQ_PENDING ((uint32_t) 1 << 31)
#define NH_WD_REQ_SPAN (NH_WD_REQ_PENDING - 1)

/* True on the thread that installed the handler, which is the main
   thread: a thread-local flag, so the handler's first test is a load,
   not a call.  */
static _Thread_local bool nh_wd_this_is_main;

/* The main thread's thread_state (&main_thread.s, static in thread.c),
   saved when the handler is installed.  */
static struct thread_state *nh_wd_main_state;

/* The words of the hook runners' symbols, computed at install.  */
enum { NH_WD_HOOK_RUNNERS = 5, NH_WD_HOOKS = 4 };
static uintptr_t nh_wd_hook_runner[NH_WD_HOOK_RUNNERS];

/* State only the handler writes (and nh_wd_sample_close, while the
   handler is shut out by `closed').  USED is a bit per slot of
   nh_wd_cur.acc.s; NREQ counts the span's requests; SEQ is the request
   number each kept slot answered (for the tests).  */
static struct
{
  uint64_t used;
  uint32_t nreq;
  uint32_t seq[NH_WD_SAMPLES];
} nh_wd_hs;

/* Test-only flags (PD-3), read by the handler.  */
static volatile sig_atomic_t nh_wd_test_lisp_thread;

/* The installed action and the one it replaced.  */
static bool nh_wd_installed;
static struct sigaction nh_wd_old_action;

/* The sorted addresses of the builtin (non-heap) subrs, collected at
   install: a word naming a non-heap subr is accepted only when it is
   one of these.  */
static struct Lisp_Subr **nh_wd_subrs;
static ptrdiff_t nh_wd_nsubrs, nh_wd_subrs_alloc;


/* The handler.  */

/* Start the accumulator over for SPAN (handler only).  */
static void
nh_wd_acc_reset (struct nh_wd_stall *acc, uint64_t span)
{
  acc->span = span;
  acc->samples = 0;
  acc->thinned = 0;
  acc->stride = 1;
  acc->lisp_thread = 0;
  acc->deep = __atomic_load_n (&nh_wd.deep_left, __ATOMIC_RELAXED) > 0;
  nh_wd_hs.used = 0;
  nh_wd_hs.nreq = 0;
  for (int i = 0; i < 4; i++)
    nh_wd_cur.names[i].n = 0;
  __atomic_store_n (&nh_wd_cur.names_next, 0, __ATOMIC_RELEASE);
  __atomic_store_n (&nh_wd.stride, 1, __ATOMIC_RELAXED);
}

/* Keep every other kept sample, free the others' slots, double the
   stride.  Only the index array moves (PD-5).  */
static void
nh_wd_acc_thin (struct nh_wd_stall *acc)
{
  uint32_t k = 0;
  for (uint32_t i = 0; i < acc->samples; i++)
    if (i % 2 == 0)
      acc->order[k++] = acc->order[i];
    else
      nh_wd_hs.used &= ~((uint64_t) 1 << acc->order[i]);
  acc->thinned += acc->samples - k;
  acc->samples = k;
  acc->stride *= 2;
  __atomic_store_n (&nh_wd.stride, acc->stride, __ATOMIC_RELAXED);
}

/* The slot for the span's request number N (0-based), or NULL when the
   stride skips it.  Thins first when the stall is full.  */
static struct nh_wd_sample *
nh_wd_acc_slot (struct nh_wd_stall *acc, uint32_t n)
{
  if (n % acc->stride != 0)
    return NULL;
  uint32_t cap = acc->deep ? NH_WD_SAMPLES / 2 : NH_WD_SAMPLES;
  if (acc->samples >= cap)
    {
      nh_wd_acc_thin (acc);
      if (n % acc->stride != 0)
	return NULL;
    }
  int slot = __builtin_ctzll (~nh_wd_hs.used);
  nh_wd_hs.used |= (uint64_t) 1 << slot;
  nh_wd_hs.seq[slot] = n + 1;
  acc->order[acc->samples++] = slot;
  return &acc->s[slot];
}

static bool
nh_wd_hook_runner_p (uintptr_t fn)
{
  for (int i = 0; i < NH_WD_HOOK_RUNNERS; i++)
    if (fn == nh_wd_hook_runner[i])
      return true;
  return false;
}

/* Note the hook named by the frame P at WORDS index I, when P runs a
   hook (P is below the innermost frame: complete).  */
static void
nh_wd_note_hook (struct nh_wd_sample *s, int *nhooks,
		 const union specbinding *p, int i)
{
  if (*nhooks >= NH_WD_HOOKS || !nh_wd_hook_runner_p (XLI (p->bt.function)))
    return;
  ptrdiff_t nargs = p->bt.nargs;
  Lisp_Object *args = p->bt.args;
  if (nargs < 1 || !args)
    return;
  Lisp_Object hook = args[0];
  if (!BARE_SYMBOL_P (hook))
    return;
  s->hook_sym[*nhooks] = XLI (hook);
  s->hook_frame[*nhooks] = i;
  (*nhooks)++;
}

/* Copy the main thread's backtrace words into S.  BASE..TOP are the
   specpdl's used slots, checked consistent by the caller.  */
static void
nh_wd_walk (struct nh_wd_sample *s, const union specbinding *base,
	    ptrdiff_t top, int max_inner)
{
  int ni = 0, no = 0, nhooks = 0;
  ptrdiff_t low = top;

  /* Innermost first, down from the top.  */
  for (ptrdiff_t i = top; 0 < i && ni < max_inner; )
    {
      const union specbinding *p = &base[--i];
      if (p->kind != SPECPDL_BACKTRACE)
	continue;
      if (ni > 0)
	nh_wd_note_hook (s, &nhooks, p, ni);
      s->words[ni++] = XLI (p->bt.function);
      low = i;
    }

  /* The outermost frames, below LOW: find the slot past the
     NH_WD_OUTER-th frame up from the bottom, then copy down from it so
     they too are innermost first.  */
  ptrdiff_t stop = 0;
  for (int n = 0; stop < low && n < NH_WD_OUTER; stop++)
    if (base[stop].kind == SPECPDL_BACKTRACE)
      n++;
  for (ptrdiff_t i = stop; 0 < i; )
    {
      const union specbinding *p = &base[--i];
      if (p->kind != SPECPDL_BACKTRACE)
	continue;
      nh_wd_note_hook (s, &nhooks, p, ni + no);
      s->words[ni + no++] = XLI (p->bt.function);
    }

  /* The frames between the two.  */
  uint32_t skipped = 0;
  for (ptrdiff_t i = stop; i < low; i++)
    skipped += base[i].kind == SPECPDL_BACKTRACE;

  s->n_inner = ni;
  s->n_outer = no;
  s->skipped = skipped < UINT16_MAX ? skipped : UINT16_MAX;
}

/* Fill S: the metadata always, the walk unless a guard says no.  */
static void
nh_wd_fill (struct nh_wd_sample *s, struct nh_wd_stall *acc)
{
  struct timespec ts;
  clock_gettime (CLOCK_MONOTONIC, &ts);
  uint64_t now = (uint64_t) ts.tv_sec * 1000000000 + ts.tv_nsec;
  uint64_t since = __atomic_load_n (&nh_wd.busy_since_ns, __ATOMIC_RELAXED);
  s->t_ns = now > since ? now - since : 0;
  nh_wd_phase_top (&s->phase, &s->phase_reason);
  s->gc = gc_in_progress;
  s->deep = acc->deep;
  s->lisp_thread = 0;
  s->thread_name = 0;
  s->n_inner = s->n_outer = s->skipped = 0;
  for (int i = 0; i < NH_WD_HOOKS; i++)
    {
      s->hook_sym[i] = 0;
      s->hook_frame[i] = 0;
    }

  /* Guard: the specpdl is being reallocated.  */
  if (nh_wd_specpdl_unstable)
    return;

  /* Guard: a Lisp thread holds the lock.  Its specpdl changes under us,
     so only the thread is recorded.  */
  struct thread_state *t = __atomic_load_n (&current_thread,
					    __ATOMIC_RELAXED);
  if (t != nh_wd_main_state || nh_wd_test_lisp_thread)
    {
      s->lisp_thread = 1;
      s->thread_name = (uintptr_t) t;
      acc->lisp_thread = 1;
      return;
    }

  /* Guard: the pointers must be consistent.  */
  union specbinding *base = t->m_specpdl;
  union specbinding *ptr = t->m_specpdl_ptr;
  union specbinding *end = t->m_specpdl_end;
  if (!base || !(base <= ptr && ptr <= end))
    return;

  nh_wd_walk (s, base, ptr - base,
	      acc->deep ? NH_WD_DEEP_INNER : NH_WD_INNER);
}

static void
nh_wd_handler (int sig)
{
  int saved_errno = errno;
  (void) sig;

  uint32_t req = __atomic_load_n (&nh_wd.request, __ATOMIC_ACQUIRE);
  uint64_t span = __atomic_load_n (&nh_wd.span, __ATOMIC_ACQUIRE);
  if (!nh_wd_this_is_main
      || !(req & NH_WD_REQ_PENDING)
      || (req & NH_WD_REQ_SPAN) != (span & NH_WD_REQ_SPAN)
      || __atomic_load_n (&nh_wd.waiting, __ATOMIC_ACQUIRE) != 0
      || nh_wd_cur.closed)
    {
      __atomic_fetch_add (&nh_wd.stale, 1, __ATOMIC_RELAXED);
      errno = saved_errno;
      return;
    }

  /* Span isolation: the accumulator belongs to one span, and only the
     handler starts it over.  */
  struct nh_wd_stall *acc = &nh_wd_cur.acc;
  if (acc->stride == 0 || acc->span != span)
    nh_wd_acc_reset (acc, span);

  struct nh_wd_sample *s = nh_wd_acc_slot (acc, nh_wd_hs.nreq++);
  if (s)
    {
      nh_wd_fill (s, acc);
      __atomic_fetch_add (&nh_wd.samples_taken, 1, __ATOMIC_RELAXED);
    }

  /* Answered: clear the request this handler saw, not a newer one.  */
  __atomic_compare_exchange_n (&nh_wd.request, &req, 0, false,
			       __ATOMIC_RELEASE, __ATOMIC_RELAXED);
  errno = saved_errno;
}


/* Installing it.  */

static void
nh_wd_collect_subr (Lisp_Object sym, Lisp_Object arg)
{
  (void) arg;
  Lisp_Object fn = XSYMBOL (sym)->u.s.function;
  if (!SUBRP (fn) || NATIVE_COMP_FUNCTIONP (fn))
    return;
  if (nh_wd_nsubrs == nh_wd_subrs_alloc)
    nh_wd_subrs = xpalloc (nh_wd_subrs, &nh_wd_subrs_alloc, 1, -1,
			   sizeof *nh_wd_subrs);
  nh_wd_subrs[nh_wd_nsubrs++] = XSUBR (fn);
}

static int
nh_wd_ptr_cmp (const void *a, const void *b)
{
  uintptr_t x = (uintptr_t) *(struct Lisp_Subr *const *) a;
  uintptr_t y = (uintptr_t) *(struct Lisp_Subr *const *) b;
  return (x > y) - (x < y);
}

static bool
nh_wd_builtin_subr_p (const void *p)
{
  return (nh_wd_nsubrs
	  && bsearch (&p, nh_wd_subrs, nh_wd_nsubrs, sizeof *nh_wd_subrs,
		      nh_wd_ptr_cmp));
}

int
nh_wd_handler_install (void)
{
  if (nh_wd_installed)
    return 0;
  if (!main_thread_p (current_thread))
    return -1;

  /* A library that claimed the signal keeps it: the watchdog refuses
     rather than break it (the W0 survey found none, Task 6).  */
  struct sigaction old;
  if (sigaction (NIDHOGG_WATCHDOG_SIGNAL, NULL, &old) != 0
      || ((old.sa_flags & SA_SIGINFO)
	  || (old.sa_handler != SIG_DFL && old.sa_handler != SIG_IGN)))
    return -1;

  nh_wd_main_state = current_thread;
  nh_wd_this_is_main = true;
  nh_wd_hook_runner[0] = XLI (Qrun_hooks);
  nh_wd_hook_runner[1] = XLI (Qrun_hook_with_args);
  nh_wd_hook_runner[2] = XLI (Qrun_hook_with_args_until_success);
  nh_wd_hook_runner[3] = XLI (Qrun_hook_with_args_until_failure);
  nh_wd_hook_runner[4] = XLI (Qrun_hook_wrapped);

  nh_wd_nsubrs = 0;
  map_obarray (Vobarray, nh_wd_collect_subr, Qnil);
  if (nh_wd_nsubrs)
    qsort (nh_wd_subrs, nh_wd_nsubrs, sizeof *nh_wd_subrs, nh_wd_ptr_cmp);

  /* A fresh accumulator: the first sample starts it.  */
  nh_wd_cur.acc.stride = 0;
  nh_wd_cur.acc.samples = 0;
  nh_wd_cur.closed = 0;

  struct sigaction act;
  emacs_sigaction_init (&act, nh_wd_handler);
  /* Explicitly: emacs_sigaction_init sets SA_RESTART only in batch, and
     the watchdog never needs to break a system call (§4.3).  */
  act.sa_flags = SA_RESTART;
  if (sigaction (NIDHOGG_WATCHDOG_SIGNAL, &act, &nh_wd_old_action) != 0)
    return -1;
  nh_wd_installed = true;
  return 0;
}

void
nh_wd_handler_uninstall (void)
{
  if (!nh_wd_installed)
    return;
  sigaction (NIDHOGG_WATCHDOG_SIGNAL, &nh_wd_old_action, NULL);
  nh_wd_installed = false;
  xfree (nh_wd_subrs);
  nh_wd_subrs = NULL;
  nh_wd_nsubrs = nh_wd_subrs_alloc = 0;
}


/* Closing the span's samples (the main thread's finalise, Task 7).  */

void
nh_wd_sample_close (struct nh_wd_stall *out)
{
  struct nh_wd_stall *acc = &nh_wd_cur.acc;

  /* From here on the handler returns at once.  */
  nh_wd_cur.closed = 1;
  __atomic_signal_fence (__ATOMIC_SEQ_CST);

  /* Only this span's samples: an accumulator left by a shorter span
     that ended below the threshold is not this stall's.  */
  uint64_t span = __atomic_load_n (&nh_wd.span, __ATOMIC_RELAXED);
  bool mine = acc->stride != 0 && acc->span == span;
  uint32_t n = mine ? acc->samples : 0;
  for (uint32_t i = 0; i < n; i++)
    {
      out->s[i] = acc->s[acc->order[i]];
      out->order[i] = i;
    }
  out->samples = n;
  out->thinned = mine ? acc->thinned : 0;
  out->stride = mine ? acc->stride : 1;
  out->lisp_thread = mine && acc->lisp_thread;
  out->deep = mine && acc->deep;

  acc->samples = 0;
  acc->stride = 0;
  __atomic_signal_fence (__ATOMIC_SEQ_CST);
  nh_wd_cur.closed = 0;
}


/* Validation: raw words become Lisp objects (main thread, outside the
   handler).  */

/* Whether P is the start of a live object in the heap or the dump: a
   probe tagged as a float never takes valid_lisp_object_p's
   "readable memory that looks like a subr" path, which a stale word
   could satisfy.  */
static bool
nh_wd_heap_object_p (void *p)
{
  return valid_lisp_object_p (make_lisp_ptr (p, Lisp_Float)) == 1;
}

/* Whether the dump's type of the object at P (when P is in the dump)
   agrees with TYPE.  */
static bool
nh_wd_dump_type_ok (void *p, enum Lisp_Type type)
{
  return !pdumper_object_p (p) || pdumper_find_object_type (p) == type;
}

static bool
nh_wd_valid_word_p (Lisp_Object obj)
{
  return valid_lisp_object_p (obj) == 1;
}

/* OBJ (vectorlike, validated) looks like a whole closure.  */
static bool
nh_wd_closure_ok (Lisp_Object obj)
{
  ptrdiff_t size = XVECTOR (obj)->header.size & PSEUDOVECTOR_SIZE_MASK;
  if (size <= CLOSURE_CODE || 64 < size)
    return false;
  Lisp_Object args = XVECTOR (obj)->contents[CLOSURE_ARGLIST];
  Lisp_Object code = XVECTOR (obj)->contents[CLOSURE_CODE];
  return (nh_wd_valid_word_p (args) && nh_wd_valid_word_p (code)
	  && (NILP (code) || CONSP (code) || STRINGP (code)));
}

/* OBJ (vectorlike, validated, not a builtin) looks like a native subr.  */
static bool
nh_wd_native_subr_ok (Lisp_Object obj)
{
#ifdef HAVE_NATIVE_COMP
  Lisp_Object u = XSUBR (obj)->native_comp_u;
  return nh_wd_valid_word_p (u) && NATIVE_COMP_UNITP (u);
#else
  (void) obj;
  return false;
#endif
}

/* A vectorlike word W, validated: the object, or Qunbound.  */
static Lisp_Object
nh_wd_vectorlike (uintptr_t w)
{
  Lisp_Object obj = XIL (w);
  if (!VECTORLIKEP (obj))
    return Qunbound;
  void *p = XUNTAG (obj, Lisp_Vectorlike, struct Lisp_Vector);
  if (nh_wd_builtin_subr_p (p))
    return obj;
  if (!nh_wd_valid_word_p (obj) || !nh_wd_heap_object_p (p)
      || !nh_wd_dump_type_ok (p, Lisp_Vectorlike))
    return Qunbound;
  return obj;
}

/* A symbol word W, validated: the symbol, or Qunbound.  */
static Lisp_Object
nh_wd_symbol (uintptr_t w)
{
  Lisp_Object obj = XIL (w);
  if (!BARE_SYMBOL_P (obj) || !nh_wd_valid_word_p (obj))
    return Qunbound;
  struct Lisp_Symbol *p = XBARE_SYMBOL (obj);
  if (c_symbol_p (p))
    return obj;
  if (!nh_wd_dump_type_ok (p, Lisp_Symbol))
    return Qunbound;
  Lisp_Object name = p->u.s.name;
  return nh_wd_valid_word_p (name) && STRINGP (name) ? obj : Qunbound;
}

/* The function object a raw word W names, or `nidhogg-watchdog--gone':
   a symbol, a closure or a subr that valid_lisp_object_p calls live
   (exactly 1: -1 means it cannot tell, under ASan or rr), a non-heap
   subr only when it is a builtin's address.  */
static Lisp_Object
nh_wd_word_function (uintptr_t w)
{
  if (BARE_SYMBOL_P (XIL (w)))
    {
      Lisp_Object sym = nh_wd_symbol (w);
      return BASE_EQ (sym, Qunbound) ? Qnidhogg_watchdog__gone : sym;
    }
  Lisp_Object obj = nh_wd_vectorlike (w);
  if (BASE_EQ (obj, Qunbound))
    return Qnidhogg_watchdog__gone;
  if (CLOSUREP (obj))
    return nh_wd_closure_ok (obj) ? obj : Qnidhogg_watchdog__gone;
  if (SUBRP (obj))
    return (nh_wd_builtin_subr_p (XSUBR (obj)) || nh_wd_native_subr_ok (obj)
	    ? obj : Qnidhogg_watchdog__gone);
  return Qnidhogg_watchdog__gone;
}

/* A per-call cache of word -> object.  */
struct nh_wd_cache
{
  int n;
  uintptr_t w[NH_WD_WORDS + 4];
  Lisp_Object o[NH_WD_WORDS + 4];
};

static Lisp_Object
nh_wd_cached (struct nh_wd_cache *c, uintptr_t w, Lisp_Object (*f) (uintptr_t))
{
  for (int i = 0; i < c->n; i++)
    if (c->w[i] == w)
      return c->o[i];
  Lisp_Object o = f (w);
  if (c->n < NH_WD_WORDS + 4)
    {
      c->w[c->n] = w;
      c->o[c->n++] = o;
    }
  return o;
}

static Lisp_Object
nh_wd_hook_symbol (uintptr_t w)
{
  Lisp_Object sym = nh_wd_symbol (w);
  return BASE_EQ (sym, Qunbound) ? Qnidhogg_watchdog__gone : sym;
}

static const char *const nh_wd_phase_names[NH_WD_PH_N] = {
  "lisp", "redisplay", "filter", "sentinel", "process-wait",
  "call-process", "json", "regex", "compose", "deliver", "treesit",
  "treesit-wait", "image", "worker-wait"
};

static const char *const nh_wd_reason_names[] = {
  NULL, "snapshot", "display", "image-pixels", "detach", "drain"
};

/* The `:lisp-thread' of a sample: the thread's name, t for an unnamed
   or vanished thread, nil when the main thread held the lock.  */
static Lisp_Object
nh_wd_thread_name (const struct nh_wd_sample *s)
{
  if (!s->lisp_thread)
    return Qnil;
  if (!s->thread_name)
    return Qt;
  Lisp_Object obj
    = nh_wd_vectorlike (XLI (make_lisp_ptr ((void *) s->thread_name,
					     Lisp_Vectorlike)));
  if (BASE_EQ (obj, Qunbound) || !THREADP (obj))
    return Qt;
  Lisp_Object name = XTHREAD (obj)->name;
  return STRINGP (name) ? name : Qt;
}

Lisp_Object
nh_wd_sample_to_lisp (const struct nh_wd_sample *s)
{
  struct nh_wd_cache c = { .n = 0 };
  int ni = min (s->n_inner, NH_WD_WORDS);
  int no = min (s->n_outer, NH_WD_WORDS - ni);
  Lisp_Object frames = make_nil_vector (ni);
  Lisp_Object outer = make_nil_vector (no);
  Lisp_Object hooks = make_nil_vector (ni + no);
  for (int i = 0; i < ni; i++)
    ASET (frames, i, nh_wd_cached (&c, s->words[i], nh_wd_word_function));
  for (int i = 0; i < no; i++)
    ASET (outer, i, nh_wd_cached (&c, s->words[ni + i], nh_wd_word_function));
  for (int i = 0; i < NH_WD_HOOKS; i++)
    if (s->hook_sym[i] && s->hook_frame[i] < ni + no)
      ASET (hooks, s->hook_frame[i],
	    nh_wd_cached (&c, s->hook_sym[i], nh_wd_hook_symbol));

  Lisp_Object phase = (s->phase < NH_WD_PH_N
		       ? intern_c_string (nh_wd_phase_names[s->phase]) : Qnil);
  Lisp_Object reason = (0 < s->phase_reason
			&& s->phase_reason < ARRAYELTS (nh_wd_reason_names)
			? intern_c_string (nh_wd_reason_names[s->phase_reason])
			: Qnil);
  return list (QCt_ms, make_float (s->t_ns / 1e6),
	       QCphase, phase, QCreason, reason,
	       QCgc, s->gc ? Qt : Qnil,
	       QCframes, frames, QCouter, outer,
	       QCskipped, make_fixnum (s->skipped),
	       QChooks, hooks,
	       QClisp_thread, nh_wd_thread_name (s));
}


/* Lisp: deep mode and the test-only primitives (PD-3).  */

DEFUN ("nidhogg-watchdog--deep", Fnidhogg_watchdog__deep,
       Snidhogg_watchdog__deep, 1, 1, 0,
       doc: /* Sample the next N stalls deeply: 56 inner frames, P = 1 ms.
Return N.  */)
  (Lisp_Object n)
{
  CHECK_FIXNAT (n);
  __atomic_store_n (&nh_wd.deep_left, (int32_t) min (XFIXNAT (n), INT32_MAX),
		    __ATOMIC_RELAXED);
  return n;
}

/* Block the signal on this thread (OLD gets the previous mask), so the
   handler cannot run while the caller copies or plants.  Nothing
   between this and the restore may signal or allocate.  */
static void
nh_wd_block_signal (sigset_t *old)
{
  sigset_t block;
  sigemptyset (&block);
  sigaddset (&block, NIDHOGG_WATCHDOG_SIGNAL);
  pthread_sigmask (SIG_BLOCK, &block, old);
}

static void
nh_wd_require_installed (void)
{
  if (!nh_wd_installed)
    error ("nidhogg-watchdog: not started");
}

/* Raise one synchronous sample for the current span: a forged request,
   then pthread_kill to this thread, which Linux delivers before
   pthread_kill returns.  Return whether it was kept.  */
static bool
nh_wd_raise_sample (void)
{
  uint64_t span = __atomic_load_n (&nh_wd.span, __ATOMIC_RELAXED);
  uint32_t req = NH_WD_REQ_PENDING | (span & NH_WD_REQ_SPAN);
  uint64_t before = __atomic_load_n (&nh_wd.samples_taken, __ATOMIC_RELAXED);
  __atomic_store_n (&nh_wd.request, req, __ATOMIC_RELEASE);
  pthread_kill (pthread_self (), NIDHOGG_WATCHDOG_SIGNAL);
  __atomic_compare_exchange_n (&nh_wd.request, &req, 0, false,
			       __ATOMIC_RELEASE, __ATOMIC_RELAXED);
  return __atomic_load_n (&nh_wd.samples_taken, __ATOMIC_RELAXED) != before;
}

DEFUN ("nidhogg-watchdog--test-sample", Fnidhogg_watchdog__test_sample,
       Snidhogg_watchdog__test_sample, 0, 0, 0,
       doc: /* Test only: take one sample of this stack, synchronously.
Return it converted, or nil when the stride skipped it.  */)
  (void)
{
  nh_wd_require_installed ();
  if (!nh_wd_raise_sample ())
    return Qnil;
  struct nh_wd_sample s;
  sigset_t old;
  nh_wd_block_signal (&old);
  struct nh_wd_stall *acc = &nh_wd_cur.acc;
  s = acc->s[acc->order[acc->samples - 1]];
  pthread_sigmask (SIG_SETMASK, &old, NULL);
  return nh_wd_sample_to_lisp (&s);
}

DEFUN ("nidhogg-watchdog--test-raw-samples",
       Fnidhogg_watchdog__test_raw_samples,
       Snidhogg_watchdog__test_raw_samples, 1, 1, 0,
       doc: /* Test only: raise N synchronous samples, converting nothing.
Return N.  */)
  (Lisp_Object n)
{
  CHECK_FIXNAT (n);
  nh_wd_require_installed ();
  for (EMACS_INT i = 0; i < XFIXNAT (n); i++)
    nh_wd_raise_sample ();
  return n;
}

DEFUN ("nidhogg-watchdog--test-forge-late-request",
       Fnidhogg_watchdog__test_forge_late_request,
       Snidhogg_watchdog__test_forge_late_request, 0, 0, 0,
       doc: /* Test only: a request that arrives after its span ended.
Publish waiting as the finalise does, raise a request for the span that
just ended, then publish busy again.  Return the number of samples the
handler kept for it (0 expected: it counts the request stale).  */)
  (void)
{
  nh_wd_require_installed ();
  nh_wd_publish_waiting (nh_wd_now_ns ());
  bool kept = nh_wd_raise_sample ();
  nh_wd_publish_busy (nh_wd_now_ns ());
  return make_fixnum (kept);
}

DEFUN ("nidhogg-watchdog--test-plant-words",
       Fnidhogg_watchdog__test_plant_words,
       Snidhogg_watchdog__test_plant_words, 1, 1, 0,
       doc: /* Test only: add a sample whose inner frames are the raw WORDS.
WORDS is a list of non-negative integers.  Return nil.  */)
  (Lisp_Object words)
{
  nh_wd_require_installed ();
  uintptr_t w[NH_WD_WORDS];
  int n = 0;
  for (Lisp_Object tail = words; CONSP (tail); tail = XCDR (tail))
    {
      uintmax_t v;
      if (n == NH_WD_WORDS || !integer_to_uintmax (XCAR (tail), &v)
	  || UINTPTR_MAX < v)
	xsignal1 (Qargs_out_of_range, XCAR (tail));
      w[n++] = v;
    }

  /* As the handler would, with the signal blocked so it cannot run
     meanwhile.  */
  sigset_t old;
  nh_wd_block_signal (&old);
  struct nh_wd_stall *acc = &nh_wd_cur.acc;
  uint64_t span = __atomic_load_n (&nh_wd.span, __ATOMIC_RELAXED);
  if (acc->stride == 0 || acc->span != span)
    nh_wd_acc_reset (acc, span);
  uint32_t nreq = nh_wd_hs.nreq;
  while (nreq % acc->stride != 0)
    nreq++;
  nh_wd_hs.nreq = nreq + 1;
  struct nh_wd_sample *s = nh_wd_acc_slot (acc, nreq);
  if (s)
    {
      nh_wd_fill (s, acc);
      s->n_outer = s->skipped = 0;
      for (int i = 0; i < NH_WD_HOOKS; i++)
	s->hook_sym[i] = 0;
      for (int i = 0; i < n; i++)
	s->words[i] = w[i];
      s->n_inner = n;
    }
  pthread_sigmask (SIG_SETMASK, &old, NULL);
  return Qnil;
}

DEFUN ("nidhogg-watchdog--test-word", Fnidhogg_watchdog__test_word,
       Snidhogg_watchdog__test_word, 1, 1, 0,
       doc: /* Test only: the raw word of OBJ, as an integer.
OBJ `:c-function' gives a C function's address; `:text-vectorlike' an
address inside the program text, tagged as a vectorlike object.  */)
  (Lisp_Object obj)
{
  uintptr_t fn = (uintptr_t) Fcar;
  if (EQ (obj, QCc_function))
    return make_uint (fn);
  if (EQ (obj, QCtext_vectorlike))
    return make_uint (XLI (make_lisp_ptr ((void *) (fn & ~(uintptr_t) 7),
					   Lisp_Vectorlike)));
  return make_uint (XLI (obj));
}

/* Allocate N large vectors in this frame and store their words, masked,
   in W: once this frame is gone no stack slot holds them, so the next
   collections free them.  */
#define NH_WD_MASK ((uintptr_t) 0x5a5a5a5a5a5a5a5a)
static NO_INLINE void
nh_wd_alloc_garbage (uintptr_t *w, int n)
{
  for (int i = 0; i < n; i++)
    w[i] = XLI (make_nil_vector (2048)) ^ NH_WD_MASK;
}

DEFUN ("nidhogg-watchdog--test-freed-word",
       Fnidhogg_watchdog__test_freed_word,
       Snidhogg_watchdog__test_freed_word, 0, 0, 0,
       doc: /* Test only: the raw word of a vector that has been collected.
Return it as an integer.  */)
  (void)
{
  enum { N = 32 };
  uintptr_t *w = xmalloc (N * sizeof *w);
  nh_wd_alloc_garbage (w, N);
  Fgarbage_collect ();
  Fgarbage_collect ();
  uintptr_t found = 0;
  for (int i = 0; i < N && !found; i++)
    if (valid_lisp_object_p (XIL (w[i] ^ NH_WD_MASK)) == 0)
      found = w[i] ^ NH_WD_MASK;
  xfree (w);
  if (!found)
    error ("nidhogg-watchdog: no collected vector found");
  return make_uint (found);
}

DEFUN ("nidhogg-watchdog--test-force-lisp-thread",
       Fnidhogg_watchdog__test_force_lisp_thread,
       Snidhogg_watchdog__test_force_lisp_thread, 1, 1, 0,
       doc: /* Test only: non-nil makes the handler see a Lisp thread.  */)
  (Lisp_Object flag)
{
  nh_wd_test_lisp_thread = !NILP (flag);
  return flag;
}

DEFUN ("nidhogg-watchdog--test-set-unstable",
       Fnidhogg_watchdog__test_set_unstable,
       Snidhogg_watchdog__test_set_unstable, 1, 1, 0,
       doc: /* Test only: set the specpdl-growth flag to FLAG.  */)
  (Lisp_Object flag)
{
  __atomic_signal_fence (__ATOMIC_SEQ_CST);
  nh_wd_specpdl_unstable = !NILP (flag);
  __atomic_signal_fence (__ATOMIC_SEQ_CST);
  return flag;
}

DEFUN ("nidhogg-watchdog--test-handler-flags",
       Fnidhogg_watchdog__test_handler_flags,
       Snidhogg_watchdog__test_handler_flags, 0, 1, 0,
       doc: /* Test only: the `sa_flags' of the signal's installed action.
With CONSTANT non-nil, the value of SA_RESTART instead.  */)
  (Lisp_Object constant)
{
  if (!NILP (constant))
    return make_fixnum (SA_RESTART);
  struct sigaction cur;
  if (sigaction (NIDHOGG_WATCHDOG_SIGNAL, NULL, &cur) != 0)
    report_file_error ("sigaction", Qnil);
  return make_fixnum (cur.sa_flags);
}

DEFUN ("nidhogg-watchdog--test-accumulator",
       Fnidhogg_watchdog__test_accumulator,
       Snidhogg_watchdog__test_accumulator, 0, 0, 0,
       doc: /* Test only: the span in progress's samples, converted.
A plist: :span, :stride, :thinned, :deep, :lisp-thread, :seq (each kept
sample's request number), :samples, and the handler's :stale.  */)
  (void)
{
  /* Copy under the blocked signal, convert after: conversion allocates
     and may signal, which must not leave the signal blocked.  */
  specpdl_ref count = SPECPDL_INDEX ();
  struct nh_wd_stall *acc = xmalloc (sizeof *acc);
  record_unwind_protect_ptr (xfree, acc);
  uint32_t seqs[NH_WD_SAMPLES];
  sigset_t old;
  nh_wd_block_signal (&old);
  *acc = nh_wd_cur.acc;
  uint32_t n = acc->stride ? acc->samples : 0;
  for (uint32_t i = 0; i < n; i++)
    seqs[i] = nh_wd_hs.seq[acc->order[i]];
  pthread_sigmask (SIG_SETMASK, &old, NULL);

  Lisp_Object samples = make_nil_vector (n);
  Lisp_Object seq = make_nil_vector (n);
  for (uint32_t i = 0; i < n; i++)
    {
      ASET (samples, i, nh_wd_sample_to_lisp (&acc->s[acc->order[i]]));
      ASET (seq, i, make_fixnum (seqs[i]));
    }
  Lisp_Object result
    = list (QCspan, make_uint (acc->span),
	    QCstride, make_fixnum (acc->stride),
	    QCthinned, make_fixnum (acc->thinned),
	    QCdeep, acc->deep ? Qt : Qnil,
	    QClisp_thread, acc->lisp_thread ? Qt : Qnil,
	    QCseq, seq, QCsamples, samples,
	    QCstale, make_uint (__atomic_load_n (&nh_wd.stale,
						 __ATOMIC_RELAXED)));
  return unbind_to (count, result);
}

void
nh_wd_syms_of_sample (void)
{
  DEFSYM (Qnidhogg_watchdog__gone, "nidhogg-watchdog--gone");
  DEFSYM (Qrun_hooks, "run-hooks");
  DEFSYM (Qrun_hook_with_args, "run-hook-with-args");
  DEFSYM (Qrun_hook_with_args_until_success,
	  "run-hook-with-args-until-success");
  DEFSYM (Qrun_hook_with_args_until_failure,
	  "run-hook-with-args-until-failure");
  DEFSYM (Qrun_hook_wrapped, "run-hook-wrapped");

  DEFSYM (QCt_ms, ":t-ms");
  DEFSYM (QCphase, ":phase");
  DEFSYM (QCreason, ":reason");
  DEFSYM (QCgc, ":gc");
  DEFSYM (QCframes, ":frames");
  DEFSYM (QCouter, ":outer");
  DEFSYM (QCskipped, ":skipped");
  DEFSYM (QChooks, ":hooks");
  DEFSYM (QClisp_thread, ":lisp-thread");
  DEFSYM (QCspan, ":span");
  DEFSYM (QCstride, ":stride");
  DEFSYM (QCthinned, ":thinned");
  DEFSYM (QCdeep, ":deep");
  DEFSYM (QCseq, ":seq");
  DEFSYM (QCsamples, ":samples");
  DEFSYM (QCstale, ":stale");
  DEFSYM (QCc_function, ":c-function");
  DEFSYM (QCtext_vectorlike, ":text-vectorlike");

  defsubr (&Snidhogg_watchdog__deep);
  defsubr (&Snidhogg_watchdog__test_sample);
  defsubr (&Snidhogg_watchdog__test_raw_samples);
  defsubr (&Snidhogg_watchdog__test_forge_late_request);
  defsubr (&Snidhogg_watchdog__test_plant_words);
  defsubr (&Snidhogg_watchdog__test_word);
  defsubr (&Snidhogg_watchdog__test_freed_word);
  defsubr (&Snidhogg_watchdog__test_force_lisp_thread);
  defsubr (&Snidhogg_watchdog__test_set_unstable);
  defsubr (&Snidhogg_watchdog__test_handler_flags);
  defsubr (&Snidhogg_watchdog__test_accumulator);
}

#endif /* HAVE_NIDHOGG_WATCHDOG */

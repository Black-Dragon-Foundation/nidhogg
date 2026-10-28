/* nhglue.h -- the C ABI of Nidhogg's shell glue (crate nhglue, libnhglue.a).

   The glue holds the native shell's state and makes the decisions that need no live Emacs state; the C hooks in
   src/nhwl*.c touch Emacs and call in here.  Calls go one way: C calls nhg_*, the glue answers with plain records and
   never calls back into Emacs (nidhogg/glue/nm-check.sh checks the archive for it).  Call everything on the Lisp
   thread that holds Emacs's global lock.  Every call returns NHG_OK (or a count, where said) or a negative NHG_E_*
   code; a panic inside the glue is caught at the boundary and reported as NHG_E_PANIC.  libnhglue.a also carries every
   nhshell_* and glitnir_* symbol: it is the one Rust library Emacs links.

   Pixels: none of the calls below reads or writes a backing's pixels.  */

#ifndef NHGLUE_H
#define NHGLUE_H

#include <stddef.h>
#include <stdint.h>

#include "nhshell.h"

#ifdef __cplusplus
extern "C" {
#endif

#define NHG_ABI_VERSION      1

/* The codes equal NHSHELL_E_* where they mean the same.  */
#define NHG_OK               0
#define NHG_E_PANIC         -1   /* a panic inside the glue */
#define NHG_E_ARG           -2
#define NHG_E_STATE         -3
#define NHG_E_UNSUPPORTED   -4
#define NHG_E_DISCONNECTED  -5
#define NHG_E_NOMEM         -6
#define NHG_E_TIMEOUT       -7
#define NHG_E_TOO_SMALL     -8
#define NHG_E_THREAD        -9   /* called from a thread the backend refuses */

/* One display: a shell connection and the glue's state for it.  */
typedef struct NhgDisplay NhgDisplay;

/* Write the ABI version (NHG_ABI_VERSION) to *OUT.  NHG_E_ARG if OUT is NULL.  */
int32_t nhg_abi_version (uint32_t *out);

/* Connect to DISPLAY (NULL: the environment's) as nhshell_open does.  NULL on failure, with a NUL-terminated reason
   in ERR (CAP bytes; ERR may be NULL when CAP is 0).  */
NhgDisplay *nhg_open (const char *display, char *err, size_t cap);

/* Close the connection and free D.  NULL is accepted.  D is not used again.  */
int32_t nhg_close (NhgDisplay *d);

/* The descriptor that is readable when events are queued (>= 0), or a negative code.  */
int32_t nhg_fd (NhgDisplay *d);

/* The shell handle inside D, for the nhshell_* calls not yet ported; valid until nhg_close.  NULL if D is NULL.  */
NhShell *nhg_shell (NhgDisplay *d);

#ifdef __cplusplus
}
#endif

#endif /* NHGLUE_H */

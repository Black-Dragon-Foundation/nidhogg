/* Sjá, a native view model: its C ABI (nidhogg/sja; README.md).
   One entry point.  C passes an op and its arguments and builds the Lisp value from the answer's tokens; Sjá never
   sees a Lisp object and never calls Emacs.  Call only on the thread that holds Emacs's global lock.  */

#ifndef SJA_H
#define SJA_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

#define SJA_ABI_VERSION 1

/* Codes.  */
#define SJA_OK 0
#define SJA_E_PANIC (-1)
#define SJA_E_ARG (-2)
#define SJA_E_NOMEM (-3)
#define SJA_E_STATE (-4)

/* Argument and token tags.  */
#define SJA_NIL 0
#define SJA_INT 1
#define SJA_STR 2
#define SJA_LIST 3   /* INT elements follow */
#define SJA_DOTTED 4 /* INT elements follow, the last one the tail */
#define SJA_VECTOR 5 /* INT elements follow */

/* Ops (the arguments each takes: README.md, "The C door").  */
#define SJA_APPLY 1
#define SJA_OPEN 2
#define SJA_CLOSE 3
#define SJA_FOLLOW 4
#define SJA_WALK 5
#define SJA_WALK_TO 6
#define SJA_SET 7
#define SJA_GET 8
#define SJA_HUB_LOST 9
#define SJA_STATS 10
#define SJA_TEST_PANIC 99

typedef struct
{
  int32_t tag;
  int64_t int_;
  const unsigned char *ptr;
  size_t len;
} sja_arg;

typedef sja_arg sja_tok;

/* One call: on SJA_OK, *NOUT tokens are at *OUT (prefix order), valid until the next call on this thread.  */
int32_t sja_call (int32_t op, const sja_arg *args, size_t n, const sja_tok **out, size_t *nout);

#ifdef __cplusplus
}
#endif

#endif /* SJA_H */

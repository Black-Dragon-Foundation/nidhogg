/* glitnir.h -- the C ABI of the Glitnir compositor library.

   Every function returns a status code.  Ids and numbers cross the boundary, never pointers to the library's
   memory.  A panic inside the library is caught at the boundary and reported as GLITNIR_E_PANIC.  */

#ifndef GLITNIR_H
#define GLITNIR_H

#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

#define GLITNIR_OK        0
#define GLITNIR_E_PANIC  -1
#define GLITNIR_E_ARG    -2
#define GLITNIR_E_STATE  -3

/* Write the ABI version (1) to *OUT.  GLITNIR_E_ARG if OUT is NULL.  */
int32_t glitnir_abi_version (uint32_t *out);

/* Panic inside the guard; always returns GLITNIR_E_PANIC.  For tests.  */
int32_t glitnir_debug_panic (void);

#ifdef __cplusplus
}
#endif

#endif /* GLITNIR_H */

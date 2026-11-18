/* Hlidskjalf, a buffer text's line model: its C ABI (nidhogg/hlidskjalf; README.md).
   Models are keyed by the host (Nidhogg: the buffer text's address).  Positions are 1-based characters over the
   whole text.  Call only from the thread that holds Emacs's global lock.  */

#ifndef HLIDSKJALF_H
#define HLIDSKJALF_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

#define HLID_OK 0
#define HLID_E_PANIC (-1)
#define HLID_E_ARG (-2)
#define HLID_E_NONE (-3)
#define HLID_BUILDING 1
#define HLID_STALE 2
#define HLID_NOTHING 3

typedef struct
{
  uint64_t pos, end;		/* END 0: no run */
  uint8_t lane, class_, prio, kind;
} hlid_mark;

typedef struct
{
  uint64_t beg, end;
  uint8_t kind, depth, source;
} hlid_fold;

typedef struct
{
  uint64_t beg, end;
  uint8_t class_;
} hlid_span;

typedef struct
{
  uint16_t col;			/* characters from the line's start */
  uint8_t len, class_;		/* LEN 0: no run */
} hlid_run;

#define HLID_RUNS 6

typedef struct
{
  uint64_t lines, chars, chunks, marks, folds, hidden, heap_bytes, edits;
  int32_t state;		/* 0 ready, 1 building, 2 stale */
} hlid_stats;

int32_t hlid_build (uint64_t key, uint32_t multibyte, uint32_t tab_width,
		    const unsigned char *p1, size_t n1,
		    const unsigned char *p2, size_t n2, uint32_t background);
int32_t hlid_drop (uint64_t key);
int32_t hlid_wait (uint64_t key);
int32_t hlid_edit (uint64_t key, uint64_t start, uint64_t old_end,
		   uint64_t new_end, const unsigned char *p1, size_t n1,
		   const unsigned char *p2, size_t n2);
int32_t hlid_line_of (uint64_t key, uint64_t pos, uint64_t *out);
int32_t hlid_pos_of (uint64_t key, uint64_t line, uint64_t *out);
int32_t hlid_fraction (uint64_t key, uint64_t pos, uint64_t begv,
		       uint64_t zv, double *out);
int32_t hlid_set_marks (uint64_t key, uint32_t kind, const hlid_mark *marks,
			size_t n);
int32_t hlid_next_mark (uint64_t key, uint64_t pos, uint64_t kinds,
			int32_t dir, uint64_t *out);
int32_t hlid_counts (uint64_t key, uint64_t beg, uint64_t end, uint32_t *out);
int32_t hlid_set_folds (uint64_t key, uint32_t source, const hlid_fold *folds,
			size_t n, uint64_t max);
int32_t hlid_replace_folds (uint64_t key, uint32_t source, uint64_t beg,
			    uint64_t end, const hlid_fold *folds, size_t n);
int32_t hlid_replace_classes (uint64_t key, uint64_t beg, uint64_t end,
			      const hlid_span *spans, size_t n);
int32_t hlid_classes_missed (uint64_t key);
int32_t hlid_line_runs (uint64_t key, uint64_t line, hlid_run *out, size_t *n);
int32_t hlid_folds (uint64_t key, uint64_t beg, uint64_t end, hlid_fold *out,
		    size_t cap, size_t *n);
int32_t hlid_fold_at (uint64_t key, uint64_t pos, hlid_fold *out);
int32_t hlid_set_hidden (uint64_t key, const uint64_t *ranges, size_t n);
/* As hlid_set_hidden, with a name for each range: LENS[I] bytes of UTF-8 at
   NAMES[I] (null: none); NAMES and LENS may both be null.  */
int32_t hlid_set_hidden_named (uint64_t key, const uint64_t *ranges,
			       const uint8_t *const *names,
			       const size_t *lens, size_t n);
int32_t hlid_trail_note (uint64_t key, uint64_t pos);
int32_t hlid_trail_clear (uint64_t key);
int32_t hlid_trail (uint64_t key, uint64_t *out, size_t cap, size_t *n);
int32_t hlid_describe (uint64_t key, hlid_stats *out);
/* MARK sections: the expression (RE null: none), then the Ith section whose
   line starts in BEG..END (its name valid until the model next changes).  */
int32_t hlid_set_sections (uint64_t key, const uint8_t *re, size_t n);
int32_t hlid_section (uint64_t key, uint64_t beg, uint64_t end, size_t i,
		      uint64_t *pos, const uint8_t **name, size_t *len);
/* The fold ranges the model finds itself: by indentation (INDENT non-zero)
   and by headings (RE, its first group the marks; null: none).  */
int32_t hlid_set_derived_folds (uint64_t key, int32_t indent,
				const uint8_t *re, size_t n);

#ifdef __cplusplus
}
#endif

#endif /* HLIDSKJALF_H */

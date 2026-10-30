/* crates/llvm_poc/runtime/ubel_rt.h
 *
 * Stage 0 runtime for the Ubel LLVM proof of concept.
 *
 * Three allocation strategies, one per Ubel memory tier:
 *   HIGH -> ubel_gc_*     non-moving, stop-the-world mark-sweep GC
 *   MID  -> ubel_arena_*  bump-pointer arenas (fast path is inlined in the IR)
 *   LOW  -> malloc/free   called directly from the IR, no wrapper at all
 *
 * This is measurement scaffolding, not the final runtime. See README.md for
 * exactly what it does and does not model.
 */
#ifndef UBEL_RT_H
#define UBEL_RT_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

uint64_t ubel_now_ns(void);

/* ----------------------------------------------- support for emitted programs --- */

/* What `println` / `print` lower to (Stage 1B). Output goes to stdout in the same
 * format the interpreter uses: integers in decimal, bools as true / false. */
void ubel_print_i64(int64_t v);
void ubel_print_bool(int32_t b);
void ubel_print_str(const char *s, int64_t len);
void ubel_print_nl(void);

/* Runtime errors the interpreter would report; here they print to stderr and exit(1). */
void ubel_trap_div_zero(void);
void ubel_trap_no_return(void);

/* Number of arenas created and not yet destroyed. A correct program ends with 0. */
uint64_t ubel_arena_live(void);

/* ---------------------------------------------------------------- HIGH --- */

/* Pointer-width note: the collector works in pointer-sized words (4 bytes on
 * 32-bit targets, 8 on 64-bit). `ptrmask` bit i refers to pointer-word i, and
 * the mask is 64 bits wide, so the largest object a single mask can describe is
 * 64 pointer-words: 512 bytes on 64-bit, 256 bytes on 32-bit. A bigger object that
 * holds pointers is allocated with ubel_gc_alloc_array, which repeats a per-element
 * mask; a bigger object without pointers (a string) needs no mask at all.
 *
 * Objects up to UBEL_GC_MAX_CLASS bytes live in size-class spans. Larger ones take a run
 * of whole 32 KiB spans each (a "large" object); the only limit is the heap size. */
#define UBEL_GC_MAX_SMALL (64u * (unsigned)sizeof(void *))
#define UBEL_GC_MAX_CLASS (32u * 1024u)

typedef struct {
    uint64_t collections;
    uint64_t alloc_count;
    uint64_t alloc_bytes;       /* rounded up to the size class */
    uint64_t mark_ns;           /* root scan + mark, summed over collections */
    uint64_t sweep_ns;
    uint64_t max_pause_ns;      /* stop-the-world: whole collection */
    uint64_t peak_heap_bytes;   /* spans handed out x span size */
    uint64_t live_bytes_last;   /* live bytes found by the last collection */
    uint64_t large_allocs;      /* objects above UBEL_GC_MAX_CLASS (counted in alloc_count too) */
    uint64_t large_bytes;       /* their size rounded up to whole spans (in alloc_bytes too) */
} ubel_gc_stats;

/* Call once, early, from the outermost frame that can hold GC pointers.
 * `stack_base` is the high end of the stack region to scan, typically
 * __builtin_frame_address(0) taken in main(). */
void  ubel_gc_init(void *stack_base);

/* Allocate `size` bytes, zeroed. Bit i of `ptrmask` says pointer-word i of the object
 * (offset i * sizeof(void*)) holds a pointer. A compiler derives the mask from the target's
 * data layout. With a non-zero mask `size` must be <= UBEL_GC_MAX_SMALL (the call aborts
 * otherwise); with mask 0 it can be anything the heap can hold. Interior pointers keep an
 * object alive; for a large object a pointer one past its end does not. */
void *ubel_gc_alloc(uint64_t size, uint64_t ptrmask);

/* An object with no pointers inside, of any size (strings, byte buffers). Large ones are
 * marked but never scanned. Same as ubel_gc_alloc(size, 0). */
void *ubel_gc_alloc_noscan(uint64_t size);

/* `count` elements of `elem_size` bytes, zeroed, each with the pointer layout `elem_ptrmask`
 * (bit i = pointer-word i of one element). With a non-zero mask `elem_size` must be a whole
 * number of words, at most 64 of them, and the mask must stay inside the element. The total
 * size is unlimited. A mask of 0 makes it pointer-free. */
void *ubel_gc_alloc_array(uint64_t elem_size, uint64_t count, uint64_t elem_ptrmask);

void  ubel_gc_collect(void);
void  ubel_gc_get_stats(ubel_gc_stats *out);
void  ubel_gc_reset_stats(void);

/* Test knob: lower (or restore) the minimum allocation volume between
 * collections. The UBEL_GC_TRIGGER_KB environment variable sets it at startup. */
void  ubel_gc_set_min_trigger_kb(uint64_t kb);

/* ----------------------------------------------------------------- MID --- */

/* The first four fields are shared with %Arena in the bench .ll files. Keep them in
 * sync; the IR never touches the fields after chunk_size. The IR inlines the
 * bump fast path (cur/end) and only calls ubel_arena_grow when the current
 * chunk is full. Request sizes must be multiples of 16. Arena memory is not
 * zeroed.
 *
 * A plain arena is never scanned by the GC, so it must not hold the only
 * reference to a GC object (the "hidden root" hazard). A scannable arena
 * (ubel_arena_create_scan) is treated as a conservative root set by every
 * collection, for arenas whose contents may point into the GC heap. */
typedef struct ubel_arena {
    char    *cur;
    char    *end;
    void    *chunks;
    uint64_t chunk_size;
    struct ubel_arena *scan_next; /* registry of scannable arenas (runtime only) */
    struct ubel_arena *scan_prev;
    uint32_t scannable;
} ubel_arena;

ubel_arena *ubel_arena_create(uint64_t chunk_size); /* 0 = default (64 KiB) */
ubel_arena *ubel_arena_create_scan(uint64_t chunk_size);
void       *ubel_arena_grow(ubel_arena *a, uint64_t size);
void        ubel_arena_destroy(ubel_arena *a);

#ifdef __cplusplus
}
#endif

#endif /* UBEL_RT_H */

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

/* ---------------------------------------------------------------- HIGH --- */

/* Pointer-width note: the collector works in pointer-sized words (4 bytes on
 * 32-bit targets, 8 on 64-bit). `ptrmask` bit i refers to pointer-word i, and
 * the mask is 64 bits wide, so the largest object is 64 pointer-words:
 * 512 bytes on 64-bit, 256 bytes on 32-bit. Bigger requests abort. */
#define UBEL_GC_MAX_SMALL (64u * (unsigned)sizeof(void *))

typedef struct {
    uint64_t collections;
    uint64_t alloc_count;
    uint64_t alloc_bytes;       /* rounded up to the size class */
    uint64_t mark_ns;           /* root scan + mark, summed over collections */
    uint64_t sweep_ns;
    uint64_t max_pause_ns;      /* stop-the-world: whole collection */
    uint64_t peak_heap_bytes;   /* spans handed out x span size */
    uint64_t live_bytes_last;   /* live bytes found by the last collection */
} ubel_gc_stats;

/* Call once, early, from the outermost frame that can hold GC pointers.
 * `stack_base` is the high end of the stack region to scan, typically
 * __builtin_frame_address(0) taken in main(). */
void  ubel_gc_init(void *stack_base);

/* Allocate `size` bytes (<= UBEL_GC_MAX_SMALL), zeroed. Bit i of `ptrmask`
 * says pointer-word i of the object (offset i * sizeof(void*)) holds a
 * pointer. A compiler derives the mask from the target's data layout.
 * Interior pointers keep an object alive. */
void *ubel_gc_alloc(uint64_t size, uint64_t ptrmask);

void  ubel_gc_collect(void);
void  ubel_gc_get_stats(ubel_gc_stats *out);
void  ubel_gc_reset_stats(void);

/* ----------------------------------------------------------------- MID --- */

/* Layout is shared with %Arena in bench/tiers.ll. Keep both in sync.
 * The IR inlines the bump fast path (cur/end) and only calls
 * ubel_arena_grow when the current chunk is full. Request sizes must be
 * multiples of 16. Arena memory is NOT zeroed and is never scanned by the GC. */
typedef struct ubel_arena {
    char    *cur;
    char    *end;
    void    *chunks;
    uint64_t chunk_size;
} ubel_arena;

ubel_arena *ubel_arena_create(uint64_t chunk_size); /* 0 = default (64 KiB) */
void       *ubel_arena_grow(ubel_arena *a, uint64_t size);
void        ubel_arena_destroy(ubel_arena *a);

#ifdef __cplusplus
}
#endif

#endif /* UBEL_RT_H */

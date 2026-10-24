/* crates/llvm_poc/runtime/ubel_rt.c
 *
 * Stage 0 runtime: see ubel_rt.h and README.md.
 *
 * GC design (deliberately Go-flavoured, deliberately simplified):
 *   - one reserved virtual heap, cut into 32 KiB spans, each span holding
 *     objects of a single size class (like Go's mspan);
 *   - non-moving, so pointers never change and no read barrier is needed;
 *   - tri-colour mark-sweep: white = unmarked, grey = marked and on the mark
 *     stack, black = marked and scanned;
 *   - heap objects are scanned precisely (a 1-bit-per-word pointer bitmap is
 *     written at allocation time), the stack is scanned CONSERVATIVELY;
 *   - stop-the-world and eager sweep: no write barrier, no concurrency;
 *   - GOGC=100-style pacing: collect once as many bytes have been allocated
 *     since the last collection as were live after it (with a floor).
 */
#include "ubel_rt.h"

#include <setjmp.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <time.h>

#ifndef MAP_ANONYMOUS
#define MAP_ANONYMOUS MAP_ANON
#endif
#ifndef MAP_NORESERVE
#define MAP_NORESERVE 0
#endif

uint64_t ubel_now_ns(void) {
    struct timespec ts;
    clock_gettime(CLOCK_MONOTONIC, &ts);
    return (uint64_t)ts.tv_sec * 1000000000ull + (uint64_t)ts.tv_nsec;
}

/* ------------------------------------------------------ emitted-program support */

void ubel_print_i64(int64_t v)            { fprintf(stdout, "%lld", (long long)v); }
void ubel_print_bool(int32_t b)           { fputs(b ? "true" : "false", stdout); }
void ubel_print_str(const char *s, int64_t len) { fwrite(s, 1, (size_t)len, stdout); }
void ubel_print_nl(void)                  { fputc('\n', stdout); }

void ubel_trap_div_zero(void) {
    fflush(stdout);
    fprintf(stderr, "runtime error: division by zero\n");
    exit(1);
}

void ubel_trap_no_return(void) {
    fflush(stdout);
    fprintf(stderr, "runtime error: a function with a return type reached its end without returning\n");
    exit(1);
}

/* ===================================================================== GC */

_Static_assert(sizeof(void *) == 4 || sizeof(void *) == 8, "Stage 0 supports 32- and 64-bit targets");

/* The pointer bitmap and ptrmask work in pointer-sized words. */
#define WORD_SHIFT   (sizeof(void *) == 8 ? 3u : 2u)
#define SPAN_SHIFT   15u
#define SPAN_SIZE    (1u << SPAN_SHIFT) /* 32 KiB */
#define NUM_CLASSES  10u
#define MIN_TRIGGER  (4u * 1024u * 1024u)

static const uint16_t class_size[NUM_CLASSES] = {16, 32, 48, 64, 96, 128, 192, 256, 384, 512};
static uint8_t size_to_class[UBEL_GC_MAX_SMALL / 16 + 1];

typedef struct span {
    uint8_t  state;      /* 0 = free, 1 = in use */
    uint8_t  cls;
    uint8_t  needzero;   /* memory may hold old data */
    uint16_t elem;       /* object size in bytes */
    uint16_t nelems;
    uint16_t freeindex;  /* allocation scan resumes here */
    struct span *next;   /* link in the per-class partial list */
    uint64_t alloc_bits[32]; /* 1 = slot holds an allocated object */
    uint64_t mark_bits[32];  /* 1 = reached in the current cycle */
} span;

static char     *heap_base;
static size_t    heap_bytes;
static size_t    nspans_total;
static size_t    next_fresh_span;   /* spans below this index have been used at least once */
static size_t    spans_in_use;
static span     *spans;
static uint64_t *heapbits;          /* 1 bit per pointer-sized heap word: "this word is a pointer" */
static uint32_t *free_spans;
static size_t    n_free_spans;
static span     *cur_span[NUM_CLASSES];
static span     *partial[NUM_CLASSES];
static void     *stack_base;

static uint64_t last_live;
static uint64_t bytes_since_gc;
static uint64_t trigger_delta = MIN_TRIGGER;
static uint64_t min_trigger   = MIN_TRIGGER;
static ubel_gc_stats st;

static uintptr_t *mstack;
static size_t     mtop, mcap;

static void *meta_alloc(size_t bytes) {
    void *p = mmap(NULL, bytes, PROT_READ | PROT_WRITE,
                   MAP_PRIVATE | MAP_ANONYMOUS | MAP_NORESERVE, -1, 0);
    if (p == MAP_FAILED) {
        fprintf(stderr, "ubel_rt: cannot map %zu bytes of GC metadata\n", bytes);
        abort();
    }
    return p; /* zero-filled */
}

void ubel_gc_init(void *stack_base_) {
    stack_base = stack_base_;

    size_t mb = 256;
    const char *env = getenv("UBEL_HEAP_MB");
    if (env) {
        long v = strtol(env, NULL, 10);
        if (v >= 16 && v <= 65536) mb = (size_t)v;
    }
    for (;;) {
        size_t bytes = mb << 20;
        void *p = mmap(NULL, bytes, PROT_READ | PROT_WRITE,
                       MAP_PRIVATE | MAP_ANONYMOUS | MAP_NORESERVE, -1, 0);
        if (p != MAP_FAILED) { heap_base = (char *)p; heap_bytes = bytes; break; }
        if (mb <= 16) { fprintf(stderr, "ubel_rt: cannot reserve a GC heap\n"); abort(); }
        mb >>= 1;
    }
    nspans_total = heap_bytes >> SPAN_SHIFT;
    spans      = (span *)meta_alloc(nspans_total * sizeof(span));
    heapbits   = (uint64_t *)meta_alloc((heap_bytes >> WORD_SHIFT) / 8 + 64);
    free_spans = (uint32_t *)meta_alloc(nspans_total * sizeof(uint32_t));

    for (unsigned i = 0; i <= UBEL_GC_MAX_SMALL / 16; i++) {
        unsigned need = i ? i * 16u : 16u;
        unsigned c = 0;
        while (class_size[c] < need) c++;
        size_to_class[i] = (uint8_t)c;
    }

    const char *t = getenv("UBEL_GC_TRIGGER_KB"); /* test knob: force frequent collections */
    if (t) {
        long v = strtol(t, NULL, 10);
        if (v >= 1) min_trigger = (uint64_t)v * 1024u;
    }
    trigger_delta = min_trigger;
}

/* ---- pointer bitmap helpers (bit i of word `widx..` = "word is a pointer") */

static inline void hb_write(size_t widx, unsigned nwords, uint64_t mask) {
    uint64_t *w = &heapbits[widx >> 6];
    unsigned off = (unsigned)(widx & 63);
    uint64_t range = (nwords == 64) ? ~0ull : ((1ull << nwords) - 1ull);
    mask &= range;
    w[0] = (w[0] & ~(range << off)) | (mask << off);
    if (off + nwords > 64) { /* straddles into the next bitmap word */
        unsigned lo = 64 - off;
        w[1] = (w[1] & ~(range >> lo)) | (mask >> lo);
    }
}

static inline uint64_t hb_read(size_t widx, unsigned nwords) {
    uint64_t *w = &heapbits[widx >> 6];
    unsigned off = (unsigned)(widx & 63);
    uint64_t v = w[0] >> off;
    if (off + nwords > 64) v |= w[1] << (64 - off);
    return (nwords == 64) ? v : (v & ((1ull << nwords) - 1ull));
}

/* ---- allocation ------------------------------------------------------- */

static span *new_span(unsigned cls) {
    size_t idx;
    int fresh;
    if (n_free_spans) {
        idx = free_spans[--n_free_spans];
        fresh = 0;
    } else if (next_fresh_span < nspans_total) {
        idx = next_fresh_span++;
        fresh = 1;
    } else {
        fprintf(stderr, "ubel_rt: GC heap exhausted (raise UBEL_HEAP_MB)\n");
        abort();
    }
    span *s = &spans[idx];
    memset(s->alloc_bits, 0, sizeof s->alloc_bits);
    memset(s->mark_bits, 0, sizeof s->mark_bits);
    s->state = 1;
    s->cls = (uint8_t)cls;
    s->elem = class_size[cls];
    s->nelems = (uint16_t)(SPAN_SIZE / s->elem);
    s->freeindex = 0;
    s->needzero = fresh ? 0 : 1;
    s->next = NULL;
    spans_in_use++;
    if ((uint64_t)spans_in_use * SPAN_SIZE > st.peak_heap_bytes)
        st.peak_heap_bytes = (uint64_t)spans_in_use * SPAN_SIZE;
    return s;
}

static inline int span_next_free(span *s) {
    unsigned i = s->freeindex;
    while (i < s->nelems) {
        uint64_t free_bits = (~s->alloc_bits[i >> 6]) >> (i & 63);
        if (free_bits) {
            unsigned idx = i + (unsigned)__builtin_ctzll(free_bits);
            if (idx >= s->nelems) break;
            s->alloc_bits[idx >> 6] |= 1ull << (idx & 63);
            s->freeindex = (uint16_t)(idx + 1);
            return (int)idx;
        }
        i = (i | 63u) + 1u;
    }
    s->freeindex = s->nelems;
    return -1;
}

void *ubel_gc_alloc(uint64_t size, uint64_t ptrmask) {
    if (size > UBEL_GC_MAX_SMALL) {
        fprintf(stderr, "ubel_rt: Stage 0 GC handles objects up to %u bytes (asked for %llu)\n",
                UBEL_GC_MAX_SMALL, (unsigned long long)size);
        abort();
    }
    unsigned cls = size_to_class[(size + 15) >> 4];
    unsigned elem = class_size[cls];

    if (bytes_since_gc >= trigger_delta) ubel_gc_collect();
    bytes_since_gc += elem;
    st.alloc_count++;
    st.alloc_bytes += elem;

    span *s = cur_span[cls];
    for (;;) {
        if (s) {
            int idx = span_next_free(s);
            if (idx >= 0) {
                char *p = heap_base + ((size_t)(s - spans) << SPAN_SHIFT) + (size_t)idx * elem;
                if (s->needzero) memset(p, 0, elem);
                hb_write((size_t)(p - heap_base) >> WORD_SHIFT, elem >> WORD_SHIFT, ptrmask);
                return p;
            }
        }
        s = partial[cls];
        if (s) { partial[cls] = s->next; s->next = NULL; }
        else   { s = new_span(cls); }
        cur_span[cls] = s;
    }
}

/* ---- marking ---------------------------------------------------------- */

static inline void mpush(uintptr_t a) {
    if (mtop == mcap) {
        mcap = mcap ? mcap * 2 : 4096;
        mstack = (uintptr_t *)realloc(mstack, mcap * sizeof(uintptr_t));
        if (!mstack) { fprintf(stderr, "ubel_rt: out of memory for the mark stack\n"); abort(); }
    }
    mstack[mtop++] = a;
}

/* If `p` points into (or at) an allocated heap object, grey it. */
static inline void mark_ptr(uintptr_t p) {
    uintptr_t off = p - (uintptr_t)heap_base; /* wraps for p < heap_base */
    if (off >= ((uintptr_t)next_fresh_span << SPAN_SHIFT)) return;
    span *s = &spans[off >> SPAN_SHIFT];
    if (!s->state) return;
    unsigned idx = (unsigned)((off & (SPAN_SIZE - 1u)) / s->elem);
    if (idx >= s->nelems) return;
    uint64_t bit = 1ull << (idx & 63);
    if (!(s->alloc_bits[idx >> 6] & bit)) return; /* free slot */
    if (s->mark_bits[idx >> 6] & bit) return;     /* already grey or black */
    s->mark_bits[idx >> 6] |= bit;
    mpush((uintptr_t)heap_base + ((uintptr_t)(s - spans) << SPAN_SHIFT) + (uintptr_t)idx * s->elem);
}

static void scan_object(uintptr_t addr) { /* grey -> black */
    size_t off = addr - (uintptr_t)heap_base;
    span *s = &spans[off >> SPAN_SHIFT];
    uint64_t m = hb_read(off >> WORD_SHIFT, s->elem >> WORD_SHIFT);
    const uintptr_t *words = (const uintptr_t *)addr;
    while (m) {
        unsigned i = (unsigned)__builtin_ctzll(m);
        m &= m - 1;
        mark_ptr(words[i]);
    }
}

/* The conservative scan reads every word of the stack, including words an address sanitizer
 * has poisoned as redzones or as the shadow of dead locals. That is what a stack scan does,
 * not a bug, so the two functions that read the stack are exempt from instrumentation. */
#if defined(__clang__) || defined(__GNUC__)
#define UBEL_NO_ASAN __attribute__((no_sanitize("address")))
#else
#define UBEL_NO_ASAN
#endif

static void __attribute__((noinline)) UBEL_NO_ASAN scan_stack_region(void) {
    volatile uintptr_t here = 0;
    uintptr_t lo = (uintptr_t)&here & ~(uintptr_t)(sizeof(uintptr_t) - 1);
    const uintptr_t *p  = (const uintptr_t *)lo;
    const uintptr_t *hi = (const uintptr_t *)stack_base;
    for (; p < hi; ++p) mark_ptr(*p);
}

static void __attribute__((noinline)) UBEL_NO_ASAN scan_roots(void) {
    jmp_buf jb;
    setjmp(jb); /* spills callee-saved registers into this frame */
#if defined(__GNUC__) || defined(__clang__)
    __builtin_unwind_init(); /* also covers registers glibc's setjmp mangles (e.g. rbp) */
#endif
    scan_stack_region();
    __asm__ volatile("" : : "r"(&jb) : "memory"); /* keep jb alive and in this frame */
}

/* Scannable arenas (defined with the arena code below) act as extra roots. */
static void scan_arena_roots(void);

/* ---- sweeping --------------------------------------------------------- */

static uint64_t sweep(void) {
    uint64_t live = 0;
    for (unsigned c = 0; c < NUM_CLASSES; c++) { partial[c] = NULL; cur_span[c] = NULL; }
    n_free_spans = 0;
    for (size_t i = 0; i < next_fresh_span; i++) {
        span *s = &spans[i];
        if (!s->state) { free_spans[n_free_spans++] = (uint32_t)i; continue; }
        unsigned nw = ((unsigned)s->nelems + 63u) >> 6;
        unsigned n = 0;
        for (unsigned w = 0; w < nw; w++) {
            s->alloc_bits[w] = s->mark_bits[w]; /* survivors stay allocated */
            n += (unsigned)__builtin_popcountll(s->mark_bits[w]);
            s->mark_bits[w] = 0;
        }
        if (n == 0) {
            s->state = 0;
            spans_in_use--;
            free_spans[n_free_spans++] = (uint32_t)i;
        } else {
            live += (uint64_t)n * s->elem;
            s->freeindex = 0;
            s->needzero = 1;
            s->next = NULL;
            if (n < s->nelems) { s->next = partial[s->cls]; partial[s->cls] = s; }
        }
    }
    return live;
}

void ubel_gc_collect(void) {
    if (!heap_base) return;
    uint64_t t0 = ubel_now_ns();
    scan_roots();
    scan_arena_roots();
    while (mtop) scan_object(mstack[--mtop]);
    uint64_t t1 = ubel_now_ns();
    uint64_t live = sweep();
    uint64_t t2 = ubel_now_ns();

    st.collections++;
    st.mark_ns  += t1 - t0;
    st.sweep_ns += t2 - t1;
    if (t2 - t0 > st.max_pause_ns) st.max_pause_ns = t2 - t0;
    st.live_bytes_last = live;
    last_live = live;
    bytes_since_gc = 0;
    trigger_delta = live > min_trigger ? live : min_trigger; /* GOGC=100 */
}

void ubel_gc_set_min_trigger_kb(uint64_t kb) {
    min_trigger = (kb ? kb : 4096) * 1024u;
    trigger_delta = last_live > min_trigger ? last_live : min_trigger;
}

void ubel_gc_get_stats(ubel_gc_stats *out) { *out = st; }

void ubel_gc_reset_stats(void) {
    memset(&st, 0, sizeof st);
    st.peak_heap_bytes = (uint64_t)spans_in_use * SPAN_SIZE;
}

/* ================================================================== ARENA */

typedef struct arena_chunk {
    struct arena_chunk *next;
    uint64_t size; /* payload bytes */
} arena_chunk;     /* 16 bytes, so the payload stays 16-byte aligned */

#define ARENA_DEFAULT_CHUNK (64u * 1024u)
#define ARENA_CACHE_MAX     64u

static arena_chunk *chunk_cache;
static unsigned     chunk_cache_n;

static void arena_install_chunk(ubel_arena *a, uint64_t payload) {
    arena_chunk *c;
    if (payload == ARENA_DEFAULT_CHUNK && chunk_cache) {
        c = chunk_cache;
        chunk_cache = c->next;
        chunk_cache_n--;
    } else {
        c = (arena_chunk *)malloc(sizeof(arena_chunk) + payload);
        if (!c) { fprintf(stderr, "ubel_rt: out of memory for an arena chunk\n"); abort(); }
        c->size = payload;
    }
    c->next = (arena_chunk *)a->chunks;
    a->chunks = c;
    a->cur = (char *)(c + 1);
    a->end = a->cur + payload;
}

static ubel_arena *scan_head;
static uint64_t arenas_live;

uint64_t ubel_arena_live(void) { return arenas_live; }

ubel_arena *ubel_arena_create(uint64_t chunk_size) {
    ubel_arena *a = (ubel_arena *)malloc(sizeof *a);
    if (!a) { fprintf(stderr, "ubel_rt: out of memory for an arena\n"); abort(); }
    a->chunk_size = chunk_size ? chunk_size : ARENA_DEFAULT_CHUNK;
    a->chunks = NULL;
    a->cur = a->end = NULL;
    a->scan_next = a->scan_prev = NULL;
    a->scannable = 0;
    arena_install_chunk(a, a->chunk_size);
    arenas_live++;
    return a;
}

ubel_arena *ubel_arena_create_scan(uint64_t chunk_size) {
    ubel_arena *a = ubel_arena_create(chunk_size);
    a->scannable = 1;
    a->scan_next = scan_head;
    if (scan_head) scan_head->scan_prev = a;
    scan_head = a;
    return a;
}

/* Conservatively scan every live scannable arena: the whole payload of each
 * retired chunk, and only the bumped part of the current (first) chunk. */
static void scan_arena_roots(void) {
    for (ubel_arena *a = scan_head; a; a = a->scan_next) {
        for (arena_chunk *c = (arena_chunk *)a->chunks; c; c = c->next) {
            const char *lo = (const char *)(c + 1);
            const char *hi = (c == (arena_chunk *)a->chunks) ? a->cur : lo + c->size;
            for (const uintptr_t *p = (const uintptr_t *)lo; (const char *)(p + 1) <= hi; ++p)
                mark_ptr(*p);
        }
    }
}

void *ubel_arena_grow(ubel_arena *a, uint64_t size) {
    uint64_t payload = a->chunk_size;
    if (size > payload) payload = (size + 15u) & ~(uint64_t)15u;
    arena_install_chunk(a, payload);
    char *p = a->cur;
    a->cur += size;
    return p;
}

void ubel_arena_destroy(ubel_arena *a) {
    arenas_live--;
    if (a->scannable) {
        if (a->scan_prev) a->scan_prev->scan_next = a->scan_next; else scan_head = a->scan_next;
        if (a->scan_next) a->scan_next->scan_prev = a->scan_prev;
    }
    arena_chunk *c = (arena_chunk *)a->chunks;
    while (c) {
        arena_chunk *n = c->next;
        if (c->size == ARENA_DEFAULT_CHUNK && chunk_cache_n < ARENA_CACHE_MAX) {
            c->next = chunk_cache;
            chunk_cache = c;
            chunk_cache_n++;
        } else {
            free(c);
        }
        c = n;
    }
    free(a);
}

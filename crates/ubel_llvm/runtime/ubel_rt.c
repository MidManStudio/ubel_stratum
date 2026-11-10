/* crates/ubel_llvm/runtime/ubel_rt.c
 *
 * Stage 0 runtime: see ubel_rt.h and README.md.
 *
 * GC design (deliberately Go-flavoured, deliberately simplified):
 *   - one reserved virtual heap, cut into 32 KiB spans, each span holding
 *     objects of a single size class (like Go's mspan);
 *   - objects above 32 KiB are "large": each takes a run of whole spans to itself, still
 *     inside the same reserved heap, so one range check and one pointer bitmap cover both;
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

#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <time.h>

#include "ubel_unicode.h"

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

/* Strings (D3b, docs/ARENA_STRINGS.md). A string value is a pointer to { int64 length, bytes }:
 * UTF-8, no terminator, immutable. The emitter builds them (interpolation) with the helpers below. */

int32_t ubel_str_eq(const void *a, const void *b) {
    if (a == b) return 1;
    int64_t la = *(const int64_t *)a;
    if (la != *(const int64_t *)b) return 0;
    return memcmp((const char *)a + 8, (const char *)b + 8, (size_t)la) == 0;
}

/* Byte-wise lexicographic order (UTF-8 bytes, the interpreter's String comparison): -1, 0 or 1. */
int32_t ubel_str_cmp(const void *a, const void *b) {
    if (a == b) return 0;
    int64_t la = *(const int64_t *)a, lb = *(const int64_t *)b;
    int64_t n = la < lb ? la : lb;
    int c = n > 0 ? memcmp((const char *)a + 8, (const char *)b + 8, (size_t)n) : 0;
    if (c != 0) return c < 0 ? -1 : 1;
    return la < lb ? -1 : (la > lb ? 1 : 0);
}

/* String methods that read their operands and allocate nothing (docs/LLVM_POC.md, "Strings"). They work on
 * bytes, like the interpreter (Rust's str::len / contains / starts_with / ends_with): a valid UTF-8 needle
 * can only match at a character boundary, so a byte search gives the same answers. An empty needle is found
 * everywhere (contains, starts_with and ends_with all answer 1); a needle longer than the string nowhere.
 * `contains` is a plain scan, O(n*m) in the worst case where Rust's search is linear. */

int32_t ubel_str_contains(const void *s, const void *t) {
    int64_t n = *(const int64_t *)s, m = *(const int64_t *)t;
    if (m == 0) return 1;
    if (m > n) return 0;
    const unsigned char *h = (const unsigned char *)s + 8, *p = (const unsigned char *)t + 8;
    for (int64_t i = 0; i + m <= n; i++) {
        if (h[i] == p[0] && memcmp(h + i, p, (size_t)m) == 0) return 1;
    }
    return 0;
}

int32_t ubel_str_starts_with(const void *s, const void *t) {
    int64_t n = *(const int64_t *)s, m = *(const int64_t *)t;
    if (m > n) return 0;
    return memcmp((const char *)s + 8, (const char *)t + 8, (size_t)m) == 0;
}

int32_t ubel_str_ends_with(const void *s, const void *t) {
    int64_t n = *(const int64_t *)s, m = *(const int64_t *)t;
    if (m > n) return 0;
    return memcmp((const char *)s + 8 + (n - m), (const char *)t + 8, (size_t)m) == 0;
}

/* ---- String methods that build a new string (docs/LLVM_POC.md, "String methods that build a string").
 *
 * to_upper, to_lower, trim, trim_start, trim_end and replace are the interpreter's: Rust's str::to_uppercase,
 * to_lowercase, trim, trim_start, trim_end and replace. Every result is a new string: in the arena the last
 * parameter points to (the arena of an arena receiver), or a GC string (mask 0) when that is NULL. The
 * arguments are only read, so the collector running inside the allocation cannot free them (they are reachable
 * from the caller's frame, which the conservative scan covers). The case and whitespace tables are generated
 * from Rust's standard library (scripts/gen_unicode.rs, ubel_unicode.h). A string is valid UTF-8, so decoding
 * never fails; a stray byte is passed through unchanged rather than trusted. */

#define BAD_BYTE 0x200000u /* a byte that is not part of a valid sequence travels as BAD_BYTE | byte */

static uint32_t u8_next(const unsigned char *p, const unsigned char *end, int *len) {
    unsigned c = p[0];
    if (c < 0x80) {
        *len = 1;
        return c;
    }
    if (c >= 0xC2 && c <= 0xDF && p + 1 < end && (p[1] & 0xC0) == 0x80) {
        *len = 2;
        return ((c & 0x1Fu) << 6) | (p[1] & 0x3Fu);
    }
    if (c >= 0xE0 && c <= 0xEF && p + 2 < end && (p[1] & 0xC0) == 0x80 && (p[2] & 0xC0) == 0x80) {
        *len = 3;
        return ((c & 0x0Fu) << 12) | ((p[1] & 0x3Fu) << 6) | (p[2] & 0x3Fu);
    }
    if (c >= 0xF0 && c <= 0xF4 && p + 3 < end && (p[1] & 0xC0) == 0x80 && (p[2] & 0xC0) == 0x80 && (p[3] & 0xC0) == 0x80) {
        *len = 4;
        return ((c & 0x07u) << 18) | ((p[1] & 0x3Fu) << 12) | ((p[2] & 0x3Fu) << 6) | (p[3] & 0x3Fu);
    }
    *len = 1;
    return BAD_BYTE | c;
}

static int u8_size(uint32_t cp) {
    return cp >= BAD_BYTE || cp < 0x80 ? 1 : cp < 0x800 ? 2 : cp < 0x10000 ? 3 : 4;
}

static int u8_put(unsigned char *o, uint32_t cp) {
    if (cp >= BAD_BYTE) {
        o[0] = (unsigned char)cp;
        return 1;
    }
    if (cp < 0x80) {
        o[0] = (unsigned char)cp;
        return 1;
    }
    if (cp < 0x800) {
        o[0] = (unsigned char)(0xC0 | (cp >> 6));
        o[1] = (unsigned char)(0x80 | (cp & 0x3F));
        return 2;
    }
    if (cp < 0x10000) {
        o[0] = (unsigned char)(0xE0 | (cp >> 12));
        o[1] = (unsigned char)(0x80 | ((cp >> 6) & 0x3F));
        o[2] = (unsigned char)(0x80 | (cp & 0x3F));
        return 3;
    }
    o[0] = (unsigned char)(0xF0 | (cp >> 18));
    o[1] = (unsigned char)(0x80 | ((cp >> 12) & 0x3F));
    o[2] = (unsigned char)(0x80 | ((cp >> 6) & 0x3F));
    o[3] = (unsigned char)(0x80 | (cp & 0x3F));
    return 4;
}

static int in_ranges(const uint32_t (*r)[2], size_t n, uint32_t c) {
    size_t lo = 0, hi = n;
    while (lo < hi) {
        size_t mid = lo + (hi - lo) / 2;
        if (c < r[mid][0]) hi = mid;
        else if (c > r[mid][1]) lo = mid + 1;
        else return 1;
    }
    return 0;
}

/* The mapping of `c` (1 to 3 code points) into out[]; returns how many. */
static int case_map(const uint32_t *cps, const uint32_t *vals, size_t n, uint32_t c, uint32_t out[3]) {
    size_t lo = 0, hi = n;
    while (lo < hi) {
        size_t mid = lo + (hi - lo) / 2;
        if (cps[mid] < c) lo = mid + 1;
        else hi = mid;
    }
    if (lo < n && cps[lo] == c) {
        uint32_t v = vals[lo];
        if (v & 0x80000000u) {
            int len = (int)((v >> 24) & 3u);
            uint32_t off = v & 0xFFFFFFu;
            for (int i = 0; i < len; i++) out[i] = ubel_case_pool[off + (uint32_t)i];
            return len;
        }
        out[0] = v;
        return 1;
    }
    out[0] = c;
    return 1;
}

/* Rust's final-sigma context: skip Case_Ignorable characters, then ask whether the next one is cased. */
static int ignorable_then_cased_before(const unsigned char *s, const unsigned char *p) {
    while (p > s) {
        const unsigned char *q = p - 1;
        while (q > s && (*q & 0xC0) == 0x80) q--;
        int len;
        uint32_t cp = u8_next(q, p, &len);
        if (len != (int)(p - q)) return 0; /* not a whole character: not cased */
        if (in_ranges(ubel_ign, UBEL_IGN_N, cp)) {
            p = q;
            continue;
        }
        return in_ranges(ubel_cased, UBEL_CASED_N, cp);
    }
    return 0;
}

static int ignorable_then_cased_after(const unsigned char *p, const unsigned char *end) {
    while (p < end) {
        int len;
        uint32_t cp = u8_next(p, end, &len);
        if (in_ranges(ubel_ign, UBEL_IGN_N, cp)) {
            p += len;
            continue;
        }
        return in_ranges(ubel_cased, UBEL_CASED_N, cp);
    }
    return 0;
}

/* The arena fast path of the IR (ubel_arena_bump), for a size that is a multiple of 16. */
static void *arena_bump(ubel_arena *a, uint64_t size) {
    char *p = a->cur;
    if ((uint64_t)(a->end - p) >= size) {
        a->cur = p + size;
        return p;
    }
    return ubel_arena_grow(a, size);
}

/* A new string object of `len` bytes: in `arena` when it is not NULL (the receiver's own arena, so the result
 * lives exactly as long as sema says it may), else a GC object (mask 0: a string holds no pointers). */
static void *new_string(void *arena, int64_t len) {
    unsigned char *p = arena ? (unsigned char *)arena_bump((ubel_arena *)arena, ((uint64_t)len + 8 + 15u) & ~(uint64_t)15u)
                             : (unsigned char *)ubel_gc_alloc_noscan((uint64_t)len + 8);
    memcpy(p, &len, 8);
    return p;
}

/* One pass over the string; with out == NULL it only counts. Returns the length of the result. */
static int64_t case_pass(const unsigned char *s, const unsigned char *end, int upper, unsigned char *out) {
    int64_t total = 0;
    const unsigned char *p = s;
    while (p < end) {
        int len;
        uint32_t cp = u8_next(p, end, &len);
        uint32_t m[3];
        int k;
        if (cp < 0x80) {
            m[0] = upper ? (cp >= 'a' && cp <= 'z' ? cp - 32 : cp) : (cp >= 'A' && cp <= 'Z' ? cp + 32 : cp);
            k = 1;
        } else if (!upper && cp == 0x3A3) {
            /* capital sigma: final sigma at the end of a word, else sigma (Rust's only contextual mapping) */
            int final = ignorable_then_cased_before(s, p) && !ignorable_then_cased_after(p + len, end);
            m[0] = final ? 0x3C2 : 0x3C3;
            k = 1;
        } else if (upper) {
            k = case_map(ubel_upper_cp, ubel_upper_val, UBEL_UPPER_N, cp, m);
        } else {
            k = case_map(ubel_lower_cp, ubel_lower_val, UBEL_LOWER_N, cp, m);
        }
        for (int i = 0; i < k; i++) {
            if (out) total += u8_put(out + total, m[i]);
            else total += u8_size(m[i]);
        }
        p += len;
    }
    return total;
}

static void *str_case(const void *sv, int upper, void *arena) {
    int64_t n = *(const int64_t *)sv;
    const unsigned char *s = (const unsigned char *)sv + 8;
    int64_t size = case_pass(s, s + n, upper, NULL);
    unsigned char *r = (unsigned char *)new_string(arena, size);
    case_pass(s, s + n, upper, r + 8);
    return r;
}

void *ubel_str_to_upper(const void *s, void *arena) { return str_case(s, 1, arena); }
void *ubel_str_to_lower(const void *s, void *arena) { return str_case(s, 0, arena); }

static void *str_copy_range(const unsigned char *from, const unsigned char *to, void *arena) {
    unsigned char *r = (unsigned char *)new_string(arena, (int64_t)(to - from));
    memcpy(r + 8, from, (size_t)(to - from));
    return r;
}

static const unsigned char *trim_front(const unsigned char *s, const unsigned char *end) {
    while (s < end) {
        int len;
        uint32_t cp = u8_next(s, end, &len);
        if (!in_ranges(ubel_ws, UBEL_WS_N, cp)) break;
        s += len;
    }
    return s;
}

static const unsigned char *trim_back(const unsigned char *s, const unsigned char *end) {
    while (end > s) {
        const unsigned char *q = end - 1;
        while (q > s && (*q & 0xC0) == 0x80) q--;
        int len;
        uint32_t cp = u8_next(q, end, &len);
        if (len != (int)(end - q) || !in_ranges(ubel_ws, UBEL_WS_N, cp)) break;
        end = q;
    }
    return end;
}

void *ubel_str_trim(const void *sv, void *arena) {
    int64_t n = *(const int64_t *)sv;
    const unsigned char *s = (const unsigned char *)sv + 8, *end = s + n;
    const unsigned char *a = trim_front(s, end);
    return str_copy_range(a, trim_back(a, end), arena);
}

void *ubel_str_trim_start(const void *sv, void *arena) {
    int64_t n = *(const int64_t *)sv;
    const unsigned char *s = (const unsigned char *)sv + 8, *end = s + n;
    return str_copy_range(trim_front(s, end), end, arena);
}

void *ubel_str_trim_end(const void *sv, void *arena) {
    int64_t n = *(const int64_t *)sv;
    const unsigned char *s = (const unsigned char *)sv + 8, *end = s + n;
    return str_copy_range(s, trim_back(s, end), arena);
}

/* Rust's str::replace: every non-overlapping match, scanning left to right. An empty needle matches at every
 * character boundary, including the start and the end ("abc".replace("", "-") is "-a-b-c-"). */
void *ubel_str_replace(const void *sv, const void *fv, const void *tv, void *arena) {
    int64_t n = *(const int64_t *)sv, m = *(const int64_t *)fv, k = *(const int64_t *)tv;
    const unsigned char *h = (const unsigned char *)sv + 8, *f = (const unsigned char *)fv + 8, *t = (const unsigned char *)tv + 8;
    int64_t matches = 0;
    if (m == 0) {
        for (const unsigned char *p = h; p < h + n;) {
            int len;
            u8_next(p, h + n, &len);
            matches++;
            p += len;
        }
        matches++; /* and one more at the end */
    } else {
        for (int64_t i = 0; i + m <= n;) {
            if (h[i] == f[0] && memcmp(h + i, f, (size_t)m) == 0) {
                matches++;
                i += m;
            } else {
                i++;
            }
        }
    }
    unsigned char *r = (unsigned char *)new_string(arena, n + matches * (k - m));
    unsigned char *o = r + 8;
    if (m == 0) {
        for (const unsigned char *p = h;;) {
            memcpy(o, t, (size_t)k);
            o += k;
            if (p >= h + n) break;
            int len;
            u8_next(p, h + n, &len);
            memcpy(o, p, (size_t)len);
            o += len;
            p += len;
        }
    } else {
        for (int64_t i = 0; i < n;) {
            if (i + m <= n && h[i] == f[0] && memcmp(h + i, f, (size_t)m) == 0) {
                memcpy(o, t, (size_t)k);
                o += k;
                i += m;
            } else {
                *o++ = h[i++];
            }
        }
    }
    return r;
}

/* Number of characters `v` takes in decimal, minus sign included. */
int64_t ubel_i64_len(int64_t v) {
    uint64_t u = v < 0 ? 0 - (uint64_t)v : (uint64_t)v;
    int64_t n = v < 0 ? 2 : 1;
    while (u >= 10) { u /= 10; n++; }
    return n;
}

/* Writes exactly ubel_i64_len(v) characters at dst, no terminator. */
void ubel_i64_write(char *dst, int64_t v) {
    uint64_t u = v < 0 ? 0 - (uint64_t)v : (uint64_t)v;
    int64_t first = v < 0 ? 1 : 0;
    if (v < 0) dst[0] = '-';
    for (int64_t i = ubel_i64_len(v) - 1; i >= first; i--) { dst[i] = (char)('0' + (int)(u % 10)); u /= 10; }
}

void ubel_put(char *dst, const char *src, int64_t n) { memcpy(dst, src, (size_t)n); }

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
#define NUM_CLASSES  27u
#define MIN_TRIGGER  (4u * 1024u * 1024u)

/* Up to 512 bytes the classes are the original ones (the benchmarks depend on them). Above
 * that they grow by about a quarter per step, which keeps both the rounding waste and the
 * unused tail of a 32 KiB span small, up to a single 32 KiB object per span. */
static const uint16_t class_size[NUM_CLASSES] = {
    16, 32, 48, 64, 96, 128, 192, 256, 384, 512,
    640, 768, 896, 1024, 1280, 1536, 1792, 2048, 2560, 3072, 4096, 5120, 6144, 8192, 10240,
    16384, 32768};
static uint8_t size_to_class[UBEL_GC_MAX_CLASS / 16 + 1];

enum { SPAN_FREE = 0, SPAN_SMALL = 1, SPAN_LARGE = 2, SPAN_CONT = 3 };

typedef struct span {
    uint8_t  state;      /* SPAN_FREE, SPAN_SMALL (slots of one class), SPAN_LARGE (first span
                            of a large object's run) or SPAN_CONT (any later span of that run) */
    uint8_t  cls;
    uint8_t  needzero;   /* memory may hold old data */
    uint8_t  noscan;     /* large head: no pointers inside, so mark it but never scan it */
    uint16_t elem;       /* object size in bytes */
    uint16_t nelems;
    uint16_t freeindex;  /* allocation scan resumes here */
    uint32_t run_len;    /* large head: number of spans in the run */
    uint32_t head_idx;   /* large continuation: index of the head span */
    uint64_t obj_bytes;  /* large head: the object's size in bytes */
    struct span *next;   /* link in the per-class partial list */
    uint64_t alloc_bits[32]; /* 1 = slot holds an allocated object */
    uint64_t mark_bits[32];  /* 1 = reached in the current cycle (a large head uses bit 0) */
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
/* Debug mode (UBEL_GC_STRESS=N, ubel_gc_set_stress): collect before every N-th allocation. It is
 * implemented by holding trigger_delta at 0, so the existing check in front of every allocation
 * fires each time and the normal fast path pays nothing for it. */
static uint64_t stress_every, stress_skip;
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

    for (unsigned i = 0; i <= UBEL_GC_MAX_CLASS / 16; i++) {
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
    const char *sx = getenv("UBEL_GC_STRESS"); /* debug mode: collect before every N-th allocation */
    if (sx) {
        long v = strtol(sx, NULL, 10);
        if (v >= 1) stress_every = (uint64_t)v;
    }
    trigger_delta = stress_every ? 0 : min_trigger;
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

/* Clear the pointer bits of `nwords` words starting at word `widx`, any length. */
static void hb_clear_range(size_t widx, size_t nwords) {
    size_t end = widx + nwords;
    while (widx < end) {
        unsigned off = (unsigned)(widx & 63);
        size_t take = 64u - off;
        if (take > end - widx) take = end - widx;
        uint64_t m = (take == 64) ? ~0ull : (((1ull << take) - 1ull) << off);
        heapbits[widx >> 6] &= ~m;
        widx += take;
    }
}

/* ---- allocation ------------------------------------------------------- */

static span *new_span(unsigned cls) {
    size_t idx = 0;
    int fresh = 0, have = 0;
    while (n_free_spans) { /* an entry can be stale: a large run may have claimed that span */
        size_t c = free_spans[--n_free_spans];
        if (spans[c].state == SPAN_FREE) { idx = c; have = 1; break; }
    }
    if (!have) {
        if (next_fresh_span < nspans_total) {
            idx = next_fresh_span++;
            fresh = 1;
        } else {
            fprintf(stderr, "ubel_rt: GC heap exhausted (raise UBEL_HEAP_MB)\n");
            abort();
        }
    }
    span *s = &spans[idx];
    memset(s->alloc_bits, 0, sizeof s->alloc_bits);
    memset(s->mark_bits, 0, sizeof s->mark_bits);
    s->state = SPAN_SMALL;
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

/* ---- large objects -------------------------------------------------------
 * An object above UBEL_GC_MAX_CLASS bytes takes a run of whole spans: the first is marked
 * SPAN_LARGE and keeps the size, the mark bit and the no-scan flag, the rest are SPAN_CONT
 * and point back at it. A run comes from the lowest stretch of free spans that is long
 * enough, else from fresh heap. The pointer bitmap needs nothing special: it covers the whole
 * reserved heap, one bit per word. Interior pointers keep the object alive; a pointer one
 * past the end does not. Free runs are not returned to the OS and not coalesced beyond what
 * the sweep already does: they become free spans again. */

static size_t find_free_run(size_t n) {
    if (n_free_spans < n) return (size_t)-1; /* cheap exit: the stack holds every free span */
    size_t run = 0;
    for (size_t i = 0; i < next_fresh_span; i++) {
        if (spans[i].state == SPAN_FREE) {
            if (++run == n) return i + 1 - n;
        } else {
            run = 0;
        }
    }
    return (size_t)-1;
}

/* Reached when the allocation volume since the last collection hit trigger_delta. Normally that
 * means collect. In stress mode it is reached on every allocation, and collects on every N-th.
 * No locals on purpose: this frame is inside the range the stack scan reads. */
static void __attribute__((noinline)) gc_trigger(void) {
    if (stress_every > 1 && ++stress_skip < stress_every) return;
    stress_skip = 0;
    ubel_gc_collect();
}

static void *gc_alloc_large(uint64_t size, int noscan, size_t *nwords) {
    if (size > (uint64_t)heap_bytes) {
        fprintf(stderr, "ubel_rt: a %llu-byte object does not fit in the GC heap (raise UBEL_HEAP_MB)\n",
                (unsigned long long)size);
        abort();
    }
    size_t n = (size_t)((size + SPAN_SIZE - 1u) >> SPAN_SHIFT);
    uint64_t rounded = (uint64_t)n << SPAN_SHIFT;

    if (bytes_since_gc >= trigger_delta) gc_trigger();
    bytes_since_gc += rounded;
    st.alloc_count++;
    st.alloc_bytes += rounded;
    st.large_allocs++;
    st.large_bytes += rounded;

    size_t idx = find_free_run(n);
    int fresh = 0;
    if (idx == (size_t)-1) {
        if (next_fresh_span + n > nspans_total) {
            fprintf(stderr, "ubel_rt: GC heap exhausted: no run of %zu free spans for a %llu-byte object "
                            "(raise UBEL_HEAP_MB)\n", n, (unsigned long long)size);
            abort();
        }
        idx = next_fresh_span;
        next_fresh_span += n;
        fresh = 1;
    }

    span *h = &spans[idx];
    h->state = SPAN_LARGE;
    h->noscan = (uint8_t)(noscan != 0);
    h->needzero = 0;
    h->run_len = (uint32_t)n;
    h->obj_bytes = size;
    h->mark_bits[0] = 0;
    h->next = NULL;
    for (size_t k = 1; k < n; k++) {
        spans[idx + k].state = SPAN_CONT;
        spans[idx + k].head_idx = (uint32_t)idx;
    }
    spans_in_use += n;
    if ((uint64_t)spans_in_use * SPAN_SIZE > st.peak_heap_bytes)
        st.peak_heap_bytes = (uint64_t)spans_in_use * SPAN_SIZE;

    char *p = heap_base + (idx << SPAN_SHIFT);
    if (!fresh) memset(p, 0, (size_t)size); /* fresh heap is already zero */
    *nwords = (size_t)(size >> WORD_SHIFT);
    return p;
}

/* A zeroed slot of class `cls`, counted and paced like any allocation. The one place that knows
 * how to find a slot; the fast path below and the general path both use it. */
static inline __attribute__((always_inline)) void *gc_slot(unsigned cls, unsigned elem) {
    if (bytes_since_gc >= trigger_delta) gc_trigger();
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
                return p;
            }
        }
        s = partial[cls];
        if (s) { partial[cls] = s->next; s->next = NULL; }
        else   { s = new_span(cls); }
        cur_span[cls] = s;
    }
}

/* Reserve a zeroed object of `size` bytes of any size, and report how many words of the pointer
 * bitmap belong to it (the slot for a class, the object itself for a large one). The caller
 * writes those bits. Out of line: the common case, an object a 64-bit mask describes, never
 * comes through here. */
static __attribute__((noinline)) void *gc_obj(uint64_t size, int noscan, size_t *nwords) {
    if (size > UBEL_GC_MAX_CLASS) return gc_alloc_large(size, noscan, nwords);
    unsigned cls = size_to_class[(size + 15) >> 4];
    unsigned elem = class_size[cls];
    void *p = gc_slot(cls, elem);
    *nwords = elem >> WORD_SHIFT;
    return p;
}

/* An object above UBEL_GC_MAX_SMALL can hold no pointers when allocated this way (a mask cannot
 * describe it): a size class above 512 bytes or a large object, filled in by the caller. */
static __attribute__((noinline)) void *gc_alloc_big(uint64_t size, uint64_t ptrmask) {
    if (ptrmask) {
        fprintf(stderr, "ubel_rt: a 64-bit pointer mask describes objects up to %u bytes (asked for %llu "
                        "with pointers); use ubel_gc_alloc_array\n",
                UBEL_GC_MAX_SMALL, (unsigned long long)size);
        abort();
    }
    size_t nw;
    void *p = gc_obj(size, 1, &nw);
    hb_clear_range((size_t)((char *)p - heap_base) >> WORD_SHIFT, nw); /* more than 64 words here */
    return p;
}

void *ubel_gc_alloc(uint64_t size, uint64_t ptrmask) {
    if (__builtin_expect(size > UBEL_GC_MAX_SMALL, 0)) return gc_alloc_big(size, ptrmask);
    unsigned cls = size_to_class[(size + 15) >> 4];
    unsigned elem = class_size[cls];
    void *p = gc_slot(cls, elem);
    hb_write((size_t)((char *)p - heap_base) >> WORD_SHIFT, elem >> WORD_SHIFT, ptrmask);
    return p;
}

void *ubel_gc_alloc_noscan(uint64_t size) { return ubel_gc_alloc(size, 0); }

void *ubel_gc_alloc_array(uint64_t elem_size, uint64_t count, uint64_t elem_ptrmask) {
    uint64_t total;
    if (__builtin_mul_overflow(elem_size, count, &total)) {
        fprintf(stderr, "ubel_rt: array of %llu elements of %llu bytes overflows\n",
                (unsigned long long)count, (unsigned long long)elem_size);
        abort();
    }
    unsigned ew = 0;
    if (elem_ptrmask) {
        if (elem_size == 0 || (elem_size & (sizeof(void *) - 1u)) || (elem_size >> WORD_SHIFT) > 64 ||
            ((elem_size >> WORD_SHIFT) < 64 && (elem_ptrmask >> (elem_size >> WORD_SHIFT)) != 0)) {
            fprintf(stderr, "ubel_rt: a pointer element must be a whole number of words (at most 64) "
                            "and its mask must stay inside it (size %llu, mask %llx)\n",
                    (unsigned long long)elem_size, (unsigned long long)elem_ptrmask);
            abort();
        }
        ew = (unsigned)(elem_size >> WORD_SHIFT);
    }
    size_t nw;
    void *p = gc_obj(total, elem_ptrmask == 0, &nw);
    size_t widx = (size_t)((char *)p - heap_base) >> WORD_SHIFT;
    if (!elem_ptrmask) {
        hb_clear_range(widx, nw);
    } else {
        for (uint64_t i = 0; i < count; i++) hb_write(widx + (size_t)i * ew, ew, elem_ptrmask);
        hb_clear_range(widx + (size_t)count * ew, nw - (size_t)count * ew); /* the slot's slack */
    }
    return p;
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

/* A word that points into a large object's run (anywhere inside its bytes) greys the object. */
static void __attribute__((noinline)) mark_large(size_t si, uintptr_t off) {
    span *s = &spans[si];
    if (s->state == SPAN_FREE) return;
    size_t hi = (s->state == SPAN_CONT) ? s->head_idx : si;
    span *h = &spans[hi];
    uintptr_t hoff = (uintptr_t)hi << SPAN_SHIFT;
    if (off - hoff >= h->obj_bytes) return; /* in the rounding slack after the object */
    if (h->mark_bits[0] & 1) return;        /* already grey or black */
    h->mark_bits[0] |= 1;
    if (!h->noscan) mpush((uintptr_t)heap_base + hoff);
}

/* If `p` points into (or at) an allocated heap object, grey it. */
static inline void mark_ptr(uintptr_t p) {
    uintptr_t off = p - (uintptr_t)heap_base; /* wraps for p < heap_base */
    if (off >= ((uintptr_t)next_fresh_span << SPAN_SHIFT)) return;
    span *s = &spans[off >> SPAN_SHIFT];
    if (__builtin_expect(s->state != SPAN_SMALL, 0)) { mark_large((size_t)(off >> SPAN_SHIFT), off); return; }
    unsigned idx = (unsigned)((off & (SPAN_SIZE - 1u)) / s->elem);
    if (idx >= s->nelems) return;
    uint64_t bit = 1ull << (idx & 63);
    if (!(s->alloc_bits[idx >> 6] & bit)) return; /* free slot */
    if (s->mark_bits[idx >> 6] & bit) return;     /* already grey or black */
    s->mark_bits[idx >> 6] |= bit;
    mpush((uintptr_t)heap_base + ((uintptr_t)(s - spans) << SPAN_SHIFT) + (uintptr_t)idx * s->elem);
}

/* Grey every pointer word of an object longer than the single 64-bit read can cover. */
static void scan_words(const uintptr_t *words, size_t widx, size_t nwords) {
    size_t end = widx + nwords, w = widx;
    while (w < end) {
        unsigned off = (unsigned)(w & 63);
        size_t take = 64u - off;
        if (take > end - w) take = end - w;
        uint64_t m = heapbits[w >> 6] >> off;
        if (take < 64) m &= (1ull << take) - 1ull;
        while (m) {
            unsigned i = (unsigned)__builtin_ctzll(m);
            m &= m - 1;
            mark_ptr(words[(w - widx) + i]);
        }
        w += take;
    }
}

static void scan_object(uintptr_t addr) { /* grey -> black */
    size_t off = addr - (uintptr_t)heap_base;
    span *s = &spans[off >> SPAN_SHIFT];
    if (s->state == SPAN_LARGE) {
        scan_words((const uintptr_t *)addr, off >> WORD_SHIFT, (size_t)(s->obj_bytes >> WORD_SHIFT));
        return;
    }
    unsigned nw = s->elem >> WORD_SHIFT;
    if (nw > 64) { scan_words((const uintptr_t *)addr, off >> WORD_SHIFT, nw); return; }
    uint64_t m = hb_read(off >> WORD_SHIFT, nw);
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

/* The callee-saved registers can hold the only copy of a pointer. __builtin_unwind_init makes this
 * function save every one of them in its own prologue, so they sit in this frame, which the scan below
 * covers; the barrier after the call stops a tail call from popping the frame first.
 *
 * Do not use setjmp for this. glibc stores some of the registers (esp and eip, and ebp or rbp) XORed
 * with a random per-process pointer guard, and the rest of a jmp_buf is uninitialised (the signal-mask
 * area is 128 bytes). Scanning one reads a few random words and some stale ones. On a 64-bit target a
 * random word never lands in the heap, but on a 32-bit target it does with probability about
 * (heap in use) / 4 GiB per word, and because the guard is fixed for the process, the object it hits
 * stays retained for the whole run (found by the rt_gc_test flake at 32-bit, 2026-11-04). */
static void __attribute__((noinline)) UBEL_NO_ASAN scan_roots(void) {
    __builtin_unwind_init();
    scan_stack_region();
    __asm__ volatile("" : : : "memory");
}

/* Scannable arenas (defined with the arena code below) act as extra roots. */
static void scan_arena_roots(void);
static void scan_global_roots(void);

/* ---- sweeping --------------------------------------------------------- */

static uint64_t sweep(void) {
    uint64_t live = 0;
    for (unsigned c = 0; c < NUM_CLASSES; c++) { partial[c] = NULL; cur_span[c] = NULL; }
    n_free_spans = 0;
    for (size_t i = 0; i < next_fresh_span; i++) {
        span *s = &spans[i];
        if (s->state == SPAN_CONT) continue; /* settled with its head */
        if (s->state == SPAN_FREE) { free_spans[n_free_spans++] = (uint32_t)i; continue; }
        if (s->state == SPAN_LARGE) {
            size_t n = s->run_len;
            if (s->mark_bits[0] & 1) {
                s->mark_bits[0] = 0;
                live += (uint64_t)n << SPAN_SHIFT;
            } else {
                s->state = SPAN_FREE;
                spans_in_use -= n;
                free_spans[n_free_spans++] = (uint32_t)i;
                for (size_t k = 1; k < n; k++) spans[i + k].state = SPAN_FREE; /* listed when reached */
            }
            continue;
        }
        unsigned nw = ((unsigned)s->nelems + 63u) >> 6;
        unsigned n = 0;
        for (unsigned w = 0; w < nw; w++) {
            s->alloc_bits[w] = s->mark_bits[w]; /* survivors stay allocated */
            n += (unsigned)__builtin_popcountll(s->mark_bits[w]);
            s->mark_bits[w] = 0;
        }
        if (n == 0) {
            s->state = SPAN_FREE;
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

/* The collector's own frames are inside the range the stack scan reads, so they must not hold
 * words that look like addresses by accident. A clock reading is the worst case: on a 32-bit target
 * its low word falls inside any given multi-megabyte object for several milliseconds out of every
 * 4.3 seconds, and a local that keeps the last reading keeps that object alive across consecutive
 * collections. So the readings live in statics, never in a local (found at 32-bit, 2026-11-04). */
static uint64_t gc_t0, gc_t1, gc_t2;
static uint64_t (*gc_clock)(void); /* test knob: ubel_gc_set_clock */

/* The frames of scan_roots and scan_stack_region are laid over whatever the collector's earlier calls
 * (the clock, ubel_gc_collect's own callees) left below it, and a frame has padding and unwritten
 * slots that the scan reads. Zero that region first, so the scan reads zeros there and not the
 * collector's own leftovers (Boehm does the same, GC_clear_stack). 4 KiB is far more than those two
 * small frames need. It cannot touch the program's frames: they are above this one. */
#define GC_CLEAR_BYTES 4096u
static void __attribute__((noinline)) UBEL_NO_ASAN clear_scan_zone(void) {
    volatile unsigned char zone[GC_CLEAR_BYTES];
    for (unsigned i = 0; i < GC_CLEAR_BYTES; i++) zone[i] = 0;
    __asm__ volatile("" : : "r"(zone) : "memory");
}

void ubel_gc_set_clock(uint64_t (*now)(void)) { gc_clock = now; }

/* Takes a reading straight into a static. A separate function on purpose: in an unoptimised build
 * the result of `gc_clock ? gc_clock() : ubel_now_ns()` is spilled to a slot of whatever function
 * evaluates it, and ubel_gc_collect's own frame is always inside the scanned range. In here the
 * spill lives in a frame that is gone (and wiped by clear_scan_zone) before the scan starts. */
static void __attribute__((noinline)) stamp(uint64_t *dst) { *dst = gc_clock ? gc_clock() : ubel_now_ns(); }

void ubel_gc_collect(void) {
    if (!heap_base) return;
    stamp(&gc_t0);
    clear_scan_zone();
    scan_roots();
    scan_arena_roots();
    scan_global_roots();
    while (mtop) scan_object(mstack[--mtop]);
    stamp(&gc_t1);
    uint64_t live = sweep();
    stamp(&gc_t2);

    st.collections++;
    st.mark_ns  += gc_t1 - gc_t0;
    st.sweep_ns += gc_t2 - gc_t1;
    if (gc_t2 - gc_t0 > st.max_pause_ns) st.max_pause_ns = gc_t2 - gc_t0;
    st.live_bytes_last = live;
    last_live = live;
    bytes_since_gc = 0;
    trigger_delta = stress_every ? 0 : (live > min_trigger ? live : min_trigger); /* GOGC=100 */
}

void ubel_gc_set_min_trigger_kb(uint64_t kb) {
    min_trigger = (kb ? kb : 4096) * 1024u;
    trigger_delta = stress_every ? 0 : (last_live > min_trigger ? last_live : min_trigger);
}

void ubel_gc_set_stress(uint64_t every) {
    stress_every = every;
    stress_skip = 0;
    trigger_delta = every ? 0 : (last_live > min_trigger ? last_live : min_trigger);
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
    uint64_t used; /* payload bytes written, recorded when the arena moves on to the next chunk */
} __attribute__((aligned(16))) arena_chunk; /* a multiple of 16 bytes, so the payload stays 16-byte aligned */

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

/* Conservatively scan every live scannable arena: only the bytes that were bumped. For the current
 * (first) chunk that is everything below `cur`; for a retired chunk it is the `used` bytes recorded when
 * the arena moved on. Bytes nobody wrote (malloc memory, a recycled chunk) are never read, so a stale
 * word there cannot keep garbage alive. */
static void scan_arena_roots(void) {
    for (ubel_arena *a = scan_head; a; a = a->scan_next) {
        for (arena_chunk *c = (arena_chunk *)a->chunks; c; c = c->next) {
            const char *lo = (const char *)(c + 1);
            const char *hi = (c == (arena_chunk *)a->chunks) ? a->cur : lo + c->used;
            for (const uintptr_t *p = (const uintptr_t *)lo; (const char *)(p + 1) <= hi; ++p)
                mark_ptr(*p);
        }
    }
}

/* ---- global roots ----------------------------------------------------- */

/* Memory registered with ubel_gc_add_root is scanned word by word at every collection, like the
 * stack. The table is plain malloc memory, never GC heap, and is keyed by region start. */
typedef struct { uintptr_t lo, hi; } root_region;
static root_region *groots;
static size_t       ngroots, gcap;

static void __attribute__((noinline)) UBEL_NO_ASAN scan_global_roots(void) {
    for (size_t i = 0; i < ngroots; i++) {
        uintptr_t p = (groots[i].lo + (sizeof(uintptr_t) - 1)) & ~(uintptr_t)(sizeof(uintptr_t) - 1);
        for (; p + sizeof(uintptr_t) <= groots[i].hi; p += sizeof(uintptr_t)) mark_ptr(*(const uintptr_t *)p);
    }
}

void ubel_gc_add_root(void *addr, uint64_t bytes) {
    uintptr_t lo = (uintptr_t)addr, hi = lo + (uintptr_t)bytes;
    if (!addr || bytes < sizeof(uintptr_t) || bytes > UINTPTR_MAX || hi < lo) {
        fprintf(stderr, "ubel_rt: ubel_gc_add_root needs a non-null address and a size of at least one word\n");
        abort();
    }
    for (size_t i = 0; i < ngroots; i++)
        if (groots[i].lo == lo) { groots[i].hi = hi; return; } /* same start: change the length */
    if (ngroots == gcap) {
        gcap = gcap ? gcap * 2 : 16;
        groots = (root_region *)realloc(groots, gcap * sizeof(root_region));
        if (!groots) { fprintf(stderr, "ubel_rt: out of memory registering a global root\n"); abort(); }
    }
    groots[ngroots].lo = lo;
    groots[ngroots].hi = hi;
    ngroots++;
}

void ubel_gc_remove_root(void *addr) {
    for (size_t i = 0; i < ngroots; i++)
        if (groots[i].lo == (uintptr_t)addr) {
            groots[i] = groots[--ngroots];
            return;
        }
    fprintf(stderr, "ubel_rt: ubel_gc_remove_root: %p is not a registered root\n", addr);
    abort();
}

void *ubel_arena_grow(ubel_arena *a, uint64_t size) {
    if (a->chunks) {
        arena_chunk *old = (arena_chunk *)a->chunks;
        old->used = (uint64_t)(a->cur - (char *)(old + 1)); /* what was written in the chunk being retired */
    }
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

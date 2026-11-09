/* crates/llvm_poc/harness/rt_gc_test.c
 *
 * Tests for the collector's large objects, the extended size classes and the array
 * allocator (roadmap A5), and for the string helpers (str_helpers). Run one test per process so a failure or an abort in one cannot
 * hide another and each starts with an empty heap:
 *
 *     rt_gc_test <name>      or      rt_gc_test all   (runs every test in its own child)
 *
 * Built and run by scripts/run_equiv.sh together with ubel_rt.c, at -O0, -O2 and under
 * ASan + UBSan, on 64-bit and (with EXTRA_FLAGS=-m32) 32-bit.
 *
 * How the tests keep the collector honest:
 *   - The roots are a table inside a scannable arena, so what is alive is exactly what the
 *     table points at, whatever the optimizer does with registers and stack slots.
 *   - A raw pointer is never kept in a local across a collection (it would be a root). Helpers
 *     that touch objects are noinline and take a slot number; addresses worth remembering are
 *     stored complemented.
 *   - Expected sizes come from the runtime's own statistics (alloc_bytes before and after an
 *     allocation), not from a copy of its size classes, and a collection is preceded by a stack
 *     scrub so stale words below the test's frames cannot retain anything. Live-size checks
 *     retry once or twice for the same reason.
 *   - Every object is filled with a pattern derived from its id and checked before it is
 *     dropped, so a sweep that frees something reachable, or hands one block out twice,
 *     shows up as a pattern mismatch and not only as a number.
 */
#include "../runtime/ubel_rt.h"

#include <signal.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/wait.h>
#include <unistd.h>

static int failures;
#define CHECK(cond, ...)                                                      \
    do {                                                                      \
        if (!(cond)) {                                                        \
            failures++;                                                       \
            printf("    FAIL line %d: ", __LINE__);                           \
            printf(__VA_ARGS__);                                              \
            printf("\n");                                                     \
        }                                                                     \
    } while (0)

#define NOINLINE __attribute__((noinline))

/* ----------------------------------------------------------------- roots */

#define NROOTS 4096 /* even: arena requests are multiples of 16 */
static void **roots;

/* The arena's only chunk is exactly the table, and the table is all of it bumped, so it is all
 * scanned: it is zeroed, or stale malloc bytes in it could hold pointers and keep garbage alive. */
static void roots_init(void) {
    ubel_arena *a = ubel_arena_create_scan(NROOTS * sizeof(void *));
    roots = (void **)a->cur;
    a->cur += NROOTS * sizeof(void *);
    memset(roots, 0, NROOTS * sizeof(void *));
}

/* ------------------------------------------------------------ the pattern */

static void NOINLINE fill(void *p, size_t n, uint32_t id) {
    unsigned char *b = (unsigned char *)p;
    for (size_t i = 0; i < n; i++) b[i] = (unsigned char)(id * 131u + i * 7u + (i >> 9));
}

static int NOINLINE intact(const void *p, size_t n, uint32_t id) {
    const unsigned char *b = (const unsigned char *)p;
    for (size_t i = 0; i < n; i++)
        if (b[i] != (unsigned char)(id * 131u + i * 7u + (i >> 9))) return 0;
    return 1;
}

static int NOINLINE all_zero(const void *p, size_t n) {
    const unsigned char *b = (const unsigned char *)p;
    for (size_t i = 0; i < n; i++)
        if (b[i]) return 0;
    return 1;
}

/* --------------------------------------------------------------- helpers */

static uint64_t alloc_bytes_now(void) {
    ubel_gc_stats s;
    ubel_gc_get_stats(&s);
    return s.alloc_bytes;
}

static uint64_t NOINLINE collect_and_live(void) {
    ubel_gc_collect();
    ubel_gc_stats s;
    ubel_gc_get_stats(&s);
    return s.live_bytes_last;
}

/* Zero the stack below the caller before collecting. It has to be done inline in the TEST
 * function, not by a helper: the bytes just below a function's frame become the header of the
 * next callee's frame (return address, saved registers, sanitizer redzones) and a callee cannot
 * clear its own header, so a stale pointer left there by a dead call would be read as a root.
 * A variable-length array in an inner block zeroes that region with no call frame in the way,
 * and is released before the collecting call. */
static volatile size_t scrub_n = 96 * 1024; /* a runtime length, so the array below is a real VLA */
#define SCRUB()                                                                    \
    do {                                                                           \
        size_t n_ = scrub_n;                                                       \
        volatile unsigned char b_[n_]; /* released at the end of this block */     \
        memset((void *)b_, 0, n_);                                                 \
        __asm__ volatile("" : : "r"(b_) : "memory");                               \
    } while (0)
#define COLLECT()                                                                  \
    ({                                                                             \
        SCRUB();                                                                   \
        collect_and_live();                                                        \
    })

/* Live bytes after a collection must equal `expected`. A stale word on the stack could keep an
 * object alive, so retry a couple of times before calling it a failure.
 *
 * Built with -DRT_SOFT_LIVE (the sanitizer builds) the check is one-sided: `got >= expected`.
 * Instrumented frames carry redzones and spill slots that no portable code can zero, so a stale
 * word may keep one more object alive there, which a conservative collector is allowed to do.
 * Freeing something that is still referenced is a bug in every build, so that direction stays
 * checked; the sanitizers are what these builds are for. */
#ifdef RT_SOFT_LIVE
#define LIVE_OK(got, want) ((got) >= (want))
#else
#define LIVE_OK(got, want) ((got) == (want))
#endif
#define EXPECT_LIVE(expected, what)                                                   \
    do {                                                                              \
        uint64_t want_ = (expected), got_ = 0;                                        \
        for (int try_ = 0; try_ < 3; try_++) {                                        \
            got_ = COLLECT();                                                         \
            if (LIVE_OK(got_, want_)) break;                                          \
        }                                                                             \
        CHECK(LIVE_OK(got_, want_), "%s: live %llu, expected %llu", what,             \
              (unsigned long long)got_, (unsigned long long)want_);                   \
    } while (0)

/* Allocate a pointer-free object, fill it, put it in roots[slot]. Returns its rounded size. */
static uint64_t NOINLINE alloc_root(int slot, size_t size, uint32_t id) {
    uint64_t before = alloc_bytes_now();
    void *p = ubel_gc_alloc_noscan(size);
    uint64_t rounded = alloc_bytes_now() - before;
    CHECK(p != NULL, "allocation of %zu bytes returned NULL", size);
    CHECK(((uintptr_t)p & 15u) == 0, "%zu-byte object at %p is not 16-aligned", size, p);
    CHECK(rounded >= size, "%zu-byte object was given only %llu bytes", size, (unsigned long long)rounded);
    CHECK(all_zero(p, size), "%zu-byte object was not zeroed", size);
    fill(p, size, id);
    roots[slot] = p;
    return rounded;
}

static int NOINLINE root_intact(int slot, size_t size, uint32_t id) {
    return intact(roots[slot], size, id);
}

/* ----------------------------------------------------------------- tests */

/* The class table above 512 bytes: every boundary and its neighbours, several objects each,
 * all alive at once. A wrong class lookup shows up as overlapping objects (a pattern mismatch)
 * or as an object that is too small. */
static const unsigned class_bounds[] = {16, 32, 48, 64, 96, 128, 192, 256, 384, 512, 640, 768, 896,
                                        1024, 1280, 1536, 1792, 2048, 2560, 3072, 4096, 5120, 6144,
                                        8192, 10240, 16384, 32768};

static void t_classes(void) {
    enum { REPS = 3 };
    size_t sizes[3 * sizeof class_bounds / sizeof class_bounds[0]];
    int ns = 0;
    for (size_t i = 0; i < sizeof class_bounds / sizeof class_bounds[0]; i++) {
        sizes[ns++] = class_bounds[i] - 1;
        sizes[ns++] = class_bounds[i];
        sizes[ns++] = class_bounds[i] + 1;
    }
    uint64_t expect = 0;
    int slot = 0;
    for (int r = 0; r < REPS; r++)
        for (int i = 0; i < ns; i++, slot++) expect += alloc_root(slot, sizes[i], (uint32_t)slot + 1u);
    int bad = 0;
    for (int r = 0, k = 0; r < REPS; r++)
        for (int i = 0; i < ns; i++, k++) bad += !root_intact(k, sizes[i], (uint32_t)k + 1u);
    CHECK(bad == 0, "%d objects were damaged by their neighbours", bad);
    EXPECT_LIVE(expect, "all classes alive");
    bad = 0;
    for (int r = 0, k = 0; r < REPS; r++)
        for (int i = 0; i < ns; i++, k++) bad += !root_intact(k, sizes[i], (uint32_t)k + 1u);
    CHECK(bad == 0, "%d objects were damaged by a collection", bad);
    for (int k = 0; k < slot; k++) roots[k] = NULL;
    EXPECT_LIVE(0, "all classes dropped");
}

static const size_t large_sizes[] = {32769, 65536, 65537, 100000, (1u << 20) + 13, 5u << 20};

static void t_large_basic(void) {
    int n = (int)(sizeof large_sizes / sizeof large_sizes[0]);
    uint64_t rounded[16], total = 0;
    for (int i = 0; i < n; i++) {
        rounded[i] = alloc_root(i, large_sizes[i], 100u + (uint32_t)i);
        total += rounded[i];
        CHECK(rounded[i] % (32u * 1024u) == 0, "a large object is counted in whole spans (%llu)",
              (unsigned long long)rounded[i]);
    }
    EXPECT_LIVE(total, "all large objects alive");
    EXPECT_LIVE(total, "and again");
    for (int i = 0; i < n; i++) CHECK(root_intact(i, large_sizes[i], 100u + (uint32_t)i), "large object %d damaged", i);
    for (int i = 0; i < n; i++) {
        roots[i] = NULL;
        total -= rounded[i];
        EXPECT_LIVE(total, "after dropping one");
    }
    ubel_gc_stats s;
    ubel_gc_get_stats(&s);
    CHECK(s.large_allocs == (uint64_t)n, "large_allocs is %llu, expected %d", (unsigned long long)s.large_allocs, n);
}

/* Interior pointers keep a large object alive from any of its bytes. A pointer one past the
 * end does not. */
static uintptr_t hidden_base;
static void NOINLINE hide_root(int slot) { hidden_base = ~(uintptr_t)roots[slot]; }
static void NOINLINE point_root_at(int slot, size_t offset) { roots[slot] = (char *)(~hidden_base) + offset; }
static int NOINLINE hidden_intact(size_t n, uint32_t id) { return intact((const void *)(~hidden_base), n, id); }

static void t_interior(void) {
    static const size_t sizes[] = {100000, 65536, 32769};
    for (size_t k = 0; k < sizeof sizes / sizeof sizes[0]; k++) {
        size_t n = sizes[k];
        uint64_t rounded = alloc_root(0, n, 7u + (uint32_t)k);
        hide_root(0);
        const size_t offsets[] = {0, 1, n / 2, 32768, n - 1};
        for (size_t o = 0; o < sizeof offsets / sizeof offsets[0]; o++) {
            if (offsets[o] >= n) continue;
            point_root_at(0, offsets[o]);
            EXPECT_LIVE(rounded, "interior pointer keeps the object alive");
            CHECK(hidden_intact(n, 7u + (uint32_t)k), "object damaged (offset %zu of %zu)", offsets[o], n);
        }
        point_root_at(0, n); /* one past the end */
        EXPECT_LIVE(0, "a pointer one past the end does not keep it alive");
        roots[0] = NULL;
        EXPECT_LIVE(0, "dropped");
    }
}

/* A freed run is reused, comes back zeroed, and its neighbours are untouched. Needs an empty
 * heap, hence its own process. */
static uintptr_t hidden_b;
static void NOINLINE note_b(int slot) { hidden_b = ~(uintptr_t)roots[slot]; }
static int NOINLINE d_is_b(int slot) { return (uintptr_t)roots[slot] == ~hidden_b; }

static void t_reuse(void) {
    const size_t n = 1u << 20;
    uint64_t ra = alloc_root(0, n, 1), rb = alloc_root(1, n, 2), rc = alloc_root(2, n, 3);
    note_b(1);
    roots[1] = NULL;
    EXPECT_LIVE(ra + rc, "middle run freed");
    uint64_t rd = alloc_root(1, n, 4); /* alloc_root also checks that it is zeroed */
    CHECK(rd == rb, "the reused run has size %llu, not %llu", (unsigned long long)rd, (unsigned long long)rb);
    CHECK(d_is_b(1), "the freed middle run was not reused first");
    CHECK(root_intact(0, n, 1) && root_intact(1, n, 4) && root_intact(2, n, 3), "a neighbour was damaged by the reuse");
    /* a freed hole is also usable by small objects */
    roots[1] = NULL;
    EXPECT_LIVE(ra + rc, "run freed again");
    uint64_t small = 0;
    for (int i = 0; i < 2000; i++) small += alloc_root(10 + i, 1000, 500u + (uint32_t)i);
    int bad = 0;
    for (int i = 0; i < 2000; i++) bad += !root_intact(10 + i, 1000, 500u + (uint32_t)i);
    CHECK(bad == 0, "%d small objects damaged next to a freed large run", bad);
    CHECK(root_intact(0, n, 1) && root_intact(2, n, 3), "a large neighbour was damaged by small allocations");
    EXPECT_LIVE(ra + rc + small, "large and small together");
    /* a request longer than any free stretch still fits: it comes from fresh heap */
    uint64_t re = alloc_root(3000, 3u << 20, 9);
    CHECK(root_intact(0, n, 1) && root_intact(2, n, 3), "a neighbour was damaged by a longer run");
    EXPECT_LIVE(ra + rc + small + re, "after a longer run");
}

/* Arrays whose elements hold pointers: scanned by the repeated element mask. */
struct elem { void *a; long v; void *b; long w; }; /* 4 words on both 32- and 64-bit */
#define ELEM_MASK 0x5u

/* The rounded size of a `size`-byte object, measured with a throwaway allocation. The pointer
 * must not outlive this call (at -O0 it would stay in a stack slot and be a root). */
static uint64_t NOINLINE rounded_of(size_t size) {
    uint64_t before = alloc_bytes_now();
    (void)ubel_gc_alloc_noscan(size);
    return alloc_bytes_now() - before;
}

static void NOINLINE build_array(int slot, size_t count, uint64_t *rounded_array, uint64_t *rounded_child) {
    uint64_t before = alloc_bytes_now();
    struct elem *arr = (struct elem *)ubel_gc_alloc_array(sizeof(struct elem), count, ELEM_MASK);
    *rounded_array = alloc_bytes_now() - before;
    roots[slot] = arr; /* rooted before any child is allocated */
    for (size_t i = 0; i < count; i++) {
        before = alloc_bytes_now();
        void *x = ubel_gc_alloc_noscan(24);
        *rounded_child = alloc_bytes_now() - before;
        fill(x, 24, (uint32_t)(2 * i + 1));
        ((struct elem *)roots[slot])[i].a = x;
        void *y = ubel_gc_alloc_noscan(40);
        fill(y, 40, (uint32_t)(2 * i + 2));
        ((struct elem *)roots[slot])[i].b = y;
        ((struct elem *)roots[slot])[i].v = (long)i;
    }
}

static int NOINLINE check_array(int slot, size_t count, int skip_a_even) {
    struct elem *arr = (struct elem *)roots[slot];
    int bad = 0;
    for (size_t i = 0; i < count; i++) {
        if (skip_a_even && i % 2 == 0) {
            bad += arr[i].a != NULL;
        } else {
            bad += !(arr[i].a && intact(arr[i].a, 24, (uint32_t)(2 * i + 1)));
        }
        bad += !(arr[i].b && intact(arr[i].b, 40, (uint32_t)(2 * i + 2)));
        bad += arr[i].v != (long)i;
    }
    return bad;
}

static void NOINLINE drop_even_a(int slot, size_t count) {
    struct elem *arr = (struct elem *)roots[slot];
    for (size_t i = 0; i < count; i += 2) arr[i].a = NULL;
}

static void t_array_ptr(void) {
    const size_t count = 20000; /* large on both word sizes */
    uint64_t ra = 0, rc = 0;
    build_array(0, count, &ra, &rc);
    uint64_t rb = rounded_of(40); /* the 40-byte child's rounded size */
    EXPECT_LIVE(ra + (uint64_t)count * (rc + rb), "array and children alive");
    CHECK(check_array(0, count, 0) == 0, "children damaged after collections");
    EXPECT_LIVE(ra + (uint64_t)count * (rc + rb), "and again");
    drop_even_a(0, count);
    EXPECT_LIVE(ra + (uint64_t)count * rb + (uint64_t)(count / 2) * rc, "half of the first children dropped");
    CHECK(check_array(0, count, 1) == 0, "the survivors were damaged");
    roots[0] = NULL;
    EXPECT_LIVE(0, "array dropped");
}

/* An array of pointers to large objects. */
static void NOINLINE build_ptr_array(int slot, size_t count, size_t child_size, uint64_t *rounded_child) {
    void **arr = (void **)ubel_gc_alloc_array(sizeof(void *), count, 1u);
    roots[slot] = arr;
    for (size_t i = 0; i < count; i++) {
        uint64_t before = alloc_bytes_now();
        void *x = ubel_gc_alloc_noscan(child_size);
        *rounded_child = alloc_bytes_now() - before;
        fill(x, child_size, (uint32_t)(i + 1));
        ((void **)roots[slot])[i] = x;
    }
}

static int NOINLINE check_ptr_array(int slot, size_t count, size_t child_size, size_t from) {
    void **arr = (void **)roots[slot];
    int bad = 0;
    for (size_t i = from; i < count; i++) bad += !(arr[i] && intact(arr[i], child_size, (uint32_t)(i + 1)));
    return bad;
}

static void NOINLINE drop_first(int slot, size_t n) {
    void **arr = (void **)roots[slot];
    for (size_t i = 0; i < n; i++) arr[i] = NULL;
}

static void t_array_large_children(void) {
    const size_t count = 64, child = 100000;
    uint64_t rc = 0;
    build_ptr_array(0, count, child, &rc);
    uint64_t live_all = COLLECT();
    CHECK(live_all >= (uint64_t)count * rc, "children of a pointer array were freed: live %llu", (unsigned long long)live_all);
    CHECK(check_ptr_array(0, count, child, 0) == 0, "a child was damaged");
    drop_first(0, 32);
    uint64_t live_half = COLLECT();
    (void)live_half;
#ifndef RT_SOFT_LIVE
    CHECK(live_all - live_half == 32 * rc, "dropping 32 children freed %llu, expected %llu",
          (unsigned long long)(live_all - live_half), (unsigned long long)(32 * rc));
#endif
    CHECK(check_ptr_array(0, count, child, 32) == 0, "a surviving child was damaged");
    roots[0] = NULL;
    EXPECT_LIVE(0, "array dropped");
}

/* A whole-element mask of 64 words (the widest allowed), and a small array whose slot has slack. */
static uint64_t NOINLINE build_wide(int slot, size_t count, void *target) {
    size_t words = 64, esz = words * sizeof(void *);
    uint64_t before = alloc_bytes_now();
    void **arr = (void **)ubel_gc_alloc_array(esz, count, ~0ull);
    uint64_t rounded = alloc_bytes_now() - before;
    roots[slot] = arr;
    for (size_t i = 0; i < count * words; i++) ((void **)roots[slot])[i] = target;
    return rounded;
}

static void NOINLINE one_child(int slot) { alloc_root(slot, 200, 77); }

static void NOINLINE make_small_array(uint64_t *rounded_array, uint64_t *rounded_child) {
    /* 5 elements of 3 words in a slot with slack */
    uint64_t before = alloc_bytes_now();
    roots[0] = ubel_gc_alloc_array(3 * sizeof(void *), 5, 0x1u);
    *rounded_array = alloc_bytes_now() - before;
    for (int i = 0; i < 5; i++) {
        before = alloc_bytes_now();
        void *x = ubel_gc_alloc_noscan(100);
        *rounded_child = alloc_bytes_now() - before;
        fill(x, 100, 300u + (uint32_t)i);
        ((void **)roots[0])[i * 3] = x;
    }
}

static int NOINLINE check_small_array(void) {
    int bad = 0;
    for (int i = 0; i < 5; i++) bad += !intact(((void **)roots[0])[i * 3], 100, 300u + (uint32_t)i);
    return bad;
}

/* 16000 bytes of pointers: a size-class object with more than 64 words, so it is scanned by the
 * long-object path rather than the single 64-bit read. */
static void NOINLINE make_mid_array(uint64_t *rounded_array, uint64_t *rounded_child) {
    uint64_t before = alloc_bytes_now();
    roots[0] = ubel_gc_alloc_array(sizeof(void *), 2000, 1u);
    *rounded_array = alloc_bytes_now() - before;
    for (int i = 0; i < 2000; i++) {
        before = alloc_bytes_now();
        void *x = ubel_gc_alloc_noscan(100);
        *rounded_child = alloc_bytes_now() - before;
        fill(x, 100, 900u + (uint32_t)i);
        ((void **)roots[0])[i] = x;
    }
}

static int NOINLINE check_mid_array(void) {
    int bad = 0;
    for (int i = 0; i < 2000; i++) bad += !intact(((void **)roots[0])[i], 100, 900u + (uint32_t)i);
    return bad;
}

static int NOINLINE check_wide_child(void) {
    return intact(((void **)roots[0])[0], 200, 77) && intact(((void **)roots[0])[100 * 64 - 1], 200, 77);
}

static void t_array_wide(void) {
    one_child(1);
    uint64_t child = COLLECT();
    uint64_t rw = build_wide(0, 100, roots[1]);
    roots[1] = NULL; /* only the wide array refers to it now */
    EXPECT_LIVE(rw + child, "a 64-word-element array keeps its child alive");
    CHECK(check_wide_child(), "the child kept alive by 64-word elements is damaged");
    roots[0] = NULL;
    EXPECT_LIVE(0, "wide array and its child dropped");

    uint64_t ra = 0, rc = 0;
    make_small_array(&ra, &rc);
    EXPECT_LIVE(ra + 5 * rc, "a small array keeps its children alive");
    CHECK(check_small_array() == 0, "children of a small array were damaged");
    roots[0] = NULL;
    EXPECT_LIVE(0, "small array dropped");

    make_mid_array(&ra, &rc);
    EXPECT_LIVE(ra + 2000 * rc, "a size-class array of more than 64 words keeps its children alive");
    CHECK(check_mid_array() == 0, "children of a size-class array with more than 64 words were damaged");
    roots[0] = NULL;
    EXPECT_LIVE(0, "mid-size array dropped");
}

/* A pointer-free object that reuses memory which used to hold pointers must not inherit the old
 * pointer bits: its words are data, even a word that happens to equal a live object's address.
 * A size-class object (12000 bytes) is scanned through the shared bitmap, so stale bits would
 * show; a large one carries a no-scan flag instead. */
static void NOINLINE make_pointer_array(int slot, size_t bytes) {
    size_t count = bytes / sizeof(void *);
    /* The child goes first so the array gets the higher span: after the sweep the highest free
     * span is handed out first, which is what makes the next allocation land on the array's
     * old memory and its old pointer bits. */
    void *child = ubel_gc_alloc_noscan(200);
    fill(child, 200, 3);
    roots[slot] = ubel_gc_alloc_array(sizeof(void *), count, 1u);
    for (size_t i = 0; i < count; i++) ((void **)roots[slot])[i] = child;
}

static uint64_t NOINLINE make_noscan_holding_an_address(int slot, size_t bytes) {
    uint64_t before = alloc_bytes_now();
    roots[slot] = ubel_gc_alloc_noscan(bytes);
    uint64_t rounded = alloc_bytes_now() - before;
    void *other = ubel_gc_alloc_noscan(100);
    fill(other, 100, 5);
    ((void **)roots[slot])[0] = other; /* an address, but stored as data */
    return rounded;
}

static void t_noscan_stale(void) {
    static const size_t sizes[] = {12000, 40000};
    for (size_t k = 0; k < sizeof sizes / sizeof sizes[0]; k++) {
        make_pointer_array(0, sizes[k]);
        roots[0] = NULL;
        EXPECT_LIVE(0, "pointer array and its child freed");
        uint64_t rb = make_noscan_holding_an_address(0, sizes[k]);
        EXPECT_LIVE(rb, "a pointer-free object keeps nothing alive through a word that looks like a pointer");
        roots[0] = NULL;
        EXPECT_LIVE(0, "dropped");
    }
}

/* Random allocation and release of pointer-free objects of every size, with frequent
 * collections (UBEL_GC_TRIGGER_KB is set small for this test), checking each object's pattern. */
static uint32_t rng_state = 12345u;
static uint32_t rnd(void) {
    rng_state = rng_state * 1664525u + 1013904223u;
    return rng_state >> 8;
}

static size_t pick_size(void) {
    uint32_t r = rnd() % 100;
    if (r < 55) return 1 + rnd() % 512;
    if (r < 90) return 513 + rnd() % (32768 - 512);
    return 32769 + rnd() % 270000;
}

static size_t stress_size[1024];
static uint32_t stress_id[1024];

#define STRESS_SLOTS 128 /* few live objects, so the pacing triggers many collections */

static void NOINLINE stress_step(int steps) {
    for (int s = 0; s < steps; s++) {
        int slot = (int)(rnd() % STRESS_SLOTS);
        if (roots[slot]) {
            CHECK(root_intact(slot, stress_size[slot], stress_id[slot]), "object in slot %d (%zu bytes) was damaged", slot,
                  stress_size[slot]);
            if (rnd() & 1u) roots[slot] = NULL;
        } else {
            stress_size[slot] = pick_size();
            stress_id[slot] = rnd();
            alloc_root(slot, stress_size[slot], stress_id[slot]);
        }
    }
}

static void t_stress(void) {
    for (int round = 0; round < 40; round++) {
        stress_step(1500);
        int bad = 0;
        for (int i = 0; i < 1024; i++)
            if (roots[i]) bad += !root_intact(i, stress_size[i], stress_id[i]);
        CHECK(bad == 0, "round %d: %d objects damaged", round, bad);
        COLLECT();
    }
    ubel_gc_stats s;
    ubel_gc_get_stats(&s);
    CHECK(s.collections > 150, "the test knob did not force frequent collections (%llu)", (unsigned long long)s.collections);
    CHECK(s.large_allocs > 100, "too few large objects were exercised (%llu)", (unsigned long long)s.large_allocs);
    for (int i = 0; i < 1024; i++) roots[i] = NULL;
    EXPECT_LIVE(0, "everything dropped");
}

/* Misuse is refused loudly: each call must abort. */
static void *misuse_word;
static int aborts(int which) {
    pid_t pid = fork();
    if (pid == 0) {
        if (!freopen("/dev/null", "w", stderr)) _exit(77);
        switch (which) {
        case 0: ubel_gc_alloc(UBEL_GC_MAX_SMALL + 16, 1u); break;          /* pointers beyond a 64-bit mask */
        case 1: ubel_gc_alloc_array(6, 4, 1u); break;                        /* not a whole number of words */
        case 2: ubel_gc_alloc_array(2 * sizeof(void *), 4, 0x8u); break;     /* mask outside the element */
        case 3: ubel_gc_alloc_array(sizeof(void *), ~0ull / 2, 1u); break;   /* total overflows */
        case 4: ubel_gc_alloc_noscan(~0ull); break;                          /* larger than the heap */
        case 5: ubel_gc_add_root(NULL, 64); break;                           /* no address */
        case 6: ubel_gc_add_root(&misuse_word, 1); break;                    /* less than a word */
        case 7: ubel_gc_remove_root(&misuse_word); break;                    /* never registered */
        }
        _exit(0); /* returned: not refused */
    }
    int status = 0;
    waitpid(pid, &status, 0);
    return WIFSIGNALED(status) && WTERMSIG(status) == SIGABRT;
}

/* A scannable arena's roots are the bytes that were written, no more. Object A is referenced from
 * the written part of the first chunk, which the arena then retires; object B only from the unwritten
 * tail of that same chunk (a stale word, as left by recycled memory); object C from the second chunk.
 * A and C must survive, B must be collected. (Before the chunk header recorded `used`, a retired chunk
 * was scanned in full and B was kept alive.) */
static ubel_arena *scan_arena;
static void **cell_a, **cell_c;
static uint64_t sz_a, sz_c;

static void NOINLINE arena_scan_prepare(void) {
    scan_arena = ubel_arena_create_scan(256);
    uint64_t before = alloc_bytes_now();
    void *a = ubel_gc_alloc_noscan(64);
    sz_a = alloc_bytes_now() - before;
    fill(a, 64, 1);
    void *b = ubel_gc_alloc_noscan(64);
    fill(b, 64, 2);
    before = alloc_bytes_now();
    void *c = ubel_gc_alloc_noscan(64);
    sz_c = alloc_bytes_now() - before;
    fill(c, 64, 3);

    cell_a = (void **)scan_arena->cur; /* what the emitted bump path does: take 16 bytes at cur */
    scan_arena->cur += 16;
    cell_a[0] = a;
    ((void **)(scan_arena->end - 32))[0] = b; /* inside the chunk, past cur: written by nobody */
    cell_c = (void **)ubel_arena_grow(scan_arena, 512); /* retires the first chunk, used = 16 */
    cell_c[0] = c;
}

static int NOINLINE arena_scan_intact(void) {
    return intact(cell_a[0], 64, 1) && intact(cell_c[0], 64, 3);
}

static void t_arena_scan(void) {
    arena_scan_prepare();
    EXPECT_LIVE(sz_a + sz_c, "A (written, retired chunk) and C (second chunk) live, B (unwritten tail) freed");
    CHECK(arena_scan_intact(), "the objects the arena references were damaged");
    ubel_arena_destroy(scan_arena);
    EXPECT_LIVE(0, "after the arena is destroyed nothing references them");
}

static void t_misuse(void) {
    for (int i = 0; i < 8; i++) CHECK(aborts(i), "misuse case %d was not refused", i);
}

/* ------------------------------------------------------------------ main */

typedef struct { const char *name; void (*fn)(void); const char *trigger_kb; } test;

/* Objects held only by callee-saved registers. A conservative collector has to treat the registers
 * of the interrupted program as roots; the runtime does it by making scan_roots save all of them
 * in its own frame (__builtin_unwind_init). The probe loads one object per callee-saved register,
 * and runs a collection with nothing else pointing at them: the table holds the complement of each
 * address (not a pointer), the probe un-complements straight into the register, and the stack was
 * scrubbed first. If the register scan is lost, the objects are freed and live is 0.
 * x86-64 and i386 only (naked asm); other targets skip. */
#if defined(__x86_64__) || defined(__i386__)
#define HAVE_REG_PROBE 1
static void NOINLINE collect_cb(void) { ubel_gc_collect(); }

#if defined(__x86_64__)
#define NREGP 5 /* rbx r12 r13 r14 r15 */
__attribute__((naked, noinline)) static void reg_probe(void **words, void (*fn)(void)) {
    __asm__ volatile(
        "pushq %rbx\n\t pushq %r12\n\t pushq %r13\n\t pushq %r14\n\t pushq %r15\n\t" /* rsp is 16-aligned again */
        "movq   (%rdi), %rbx\n\t notq %rbx\n\t"
        "movq  8(%rdi), %r12\n\t notq %r12\n\t"
        "movq 16(%rdi), %r13\n\t notq %r13\n\t"
        "movq 24(%rdi), %r14\n\t notq %r14\n\t"
        "movq 32(%rdi), %r15\n\t notq %r15\n\t"
        "movq %rsi, %rax\n\t xorl %edi, %edi\n\t xorl %esi, %esi\n\t"
        "callq *%rax\n\t"
        "popq %r15\n\t popq %r14\n\t popq %r13\n\t popq %r12\n\t popq %rbx\n\t"
        "ret\n\t");
}
#else
#define NREGP 3 /* ebx esi edi */
__attribute__((naked, noinline)) static void reg_probe(void **words, void (*fn)(void)) {
    __asm__ volatile(
        "pushl %ebx\n\t pushl %esi\n\t pushl %edi\n\t" /* esp is 16-aligned again */
        "movl 16(%esp), %eax\n\t movl 20(%esp), %ecx\n\t"
        "movl  (%eax), %ebx\n\t notl %ebx\n\t"
        "movl 4(%eax), %esi\n\t notl %esi\n\t"
        "movl 8(%eax), %edi\n\t notl %edi\n\t"
        "xorl %eax, %eax\n\t xorl %edx, %edx\n\t"
        "call *%ecx\n\t"
        "popl %edi\n\t popl %esi\n\t popl %ebx\n\t"
        "ret\n\t");
}
#endif

static void NOINLINE complement_root(int slot) { roots[slot] = (void *)~(uintptr_t)roots[slot]; }

static void t_register_root(void) {
    uint64_t want = 0;
    for (int i = 0; i < NREGP; i++) {
        want += alloc_root(i, 100u + 16u * (unsigned)i, 9000u + (uint32_t)i);
        complement_root(i);
    }
    SCRUB();
    reg_probe(roots, collect_cb);
    ubel_gc_stats s;
    ubel_gc_get_stats(&s);
    CHECK(LIVE_OK(s.live_bytes_last, want), "objects held only by callee-saved registers: live %llu, expected %llu",
          (unsigned long long)s.live_bytes_last, (unsigned long long)want);
    for (int i = 0; i < NREGP; i++) roots[i] = NULL;
    EXPECT_LIVE(0, "registers released");
}
#else
static void t_register_root(void) { printf("    (no register probe for this target)\n"); }
#endif

/* The collector's frames are inside the stack range it scans, so nothing in them may look like an
 * address by accident. A clock reading is the realistic case (on a 32-bit target its low word lands
 * inside a given large object for milliseconds at a time), so this feeds the collector a "clock"
 * that returns an address inside a dead object. If a reading is left in a local of
 * ubel_gc_collect, that object stays alive and live is not 0. */
static uintptr_t clock_hidden;
static uint64_t NOINLINE alias_clock(void) { return (uint64_t)(~clock_hidden + 4096u); }
static void NOINLINE remember_complement(int slot) { clock_hidden = (uintptr_t)roots[slot]; roots[slot] = NULL; clock_hidden = ~clock_hidden; }

static void t_clock_alias(void) {
    uint64_t rounded = alloc_root(0, 200000, 5150); /* a large object: an address 4096 bytes in is inside it */
    EXPECT_LIVE(rounded, "held by its root");
    remember_complement(0);          /* drop it, remembering only ~address */
    ubel_gc_set_clock(alias_clock);  /* every reading the collector takes is now an address in the dead object */
    EXPECT_LIVE(0, "dead object whose address was a clock reading");
    ubel_gc_set_clock(NULL);
}

/* Stress mode. ubel_gc_set_stress(1) collects before EVERY allocation, so a pointer that only a
 * register or a half-built object holds is tested at the very next allocation, not whenever the
 * heap happens to fill. Here, in that mode, objects are built so that each one is reachable from
 * one kind of place only: struct fields (a list), an array of pointers, an array large enough for the
 * large-object path, a scannable arena of several chunks (retired ones included), and enum-like and
 * closure-like objects with a pointer payload / captures. While an object is being filled the only
 * reference to it is a local of the builder. Everything is allocated and nothing is garbage, so the live
 * bytes after the build must equal the bytes allocated. */
typedef struct snode { struct snode *next; void *payload; uintptr_t id; } snode; /* pointer words 0, 1 */
#define SNODE_MASK 0x3u
typedef struct { uintptr_t tag; void *payload; } senum;                          /* pointer word 1 */
#define SENUM_MASK 0x2u
typedef struct { void *code; void *cap0; uintptr_t cap1; void *cap2; } sclos;    /* code is not a heap pointer */
#define SCLOS_MASK 0xAu

#define ST_LIST 150u
#define ST_ARR 60u
#define ST_ARENA 120u
#define ST_ENUM 80u
#define ST_CLOS 40u
static ubel_arena *st_arena;
static void **st_cells[ST_ARENA]; /* where each arena cell is; arena memory, not a GC pointer */

static size_t st_psize(unsigned id) { return 24u + (id * 13u) % 90u; }
static void *NOINLINE st_payload(unsigned id) {
    size_t n = st_psize(id);
    void *p = ubel_gc_alloc_noscan(n);
    fill(p, n, id);
    return p;
}
static void *NOINLINE arena_bump16(ubel_arena *a) {
    if ((size_t)(a->end - a->cur) >= 16) {
        void *p = a->cur;
        a->cur += 16;
        return p;
    }
    return ubel_arena_grow(a, 16);
}
static void NOINLINE st_fn(void) {}

static void NOINLINE stress_build(void) {
    snode *head = NULL;
    for (unsigned i = 0; i < ST_LIST; i++) {
        snode *n = (snode *)ubel_gc_alloc(sizeof(snode), SNODE_MASK);
        n->id = i;
        n->payload = st_payload(100u + i); /* collects first: n is held by this frame only */
        n->next = head;
        head = n;
    }
    roots[0] = head;

    void **arr = (void **)ubel_gc_alloc_array(sizeof(void *), ST_ARR, 1u);
    for (unsigned i = 0; i < ST_ARR; i++) arr[i] = st_payload(1000u + i);
    roots[1] = arr;

    size_t nl = UBEL_GC_MAX_CLASS / sizeof(void *) + 200; /* more than 32 KiB of pointers: a run of spans */
    void **big = (void **)ubel_gc_alloc_array(sizeof(void *), nl, 1u);
    for (size_t i = 0; i < nl; i++) big[i] = st_payload(2000u + (unsigned)(i % 5000u));
    roots[2] = big;

    st_arena = ubel_arena_create_scan(256); /* 16 cells per chunk: several chunks, all but one retired */
    for (unsigned i = 0; i < ST_ARENA; i++) {
        void **cell = (void **)arena_bump16(st_arena);
        cell[0] = NULL;
        cell[1] = NULL;
        st_cells[i] = cell;
        cell[0] = st_payload(20000u + i); /* the arena cell is the only reference */
    }

    void **earr = (void **)ubel_gc_alloc_array(sizeof(void *), ST_ENUM, 1u);
    for (unsigned i = 0; i < ST_ENUM; i++) {
        senum *e = (senum *)ubel_gc_alloc(sizeof(senum), SENUM_MASK);
        e->tag = i;
        e->payload = st_payload(30000u + i);
        earr[i] = e;
    }
    roots[3] = earr;

    void **carr = (void **)ubel_gc_alloc_array(sizeof(void *), ST_CLOS, 1u);
    for (unsigned i = 0; i < ST_CLOS; i++) {
        sclos *c = (sclos *)ubel_gc_alloc(sizeof(sclos), SCLOS_MASK);
        c->code = (void *)st_fn;
        c->cap1 = i;
        c->cap0 = st_payload(40000u + i);
        c->cap2 = st_payload(50000u + i);
        carr[i] = c;
    }
    roots[4] = carr;
}

/* Number of things wrong: every object reachable from the structures, checked. */
static int NOINLINE stress_check(void) {
    int bad = 0;
    unsigned i = ST_LIST;
    for (snode *n = (snode *)roots[0]; n && i; n = n->next) {
        i--;
        if (n->id != i || !intact(n->payload, st_psize(100u + i), 100u + i)) bad++;
    }
    if (i != 0) bad++;
    void **arr = (void **)roots[1];
    for (unsigned k = 0; k < ST_ARR; k++) if (!intact(arr[k], st_psize(1000u + k), 1000u + k)) bad++;
    void **big = (void **)roots[2];
    size_t nl = UBEL_GC_MAX_CLASS / sizeof(void *) + 200;
    for (size_t k = 0; k < nl; k++) {
        unsigned id = 2000u + (unsigned)(k % 5000u);
        if (!intact(big[k], st_psize(id), id)) bad++;
    }
    for (unsigned k = 0; k < ST_ARENA; k++)
        if (!intact(st_cells[k][0], st_psize(20000u + k), 20000u + k)) bad++;
    void **earr = (void **)roots[3];
    for (unsigned k = 0; k < ST_ENUM; k++) {
        senum *e = (senum *)earr[k];
        if (e->tag != k || !intact(e->payload, st_psize(30000u + k), 30000u + k)) bad++;
    }
    void **carr = (void **)roots[4];
    for (unsigned k = 0; k < ST_CLOS; k++) {
        sclos *c = (sclos *)carr[k];
        if (c->cap1 != k || c->code != (void *)st_fn || !intact(c->cap0, st_psize(40000u + k), 40000u + k) ||
            !intact(c->cap2, st_psize(50000u + k), 50000u + k))
            bad++;
    }
    return bad;
}

static void NOINLINE stress_churn(unsigned rounds) {
    for (unsigned r = 0; r < rounds; r++) {
        void *g = ubel_gc_alloc_noscan(16u + (r * 37u) % 3000u); /* garbage of mixed classes, reuses freed slots */
        memset(g, 0xA5, 16);
    }
}

static void t_stress_mode(void) {
    ubel_gc_stats s0, s1;
    ubel_gc_get_stats(&s0);
    ubel_gc_set_stress(1);
    stress_build();
    ubel_gc_set_stress(0);
    ubel_gc_get_stats(&s1);
    uint64_t allocs = s1.alloc_count - s0.alloc_count, colls = s1.collections - s0.collections;
    CHECK(allocs > 1000 && colls == allocs, "stress mode: %llu allocations but %llu collections",
          (unsigned long long)allocs, (unsigned long long)colls);
    CHECK(stress_check() == 0, "an object was damaged or lost while building under stress");
    uint64_t built = s1.alloc_bytes - s0.alloc_bytes; /* nothing was garbage: all of it must be live */
    EXPECT_LIVE(built, "every object built under stress is live");

    ubel_gc_set_stress(1);
    stress_churn(400);
    ubel_gc_set_stress(0);
    CHECK(stress_check() == 0, "an object was damaged by allocations under stress that reuse freed slots");
    EXPECT_LIVE(built, "after churn only the structures are live");

    for (int k = 0; k < 5; k++) roots[k] = NULL;
    ubel_arena_destroy(st_arena);
    st_arena = NULL;
    EXPECT_LIVE(0, "everything dropped");
}

/* Global roots. A pointer kept in a global is only a root if the memory holding it is registered.
 * Each phase refills the whole array with fresh objects and registers a different region of it, so
 * the live bytes say exactly which words were scanned: both ends of a region, a changed length, a
 * start that is not word-aligned. The second half registers 300 one-word regions (the table has to
 * grow) and removes them in an order that exercises the swap-with-last removal. */
static void *g_slots[8];
static uint64_t g_sz[8];
static void *g_many[300];

static void NOINLINE g_refill(void) {
    for (unsigned i = 0; i < 8; i++) {
        uint64_t before = alloc_bytes_now();
        void *p = ubel_gc_alloc_noscan(100u + 40u * i);
        g_sz[i] = alloc_bytes_now() - before;
        fill(p, 100u + 40u * i, 700u + i);
        g_slots[i] = p;
    }
}
static int NOINLINE g_intact(unsigned i) { return intact(g_slots[i], 100u + 40u * i, 700u + i); }
static uint64_t g_sum(unsigned from, unsigned to) { /* slots from..to-1 */
    uint64_t t = 0;
    for (unsigned i = from; i < to; i++) t += g_sz[i];
    return t;
}
static void NOINLINE gm_put(unsigned i) {
    void *p = ubel_gc_alloc_noscan(64);
    fill(p, 64, 500u + i);
    g_many[i] = p;
}
static int NOINLINE gm_intact(unsigned i) { return intact(g_many[i], 64, 500u + i); }

static void t_global_roots(void) {
    const size_t W = sizeof(void *);

    /* the first four words only: word 3 is scanned, word 4 (one past the end) is not */
    ubel_gc_add_root(g_slots, 4 * W);
    g_refill();
    EXPECT_LIVE(g_sum(0, 4), "words 0..3 registered: only those objects live");
    for (unsigned i = 0; i < 4; i++) CHECK(g_intact(i), "global-root object %u was damaged", i);

    /* the same start with a longer length */
    ubel_gc_add_root(g_slots, 8 * W);
    g_refill();
    EXPECT_LIVE(g_sum(0, 8), "same start, length changed to 8 words: all live");
    for (unsigned i = 0; i < 8; i++) CHECK(g_intact(i), "global-root object %u was damaged after the length change", i);

    /* a region that starts in the middle: the word before it is not scanned */
    ubel_gc_remove_root(g_slots);
    ubel_gc_add_root(&g_slots[2], 4 * W);
    g_refill();
    EXPECT_LIVE(g_sum(2, 6), "words 2..5 registered: 0, 1, 6 and 7 are not roots");
    for (unsigned i = 2; i < 6; i++) CHECK(g_intact(i), "global-root object %u was damaged (middle region)", i);

    /* a start that is not word-aligned: only the words that fit entirely inside are scanned (1, 2, 3) */
    ubel_gc_remove_root(&g_slots[2]);
    ubel_gc_add_root((char *)g_slots + 1, 4 * W);
    g_refill();
    EXPECT_LIVE(g_sum(1, 4), "unaligned region: words 1..3 fit, word 4 does not");

    /* nothing registered: the global holds the only reference and does not keep anything alive */
    ubel_gc_remove_root((char *)g_slots + 1);
    g_refill();
    EXPECT_LIVE(0, "no root registered");

    /* many regions: the table has to grow, and removal swaps the last entry into the hole */
    for (unsigned i = 0; i < 300; i++) ubel_gc_add_root(&g_many[i], W);
    uint64_t before300 = alloc_bytes_now();
    for (unsigned i = 0; i < 300; i++) gm_put(i);
    uint64_t all300 = alloc_bytes_now() - before300; /* nothing else was allocated: this is all live */
    EXPECT_LIVE(all300, "300 one-word regions registered: all 300 objects live");
    for (unsigned i = 0; i < 300; i += 2) ubel_gc_remove_root(&g_many[i]); /* evens */
    EXPECT_LIVE(all300 / 2u, "after removing the even regions only the odd 150 are live");
    for (unsigned i = 1; i < 300; i += 2) CHECK(gm_intact(i), "global-root object %u (odd) was damaged", i);
    for (unsigned i = 1; i < 300; i += 2) ubel_gc_remove_root(&g_many[i]);
    EXPECT_LIVE(0, "all regions removed");
}

/* ------------------------------------------------------- string helpers (A9) */

/* A string object is { int64 length, bytes } with no terminator. `mk` builds one in a caller-owned buffer. */
static void *mk(int64_t *buf, const char *text, size_t len) {
    int64_t n = (int64_t)len;
    memcpy(buf, &n, 8);
    memcpy((char *)buf + 8, text, len);
    return buf;
}

/* ubel_str_contains / starts_with / ends_with against Rust's str methods (the interpreter's). The answers
 * below are the ones Rust gives; the last two blocks put the bytes a longer needle would need right behind
 * (or in front of) a short string, so a helper that trusts the needle's length instead of the string's
 * would read past it and still match. */
static void t_str_helpers(void) {
    static const struct {
        const char *s, *t;
        int contains, starts, ends;
    } cases[] = {
        {"", "", 1, 1, 1},
        {"", "a", 0, 0, 0},
        {"a", "", 1, 1, 1},
        {"abc", "abc", 1, 1, 1},
        {"abc", "abcd", 0, 0, 0},
        {"abcd", "abc", 1, 1, 0},
        {"abcd", "bcd", 1, 0, 1},
        {"ab", "b", 1, 0, 1},
        {"ab", "a", 1, 1, 0},
        {"ab", "ba", 0, 0, 0},
        {"abc", "ax", 0, 0, 0},
        {"xay", "az", 0, 0, 0},
        {"xabx", "ab", 1, 0, 0},
        {"aaab", "aab", 1, 0, 1},
        {"abab", "abab", 1, 1, 1},
        {"ababab", "babab", 1, 0, 1},
        {"mississippi", "issip", 1, 0, 0},
        {"mississippi", "sip", 1, 0, 0},
        {"mississippi", "ippi", 1, 0, 1},
        {"mississippi", "miss", 1, 1, 0},
        {"mississippi", "mississippi!", 0, 0, 0},
        {"h\xc3\xa9llo w\xc3\xb6rld", "w\xc3\xb6r", 1, 0, 0},
        {"h\xc3\xa9llo", "\xc3\xa9", 1, 0, 0},
        {"\xc3\xa9", "\xc3\xa9", 1, 1, 1},
    };
    for (size_t i = 0; i < sizeof cases / sizeof cases[0]; i++) {
        int64_t sb[8], tb[8];
        void *s = mk(sb, cases[i].s, strlen(cases[i].s));
        void *t = mk(tb, cases[i].t, strlen(cases[i].t));
        CHECK(ubel_str_contains(s, t) == cases[i].contains, "contains(\"%s\", \"%s\") != %d", cases[i].s, cases[i].t, cases[i].contains);
        CHECK(ubel_str_starts_with(s, t) == cases[i].starts, "starts_with(\"%s\", \"%s\") != %d", cases[i].s, cases[i].t, cases[i].starts);
        CHECK(ubel_str_ends_with(s, t) == cases[i].ends, "ends_with(\"%s\", \"%s\") != %d", cases[i].s, cases[i].t, cases[i].ends);
    }

    /* "ab" with "cd" right behind it in memory, and the needle "abcd": no method may match */
    int64_t buf[8], nb[8];
    memset(buf, 0, sizeof buf);
    int64_t two = 2;
    memcpy(buf, &two, 8);
    memcpy((char *)buf + 8, "abcd", 4);
    void *ab = buf, *abcd = mk(nb, "abcd", 4);
    CHECK(!ubel_str_starts_with(ab, abcd), "starts_with read past the end of the string");
    CHECK(!ubel_str_contains(ab, abcd), "contains read past the end of the string");

    /* "cd" with two zero bytes right in front of its bytes (the high bytes of its length), and the needle
     * "\0\0cd": ends_with must not look in front of the string */
    int64_t cb[8], zb[8];
    memset(cb, 0, sizeof cb);
    memcpy(cb, &two, 8);
    memcpy((char *)cb + 8, "cd", 2);
    void *cd = cb, *zzcd = mk(zb, "\0\0cd", 4);
    CHECK(!ubel_str_ends_with(cd, zzcd), "ends_with read in front of the string");
}

/* The string-building helpers allocate in the arena they are given, and in the GC heap when it is NULL. The
 * arena path must not touch the collector's allocation count and must bump the arena by the rounded size of
 * the result; the GC path must allocate exactly one object. */
static void t_str_build_arena(void) {
    ubel_arena *a = ubel_arena_create(0);
    int64_t sb[16], fb[8], tb[8];
    void *s = mk(sb, "  h\xc3\xa9llo \xc3\x9f  ", 13); /* "  héllo ß  ", 13 bytes */
    void *from = mk(fb, "l", 1), *to = mk(tb, "LL", 2);
    for (int which = 0; which < 6; which++) {
        for (int in_arena = 0; in_arena < 2; in_arena++) {
            ubel_gc_stats s0, s1;
            ubel_gc_get_stats(&s0);
            char *cur0 = a->cur;
            void *r = NULL;
            void *ar = in_arena ? (void *)a : NULL;
            switch (which) {
                case 0: r = ubel_str_to_upper(s, ar); break;
                case 1: r = ubel_str_to_lower(s, ar); break;
                case 2: r = ubel_str_trim(s, ar); break;
                case 3: r = ubel_str_trim_start(s, ar); break;
                case 4: r = ubel_str_trim_end(s, ar); break;
                default: r = ubel_str_replace(s, from, to, ar); break;
            }
            ubel_gc_get_stats(&s1);
            int64_t len = *(const int64_t *)r;
            uint64_t rounded = ((uint64_t)len + 8 + 15u) & ~(uint64_t)15u;
            if (in_arena) {
                CHECK(s1.alloc_count == s0.alloc_count, "helper %d with an arena allocated in the GC heap", which);
                CHECK((char *)r == cur0, "helper %d result is not where the arena was", which);
                CHECK(a->cur == cur0 + rounded, "helper %d bumped the arena by %lld, expected %llu", which, (long long)(a->cur - cur0), (unsigned long long)rounded);
            } else {
                CHECK(s1.alloc_count == s0.alloc_count + 1, "helper %d without an arena did not allocate exactly one GC object", which);
                CHECK(a->cur == cur0, "helper %d without an arena touched the arena", which);
            }
        }
    }
    /* the text is right in both places */
    void *u = ubel_str_to_upper(s, a);
    CHECK(*(const int64_t *)u == 13 && memcmp((const char *)u + 8, "  H\xc3\x89LLO SS  ", 13) == 0, "to_upper in an arena gave the wrong text");
    void *t = ubel_str_trim(s, a);
    CHECK(*(const int64_t *)t == 9 && memcmp((const char *)t + 8, "h\xc3\xa9llo \xc3\x9f", 9) == 0, "trim in an arena gave the wrong text");
    ubel_arena_destroy(a);
}

static const test tests[] = {
    {"classes", t_classes, NULL},
    {"large_basic", t_large_basic, NULL},
    {"interior", t_interior, NULL},
    {"reuse", t_reuse, NULL},
    {"array_ptr", t_array_ptr, "64"},
    {"array_large_children", t_array_large_children, "64"},
    {"array_wide", t_array_wide, NULL},
    {"noscan_stale", t_noscan_stale, NULL},
    {"stress", t_stress, "64"},
    {"misuse", t_misuse, NULL},
    {"arena_scan", t_arena_scan, NULL},
    {"register_root", t_register_root, NULL},
    {"clock_alias", t_clock_alias, NULL},
    {"stress_mode", t_stress_mode, NULL},
    {"global_roots", t_global_roots, NULL},
    {"str_helpers", t_str_helpers, NULL},
    {"str_build_arena", t_str_build_arena, NULL},
};
#define NTESTS (sizeof tests / sizeof tests[0])

int main(int argc, char **argv) {
    if (argc < 2) {
        fprintf(stderr, "usage: %s <test>|all\n", argv[0]);
        return 2;
    }
    if (strcmp(argv[1], "all") == 0) {
        int bad = 0;
        for (size_t i = 0; i < NTESTS; i++) {
            pid_t pid = fork();
            if (pid == 0) {
                execl(argv[0], argv[0], tests[i].name, (char *)NULL);
                _exit(127);
            }
            int status = 0;
            waitpid(pid, &status, 0);
            if (!(WIFEXITED(status) && WEXITSTATUS(status) == 0)) bad++;
        }
        printf("rt_gc_test: %zu tests, %d failed\n", NTESTS, bad);
        return bad != 0;
    }
    for (size_t i = 0; i < NTESTS; i++) {
        if (strcmp(argv[1], tests[i].name) != 0) continue;
        if (tests[i].trigger_kb) setenv("UBEL_GC_TRIGGER_KB", tests[i].trigger_kb, 1);
        ubel_gc_init(__builtin_frame_address(0));
        roots_init();
        tests[i].fn();
        printf("%s %s\n", failures ? "FAIL" : "PASS", tests[i].name);
        return failures != 0;
    }
    fprintf(stderr, "unknown test '%s'\n", argv[1]);
    return 2;
}

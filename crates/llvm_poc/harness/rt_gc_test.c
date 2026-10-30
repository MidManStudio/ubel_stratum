/* crates/llvm_poc/harness/rt_gc_test.c
 *
 * Tests for the collector's large objects, the extended size classes and the array
 * allocator (roadmap A5). Run one test per process so a failure or an abort in one cannot
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

/* The arena's only chunk is exactly the table, and it is zeroed: a scannable arena scans every
 * byte of its chunks, so bytes nobody wrote (recycled malloc memory) could hold stale pointers
 * and keep garbage alive. */
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
#define COLLECT()                                                                  \
    ({                                                                             \
        {                                                                          \
            size_t n_ = scrub_n;                                                   \
            volatile unsigned char b_[n_]; /* released at the end of this block */ \
            memset((void *)b_, 0, n_);                                             \
            __asm__ volatile("" : : "r"(b_) : "memory");                           \
        }                                                                          \
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
        }
        _exit(0); /* returned: not refused */
    }
    int status = 0;
    waitpid(pid, &status, 0);
    return WIFSIGNALED(status) && WTERMSIG(status) == SIGABRT;
}

static void t_misuse(void) {
    for (int i = 0; i < 5; i++) CHECK(aborts(i), "misuse case %d was not refused", i);
}

/* ------------------------------------------------------------------ main */

typedef struct { const char *name; void (*fn)(void); const char *trigger_kb; } test;
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

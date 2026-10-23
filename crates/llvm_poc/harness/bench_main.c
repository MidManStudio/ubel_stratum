/* crates/llvm_poc/harness/bench_main.c
 *
 * Drives the hand-written IR workloads in bench/tiers.ll, once per tier,
 * checks that every tier computes the same checksum, and prints markdown.
 *
 * Usage: bench [--iters N] [--scale X] [--only SUBSTR] [--require-gc]
 *   --iters  timed runs per benchmark and tier (default 5); min and median are shown
 *   --scale  multiplies the amount of work (default 1.0); use 0.02 for a smoke test
 *   --only   run only benchmarks or sections whose name contains SUBSTR
 *            (Stage 0: churn, churn+live, tree, arith. Stage 1A: xing-lifecycle,
 *            xing-callback, xing-iter, xing-low, or just xing for all four)
 *   --require-gc  self-test mode: also fail unless the HIGH runs really exercised the
 *                 collector (at least one collection, and the collector kept a known
 *                 live list alive). Pair it with UBEL_GC_TRIGGER_KB=16 so collections
 *                 happen even at small scale. run_bench.sh --selftest does exactly this.
 *
 * Exit status: 0 ok, 2 if any checksum differed between tiers or runs,
 *              3 if --require-gc was given and the GC was not exercised or lost live data.
 */
#include "ubel_rt.h"

#include <stdio.h>
#include <stdlib.h>
#include <string.h>

extern int64_t churn_high(int64_t, int64_t, int64_t);
extern int64_t churn_mid(int64_t, int64_t, int64_t);
extern int64_t churn_low(int64_t, int64_t, int64_t);
extern int64_t tree_high(int64_t, int64_t);
extern int64_t tree_mid(int64_t, int64_t);
extern int64_t tree_low(int64_t, int64_t);
extern int64_t arith(int64_t);

typedef int64_t (*bench_fn)(int64_t, int64_t, int64_t);

/* Two- and one-argument workloads, adapted to the common three-argument shape.
 * The tree workloads take (depth, reps); a is the scaled count, so reps = a, depth = b. */
static int64_t tree_high_w(int64_t a, int64_t b, int64_t c) { (void)c; return tree_high(b, a); }
static int64_t tree_mid_w(int64_t a, int64_t b, int64_t c)  { (void)c; return tree_mid(b, a); }
static int64_t tree_low_w(int64_t a, int64_t b, int64_t c)  { (void)c; return tree_low(b, a); }
static int64_t arith_w(int64_t a, int64_t b, int64_t c)     { (void)b; (void)c; return arith(a); }

enum { HIGH = 0, MID = 1, LOW = 2 };

typedef struct {
    const char *name;
    const char *what;
    int64_t a, b, c;         /* a is the work count and is multiplied by --scale */
    int tiered;              /* 0 = single tier-independent function */
    bench_fn fn[3];
} bench_t;

typedef struct {
    uint64_t ns;
    int64_t result;
    ubel_gc_stats gc;
} run_t;

static int cmp_u64(const void *x, const void *y) {
    uint64_t a = *(const uint64_t *)x, b = *(const uint64_t *)y;
    return (a > b) - (a < b);
}

static run_t run_once(const bench_t *bn, int64_t a, int tier) {
    run_t r;
    memset(&r, 0, sizeof r);
    if (tier == HIGH) {
        ubel_gc_collect();      /* start every HIGH run from a clean heap */
        ubel_gc_reset_stats();
    }
    uint64_t t0 = ubel_now_ns();
    r.result = bn->fn[tier](a, bn->b, bn->c);
    r.ns = ubel_now_ns() - t0;
    if (tier == HIGH) ubel_gc_get_stats(&r.gc);
    return r;
}

typedef struct {
    uint64_t min_ns, med_ns;
    int64_t result;
    ubel_gc_stats gc;        /* stats of the fastest HIGH run */
} summary_t;

static summary_t run_tier(const bench_t *bn, int64_t a, int tier, int iters, int *mismatch) {
    uint64_t *times = (uint64_t *)malloc((size_t)iters * sizeof *times);
    summary_t s;
    memset(&s, 0, sizeof s);
    s.min_ns = UINT64_MAX;
    for (int i = 0; i < iters; i++) {
        run_t r = run_once(bn, a, tier);
        times[i] = r.ns;
        if (i == 0) s.result = r.result;
        else if (r.result != s.result) *mismatch = 1;
        if (r.ns < s.min_ns) { s.min_ns = r.ns; s.gc = r.gc; }
    }
    qsort(times, (size_t)iters, sizeof *times, cmp_u64);
    s.med_ns = times[iters / 2];
    free(times);
    return s;
}

static double ms(uint64_t ns) { return (double)ns / 1e6; }


/* ======================================================================
 * Stage 1A: tier boundaries (bench/boundaries.ll). See docs/LLVM_POC.md.
 * ====================================================================== */

extern int64_t xcb_callback(int64_t, int64_t);
extern int64_t xcb_copyout(int64_t, int64_t);
extern int64_t xcb_mid_only(int64_t, int64_t);
extern int64_t xcb_high_only(int64_t, int64_t);
extern int64_t xcb_low_only(int64_t, int64_t);
extern int64_t xit_arena_plain(int64_t);
extern int64_t xit_arena_scan(int64_t);
extern int64_t xit_gc_list(int64_t);
extern int64_t xlow_from_high(int64_t, int64_t);
extern int64_t xlow_from_mid(int64_t, int64_t);
extern int64_t xlow_inline(int64_t, int64_t);

/* Adapters to the common three-argument shape. */
static int64_t w_cb_callback(int64_t a, int64_t b, int64_t c) { (void)c; return xcb_callback(a, b); }
static int64_t w_cb_copyout(int64_t a, int64_t b, int64_t c)  { (void)c; return xcb_copyout(a, b); }
static int64_t w_cb_mid(int64_t a, int64_t b, int64_t c)      { (void)c; return xcb_mid_only(a, b); }
static int64_t w_cb_high(int64_t a, int64_t b, int64_t c)     { (void)c; return xcb_high_only(a, b); }
static int64_t w_cb_low(int64_t a, int64_t b, int64_t c)      { (void)c; return xcb_low_only(a, b); }
static int64_t w_it_plain(int64_t a, int64_t b, int64_t c)    { (void)b; (void)c; return xit_arena_plain(a); }
static int64_t w_it_scan(int64_t a, int64_t b, int64_t c)     { (void)b; (void)c; return xit_arena_scan(a); }
static int64_t w_it_list(int64_t a, int64_t b, int64_t c)     { (void)b; (void)c; return xit_gc_list(a); }
static int64_t w_low_high(int64_t a, int64_t b, int64_t c)    { (void)c; return xlow_from_high(a, b); }
static int64_t w_low_mid(int64_t a, int64_t b, int64_t c)     { (void)c; return xlow_from_mid(a, b); }
static int64_t w_low_inl(int64_t a, int64_t b, int64_t c)     { (void)c; return xlow_inline(a, b); }

typedef struct {
    uint64_t min_ns, med_ns;
    int64_t result;
    ubel_gc_stats gc;   /* stats of the fastest run */
} vsum_t;

/* Run f(a, b, c) `iters` times. Every run starts from a collected heap with
 * fresh GC stats, so the stats of the fastest run are meaningful for any tier. */
static vsum_t run_var(bench_fn f, int64_t a, int64_t b, int64_t c, int iters, int *mismatch) {
    uint64_t *times = (uint64_t *)malloc((size_t)iters * sizeof *times);
    vsum_t s;
    memset(&s, 0, sizeof s);
    s.min_ns = UINT64_MAX;
    for (int i = 0; i < iters; i++) {
        ubel_gc_collect();
        ubel_gc_reset_stats();
        uint64_t t0 = ubel_now_ns();
        int64_t r = f(a, b, c);
        uint64_t ns = ubel_now_ns() - t0;
        ubel_gc_stats g;
        ubel_gc_get_stats(&g);
        times[i] = ns;
        if (i == 0) s.result = r;
        else if (r != s.result) *mismatch = 1;
        if (ns < s.min_ns) { s.min_ns = ns; s.gc = g; }
    }
    qsort(times, (size_t)iters, sizeof *times, cmp_u64);
    s.med_ns = times[iters / 2];
    free(times);
    return s;
}

static int section_wanted(const char *only, const char *name) { return !only || strstr(name, only) != NULL; }

/* Under --require-gc: a variant that allocated a meaningful amount in the GC heap
 * must have collected at least once, or the run proves nothing about GC safety. */
static void check_gc_exercised(const char *what, const vsum_t *v, int require_gc, int *problem) {
    if (require_gc && v->gc.alloc_bytes >= 64u * 1024u && v->gc.collections == 0) {
        fprintf(stderr, "GC SELF-TEST: %s allocated %llu GC bytes but never collected\n",
                what, (unsigned long long)v->gc.alloc_bytes);
        *problem = 1;
    }
}

static void run_boundaries(const char *only, int iters, double scale, int require_gc,
                           int *mismatch, int *gc_problem) {
    char what[96];

    /* ---- 1. Arena lifecycle: how many objects does an arena need to pay for itself? */
    if (section_wanted(only, "xing-lifecycle")) {
        static const int64_t per_frame[] = {1, 2, 4, 8, 16, 32, 64, 256, 1024};
        const int np = (int)(sizeof per_frame / sizeof per_frame[0]);
        int64_t total = (int64_t)(2000000.0 * scale);
        if (total < 2048) total = 2048;
        printf("\n## Tier boundaries 1: arena lifecycle\n\n"
               "A fixed %lld objects (32 B) are allocated, in groups of N per \"frame\". HIGH just allocates, "
               "MID creates, fills and destroys an arena per frame, LOW mallocs and frees. "
               "Times are ns per object (min of %d runs). "
               "Small N shows what creating and destroying an arena costs.\n\n",
               (long long)total, iters);
        printf("| objects per arena (N) | HIGH | MID | LOW | MID / HIGH | MID / LOW |\n|---:|---:|---:|---:|---:|---:|\n");
        int64_t first_beats_high = 0, first_beats_low = 0;
        for (int i = 0; i < np; i++) {
            int64_t n = per_frame[i];
            int64_t frames = total / n;
            if (frames < 1) frames = 1;
            double objs = (double)(frames * n);
            vsum_t h = run_var(churn_high, frames, n, 0, iters, mismatch);
            vsum_t m = run_var(churn_mid,  frames, n, 0, iters, mismatch);
            vsum_t l = run_var(churn_low,  frames, n, 0, iters, mismatch);
            if (h.result != m.result || h.result != l.result) *mismatch = 1;
            double hn = (double)h.min_ns / objs, mn = (double)m.min_ns / objs, ln = (double)l.min_ns / objs;
            printf("| %lld | %.1f | %.1f | %.1f | %.2f | %.2f |\n", (long long)n, hn, mn, ln, mn / hn, mn / ln);
            if (!first_beats_high && mn < hn) first_beats_high = n;
            if (!first_beats_low && mn < ln) first_beats_low = n;
        }
        if (first_beats_high) printf("\nMID first beats HIGH at N = %lld objects per arena.", (long long)first_beats_high);
        else printf("\nMID never beat HIGH in this range.");
        if (first_beats_low) printf(" MID first beats LOW at N = %lld.\n", (long long)first_beats_low);
        else printf(" MID never beat LOW in this range.\n");
        fprintf(stderr, "done: xing-lifecycle\n");
    }

    /* ---- 2. Callbacks and owned results across the MID/HIGH boundary */
    if (section_wanted(only, "xing-callback")) {
        static const struct { int64_t depth, reps; } shape[2] = { {4, 100000}, {9, 3000} };
        printf("\n## Tier boundaries 2: MID builds an arena AST, HIGH gets a pointer into it\n\n"
               "Modelled on examples/arena_demo/ast_builder.ubl. Per repetition a binary expression tree is built and evaluated. "
               "`callback`: HIGH calls MID, MID builds the tree in an arena and calls a HIGH closure with a pointer into it; "
               "the closure evaluates it and returns a GC-owned result. `copy-out`: the closure instead deep-copies the tree into "
               "the GC heap, the arena is destroyed, and HIGH evaluates the copy afterwards. The other three do the whole job in one tier "
               "with no boundary. Times are ms (min of %d runs); `ns/rep` is per tree.\n\n", iters);
        for (int s = 0; s < 2; s++) {
            int64_t depth = shape[s].depth;
            int64_t reps = (int64_t)((double)shape[s].reps * scale);
            if (reps < 2) reps = 2;
            int64_t nodes = ((int64_t)1 << (depth + 1)) - 1;
            vsum_t cb = run_var(w_cb_callback, depth, reps, 0, iters, mismatch);
            vsum_t co = run_var(w_cb_copyout,  depth, reps, 0, iters, mismatch);
            vsum_t mo = run_var(w_cb_mid,      depth, reps, 0, iters, mismatch);
            vsum_t ho = run_var(w_cb_high,     depth, reps, 0, iters, mismatch);
            vsum_t lo = run_var(w_cb_low,      depth, reps, 0, iters, mismatch);
            if (cb.result != co.result || cb.result != mo.result || cb.result != ho.result || cb.result != lo.result)
                *mismatch = 1;
            snprintf(what, sizeof what, "callback (depth %lld)", (long long)depth);
            check_gc_exercised(what, &cb, require_gc, gc_problem);
            snprintf(what, sizeof what, "copy-out (depth %lld)", (long long)depth);
            check_gc_exercised(what, &co, require_gc, gc_problem);
            snprintf(what, sizeof what, "HIGH only (depth %lld)", (long long)depth);
            check_gc_exercised(what, &ho, require_gc, gc_problem);

            printf("Tree depth %lld (%lld nodes), %lld trees\n\n", (long long)depth, (long long)nodes, (long long)reps);
            printf("| variant | time (ms) | ns/rep | what it measures |\n|---|---:|---:|---|\n");
            struct { const char *n; const vsum_t *v; const char *w; } rows[] = {
                {"callback (HIGH->MID->HIGH)", &cb, "arena + indirect call into HIGH + GC result"},
                {"copy-out (HIGH->MID->HIGH)", &co, "arena + callback + deep copy into the GC heap"},
                {"MID only",  &mo, "arena build and evaluate, no boundary"},
                {"HIGH only", &ho, "GC build and evaluate, no boundary"},
                {"LOW only",  &lo, "malloc build, evaluate, free, no boundary"},
            };
            for (int r = 0; r < 5; r++)
                printf("| %s | %.2f | %.0f | %s |\n", rows[r].n, ms(rows[r].v->min_ns),
                       (double)rows[r].v->min_ns / (double)reps, rows[r].w);
            double cross = ((double)cb.min_ns - (double)mo.min_ns) / (double)reps;
            double copy  = ((double)co.min_ns - (double)mo.min_ns) / (double)reps;
            printf("\nCrossing cost per tree (callback minus MID only): %.0f ns. "
                   "Copy-out cost per tree (copy-out minus MID only): %.0f ns, %.1f ns per node copied.\n\n",
                   cross, copy, copy / (double)nodes);
            printf("| GC activity | collections | GC time (ms) | max pause (ms) |\n|---|---:|---:|---:|\n");
            struct { const char *n; const vsum_t *v; } g[] = { {"callback", &cb}, {"copy-out", &co}, {"HIGH only", &ho} };
            for (int r = 0; r < 3; r++)
                printf("| %s | %llu | %.2f | %.3f |\n", g[r].n, (unsigned long long)g[r].v->gc.collections,
                       ms(g[r].v->gc.mark_ns + g[r].v->gc.sweep_ns), ms(g[r].v->gc.max_pause_ns));
            printf("\n");
        }
        fprintf(stderr, "done: xing-callback\n");
    }

    /* ---- 3. Iterator pattern, and the hidden-root hazard */
    if (section_wanted(only, "xing-iter")) {
        int64_t n = (int64_t)(50000.0 * scale);
        if (n < 200) n = 200;
        uint64_t ref = 0;
        for (int64_t i = 0; i < n; i++) ref += (uint64_t)i * (uint64_t)(7 * i + 1) + 3u;

        printf("\n## Tier boundaries 3: the iterator pattern and hidden roots\n\n"
               "MID loops over %lld items. Each item is an arena object; a HIGH closure turns it into a GC-owned result "
               "(and allocates short-lived garbage, so collections happen while the MID frame and its arena are live); "
               "MID keeps the results. Where it keeps them matters.\n\n", (long long)n);
        vsum_t sc = run_var(w_it_scan, n, 0, 0, iters, mismatch);
        vsum_t li = run_var(w_it_list, n, 0, 0, iters, mismatch);
        if ((uint64_t)sc.result != ref || (uint64_t)li.result != ref) {
            *mismatch = 1;
            fprintf(stderr, "ITERATOR MISMATCH: expected %llu, scannable arena gave %llu, GC list gave %llu\n",
                    (unsigned long long)ref, (unsigned long long)(uint64_t)sc.result, (unsigned long long)(uint64_t)li.result);
        }
        check_gc_exercised("iterator, scannable arena", &sc, require_gc, gc_problem);
        check_gc_exercised("iterator, GC list", &li, require_gc, gc_problem);
        printf("| results kept in | time (ms) | ns/item | collections | GC time (ms) | max pause (ms) | correct |\n|---|---:|---:|---:|---:|---:|---|\n");
        struct { const char *n; const vsum_t *v; } it[] = { {"scannable arena array", &sc}, {"GC-owned list", &li} };
        for (int r = 0; r < 2; r++)
            printf("| %s | %.2f | %.0f | %llu | %.2f | %.3f | %s |\n", it[r].n, ms(it[r].v->min_ns),
                   (double)it[r].v->min_ns / (double)n, (unsigned long long)it[r].v->gc.collections,
                   ms(it[r].v->gc.mark_ns + it[r].v->gc.sweep_ns), ms(it[r].v->gc.max_pause_ns),
                   (uint64_t)it[r].v->result == ref ? "yes" : "NO");

        /* The hazard: a plain arena is invisible to the collector. Force frequent collections. */
        int64_t dn = n < 20000 ? n : 20000;
        uint64_t dref = 0;
        for (int64_t i = 0; i < dn; i++) dref += (uint64_t)i * (uint64_t)(7 * i + 1) + 3u;
        const char *envt = getenv("UBEL_GC_TRIGGER_KB");
        uint64_t restore_kb = envt && atol(envt) > 0 ? (uint64_t)atol(envt) : 0;
        ubel_gc_set_min_trigger_kb(16);
        int dm = 0;
        vsum_t pl = run_var(w_it_plain, dn, 0, 0, 1, &dm);
        vsum_t sc2 = run_var(w_it_scan, dn, 0, 0, 1, &dm);
        ubel_gc_set_min_trigger_kb(restore_kb);
        int plain_ok = (uint64_t)pl.result == dref;
        int scan_ok = (uint64_t)sc2.result == dref;
        printf("\nHazard demo (%lld items, a collection every 16 KiB of GC allocation):\n\n", (long long)dn);
        printf("| results kept in | collections | result |\n|---|---:|---|\n");
        printf("| plain arena array (not scanned) | %llu | %s |\n", (unsigned long long)pl.gc.collections,
               plain_ok ? (pl.gc.collections ? "correct by luck" : "correct, but no collection ran") : "WRONG: GC freed live results");
        printf("| scannable arena array | %llu | %s |\n", (unsigned long long)sc2.gc.collections, scan_ok ? "correct" : "WRONG");
        if (!scan_ok) {
            *mismatch = 1;
            fprintf(stderr, "ITERATOR MISMATCH under forced collections with a scannable arena\n");
        }
        fprintf(stderr, "done: xing-iter\n");
    }

    /* ---- 4. Calls into LOW code */
    if (section_wanted(only, "xing-low")) {
        int64_t reps = (int64_t)(5000000.0 * scale);
        if (reps < 1000) reps = 1000;
        const int64_t n = 32;
        vsum_t hl = run_var(w_low_high, n, reps, 0, iters, mismatch);
        vsum_t ml = run_var(w_low_mid,  n, reps, 0, iters, mismatch);
        vsum_t il = run_var(w_low_inl,  n, reps, 0, iters, mismatch);
        if (hl.result != ml.result || hl.result != il.result) *mismatch = 1;
        printf("\n## Tier boundaries 4: calls into LOW code\n\n"
               "A LOW function sums a 32-word buffer that lives in the GC heap (HIGH->LOW) or in an arena (MID->LOW). "
               "The baseline inlines the same loop, so it has no call boundary. LOW code allocates nothing and calls nothing in "
               "another tier, so no collection can start while it runs. %lld calls, ns per call (min of %d runs).\n\n",
               (long long)reps, iters);
        printf("| variant | ns/call |\n|---|---:|\n");
        printf("| HIGH -> LOW (GC buffer) | %.2f |\n", (double)hl.min_ns / (double)reps);
        printf("| MID -> LOW (arena buffer) | %.2f |\n", (double)ml.min_ns / (double)reps);
        printf("| inlined, no boundary | %.2f |\n", (double)il.min_ns / (double)reps);
        fprintf(stderr, "done: xing-low\n");
    }
}

/* The CPU model, so a result can be tied to the hardware that produced it. Shared CI runners
 * are not uniform: three runs on the x86_64 pool differed by up to 65% in absolute time. */
static void host_cpu(char *out, size_t cap) {
    out[0] = '\0';
    FILE *p = popen("lscpu 2>/dev/null | grep -m1 -i 'model name'", "r");
    if (p) {
        if (fgets(out, (int)cap, p) == NULL) out[0] = '\0';
        pclose(p);
    }
    if (!out[0]) {
        FILE *f = fopen("/proc/cpuinfo", "r");
        char line[256];
        while (f && fgets(line, sizeof line, f)) {
            if (strncmp(line, "model name", 10) == 0 || strncmp(line, "Hardware", 8) == 0) {
                snprintf(out, cap, "%s", line);
                break;
            }
        }
        if (f) fclose(f);
    }
    char *colon = strchr(out, ':');
    if (colon) memmove(out, colon + 1, strlen(colon));
    size_t n = strlen(out);
    while (n && (out[n - 1] == '\n' || out[n - 1] == ' ')) out[--n] = '\0';
    size_t lead = strspn(out, " \t");
    if (lead) memmove(out, out + lead, strlen(out + lead) + 1);
    if (!out[0]) snprintf(out, cap, "unknown");
}

int main(int argc, char **argv) {
    ubel_gc_init(__builtin_frame_address(0));

    int iters = 5;
    double scale = 1.0;
    const char *only = NULL;
    int require_gc = 0;
    for (int i = 1; i < argc; i++) {
        if (!strcmp(argv[i], "--iters") && i + 1 < argc) iters = atoi(argv[++i]);
        else if (!strcmp(argv[i], "--scale") && i + 1 < argc) scale = atof(argv[++i]);
        else if (!strcmp(argv[i], "--only") && i + 1 < argc) only = argv[++i];
        else if (!strcmp(argv[i], "--require-gc")) require_gc = 1;
        else { fprintf(stderr, "usage: %s [--iters N] [--scale X] [--only SUBSTR] [--require-gc]\n", argv[0]); return 1; }
    }
    if (iters < 1) iters = 1;
    if (scale <= 0.0) scale = 1.0;

    bench_t benches[] = {
        { "churn",      "4000 frames x 1000 temp list nodes (32 B), nothing survives a frame",
          4000, 1000, 0,      1, { churn_high, churn_mid, churn_low } },
        { "churn+live", "same churn while a 200k-node (6.4 MB) list stays live the whole run",
          4000, 1000, 200000, 1, { churn_high, churn_mid, churn_low } },
        { "tree",       "build, sum and drop a depth-14 binary tree (32767 nodes), 100 times",
          100,  14,   0,      1, { tree_high_w, tree_mid_w, tree_low_w } },
        { "arith",      "100M LCG steps, no allocation (tier-independent baseline)",
          100000000, 0, 0,    0, { arith_w, NULL, NULL } },
    };
    const int nb = (int)(sizeof benches / sizeof benches[0]);

    summary_t res[8][3];
    int ran[8];
    int mismatch = 0;
    memset(res, 0, sizeof res);
    memset(ran, 0, sizeof ran);

    for (int i = 0; i < nb; i++) {
        const bench_t *bn = &benches[i];
        if (only && !strstr(bn->name, only)) continue;
        int64_t a = (int64_t)((double)bn->a * scale);
        if (a < 1) a = 1;
        ran[i] = 1;
        for (int t = 0; t < (bn->tiered ? 3 : 1); t++)
            res[i][t] = run_tier(bn, a, t, iters, &mismatch);
        if (bn->tiered && (res[i][HIGH].result != res[i][MID].result ||
                           res[i][HIGH].result != res[i][LOW].result))
            mismatch = 1;
        fprintf(stderr, "done: %s\n", bn->name);
    }

    char cpu[256];
    host_cpu(cpu, sizeof cpu);
    printf("Host CPU: %s\n\n", cpu);
    printf("Stage 0 tier benchmark: %d timed run(s) per tier, scale %.3g. "
           "Times are min / median in ms. Ratio = tier time / HIGH time, so below 1.00 is faster than HIGH.\n\n",
           iters, scale);

    printf("| benchmark | HIGH (GC) | MID (arena) | LOW (malloc/free) | MID / HIGH | LOW / HIGH |\n");
    printf("|---|---:|---:|---:|---:|---:|\n");
    for (int i = 0; i < nb; i++) {
        if (!ran[i]) continue;
        const bench_t *bn = &benches[i];
        if (!bn->tiered) {
            printf("| %s | %.2f / %.2f | n/a | n/a | n/a | n/a |\n", bn->name,
                   ms(res[i][0].min_ns), ms(res[i][0].med_ns));
            continue;
        }
        const summary_t *h = &res[i][HIGH], *m = &res[i][MID], *l = &res[i][LOW];
        printf("| %s | %.2f / %.2f | %.2f / %.2f | %.2f / %.2f | %.2f | %.2f |\n", bn->name,
               ms(h->min_ns), ms(h->med_ns), ms(m->min_ns), ms(m->med_ns),
               ms(l->min_ns), ms(l->med_ns),
               (double)m->min_ns / (double)h->min_ns, (double)l->min_ns / (double)h->min_ns);
    }

    printf("\nGC activity in the fastest HIGH run (stop-the-world, so pause = whole collection):\n\n");
    printf("| benchmark | collections | allocations | GC time (ms) | GC share of run | max pause (ms) | peak heap (MiB) | live at last GC (KiB) |\n");
    printf("|---|---:|---:|---:|---:|---:|---:|---:|\n");
    for (int i = 0; i < nb; i++) {
        if (!ran[i] || !benches[i].tiered) continue;
        const summary_t *h = &res[i][HIGH];
        uint64_t gc_ns = h->gc.mark_ns + h->gc.sweep_ns;
        printf("| %s | %llu | %llu | %.2f (mark %.2f, sweep %.2f) | %.1f%% | %.3f | %.1f | %.0f |\n",
               benches[i].name,
               (unsigned long long)h->gc.collections, (unsigned long long)h->gc.alloc_count,
               ms(gc_ns), ms(h->gc.mark_ns), ms(h->gc.sweep_ns),
               h->min_ns ? 100.0 * (double)gc_ns / (double)h->min_ns : 0.0,
               ms(h->gc.max_pause_ns),
               (double)h->gc.peak_heap_bytes / (1024.0 * 1024.0),
               (double)h->gc.live_bytes_last / 1024.0);
    }

    printf("\nWorkloads:\n\n");
    for (int i = 0; i < nb; i++)
        if (ran[i]) printf("- **%s**: %s\n", benches[i].name, benches[i].what);

    /* Stage 1A: tier boundaries. */
    int xing_gc_problem = 0;
    run_boundaries(only, iters, scale, require_gc, &mismatch, &xing_gc_problem);

    /* Was the collector actually exercised, and did it keep live data alive? */
    int gc_problem = xing_gc_problem;
    int idle = 0;
    char idle_names[256] = "";
    for (int i = 0; i < nb; i++) {
        if (!ran[i] || !benches[i].tiered) continue;
        const ubel_gc_stats *g = &res[i][HIGH].gc;
        if (g->collections == 0) {
            idle++;
            if (strlen(idle_names) + strlen(benches[i].name) + 3 < sizeof idle_names) {
                if (idle > 1) strcat(idle_names, ", ");
                strcat(idle_names, benches[i].name);
            }
            if (require_gc) {
                fprintf(stderr, "GC SELF-TEST: %s ran with zero collections\n", benches[i].name);
                gc_problem = 1;
            }
        } else if (require_gc && benches[i].c > 0) {
            uint64_t need = (uint64_t)benches[i].c * 32u * 95u / 100u; /* live list, 32-byte size class */
            if (g->live_bytes_last < need) {
                fprintf(stderr, "GC SELF-TEST: %s kept only %llu live bytes, expected at least %llu\n",
                        benches[i].name, (unsigned long long)g->live_bytes_last, (unsigned long long)need);
                gc_problem = 1;
            }
        }
    }
    if (idle && !require_gc)
        printf("\nNote: HIGH did no collection in: %s. Those HIGH times exclude GC cost; raise --scale (or lower UBEL_GC_TRIGGER_KB) to exercise the collector.\n", idle_names);

    printf("\nChecksums across tiers and runs: %s\n", mismatch ? "MISMATCH" : "all equal");
    if (require_gc) printf("GC self-test: %s\n", (gc_problem || mismatch) ? "FAILED" : "passed");
    if (mismatch) {
        fprintf(stderr, "CHECKSUM MISMATCH between tiers or runs\n");
        return 2;
    }
    if (gc_problem) return 3;
    return 0;
}

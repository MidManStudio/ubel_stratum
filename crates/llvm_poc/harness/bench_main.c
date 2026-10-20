/* crates/llvm_poc/harness/bench_main.c
 *
 * Drives the hand-written IR workloads in bench/tiers.ll, once per tier,
 * checks that every tier computes the same checksum, and prints markdown.
 *
 * Usage: bench [--iters N] [--scale X] [--only SUBSTR] [--require-gc]
 *   --iters  timed runs per benchmark and tier (default 5); min and median are shown
 *   --scale  multiplies the amount of work (default 1.0); use 0.02 for a smoke test
 *   --only   run only benchmarks whose name contains SUBSTR
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

    /* Was the collector actually exercised, and did it keep live data alive? */
    int gc_problem = 0;
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

/* crates/llvm_poc/runtime/ubel_main.c
 *
 * `main` for programs emitted by ubel-llvm (Stage 1B). The emitter produces a function
 * `ubel_main` from the program's `fn main()`. This file starts the runtime, runs it, and
 * (when UBEL_CHECK_LEAKS is set) fails if any arena was left undestroyed, which would mean
 * an exit path skipped its cleanup. With UBEL_STATS set it also prints the collector's
 * statistics to stderr at exit.
 */
#include "ubel_rt.h"

#include <stdio.h>
#include <stdlib.h>

extern void ubel_main(void);

int main(void) {
    ubel_gc_init(__builtin_frame_address(0));
    ubel_main();
    fflush(stdout);
    if (getenv("UBEL_STATS")) { /* read by scripts/run_equiv.sh for the CI report */
        ubel_gc_stats st;
        ubel_gc_get_stats(&st);
        fprintf(stderr,
                "ubel_stats: collections=%llu alloc_count=%llu alloc_bytes=%llu mark_ns=%llu "
                "sweep_ns=%llu max_pause_ns=%llu peak_heap_bytes=%llu live_bytes_last=%llu "
                "large_allocs=%llu large_bytes=%llu\n",
                (unsigned long long)st.collections, (unsigned long long)st.alloc_count,
                (unsigned long long)st.alloc_bytes, (unsigned long long)st.mark_ns,
                (unsigned long long)st.sweep_ns, (unsigned long long)st.max_pause_ns,
                (unsigned long long)st.peak_heap_bytes, (unsigned long long)st.live_bytes_last,
                (unsigned long long)st.large_allocs, (unsigned long long)st.large_bytes);
    }
    if (getenv("UBEL_CHECK_LEAKS") && ubel_arena_live() != 0) {
        fprintf(stderr, "arena leak: %llu arena(s) still live at exit\n",
                (unsigned long long)ubel_arena_live());
        return 3;
    }
    return 0;
}

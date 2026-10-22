/* crates/llvm_poc/runtime/ubel_main.c
 *
 * `main` for programs emitted by ubel-llvm (Stage 1B). The emitter produces a function
 * `ubel_main` from the program's `fn main()`. This file starts the runtime, runs it, and
 * (when UBEL_CHECK_LEAKS is set) fails if any arena was left undestroyed, which would mean
 * an exit path skipped its cleanup.
 */
#include "ubel_rt.h"

#include <stdio.h>
#include <stdlib.h>

extern void ubel_main(void);

int main(void) {
    ubel_gc_init(__builtin_frame_address(0));
    ubel_main();
    fflush(stdout);
    if (getenv("UBEL_CHECK_LEAKS") && ubel_arena_live() != 0) {
        fprintf(stderr, "arena leak: %llu arena(s) still live at exit\n",
                (unsigned long long)ubel_arena_live());
        return 3;
    }
    return 0;
}

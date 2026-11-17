/* Reads the corpus written by scripts/float_diff_gen.rs on stdin (lines "d <hex64> <text>" and "f <hex32> <text>",
 * the text being Rust's `{}`), prints each number with ubel_float.h and reports every difference.
 * Exit status 0: no difference; 1: at least one; 2: bad input. Provisional. */
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include "ubel_float.h"

int main(void) {
    static char line[4096], out[UBEL_FLT_BUF + 16];
    long long total = 0, bad = 0, nd = 0, nf = 0;
    while (fgets(line, sizeof line, stdin)) {
        size_t len = strlen(line);
        while (len && (line[len - 1] == '\n' || line[len - 1] == '\r')) line[--len] = 0;
        if (len < 5 || (line[0] != 'd' && line[0] != 'f') || line[1] != ' ') { fprintf(stderr, "bad line: %s\n", line); return 2; }
        char *end;
        unsigned long long bits = strtoull(line + 2, &end, 16);
        if (*end != ' ') { fprintf(stderr, "bad line: %s\n", line); return 2; }
        const char *want = end + 1;
        int64_t n;
        if (line[0] == 'd') {
            double v; uint64_t b = bits; memcpy(&v, &b, sizeof v);
            n = ubel_f64_format(out, v); nd++;
        } else {
            float v; uint32_t b = (uint32_t)bits; memcpy(&v, &b, sizeof v);
            n = ubel_f32_format(out, v); nf++;
        }
        out[n] = 0;
        total++;
        if (strcmp(out, want) != 0) {
            if (++bad <= 20) fprintf(stderr, "MISMATCH %c %llx\n   rust: %.120s\n   ours: %.120s\n", line[0], bits, want, out);
        }
    }
    printf("float_diff: %lld values (%lld double, %lld float), %lld mismatches\n", total, nd, nf, bad);
    return bad ? 1 : 0;
}

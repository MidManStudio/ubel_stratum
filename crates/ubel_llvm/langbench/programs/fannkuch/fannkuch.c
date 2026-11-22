/* fannkuch-redux: see fannkuch.ubl. Same algorithm. */
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>

int main(void) {
    int64_t n = @N@;
    int64_t *perm1 = malloc(n * sizeof(int64_t)), *perm = malloc(n * sizeof(int64_t)), *count = malloc(n * sizeof(int64_t));
    for (int64_t i = 0; i < n; i++) { perm1[i] = i; perm[i] = 0; count[i] = 0; }
    int64_t max_flips = 0, perm_count = 0, checksum = 0, r = n;
    int running = 1;
    while (running) {
        while (r != 1) { count[r - 1] = r; r -= 1; }
        for (int64_t i = 0; i < n; i++) perm[i] = perm1[i];
        int64_t flips = 0, k = perm[0];
        while (k != 0) {
            int64_t i = 0, j = k;
            while (i < j) { int64_t t = perm[i]; perm[i] = perm[j]; perm[j] = t; i += 1; j -= 1; }
            flips += 1;
            k = perm[0];
        }
        if (flips > max_flips) max_flips = flips;
        if (perm_count % 2 == 0) checksum += flips; else checksum -= flips;
        int more = 1;
        while (more) {
            if (r == n) {
                running = 0;
                more = 0;
            } else {
                int64_t p0 = perm1[0], i = 0;
                while (i < r) { perm1[i] = perm1[i + 1]; i += 1; }
                perm1[r] = p0;
                count[r] -= 1;
                if (count[r] > 0) more = 0; else r += 1;
            }
        }
        perm_count += 1;
    }
    printf("%lld\n%lld\n", (long long)checksum, (long long)max_flips);
    free(perm1); free(perm); free(count);
    return 0;
}
